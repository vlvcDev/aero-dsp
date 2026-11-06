//! Diffused, damped eight-line feedback-delay network. Orthogonal feedback
//! mixing preserves energy before the per-line decay gains (< 1).
use std::f32::consts::TAU;
struct Allpass {
    samples: Vec<f32>,
    index: usize,
}
impl Allpass {
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.samples[self.index] - 0.65 * x;
        self.samples[self.index] = x + 0.65 * y;
        self.index = (self.index + 1) % self.samples.len();
        y
    }
}
pub struct Reverb {
    diffusers: [Allpass; 4],
    lines: [Vec<f32>; 8],
    indices: [usize; 8],
    damped: [f32; 8],
    phases: [f32; 8],
    feedback: [f32; 8],
    targets: [f32; 8],
    lengths: [usize; 8],
    rate: f32,
    damping: f32,
    smoothing: f32,
    decay: f32,
}
impl Reverb {
    pub fn clear(&mut self) {
        for diffuser in &mut self.diffusers {
            diffuser.samples.fill(0.0);
        }
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.damped.fill(0.0);
    }
    pub fn new(rate: u32) -> Self {
        Self::with_spread(rate, 1.0)
    }
    pub fn with_spread(rate: u32, spread: f32) -> Self {
        let rate = rate as f32;
        let lengths = [
            0.0297, 0.0371, 0.0411, 0.0437, 0.0533, 0.0617, 0.0719, 0.0793,
        ]
        .map(|s| (s * rate * spread.clamp(0.9, 1.1)) as usize);
        let mut reverb = Self {
            diffusers: [0.0047, 0.0073, 0.0119, 0.0173].map(|s| Allpass {
                samples: vec![0.0; ((s * rate) as usize).max(1)],
                index: 0,
            }),
            lines: std::array::from_fn(|i| vec![0.0; lengths[i] + 8]),
            indices: [0; 8],
            damped: [0.0; 8],
            phases: std::array::from_fn(|i| i as f32 * 0.7),
            feedback: [0.0; 8],
            targets: [0.0; 8],
            lengths,
            rate,
            damping: 1.0 - (-TAU * 6000.0_f32.min(rate * 0.4) / rate).exp(),
            smoothing: 1.0 - (-1.0 / (0.020 * rate)).exp(),
            decay: 0.0,
        };
        reverb.set_decay(2.5);
        reverb.feedback = reverb.targets;
        reverb
    }
    pub fn set_decay(&mut self, seconds: f32) {
        let seconds = seconds.clamp(0.3, 12.0);
        if seconds == self.decay {
            return;
        }
        self.decay = seconds;
        self.targets = std::array::from_fn(|i| {
            10.0_f32.powf(-3.0 * self.lengths[i] as f32 / self.rate / seconds)
        });
    }
    pub fn tick(&mut self, mut input: f32) -> f32 {
        for diffuser in &mut self.diffusers {
            input = diffuser.tick(input);
        }
        for i in 0..8 {
            let delay = self.lengths[i] as f32 + 2.0 + self.phases[i].sin() * 1.5;
            let line = &self.lines[i];
            let r = (self.indices[i] + line.len() - delay as usize) % line.len();
            let previous = (r + line.len() - 1) % line.len();
            let delayed = line[r] + (line[previous] - line[r]) * delay.fract();
            self.damped[i] += (delayed - self.damped[i]) * self.damping;
            self.phases[i] = (self.phases[i] + TAU * (0.11 + i as f32 * 0.019) / self.rate) % TAU;
            self.feedback[i] += (self.targets[i] - self.feedback[i]) * self.smoothing;
        }
        let sum = self.damped.iter().sum::<f32>() * 0.25;
        let mut wet = 0.0;
        for i in 0..8 {
            let polarity = if i % 2 == 0 { 1.0 } else { -1.0 };
            self.lines[i][self.indices[i]] =
                input * polarity * 0.35355338 + (self.damped[i] - sum) * self.feedback[i];
            self.indices[i] = (self.indices[i] + 1) % self.lines[i].len();
            wet += self.damped[i] * if i % 3 == 0 { -0.35355338 } else { 0.35355338 };
        }
        wet
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn impulse_builds_a_dense_stable_decaying_tail() {
        let mut r = Reverb::new(48_000);
        r.set_decay(0.5);
        let mut first = 0.0;
        let mut last = 0.0;
        let mut nonzero = 0;
        for n in 0..144_000 {
            let y = r.tick(if n == 0 { 1.0 } else { 0.0 });
            assert!(y.is_finite() && y.abs() < 2.0);
            if (4800..24000).contains(&n) {
                first += y * y;
                if y.abs() > 1e-6 {
                    nonzero += 1;
                }
            }
            if n > 120_000 {
                last += y * y;
            }
        }
        assert!(nonzero > 15000);
        assert!(first > 0.00001);
        assert!(last < first * 0.00001);
    }
    #[test]
    fn silence_does_not_generate_noise() {
        let mut r = Reverb::new(48_000);
        for _ in 0..10000 {
            assert_eq!(r.tick(0.0), 0.0);
        }
    }
}
