//! DE-MUD: a dynamic low-mid EQ band after the amp/cab.
//!
//! Palm mutes and low chords dump energy into ~150-400 Hz, where it masks
//! note definition ("notes blend together"). A static cut thins every note;
//! this one only cuts while that band is actually booming. The band is a
//! 12 dB/oct state-variable band-pass whose peak gain is unity, so
//! `out = in - cut * band` is a bell cut of depth `cut` at the centre and
//! exactly the input when `cut == 0`.
//!
//! Detection is RELATIVE: the band's envelope is compared with the envelope
//! of everything else (`in - band`). An absolute threshold depended on how
//! loud each capture runs before the output stage, so the same setting did
//! nothing on one rig and everything on another. Relative, the threshold
//! means "how far the low-mids may stand out" and holds across captures and
//! levels. Measured on real high-gain riffs at 250 Hz, band-vs-rest runs
//! about -5 dB on bright notes to +4 dB on the heaviest chugs; the -4 dB
//! default cuts ~2 dB on typical playing and the full range on chugs.
//! Above the threshold the cut deepens 2 dB per 3 dB over (3:1), up to RANGE.
//! Below an absolute floor (-60 dBFS) it never cuts, so idle noise and
//! decaying tails are left alone. Fast attack (5 ms) catches the chug; 100 ms
//! release lets the low end back between chugs. Stereo channels share one
//! detector and cut.
use crate::svf::{Svf, SvfCoefficients};

/// Band level relative to the rest of the signal.
pub const THRESHOLD_RANGE_DB: (f32, f32) = (-12.0, 6.0);
pub const DEFAULT_THRESHOLD_DB: f32 = -4.0;
pub const FREQUENCY_RANGE_HZ: (f32, f32) = (120.0, 500.0);
pub const RANGE_RANGE_DB: (f32, f32) = (0.0, 12.0);
const RATIO: f32 = 3.0;
/// No cutting while the whole signal is below this (idle noise, tails).
const FLOOR_DB: f32 = -60.0;
/// Cut ballistics: the audible timing lives here, in one stage.
const ATTACK_SECONDS: f32 = 0.005;
const RELEASE_SECONDS: f32 = 0.100;
/// Detector peak followers (band and rest alike, so their ratio is fair):
/// smooth enough to ride over a 120 Hz waveform without rippling the cut.
const DETECTOR_ATTACK_SECONDS: f32 = 0.005;
const DETECTOR_RELEASE_SECONDS: f32 = 0.050;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemudSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub frequency_hz: f32,
    pub range_db: f32,
}

/// Bell bandwidth of the band filter (Q = 1.2: about 1.2 octaves wide).
fn band_coefficients(frequency: f32, rate: f32) -> SvfCoefficients {
    SvfCoefficients::with_q(frequency, rate, 1.2)
}

pub struct Demud {
    band: [Svf; 2],
    band_envelope: f32,
    rest_envelope: f32,
    /// Current cut in dB (>= 0).
    cut_db: f32,
    attack: f32,
    release: f32,
    detector_attack: f32,
    detector_release: f32,
    sample_rate: f32,
}

impl Demud {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let coefficient = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        Self {
            band: [Svf::default(); 2],
            band_envelope: 0.0,
            rest_envelope: 0.0,
            cut_db: 0.0,
            attack: coefficient(ATTACK_SECONDS),
            release: coefficient(RELEASE_SECONDS),
            detector_attack: coefficient(DETECTOR_ATTACK_SECONDS),
            detector_release: coefficient(DETECTOR_RELEASE_SECONDS),
            sample_rate: rate,
        }
    }

    /// In place, allocation-free. Returns the block's deepest cut in dB (GR
    /// meter). Disabled and released, it is an exact bypass.
    pub fn process(
        &mut self,
        settings: DemudSettings,
        left: &mut [f32],
        mut right: Option<&mut [f32]>,
    ) -> f32 {
        if !settings.enabled && self.cut_db == 0.0 {
            self.band = [Svf::default(); 2];
            self.band_envelope = 0.0;
            self.rest_envelope = 0.0;
            return 0.0;
        }
        let coefficients = band_coefficients(
            settings
                .frequency_hz
                .clamp(FREQUENCY_RANGE_HZ.0, FREQUENCY_RANGE_HZ.1),
            self.sample_rate,
        );
        let threshold_db = settings
            .threshold_db
            .clamp(THRESHOLD_RANGE_DB.0, THRESHOLD_RANGE_DB.1);
        let range_db = settings.range_db.clamp(RANGE_RANGE_DB.0, RANGE_RANGE_DB.1);
        let floor = 10.0_f32.powf(FLOOR_DB / 20.0);
        let mut deepest = 0.0_f32;
        for index in 0..left.len() {
            let band_left = self.band[0].band_pass(left[index], coefficients);
            let mut band_level = band_left.abs();
            let mut rest_level = (left[index] - band_left).abs();
            let mut band_right = 0.0;
            if let Some(right) = right.as_deref() {
                band_right = self.band[1].band_pass(right[index], coefficients);
                band_level = band_level.max(band_right.abs());
                rest_level = rest_level.max((right[index] - band_right).abs());
            }
            let (attack, release) = (self.detector_attack, self.detector_release);
            for (envelope, level) in [
                (&mut self.band_envelope, band_level),
                (&mut self.rest_envelope, rest_level),
            ] {
                let coefficient = if level > *envelope { attack } else { release };
                *envelope += (level - *envelope) * coefficient;
            }
            let loud = self.band_envelope.max(self.rest_envelope) > floor;
            let over = 20.0
                * (self.band_envelope.max(1e-9) / self.rest_envelope.max(1e-9)).log10()
                - threshold_db;
            let target = if settings.enabled && loud && over > 0.0 {
                (over * (1.0 - 1.0 / RATIO)).min(range_db)
            } else {
                0.0
            };
            let smoothing = if target > self.cut_db {
                self.attack
            } else {
                self.release
            };
            self.cut_db += (target - self.cut_db) * smoothing;
            if target == 0.0 && self.cut_db < 1e-4 {
                self.cut_db = 0.0;
            }
            deepest = deepest.max(self.cut_db);
            // Unity-peak band: subtracting (1 - g) of it is a bell of gain g.
            let amount = 1.0 - 10.0_f32.powf(-self.cut_db / 20.0);
            left[index] -= amount * band_left;
            if let Some(right) = right.as_deref_mut() {
                right[index] -= amount * band_right;
            }
        }
        deepest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn settings() -> DemudSettings {
        DemudSettings {
            enabled: true,
            threshold_db: DEFAULT_THRESHOLD_DB,
            frequency_hz: 250.0,
            range_db: 6.0,
        }
    }

    fn sine(hz: f32, amplitude: f32, n: usize) -> f32 {
        amplitude * (std::f32::consts::TAU * hz * n as f32 / RATE).sin()
    }

    /// Amplitude of `hz` in `x[range]` (single-bin DFT).
    fn level(x: &[f32], hz: f32, range: std::ops::Range<usize>) -> f32 {
        let (mut re, mut im) = (0.0, 0.0);
        for n in range.clone() {
            let phase = std::f32::consts::TAU * hz * n as f32 / RATE;
            re += x[n] * phase.cos();
            im += x[n] * phase.sin();
        }
        2.0 * (re * re + im * im).sqrt() / range.len() as f32
    }

    fn run(demud: &mut Demud, s: DemudSettings, x: &mut [f32]) -> f32 {
        x.chunks_mut(64)
            .map(|block| demud.process(s, block, None))
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_booming_chug_is_cut_in_the_low_mids_only() {
        // Loud 250 Hz "boom" plus 2 kHz pick/harmonic content.
        let mut x: Vec<f32> = (0..24_000)
            .map(|n| sine(250.0, 0.3, n) + sine(2_000.0, 0.1, n))
            .collect();
        let original = x.clone();
        let deepest = run(&mut Demud::new(48_000), settings(), &mut x);
        assert!((deepest - 6.0).abs() < 0.1, "range-limited cut: {deepest}");
        let tail = 12_000..24_000;
        let low =
            20.0 * (level(&x, 250.0, tail.clone()) / level(&original, 250.0, tail.clone())).log10();
        let high = 20.0
            * (level(&x, 2_000.0, tail.clone()) / level(&original, 2_000.0, tail.clone())).log10();
        assert!((low + 6.0).abs() < 0.3, "250 Hz cut {low} dB");
        assert!(high.abs() < 0.3, "2 kHz changed {high} dB");
    }

    #[test]
    fn bright_playing_idle_noise_and_disabled_are_exact_passthrough() {
        // Low-mids well under the rest of the signal: never cuts.
        let mut x: Vec<f32> = (0..9_600)
            .map(|n| sine(250.0, 0.05, n) + sine(2_000.0, 0.3, n))
            .collect();
        let original = x.clone();
        assert_eq!(run(&mut Demud::new(48_000), settings(), &mut x), 0.0);
        assert_eq!(x, original);
        // All low-mids but below the -60 dBFS floor (a decaying tail).
        let mut x: Vec<f32> = (0..9_600).map(|n| sine(250.0, 0.0005, n)).collect();
        let original = x.clone();
        assert_eq!(run(&mut Demud::new(48_000), settings(), &mut x), 0.0);
        assert_eq!(x, original);
        let mut loud: Vec<f32> = (0..9_600).map(|n| sine(250.0, 0.5, n)).collect();
        let original = loud.clone();
        let mut off = settings();
        off.enabled = false;
        run(&mut Demud::new(48_000), off, &mut loud);
        assert_eq!(loud, original);
    }

    #[test]
    fn the_same_setting_works_at_any_level() {
        // A booming chug 30 dB quieter is cut just as deeply.
        for gain in [1.0, 0.03] {
            let mut x: Vec<f32> = (0..24_000)
                .map(|n| gain * (sine(250.0, 0.3, n) + sine(2_000.0, 0.1, n)))
                .collect();
            let deepest = run(&mut Demud::new(48_000), settings(), &mut x);
            assert!((deepest - 6.0).abs() < 0.1, "gain {gain}: {deepest}");
        }
    }

    #[test]
    fn the_low_end_returns_between_chugs_and_stereo_is_linked() {
        // 120 ms chug, then a bright held note: the cut releases.
        let mut x: Vec<f32> = (0..24_000)
            .map(|n| {
                if n < 5_760 {
                    sine(250.0, 0.4, n)
                } else {
                    sine(250.0, 0.01, n) + sine(1_500.0, 0.2, n)
                }
            })
            .collect();
        let mut demud = Demud::new(48_000);
        run(&mut demud, settings(), &mut x);
        // ~380 ms after a 6 dB cut, the 100 ms release has it under 0.5 dB.
        assert!(demud.cut_db < 0.5, "still cutting {} dB", demud.cut_db);

        let mut left: Vec<f32> = (0..9_600).map(|n| sine(250.0, 0.4, n)).collect();
        let mut right: Vec<f32> = left.iter().map(|v| v * 0.5).collect();
        let mut demud = Demud::new(48_000);
        for (l, r) in left.chunks_mut(64).zip(right.chunks_mut(64)) {
            demud.process(settings(), l, Some(r));
        }
        for (l, r) in left.iter().zip(&right) {
            assert!((l * 0.5 - r).abs() < 1e-6);
        }
    }
}
