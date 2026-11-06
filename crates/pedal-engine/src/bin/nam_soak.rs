use nam_bridge::{NamModel, NamQuality};
use pedal_engine::dsp::{InterleavedLayout, process_interleaved_i32};
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SAMPLE_RATE: usize = 48_000;
const FRAMES_PER_BLOCK: usize = 64;
const CAPTURE_CHANNELS: usize = 2;
const PLAYBACK_CHANNELS: usize = 2;
const AUDIO_SECONDS: usize = 10;

fn main() {
    let model_path = env::args_os()
        .nth(1)
        .map_or_else(default_model, PathBuf::from);
    let quality = match env::args().nth(2).as_deref() {
        None | Some("--full") => NamQuality::Full,
        Some("--lite") => NamQuality::Lite,
        _ => {
            eprintln!("Usage: pedal-nam-soak [model.nam] [--lite|--full]");
            std::process::exit(1);
        }
    };
    let mut model =
        NamModel::load_with_quality(&model_path, SAMPLE_RATE as f64, FRAMES_PER_BLOCK, quality)
            .unwrap_or_else(|error| {
                eprintln!("Could not load {}: {error}", model_path.display());
                std::process::exit(1);
            });
    println!(
        "Requested {quality:?}; capture supports multiple sizes: {}",
        model.supports_sizes()
    );
    let blocks = SAMPLE_RATE * AUDIO_SECONDS / FRAMES_PER_BLOCK;
    let deadline = Duration::from_secs_f64(FRAMES_PER_BLOCK as f64 / SAMPLE_RATE as f64);
    let layout = InterleavedLayout {
        frames: FRAMES_PER_BLOCK,
        capture_channels: CAPTURE_CHANNELS,
        playback_channels: PLAYBACK_CHANNELS,
        guitar_input_channel: 0,
    };
    let mut input = vec![0_i32; FRAMES_PER_BLOCK * CAPTURE_CHANNELS];
    let mut output = vec![0_i32; FRAMES_PER_BLOCK * PLAYBACK_CHANNELS];
    let mut mono = vec![0.0_f32; FRAMES_PER_BLOCK];
    let mut phase = 0_u32;
    let mut checksum = 0_i64;
    let mut clipped_samples = 0_u64;
    let mut worst_block = Duration::ZERO;
    let mut deadline_misses = 0_u64;

    let started = Instant::now();
    for _ in 0..blocks {
        for frame in 0..FRAMES_PER_BLOCK {
            phase = phase.wrapping_add(0x012d_97c8);
            input[frame * CAPTURE_CHANNELS] = (phase as i32) >> 4;
            input[frame * CAPTURE_CHANNELS + 1] = 0;
        }

        let block_started = Instant::now();
        process_interleaved_i32(&input, &mut output, &mut mono, layout, &mut model)
            .expect("synthetic NAM buffers are valid");
        let elapsed = block_started.elapsed();
        worst_block = worst_block.max(elapsed);
        if elapsed > deadline {
            deadline_misses += 1;
        }
        for frame in 0..FRAMES_PER_BLOCK {
            let sample = output[frame * PLAYBACK_CHANNELS];
            checksum = checksum.wrapping_add(i64::from(sample));
            if sample == i32::MIN || sample == i32::MAX {
                clipped_samples += 1;
            }
        }
    }

    println!(
        "Processed {AUDIO_SECONDS}s through {} in {:.2?}; worst block {:.2?}; deadline misses {}; clipped samples {}; checksum {}",
        model_path.display(),
        started.elapsed(),
        worst_block,
        deadline_misses,
        clipped_samples,
        checksum,
    );
    if model.is_faulted() {
        eprintln!("NAM processor entered a faulted state");
        std::process::exit(2);
    }
}

fn default_model() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models/A2.nam")
}
