//! Lightweight, allocation-free guitar effects for the real-time signal path.
//!
//! The processors deliberately use small, predictable algorithms so the NAM
//! models remain the expensive part of the rig on a Raspberry Pi.

use crate::EngineState;
use crate::dsp::MonoProcessor;
use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

/// Increasing fractional delay moves toward the OLDER sample, including at
/// ring wrap and whole-sample boundaries. Shared by chorus and flanger.
fn delayed_sample(buffer: &[f32], write: usize, delay: f32) -> f32 {
    let whole = delay as usize;
    let read = (write + buffer.len() - whole % buffer.len()) % buffer.len();
    let older = (read + buffer.len() - 1) % buffer.len();
    buffer[read] + (buffer[older] - buffer[read]) * delay.fract()
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
    squelch_enabled: AtomicBool,
    squelch_threshold_db: AtomicFloat,
    /// Clean-input detector peaks since the last noise-floor LEARN start.
    pub input_floor_meter: crate::squelch::FloorMeter,
    pub gate_floor_meter: crate::squelch::FloorMeter,
    floor_learning: AtomicBool,
    dehiss_enabled: AtomicBool,
    dehiss_threshold_db: AtomicFloat,
    dehiss_frequency_hz: AtomicFloat,
    dehiss_range_db: AtomicFloat,
    /// LEARN in progress: keep measuring even while dehiss is off.
    dehiss_measuring: AtomicBool,
    /// Loudest high-band envelope since LEARN started.
    pub dehiss_meter: crate::dehiss::HissMeter,
    gate_enabled: AtomicBool,
    gate_threshold_db: AtomicFloat,
    gate_hysteresis_db: AtomicFloat,
    gate_hold_ms: AtomicFloat,
    gate_release_ms: AtomicFloat,
    gate_range_db: AtomicFloat,
    compressor_enabled: AtomicBool,
    compressor_threshold_db: AtomicFloat,
    compressor_ratio: AtomicFloat,
    compressor_attack_ms: AtomicFloat,
    compressor_release_ms: AtomicFloat,
    compressor_level_db: AtomicFloat,
    compressor_auto_makeup: AtomicBool,
    compressor_mix: AtomicFloat,
    /// Largest gain reduction (dB, f32 bits) since the meter last read it.
    compressor_reduction_db: AtomicU32,
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
    octaver_clean: AtomicBool,
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
            squelch_enabled: AtomicBool::new(state.squelch_enabled),
            squelch_threshold_db: AtomicFloat::new(state.squelch_threshold_db),
            input_floor_meter: crate::squelch::FloorMeter::default(),
            gate_floor_meter: crate::squelch::FloorMeter::default(),
            floor_learning: AtomicBool::new(false),
            dehiss_enabled: AtomicBool::new(state.dehiss_enabled),
            dehiss_threshold_db: AtomicFloat::new(state.dehiss_threshold_db),
            dehiss_frequency_hz: AtomicFloat::new(state.dehiss_frequency_hz),
            dehiss_range_db: AtomicFloat::new(state.dehiss_range_db),
            dehiss_measuring: AtomicBool::new(false),
            dehiss_meter: crate::dehiss::HissMeter::default(),
            gate_enabled: AtomicBool::new(state.gate_enabled),
            gate_threshold_db: AtomicFloat::new(state.gate_threshold_db),
            gate_hysteresis_db: AtomicFloat::new(state.gate_hysteresis_db),
            gate_hold_ms: AtomicFloat::new(state.gate_hold_ms),
            gate_release_ms: AtomicFloat::new(state.gate_release_ms),
            gate_range_db: AtomicFloat::new(state.gate_range_db),
            compressor_enabled: AtomicBool::new(state.compressor_enabled),
            compressor_threshold_db: AtomicFloat::new(state.compressor_threshold_db),
            compressor_ratio: AtomicFloat::new(state.compressor_ratio),
            compressor_attack_ms: AtomicFloat::new(state.compressor_attack_ms),
            compressor_release_ms: AtomicFloat::new(state.compressor_release_ms),
            compressor_level_db: AtomicFloat::new(state.compressor_level_db),
            compressor_auto_makeup: AtomicBool::new(state.compressor_auto_makeup),
            compressor_mix: AtomicFloat::new(state.compressor_mix),
            compressor_reduction_db: AtomicU32::new(0),
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
            octaver_clean: AtomicBool::new(state.octaver_clean),
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
        self.squelch_enabled
            .store(state.squelch_enabled, Ordering::Relaxed);
        self.squelch_threshold_db.store(state.squelch_threshold_db);
        self.dehiss_enabled
            .store(state.dehiss_enabled, Ordering::Relaxed);
        self.dehiss_threshold_db.store(state.dehiss_threshold_db);
        self.dehiss_frequency_hz.store(state.dehiss_frequency_hz);
        self.dehiss_range_db.store(state.dehiss_range_db);
        self.gate_enabled
            .store(state.gate_enabled, Ordering::Relaxed);
        self.gate_threshold_db.store(state.gate_threshold_db);
        self.gate_hysteresis_db.store(state.gate_hysteresis_db);
        self.gate_hold_ms.store(state.gate_hold_ms);
        self.gate_release_ms.store(state.gate_release_ms);
        self.gate_range_db.store(state.gate_range_db);
        self.compressor_enabled
            .store(state.compressor_enabled, Ordering::Relaxed);
        self.compressor_threshold_db
            .store(state.compressor_threshold_db);
        self.compressor_ratio.store(state.compressor_ratio);
        self.compressor_attack_ms.store(state.compressor_attack_ms);
        self.compressor_release_ms
            .store(state.compressor_release_ms);
        self.compressor_level_db.store(state.compressor_level_db);
        self.compressor_auto_makeup
            .store(state.compressor_auto_makeup, Ordering::Relaxed);
        self.compressor_mix.store(state.compressor_mix);
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
        self.octaver_clean
            .store(state.octaver_clean, Ordering::Relaxed);
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

    /// (enabled, threshold dB) for input-floor squelch users outside the
    /// effect chain, such as the Patch Lab route.
    pub fn squelch_settings(&self) -> (bool, f32) {
        (
            self.squelch_enabled.load(Ordering::Relaxed),
            self.squelch_threshold_db.load(),
        )
    }

    fn compressor_settings(&self) -> crate::compressor::CompressorSettings {
        crate::compressor::CompressorSettings {
            enabled: self.compressor_enabled.load(Ordering::Relaxed),
            threshold_db: self.compressor_threshold_db.load(),
            ratio: self.compressor_ratio.load(),
            attack_ms: self.compressor_attack_ms.load(),
            release_ms: self.compressor_release_ms.load(),
            level_db: self.compressor_level_db.load(),
            auto_makeup: self.compressor_auto_makeup.load(Ordering::Relaxed),
            mix: self.compressor_mix.load(),
        }
    }

    /// Peak compressor gain reduction (dB) since the previous call (meters).
    pub fn take_compressor_reduction_db(&self) -> f32 {
        f32::from_bits(self.compressor_reduction_db.swap(0, Ordering::Relaxed))
    }

    /// Noise-floor LEARN step 1 (control thread): the player stays silent.
    pub fn start_floor_learn(&self) {
        self.input_floor_meter.reset();
        self.gate_floor_meter.reset();
        self.floor_learning.store(true, Ordering::Relaxed);
    }

    /// Step 2: (squelch threshold, gate threshold) just above the measured
    /// floor, or `None` if nothing was measured. The gate's close point
    /// (threshold - hysteresis) lands 4 dB above the floor so it can close.
    pub fn finish_floor_learn(&self, gate_hysteresis_db: f32) -> Option<(f32, f32)> {
        let was_learning = self.floor_learning.swap(false, Ordering::Relaxed);
        let input = self.input_floor_meter.peak();
        let gate = self.gate_floor_meter.peak();
        (was_learning && input > 0.0 && gate > 0.0).then(|| {
            let gate_db = (20.0 * gate.log10() + gate_hysteresis_db.max(0.0) + 4.0)
                .clamp(crate::gate::MIN_THRESHOLD_DB, -10.0);
            (crate::squelch::learned_threshold_db(input), gate_db)
        })
    }

    /// LEARN step 1 (control thread): start measuring the idle hiss floor.
    pub fn start_dehiss_learn(&self) {
        self.dehiss_meter.reset();
        self.dehiss_measuring.store(true, Ordering::Relaxed);
    }

    /// LEARN step 2: stop measuring and return the threshold to use, or
    /// `None` if no audio was measured (engine not running).
    pub fn finish_dehiss_learn(&self) -> Option<f32> {
        let was_measuring = self.dehiss_measuring.swap(false, Ordering::Relaxed);
        let peak = self.dehiss_meter.peak();
        (was_measuring && peak > 0.0).then(|| crate::dehiss::HissMeter::learned_threshold_db(peak))
    }

    fn dehiss_settings(&self) -> crate::dehiss::DehissSettings {
        crate::dehiss::DehissSettings {
            enabled: self.dehiss_enabled.load(Ordering::Relaxed),
            threshold_db: self.dehiss_threshold_db.load(),
            frequency_hz: self.dehiss_frequency_hz.load(),
            range_db: self.dehiss_range_db.load(),
            measure: self.dehiss_measuring.load(Ordering::Relaxed),
        }
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
    right: Option<Box<EffectChainProcessor<crate::dsp::Passthrough>>>,
    looper_right: Vec<f32>,
    source: P,
    parameters: Arc<EffectsParameters>,
    sample_rate: f32,
    gate: crate::gate::Gate,
    gate_gains: Vec<f32>,
    squelch: crate::squelch::Squelch,
    dehiss: crate::dehiss::Dehiss,
    compressor: crate::compressor::Compressor,
    eq: crate::tone_eq::ToneEq,
    chorus_buffer: Vec<f32>,
    chorus_write: usize,
    chorus_phase: f32,
    flanger_buffer: Vec<f32>,
    flanger_write: usize,
    flanger_phase: f32,
    tremolo_phase: f32,
    octaver: crate::octaver::Octaver,
    delay_buffer: Vec<f32>,
    delay_write: usize,
    reverb: crate::reverb::Reverb,
    reverb_mix: f32,
    reverb_remaining: usize,
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
        Self::new_channel(source, parameters, sample_rate, max_loop_seconds, true)
    }
    fn new_channel(
        source: P,
        parameters: Arc<EffectsParameters>,
        sample_rate: u32,
        max_loop_seconds: usize,
        stereo: bool,
    ) -> Self {
        let sample_rate_f32 = sample_rate as f32;
        let gate =
            crate::gate::Gate::new(sample_rate, parameters.gate_enabled.load(Ordering::Relaxed));
        Self {
            right: if stereo {
                let mut right = EffectChainProcessor::new_channel(
                    crate::dsp::Passthrough,
                    Arc::clone(&parameters),
                    sample_rate,
                    0,
                    false,
                );
                right.chorus_phase = std::f32::consts::FRAC_PI_2;
                right.flanger_phase = std::f32::consts::FRAC_PI_2;
                right.reverb = crate::reverb::Reverb::with_spread(sample_rate, 1.071);
                Some(Box::new(right))
            } else {
                None
            },
            looper_right: vec![0.0; sample_rate as usize * max_loop_seconds],
            source,
            parameters,
            sample_rate: sample_rate_f32,
            gate,
            gate_gains: vec![1.0; 1024],
            squelch: crate::squelch::Squelch::new(sample_rate),
            dehiss: crate::dehiss::Dehiss::new(sample_rate),
            compressor: crate::compressor::Compressor::new(sample_rate),
            eq: crate::tone_eq::ToneEq::new(sample_rate, [0.0; 3]),
            chorus_buffer: vec![0.0; (sample_rate_f32 * 0.04) as usize],
            chorus_write: 0,
            chorus_phase: 0.0,
            flanger_buffer: vec![0.0; (sample_rate_f32 * 0.012) as usize],
            flanger_write: 0,
            flanger_phase: 0.0,
            tremolo_phase: 0.0,
            octaver: crate::octaver::Octaver::new(sample_rate),
            delay_buffer: vec![0.0; (sample_rate_f32 * 2.0) as usize],
            delay_write: 0,
            reverb: crate::reverb::Reverb::new(sample_rate),
            reverb_mix: 0.0,
            reverb_remaining: 0,
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

    fn process_gate(&mut self, samples: &[f32]) {
        let mut extra_hold = 0;
        if self.parameters.octaver_enabled.load(Ordering::Relaxed)
            && self.parameters.octaver_clean.load(Ordering::Relaxed)
        {
            extra_hold = crate::octaver::clean_latency_frames(self.sample_rate as u32);
        }
        self.gate.configure(
            self.parameters.gate_enabled.load(Ordering::Relaxed),
            self.parameters.gate_threshold_db.load(),
            self.parameters.gate_hysteresis_db.load(),
            self.parameters.gate_hold_ms.load(),
            self.parameters.gate_release_ms.load(),
            self.parameters.gate_range_db.load(),
            extra_hold,
        );
        let mut loudest = 0.0_f32;
        for (index, sample) in samples.iter().enumerate() {
            self.gate_gains[index] = self.gate.tick(*sample);
            loudest = loudest.max(self.gate.envelope());
        }
        self.parameters.gate_floor_meter.record(loudest);
    }

    /// Feeds the models exact zeros while the clean input sits at the
    /// interface's noise floor (see `squelch`). Runs after gate detection,
    /// which keeps listening to the raw input.
    pub(crate) fn process_squelch(&mut self, samples: &mut [f32]) {
        self.squelch.process(
            self.parameters.squelch_enabled.load(Ordering::Relaxed),
            self.parameters.squelch_threshold_db.load(),
            &self.parameters.input_floor_meter,
            samples,
        );
    }

    /// The gate listens to clean input and applies its time-aligned envelope
    /// once, after the NAM chain. That means an amp capture's idle hiss is silenced
    /// when the player is not touching the strings, instead of being allowed
    /// through merely because it was generated by the model.
    fn apply_gate_to_model_output(&self, samples: &mut [f32]) {
        for (sample, gain) in samples.iter_mut().zip(&self.gate_gains) {
            *sample *= gain;
        }
    }

    fn process_compressor(&mut self, samples: &mut [f32]) {
        let settings = self.parameters.compressor_settings();
        let reduction = self.compressor.process(settings, samples);
        self.parameters
            .compressor_reduction_db
            .fetch_max(reduction.max(0.0).to_bits(), Ordering::Relaxed);
    }

    fn process_eq(&mut self, samples: &mut [f32]) {
        let gains = if self.parameters.eq_enabled.load(Ordering::Relaxed) {
            [
                self.parameters.eq_low_db.load(),
                self.parameters.eq_mid_db.load(),
                self.parameters.eq_high_db.load(),
            ]
        } else {
            [0.0; 3]
        };
        self.eq.set_gains(gains);
        for sample in samples {
            *sample = self.eq.tick(*sample);
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
            let delayed = delayed_sample(&self.chorus_buffer, self.chorus_write, delay);
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
            let delayed = delayed_sample(&self.flanger_buffer, self.flanger_write, delay);
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

    /// Selectable analog/polyphonic octave voice before the NAM chain.
    fn process_octaver(&mut self, samples: &mut [f32]) {
        self.octaver.process(
            samples,
            self.parameters.octaver_enabled.load(Ordering::Relaxed),
            self.parameters.octaver_clean.load(Ordering::Relaxed),
            self.parameters.octaver_mix.load().clamp(0.0, 1.0),
        );
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
        let enabled = self.parameters.reverb_enabled.load(Ordering::Relaxed);
        if enabled {
            self.reverb_remaining = (self.sample_rate
                * (self.parameters.reverb_decay_seconds.load().clamp(0.3, 12.0) * 2.0 + 0.2))
                as usize;
        } else if self.reverb_remaining == 0 {
            return;
        }
        self.reverb
            .set_decay(self.parameters.reverb_decay_seconds.load());
        let target = if enabled {
            self.parameters.reverb_mix.load().clamp(0.0, 1.0)
        } else {
            0.0
        };
        let smoothing = 1.0 - (-1.0 / (0.010 * self.sample_rate)).exp();
        for sample in samples.iter_mut() {
            let dry = *sample;
            let wet = self.reverb.tick(if enabled { dry } else { 0.0 });
            self.reverb_mix += (target - self.reverb_mix) * smoothing;
            *sample = dry * (1.0 - self.reverb_mix) + wet * self.reverb_mix;
        }
        if !enabled {
            self.reverb_remaining = self.reverb_remaining.saturating_sub(samples.len());
            if self.reverb_remaining == 0 {
                self.reverb.clear();
                self.reverb_mix = 0.0;
            }
        }
    }

    fn process_looper(&mut self, samples: &mut [f32]) {
        self.process_looper_channels(samples, None);
    }
    fn process_looper_channels(&mut self, samples: &mut [f32], mut right: Option<&mut [f32]>) {
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
        for (i, sample) in samples.iter_mut().enumerate() {
            let mode = self.looper_mode();
            if mode == LooperMode::CountIn {
                let beat_frame = self.count_in_elapsed % frames_per_beat.max(1);
                if beat_frame < click_frames {
                    let beat = self.count_in_elapsed / frames_per_beat.max(1);
                    let frequency = if beat == 0 { 1_760.0 } else { 1_180.0 };
                    let envelope = 1.0 - beat_frame as f32 / click_frames as f32;
                    let click = (TAU * frequency * beat_frame as f32 / self.sample_rate).sin()
                        * envelope
                        * 0.18;
                    *sample += click;
                    if let Some(r) = right.as_deref_mut() {
                        r[i] += click;
                    }
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
                    self.looper_right[self.looper_position] =
                        right.as_deref().map_or(*sample, |r| r[i]);
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
                    if let Some(r) = right.as_deref_mut() {
                        r[i] += self.looper_right[self.looper_position];
                    }
                    self.looper_position = (self.looper_position + 1) % self.looper_length;
                }
                LooperMode::Overdubbing if self.looper_length > 0 => {
                    let loop_sample = self.looper_buffer[self.looper_position];
                    self.looper_buffer[self.looper_position] = loop_sample + *sample * 0.5;
                    *sample += loop_sample;
                    if let Some(r) = right.as_deref_mut() {
                        let previous = self.looper_right[self.looper_position];
                        self.looper_right[self.looper_position] += r[i] * 0.5;
                        r[i] += previous;
                    }
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
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        let mut left = [0.0; 1024];
        let mut right = [0.0; 1024];
        for (input, out) in samples.chunks_mut(1024).zip(output.chunks_mut(1024)) {
            self.process_gate(input);
            self.process_squelch(input);
            self.process_compressor(input);
            self.process_octaver(input);
            self.source.process_stereo(input, out);
            for (i, f) in out.iter().enumerate() {
                left[i] = f[0];
                right[i] = f[1];
            }
            let left = &mut left[..input.len()];
            let right = &mut right[..input.len()];
            let settings = self.parameters.dehiss_settings();
            self.dehiss
                .process(settings, &self.parameters.dehiss_meter, left, Some(right));
            self.apply_gate_to_model_output(left);
            self.apply_gate_to_model_output(right);
            self.process_eq(left);
            self.process_chorus(left);
            self.process_flanger(left);
            self.process_tremolo(left);
            self.process_delay(left);
            self.process_reverb(left);
            if let Some(r) = &mut self.right {
                r.process_eq(right);
                r.process_chorus(right);
                r.process_flanger(right);
                r.process_tremolo(right);
                r.process_delay(right);
                r.process_reverb(right);
            }
            self.process_looper_channels(left, Some(right));
            for (i, f) in out.iter_mut().enumerate() {
                *f = [left[i], right[i]];
            }
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        // Bounded scratch space even for offline callers passing whole files.
        // No allocation occurs in process().
        for samples in samples.chunks_mut(self.gate_gains.len()) {
            self.process_gate(samples);
            self.process_squelch(samples);
            self.process_compressor(samples);
            self.process_octaver(samples);
            self.source.process(samples);
            let settings = self.parameters.dehiss_settings();
            self.dehiss
                .process(settings, &self.parameters.dehiss_meter, samples, None);
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
    fn fractional_delay_is_continuous_at_integer_and_ring_boundaries() {
        let buffer = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        assert_eq!(delayed_sample(&buffer, 6, 2.25), 3.75);
        assert_eq!(delayed_sample(&buffer, 1, 1.5), 3.5);
        for write in 0..buffer.len() {
            for whole in 1..buffer.len() - 1 {
                let below = delayed_sample(&buffer, write, whole as f32 - 0.00001);
                let above = delayed_sample(&buffer, write, whole as f32 + 0.00001);
                assert!((below - above).abs() < 0.001);
            }
        }
    }

    #[test]
    fn gate_is_block_size_independent_and_does_not_pre_distort_model_input() {
        let state = EngineState {
            gate_enabled: true,
            gate_threshold_db: -40.0,
            // The probe "model input" is a constant 0.001, which the input
            // squelch rightly treats as no signal; this test is about the gate.
            squelch_enabled: false,
            ..Default::default()
        };
        let source: Vec<_> = (0..48_000)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                0.04 * (-2.0 * t).exp() * (TAU * 82.41 * t).sin()
            })
            .collect();
        let render = |size| {
            let mut p = EffectChainProcessor::new(
                Passthrough,
                Arc::new(EffectsParameters::from_state(&state)),
                48_000,
                1,
            );
            let mut out = source.clone();
            for block in out.chunks_mut(size) {
                p.process(block);
            }
            out
        };
        assert_eq!(render(64), render(128));
        assert_eq!(render(64), render(517));

        struct InspectInput;
        impl MonoProcessor for InspectInput {
            fn process(&mut self, samples: &mut [f32]) {
                assert!(samples.iter().all(|s| *s == 0.001));
            }
        }
        let mut p = EffectChainProcessor::new(
            InspectInput,
            Arc::new(EffectsParameters::from_state(&state)),
            48_000,
            1,
        );
        p.process(&mut [0.001; 128]);
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

    /// Stand-in amp: emits deterministic white hiss regardless of input.
    struct HissingAmp(u32);
    impl MonoProcessor for HissingAmp {
        fn process(&mut self, samples: &mut [f32]) {
            for sample in samples {
                self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *sample = ((self.0 >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.004;
            }
        }
    }

    fn fizz_rms(x: &[f32]) -> f32 {
        let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
        (d.iter().map(|v| v * v).sum::<f32>() / d.len() as f32).sqrt()
    }

    #[test]
    fn dehiss_learn_measures_while_off_then_cuts_the_amps_hiss() {
        let mut state = EngineState::default();
        let params = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(HissingAmp(1), Arc::clone(&params), 48_000, 1);
        // Finishing without a measurement fails cleanly.
        assert_eq!(params.finish_dehiss_learn(), None);

        params.start_dehiss_learn();
        let mut idle = vec![0.0; 48_000];
        chain.process(&mut idle);
        let before = fizz_rms(&idle[24_000..]);
        let threshold = params.finish_dehiss_learn().expect("measured hiss");
        // ~0.002 peak hiss -> about -54 dBFS, plus the 10 dB learn margin.
        assert!((-56.0..=-38.0).contains(&threshold), "{threshold}");
        assert_eq!(params.finish_dehiss_learn(), None, "learn ended");

        state.dehiss_enabled = true;
        state.dehiss_threshold_db = threshold;
        params.update(&state);
        let mut idle = vec![0.0; 48_000];
        chain.process(&mut idle);
        let after = fizz_rms(&idle[24_000..]);
        let reduction = 20.0 * (after / before).log10();
        assert!(reduction < -12.0, "hiss only reduced {reduction} dB");
    }

    #[test]
    fn gate_live_updates_preserve_stereo_ratio() {
        struct StereoNoise;
        impl MonoProcessor for StereoNoise {
            fn process(&mut self, samples: &mut [f32]) {
                samples.fill(0.2);
            }
            fn process_stereo(&mut self, _: &mut [f32], output: &mut [[f32; 2]]) {
                output.fill([0.2, -0.1]);
            }
        }
        let mut state = EngineState {
            gate_enabled: true,
            ..Default::default()
        };
        let parameters = Arc::new(EffectsParameters::from_state(&state));
        let mut chain = EffectChainProcessor::new(StereoNoise, parameters.clone(), 48_000, 0);
        let mut input = vec![0.0; 48_000];
        let mut output = vec![[0.0; 2]; input.len()];
        chain.process_stereo(&mut input, &mut output);
        assert!(output.last().unwrap()[0].abs() < 0.001);
        state.gate_range_db = 20.0;
        parameters.update(&state);
        chain.process_stereo(&mut input, &mut output);
        for frame in &output {
            assert!((frame[0] * 0.5 + frame[1]).abs() < 1e-7);
        }
        assert!((output.last().unwrap()[0] - 0.02).abs() < 1e-5);
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

/// Prepared Patch Lab modulation node. Each channel has its own delay history;
/// chorus/flanger use the same quadrature stereo phases as the pedal chain.
pub struct ModulationNode {
    kind: ModulationKind,
    buffers: [Vec<f32>; 2],
    write: usize,
    phase: [f32; 2],
    sample_rate: f32,
    rate: f32,
    depth: f32,
    feedback: f32,
    mix: f32,
}
#[derive(Clone, Copy)]
pub enum ModulationKind {
    Chorus,
    Flanger,
    Tremolo,
}
impl ModulationNode {
    pub fn new(
        rate: u32,
        kind: ModulationKind,
        hz: f32,
        depth: f32,
        feedback: f32,
        mix: f32,
    ) -> Result<Self, &'static str> {
        let rate_range = match kind {
            ModulationKind::Chorus => 0.05..=8.0,
            ModulationKind::Flanger => 0.05..=5.0,
            ModulationKind::Tremolo => 0.1..=18.0,
        };
        if !(8_000..=192_000).contains(&rate)
            || !hz.is_finite()
            || !rate_range.contains(&hz)
            || !depth.is_finite()
            || !(0.0..=1.0).contains(&depth)
            || !feedback.is_finite()
            || !(-0.85..=0.85).contains(&feedback)
            || !mix.is_finite()
            || !(0.0..=1.0).contains(&mix)
        {
            return Err("Invalid modulation configuration");
        }
        let len = match kind {
            ModulationKind::Chorus => (rate as f32 * 0.04) as usize,
            ModulationKind::Flanger => (rate as f32 * 0.012) as usize,
            ModulationKind::Tremolo => 0,
        };
        let phase = if matches!(kind, ModulationKind::Tremolo) {
            [0.0, 0.0]
        } else {
            [0.0, std::f32::consts::FRAC_PI_2]
        };
        Ok(Self {
            kind,
            buffers: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            phase,
            sample_rate: rate as f32,
            rate: hz,
            depth,
            feedback,
            mix,
        })
    }
}
impl crate::node_rig::StereoNode for ModulationNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        match parameter {
            "rate_hz"
                if (match self.kind {
                    ModulationKind::Chorus => 0.05..=8.0,
                    ModulationKind::Flanger => 0.05..=5.0,
                    ModulationKind::Tremolo => 0.1..=18.0,
                })
                .contains(&value) =>
            {
                self.rate = value
            }
            "depth" if (0.0..=1.0).contains(&value) => self.depth = value,
            "feedback"
                if matches!(self.kind, ModulationKind::Flanger)
                    && (-0.85..=0.85).contains(&value) =>
            {
                self.feedback = value
            }
            "mix"
                if !matches!(self.kind, ModulationKind::Tremolo)
                    && (0.0..=1.0).contains(&value) =>
            {
                self.mix = value
            }
            _ => return false,
        }
        true
    }
    fn latency_samples(&self) -> usize {
        0
    } // Effect delay is intentional, not compensation latency.
    fn tick(&mut self, input: [f32; 2]) -> [f32; 2] {
        let mut output = input;
        for channel in 0..2 {
            let lfo = (self.phase[channel].sin() + 1.0) * 0.5;
            match self.kind {
                ModulationKind::Tremolo => output[channel] *= 1.0 - self.depth + self.depth * lfo,
                kind => {
                    let (base, span) = match kind {
                        ModulationKind::Chorus => (0.014, 0.009),
                        _ => (0.0015, 0.0045),
                    };
                    let delay = self.sample_rate * (base + span * self.depth * lfo);
                    let delayed = delayed_sample(&self.buffers[channel], self.write, delay);
                    self.buffers[channel][self.write] = input[channel] + delayed * self.feedback;
                    output[channel] = input[channel] * (1.0 - self.mix) + delayed * self.mix;
                }
            }
            self.phase[channel] = (self.phase[channel] + TAU * self.rate / self.sample_rate) % TAU;
        }
        if !self.buffers[0].is_empty() {
            self.write = (self.write + 1) % self.buffers[0].len();
        }
        output
    }
    fn reset(&mut self) {
        for buffer in &mut self.buffers {
            buffer.fill(0.0);
        }
        self.write = 0;
        self.phase = if matches!(self.kind, ModulationKind::Tremolo) {
            [0.0, 0.0]
        } else {
            [0.0, std::f32::consts::FRAC_PI_2]
        };
    }
}

/// Stereo octave-down pedal voices. The clean voice intentionally delays only
/// its wet component; the pedal's dry path stays immediate, so latency is zero.
pub struct OctaverNode {
    channels: [crate::octaver::Octaver; 2],
    clean_mode: bool,
    mix: f32,
}
impl OctaverNode {
    pub fn new(rate: u32, clean_mode: bool, mix: f32) -> Result<Self, &'static str> {
        if !(8_000..=192_000).contains(&rate) || !mix.is_finite() || !(0.0..=1.0).contains(&mix) {
            return Err("Invalid octaver configuration");
        }
        Ok(Self {
            channels: [
                crate::octaver::Octaver::new(rate),
                crate::octaver::Octaver::new(rate),
            ],
            clean_mode,
            mix,
        })
    }
}
impl crate::node_rig::StereoNode for OctaverNode {
    fn set_parameter(&mut self, parameter: &str, value: f32) -> bool {
        if parameter == "mix" && value.is_finite() && (0.0..=1.0).contains(&value) {
            self.mix = value;
            true
        } else {
            false
        }
    }
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, mut input: [f32; 2]) -> [f32; 2] {
        for (channel, sample) in self.channels.iter_mut().zip(input.iter_mut()) {
            channel.process(
                std::slice::from_mut(sample),
                true,
                self.clean_mode,
                self.mix,
            );
        }
        input
    }
    fn reset(&mut self) {
        for channel in &mut self.channels {
            channel.reset();
        }
    }
    fn process_block(&mut self, frames: &mut [[f32; 2]]) {
        for chunk in frames.chunks_mut(128) {
            let mut left = [0.0; 128];
            let mut right = [0.0; 128];
            for (i, frame) in chunk.iter().enumerate() {
                left[i] = frame[0];
                right[i] = frame[1];
            }
            self.channels[0].process(&mut left[..chunk.len()], true, self.clean_mode, self.mix);
            self.channels[1].process(&mut right[..chunk.len()], true, self.clean_mode, self.mix);
            for (i, frame) in chunk.iter_mut().enumerate() {
                *frame = [left[i], right[i]];
            }
        }
    }
}

#[cfg(test)]
mod patch_node_tests {
    use super::*;
    use crate::node_rig::StereoNode;

    #[test]
    fn modulation_live_update_keeps_delay_and_phase() {
        let mut node =
            ModulationNode::new(8000, ModulationKind::Flanger, 1.0, 0.0, 0.5, 1.0).unwrap();
        node.tick([1.0, 0.0]);
        for _ in 0..4 {
            node.tick([0.0; 2]);
        }
        let phase = node.phase;
        let write = node.write;
        assert!(node.set_parameter("rate_hz", 2.0));
        assert_eq!(node.phase, phase);
        assert_eq!(node.write, write);
        assert!((0..20).any(|_| node.tick([0.0; 2])[0].abs() > 0.01));
        assert!(!node.set_parameter("feedback", 0.9));
    }
    #[test]
    fn modulation_is_audible_stereo_independent_and_resets() {
        for kind in [
            ModulationKind::Chorus,
            ModulationKind::Flanger,
            ModulationKind::Tremolo,
        ] {
            let mut node = ModulationNode::new(48_000, kind, 1.0, 0.8, 0.2, 0.7).unwrap();
            let original: Vec<_> = (0..4000).map(|i| [((i as f32) * 0.1).sin(), 0.0]).collect();
            let mut first = original.clone();
            node.process_block(&mut first);
            assert!(
                first
                    .iter()
                    .zip(&original)
                    .any(|(a, b)| (a[0] - b[0]).abs() > 0.01)
            );
            assert!(first.iter().all(|x| x[1] == 0.0));
            assert_eq!(node.latency_samples(), 0);
            node.reset();
            let mut second = original;
            for block in second.chunks_mut(17) {
                node.process_block(block);
            }
            assert_eq!(first, second);
        }
        assert!(
            ModulationNode::new(48_000, ModulationKind::Chorus, f32::NAN, 0.5, 0.0, 0.5).is_err()
        );
        assert!(ModulationNode::new(48_000, ModulationKind::Flanger, 1.0, 0.5, 0.86, 0.5).is_err());
        assert!(ModulationNode::new(48_000, ModulationKind::Tremolo, 19.0, 0.5, 0.0, 0.0).is_err());
    }

    #[test]
    fn octaver_is_audible_stereo_independent_and_resets() {
        for clean in [false, true] {
            let mut node = OctaverNode::new(48_000, clean, 1.0).unwrap();
            let original: Vec<_> = (0..24_000)
                .map(|i| [((i as f32) * 0.04).sin() * 0.2, 0.0])
                .collect();
            let mut first = original.clone();
            node.process_block(&mut first);
            assert!(
                first[12_000..]
                    .iter()
                    .zip(&original[12_000..])
                    .any(|(a, b)| (a[0] - b[0]).abs() > 0.01)
            );
            assert!(first.iter().all(|x| x[1] == 0.0));
            assert_eq!(node.latency_samples(), 0);
            node.reset();
            let mut second = original;
            node.process_block(&mut second);
            assert_eq!(first, second);
        }
        assert!(OctaverNode::new(48_000, false, f32::INFINITY).is_err());
        assert!(OctaverNode::new(0, false, 0.5).is_err());
    }
}
