//! Two octave-down voices. The dry signal is deliberately not delayed.
//! Configure/allocate at graph creation, never when the mode button is pressed.
use signalsmith_stretch::Stretch;

pub fn clean_latency_frames(sample_rate: u32) -> usize {
    // Equal analysis/synthesis offsets: total latency equals the window size.
    // Verified against the library's latency getters in regression tests.
    ((sample_rate as usize * 4096 / 48_000).max(512)).next_power_of_two()
}

pub struct Octaver {
    clean: Stretch,
    latency: usize,
    warmup: usize,
    clean_running: bool,
    previous: f32,
    polarity: f32,
    analog_gain: f32,
    clean_gain: f32,
    step: f32,
}

impl Octaver {
    pub fn new(rate: u32) -> Self {
        let window = clean_latency_frames(rate);
        let mut clean = Stretch::new(1, window, window / 8);
        clean.set_transpose_factor(0.5, None);
        let latency = clean.input_latency() + clean.output_latency();
        Self {
            clean,
            latency,
            warmup: 0,
            clean_running: false,
            previous: 0.0,
            polarity: 1.0,
            analog_gain: 0.0,
            clean_gain: 0.0,
            step: 1.0 / (0.010 * rate.max(1) as f32),
        }
    }

    pub fn process(&mut self, samples: &mut [f32], enabled: bool, clean_mode: bool, mix: f32) {
        let want_clean = enabled && clean_mode && mix > 0.0;
        if want_clean && !self.clean_running {
            // Feed fresh audio before exposing this voice. This also replaces
            // stale history after a bypass, without resetting/allocating in RT.
            self.warmup = self.latency * 2;
        }
        let run_clean = want_clean || self.clean_gain > 0.0;
        self.clean_running = run_clean;
        for samples in samples.chunks_mut(128) {
            let mut shifted = [0.0; 128];
            if run_clean {
                self.clean.process(&*samples, &mut shifted[..samples.len()]);
            }
            for (dry, shifted) in samples.iter_mut().zip(shifted) {
                if self.previous <= 0.0 && *dry > 0.0 {
                    self.polarity = -self.polarity;
                }
                self.previous = *dry;
                let analog = dry.abs() * self.polarity;
                self.warmup = self.warmup.saturating_sub(1);
                let a = if enabled && !clean_mode { mix } else { 0.0 };
                let c = if want_clean && self.warmup == 0 {
                    mix
                } else {
                    0.0
                };
                self.analog_gain += (a - self.analog_gain).clamp(-self.step, self.step);
                self.clean_gain += (c - self.clean_gain).clamp(-self.step, self.step);
                *dry = *dry * (1.0 - self.analog_gain - self.clean_gain)
                    + analog * self.analog_gain
                    + shifted * self.clean_gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn displayed_latency_matches_the_actual_library_configuration() {
        for rate in [44_100, 48_000, 96_000] {
            assert_eq!(Octaver::new(rate).latency, clean_latency_frames(rate));
        }
    }
    #[test]
    fn bypass_is_exact_and_modes_switch_without_nonfinite_audio() {
        let mut effect = Octaver::new(48000);
        let mut bypass = [0.1, -0.2, 0.3];
        effect.process(&mut bypass, false, false, 0.5);
        assert_eq!(bypass, [0.1, -0.2, 0.3]);
        for i in 0..1000 {
            let mut audio = [0.1; 64];
            effect.process(&mut audio, i % 200 < 150, i % 400 < 200, 0.5);
            assert!(audio.iter().all(|s| s.is_finite() && s.abs() < 1.0));
        }
    }
    #[test]
    fn clean_voice_shifts_multiple_notes_down_an_octave() {
        let mut effect = Octaver::new(48000);
        let mut audio: Vec<f32> = (0..96000)
            .map(|i| {
                let t = i as f32 / 48000.0 * std::f32::consts::TAU;
                0.1 * ((220.0 * t).sin() + (330.0 * t).sin() + (440.0 * t).sin())
            })
            .collect();
        for block in audio.chunks_mut(64) {
            effect.process(block, true, true, 1.0);
        }
        let tail = &audio[48000..];
        let magnitude = |hz: f32| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, s) in tail.iter().enumerate() {
                let p = std::f32::consts::TAU * hz * i as f32 / 48000.0;
                re += s * p.cos();
                im += s * p.sin();
            }
            (re * re + im * im).sqrt() / tail.len() as f32
        };
        for frequency in [110.0, 165.0, 220.0] {
            assert!(
                magnitude(frequency) > 0.015,
                "missing shifted note {frequency}"
            );
        }
        assert!(
            magnitude(330.0) < 0.01,
            "unshifted input leaked into wet-only output"
        );
    }
}
