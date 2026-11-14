//! Aliasing and CPU measurements for phase-interleaved NAM oversampling,
//! mirroring Tone3000's real-capture aliasing test (4998 Hz sine, folded
//! harmonics relative to the fundamental).
use nam_bridge::NamQuality;
use pedal_engine::nam_oversampling::OversampledNam;
use rustfft::{FftPlanner, num_complex::Complex};
use std::path::{Path, PathBuf};
use std::time::Instant;

const RATE: f64 = 48_000.0;
const FFT: usize = 16_384;
const BLOCK: usize = 128;
/// Bin-centred near 5 kHz: harmonics 5+ fold back below Nyquist.
const FUNDAMENTAL_BIN: usize = 1_706;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models")
        .join(name)
}

/// Folded-harmonic power relative to the fundamental, in dB.
fn aliasing_db(path: &Path, factor: u8, amplitude: f32) -> f32 {
    let mut model = OversampledNam::load(path, RATE, BLOCK, NamQuality::Full, false, factor)
        .expect("load model");
    let frequency = FUNDAMENTAL_BIN as f64 * RATE / FFT as f64;
    let settle = 24_000;
    let mut signal: Vec<f32> = (0..settle + FFT)
        .map(|i| amplitude * (std::f64::consts::TAU * frequency * i as f64 / RATE).sin() as f32)
        .collect();
    for block in signal.chunks_mut(BLOCK) {
        assert!(model.process_realtime(block));
    }
    let tail = &signal[settle..];
    let mut spectrum: Vec<Complex<f64>> = tail
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // 4-term Blackman-Harris.
            let x = std::f64::consts::TAU * i as f64 / FFT as f64;
            let w =
                0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos();
            Complex::new(f64::from(*s) * w, 0.0)
        })
        .collect();
    FftPlanner::new()
        .plan_fft_forward(FFT)
        .process(&mut spectrum);
    let power: Vec<f64> = spectrum[..FFT / 2].iter().map(|c| c.norm_sqr()).collect();
    let near = |bin: usize, centre: usize| bin.abs_diff(centre) <= 5;
    let fundamental: f64 = (0..FFT / 2)
        .filter(|&b| near(b, FUNDAMENTAL_BIN))
        .map(|b| power[b])
        .sum();
    // True harmonics below Nyquist (k = 1..4) and the DC region are not aliasing.
    let aliasing: f64 = (0..FFT / 2)
        .filter(|&b| b > 20 && !(1..=4).any(|k| near(b, k * FUNDAMENTAL_BIN)))
        .map(|b| power[b])
        .sum();
    (10.0 * (aliasing / fundamental).log10()) as f32
}

/// Audio-thread wall time per second of audio (mean) and the worst block as
/// a fraction of its deadline. `parallel` spreads phases across the pool.
fn realtime_load(path: &Path, factor: u8, parallel: bool) -> (f64, f64) {
    pedal_engine::rt_pool::ensure_global_started();
    let mut model = OversampledNam::load(path, RATE, BLOCK, NamQuality::Full, false, factor)
        .unwrap()
        .with_pool(parallel.then(pedal_engine::rt_pool::global));
    let seconds = 3;
    let mut signal: Vec<f32> = (0..48_000 * seconds)
        .map(|i| 0.3 * (i as f32 * 0.05).sin())
        .collect();
    let deadline = BLOCK as f64 / RATE;
    let mut worst = 0.0_f64;
    let start = Instant::now();
    for block in signal.chunks_mut(BLOCK) {
        let block_start = Instant::now();
        model.process_realtime(block);
        worst = worst.max(block_start.elapsed().as_secs_f64() / deadline);
    }
    (start.elapsed().as_secs_f64() / seconds as f64, worst)
}

#[test]
fn oversampling_reduces_aliasing_of_a_real_a2_capture() {
    let path = example("A2.nam");
    let base = aliasing_db(&path, 1, 0.8);
    let x2 = aliasing_db(&path, 2, 0.8);
    let x4 = aliasing_db(&path, 4, 0.8);
    eprintln!("A2.nam aliasing: 1x {base:.1} dB, 2x {x2:.1} dB, 4x {x4:.1} dB");
    assert!(x2 < base - 3.0, "2x {x2} vs 1x {base}");
    assert!(x4 < base - 6.0, "4x {x4} vs 1x {base}");
}

/// Manual: `cargo test --release -p pedal-engine --test nam_oversampling_quality
/// -- --ignored --nocapture`. Optionally set NAM_MEASURE_DIR to a folder of
/// captures (defaults to the project's data/models).
#[test]
#[ignore = "manual release-build measurement over local captures"]
fn measure_local_captures() {
    let directory = std::env::var_os("NAM_MEASURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/models"));
    let mut paths: Vec<_> = std::fs::read_dir(&directory)
        .expect("read capture folder")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|e| e == "nam"))
        .collect();
    paths.sort();
    let limit = std::env::var("NAM_MEASURE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    for path in paths.into_iter().take(limit) {
        let name = path.file_name().unwrap().to_string_lossy();
        let mut line = format!("{name:<60}");
        for factor in [1, 2, 4, 8] {
            let alias = aliasing_db(&path, factor, 0.8);
            let (serial, _) = realtime_load(&path, factor, false);
            let (parallel, worst) = realtime_load(&path, factor, true);
            line += &format!(
                " | {factor}x {alias:6.1} dB, serial {:5.1}% / parallel {:5.1}% (worst block {:4.0}%)",
                serial * 100.0,
                parallel * 100.0,
                worst * 100.0
            );
        }
        eprintln!("{line}");
    }
}
