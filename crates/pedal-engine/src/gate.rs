//! Clean-input detector; its gain is applied after NAM, before spatial effects.
//! No buffering, allocation, or locks in the sample path. The detector filters
//! are never applied to the audible guitar signal.

pub(crate) struct Gate {
    rate: f32,
    high_pass_memory: f32,
    low_pass_memory: f32,
    high_pass_coefficient: f32,
    low_pass_coefficient: f32,
    envelope: f32,
    envelope_decay: f32,
    gain: f32,
    attack: f32,
    release: f32,
    threshold: f32,
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
            high_pass_memory: 0.0,
            low_pass_memory: 0.0,
            // Gentle 40 Hz / 5 kHz detector band: retain low-tuned guitar,
            // reject DC/rumble, and reduce sensitivity to high-frequency hiss.
            high_pass_coefficient: 1.0 - (-std::f32::consts::TAU * 40.0 / rate).exp(),
            low_pass_coefficient: 1.0 - (-std::f32::consts::TAU * 5000.0 / rate).exp(),
            envelope: 0.0,
            envelope_decay: (-1.0 / (0.010 * rate)).exp(),
            gain: if enabled { 0.0001 } else { 1.0 },
            attack: 1.0 - (-1.0 / (0.0005 * rate)).exp(),
            release: 1.0 - (-1.0 / (0.050 * rate)).exp(),
            threshold: 10.0_f32.powf(-80.0 / 20.0),
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
        hold_ms: f32,
        release_ms: f32,
        range_db: f32,
        extra_hold: usize,
    ) {
        self.enabled = enabled;
        self.threshold = 10.0_f32.powf(bounded(threshold_db, -80.0, -80.0, -10.0) / 20.0);
        self.floor = 10.0_f32.powf(-bounded(range_db, 80.0, 0.0, 90.0) / 20.0);
        self.release =
            1.0 - (-1.0 / (bounded(release_ms, 50.0, 5.0, 1000.0) * 0.001 * self.rate)).exp();
        self.hold_frames =
            (bounded(hold_ms, 20.0, 0.0, 500.0) * 0.001 * self.rate) as usize + extra_hold;
        self.remaining = self.remaining.min(self.hold_frames);
    }

    pub(crate) fn tick(&mut self, input: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        self.high_pass_memory += self.high_pass_coefficient * (input - self.high_pass_memory);
        let high_pass = input - self.high_pass_memory;
        self.low_pass_memory += self.low_pass_coefficient * (high_pass - self.low_pass_memory);
        self.envelope = self
            .low_pass_memory
            .abs()
            .max(self.envelope * self.envelope_decay);
        let close_threshold = self.threshold * 0.5; // 6 dB hysteresis
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
            // 4:1 downward expansion: output amplitude falls as input^4.
            // The cubic gain is continuous at the close threshold, with a
            // user-selected attenuation floor instead of a hard mute.
            let ratio = (self.envelope / close_threshold).min(1.0);
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
        gate.configure(true, -30.0, hold, release, range, 0);
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
        for _ in 0..4800 {
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
        gate.configure(true, f32::NAN, f32::INFINITY, f32::NAN, f32::NAN, 0);
        for _ in 0..48_000 {
            assert!(gate.tick(f32::NAN).is_finite());
        }
        let before = gate.gain;
        gate.configure(false, -30.0, 20.0, 50.0, 80.0, 0);
        assert!(gate.tick(0.0) - before < 0.05);
        for _ in 0..4800 {
            gate.tick(0.0);
        }
        assert!(gate.gain > 0.999);
    }
}
