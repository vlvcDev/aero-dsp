//! Exercise saved documents through the same preparation path as the live editor.
use pedal_engine::{audio_file::AudioFile, node_document::RigDocument};
use serde_json::{Value, json};
use std::path::Path;

fn processors() -> Vec<Value> {
    vec![
        json!({"type":"transpose","semitones":-2,"quality":"fast"}),
        json!({"type":"envelope_filter","sensitivity":0.5,"base_hz":300,
            "resonance":2,"decay_ms":200,"mix":1}),
        json!({"type":"ring_mod","frequency_hz":80,"mix":0.5}),
        json!({"type":"slicer","bpm":120,"division":2,"pattern":"pulse","depth":1}),
        json!({"type":"bitcrusher","bits":8,"rate_hz":12000,"mix":0.5}),
    ]
}

fn document(processors: Vec<Value>) -> RigDocument {
    let mut nodes = vec![json!({"id":1,"inputs":[],"processor":{"type":"input"}})];
    let mut bindings = serde_json::Map::new();
    let mut previous = 1;
    for (i, processor) in processors.into_iter().enumerate() {
        let id = i + 3;
        nodes.push(json!({"id":id,"inputs":[previous],"processor":{"type":"external"}}));
        bindings.insert(id.to_string(), processor);
        previous = id;
    }
    nodes.push(json!({"id":2,"inputs":[previous],"processor":{"type":"output"}}));
    serde_json::from_value(json!({"version":1,"sample_rate":48000,
        "graph":{"version":1,"nodes":nodes},"processors":bindings}))
    .unwrap()
}

fn source() -> AudioFile {
    AudioFile {
        sample_rate: 48000,
        channels: 2,
        samples: (0..48000)
            .flat_map(|i| {
                let t = i as f32 / 48000.0 * std::f32::consts::TAU;
                [0.1 * ((110.0 * t).sin() + (220.0 * t).sin()), 0.0]
            })
            .collect(),
    }
}

#[test]
fn saved_effects_render_individually_in_series_and_inside_custom_nodes() {
    let root = std::env::current_dir().unwrap();
    let input = source();
    let mut configurations: Vec<_> = processors().into_iter().map(|p| vec![p]).collect();
    configurations.push(processors());
    for configuration in configurations {
        let doc = document(configuration);
        let encoded = serde_json::to_value(&doc).unwrap();
        let restored: RigDocument = serde_json::from_value(encoded.clone()).unwrap();
        let direct = restored.render(&root, &input, 0.5).unwrap();
        assert_eq!(direct.len(), 72000);
        assert!(direct.iter().flatten().all(|x| x.is_finite()));
        assert!(direct.iter().any(|x| x[0].abs() > 0.001));
        assert!(
            direct.iter().all(|x| x[1].abs() < 1e-6),
            "silent channel leaked"
        );
        let nested = document(vec![json!({"type":"custom","name":"Creative rig",
            "library_id":"test-creative","revision":1,"asset_directory":root,
            "document":encoded})]);
        let nested_audio = nested.render(&root, &input, 0.5).unwrap();
        assert_eq!(direct, nested_audio, "embedding changed the sound");
        assert_eq!(
            doc.prepare(&root).unwrap().graph.latency_samples(),
            nested.prepare(&root).unwrap().graph.latency_samples()
        );
    }
}

#[test]
fn bad_imported_effect_settings_are_rejected_before_processing() {
    for (index, field, value) in [
        (0, "semitones", json!(13)),
        (1, "resonance", json!(0)),
        (2, "frequency_hz", json!(0)),
        (3, "division", json!(3)),
        (4, "bits", json!(3)),
    ] {
        let mut descriptor = processors().remove(index);
        descriptor[field] = value;
        assert!(document(vec![descriptor]).prepare(Path::new(".")).is_err());
    }
}

#[test]
#[ignore = "manual release performance measurement; not a real-time deadline guarantee"]
fn profile_effects_in_64_frame_blocks() {
    let input = source();
    let mut configurations: Vec<_> = processors().into_iter().map(|p| vec![p]).collect();
    let mut smooth = processors().remove(0);
    smooth["quality"] = json!("smooth");
    configurations.insert(1, vec![smooth]);
    configurations.push(processors());
    for configuration in configurations {
        let label = configuration
            .iter()
            .map(|p| {
                format!(
                    "{} {}",
                    p["type"].as_str().unwrap(),
                    p["quality"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join(" + ");
        let doc = document(configuration);
        let mut rig = doc.prepare(Path::new(".")).unwrap();
        let mut durations = Vec::with_capacity(7500);
        let mut block = [[0.0; 2]; 64];
        for _ in 0..10 {
            for samples in input.samples.chunks_exact(128) {
                for (frame, pair) in block.iter_mut().zip(samples.chunks_exact(2)) {
                    *frame = [pair[0], pair[1]];
                }
                let start = std::time::Instant::now();
                rig.graph.process(&mut block);
                durations.push(start.elapsed().as_secs_f64() * 1e6);
            }
        }
        let mean = durations.iter().sum::<f64>() / durations.len() as f64;
        durations.sort_by(f64::total_cmp);
        println!(
            "{label}: mean {mean:.1} us, p99 {:.1} us, max {:.1} us per 1333.3 us block; added latency {:.2} ms",
            durations[durations.len() * 99 / 100],
            durations.last().unwrap(),
            rig.graph.latency_samples() as f64 * 1000.0 / 48000.0
        );
    }
}
