//! A tiny click generator kept inside the audio engine, so practice tempo is
//! heard on the same wired output as the guitar without Bluetooth/UI latency.

use crate::dsp::MonoProcessor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const MIN_BPM: u32 = 30;
const MAX_BPM: u32 = 300;
const CLICK_SECONDS: f32 = 0.030;

pub struct MetronomeParameters {
    enabled: AtomicBool,
    bpm: AtomicU32,
    generation: AtomicU64,
}

impl MetronomeParameters {
    pub fn new(enabled: bool, bpm: u32) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            bpm: AtomicU32::new(bpm.clamp(MIN_BPM, MAX_BPM)),
            generation: AtomicU64::new(0),
        }
    }

    pub fn update(&self, enabled: bool, bpm: u32) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.bpm
            .store(bpm.clamp(MIN_BPM, MAX_BPM), Ordering::Relaxed);
        // Starting a new drill should always put beat one at its count-in.
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (bool, u32, u64) {
        (
            self.enabled.load(Ordering::Relaxed),
            self.bpm.load(Ordering::Relaxed),
            self.generation.load(Ordering::Relaxed),
        )
    }
}

pub struct MetronomeProcessor<P> {
    processor: P,
    parameters: Arc<MetronomeParameters>,
    sample_rate: f32,
    generation: u64,
    beat_phase: f32,
    beat_index: u8,
}

impl<P> MetronomeProcessor<P> {
    pub fn new(processor: P, parameters: Arc<MetronomeParameters>, sample_rate: u32) -> Self {
        Self {
            processor,
            parameters,
            sample_rate: sample_rate as f32,
            generation: 0,
            beat_phase: 0.0,
            beat_index: 0,
        }
    }
}

impl<P> MetronomeProcessor<P> {
    fn add_click(&mut self, samples: &mut [f32]) {
        let (enabled, bpm, generation) = self.parameters.snapshot();
        if generation != self.generation {
            self.generation = generation;
            self.beat_phase = 0.0;
            self.beat_index = 0;
        }
        if !enabled {
            return;
        }
        let frames_per_beat = self.sample_rate * 60.0 / bpm as f32;
        let click_frames = (self.sample_rate * CLICK_SECONDS) as usize;
        for sample in samples {
            let frame = self.beat_phase as usize;
            if frame < click_frames {
                let envelope = 1.0 - frame as f32 / click_frames as f32;
                let frequency = if self.beat_index == 0 {
                    1_760.0
                } else {
                    1_180.0
                };
                *sample += (2.0 * std::f32::consts::PI * frequency * frame as f32
                    / self.sample_rate)
                    .sin()
                    * envelope
                    * 0.18;
            }
            self.beat_phase += 1.0;
            if self.beat_phase >= frames_per_beat {
                self.beat_phase -= frames_per_beat;
                self.beat_index = (self.beat_index + 1) % 4;
            }
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for MetronomeProcessor<P> {
    fn discontinuity(&mut self) {
        self.processor.discontinuity();
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.processor.process(samples);
        self.add_click(samples);
    }
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.processor.process_stereo(samples, output);
        let mut click = [0.0; 1024];
        for chunk in output.chunks_mut(1024) {
            click.fill(0.0);
            self.add_click(&mut click[..chunk.len()]);
            for (frame, value) in chunk.iter_mut().zip(click) {
                frame[0] += value;
                frame[1] += value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    #[test]
    fn click_starts_immediately_and_can_be_disabled() {
        let parameters = Arc::new(MetronomeParameters::new(true, 120));
        let mut processor = MetronomeProcessor::new(Passthrough, Arc::clone(&parameters), 48_000);
        let mut audible = [0.0_f32; 64];
        processor.process(&mut audible);
        assert!(audible.iter().any(|sample| sample.abs() > 0.001));

        parameters.update(false, 120);
        let mut silent = [0.0_f32; 64];
        processor.process(&mut silent);
        assert!(silent.iter().all(|sample| *sample == 0.0));
    }
}
