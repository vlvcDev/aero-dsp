//! Free-running creative stereo effects. Slicer reset restarts its pattern;
//! there is no host transport synchronization.
use crate::node_rig::{Frame, StereoNode};
use std::f32::consts::{PI, TAU};
fn clean(f: Frame) -> Frame {
    f.map(|x| {
        if x.is_finite() {
            x.clamp(-1e6, 1e6)
        } else {
            0.0
        }
    })
}
fn valid(rate: u32, p: &[(f32, f32, f32)]) -> bool {
    (8000..=192000).contains(&rate)
        && p.iter()
            .all(|&(x, a, b)| x.is_finite() && (a..=b).contains(&x))
}
/// Stereo-linked envelope and independent resonant low-pass filters.
pub struct EnvelopeFilterNode {
    rate: f32,
    sensitivity: f32,
    base: f32,
    resonance: f32,
    mix: f32,
    attack: f32,
    release: f32,
    env: f32,
    ic1: Frame,
    ic2: Frame,
}
impl EnvelopeFilterNode {
    pub fn new(
        rate: u32,
        sensitivity: f32,
        base_hz: f32,
        resonance: f32,
        decay_ms: f32,
        mix: f32,
    ) -> Result<Self, &'static str> {
        if !valid(
            rate,
            &[
                (sensitivity, 0., 1.),
                (base_hz, 80., 2000.),
                (resonance, 0.5, 8.),
                (decay_ms, 20., 1000.),
                (mix, 0., 1.),
            ],
        ) {
            return Err("Invalid envelope filter configuration");
        }
        let r = rate as f32;
        Ok(Self {
            rate: r,
            sensitivity,
            base: base_hz,
            resonance,
            mix,
            attack: 1. - (-1. / (0.005 * r)).exp(),
            release: 1. - (-1. / (0.001 * decay_ms * r)).exp(),
            env: 0.,
            ic1: [0.; 2],
            ic2: [0.; 2],
        })
    }
}
impl StereoNode for EnvelopeFilterNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "sensitivity" if (0.0..=1.0).contains(&value) => self.sensitivity = value,
            "base_hz" if (80.0..=2000.0).contains(&value) => self.base = value,
            "resonance" if (0.5..=8.0).contains(&value) => self.resonance = value,
            "decay_ms" if (20.0..=1000.0).contains(&value) => {
                self.release = 1.0 - (-1.0 / (0.001 * value * self.rate)).exp()
            }
            "mix" if (0.0..=1.0).contains(&value) => self.mix = value,
            _ => return false,
        }
        true
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let x = clean(input);
        let level = x[0].abs().max(x[1].abs()).min(1.);
        let c = if level > self.env {
            self.attack
        } else {
            self.release
        };
        self.env += c * (level - self.env);
        // A quiet guitar signal still needs a musically useful sweep. At the
        // midpoint sensitivity, a 0.05 envelope moves the cutoff by 2.6x.
        let hz = (self.base * (1. + 64. * self.sensitivity * self.env)).min(self.rate * 0.45);
        // Trapezoidal state-variable filter: Wishnick, DAFx-14, section 2.
        // https://www.dafx.de/paper-archive/2014/dafx14_aaron_wishnick_time_varying_filters_for_.pdf
        let g = (PI * hz / self.rate).tan();
        let a = 1. / (1. + g * (g + 1. / self.resonance));
        std::array::from_fn(|i| {
            let v1 = a * (self.ic1[i] + g * (x[i] - self.ic2[i]));
            let v2 = self.ic2[i] + g * v1;
            self.ic1[i] = 2. * v1 - self.ic1[i];
            self.ic2[i] = 2. * v2 - self.ic2[i];
            x[i] * (1. - self.mix) + v2 * self.mix
        })
    }
    fn reset(&mut self) {
        self.env = 0.;
        self.ic1 = [0.; 2];
        self.ic2 = [0.; 2];
    }
}
/// Shared carrier preserves stereo phase.
pub struct RingModNode {
    rate: f32,
    step: f32,
    phase: f32,
    mix: f32,
}
impl RingModNode {
    pub fn new(rate: u32, frequency_hz: f32, mix: f32) -> Result<Self, &'static str> {
        if !valid(rate, &[(frequency_hz, 1., 4000.), (mix, 0., 1.)]) {
            return Err("Invalid ring modulator configuration");
        }
        Ok(Self {
            rate: rate as f32,
            step: TAU * frequency_hz / rate as f32,
            phase: 0.,
            mix,
        })
    }
}
impl StereoNode for RingModNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "frequency_hz" if (1.0..=4000.0).contains(&value) => {
                self.step = TAU * value / self.rate
            }
            "mix" if (0.0..=1.0).contains(&value) => self.mix = value,
            _ => return false,
        }
        true
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let c = self.phase.sin();
        self.phase = (self.phase + self.step).rem_euclid(TAU);
        clean(input).map(|x| x * (1. - self.mix + self.mix * c))
    }
    fn reset(&mut self) {
        self.phase = 0.;
    }
}
/// Four-subdivision cycle: pulse opens on steps 1 and 3, offbeat on 2 and 4.
/// Gallop opens for half of steps 1 and 2, then nearly all of steps 3 and 4.
/// Transitions use a three millisecond gate ramp.
pub struct SlicerNode {
    rate: f64,
    division: f64,
    step: f64,
    phase: f64,
    pattern: u8,
    depth: f32,
    gain: f32,
    ramp: f32,
}
impl SlicerNode {
    pub fn new(
        rate: u32,
        bpm: f32,
        division: u32,
        pattern: &str,
        depth: f32,
    ) -> Result<Self, &'static str> {
        if !valid(rate, &[(bpm, 30., 300.), (depth, 0., 1.)]) || ![1, 2, 4].contains(&division) {
            return Err("Invalid slicer configuration");
        }
        let pattern = match pattern {
            "pulse" => 0,
            "offbeat" => 1,
            "gallop" => 2,
            _ => return Err("Invalid slicer pattern"),
        };
        Ok(Self {
            rate: rate as f64,
            division: division as f64,
            step: bpm as f64 * division as f64 / (60. * rate as f64),
            phase: 0.,
            pattern,
            depth,
            gain: 1.,
            ramp: 1. / (0.003 * rate as f32),
        })
    }
}
impl StereoNode for SlicerNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "bpm" if (30.0..=300.0).contains(&value) => {
                self.step = value as f64 * self.division / (60.0 * self.rate)
            }
            "depth" if (0.0..=1.0).contains(&value) => self.depth = value,
            _ => return false,
        }
        true
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let open = match self.pattern {
            0 => (self.phase.floor() as u32) % 2 == 0,
            1 => (self.phase.floor() as u32) % 2 == 1,
            _ => {
                self.phase < 0.5
                    || (1.0..1.5).contains(&self.phase)
                    || (2.0..3.75).contains(&self.phase)
            }
        };
        let target = if open { 1. } else { 1. - self.depth };
        self.gain += (target - self.gain).clamp(-self.ramp, self.ramp);
        self.phase = (self.phase + self.step) % 4.;
        clean(input).map(|x| x * self.gain)
    }
    fn reset(&mut self) {
        self.phase = 0.;
        self.gain = 1.;
    }
}
/// Quantization and sample-and-hold intentionally produce distortion and aliasing.
pub struct BitcrusherNode {
    rate: f32,
    levels: f32,
    step: f64,
    phase: f64,
    held: Frame,
    mix: f32,
}
impl BitcrusherNode {
    pub fn new(rate: u32, bits: u32, rate_hz: f32, mix: f32) -> Result<Self, &'static str> {
        if !valid(rate, &[(rate_hz, 1000., 48000.), (mix, 0., 1.)]) || !(4..=16).contains(&bits) {
            return Err("Invalid bitcrusher configuration");
        }
        Ok(Self {
            rate: rate as f32,
            levels: ((1_u32 << (bits - 1)) - 1) as f32,
            step: (rate_hz.min(rate as f32) / rate as f32) as f64,
            phase: 1.,
            held: [0.; 2],
            mix,
        })
    }
}
impl StereoNode for BitcrusherNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "rate_hz" if (1000.0..=48000.0).contains(&value) => {
                self.step = (value.min(self.rate) / self.rate) as f64
            }
            "mix" if (0.0..=1.0).contains(&value) => self.mix = value,
            _ => return false,
        }
        true
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let x = clean(input);
        if self.phase >= 1. {
            self.phase -= 1.;
            self.held = x.map(|v| (v.clamp(-1., 1.) * self.levels).round() / self.levels);
        }
        self.phase += self.step;
        std::array::from_fn(|i| x[i] * (1. - self.mix) + self.held[i] * self.mix)
    }
    fn reset(&mut self) {
        self.phase = 1.;
        self.held = [0.; 2];
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_parameters_preserve_running_state_and_change_output() {
        let mut ring = RingModNode::new(48000, 100., 1.).unwrap();
        for _ in 0..123 {
            ring.tick([1.; 2]);
        }
        let phase = ring.phase;
        assert!(ring.set_parameter("frequency_hz", 200.));
        assert_eq!(ring.phase, phase);
        assert!((ring.tick([1.; 2])[0] - phase.sin()).abs() < 1e-6);
        assert!(!ring.set_parameter("frequency_hz", f32::NAN));
        let mut filter = EnvelopeFilterNode::new(48000, 0.5, 300., 2., 200., 1.).unwrap();
        for _ in 0..100 {
            filter.tick([0.5; 2]);
        }
        let env = filter.env;
        let state = filter.ic2;
        assert!(filter.set_parameter("base_hz", 1000.));
        assert_eq!(filter.env, env);
        assert_eq!(filter.ic2, state);
        let mut crusher = BitcrusherNode::new(48000, 4, 12000., 1.).unwrap();
        let held = crusher.tick([0.3; 2]);
        assert!(crusher.set_parameter("rate_hz", 24000.));
        assert_eq!(crusher.tick([0.8; 2]), held);
        assert!(crusher.set_parameter("mix", 0.));
        assert_eq!(crusher.tick([0.8; 2]), [0.8; 2]);
        assert!(!crusher.set_parameter("bits", 8.));
    }
    #[test]
    fn envelope_dynamics() {
        assert!(EnvelopeFilterNode::new(48000, f32::NAN, 300., 2., 200., 1.).is_err());
        let mut a = EnvelopeFilterNode::new(48000, 0., 300., 8., 200., 1.).unwrap();
        let mut b = EnvelopeFilterNode::new(48000, 1., 300., 8., 200., 1.).unwrap();
        let mut d = 0.;
        for i in 0..48000 {
            let x = 0.8 * (TAU * 800. * i as f32 / 48000.).sin();
            let y = a.tick([x, 0.]);
            let z = b.tick([x, 0.]);
            assert!(y[0].is_finite() && z[0].is_finite() && z[0].abs() < 20.);
            assert_eq!(z[1], 0.);
            d += (y[0] - z[0]).abs();
        }
        assert!(d > 100.);
        b.reset();
        assert_eq!(b.tick([0.; 2]), [0.; 2]);
    }
    #[test]
    fn envelope_responds_to_quiet_guitar_levels() {
        let mut fixed = EnvelopeFilterNode::new(48000, 0., 300., 2., 200., 1.).unwrap();
        let mut dynamic = EnvelopeFilterNode::new(48000, 0.5, 300., 2., 200., 1.).unwrap();
        let mut difference = 0.0;
        let mut fixed_energy = 0.0;
        for i in 0..24000 {
            let x = 0.05 * (TAU * 800. * i as f32 / 48000.).sin();
            let a = fixed.tick([x, x])[0];
            let b = dynamic.tick([x, x])[0];
            if i >= 12000 {
                difference += (a - b).abs();
                fixed_energy += a.abs();
            }
        }
        assert!(
            difference > fixed_energy,
            "quiet input should produce an audible sweep"
        );
    }
    #[test]
    fn ring_sidebands() {
        let mut n = RingModNode::new(48000, 100., 1.).unwrap();
        let (mut lo, mut hi, mut center) = (0., 0., 0.);
        for i in 0..48000 {
            let t = i as f32 / 48000.;
            let y = n.tick([(TAU * 1000. * t).sin(), 0.]);
            assert_eq!(y[1], 0.);
            lo += y[0] * (TAU * 900. * t).cos();
            hi += y[0] * (TAU * 1100. * t).cos();
            center += y[0] * (TAU * 1000. * t).sin();
        }
        assert!(lo.abs() > 10000. && hi.abs() > 10000. && center.abs() < 10.);
        n.reset();
        assert_eq!(n.tick([1.; 2]), [0.; 2]);
    }
    #[test]
    fn slicer_timing() {
        for (pattern, expected) in [
            ("pulse", [1, 0, 1, 0]),
            ("offbeat", [0, 1, 0, 1]),
            ("gallop", [0, 0, 1, 1]),
        ] {
            let mut n = SlicerNode::new(8000, 120., 4, pattern, 1.).unwrap();
            let mut previous = 1.;
            for i in 0..4000 {
                let y = n.tick([1.; 2])[0];
                assert!((y - previous).abs() <= n.ramp + 1e-6);
                previous = y;
                if i % 1000 == 750 {
                    assert!((y - expected[i / 1000] as f32).abs() < 0.001);
                }
            }
            n.reset();
            assert_eq!(n.phase, 0.);
        }
    }
    #[test]
    fn crusher_hold() {
        let mut n = BitcrusherNode::new(48000, 4, 12000., 1.).unwrap();
        let a = n.tick([0.3, -0.7]);
        assert_ne!(a[0], a[1]);
        for _ in 0..3 {
            assert_eq!(n.tick([0.8, 0.9]), a);
        }
        assert_ne!(n.tick([0.8, 0.9]), a);
        n.reset();
        assert_eq!(n.tick([0.3, -0.7]), a);
    }
    #[test]
    fn parameter_ranges_and_nonfinite_are_rejected() {
        assert!(RingModNode::new(48000, 0.0, 0.5).is_err());
        assert!(RingModNode::new(48000, 4001.0, 0.5).is_err());
        assert!(RingModNode::new(48000, 80.0, f32::NAN).is_err());
        assert!(SlicerNode::new(48000, 29.0, 2, "pulse", 1.0).is_err());
        assert!(SlicerNode::new(48000, 120.0, 3, "pulse", 1.0).is_err());
        assert!(SlicerNode::new(48000, 120.0, 2, "bad", 1.0).is_err());
        assert!(SlicerNode::new(48000, 120.0, 2, "pulse", f32::INFINITY).is_err());
        assert!(BitcrusherNode::new(48000, 3, 12000.0, 0.5).is_err());
        assert!(BitcrusherNode::new(48000, 17, 12000.0, 0.5).is_err());
        assert!(BitcrusherNode::new(48000, 8, 999.0, 0.5).is_err());
        assert!(BitcrusherNode::new(48000, 8, f32::NAN, 0.5).is_err());
        assert!(EnvelopeFilterNode::new(48000, 1.1, 300.0, 2.0, 200.0, 1.0).is_err());
        assert!(EnvelopeFilterNode::new(48000, 0.5, 79.0, 2.0, 200.0, 1.0).is_err());
        assert!(EnvelopeFilterNode::new(48000, 0.5, 300.0, 8.1, 200.0, 1.0).is_err());
        assert!(EnvelopeFilterNode::new(48000, 0.5, 300.0, 2.0, 19.0, 1.0).is_err());
        assert!(EnvelopeFilterNode::new(7999, 0.5, 300.0, 2.0, 200.0, 1.0).is_err());
    }
    #[test]
    fn envelope_extremes_stay_stable() {
        for rate in [8000, 192000] {
            for base in [80.0, 2000.0] {
                for q in [0.5, 8.0] {
                    let mut n = EnvelopeFilterNode::new(rate, 1.0, base, q, 20.0, 1.0).unwrap();
                    for i in 0..rate {
                        let x = if i % 173 < 86 { 1.0 } else { -1.0 };
                        let y = n.tick([x, -x]);
                        assert!(y[0].is_finite() && y[0].abs() < 100.0 && y[1].is_finite());
                    }
                }
            }
        }
    }
    #[test]
    fn all_effects_reset_and_process_block_consistently() {
        fn check(mut a: Box<dyn StereoNode>, mut b: Box<dyn StereoNode>) {
            let input: Vec<Frame> = (0..5000)
                .map(|i| [(i as f32 * 0.03).sin(), (i as f32 * 0.021).cos()])
                .collect();
            let expected: Vec<_> = input.iter().map(|&x| a.tick(x)).collect();
            let mut actual = input.clone();
            for block in actual.chunks_mut(127) {
                b.process_block(block);
            }
            assert_eq!(expected, actual);
            a.reset();
            b.reset();
            for x in input.iter().take(300) {
                assert_eq!(a.tick(*x), b.tick(*x));
            }
        }
        check(
            Box::new(EnvelopeFilterNode::new(48000, 0.8, 300., 3., 200., 1.).unwrap()),
            Box::new(EnvelopeFilterNode::new(48000, 0.8, 300., 3., 200., 1.).unwrap()),
        );
        check(
            Box::new(RingModNode::new(48000, 83., 0.5).unwrap()),
            Box::new(RingModNode::new(48000, 83., 0.5).unwrap()),
        );
        check(
            Box::new(SlicerNode::new(48000, 121., 4, "gallop", 1.).unwrap()),
            Box::new(SlicerNode::new(48000, 121., 4, "gallop", 1.).unwrap()),
        );
        check(
            Box::new(BitcrusherNode::new(48000, 8, 11025., 1.).unwrap()),
            Box::new(BitcrusherNode::new(48000, 8, 11025., 1.).unwrap()),
        );
    }
    #[test]
    fn fractional_crusher_rate_has_correct_average_and_holds() {
        let mut n = BitcrusherNode::new(48000, 16, 11025., 1.).unwrap();
        let mut changes = 0;
        let mut last = n.tick([0., 0.]);
        for i in 1..48000 {
            let y = n.tick([i as f32 / 48000., -(i as f32) / 48000.]);
            if y != last {
                changes += 1;
                last = y;
            }
        }
        assert!((11024..=11026).contains(&changes), "{changes}");
    }
    #[test]
    fn gallop_has_two_short_gates_then_a_long_gate() {
        let mut n = SlicerNode::new(8000, 120., 4, "gallop", 1.).unwrap();
        let mut samples = Vec::new();
        for _ in 0..4000 {
            samples.push(n.tick([1., 1.])[0]);
        }
        for (i, expected) in [
            (250, 1.),
            (750, 0.),
            (1250, 1.),
            (1750, 0.),
            (2250, 1.),
            (3500, 1.),
            (3900, 0.),
        ] {
            assert!((samples[i] - expected).abs() < 0.01, "{i}");
        }
    }
}
