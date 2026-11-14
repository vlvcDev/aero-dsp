use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};

pub const I32_TO_FLOAT: f32 = 1.0 / 2_147_483_648.0;
pub const FLOAT_TO_I32: f32 = 2_147_483_647.0;

pub trait MonoProcessor {
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        None
    }
    fn discontinuity(&mut self) {}
    fn process(&mut self, samples: &mut [f32]);
    /// Mono guitar input, stereo bus output. Mono processors remain compatible.
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.process(samples);
        for (sample, frame) in samples.iter().zip(output) {
            *frame = [*sample; 2];
        }
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimiterStage {
    Rig,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelemetrySnapshot {
    pub rig_limiter_reduction_db: f32,
    pub output_limiter_reduction_db: f32,
    pub input_peak: f32,
    pub output_peak: f32,
    pub clipped: bool,
    pub cpu_percent: f32,
    pub xruns: u64,
    pub worst_cpu_percent: f32,
    pub deadline_misses: u64,
}

pub struct RealtimeTelemetry {
    rig_limiter_gain_bits: AtomicU32,
    output_limiter_gain_bits: AtomicU32,
    input_peak_bits: AtomicU32,
    output_peak_bits: AtomicU32,
    clipped: AtomicBool,
    cpu_percent_bits: AtomicU32,
    xruns: AtomicU64,
    worst_cpu_percent_bits: AtomicU32,
    deadline_misses: AtomicU64,
    audio_status: AtomicU32,
    audio_error: Mutex<Option<String>>,
}

impl Default for RealtimeTelemetry {
    fn default() -> Self {
        Self {
            rig_limiter_gain_bits: AtomicU32::new(1.0_f32.to_bits()),
            output_limiter_gain_bits: AtomicU32::new(1.0_f32.to_bits()),
            input_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            output_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            clipped: AtomicBool::new(false),
            cpu_percent_bits: AtomicU32::new(0.0_f32.to_bits()),
            xruns: AtomicU64::new(0),
            worst_cpu_percent_bits: AtomicU32::new(0.0_f32.to_bits()),
            deadline_misses: AtomicU64::new(0),
            audio_status: AtomicU32::new(0),
            audio_error: Mutex::new(None),
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

    /// Record the minimum applied gain for one processing block.
    pub fn record_limiter_gain(&self, stage: LimiterStage, gain: f32) {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return;
        }
        let target = match stage {
            LimiterStage::Rig => &self.rig_limiter_gain_bits,
            LimiterStage::Output => &self.output_limiter_gain_bits,
        };
        // Positive finite floats have the same ordering as their bit patterns.
        target.fetch_min(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn record_levels(&self, input_peak: f32, output_peak: f32, clipped: bool) {
        Self::record_peak(&self.input_peak_bits, input_peak);
        Self::record_peak(&self.output_peak_bits, output_peak);
        if clipped {
            self.clipped.store(true, Ordering::Relaxed);
        }
    }

    pub fn record_cpu_percent(&self, cpu_percent: f32) {
        Self::record_peak(&self.worst_cpu_percent_bits, cpu_percent);
        if cpu_percent > 100.0 {
            self.deadline_misses.fetch_add(1, Ordering::Relaxed);
        }
        self.audio_status.store(2, Ordering::Relaxed);
        self.cpu_percent_bits
            .store(cpu_percent.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn record_xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }

    /// Worker/control-thread only; no locks are acquired in sample processing.
    pub fn set_audio_status(&self, status: u32, error: Option<String>) {
        if let Ok(mut current) = self.audio_error.lock() {
            *current = error;
        }
        self.audio_status.store(status, Ordering::Release);
    }

    pub fn audio_status(&self) -> (&'static str, Option<String>) {
        let status = match self.audio_status.load(Ordering::Acquire) {
            1 => "starting",
            2 => "running",
            3 => "recovering",
            _ => "disabled",
        };
        (status, self.audio_error.lock().ok().and_then(|s| s.clone()))
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let reduction_db = |gain: &AtomicU32| {
            let gain = f32::from_bits(gain.swap(1.0_f32.to_bits(), Ordering::Relaxed));
            if gain >= 1.0 {
                0.0
            } else {
                -20.0 * gain.max(f32::MIN_POSITIVE).log10()
            }
        };
        TelemetrySnapshot {
            rig_limiter_reduction_db: reduction_db(&self.rig_limiter_gain_bits),
            output_limiter_reduction_db: reduction_db(&self.output_limiter_gain_bits),
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
            worst_cpu_percent: f32::from_bits(self.worst_cpu_percent_bits.load(Ordering::Relaxed)),
            deadline_misses: self.deadline_misses.load(Ordering::Relaxed),
        }
    }
}

/// Corner of the post-rig DC blocker. Low enough to leave a 7-string's B1
/// (61.7 Hz) and a bass's E1 (41.2 Hz) untouched in level and phase, matching
/// Tone3000's 5 Hz output high-pass.
pub const DC_BLOCKER_HZ: f32 = 5.0;

/// Pole of the first-order DC blocker `y = c * (y[-1] + x - x[-1])`.
pub fn dc_blocker_coefficient(sample_rate: u32) -> f32 {
    (-2.0 * std::f32::consts::PI * DC_BLOCKER_HZ / sample_rate as f32).exp()
}

/// Flush denormal floats to zero on the calling thread (FTZ + DAZ on x86_64,
/// FPCR.FZ on aarch64). Decaying filter, reverb and NAM states otherwise fall
/// into denormals while the gate is closed and cause CPU spikes and xruns.
/// Call once at the start of the realtime audio thread; no-op elsewhere.
pub fn flush_denormals_to_zero() {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: only sets the FTZ (bit 15) and DAZ (bit 6) flags of this thread's MXCSR.
    unsafe {
        let mut csr: u32 = 0;
        std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr, options(nostack, preserves_flags));
        csr |= (1 << 15) | (1 << 6);
        std::arch::asm!("ldmxcsr [{}]", in(reg) &csr, options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: only sets the FZ (bit 24) flag of this thread's FPCR.
    unsafe {
        let mut fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
        fpcr |= 1 << 24;
        std::arch::asm!("msr fpcr, {}", in(reg) fpcr, options(nomem, nostack, preserves_flags));
    }
}

pub struct SafetyProcessor<P> {
    stereo_history: [[f32; 2]; 2],
    processor: P,
    telemetry: Arc<RealtimeTelemetry>,
    high_pass_coefficient: f32,
    previous_input: f32,
    previous_output: f32,
    limiter_stage: LimiterStage,
    limiter_ceiling: f32,
    limiter_gain: f32,
    limiter_release: f32,
}

impl<P> SafetyProcessor<P> {
    /// Transparent at normal rig levels: only protects the final monitor sum.
    /// Guitar DC filtering/limiting already happened inside the shared rig.
    pub fn monitor(processor: P, telemetry: Arc<RealtimeTelemetry>, sample_rate: u32) -> Self {
        let mut output = Self::new(processor, telemetry, sample_rate);
        output.high_pass_coefficient = 1.0;
        output.limiter_ceiling = 1.0;
        output.limiter_stage = LimiterStage::Output;
        output
    }
    pub fn new(processor: P, telemetry: Arc<RealtimeTelemetry>, sample_rate: u32) -> Self {
        Self {
            stereo_history: [[0.0; 2]; 2],
            processor,
            telemetry,
            high_pass_coefficient: dc_blocker_coefficient(sample_rate),
            previous_input: 0.0,
            previous_output: 0.0,
            limiter_stage: LimiterStage::Rig,
            limiter_ceiling: 10.0_f32.powf(-1.0 / 20.0),
            limiter_gain: 1.0,
            limiter_release: (-1.0 / (0.080 * sample_rate as f32)).exp(),
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for SafetyProcessor<P> {
    fn discontinuity(&mut self) {
        self.processor.discontinuity();
    }
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        let mut clipped = false;
        let mut input_peak = 0.0_f32;
        for sample in samples.iter_mut() {
            if !sample.is_finite() {
                *sample = 0.0;
                clipped = true;
            }
            input_peak = input_peak.max(sample.abs());
        }
        self.processor.process_stereo(samples, output);
        let mut output_peak = 0.0_f32;
        let mut minimum_gain = 1.0_f32;
        for frame in output {
            for (channel, sample) in frame.iter_mut().enumerate() {
                if !sample.is_finite() {
                    *sample = 0.0;
                    self.stereo_history[channel] = [0.0; 2];
                    clipped = true;
                }
                let input = *sample;
                if self.high_pass_coefficient != 1.0 {
                    *sample = self.high_pass_coefficient
                        * (self.stereo_history[channel][1]
                            + (input - self.stereo_history[channel][0]));
                }
                if !sample.is_finite() {
                    *sample = 0.0;
                    self.stereo_history[channel] = [0.0; 2];
                    clipped = true;
                    continue;
                }
                self.stereo_history[channel] = [input, *sample];
            }
            let peak = frame[0].abs().max(frame[1].abs());
            let target = self.limiter_ceiling / peak.max(self.limiter_ceiling);
            self.limiter_gain = if target < self.limiter_gain {
                target
            } else {
                target + self.limiter_release * (self.limiter_gain - target)
            };
            minimum_gain = minimum_gain.min(self.limiter_gain);
            clipped |= target < 1.0;
            for sample in frame {
                // Same final clamp as the mono path: rounding must never exceed the ceiling.
                *sample = (*sample * self.limiter_gain)
                    .clamp(-self.limiter_ceiling, self.limiter_ceiling);
                output_peak = output_peak.max(sample.abs());
            }
        }
        self.telemetry
            .record_limiter_gain(self.limiter_stage, minimum_gain);
        self.telemetry
            .record_levels(input_peak, output_peak, clipped);
    }
    fn process(&mut self, samples: &mut [f32]) {
        let mut clipped = false;
        // Invalid imported/model samples must not poison downstream filters.
        for sample in samples.iter_mut() {
            if !sample.is_finite() {
                *sample = 0.0;
                clipped = true;
            }
        }
        let input_peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        self.processor.process(samples);

        let mut output_peak = 0.0_f32;
        let mut minimum_gain = 1.0_f32;
        for sample in samples {
            if !sample.is_finite() {
                *sample = 0.0;
                self.previous_input = 0.0;
                self.previous_output = 0.0;
                self.limiter_gain = 1.0;
                clipped = true;
                continue;
            }
            // Block DC generated by the rig, not just DC present at its input.
            // This also leaves the raw DI recording tap genuinely unfiltered.
            let input = *sample;
            let output = if self.high_pass_coefficient == 1.0 {
                input
            } else {
                // Difference first: summing a large offset into y[-1] would
                // quantize it to the offset's precision and stall the decay.
                self.high_pass_coefficient * (self.previous_output + (input - self.previous_input))
            };
            if !output.is_finite() {
                self.previous_input = 0.0;
                self.previous_output = 0.0;
                self.limiter_gain = 1.0;
                *sample = 0.0;
                clipped = true;
                continue;
            }
            self.previous_input = input;
            self.previous_output = output;
            // Zero-lookahead peak limiting: immediate gain reduction, gradual
            // release. Unlike clipping this holds gain between waveform peaks.
            let target = (self.limiter_ceiling / output.abs().max(self.limiter_ceiling)).min(1.0);
            self.limiter_gain = if target < self.limiter_gain {
                target
            } else {
                target + self.limiter_release * (self.limiter_gain - target)
            };
            minimum_gain = minimum_gain.min(self.limiter_gain);
            clipped |= self.limiter_gain < 0.9999;
            *sample =
                (output * self.limiter_gain).clamp(-self.limiter_ceiling, self.limiter_ceiling);
            output_peak = output_peak.max(sample.abs());
        }
        self.telemetry
            .record_limiter_gain(self.limiter_stage, minimum_gain);
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
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        self.alignment()
    }
    fn process(&mut self, samples: &mut [f32]) {
        if nam_bridge::NamModel::process(self, samples).is_err() {
            samples.fill(0.0);
        }
    }
}

/// Fade time for model/patch swaps and bypass. 5 ms fades tick audibly on a
/// sustained note; Tone3000 uses 25 ms.
pub const TRANSITION_SECONDS: f32 = 0.02;

/// Smoothstep: maps a linear 0..1 ramp to an S-curve with zero slope at both
/// ends, so a fade has no slope discontinuity (part of what makes a short
/// linear fade click). Exact at 0 and 1, so settled states are untouched.
#[inline]
pub fn smooth_ramp(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

pub struct BypassProcessor<P> {
    processor: P,
    bypassed: Arc<AtomicBool>,
    wet: f32,
    step: f32,
}

impl<P> BypassProcessor<P> {
    pub fn new(processor: P, bypassed: Arc<AtomicBool>, sample_rate: u32) -> Self {
        Self {
            processor,
            wet: if bypassed.load(Ordering::Relaxed) {
                0.0
            } else {
                1.0
            },
            step: 1.0 / (TRANSITION_SECONDS * sample_rate.max(1) as f32),
            bypassed,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for BypassProcessor<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        let target = if self.bypassed.load(Ordering::Relaxed) {
            0.0
        } else {
            1.0
        };
        if target == 0.0 && self.wet == 0.0 {
            for (sample, frame) in samples.iter().zip(output) {
                *frame = [*sample; 2];
            }
            return;
        }
        let mut dry = [0.0; 128];
        for (input, out) in samples.chunks_mut(128).zip(output.chunks_mut(128)) {
            dry[..input.len()].copy_from_slice(input);
            self.processor.process_stereo(input, out);
            for (frame, dry) in out.iter_mut().zip(dry) {
                self.wet += (target - self.wet).clamp(-self.step, self.step);
                let wet = smooth_ramp(self.wet);
                for channel in frame {
                    *channel = dry + wet * (*channel - dry);
                }
            }
        }
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        if self.bypassed.load(Ordering::Relaxed) {
            None
        } else {
            self.processor.model_metadata()
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        let target = if self.bypassed.load(Ordering::Relaxed) {
            0.0
        } else {
            1.0
        };
        if target == 0.0 && self.wet == 0.0 {
            return;
        }
        for samples in samples.chunks_mut(128) {
            let mut dry = [0.0; 128];
            dry[..samples.len()].copy_from_slice(samples);
            self.processor.process(samples);
            for (wet, dry) in samples.iter_mut().zip(dry) {
                self.wet += (target - self.wet).clamp(-self.step, self.step);
                *wet = dry + smooth_ramp(self.wet) * (*wet - dry);
            }
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
    pub align_enabled: AtomicBool,
    drive_gain: AtomicParameter,
    mix: AtomicParameter,
    level_gain: AtomicParameter,
}

impl PedalParameters {
    pub fn with_alignment(self, enabled: bool) -> Self {
        self.align_enabled.store(enabled, Ordering::Relaxed);
        self
    }
    pub fn new(drive_db: f32, mix: f32, level_db: f32) -> Self {
        Self {
            align_enabled: AtomicBool::new(true),
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
    aligner: crate::dry_alignment::DryAligner,
    processor: P,
    parameters: Arc<PedalParameters>,
    dry: Vec<f32>,
    drive_gain: f32,
    mix: f32,
    level_gain: f32,
    smoothing: f32,
}

impl<P: MonoProcessor> PedalControlProcessor<P> {
    pub fn new(
        processor: P,
        parameters: Arc<PedalParameters>,
        max_frames: usize,
        sample_rate: u32,
    ) -> Self {
        let drive_gain = parameters.drive_gain.load();
        let mix = parameters.mix.load();
        let level_gain = parameters.level_gain.load();
        let delay = if parameters.align_enabled.load(Ordering::Relaxed) {
            processor
                .dry_wet_alignment()
                .map_or(0, |a| a.applied_delay())
        } else {
            0
        };
        Self {
            aligner: crate::dry_alignment::DryAligner::new(sample_rate, delay),
            processor,
            parameters,
            dry: vec![0.0; max_frames],
            drive_gain,
            mix,
            level_gain,
            smoothing: 1.0 - (-1.0 / (0.010 * sample_rate.max(1) as f32)).exp(),
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for PedalControlProcessor<P> {
    fn discontinuity(&mut self) {
        self.aligner.clear();
        self.processor.discontinuity();
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        self.processor.model_metadata()
    }
    fn process(&mut self, samples: &mut [f32]) {
        if samples.len() > self.dry.len() {
            samples.fill(0.0);
            return;
        }
        let dry = &mut self.dry[..samples.len()];
        dry.copy_from_slice(samples);
        let drive_target = self.parameters.drive_gain.load();
        for sample in samples.iter_mut() {
            self.drive_gain += self.smoothing * (drive_target - self.drive_gain);
            *sample *= self.drive_gain;
        }
        self.processor.process(samples);

        // Query AFTER process: SwappableProcessor can adopt a new capture in
        // this block. Do not retain the previous capture's delay on a swap.
        self.aligner
            .set_delay(if self.parameters.align_enabled.load(Ordering::Relaxed) {
                self.processor
                    .dry_wet_alignment()
                    .map_or(0, |a| a.applied_delay())
            } else {
                0
            });

        let mix_target = self.parameters.mix.load();
        let level_target = self.parameters.level_gain.load();
        for (wet, dry) in samples.iter_mut().zip(dry.iter()) {
            self.mix += self.smoothing * (mix_target - self.mix);
            self.level_gain += self.smoothing * (level_target - self.level_gain);
            let delayed = self.aligner.tick(*dry);
            // Pure dry stays undelayed. Fade this exception in only in the
            // last 0.1% of the mix travel to avoid a jump at the endpoint.
            let amount = (self.mix * 1000.0).clamp(0.0, 1.0);
            let dry = if amount == 1.0 {
                delayed
            } else {
                *dry + amount * (delayed - *dry)
            };
            *wet = (dry * (1.0 - self.mix) + *wet * self.mix) * self.level_gain;
        }
    }
}

pub struct AmpParameters {
    bass_gain: AtomicParameter,
    mid_gain: AtomicParameter,
    treble_gain: AtomicParameter,
    volume_gain: AtomicParameter,
    /// Pre-amp TIGHT high-pass corner in Hz; 0 = off.
    tight_hz: AtomicParameter,
}

impl AmpParameters {
    pub fn new(bass_db: f32, mid_db: f32, treble_db: f32, volume_db: f32) -> Self {
        Self {
            bass_gain: AtomicParameter::new(db_to_gain(bass_db)),
            mid_gain: AtomicParameter::new(db_to_gain(mid_db)),
            treble_gain: AtomicParameter::new(db_to_gain(treble_db)),
            volume_gain: AtomicParameter::new(db_to_gain(volume_db)),
            tight_hz: AtomicParameter::new(0.0),
        }
    }

    pub fn with_tight(self, tight_hz: f32) -> Self {
        self.set_tight(tight_hz);
        self
    }

    pub fn set_tight(&self, tight_hz: f32) {
        self.tight_hz
            .store(if tight_hz.is_finite() { tight_hz } else { 0.0 });
    }

    pub fn update(&self, bass_db: f32, mid_db: f32, treble_db: f32, volume_db: f32) {
        self.bass_gain.store(db_to_gain(bass_db));
        self.mid_gain.store(db_to_gain(mid_db));
        self.treble_gain.store(db_to_gain(treble_db));
        self.volume_gain.store(db_to_gain(volume_db));
    }
}

/// TIGHT range. Classic "tight boost" corners sit around 100-200 Hz.
pub const TIGHT_MIN_HZ: f32 = 20.0;
pub const TIGHT_MAX_HZ: f32 = 300.0;
const TIGHT_GLIDE_SECONDS: f32 = 0.02;

pub struct AmpControlProcessor<P> {
    processor: P,
    parameters: Arc<AmpParameters>,
    eq: crate::tone_eq::ToneEq,
    volume_gain: f32,
    smoothing: f32,
    /// 12 dB/oct high-pass on the amp model's INPUT. Cutting lows before the
    /// gain stage (what a tight boost or pedal low cut does in front of a
    /// real high-gain amp) keeps palm mutes from blooming into mud; cutting
    /// the same lows after the amp cannot undo that compression.
    tight: crate::svf::Svf,
    tight_frequency: f32,
    /// 0..1 crossfade so switching TIGHT on/off is click-free.
    tight_mix: f32,
    sample_rate: f32,
}
impl<P> AmpControlProcessor<P> {
    pub fn new(processor: P, parameters: Arc<AmpParameters>, sample_rate: u32) -> Self {
        let gains = [
            parameters.bass_gain.load(),
            parameters.mid_gain.load(),
            parameters.treble_gain.load(),
        ]
        .map(|g| 20.0 * g.max(1e-6).log10());
        let volume_gain = parameters.volume_gain.load();
        let tight_hz = parameters.tight_hz.load();
        let tight_on = tight_hz > 0.0;
        Self {
            processor,
            parameters,
            eq: crate::tone_eq::ToneEq::new(sample_rate, gains),
            volume_gain,
            smoothing: 1.0 - (-1.0 / (0.010 * sample_rate as f32)).exp(),
            tight: crate::svf::Svf::default(),
            tight_frequency: if tight_on {
                tight_hz.clamp(TIGHT_MIN_HZ, TIGHT_MAX_HZ)
            } else {
                100.0
            },
            tight_mix: if tight_on { 1.0 } else { 0.0 },
            sample_rate: sample_rate as f32,
        }
    }

    fn apply_tight(&mut self, samples: &mut [f32]) {
        let tight_hz = self.parameters.tight_hz.load();
        let target_mix = if tight_hz > 0.0 { 1.0 } else { 0.0 };
        if target_mix == 0.0 && self.tight_mix == 0.0 {
            self.tight = crate::svf::Svf::default();
            return;
        }
        if tight_hz > 0.0 {
            let glide =
                1.0 - (-(samples.len() as f32) / (TIGHT_GLIDE_SECONDS * self.sample_rate)).exp();
            let target = tight_hz.clamp(TIGHT_MIN_HZ, TIGHT_MAX_HZ);
            self.tight_frequency += (target - self.tight_frequency) * glide;
        }
        let coefficients = crate::svf::SvfCoefficients::new(self.tight_frequency, self.sample_rate);
        let step = 1.0 / (TRANSITION_SECONDS * self.sample_rate);
        for sample in samples {
            let (_, high_passed) = self.tight.tick(*sample, coefficients);
            self.tight_mix += (target_mix - self.tight_mix).clamp(-step, step);
            *sample += (high_passed - *sample) * smooth_ramp(self.tight_mix);
        }
    }
}
impl<P: MonoProcessor> MonoProcessor for AmpControlProcessor<P> {
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        self.processor.model_metadata()
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.apply_tight(samples);
        self.processor.process(samples);
        self.eq.set_gains(
            [
                self.parameters.bass_gain.load(),
                self.parameters.mid_gain.load(),
                self.parameters.treble_gain.load(),
            ]
            .map(|g| 20.0 * g.max(1e-6).log10()),
        );
        let target = self.parameters.volume_gain.load();
        for sample in samples {
            self.volume_gain += (target - self.volume_gain) * self.smoothing;
            *sample = self.eq.tick(*sample) * self.volume_gain;
        }
    }
}

pub enum AmpProcessor {
    Clean,
    Nam(crate::nam_oversampling::OversampledNam),
}

impl MonoProcessor for AmpProcessor {
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        match self {
            Self::Clean => None,
            Self::Nam(m) => m.alignment(),
        }
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        match self {
            Self::Clean => None,
            Self::Nam(model) => Some(model.metadata()),
        }
    }
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
    /// Replaced values awaiting the control thread. Preallocated so the
    /// audio thread can retire without allocating, and never drops from it.
    retired: Vec<P>,
}

/// Retired values held between control-thread stages. Each stage clears the
/// list and every audio-side swap consumes one staged value, so at most two
/// are ever waiting (a crossfade finishing just after the next stage).
const RETIRED_CAPACITY: usize = 4;

pub struct SwapMailbox<P> {
    slot: Mutex<SwapSlot<P>>,
}

impl<P> Default for SwapMailbox<P> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(SwapSlot {
                pending: None,
                retired: Vec::with_capacity(RETIRED_CAPACITY),
            }),
        }
    }
}

impl<P> SwapMailbox<P> {
    pub fn has_pending(&self) -> bool {
        self.slot
            .try_lock()
            .is_ok_and(|slot| slot.pending.is_some())
    }
    /// Non-blocking: whether a staged value exists and satisfies `predicate`.
    pub fn pending_matches(&self, predicate: impl FnOnce(&P) -> bool) -> bool {
        self.slot
            .try_lock()
            .is_ok_and(|slot| slot.pending.as_ref().is_some_and(predicate))
    }
    pub fn stage(&self, prepared: P) {
        let mut slot = self.slot.lock().expect("processor swap mailbox poisoned");
        // Both values are dropped here, on the control/loader thread.
        slot.retired.clear();
        slot.pending = Some(prepared);
    }

    /// Control-thread collection after an acknowledged swap.
    pub fn take_retired(&self) -> Option<P> {
        self.slot
            .lock()
            .expect("swap mailbox poisoned")
            .retired
            .pop()
    }

    /// Non-blocking handoff for a processor that owns its own active state.
    /// The replaced value is retained in the mailbox and dropped next time a
    /// control-thread stage occurs, never from the audio callback.
    pub fn swap_pending(&self, active: &mut P) -> bool {
        match self.slot.try_lock() {
            Ok(mut slot) => {
                if slot.retired.len() == RETIRED_CAPACITY {
                    return false; // never reallocate on the audio thread
                }
                let Some(prepared) = slot.pending.take() else {
                    return false;
                };
                let old = std::mem::replace(active, prepared);
                slot.retired.push(old);
                true
            }
            Err(TryLockError::WouldBlock | TryLockError::Poisoned(_)) => false,
        }
    }

    /// Non-blocking: takes the staged value if it satisfies `predicate`,
    /// leaving retirement of the value it replaces to the caller (e.g. after
    /// a crossfade, via [`Self::retire`]).
    pub fn take_pending_if(&self, predicate: impl FnOnce(&P) -> bool) -> Option<P> {
        let mut slot = self.slot.try_lock().ok()?;
        if slot.pending.as_ref().is_some_and(predicate) {
            slot.pending.take()
        } else {
            None
        }
    }

    /// Non-blocking, allocation-free hand-back of a replaced value for the
    /// control thread to drop. Returns it if the mailbox is busy or full;
    /// the caller keeps it and retries later.
    pub fn retire(&self, value: P) -> Result<(), P> {
        match self.slot.try_lock() {
            Ok(mut slot) if slot.retired.len() < RETIRED_CAPACITY => {
                slot.retired.push(value);
                Ok(())
            }
            _ => Err(value),
        }
    }
}

pub struct SwappableProcessor<P> {
    active: P,
    mailbox: Arc<SwapMailbox<P>>,
    gain: f32,
    step: f32,
    swapping: bool,
}

impl<P> SwappableProcessor<P> {
    pub fn new(active: P, mailbox: Arc<SwapMailbox<P>>, sample_rate: u32) -> Self {
        Self {
            active,
            mailbox,
            gain: 1.0,
            step: 1.0 / (TRANSITION_SECONDS * sample_rate.max(1) as f32),
            swapping: false,
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for SwappableProcessor<P> {
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        self.active.dry_wet_alignment()
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        self.active.model_metadata()
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.swapping |= self.mailbox.has_pending();
        if self.swapping && self.gain == 0.0 && self.mailbox.swap_pending(&mut self.active) {
            self.swapping = false;
        }
        self.active.process(samples);
        let target = if self.swapping { 0.0 } else { 1.0 };
        for sample in samples {
            self.gain += (target - self.gain).clamp(-self.step, self.step);
            *sample *= smooth_ramp(self.gain);
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

    let mut stereo = [[0.0_f32; 2]; 1024];
    for (block, chunk) in mono.chunks_mut(1024).enumerate() {
        processor.process_stereo(chunk, &mut stereo[..chunk.len()]);
        for (i, sample) in stereo[..chunk.len()].iter().enumerate() {
            let frame_start = (block * 1024 + i) * layout.playback_channels;
            for channel in 0..layout.playback_channels {
                let value = if layout.playback_channels == 1 {
                    (sample[0] + sample[1]) * 0.5
                } else if channel < 2 {
                    sample[channel]
                } else {
                    0.0
                };
                output[frame_start + channel] = (value.clamp(-1.0, 1.0) * FLOAT_TO_I32) as i32;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Stand-in amp model that records the RMS of the signal it receives.
    struct InputProbe(Arc<std::sync::Mutex<Vec<f32>>>);
    impl MonoProcessor for InputProbe {
        fn process(&mut self, samples: &mut [f32]) {
            self.0.lock().unwrap().extend_from_slice(samples);
        }
    }

    fn tight_amp(
        tight_hz: f32,
    ) -> (
        AmpControlProcessor<InputProbe>,
        Arc<std::sync::Mutex<Vec<f32>>>,
        Arc<AmpParameters>,
    ) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let parameters = Arc::new(AmpParameters::new(0.0, 0.0, 0.0, 0.0).with_tight(tight_hz));
        let amp = AmpControlProcessor::new(
            InputProbe(Arc::clone(&seen)),
            Arc::clone(&parameters),
            48_000,
        );
        (amp, seen, parameters)
    }

    fn model_input_gain_db(tight_hz: f32, frequency: f32) -> f32 {
        let (mut amp, seen, _) = tight_amp(tight_hz);
        let input: Vec<f32> = (0..24_000)
            .map(|n| (std::f32::consts::TAU * frequency * n as f32 / 48_000.0).sin())
            .collect();
        for block in input.chunks(128) {
            amp.process(&mut block.to_vec());
        }
        let seen = seen.lock().unwrap();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        20.0 * (rms(&seen[12_000..]) / rms(&input[12_000..])).log10()
    }

    #[test]
    fn tight_high_passes_the_amp_models_input_at_12_db_per_octave() {
        // One octave below a 150 Hz corner: -12.3 dB, before the model.
        assert!((model_input_gain_db(150.0, 75.0) + 12.3).abs() < 0.3);
        assert!((model_input_gain_db(150.0, 150.0) + 3.0).abs() < 0.2);
        assert!(model_input_gain_db(150.0, 1_000.0).abs() < 0.1);
        // Off is exactly transparent.
        let (mut amp, seen, _) = tight_amp(0.0);
        let mut block: Vec<f32> = (0..256).map(|n| (n as f32 * 0.01).sin()).collect();
        let original = block.clone();
        amp.process(&mut block);
        assert_eq!(*seen.lock().unwrap(), original);
    }

    #[test]
    fn switching_and_sweeping_tight_is_click_free() {
        let (mut amp, seen, parameters) = tight_amp(0.0);
        let tone = |n: usize| 0.5 * (std::f32::consts::TAU * 220.0 * n as f32 / 48_000.0).sin();
        for block in 0..600 {
            if block % 25 == 0 {
                parameters.set_tight([0.0, 300.0, 20.0, 150.0][(block / 25) % 4]);
            }
            let mut samples: Vec<f32> = (0..64).map(|i| tone(block * 64 + i)).collect();
            amp.process(&mut samples);
        }
        let seen = seen.lock().unwrap();
        assert!(seen.iter().all(|x| x.is_finite()));
        let worst = seen
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        // A 220 Hz sine at 0.5 moves at most ~0.0144 per sample.
        assert!(worst < 0.03, "{worst}");
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn denormal_flush_applies_to_the_calling_thread_only() {
        let tiny = || std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.25);
        let flushed = std::thread::spawn(move || {
            super::flush_denormals_to_zero();
            tiny()
        })
        .join()
        .unwrap();
        assert_eq!(flushed, 0.0);
        assert!(std::thread::spawn(tiny).join().unwrap().is_subnormal());
    }

    use super::*;
    #[test]
    fn stereo_output_routing_and_limiter_preserve_the_channel_ratio() {
        struct Stereo;
        impl MonoProcessor for Stereo {
            fn process(&mut self, _: &mut [f32]) {}
            fn process_stereo(&mut self, _: &mut [f32], output: &mut [[f32; 2]]) {
                output.fill([2.0, -1.0]);
            }
        }
        let mut processor =
            SafetyProcessor::monitor(Stereo, Arc::new(RealtimeTelemetry::default()), 48000);
        let mut out = [0; 4];
        process_interleaved_i32(
            &[0; 4],
            &mut out,
            &mut [0.0; 2],
            InterleavedLayout {
                frames: 2,
                capture_channels: 2,
                playback_channels: 2,
                guitar_input_channel: 0,
            },
            &mut processor,
        )
        .unwrap();
        assert_eq!(out, [i32::MAX, -1073741824, i32::MAX, -1073741824]);
    }

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
    fn bypass_crossfades_without_locking() {
        let bypassed = Arc::new(AtomicBool::new(true));
        let mut processor = BypassProcessor::new(Invert, Arc::clone(&bypassed), 48_000);
        let mut samples = [0.25, -0.5];

        processor.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5]);

        bypassed.store(false, Ordering::Relaxed);
        processor.process(&mut samples);
        assert!(samples[0] > 0.24); // No instantaneous polarity flip.
        processor.process(&mut [0.0; 1024]);
        let mut samples = [0.25, -0.5];
        processor.process(&mut samples);
        assert_eq!(samples, [-0.25, 0.5]);
    }

    #[test]
    fn prepared_processor_fades_before_swapping_at_a_block_boundary() {
        struct Gain(f32);
        impl MonoProcessor for Gain {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= self.0;
                }
            }
        }

        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor = SwappableProcessor::new(Gain(2.0), Arc::clone(&mailbox), 48_000);
        let mut first = [0.25];
        processor.process(&mut first);
        assert_eq!(first, [0.5]);

        mailbox.stage(Gain(3.0));
        let mut second = [0.25];
        processor.process(&mut second);
        assert!(second[0] < 0.5 && second[0] > 0.49);
        // 20 ms out, swap, 20 ms in: 1920 samples at 48 kHz.
        processor.process(&mut [0.25; 1024]);
        processor.process(&mut [0.25; 1024]);
        second[0] = 0.25;
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
        let mut processor =
            PedalControlProcessor::new(Gain(2.0), Arc::clone(&parameters), 64, 48_000);
        let mut samples = [0.25];
        processor.process(&mut samples);

        assert!((samples[0] - 0.625).abs() < 0.0001);
        parameters.update(0.0, 0.0, -6.0206);
        for _ in 0..200 {
            let mut settle = [0.625; 64];
            processor.process(&mut settle);
            samples[0] = settle[63];
        }
        assert!((samples[0] - 0.3125).abs() < 0.0001);
    }

    #[test]
    fn pedal_smoothing_is_independent_of_callback_size() {
        let render = |block| {
            let parameters = Arc::new(PedalParameters::new(0.0, 1.0, 0.0));
            let mut p = PedalControlProcessor::new(Passthrough, parameters.clone(), block, 48_000);
            parameters.update(12.0, 0.4, -6.0);
            let mut output = vec![0.1; 4800];
            for chunk in output.chunks_mut(block) {
                p.process(chunk);
            }
            output
        };
        assert_eq!(render(64), render(517));
    }

    #[test]
    fn audio_health_preserves_worst_deadline_and_reports_recovery() {
        let telemetry = RealtimeTelemetry::default();
        assert_eq!(telemetry.audio_status().0, "disabled");
        telemetry.set_audio_status(3, Some("device disconnected".into()));
        assert_eq!(
            telemetry.audio_status(),
            ("recovering", Some("device disconnected".into()))
        );
        telemetry.set_audio_status(1, None);
        telemetry.record_cpu_percent(125.0);
        telemetry.record_cpu_percent(20.0);
        assert_eq!(telemetry.audio_status(), ("running", None));
        for _ in 0..2 {
            let snapshot = telemetry.snapshot();
            assert_eq!(snapshot.worst_cpu_percent, 125.0);
            assert_eq!(snapshot.deadline_misses, 1);
            assert_eq!(snapshot.cpu_percent, 20.0);
        }
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
        assert!(samples[0] > 0.25 && samples[0] < 0.5);
        for _ in 0..4800 {
            processor.process(&mut [0.0]);
        }
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

        // 0.5 s: a 5 Hz corner settles more slowly than the old 30 Hz one.
        let mut dc = [0.25; 24_000];
        let mut dc_processor = SafetyProcessor::new(Passthrough, telemetry, 48_000);
        dc_processor.process(&mut dc);
        assert!(dc.last().expect("sample").abs() < 0.0001);
    }

    #[test]
    fn safety_rejects_model_dc_and_recovers_from_invalid_samples() {
        struct Offset;
        impl MonoProcessor for Offset {
            fn process(&mut self, s: &mut [f32]) {
                s.fill(0.2);
            }
        }
        let mut p = SafetyProcessor::new(Offset, Arc::new(RealtimeTelemetry::default()), 48_000);
        let mut audio = vec![0.0; 48_000];
        p.process(&mut audio);
        assert!(audio.last().unwrap().abs() < 1e-5);
        let mut p =
            SafetyProcessor::new(Passthrough, Arc::new(RealtimeTelemetry::default()), 48_000);
        let mut bad = [f32::NAN, f32::INFINITY, f32::MAX, -f32::MAX];
        p.process(&mut bad);
        assert!(bad.iter().all(|s| s.is_finite() && s.abs() <= 0.892));
        let mut good = [0.1; 64];
        p.process(&mut good);
        assert!(good.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn limiter_holds_gain_between_peaks_instead_of_clipping_each_sample() {
        let mut p =
            SafetyProcessor::new(Passthrough, Arc::new(RealtimeTelemetry::default()), 48_000);
        let mut audio = [2.0, 0.5, -0.5];
        p.process(&mut audio);
        assert!(audio[0] <= 0.892);
        assert!(audio[1].abs() < 0.24);
        assert!(audio[2].abs() < 0.24);
    }

    #[test]
    fn limiter_telemetry_tracks_interval_peak_and_stages() {
        let telemetry = RealtimeTelemetry::default();
        telemetry.record_limiter_gain(LimiterStage::Rig, 0.5);
        telemetry.record_limiter_gain(LimiterStage::Rig, 0.8);
        telemetry.record_limiter_gain(LimiterStage::Output, 0.25);
        for bad in [f32::NAN, f32::INFINITY, -1.0, 2.0] {
            telemetry.record_limiter_gain(LimiterStage::Rig, bad);
        }
        let first = telemetry.snapshot();
        assert!((first.rig_limiter_reduction_db - 6.0206).abs() < 0.001);
        assert!((first.output_limiter_reduction_db - 12.0412).abs() < 0.001);
        let next = telemetry.snapshot();
        assert_eq!(next.rig_limiter_reduction_db, 0.0);
        assert_eq!(next.output_limiter_reduction_db, 0.0);
    }

    #[test]
    fn limiter_telemetry_reports_release_and_stereo_linked_gain() {
        let telemetry = Arc::new(RealtimeTelemetry::default());
        let mut mono = SafetyProcessor::monitor(Passthrough, Arc::clone(&telemetry), 48_000);
        mono.process(&mut [2.0]);
        let first = telemetry.snapshot();
        assert!((first.output_limiter_reduction_db - 6.0206).abs() < 0.001);
        assert_eq!(first.rig_limiter_reduction_db, 0.0);
        mono.process(&mut [0.0; 16]);
        let release = telemetry.snapshot().output_limiter_reduction_db;
        assert!(release > 0.0 && release < first.output_limiter_reduction_db);

        struct Stereo;
        impl MonoProcessor for Stereo {
            fn process(&mut self, _: &mut [f32]) {}
            fn process_stereo(&mut self, _: &mut [f32], output: &mut [[f32; 2]]) {
                output[0] = [2.0, 0.5];
            }
        }
        let mut stereo = SafetyProcessor::monitor(Stereo, Arc::clone(&telemetry), 48_000);
        let mut output = [[0.0; 2]];
        stereo.process_stereo(&mut [0.0], &mut output);
        assert!((output[0][0] - 1.0).abs() < 0.0001);
        assert!((output[0][1] - 0.25).abs() < 0.0001);
        assert!((telemetry.snapshot().output_limiter_reduction_db - 6.0206).abs() < 0.001);
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
