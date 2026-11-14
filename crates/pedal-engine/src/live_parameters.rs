//! Control-thread parameter handles and allocation-free audio-thread smoothing.
use crate::node_rig::{Frame, GainControl, StereoNode};
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
pub type LiveControls = BTreeMap<String, ParameterControl>;
#[derive(Clone)]
pub struct ParameterControl {
    target: Target,
    min: f32,
    max: f32,
}
#[derive(Clone)]
enum Target {
    Parameter(Arc<AtomicU32>),
    Gain(GainControl),
}
impl ParameterControl {
    pub fn gain(control: GainControl) -> Self {
        Self {
            target: Target::Gain(control),
            min: -80.0,
            max: 24.0,
        }
    }
    pub fn validate(&self, value: f32) -> Result<(), String> {
        if value.is_finite() && (self.min..=self.max).contains(&value) {
            Ok(())
        } else {
            Err(format!(
                "parameter must be finite and between {} and {}",
                self.min, self.max
            ))
        }
    }
    pub fn set(&self, value: f32) -> Result<(), String> {
        self.validate(value)?;
        match &self.target {
            Target::Parameter(target) => {
                target.store(value.to_bits(), Ordering::Relaxed);
                Ok(())
            }
            Target::Gain(control) => control.set_db(value).map_err(str::to_owned),
        }
    }
}
struct Smoothed {
    name: &'static str,
    target: Arc<AtomicU32>,
    current: f32,
    destination: f32,
    step: f32,
    remaining: u32,
}
struct LiveNode {
    inner: Box<dyn StereoNode>,
    parameters: Vec<Smoothed>,
    ramp_samples: u32,
}
impl LiveNode {
    fn begin_block(&mut self) {
        for p in &mut self.parameters {
            let target = f32::from_bits(p.target.load(Ordering::Relaxed));
            if target != p.destination {
                p.destination = target;
                p.remaining = self.ramp_samples;
                p.step = (target - p.current) / self.ramp_samples as f32;
            }
        }
    }
    fn advance(&mut self) {
        for p in &mut self.parameters {
            if p.remaining != 0 {
                p.remaining -= 1;
                let next = if p.remaining == 0 {
                    p.destination
                } else {
                    (p.current + p.step)
                        .clamp(p.current.min(p.destination), p.current.max(p.destination))
                };
                if next != p.current {
                    let accepted = self.inner.set_parameter(p.name, next);
                    debug_assert!(accepted, "prepared live control rejected a bounded ramp");
                    p.current = next;
                }
            }
        }
    }
}
impl StereoNode for LiveNode {
    fn latency_samples(&self) -> usize {
        self.inner.latency_samples()
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn tick(&mut self, input: Frame) -> Frame {
        self.begin_block();
        self.advance();
        self.inner.tick(input)
    }
    fn process_block(&mut self, frames: &mut [Frame]) {
        self.begin_block();
        if self.parameters.iter().all(|p| p.remaining == 0) {
            self.inner.process_block(frames);
            return;
        }
        for frame in frames {
            self.advance();
            *frame = self.inner.tick(*frame);
        }
    }
}
type Spec = (&'static str, f32, f32);
fn specs(kind: &str) -> &'static [Spec] {
    match kind {
        "fuzz" => &[
            ("drive_db", 0., 36.),
            ("bias", -1., 1.),
            ("tone", 0., 1.),
            ("mix", 0., 1.),
            ("output_db", -36., 6.),
        ],
        "overdrive" => &[
            ("drive_db", 0., 24.),
            ("tone", 0., 1.),
            ("mix", 0., 1.),
            ("output_db", -24., 6.),
        ],
        "edge_amp" => &[
            ("drive_db", 0., 30.),
            ("tone", 0., 1.),
            ("touch", 0., 1.),
            ("master_db", -36., 6.),
        ],
        "chorus" => &[("rate_hz", 0.05, 8.), ("depth", 0., 1.), ("mix", 0., 1.)],
        "flanger" => &[
            ("rate_hz", 0.05, 5.),
            ("depth", 0., 1.),
            ("mix", 0., 1.),
            ("feedback", -0.85, 0.85),
        ],
        "tremolo" => &[("rate_hz", 0.1, 18.), ("depth", 0., 1.)],
        "octaver" => &[("mix", 0., 1.)],
        "reverb" => &[("decay_seconds", 0.3, 12.)],
        "envelope_filter" => &[
            ("sensitivity", 0., 1.),
            ("mix", 0., 1.),
            ("base_hz", 80., 2000.),
            ("resonance", 0.5, 8.),
            ("decay_ms", 20., 1000.),
        ],
        "ring_mod" => &[("frequency_hz", 1., 4000.), ("mix", 0., 1.)],
        "slicer" => &[("bpm", 30., 300.), ("depth", 0., 1.)],
        "bitcrusher" => &[("rate_hz", 1000., 48000.), ("mix", 0., 1.)],
        _ => &[],
    }
}
pub fn wrap(
    mut inner: Box<dyn StereoNode>,
    rate: u32,
    kind: &str,
    descriptor: &serde_json::Value,
) -> Result<(Box<dyn StereoNode>, LiveControls), String> {
    let mut controls = BTreeMap::new();
    let mut parameters = Vec::new();
    for &(name, min, max) in specs(kind) {
        let value = descriptor
            .get(name)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| format!("missing or invalid {name}"))? as f32;
        let target = Arc::new(AtomicU32::new(value.to_bits()));
        let control = ParameterControl {
            target: Target::Parameter(target.clone()),
            min,
            max,
        };
        control.validate(value)?;
        if !inner.set_parameter(name, value) {
            return Err(format!("{kind} does not support live parameter {name}"));
        }
        parameters.push(Smoothed {
            name,
            target,
            current: value,
            destination: value,
            step: 0.,
            remaining: 0,
        });
        controls.insert(name.to_owned(), control);
    }
    if parameters.is_empty() {
        return Ok((inner, controls));
    }
    Ok((
        Box::new(LiveNode {
            inner,
            parameters,
            ramp_samples: ((rate as u64 * 20 / 1000).max(1)) as u32,
        }),
        controls,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct State {
        value: f32,
        ticks: usize,
        blocks: usize,
        resets: usize,
    }
    struct Dummy(Arc<Mutex<State>>);
    impl StereoNode for Dummy {
        fn latency_samples(&self) -> usize {
            7
        }
        fn set_parameter(&mut self, name: &str, value: f32) -> bool {
            if name != "mix" {
                return false;
            }
            self.0.lock().unwrap().value = value;
            true
        }
        fn tick(&mut self, _: Frame) -> Frame {
            let mut s = self.0.lock().unwrap();
            s.ticks += 1;
            [s.value, s.ticks as f32]
        }
        fn reset(&mut self) {
            self.0.lock().unwrap().resets += 1;
        }
        fn process_block(&mut self, frames: &mut [Frame]) {
            self.0.lock().unwrap().blocks += 1;
            for frame in frames {
                *frame = self.tick(*frame);
            }
        }
    }
    #[test]
    fn ramps_retarget_and_delegate_without_resetting() {
        let state = Arc::new(Mutex::new(State::default()));
        let (mut node, controls) = wrap(
            Box::new(Dummy(state.clone())),
            1000,
            "octaver",
            &serde_json::json!({"mix": 0.0}),
        )
        .unwrap();
        assert_eq!(node.latency_samples(), 7);
        let mut idle = [[0.; 2]; 2];
        node.process_block(&mut idle);
        assert_eq!(state.lock().unwrap().blocks, 1);
        controls["mix"].set(1.).unwrap();
        let mut half = [[0.; 2]; 10];
        node.process_block(&mut half);
        assert!((half[9][0] - 0.5).abs() < 1e-6);
        assert!(controls["mix"].set(f32::NAN).is_err());
        controls["mix"].set(0.).unwrap();
        let mut ramp = [[0.; 2]; 20];
        node.process_block(&mut ramp);
        assert!((ramp[0][0] - 0.475).abs() < 1e-6);
        assert_eq!(ramp[19][0], 0.);
        assert_eq!(state.lock().unwrap().resets, 0);
        assert_eq!(state.lock().unwrap().ticks, 32);
        node.process_block(&mut idle);
        assert_eq!(state.lock().unwrap().blocks, 2);
    }
}
