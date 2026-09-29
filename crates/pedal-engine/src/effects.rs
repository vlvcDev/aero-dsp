//! Lightweight, allocation-free guitar effects for the real-time signal path.
//!
//! The processors deliberately use small, predictable algorithms so the NAM
//! models remain the expensive part of the rig on a Raspberry Pi.

use crate::EngineState;
use crate::dsp::MonoProcessor;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

fn gain_from_db(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LooperMode {
    Stopped = 0,
    Recording = 1,
    Playing = 2,
    Overdubbing = 3,
    CountIn = 4,
}

impl LooperMode {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Recording,
            2 => Self::Playing,
            3 => Self::Overdubbing,
            4 => Self::CountIn,
            _ => Self::Stopped,
        }
    }

    pub fn from_state(value: &str) -> Self {
        match value {
            "recording" => Self::Recording,
            "playing" => Self::Playing,
            "overdubbing" => Self::Overdubbing,
            "count_in" => Self::CountIn,
            _ => Self::Stopped,
        }
    }

    pub fn as_state(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Recording => "recording",
            Self::Playing => "playing",
            Self::Overdubbing => "overdubbing",
            Self::CountIn => "count_in",
        }
    }
}

/// Controls are atomics so updates never lock the audio callback.
pub struct EffectsParameters {
    gate_enabled: AtomicBool,
    gate_threshold_db: AtomicFloat,
    compressor_enabled: AtomicBool,
    compressor_threshold_db: AtomicFloat,
    compressor_ratio: AtomicFloat,
    eq_enabled: AtomicBool,
    eq_low_db: AtomicFloat,
    eq_mid_db: AtomicFloat,
    eq_high_db: AtomicFloat,
    chorus_enabled: AtomicBool,
    chorus_rate_hz: AtomicFloat,
    chorus_depth: AtomicFloat,
    chorus_mix: AtomicFloat,
    flanger_enabled: AtomicBool,
    flanger_rate_hz: AtomicFloat,
    flanger_depth: AtomicFloat,
    flanger_feedback: AtomicFloat,
    flanger_mix: AtomicFloat,
    tremolo_enabled: AtomicBool,
    tremolo_rate_hz: AtomicFloat,
    tremolo_depth: AtomicFloat,
    octaver_enabled: AtomicBool,
    octaver_mix: AtomicFloat,
    delay_enabled: AtomicBool,
    delay_time_ms: AtomicFloat,
    delay_feedback: AtomicFloat,
    delay_mix: AtomicFloat,
    reverb_enabled: AtomicBool,
    reverb_decay_seconds: AtomicFloat,
    reverb_mix: AtomicFloat,
    looper_mode: AtomicU8,
    looper_bpm: AtomicU32,
    looper_bars: AtomicU8,
    looper_generation: AtomicU64,
}

impl EffectsParameters {
    pub fn from_state(state: &EngineState) -> Self {
        Self {
            gate_enabled: AtomicBool::new(state.gate_enabled),
            gate_threshold_db: AtomicFloat::new(state.gate_threshold_db),
            compressor_enabled: AtomicBool::new(state.compressor_enabled),
            compressor_threshold_db: AtomicFloat::new(state.compressor_threshold_db),
            compressor_ratio: AtomicFloat::new(state.compressor_ratio),
            eq_enabled: AtomicBool::new(state.eq_enabled),
            eq_low_db: AtomicFloat::new(state.eq_low_db),
            eq_mid_db: AtomicFloat::new(state.eq_mid_db),
            eq_high_db: AtomicFloat::new(state.eq_high_db),
            chorus_enabled: AtomicBool::new(state.chorus_enabled),
            chorus_rate_hz: AtomicFloat::new(state.chorus_rate_hz),
            chorus_depth: AtomicFloat::new(state.chorus_depth),
            chorus_mix: AtomicFloat::new(state.chorus_mix),
            flanger_enabled: AtomicBool::new(state.flanger_enabled),
            flanger_rate_hz: AtomicFloat::new(state.flanger_rate_hz),
            flanger_depth: AtomicFloat::new(state.flanger_depth),
            flanger_feedback: AtomicFloat::new(state.flanger_feedback),
            flanger_mix: AtomicFloat::new(state.flanger_mix),
            tremolo_enabled: AtomicBool::new(state.tremolo_enabled),
            tremolo_rate_hz: AtomicFloat::new(state.tremolo_rate_hz),
            tremolo_depth: AtomicFloat::new(state.tremolo_depth),
            octaver_enabled: AtomicBool::new(state.octaver_enabled),
            octaver_mix: AtomicFloat::new(state.octaver_mix),
            delay_enabled: AtomicBool::new(state.delay_enabled),
            delay_time_ms: AtomicFloat::new(state.delay_time_ms),
            delay_feedback: AtomicFloat::new(state.delay_feedback),
            delay_mix: AtomicFloat::new(state.delay_mix),
            reverb_enabled: AtomicBool::new(state.reverb_enabled),
            reverb_decay_seconds: AtomicFloat::new(state.reverb_decay_seconds),
            reverb_mix: AtomicFloat::new(state.reverb_mix),
            looper_mode: AtomicU8::new(LooperMode::from_state(&state.looper_mode) as u8),
            looper_bpm: AtomicU32::new(state.looper_bpm),
            looper_bars: AtomicU8::new(state.looper_bars),
            looper_generation: AtomicU64::new(0),
        }
    }

    pub fn update(&self, state: &EngineState) {
        self.gate_enabled
            .store(state.gate_enabled, Ordering::Relaxed);
        self.gate_threshold_db.store(state.gate_threshold_db);
        self.compressor_enabled
            .store(state.compressor_enabled, Ordering::Relaxed);
        self.compressor_threshold_db
            .store(state.compressor_threshold_db);
        self.compressor_ratio.store(state.compressor_ratio);
        self.eq_enabled.store(state.eq_enabled, Ordering::Relaxed);
        self.eq_low_db.store(state.eq_low_db);
        self.eq_mid_db.store(state.eq_mid_db);
        self.eq_high_db.store(state.eq_high_db);
        self.chorus_enabled
            .store(state.chorus_enabled, Ordering::Relaxed);
        self.chorus_rate_hz.store(state.chorus_rate_hz);
        self.chorus_depth.store(state.chorus_depth);
        self.chorus_mix.store(state.chorus_mix);
        self.flanger_enabled
            .store(state.flanger_enabled, Ordering::Relaxed);
        self.flanger_rate_hz.store(state.flanger_rate_hz);
        self.flanger_depth.store(state.flanger_depth);
        self.flanger_feedback.store(state.flanger_feedback);
        self.flanger_mix.store(state.flanger_mix);
        self.tremolo_enabled
            .store(state.tremolo_enabled, Ordering::Relaxed);
        self.tremolo_rate_hz.store(state.tremolo_rate_hz);
        self.tremolo_depth.store(state.tremolo_depth);
        self.octaver_enabled
            .store(state.octaver_enabled, Ordering::Relaxed);
        self.octaver_mix.store(state.octaver_mix);
        self.delay_enabled
            .store(state.delay_enabled, Ordering::Relaxed);
        self.delay_time_ms.store(state.delay_time_ms);
        self.delay_feedback.store(state.delay_feedback);
        self.delay_mix.store(state.delay_mix);
        self.reverb_enabled
            .store(state.reverb_enabled, Ordering::Relaxed);
        self.reverb_decay_seconds.store(state.reverb_decay_seconds);
        self.reverb_mix.store(state.reverb_mix);
    }

    pub fn start_quantized_loop(&self, bpm: u32, bars: u8) {
        self.looper_bpm.store(bpm.clamp(30, 300), Ordering::Relaxed);
        self.looper_bars.store(bars, Ordering::Relaxed);
        self.looper_mode
            .store(LooperMode::CountIn as u8, Ordering::Relaxed);
        self.looper_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_looper_mode(&self, mode: LooperMode) {
        self.looper_mode.store(mode as u8, Ordering::Relaxed);
        self.looper_generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn looper_mode(&self) -> LooperMode {
        LooperMode::from_u8(self.looper_mode.load(Ordering::Relaxed))
    }

    fn looper_schedule(&self) -> (LooperMode, u32, u8, u64) {
        (
            self.looper_mode(),
            self.looper_bpm.load(Ordering::Relaxed),
            self.looper_bars.load(Ordering::Relaxed),
            self.looper_generation.load(Ordering::Relaxed),
        )
    }
}

pub struct EffectChainProcessor<P> {
    source: P,
    parameters: Arc<EffectsParameters>,
    sample_rate: f32,
    gate_gain: f32,
    compressor_envelope: f32,
    compressor_gain: f32,
    eq_low_state: f32,
    eq_high_state: f32,
    chorus_buffer: Vec<f32>,
    chorus_write: usize,
    chorus_phase: f32,
    flanger_buffer: Vec<f32>,
    flanger_write: usize,
    flanger_phase: f32,
    tremolo_phase: f32,
    octaver_last_input: f32,
    octaver_polarity: f32,
    delay_buffer: Vec<f32>,
    delay_write: usize,
    reverb_buffers: [Vec<f32>; 4],
    reverb_indices: [usize; 4],
    looper_buffer: Vec<f32>,
    looper_position: usize,
    looper_length: usize,
    previous_looper_mode: LooperMode,
    looper_generation: u64,
    count_in_remaining: usize,
    count_in_elapsed: usize,
    looper_target_length: usize,
}

impl<P> EffectChainProcessor<P> {
    pub fn new(
        source: P,
        parameters: Arc<EffectsParameters>,
        sample_rate: u32,
        max_loop_seconds: usize,
    ) -> Self {
        let sample_rate_f32 = sample_rate as f32;
        let make_reverb_line = |seconds: f32| vec![0.0; (seconds * sample_rate_f32) as usize];
        Self {
            source,
            parameters,
            sample_rate: sample_rate_f32,
            gate_gain: 1.0,
            compressor_envelope: 0.0,
            compressor_gain: 1.0,
            eq_low_state: 0.0,
            eq_high_state: 0.0,
            chorus_buffer: vec![0.0; (sample_rate_f32 * 0.04) as usize],
            chorus_write: 0,
            chorus_phase: 0.0,
            flanger_buffer: vec![0.0; (sample_rate_f32 * 0.012) as usize],
            flanger_write: 0,
            flanger_phase: 0.0,
            tremolo_phase: 0.0,
            octaver_last_input: 0.0,
            octaver_polarity: 1.0,
            delay_buffer: vec![0.0; (sample_rate_f32 * 2.0) as usize],
            delay_write: 0,
            reverb_buffers: [
                make_reverb_line(0.0297),
                make_reverb_line(0.0371),
                make_reverb_line(0.0411),
                make_reverb_line(0.0437),
            ],
            reverb_indices: [0; 4],
            looper_buffer: vec![0.0; sample_rate as usize * max_loop_seconds],
            looper_position: 0,
            looper_length: 0,
            previous_looper_mode: LooperMode::Stopped,
            looper_generation: 0,
            count_in_remaining: 0,
            count_in_elapsed: 0,
            looper_target_length: 0,
        }
    }

    fn looper_mode(&self) -> LooperMode {
        self.parameters.looper_mode()
    }

    fn process_gate(&mut self, samples: &mut [f32]) {
        if !self.parameters.gate_enabled.load(Ordering::Relaxed) {
            self.gate_gain = 1.0;
            return;
        }
        let threshold = gain_from_db(self.parameters.gate_threshold_db.load());
        for sample in samples {
            let target = if sample.abs() >= threshold { 1.0 } else { 0.0 };
            let smoothing = if target > self.gate_gain {
                0.08
            } else {
                0.0025
            };
            self.gate_gain += (target - self.gate_gain) * smoothing;
            *sample *= self.gate_gain;
        }
    }

    /// The gate listens to the clean guitar input, then applies the same gain
    /// after the NAM chain. That means an amp capture's idle hiss is silenced
    /// when the player is not touching the strings, instead of being allowed
    /// through merely because it was generated by the model.
    fn apply_gate_to_model_output(&self, samples: &mut [f32]) {
        if self.parameters.gate_enabled.load(Ordering::Relaxed) {
            for sample in samples {
                *sample *= self.gate_gain;
            }
        }
    }

    fn process_compressor(&mut self, samples: &mut [f32]) {
        if !self.parameters.compressor_enabled.load(Ordering::Relaxed) {
            self.compressor_gain = 1.0;
            return;
        }
        let threshold = gain_from_db(self.parameters.compressor_threshold_db.load());
        let ratio = self.parameters.compressor_ratio.load().max(1.0);
        for sample in samples {
            let level = sample.abs();
            let envelope_rate = if level > self.compressor_envelope {
                0.05
            } else {
                0.0015
            };
            self.compressor_envelope += (level - self.compressor_envelope) * envelope_rate;
            let target = if self.compressor_envelope > threshold {
                (threshold / self.compressor_envelope).powf(1.0 - 1.0 / ratio)
            } else {
                1.0
            };
            self.compressor_gain += (target - self.compressor_gain) * 0.025;
            *sample *= self.compressor_gain;
        }
    }

    fn process_eq(&mut self, samples: &mut [f32]) {
        if !self.parameters.eq_enabled.load(Ordering::Relaxed) {
            return;
        }
        let low_gain = gain_from_db(self.parameters.eq_low_db.load());
        let mid_gain = gain_from_db(self.parameters.eq_mid_db.load());
        let high_gain = gain_from_db(self.parameters.eq_high_db.load());
        let low_alpha = 1.0 - (-TAU * 250.0 / self.sample_rate).exp();
        let high_alpha = 1.0 - (-TAU * 2_500.0 / self.sample_rate).exp();
        for sample in samples {
            let input = *sample;
            self.eq_low_state += low_alpha * (input - self.eq_low_state);
            self.eq_high_state += high_alpha * (input - self.eq_high_state);
            let low = self.eq_low_state;
            let high = input - self.eq_high_state;
            let mid = input - low - high;
            *sample = low * low_gain + mid * mid_gain + high * high_gain;
        }
    }

    fn process_chorus(&mut self, samples: &mut [f32]) {
        if !self.parameters.chorus_enabled.load(Ordering::Relaxed) {
            return;
        }
        let rate = self.parameters.chorus_rate_hz.load().clamp(0.05, 8.0);
        let depth = self.parameters.chorus_depth.load().clamp(0.0, 1.0);
        let mix = self.parameters.chorus_mix.load().clamp(0.0, 1.0);
        let base_delay = self.sample_rate * 0.014;
        let depth_samples = self.sample_rate * 0.009 * depth;
        for sample in samples {
            let delay = base_delay + depth_samples * (self.chorus_phase.sin() + 1.0) * 0.5;
            let read = (self.chorus_write + self.chorus_buffer.len() - delay as usize)
                % self.chorus_buffer.len();
            let next = (read + 1) % self.chorus_buffer.len();
            let fraction = delay.fract();
            let delayed =
                self.chorus_buffer[read] * (1.0 - fraction) + self.chorus_buffer[next] * fraction;
            self.chorus_buffer[self.chorus_write] = *sample;
            self.chorus_write = (self.chorus_write + 1) % self.chorus_buffer.len();
            self.chorus_phase = (self.chorus_phase + TAU * rate / self.sample_rate) % TAU;
            *sample = *sample * (1.0 - mix) + delayed * mix;
        }
    }

    /// A short modulated delay with feedback for the classic comb-filter
    /// sweep. Its buffer is preallocated once and never touches the allocator
    /// from the audio callback.
    fn process_flanger(&mut self, samples: &mut [f32]) {
        if !self.parameters.flanger_enabled.load(Ordering::Relaxed) {
            return;
        }
        let rate = self.parameters.flanger_rate_hz.load().clamp(0.05, 5.0);
        let depth = self.parameters.flanger_depth.load().clamp(0.0, 1.0);
        let feedback = self.parameters.flanger_feedback.load().clamp(-0.85, 0.85);
        let mix = self.parameters.flanger_mix.load().clamp(0.0, 1.0);
        let base_delay = self.sample_rate * 0.0015;
        let depth_samples = self.sample_rate * 0.0045 * depth;
        for sample in samples {
            let delay = base_delay + depth_samples * (self.flanger_phase.sin() + 1.0) * 0.5;
            let whole = delay as usize;
            let read = (self.flanger_write + self.flanger_buffer.len() - whole)
                % self.flanger_buffer.len();
            let next = (read + 1) % self.flanger_buffer.len();
            let delayed = self.flanger_buffer[read] * (1.0 - delay.fract())
                + self.flanger_buffer[next] * delay.fract();
            self.flanger_buffer[self.flanger_write] = *sample + delayed * feedback;
            self.flanger_write = (self.flanger_write + 1) % self.flanger_buffer.len();
            self.flanger_phase = (self.flanger_phase + TAU * rate / self.sample_rate) % TAU;
            *sample = *sample * (1.0 - mix) + delayed * mix;
        }
    }

    fn process_tremolo(&mut self, samples: &mut [f32]) {
        if !self.parameters.tremolo_enabled.load(Ordering::Relaxed) {
            return;
        }
        let rate = self.parameters.tremolo_rate_hz.load().clamp(0.1, 18.0);
        let depth = self.parameters.tremolo_depth.load().clamp(0.0, 1.0);
        for sample in samples {
            let lfo = (self.tremolo_phase.sin() + 1.0) * 0.5;
            let gain = 1.0 - depth + depth * lfo;
            *sample *= gain;
            self.tremolo_phase = (self.tremolo_phase + TAU * rate / self.sample_rate) % TAU;
        }
    }

    /// Lightweight analog-style octave-down. It derives a sub-octave square
    /// wave by flipping polarity on each rising zero crossing, then blends it
    /// with the dry guitar before the NAM chain.
    fn process_octaver(&mut self, samples: &mut [f32]) {
        if !self.parameters.octaver_enabled.load(Ordering::Relaxed) {
            return;
        }
        let mix = self.parameters.octaver_mix.load().clamp(0.0, 1.0);
        for sample in samples {
            let dry = *sample;
            if self.octaver_last_input <= 0.0 && dry > 0.0 {
                self.octaver_polarity = -self.octaver_polarity;
            }
            self.octaver_last_input = dry;
            let octave_down = dry.abs() * self.octaver_polarity;
            *sample = dry * (1.0 - mix) + octave_down * mix;
        }
    }

    fn process_delay(&mut self, samples: &mut [f32]) {
        if !self.parameters.delay_enabled.load(Ordering::Relaxed) {
            return;
        }
        let delay_samples = (self.parameters.delay_time_ms.load().clamp(20.0, 1_800.0)
            * self.sample_rate
            / 1_000.0) as usize;
        let feedback = self.parameters.delay_feedback.load().clamp(0.0, 0.92);
        let mix = self.parameters.delay_mix.load().clamp(0.0, 1.0);
        for sample in samples {
            let read = (self.delay_write + self.delay_buffer.len() - delay_samples)
                % self.delay_buffer.len();
            let delayed = self.delay_buffer[read];
            self.delay_buffer[self.delay_write] = *sample + delayed * feedback;
            self.delay_write = (self.delay_write + 1) % self.delay_buffer.len();
            *sample = *sample * (1.0 - mix) + delayed * mix;
        }
    }

    fn process_reverb(&mut self, samples: &mut [f32]) {
        if !self.parameters.reverb_enabled.load(Ordering::Relaxed) {
            return;
        }
        let decay = self.parameters.reverb_decay_seconds.load().clamp(0.3, 12.0);
        let mix = self.parameters.reverb_mix.load().clamp(0.0, 1.0);
        let feedbacks = self
            .reverb_buffers
            .each_ref()
            .map(|line| 10.0_f32.powf(-3.0 * line.len() as f32 / self.sample_rate / decay));
        for sample in samples {
            let dry = *sample;
            let mut wet = 0.0;
            for ((line, index), feedback) in self
                .reverb_buffers
                .iter_mut()
                .zip(self.reverb_indices.iter_mut())
                .zip(feedbacks)
            {
                let delayed = line[*index];
                line[*index] = dry + delayed * feedback;
                *index = (*index + 1) % line.len();
                wet += delayed;
            }
            *sample = dry * (1.0 - mix) + wet * 0.25 * mix;
        }
    }

    fn process_looper(&mut self, samples: &mut [f32]) {
        let (_, bpm, bars, generation) = self.parameters.looper_schedule();
        if generation != self.looper_generation {
            self.looper_generation = generation;
            let frames_per_beat = (self.sample_rate * 60.0 / bpm.max(30) as f32).round() as usize;
            self.count_in_remaining = frames_per_beat * 4;
            self.count_in_elapsed = 0;
            self.looper_target_length =
                (frames_per_beat * 4 * bars as usize).min(self.looper_buffer.len());
        }
        let frames_per_beat = (self.sample_rate * 60.0 / bpm.max(30) as f32).round() as usize;
        let click_frames = (self.sample_rate * 0.030) as usize;
        let mut previous = self.previous_looper_mode;
        for sample in samples {
            let mode = self.looper_mode();
            if mode == LooperMode::CountIn {
                let beat_frame = self.count_in_elapsed % frames_per_beat.max(1);
                if beat_frame < click_frames {
                    let beat = self.count_in_elapsed / frames_per_beat.max(1);
                    let frequency = if beat == 0 { 1_760.0 } else { 1_180.0 };
                    let envelope = 1.0 - beat_frame as f32 / click_frames as f32;
                    *sample += (TAU * frequency * beat_frame as f32 / self.sample_rate).sin()
                        * envelope
                        * 0.18;
                }
                self.count_in_elapsed += 1;
                self.count_in_remaining = self.count_in_remaining.saturating_sub(1);
                if self.count_in_remaining == 0 {
                    self.parameters.set_looper_mode(LooperMode::Recording);
                }
                previous = LooperMode::CountIn;
                continue;
            }
            if mode == LooperMode::Recording && previous != LooperMode::Recording {
                self.looper_position = 0;
                self.looper_length = 0;
            }
            if mode == LooperMode::Playing || mode == LooperMode::Overdubbing {
                if self.looper_length > 0 {
                    self.looper_position %= self.looper_length;
                } else {
                    self.looper_position = 0;
                }
            }
            match mode {
                LooperMode::Recording => {
                    self.looper_buffer[self.looper_position] = *sample;
                    self.looper_position += 1;
                    self.looper_length = self.looper_length.max(self.looper_position);
                    if self.looper_position == self.looper_buffer.len() {
                        self.looper_position = 0;
                        self.looper_length = self.looper_buffer.len();
                    }
                    if self.looper_target_length > 0
                        && self.looper_length >= self.looper_target_length
                    {
                        self.looper_position = 0;
                        self.parameters.set_looper_mode(LooperMode::Playing);
                    }
                }
                LooperMode::Playing if self.looper_length > 0 => {
                    *sample += self.looper_buffer[self.looper_position];
                    self.looper_position = (self.looper_position + 1) % self.looper_length;
                }
                LooperMode::Overdubbing if self.looper_length > 0 => {
                    let loop_sample = self.looper_buffer[self.looper_position];
                    self.looper_buffer[self.looper_position] = loop_sample + *sample * 0.5;
                    *sample += loop_sample;
                    self.looper_position = (self.looper_position + 1) % self.looper_length;
                }
                _ => {}
            }
            previous = mode;
        }
        self.previous_looper_mode = previous;
    }
}

impl<P: MonoProcessor> MonoProcessor for EffectChainProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        self.process_gate(samples);
        self.process_compressor(samples);
        self.process_octaver(samples);
        self.source.process(samples);
        self.apply_gate_to_model_output(samples);
        self.process_eq(samples);
        self.process_chorus(samples);
        self.process_flanger(samples);
        self.process_tremolo(samples);
        self.process_delay(samples);
        self.process_reverb(samples);
        self.process_looper(samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    struct ConstantModelOutput;

    impl MonoProcessor for ConstantModelOutput {
        fn process(&mut self, samples: &mut [f32]) {
            samples.fill(0.5);
        }
    }

    #[test]
    fn disabled_effects_are_transparent() {
        let state = EngineState::default();
        let params = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(Passthrough, params, 48_000, 1);
        let mut samples = [0.25, -0.5, 0.75];
        chain.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5, 0.75]);
    }

    #[test]
    fn modulation_and_octave_effects_change_the_signal_when_enabled() {
        let state = EngineState {
            flanger_enabled: true,
            flanger_rate_hz: 1.0,
            flanger_depth: 0.8,
            flanger_feedback: 0.3,
            flanger_mix: 0.6,
            tremolo_enabled: true,
            tremolo_rate_hz: 3.0,
            tremolo_depth: 0.7,
            octaver_enabled: true,
            octaver_mix: 0.5,
            ..EngineState::default()
        };
        let params = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(Passthrough, params, 1_000, 1);
        let mut samples = (0..256)
            .map(|index| (TAU * 55.0 * index as f32 / 1_000.0).sin() * 0.7)
            .collect::<Vec<_>>();
        let original = samples.clone();

        chain.process(&mut samples);

        assert!(
            samples
                .iter()
                .zip(original)
                .any(|(processed, dry)| (processed - dry).abs() > 0.01)
        );
    }

    #[test]
    fn gate_reduces_a_signal_below_its_threshold() {
        let state = EngineState {
            gate_enabled: true,
            gate_threshold_db: -20.0,
            ..EngineState::default()
        };
        let params = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(Passthrough, params, 48_000, 1);
        let mut samples = [0.01; 512];
        chain.process(&mut samples);
        assert!(samples.last().expect("sample").abs() < 0.003);
    }

    #[test]
    fn gate_silences_nam_idle_noise_using_the_clean_input_signal() {
        let state = EngineState {
            gate_enabled: true,
            gate_threshold_db: -35.0,
            ..EngineState::default()
        };
        let params = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(ConstantModelOutput, params, 48_000, 1);
        let mut silent_guitar_input = vec![0.0; 4_096];

        chain.process(&mut silent_guitar_input);

        assert!(silent_guitar_input.last().expect("sample").abs() < 0.001);
    }

    #[test]
    fn quantized_looper_ticks_then_records_one_exact_bar_before_playback() {
        let parameters = Arc::new(EffectsParameters::from_state(&EngineState::default()));
        let mut chain = EffectChainProcessor::new(Passthrough, Arc::clone(&parameters), 1_000, 30);
        parameters.start_quantized_loop(60, 1);

        let mut count_in = vec![0.0; 4_000];
        chain.process(&mut count_in);
        assert!(count_in.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(parameters.looper_mode(), LooperMode::Recording);

        let mut recorded = vec![0.25; 4_000];
        chain.process(&mut recorded);
        assert_eq!(parameters.looper_mode(), LooperMode::Playing);

        let mut playback = [0.0];
        chain.process(&mut playback);
        assert!((playback[0] - 0.25).abs() < 0.001);
    }
}
