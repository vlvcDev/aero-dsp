//! Original biased soft-clipping fuzz. First-order antiderivative antialiasing
//! averages the transfer function between samples; this reduces, but does not
//! eliminate, aliasing. A second two-point average gives the nonlinear stage
//! a nominal one-sample delay in the small-signal limit; the dry blend is
//! delayed by one sample. DC and tone filters add frequency-dependent phase,
//! and nonlinear waveforms need not align exactly with the dry path.
use crate::node_rig::{Frame, StereoNode};

#[derive(Clone, Copy, Default)]
struct Channel {
    previous: f64,
    previous_shaped: f64,
    dc_input: f64,
    dc_output: f64,
    tone: f64,
}

pub struct FuzzNode {
    channels: [Channel; 2],
    drive: f64,
    bias: f64,
    baseline: f64,
    mix: f64,
    output: f64,
    dc_coefficient: f64,
    tone_coefficient: f64,
}

// Integral of tanh(x), evaluated without cosh overflow.
fn integral(x: f64) -> f64 {
    let a = x.abs();
    a + (-2.0 * a).exp().ln_1p() - std::f64::consts::LN_2
}

impl FuzzNode {
    pub fn new(
        rate: u32,
        drive_db: f32,
        bias: f32,
        tone: f32,
        mix: f32,
        output_db: f32,
    ) -> Result<Self, &'static str> {
        if !(8_000..=192_000).contains(&rate)
            || !drive_db.is_finite()
            || !(0.0..=36.0).contains(&drive_db)
            || !bias.is_finite()
            || !(-1.0..=1.0).contains(&bias)
            || !tone.is_finite()
            || !(0.0..=1.0).contains(&tone)
            || !mix.is_finite()
            || !(0.0..=1.0).contains(&mix)
            || !output_db.is_finite()
            || !(-36.0..=6.0).contains(&output_db)
        {
            return Err("Invalid fuzz configuration");
        }
        let bias = bias as f64;
        let cutoff = (500.0 * 24.0_f64.powf(tone as f64)).min(rate as f64 * 0.45);
        Ok(Self {
            channels: [Channel::default(); 2],
            drive: 10.0_f64.powf(drive_db as f64 / 20.0),
            bias,
            baseline: bias.tanh(),
            mix: mix as f64,
            output: 10.0_f64.powf(output_db as f64 / 20.0),
            dc_coefficient: (-std::f64::consts::TAU * 20.0 / rate as f64).exp(),
            tone_coefficient: 1.0 - (-std::f64::consts::TAU * cutoff / rate as f64).exp(),
        })
    }
}
impl StereoNode for FuzzNode {
    fn latency_samples(&self) -> usize {
        1
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let mut out = [0.0; 2];
        for (i, state) in self.channels.iter_mut().enumerate() {
            // Bound finite but pathological input as well as NaN/inf, preventing
            // poisoned state and numerical overflow in the nonlinear stage.
            let x = if input[i].is_finite() {
                (input[i] as f64).clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            };
            let a = x * self.drive + self.bias;
            let b = state.previous * self.drive + self.bias;
            let delta = a - b;
            let shaped = if delta.abs() < 1.0e-5 {
                ((a + b) * 0.5).tanh()
            } else {
                (integral(a) - integral(b)) / delta
            } - self.baseline;
            let aligned = (shaped + state.previous_shaped) * 0.5;
            state.previous_shaped = shaped;
            let dc = aligned - state.dc_input + self.dc_coefficient * state.dc_output;
            state.dc_input = aligned;
            state.dc_output = dc;
            state.tone += self.tone_coefficient * (dc - state.tone);
            let dry = state.previous;
            state.previous = x;
            out[i] = ((dry * (1.0 - self.mix) + state.tone * self.mix) * self.output) as f32;
        }
        out
    }
    fn reset(&mut self) {
        self.channels = [Channel::default(); 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(drive: f32, bias: f32, tone: f32, mix: f32, output: f32) -> FuzzNode {
        FuzzNode::new(48_000, drive, bias, tone, mix, output).unwrap()
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|x| x * x).sum::<f32>() / x.len() as f32).sqrt()
    }
    #[test]
    fn validates_and_stays_finite() {
        assert!(FuzzNode::new(0, 0.0, 0.0, 0.5, 1.0, 0.0).is_err());
        assert!(FuzzNode::new(48_000, f32::NAN, 0.0, 0.5, 1.0, 0.0).is_err());
        assert!(FuzzNode::new(48_000, 37.0, 0.0, 0.5, 1.0, 0.0).is_err());
        let mut n = node(36.0, 1.0, 1.0, 1.0, 6.0);
        for x in [f32::NAN, f32::INFINITY, f32::MAX, 0.0] {
            assert!(n.tick([x, x]).iter().all(|x| x.is_finite()));
        }
    }
    #[test]
    fn dry_mix_and_output_gain_are_predictable() {
        let mut n = node(18.0, 0.0, 0.5, 0.0, 0.0);
        assert_eq!(n.latency_samples(), 1);
        assert_eq!(n.tick([0.5, -0.25]), [0.0, 0.0]);
        assert_eq!(n.tick([0.0, 0.0]), [0.5, -0.25]);
        assert_eq!(n.tick([0.0, 0.0]), [0.0, 0.0]);
        let mut low = node(18.0, 0.0, 0.5, 0.0, -6.0206);
        assert_eq!(low.tick([1.0, 0.0])[0], 0.0);
        assert!((low.tick([0.0, 0.0])[0] - 0.5).abs() < 1e-6);
    }
    #[test]
    fn wet_small_signal_impulse_has_one_sample_nominal_center() {
        // Undo the known linear output filters to inspect the waveshaper's
        // small-signal impulse. The two averages yield a symmetric 1:2:1 FIR.
        let mut n = node(0.0, 0.0, 1.0, 1.0, 0.0);
        let mut previous_tone = 0.0;
        let mut previous_dc = 0.0;
        let mut previous_aligned = 0.0;
        let mut response = [0.0; 4];
        for (i, sample) in response.iter_mut().enumerate() {
            let y = n.tick([if i == 0 { 0.001 } else { 0.0 }, 0.0])[0] as f64;
            let dc = (y - (1.0 - n.tone_coefficient) * previous_tone) / n.tone_coefficient;
            let aligned = dc + previous_aligned - n.dc_coefficient * previous_dc;
            previous_tone = y;
            previous_dc = dc;
            previous_aligned = aligned;
            *sample = aligned;
        }
        assert!((response[0] - 0.00025).abs() < 1e-8);
        assert!((response[1] - 0.0005).abs() < 1e-8);
        assert!((response[2] - 0.00025).abs() < 1e-8);
        assert!(response[3].abs() < 1e-8);
    }
    #[test]
    fn drive_bias_tone_and_dc_removal_change_signal() {
        let mut nodes = [
            node(0.0, 0.0, 1.0, 1.0, 0.0),
            node(24.0, 0.0, 1.0, 1.0, 0.0),
            node(24.0, 0.8, 1.0, 1.0, 0.0),
            node(24.0, 0.0, 0.0, 1.0, 0.0),
        ];
        let mut outputs = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for i in 0..12_000 {
            let x = 0.1 * (std::f32::consts::TAU * 4000.0 * i as f32 / 48_000.0).sin();
            for (n, y) in nodes.iter_mut().zip(outputs.iter_mut()) {
                y.push(n.tick([x, 0.0])[0]);
            }
        }
        assert!(rms(&outputs[1][1000..]) > rms(&outputs[0][1000..]) * 2.0);
        assert!(rms(&outputs[3][1000..]) < rms(&outputs[1][1000..]) * 0.5);
        assert!(
            outputs[1]
                .iter()
                .zip(&outputs[2])
                .any(|(a, b)| (a - b).abs() > 0.02)
        );
        let mut n = node(18.0, 1.0, 1.0, 1.0, 0.0);
        let mut last = 0.0;
        for _ in 0..48_000 {
            last = n.tick([0.2, 0.0])[0];
        }
        assert!(last.abs() < 1e-5);
    }
    #[test]
    fn stereo_reset_and_blocks_are_independent() {
        let mut a = node(18.0, 0.3, 0.5, 1.0, -12.0);
        let mut b = node(18.0, 0.3, 0.5, 1.0, -12.0);
        let mut x: Vec<Frame> = (0..1000).map(|i| [(i as f32 * 0.1).sin(), 0.0]).collect();
        let mut y = x.clone();
        a.process_block(&mut x);
        for chunk in y.chunks_mut(17) {
            b.process_block(chunk);
        }
        assert_eq!(x, y);
        assert!(x.iter().all(|f| f[1] == 0.0));
        a.reset();
        b.reset();
        assert_eq!(a.tick([0.2, -0.3]), b.tick([0.2, -0.3]));
        assert_eq!(a.latency_samples(), 1);
    }
    #[test]
    fn antiderivative_reduces_a_high_frequency_alias() {
        // 9 kHz third harmonic folds to 21 kHz at 48 kHz. Compare the
        // alias-to-fundamental ratio with direct base-rate tanh clipping.
        let mut n = node(24.0, 0.0, 1.0, 1.0, 0.0);
        let mut filtered = Vec::new();
        let mut direct = Vec::new();
        for i in 0..16_384 {
            let x = 0.5 * (std::f64::consts::TAU * 9000.0 * i as f64 / 48_000.0).sin();
            let y = n.tick([x as f32, 0.0])[0] as f64;
            if i >= 4096 {
                filtered.push(y);
                direct.push((x * 10.0_f64.powf(24.0 / 20.0)).tanh());
            }
        }
        let magnitude = |x: &[f64], hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, s) in x.iter().enumerate() {
                let phase = std::f64::consts::TAU * hz * i as f64 / 48_000.0;
                re += s * phase.cos();
                im += s * phase.sin();
            }
            re.hypot(im)
        };
        assert!(
            magnitude(&filtered, 21_000.0) / magnitude(&filtered, 9_000.0)
                < magnitude(&direct, 21_000.0) / magnitude(&direct, 9_000.0) * 0.7
        );
    }
}
