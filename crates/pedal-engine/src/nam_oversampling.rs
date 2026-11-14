//! Oversampled NAM playback, following Tone3000's phase-interleaved design.
//!
//! A NAM WaveNet/ConvNet run at N x its native rate with every dilation scaled
//! by N is exactly N independent copies of the unscaled model, each fed every
//! Nth sample of the oversampled stream (dilated taps only ever connect samples
//! of the same phase; 1x1 convolutions, activations and the head act per
//! sample). So each phase instance runs the stock model at its native rate,
//! the receptive field stays the same in seconds, and the capture sounds the
//! same -- except that the harmonics its activations generate now land in the
//! widened band instead of folding back below Nyquist. The half-band
//! decimator then removes everything above the base Nyquist, including the
//! spectral images a dilated kernel produces there.
//!
//! LSTMs (and anything not known to be phase-safe) have recurrent state that
//! cannot be split by phase. Tone3000 feeds them the raw oversampled stream,
//! which shortens their time constants and changes the tone; here they simply
//! stay at 1x and `factor()` reports that.
use crate::oversampling::Oversampler;
use crate::rt_pool::RtWorkerPool;
use nam_bridge::{NamError, NamModel, NamQuality};
use std::path::Path;

/// Valid oversampling factors. Anything else is rejected at the control layer.
pub const FACTORS: [u8; 4] = [1, 2, 4, 8];

pub fn is_valid_factor(factor: u8) -> bool {
    FACTORS.contains(&factor)
}

/// Architectures whose dilated convolutions are phase-separable.
fn architecture_is_phase_safe(value: &serde_json::Value) -> bool {
    let Some(architecture) = value.get("architecture").and_then(|a| a.as_str()) else {
        return false;
    };
    match architecture {
        "WaveNet" | "ConvNet" | "Linear" => value
            .get("config")
            .is_none_or(|config| nested_models_are_phase_safe(config)),
        "SlimmableContainer" => value
            .get("config")
            .and_then(|config| config.get("submodels"))
            .and_then(|submodels| submodels.as_array())
            .is_some_and(|submodels| {
                !submodels.is_empty()
                    && submodels.iter().all(|submodel| {
                        submodel
                            .get("model")
                            .is_some_and(architecture_is_phase_safe)
                    })
            }),
        _ => false,
    }
}

/// A WaveNet may embed another DSP (e.g. a condition model); every embedded
/// model must be phase-safe too.
fn nested_models_are_phase_safe(config: &serde_json::Value) -> bool {
    match config {
        serde_json::Value::Object(map) => map.iter().all(|(key, value)| {
            if key == "weights" {
                return true;
            }
            if value.get("architecture").is_some() {
                architecture_is_phase_safe(value)
            } else {
                nested_models_are_phase_safe(value)
            }
        }),
        serde_json::Value::Array(items) => items.iter().all(nested_models_are_phase_safe),
        _ => true,
    }
}

/// Reads the `.nam` JSON to decide whether the model may be phase-interleaved.
/// Unreadable or unknown files are treated as not phase-safe (1x).
pub fn model_is_phase_safe(path: &Path) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|model| architecture_is_phase_safe(&model))
}

/// One phase instance with its own input/output buffer, so phases share no
/// data and can run on different threads.
struct Phase {
    model: NamModel,
    buffer: Vec<f32>,
    frames: usize,
    ok: bool,
}

impl Phase {
    fn new(model: NamModel, max_frames: usize) -> Self {
        Self {
            model,
            buffer: vec![0.0; max_frames],
            frames: 0,
            ok: true,
        }
    }

    fn run(&mut self) {
        self.ok = self.model.process_realtime(&mut self.buffer[..self.frames]);
    }
}

pub struct OversampledNam {
    /// One instance per phase; a single instance at 1x.
    phases: Vec<Phase>,
    oversampler: Option<Oversampler>,
    /// Runs phases in parallel; `None` runs them serially on the caller.
    pool: Option<&'static RtWorkerPool>,
    max_frames: usize,
    /// `max_frames * factor` oversampled samples.
    upsampled: Vec<f32>,
    faulted: bool,
}

impl OversampledNam {
    /// Not real-time safe: loads and prewarms `factor` model instances, and
    /// starts the shared DSP worker pool the first time oversampling is used.
    /// `requested_factor` must be one of [`FACTORS`]; models that are not
    /// phase-safe load at 1x.
    pub fn load(
        path: impl AsRef<Path>,
        sample_rate: f64,
        max_frames: usize,
        quality: NamQuality,
        alignment: bool,
        requested_factor: u8,
    ) -> Result<Self, NamError> {
        let path = path.as_ref();
        if !is_valid_factor(requested_factor) {
            return Err(NamError {
                status: 1,
                message: format!("unsupported oversampling factor {requested_factor}"),
            });
        }
        let factor = if requested_factor > 1 && model_is_phase_safe(path) {
            usize::from(requested_factor)
        } else {
            1
        };
        let mut phases = Vec::with_capacity(factor);
        // The dry/wet alignment probe measures the model itself, so it is only
        // needed once; every phase instance runs the identical network.
        let first = if alignment {
            NamModel::load_with_alignment(path, sample_rate, max_frames, quality)?
        } else {
            NamModel::load_with_quality(path, sample_rate, max_frames, quality)?
        };
        phases.push(Phase::new(first, max_frames));
        for _ in 1..factor {
            let model = NamModel::load_with_quality(path, sample_rate, max_frames, quality)?;
            phases.push(Phase::new(model, max_frames));
        }
        let pool = (factor > 1).then(|| {
            crate::rt_pool::ensure_global_started();
            crate::rt_pool::global()
        });
        Ok(Self {
            phases,
            oversampler: (factor > 1).then(|| Oversampler::new(factor, 1, max_frames)),
            pool,
            max_frames,
            upsampled: vec![0.0; max_frames * factor],
            faulted: false,
        })
    }

    /// Run phases on `pool`, or serially on the calling thread with `None`.
    /// Output is bit-identical either way.
    pub fn with_pool(mut self, pool: Option<&'static RtWorkerPool>) -> Self {
        if self.phases.len() > 1 {
            self.pool = pool;
        }
        self
    }

    /// Effective factor (1 when the model is not phase-safe).
    pub fn factor(&self) -> usize {
        self.phases.len()
    }

    pub fn alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        self.phases[0].model.alignment()
    }

    pub fn metadata(&self) -> nam_bridge::ModelMetadata {
        self.phases[0].model.metadata()
    }

    pub fn expected_sample_rate(&self) -> f64 {
        self.phases[0].model.expected_sample_rate()
    }

    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// Allocation-free and lock-free. On failure the block is silenced and
    /// the processor stays faulted (fails closed), like
    /// `NamModel::process_realtime`.
    pub fn process_realtime(&mut self, samples: &mut [f32]) -> bool {
        if self.faulted {
            samples.fill(0.0);
            return false;
        }
        let ok = if self.oversampler.is_none() {
            self.phases[0].model.process_realtime(samples)
        } else {
            samples
                .chunks_mut(self.max_frames.max(1))
                .all(|block| self.process_oversampled(block))
        };
        if !ok {
            self.faulted = true;
            samples.fill(0.0);
        }
        ok
    }

    /// `block.len() <= max_frames`. Each base frame becomes exactly `factor`
    /// oversampled frames, so phase `p` always holds samples `k*factor + p`.
    fn process_oversampled(&mut self, block: &mut [f32]) -> bool {
        let factor = self.phases.len();
        let frames = block.len();
        let upsampled = &mut self.upsampled[..frames * factor];
        let oversampler = self.oversampler.as_mut().expect("oversampled path");
        oversampler.upsample(0, block, upsampled);
        for (index, phase) in self.phases.iter_mut().enumerate() {
            phase.frames = frames;
            for (sample, source) in phase.buffer[..frames]
                .iter_mut()
                .zip(upsampled[index..].iter().step_by(factor))
            {
                *sample = *source;
            }
        }
        match self.pool {
            Some(pool) => pool.for_each(&mut self.phases, Phase::run),
            None => self.phases.iter_mut().for_each(Phase::run),
        }
        for (index, phase) in self.phases.iter().enumerate() {
            if !phase.ok {
                return false;
            }
            for (destination, sample) in upsampled[index..]
                .iter_mut()
                .step_by(factor)
                .zip(&phase.buffer[..frames])
            {
                *destination = *sample;
            }
        }
        oversampler.downsample(0, upsampled, block);
        true
    }
}

impl crate::dsp::MonoProcessor for OversampledNam {
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        self.alignment()
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        Some(self.metadata())
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.process_realtime(samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/NeuralAmpModelerCore/example_models")
            .join(name)
    }

    fn sine(frequency: f32, amplitude: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amplitude * (std::f32::consts::TAU * frequency * i as f32 / 48_000.0).sin())
            .collect()
    }

    #[test]
    fn phase_safety_follows_the_architecture() {
        for (name, safe) in [
            ("wavenet.nam", true),
            ("wavenet_a1_standard.nam", true),
            ("wavenet_a2_max.nam", true),
            ("slimmable_wavenet.nam", true),
            ("A2.nam", true),
            // Mixes an LSTM submodel with WaveNets: one unsafe submodel keeps 1x.
            ("slimmable_container.nam", false),
            ("lstm.nam", false),
        ] {
            assert_eq!(model_is_phase_safe(&example(name)), safe, "{name}");
        }
        assert!(!model_is_phase_safe(Path::new("/nonexistent.nam")));
    }

    #[test]
    fn lstm_stays_at_1x_and_invalid_factors_are_rejected() {
        let lstm = OversampledNam::load(
            example("lstm.nam"),
            48_000.0,
            64,
            NamQuality::Full,
            false,
            4,
        )
        .unwrap();
        assert_eq!(lstm.factor(), 1);
        assert!(
            OversampledNam::load(
                example("wavenet.nam"),
                48_000.0,
                64,
                NamQuality::Full,
                false,
                3
            )
            .is_err()
        );
    }

    #[test]
    fn factor_one_is_bit_identical_to_the_plain_model() {
        let mut plain = NamModel::load(example("wavenet.nam"), 48_000.0, 64).unwrap();
        let mut wrapped = OversampledNam::load(
            example("wavenet.nam"),
            48_000.0,
            64,
            NamQuality::Full,
            false,
            1,
        )
        .unwrap();
        let mut a = sine(220.0, 0.3, 4096);
        let mut b = a.clone();
        for (x, y) in a.chunks_mut(64).zip(b.chunks_mut(64)) {
            plain.process(x).unwrap();
            assert!(wrapped.process_realtime(y));
        }
        assert_eq!(a, b);
    }

    /// Phase interleaving must equal running the stock model on each
    /// polyphase stream of the oversampled signal by hand.
    #[test]
    fn interleaved_phases_match_a_manual_phase_reference() {
        let factor = 2;
        let input = sine(330.0, 0.4, 2048);
        let mut wrapped = OversampledNam::load(
            example("wavenet.nam"),
            48_000.0,
            64,
            NamQuality::Full,
            false,
            factor as u8,
        )
        .unwrap();
        let mut actual = input.clone();
        for block in actual.chunks_mut(64) {
            assert!(wrapped.process_realtime(block));
        }

        let mut up = Oversampler::new(factor, 1, input.len());
        let mut down = Oversampler::new(factor, 1, input.len());
        let mut upsampled = vec![0.0; input.len() * factor];
        up.upsample(0, &input, &mut upsampled);
        for phase in 0..factor {
            let mut model = NamModel::load(example("wavenet.nam"), 48_000.0, 64).unwrap();
            let mut stream: Vec<f32> = upsampled[phase..].iter().step_by(factor).copied().collect();
            for block in stream.chunks_mut(64) {
                model.process(block).unwrap();
            }
            for (destination, sample) in upsampled[phase..].iter_mut().step_by(factor).zip(stream) {
                *destination = sample;
            }
        }
        let mut expected = vec![0.0; input.len()];
        down.downsample(0, &upsampled, &mut expected);
        assert_eq!(actual, expected);
    }

    #[test]
    fn oversampling_keeps_the_capture_tone_and_level() {
        // A low note's fundamental is far below any aliasing, so the
        // oversampled model must reproduce the 1x model's output closely.
        let input = sine(110.0, 0.2, 48_000);
        let render = |factor: u8| {
            let mut model = OversampledNam::load(
                example("wavenet.nam"),
                48_000.0,
                128,
                NamQuality::Full,
                false,
                factor,
            )
            .unwrap();
            let mut output = input.clone();
            for block in output.chunks_mut(128) {
                assert!(model.process_realtime(block));
            }
            output
        };
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        let base = render(1);
        for factor in [2, 4] {
            let oversampled = render(factor);
            let ratio_db = 20.0 * (rms(&oversampled[24_000..]) / rms(&base[24_000..])).log10();
            assert!(
                ratio_db.abs() < 0.5,
                "{factor}x level changed {ratio_db} dB"
            );
        }
    }

    #[test]
    fn oversized_and_uneven_blocks_are_exact() {
        let input = sine(440.0, 0.3, 3000);
        let render = |sizes: &[usize]| {
            let mut model = OversampledNam::load(
                example("wavenet.nam"),
                48_000.0,
                64,
                NamQuality::Full,
                false,
                2,
            )
            .unwrap();
            let mut output = input.clone();
            let mut start = 0;
            let mut index = 0;
            while start < output.len() {
                let end = (start + sizes[index % sizes.len()]).min(output.len());
                assert!(model.process_realtime(&mut output[start..end]));
                start = end;
                index += 1;
            }
            output
        };
        assert_eq!(render(&[64]), render(&[37, 1, 200, 64, 5]));
    }

    #[test]
    fn parallel_phases_are_bit_identical_to_serial() {
        crate::rt_pool::ensure_global_started();
        let input = sine(523.0, 0.5, 9600);
        for factor in [2, 4, 8] {
            let load = || {
                OversampledNam::load(
                    example("wavenet.nam"),
                    48_000.0,
                    128,
                    NamQuality::Full,
                    false,
                    factor,
                )
                .unwrap()
            };
            let mut parallel = load().with_pool(Some(crate::rt_pool::global()));
            let mut serial = load().with_pool(None);
            let mut a = input.clone();
            let mut b = input.clone();
            for (x, y) in a.chunks_mut(128).zip(b.chunks_mut(128)) {
                assert!(parallel.process_realtime(x));
                assert!(serial.process_realtime(y));
            }
            assert_eq!(a, b, "{factor}x");
        }
    }
}
