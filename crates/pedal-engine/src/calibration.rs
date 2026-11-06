//! Interface-agnostic gain staging. Never infer a hardware gain-knob position.
use crate::{EngineState, dsp::MonoProcessor};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

pub struct CalibrationParameters {
    enabled: AtomicBool,
    reference: AtomicU32,
    trim: AtomicU32,
    level: AtomicU32,
    pedal_mix: AtomicU32,
}
impl CalibrationParameters {
    pub fn new(s: &EngineState) -> Self {
        let p = Self {
            enabled: AtomicBool::new(false),
            reference: AtomicU32::new(0),
            trim: AtomicU32::new(0),
            level: AtomicU32::new(0),
            pedal_mix: AtomicU32::new(0),
        };
        p.update(s);
        p
    }
    pub fn update(&self, s: &EngineState) {
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
}
fn value(p: &AtomicU32) -> f32 {
    f32::from_bits(p.load(Ordering::Relaxed))
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
        let input_db = if calibrated {
            pre.or(amp)
                .and_then(|m| m.input_level_dbu)
                .map(|db| reference - db)
                .unwrap_or(0.0)
        } else {
            0.0
        };
        let between_db = if calibrated {
            pre.and_then(|m| m.output_level_dbu)
                .zip(amp.and_then(|m| m.input_level_dbu))
                .map(|(out, input)| out - input)
                .unwrap_or(0.0)
        } else {
            0.0
        };
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
