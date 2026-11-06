//! Offline CPU benchmark, not a measurement of interface round-trip latency.
use pedal_engine::{convolution::Convolver, reverb::Reverb, tone_eq::ToneEq};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    const RATE: usize = 48_000;
    const BLOCK: usize = 64;
    let clean_octave = std::env::args().any(|arg| arg == "--clean-octave");
    let mut octaver = pedal_engine::octaver::Octaver::new(RATE as u32);
    let impulse: Vec<_> = (0..RATE / 5)
        .map(|i| {
            if i == 0 {
                1.0
            } else {
                0.01 * (i as f32 * 0.37).sin() * (-(i as f32) / 2400.0).exp()
            }
        })
        .collect();
    let mut cabinet = Convolver::new(&impulse);
    let mut reverb = Reverb::new(RATE as u32);
    let mut eq = ToneEq::new(RATE as u32, [3.0, -2.0, 4.0]);
    let mut times = Vec::with_capacity(RATE * 30 / BLOCK);
    let mut checksum = 0.0_f64;
    for block in 0..RATE * 30 / BLOCK {
        let mut audio = [0.0_f32; BLOCK];
        for (offset, sample) in audio.iter_mut().enumerate() {
            *sample = ((block * BLOCK + offset) as f32 * 0.057).sin() * 0.1;
        }
        let started = Instant::now();
        if clean_octave {
            octaver.process(&mut audio, true, true, 0.5);
        }
        for sample in &mut audio {
            let dry = eq.tick(cabinet.tick(*sample));
            *sample = dry + reverb.tick(dry) * 0.2;
        }
        times.push(started.elapsed().as_secs_f64());
        for sample in black_box(audio) {
            assert!(sample.is_finite());
            checksum += sample as f64;
        }
    }
    times.sort_by(f64::total_cmp);
    let deadline = BLOCK as f64 / RATE as f64;
    let percentile = |p: f64| times[((times.len() - 1) as f64 * p) as usize] / deadline * 100.0;
    println!(
        "48k/64: 200ms mono IR + three-band EQ + FDN reverb; 30s synthetic audio (NO NAM, device or recording)."
    );
    if clean_octave {
        println!(
            "Includes clean octaver; octave-voice latency {:.2} ms",
            pedal_engine::octaver::clean_latency_frames(RATE as u32) as f64 / RATE as f64 * 1000.0
        );
    }
    println!(
        "DSP deadline usage p50 {:.2}%, p99 {:.2}%, p99.9 {:.2}%, max {:.2}%; misses {}; checksum {:.4}",
        percentile(0.5),
        percentile(0.99),
        percentile(0.999),
        percentile(1.0),
        times.iter().filter(|t| **t > deadline).count(),
        checksum
    );
}
