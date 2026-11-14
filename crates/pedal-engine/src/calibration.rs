//! Interface-agnostic gain staging. Never infer a hardware gain-knob position.
use crate::{EngineState, dsp::MonoProcessor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

pub struct CalibrationParameters {
    enabled: AtomicBool,
    pedal_normalize: AtomicBool,
    amp_normalize: AtomicBool,
    suppress_pedal_normalize: AtomicBool,
    reference: AtomicU32,
    trim: AtomicU32,
    level: AtomicU32,
    pedal_mix: AtomicU32,
}
impl CalibrationParameters {
    pub fn new(s: &EngineState) -> Self {
        let p = Self {
            enabled: AtomicBool::new(false),
            pedal_normalize: AtomicBool::new(false),
            amp_normalize: AtomicBool::new(false),
            suppress_pedal_normalize: AtomicBool::new(false),
            reference: AtomicU32::new(0),
            trim: AtomicU32::new(0),
            level: AtomicU32::new(0),
            pedal_mix: AtomicU32::new(0),
        };
        p.update(s);
        p
    }
    pub fn update(&self, s: &EngineState) {
        self.pedal_normalize
            .store(s.pedal_normalize_enabled, Ordering::Relaxed);
        self.amp_normalize
            .store(s.amp_normalize_enabled, Ordering::Relaxed);
        self.reference
            .store(s.input_reference_dbu.to_bits(), Ordering::Relaxed);
        self.trim
            .store(s.input_trim_db.to_bits(), Ordering::Relaxed);
        self.level
            .store(s.rig_level_db.to_bits(), Ordering::Relaxed);
        self.pedal_mix
            .store(s.pedal_mix.to_bits(), Ordering::Relaxed);
        self.enabled.store(
            s.calibration_enabled && s.input_reference_set,
            Ordering::Relaxed,
        );
    }
    pub fn normalize_enabled(&self, pedal: bool) -> bool {
        if pedal {
            self.pedal_normalize.load(Ordering::Relaxed)
                && !self.suppress_pedal_normalize.load(Ordering::Relaxed)
        } else {
            self.amp_normalize.load(Ordering::Relaxed)
        }
    }
}
fn value(p: &AtomicU32) -> f32 {
    f32::from_bits(p.load(Ordering::Relaxed))
}
fn valid_level(level: Option<f64>) -> Option<f64> {
    level.filter(|v| v.is_finite() && (-60.0..=60.0).contains(v))
}
fn gain(db: f32) -> f32 {
    10.0_f32.powf(db.clamp(-24.0, 24.0) / 20.0)
}

pub struct CalibratedChain<P, A> {
    pre: P,
    amp: A,
    parameters: Arc<CalibrationParameters>,
    input_gain: f32,
    interstage_gain: f32,
    smoothing: f32,
}
impl<P, A> CalibratedChain<P, A> {
    pub fn new(pre: P, amp: A, parameters: Arc<CalibrationParameters>, rate: u32) -> Self {
        Self {
            pre,
            amp,
            parameters,
            input_gain: 1.0,
            interstage_gain: 1.0,
            smoothing: 1.0 - (-1.0 / (0.01 * rate as f32)).exp(),
        }
    }
}
impl<P: MonoProcessor, A: MonoProcessor> MonoProcessor for CalibratedChain<P, A> {
    fn process(&mut self, samples: &mut [f32]) {
        let p = &self.parameters;
        let pre = self.pre.model_metadata();
        let amp = self.amp.model_metadata();
        let enabled = p.enabled.load(Ordering::Relaxed);
        // Parallel dry/pedal mixing does not have a single output calibration.
        // Keep manual gain staging rather than pretending the chain is calibrated.
        let calibrated = enabled && (pre.is_none() || value(&p.pedal_mix) >= 0.999);
        let reference = value(&p.reference) as f64;
        // Each NAM is calibrated on its own, as in Tone3000: the first model
        // in the chain receives `reference - its input level`. A pedal that
        // feeds an amp model hands off at its true output level (`output -
        // reference`, back in the user's reference frame), and the amp then
        // applies `reference - its input level`. With both levels known the
        // reference cancels to `pedal output - amp input`; when only one
        // side is known, that side is still honoured instead of dropping
        // the amp's calibration altogether.
        let level = |metadata: Option<nam_bridge::ModelMetadata>,
                     pick: fn(&nam_bridge::ModelMetadata) -> Option<f64>| {
            if calibrated {
                valid_level(metadata.as_ref().and_then(pick))
            } else {
                None
            }
        };
        let first = if pre.is_some() { pre } else { amp };
        let input_db = level(first, |m| m.input_level_dbu)
            .map(|db| reference - db)
            .unwrap_or(0.0);
        // Hand-off only applies when another model follows the pedal; a lone
        // pedal is the last NAM and keeps loudness normalization.
        let pedal_handoff = if amp.is_some() {
            level(pre, |m| m.output_level_dbu).map(|db| db - reference)
        } else {
            None
        };
        let amp_input = if pre.is_some() {
            level(amp, |m| m.input_level_dbu).map(|db| reference - db)
        } else {
            None // already applied as the chain's input calibration
        };
        let between_db = pedal_handoff.unwrap_or(0.0) + amp_input.unwrap_or(0.0);
        // Calibrated hand-off and normalization have contradictory goals:
        // a pedal that hands off at its true level must not also normalize.
        p.suppress_pedal_normalize
            .store(pedal_handoff.is_some(), Ordering::Relaxed);
        let input_target = gain(input_db as f32 + value(&p.trim));
        let between_target = gain(between_db as f32);
        for sample in samples.iter_mut() {
            self.input_gain += (input_target - self.input_gain) * self.smoothing;
            *sample *= self.input_gain;
        }
        self.pre.process(samples);
        for sample in samples.iter_mut() {
            self.interstage_gain += (between_target - self.interstage_gain) * self.smoothing;
            *sample *= self.interstage_gain;
        }
        self.amp.process(samples);
    }
}

pub struct RigLevel<P> {
    processor: P,
    parameters: Arc<CalibrationParameters>,
    gain: f32,
    smoothing: f32,
}
impl<P> RigLevel<P> {
    pub fn new(processor: P, parameters: Arc<CalibrationParameters>, rate: u32) -> Self {
        Self {
            processor,
            gain: gain(value(&parameters.level)),
            parameters,
            smoothing: 1.0 - (-1.0 / (0.01 * rate as f32)).exp(),
        }
    }
}
impl<P: MonoProcessor> MonoProcessor for RigLevel<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        self.processor.process_stereo(samples, output);
        let target = gain(value(&self.parameters.level));
        for frame in output {
            self.gain += (target - self.gain) * self.smoothing;
            for sample in frame {
                *sample *= self.gain;
            }
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        self.processor.process(samples);
        let target = gain(value(&self.parameters.level));
        for sample in samples {
            self.gain += (target - self.gain) * self.smoothing;
            *sample *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nam_bridge::ModelMetadata;

    /// Stand-in model: unity gain with chosen metadata.
    struct Model(Option<ModelMetadata>);
    impl MonoProcessor for Model {
        fn process(&mut self, _: &mut [f32]) {}
        fn model_metadata(&self) -> Option<ModelMetadata> {
            self.0
        }
    }

    fn model(input: Option<f64>, output: Option<f64>) -> Model {
        Model(Some(ModelMetadata {
            input_level_dbu: input,
            output_level_dbu: output,
            loudness_db: None,
            expected_sample_rate: 48_000.0,
        }))
    }

    /// Settled (input, interstage) gains in dB and whether pedal
    /// normalization was suppressed, for a reference of 12 dBu.
    fn staging(pre: Model, amp: Model) -> (f32, f32, bool) {
        let state = EngineState {
            calibration_enabled: true,
            input_reference_set: true,
            input_reference_dbu: 12.0,
            pedal_mix: 1.0,
            ..Default::default()
        };
        let parameters = Arc::new(CalibrationParameters::new(&state));
        let mut chain = CalibratedChain::new(pre, amp, Arc::clone(&parameters), 48_000);
        chain.process(&mut vec![0.0; 48_000]);
        let db = |g: f32| 20.0 * g.log10();
        (
            db(chain.input_gain),
            db(chain.interstage_gain),
            parameters.suppress_pedal_normalize.load(Ordering::Relaxed),
        )
    }

    fn close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 0.01, "{actual} vs {expected}");
    }

    #[test]
    fn full_metadata_hands_off_pedal_output_to_amp_input() {
        let (input, between, suppressed) =
            staging(model(Some(10.0), Some(8.0)), model(Some(6.0), Some(20.0)));
        close(input, 12.0 - 10.0);
        close(between, 8.0 - 6.0); // reference cancels
        assert!(suppressed);
    }

    #[test]
    fn amp_is_still_calibrated_when_the_pedal_lacks_output_metadata() {
        // Previously the amp received no calibration at all here.
        let (input, between, suppressed) = staging(model(Some(10.0), None), model(Some(6.0), None));
        close(input, 2.0);
        close(between, 12.0 - 6.0);
        assert!(!suppressed, "the pedal normalizes instead of handing off");
    }

    #[test]
    fn pedal_hands_off_even_when_the_amp_lacks_input_metadata() {
        let (_, between, suppressed) = staging(model(Some(10.0), Some(8.0)), model(None, None));
        close(between, 8.0 - 12.0);
        assert!(suppressed);
    }

    #[test]
    fn a_lone_pedal_or_amp_gets_input_calibration_only() {
        let (input, between, suppressed) = staging(model(Some(10.0), Some(8.0)), Model(None));
        close(input, 2.0);
        close(between, 0.0);
        assert!(!suppressed, "the last NAM keeps loudness normalization");
        let (input, between, _) = staging(Model(None), model(Some(6.0), Some(20.0)));
        close(input, 6.0);
        close(between, 0.0);
    }

    #[test]
    fn pedal_without_input_metadata_still_calibrates_the_amp() {
        let (input, between, _) = staging(model(None, None), model(Some(6.0), None));
        close(input, 0.0);
        close(between, 6.0);
    }
}
