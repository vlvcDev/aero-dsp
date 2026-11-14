use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Engine(Child, std::path::PathBuf);
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

#[test]
fn preview_runs_through_real_control_socket_without_audio_hardware() {
    let root = std::env::temp_dir().join(format!(
        "aero-patch-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let source = root.join("source.wav");
    pedal_engine::audio_file::write_float_wav(&source, 1, 48000, vec![0.2; 480]).unwrap();
    let socket = root.join("control.sock");
    let child = Command::new(env!("CARGO_BIN_EXE_pedal-engine"))
        .env("PEDAL_AUDIO_ENABLED", "0")
        .env("PEDAL_CONTROL_SOCKET", &socket)
        .env("PEDAL_STATE_FILE", root.join("state.json"))
        .env("PEDAL_PRESET_FILE", root.join("presets.json"))
        .env("PEDAL_PREVIEW_FILE", root.join("preview.wav"))
        .env("PEDAL_TEST_AUDIO_FILE", &source)
        .env("PEDAL_RIFF_DIRECTORY", root.join("riffs"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut engine = Engine(child, root.clone());
    let start = Instant::now();
    let mut stream = loop {
        assert!(
            engine.0.try_wait().unwrap().is_none(),
            "engine exited during startup"
        );
        if let Ok(stream) = UnixStream::connect(&socket) {
            break stream;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "engine did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = |kind: &str, payload: serde_json::Value| {
        writeln!(
            stream,
            "{}",
            serde_json::json!({"api":1,"id":"test","type":kind,"payload":payload})
        )
        .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()
    };
    assert_eq!(
        request("patch_status", serde_json::json!({}))["payload"]["available"],
        false
    );
    let document = serde_json::json!({"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[
        {"id":1,"inputs":[],"processor":{"type":"input"}},
        {"id":3,"inputs":[1],"processor":{"type":"gain","db":-6.0206}},
        {"id":2,"inputs":[3],"processor":{"type":"output"}}]},"processors":{}});
    let payload = serde_json::json!({"document":document,"directory":root});
    assert_eq!(request("apply_patch", payload.clone())["type"], "error");
    let response = request("render_patch", payload);
    assert_eq!(response["type"], "patch", "{response}");
    let audio = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["path"].as_str().unwrap(),
    ))
    .unwrap();
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.samples.len(), (480 + 48000 * 3) * 2);
    assert!((audio.samples[0] - 0.1).abs() < 1e-5);
    // Two embedded copies travel through the same protocol as the editor.
    // No library lookup is required to audition a saved outer graph.
    let custom = serde_json::json!({"type":"custom","name":"Half level",
        "library_id":"half-level","revision":1,"asset_directory":root,
        "document":document});
    let grouped = serde_json::json!({"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[
        {"id":1,"inputs":[],"processor":{"type":"input"}},
        {"id":3,"inputs":[1],"processor":{"type":"external"}},
        {"id":4,"inputs":[3],"processor":{"type":"external"}},
        {"id":2,"inputs":[4],"processor":{"type":"output"}}]},
        "processors":{"3":custom,"4":custom}});
    let response = request(
        "render_patch",
        serde_json::json!({"document":grouped,"directory":root}),
    );
    assert_eq!(response["type"], "patch", "{response}");
    let grouped_audio = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["path"].as_str().unwrap(),
    ))
    .unwrap();
    assert!((grouped_audio.samples[0] - 0.05).abs() < 1e-5);
    let mut fuzz = serde_json::json!({"version":1,"sample_rate":48000,"graph":{"version":1,"nodes":[
        {"id":1,"inputs":[],"processor":{"type":"input"}},
        {"id":3,"inputs":[1],"processor":{"type":"external"}},
        {"id":2,"inputs":[3],"processor":{"type":"output"}}]},
        "processors":{"3":{"type":"fuzz","drive_db":18.0,"bias":0.0,"tone":0.5,"mix":1.0,"output_db":-12.0}}});
    let response = request(
        "render_patch",
        serde_json::json!({"document":fuzz,"directory":root}),
    );
    assert_eq!(response["type"], "patch", "{response}");
    let fuzz_audio = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["path"].as_str().unwrap(),
    ))
    .unwrap();
    assert!(fuzz_audio.samples.iter().all(|x| x.is_finite()));
    assert!(fuzz_audio.samples[..960].iter().any(|x| x.abs() > 0.001));
    assert!(
        fuzz_audio
            .samples
            .iter()
            .rev()
            .take(100)
            .all(|x| x.abs() < 0.001)
    );
    fuzz["processors"]["3"]["drive_db"] = serde_json::json!(100);
    // Audition one portable rig made entirely from the creative processors.
    // This exercises the same descriptor keys emitted by the editor.
    let mut creative = fuzz.clone();
    let processors = [
        serde_json::json!({"type":"overdrive","drive_db":9,"tone":0.5,"mix":1,"output_db":-6}),
        serde_json::json!({"type":"octaver","clean_mode":false,"mix":0.45}),
        serde_json::json!({"type":"chorus","rate_hz":0.8,"depth":0.5,"mix":0.35}),
        serde_json::json!({"type":"flanger","rate_hz":0.3,"depth":0.55,"feedback":0.25,"mix":0.4}),
        serde_json::json!({"type":"tremolo","rate_hz":4,"depth":0.55}),
    ];
    let mut source_id = 1;
    for (index, processor) in processors.into_iter().enumerate() {
        let id = index + 3;
        if index != 0 {
            creative["graph"]["nodes"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!(
                {"id":id,"inputs":[source_id],"processor":{"type":"external"}}));
        }
        creative["processors"][id.to_string()] = processor;
        source_id = id;
    }
    creative["graph"]["nodes"][2]["inputs"] = serde_json::json!([source_id]);
    let response = request(
        "render_patch",
        serde_json::json!({"document":creative,"directory":root}),
    );
    assert_eq!(response["type"], "patch", "{response}");
    let rendered = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
        response["payload"]["path"].as_str().unwrap(),
    ))
    .unwrap();
    assert!(rendered.samples.iter().all(|x| x.is_finite()));
    assert!(rendered.samples.iter().any(|x| x.abs() > 0.001));
    // Exercise the creative nodes through JSON and the preview socket, including
    // an embedded processor map inside a Custom Node (Serde's tagged-enum path).
    let chord_path = root.join("chord.wav");
    pedal_engine::audio_file::write_float_wav(
        &chord_path,
        1,
        48000,
        (0..48000).map(|i| {
            let t = std::f32::consts::TAU * i as f32 / 48000.0;
            0.1 * ((110.0 * t).sin() + (220.0 * t).sin())
        }),
    )
    .unwrap();
    for processor in [
        serde_json::json!({"type":"transpose","semitones":-2,"quality":"fast"}),
        serde_json::json!({"type":"envelope_filter","sensitivity":0.5,"base_hz":300,"resonance":2,"decay_ms":200,"mix":1}),
        serde_json::json!({"type":"ring_mod","frequency_hz":80,"mix":0.5}),
        serde_json::json!({"type":"slicer","bpm":120,"division":2,"pattern":"gallop","depth":1}),
        serde_json::json!({"type":"bitcrusher","bits":8,"rate_hz":12000,"mix":0.5}),
    ] {
        let mut child = fuzz.clone();
        child["processors"]["3"] = processor;
        let mut outer = child.clone();
        outer["processors"]["3"] = serde_json::json!({"type":"custom","name":"Effect",
            "library_id":"effect","revision":1,"asset_directory":root,"document":child});
        let response = request(
            "render_patch",
            serde_json::json!({
            "document":outer,"directory":root,"path":chord_path}),
        );
        assert_eq!(response["type"], "patch", "{response}");
        let result = pedal_engine::audio_file::AudioFile::load(std::path::Path::new(
            response["payload"]["path"].as_str().unwrap(),
        ))
        .unwrap();
        assert_eq!(result.samples.len(), 48000 * 4 * 2);
        assert!(result.samples.iter().all(|x| x.is_finite()));
        assert!(result.samples.iter().any(|x| x.abs() > 0.001));
    }
    assert_eq!(
        request(
            "render_patch",
            serde_json::json!({"document":fuzz,"directory":root})
        )["type"],
        "error"
    );
    assert_eq!(
        request(
            "render_patch",
            serde_json::json!({"document":{},"directory":root})
        )["type"],
        "error"
    );
    assert_eq!(
        request("patch_status", serde_json::json!({}))["payload"]["active"],
        false
    );
}
