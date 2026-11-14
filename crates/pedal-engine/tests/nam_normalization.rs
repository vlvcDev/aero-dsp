use pedal_engine::{
    EngineState,
    audio_file::{AudioFile, write_float_wav},
    dsp::{AmpProcessor, MonoProcessor, RealtimeTelemetry},
    rig::{self, RigControls},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "aero-nam-normalization-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn model(&self, name: &str, metadata: serde_json::Value) -> String {
        let path = self.0.join(name);
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "version":"0.5.0", "architecture":"Linear", "sample_rate":48000,
                "config":{"receptive_field":1,"bias":false}, "weights":[1.0],
                "metadata":metadata
            }))
            .unwrap(),
        )
        .unwrap();
        path.to_str().unwrap().to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn source() -> Vec<f32> {
    (0..9600)
        .map(|n| 0.005 * (std::f32::consts::TAU * 1000.0 * n as f32 / 48000.0).sin())
        .collect()
}
fn live(state: &EngineState, input: &[f32]) -> Vec<f32> {
    let load = |path: &Option<String>| {
        path.as_ref().map_or(AmpProcessor::Clean, |path| {
            AmpProcessor::Nam(
                pedal_engine::quality::load_nam(
                    Path::new(path),
                    48000.0,
                    64,
                    nam_bridge::NamQuality::Full,
                    false,
                    1,
                )
                .unwrap(),
            )
        })
    };
    let mut processor = rig::build(
        load(&state.pre_model),
        load(&state.model),
        None,
        RigControls::from_state(state),
        48000,
        64,
        Arc::new(RealtimeTelemetry::default()),
    );
    let mut input = input.to_vec();
    let mut output = vec![[0.0; 2]; input.len()];
    for (block, frames) in input.chunks_mut(64).zip(output.chunks_mut(64)) {
        processor.process_stereo(block, frames);
    }
    output.into_iter().flatten().collect()
}
fn power(samples: &[f32]) -> f64 {
    samples[samples.len() / 2..]
        .iter()
        .map(|&x| f64::from(x).powi(2))
        .sum()
}

#[test]
fn actual_nam_metadata_normalizes_each_wet_slot_and_matches_offline_render() {
    let fixture = Fixture::new();
    let model = fixture.model("linear.nam", serde_json::json!({"loudness":-12.0}));
    let mut state = EngineState {
        pre_model: Some(model.clone()),
        model: Some(model),
        pedal_align_enabled: false,
        pedal_normalize_enabled: false,
        amp_normalize_enabled: false,
        ..EngineState::default()
    };
    let input = source();
    let raw = live(&state, &input);
    state.pedal_normalize_enabled = true;
    state.amp_normalize_enabled = true;
    let normalized = live(&state, &input);
    let change_db = 10.0 * (power(&normalized) / power(&raw)).log10();
    assert!(
        (change_db + 12.0).abs() < 0.01,
        "two −6 dB corrections: {change_db}"
    );

    let source_path = fixture.0.join("source.wav");
    let rendered_path = fixture.0.join("rendered.wav");
    write_float_wav(&source_path, 1, 48000, input.iter().copied()).unwrap();
    pedal_engine::preview::render_source(
        &state,
        &rendered_path,
        Some(&source_path),
        true,
        None,
        None,
    )
    .unwrap();
    let rendered = AudioFile::load(&rendered_path).unwrap();
    assert_eq!(rendered.samples.len(), normalized.len());
    assert!(
        rendered
            .samples
            .iter()
            .zip(&normalized)
            .all(|(a, b)| (a - b).abs() < 1e-6)
    );

    // The Drive's pure dry endpoint must not inherit its NAM normalization.
    state.pedal_mix = 0.0;
    state.amp_normalize_enabled = false;
    let dry_on = live(&state, &input);
    state.pedal_normalize_enabled = false;
    let dry_off = live(&state, &input);
    assert_eq!(dry_on, dry_off);
}

#[test]
fn calibrated_handoff_preserves_drive_level_but_terminal_head_can_normalize() {
    let fixture = Fixture::new();
    let mut state = EngineState {
        pre_model: Some(fixture.model(
            "drive.nam",
            serde_json::json!({
            "input_level_dbu":10.0,"output_level_dbu":8.0,"loudness":-12.0}),
        )),
        model: Some(fixture.model(
            "head.nam",
            serde_json::json!({
            "input_level_dbu":6.0,"output_level_dbu":20.0,"loudness":-24.0}),
        )),
        calibration_enabled: true,
        input_reference_set: true,
        input_reference_dbu: 10.0,
        pedal_align_enabled: false,
        pedal_normalize_enabled: true,
        amp_normalize_enabled: true,
        ..EngineState::default()
    };
    let input = source();
    let both_on = live(&state, &input);
    state.pedal_normalize_enabled = false;
    let drive_off = live(&state, &input);
    assert_eq!(
        both_on, drive_off,
        "normalization must not stack with calibrated handoff"
    );
    state.amp_normalize_enabled = false;
    let raw = live(&state, &input);
    let head_db = 10.0 * (power(&both_on) / power(&raw)).log10();
    assert!(
        (head_db - 6.0).abs() < 0.01,
        "terminal head correction: {head_db}"
    );
}
