//! Offline test harness. Never opens an audio device or replaces an existing file.
use pedal_engine::{audio_file::AudioFile, node_document::RigDocument};
use std::{io::Read, path::Path};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(4..=5).contains(&args.len()) {
        return Err("Usage: node_render RIG.json INPUT.wav NEW_OUTPUT.wav [TAIL_SECONDS]".into());
    }
    let path = Path::new(&args[1]);
    let mut json = String::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(1_048_577)
        .read_to_string(&mut json)
        .map_err(|e| e.to_string())?;
    if json.len() > 1_048_576 {
        return Err("Rig document exceeds 1 MiB".into());
    }
    let document: RigDocument = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    if std::fs::metadata(&args[2])
        .map_err(|e| e.to_string())?
        .len()
        > 256 * 1024 * 1024
    {
        return Err("Input WAV exceeds 256 MiB".into());
    }
    let source = AudioFile::load(Path::new(&args[2]))?;
    let tail = args
        .get(4)
        .map(|x| x.parse::<f32>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(0.0);
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output = document.render(directory, &source, tail)?;
    let peak = output.iter().flatten().fold(0.0f32, |a, x| a.max(x.abs()));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])
        .map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(
        std::io::BufWriter::new(file),
        hound::WavSpec {
            channels: 2,
            sample_rate: document.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(|e| e.to_string())?;
    for frame in &output {
        for sample in frame {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
    }
    writer.finalize().map_err(|e| e.to_string())?;
    println!(
        "Rendered {} stereo frames; sample peak {:.3}. Float output is not playback-limited.",
        output.len(),
        peak
    );
    if peak > 1.0 {
        println!("Warning: reduce output gain before playback; output exceeds 0 dBFS.");
    }
    Ok(())
}
