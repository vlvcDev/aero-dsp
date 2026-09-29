use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};

pub const I32_TO_FLOAT: f32 = 1.0 / 2_147_483_648.0;
pub const FLOAT_TO_I32: f32 = 2_147_483_647.0;

pub trait MonoProcessor {
    fn process(&mut self, samples: &mut [f32]);
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetrySnapshot {
    pub input_peak: f32,
    pub output_peak: f32,
    pub clipped: bool,
    pub cpu_percent: f32,
    pub xruns: u64,
}

pub struct RealtimeTelemetry {
    input_peak_bits: AtomicU32,
    output_peak_bits: AtomicU32,
    clipped: AtomicBool,
    cpu_percent_bits: AtomicU32,
    xruns: AtomicU64,
}

impl Default for RealtimeTelemetry {
    fn default() -> Self {
        Self {
            input_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            output_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            clipped: AtomicBool::new(false),
            cpu_percent_bits: AtomicU32::new(0.0_f32.to_bits()),
            xruns: AtomicU64::new(0),
        }
    }
}

impl RealtimeTelemetry {
    fn record_peak(target: &AtomicU32, peak: f32) {
        let bits = peak.to_bits();
        let mut current = target.load(Ordering::Relaxed);
        while bits > current {
            match target.compare_exchange_weak(current, bits, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    pub fn record_levels(&self, input_peak: f32, output_peak: f32, clipped: bool) {
        Self::record_peak(&self.input_peak_bits, input_peak);
        Self::record_peak(&self.output_peak_bits, output_peak);
        if clipped {
            self.clipped.store(true, Ordering::Relaxed);
        }
    }

    pub fn record_cpu_percent(&self, cpu_percent: f32) {
        self.cpu_percent_bits
            .store(cpu_percent.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn record_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot {
            input_peak: f32::from_bits(
                self.input_peak_bits
                    .swap(0.0_f32.to_bits(), Ordering::Relaxed),
            ),
            output_peak: f32::from_bits(
                self.output_peak_bits
                    .swap(0.0_f32.to_bits(), Ordering::Relaxed),
            ),
            clipped: self.clipped.swap(false, Ordering::Relaxed),
            cpu_percent: f32::from_bits(self.cpu_percent_bits.load(Ordering::Relaxed)),
            xruns: self.xruns.load(Ordering::Relaxed),
        }
    }
}

pub struct SafetyProcessor<P> {
    processor: P,
    telemetry: Arc<RealtimeTelemetry>,
    high_pass_coefficient: f32,
    previous_input: f32,
    previous_output: f32,
    limiter_ceiling: f32,
}

impl<P> SafetyProcessor<P> {
    pub fn new(processor: P, telemetry: Arc<RealtimeTelemetry>, sample_rate: u32) -> Self {
        Self {
            processor,
            telemetry,
            high_pass_coefficient: (-2.0 * std::f32::consts::PI * 30.0 / sample_rate as f32).exp(),
            previous_input: 0.0,
            previous_output: 0.0,
            limiter_ceiling: 10.0_f32.powf(-1.0 / 20.0),
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for SafetyProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        let input_peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        for sample in samples.iter_mut() {
            let input = *sample;
            let output =
                self.high_pass_coefficient * (self.previous_output + input - self.previous_input);
            self.previous_input = input;
            self.previous_output = output;
            *sample = output;
        }

        self.processor.process(samples);

        let mut clipped = false;
        let mut output_peak = 0.0_f32;
        for sample in samples {
            if sample.abs() > self.limiter_ceiling {
                clipped = true;
                *sample = sample.clamp(-self.limiter_ceiling, self.limiter_ceiling);
            }
            output_peak = output_peak.max(sample.abs());
        }
        self.telemetry
            .record_levels(input_peak, output_peak, clipped);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterleavedLayout {
    pub frames: usize,
    pub capture_channels: usize,
    pub playback_channels: usize,
    pub guitar_input_channel: usize,
}

#[derive(Debug, Default)]
pub struct Passthrough;

impl MonoProcessor for Passthrough {
    fn process(&mut self, _samples: &mut [f32]) {}
}

impl MonoProcessor for nam_bridge::NamModel {
    fn process(&mut self, samples: &mut [f32]) {
        if nam_bridge::NamModel::process(self, samples).is_err() {
            samples.fill(0.0);
        }
    }
}

pub struct BypassProcessor<P> {
    processor: P,
    bypassed: Arc<AtomicBool>,
}

impl<P> BypassProcessor<P> {
    pub fn new(processor: P, bypassed: Arc<AtomicBool>) -> Self {
        Self {
            processor,
            bypassed,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for BypassProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        if !self.bypassed.load(Ordering::Relaxed) {
            self.processor.process(samples);
        }
    }
}

fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

struct AtomicParameter(AtomicU32);

impl AtomicParameter {
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

pub struct PedalParameters {
    drive_gain: AtomicParameter,
    mix: AtomicParameter,
    level_gain: AtomicParameter,
}

impl PedalParameters {
    pub fn new(drive_db: f32, mix: f32, level_db: f32) -> Self {
        Self {
            drive_gain: AtomicParameter::new(db_to_gain(drive_db)),
            mix: AtomicParameter::new(mix),
            level_gain: AtomicParameter::new(db_to_gain(level_db)),
        }
    }

    pub fn update(&self, drive_db: f32, mix: f32, level_db: f32) {
        self.drive_gain.store(db_to_gain(drive_db));
        self.mix.store(mix);
        self.level_gain.store(db_to_gain(level_db));
    }
}

pub struct PedalControlProcessor<P> {
    processor: P,
    parameters: Arc<PedalParameters>,
    dry: Vec<f32>,
    drive_gain: f32,
    mix: f32,
    level_gain: f32,
}

impl<P> PedalControlProcessor<P> {
    pub fn new(processor: P, parameters: Arc<PedalParameters>, max_frames: usize) -> Self {
        let drive_gain = parameters.drive_gain.load();
        let mix = parameters.mix.load();
        let level_gain = parameters.level_gain.load();
        Self {
            processor,
            parameters,
            dry: vec![0.0; max_frames],
            drive_gain,
            mix,
            level_gain,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for PedalControlProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        if samples.len() > self.dry.len() {
            samples.fill(0.0);
            return;
        }
        let dry = &mut self.dry[..samples.len()];
        dry.copy_from_slice(samples);
        let frames = samples.len().max(1) as f32;
        let drive_target = self.parameters.drive_gain.load();
        let drive_step = (drive_target - self.drive_gain) / frames;
        for sample in samples.iter_mut() {
            self.drive_gain += drive_step;
            *sample *= self.drive_gain;
        }
        self.processor.process(samples);

        let mix_target = self.parameters.mix.load();
        let level_target = self.parameters.level_gain.load();
        let mix_step = (mix_target - self.mix) / frames;
        let level_step = (level_target - self.level_gain) / frames;
        for (wet, dry) in samples.iter_mut().zip(dry.iter()) {
            self.mix += mix_step;
            self.level_gain += level_step;
            *wet = (*dry * (1.0 - self.mix) + *wet * self.mix) * self.level_gain;
        }
    }
}

pub struct AmpParameters {
    bass_gain: AtomicParameter,
    mid_gain: AtomicParameter,
    treble_gain: AtomicParameter,
    volume_gain: AtomicParameter,
}

impl AmpParameters {
    pub fn new(bass_db: f32, mid_db: f32, treble_db: f32, volume_db: f32) -> Self {
        Self {
            bass_gain: AtomicParameter::new(db_to_gain(bass_db)),
            mid_gain: AtomicParameter::new(db_to_gain(mid_db)),
            treble_gain: AtomicParameter::new(db_to_gain(treble_db)),
            volume_gain: AtomicParameter::new(db_to_gain(volume_db)),
        }
    }

    pub fn update(&self, bass_db: f32, mid_db: f32, treble_db: f32, volume_db: f32) {
        self.bass_gain.store(db_to_gain(bass_db));
        self.mid_gain.store(db_to_gain(mid_db));
        self.treble_gain.store(db_to_gain(treble_db));
        self.volume_gain.store(db_to_gain(volume_db));
    }
}

pub struct AmpControlProcessor<P> {
    processor: P,
    parameters: Arc<AmpParameters>,
    low_state: f32,
    high_split_state: f32,
    low_alpha: f32,
    high_alpha: f32,
    bass_gain: f32,
    mid_gain: f32,
    treble_gain: f32,
    volume_gain: f32,
}

impl<P> AmpControlProcessor<P> {
    pub fn new(processor: P, parameters: Arc<AmpParameters>, sample_rate: u32) -> Self {
        let coefficient = |frequency: f32| {
            1.0 - (-2.0 * std::f32::consts::PI * frequency / sample_rate as f32).exp()
        };
        let bass_gain = parameters.bass_gain.load();
        let mid_gain = parameters.mid_gain.load();
        let treble_gain = parameters.treble_gain.load();
        let volume_gain = parameters.volume_gain.load();
        Self {
            processor,
            parameters,
            low_state: 0.0,
            high_split_state: 0.0,
            low_alpha: coefficient(250.0),
            high_alpha: coefficient(2_500.0),
            bass_gain,
            mid_gain,
            treble_gain,
            volume_gain,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for AmpControlProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        self.processor.process(samples);
        let frames = samples.len().max(1) as f32;
        let bass_step = (self.parameters.bass_gain.load() - self.bass_gain) / frames;
        let mid_step = (self.parameters.mid_gain.load() - self.mid_gain) / frames;
        let treble_step = (self.parameters.treble_gain.load() - self.treble_gain) / frames;
        let volume_step = (self.parameters.volume_gain.load() - self.volume_gain) / frames;
        for sample in samples {
            self.bass_gain += bass_step;
            self.mid_gain += mid_step;
            self.treble_gain += treble_step;
            self.volume_gain += volume_step;
            let input = *sample;
            self.low_state += self.low_alpha * (input - self.low_state);
            self.high_split_state += self.high_alpha * (input - self.high_split_state);
            let low = self.low_state;
            let high = input - self.high_split_state;
            let middle = input - low - high;
            *sample = (low * self.bass_gain + middle * self.mid_gain + high * self.treble_gain)
                * self.volume_gain;
        }
    }
}

pub enum AmpProcessor {
    Clean,
    Nam(nam_bridge::NamModel),
}

impl MonoProcessor for AmpProcessor {
    fn process(&mut self, samples: &mut [f32]) {
        match self {
            Self::Clean => {}
            Self::Nam(model) => MonoProcessor::process(model, samples),
        }
    }
}

pub struct SerialProcessor<A, B> {
    first: A,
    second: B,
}

impl<A, B> SerialProcessor<A, B> {
    pub fn new(first: A, second: B) -> Self {
        Self { first, second }
    }
}

impl<A: MonoProcessor, B: MonoProcessor> MonoProcessor for SerialProcessor<A, B> {
    fn process(&mut self, samples: &mut [f32]) {
        self.first.process(samples);
        self.second.process(samples);
    }
}

struct SwapSlot<P> {
    pending: Option<P>,
    retired: Option<P>,
}

pub struct SwapMailbox<P> {
    slot: Mutex<SwapSlot<P>>,
}

impl<P> Default for SwapMailbox<P> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(SwapSlot {
                pending: None,
                retired: None,
            }),
        }
    }
}

impl<P> SwapMailbox<P> {
    pub fn stage(&self, prepared: P) {
        let mut slot = self.slot.lock().expect("processor swap mailbox poisoned");
        // Both values are dropped here, on the control/loader thread.
        slot.retired.take();
        slot.pending = Some(prepared);
    }

    /// Non-blocking handoff for a processor that owns its own active state.
    /// The replaced value is retained in the mailbox and dropped next time a
    /// control-thread stage occurs, never from the audio callback.
    pub fn swap_pending(&self, active: &mut P) -> bool {
        match self.slot.try_lock() {
            Ok(mut slot) => {
                let Some(prepared) = slot.pending.take() else {
                    return false;
                };
                slot.retired = Some(std::mem::replace(active, prepared));
                true
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => false,
        }
    }
}

pub struct SwappableProcessor<P> {
    active: Option<P>,
    mailbox: Arc<SwapMailbox<P>>,
}

impl<P> SwappableProcessor<P> {
    pub fn new(active: P, mailbox: Arc<SwapMailbox<P>>) -> Self {
        Self {
            active: Some(active),
            mailbox,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for SwappableProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        match self.mailbox.slot.try_lock() {
            Ok(mut slot) => {
                if let Some(prepared) = slot.pending.take() {
                    // Retain the old processor in the mailbox. A later control-side stage()
                    // drops it away from the real-time thread.
                    slot.retired = self.active.replace(prepared);
                }
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(_)) => {
                samples.fill(0.0);
                return;
            }
        }
        if let Some(active) = &mut self.active {
            active.process(samples);
        }
    }
}

pub fn process_interleaved_i32<P: MonoProcessor + ?Sized>(
    input: &[i32],
    output: &mut [i32],
    mono_workspace: &mut [f32],
    layout: InterleavedLayout,
    processor: &mut P,
) -> Result<(), &'static str> {
    if layout.capture_channels == 0 || layout.playback_channels == 0 {
        return Err("channel counts must be non-zero");
    }
    if layout.guitar_input_channel >= layout.capture_channels {
        return Err("guitar input channel is outside the capture stream");
    }
    if input.len() < layout.frames * layout.capture_channels
        || output.len() < layout.frames * layout.playback_channels
        || mono_workspace.len() < layout.frames
    {
        return Err("audio buffer is smaller than the requested frame count");
    }

    let mono = &mut mono_workspace[..layout.frames];
    for (frame, sample) in mono.iter_mut().enumerate() {
        *sample = input[frame * layout.capture_channels + layout.guitar_input_channel] as f32
            * I32_TO_FLOAT;
    }

    processor.process(mono);

    for (frame, sample) in mono.iter().enumerate() {
        let converted = (sample.clamp(-1.0, 1.0) * FLOAT_TO_I32) as i32;
        let frame_start = frame * layout.playback_channels;
        output[frame_start..frame_start + layout.playback_channels].fill(converted);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Invert;

    impl MonoProcessor for Invert {
        fn process(&mut self, samples: &mut [f32]) {
            for sample in samples {
                *sample = -*sample;
            }
        }
    }

    #[test]
    fn selects_one_input_processes_it_and_routes_to_stereo() {
        let input = [1000, 2000, 3000, 4000];
        let mut output = [0; 4];
        let mut mono = [0.0; 2];

        let layout = InterleavedLayout {
            frames: 2,
            capture_channels: 2,
            playback_channels: 2,
            guitar_input_channel: 1,
        };
        process_interleaved_i32(&input, &mut output, &mut mono, layout, &mut Invert)
            .expect("valid block");

        assert!((output[0] + 2000).abs() <= 1);
        assert_eq!(output[0], output[1]);
        assert!((output[2] + 4000).abs() <= 1);
        assert_eq!(output[2], output[3]);
    }

    #[test]
    fn rejects_an_invalid_channel_without_touching_output() {
        let input = [1000, 2000];
        let mut output = [7; 2];
        let mut mono = [0.0; 1];

        let layout = InterleavedLayout {
            frames: 1,
            capture_channels: 2,
            playback_channels: 2,
            guitar_input_channel: 2,
        };
        let result =
            process_interleaved_i32(&input, &mut output, &mut mono, layout, &mut Passthrough);

        assert!(result.is_err());
        assert_eq!(output, [7, 7]);
    }

    #[test]
    fn bypass_flag_skips_the_wrapped_processor_without_locking() {
        let bypassed = Arc::new(AtomicBool::new(true));
        let mut processor = BypassProcessor::new(Invert, Arc::clone(&bypassed));
        let mut samples = [0.25, -0.5];

        processor.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5]);

        bypassed.store(false, Ordering::Relaxed);
        processor.process(&mut samples);
        assert_eq!(samples, [-0.25, 0.5]);
    }

    #[test]
    fn prepared_processor_swaps_at_the_next_block_boundary() {
        struct Gain(f32);
        impl MonoProcessor for Gain {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= self.0;
                }
            }
        }

        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor = SwappableProcessor::new(Gain(2.0), Arc::clone(&mailbox));
        let mut first = [0.25];
        processor.process(&mut first);
        assert_eq!(first, [0.5]);

        mailbox.stage(Gain(3.0));
        let mut second = [0.25];
        processor.process(&mut second);
        assert_eq!(second, [0.75]);
    }

    #[test]
    fn serial_processor_runs_pedal_before_amp() {
        struct Add(f32);
        impl MonoProcessor for Add {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample += self.0;
                }
            }
        }
        struct Multiply(f32);
        impl MonoProcessor for Multiply {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= self.0;
                }
            }
        }

        let mut chain = SerialProcessor::new(Add(1.0), Multiply(3.0));
        let mut samples = [1.0];
        chain.process(&mut samples);

        assert_eq!(samples, [6.0]);
    }

    #[test]
    fn pedal_controls_apply_drive_parallel_mix_and_level() {
        struct Gain(f32);
        impl MonoProcessor for Gain {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= self.0;
                }
            }
        }

        let parameters = Arc::new(PedalParameters::new(6.0206, 0.5, 0.0));
        let mut processor = PedalControlProcessor::new(Gain(2.0), Arc::clone(&parameters), 64);
        let mut samples = [0.25];
        processor.process(&mut samples);

        assert!((samples[0] - 0.625).abs() < 0.0001);
        parameters.update(0.0, 0.0, -6.0206);
        processor.process(&mut samples);
        assert!((samples[0] - 0.3125).abs() < 0.0001);
    }

    #[test]
    fn neutral_amp_tone_controls_are_transparent() {
        let parameters = Arc::new(AmpParameters::new(0.0, 0.0, 0.0, 0.0));
        let mut processor = AmpControlProcessor::new(Passthrough, parameters, 48_000);
        let original = [0.25, -0.5, 0.75, -0.125];
        let mut samples = original;

        processor.process(&mut samples);

        for (actual, expected) in samples.iter().zip(original) {
            assert!((actual - expected).abs() < f32::EPSILON * 4.0);
        }
    }

    #[test]
    fn amp_volume_updates_without_rebuilding_the_processor() {
        let parameters = Arc::new(AmpParameters::new(0.0, 0.0, 0.0, 0.0));
        let mut processor = AmpControlProcessor::new(Passthrough, Arc::clone(&parameters), 48_000);
        parameters.update(0.0, 0.0, 0.0, -6.0206);
        let mut samples = [0.5, -0.5];

        processor.process(&mut samples);
        samples = [0.5, -0.5];
        processor.process(&mut samples);

        assert!((samples[0] - 0.25).abs() < 0.0001);
        assert!((samples[1] + 0.25).abs() < 0.0001);
    }

    #[test]
    fn safety_stage_blocks_dc_and_enforces_the_output_ceiling() {
        struct Gain(f32);
        impl MonoProcessor for Gain {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= self.0;
                }
            }
        }

        let telemetry = Arc::new(RealtimeTelemetry::default());
        let mut processor = SafetyProcessor::new(Gain(4.0), Arc::clone(&telemetry), 48_000);
        let mut loud = [0.5, -0.5];
        processor.process(&mut loud);
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        assert!(loud.iter().all(|sample| sample.abs() <= ceiling));
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.input_peak, 0.5);
        assert!(snapshot.clipped);

        let mut dc = [0.25; 4096];
        let mut dc_processor = SafetyProcessor::new(Passthrough, telemetry, 48_000);
        dc_processor.process(&mut dc);
        assert!(dc.last().expect("sample").abs() < 0.0001);
    }

    #[test]
    fn meter_peaks_reset_but_xruns_are_cumulative() {
        let telemetry = RealtimeTelemetry::default();
        telemetry.record_levels(0.25, 0.5, true);
        telemetry.record_levels(0.75, 0.4, false);
        telemetry.record_cpu_percent(12.5);
        telemetry.record_xrun();

        let first = telemetry.snapshot();
        assert_eq!(first.input_peak, 0.75);
        assert_eq!(first.output_peak, 0.5);
        assert!(first.clipped);
        assert_eq!(first.cpu_percent, 12.5);
        assert_eq!(first.xruns, 1);

        let second = telemetry.snapshot();
        assert_eq!(second.input_peak, 0.0);
        assert_eq!(second.output_peak, 0.0);
        assert!(!second.clipped);
        assert_eq!(second.xruns, 1);
    }
}
