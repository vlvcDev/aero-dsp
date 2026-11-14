//! Clean-input detector; its gain is applied after NAM, before spatial effects.
//! No buffering, allocation, or locks in the sample path. The detector filters
//! are never applied to the audible guitar signal.

/// Default distance between the open and close thresholds. High-gain amps keep
/// a decaying note loud long after the clean signal has dropped, so the gate
/// must stay open well below the level that opened it.
pub const DEFAULT_HYSTERESIS_DB: f32 = 12.0;
pub const MIN_THRESHOLD_DB: f32 = -120.0;
/// Detector high-pass corner. Second order: attenuates 50/60 Hz hum by
/// roughly 6-9 dB so hum cannot hold the gate open, while low notes still
/// trigger through their harmonics (as in Tone3000's gate detector).
const DETECTOR_HIGH_PASS_HZ: f32 = 80.0;
/// Peak-envelope fall time. Long enough that a low note does not ripple
/// across the close threshold between waveform peaks.
const ENVELOPE_FALL_SECONDS: f32 = 0.025;

/// Second-order Butterworth high-pass (RBJ), transposed direct form II.
struct DetectorHighPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl DetectorHighPass {
    fn new(rate: f32, frequency: f32) -> Self {
        let omega = std::f32::consts::TAU * (frequency / rate).min(0.49);
        let alpha = omega.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let cosine = omega.cos();
        let a0 = 1.0 + alpha;
        let b0 = (1.0 + cosine) * 0.5 / a0;
        Self {
            b0,
            b1: -2.0 * b0,
            b2: b0,
            a1: -2.0 * cosine / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn tick(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * output + self.z2;
        self.z2 = self.b2 * input - self.a2 * output;
        if !output.is_finite() {
            self.z1 = 0.0;
            self.z2 = 0.0;
            return 0.0;
        }
        output
    }
}

pub(crate) struct Gate {
    rate: f32,
    high_pass: DetectorHighPass,
    low_pass_memory: f32,
    low_pass_coefficient: f32,
    envelope: f32,
    envelope_decay: f32,
    gain: f32,
    attack: f32,
    release: f32,
    threshold: f32,
    close_threshold: f32,
    floor: f32,
    hold_frames: usize,
    remaining: usize,
    open: bool,
    enabled: bool,
}

fn bounded(value: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}

impl Gate {
    pub(crate) fn new(rate: u32, enabled: bool) -> Self {
        let rate = rate.max(1) as f32;
        Self {
            rate,
            // 80 Hz (12 dB/oct) / 5 kHz detector band: reject DC, rumble and
            // mains hum, and reduce sensitivity to high-frequency hiss.
            high_pass: DetectorHighPass::new(rate, DETECTOR_HIGH_PASS_HZ),
            low_pass_memory: 0.0,
            low_pass_coefficient: 1.0 - (-std::f32::consts::TAU * 5000.0 / rate).exp(),
            envelope: 0.0,
            envelope_decay: (-1.0 / (ENVELOPE_FALL_SECONDS * rate)).exp(),
            gain: if enabled { 0.0001 } else { 1.0 },
            attack: 1.0 - (-1.0 / (0.0005 * rate)).exp(),
            release: 1.0 - (-1.0 / (0.050 * rate)).exp(),
            threshold: 10.0_f32.powf(-80.0 / 20.0),
            close_threshold: 10.0_f32.powf((-80.0 - DEFAULT_HYSTERESIS_DB) / 20.0),
            floor: 0.0001,
            hold_frames: (0.020 * rate) as usize,
            remaining: 0,
            open: false,
            enabled,
        }
    }

    pub(crate) fn configure(
        &mut self,
        enabled: bool,
        threshold_db: f32,
        hysteresis_db: f32,
        hold_ms: f32,
        release_ms: f32,
        range_db: f32,
        extra_hold: usize,
    ) {
        self.enabled = enabled;
        // Down to -120 dBFS: after a high-gain capture (60-100 dB of gain on
        // small signals) a note fading through -90 dBFS clean is still loud,
        // so a -80 dBFS floor cut audible sustain.
        let threshold_db = bounded(threshold_db, -80.0, MIN_THRESHOLD_DB, -10.0);
        let hysteresis_db = bounded(hysteresis_db, DEFAULT_HYSTERESIS_DB, 0.0, 30.0);
        self.threshold = 10.0_f32.powf(threshold_db / 20.0);
        // Opens at the threshold; once open, stays open until the clean
        // signal falls `hysteresis_db` below it.
        self.close_threshold = 10.0_f32.powf((threshold_db - hysteresis_db) / 20.0);
        self.floor = 10.0_f32.powf(-bounded(range_db, 80.0, 0.0, 90.0) / 20.0);
        self.release =
            1.0 - (-1.0 / (bounded(release_ms, 50.0, 5.0, 1000.0) * 0.001 * self.rate)).exp();
        self.hold_frames =
            (bounded(hold_ms, 20.0, 0.0, 500.0) * 0.001 * self.rate) as usize + extra_hold;
        self.remaining = self.remaining.min(self.hold_frames);
    }

    /// Detector envelope (clean-input level as the gate sees it).
    pub(crate) fn envelope(&self) -> f32 {
        self.envelope
    }

    pub(crate) fn tick(&mut self, input: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        let high_pass = self.high_pass.tick(input);
        self.low_pass_memory += self.low_pass_coefficient * (high_pass - self.low_pass_memory);
        self.envelope = self
            .low_pass_memory
            .abs()
            .max(self.envelope * self.envelope_decay);
        let close_threshold = self.close_threshold;
        if self.envelope >= self.threshold {
            self.open = true;
        }
        if self.open {
            if self.envelope >= close_threshold {
                // Hold must be consecutive time below the close threshold.
                self.remaining = self.hold_frames;
            } else if self.remaining > 0 {
                self.remaining -= 1;
            } else {
                self.open = false;
            }
        }
        let target = if !self.enabled || self.open {
            1.0
        } else {
            // 4:1 downward expansion below the OPEN threshold: output
            // amplitude falls as input^4, with a user-selected attenuation
            // floor instead of a hard mute. Measuring from the close threshold
            // would pass anything between the two thresholds at full gain, so
            // a closed gate would never really be closed there. The jump at
            // the moment of closing is smoothed by the release time.
            let ratio = (self.envelope / self.threshold).min(1.0);
            (ratio * ratio * ratio).max(self.floor)
        };
        let coefficient = if target > self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain += coefficient * (target - self.gain);
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured(hold: f32, release: f32, range: f32) -> Gate {
        let mut gate = Gate::new(48_000, true);
        gate.configure(true, -30.0, DEFAULT_HYSTERESIS_DB, hold, release, range, 0);
        gate
    }
    fn excite(gate: &mut Gate) {
        for i in 0..4800 {
            gate.tick(0.2 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin());
        }
        assert!(gate.gain > 0.99);
    }

    #[test]
    fn range_is_an_attenuation_floor_and_zero_range_is_unity() {
        for range in [0.0, 20.0, 40.0, 80.0, 90.0] {
            let mut gate = configured(0.0, 5.0, range);
            for _ in 0..48_000 {
                gate.tick(0.0);
            }
            let expected = 10.0_f32.powf(-range / 20.0);
            assert!((gate.gain - expected).abs() < 0.0001);
        }
    }

    #[test]
    fn longer_hold_and_release_preserve_more_tail() {
        let mut tight = configured(0.0, 5.0, 80.0);
        let mut hold = configured(200.0, 5.0, 80.0);
        let mut release = configured(0.0, 400.0, 80.0);
        for gate in [&mut tight, &mut hold, &mut release] {
            excite(gate);
        }
        // 200 ms: the envelope needs ~80 ms to fall 28 dB to the close level.
        for _ in 0..9600 {
            tight.tick(0.0);
            hold.tick(0.0);
            release.tick(0.0);
        }
        assert!(tight.gain < 0.01);
        assert!(hold.gain > 0.99);
        assert!(release.gain > 0.7);
    }

    #[test]
    fn detector_rejects_dc_and_tracks_low_guitar_notes() {
        let mut gate = configured(20.0, 50.0, 80.0);
        for _ in 0..48_000 {
            gate.tick(0.1);
        }
        assert!(gate.gain < 0.001);
        for frequency in [61.74, 82.41, 329.63] {
            for i in 0..4800 {
                gate.tick(0.1 * (std::f32::consts::TAU * frequency * i as f32 / 48_000.0).sin());
            }
            assert!(gate.gain > 0.99);
        }
    }

    #[test]
    fn bypass_and_parameter_changes_are_smoothed_and_nonfinite_is_contained() {
        let mut gate = configured(20.0, 50.0, 80.0);
        excite(&mut gate);
        gate.configure(
            true,
            f32::NAN,
            f32::NAN,
            f32::INFINITY,
            f32::NAN,
            f32::NAN,
            0,
        );
        for _ in 0..48_000 {
            assert!(gate.tick(f32::NAN).is_finite());
        }
        let before = gate.gain;
        gate.configure(false, -30.0, 12.0, 20.0, 50.0, 80.0, 0);
        assert!(gate.tick(0.0) - before < 0.05);
        for _ in 0..4800 {
            gate.tick(0.0);
        }
        assert!(gate.gain > 0.999);
    }

    fn sine(frequency: f32, amplitude: f32, i: usize) -> f32 {
        amplitude * (std::f32::consts::TAU * frequency * i as f32 / 48_000.0).sin()
    }

    /// Seconds a slowly decaying note keeps the gate (nearly) fully open.
    fn sustain_seconds(hysteresis_db: f32) -> f32 {
        let mut gate = Gate::new(48_000, true);
        gate.configure(true, -40.0, hysteresis_db, 20.0, 50.0, 80.0, 0);
        // Pick attack at -14 dB, then a 0.5 s exponential decay (-17.4 dB/s).
        for i in 0..(48_000 * 4) {
            let amplitude = 0.2 * (-(i as f32) / 24_000.0).exp();
            if gate.tick(sine(110.0, amplitude, i)) < 0.5 && i > 4800 {
                return i as f32 / 48_000.0;
            }
        }
        4.0
    }

    #[test]
    fn hysteresis_lets_a_decaying_note_ring_below_the_open_threshold() {
        let narrow = sustain_seconds(0.0);
        let legacy = sustain_seconds(6.0);
        let wide = sustain_seconds(18.0);
        // Each extra dB of hysteresis buys ~0.058 s of this 17.4 dB/s decay.
        assert!(legacy - narrow > 0.25, "{narrow} {legacy}");
        assert!(wide - legacy > 0.6, "{legacy} {wide}");
        assert!(wide < 3.0, "the gate must still close: {wide}");
    }

    #[test]
    fn a_closed_gate_attenuates_noise_between_the_close_and_open_levels() {
        let mut gate = Gate::new(48_000, true);
        gate.configure(true, -40.0, 12.0, 20.0, 50.0, 80.0, 0);
        // 1 kHz "noise" 9 dB under the open threshold, above the close level.
        for i in 0..48_000 {
            gate.tick(sine(1000.0, 10.0_f32.powf(-49.0 / 20.0), i));
        }
        assert!(!gate.open);
        assert!(gate.gain < 0.05, "{}", gate.gain);
    }

    #[test]
    fn mains_hum_above_threshold_cannot_open_the_gate_but_low_e_can() {
        for hum_hz in [50.0, 60.0] {
            let mut gate = Gate::new(48_000, true);
            gate.configure(true, -40.0, 12.0, 20.0, 50.0, 80.0, 0);
            // Hum peaking 2 dB above the threshold on the clean input. The
            // old 40 Hz first-order detector saw it above threshold and
            // opened fully; now it stays on the expander's knee.
            for i in 0..48_000 {
                gate.tick(sine(hum_hz, 0.0126, i));
            }
            assert!(!gate.open, "{hum_hz} Hz hum opened the gate");
            assert!(gate.gain < 0.3, "{hum_hz} Hz: {}", gate.gain);
        }
        let mut gate = Gate::new(48_000, true);
        gate.configure(true, -40.0, 12.0, 20.0, 50.0, 80.0, 0);
        for i in 0..4800 {
            gate.tick(sine(82.41, 0.03, i));
        }
        assert!(gate.gain > 0.99);
    }

    #[test]
    fn low_notes_near_the_close_level_do_not_chatter() {
        let mut gate = Gate::new(48_000, true);
        gate.configure(true, -40.0, 12.0, 0.0, 5.0, 80.0, 0);
        for i in 0..4800 {
            gate.tick(sine(61.74, 0.1, i));
        }
        // Low B held 4 dB above the close threshold, with no hold time to
        // mask dips between waveform peaks. The 25 ms envelope ripples ~2.8 dB
        // per half-cycle here (the old 10 ms one rippled ~7 dB and chattered).
        let close = 10.0_f32.powf((-40.0 - 12.0 + 4.0) / 20.0);
        let detector_gain = 0.51; // 80 Hz 2nd-order high-pass at 61.7 Hz
        let mut minimum = 1.0_f32;
        for i in 0..48_000 {
            minimum = minimum.min(gate.tick(sine(61.74, close / detector_gain, i)));
        }
        assert!(minimum > 0.99, "{minimum}");
    }
}
