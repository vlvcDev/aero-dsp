//! Cabinet impulse-response loading and real-time processing.
//!
//! Loading, decoding, resampling, and trimming happen on the control thread.
//! The audio callback only reads a preallocated impulse and fixed history
//! buffer, so selecting a new cabinet never performs file I/O in the signal
//! path. The initial implementation intentionally limits IRs to the first
//! 1,024 samples after their onset: that keeps a single-cab V0 predictable on
//! a Raspberry Pi while preserving the high-impact cabinet response.

use crate::dsp::{MonoProcessor, SwapMailbox};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub const MAX_IR_SAMPLES: usize = 1_024;

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
    samples: Vec<f32>,
}

impl CabinetImpulse {
    pub fn load(path: &Path, destination_rate: u32) -> Result<Self, String> {
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
        let mut reader = hound::WavReader::open(path)
            .map_err(|error| format!("could not open cabinet IR {}: {error}", path.display()))?;
        let spec = reader.spec();
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err("cabinet IR WAV has an invalid channel count or sample rate".to_owned());
        }
        let interleaved = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .map(|sample| sample.map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
                .samples::<i16>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 / i16::MAX as f32)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int if spec.bits_per_sample <= 32 => {
                let scale = ((1_i64 << (spec.bits_per_sample - 1)) - 1) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| {
                        sample
                            .map(|value| value as f32 / scale)
                            .map_err(|error| error.to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            _ => return Err("cabinet IR must use 16-, 24-, or 32-bit PCM/float audio".to_owned()),
        };
        let channels = usize::from(spec.channels);
        let mono = interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect::<Vec<_>>();
        let mut samples = resample_linear(&mono, spec.sample_rate, destination_rate);
        let onset = samples
            .iter()
            .position(|sample| sample.abs() >= 0.0005)
            .unwrap_or(0);
        samples.drain(..onset);
        samples.truncate(MAX_IR_SAMPLES);
        if samples.is_empty() {
            return Err("cabinet IR contains no audible samples".to_owned());
        }
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if peak > 1.0 {
            let scale = 0.9 / peak;
            for sample in &mut samples {
                *sample *= scale;
            }
        }
        Ok(Self { samples })
    }
}

fn resample_linear(input: &[f32], source_rate: u32, destination_rate: u32) -> Vec<f32> {
    if source_rate == destination_rate || input.len() < 2 {
        return input.to_vec();
    }
    let output_len = input.len() * destination_rate as usize / source_rate as usize;
    let ratio = source_rate as f64 / destination_rate as f64;
    (0..output_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let lower = position.floor() as usize;
            let upper = (lower + 1).min(input.len() - 1);
            let fraction = (position - lower as f64) as f32;
            input[lower] + (input[upper] - input[lower]) * fraction
        })
        .collect()
}

/// A single, zero-latency convolution cabinet. Swapping a loaded IR is safe:
/// the old allocation is retired on the control thread by `SwapMailbox`.
pub struct CabinetProcessor<P> {
    source: P,
    parameters: Arc<CabinetParameters>,
    mailbox: Arc<SwapMailbox<Option<CabinetImpulse>>>,
    active: Option<CabinetImpulse>,
    history: Vec<f32>,
    history_write: usize,
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
        Self {
            source,
            parameters,
            mailbox,
            active,
            history: vec![0.0; MAX_IR_SAMPLES],
            history_write: 0,
            sample_rate: sample_rate as f32,
            low_cut_state: 0.0,
            low_cut_input: 0.0,
            high_cut_state: 0.0,
        }
    }

    fn reset_history(&mut self) {
        self.history.fill(0.0);
        self.history_write = 0;
        self.low_cut_state = 0.0;
        self.low_cut_input = 0.0;
        self.high_cut_state = 0.0;
    }

    fn install_pending(&mut self) {
        if self.mailbox.swap_pending(&mut self.active) {
            self.reset_history();
        }
    }
}

impl<P: MonoProcessor> MonoProcessor for CabinetProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        self.source.process(samples);
        self.install_pending();
        if !self.parameters.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Some(impulse) = &self.active else {
            return;
        };
        let gain = 10.0_f32.powf(self.parameters.gain_db.load().clamp(-24.0, 12.0) / 20.0);
        let low_cut = self.parameters.low_cut_hz.load().clamp(20.0, 500.0);
        let high_cut = self.parameters.high_cut_hz.load().clamp(1_000.0, 20_000.0);
        let low_alpha = (-std::f32::consts::TAU * low_cut / self.sample_rate).exp();
        let high_alpha = 1.0 - (-std::f32::consts::TAU * high_cut / self.sample_rate).exp();
        for sample in samples {
            self.history[self.history_write] = *sample;
            let mut wet = 0.0;
            for (tap, coefficient) in impulse.samples.iter().enumerate() {
                let index = (self.history_write + self.history.len() - tap) % self.history.len();
                wet += self.history[index] * coefficient;
            }
            self.history_write = (self.history_write + 1) % self.history.len();
            // First-order high/low cut filters provide predictable final tone
            // shaping without needing a second IR render.
            let high_pass = low_alpha * (self.low_cut_state + wet - self.low_cut_input);
            self.low_cut_input = wet;
            self.low_cut_state = high_pass;
            self.high_cut_state += high_alpha * (high_pass - self.high_cut_state);
            *sample = self.high_cut_state * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    #[test]
    fn cabinet_convolves_an_impulse_without_latency() {
        let impulse = CabinetImpulse {
            samples: vec![1.0, 0.5],
        };
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
        let impulse = CabinetImpulse { samples: vec![0.0] };
        let params = Arc::new(CabinetParameters::new(false, 0.0, 20.0, 20_000.0));
        let mailbox = Arc::new(SwapMailbox::default());
        let mut processor =
            CabinetProcessor::new(Passthrough, Some(impulse), params, mailbox, 48_000);
        let mut samples = [0.25, -0.5];
        processor.process(&mut samples);
        assert_eq!(samples, [0.25, -0.5]);
    }
}
