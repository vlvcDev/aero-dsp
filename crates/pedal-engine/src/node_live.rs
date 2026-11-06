//! Live route exchange: prepare/drop graphs on control threads only.
use crate::{dsp::MonoProcessor, node_document::PreparedRig};
use crossbeam_queue::ArrayQueue;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

struct Exchange {
    pending: ArrayQueue<Option<PreparedRig>>,
    retired: ArrayQueue<Option<PreparedRig>>,
    active: AtomicBool,
    fault: AtomicBool,
    transitioning: AtomicBool,
    latency: AtomicU32,
}
#[derive(Clone)]
pub struct PatchControl {
    exchange: Arc<Exchange>,
    pub sample_rate: u32,
}
impl PatchControl {
    pub fn submit(&self, rig: Option<PreparedRig>) -> Result<(), String> {
        // Reclaim outside the audio thread, including before the next exchange.
        while self.exchange.retired.pop().is_some() {}
        if rig.as_ref().is_some_and(|r| {
            r.graph.sample_rate() != self.sample_rate || r.graph.latency_samples() > 64
        }) {
            return Err(
                "Live patch must match the device rate and add at most 64 samples of processing latency"
                    .into(),
            );
        }
        if self.exchange.transitioning.swap(true, Ordering::AcqRel) {
            return Err("Patch is still switching; try again shortly".into());
        }
        if self.exchange.pending.push(rig).is_err() {
            self.exchange.transitioning.store(false, Ordering::Release);
            return Err("Patch queue is busy".into());
        }
        Ok(())
    }
    pub fn status(&self) -> (bool, bool, bool) {
        while self.exchange.retired.pop().is_some() {}
        (
            self.exchange.active.load(Ordering::Acquire),
            self.exchange.fault.load(Ordering::Acquire),
            self.exchange.transitioning.load(Ordering::Acquire),
        )
    }
    pub fn latency_samples(&self) -> u32 {
        self.exchange.latency.load(Ordering::Acquire)
    }
}
pub struct PatchRoute<P> {
    pedalboard: P,
    active: Option<PreparedRig>,
    exchange: Arc<Exchange>,
    bypass: Arc<AtomicBool>,
    gain: f32,
    wet: f32,
    step: f32,
    fading_out: bool,
    dry_history: [f32; 65],
    dry_position: usize,
}
impl<P> PatchRoute<P> {
    pub fn new(pedalboard: P, rate: u32, bypass: Arc<AtomicBool>) -> (Self, PatchControl) {
        let exchange = Arc::new(Exchange {
            pending: ArrayQueue::new(1),
            retired: ArrayQueue::new(1),
            active: AtomicBool::new(false),
            fault: AtomicBool::new(false),
            transitioning: AtomicBool::new(false),
            latency: AtomicU32::new(0),
        });
        (
            Self {
                pedalboard,
                active: None,
                exchange: Arc::clone(&exchange),
                bypass,
                gain: 1.0,
                wet: 1.0,
                step: 1.0 / (rate as f32 * 0.005),
                fading_out: false,
                dry_history: [0.0; 65],
                dry_position: 0,
            },
            PatchControl {
                exchange,
                sample_rate: rate,
            },
        )
    }
}
impl<P: MonoProcessor> MonoProcessor for PatchRoute<P> {
    fn discontinuity(&mut self) {
        self.dry_history.fill(0.0);
        self.pedalboard.discontinuity();
        if let Some(rig) = &mut self.active {
            rig.graph.reset();
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        let mut frames = [[0.0; 2]; 128];
        for chunk in samples.chunks_mut(128) {
            self.process_stereo(chunk, &mut frames[..chunk.len()]);
            for (sample, frame) in chunk.iter_mut().zip(&frames) {
                *sample = (frame[0] + frame[1]) * 0.5;
            }
        }
    }
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        for (input, out) in samples.chunks_mut(128).zip(output.chunks_mut(128)) {
            if self.gain == 1.0
                && !self.exchange.pending.is_empty()
                && self.exchange.retired.is_empty()
            {
                self.fading_out = true;
            }
            if self.fading_out && self.gain == 0.0 {
                if let Some(next) = self.exchange.pending.pop() {
                    let old = std::mem::replace(&mut self.active, next);
                    self.dry_history.fill(0.0);
                    self.dry_position = 0;
                    self.exchange.latency.store(
                        self.active
                            .as_ref()
                            .map_or(0, |r| r.graph.latency_samples() as u32),
                        Ordering::Release,
                    );
                    if self.exchange.retired.push(old).is_err() {
                        unreachable!("single retirement producer");
                    }
                    self.exchange
                        .active
                        .store(self.active.is_some(), Ordering::Release);
                    self.exchange.fault.store(false, Ordering::Release);
                }
                self.fading_out = false;
            }
            if let Some(rig) = &mut self.active {
                for (frame, sample) in out.iter_mut().zip(input.iter()) {
                    *frame = [*sample; 2];
                }
                rig.graph.process(out);
                let fault = rig.faulted()
                    || self.exchange.fault.load(Ordering::Relaxed)
                    || out.iter().flatten().any(|sample| !sample.is_finite());
                self.exchange.fault.store(fault, Ordering::Release);
                if fault {
                    out.fill([0.0; 2]);
                }
                let target_wet = if self.bypass.load(Ordering::Relaxed) {
                    0.0
                } else {
                    1.0
                };
                for (frame, dry) in out.iter_mut().zip(input.iter()) {
                    self.dry_history[self.dry_position] = *dry;
                    let delayed_dry = self.dry_history
                        [(self.dry_position + 65 - rig.graph.latency_samples()) % 65];
                    self.dry_position = (self.dry_position + 1) % 65;
                    self.wet += (target_wet - self.wet).clamp(-self.step, self.step);
                    for sample in frame {
                        *sample = delayed_dry * (1.0 - self.wet) + *sample * self.wet;
                    }
                }
            } else {
                self.pedalboard.process_stereo(input, out);
            }
            let target = if self.fading_out { 0.0 } else { 1.0 };
            for frame in out {
                self.gain += (target - self.gain).clamp(-self.step, self.step);
                for sample in frame {
                    *sample *= self.gain;
                }
            }
            if self.gain == 1.0 && self.exchange.pending.is_empty() {
                self.exchange.transitioning.store(false, Ordering::Release);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_document::RigDocument;
    struct Board;
    impl MonoProcessor for Board {
        fn process(&mut self, x: &mut [f32]) {
            for s in x {
                *s *= 0.25;
            }
        }
    }
    fn rig() -> PreparedRig {
        let json = r#"{"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[{"id":1,"inputs":[],"processor":{"type":"input"}},{"id":3,"inputs":[1],"processor":{"type":"gain","db":-6.0206}},{"id":2,"inputs":[3],"processor":{"type":"output"}}]},"processors":{}}"#;
        serde_json::from_str::<RigDocument>(json)
            .unwrap()
            .prepare(std::path::Path::new("."))
            .unwrap()
    }
    #[test]
    fn fuzz_latency_is_reported_and_bypass_uses_matching_delay() {
        let document = serde_json::json!({"version":1,"sample_rate":48000,
          "graph":{"version":1,"nodes":[
            {"id":1,"inputs":[],"processor":{"type":"input"}},
            {"id":3,"inputs":[1],"processor":{"type":"external"}},
            {"id":2,"inputs":[3],"processor":{"type":"output"}}]},
          "processors":{"3":{"type":"fuzz","drive_db":18,"bias":0,"tone":0.5,"mix":0,"output_db":0}}});
        let rig = serde_json::from_value::<RigDocument>(document)
            .unwrap()
            .prepare(std::path::Path::new("."))
            .unwrap();
        let bypass = Arc::new(AtomicBool::new(false));
        let (mut route, control) = PatchRoute::new(Board, 48000, Arc::clone(&bypass));
        control.submit(Some(rig)).unwrap();
        route.process_stereo(&mut [0.0; 1024], &mut [[0.0; 2]; 1024]);
        assert_eq!(control.latency_samples(), 1);
        let mut output = [[0.0; 2]; 4];
        route.process_stereo(&mut [1.0, 0.0, 0.0, 0.0], &mut output);
        assert_eq!(output, [[0.0; 2], [1.0; 2], [0.0; 2], [0.0; 2]]);
        bypass.store(true, Ordering::Relaxed);
        route.process_stereo(&mut [0.0; 1024], &mut [[0.0; 2]; 1024]);
        route.process_stereo(&mut [1.0, 0.0, 0.0, 0.0], &mut output);
        assert_eq!(output, [[0.0; 2], [1.0; 2], [0.0; 2], [0.0; 2]]);
    }
    #[test]
    fn routes_guitar_returns_to_board_and_respects_bypass() {
        let bypass = Arc::new(AtomicBool::new(false));
        let (mut route, control) = PatchRoute::new(Board, 48000, Arc::clone(&bypass));
        control.submit(Some(rig())).unwrap();
        assert!(control.submit(None).is_err());
        let mut input = [1.0; 1024];
        let mut output = [[0.0; 2]; 1024];
        route.process_stereo(&mut input, &mut output);
        assert!((output[1023][0] - 0.5).abs() < 1e-5);
        assert_eq!(control.status(), (true, false, false));
        bypass.store(true, Ordering::Relaxed);
        route.process_stereo(&mut [1.0; 1024], &mut output);
        assert_eq!(output[1023], [1.0; 2]);
        control.submit(None).unwrap();
        route.process_stereo(&mut [1.0; 1024], &mut output);
        assert_eq!(output[1023], [0.25; 2]);
        assert_eq!(control.status(), (false, false, false));
    }
}
