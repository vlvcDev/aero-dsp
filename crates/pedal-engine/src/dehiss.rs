//! High-band downward expander ("dehiss") for amp/cab output.
//!
//! A broadband gate has to choose between cutting a decaying note short and
//! letting hiss through. Hiss from a high-gain amp lives mostly above a few
//! kHz, while a decaying note's body is lower. So this splits the signal at
//! `frequency_hz` with a complementary crossover (high = input - low, so the
//! bands sum back to the exact input) and only turns the HIGH band down when
//! its level falls toward the hiss floor: notes keep their low/mid body as
//! they ring out, and the hiss above them fades.
//!
//! Gain law: 1:3 downward expansion below the threshold, i.e. the high band
//! drops 2 dB per dB under it (gain = (envelope / threshold)^2), floored at
//! `range_db`. Opens in ~2 ms so pick attacks and pinch harmonics come
//! through; closes over ~80 ms so it does not pump. Stereo channels share one
//! detector and gain so the image never shifts.
//!
//! Detection runs on the rig output before the gate is applied, so LEARN can
//! measure the amp's real hiss floor even while the gate is closed.
use crate::svf::{Svf, SvfCoefficients};
use std::sync::atomic::{AtomicU32, Ordering};

const ENVELOPE_ATTACK_SECONDS: f32 = 0.001;
const ENVELOPE_RELEASE_SECONDS: f32 = 0.030;
const GAIN_OPEN_SECONDS: f32 = 0.002;
const GAIN_CLOSE_SECONDS: f32 = 0.080;
/// LEARN places the threshold this far above the loudest hiss envelope it
/// measured. The detector rides near the hiss peaks, so a smaller margin
/// leaves idle hiss only a few dB under the threshold and, at 2 dB of cut per
/// dB, well short of the full range; 10 dB puts it deep enough to reach it.
pub const LEARN_MARGIN_DB: f32 = 10.0;

pub const THRESHOLD_RANGE_DB: (f32, f32) = (-100.0, -30.0);
pub const FREQUENCY_RANGE_HZ: (f32, f32) = (1_500.0, 8_000.0);
pub const RANGE_RANGE_DB: (f32, f32) = (0.0, 40.0);

/// Loudest high-band envelope seen since the last reset, as f32 bits
/// (non-negative floats order like their bits, so `fetch_max` works).
#[derive(Default)]
pub struct HissMeter(AtomicU32);

impl HissMeter {
    pub fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn record(&self, envelope: f32) {
        self.0
            .fetch_max(envelope.max(0.0).to_bits(), Ordering::Relaxed);
    }
    /// Threshold for a measured hiss envelope peak, clamped to the range.
    pub fn learned_threshold_db(peak: f32) -> f32 {
        (20.0 * peak.max(1e-7).log10() + LEARN_MARGIN_DB)
            .clamp(THRESHOLD_RANGE_DB.0, THRESHOLD_RANGE_DB.1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DehissSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub frequency_hz: f32,
    pub range_db: f32,
    /// Keep the detector (and LEARN meter) running even while disabled.
    pub measure: bool,
}

pub struct Dehiss {
    split: [Svf; 2],
    envelope: f32,
    gain: f32,
    attack: f32,
    release: f32,
    open: f32,
    close: f32,
    sample_rate: f32,
}

impl Dehiss {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let coefficient = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        Self {
            split: [Svf::default(); 2],
            envelope: 0.0,
            gain: 1.0,
            attack: coefficient(ENVELOPE_ATTACK_SECONDS),
            release: coefficient(ENVELOPE_RELEASE_SECONDS),
            open: coefficient(GAIN_OPEN_SECONDS),
            close: coefficient(GAIN_CLOSE_SECONDS),
            sample_rate: rate,
        }
    }

    /// Current high-band gain (1 = untouched).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Processes one channel (`right == None`) or a linked stereo pair.
    /// Allocation-free. When disabled and fully open it is an exact bypass.
    pub fn process(
        &mut self,
        settings: DehissSettings,
        meter: &HissMeter,
        left: &mut [f32],
        mut right: Option<&mut [f32]>,
    ) {
        if !settings.enabled && !settings.measure && self.gain >= 1.0 {
            self.split = [Svf::default(); 2];
            self.envelope = 0.0;
            return;
        }
        let frequency = settings
            .frequency_hz
            .clamp(FREQUENCY_RANGE_HZ.0, FREQUENCY_RANGE_HZ.1);
        let coefficients = SvfCoefficients::new(frequency, self.sample_rate);
        let threshold = 10.0_f32.powf(
            settings
                .threshold_db
                .clamp(THRESHOLD_RANGE_DB.0, THRESHOLD_RANGE_DB.1)
                / 20.0,
        );
        let floor =
            10.0_f32.powf(-settings.range_db.clamp(RANGE_RANGE_DB.0, RANGE_RANGE_DB.1) / 20.0);
        let mut loudest = 0.0_f32;
        for index in 0..left.len() {
            let x_left = left[index];
            let high_left = x_left - self.split[0].tick(x_left, coefficients).0;
            let mut level = high_left.abs();
            let mut high_right = 0.0;
            if let Some(right) = right.as_deref() {
                let x_right = right[index];
                high_right = x_right - self.split[1].tick(x_right, coefficients).0;
                level = level.max(high_right.abs());
            }
            let coefficient = if level > self.envelope {
                self.attack
            } else {
                self.release
            };
            self.envelope += (level - self.envelope) * coefficient;
            loudest = loudest.max(self.envelope);
            let target = if settings.enabled {
                let ratio = self.envelope / threshold;
                (ratio * ratio).clamp(floor, 1.0)
            } else {
                1.0
            };
            let smoothing = if target > self.gain {
                self.open
            } else {
                self.close
            };
            self.gain += (target - self.gain) * smoothing;
            if target == 1.0 && self.gain > 0.99999 {
                self.gain = 1.0;
            }
            // out = low + gain * high = input - (1 - gain) * high: exact at 1.
            let cut = 1.0 - self.gain;
            left[index] = x_left - cut * high_left;
            if let Some(right) = right.as_deref_mut() {
                right[index] -= cut * high_right;
            }
        }
        meter.record(loudest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn settings(threshold_db: f32) -> DehissSettings {
        DehissSettings {
            enabled: true,
            threshold_db,
            frequency_hz: 3_000.0,
            range_db: 24.0,
            measure: false,
        }
    }

    fn noise(seed: &mut u32, amplitude: f32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 2.0 * amplitude
    }

    fn sine(hz: f32, n: usize) -> f32 {
        (std::f32::consts::TAU * hz * n as f32 / RATE).sin()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn run(dehiss: &mut Dehiss, settings: DehissSettings, signal: &mut [f32]) {
        let meter = HissMeter::default();
        for block in signal.chunks_mut(64) {
            dehiss.process(settings, &meter, block, None);
        }
    }

    #[test]
    fn disabled_is_an_exact_bypass() {
        let mut dehiss = Dehiss::new(48_000);
        let mut seed = 7;
        let original: Vec<f32> = (0..4_800).map(|_| noise(&mut seed, 0.3)).collect();
        let mut signal = original.clone();
        let mut off = settings(-60.0);
        off.enabled = false;
        run(&mut dehiss, off, &mut signal);
        assert_eq!(signal, original);
    }

    #[test]
    fn idle_hiss_is_reduced_by_the_range_but_low_end_is_untouched() {
        // Idle white hiss alone: everything above the crossover is cut.
        let mut seed = 1;
        let hiss: Vec<f32> = (0..48_000).map(|_| noise(&mut seed, 0.001)).collect();
        let mut processed = hiss.clone();
        let mut dehiss = Dehiss::new(48_000);
        run(&mut dehiss, settings(-40.0), &mut processed);
        let tail = 24_000..48_000;
        let overall = 20.0 * (rms(&processed[tail.clone()]) / rms(&hiss[tail.clone()])).log10();
        // White noise keeps ~1/8 of its power below 3 kHz by design (-9 dB).
        assert!(overall < -7.0, "overall hiss only reduced {overall} dB");
        // High-emphasis view (first difference ~ +6 dB/oct): the audible
        // fizz band drops by close to the full 24 dB range.
        let bright = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        let fizz = 20.0 * (bright(&processed[tail.clone()]) / bright(&hiss[tail.clone()])).log10();
        assert!(fizz < -15.0, "fizz band only reduced {fizz} dB");

        // Low content riding on the hiss keeps its level (the low band only
        // takes the crossover's small phase shift while the highs are cut).
        let mut seed = 2;
        let original: Vec<f32> = (0..48_000)
            .map(|n| noise(&mut seed, 0.001) + 0.01 * sine(110.0, n))
            .collect();
        let mut processed = original.clone();
        let mut dehiss = Dehiss::new(48_000);
        run(&mut dehiss, settings(-40.0), &mut processed);
        let level_110 = |x: &[f32]| {
            let (mut re, mut im) = (0.0, 0.0);
            for n in tail.clone() {
                let phase = std::f32::consts::TAU * 110.0 * n as f32 / RATE;
                re += x[n] * phase.cos();
                im += x[n] * phase.sin();
            }
            (re * re + im * im).sqrt()
        };
        let change = 20.0 * (level_110(&processed) / level_110(&original)).log10();
        assert!(change.abs() < 0.1, "110 Hz level changed {change} dB");
    }

    #[test]
    fn a_playing_note_with_highs_passes_and_its_decay_keeps_its_body() {
        // Bright note: 220 Hz body plus 4 kHz harmonics, decaying, on hiss.
        let mut seed = 3;
        let signal: Vec<f32> = (0..96_000)
            .map(|n| {
                let decay = (-(n as f32) / 24_000.0).exp();
                decay * (0.3 * sine(220.0, n) + 0.1 * sine(4_000.0, n)) + noise(&mut seed, 0.001)
            })
            .collect();
        let mut processed = signal.clone();
        let mut dehiss = Dehiss::new(48_000);
        run(&mut dehiss, settings(-40.0), &mut processed);
        // While loud (first 100 ms after onset), output equals input closely.
        let early = 480..4_800;
        let error = rms(&early
            .clone()
            .map(|n| processed[n] - signal[n])
            .collect::<Vec<_>>());
        assert!(error < 0.002, "playing note altered: {error}");
        // Late in the decay the 220 Hz body is still there.
        let late = 72_000..96_000;
        let body = |x: &[f32]| {
            let (mut re, mut im) = (0.0, 0.0);
            for n in late.clone() {
                let phase = std::f32::consts::TAU * 220.0 * n as f32 / RATE;
                re += x[n] * phase.cos();
                im += x[n] * phase.sin();
            }
            (re * re + im * im).sqrt()
        };
        let change = 20.0 * (body(&processed) / body(&signal)).log10();
        assert!(change.abs() < 0.2, "decaying body changed {change} dB");
    }

    #[test]
    fn linked_stereo_applies_one_gain_to_both_channels() {
        let mut seed = 5;
        let mut left: Vec<f32> = (0..9_600).map(|_| noise(&mut seed, 0.001)).collect();
        let mut right = left.iter().map(|x| x * 0.5).collect::<Vec<_>>();
        let original_left = left.clone();
        let meter = HissMeter::default();
        let mut dehiss = Dehiss::new(48_000);
        for (l, r) in left.chunks_mut(64).zip(right.chunks_mut(64)) {
            dehiss.process(settings(-40.0), &meter, l, Some(r));
        }
        // Identical processing up to the 0.5 scale: the image is preserved.
        for (n, (l, r)) in left.iter().zip(&right).enumerate().skip(100) {
            assert!((l * 0.5 - r).abs() < 1e-6, "{n}");
        }
        assert!(rms(&left[4_800..]) < rms(&original_left[4_800..]));
    }

    #[test]
    fn learn_measures_the_hiss_peak_and_sets_a_threshold_above_it() {
        let meter = HissMeter::default();
        let mut dehiss = Dehiss::new(48_000);
        let mut seed = 9;
        let mut hiss: Vec<f32> = (0..48_000).map(|_| noise(&mut seed, 0.002)).collect();
        // LEARN runs with dehiss still switched off: the detector measures
        // while the audio passes through untouched.
        let original = hiss.clone();
        let mut probe = settings(-60.0);
        probe.enabled = false;
        probe.measure = true;
        for block in hiss.chunks_mut(64) {
            dehiss.process(probe, &meter, block, None);
        }
        assert_eq!(hiss, original);
        let peak = meter.peak();
        assert!(peak > 0.0005 && peak < 0.002, "{peak}");
        let threshold = HissMeter::learned_threshold_db(peak);
        assert!((threshold - (20.0 * peak.log10() + LEARN_MARGIN_DB)).abs() < 1e-4);
        meter.reset();
        assert_eq!(meter.peak(), 0.0);
        assert_eq!(HissMeter::learned_threshold_db(0.0), THRESHOLD_RANGE_DB.0);
        assert_eq!(HissMeter::learned_threshold_db(1.0), THRESHOLD_RANGE_DB.1);
    }
}
