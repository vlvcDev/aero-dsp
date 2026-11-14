use std::process::Command;
#[test]
fn command_renders_saved_nam_rig_and_refuses_overwrite() {
    let root = std::env::temp_dir().join(format!(
        "aero-node-render-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("model.nam"), r#"{"version":"0.5.0","architecture":"Linear","sample_rate":48000,"config":{"receptive_field":1,"bias":false},"weights":[0.5]}"#).unwrap();
    let rig = root.join("rig.json");
    std::fs::write(&rig, r#"{"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[{"id":1,"inputs":[],"processor":{"type":"input"}},{"id":2,"inputs":[1],"processor":{"type":"external"}},{"id":3,"inputs":[2],"processor":{"type":"output"}}]},"processors":{"2":{"type":"nam","path":"model.nam","input":"average"}}}"#).unwrap();
    let input = root.join("input.wav");
    // Add a real stereo cabinet asset to the saved NAM chain.
    pedal_engine::audio_file::write_float_wav(&root.join("cab.wav"), 2, 48_000, [0.5, -0.25])
        .unwrap();
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&rig).unwrap()).unwrap();
    document["graph"]["nodes"][2]["inputs"] = serde_json::json!([4]);
    document["graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"id":4,"inputs":[2],"processor":{"type":"external"}}));
    document["processors"]["4"] =
        serde_json::json!({"type":"cabinet","path":"cab.wav","duration_seconds":0.2});
    std::fs::write(&rig, serde_json::to_vec(&document).unwrap()).unwrap();
    let output = root.join("output.wav");
    pedal_engine::audio_file::write_float_wav(
        &input,
        2,
        48_000,
        [0.2, 0.6].into_iter().cycle().take(1026),
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_node_render"))
            .current_dir(&root)
            .args([
                rig.file_name().unwrap(),
                input.as_os_str(),
                output.as_os_str(),
            ])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wav = pedal_engine::audio_file::AudioFile::load(&output).unwrap();
    assert_eq!(wav.channels, 2);
    assert_eq!(wav.samples.len(), 1026);
    let cabinet_pad = 10.0_f32.powf(pedal_engine::cabinet::CABINET_PAD_DB / 20.0);
    assert!(wav.samples.as_chunks::<2>().0.iter().all(
        |x| (x[0] - 0.1 * cabinet_pad).abs() < 1e-6 && (x[1] + 0.05 * cabinet_pad).abs() < 1e-6
    ));
    let bytes = std::fs::read(&output).unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}
