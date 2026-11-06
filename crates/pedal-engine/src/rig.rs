//! The common guitar graph used by live monitoring, capture and offline render.
//! Monitor-only sources (clicks/backing audio) belong outside this graph.
use crate::{
    EngineState,
    cabinet::{CabinetImpulse, CabinetParameters, CabinetProcessor},
    dsp::{
        AmpControlProcessor, AmpParameters, BypassProcessor, MonoProcessor, PedalControlProcessor,
        PedalParameters, RealtimeTelemetry, SafetyProcessor, SwapMailbox,
    },
    effects::{EffectChainProcessor, EffectsParameters},
};
use std::sync::{Arc, atomic::AtomicBool};

pub struct RigControls {
    pub pedal_eq: Arc<crate::nam_eq::NamEqParameters>,
    pub amp_eq: Arc<crate::nam_eq::NamEqParameters>,
    pub calibration: Arc<crate::calibration::CalibrationParameters>,
    pub pedal: Arc<PedalParameters>,
    pub amp: Arc<AmpParameters>,
    pub cabinet: Arc<CabinetParameters>,
    pub effects: Arc<EffectsParameters>,
    pub pre_bypass: Arc<AtomicBool>,
    pub amp_bypass: Arc<AtomicBool>,
    pub bypass: Arc<AtomicBool>,
    pub cabinet_mailbox: Arc<SwapMailbox<Option<CabinetImpulse>>>,
}

impl RigControls {
    pub fn from_state(s: &EngineState) -> Self {
        Self::from_state_at_rate(s, s.sample_rate)
    }

    pub fn from_state_at_rate(s: &EngineState, rate: u32) -> Self {
        Self {
            pedal_eq: Arc::new(crate::nam_eq::NamEqParameters::new(&s.pedal_eq, rate)),
            amp_eq: Arc::new(crate::nam_eq::NamEqParameters::new(&s.amp_eq, rate)),
            calibration: Arc::new(crate::calibration::CalibrationParameters::new(s)),
            pedal: Arc::new(
                PedalParameters::new(s.pedal_drive_db, s.pedal_mix, s.pedal_level_db)
                    .with_alignment(s.pedal_align_enabled),
            ),
            amp: Arc::new(AmpParameters::new(
                s.amp_bass_db,
                s.amp_mid_db,
                s.amp_treble_db,
                s.amp_volume_db,
            )),
            cabinet: Arc::new(CabinetParameters::new(
                s.cabinet_enabled,
                s.cabinet_gain_db,
                s.cabinet_low_cut_hz,
                s.cabinet_high_cut_hz,
            )),
            effects: Arc::new(EffectsParameters::from_state(s)),
            pre_bypass: Arc::new(AtomicBool::new(s.pre_bypassed)),
            amp_bypass: Arc::new(AtomicBool::new(s.amp_bypassed)),
            bypass: Arc::new(AtomicBool::new(s.bypassed)),
            cabinet_mailbox: Arc::new(SwapMailbox::default()),
        }
    }
}

pub fn build<P: MonoProcessor, A: MonoProcessor>(
    pre: P,
    amp: A,
    cabinet: Option<CabinetImpulse>,
    controls: RigControls,
    rate: u32,
    max_frames: usize,
    telemetry: Arc<RealtimeTelemetry>,
) -> impl MonoProcessor {
    // Inside the slot trim/mix wrappers: pre EQ follows input gain; post EQ
    // affects only the model's wet path, never the pedal's parallel dry share.
    let pre = crate::nam_eq::NamEqProcessor::new(pre, controls.pedal_eq, rate);
    let amp = crate::nam_eq::NamEqProcessor::new(amp, controls.amp_eq, rate);
    let pre = PedalControlProcessor::new(pre, controls.pedal, max_frames, rate);
    let pre = BypassProcessor::new(pre, controls.pre_bypass, rate);
    let amp = AmpControlProcessor::new(amp, controls.amp, rate);
    let amp = BypassProcessor::new(amp, controls.amp_bypass, rate);
    let chain =
        crate::calibration::CalibratedChain::new(pre, amp, Arc::clone(&controls.calibration), rate);
    let chain = CabinetProcessor::new(
        chain,
        cabinet,
        controls.cabinet,
        controls.cabinet_mailbox,
        rate,
    );
    let chain = EffectChainProcessor::new(chain, controls.effects, rate, 30);
    let chain = crate::calibration::RigLevel::new(chain, controls.calibration, rate);
    let chain = BypassProcessor::new(chain, controls.bypass, rate);
    SafetyProcessor::new(chain, telemetry, rate)
}
