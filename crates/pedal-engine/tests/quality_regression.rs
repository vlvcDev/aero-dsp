//! Audio-quality regression check: one fixed DI through one fixed full rig
//! (the offline render path, so the live graph), reduced to a few numbers and
//! compared with `quality_baseline.json`.
//!
//! Quality metrics (aliasing, idle noise, latency) only fail when they get
//! WORSE; tone metrics (level, band balance) fail on a change either way.
//! After an intentional sound change, accept it with:
//!
//!     QUALITY_BASELINE=update cargo test -p pedal-engine --test quality_regression
use pedal_engine::{EngineState, audio_file::AudioFile};
use rustfft::{FftPlanner, num_complex::Complex};
use std::{collections::BTreeMap, path::PathBuf};

const RATE: usize = 48_000;
const PROBE_BIN: usize = 1_706; // 4998.05 Hz in a 16384-point FFT

/// (metric, tolerance, worse-only?)
const METRICS: [(&str, f64, bool); 7] = [
    ("level_db", 0.5, false),
    ("band_low_db", 0.5, false),
    ("band_mid_db", 0.5, false),
    ("band_high_db", 0.75, false),
    ("aliasing_db", 3.0, true),
    ("idle_noise_db", 2.0, true),
    ("latency_samples", 2.0, true),
];

fn sine(hz: f64, n: usize) -> f32 {
    (std::f64::consts::TAU * hz * n as f64 / RATE as f64).sin() as f32
}

/// Guitar-like tone: fundamental plus decaying harmonics.
fn string(hz: f64, n: usize) -> f32 {
    (1..=8)
        .map(|k| sine(hz * k as f64, n) / k as f32)
        .sum::<f32>()
        * 0.35
}

/// 0-2 s palm-muted low-E chugs, 2-3.5 s held A, 3.5-4.5 s 5 kHz probe tone,
/// 4.6 s impulse, 5-6 s idle pickup hiss (-70 dBFS).
fn di() -> Vec<f32> {
    let mut seed = 12_345u32;
    (0..6 * RATE)
        .map(|n| {
            let t = n as f64 / RATE as f64;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let hiss = ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.0006;
            hiss + if t < 2.0 {
                let local = t % 0.25;
                string(82.41, n) * (-(local / 0.06)).exp() as f32
            } else if t < 3.5 {
                string(110.0, n) * (-((t - 2.0) / 0.8)).exp() as f32
            } else if t < 4.5 {
                0.3 * sine(PROBE_BIN as f64 * RATE as f64 / 16_384.0, n)
            } else if n == RATE * 46 / 10 {
                0.5
            } else {
                0.0
            }
        })
        .collect()
}

fn cabinet_ir() -> Vec<f32> {
    (0..RATE / 10)
        .map(|n| {
            let t = n as f64 / RATE as f64;
            let ring = [(110.0, 0.5), (900.0, 0.3), (2_600.0, 0.2), (5_200.0, 0.05)]
                .iter()
                .map(|(hz, a)| a * (std::f64::consts::TAU * hz * t).sin())
                .sum::<f64>();
            (ring * (-t / 0.008).exp()) as f32
        })
        .collect()
}

fn db(power: f64) -> f64 {
    10.0 * power.max(1e-30).log10()
}

fn rms_db(x: &[f32]) -> f64 {
    db(x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64)
}

fn spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let mut bins: Vec<Complex<f64>> = x
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = std::f64::consts::TAU * i as f64 / n as f64;
            let window =
                0.35875 - 0.48829 * w.cos() + 0.14128 * (2.0 * w).cos() - 0.01168 * (3.0 * w).cos();
            Complex::new(f64::from(*s) * window, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut bins);
    bins[..n / 2].iter().map(|c| c.norm_sqr()).collect()
}

fn measure(out: &[f32]) -> BTreeMap<String, f64> {
    let at = |seconds: f64| (seconds * RATE as f64) as usize;
    let mut m = BTreeMap::new();
    m.insert("level_db".into(), rms_db(&out[..at(3.5)]));

    let music = spectrum(&out[at(0.5)..at(0.5) + 131_072]);
    let hz_per_bin = RATE as f64 / 131_072.0;
    let total: f64 = music.iter().sum();
    let band = |lo: f64, hi: f64| {
        db(music
            .iter()
            .enumerate()
            .filter(|(b, _)| (lo..hi).contains(&(*b as f64 * hz_per_bin)))
            .map(|(_, p)| p)
            .sum::<f64>()
            / total)
    };
    m.insert("band_low_db".into(), band(0.0, 250.0));
    m.insert("band_mid_db".into(), band(250.0, 2_500.0));
    m.insert("band_high_db".into(), band(2_500.0, 24_000.0));

    // Non-harmonic (folded) power relative to the probe fundamental.
    let probe = spectrum(&out[at(3.7)..at(3.7) + 16_384]);
    let near = |b: usize, c: usize| b.abs_diff(c) <= 5;
    let fundamental: f64 = (0..probe.len())
        .filter(|&b| near(b, PROBE_BIN))
        .map(|b| probe[b])
        .sum();
    let aliasing: f64 = (0..probe.len())
        .filter(|&b| b > 20 && !(1..=4).any(|k| near(b, k * PROBE_BIN)))
        .map(|b| probe[b])
        .sum();
    m.insert("aliasing_db".into(), db(aliasing / fundamental));

    let impulse = at(4.6);
    let window = &out[impulse..impulse + 4_800];
    let peak = window.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
    let onset = window
        .iter()
        .position(|s| s.abs() >= 0.1 * peak)
        .unwrap_or(0);
    m.insert("latency_samples".into(), onset as f64);

    m.insert("idle_noise_db".into(), rms_db(&out[at(5.3)..at(6.0)]));
    m
}

#[test]
fn full_rig_quality_matches_baseline() {
    let root = std::env::temp_dir().join(format!("aero-quality-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("di.wav");
    let ir = root.join("cab.wav");
    let rendered = root.join("out.wav");
    pedal_engine::audio_file::write_float_wav(&source, 1, RATE as u32, di()).unwrap();
    pedal_engine::audio_file::write_float_wav(&ir, 1, RATE as u32, cabinet_ir()).unwrap();
    let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/NeuralAmpModelerCore/example_models/A2.nam");

    let state = EngineState {
        model: Some(model.canonicalize().unwrap().to_string_lossy().into_owned()),
        cabinet_ir: Some(ir.to_string_lossy().into_owned()),
        cabinet_enabled: true,
        cabinet_low_cut_hz: 90.0,
        cabinet_high_cut_hz: 9_000.0,
        amp_tight_hz: 120.0,
        nam_oversampling: 2,
        dehiss_enabled: true,
        dehiss_threshold_db: -65.0,
        ..EngineState::default()
    };
    pedal_engine::preview::render_source(&state, &rendered, Some(&source), true, None, None)
        .unwrap();
    let out = AudioFile::load(&rendered).unwrap().mono();
    std::fs::remove_dir_all(&root).ok();
    let measured = measure(&out);

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/quality_baseline.json");
    let update = std::env::var("QUALITY_BASELINE").is_ok_and(|v| v == "update");
    if update || !path.exists() {
        let rounded: BTreeMap<_, _> = measured
            .iter()
            .map(|(k, v)| (k.clone(), (v * 100.0).round() / 100.0))
            .collect();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&rounded).unwrap() + "\n",
        )
        .unwrap();
        eprintln!("wrote quality baseline: {rounded:?}");
        return;
    }
    let baseline: BTreeMap<String, f64> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut failures = Vec::new();
    for (name, tolerance, worse_only) in METRICS {
        let (now, before) = (measured[name], baseline[name]);
        let delta = now - before;
        let failed = if worse_only {
            delta > tolerance
        } else {
            delta.abs() > tolerance
        };
        if failed {
            failures.push(format!("{name}: {before:.2} -> {now:.2}"));
        } else if worse_only && delta < -tolerance {
            eprintln!("{name} improved {before:.2} -> {now:.2}; consider updating the baseline");
        }
    }
    assert!(
        failures.is_empty(),
        "audio quality regressed (QUALITY_BASELINE=update to accept an intended change):\n  {}",
        failures.join("\n  ")
    );
}
