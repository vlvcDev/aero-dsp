//! Cabinet impulse-response loading and real-time processing.
//!
//! Loading, decoding, resampling, and trimming happen on the control thread.
//! Prepared partitioned convolution retains 200 ms by default without added
//! algorithmic latency. IR onset/phase is preserved; truncation uses a taper.

use crate::dsp::{MonoProcessor, SwapMailbox};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const DEFAULT_IR_SECONDS: f32 = 0.2;

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
        if peak > 1.0 {
            for sample in &mut samples {
                *sample *= 0.9 / peak;
            }
        }
        let left: Vec<_> = samples.chunks_exact(audio.channels).map(|f| f[0]).collect();
        let mut impulse = Self::from_samples(&left);
        if audio.channels == 2 {
            let right: Vec<_> = samples.as_chunks::<2>().0.iter().map(|f| f[1]).collect();
            impulse.right = Some(crate::convolution::Convolver::new(&right));
        } else if stereo_input {
            impulse.right = Some(crate::convolution::Convolver::new(&left));
        }
        Ok(impulse)
    }

    fn from_samples(samples: &[f32]) -> Self {
        Self {
            convolution: crate::convolution::Convolver::new(samples),
            right: None,
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
        let mut left = crate::convolution::Convolver::new(&taps);
        let mut right = crate::convolution::Convolver::new(&taps);
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

/// A single, zero-latency convolution cabinet. Swapping a loaded IR is safe:
/// the old allocation is retired on the control thread by `SwapMailbox`.
pub struct CabinetProcessor<P> {
    right_filters: [f32; 3],
    source: P,
    parameters: Arc<CabinetParameters>,
    mailbox: Arc<SwapMailbox<Option<CabinetImpulse>>>,
    active: Option<CabinetImpulse>,
    wet_mix: f32,
    gain: f32,
    sample_rate: f32,
    low_cut_state: f32,
    low_cut_input: f32,
    high_cut_state: f32,
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
            right_filters: [0.0; 3],
            parameters,
            mailbox,
            active,
            wet_mix,
            gain,
            sample_rate: sample_rate as f32,
            low_cut_state: 0.0,
            low_cut_input: 0.0,
            high_cut_state: 0.0,
        }
    }

    fn reset_history(&mut self) {
        self.right_filters = [0.0; 3];
        self.low_cut_state = 0.0;
        self.low_cut_input = 0.0;
        self.high_cut_state = 0.0;
    }

    fn install_pending(&mut self) {
        if self.wet_mix < 0.001 && self.mailbox.swap_pending(&mut self.active) {
            self.reset_history();
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for CabinetProcessor<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.source.process(samples); // NAM head is intentionally mono.
        self.install_pending();
        let target =
            if self.parameters.enabled.load(Ordering::Relaxed) && !self.mailbox.has_pending() {
                1.0
            } else {
                0.0
            };
        let smoothing = 1.0 - (-1.0 / (0.005 * self.sample_rate)).exp();
        let gain = 10.0_f32.powf(self.parameters.gain_db.load().clamp(-24.0, 12.0) / 20.0);
        let low = (-std::f32::consts::TAU * self.parameters.low_cut_hz.load().clamp(20.0, 500.0)
            / self.sample_rate)
            .exp();
        let high = 1.0
            - (-std::f32::consts::TAU * self.parameters.high_cut_hz.load().clamp(1000.0, 20000.0)
                / self.sample_rate)
                .exp();
        for (dry, frame) in samples.iter().zip(output) {
            let Some(ir) = &mut self.active else {
                *frame = [*dry; 2];
                continue;
            };
            let left = ir.convolution.tick(*dry);
            let right = ir.right.as_mut().map_or(left, |r| r.tick(*dry));
            self.wet_mix += (target - self.wet_mix) * smoothing;
            self.gain += (gain - self.gain) * smoothing;
            let hp = low * (self.low_cut_state + left - self.low_cut_input);
            self.low_cut_input = left;
            self.low_cut_state = hp;
            self.high_cut_state += high * (hp - self.high_cut_state);
            let hp = low * (self.right_filters[0] + right - self.right_filters[1]);
            self.right_filters[0] = hp;
            self.right_filters[1] = right;
            self.right_filters[2] += high * (hp - self.right_filters[2]);
            *frame = [
                *dry + (self.high_cut_state * self.gain - *dry) * self.wet_mix,
                *dry + (self.right_filters[2] * self.gain - *dry) * self.wet_mix,
            ];
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.source.process(samples);
        self.install_pending();
        let pending = self.mailbox.has_pending();
        let target_mix = if self.parameters.enabled.load(Ordering::Relaxed) && !pending {
            1.0
        } else {
            0.0
        };
        let Some(impulse) = &mut self.active else {
            return;
        };
        let smoothing = 1.0 - (-1.0 / (0.005 * self.sample_rate)).exp();
        let gain = 10.0_f32.powf(self.parameters.gain_db.load().clamp(-24.0, 12.0) / 20.0);
        let low_cut = self.parameters.low_cut_hz.load().clamp(20.0, 500.0);
        let high_cut = self.parameters.high_cut_hz.load().clamp(1_000.0, 20_000.0);
        let low_alpha = (-std::f32::consts::TAU * low_cut / self.sample_rate).exp();
        let high_alpha = 1.0 - (-std::f32::consts::TAU * high_cut / self.sample_rate).exp();
        for sample in samples {
            let dry = *sample;
            let left = impulse.convolution.tick(dry);
            let wet = impulse
                .right
                .as_mut()
                .map_or(left, |right| (left + right.tick(dry)) * 0.5);
            self.wet_mix += (target_mix - self.wet_mix) * smoothing;
            self.gain += (gain - self.gain) * smoothing;
            // First-order high/low cut filters provide predictable final tone
            // shaping without needing a second IR render.
            let high_pass = low_alpha * (self.low_cut_state + wet - self.low_cut_input);
            self.low_cut_input = wet;
            self.low_cut_state = high_pass;
            self.high_cut_state += high_alpha * (high_pass - self.high_cut_state);
            *sample = dry + (self.high_cut_state * self.gain - dry) * self.wet_mix;
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
        let params = Arc::new(CabinetParameters::new(true, 0.0, 20.0, 20_000.0));
        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor =
            CabinetProcessor::new(Passthrough, Some(impulse), params, mailbox, 48_000);
        let mut samples = [1.0, 0.0, 0.0];
        processor.process(&mut samples);
        assert!(samples[0] > 0.9);
        assert!(samples[1] > 0.45);
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
}
