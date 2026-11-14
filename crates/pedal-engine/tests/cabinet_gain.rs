use pedal_engine::{cabinet::CabinetNode, node_rig::StereoNode};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

fn load(rate: u32, host_rate: u32, channels: &[Vec<f32>]) -> CabinetNode {
    let path = std::env::temp_dir().join(format!(
        "cabinet-gain-{}-{}.wav",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..channels[0].len() {
        for channel in channels {
            writer.write_sample(channel[i]).unwrap();
        }
    }
    writer.finalize().unwrap();
    let node = CabinetNode::load(&path, host_rate, 0.2).unwrap();
    std::fs::remove_file(path).unwrap();
    node
}

// The same smooth, delayed continuous-time impulse sampled at different rates.
fn physical_ir(rate: u32, level: f32) -> Vec<f32> {
    (0..(rate / 100) as usize)
        .map(|n| {
            let t = n as f32 / rate as f32;
            let x = (t - 0.002) / 0.00008;
            level * (48_000.0 / rate as f32) * (-0.5 * x * x).exp()
        })
        .collect()
}

fn response(mut node: CabinetNode, samples: usize) -> Vec<[f32; 2]> {
    assert_eq!(node.latency_samples(), 0);
    (0..samples)
        .map(|i| node.tick(if i == 0 { [1.0; 2] } else { [0.0; 2] }))
        .collect()
}

fn magnitude(response: &[[f32; 2]], rate: u32, hz: f64, channel: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (n, frame) in response.iter().enumerate() {
        let phase = std::f64::consts::TAU * hz * n as f64 / rate as f64;
        re += frame[channel] as f64 * phase.cos();
        im -= frame[channel] as f64 * phase.sin();
    }
    re.hypot(im)
}

fn assert_db_close(a: f64, b: f64, tolerance: f64) {
    let difference = 20.0 * (a / b).log10();
    assert!(difference.abs() < tolerance, "{a} vs {b}: {difference} dB");
}

#[test]
fn physical_ir_has_stable_level_and_frequency_balance_across_file_rates() {
    let responses: Vec<_> = [44_100, 48_000, 96_000]
        .into_iter()
        .map(|rate| response(load(rate, 48_000, &[physical_ir(rate, 0.02)]), 1024))
        .collect();
    for hz in [100.0, 500.0, 1000.0, 3000.0, 5000.0] {
        let reference = magnitude(&responses[1], 48_000, hz, 0);
        for actual in [&responses[0], &responses[2]] {
            assert_db_close(magnitude(actual, 48_000, hz, 0), reference, 0.1);
        }
    }
    // A scalar gain must preserve the IR's relative spectral shape.
    let low = magnitude(&responses[1], 48_000, 100.0, 0);
    let high = magnitude(&responses[1], 48_000, 5000.0, 0);
    let raw: Vec<_> = physical_ir(48_000, 0.02)
        .into_iter()
        .map(|x| [x; 2])
        .collect();
    assert_db_close(
        high / low,
        magnitude(&raw, 48_000, 5000.0, 0) / magnitude(&raw, 48_000, 100.0, 0),
        0.01,
    );
}

#[test]
fn hot_scaled_ir_is_normalized_by_energy_not_peak() {
    let a = response(load(48_000, 48_000, &[physical_ir(48_000, 2.0)]), 1024);
    let b = response(load(48_000, 48_000, &[physical_ir(48_000, 8.0)]), 1024);
    for hz in [100.0, 1000.0, 5000.0] {
        assert_db_close(
            magnitude(&a, 48_000, hz, 0),
            magnitude(&b, 48_000, hz, 0),
            0.01,
        );
    }
}

#[test]
fn quiet_ir_receives_only_fixed_pad_and_has_no_added_latency() {
    let pad = 10.0_f32.powf(-18.0 / 20.0);
    let mut node = load(48_000, 48_000, &[vec![0.25, -0.125]]);
    assert_eq!(node.latency_samples(), 0);
    let first = node.tick([1.0, 1.0]);
    let second = node.tick([0.0, 0.0]);
    for channel in 0..2 {
        assert!((first[channel] - 0.25 * pad).abs() < 1e-6);
        assert!((second[channel] + 0.125 * pad).abs() < 1e-6);
    }
}

#[test]
fn linked_stereo_normalization_preserves_balance_and_mono_level() {
    let left = physical_ir(48_000, 2.0);
    let right: Vec<_> = left.iter().map(|v| v * 0.25).collect();
    let mono = response(load(48_000, 48_000, std::slice::from_ref(&left)), 1024);
    let stereo = response(load(48_000, 48_000, &[left.clone(), right]), 1024);
    let doubled = response(load(48_000, 48_000, &[left.clone(), left]), 1024);
    for hz in [100.0, 1000.0, 5000.0] {
        let m = magnitude(&mono, 48_000, hz, 0);
        assert_db_close(magnitude(&stereo, 48_000, hz, 0), m, 0.01);
        assert_db_close(magnitude(&stereo, 48_000, hz, 1), m * 0.25, 0.01);
        assert_db_close(magnitude(&doubled, 48_000, hz, 0), m, 0.01);
        assert_db_close(magnitude(&doubled, 48_000, hz, 1), m, 0.01);
    }
}

#[test]
fn hot_physical_ir_level_is_consistent_at_different_host_rates() {
    let a = response(load(48_000, 48_000, &[physical_ir(48_000, 2.0)]), 1024);
    let b = response(load(96_000, 96_000, &[physical_ir(96_000, 2.0)]), 2048);
    for hz in [100.0, 1000.0, 5000.0] {
        assert_db_close(
            magnitude(&a, 48_000, hz, 0),
            magnitude(&b, 96_000, hz, 0),
            0.05,
        );
    }
}

#[test]
fn energy_normalization_applies_below_peak_limit() {
    let output = response(load(48_000, 48_000, &[physical_ir(48_000, 0.8)]), 1024);
    let energy: f32 = output.iter().map(|frame| frame[0] * frame[0]).sum();
    let expected = 10.0_f32.powf(-18.0 / 10.0);
    assert!(
        (energy - expected).abs() < 1e-5,
        "energy {energy}, expected {expected}"
    );
}

#[test]
fn resampled_hot_ir_level_is_consistent_between_host_rates() {
    let ir = physical_ir(48_000, 2.0);
    let a = response(load(48_000, 48_000, std::slice::from_ref(&ir)), 1024);
    let b = response(load(48_000, 96_000, &[ir]), 2048);
    for hz in [100.0, 1000.0, 5000.0] {
        assert_db_close(
            magnitude(&a, 48_000, hz, 0),
            magnitude(&b, 96_000, hz, 0),
            0.1,
        );
    }
}
