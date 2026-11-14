#![recursion_limit = "256"]

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod audio;
pub mod audio_file;
pub mod cabinet;
pub mod calibration;
pub mod capture_info;
pub mod compressor;
pub mod convolution;
pub mod creative_effects;
pub mod dehiss;
mod dry_alignment;
pub mod dsp;
pub mod edge_amp;
pub mod effects;
pub mod fuzz;
pub mod gate;
pub mod live_parameters;
pub mod metronome;
pub mod nam_eq;
pub mod nam_oversampling;
pub mod node_document;
pub mod node_live;
pub mod node_rig;
pub mod node_runtime;
pub mod octaver;
pub mod overdrive;
pub mod oversampling;
pub mod patch_comparison;
pub mod presets;
pub mod preview;
pub mod quality;
pub mod realtime;
pub mod reverb;
pub mod riff_vault;
pub mod rig;
pub mod rt_pool;
pub mod squelch;
pub mod state;
pub mod svf;
pub mod timeline;
pub mod tone_eq;
pub mod transpose;
pub mod tuner;

pub const API_VERSION: u8 = 1;

#[derive(Debug, Deserialize)]
pub struct Request {
    pub api: u8,
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub api: u8,
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineState {
    pub pedal_align_enabled: bool,
    /// Recomputed from the loaded capture, never trusted from a saved file.
    #[serde(skip)]
    pub pedal_alignment: Option<nam_bridge::alignment::AlignmentEstimate>,
    pub pedal_eq: nam_eq::NamEqSettings,
    pub amp_eq: nam_eq::NamEqSettings,
    #[serde(default)]
    pub pedal_normalize_enabled: bool,
    #[serde(default)]
    pub amp_normalize_enabled: bool,
    pub calibration_enabled: bool,
    pub input_reference_dbu: f32,
    pub input_reference_set: bool,
    pub input_trim_db: f32,
    pub rig_level_db: f32,
    pub quality_profile: u8,
    pub render_quality_profile: u8,
    /// Device preferences: 0 Lite, 1 Full; render additionally accepts 2 Same as live.
    pub nam_quality: u8,
    pub render_nam_quality: u8,
    /// Live NAM oversampling factor: 1 (off), 2, 4 or 8. CPU scales with it.
    pub nam_oversampling: u8,
    /// Render override: 0 Same as live, otherwise 1, 2, 4 or 8.
    pub render_nam_oversampling: u8,
    pub bypassed: bool,
    pub preset: String,
    pub preset_id: Option<String>,
    pub preset_dirty: bool,
    pub pre_bypassed: bool,
    pub pre_model: Option<String>,
    pub amp_bypassed: bool,
    pub model: Option<String>,
    pub pedal_drive_db: f32,
    pub pedal_mix: f32,
    pub pedal_level_db: f32,
    pub amp_bass_db: f32,
    pub amp_mid_db: f32,
    pub amp_treble_db: f32,
    /// Pre-amp TIGHT high-pass corner (Hz); 0 = off.
    pub amp_tight_hz: f32,
    pub amp_volume_db: f32,
    pub cabinet_enabled: bool,
    pub cabinet_ir: Option<String>,
    pub cabinet_gain_db: f32,
    pub cabinet_low_cut_hz: f32,
    pub cabinet_high_cut_hz: f32,
    pub gate_enabled: bool,
    /// Device setting: zero the clean input at the interface noise floor.
    pub squelch_enabled: bool,
    pub squelch_threshold_db: f32,
    /// High-band downward expander after the amp/cab (see `dehiss`).
    pub dehiss_enabled: bool,
    pub dehiss_threshold_db: f32,
    pub dehiss_frequency_hz: f32,
    pub dehiss_range_db: f32,
    pub gate_threshold_db: f32,
    /// How far below the threshold a ringing note may fall before the gate closes.
    pub gate_hysteresis_db: f32,
    pub gate_hold_ms: f32,
    pub gate_release_ms: f32,
    pub gate_range_db: f32,
    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
    pub compressor_ratio: f32,
    pub compressor_attack_ms: f32,
    pub compressor_release_ms: f32,
    /// Manual makeup (LEVEL), added to the automatic makeup when enabled.
    pub compressor_level_db: f32,
    pub compressor_auto_makeup: bool,
    /// Parallel blend: 1 = fully compressed, 0 = dry.
    pub compressor_mix: f32,
    pub eq_enabled: bool,
    pub eq_visible: bool,
    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_high_db: f32,
    pub chorus_enabled: bool,
    pub chorus_visible: bool,
    pub chorus_rate_hz: f32,
    pub chorus_depth: f32,
    pub chorus_mix: f32,
    pub flanger_enabled: bool,
    pub flanger_visible: bool,
    pub flanger_rate_hz: f32,
    pub flanger_depth: f32,
    pub flanger_feedback: f32,
    pub flanger_mix: f32,
    pub tremolo_enabled: bool,
    pub tremolo_visible: bool,
    pub tremolo_rate_hz: f32,
    pub tremolo_depth: f32,
    pub octaver_enabled: bool,
    pub octaver_visible: bool,
    pub dehiss_visible: bool,
    pub octaver_mix: f32,
    pub octaver_clean: bool,
    pub delay_enabled: bool,
    pub delay_visible: bool,
    pub delay_time_ms: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    pub reverb_enabled: bool,
    pub reverb_visible: bool,
    pub reverb_decay_seconds: f32,
    pub reverb_mix: f32,
    pub looper_mode: String,
    pub looper_visible: bool,
    pub looper_bpm: u32,
    pub looper_bars: u8,
    pub tuner_enabled: bool,
    pub metronome_enabled: bool,
    pub metronome_bpm: u32,
    pub sample_rate: u32,
    pub buffer_frames: u32,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            pedal_align_enabled: true,
            pedal_alignment: None,
            pedal_eq: nam_eq::NamEqSettings::default(),
            amp_eq: nam_eq::NamEqSettings::default(),
            pedal_normalize_enabled: true,
            amp_normalize_enabled: true,
            calibration_enabled: false,
            input_reference_dbu: 0.0,
            input_reference_set: false,
            input_trim_db: 0.0,
            rig_level_db: 0.0,
            quality_profile: 1,
            render_quality_profile: 3,
            nam_quality: 1,
            render_nam_quality: 2,
            nam_oversampling: 1,
            render_nam_oversampling: 0,
            bypassed: false,
            preset: "Default".to_owned(),
            preset_id: None,
            preset_dirty: false,
            pre_bypassed: false,
            pre_model: None,
            amp_bypassed: false,
            model: None,
            pedal_drive_db: 0.0,
            pedal_mix: 1.0,
            pedal_level_db: 0.0,
            amp_bass_db: 0.0,
            amp_mid_db: 0.0,
            amp_treble_db: 0.0,
            amp_tight_hz: 0.0,
            amp_volume_db: 0.0,
            cabinet_enabled: false,
            cabinet_ir: None,
            cabinet_gain_db: 0.0,
            cabinet_low_cut_hz: 0.0,
            cabinet_high_cut_hz: 0.0,
            gate_enabled: false,
            squelch_enabled: true,
            squelch_threshold_db: crate::squelch::DEFAULT_THRESHOLD_DB,
            dehiss_enabled: false,
            dehiss_threshold_db: -70.0,
            dehiss_frequency_hz: 3_000.0,
            dehiss_range_db: 24.0,
            gate_threshold_db: -80.0,
            gate_hysteresis_db: 12.0,
            gate_hold_ms: 20.0,
            gate_release_ms: 50.0,
            gate_range_db: 80.0,
            compressor_enabled: false,
            compressor_threshold_db: -18.0,
            compressor_ratio: 3.0,
            compressor_attack_ms: 10.0,
            compressor_release_ms: 120.0,
            compressor_level_db: 0.0,
            compressor_auto_makeup: true,
            compressor_mix: 1.0,
            eq_enabled: false,
            eq_visible: false,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_high_db: 0.0,
            chorus_enabled: false,
            chorus_visible: false,
            chorus_rate_hz: 0.8,
            chorus_depth: 0.5,
            chorus_mix: 0.35,
            flanger_enabled: false,
            flanger_visible: false,
            flanger_rate_hz: 0.3,
            flanger_depth: 0.55,
            flanger_feedback: 0.25,
            flanger_mix: 0.4,
            tremolo_enabled: false,
            tremolo_visible: false,
            tremolo_rate_hz: 4.0,
            tremolo_depth: 0.55,
            octaver_enabled: false,
            octaver_visible: false,
            dehiss_visible: false,
            octaver_mix: 0.45,
            octaver_clean: false,
            delay_enabled: false,
            delay_visible: false,
            delay_time_ms: 360.0,
            delay_feedback: 0.35,
            delay_mix: 0.25,
            reverb_enabled: false,
            reverb_visible: false,
            reverb_decay_seconds: 2.5,
            reverb_mix: 0.25,
            looper_mode: "stopped".to_owned(),
            looper_visible: false,
            looper_bpm: 100,
            looper_bars: 4,
            tuner_enabled: false,
            metronome_enabled: false,
            metronome_bpm: 80,
            sample_rate: 48_000,
            buffer_frames: 64,
        }
    }
}

impl EngineState {
    pub fn snapshot(&self) -> Value {
        let mut snapshot = json!({
            "pedal_eq": self.pedal_eq,
            "amp_eq": self.amp_eq,
            "calibration_enabled": self.calibration_enabled,
            "input_reference_dbu": self.input_reference_dbu,
            "input_reference_set": self.input_reference_set,
            "input_trim_db": self.input_trim_db,
            "rig_level_db": self.rig_level_db,
            "quality_profile": self.quality_profile,
            "render_quality_profile": self.render_quality_profile,
            "nam_quality": self.nam_quality,
            "render_nam_quality": self.render_nam_quality,
            "engine": "ready",
            "bypassed": self.bypassed,
            "preset": self.preset,
            "preset_id": self.preset_id,
            "preset_dirty": self.preset_dirty,
            "pre_bypassed": self.pre_bypassed,
            "pre_model": self.pre_model,
            "amp_bypassed": self.amp_bypassed,
            "model": self.model,
            "pedal_drive_db": self.pedal_drive_db,
            "pedal_mix": self.pedal_mix,
            "pedal_level_db": self.pedal_level_db,
            "amp_bass_db": self.amp_bass_db,
            "amp_mid_db": self.amp_mid_db,
            "amp_treble_db": self.amp_treble_db,
            "amp_volume_db": self.amp_volume_db,
            "cabinet_enabled": self.cabinet_enabled,
            "cabinet_ir": self.cabinet_ir,
            "cabinet_gain_db": self.cabinet_gain_db,
            "cabinet_low_cut_hz": self.cabinet_low_cut_hz,
            "cabinet_high_cut_hz": self.cabinet_high_cut_hz,
            "gate_enabled": self.gate_enabled,
            "gate_threshold_db": self.gate_threshold_db,
            "gate_hysteresis_db": self.gate_hysteresis_db,
            "gate_hold_ms": self.gate_hold_ms,
            "gate_release_ms": self.gate_release_ms,
            "gate_range_db": self.gate_range_db,
            "compressor_enabled": self.compressor_enabled,
            "compressor_threshold_db": self.compressor_threshold_db,
            "compressor_ratio": self.compressor_ratio,
            "eq_enabled": self.eq_enabled,
            "eq_visible": self.eq_visible,
            "eq_low_db": self.eq_low_db,
            "eq_mid_db": self.eq_mid_db,
            "eq_high_db": self.eq_high_db,
            "chorus_enabled": self.chorus_enabled,
            "chorus_visible": self.chorus_visible,
            "chorus_rate_hz": self.chorus_rate_hz,
            "chorus_depth": self.chorus_depth,
            "chorus_mix": self.chorus_mix,
            "flanger_enabled": self.flanger_enabled,
            "flanger_visible": self.flanger_visible,
            "flanger_rate_hz": self.flanger_rate_hz,
            "flanger_depth": self.flanger_depth,
            "flanger_feedback": self.flanger_feedback,
            "flanger_mix": self.flanger_mix,
            "tremolo_enabled": self.tremolo_enabled,
            "tremolo_visible": self.tremolo_visible,
            "tremolo_rate_hz": self.tremolo_rate_hz,
            "tremolo_depth": self.tremolo_depth,
            "octaver_enabled": self.octaver_enabled,
            "octaver_visible": self.octaver_visible,
            "octaver_mix": self.octaver_mix,
            "octaver_clean": self.octaver_clean,
            "octaver_clean_latency_ms": crate::octaver::clean_latency_frames(self.sample_rate) as f64 * 1000.0 / self.sample_rate.max(1) as f64,
            "delay_enabled": self.delay_enabled,
            "delay_visible": self.delay_visible,
            "delay_time_ms": self.delay_time_ms,
            "delay_feedback": self.delay_feedback,
            "delay_mix": self.delay_mix,
            "reverb_enabled": self.reverb_enabled,
            "reverb_visible": self.reverb_visible,
            "reverb_decay_seconds": self.reverb_decay_seconds,
            "reverb_mix": self.reverb_mix,
            "looper_mode": self.looper_mode,
            "looper_visible": self.looper_visible,
            "looper_bpm": self.looper_bpm,
            "looper_bars": self.looper_bars,
            "tuner_enabled": self.tuner_enabled,
            "metronome_enabled": self.metronome_enabled,
            "metronome_bpm": self.metronome_bpm,
            "sample_rate": self.sample_rate,
            "buffer_frames": self.buffer_frames,
        });
        snapshot["pedal_normalize_enabled"] = json!(self.pedal_normalize_enabled);
        snapshot["amp_normalize_enabled"] = json!(self.amp_normalize_enabled);
        snapshot["pedal_align_enabled"] = json!(self.pedal_align_enabled);
        snapshot["amp_tight_hz"] = json!(self.amp_tight_hz);
        // A capture that already contains a speaker cab plus a cab IR filters
        // the sound twice (harsh, boxy, smeared); the UI offers to turn the IR off.
        snapshot["pre_includes_cab"] = json!(
            self.pre_model
                .as_deref()
                .is_some_and(crate::capture_info::includes_cab_cached)
        );
        snapshot["amp_includes_cab"] = json!(
            self.model
                .as_deref()
                .is_some_and(crate::capture_info::includes_cab_cached)
        );
        snapshot["compressor_attack_ms"] = json!(self.compressor_attack_ms);
        snapshot["compressor_release_ms"] = json!(self.compressor_release_ms);
        snapshot["compressor_level_db"] = json!(self.compressor_level_db);
        snapshot["compressor_auto_makeup"] = json!(self.compressor_auto_makeup);
        snapshot["compressor_mix"] = json!(self.compressor_mix);
        snapshot["squelch_enabled"] = json!(self.squelch_enabled);
        snapshot["squelch_threshold_db"] = json!(self.squelch_threshold_db);
        snapshot["dehiss_enabled"] = json!(self.dehiss_enabled);
        snapshot["dehiss_visible"] = json!(self.dehiss_visible);
        snapshot["dehiss_threshold_db"] = json!(self.dehiss_threshold_db);
        snapshot["dehiss_frequency_hz"] = json!(self.dehiss_frequency_hz);
        snapshot["dehiss_range_db"] = json!(self.dehiss_range_db);
        snapshot["nam_oversampling"] = json!(self.nam_oversampling);
        snapshot["render_nam_oversampling"] = json!(self.render_nam_oversampling);
        snapshot["pedal_alignment"] = json!(self.pedal_alignment);
        snapshot
    }
}

pub fn handle_request(state: &mut EngineState, request: Request) -> Response {
    if request.api != API_VERSION {
        return error(
            request.id,
            "unsupported_api",
            "Only control API v1 is supported",
        );
    }

    match request.message_type.as_str() {
        "get_state" => Response {
            api: API_VERSION,
            id: request.id,
            message_type: "state",
            payload: state.snapshot(),
        },
        "set_bypass" => match request.payload.get("bypassed").and_then(Value::as_bool) {
            Some(bypassed) => {
                state.bypassed = bypassed;
                Response {
                    api: API_VERSION,
                    id: request.id,
                    message_type: "state",
                    payload: state.snapshot(),
                }
            }
            None => error(
                request.id,
                "invalid_payload",
                "set_bypass requires a boolean 'bypassed' field",
            ),
        },
        _ => error(
            request.id,
            "unknown_message",
            "Unknown control message type",
        ),
    }
}

pub fn parse_and_handle(state: &mut EngineState, line: &str) -> Response {
    match serde_json::from_str::<Request>(line) {
        Ok(request) => handle_request(state, request),
        Err(err) => error("unknown".to_owned(), "invalid_json", &err.to_string()),
    }
}

fn error(id: String, code: &str, message: &str) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "error",
        payload: json!({"code": code, "message": message}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_change_returns_fresh_snapshot() {
        let mut state = EngineState::default();
        let response = parse_and_handle(
            &mut state,
            r#"{"api":1,"id":"test-1","type":"set_bypass","payload":{"bypassed":true}}"#,
        );

        assert_eq!(response.message_type, "state");
        assert_eq!(response.payload["bypassed"], true);
        assert!(state.bypassed);
    }

    #[test]
    fn rejects_an_unknown_api_version() {
        let mut state = EngineState::default();
        let response = parse_and_handle(
            &mut state,
            r#"{"api":9,"id":"test-2","type":"get_state","payload":{}}"#,
        );

        assert_eq!(response.message_type, "error");
        assert_eq!(response.payload["code"], "unsupported_api");
    }
}
