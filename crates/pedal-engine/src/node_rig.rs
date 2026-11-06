//! Experimental stereo routing core. Preparation allocates; processing does not.
//! Not yet connected to live rig controls. Delay is intentional musical delay,
//! not reported algorithmic latency. Merge compensation handles whole-sample
//! processor latency, not frequency-dependent phase or intentional effect delays.
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

pub type Frame = [f32; 2];
const MAX_NODES: usize = 64;
const MAX_DELAY: usize = 192_000;
const BLOCK: usize = 128;

/// Control-thread handle; invalid values never reach the audio processor.
#[derive(Clone)]
pub struct GainControl(Arc<AtomicU32>);
impl GainControl {
    pub fn set_db(&self, db: f32) -> Result<(), &'static str> {
        if !db.is_finite() || !(-80.0..=24.0).contains(&db) {
            return Err("Invalid gain");
        }
        self.0
            .store(10.0_f32.powf(db / 20.0).to_bits(), Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    pub version: u32,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: u32,
    pub inputs: Vec<u32>,
    pub processor: Processor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Processor {
    /// Prepared native processor supplied by the control thread for this node ID.
    External,
    Input,
    Output,
    Gain {
        db: f32,
    },
    /// Unnormalized sum: gain staging is explicit, never silently changed.
    Mixer,
    /// In samples at the rate used to prepare this graph.
    Delay {
        samples: usize,
    },
}

/// Prepared stereo adapter. Implementations must not allocate, lock, or perform
/// I/O in tick/reset. Report constant integer algorithmic latency, excluding
/// musical delays. Fractional latency and dynamic latency are not supported yet.
pub trait StereoNode: Send {
    fn latency_samples(&self) -> usize;
    fn tick(&mut self, input: Frame) -> Frame;
    fn reset(&mut self);
    /// Override for efficient native processing. At most 128 frames per call.
    fn process_block(&mut self, frames: &mut [Frame]) {
        for frame in frames {
            *frame = self.tick(*frame);
        }
    }
}

/// Existing FDN algorithm exposed as a stereo wet-return node. Use an explicit
/// mixer/gain branch for dry/wet; predelay/tail is not compensation latency.
pub struct ReverbNode {
    left: crate::reverb::Reverb,
    right: crate::reverb::Reverb,
}
impl ReverbNode {
    pub fn new(rate: u32, decay: f32) -> Result<Self, &'static str> {
        if !(8_000..=192_000).contains(&rate)
            || !decay.is_finite()
            || !(0.3..=12.0).contains(&decay)
        {
            return Err("Invalid reverb configuration");
        }
        let mut left = crate::reverb::Reverb::new(rate);
        let mut right = crate::reverb::Reverb::with_spread(rate, 1.071);
        left.set_decay(decay);
        right.set_decay(decay);
        Ok(Self { left, right })
    }
}
impl StereoNode for ReverbNode {
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: Frame) -> Frame {
        [self.left.tick(input[0]), self.right.tick(input[1])]
    }
    fn reset(&mut self) {
        self.left.clear();
        self.right.clear();
    }
}

struct Edge {
    source: usize,
    delay: Vec<Frame>,
    cursor: usize,
}

impl Edge {
    fn tick(&mut self, mut value: Frame) -> Frame {
        if !self.delay.is_empty() {
            std::mem::swap(&mut value, &mut self.delay[self.cursor]);
            self.cursor = (self.cursor + 1) % self.delay.len();
        }
        value
    }
}

struct PreparedNode {
    id: u32,
    inputs: Vec<Edge>,
    external: Option<Box<dyn StereoNode>>,
    kind: Processor,
    gain: f32,
    gain_control: Option<GainControl>,
    target_gain: f32,
    gain_step: f32,
    ramp_remaining: usize,
    delay: Vec<Frame>,
    cursor: usize,
}

pub struct PreparedGraph {
    rate: u32,
    nodes: Vec<PreparedNode>,
    values: Vec<[Frame; BLOCK]>,
    output: usize,
    latency: usize,
    ramp_samples: usize,
}

impl Graph {
    /// Validate and compile on a control/offline thread, never the audio callback.
    pub fn prepare(&self) -> Result<PreparedGraph, String> {
        self.prepare_with(Vec::new())
    }

    /// Bind already-prepared processors by node ID. Ownership transfers on this
    /// control-thread call; errors and retired graphs must also be dropped there.
    pub fn prepare_with(
        &self,
        processors: Vec<(u32, Box<dyn StereoNode>)>,
    ) -> Result<PreparedGraph, String> {
        self.prepare_at_rate(processors, 48_000)
    }

    pub fn prepare_at_rate(
        &self,
        processors: Vec<(u32, Box<dyn StereoNode>)>,
        rate: u32,
    ) -> Result<PreparedGraph, String> {
        if !(8_000..=192_000).contains(&rate) {
            return Err("Unsupported sample rate".into());
        }
        if self.version != 1 || self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err("Unsupported graph version or node count".into());
        }
        let mut total_delay = 0usize;
        for (i, (id, processor)) in processors.iter().enumerate() {
            if processors[..i].iter().any(|(other, _)| other == id)
                || !self
                    .nodes
                    .iter()
                    .any(|n| n.id == *id && matches!(n.processor, Processor::External))
            {
                return Err("Invalid external processor binding".into());
            }
            if processor.latency_samples() > MAX_DELAY {
                return Err("Processor latency exceeds budget".into());
            }
        }
        for (i, node) in self.nodes.iter().enumerate() {
            if self.nodes[..i].iter().any(|n| n.id == node.id) {
                return Err("Duplicate node ID".into());
            }
            let valid_count = match node.processor {
                Processor::Input => node.inputs.is_empty(),
                Processor::Mixer => (2..=8).contains(&node.inputs.len()),
                _ => node.inputs.len() == 1,
            };
            if !valid_count {
                return Err("Invalid input count".into());
            }
            for (j, source) in node.inputs.iter().enumerate() {
                if node.inputs[..j].contains(source) {
                    return Err("Duplicate connection".into());
                }
                let source = self
                    .nodes
                    .iter()
                    .find(|n| n.id == *source)
                    .ok_or("Missing source node")?;
                if matches!(source.processor, Processor::Output) {
                    return Err("Output cannot feed another node".into());
                }
            }
            match node.processor {
                Processor::External if !processors.iter().any(|(id, _)| *id == node.id) => {
                    return Err("Missing prepared processor".into());
                }
                Processor::Gain { db } if !db.is_finite() || !(-80.0..=24.0).contains(&db) => {
                    return Err("Gain outside supported range".into());
                }
                Processor::Delay { samples } => {
                    total_delay = total_delay
                        .checked_add(samples)
                        .ok_or("Delay budget overflow")?;
                    if total_delay > MAX_DELAY {
                        return Err("Delay memory budget exceeded".into());
                    }
                }
                _ => {}
            }
        }
        for kind in [Processor::Input, Processor::Output] {
            if self.nodes.iter().filter(|n| n.processor == kind).count() != 1 {
                return Err("Graph requires exactly one input and one output".into());
            }
        }
        let mut order = Vec::with_capacity(self.nodes.len());
        while order.len() < self.nodes.len() {
            let next = (0..self.nodes.len()).find(|i| {
                !order.contains(i)
                    && self.nodes[*i]
                        .inputs
                        .iter()
                        .all(|id| order.iter().any(|j| self.nodes[*j].id == *id))
            });
            order.push(next.ok_or("Graph contains a cycle")?);
        }
        let output = order
            .iter()
            .position(|i| matches!(self.nodes[*i].processor, Processor::Output))
            .unwrap();
        // Reject orphan branches rather than spending callback time on inaudible nodes.
        let mut reachable = vec![false; order.len()];
        reachable[output] = true;
        for i in (0..order.len()).rev() {
            if reachable[i] {
                for id in &self.nodes[order[i]].inputs {
                    reachable[order.iter().position(|j| self.nodes[*j].id == *id).unwrap()] = true;
                }
            }
        }
        if reachable.contains(&false) {
            return Err("Disconnected node".into());
        }
        let mut latency = vec![0usize; order.len()];
        let mut compensation = Vec::with_capacity(order.len());
        for (position, index) in order.iter().enumerate() {
            let node = &self.nodes[*index];
            let sources: Vec<_> = node
                .inputs
                .iter()
                .map(|id| order.iter().position(|j| self.nodes[*j].id == *id).unwrap())
                .collect();
            let arrival = sources.iter().map(|i| latency[*i]).max().unwrap_or(0);
            let own = processors
                .iter()
                .find(|(id, _)| *id == node.id)
                .map(|(_, p)| p.latency_samples())
                .unwrap_or(0);
            latency[position] = arrival.checked_add(own).ok_or("Latency overflow")?;
            if latency[position] > MAX_DELAY {
                return Err("Path latency exceeds budget".into());
            }
            let delays: Vec<_> = sources.iter().map(|i| arrival - latency[*i]).collect();
            for delay in &delays {
                total_delay = total_delay
                    .checked_add(*delay)
                    .ok_or("Compensation budget overflow")?;
                if total_delay > MAX_DELAY {
                    return Err("Delay and compensation memory budget exceeded".into());
                }
            }
            compensation.push(delays);
        }
        let mut processors = processors;
        let nodes = order
            .iter()
            .enumerate()
            .map(|(position, i)| {
                let node = &self.nodes[*i];
                PreparedNode {
                    id: node.id,
                    gain_control: if let Processor::Gain { db } = node.processor {
                        Some(GainControl(Arc::new(AtomicU32::new(
                            10.0_f32.powf(db / 20.0).to_bits(),
                        ))))
                    } else {
                        None
                    },
                    target_gain: if let Processor::Gain { db } = node.processor {
                        10.0_f32.powf(db / 20.0)
                    } else {
                        1.0
                    },
                    gain_step: 0.0,
                    ramp_remaining: 0,
                    inputs: node
                        .inputs
                        .iter()
                        .zip(&compensation[position])
                        .map(|(id, delay)| Edge {
                            source: order.iter().position(|j| self.nodes[*j].id == *id).unwrap(),
                            delay: vec![[0.0; 2]; *delay],
                            cursor: 0,
                        })
                        .collect(),
                    external: processors
                        .iter()
                        .position(|(id, _)| *id == node.id)
                        .map(|p| processors.swap_remove(p).1),
                    kind: node.processor.clone(),
                    gain: if let Processor::Gain { db } = node.processor {
                        10.0_f32.powf(db / 20.0)
                    } else {
                        1.0
                    },
                    delay: vec![
                        [0.0; 2];
                        if let Processor::Delay { samples } = node.processor {
                            samples
                        } else {
                            0
                        }
                    ],
                    cursor: 0,
                }
            })
            .collect();
        Ok(PreparedGraph {
            rate,
            nodes,
            values: vec![[[0.0; 2]; BLOCK]; order.len()],
            output,
            latency: latency[output],
            ramp_samples: (rate as usize / 100).max(1),
        })
    }
}

impl PreparedGraph {
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
    /// Obtain on the control thread before moving the graph to audio ownership.
    pub fn gain_control(&self, id: u32) -> Option<GainControl> {
        self.nodes
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.gain_control.clone())
    }
    pub fn latency_samples(&self) -> usize {
        self.latency
    }
    pub fn reset(&mut self) {
        self.values.fill([[0.0; 2]; BLOCK]);
        for node in &mut self.nodes {
            for edge in &mut node.inputs {
                edge.delay.fill([0.0; 2]);
                edge.cursor = 0;
            }
            if let Some(p) = &mut node.external {
                p.reset();
            }
            node.delay.fill([0.0; 2]);
            node.cursor = 0;
        }
    }

    /// In-place stereo blocks; arbitrary block lengths, no heap operations.
    /// Final output limiting remains the responsibility of the host monitor bus.
    pub fn process(&mut self, frames: &mut [Frame]) {
        for frames in frames.chunks_mut(BLOCK) {
            for (i, node) in self.nodes.iter_mut().enumerate() {
                let mut scratch = [[0.0; 2]; BLOCK];
                let block = &mut scratch[..frames.len()];
                if matches!(node.kind, Processor::Input) {
                    block.copy_from_slice(frames);
                }
                for edge in &mut node.inputs {
                    for (n, value) in block.iter_mut().enumerate() {
                        for (v, x) in value.iter_mut().zip(edge.tick(self.values[edge.source][n])) {
                            *v += x;
                        }
                    }
                }
                if let Some(control) = &node.gain_control {
                    let target = f32::from_bits(control.0.load(Ordering::Relaxed));
                    if target != node.target_gain {
                        node.target_gain = target;
                        node.ramp_remaining = self.ramp_samples;
                        node.gain_step = (target - node.gain) / self.ramp_samples as f32;
                    }
                }
                for value in block.iter_mut() {
                    if node.ramp_remaining > 0 {
                        node.gain += node.gain_step;
                        node.ramp_remaining -= 1;
                        if node.ramp_remaining == 0 {
                            node.gain = node.target_gain;
                        }
                    }
                    for v in value.iter_mut() {
                        *v *= node.gain;
                        if !v.is_finite() {
                            *v = 0.0;
                        }
                    }
                    if !node.delay.is_empty() {
                        std::mem::swap(value, &mut node.delay[node.cursor]);
                        node.cursor = (node.cursor + 1) % node.delay.len();
                    }
                }
                if let Some(p) = &mut node.external {
                    p.process_block(block);
                }
                for value in block.iter_mut() {
                    for v in value {
                        if !v.is_finite() {
                            *v = 0.0;
                        }
                    }
                }
                self.values[i][..frames.len()].copy_from_slice(block);
            }
            frames.copy_from_slice(&self.values[self.output][..frames.len()]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_gain_ramp_is_bounded_and_block_independent() {
        let g = graph();
        let mut a = g.prepare().unwrap();
        let mut b = g.prepare().unwrap();
        let control = a.gain_control(2).unwrap();
        assert!(a.gain_control(1).is_none());
        assert!(control.set_db(f32::NAN).is_err());
        control.set_db(0.0).unwrap();
        b.gain_control(2).unwrap().set_db(0.0).unwrap();
        let mut x = [[1.0, -1.0]; 600];
        let mut y = x;
        a.process(&mut x);
        for chunk in y.chunks_mut(17) {
            b.process(chunk);
        }
        assert_eq!(x, y);
        // After intentional branch delay, only the smoothed gain changes.
        for pair in x[4..480].windows(2) {
            assert!((pair[1][0] - pair[0][0]).abs() < 0.002);
        }
        assert_eq!(x[479], [2.0, -2.0]);
    }
    #[test]
    fn native_adapter_uses_blocks_instead_of_single_sample_calls() {
        struct BlockOnly;
        impl StereoNode for BlockOnly {
            fn latency_samples(&self) -> usize {
                0
            }
            fn tick(&mut self, _: Frame) -> Frame {
                panic!("must use block API")
            }
            fn reset(&mut self) {}
            fn process_block(&mut self, block: &mut [Frame]) {
                assert!(block.len() <= BLOCK);
                for frame in block {
                    frame[0] *= 0.5;
                    frame[1] *= 0.5;
                }
            }
        }
        let mut g = graph();
        g.nodes[2].processor = Processor::External;
        let mut p = g.prepare_with(vec![(2, Box::new(BlockOnly))]).unwrap();
        let mut x = [[1.0, -1.0]; 513];
        p.process(&mut x);
        assert_eq!(x[0], [0.5, -0.5]);
        assert_eq!(x[512], [1.5, -1.5]);
    }
    #[test]
    fn reverb_adapter_matches_existing_algorithm_and_preserves_channels() {
        let mut node = ReverbNode::new(48_000, 2.5).unwrap();
        let mut reference = crate::reverb::Reverb::new(48_000);
        for i in 0..12_000 {
            let input = if i == 0 { 1.0 } else { 0.0 };
            assert_eq!(node.tick([input, 0.0]), [reference.tick(input), 0.0]);
        }
        node.reset();
        for _ in 0..12_000 {
            assert_eq!(node.tick([0.0; 2]), [0.0; 2]);
        }
        assert!(ReverbNode::new(0, 2.5).is_err());
    }
    struct Buffered {
        ring: Vec<Frame>,
        cursor: usize,
    }
    impl StereoNode for Buffered {
        fn latency_samples(&self) -> usize {
            self.ring.len()
        }
        fn tick(&mut self, mut x: Frame) -> Frame {
            if !self.ring.is_empty() {
                std::mem::swap(&mut x, &mut self.ring[self.cursor]);
                self.cursor = (self.cursor + 1) % self.ring.len();
            }
            x
        }
        fn reset(&mut self) {
            self.ring.fill([0.0; 2]);
            self.cursor = 0;
        }
    }
    fn buffered(samples: usize) -> Box<dyn StereoNode> {
        Box::new(Buffered {
            ring: vec![[0.0; 2]; samples],
            cursor: 0,
        })
    }
    #[test]
    fn compensates_processor_latency_without_erasing_musical_delay() {
        let mut g = graph();
        g.nodes[2].processor = Processor::External;
        let mut p = g.prepare_with(vec![(2, buffered(5))]).unwrap();
        assert_eq!(p.latency_samples(), 5);
        let mut x = [[0.0; 2]; 16];
        x[0] = [0.25, -0.5];
        for chunk in x.chunks_mut(3) {
            p.process(chunk);
        }
        // External path arrives at 5; musical delay remains 3 samples later.
        assert_eq!(x[5], [0.25, -0.5]);
        assert_eq!(x[8], [0.25, -0.5]);
        assert_eq!(x.iter().filter(|x| **x != [0.0; 2]).count(), 2);
        p.reset();
        let mut silence = [[0.0; 2]; 16];
        p.process(&mut silence);
        assert_eq!(silence, [[0.0; 2]; 16]);
    }
    #[test]
    fn parallel_and_serial_latency_accumulate_correctly() {
        let mut g = graph();
        g.nodes[2].processor = Processor::External;
        g.nodes[3].processor = Processor::External;
        g.nodes.push(Node {
            id: 11,
            inputs: vec![9],
            processor: Processor::External,
        });
        g.nodes[4].inputs = vec![11];
        let mut p = g
            .prepare_with(vec![(2, buffered(2)), (3, buffered(7)), (11, buffered(4))])
            .unwrap();
        assert_eq!(p.latency_samples(), 11);
        let mut x = [[0.0; 2]; 20];
        x[0] = [0.2, -0.3];
        p.process(&mut x);
        assert_eq!(x[11], [0.4, -0.6]);
        assert_eq!(x.iter().filter(|x| **x != [0.0; 2]).count(), 1);
    }
    #[test]
    fn bindings_and_compensation_are_bounded() {
        let mut g = graph();
        g.nodes[2].processor = Processor::External;
        assert!(g.prepare().is_err());
        assert!(g.prepare_with(vec![(999, buffered(0))]).is_err());
        assert!(
            g.prepare_with(vec![(2, buffered(0)), (2, buffered(0))])
                .is_err()
        );
        assert!(g.prepare_with(vec![(2, buffered(MAX_DELAY))]).is_err());
        assert!(g.prepare_with(vec![(2, buffered(MAX_DELAY + 1))]).is_err());
    }
    fn graph() -> Graph {
        Graph {
            version: 1,
            nodes: vec![
                Node {
                    id: 9,
                    inputs: vec![2, 3],
                    processor: Processor::Mixer,
                },
                Node {
                    id: 1,
                    inputs: vec![],
                    processor: Processor::Input,
                },
                Node {
                    id: 2,
                    inputs: vec![1],
                    processor: Processor::Gain { db: -6.0206 },
                },
                Node {
                    id: 3,
                    inputs: vec![1],
                    processor: Processor::Delay { samples: 3 },
                },
                Node {
                    id: 10,
                    inputs: vec![9],
                    processor: Processor::Output,
                },
            ],
        }
    }
    #[test]
    fn roundtrip_and_stereo_parallel_routing() {
        let g = graph();
        let decoded: Graph = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(g, decoded);
        let mut p = decoded.prepare().unwrap();
        let mut x = [[0.0; 2]; 8];
        x[0] = [1.0, -0.5];
        p.process(&mut x);
        assert!((x[0][0] - 0.5).abs() < 1e-6);
        assert!((x[0][1] + 0.25).abs() < 1e-6);
        assert_eq!(x[3], [1.0, -0.5]);
        assert_eq!(x[1], [0.0; 2]);
    }
    #[test]
    fn block_size_invariance_and_reset() {
        let g = graph();
        let mut a = g.prepare().unwrap();
        let mut b = g.prepare().unwrap();
        let mut x: Vec<_> = (0..100).map(|i| [(i as f32).sin(), 0.0]).collect();
        let mut y = x.clone();
        a.process(&mut x);
        for chunk in y.chunks_mut(7) {
            b.process(chunk);
        }
        assert_eq!(x, y);
        b.reset();
        let mut silence = [[0.0; 2]; 8];
        b.process(&mut silence);
        assert_eq!(silence, [[0.0; 2]; 8]);
    }
    #[test]
    fn rejects_invalid_graphs() {
        let g = graph();
        let mut bad = g.clone();
        bad.version = 2;
        assert!(bad.prepare().is_err());
        let mut bad = g.clone();
        bad.nodes[2].inputs = vec![9];
        assert!(bad.prepare().is_err());
        let mut bad = g.clone();
        bad.nodes[2].inputs = vec![999];
        assert!(bad.prepare().is_err());
        let mut bad = g.clone();
        bad.nodes[2].id = 1;
        assert!(bad.prepare().is_err());
        let mut bad = g.clone();
        bad.nodes[2].processor = Processor::Gain { db: f32::NAN };
        assert!(bad.prepare().is_err());
        let mut bad = g.clone();
        bad.nodes[3].processor = Processor::Delay {
            samples: usize::MAX,
        };
        assert!(bad.prepare().is_err());
        let mut bad = g;
        bad.nodes[0].inputs = vec![2, 2];
        assert!(bad.prepare().is_err());
    }
}
