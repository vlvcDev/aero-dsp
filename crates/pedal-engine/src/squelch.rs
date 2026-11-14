//! Input-floor squelch: ramps the clean input to EXACT digital zero before the
//! NAM models whenever it sits at the interface's noise floor.
//!
//! High-gain captures (and pedal + amp stacks) apply ~60-100 dB of gain to
//! near-silent signals, so even an interface's converter floor (~-110 to
//! -120 dBFS, with every knob at zero) comes out as loud hiss. NAM networks
//! are deterministic: an exactly-zero input produces silence. So instead of
//! attenuating the amp's output by some finite range, this feeds the models
//! true zeros while idle. The threshold sits just above the measured floor,
//! far below any playing, so notes open it instantly and sustain rings until
//! it genuinely reaches the floor.
use std::sync::atomic::{AtomicU32, Ordering};

pub const THRESHOLD_RANGE_DB: (f32, f32) = (-130.0, -60.0);
pub const DEFAULT_THRESHOLD_DB: f32 = -105.0;
/// LEARN puts the open threshold this far above the floor's envelope peak.
pub const LEARN_MARGIN_DB: f32 = 6.0;
/// Closes 3 dB below the open threshold: still above the floor peaks.
const HYSTERESIS_DB: f32 = 3.0;
const HOLD_SECONDS: f32 = 0.050;
const OPEN_SECONDS: f32 = 0.001;
const CLOSE_SECONDS: f32 = 0.020;
const ENVELOPE_DECAY_SECONDS: f32 = 0.025;

/// Loudest detector envelope since reset (f32 bits; non-negative floats
/// order like their bits, so `fetch_max` works). Used by LEARN.
#[derive(Default)]
pub struct FloorMeter(AtomicU32);

impl FloorMeter {
    pub fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn record(&self, envelope: f32) {
        self.0
            .fetch_max(envelope.max(0.0).to_bits(), Ordering::Relaxed);
    }
}

pub fn learned_threshold_db(floor_peak: f32) -> f32 {
    (20.0 * floor_peak.max(1e-9).log10() + LEARN_MARGIN_DB)
        .clamp(THRESHOLD_RANGE_DB.0, THRESHOLD_RANGE_DB.1)
}

pub struct Squelch {
    dc_input: f32,
    dc_output: f32,
    dc_coefficient: f32,
    low_pass: f32,
    low_pass_coefficient: f32,
    envelope: f32,
    envelope_decay: f32,
    open: bool,
    hold_frames: usize,
    hold_remaining: usize,
    gain: f32,
    open_step: f32,
    close_step: f32,
}

impl Squelch {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            dc_input: 0.0,
            dc_output: 0.0,
            // Ignore interface DC offset and rumble below ~20 Hz...
            dc_coefficient: (-std::f32::consts::TAU * 20.0 / rate).exp(),
            low_pass: 0.0,
            // ...and converter noise above the guitar's useful band.
            low_pass_coefficient: 1.0 - (-std::f32::consts::TAU * 5_000.0 / rate).exp(),
            envelope: 0.0,
            envelope_decay: (-1.0 / (ENVELOPE_DECAY_SECONDS * rate)).exp(),
            open: true,
            hold_frames: (HOLD_SECONDS * rate) as usize,
            hold_remaining: 0,
            gain: 1.0,
            open_step: 1.0 / (OPEN_SECONDS * rate),
            close_step: 1.0 / (CLOSE_SECONDS * rate),
        }
    }

    /// In place, allocation-free. While closed the output is exactly 0.0.
    /// Disabled and fully open, it is an exact bypass. The meter always
    /// records the detector so LEARN works whether or not it is enabled.
    pub fn process(
        &mut self,
        enabled: bool,
        threshold_db: f32,
        meter: &FloorMeter,
        samples: &mut [f32],
    ) {
        let threshold_db = threshold_db.clamp(THRESHOLD_RANGE_DB.0, THRESHOLD_RANGE_DB.1);
        let open_at = 10.0_f32.powf(threshold_db / 20.0);
        let close_at = 10.0_f32.powf((threshold_db - HYSTERESIS_DB) / 20.0);
        let mut loudest = 0.0_f32;
        for sample in samples {
            let input = if sample.is_finite() { *sample } else { 0.0 };
            let high_passed = self.dc_coefficient * (self.dc_output + (input - self.dc_input));
            self.dc_input = input;
            self.dc_output = high_passed;
            self.low_pass += self.low_pass_coefficient * (high_passed - self.low_pass);
            self.envelope = self.low_pass.abs().max(self.envelope * self.envelope_decay);
            loudest = loudest.max(self.envelope);

            if self.envelope >= open_at {
                self.open = true;
                self.hold_remaining = self.hold_frames;
            } else if self.open && self.envelope < close_at {
                if self.hold_remaining > 0 {
                    self.hold_remaining -= 1;
                } else {
                    self.open = false;
                }
            }
            let target = if self.open || !enabled { 1.0 } else { 0.0 };
            if target > self.gain {
                self.gain = (self.gain + self.open_step).min(1.0);
            } else if target < self.gain {
                self.gain = (self.gain - self.close_step).max(0.0);
            }
            *sample = if self.gain >= 1.0 {
                *sample
            } else if self.gain <= 0.0 {
                0.0
            } else {
                input * crate::dsp::smooth_ramp(self.gain)
            };
        }
        meter.record(loudest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(seed: &mut u32, rms_db: f32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let amplitude = 10.0_f32.powf(rms_db / 20.0) * 3.0_f32.sqrt();
        ((*seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amplitude
    }

    fn run(squelch: &mut Squelch, enabled: bool, threshold: f32, signal: &mut [f32]) -> FloorMeter {
        let meter = FloorMeter::default();
        for block in signal.chunks_mut(64) {
            squelch.process(enabled, threshold, &meter, block);
        }
        meter
    }

    #[test]
    fn floor_noise_becomes_exact_zero_and_notes_pass_untouched() {
        let mut seed = 1;
        // 0.5 s of -120 dBFS converter floor, a -40 dBFS note, floor again.
        let mut signal: Vec<f32> = (0..72_000)
            .map(|n| {
                let floor = noise(&mut seed, -120.0);
                if (24_000..48_000).contains(&n) {
                    floor + 0.01 * (n as f32 * 0.05).sin()
                } else {
                    floor
                }
            })
            .collect();
        let original = signal.clone();
        let mut squelch = Squelch::new(48_000);
        let meter = run(&mut squelch, true, -105.0, &mut signal);
        assert!(
            signal[12_000..24_000].iter().all(|s| *s == 0.0),
            "idle floor not zeroed"
        );
        // The note passes bit-exact once open (1 ms ramp).
        assert_eq!(signal[24_100..48_000], original[24_100..48_000]);
        // After the note: ~200 ms for the detector to fall 68 dB to the close
        // point, 50 ms hold, 20 ms fade, then exact zeros again.
        assert!(signal[62_000..].iter().all(|s| *s == 0.0));
        // LEARN on this floor lands well below the note and above the floor.
        let floor = run(
            &mut Squelch::new(48_000),
            false,
            -105.0,
            &mut original[..20_000].to_vec(),
        );
        let learned = learned_threshold_db(floor.peak());
        assert!((-118.0..=-100.0).contains(&learned), "{learned}");
        assert!(meter.peak() > 0.005);
    }

    #[test]
    fn a_decaying_note_stays_open_until_it_reaches_the_floor() {
        let mut seed = 2;
        // Exponential decay from -30 dBFS at 40 dB/s, on a -120 dBFS floor.
        let mut signal: Vec<f32> = (0..96_000)
            .map(|n| {
                let level = -30.0 - 40.0 * n as f32 / 48_000.0;
                10.0_f32.powf(level / 20.0) * (n as f32 * 0.03).sin() + noise(&mut seed, -120.0)
            })
            .collect();
        let original = signal.clone();
        run(&mut Squelch::new(48_000), true, -110.0, &mut signal);
        // At 1.5 s the note is at -90 dBFS (inaudible clean, loud after a
        // high-gain amp): still passing untouched.
        assert_eq!(signal[72_000], original[72_000]);
    }

    #[test]
    fn disabled_is_an_exact_bypass() {
        let mut seed = 3;
        let mut signal: Vec<f32> = (0..24_000).map(|_| noise(&mut seed, -120.0)).collect();
        let original = signal.clone();
        run(&mut Squelch::new(48_000), false, -105.0, &mut signal);
        assert_eq!(signal, original);
    }
}
