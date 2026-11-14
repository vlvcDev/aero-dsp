//! Original clean-to-breakup voice. A bass shelf tightens the signal before an
//! asymmetric ADAA stage. A slow rectified-level follower reduces sensitivity
//! on sustained notes. The master is linear output trim. Cabinet filtering is
//! a separate graph node. The clipper contributes one nominal sample of delay.
use crate::{
    fuzz::FuzzNode,
    node_rig::{Frame, StereoNode},
};

pub struct EdgeAmp {
    stage: FuzzNode,
    low: Frame,
    envelope: Frame,
    shelf: f32,
    attack: f32,
    release: f32,
    touch: f32,
    compensation: f32,
}

impl EdgeAmp {
    pub fn new(
        rate: u32,
        drive_db: f32,
        tone: f32,
        touch: f32,
        master_db: f32,
    ) -> Result<Self, &'static str> {
        if !(0.0..=30.0).contains(&drive_db)
            || !(0.0..=1.0).contains(&tone)
            || !(0.0..=1.0).contains(&touch)
            || !(-36.0..=6.0).contains(&master_db)
        {
            return Err("Invalid edge amp configuration");
        }
        let stage = FuzzNode::new(rate, drive_db, 0.12, 0.45 + 0.55 * tone, 1.0, master_db)?;
        let rate = rate as f32;
        Ok(Self {
            stage,
            low: [0.0; 2],
            envelope: [0.0; 2],
            shelf: 1.0 - (-std::f32::consts::TAU * 170.0 / rate).exp(),
            attack: 1.0 - (-1.0 / (0.025 * rate)).exp(),
            release: 1.0 - (-1.0 / (0.110 * rate)).exp(),
            touch,
            // Partial level compensation leaves room to turn up preamp drive.
            compensation: 10.0_f32.powf(-0.25 * drive_db / 20.0),
        })
    }
}

impl StereoNode for EdgeAmp {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "drive_db" if (0.0..=30.0).contains(&value) => {
                self.compensation = 10.0_f32.powf(-0.25 * value / 20.0);
                self.stage.set_parameter("drive_db", value)
            }
            "tone" if (0.0..=1.0).contains(&value) => {
                self.stage.set_parameter("tone", 0.45 + 0.55 * value)
            }
            "touch" if (0.0..=1.0).contains(&value) => {
                self.touch = value;
                true
            }
            "master_db" if (-36.0..=6.0).contains(&value) => {
                self.stage.set_parameter("output_db", value)
            }
            _ => false,
        }
    }
    fn latency_samples(&self) -> usize {
        self.stage.latency_samples()
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let signal = std::array::from_fn(|ch| {
            let x = if input[ch].is_finite() {
                input[ch].clamp(-8.0, 8.0)
            } else {
                0.0
            };
            self.low[ch] += self.shelf * (x - self.low[ch]);
            let level = x.abs();
            let coefficient = if level > self.envelope[ch] {
                self.attack
            } else {
                self.release
            };
            self.envelope[ch] += coefficient * (level - self.envelope[ch]);
            (x - 0.40 * self.low[ch]) / (1.0 + 5.0 * self.touch * self.envelope[ch])
        });
        self.stage.tick(signal).map(|v| v * self.compensation)
    }
    fn reset(&mut self) {
        self.stage.reset();
        self.low = [0.0; 2];
        self.envelope = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn excitation(amp: f32, drive: f32, touch: f32, tone: f32, hz: f32, master: f32) -> Vec<f32> {
        let mut node = EdgeAmp::new(48_000, drive, tone, touch, master).unwrap();
        (0..12_000)
            .map(|i| {
                let x = amp * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin();
                let y = node.tick([x, 0.0]);
                assert_eq!(y[1], 0.0);
                y[0]
            })
            .collect()
    }
    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }
    fn harmonic(x: &[f32], hz: f32) -> f32 {
        let (mut a, mut b) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let phase = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
            a += v * phase.sin();
            b += v * phase.cos();
        }
        a.hypot(b) / x.len() as f32
    }
    #[test]
    fn softer_input_cleans_up_and_drive_adds_harmonics() {
        let dirt = |amplitude, drive| {
            let y = excitation(amplitude, drive, 0.0, 0.6, 400.0, -6.0);
            harmonic(&y[6000..], 1200.0) / harmonic(&y[6000..], 400.0)
        };
        assert!(dirt(0.4, 18.0) > 0.08);
        assert!(dirt(0.01, 18.0) < 0.003);
        assert!(dirt(0.4, 0.0) < 0.02);
    }
    #[test]
    fn touch_preserves_more_attack_than_sustain() {
        let plain = excitation(0.4, 6.0, 0.0, 0.6, 400.0, -6.0);
        let soft = excitation(0.4, 6.0, 1.0, 0.6, 400.0, -6.0);
        let attack = rms(&soft[..240]) / rms(&plain[..240]);
        let sustain = rms(&soft[6000..]) / rms(&plain[6000..]);
        assert!(attack > sustain * 1.3);
    }
    #[test]
    fn controls_shape_tone_and_level_and_recover_from_bad_input() {
        let strength =
            |tone, master| rms(&excitation(0.03, 6.0, 0.55, tone, 5000.0, master)[6000..]);
        assert!(strength(1.0, -6.0) > 2.0 * strength(0.0, -6.0));
        assert!((strength(0.5, -6.0206) / strength(0.5, 0.0) - 0.5).abs() < 0.001);
        assert!(EdgeAmp::new(48000, 12.0, 0.5, f32::NAN, 0.0).is_err());
        let mut n = EdgeAmp::new(8000, 30.0, 1.0, 1.0, 6.0).unwrap();
        assert_eq!(n.latency_samples(), 1);
        for i in 0..16000 {
            for v in n.tick([if i % 2 == 0 { f32::MAX } else { -f32::MAX }, f32::NAN]) {
                assert!(v.is_finite() && v.abs() < 5.0);
            }
        }
        n.reset();
        assert_eq!(n.tick([0.0; 2]), [0.0; 2]);
    }
}
