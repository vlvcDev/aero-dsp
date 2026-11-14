//! Cabinet impulse-response loading and real-time processing.
//!
//! Loading, decoding, resampling, and trimming happen on the control thread.
//! Prepared partitioned convolution retains 500 ms by default (long enough for
//! room/mic tails) without added algorithmic latency. IR onset/phase is
//! preserved apart from trimming leading silence; truncation uses a taper; DC
//! is removed at load.

use crate::dsp::{MonoProcessor, SwapMailbox};
use crate::svf::{Svf, SvfCoefficients};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const DEFAULT_IR_SECONDS: f32 = 0.5;
/// Listening headroom for cabinet IRs, before the user's LEVEL control.
pub const CABINET_PAD_DB: f32 = -18.0;
const NORMALIZATION_REFERENCE_RATE: f64 = 48_000.0;
/// DC removal only runs on kernels at least this long, so its correction
/// (a Hann-shaped offset whose spectrum spans about 2 / length) stays below
/// ~40 Hz and never touches the audible cabinet response.
const DC_REMOVAL_MIN_SECONDS: f32 = 0.05;
/// A mono rig plays one channel of a stereo IR. Channel 0 is used (as in
/// Tone3000) unless it is this much quieter than channel 1.
const MONO_CHANNEL_FALLBACK_DB: f64 = 30.0;
/// Leading silence ends at the first sample within this many dB of the peak.
const ONSET_THRESHOLD_DB: f32 = -60.0;
/// Kept before the detected onset so the rising edge, and any low-level
/// pre-ringing from sample-rate conversion, survive the trim.
const ONSET_MARGIN_SECONDS: f32 = 0.0005;
/// Old-to-new IR crossfade length when a different IR is selected. Both
/// convolvers run for this long, so the cab never drops out to the raw amp.
const IR_CROSSFADE_SECONDS: f32 = 0.03;
/// Cut-frequency glide time when a knob moves.
const CUT_GLIDE_SECONDS: f32 = 0.02;

/// Frames of leading silence to drop: everything before the earliest onset in
/// any channel, minus a short margin. One trim point for all channels keeps a
/// stereo IR's two mics time-aligned with each other. Silence before the
/// onset is pure delay: it costs convolution CPU and adds latency without
/// changing the cabinet's tone.
fn leading_silence_frames(samples: &[f32], channels: usize, rate: u32) -> usize {
    let peak = samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
    if peak <= 0.0 {
        return 0;
    }
    let threshold = peak * 10.0_f32.powf(ONSET_THRESHOLD_DB / 20.0);
    let onset = samples
        .iter()
        .position(|sample| sample.abs() >= threshold)
        .map_or(0, |index| index / channels);
    onset.saturating_sub((rate as f32 * ONSET_MARGIN_SECONDS) as usize)
}

/// Removes the kernel's 0 Hz response (sum of taps) in place by subtracting
/// a Hann-shaped offset, which confines the change to the lowest bins.
/// Many captured IRs carry DC from the measurement or truncation; convolving
/// with it pushes an offset and sub-bass rumble into the cab output.
fn remove_dc(samples: &mut [f32], channels: usize) {
    let frames = samples.len() / channels;
    if frames < 2 {
        return;
    }
    let window: Vec<f64> = (0..frames)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * (i as f64 + 0.5) / frames as f64).cos())
        .collect();
    let window_sum: f64 = window.iter().sum();
    for channel in 0..channels {
        let sum: f64 = samples
            .iter()
            .skip(channel)
            .step_by(channels)
            .map(|s| f64::from(*s))
            .sum();
        let scale = sum / window_sum;
        for (sample, weight) in samples
            .iter_mut()
            .skip(channel)
            .step_by(channels)
            .zip(&window)
        {
            *sample = (f64::from(*sample) - scale * weight) as f32;
        }
    }
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

/// Runtime controls intentionally remain independent of an IR selection so
/// they can be changed without rebuilding cabinet buffers.
pub struct CabinetParameters {
    enabled: AtomicBool,
    gain_db: AtomicFloat,
    low_cut_hz: AtomicFloat,
    high_cut_hz: AtomicFloat,
}

impl CabinetParameters {
    pub fn new(enabled: bool, gain_db: f32, low_cut_hz: f32, high_cut_hz: f32) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            gain_db: AtomicFloat::new(gain_db),
            low_cut_hz: AtomicFloat::new(low_cut_hz),
            high_cut_hz: AtomicFloat::new(high_cut_hz),
        }
    }

    pub fn update(&self, enabled: bool, gain_db: f32, low_cut_hz: f32, high_cut_hz: f32) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.gain_db.store(gain_db);
        self.low_cut_hz.store(low_cut_hz);
        self.high_cut_hz.store(high_cut_hz);
    }
}

pub struct CabinetImpulse {
    convolution: crate::convolution::Convolver,
    right: Option<crate::convolution::Convolver>,
    /// Which channel a mono rig plays (1 only for a stereo IR whose left
    /// channel is effectively silent).
    mono_channel: usize,
    /// Level correction for the mono path, so the chosen channel is
    /// normalized on its own energy rather than the louder channel's.
    mono_gain: f32,
}

impl CabinetImpulse {
    pub fn load(path: &Path, destination_rate: u32) -> Result<Self, String> {
        Self::load_with_duration(path, destination_rate, DEFAULT_IR_SECONDS)
    }

    pub fn load_with_duration(
        path: &Path,
        destination_rate: u32,
        seconds: f32,
    ) -> Result<Self, String> {
        Self::load_channels(path, destination_rate, seconds, false)
    }

    fn load_channels(
        path: &Path,
        destination_rate: u32,
        seconds: f32,
        stereo_input: bool,
    ) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&destination_rate) || !seconds.is_finite() {
            return Err("Invalid cabinet rate or duration".into());
        }
        if !path.is_absolute() || !path.is_file() {
            return Err("cabinet IR must be an existing absolute WAV file".to_owned());
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
        {
            return Err("cabinet IR must be a .wav file".to_owned());
        }
        let audio = crate::audio_file::AudioFile::load(path)?;
        if !(1..=2).contains(&audio.channels) {
            return Err("cabinet IR must have one or two channels".into());
        }
        let mut samples = crate::audio_file::resample(
            &audio.samples,
            audio.channels,
            audio.sample_rate,
            destination_rate,
        )?;
        // IR taps are filter coefficients, not ordinary recorded audio.
        // Preserve the transfer gain when changing the number of taps/second.
        let rate_scale = audio.sample_rate as f32 / destination_rate as f32;
        for sample in &mut samples {
            *sample *= rate_scale;
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err("cabinet IR became nonfinite during sample-rate conversion".into());
        }
        // Trim before truncating, so removed silence frees room for tail.
        let silence = leading_silence_frames(&samples, audio.channels, destination_rate);
        samples.drain(..silence * audio.channels);
        let limit = (destination_rate as f32 * seconds.clamp(0.02, 1.0)) as usize;
        if samples.len() / audio.channels > limit {
            samples.truncate(limit * audio.channels);
            let taper = ((destination_rate as f32 * 0.005) as usize).min(limit);
            for (i, frame) in samples
                .chunks_exact_mut(audio.channels)
                .skip(limit - taper)
                .enumerate()
            {
                let gain = 0.5
                    * (1.0 + (std::f32::consts::PI * i as f32 / (taper - 1).max(1) as f32).cos());
                for sample in frame {
                    *sample *= gain;
                }
            }
        }
        let peak = samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        if peak < 1e-10 {
            return Err("cabinet IR contains no audible samples".into());
        }
        if samples.len() / audio.channels
            >= (destination_rate as f32 * DC_REMOVAL_MIN_SECONDS) as usize
        {
            remove_dc(&mut samples, audio.channels);
        }
        // Level from the actual retained kernel, including any tail taper.
        // Refer energy to 48 kHz so changing the engine rate doesn't change
        // the normalization. Link channels using the louder channel: preserve
        // stereo balance and keep duplicated mono identical to a mono IR.
        let mut energy = [0.0_f64; 2];
        for frame in samples.chunks_exact(audio.channels) {
            for (channel, sample) in frame.iter().enumerate() {
                energy[channel] += f64::from(*sample).powi(2);
            }
        }
        let to_reference = f64::from(destination_rate) / NORMALIZATION_REFERENCE_RATE;
        let reference_energy = energy[0].max(energy[1]) * to_reference;
        let normalization = 1.0 / reference_energy.max(1.0).sqrt();
        // Averaging a stereo IR's two mics in mono comb-filters wherever their
        // timing differs, so a mono rig plays one channel instead.
        let mono_channel = usize::from(
            audio.channels == 2
                && energy[1] > energy[0] * 10.0_f64.powf(MONO_CHANNEL_FALLBACK_DB / 10.0),
        );
        let mono_normalization = 1.0 / (energy[mono_channel] * to_reference).max(1.0).sqrt();
        let mono_gain = (mono_normalization / normalization) as f32;
        let level = normalization * 10.0_f64.powf(f64::from(CABINET_PAD_DB) / 20.0);
        for sample in &mut samples {
            *sample = (f64::from(*sample) * level) as f32;
        }
        let left: Vec<_> = samples.chunks_exact(audio.channels).map(|f| f[0]).collect();
        let mut impulse = Self::from_samples(&left);
        impulse.mono_channel = mono_channel;
        impulse.mono_gain = mono_gain;
        if audio.channels == 2 {
            let right: Vec<_> = samples.as_chunks::<2>().0.iter().map(|f| f[1]).collect();
            impulse.right = Some(crate::convolution::Convolver::new(&right));
        } else if stereo_input {
            impulse.right = Some(crate::convolution::Convolver::new(&left));
        }
        Ok(impulse)
    }

    fn tick_stereo(&mut self, input: f32) -> [f32; 2] {
        let left = self.convolution.tick(input);
        [left, self.right.as_mut().map_or(left, |r| r.tick(input))]
    }

    /// A stereo IR plays one channel in mono; averaging its two mics would
    /// comb-filter wherever their timing differs.
    fn tick_mono(&mut self, input: f32) -> f32 {
        let convolver = match (self.mono_channel, self.right.as_mut()) {
            (1, Some(right)) => right,
            _ => &mut self.convolution,
        };
        convolver.tick(input) * self.mono_gain
    }

    fn from_samples(samples: &[f32]) -> Self {
        Self {
            convolution: crate::convolution::Convolver::new(samples),
            right: None,
            mono_channel: 0,
            mono_gain: 1.0,
        }
    }
}

/// Stereo-in/stereo-out cabinet for the experimental node graph. Mono IRs use
/// independent L/R histories; stereo IRs are diagonal, not a 4-channel matrix.
pub struct CabinetNode(CabinetImpulse);
impl CabinetNode {
    pub fn load(path: &Path, rate: u32, seconds: f32) -> Result<Self, String> {
        if !(8_000..=192_000).contains(&rate)
            || !seconds.is_finite()
            || !(0.02..=1.0).contains(&seconds)
        {
            return Err("Invalid cabinet rate or duration".into());
        }
        if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
            return Err("Cabinet IR exceeds 64 MiB".into());
        }
        Ok(Self(CabinetImpulse::load_channels(
            path, rate, seconds, true,
        )?))
    }
}
impl crate::node_rig::StereoNode for CabinetNode {
    fn latency_samples(&self) -> usize {
        0
    }
    fn tick(&mut self, input: [f32; 2]) -> [f32; 2] {
        [
            self.0.convolution.tick(input[0]),
            self.0
                .right
                .as_mut()
                .expect("prepared stereo history")
                .tick(input[1]),
        ]
    }
    fn reset(&mut self) {
        self.0.convolution.clear();
        if let Some(right) = &mut self.0.right {
            right.clear();
        }
    }
}

#[cfg(test)]
mod node_tests {
    use super::*;
    use crate::node_rig::StereoNode;
    #[test]
    fn mono_ir_keeps_independent_stereo_history_and_reset_clears_fft_tail() {
        let path = std::env::temp_dir().join(format!(
            "aero-node-cab-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut taps = vec![0.0; 300];
        taps[0] = 0.5;
        taps[129] = 0.2;
        taps[299] = -0.1;
        crate::audio_file::write_float_wav(&path, 1, 48_000, taps.iter().copied()).unwrap();
        let mut node = CabinetNode::load(&path, 48_000, 0.2).unwrap();
        assert!(CabinetNode::load(&path, 48_000, f32::NAN).is_err());
        std::fs::remove_file(path).unwrap();
        let padded: Vec<_> = taps
            .iter()
            .map(|tap| (f64::from(*tap) * 10.0_f64.powf(f64::from(CABINET_PAD_DB) / 20.0)) as f32)
            .collect();
        let mut left = crate::convolution::Convolver::new(&padded);
        let mut right = crate::convolution::Convolver::new(&padded);
        assert_eq!(node.latency_samples(), 0);
        for n in 0..700 {
            let input = [
                if n == 0 { 1.0 } else { 0.0 },
                if n == 37 { -0.6 } else { 0.0 },
            ];
            assert_eq!(
                node.tick(input),
                [left.tick(input[0]), right.tick(input[1])]
            );
        }
        for _ in 0..260 {
            node.tick([1.0, -0.5]);
        }
        node.reset();
        for _ in 0..700 {
            assert_eq!(node.tick([0.0; 2]), [0.0; 2]);
        }
    }
}

/// Low cut then high cut for one output channel.
#[derive(Clone, Copy, Default)]
struct CutFilters {
    low_cut: Svf,
    high_cut: Svf,
}

/// Per-block settings shared by both channels.
#[derive(Clone, Copy)]
struct CutSettings {
    low: SvfCoefficients,
    high: SvfCoefficients,
}

impl CutFilters {
    /// The filters always run, so switching a cut on or off is a short
    /// crossfade between already-settled signals (`low_mix`/`high_mix`).
    #[inline]
    fn tick(&mut self, input: f32, settings: CutSettings, low_mix: f32, high_mix: f32) -> f32 {
        let (_, high_passed) = self.low_cut.tick(input, settings.low);
        let shaped = input + (high_passed - input) * low_mix;
        let (low_passed, _) = self.high_cut.tick(shaped, settings.high);
        shaped + (low_passed - shaped) * high_mix
    }
}

/// A single, zero-latency convolution cabinet. Swapping a loaded IR is safe:
/// the old allocation is retired on the control thread by `SwapMailbox`.
pub struct CabinetProcessor<P> {
    source: P,
    parameters: Arc<CabinetParameters>,
    mailbox: Arc<SwapMailbox<Option<CabinetImpulse>>>,
    active: Option<CabinetImpulse>,
    wet_mix: f32,
    low_mix: f32,
    high_mix: f32,
    low_frequency: f32,
    high_frequency: f32,
    gain: f32,
    sample_rate: f32,
    cuts: [CutFilters; 2],
    /// The previous IR while it crossfades out; then handed back for the
    /// control thread to drop.
    outgoing: Option<CabinetImpulse>,
    /// Crossfade progress 0..1 (1 = new IR only).
    crossfade: f32,
}

impl<P> CabinetProcessor<P> {
    pub fn new(
        source: P,
        active: Option<CabinetImpulse>,
        parameters: Arc<CabinetParameters>,
        mailbox: Arc<SwapMailbox<Option<CabinetImpulse>>>,
        sample_rate: u32,
    ) -> Self {
        let wet_mix = if active.is_some() && parameters.enabled.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let gain = 10.0_f32.powf(parameters.gain_db.load() / 20.0);
        Self {
            source,
            parameters: Arc::clone(&parameters),
            mailbox,
            active,
            wet_mix,
            low_mix: if parameters.low_cut_hz.load() > 0.0 {
                1.0
            } else {
                0.0
            },
            high_mix: if parameters.high_cut_hz.load() > 0.0 {
                1.0
            } else {
                0.0
            },
            low_frequency: if parameters.low_cut_hz.load() > 0.0 {
                parameters.low_cut_hz.load().clamp(20.0, 500.0)
            } else {
                80.0
            },
            high_frequency: if parameters.high_cut_hz.load() > 0.0 {
                parameters.high_cut_hz.load().clamp(1_000.0, 20_000.0)
            } else {
                8_000.0
            },
            gain,
            sample_rate: sample_rate as f32,
            cuts: [CutFilters::default(); 2],
            outgoing: None,
            crossfade: 1.0,
        }
    }

    fn reset_history(&mut self) {
        self.cuts = [CutFilters::default(); 2];
    }

    /// Per block, before processing. A replacement IR crossfades directly
    /// from the playing one; clearing the IR (or installing one while the cab
    /// is inaudible) still fades through dry, since there is nothing to blend.
    fn install_pending(&mut self) {
        if self.crossfade >= 1.0 {
            if let Some(old) = self.outgoing.take() {
                if let Err(Some(old)) = self.mailbox.retire(Some(old)) {
                    self.outgoing = Some(old); // mailbox busy: retry next block
                }
            }
        }
        if self.wet_mix < 0.001 {
            if self.outgoing.is_none() && self.mailbox.swap_pending(&mut self.active) {
                self.reset_history();
            }
            return;
        }
        if self.outgoing.is_none() && self.active.is_some() {
            if let Some(Some(next)) = self.mailbox.take_pending_if(|ir| ir.is_some()) {
                self.outgoing = std::mem::replace(&mut self.active, Some(next));
                self.crossfade = 0.0;
            }
        }
    }

    /// Fade the wet signal to dry only while clearing the IR (no IR to
    /// crossfade to) or while disabled.
    fn wet_target(&self) -> f32 {
        let clearing = self.mailbox.pending_matches(|ir| ir.is_none());
        if self.parameters.enabled.load(Ordering::Relaxed) && !clearing {
            1.0
        } else {
            0.0
        }
    }

    /// Glides each enabled cut toward its knob frequency (a disabled cut
    /// keeps its last frequency while it fades out), then derives this
    /// block's coefficients.
    fn cut_settings(&mut self, frames: usize) -> CutSettings {
        let glide = 1.0 - (-(frames as f32) / (CUT_GLIDE_SECONDS * self.sample_rate)).exp();
        let low_hz = self.parameters.low_cut_hz.load();
        let high_hz = self.parameters.high_cut_hz.load();
        if low_hz > 0.0 {
            let target = low_hz.clamp(20.0, 500.0);
            self.low_frequency += (target - self.low_frequency) * glide;
        }
        if high_hz > 0.0 {
            let target = high_hz.clamp(1_000.0, 20_000.0);
            self.high_frequency += (target - self.high_frequency) * glide;
        }
        CutSettings {
            low: SvfCoefficients::new(self.low_frequency, self.sample_rate),
            high: SvfCoefficients::new(self.high_frequency, self.sample_rate),
        }
    }

    fn cut_targets(&self) -> (f32, f32) {
        let on = |hz: f32| if hz > 0.0 { 1.0 } else { 0.0 };
        (
            on(self.parameters.low_cut_hz.load()),
            on(self.parameters.high_cut_hz.load()),
        )
    }
}

fn move_mix(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

impl<P: MonoProcessor> MonoProcessor for CabinetProcessor<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.source.process(samples); // NAM head is intentionally mono.
        self.install_pending();
        let target = self.wet_target();
        let crossfade_step = 1.0 / (IR_CROSSFADE_SECONDS * self.sample_rate);
        let smoothing = 1.0 - (-1.0 / (0.005 * self.sample_rate)).exp();
        let gain = 10.0_f32.powf(self.parameters.gain_db.load().clamp(-24.0, 12.0) / 20.0);
        let (low_target, high_target) = self.cut_targets();
        let step = 1.0 / (0.005 * self.sample_rate);
        let settings = self.cut_settings(samples.len());
        for (dry, frame) in samples.iter().zip(output) {
            let Some(ir) = &mut self.active else {
                *frame = [*dry; 2];
                continue;
            };
            let [mut left, mut right] = ir.tick_stereo(*dry);
            if let Some(old) = self.outgoing.as_mut().filter(|_| self.crossfade < 1.0) {
                let [old_left, old_right] = old.tick_stereo(*dry);
                self.crossfade = (self.crossfade + crossfade_step).min(1.0);
                let x = crate::dsp::smooth_ramp(self.crossfade);
                left = old_left + (left - old_left) * x;
                right = old_right + (right - old_right) * x;
            }
            self.wet_mix += (target - self.wet_mix) * smoothing;
            self.gain += (gain - self.gain) * smoothing;
            self.low_mix = move_mix(self.low_mix, low_target, step);
            self.high_mix = move_mix(self.high_mix, high_target, step);
            let left_out = self.cuts[0].tick(left, settings, self.low_mix, self.high_mix);
            let right_out = self.cuts[1].tick(right, settings, self.low_mix, self.high_mix);
            *frame = [
                *dry + (left_out * self.gain - *dry) * self.wet_mix,
                *dry + (right_out * self.gain - *dry) * self.wet_mix,
            ];
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.source.process(samples);
        self.install_pending();
        let target_mix = self.wet_target();
        if self.active.is_none() {
            return;
        }
        let crossfade_step = 1.0 / (IR_CROSSFADE_SECONDS * self.sample_rate);
        let smoothing = 1.0 - (-1.0 / (0.005 * self.sample_rate)).exp();
        let gain = 10.0_f32.powf(self.parameters.gain_db.load().clamp(-24.0, 12.0) / 20.0);
        let (low_target, high_target) = self.cut_targets();
        let step = 1.0 / (0.005 * self.sample_rate);
        let settings = self.cut_settings(samples.len());
        let impulse = self.active.as_mut().expect("checked above");
        for sample in samples {
            let dry = *sample;
            let mut wet = impulse.tick_mono(dry);
            if let Some(old) = self.outgoing.as_mut().filter(|_| self.crossfade < 1.0) {
                let old_wet = old.tick_mono(dry);
                self.crossfade = (self.crossfade + crossfade_step).min(1.0);
                wet = old_wet + (wet - old_wet) * crate::dsp::smooth_ramp(self.crossfade);
            }
            self.wet_mix += (target_mix - self.wet_mix) * smoothing;
            self.gain += (gain - self.gain) * smoothing;
            self.low_mix = move_mix(self.low_mix, low_target, step);
            self.high_mix = move_mix(self.high_mix, high_target, step);
            let shaped = self.cuts[0].tick(wet, settings, self.low_mix, self.high_mix);
            *sample = dry + (shaped * self.gain - dry) * self.wet_mix;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    #[test]
    fn cabinet_convolves_an_impulse_without_latency() {
        let impulse = CabinetImpulse::from_samples(&[1.0, 0.5]);
        let params = Arc::new(CabinetParameters::new(true, 0.0, 0.0, 0.0));
        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor =
            CabinetProcessor::new(Passthrough, Some(impulse), params, mailbox, 48_000);
        let mut samples = [1.0, 0.0, 0.0];
        processor.process(&mut samples);
        assert!(samples[0] > 0.9);
        assert!(samples[1] > 0.45);
    }

    #[test]
    fn flat_cuts_preserve_convolution_in_mono_and_stereo() {
        let params = Arc::new(CabinetParameters::new(true, 0.0, 0.0, 0.0));
        let make = || {
            CabinetProcessor::new(
                Passthrough,
                Some(CabinetImpulse::from_samples(&[1.0, 0.5])),
                Arc::clone(&params),
                Arc::new(SwapMailbox::default()),
                48_000,
            )
        };
        let mut mono = make();
        let mut samples = [1.0, 0.0, 0.0];
        mono.process(&mut samples);
        assert_eq!(samples, [1.0, 0.5, 0.0]);
        let mut stereo = make();
        let mut samples = [1.0, 0.0, 0.0];
        let mut frames = [[0.0; 2]; 3];
        stereo.process_stereo(&mut samples, &mut frames);
        assert_eq!(frames, [[1.0; 2], [0.5; 2], [0.0; 2]]);
    }

    #[test]
    fn cuts_switch_independently_and_settle_to_exact_bypass() {
        let params = Arc::new(CabinetParameters::new(true, 0.0, 0.0, 0.0));
        let mut processor = CabinetProcessor::new(
            Passthrough,
            Some(CabinetImpulse::from_samples(&[1.0])),
            Arc::clone(&params),
            Arc::new(SwapMailbox::default()),
            48_000,
        );
        for (low, high) in [(500.0, 0.0), (0.0, 1000.0), (0.0, 0.0)] {
            params.update(true, 0.0, low, high);
            let mut signal = [1.0; 1000];
            processor.process(&mut signal);
            assert!(signal.iter().all(|x| x.is_finite()));
            assert!((signal[0] - 1.0).abs() < 0.02 || low == 0.0);
            if low == 0.0 && high == 0.0 {
                assert_eq!(signal[999], 1.0);
            }
            if low > 0.0 {
                assert!(signal[999].abs() < 0.01);
            }
        }
    }

    #[test]
    fn sine_response_and_stereo_channel_balance() {
        fn measure(freq: f32, low: f32, high: f32) -> [f32; 2] {
            let mut impulse = CabinetImpulse::from_samples(&[1.0]);
            impulse.right = Some(crate::convolution::Convolver::new(&[0.35]));
            let params = Arc::new(CabinetParameters::new(true, 0.0, low, high));
            let mut p = CabinetProcessor::new(
                Passthrough,
                Some(impulse),
                params,
                Arc::new(SwapMailbox::default()),
                48_000,
            );
            let mut samples: Vec<f32> = (0..8192)
                .map(|n| (std::f32::consts::TAU * freq * n as f32 / 48_000.0).sin())
                .collect();
            let mut out = vec![[0.0; 2]; samples.len()];
            p.process_stereo(&mut samples, &mut out);
            let mut rms = [0.0; 2];
            for frame in &out[4096..] {
                for c in 0..2 {
                    rms[c] += frame[c] * frame[c];
                }
            }
            for x in &mut rms {
                *x = (*x / 4096.0).sqrt();
            }
            rms
        }
        for (frequency, low, high) in [(60.0, 500.0, 0.0), (10_000.0, 0.0, 1_000.0)] {
            let flat = measure(frequency, 0.0, 0.0);
            let cut = measure(frequency, low, high);
            assert!((flat[1] / flat[0] - 0.35).abs() < 0.001);
            assert!((cut[1] / cut[0] - 0.35).abs() < 0.001);
            assert!(cut[0] < flat[0] * 0.2 && cut[1] < flat[1] * 0.2);
        }
    }

    #[test]
    fn switching_off_retains_frequency_during_fade() {
        for (low, high, freq) in [(500.0, 0.0, 100.0), (0.0, 1000.0, 5000.0)] {
            let params = Arc::new(CabinetParameters::new(true, 0.0, low, high));
            let reference_params = Arc::new(CabinetParameters::new(true, 0.0, low, high));
            let make = |params| {
                CabinetProcessor::new(
                    Passthrough,
                    Some(CabinetImpulse::from_samples(&[1.0])),
                    params,
                    Arc::new(SwapMailbox::default()),
                    48_000,
                )
            };
            let mut p = make(Arc::clone(&params));
            let mut reference = make(reference_params);
            let mut warm: Vec<f32> = (0..4000)
                .map(|n| (std::f32::consts::TAU * freq * n as f32 / 48_000.0).sin())
                .collect();
            let mut warm_reference = warm.clone();
            p.process(&mut warm);
            reference.process(&mut warm_reference);
            params.update(true, 0.0, 0.0, 0.0);
            let mut next = [(std::f32::consts::TAU * freq * 4000.0 / 48_000.0).sin()];
            let mut expected = next;
            p.process(&mut next);
            reference.process(&mut expected);
            assert!((next[0] - expected[0]).abs() < 0.005);
            assert_eq!(p.low_frequency, if low > 0.0 { low } else { 80.0 });
            assert_eq!(p.high_frequency, if high > 0.0 { high } else { 8000.0 });
        }
    }

    #[test]
    fn disabled_cabinet_is_transparent() {
        let impulse = CabinetImpulse::from_samples(&[0.0]);
        let params = Arc::new(CabinetParameters::new(false, 0.0, 20.0, 20_000.0));
        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor =
            CabinetProcessor::new(Passthrough, Some(impulse), params, mailbox, 48_000);
        let mut samples = [0.25, -0.5];
        processor.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5]);
    }

    fn flat_processor(low: f32, high: f32) -> CabinetProcessor<Passthrough> {
        CabinetProcessor::new(
            Passthrough,
            Some(CabinetImpulse::from_samples(&[1.0])),
            Arc::new(CabinetParameters::new(true, 0.0, low, high)),
            Arc::new(SwapMailbox::default()),
            48_000,
        )
    }

    fn sine_gain_db(processor: &mut CabinetProcessor<Passthrough>, frequency: f32) -> f32 {
        let mut samples: Vec<f32> = (0..24_000)
            .map(|n| (std::f32::consts::TAU * frequency * n as f32 / 48_000.0).sin())
            .collect();
        processor.process(&mut samples);
        let tail = &samples[12_000..];
        let rms = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        20.0 * (rms * std::f32::consts::SQRT_2).log10()
    }

    #[test]
    fn cuts_are_12_db_per_octave_butterworth() {
        // One octave past a 2nd-order Butterworth corner: -12.3 dB (a
        // first-order cut gave only -7 dB). At the corner: -3 dB.
        let mut low = flat_processor(200.0, 0.0);
        assert!((sine_gain_db(&mut low, 100.0) + 12.3).abs() < 0.3);
        let mut low = flat_processor(200.0, 0.0);
        assert!((sine_gain_db(&mut low, 200.0) + 3.0).abs() < 0.2);
        // The digital filter reaches zero at Nyquist, so near the top of the
        // band it is a little steeper than the analog prototype: compare with
        // the exact bilinear-warped Butterworth response.
        let warped = |hz: f32| (std::f32::consts::PI * hz / 48_000.0).tan();
        let ratio = warped(8_000.0) / warped(4_000.0);
        let expected_db = -10.0 * (1.0 + ratio.powi(4)).log10();
        let mut high = flat_processor(0.0, 4_000.0);
        let measured = sine_gain_db(&mut high, 8_000.0);
        assert!(
            (measured - expected_db).abs() < 0.3,
            "{measured} vs {expected_db}"
        );
        let mut high = flat_processor(0.0, 4_000.0);
        assert!((sine_gain_db(&mut high, 4_000.0) + 3.0).abs() < 0.2);
        // Two octaves past: about -24 dB.
        let mut low = flat_processor(400.0, 0.0);
        assert!(sine_gain_db(&mut low, 100.0) < -23.0);
    }

    #[test]
    fn sweeping_a_cut_glides_without_clicks() {
        let params = Arc::new(CabinetParameters::new(true, 0.0, 20.0, 1_000.0));
        let mut processor = CabinetProcessor::new(
            Passthrough,
            Some(CabinetImpulse::from_samples(&[1.0])),
            Arc::clone(&params),
            Arc::new(SwapMailbox::default()),
            48_000,
        );
        let tone = |n: usize| 0.5 * (std::f32::consts::TAU * 440.0 * n as f32 / 48_000.0).sin();
        let mut previous = 0.0_f32;
        let mut worst_step = 0.0_f32;
        for block in 0..400 {
            // Slam both knobs between extremes every 20 blocks.
            if block % 20 == 0 {
                let up = block % 40 == 0;
                params.update(
                    true,
                    0.0,
                    if up { 500.0 } else { 20.0 },
                    if up { 20_000.0 } else { 1_000.0 },
                );
            }
            let mut samples: Vec<f32> = (0..64).map(|i| tone(block * 64 + i)).collect();
            processor.process(&mut samples);
            for sample in samples {
                assert!(sample.is_finite());
                worst_step = worst_step.max((sample - previous).abs());
                previous = sample;
            }
        }
        // A 440 Hz sine at 0.5 moves at most ~0.029 per sample.
        assert!(worst_step < 0.04, "{worst_step}");
    }

    fn temporary_wav(name: &str, channels: u16, frames: &[f32]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aero-cab-{name}-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::audio_file::write_float_wav(&path, channels, 48_000, frames.iter().copied())
            .unwrap();
        path
    }

    fn mono_processor(impulse: CabinetImpulse) -> CabinetProcessor<Passthrough> {
        CabinetProcessor::new(
            Passthrough,
            Some(impulse),
            Arc::new(CabinetParameters::new(true, 0.0, 0.0, 0.0)),
            Arc::new(SwapMailbox::default()),
            48_000,
        )
    }

    #[test]
    fn mono_plays_one_channel_of_a_stereo_ir_instead_of_averaging() {
        // Two "mics" 10 samples apart: averaging them would comb-filter.
        let mut frames = vec![0.0; 1_000 * 2];
        frames[0] = 0.8; // left at t=0
        frames[10 * 2 + 1] = 0.8; // right at t=10
        let path = temporary_wav("stereo", 2, &frames);
        let impulse = CabinetImpulse::load(&path, 48_000).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut processor = mono_processor(impulse);
        let mut samples = [0.0; 32];
        samples[0] = 1.0;
        processor.process(&mut samples);
        assert!(samples[0] > 0.0);
        assert_eq!(samples[10], 0.0, "right channel leaked into mono");

        // An effectively silent left channel falls back to the right one,
        // normalized on its own energy.
        let mut frames = vec![0.0; 1_000 * 2];
        frames[0] = 1e-4;
        frames[10 * 2 + 1] = 0.8;
        let path = temporary_wav("silent-left", 2, &frames);
        let impulse = CabinetImpulse::load(&path, 48_000).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut processor = mono_processor(impulse);
        let mut samples = [0.0; 32];
        samples[0] = 1.0;
        processor.process(&mut samples);
        assert!(samples[0].abs() < 1e-6);
        assert!(samples[10] > 0.0);
    }

    #[test]
    fn long_irs_lose_their_dc_but_keep_their_tone() {
        // 0.3 s decaying cab-like IR with a DC offset baked in.
        let frames = 14_400;
        let mut seed = 1u32;
        let taps: Vec<f32> = (0..frames)
            .map(|n| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                noise * (-(n as f32) / 1_500.0).exp() + 0.002
            })
            .collect();
        let path = temporary_wav("dc", 1, &taps);
        let impulse = CabinetImpulse::load(&path, 48_000).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut processor = mono_processor(impulse);
        // A constant input settles to the kernel's DC gain: now ~0.
        let mut constant = vec![1.0; frames * 2];
        processor.process(&mut constant);
        assert!(
            constant[frames * 2 - 1].abs() < 1e-4,
            "{}",
            constant[frames * 2 - 1]
        );
    }

    #[test]
    fn dc_removal_only_touches_the_lowest_bins() {
        let frames = (48_000.0 * DC_REMOVAL_MIN_SECONDS) as usize;
        let original: Vec<f32> = (0..frames)
            .map(|n| (-(n as f32) / 400.0).exp() * (n as f32 * 0.7).sin() + 0.003)
            .collect();
        let mut corrected = original.clone();
        remove_dc(&mut corrected, 1);
        let magnitude = |taps: &[f32], hz: f64| {
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            for (n, tap) in taps.iter().enumerate() {
                let phase = std::f64::consts::TAU * hz * n as f64 / 48_000.0;
                re += f64::from(*tap) * phase.cos();
                im -= f64::from(*tap) * phase.sin();
            }
            (re * re + im * im).sqrt()
        };
        assert!(magnitude(&corrected, 0.0) < 1e-4);
        // Even at the shortest corrected length, 100 Hz and up are untouched.
        for hz in [100.0, 1_000.0] {
            let change_db = 20.0 * (magnitude(&corrected, hz) / magnitude(&original, hz)).log10();
            assert!(change_db.abs() < 0.05, "{hz} Hz changed {change_db} dB");
        }
    }

    #[test]
    fn default_ir_length_keeps_half_a_second_of_tail() {
        assert_eq!(DEFAULT_IR_SECONDS, 0.5);
        assert_eq!(crate::quality::ir_seconds(1), 0.5);
        let mut taps = vec![0.0; 30_000]; // 0.625 s
        taps[0] = 0.5;
        taps[22_000] = 0.25; // 0.458 s: was cut by the old 0.2 s default
        let path = temporary_wav("tail", 1, &taps);
        let impulse = CabinetImpulse::load(&path, 48_000).unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut processor = mono_processor(impulse);
        let mut samples = vec![0.0; 22_100];
        samples[0] = 1.0;
        processor.process(&mut samples);
        assert!(samples[22_000].abs() > samples[0].abs() * 0.4);
    }

    fn impulse_response(path: &std::path::Path, frames: usize) -> Vec<[f32; 2]> {
        let mut node = CabinetNode::load(path, 48_000, 0.5).unwrap();
        (0..frames)
            .map(|i| {
                crate::node_rig::StereoNode::tick(
                    &mut node,
                    if i == 0 { [1.0; 2] } else { [0.0; 2] },
                )
            })
            .collect()
    }

    fn peak_index(response: &[[f32; 2]], channel: usize) -> usize {
        (0..response.len())
            .max_by(|&a, &b| {
                response[a][channel]
                    .abs()
                    .total_cmp(&response[b][channel].abs())
            })
            .unwrap()
    }

    #[test]
    fn leading_silence_is_trimmed_but_the_onset_keeps_a_margin() {
        // 2000 frames of near-silence (-90 dB) before the cab's first sound.
        let mut taps = vec![0.0; 3_000];
        for (n, tap) in taps.iter_mut().take(2_000).enumerate() {
            *tap = if n % 2 == 0 { 2.5e-5 } else { -2.5e-5 };
        }
        taps[2_000] = 0.8;
        taps[2_001] = -0.3;
        let path = temporary_wav("predelay", 1, &taps);
        let response = impulse_response(&path, 200);
        std::fs::remove_file(&path).unwrap();
        // The onset now arrives after only the 0.5 ms (24 frame) margin.
        assert_eq!(peak_index(&response, 0), 24);
    }

    #[test]
    fn stereo_trim_keeps_the_two_mics_aligned() {
        let mut frames = vec![0.0; 2_000 * 2];
        frames[500 * 2] = 0.8; // left mic onset
        frames[530 * 2 + 1] = 0.6; // right mic, 30 frames later
        let path = temporary_wav("stereo-trim", 2, &frames);
        let response = impulse_response(&path, 200);
        std::fs::remove_file(&path).unwrap();
        let left = peak_index(&response, 0);
        assert_eq!(left, 24);
        assert_eq!(peak_index(&response, 1), left + 30);
    }

    #[test]
    fn audible_pre_roll_and_onset_at_zero_are_kept() {
        // Pre-roll at -40 dB is real signal (above the -60 dB threshold).
        let mut taps = vec![0.0; 1_000];
        taps[100] = 0.008;
        taps[400] = 0.8;
        assert_eq!(leading_silence_frames(&taps, 1, 48_000), 100 - 24);
        let mut taps = vec![0.0; 1_000];
        taps[0] = 0.8;
        assert_eq!(leading_silence_frames(&taps, 1, 48_000), 0);
        assert_eq!(leading_silence_frames(&[0.0; 8], 1, 48_000), 0);
    }

    fn swapping_cab(
        gain: f32,
    ) -> (
        CabinetProcessor<Passthrough>,
        Arc<SwapMailbox<Option<CabinetImpulse>>>,
    ) {
        let mailbox = Arc::new(SwapMailbox::default());
        let processor = CabinetProcessor::new(
            Passthrough,
            Some(CabinetImpulse::from_samples(&[gain])),
            Arc::new(CabinetParameters::new(true, 0.0, 0.0, 0.0)),
            Arc::clone(&mailbox),
            48_000,
        );
        (processor, mailbox)
    }

    fn run(processor: &mut CabinetProcessor<Passthrough>, frames: usize) -> Vec<f32> {
        // Constant "amp" signal of 1.0: dry is 1.0, each cab is its gain.
        let mut out = Vec::with_capacity(frames);
        for _ in 0..frames / 64 {
            let mut block = [1.0; 64];
            processor.process(&mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    #[test]
    fn a_new_ir_crossfades_from_the_old_one_without_exposing_the_dry_amp() {
        let (mut processor, mailbox) = swapping_cab(0.5);
        run(&mut processor, 640);
        mailbox.stage(Some(CabinetImpulse::from_samples(&[0.25])));
        let out = run(&mut processor, 4_800);
        // Old behaviour faded toward dry (1.0) first; now the output only
        // moves between the two cab levels, monotonically.
        assert!(out.iter().all(|x| (0.25 - 1e-6..=0.5 + 1e-6).contains(x)));
        assert!(out.windows(2).all(|w| w[1] <= w[0] + 1e-6));
        // The 30 ms crossfade is complete by 1440 samples (+1 block).
        assert!((out[1_600] - 0.25).abs() < 1e-6, "{}", out[1_600]);
        assert!(
            out[600] < 0.5 && out[600] > 0.25,
            "mid-crossfade {}",
            out[600]
        );
        // The old IR went back to the mailbox, not dropped on this thread.
        assert!(matches!(mailbox.take_retired(), Some(Some(_))));
    }

    #[test]
    fn clearing_the_ir_still_fades_to_dry() {
        let (mut processor, mailbox) = swapping_cab(0.5);
        run(&mut processor, 640);
        mailbox.stage(None);
        let out = run(&mut processor, 9_600);
        assert!((out[9_599] - 1.0).abs() < 1e-6);
        assert!(processor.active.is_none());
    }

    #[test]
    fn an_ir_staged_mid_crossfade_follows_once_it_finishes() {
        let (mut processor, mailbox) = swapping_cab(0.5);
        run(&mut processor, 640);
        mailbox.stage(Some(CabinetImpulse::from_samples(&[0.25])));
        run(&mut processor, 640); // part-way through the first crossfade
        mailbox.stage(Some(CabinetImpulse::from_samples(&[0.125])));
        let out = run(&mut processor, 9_600);
        assert!(out.iter().all(|x| (0.125 - 1e-6..=0.5 + 1e-6).contains(x)));
        assert!((out[9_599] - 0.125).abs() < 1e-6);
        assert!(processor.outgoing.is_none());
        // Both replaced IRs are waiting for the control thread.
        assert!(mailbox.take_retired().is_some());
    }
}
