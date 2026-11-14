//! Control/audio ownership boundary for experimental node rigs.
use crate::node_rig::{Frame, Graph, PreparedGraph, Processor, StereoNode};
use crossbeam_queue::ArrayQueue;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Explicit mono source selection; output is dual mono, never fake stereo NAM.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonoSource {
    Left,
    Right,
    Average,
}

pub struct NamNode {
    model: crate::nam_oversampling::OversampledNam,
    source: MonoSource,
    failed: Arc<AtomicBool>,
}
impl NamNode {
    /// Native-rate only until fractional resampler latency is represented.
    /// `oversampling` phase-interleaves native-rate instances (1, 2, 4 or 8).
    pub fn load(
        path: &std::path::Path,
        rate: u32,
        source: MonoSource,
        oversampling: u8,
    ) -> Result<Self, String> {
        let model = crate::nam_oversampling::OversampledNam::load(
            path,
            rate as f64,
            128,
            nam_bridge::NamQuality::Full,
            false,
            oversampling,
        )
        .map_err(|e| e.to_string())?;
        if model.expected_sample_rate() != rate as f64 {
            return Err("Node NAM requires its declared native sample rate".into());
        }
        Ok(Self {
            model,
            source,
            failed: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn fault_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.failed)
    }
}
impl StereoNode for NamNode {
    // No adapter buffering at native rate. This is not capture-phase alignment.
    // Oversampling adds only the half-band filters' few-sample minimum-phase
    // group delay, reported as 0 like Tone3000's oversampler.
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let mut x = [input];
        self.process_block(&mut x);
        x[0]
    }
    // NAM Core reset/prewarm is not callback safe. Fail closed until re-prepared.
    fn reset(&mut self) {
        self.failed.store(true, Ordering::Relaxed);
    }
    fn process_block(&mut self, frames: &mut [Frame]) {
        if self.failed.load(Ordering::Relaxed) {
            frames.fill([0.0; 2]);
            return;
        }
        for frames in frames.chunks_mut(128) {
            let mut mono = [0.0; 128];
            for (sample, frame) in mono.iter_mut().zip(frames.iter()) {
                *sample = match self.source {
                    MonoSource::Left => frame[0],
                    MonoSource::Right => frame[1],
                    MonoSource::Average => frame[0] * 0.5 + frame[1] * 0.5,
                };
            }
            if !self.model.process_realtime(&mut mono[..frames.len()]) {
                self.failed.store(true, Ordering::Relaxed);
                frames.fill([0.0; 2]);
            } else {
                for (frame, sample) in frames.iter_mut().zip(mono) {
                    *frame = [sample; 2];
                }
            }
        }
    }
}

struct Exchange {
    pending: ArrayQueue<PreparedGraph>,
    retired: ArrayQueue<PreparedGraph>,
}
pub struct GraphControl {
    exchange: Arc<Exchange>,
    latency: usize,
    rate: u32,
}
impl GraphControl {
    /// Call on control thread. Rejected graphs remain owned by the caller.
    pub fn submit(&self, graph: PreparedGraph) -> Result<(), PreparedGraph> {
        if graph.latency_samples() != self.latency || graph.sample_rate() != self.rate {
            return Err(graph);
        }
        self.exchange.pending.push(graph)
    }
    /// Destructors and native resources are reclaimed here, never by processing.
    pub fn collect_retired(&self) {
        while self.exchange.retired.pop().is_some() {}
    }
}
pub struct GraphRuntime {
    active: PreparedGraph,
    exchange: Arc<Exchange>,
    gain: f32,
    step: f32,
    fading_out: bool,
}
impl GraphRuntime {
    pub fn new(active: PreparedGraph) -> (Self, GraphControl) {
        let rate = active.sample_rate();
        let latency = active.latency_samples();
        let exchange = Arc::new(Exchange {
            pending: ArrayQueue::new(1),
            retired: ArrayQueue::new(1),
        });
        (
            Self {
                active,
                exchange: Arc::clone(&exchange),
                gain: 1.0,
                step: 1.0 / (rate as f32 * 0.005),
                fading_out: false,
            },
            GraphControl {
                exchange,
                latency,
                rate,
            },
        )
    }
    /// One graph at a time, fade down/swap/fade up. No spillover guarantee.
    /// Stop audio before dropping the runtime itself on a control thread.
    pub fn process(&mut self, frames: &mut [Frame]) {
        for block in frames.chunks_mut(128) {
            if self.gain == 1.0
                && self.exchange.retired.is_empty()
                && !self.exchange.pending.is_empty()
            {
                self.fading_out = true;
            }
            if self.fading_out && self.gain == 0.0 {
                if let Some(next) = self.exchange.pending.pop() {
                    let old = std::mem::replace(&mut self.active, next);
                    // Only this runtime produces retired graphs. Capacity was
                    // reserved before fading; control can only remove entries.
                    if self.exchange.retired.push(old).is_err() {
                        unreachable!("single retirement producer");
                    }
                }
                self.fading_out = false;
            }
            self.active.process(block);
            let target = if self.fading_out { 0.0 } else { 1.0 };
            for frame in block {
                self.gain += (target - self.gain).clamp(-self.step, self.step);
                for sample in frame {
                    *sample *= self.gain;
                }
            }
        }
    }
}

/// Minimal persisted rig helper for tests and offline consumers.
pub fn passthrough_graph() -> Graph {
    use crate::node_rig::Node;
    Graph {
        version: 1,
        nodes: vec![
            Node {
                id: 1,
                inputs: vec![],
                processor: Processor::Input,
            },
            Node {
                id: 2,
                inputs: vec![1],
                processor: Processor::Output,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_rig::Node;
    fn gain(db: f32) -> PreparedGraph {
        let mut g = passthrough_graph();
        g.nodes[1].inputs = vec![3];
        g.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::Gain { db },
        });
        g.prepare().unwrap()
    }
    #[test]
    fn switches_smoothly_and_applies_backpressure_until_retired_collected() {
        let (mut runtime, control) = GraphRuntime::new(gain(0.0));
        assert!(control.submit(gain(-6.0206)).is_ok());
        assert!(control.submit(gain(-12.0)).is_err());
        let mut x = [[1.0, -1.0]; 1024];
        runtime.process(&mut x);
        assert!((x[1023][0] - 0.5).abs() < 1e-6);
        for pair in x.windows(2) {
            assert!((pair[1][0] - pair[0][0]).abs() < 0.005);
        }
        assert!(control.submit(gain(-12.0)).is_ok());
        let mut y = [[1.0, -1.0]; 1024];
        runtime.process(&mut y);
        assert!((y[1023][0] - 0.5).abs() < 1e-6);
        control.collect_retired();
        runtime.process(&mut y);
        let mut z = [[1.0; 2]; 1];
        runtime.process(&mut z);
        assert!((z[0][0] - 10.0_f32.powf(-12.0 / 20.0)).abs() < 1e-6);
        assert!(
            control
                .submit(passthrough_graph().prepare_at_rate(vec![], 96_000).unwrap())
                .is_err()
        );
    }
    #[test]
    fn old_processor_destruction_happens_only_during_control_collection() {
        struct Observed(Arc<AtomicBool>);
        impl Drop for Observed {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        impl StereoNode for Observed {
            fn latency_samples(&self) -> usize {
                0
            }
            fn reset(&mut self) {}
            fn tick(&mut self, x: Frame) -> Frame {
                x
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let mut g = passthrough_graph();
        g.nodes[1].inputs = vec![3];
        g.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::External,
        });
        let active = g
            .prepare_with(vec![(3, Box::new(Observed(Arc::clone(&dropped))))])
            .unwrap();
        let (mut runtime, control) = GraphRuntime::new(active);
        assert!(control.submit(gain(0.0)).is_ok());
        runtime.process(&mut [[0.0; 2]; 1024]);
        assert!(!dropped.load(Ordering::Relaxed));
        control.collect_retired();
        assert!(dropped.load(Ordering::Relaxed));
    }
    #[test]
    fn nam_native_block_adapter_and_explicit_downmix() {
        let path = std::env::temp_dir().join(format!("aero-node-test-{}.nam", std::process::id()));
        std::fs::write(&path, r#"{"version":"0.5.0","architecture":"Linear","sample_rate":48000,"config":{"receptive_field":1,"bias":false},"weights":[0.5]}"#).unwrap();
        let result = NamNode::load(&path, 48_000, MonoSource::Average, 1);
        std::fs::remove_file(&path).unwrap();
        let mut node = result.unwrap();
        let mut frames = [[0.2, 0.6]; 513];
        node.process_block(&mut frames);
        for frame in frames {
            assert!((frame[0] - 0.2).abs() < 1e-6);
            assert_eq!(frame[0], frame[1]);
        }
        let flag = node.fault_flag();
        node.reset();
        node.process_block(&mut frames);
        assert!(flag.load(Ordering::Relaxed));
        assert_eq!(frames, [[0.0; 2]; 513]);
    }
}
