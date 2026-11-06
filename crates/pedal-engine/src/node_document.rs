//! Offline-only saved rig preparation. No file access belongs in processing.
use crate::{
    audio_file::AudioFile,
    node_rig::{Frame, Graph, PreparedGraph, Processor, ReverbNode, StereoNode},
    node_runtime::{MonoSource, NamNode},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigDocument {
    pub version: u32,
    pub sample_rate: u32,
    pub graph: Graph,
    /// Entries only for External nodes. Paths are relative to the document.
    pub processors: BTreeMap<u32, NativeProcessor>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeProcessor {
    Cabinet {
        path: String,
        duration_seconds: f32,
    },
    Nam {
        path: String,
        input: MonoSource,
    },
    Reverb {
        decay_seconds: f32,
    },
    Fuzz {
        drive_db: f32,
        bias: f32,
        tone: f32,
        mix: f32,
        output_db: f32,
    },
    Custom {
        name: String,
        library_id: String,
        revision: u32,
        asset_directory: String,
        document: Box<RigDocument>,
    },
}

struct CustomNode(PreparedGraph);
impl StereoNode for CustomNode {
    fn latency_samples(&self) -> usize {
        self.0.latency_samples()
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let mut block = [input];
        self.0.process(&mut block);
        block[0]
    }
    fn process_block(&mut self, frames: &mut [Frame]) {
        self.0.process(frames);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    delay: usize,
}
impl Budget {
    fn add_delay(&mut self, amount: usize) -> Result<(), String> {
        self.delay = self
            .delay
            .checked_add(amount)
            .ok_or("Delay budget overflow")?;
        if self.delay > 192_000 {
            return Err("Nested delay memory budget exceeded".into());
        }
        Ok(())
    }
}

fn resolve_asset(directory: &Path, path: &str) -> Result<std::path::PathBuf, String> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err("Asset paths must stay relative to the rig directory".into());
    }
    let root = directory.canonicalize().map_err(|e| e.to_string())?;
    let asset = root
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !asset.starts_with(&root) {
        return Err("Asset symlink leaves rig directory".into());
    }
    Ok(asset)
}
pub struct PreparedRig {
    pub graph: PreparedGraph,
    faults: Vec<Arc<AtomicBool>>,
}
impl PreparedRig {
    pub fn faulted(&self) -> bool {
        self.faults.iter().any(|f| f.load(Ordering::Relaxed))
    }
}
impl RigDocument {
    pub fn prepare(&self, directory: &Path) -> Result<PreparedRig, String> {
        let mut budget = Budget::default();
        self.validate_nested(self.sample_rate, 0, &mut budget)?;
        self.prepare_nested(directory, &mut budget)
    }

    fn validate_nested(&self, rate: u32, depth: usize, budget: &mut Budget) -> Result<(), String> {
        if self.version != 1 || !(8_000..=192_000).contains(&self.sample_rate) {
            return Err("Unsupported rig version or rate".into());
        }
        if self.sample_rate != rate {
            return Err("Nested rig sample rate must match parent".into());
        }
        budget.nodes = budget
            .nodes
            .checked_add(self.graph.nodes.len())
            .ok_or("Node budget overflow")?;
        if budget.nodes > 64 {
            return Err("Nested node count exceeds budget".into());
        }
        for node in &self.graph.nodes {
            if let Processor::Delay { samples } = node.processor {
                budget.add_delay(samples)?;
            }
        }
        for processor in self.processors.values() {
            if let NativeProcessor::Custom {
                asset_directory,
                document,
                ..
            } = processor
            {
                if depth >= 4 {
                    return Err("Custom node nesting exceeds four levels".into());
                }
                if !Path::new(asset_directory).is_absolute() {
                    return Err("Custom asset directory must be absolute".into());
                }
                document.validate_nested(rate, depth + 1, budget)?;
            }
        }
        Ok(())
    }

    fn prepare_nested(&self, directory: &Path, budget: &mut Budget) -> Result<PreparedRig, String> {
        // Validate structural graph before opening any assets, using latency-zero stubs.
        struct Stub;
        impl StereoNode for Stub {
            fn latency_samples(&self) -> usize {
                0
            }
            fn tick(&mut self, x: Frame) -> Frame {
                x
            }
            fn reset(&mut self) {}
        }
        let stubs = self
            .processors
            .keys()
            .map(|id| (*id, Box::new(Stub) as Box<dyn StereoNode>))
            .collect();
        self.graph.prepare_at_rate(stubs, self.sample_rate)?;
        let mut bindings: Vec<(u32, Box<dyn StereoNode>)> = Vec::new();
        let mut faults = Vec::new();
        for (id, descriptor) in &self.processors {
            let node: Box<dyn StereoNode> = match descriptor {
                NativeProcessor::Cabinet {
                    path,
                    duration_seconds,
                } => {
                    let asset = resolve_asset(directory, path)
                        .map_err(|e| format!("Cabinet node {id}: {e}"))?;
                    Box::new(crate::cabinet::CabinetNode::load(
                        &asset,
                        self.sample_rate,
                        *duration_seconds,
                    )?)
                }
                NativeProcessor::Nam { path, input } => {
                    let asset = resolve_asset(directory, path)
                        .map_err(|e| format!("NAM node {id}: {e}"))?;
                    let model = NamNode::load(&asset, self.sample_rate, *input)
                        .map_err(|e| format!("NAM node {id}: {e}"))?;
                    faults.push(model.fault_flag());
                    Box::new(model)
                }
                NativeProcessor::Reverb { decay_seconds } => {
                    Box::new(ReverbNode::new(self.sample_rate, *decay_seconds)?)
                }
                NativeProcessor::Fuzz {
                    drive_db,
                    bias,
                    tone,
                    mix,
                    output_db,
                } => Box::new(
                    crate::fuzz::FuzzNode::new(
                        self.sample_rate,
                        *drive_db,
                        *bias,
                        *tone,
                        *mix,
                        *output_db,
                    )
                    .map_err(|e| format!("Fuzz node {id}: {e}"))?,
                ),
                NativeProcessor::Custom {
                    asset_directory,
                    document,
                    ..
                } => {
                    let nested = document
                        .prepare_nested(Path::new(asset_directory), budget)
                        .map_err(|e| format!("Custom node {id}: {e}"))?;
                    faults.extend(nested.faults);
                    Box::new(CustomNode(nested.graph))
                }
            };
            bindings.push((*id, node));
        }
        // The graph compiler also allocates merge compensation delays. Account for
        // those across all embedded graphs, using the actual prepared latencies.
        let mut arrivals = BTreeMap::new();
        while arrivals.len() < self.graph.nodes.len() {
            let node = self
                .graph
                .nodes
                .iter()
                .find(|n| {
                    !arrivals.contains_key(&n.id)
                        && n.inputs.iter().all(|id| arrivals.contains_key(id))
                })
                .ok_or("Graph contains a cycle")?;
            let arrival = node
                .inputs
                .iter()
                .map(|id| arrivals[id])
                .max()
                .unwrap_or(0usize);
            for id in &node.inputs {
                budget.add_delay(arrival - arrivals[id])?;
            }
            let own = bindings
                .iter()
                .find(|(id, _)| *id == node.id)
                .map(|(_, node)| node.latency_samples())
                .unwrap_or(0);
            arrivals.insert(node.id, arrival.checked_add(own).ok_or("Latency overflow")?);
        }
        Ok(PreparedRig {
            graph: self.graph.prepare_at_rate(bindings, self.sample_rate)?,
            faults,
        })
    }

    /// Fresh processor state for every render; no unsafe NAM reset/reuse.
    /// Tail is explicit, bounded, and not an inferred decay guarantee.
    pub fn render(
        &self,
        directory: &Path,
        source: &AudioFile,
        tail_seconds: f32,
    ) -> Result<Vec<Frame>, String> {
        if source.sample_rate != self.sample_rate {
            return Err("Source and rig rates must match".into());
        }
        if !(1..=2).contains(&source.channels)
            || !source.samples.len().is_multiple_of(source.channels)
            || source.samples.iter().any(|x| !x.is_finite())
        {
            return Err("Expected finite mono/stereo audio".into());
        }
        if !tail_seconds.is_finite() || !(0.0..=30.0).contains(&tail_seconds) {
            return Err("Tail must be 0–30 seconds".into());
        }
        let frames = source.samples.len() / source.channels;
        if frames > self.sample_rate as usize * 180 {
            return Err("Prototype render limit is three minutes".into());
        }
        let mut rig = self.prepare(directory)?;
        let mut output: Vec<Frame> = source
            .samples
            .chunks_exact(source.channels)
            .map(|x| [x[0], x[source.channels - 1]])
            .collect();
        output.resize(
            frames + (tail_seconds * self.sample_rate as f32).round() as usize,
            [0.0; 2],
        );
        rig.graph.process(&mut output);
        if rig.faulted() {
            return Err("NAM processing failed; render was not exported".into());
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_rig::{Node, Processor};
    fn document() -> RigDocument {
        RigDocument {
            version: 1,
            sample_rate: 48_000,
            graph: crate::node_runtime::passthrough_graph(),
            processors: BTreeMap::new(),
        }
    }
    #[test]
    fn document_roundtrip_and_render_preserve_stereo_and_tail_length() {
        let d: RigDocument =
            serde_json::from_str(&serde_json::to_string(&document()).unwrap()).unwrap();
        let input = AudioFile {
            samples: vec![0.3, -0.2, 0.1, -0.4],
            channels: 2,
            sample_rate: 48_000,
        };
        let out = d.render(Path::new("."), &input, 0.01).unwrap();
        assert_eq!(out.len(), 482);
        assert_eq!(&out[..2], &[[0.3, -0.2], [0.1, -0.4]]);
        assert!(out[2..].iter().all(|f| *f == [0.0; 2]));
        assert!(d.render(Path::new("."), &input, f32::NAN).is_err());
    }
    #[test]
    fn validates_bindings_and_paths_before_loading() {
        let mut d = document();
        d.processors.insert(
            3,
            NativeProcessor::Nam {
                path: "../private.nam".into(),
                input: MonoSource::Left,
            },
        );
        assert!(d.prepare(Path::new(".")).is_err());
        d.graph.nodes[1].inputs = vec![3];
        d.graph.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::External,
        });
        assert!(
            d.prepare(Path::new("."))
                .err()
                .unwrap()
                .contains("relative")
        );
        d.processors
            .insert(3, NativeProcessor::Reverb { decay_seconds: 2.5 });
        assert!(d.prepare(Path::new(".")).is_ok());
    }

    fn wrapped(inner: RigDocument) -> RigDocument {
        let mut d = document();
        d.graph.nodes[1].inputs = vec![3];
        d.graph.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::External,
        });
        d.processors.insert(
            3,
            NativeProcessor::Custom {
                name: "snapshot".into(),
                library_id: "id".into(),
                revision: 1,
                asset_directory: "/tmp".into(),
                document: Box::new(inner),
            },
        );
        d
    }

    #[test]
    fn custom_roundtrip_stereo_and_independent_state() {
        let mut inner = document();
        inner.graph.nodes[1].inputs = vec![3];
        inner.graph.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::Gain { db: -6.0206 },
        });
        let d = wrapped(inner);
        let d: RigDocument = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        let mut a = d.prepare(Path::new(".")).unwrap();
        let mut b = d.prepare(Path::new(".")).unwrap();
        let mut x = [[1.0, -0.5]; 257];
        let mut y = x;
        a.graph.process(&mut x);
        for block in y.chunks_mut(13) {
            b.graph.process(block);
        }
        assert_eq!(x, y);
        assert!((x[0][0] - 0.5).abs() < 1e-6);
        assert!((x[0][1] + 0.25).abs() < 1e-6);
        a.graph.reset();
        let mut z = [[1.0, -0.5]];
        a.graph.process(&mut z);
        assert_eq!(z[0], x[0]);
    }

    #[test]
    fn nested_limits_are_enforced() {
        let mut inner = document();
        inner.sample_rate = 44_100;
        assert!(wrapped(inner).prepare(Path::new(".")).is_err());
        let mut d = document();
        for _ in 0..4 {
            d = wrapped(d);
        }
        assert!(d.prepare(Path::new(".")).is_ok());
        assert!(wrapped(d).prepare(Path::new(".")).is_err());
        let mut inner = document();
        inner.graph.nodes[1].inputs = vec![3];
        inner.graph.nodes.push(Node {
            id: 3,
            inputs: vec![1],
            processor: Processor::Delay { samples: 100_000 },
        });
        let mut outer = wrapped(inner);
        outer.graph.nodes[1].inputs = vec![4];
        outer.graph.nodes.push(Node {
            id: 4,
            inputs: vec![3],
            processor: Processor::Delay { samples: 100_000 },
        });
        assert!(
            outer
                .prepare(Path::new("."))
                .err()
                .unwrap()
                .contains("budget")
        );
    }
}
