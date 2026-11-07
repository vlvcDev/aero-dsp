//! An original split-band overdrive: keep the low fundamental clean while the
//! upper band drives a soft clipping stage. No model or impulse file is needed.
use crate::{
    fuzz::FuzzNode,
    node_rig::{Frame, StereoNode},
};

pub struct OverdriveNode {
    clipper: FuzzNode,
    crossover: f32,
    low: Frame,
    previous_dry: Frame,
    mix: f32,
    output: f32,
}

impl OverdriveNode {
    pub fn new(
        rate: u32,
        drive_db: f32,
        tone: f32,
        mix: f32,
        output_db: f32,
    ) -> Result<Self, &'static str> {
        if !(0.0..=24.0).contains(&drive_db)
            || !(0.0..=1.0).contains(&tone)
            || !(0.0..=1.0).contains(&mix)
            || !(-24.0..=6.0).contains(&output_db)
        {
            return Err("Invalid overdrive configuration");
        }
        // Push mids and pick attack into clipping; restore clean bass afterward.
        let clipper = FuzzNode::new(rate, drive_db, 0.0, 0.28 + 0.68 * tone, 1.0, 0.0)?;
        Ok(Self {
            clipper,
            crossover: 1.0 - (-std::f32::consts::TAU * 320.0 / rate as f32).exp(),
            low: [0.0; 2],
            previous_dry: [0.0; 2],
            mix,
            output: 10.0_f32.powf(output_db / 20.0),
        })
    }
}

impl StereoNode for OverdriveNode {
    fn latency_samples(&self) -> usize {
        self.clipper.latency_samples()
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let clean = input.map(|x| {
            if x.is_finite() {
                x.clamp(-1.0e6, 1.0e6)
            } else {
                0.0
            }
        });
        let delayed_low = self.low;
        let high = std::array::from_fn(|i| {
            self.low[i] += self.crossover * (clean[i] - self.low[i]);
            clean[i] - self.low[i]
        });
        let driven = self.clipper.tick(high);
        let dry = std::mem::replace(&mut self.previous_dry, clean);
        std::array::from_fn(|i| {
            (dry[i] * (1.0 - self.mix) + (delayed_low[i] + driven[i]) * self.mix) * self.output
        })
    }
    fn reset(&mut self) {
        self.clipper.reset();
        self.low = [0.0; 2];
        self.previous_dry = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_changes_upper_band_more_than_bass_and_preserves_dry_timing() {
        fn strength(hz: f32, gain: f32) -> f32 {
            let mut node = OverdriveNode::new(48000, gain, 0.8, 1.0, 0.0).unwrap();
            let mut power = 0.0;
            for i in 0..4800 {
                let input = 0.2 * (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin();
                let y = node.tick([input, 0.0]);
                assert_eq!(y[1], 0.0);
                if i >= 2400 {
                    power += y[0] * y[0];
                }
            }
            power.sqrt()
        }
        let low = strength(60.0, 18.0) / strength(60.0, 0.0);
        let mids = strength(1200.0, 18.0) / strength(1200.0, 0.0);
        assert!(mids > low * 1.5);
        let mut node = OverdriveNode::new(48000, 24.0, 1.0, 0.0, 0.0).unwrap();
        assert_eq!(node.tick([0.5, -0.5]), [0.0; 2]);
        assert_eq!(node.tick([0.0; 2]), [0.5, -0.5]);
    }
}
