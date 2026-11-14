//! Live route exchange: prepare/drop graphs on control threads only.
use crate::{
    dsp::MonoProcessor,
    node_document::{PreparedRig, RigDocument},
};
use crossbeam_queue::ArrayQueue;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
};

struct LiveSession {
    revision: u64,
    controls: crate::live_parameters::LiveControls,
    structure: serde_json::Value,
    directory: std::path::PathBuf,
}

fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128 - 1) as u64;
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
        Some(last.saturating_add(1).max(now))
    })
    .unwrap()
    .saturating_add(1)
    .max(now)
}

struct Exchange {
    pending: ArrayQueue<Option<PreparedRig>>,
    retired: ArrayQueue<Option<PreparedRig>>,
    active: AtomicBool,
    fault: AtomicBool,
    transitioning: AtomicBool,
    latency: AtomicU32,
    revision: AtomicU64,
}
#[derive(Clone)]
pub struct PatchControl {
    exchange: Arc<Exchange>,
    pub sample_rate: u32,
    // This mutex and all JSON/registry work are control-thread-only.
    session: Arc<Mutex<Option<LiveSession>>>,
}
impl PatchControl {
    pub fn submit(&self, mut rig: Option<PreparedRig>) -> Result<u64, String> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| "Live control lock failed")?;
        // Reclaim outside the audio thread, including before the next exchange.
        while self.exchange.retired.pop().is_some() {}
        if rig.as_ref().is_some_and(|r| {
            r.graph.sample_rate() != self.sample_rate
                || r.graph.latency_samples() > self.sample_rate as usize / 2
        }) {
            return Err(
                "Live patch must match the device rate and add at most half a second of processing latency"
                    .into(),
            );
        }
        if self.exchange.transitioning.swap(true, Ordering::AcqRel) {
            return Err("Patch is still switching; try again shortly".into());
        }
        let revision = if rig.is_some() { next_revision() } else { 0 };
        let next = rig.as_mut().map(|r| {
            r.revision = revision;
            LiveSession {
                revision,
                controls: r.controls.clone(),
                structure: r.structure.clone(),
                directory: r.directory.clone(),
            }
        });
        if self.exchange.pending.push(rig).is_err() {
            self.exchange.transitioning.store(false, Ordering::Release);
            return Err("Patch queue is busy".into());
        }
        *session = next;
        Ok(revision)
    }
    pub fn revision(&self) -> Option<String> {
        match self.exchange.revision.load(Ordering::Acquire) {
            0 => None,
            value => Some(value.to_string()),
        }
    }
    pub fn update(
        &self,
        revision: &str,
        document: &RigDocument,
        directory: &std::path::Path,
    ) -> Result<(), String> {
        let session = self
            .session
            .lock()
            .map_err(|_| "Live control lock failed")?;
        let session = session.as_ref().ok_or("No live rig; use Apply to guitar")?;
        if self.exchange.transitioning.load(Ordering::Acquire) {
            return Err("Rig is still switching; wait for activation".into());
        }
        if self.exchange.fault.load(Ordering::Acquire) {
            return Err("Rig is muted after a fault; use Apply to guitar".into());
        }
        if revision != session.revision.to_string()
            || self.exchange.revision.load(Ordering::Acquire) != session.revision
        {
            return Err(
                "Active rig changed; use Apply to guitar before editing live controls".into(),
            );
        }
        if directory != session.directory {
            return Err("Asset folder changed; use Apply to guitar".into());
        }
        let (structure, values) = document.live_snapshot(&session.controls)?;
        if structure != session.structure {
            return Err(
                "Rig structure or an apply-only setting changed; use Apply to guitar".into(),
            );
        }
        // The complete update has passed validation. Publication only writes
        // bounded numeric targets; running DSP history is never replaced.
        for (control, value) in session.controls.values().zip(values) {
            control.set(value)?;
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
    dry_history: Vec<f32>,
    dry_position: usize,
    /// 1.0 disables. The classic pedalboard has its own blocker in
    /// `SafetyProcessor`; Patch Lab graphs bypass it, so block NAM/rig DC here,
    /// before the swap fade, so a new patch fades in already centred.
    dc_coefficient: f32,
    dc_history: [[f32; 2]; 2],
    /// Input-floor squelch for Patch Lab graphs (the classic pedalboard has
    /// its own in the effect chain). The bypass dry path stays raw.
    squelch: Option<(
        crate::squelch::Squelch,
        Arc<crate::effects::EffectsParameters>,
    )>,
}
impl<P> PatchRoute<P> {
    /// Feed active patch graphs exact zeros at the interface noise floor.
    pub fn with_squelch(
        mut self,
        parameters: Arc<crate::effects::EffectsParameters>,
        rate: u32,
    ) -> Self {
        self.squelch = Some((crate::squelch::Squelch::new(rate), parameters));
        self
    }

    #[cfg(test)]
    fn without_dc_blocker(mut self) -> Self {
        self.dc_coefficient = 1.0;
        self
    }
    pub fn new(pedalboard: P, rate: u32, bypass: Arc<AtomicBool>) -> (Self, PatchControl) {
        let exchange = Arc::new(Exchange {
            pending: ArrayQueue::new(1),
            retired: ArrayQueue::new(1),
            active: AtomicBool::new(false),
            fault: AtomicBool::new(false),
            transitioning: AtomicBool::new(false),
            latency: AtomicU32::new(0),
            revision: AtomicU64::new(0),
        });
        (
            Self {
                pedalboard,
                active: None,
                exchange: Arc::clone(&exchange),
                bypass,
                gain: 1.0,
                wet: 1.0,
                step: 1.0 / (rate as f32 * crate::dsp::TRANSITION_SECONDS),
                fading_out: false,
                dry_history: vec![0.0; rate as usize / 2 + 1],
                dry_position: 0,
                dc_coefficient: crate::dsp::dc_blocker_coefficient(rate),
                dc_history: [[0.0; 2]; 2],
                squelch: None,
            },
            PatchControl {
                exchange,
                sample_rate: rate,
                session: Arc::new(Mutex::new(None)),
            },
        )
    }
}
impl<P: MonoProcessor> MonoProcessor for PatchRoute<P> {
    fn discontinuity(&mut self) {
        self.dry_history.fill(0.0);
        self.dc_history = [[0.0; 2]; 2];
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
                    if old.is_none() {
                        // Stale state from an earlier patch must not leak in.
                        self.dc_history = [[0.0; 2]; 2];
                    }
                    self.dry_history.fill(0.0);
                    self.dry_position = 0;
                    self.exchange.revision.store(
                        self.active.as_ref().map_or(0, |rig| rig.revision),
                        Ordering::Release,
                    );
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
                let mut graph_input = [0.0_f32; 128];
                let graph_input = &mut graph_input[..input.len()];
                graph_input.copy_from_slice(input);
                if let Some((squelch, parameters)) = &mut self.squelch {
                    let (enabled, threshold) = parameters.squelch_settings();
                    squelch.process(
                        enabled,
                        threshold,
                        &parameters.input_floor_meter,
                        graph_input,
                    );
                }
                for (frame, sample) in out.iter_mut().zip(graph_input.iter()) {
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
                if self.dc_coefficient != 1.0 {
                    let c = self.dc_coefficient;
                    for frame in out.iter_mut() {
                        for (sample, [x1, y1]) in frame.iter_mut().zip(&mut self.dc_history) {
                            let y = c * (*y1 + (*sample - *x1));
                            (*x1, *y1) = (*sample, y);
                            *sample = y;
                        }
                    }
                }
                let target_wet = if self.bypass.load(Ordering::Relaxed) {
                    0.0
                } else {
                    1.0
                };
                for (frame, dry) in out.iter_mut().zip(input.iter()) {
                    self.dry_history[self.dry_position] = *dry;
                    let delayed_dry = self.dry_history[(self.dry_position
                        + self.dry_history.len()
                        - rig.graph.latency_samples())
                        % self.dry_history.len()];
                    self.dry_position = (self.dry_position + 1) % self.dry_history.len();
                    self.wet += (target_wet - self.wet).clamp(-self.step, self.step);
                    let wet = crate::dsp::smooth_ramp(self.wet);
                    for sample in frame {
                        *sample = delayed_dry * (1.0 - wet) + *sample * wet;
                    }
                }
            } else {
                self.pedalboard.process_stereo(input, out);
            }
            let target = if self.fading_out { 0.0 } else { 1.0 };
            for frame in out {
                self.gain += (target - self.gain).clamp(-self.step, self.step);
                let gain = crate::dsp::smooth_ramp(self.gain);
                for sample in frame {
                    *sample *= gain;
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
    fn editable_gain() -> RigDocument {
        serde_json::from_value(serde_json::json!({"version":1,"sample_rate":48000,
          "graph":{"version":1,"nodes":[
            {"id":1,"inputs":[],"processor":{"type":"input"}},
            {"id":3,"inputs":[1],"processor":{"type":"gain","db":-6.0206}},
            {"id":2,"inputs":[3],"processor":{"type":"output"}}]},"processors":{}}))
        .unwrap()
    }
    #[test]
    fn updates_live_gain_without_swapping_and_rejects_stale_or_structural_changes() {
        let directory = std::env::current_dir().unwrap();
        let mut doc = editable_gain();
        let (route, control) = PatchRoute::new(Board, 48000, Arc::new(AtomicBool::new(false)));
        let mut route = route.without_dc_blocker();
        let rev = control
            .submit(Some(doc.prepare(&directory).unwrap()))
            .unwrap()
            .to_string();
        assert!(
            control.update(&rev, &doc, &directory).is_err(),
            "not active yet"
        );
        route.process_stereo(&mut [1.; 2048], &mut [[0.; 2]; 2048]);
        assert_eq!(control.revision().as_deref(), Some(rev.as_str()));
        doc.graph.nodes[1].processor = crate::node_rig::Processor::Gain { db: 0. };
        control.update(&rev, &doc, &directory).unwrap();
        assert_eq!(control.status(), (true, false, false));
        let mut ramp = [[0.; 2]; 2048];
        route.process_stereo(&mut [1.; 2048], &mut ramp);
        assert!(
            ramp[0][0] > 0.49 && ramp[0][0] < 0.51,
            "must not mute/restart graph"
        );
        assert_eq!(ramp[2047], [1.; 2]);
        assert_eq!(control.revision().as_deref(), Some(rev.as_str()));
        assert!(control.update("stale", &doc, &directory).is_err());
        doc.graph.nodes[1].processor = crate::node_rig::Processor::Gain { db: 99. };
        assert!(control.update(&rev, &doc, &directory).is_err());
        doc.graph.nodes[1].processor = crate::node_rig::Processor::Delay { samples: 100 };
        assert!(control.update(&rev, &doc, &directory).is_err());
        route.process_stereo(&mut [1.; 2048], &mut ramp);
        assert!(ramp.iter().all(|frame| *frame == [1.; 2]));
        control.submit(None).unwrap();
        route.process_stereo(&mut [1.; 2048], &mut ramp);
        assert_eq!(control.revision(), None);
        assert!(control.update(&rev, &editable_gain(), &directory).is_err());
    }
    #[test]
    fn nested_updates_validate_entire_request_before_any_control_changes() {
        let directory = std::env::current_dir().unwrap();
        let child = serde_json::to_value(editable_gain()).unwrap();
        let mut outer = serde_json::json!({"version":1,"sample_rate":48000,
          "graph":{"version":1,"nodes":[
            {"id":1,"inputs":[],"processor":{"type":"input"}},
            {"id":3,"inputs":[1],"processor":{"type":"external"}},
            {"id":4,"inputs":[3],"processor":{"type":"gain","db":-6.0206}},
            {"id":2,"inputs":[4],"processor":{"type":"output"}}]},
          "processors":{"3":{"type":"custom","name":"Nested","library_id":"nested",
            "revision":1,"asset_directory":directory,"document":child}}});
        let doc: RigDocument = serde_json::from_value(outer.clone()).unwrap();
        let (route, control) = PatchRoute::new(Board, 48000, Arc::new(AtomicBool::new(false)));
        let mut route = route.without_dc_blocker();
        let rev = control
            .submit(Some(doc.prepare(&directory).unwrap()))
            .unwrap()
            .to_string();
        route.process_stereo(&mut [1.; 2048], &mut [[0.; 2]; 2048]);
        outer["graph"]["nodes"][2]["processor"]["db"] = serde_json::json!(0);
        outer["processors"]["3"]["document"]["graph"]["nodes"][1]["processor"]["db"] =
            serde_json::json!(99);
        let bad = serde_json::from_value(outer.clone()).unwrap();
        assert!(control.update(&rev, &bad, &directory).is_err());
        let mut out = [[0.; 2]; 2048];
        route.process_stereo(&mut [1.; 2048], &mut out);
        assert!(
            (out[2047][0] - 0.25).abs() < 1e-5,
            "valid prefix must not be applied"
        );
        outer["processors"]["3"]["document"]["graph"]["nodes"][1]["processor"]["db"] =
            serde_json::json!(0);
        control
            .update(&rev, &serde_json::from_value(outer).unwrap(), &directory)
            .unwrap();
        route.process_stereo(&mut [1.; 2048], &mut out);
        assert_eq!(out[2047], [1.; 2]);
    }
    #[test]
    fn changing_reverb_decay_preserves_an_existing_tail() {
        let directory = std::env::current_dir().unwrap();
        let mut data = serde_json::to_value(editable_gain()).unwrap();
        data["graph"]["nodes"][1]["processor"] = serde_json::json!({"type":"external"});
        data["processors"]["3"] = serde_json::json!({"type":"reverb","decay_seconds":2.5});
        let doc: RigDocument = serde_json::from_value(data.clone()).unwrap();
        let (route, control) = PatchRoute::new(Board, 48000, Arc::new(AtomicBool::new(false)));
        let mut route = route.without_dc_blocker();
        let rev = control
            .submit(Some(doc.prepare(&directory).unwrap()))
            .unwrap()
            .to_string();
        route.process_stereo(&mut [0.; 2048], &mut [[0.; 2]; 2048]);
        let mut impulse = [0.; 4096];
        impulse[0] = 1.;
        route.process_stereo(&mut impulse, &mut [[0.; 2]; 4096]);
        data["processors"]["3"]["decay_seconds"] = serde_json::json!(8);
        control
            .update(&rev, &serde_json::from_value(data).unwrap(), &directory)
            .unwrap();
        let mut tail = [[0.; 2]; 4096];
        route.process_stereo(&mut [0.; 4096], &mut tail);
        assert!(
            tail.iter().flatten().map(|s| s * s).sum::<f32>() > 0.001,
            "a knob change must not clear the reverb's stored audio"
        );
        assert_eq!(control.status(), (true, false, false));
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
        let (route, control) = PatchRoute::new(Board, 48000, Arc::clone(&bypass));
        let mut route = route.without_dc_blocker();
        control.submit(Some(rig)).unwrap();
        route.process_stereo(&mut [0.0; 2048], &mut [[0.0; 2]; 2048]);
        assert_eq!(control.latency_samples(), 1);
        let mut output = [[0.0; 2]; 4];
        route.process_stereo(&mut [1.0, 0.0, 0.0, 0.0], &mut output);
        assert_eq!(output, [[0.0; 2], [1.0; 2], [0.0; 2], [0.0; 2]]);
        bypass.store(true, Ordering::Relaxed);
        route.process_stereo(&mut [0.0; 2048], &mut [[0.0; 2]; 2048]);
        route.process_stereo(&mut [1.0, 0.0, 0.0, 0.0], &mut output);
        assert_eq!(output, [[0.0; 2], [1.0; 2], [0.0; 2], [0.0; 2]]);
    }
    #[test]
    fn long_latency_bypass_preserves_compensation() {
        let document = serde_json::json!({"version":1,"sample_rate":48000,
          "graph":{"version":1,"nodes":[
            {"id":1,"inputs":[],"processor":{"type":"input"}},
            {"id":3,"inputs":[1],"processor":{"type":"external"}},
            {"id":2,"inputs":[3],"processor":{"type":"output"}}]},"processors":{"3":{"type":"transpose","semitones":-2,"quality":"fast"}}});
        let rig = serde_json::from_value::<RigDocument>(document)
            .unwrap()
            .prepare(std::path::Path::new("."))
            .unwrap();
        let bypass = Arc::new(AtomicBool::new(true));
        let (route, control) = PatchRoute::new(Board, 48000, bypass);
        let mut route = route.without_dc_blocker();
        control.submit(Some(rig)).unwrap();
        route.process_stereo(&mut [0.0; 4096], &mut [[0.0; 2]; 4096]);
        assert_eq!(control.latency_samples(), 2048);
        let mut input = vec![0.0; 2050];
        input[0] = 1.0;
        let mut output = vec![[0.0; 2]; input.len()];
        route.process_stereo(&mut input, &mut output);
        assert_eq!(output[2048], [1.0; 2]);
        assert!(
            output
                .iter()
                .enumerate()
                .all(|(i, f)| i == 2048 || *f == [0.0; 2])
        );
    }
    #[test]
    fn routes_guitar_returns_to_board_and_respects_bypass() {
        let bypass = Arc::new(AtomicBool::new(false));
        let (route, control) = PatchRoute::new(Board, 48000, Arc::clone(&bypass));
        let mut route = route.without_dc_blocker();
        control.submit(Some(rig())).unwrap();
        assert!(control.submit(None).is_err());
        let mut input = [1.0; 2048];
        let mut output = [[0.0; 2]; 2048];
        route.process_stereo(&mut input, &mut output);
        assert!((output[2047][0] - 0.5).abs() < 1e-5);
        assert_eq!(control.status(), (true, false, false));
        bypass.store(true, Ordering::Relaxed);
        route.process_stereo(&mut [1.0; 2048], &mut output);
        assert_eq!(output[2047], [1.0; 2]);
        control.submit(None).unwrap();
        route.process_stereo(&mut [1.0; 2048], &mut output);
        assert_eq!(output[2047], [0.25; 2]);
        assert_eq!(control.status(), (false, false, false));
    }
    #[test]
    fn patch_output_blocks_dc_but_keeps_low_b_string() {
        let (mut route, control) = PatchRoute::new(Board, 48000, Arc::new(AtomicBool::new(false)));
        control.submit(Some(rig())).unwrap();
        route.process_stereo(&mut [0.0; 2048], &mut [[0.0; 2]; 2048]);
        // A rig emitting a 0.5 DC offset (input 1.0 through -6 dB) settles to zero.
        let mut output = vec![[0.0; 2]; 96_000];
        route.process_stereo(&mut vec![1.0; 96_000], &mut output);
        assert!(output[95_999][0].abs() < 1e-3, "{:?}", output[95_999]);
        // B1 on a 7-string (61.7 Hz) keeps its level (-6 dB gain node -> 0.5).
        let mut tone: Vec<f32> = (0..96_000)
            .map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 61.735 / 48_000.0).sin())
            .collect();
        route.process_stereo(&mut tone, &mut output);
        let peak = output[48_000..]
            .iter()
            .map(|f| f[0].abs())
            .fold(0.0, f32::max);
        assert!((peak - 0.5).abs() < 0.005, "{peak}");
    }
}
