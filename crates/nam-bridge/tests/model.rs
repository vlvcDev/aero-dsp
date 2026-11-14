use nam_bridge::{NamModel, NamQuality};
use std::path::PathBuf;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models")
        .join(name)
}

#[test]
fn alignment_analysis_does_not_leave_probe_audio_in_the_playback_model() {
    for quality in [NamQuality::Lite, NamQuality::Full] {
        for name in ["lstm.nam", "A2.nam"] {
            let mut measured =
                NamModel::load_with_alignment(example(name), 48000.0, 64, quality).unwrap();
            let mut pristine =
                NamModel::load_with_quality(example(name), 48000.0, 64, quality).unwrap();
            let estimate = measured.alignment().unwrap();
            assert!(estimate.correlation.is_finite());
            if name == "lstm.nam" {
                assert_eq!(
                    estimate.status,
                    nam_bridge::alignment::AlignmentStatus::Accepted
                );
                assert_eq!(estimate.delay_samples, 0);
            }
            for n in 0..100 {
                let mut a: [f32; 64] =
                    std::array::from_fn(|i| ((i + n * 64) as f32 * 0.04).sin() * 0.02);
                let mut b = a;
                measured.process(&mut a).unwrap();
                pristine.process(&mut b).unwrap();
                assert_eq!(a, b, "{name} {quality:?}");
            }
        }
    }
}

fn render_size(name: &str, quality: NamQuality, block: usize) -> Vec<f32> {
    let mut model = NamModel::load_with_quality(example(name), 48_000.0, 128, quality).unwrap();
    let mut samples: Vec<_> = (0..4096).map(|i| (i as f32 * 0.04).sin() * 0.1).collect();
    for chunk in samples.chunks_mut(block) {
        model.process(chunk).unwrap();
    }
    assert!(samples.iter().all(|s| s.is_finite()));
    samples
}

#[test]
fn tiers_are_supported_only_when_the_file_contains_multiple_sizes() {
    for name in ["A2.nam", "slimmable_container.nam", "slimmable_wavenet.nam"] {
        assert!(
            NamModel::load(example(name), 48_000.0, 128)
                .unwrap()
                .supports_sizes(),
            "{name}"
        );
        let lite = render_size(name, NamQuality::Lite, 64);
        let full = render_size(name, NamQuality::Full, 64);
        assert!(
            lite.iter().zip(&full).any(|(a, b)| (a - b).abs() > 1e-6),
            "{name}"
        );
        for quality in [NamQuality::Lite, NamQuality::Full] {
            let a = render_size(name, quality, 64);
            let b = render_size(name, quality, 37);
            assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5), "{name}");
        }
    }
    assert!(
        !NamModel::load(example("lstm.nam"), 48_000.0, 128)
            .unwrap()
            .supports_sizes()
    );
    assert_eq!(
        render_size("lstm.nam", NamQuality::Lite, 64),
        render_size("lstm.nam", NamQuality::Full, 64)
    );
}

#[test]
fn default_loading_is_full_and_tier_changes_do_not_change_rate_validation() {
    let mut implicit = NamModel::load(example("A2.nam"), 48_000.0, 64).unwrap();
    let mut explicit =
        NamModel::load_with_quality(example("A2.nam"), 48_000.0, 64, NamQuality::Full).unwrap();
    for _ in 0..20 {
        let mut a = [0.01; 64];
        let mut b = a;
        implicit.process(&mut a).unwrap();
        explicit.process(&mut b).unwrap();
        assert_eq!(a, b);
    }
    for mode in [NamQuality::Lite, NamQuality::Full] {
        assert!(NamModel::load_with_quality(example("A2.nam"), 44_100.0, 64, mode).is_err());
    }
}

#[test]
fn loads_and_processes_the_reference_lstm() {
    let mut model = NamModel::load(example("lstm.nam"), 48_000.0, 64).expect("load LSTM model");
    let mut audio = [0.0_f32; 64];
    audio[0] = 0.1;

    model.process(&mut audio).expect("process one block");

    assert!(audio.iter().all(|sample| sample.is_finite()));
    assert_eq!(model.expected_sample_rate(), 48_000.0);
    assert!(!model.is_faulted());
}

#[test]
fn rejects_a_model_sample_rate_mismatch() {
    let result = NamModel::load(example("lstm.nam"), 44_100.0, 64);

    assert!(result.is_err());
}

#[test]
fn exposes_optional_calibration_without_inventing_missing_values() {
    let path = std::env::temp_dir().join(format!("nam-calibration-{}.nam", std::process::id()));
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(example("lstm.nam")).unwrap()).unwrap();
    json["metadata"] =
        serde_json::json!({"input_level_dbu":10.0,"output_level_dbu":8.0,"loudness":-18.0});
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let m = NamModel::load(&path, 48_000.0, 64).unwrap();
    let metadata = m.metadata();
    assert_eq!(metadata.input_level_dbu, Some(10.0));
    assert_eq!(metadata.output_level_dbu, Some(8.0));
    assert_eq!(metadata.loudness_db, Some(-18.0));
    drop(m);
    json["metadata"] = serde_json::json!({});
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let m = NamModel::load(&path, 48_000.0, 64).unwrap();
    assert_eq!(m.metadata().input_level_dbu, None);
    assert_eq!(m.metadata().output_level_dbu, None);
    assert_eq!(m.metadata().loudness_db, None);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn oversized_blocks_are_sliced_sample_exactly_instead_of_faulting() {
    for name in ["lstm.nam", "A2.nam"] {
        let mut sliced = NamModel::load(example(name), 48_000.0, 64).unwrap();
        let mut reference = NamModel::load(example(name), 48_000.0, 64).unwrap();
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.03).sin() * 0.1).collect();
        let mut a = input.clone();
        let mut b = input.clone();
        sliced.process(&mut a).unwrap();
        assert!(!sliced.is_faulted());
        for chunk in b.chunks_mut(64) {
            reference.process(chunk).unwrap();
        }
        assert_eq!(a, b, "{name}");
        let mut c = input;
        assert!(sliced.process_realtime(&mut c), "{name}");
    }
}
