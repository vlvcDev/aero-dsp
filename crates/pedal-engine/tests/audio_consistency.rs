use pedal_engine::{
    EngineState,
    audio_file::{AudioFile, write_float_wav},
    calibration::{CalibratedChain, CalibrationParameters},
    dsp::{AmpProcessor, MonoProcessor, RealtimeTelemetry, SafetyProcessor},
    riff_vault::{self, RiffVault, RiffVaultProcessor},
    rig::{self, RigControls},
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

fn directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "aero-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}

#[test]
fn dry_alignment_live_and_render_agree_and_reduce_blend_cancellation() {
    let root = directory("dry-alignment-parity");
    let model = root.join("delay.nam");
    let mut weights = vec![0.0; 38];
    weights[37] = 1.0;
    std::fs::write(
        &model,
        serde_json::to_vec(&serde_json::json!({
            "version":"0.5.0", "architecture":"Linear", "sample_rate":48000,
            "config":{"receptive_field":38,"bias":false},"weights":weights
        }))
        .unwrap(),
    )
    .unwrap();
    let source = root.join("input.wav");
    // A 37-sample delay is half a cycle here: unaligned 50% mix cancels.
    let input: Vec<_> = (0..4800)
        .map(|i| (i as f32 * std::f32::consts::PI / 37.0).sin() * 0.1)
        .collect();
    write_float_wav(&source, 1, 48000, input.iter().copied()).unwrap();
    let mut state = EngineState {
        bypassed: false,
        pre_bypassed: false,
        pre_model: Some(model.to_string_lossy().into_owned()),
        pedal_mix: 0.5,
        ..Default::default()
    };
    let mut powers = Vec::new();
    for enabled in [true, false] {
        state.pedal_align_enabled = enabled;
        let pre = pedal_engine::quality::load_nam(
            &model,
            48000.0,
            64,
            nam_bridge::NamQuality::Full,
            true,
        )
        .unwrap();
        assert_eq!(pre.alignment().unwrap().applied_delay(), 37);
        let mut live = rig::build(
            AmpProcessor::Nam(pre),
            AmpProcessor::Clean,
            None,
            RigControls::from_state(&state),
            48000,
            64,
            Arc::new(RealtimeTelemetry::default()),
        );
        let mut mono = input.clone();
        let mut stereo = vec![[0.0; 2]; input.len()];
        for (x, y) in mono.chunks_mut(37).zip(stereo.chunks_mut(37)) {
            live.process_stereo(x, y);
        }
        let path = root.join("render.wav");
        pedal_engine::preview::render_source(&state, &path, Some(&source), true, None, None)
            .unwrap();
        let rendered = AudioFile::load(&path).unwrap();
        assert!(
            rendered
                .samples
                .iter()
                .zip(stereo.iter().flatten())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
        powers.push(rendered.samples[2000..].iter().map(|x| x * x).sum::<f32>());
    }
    assert!(
        powers[0] > 1000.0 * powers[1],
        "alignment should restore the blended signal"
    );
    for name in ["delay.nam", "input.wav", "render.wav"] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn nam_eq_matches_between_live_graph_and_render_with_real_models() {
    let root = directory("nam-eq-parity");
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models/lstm.nam");
    let source = root.join("input.wav");
    let input: Vec<_> = (0..4800).map(|i| (i as f32 * 0.08).sin() * 0.02).collect();
    write_float_wav(&source, 1, 48000, input.iter().copied()).unwrap();
    let mut state = EngineState {
        bypassed: false,
        pre_bypassed: false,
        amp_bypassed: false,
        pre_model: Some(model.to_string_lossy().into_owned()),
        model: Some(model.to_string_lossy().into_owned()),
        amp_volume_db: -12.0,
        ..Default::default()
    };
    state.pedal_eq.bands[1].gain_db = -8.0;
    state.amp_eq.bands[4].gain_db = 5.0;
    for pre in [false, true] {
        state.pedal_eq.pre = pre;
        state.amp_eq.pre = !pre;
        let mut live = rig::build(
            AmpProcessor::Nam(nam_bridge::NamModel::load(&model, 48000.0, 64).unwrap()),
            AmpProcessor::Nam(nam_bridge::NamModel::load(&model, 48000.0, 64).unwrap()),
            None,
            RigControls::from_state(&state),
            48000,
            64,
            Arc::new(RealtimeTelemetry::default()),
        );
        let mut mono = input.clone();
        let mut stereo = vec![[0.0; 2]; input.len()];
        for (x, y) in mono.chunks_mut(37).zip(stereo.chunks_mut(37)) {
            live.process_stereo(x, y);
        }
        let path = root.join("render.wav");
        pedal_engine::preview::render_source(&state, &path, Some(&source), true, None, None)
            .unwrap();
        let rendered = AudioFile::load(&path).unwrap();
        assert_eq!(rendered.samples.len(), stereo.len() * 2);
        assert!(
            rendered
                .samples
                .iter()
                .zip(stereo.iter().flatten())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
    }
    for name in ["input.wav", "render.wav"] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn nam_render_size_override_changes_render_not_live_preference() {
    let root = directory("nam-render-size");
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models/A2.nam");
    let source = root.join("input.wav");
    write_float_wav(
        &source,
        1,
        48000,
        (0..4800).map(|i| (i as f32 * 0.04).sin() * 0.1),
    )
    .unwrap();
    let mut state = EngineState {
        bypassed: false,
        amp_bypassed: false,
        model: Some(model.to_string_lossy().into_owned()),
        nam_quality: 0,
        render_nam_quality: 2,
        ..Default::default()
    };
    let render = |state: &EngineState, name: &str| {
        let path = root.join(name);
        pedal_engine::preview::render_source(state, &path, Some(&source), true, None, None)
            .unwrap();
        AudioFile::load(&path).unwrap().samples
    };
    let lite = render(&state, "lite.wav");
    state.render_nam_quality = 1;
    let full = render(&state, "full.wav");
    assert_eq!(state.nam_quality, 0);
    assert_eq!(lite.len(), full.len());
    assert!(lite.iter().zip(&full).any(|(a, b)| (a - b).abs() > 1e-6));
    state.nam_quality = 1;
    state.render_nam_quality = 2;
    assert_eq!(full, render(&state, "same-full.wav"));
    state.render_nam_quality = 0;
    assert_eq!(lite, render(&state, "override-lite.wav"));
    for name in [
        "input.wav",
        "lite.wav",
        "full.wav",
        "same-full.wav",
        "override-lite.wav",
    ] {
        std::fs::remove_file(root.join(name)).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn stereo_ir_survives_live_capture_render_and_trim() {
    let root = directory("stereo-parity");
    let ir = root.join("stereo-ir.wav");
    write_float_wav(&ir, 2, 48000, [1.0, -0.5, 0.0, 0.0]).unwrap();
    let state = EngineState {
        cabinet_ir: Some(ir.to_string_lossy().into_owned()),
        cabinet_enabled: true,
        cabinet_low_cut_hz: 20.0,
        cabinet_high_cut_hz: 20000.0,
        ..Default::default()
    };
    let input: Vec<_> = (0..4800).map(|i| (i as f32 * 0.04).sin() * 0.1).collect();
    let source = root.join("input.wav");
    write_float_wav(&source, 1, 48000, input.iter().copied()).unwrap();
    let cabinet = pedal_engine::cabinet::CabinetImpulse::load(&ir, 48000).unwrap();
    let graph = rig::build(
        AmpProcessor::Clean,
        AmpProcessor::Clean,
        Some(cabinet),
        RigControls::from_state(&state),
        48000,
        64,
        Arc::new(RealtimeTelemetry::default()),
    );
    let vault = Arc::new(Mutex::new(RiffVault::new(48000, 1)));
    let mut live = RiffVaultProcessor::new(graph, vault.clone());
    let mut monitored = vec![[0.0; 2]; input.len()];
    let mut clean = input.clone();
    for (i, o) in clean.chunks_mut(64).zip(monitored.chunks_mut(64)) {
        live.process_stereo(i, o);
    }
    assert!(monitored.iter().any(|f| f[0].abs() > 0.01));
    for f in &monitored {
        assert!((f[0] * 0.5 + f[1]).abs() < 1e-6);
    }
    let saved = riff_vault::save_latest(&vault, &root, None, "stereo", None, None).unwrap();
    let recorded = AudioFile::load(std::path::Path::new(&saved.processed_path)).unwrap();
    assert_eq!(
        recorded.samples,
        monitored.iter().flat_map(|f| *f).collect::<Vec<_>>()
    );
    let render = root.join("render.wav");
    pedal_engine::preview::render_source(&state, &render, Some(&source), true, None, None).unwrap();
    let rendered = AudioFile::load(&render).unwrap();
    assert_eq!(&rendered.samples[..input.len() * 2], &recorded.samples);
    let trimmed = riff_vault::trim_saved(
        std::path::Path::new(&saved.clean_path),
        std::path::Path::new(&saved.processed_path),
        &root,
        None,
        "stereo",
        0.01,
        0.08,
    )
    .unwrap();
    let trimmed = AudioFile::load(std::path::Path::new(&trimmed.processed_path)).unwrap();
    assert_eq!(trimmed.channels, 2);
    for f in trimmed.samples.as_chunks::<2>().0 {
        assert!((f[0] * 0.5 + f[1]).abs() < 1e-6);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stereo_effects_generate_width_without_nonfinite_samples() {
    for effect in ["chorus", "flanger", "reverb"] {
        let state = EngineState {
            chorus_enabled: effect == "chorus",
            flanger_enabled: effect == "flanger",
            reverb_enabled: effect == "reverb",
            ..Default::default()
        };
        let mut graph = rig::build(
            AmpProcessor::Clean,
            AmpProcessor::Clean,
            None,
            RigControls::from_state(&state),
            48000,
            64,
            Arc::new(RealtimeTelemetry::default()),
        );
        let mut difference = 0.0;
        for block in 0..750 {
            let mut input: Vec<_> = (0..64)
                .map(|i| ((block * 64 + i) as f32 * 0.091).sin() * 0.1)
                .collect();
            let mut out = [[0.0; 2]; 64];
            graph.process_stereo(&mut input, &mut out);
            for frame in out {
                assert!(frame.iter().all(|s| s.is_finite() && s.abs() <= 0.892));
                difference += (frame[0] - frame[1]).abs();
            }
        }
        assert!(difference > 0.1, "{effect} remained dual mono");
    }
}

#[test]
fn calibration_and_quality_round_trip_without_hardware_assumptions() {
    let state = EngineState {
        calibration_enabled: true,
        input_reference_set: true,
        input_reference_dbu: 9.5,
        input_trim_db: -2.0,
        rig_level_db: -4.5,
        quality_profile: 0,
        render_quality_profile: 2,
        ..Default::default()
    };
    let restored: EngineState =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(state, restored);
    let legacy: EngineState = serde_json::from_str("{}").unwrap();
    assert!(!legacy.calibration_enabled && !legacy.input_reference_set);
    assert_eq!(legacy.quality_profile, 1);
    assert_eq!(legacy.render_quality_profile, 3);
}

#[test]
fn live_monitor_capture_and_render_have_the_same_guitar_samples() {
    let root = directory("graph-parity");
    let state = EngineState {
        eq_enabled: true,
        eq_low_db: 3.0,
        delay_enabled: true,
        delay_time_ms: 40.0,
        delay_feedback: 0.3,
        delay_mix: 0.4,
        rig_level_db: 2.0,
        ..Default::default()
    };
    let input: Vec<_> = (0..4800).map(|i| (i as f32 * 0.037).sin() * 1.2).collect();
    let source = root.join("input.wav");
    write_float_wav(&source, 1, 48000, input.iter().copied()).unwrap();
    let telemetry = Arc::new(RealtimeTelemetry::default());
    let chain = rig::build(
        AmpProcessor::Clean,
        AmpProcessor::Clean,
        None,
        RigControls::from_state(&state),
        48000,
        64,
        telemetry.clone(),
    );
    let vault = Arc::new(Mutex::new(RiffVault::new(48000, 1)));
    let mut live = SafetyProcessor::monitor(
        RiffVaultProcessor::new(chain, vault.clone()),
        telemetry,
        48000,
    );
    let mut monitored = input.clone();
    for chunk in monitored.chunks_mut(64) {
        live.process(chunk);
    }
    let saved = riff_vault::save_latest(&vault, &root, None, "test", None, None).unwrap();
    assert_eq!(
        AudioFile::load(std::path::Path::new(&saved.clean_path))
            .unwrap()
            .mono(),
        input
    );
    assert_eq!(
        AudioFile::load(std::path::Path::new(&saved.processed_path))
            .unwrap()
            .mono(),
        monitored
    );
    let rendered = root.join("render.wav");
    pedal_engine::preview::render_source(&state, &rendered, Some(&source), true, None, None)
        .unwrap();
    let rendered = AudioFile::load(&rendered).unwrap().mono();
    assert_eq!(&rendered[..input.len()], monitored.as_slice());
    assert!(rendered.len() > input.len());
    assert!(rendered[input.len()..].iter().any(|s| s.abs() > 0.001));
    assert!(rendered.iter().all(|s| s.abs() <= 0.892));
    std::fs::remove_dir_all(root).unwrap();
}

struct Metadata(Option<nam_bridge::ModelMetadata>);
impl MonoProcessor for Metadata {
    fn process(&mut self, _: &mut [f32]) {}
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        self.0
    }
}
fn metadata(input: f64, output: f64) -> Metadata {
    Metadata(Some(nam_bridge::ModelMetadata {
        input_level_dbu: Some(input),
        output_level_dbu: Some(output),
        loudness_db: None,
        expected_sample_rate: 48000.0,
    }))
}
#[test]
fn calibration_uses_physical_level_differences_and_safe_fallbacks() {
    let state = EngineState {
        calibration_enabled: true,
        input_reference_set: true,
        input_reference_dbu: 10.0,
        ..Default::default()
    };
    let params = Arc::new(CalibrationParameters::new(&state));
    let mut chain = CalibratedChain::new(
        metadata(4.0, 8.0),
        metadata(2.0, 0.0),
        params.clone(),
        48000,
    );
    let mut audio = vec![0.01; 12000];
    chain.process(&mut audio);
    assert!((audio[11999] - 0.01 * 10.0_f32.powf(12.0 / 20.0)).abs() < 0.0001);
    let mut mixed = state.clone();
    mixed.pedal_mix = 0.5;
    params.update(&mixed);
    let mut audio = vec![0.01; 12000];
    chain.process(&mut audio);
    assert!((audio[11999] - 0.01).abs() < 0.00001);
    let mut unknown = CalibratedChain::new(Metadata(None), Metadata(None), params, 48000);
    let mut audio = [0.1, -0.2];
    unknown.process(&mut audio);
    assert_eq!(audio, [0.1, -0.2]);
}

#[test]
fn reamp_padding_preserves_original_di_extent_and_sources() {
    let root = directory("tail-extent");
    let clean = root.join("source-clean.wav");
    let wet = root.join("source-rig.wav");
    write_float_wav(&clean, 1, 48000, std::iter::repeat_n(0.1, 480)).unwrap();
    write_float_wav(&wet, 1, 48000, std::iter::repeat_n(0.2, 960)).unwrap();
    let saved = riff_vault::trim_saved(&clean, &wet, &root, None, "test", 0.0, 0.02).unwrap();
    assert_eq!(saved.source_duration_seconds, Some(0.01));
    assert_eq!(
        riff_vault::source_duration(std::path::Path::new(&saved.clean_path)),
        Some(0.01)
    );
    assert_eq!(AudioFile::load(&clean).unwrap().samples.len(), 480);
    let padded = AudioFile::load(std::path::Path::new(&saved.clean_path))
        .unwrap()
        .mono();
    assert_eq!(padded.len(), 960);
    assert!(padded[480..].iter().all(|s| *s == 0.0));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn level_matching_is_bounded_and_does_not_edit_tone_controls() {
    let root = directory("matching");
    let state = EngineState {
        amp_bass_db: 4.0,
        pedal_drive_db: 8.0,
        ..Default::default()
    };
    let before = state.clone();
    let level = pedal_engine::preview::audition_level(&state, &root.join("match.wav")).unwrap();
    assert!((-24.0..=6.0).contains(&level));
    assert_eq!(state, before);
    std::fs::remove_dir_all(root).unwrap();
}
