use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
#[test]
fn comparison_runs_over_control_socket() {
    let root = std::env::temp_dir().join(format!("patch-ab-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("source.wav");
    pedal_engine::audio_file::write_float_wav(&source, 1, 48_000, vec![0.2; 480]).unwrap();
    let socket = root.join("socket");
    let mut child = Command::new(env!("CARGO_BIN_EXE_pedal-engine"))
        .env("PEDAL_AUDIO_ENABLED", "0")
        .env("PEDAL_CONTROL_SOCKET", &socket)
        .env("PEDAL_STATE_FILE", root.join("state.json"))
        .env("PEDAL_PRESET_FILE", root.join("presets.json"))
        .env("PEDAL_PREVIEW_FILE", root.join("preview.wav"))
        .env("PEDAL_TEST_AUDIO_FILE", &source)
        .env("PEDAL_RIFF_DIRECTORY", root.join("riffs"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let mut stream = loop {
        if let Ok(stream) = UnixStream::connect(&socket) {
            break stream;
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let doc = |db| serde_json::json!({"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[{"id":1,"inputs":[],"processor":{"type":"input"}},{"id":3,"inputs":[1],"processor":{"type":"gain","db":db}},{"id":2,"inputs":[3],"processor":{"type":"output"}}]},"processors":{}});
    let payload = serde_json::json!({"a":{"document":doc(0.0),"directory":root},"b":{"document":doc(-6.0206),"directory":root},"source":"built_in"});
    writeln!(
        stream,
        "{}",
        serde_json::json!({"api":1,"id":"ab","type":"render_patch_comparison","payload":payload})
    )
    .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let response: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["type"], "patch", "{response}");
    assert_eq!(response["payload"]["matched"], true);
    let a = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["a_path"].as_str().unwrap(),
    ))
    .unwrap();
    let b = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["b_path"].as_str().unwrap(),
    ))
    .unwrap();
    assert_eq!(a.samples.len(), b.samples.len());
    assert!((a.samples[0] - b.samples[0]).abs() < 1e-6);
    child.kill().unwrap();
    child.wait().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
