//! Pre-amp guitar compressor: feed-forward, zero latency.
//!
//! - Detector: 12 dB/oct high-pass at 100 Hz (so low strings, which carry
//!   the most energy, do not over-trigger it) then a short 5 ms RMS average.
//!   The audio path itself stays full-range.
//! - Gain computer: threshold, ratio, 6 dB soft knee (in dB).
//! - One ballistics stage on the gain reduction: ATTACK lets the pick
//!   transient through before clamping (10-30 ms = articulate "snap"; ~1 ms =
//!   flat sustain); RELEASE sets how fast it recovers.
//! - Makeup: automatic (the reduction a typical picked note at
//!   `AUTO_MAKEUP_REFERENCE_DBFS` would get, so engaging it does not change
//!   how hard the amp is driven) plus a manual LEVEL.
//! - MIX: parallel blend with the dry input. No latency, so no comb filter.
use crate::svf::{Svf, SvfCoefficients};

pub const ATTACK_RANGE_MS: (f32, f32) = (0.5, 50.0);
pub const RELEASE_RANGE_MS: (f32, f32) = (30.0, 500.0);
pub const LEVEL_RANGE_DB: (f32, f32) = (-12.0, 24.0);
/// Typical clean-DI pick level the automatic makeup is referenced to.
pub const AUTO_MAKEUP_REFERENCE_DBFS: f32 = -12.0;
const KNEE_DB: f32 = 6.0;
const DETECTOR_HIGH_PASS_HZ: f32 = 100.0;
const DETECTOR_RMS_SECONDS: f32 = 0.005;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub level_db: f32,
    pub auto_makeup: bool,
    pub mix: f32,
}

/// Static gain reduction (dB, >= 0) for a detector level, with a soft knee.
pub fn gain_reduction_db(level_db: f32, threshold_db: f32, ratio: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold_db;
    let half = KNEE_DB * 0.5;
    if over <= -half {
        0.0
    } else if over >= half {
        over * slope
    } else {
        (over + half).powi(2) / (2.0 * KNEE_DB) * slope
    }
}

/// Total makeup gain in dB for the settings (automatic part + LEVEL).
pub fn makeup_db(settings: &CompressorSettings) -> f32 {
    let automatic = if settings.auto_makeup {
        // A sine at the reference peak has an RMS 3 dB lower.
        gain_reduction_db(
            AUTO_MAKEUP_REFERENCE_DBFS - 3.0,
            settings.threshold_db,
            settings.ratio,
        )
    } else {
        0.0
    };
    automatic + settings.level_db.clamp(LEVEL_RANGE_DB.0, LEVEL_RANGE_DB.1)
}

fn ballistic(ms: f32, rate: f32) -> f32 {
    1.0 - (-1.0 / (ms * 0.001 * rate)).exp()
}

pub struct Compressor {
    detector: Svf,
    detector_coefficients: SvfCoefficients,
    mean_square: f32,
    rms_coefficient: f32,
    /// Smoothed gain reduction in dB (>= 0).
    reduction_db: f32,
    /// Smoothed wet amount for click-free enable/disable.
    wet: f32,
    sample_rate: f32,
}

impl Compressor {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            detector: Svf::default(),
            detector_coefficients: SvfCoefficients::new(DETECTOR_HIGH_PASS_HZ, rate),
            mean_square: 0.0,
            rms_coefficient: ballistic(DETECTOR_RMS_SECONDS * 1000.0, rate),
            reduction_db: 0.0,
            wet: 0.0,
            sample_rate: rate,
        }
    }

    /// In place, allocation-free. Returns the block's maximum gain reduction
    /// in dB (for the GR meter). Disabled and settled, it is an exact bypass.
    pub fn process(&mut self, settings: CompressorSettings, samples: &mut [f32]) -> f32 {
        let target_wet = if settings.enabled { 1.0 } else { 0.0 };
        if target_wet == 0.0 && self.wet == 0.0 {
            self.reduction_db = 0.0;
            return 0.0;
        }
        let attack = ballistic(
            settings
                .attack_ms
                .clamp(ATTACK_RANGE_MS.0, ATTACK_RANGE_MS.1),
            self.sample_rate,
        );
        let release = ballistic(
            settings
                .release_ms
                .clamp(RELEASE_RANGE_MS.0, RELEASE_RANGE_MS.1),
            self.sample_rate,
        );
        let makeup = 10.0_f32.powf(makeup_db(&settings) / 20.0);
        let mix = settings.mix.clamp(0.0, 1.0);
        let wet_step = 1.0 / (crate::dsp::TRANSITION_SECONDS * self.sample_rate);
        let mut most = 0.0_f32;
        for sample in samples {
            let input = *sample;
            let (_, high) = self.detector.tick(input, self.detector_coefficients);
            self.mean_square += (high * high - self.mean_square) * self.rms_coefficient;
            let level_db = 10.0 * self.mean_square.max(1e-12).log10();
            let target = gain_reduction_db(level_db, settings.threshold_db, settings.ratio);
            let coefficient = if target > self.reduction_db {
                attack
            } else {
                release
            };
            self.reduction_db += (target - self.reduction_db) * coefficient;
            most = most.max(self.reduction_db);

            let gain = 10.0_f32.powf(-self.reduction_db / 20.0) * makeup;
            let compressed = input * gain;
            let processed = input + (compressed - input) * mix;
            self.wet += (target_wet - self.wet).clamp(-wet_step, wet_step);
            let wet = crate::dsp::smooth_ramp(self.wet);
            *sample = input + (processed - input) * wet;
        }
        most * self.wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn settings() -> CompressorSettings {
        CompressorSettings {
            enabled: true,
            threshold_db: -24.0,
            ratio: 4.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            level_db: 0.0,
            auto_makeup: true,
            mix: 1.0,
        }
    }

    fn sine(hz: f32, peak_db: f32, n: usize) -> f32 {
        10.0_f32.powf(peak_db / 20.0) * (std::f32::consts::TAU * hz * n as f32 / RATE).sin()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0, |p, s| p.max(s.abs()))
    }

    fn run(c: &mut Compressor, s: CompressorSettings, x: &mut [f32]) -> f32 {
        x.chunks_mut(64)
            .map(|b| c.process(s, b))
            .fold(0.0, f32::max)
    }

    #[test]
    fn auto_makeup_keeps_a_typical_pick_at_the_same_level() {
        // Steady 440 Hz at the -12 dBFS reference: compressed, then made up.
        let mut x: Vec<f32> = (0..48_000).map(|n| sine(440.0, -12.0, n)).collect();
        let reduction = run(&mut Compressor::new(48_000), settings(), &mut x);
        assert!(reduction > 6.0, "it should be compressing: {reduction}");
        let out_db = 20.0 * peak(&x[24_000..]).log10();
        assert!((out_db + 12.0).abs() < 0.5, "level moved to {out_db} dBFS");
        // Quieter notes come up, louder ones go down: the range is narrowed.
        let mut quiet: Vec<f32> = (0..48_000).map(|n| sine(440.0, -30.0, n)).collect();
        run(&mut Compressor::new(48_000), settings(), &mut quiet);
        assert!(20.0 * peak(&quiet[24_000..]).log10() > -30.0 + 3.0);
    }

    #[test]
    fn attack_lets_the_pick_transient_through() {
        // A note starting abruptly at -6 dBFS: how much of its start escapes
        // compression depends on ATTACK. Measured 5-10 ms in, once the 5 ms
        // RMS detector has registered the note.
        let onset = |attack_ms: f32| {
            let mut s = settings();
            s.attack_ms = attack_ms;
            s.auto_makeup = false;
            // 100 ms of silence first so the 20 ms enable fade has finished.
            let lead = 4_800;
            let mut x: Vec<f32> = (0..lead + 4_800)
                .map(|n| {
                    if n < lead {
                        0.0
                    } else {
                        sine(440.0, -6.0, n - lead)
                    }
                })
                .collect();
            run(&mut Compressor::new(48_000), s, &mut x);
            peak(&x[lead + 240..lead + 480])
        };
        assert!(
            onset(30.0) > onset(0.5) * 1.5,
            "{} vs {}",
            onset(30.0),
            onset(0.5)
        );
    }

    #[test]
    fn low_notes_do_not_over_trigger_and_mix_blends_dry() {
        // Same peak level: a low B (61.7 Hz) is compressed less than 440 Hz
        // because the detector ignores most of its fundamental.
        let reduction = |hz: f32| {
            let mut x: Vec<f32> = (0..48_000).map(|n| sine(hz, -12.0, n)).collect();
            run(&mut Compressor::new(48_000), settings(), &mut x)
        };
        assert!(reduction(61.7) < reduction(440.0) - 3.0);

        // MIX 0 is the dry signal exactly (once the enable fade settles).
        let mut s = settings();
        s.mix = 0.0;
        let mut x: Vec<f32> = (0..9_600).map(|n| sine(440.0, -6.0, n)).collect();
        let original = x.clone();
        run(&mut Compressor::new(48_000), s, &mut x);
        assert_eq!(x[4_800..], original[4_800..]);
    }

    #[test]
    fn disabled_is_an_exact_bypass_and_toggling_is_click_free() {
        let mut s = settings();
        s.enabled = false;
        let mut x: Vec<f32> = (0..4_800).map(|n| sine(440.0, -6.0, n)).collect();
        let original = x.clone();
        run(&mut Compressor::new(48_000), s, &mut x);
        assert_eq!(x, original);

        let mut c = Compressor::new(48_000);
        let mut previous = 0.0_f32;
        let mut worst = 0.0_f32;
        for block in 0..300 {
            let mut s = settings();
            s.enabled = (block / 30) % 2 == 0;
            s.level_db = 6.0;
            let mut x: Vec<f32> = (0..64)
                .map(|i| sine(220.0, -12.0, block * 64 + i))
                .collect();
            c.process(s, &mut x);
            for v in x {
                worst = worst.max((v - previous).abs());
                previous = v;
            }
        }
        // A 220 Hz sine at up to ~-6 dBFS moves at most ~0.015 per sample.
        assert!(worst < 0.03, "{worst}");
    }
}
