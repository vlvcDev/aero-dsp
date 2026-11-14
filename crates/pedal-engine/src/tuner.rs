//! Clean-input pitch detection for the tuner control surface.
//!
//! The analyser is inactive until the tuner screen opens. It takes a small
//! downsampled snapshot and hands it to a background analyser. Expensive pitch
//! detection never runs in the NAM/audio callback; stale snapshots are replaced.

use crate::dsp::MonoProcessor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const ANALYSIS_FRAMES: usize = 1_024;
const DECIMATION: usize = 4;
const HISTORY_FRAMES: usize = ANALYSIS_FRAMES * DECIMATION;
const ANALYSIS_HOP: usize = 2_048;
// Leave room below A1 for flat tuning and interpolation near the range edge.
const MIN_FREQUENCY_HZ: f32 = 45.0;
const MAX_FREQUENCY_HZ: f32 = 1_000.0;
// Autocorrelation also peaks at whole-number multiples of a period. Prefer the
// first strong local peak so a decaying B/E string does not get reported as a
// lower subharmonic several octaves down.
const FUNDAMENTAL_PEAK_RATIO: f32 = 0.88;

struct AtomicFloat(AtomicU32);

impl AtomicFloat {
    fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

pub struct TunerParameters {
    enabled: AtomicBool,
}

impl TunerParameters {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
        }
    }

    pub fn update(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

pub struct TunerTelemetry {
    frequency_hz: AtomicFloat,
    confidence: AtomicFloat,
}

impl Default for TunerTelemetry {
    fn default() -> Self {
        Self {
            frequency_hz: AtomicFloat::new(0.0),
            confidence: AtomicFloat::new(0.0),
        }
    }
}

impl TunerTelemetry {
    pub fn snapshot(&self) -> (f32, f32) {
        (self.frequency_hz.load(), self.confidence.load())
    }

    fn clear(&self) {
        self.frequency_hz.store(0.0);
        self.confidence.store(0.0);
    }

    fn record(&self, frequency_hz: f32, confidence: f32) {
        self.frequency_hz.store(frequency_hz);
        self.confidence.store(confidence);
    }
}

pub struct TunerTapProcessor<P> {
    processor: P,
    parameters: Arc<TunerParameters>,
    telemetry: Arc<TunerTelemetry>,
    history: Vec<f32>,
    write_index: usize,
    history_filled: usize,
    frames_since_analysis: usize,
    windows: Arc<crossbeam_queue::ArrayQueue<[f32; ANALYSIS_FRAMES]>>,
    alive: Arc<AtomicBool>,
}

impl<P> TunerTapProcessor<P> {
    pub fn new(
        processor: P,
        parameters: Arc<TunerParameters>,
        telemetry: Arc<TunerTelemetry>,
        sample_rate: u32,
    ) -> Self {
        let windows = Arc::new(crossbeam_queue::ArrayQueue::new(2));
        let alive = Arc::new(AtomicBool::new(true));
        let worker_windows = Arc::clone(&windows);
        let worker_alive = Arc::clone(&alive);
        let worker_parameters = Arc::clone(&parameters);
        let worker_telemetry = Arc::clone(&telemetry);
        std::thread::Builder::new()
            .name("pitch-analysis".into())
            .spawn(move || {
                while worker_alive.load(Ordering::Acquire) {
                    if let Some(window) = worker_windows.pop() {
                        let (hz, confidence) = analyse(&window, sample_rate as f32);
                        if worker_alive.load(Ordering::Acquire) && worker_parameters.enabled() {
                            worker_telemetry.record(hz, confidence);
                        }
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
            })
            .expect("start pitch analysis worker");
        Self {
            processor,
            parameters,
            telemetry,
            history: vec![0.0; HISTORY_FRAMES],
            write_index: 0,
            history_filled: 0,
            frames_since_analysis: 0,
            windows,
            alive,
        }
    }

    fn capture(&mut self, samples: &[f32]) {
        if !self.parameters.enabled() {
            self.frames_since_analysis = 0;
            self.history_filled = 0;
            while self.windows.pop().is_some() {}
            self.telemetry.clear();
            return;
        }
        for sample in samples {
            self.history[self.write_index] = *sample;
            self.write_index = (self.write_index + 1) % self.history.len();
            self.history_filled = (self.history_filled + 1).min(self.history.len());
        }
        self.frames_since_analysis += samples.len();
        if self.history_filled == self.history.len() && self.frames_since_analysis >= ANALYSIS_HOP {
            self.frames_since_analysis = 0;
            let mut window = [0.0_f32; ANALYSIS_FRAMES];
            for (index, sample) in window.iter_mut().enumerate() {
                let source = (self.write_index + index * DECIMATION) % self.history.len();
                *sample = self.history[source];
            }
            self.windows.force_push(window);
        }
    }
}

fn analyse(window: &[f32; ANALYSIS_FRAMES], sample_rate: f32) -> (f32, f32) {
    let mean = window.iter().sum::<f32>() / ANALYSIS_FRAMES as f32;
    let window = window.map(|s| s - mean);
    let rms =
        (window.iter().map(|sample| sample * sample).sum::<f32>() / ANALYSIS_FRAMES as f32).sqrt();
    if rms < 0.0025 {
        return (0.0, 0.0);
    }

    let analysis_rate = sample_rate / DECIMATION as f32;
    let minimum_lag = ((analysis_rate / MAX_FREQUENCY_HZ) as usize).clamp(1, ANALYSIS_FRAMES - 2);
    let maximum_lag =
        ((analysis_rate / MIN_FREQUENCY_HZ) as usize).clamp(minimum_lag, ANALYSIS_FRAMES - 1);
    let lag_count = maximum_lag - minimum_lag + 1;
    let mut scores = [0.0_f32; ANALYSIS_FRAMES];
    let mut strongest_lag = minimum_lag;
    let mut best_score = -1.0_f32;
    for lag in minimum_lag..=maximum_lag {
        let mut cross = 0.0;
        let mut left_energy = 0.0;
        let mut right_energy = 0.0;
        for index in lag..ANALYSIS_FRAMES {
            let left = window[index];
            let right = window[index - lag];
            cross += left * right;
            left_energy += left * left;
            right_energy += right * right;
        }
        let score = cross / (left_energy * right_energy).sqrt().max(0.000_001);
        scores[lag - minimum_lag] = score;
        if score > best_score {
            best_score = score;
            strongest_lag = lag;
        }
    }
    if best_score < 0.55 {
        return (0.0, 0.0);
    }
    let fundamental_threshold = (best_score * FUNDAMENTAL_PEAK_RATIO).max(0.55);
    let best_offset = strongest_lag - minimum_lag;
    let fundamental_offset = (1..lag_count.saturating_sub(1))
        .find(|&offset| {
            let score = scores[offset];
            score >= fundamental_threshold
                && score >= scores[offset - 1]
                && score > scores[offset + 1]
        })
        .unwrap_or(best_offset);
    let best_lag = minimum_lag + fundamental_offset;
    let previous = fundamental_offset
        .checked_sub(1)
        .map(|offset| scores[offset])
        .unwrap_or(-1.0);
    let after = if fundamental_offset + 1 < lag_count {
        scores[fundamental_offset + 1]
    } else {
        -1.0
    };
    let refinement = if previous > -0.5 && after > -0.5 {
        (0.5 * (previous - after) / (previous - 2.0 * scores[fundamental_offset] + after))
            .clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let frequency = analysis_rate / (best_lag as f32 + refinement);
    (frequency, scores[fundamental_offset])
}

impl<P> Drop for TunerTapProcessor<P> {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl<P: MonoProcessor> MonoProcessor for TunerTapProcessor<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.capture(samples);
        self.processor.process_stereo(samples, output);
    }
    fn process(&mut self, samples: &mut [f32]) {
        // Capture raw input before applying any NAM/effect processor.
        self.capture(samples);
        self.processor.process(samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    fn detect_pitch(frequency_hz: f32) -> (f32, f32) {
        let parameters = Arc::new(TunerParameters::new(true));
        let telemetry = Arc::new(TunerTelemetry::default());
        let mut tuner =
            TunerTapProcessor::new(Passthrough, parameters, Arc::clone(&telemetry), 48_000);
        // A short decaying harmonic-rich waveform is closer to a picked guitar
        // string than a mathematical sine, particularly on B and high E.
        let mut samples = (0..12_000)
            .map(|index| {
                let time = index as f32 / 48_000.0;
                let phase = time * frequency_hz * std::f32::consts::TAU;
                let envelope = (-time * 2.0).exp();
                envelope
                    * (phase.sin() * 0.22 + (phase * 2.0).sin() * 0.08 + (phase * 3.0).sin() * 0.03)
            })
            .collect::<Vec<_>>();
        for block in samples.chunks_mut(64) {
            tuner.process(block);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while telemetry.snapshot().0 == 0.0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        telemetry.snapshot()
    }

    #[test]
    fn detects_a_clean_low_e_when_enabled() {
        let (frequency, confidence) = detect_pitch(82.41);
        assert!((frequency - 82.41).abs() < 2.0, "detected {frequency}");
        assert!(confidence > 0.7);
    }

    #[test]
    fn detects_seven_string_low_notes_and_semitone_flat_tunings() {
        for sample_rate in [44_100.0, 48_000.0, 96_000.0] {
            for nominal_hz in [61.735_f32, 55.0] {
                for expected in [nominal_hz, nominal_hz * 2.0_f32.powf(-1.0 / 12.0)] {
                    let window = std::array::from_fn(|index| {
                        let time = (index * DECIMATION) as f32 / sample_rate;
                        let phase = time * expected * std::f32::consts::TAU;
                        let envelope = (-time * 2.0).exp();
                        envelope
                            * (phase.sin() * 0.22
                                + (phase * 2.0).sin() * 0.08
                                + (phase * 3.0).sin() * 0.03)
                    });
                    let (frequency, confidence) = analyse(&window, sample_rate);
                    let cents_error = 1200.0 * (frequency / expected).log2();
                    assert!(
                        cents_error.abs() < 3.0,
                        "{sample_rate} Hz: expected {expected} Hz, detected {frequency} Hz ({cents_error} cents)"
                    );
                    assert!(confidence > 0.7, "confidence was {confidence}");
                }
            }
        }
    }

    #[test]
    fn detects_b_and_high_e_without_falling_to_a_subharmonic() {
        for expected in [246.94, 329.63] {
            let (frequency, confidence) = detect_pitch(expected);
            assert!(
                (frequency - expected).abs() < 4.0,
                "expected {expected} Hz, detected {frequency} Hz"
            );
            assert!(confidence > 0.6, "confidence was {confidence}");
        }
    }

    #[test]
    fn does_not_report_a_pitch_while_disabled() {
        let parameters = Arc::new(TunerParameters::new(false));
        let telemetry = Arc::new(TunerTelemetry::default());
        let mut tuner =
            TunerTapProcessor::new(Passthrough, parameters, Arc::clone(&telemetry), 48_000);
        let mut samples = vec![0.25; HISTORY_FRAMES];
        tuner.process(&mut samples);
        assert_eq!(telemetry.snapshot(), (0.0, 0.0));
    }
}
