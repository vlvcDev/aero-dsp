//! Fixed storage, integer dry-path delay with crossfaded tap changes.
pub struct DryAligner {
    ring: Vec<f32>,
    cursor: usize,
    from: usize,
    to: usize,
    desired: usize,
    remaining: u32,
    ramp: u32,
}
impl DryAligner {
    pub fn new(rate: u32, delay: usize) -> Self {
        let limit = nam_bridge::alignment::max_delay(f64::from(rate));
        let delay = delay.min(limit);
        Self {
            ring: vec![0.0; limit + 1],
            cursor: 0,
            from: delay,
            to: delay,
            desired: delay,
            remaining: 0,
            ramp: (rate / 100).max(1),
        }
    }
    pub fn set_delay(&mut self, delay: usize) {
        self.desired = delay.min(self.ring.len() - 1);
    }
    pub fn clear(&mut self) {
        self.ring.fill(0.0);
        self.cursor = 0;
    }
    pub fn tick(&mut self, input: f32) -> f32 {
        if self.remaining == 0 && self.desired != self.to {
            self.from = self.to;
            self.to = self.desired;
            self.remaining = self.ramp;
        }
        self.ring[self.cursor] = input;
        let read = |delay| self.ring[(self.cursor + self.ring.len() - delay) % self.ring.len()];
        let output = if self.remaining == 0 {
            read(self.to)
        } else {
            self.remaining -= 1;
            let blend = 1.0 - self.remaining as f32 / self.ramp as f32;
            read(self.from) * (1.0 - blend) + read(self.to) * blend
        };
        self.cursor = (self.cursor + 1) % self.ring.len();
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::{
        BypassProcessor, MonoProcessor, PedalControlProcessor, PedalParameters, SwapMailbox,
        SwappableProcessor,
    };
    use nam_bridge::alignment::{AlignmentEstimate, AlignmentStatus};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct DelayedModel {
        line: DryAligner,
        delay: u32,
        status: AlignmentStatus,
    }
    impl DelayedModel {
        fn new(delay: u32) -> Self {
            Self {
                line: DryAligner::new(48000, delay as usize),
                delay,
                status: AlignmentStatus::Accepted,
            }
        }
    }
    impl MonoProcessor for DelayedModel {
        fn dry_wet_alignment(&self) -> Option<AlignmentEstimate> {
            Some(AlignmentEstimate {
                status: self.status,
                delay_samples: self.delay,
                correlation: 1.0,
                peak_ratio: 100.0,
                lobe_ratio: 100.0,
            })
        }
        fn process(&mut self, s: &mut [f32]) {
            for x in s {
                *x = self.line.tick(*x);
            }
        }
    }
    fn input() -> Vec<f32> {
        (0..4800).map(|i| (i as f32 * 0.13).sin() * 0.1).collect()
    }
    #[test]
    fn aligned_blend_nulls_against_delayed_wet_and_endpoints_are_unchanged() {
        let source = input();
        let delayed: Vec<_> = (0..source.len())
            .map(|i| if i < 37 { 0.0 } else { source[i - 37] })
            .collect();
        for block in [37, 64, 128] {
            for mix in [0.0, 0.5, 1.0] {
                let mut p = PedalControlProcessor::new(
                    DelayedModel::new(37),
                    Arc::new(PedalParameters::new(0.0, mix, 0.0)),
                    128,
                    48000,
                );
                let mut actual = source.clone();
                for c in actual.chunks_mut(block) {
                    p.process(c);
                }
                assert_eq!(
                    actual,
                    if mix == 0.0 {
                        source.clone()
                    } else {
                        delayed.clone()
                    }
                );
            }
        }
        let mut p = BypassProcessor::new(
            PedalControlProcessor::new(
                DelayedModel::new(37),
                Arc::new(PedalParameters::new(0.0, 0.5, 0.0)),
                128,
                48000,
            ),
            Arc::new(AtomicBool::new(true)),
            48000,
        );
        let mut actual = source.clone();
        for c in actual.chunks_mut(64) {
            p.process(c);
        }
        assert_eq!(actual, source);
    }
    #[test]
    fn disabled_or_uncertain_keeps_the_original_blend() {
        let source = input();
        for status in [
            AlignmentStatus::Accepted,
            AlignmentStatus::Uncertain,
            AlignmentStatus::Inverted,
        ] {
            let mut model = DelayedModel::new(17);
            model.status = status;
            let enabled = status != AlignmentStatus::Accepted;
            let parameters = PedalParameters::new(0.0, 0.5, 0.0).with_alignment(enabled);
            let mut p = PedalControlProcessor::new(model, Arc::new(parameters), 128, 48000);
            let mut actual = source.clone();
            for c in actual.chunks_mut(64) {
                p.process(c);
            }
            for (i, x) in actual.iter().enumerate() {
                let expected = source[i] * 0.5 + if i < 17 { 0.0 } else { source[i - 17] * 0.5 };
                assert_eq!(*x, expected);
            }
        }
    }
    #[test]
    fn toggles_and_capture_swaps_use_new_delay_without_discontinuities() {
        let mailbox = Arc::new(SwapMailbox::default());
        let model = SwappableProcessor::new(DelayedModel::new(17), mailbox.clone(), 48000);
        let parameters = Arc::new(PedalParameters::new(0.0, 0.5, 0.0));
        let mut p = PedalControlProcessor::new(model, parameters.clone(), 64, 48000);
        let mut previous = 0.0_f32;
        for block in 0..250 {
            if block == 40 {
                parameters.align_enabled.store(false, Ordering::Relaxed);
            }
            if block == 70 {
                parameters.align_enabled.store(true, Ordering::Relaxed);
            }
            if block == 100 {
                mailbox.stage(DelayedModel::new(37));
            }
            let mut s: [f32; 64] =
                std::array::from_fn(|i| ((block * 64 + i) as f32 * 0.03).sin() * 0.1);
            p.process(&mut s);
            for (i, x) in s.iter().enumerate() {
                assert!(x.is_finite());
                assert!((*x - previous).abs() < 0.015, "jump at block {block}");
                previous = *x;
                if block > 160 {
                    let expected = ((block * 64 + i - 37) as f32 * 0.03).sin() * 0.1;
                    assert!((*x - expected).abs() < 1e-6, "stale delay after swap");
                }
            }
        }
    }
    #[test]
    fn changing_taps_is_bounded_and_clear_removes_old_history() {
        let mut a = DryAligner::new(48000, 37);
        for i in 0..5000 {
            if i % 50 == 0 {
                a.set_delay(if i % 100 == 0 { 10 } else { usize::MAX });
            }
            assert!(a.tick(0.1).abs() <= 0.100001);
        }
        a.clear();
        for _ in 0..1000 {
            assert_eq!(a.tick(0.0), 0.0);
        }
    }
}
