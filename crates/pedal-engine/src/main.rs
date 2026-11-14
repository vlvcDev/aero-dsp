use pedal_engine::audio::{AudioConfig, probe, run_processor_with_telemetry};
use pedal_engine::cabinet::{CabinetImpulse, CabinetParameters};
use pedal_engine::dsp::{
    AmpParameters, AmpProcessor, PedalParameters, RealtimeTelemetry, SafetyProcessor, SwapMailbox,
    SwappableProcessor,
};
use pedal_engine::effects::{EffectsParameters, LooperMode};
use pedal_engine::metronome::{MetronomeParameters, MetronomeProcessor};
use pedal_engine::presets::PresetStore;
use pedal_engine::riff_vault::{
    DEFAULT_SECONDS, RiffVault, RiffVaultProcessor, SavedRiff, TempoEstimate,
};
use pedal_engine::tuner::{TunerParameters, TunerTapProcessor, TunerTelemetry};
use pedal_engine::{API_VERSION, EngineState, Request, Response, handle_request, parse_and_handle};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const DEFAULT_SOCKET: &str = "/run/pedal/control.sock";
const DEFAULT_STATE_FILE: &str = "/var/lib/pedal/engine-state.json";
const DEFAULT_PRESET_FILE: &str = "/var/lib/pedal/presets.json";
const DEFAULT_PREVIEW_FILE: &str = "/tmp/pedal-preview.wav";
const DEFAULT_TEST_AUDIO_FILE: &str = "/usr/share/pedal/test-audio/guitarjam-184.wav";
const DEFAULT_RIFF_DIRECTORY: &str = "/var/lib/pedal/riffs";

struct ModelMailboxes {
    pre: Arc<SwapMailbox<AmpProcessor>>,
    amp: Arc<SwapMailbox<AmpProcessor>>,
    cabinet: Arc<SwapMailbox<Option<CabinetImpulse>>>,
}

struct BypassFlags {
    all: Arc<AtomicBool>,
    pre: Arc<AtomicBool>,
    amp: Arc<AtomicBool>,
}

struct DspParameters {
    pedal_eq: Arc<pedal_engine::nam_eq::NamEqParameters>,
    amp_eq: Arc<pedal_engine::nam_eq::NamEqParameters>,
    calibration: Arc<pedal_engine::calibration::CalibrationParameters>,
    pedal: Arc<PedalParameters>,
    amp: Arc<AmpParameters>,
    cabinet: Arc<CabinetParameters>,
    effects: Arc<EffectsParameters>,
    tuner: Arc<TunerParameters>,
    metronome: Arc<MetronomeParameters>,
}

#[derive(Clone)]
struct ControlContext {
    patch: Arc<Mutex<Option<pedal_engine::node_live::PatchControl>>>,
    transport: Arc<pedal_engine::timeline::Transport>,
    state: Arc<Mutex<EngineState>>,
    bypass_flags: Arc<BypassFlags>,
    dsp_parameters: Arc<DspParameters>,
    telemetry: Arc<RealtimeTelemetry>,
    tuner_telemetry: Arc<TunerTelemetry>,
    model_mailboxes: Arc<ModelMailboxes>,
    presets: Arc<Mutex<PresetStore>>,
    state_path: PathBuf,
    preset_path: PathBuf,
    preview_path: PathBuf,
    test_audio_path: PathBuf,
    preview_lock: Arc<Mutex<()>>,
    riff_vault: Arc<Mutex<RiffVault>>,
    riff_directory: PathBuf,
}

impl DspParameters {
    fn from_state(state: &EngineState) -> Self {
        Self {
            pedal_eq: Arc::new(pedal_engine::nam_eq::NamEqParameters::new(
                &state.pedal_eq,
                state.sample_rate,
            )),
            amp_eq: Arc::new(pedal_engine::nam_eq::NamEqParameters::new(
                &state.amp_eq,
                state.sample_rate,
            )),
            calibration: Arc::new(pedal_engine::calibration::CalibrationParameters::new(state)),
            pedal: Arc::new(
                PedalParameters::new(state.pedal_drive_db, state.pedal_mix, state.pedal_level_db)
                    .with_alignment(state.pedal_align_enabled),
            ),
            amp: Arc::new(
                AmpParameters::new(
                    state.amp_bass_db,
                    state.amp_mid_db,
                    state.amp_treble_db,
                    state.amp_volume_db,
                )
                .with_tight(state.amp_tight_hz),
            ),
            cabinet: Arc::new(CabinetParameters::new(
                state.cabinet_enabled,
                state.cabinet_gain_db,
                state.cabinet_low_cut_hz,
                state.cabinet_high_cut_hz,
            )),
            effects: Arc::new(EffectsParameters::from_state(state)),
            tuner: Arc::new(TunerParameters::new(state.tuner_enabled)),
            metronome: Arc::new(MetronomeParameters::new(
                state.metronome_enabled,
                state.metronome_bpm,
            )),
        }
    }

    fn update(&self, state: &EngineState) {
        self.pedal
            .align_enabled
            .store(state.pedal_align_enabled, Ordering::Relaxed);
        self.pedal_eq.update(&state.pedal_eq, state.sample_rate);
        self.amp_eq.update(&state.amp_eq, state.sample_rate);
        self.calibration.update(state);
        self.pedal
            .update(state.pedal_drive_db, state.pedal_mix, state.pedal_level_db);
        self.amp.update(
            state.amp_bass_db,
            state.amp_mid_db,
            state.amp_treble_db,
            state.amp_volume_db,
        );
        self.amp.set_tight(state.amp_tight_hz);
        self.cabinet.update(
            state.cabinet_enabled,
            state.cabinet_gain_db,
            state.cabinet_low_cut_hz,
            state.cabinet_high_cut_hz,
        );
        self.effects.update(state);
        self.tuner.update(state.tuner_enabled);
        self.metronome
            .update(state.metronome_enabled, state.metronome_bpm);
    }
}

fn main() -> std::io::Result<()> {
    let socket_path = env::var_os("PEDAL_CONTROL_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET));
    let state_path = env::var_os("PEDAL_STATE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_FILE));
    let preset_path = env::var_os("PEDAL_PRESET_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PRESET_FILE));
    let preview_path = env::var_os("PEDAL_PREVIEW_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PREVIEW_FILE));
    let test_audio_path = env::var_os("PEDAL_TEST_AUDIO_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TEST_AUDIO_FILE));
    let riff_directory = env::var_os("PEDAL_RIFF_DIRECTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_RIFF_DIRECTORY));

    prepare_socket(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)?;
    let mut restored_state = pedal_engine::state::load_or_default(&state_path);
    // Loop audio is deliberately in-memory only. Never report a non-existent
    // loop as playing after a restart.
    restored_state.looper_mode = LooperMode::Stopped.as_state().to_owned();
    restored_state.tuner_enabled = false;
    restored_state.metronome_enabled = false;
    let restored_presets = pedal_engine::presets::load_or_default(&preset_path);
    if restored_state
        .preset_id
        .as_deref()
        .is_some_and(|id| restored_presets.find(id).is_none())
    {
        restored_state.preset_id = None;
        restored_state.preset_dirty = false;
    }
    let state = Arc::new(Mutex::new(restored_state));
    let presets = Arc::new(Mutex::new(restored_presets));
    let bypass_flags = {
        let state = state.lock().expect("control state lock poisoned");
        Arc::new(BypassFlags {
            all: Arc::new(AtomicBool::new(state.bypassed)),
            pre: Arc::new(AtomicBool::new(state.pre_bypassed)),
            amp: Arc::new(AtomicBool::new(state.amp_bypassed)),
        })
    };
    let dsp_parameters = {
        let state = state.lock().expect("control state lock poisoned");
        Arc::new(DspParameters::from_state(&state))
    };
    let model_mailboxes = Arc::new(ModelMailboxes {
        pre: Arc::new(SwapMailbox::default()),
        amp: Arc::new(SwapMailbox::default()),
        cabinet: Arc::new(SwapMailbox::default()),
    });
    let telemetry = Arc::new(RealtimeTelemetry::default());
    let tuner_telemetry = Arc::new(TunerTelemetry::default());
    let riff_vault = Arc::new(Mutex::new(RiffVault::new(48_000, DEFAULT_SECONDS)));
    let transport = Arc::new(pedal_engine::timeline::Transport::default());
    let patch = Arc::new(Mutex::new(None));
    if audio_is_enabled() {
        spawn_audio_worker(AudioWorkerContext {
            patch: Arc::clone(&patch),
            transport: Arc::clone(&transport),
            state: Arc::clone(&state),
            bypass_flags: Arc::clone(&bypass_flags),
            dsp_parameters: Arc::clone(&dsp_parameters),
            telemetry: Arc::clone(&telemetry),
            tuner_telemetry: Arc::clone(&tuner_telemetry),
            model_mailboxes: Arc::clone(&model_mailboxes),
            riff_vault: Arc::clone(&riff_vault),
            state_path: state_path.clone(),
        });
    } else {
        eprintln!("audio disabled by PEDAL_AUDIO_ENABLED=0");
    }
    let control_context = ControlContext {
        patch,
        transport,
        state,
        bypass_flags,
        dsp_parameters,
        telemetry,
        tuner_telemetry,
        model_mailboxes,
        presets,
        state_path,
        preset_path,
        preview_path,
        test_audio_path,
        preview_lock: Arc::new(Mutex::new(())),
        riff_vault,
        riff_directory,
    };
    eprintln!("pedal-engine listening on {}", socket_path.display());

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let context = control_context.clone();
                thread::spawn(move || {
                    if let Err(err) = serve_client(stream, context) {
                        eprintln!("control client disconnected: {err}");
                    }
                });
            }
            Err(err) => eprintln!("control accept failed: {err}"),
        }
    }
    Ok(())
}

fn prepare_socket(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn serve_client(stream: UnixStream, context: ControlContext) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = line?;
        let special_request = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION);
        let response = if let Some(request) = special_request.as_ref()
            && request.message_type.starts_with("timeline_")
        {
            let result = match request.message_type.as_str() {
                "timeline_start" => context.transport.start(&request.payload),
                "timeline_stop" => context.transport.stop(&context.riff_directory),
                "timeline_status" => Ok(context.transport.status()),
                "timeline_tracks" => context.transport.update_tracks(&request.payload),
                "timeline_click" => context.transport.set_click(&request.payload),
                "timeline_loop" => context.transport.set_looping(&request.payload),
                _ => Err("unknown timeline command".to_owned()),
            };
            match result {
                Ok(payload) => Response {
                    api: API_VERSION,
                    id: request.id.clone(),
                    message_type: "timeline",
                    payload,
                },
                Err(error) => control_error(request.id.clone(), "timeline_failed", &error),
            }
        } else if let Some(request) = special_request
            && request.message_type == "get_meters"
        {
            let mut response =
                meters_response(request.id, &context.telemetry, &context.tuner_telemetry);
            response.payload["compressor_reduction_db"] = serde_json::json!(
                context
                    .dsp_parameters
                    .effects
                    .take_compressor_reduction_db()
            );
            response.payload["patch_active"] = serde_json::json!(
                context
                    .patch
                    .lock()
                    .ok()
                    .and_then(|p| p.as_ref().map(|c| c.status().0))
                    .unwrap_or(false)
            );
            response
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|r| r.api == API_VERSION && r.message_type == "dehiss_learn")
        {
            dehiss_learn(&context, request)
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|r| r.api == API_VERSION && r.message_type == "noise_floor_learn")
        {
            noise_floor_learn(&context, request)
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|r| r.api == API_VERSION && r.message_type == "match_rig_level")
        {
            let before = context.state.lock().expect("state").clone();
            let _render_guard = context.preview_lock.lock().expect("render lock");
            match pedal_engine::preview::audition_level(&before, &context.preview_path) {
                Ok(db) => {
                    let mut state = context.state.lock().expect("state");
                    if *state != before {
                        control_error(
                            request.id,
                            "rig_changed",
                            "Rig changed while matching. Try again.",
                        )
                    } else {
                        state.rig_level_db = db;
                        mark_preset_dirty(&mut state);
                        context.dsp_parameters.update(&state);
                        match pedal_engine::state::save_atomic(&context.state_path, &state) {
                            Ok(()) => state_response(request.id, &state),
                            Err(error) => {
                                control_error(request.id, "save_failed", &error.to_string())
                            }
                        }
                    }
                }
                Err(error) => control_error(request.id, "level_match_failed", &error),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "get_state"
        {
            let mut state = context.state.lock().expect("control state lock poisoned");
            state.looper_mode = context
                .dsp_parameters
                .effects
                .looper_mode()
                .as_state()
                .to_owned();
            state_response(request.id, &state)
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "list_riffs"
        {
            match pedal_engine::riff_vault::list(&context.riff_directory) {
                Ok(riffs) => riffs_response(request.id, &riffs),
                Err(error) => control_error(request.id, "riff_list_failed", &error),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "riff_buffer_status"
        {
            match context.riff_vault.lock() {
                Ok(vault) => riff_buffer_status_response(
                    request.id,
                    vault.available_seconds(),
                    vault.capacity_seconds(),
                    vault.dropped_frames(),
                ),
                Err(_) => control_error(
                    request.id,
                    "riff_buffer_failed",
                    "riff buffer lock poisoned",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "riff_buffer_waveform"
        {
            let point_count = request
                .payload
                .get("points")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(192)
                .clamp(32, 512);
            match context.riff_vault.lock() {
                Ok(vault) => waveform_response(request.id, &vault.processed_peaks(point_count)),
                Err(_) => control_error(
                    request.id,
                    "riff_buffer_failed",
                    "riff buffer lock poisoned",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "save_riff"
        {
            let name = request
                .payload
                .get("name")
                .and_then(serde_json::Value::as_str);
            let trim_start_seconds = request
                .payload
                .get("trim_start_seconds")
                .and_then(serde_json::Value::as_f64)
                .map(|value| value as f32);
            let trim_end_seconds = request
                .payload
                .get("trim_end_seconds")
                .and_then(serde_json::Value::as_f64)
                .map(|value| value as f32);
            let preset = context
                .state
                .lock()
                .expect("control state lock poisoned")
                .preset
                .clone();
            match pedal_engine::riff_vault::save_latest(
                &context.riff_vault,
                &context.riff_directory,
                name,
                &preset,
                trim_start_seconds,
                trim_end_seconds,
            ) {
                Ok(riff) => riff_response(request.id, &riff),
                Err(error) => control_error(request.id, "riff_save_failed", &error),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "trim_riff"
        {
            let clean_path = request
                .payload
                .get("clean_path")
                .and_then(serde_json::Value::as_str);
            let processed_path = request
                .payload
                .get("processed_path")
                .and_then(serde_json::Value::as_str);
            let start = request
                .payload
                .get("trim_start_seconds")
                .and_then(serde_json::Value::as_f64);
            let end = request
                .payload
                .get("trim_end_seconds")
                .and_then(serde_json::Value::as_f64);
            let name = request
                .payload
                .get("name")
                .and_then(serde_json::Value::as_str);
            let preset = context
                .state
                .lock()
                .expect("control state lock poisoned")
                .preset
                .clone();
            match (clean_path, processed_path, start, end) {
                (Some(clean), Some(processed), Some(start), Some(end)) => {
                    match pedal_engine::riff_vault::trim_saved(
                        std::path::Path::new(clean),
                        std::path::Path::new(processed),
                        &context.riff_directory,
                        name,
                        &preset,
                        start as f32,
                        end as f32,
                    ) {
                        Ok(riff) => riff_response(request.id, &riff),
                        Err(error) => control_error(request.id, "riff_trim_failed", &error),
                    }
                }
                _ => control_error(
                    request.id,
                    "invalid_payload",
                    "trim_riff requires take paths and start/end seconds",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "estimate_riff_tempo"
        {
            let requested_path = request
                .payload
                .get("clean_path")
                .and_then(serde_json::Value::as_str);
            let riff_root = context.riff_directory.canonicalize().ok();
            match (
                requested_path
                    .map(std::path::Path::new)
                    .and_then(|path| path.canonicalize().ok()),
                riff_root,
            ) {
                (Some(path), Some(root)) if path.starts_with(&root) => {
                    match pedal_engine::riff_vault::estimate_tempo(&path) {
                        Ok(estimate) => tempo_response(request.id, &estimate),
                        Err(error) => control_error(request.id, "tempo_estimate_failed", &error),
                    }
                }
                _ => control_error(
                    request.id,
                    "invalid_payload",
                    "choose a saved clean guitar take to estimate BPM",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "reamp_riff"
        {
            let requested_path = request
                .payload
                .get("clean_path")
                .and_then(serde_json::Value::as_str);
            let name = request
                .payload
                .get("name")
                .and_then(serde_json::Value::as_str);
            let riff_root = context.riff_directory.canonicalize().ok();
            match (
                requested_path
                    .map(std::path::Path::new)
                    .and_then(|path| path.canonicalize().ok()),
                riff_root,
            ) {
                (Some(clean_path), Some(root)) if clean_path.starts_with(&root) => {
                    let preview_state = context
                        .state
                        .lock()
                        .expect("control state lock poisoned")
                        .clone();
                    let preset = preview_state.preset.clone();
                    let _preview_guard =
                        context.preview_lock.lock().expect("preview lock poisoned");
                    match pedal_engine::preview::render_source(
                        &preview_state,
                        &context.preview_path,
                        Some(&clean_path),
                        true,
                        None,
                        pedal_engine::riff_vault::source_duration(&clean_path),
                    ) {
                        Ok(rendered) => match pedal_engine::riff_vault::trim_saved(
                            &clean_path,
                            &rendered.path,
                            &context.riff_directory,
                            name,
                            &preset,
                            0.0,
                            rendered.duration_seconds,
                        ) {
                            Ok(riff) => riff_response(request.id, &riff),
                            Err(error) => control_error(request.id, "riff_reamp_failed", &error),
                        },
                        Err(error) => control_error(request.id, "riff_reamp_failed", &error),
                    }
                }
                _ => control_error(
                    request.id,
                    "invalid_payload",
                    "choose a saved clean guitar take to reamp",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "riff_waveform"
        {
            let requested_path = request
                .payload
                .get("path")
                .and_then(serde_json::Value::as_str);
            let point_count = request
                .payload
                .get("points")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(192)
                .clamp(32, 512);
            let riff_root = context.riff_directory.canonicalize().ok();
            match (
                requested_path
                    .map(std::path::Path::new)
                    .and_then(|path| path.canonicalize().ok()),
                riff_root,
            ) {
                (Some(path), Some(root)) if path.starts_with(&root) => {
                    match pedal_engine::riff_vault::waveform_peaks(&path, point_count) {
                        Ok(peaks) => waveform_response(request.id, &peaks),
                        Err(error) => control_error(request.id, "waveform_failed", &error),
                    }
                }
                _ => control_error(
                    request.id,
                    "invalid_payload",
                    "choose a saved guitar take to view its waveform",
                ),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && matches!(
                request.message_type.as_str(),
                "patch_status"
                    | "apply_patch"
                    | "update_patch"
                    | "deactivate_patch"
                    | "render_patch"
                    | "render_patch_comparison"
            )
        {
            match handle_patch_request(&request, &context) {
                Ok(payload) => Response {
                    api: API_VERSION,
                    id: request.id,
                    message_type: "patch",
                    payload,
                },
                Err(error) => control_error(request.id, "patch_failed", &error),
            }
        } else if let Some(request) = serde_json::from_str::<Request>(&line)
            .ok()
            .filter(|request| request.api == API_VERSION)
            && request.message_type == "render_preview"
        {
            let preview_state = context
                .state
                .lock()
                .expect("control state lock poisoned")
                .clone();
            let _preview_guard = context.preview_lock.lock().expect("preview lock poisoned");
            let processed = request
                .payload
                .get("processed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let source = match request
                .payload
                .get("source")
                .and_then(serde_json::Value::as_str)
            {
                Some("built_in") => Some(context.test_audio_path.as_path()),
                Some("file") => request
                    .payload
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(Path::new),
                _ => None,
            };
            let start_seconds = request
                .payload
                .get("start_seconds")
                .and_then(serde_json::Value::as_f64)
                .map(|value| value as f32);
            let end_seconds = request
                .payload
                .get("end_seconds")
                .and_then(serde_json::Value::as_f64)
                .map(|value| value as f32);
            match pedal_engine::preview::render_source(
                &preview_state,
                &context.preview_path,
                source,
                processed,
                start_seconds,
                end_seconds,
            ) {
                Ok(preview) => {
                    preview_response(request.id, &preview.path, preview.duration_seconds)
                }
                Err(error) => control_error(request.id, "preview_failed", &error),
            }
        } else {
            let mut state = context.state.lock().expect("control state lock poisoned");
            let mut presets = context.presets.lock().expect("preset store lock poisoned");
            let previous = state.clone();
            let previous_presets = presets.clone();
            let parsed_request = serde_json::from_str::<Request>(&line).ok();
            let response =
                handle_control_line(&mut state, &line, &context.model_mailboxes, &mut presets);
            if response.message_type == "state"
                && let Some(request) = parsed_request.as_ref()
                && request.message_type == "looper_action"
            {
                match state.looper_mode.as_str() {
                    "count_in" => context
                        .dsp_parameters
                        .effects
                        .start_quantized_loop(state.looper_bpm, state.looper_bars),
                    "playing" => context
                        .dsp_parameters
                        .effects
                        .set_looper_mode(LooperMode::Playing),
                    "overdubbing" => context
                        .dsp_parameters
                        .effects
                        .set_looper_mode(LooperMode::Overdubbing),
                    _ => context
                        .dsp_parameters
                        .effects
                        .set_looper_mode(LooperMode::Stopped),
                }
            }
            context
                .bypass_flags
                .all
                .store(state.bypassed, Ordering::Relaxed);
            context
                .bypass_flags
                .pre
                .store(state.pre_bypassed, Ordering::Relaxed);
            context
                .bypass_flags
                .amp
                .store(state.amp_bypassed, Ordering::Relaxed);
            context.dsp_parameters.update(&state);
            if *state != previous
                && let Err(error) = pedal_engine::state::save_atomic(&context.state_path, &state)
            {
                eprintln!("could not persist engine state: {error}");
            }
            if *presets != previous_presets
                && let Err(error) =
                    pedal_engine::presets::save_atomic(&context.preset_path, &presets)
            {
                eprintln!("could not persist presets: {error}");
            }
            response
        };
        serde_json::to_writer(&mut writer, &response).map_err(std::io::Error::other)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn audio_is_enabled() -> bool {
    env::var("PEDAL_AUDIO_ENABLED").map_or(true, |value| value != "0")
}

fn handle_patch_request(
    request: &Request,
    context: &ControlContext,
) -> Result<serde_json::Value, String> {
    use pedal_engine::{
        audio_file::{AudioFile, resample, write_float_wav},
        node_document::RigDocument,
    };
    if request.message_type == "patch_status" {
        let guard = context.patch.lock().map_err(|e| e.to_string())?;
        return Ok(match guard.as_ref() {
            Some(control) => {
                let (active, faulted, switching) = control.status();
                serde_json::json!({"available":true,"active":active,"faulted":faulted,"switching":switching,"revision":control.revision(),"sample_rate":control.sample_rate,"latency_samples":control.latency_samples(),"bypassed":context.bypass_flags.all.load(Ordering::Relaxed)})
            }
            None => {
                serde_json::json!({"available":false,"active":false,"faulted":false,"switching":false})
            }
        });
    }
    if request.message_type == "deactivate_patch" {
        let guard = context.patch.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or("Live audio is unavailable")?
            .submit(None)?;
        return Ok(serde_json::json!({"queued":true}));
    }
    if request.message_type == "render_patch_comparison" {
        let parse = |name: &str| -> Result<(RigDocument, PathBuf), String> {
            let item = request
                .payload
                .get(name)
                .ok_or("Missing comparison snapshot")?;
            let value = item.get("document").ok_or("Missing rig document")?;
            if value.to_string().len() > 1_048_576 {
                return Err("Rig document exceeds 1 MiB".into());
            }
            let document =
                serde_json::from_value(value.clone()).map_err(|e| format!("Invalid rig: {e}"))?;
            let directory = PathBuf::from(
                item.get("directory")
                    .and_then(|v| v.as_str())
                    .ok_or("Missing asset folder")?,
            );
            if !directory.is_absolute() || !directory.is_dir() {
                return Err("Asset folder must be an existing absolute path".into());
            }
            Ok((document, directory))
        };
        let (a, a_dir) = parse("a")?;
        let (b, b_dir) = parse("b")?;
        let source = match request
            .payload
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("built_in")
        {
            "built_in" => context.test_audio_path.clone(),
            "file" => PathBuf::from(
                request
                    .payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or("Choose a WAV file")?,
            ),
            "latest_clean" => PathBuf::from(
                pedal_engine::riff_vault::list(&context.riff_directory)?
                    .first()
                    .ok_or("No saved clean take available")?
                    .clean_path
                    .clone(),
            ),
            _ => return Err("Unknown comparison source".into()),
        };
        let _guard = context.preview_lock.lock().map_err(|e| e.to_string())?;
        return pedal_engine::patch_comparison::render(
            &a,
            &a_dir,
            &b,
            &b_dir,
            &source,
            &context.preview_path,
        );
    }
    let value = request
        .payload
        .get("document")
        .ok_or("Missing rig document")?;
    if value.to_string().len() > 1_048_576 {
        return Err("Rig document exceeds 1 MiB".into());
    }
    let document: RigDocument = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    let directory = request
        .payload
        .get("directory")
        .and_then(|v| v.as_str())
        .ok_or("Choose the folder containing your patch assets")?;
    let directory = Path::new(directory);
    if request.message_type == "update_patch" {
        let revision = request
            .payload
            .get("revision")
            .and_then(serde_json::Value::as_str)
            .ok_or("Missing active rig revision")?;
        let guard = context.patch.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or("Live audio is unavailable")?
            .update(revision, &document, directory)?;
        return Ok(serde_json::json!({"updated":true,"revision":revision}));
    }
    if !directory.is_absolute() || !directory.is_dir() {
        return Err("Asset folder must be an existing absolute path".into());
    }
    if request.message_type == "apply_patch" {
        let prepared = document.prepare(directory)?;
        let guard = context.patch.lock().map_err(|e| e.to_string())?;
        let control = guard.as_ref().ok_or("Live audio is unavailable")?;
        if document.sample_rate != control.sample_rate {
            return Err(format!(
                "Patch is {} Hz; your audio device is {} Hz",
                document.sample_rate, control.sample_rate
            ));
        }
        let revision = control.submit(Some(prepared))?;
        return Ok(serde_json::json!({"queued":true,"revision":revision.to_string()}));
    }
    let _guard = context.preview_lock.lock().map_err(|e| e.to_string())?;
    let source_path = request
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .unwrap_or(&context.test_audio_path);
    if fs::metadata(source_path)
        .map_err(|e| format!("Cannot open preview WAV: {e}"))?
        .len()
        > 256 * 1024 * 1024
    {
        return Err("Preview WAV exceeds 256 MiB".into());
    }
    let mut source = AudioFile::load(source_path)?;
    if !(1..=2).contains(&source.channels) {
        return Err("Preview needs a mono or stereo WAV".into());
    }
    // A bounded audition, with three seconds for effects to decay.
    source
        .samples
        .truncate(source.sample_rate as usize * source.channels * 30);
    source.samples = resample(
        &source.samples,
        source.channels,
        source.sample_rate,
        document.sample_rate,
    )?;
    source.sample_rate = document.sample_rate;
    let mut output = document.render(directory, &source, 3.0)?;
    let peak = output
        .iter()
        .flatten()
        .fold(0.0f32, |peak, x| peak.max(x.abs()));
    let attenuation = if peak > 0.89 { 0.89 / peak } else { 1.0 };
    for sample in output.iter_mut().flatten() {
        *sample *= attenuation;
    }
    let path = context.preview_path.with_extension("patch.wav");
    write_float_wav(
        &path,
        2,
        document.sample_rate,
        output.iter().flatten().copied(),
    )?;
    Ok(
        serde_json::json!({"path":path,"duration_seconds":output.len() as f32 / document.sample_rate as f32,"attenuation_db":20.0 * attenuation.log10()}),
    )
}

struct AudioWorkerContext {
    patch: Arc<Mutex<Option<pedal_engine::node_live::PatchControl>>>,
    transport: Arc<pedal_engine::timeline::Transport>,
    state: Arc<Mutex<EngineState>>,
    bypass_flags: Arc<BypassFlags>,
    dsp_parameters: Arc<DspParameters>,
    telemetry: Arc<RealtimeTelemetry>,
    tuner_telemetry: Arc<TunerTelemetry>,
    model_mailboxes: Arc<ModelMailboxes>,
    riff_vault: Arc<Mutex<RiffVault>>,
    state_path: PathBuf,
}

fn spawn_audio_worker(context: AudioWorkerContext) {
    thread::Builder::new()
        .name("pedal-audio".to_owned())
        .spawn(move || {
            let mut delay_ms = 250;
            loop {
                context.telemetry.set_audio_status(1, None);
                let error = match run_audio_worker(&context) {
                    Ok(()) => "audio stream ended unexpectedly".to_owned(),
                    Err(error) => error,
                };
                *context.patch.lock().expect("patch control") = None;
                context.telemetry.set_audio_status(3, Some(error.clone()));
                eprintln!("audio stopped; retrying in {delay_ms} ms: {error}");
                // A new graph has no in-memory loop. Do not claim the old loop is playing.
                context
                    .dsp_parameters
                    .effects
                    .set_looper_mode(LooperMode::Stopped);
                if let Ok(mut state) = context.state.lock() {
                    state.looper_mode = "stopped".to_owned();
                }
                thread::sleep(std::time::Duration::from_millis(delay_ms));
                delay_ms = (delay_ms * 2).min(5000);
            }
        })
        .expect("could not start audio worker");
}

fn run_audio_worker(context: &AudioWorkerContext) -> Result<(), String> {
    let shared_state = &context.state;
    let state_path = context.state_path.as_path();
    let bypass_flags = Arc::clone(&context.bypass_flags);
    let dsp_parameters = Arc::clone(&context.dsp_parameters);
    let telemetry = Arc::clone(&context.telemetry);
    let tuner_telemetry = Arc::clone(&context.tuner_telemetry);
    let model_mailboxes = Arc::clone(&context.model_mailboxes);
    let riff_vault = Arc::clone(&context.riff_vault);
    let config = AudioConfig {
        requested_device: env::var("PEDAL_AUDIO_DEVICE").ok(),
        requested_capture_device: env::var("PEDAL_AUDIO_CAPTURE_DEVICE").ok(),
        requested_playback_device: env::var("PEDAL_AUDIO_PLAYBACK_DEVICE").ok(),
        guitar_input_channel: env::var("PEDAL_GUITAR_INPUT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
        ..AudioConfig::default()
    };
    let negotiated = probe(&config)?;
    let (
        pre_model_path,
        amp_model_path,
        cabinet_ir_path,
        persisted_bypass,
        pre_bypassed,
        amp_bypassed,
        nam_quality,
        nam_oversampling,
    ) = {
        let mut state = shared_state.lock().expect("control state lock poisoned");
        state.sample_rate = negotiated.sample_rate;
        state.buffer_frames = negotiated.period_frames;
        let pre_model_path = state.pre_model.clone();
        let amp_model_path = state.model.clone();
        let cabinet_ir_path = state.cabinet_ir.clone();
        let persisted_bypass = state.bypassed;
        let pre_bypassed = state.pre_bypassed;
        let amp_bypassed = state.amp_bypassed;
        if let Err(error) = pedal_engine::state::save_atomic(state_path, &state) {
            eprintln!("could not persist negotiated audio settings: {error}");
        }
        (
            pre_model_path,
            amp_model_path,
            cabinet_ir_path,
            persisted_bypass,
            pre_bypassed,
            amp_bypassed,
            state.nam_quality,
            state.nam_oversampling,
        )
    };
    bypass_flags.all.store(persisted_bypass, Ordering::Relaxed);
    bypass_flags.pre.store(pre_bypassed, Ordering::Relaxed);
    bypass_flags.amp.store(amp_bypassed, Ordering::Relaxed);

    let (pre, pre_failed) = load_restored_model(
        "pedal",
        pre_model_path.as_deref(),
        negotiated.sample_rate,
        negotiated.period_frames,
        nam_quality,
        nam_oversampling,
    );
    let (amp, amp_failed) = load_restored_model(
        "amp",
        amp_model_path.as_deref(),
        negotiated.sample_rate,
        negotiated.period_frames,
        nam_quality,
        nam_oversampling,
    );
    shared_state.lock().expect("control state").pedal_alignment =
        pedal_engine::dsp::MonoProcessor::dry_wet_alignment(&pre);
    let (cabinet, cabinet_failed) = load_restored_cabinet(
        cabinet_ir_path.as_deref(),
        negotiated.sample_rate,
        shared_state.lock().expect("control state").quality_profile,
    );
    if pre_failed || amp_failed {
        bypass_flags.all.store(true, Ordering::Relaxed);
        let mut state = shared_state.lock().expect("control state lock poisoned");
        state.bypassed = true;
        if let Err(error) = pedal_engine::state::save_atomic(state_path, &state) {
            eprintln!("could not persist safe bypass state: {error}");
        }
    }
    if cabinet_failed {
        let mut state = shared_state.lock().expect("control state lock poisoned");
        state.cabinet_enabled = false;
        if let Err(error) = pedal_engine::state::save_atomic(state_path, &state) {
            eprintln!("could not persist disabled cabinet state: {error}");
        }
    }
    let pre = SwappableProcessor::new(
        pre,
        Arc::clone(&model_mailboxes.pre),
        negotiated.sample_rate,
    );
    let amp = SwappableProcessor::new(
        amp,
        Arc::clone(&model_mailboxes.amp),
        negotiated.sample_rate,
    );
    // Recompile EQ for the negotiated rate before starting the callback.
    dsp_parameters.update(&shared_state.lock().expect("control state"));
    let chain = pedal_engine::rig::build(
        pre,
        amp,
        cabinet,
        pedal_engine::rig::RigControls {
            pedal_eq: Arc::clone(&dsp_parameters.pedal_eq),
            amp_eq: Arc::clone(&dsp_parameters.amp_eq),
            calibration: Arc::clone(&dsp_parameters.calibration),
            pedal: Arc::clone(&dsp_parameters.pedal),
            amp: Arc::clone(&dsp_parameters.amp),
            cabinet: Arc::clone(&dsp_parameters.cabinet),
            effects: Arc::clone(&dsp_parameters.effects),
            pre_bypass: Arc::clone(&bypass_flags.pre),
            amp_bypass: Arc::clone(&bypass_flags.amp),
            bypass: Arc::clone(&bypass_flags.all),
            cabinet_mailbox: Arc::clone(&model_mailboxes.cabinet),
        },
        negotiated.sample_rate,
        negotiated.period_frames as usize,
        Arc::clone(&telemetry),
    );
    let (chain, patch_control) = pedal_engine::node_live::PatchRoute::new(
        chain,
        negotiated.sample_rate,
        Arc::clone(&bypass_flags.all),
    );
    let chain = chain.with_squelch(Arc::clone(&dsp_parameters.effects), negotiated.sample_rate);
    *context.patch.lock().expect("patch control") = Some(patch_control);
    let chain = TunerTapProcessor::new(
        chain,
        Arc::clone(&dsp_parameters.tuner),
        tuner_telemetry,
        negotiated.sample_rate,
    );
    // Capture the clean input and the complete rig output before adding the
    // metronome. The click needs to reach the performer's output, but it must
    // never be printed into either the DI or processed guitar take.
    let chain = RiffVaultProcessor::new(chain, riff_vault);
    let chain = pedal_engine::timeline::TimelineProcessor::new(
        chain,
        Arc::clone(&context.transport),
        negotiated.sample_rate,
    );
    let chain = MetronomeProcessor::new(
        chain,
        Arc::clone(&dsp_parameters.metronome),
        negotiated.sample_rate,
    );
    let mut processor =
        SafetyProcessor::monitor(chain, Arc::clone(&telemetry), negotiated.sample_rate);
    eprintln!(
        "audio ready on {} at {} Hz / {} frames",
        negotiated.device.name, negotiated.sample_rate, negotiated.period_frames
    );
    run_processor_with_telemetry(&config, &mut processor, None, None, Some(&telemetry)).map(|_| ())
}

fn load_restored_model(
    slot: &str,
    path: Option<&str>,
    sample_rate: u32,
    buffer_frames: u32,
    quality: u8,
    oversampling: u8,
) -> (AmpProcessor, bool) {
    let Some(path) = path else {
        return (AmpProcessor::Clean, false);
    };
    match pedal_engine::quality::load_nam(
        path,
        f64::from(sample_rate),
        buffer_frames as usize,
        pedal_engine::quality::nam_size(quality),
        slot == "pedal",
        oversampling,
    ) {
        Ok(model) => {
            eprintln!("loaded restored {slot} NAM model {path}");
            (AmpProcessor::Nam(model), false)
        }
        Err(error) => {
            eprintln!(
                "could not load restored {slot} NAM model {path}: {error}; continuing in clean bypass"
            );
            (AmpProcessor::Clean, true)
        }
    }
}

fn load_restored_cabinet(
    path: Option<&str>,
    sample_rate: u32,
    profile: u8,
) -> (Option<CabinetImpulse>, bool) {
    let Some(path) = path else {
        return (None, false);
    };
    match CabinetImpulse::load_with_duration(
        Path::new(path),
        sample_rate,
        pedal_engine::quality::ir_seconds(profile),
    ) {
        Ok(impulse) => {
            eprintln!("loaded restored cabinet IR {path}");
            (Some(impulse), false)
        }
        Err(error) => {
            eprintln!("could not load restored cabinet IR {path}: {error}; disabling cabinet");
            (None, true)
        }
    }
}

fn handle_control_line(
    state: &mut EngineState,
    line: &str,
    model_mailboxes: &ModelMailboxes,
    presets: &mut PresetStore,
) -> Response {
    let request = match serde_json::from_str::<Request>(line) {
        Ok(request) => request,
        Err(_) => return parse_and_handle(state, line),
    };
    if request.api != API_VERSION {
        return handle_request(state, request);
    }

    if request.message_type == "list_presets" {
        return presets_response(request.id, presets);
    }

    if request.message_type == "save_preset" {
        let Some(name) = preset_name(&request) else {
            return control_error(
                request.id,
                "invalid_name",
                "preset name must contain 1 to 64 characters",
            );
        };
        let preset = presets.save_current(name, state);
        state.preset.clone_from(&preset.name);
        state.preset_id = Some(preset.id.clone());
        state.preset_dirty = false;
        return state_response(request.id, state);
    }

    if request.message_type == "new_preset" {
        model_mailboxes.pre.stage(AmpProcessor::Clean);
        model_mailboxes.amp.stage(AmpProcessor::Clean);
        model_mailboxes.cabinet.stage(None);
        let sample_rate = state.sample_rate;
        let buffer_frames = state.buffer_frames;
        *state = EngineState {
            preset: "New preset".to_owned(),
            sample_rate,
            buffer_frames,
            calibration_enabled: state.calibration_enabled,
            input_reference_dbu: state.input_reference_dbu,
            input_reference_set: state.input_reference_set,
            input_trim_db: state.input_trim_db,
            quality_profile: state.quality_profile,
            render_quality_profile: state.render_quality_profile,
            nam_quality: state.nam_quality,
            render_nam_quality: state.render_nam_quality,
            nam_oversampling: state.nam_oversampling,
            render_nam_oversampling: state.render_nam_oversampling,
            squelch_enabled: state.squelch_enabled,
            squelch_threshold_db: state.squelch_threshold_db,
            ..EngineState::default()
        };
        return state_response(request.id, state);
    }

    if request.message_type == "rename_preset" {
        let Some(id) = request
            .payload
            .get("id")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "preset id is required");
        };
        let Some(name) = preset_name(&request) else {
            return control_error(
                request.id,
                "invalid_name",
                "preset name must contain 1 to 64 characters",
            );
        };
        let Some(preset) = presets.rename(id, name) else {
            return control_error(request.id, "preset_not_found", "preset was not found");
        };
        if state.preset_id.as_deref() == Some(id) {
            state.preset.clone_from(&preset.name);
        }
        return state_response(request.id, state);
    }

    if request.message_type == "delete_preset" {
        let Some(id) = request
            .payload
            .get("id")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "preset id is required");
        };
        if !presets.delete(id) {
            return control_error(request.id, "preset_not_found", "preset was not found");
        }
        if state.preset_id.as_deref() == Some(id) {
            state.preset_id = None;
            state.preset_dirty = false;
        }
        return state_response(request.id, state);
    }

    if request.message_type == "load_preset" {
        let Some(id) = request
            .payload
            .get("id")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "preset id is required");
        };
        let Some(preset) = presets.find(id).cloned() else {
            return control_error(request.id, "preset_not_found", "preset was not found");
        };
        let pre = match prepare_preset_model(preset.rig.pre_model.as_deref(), state, true) {
            Ok(model) => model,
            Err(error) => return control_error(request.id, "preset_model_failed", &error),
        };
        let amp = match prepare_preset_model(preset.rig.model.as_deref(), state, false) {
            Ok(model) => model,
            Err(error) => return control_error(request.id, "preset_model_failed", &error),
        };
        let cabinet = match prepare_preset_cabinet(preset.rig.cabinet_ir.as_deref(), state) {
            Ok(cabinet) => cabinet,
            Err(error) => return control_error(request.id, "preset_cabinet_failed", &error),
        };
        state.pedal_alignment = pedal_engine::dsp::MonoProcessor::dry_wet_alignment(&pre);
        model_mailboxes.pre.stage(pre);
        model_mailboxes.amp.stage(amp);
        model_mailboxes.cabinet.stage(cabinet);
        preset.rig.apply_to(state);
        state.preset = preset.name;
        state.preset_id = Some(preset.id);
        state.preset_dirty = false;
        return state_response(request.id, state);
    }

    if request.message_type == "set_slot_bypass" {
        let Some(slot) = valid_slot(&request) else {
            return control_error(request.id, "invalid_slot", "slot must be 'pre' or 'amp'");
        };
        let Some(bypassed) = request
            .payload
            .get("bypassed")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_slot_bypass requires a boolean 'bypassed' field",
            );
        };
        if slot == "pre" {
            state.pre_bypassed = bypassed;
        } else {
            state.amp_bypassed = bypassed;
        }
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "set_cabinet_bypass" {
        let Some(bypassed) = request
            .payload
            .get("bypassed")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_cabinet_bypass requires a boolean 'bypassed' field",
            );
        };
        state.cabinet_enabled = !bypassed && state.cabinet_ir.is_some();
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "set_effect_bypass" {
        let Some(effect) = request
            .payload
            .get("effect")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "effect is required");
        };
        let Some(bypassed) = request
            .payload
            .get("bypassed")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_effect_bypass requires a boolean 'bypassed' field",
            );
        };
        let enabled = !bypassed;
        match effect {
            "gate" => state.gate_enabled = enabled,
            "dehiss" => state.dehiss_enabled = enabled,
            "compressor" => state.compressor_enabled = enabled,
            "eq" => state.eq_enabled = enabled,
            "chorus" => state.chorus_enabled = enabled,
            "flanger" => state.flanger_enabled = enabled,
            "tremolo" => state.tremolo_enabled = enabled,
            "octaver" => state.octaver_enabled = enabled,
            "delay" => state.delay_enabled = enabled,
            "reverb" => state.reverb_enabled = enabled,
            _ => return control_error(request.id, "invalid_effect", "unknown effect"),
        }
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "set_pedal_visible" {
        let Some(pedal) = request
            .payload
            .get("pedal")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "pedal is required");
        };
        let Some(visible) = request
            .payload
            .get("visible")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_pedal_visible requires a boolean 'visible' field",
            );
        };
        match pedal {
            "eq" => state.eq_visible = visible,
            "chorus" => state.chorus_visible = visible,
            "flanger" => state.flanger_visible = visible,
            "tremolo" => state.tremolo_visible = visible,
            "octaver" => state.octaver_visible = visible,
            "dehiss" => state.dehiss_visible = visible,
            "delay" => state.delay_visible = visible,
            "reverb" => state.reverb_visible = visible,
            "looper" => state.looper_visible = visible,
            _ => {
                return control_error(
                    request.id,
                    "invalid_pedal",
                    "only optional effects and the looper can be added",
                );
            }
        }
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "set_tuner_enabled" {
        let Some(enabled) = request
            .payload
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_tuner_enabled requires a boolean 'enabled' field",
            );
        };
        state.tuner_enabled = enabled;
        return state_response(request.id, state);
    }

    if request.message_type == "set_metronome" {
        let Some(enabled) = request
            .payload
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_metronome requires a boolean 'enabled' field",
            );
        };
        let bpm = request
            .payload
            .get("bpm")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(state.metronome_bpm);
        if !(30..=300).contains(&bpm) {
            return control_error(
                request.id,
                "invalid_payload",
                "metronome BPM must be between 30 and 300",
            );
        }
        state.metronome_enabled = enabled;
        state.metronome_bpm = bpm;
        return state_response(request.id, state);
    }

    if request.message_type == "looper_action" {
        let Some(action) = request
            .payload
            .get("action")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(request.id, "invalid_payload", "looper action is required");
        };
        let mode = match action {
            "record" => {
                let bpm = request
                    .payload
                    .get("bpm")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(state.looper_bpm);
                let bars = request
                    .payload
                    .get("bars")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
                    .unwrap_or(state.looper_bars);
                if !(30..=300).contains(&bpm) || !matches!(bars, 1 | 2 | 4 | 8) {
                    return control_error(
                        request.id,
                        "invalid_looper_timing",
                        "looper BPM must be 30 to 300 and bars must be 1, 2, 4, or 8",
                    );
                }
                if bars as f32 * 4.0 * 60.0 / bpm as f32 > 30.0 {
                    return control_error(
                        request.id,
                        "loop_too_long",
                        "that bar length exceeds the 30 second looper limit at this BPM",
                    );
                }
                state.looper_bpm = bpm;
                state.looper_bars = bars;
                LooperMode::CountIn
            }
            "play" => LooperMode::Playing,
            "overdub" => LooperMode::Overdubbing,
            "stop" | "clear" => LooperMode::Stopped,
            _ => {
                return control_error(request.id, "invalid_looper_action", "unknown looper action");
            }
        };
        state.looper_mode = mode.as_state().to_owned();
        return state_response(request.id, state);
    }

    if request.message_type == "clear_model" {
        let Some(slot) = valid_slot(&request) else {
            return control_error(request.id, "invalid_slot", "slot must be 'pre' or 'amp'");
        };
        if slot == "pre" {
            model_mailboxes.pre.stage(AmpProcessor::Clean);
            state.pre_model = None;
            state.pedal_alignment = None;
            state.pre_bypassed = false;
        } else {
            model_mailboxes.amp.stage(AmpProcessor::Clean);
            state.model = None;
            state.amp_bypassed = false;
        }
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "clear_cabinet" {
        model_mailboxes.cabinet.stage(None);
        state.cabinet_ir = None;
        state.cabinet_enabled = false;
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "select_cabinet" {
        let Some(raw_path) = request
            .payload
            .get("path")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "select_cabinet requires a string 'path' field",
            );
        };
        let path = Path::new(raw_path);
        let impulse = match CabinetImpulse::load_with_duration(
            path,
            state.sample_rate,
            pedal_engine::quality::ir_seconds(state.quality_profile),
        ) {
            Ok(impulse) => impulse,
            Err(error) => return control_error(request.id, "cabinet_load_failed", &error),
        };
        model_mailboxes.cabinet.stage(Some(impulse));
        state.cabinet_ir = Some(raw_path.to_owned());
        state.cabinet_enabled = true;
        mark_preset_dirty(state);
        return state_response(request.id, state);
    }

    if request.message_type == "set_control" {
        let Some(control) = request
            .payload
            .get("control")
            .and_then(serde_json::Value::as_str)
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_control requires a string 'control' field",
            );
        };
        let Some(value) = request
            .payload
            .get("value")
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as f32)
            .filter(|value| value.is_finite())
        else {
            return control_error(
                request.id,
                "invalid_payload",
                "set_control requires a finite numeric 'value' field",
            );
        };
        if control == "quality_profile" {
            if value != 0.0 && value != 1.0 {
                return control_error(
                    request.id,
                    "invalid_control",
                    "live quality must be Portable or High",
                );
            }
            if let Some(path) = state.cabinet_ir.as_deref() {
                match CabinetImpulse::load_with_duration(
                    Path::new(path),
                    state.sample_rate,
                    pedal_engine::quality::ir_seconds(value as u8),
                ) {
                    Ok(ir) => model_mailboxes.cabinet.stage(Some(ir)),
                    Err(error) => return control_error(request.id, "cabinet_load_failed", &error),
                }
            }
        }
        let model_rebuild = match control {
            "nam_quality" => value != f32::from(state.nam_quality),
            "nam_oversampling" => value != f32::from(state.nam_oversampling),
            _ => false,
        };
        if model_rebuild {
            let mut next = state.clone();
            if let Err(message) = set_control(&mut next, control, value) {
                return control_error(request.id, "invalid_control", message);
            }
            // Prepare both slots before publishing either; failure leaves the
            // currently playing rig and preference untouched.
            let pre = match prepare_preset_model(state.pre_model.as_deref(), &next, true) {
                Ok(model) => model,
                Err(error) => return control_error(request.id, "model_load_failed", &error),
            };
            let amp = match prepare_preset_model(state.model.as_deref(), &next, false) {
                Ok(model) => model,
                Err(error) => return control_error(request.id, "model_load_failed", &error),
            };
            state.pedal_alignment = pedal_engine::dsp::MonoProcessor::dry_wet_alignment(&pre);
            model_mailboxes.pre.stage(pre);
            model_mailboxes.amp.stage(amp);
        }
        if let Err(message) = set_control(state, control, value) {
            return control_error(request.id, "invalid_control", message);
        }
        if !matches!(
            control,
            "quality_profile"
                | "render_quality_profile"
                | "nam_quality"
                | "render_nam_quality"
                | "nam_oversampling"
                | "render_nam_oversampling"
                | "squelch_enabled"
                | "squelch_threshold_db"
                | "calibration_enabled"
                | "input_reference_dbu"
                | "input_trim_db"
        ) {
            mark_preset_dirty(state);
        }
        return state_response(request.id, state);
    }

    if request.message_type != "select_model" {
        return handle_request(state, request);
    }

    let Some(raw_path) = request
        .payload
        .get("path")
        .and_then(serde_json::Value::as_str)
    else {
        return control_error(
            request.id,
            "invalid_payload",
            "select_model requires a string 'path' field",
        );
    };
    let preset = request
        .payload
        .get("preset")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Imported tone");
    let Some(slot) = valid_slot(&request) else {
        return control_error(
            request.id,
            "invalid_slot",
            "select_model slot must be 'pre' or 'amp'",
        );
    };
    let path = Path::new(raw_path);
    if !path.is_absolute()
        || !path.is_file()
        || path.extension().and_then(|value| value.to_str()) != Some("nam")
    {
        return control_error(
            request.id,
            "invalid_model_path",
            "model path must be an existing absolute .nam file",
        );
    }

    let prepared = match pedal_engine::quality::load_nam(
        path,
        f64::from(state.sample_rate),
        state.buffer_frames.max(128) as usize,
        pedal_engine::quality::nam_size(state.nam_quality),
        slot == "pre",
        state.nam_oversampling,
    ) {
        Ok(model) => model,
        Err(error) => {
            return control_error(request.id, "model_load_failed", &error.to_string());
        }
    };
    if slot == "pre" {
        state.pedal_alignment = prepared.alignment();
        model_mailboxes.pre.stage(AmpProcessor::Nam(prepared));
        state.pre_model = Some(raw_path.to_owned());
        state.pre_bypassed = false;
    } else {
        model_mailboxes.amp.stage(AmpProcessor::Nam(prepared));
        state.model = Some(raw_path.to_owned());
        state.amp_bypassed = false;
    }
    state.preset = preset.to_owned();
    state.preset_id = None;
    state.preset_dirty = false;

    state_response(request.id, state)
}

fn preset_name(request: &Request) -> Option<String> {
    let name = request.payload.get("name")?.as_str()?.trim();
    (!name.is_empty() && name.chars().count() <= 64).then(|| name.to_owned())
}

fn mark_preset_dirty(state: &mut EngineState) {
    if state.preset_id.is_some() {
        state.preset_dirty = true;
    }
}

fn prepare_preset_model(
    path: Option<&str>,
    state: &EngineState,
    alignment: bool,
) -> Result<AmpProcessor, String> {
    let Some(path) = path else {
        return Ok(AmpProcessor::Clean);
    };
    let path_ref = Path::new(path);
    if !path_ref.is_absolute()
        || !path_ref.is_file()
        || path_ref.extension().and_then(|value| value.to_str()) != Some("nam")
    {
        return Err(format!("preset model is missing or invalid: {path}"));
    }
    pedal_engine::quality::load_nam(
        path_ref,
        f64::from(state.sample_rate),
        state.buffer_frames.max(128) as usize,
        pedal_engine::quality::nam_size(state.nam_quality),
        alignment,
        state.nam_oversampling,
    )
    .map(AmpProcessor::Nam)
    .map_err(|error| error.to_string())
}

fn prepare_preset_cabinet(
    path: Option<&str>,
    state: &EngineState,
) -> Result<Option<CabinetImpulse>, String> {
    path.map(|path| {
        CabinetImpulse::load_with_duration(
            Path::new(path),
            state.sample_rate,
            pedal_engine::quality::ir_seconds(state.quality_profile),
        )
    })
    .transpose()
}

fn set_control(state: &mut EngineState, control: &str, value: f32) -> Result<(), &'static str> {
    if control == "compressor_auto_makeup" {
        if value != 0.0 && value != 1.0 {
            return Err("auto makeup must be 0 or 1");
        }
        state.compressor_auto_makeup = value == 1.0;
        return Ok(());
    }
    if control == "squelch_enabled" {
        if value != 0.0 && value != 1.0 {
            return Err("squelch must be 0 or 1");
        }
        state.squelch_enabled = value == 1.0;
        return Ok(());
    }
    if control == "pedal_align_enabled" {
        if value != 0.0 && value != 1.0 {
            return Err("alignment must be 0 or 1");
        }
        state.pedal_align_enabled = value == 1.0;
        return Ok(());
    }
    if let Some(key) = control.strip_prefix("pedal_eq_") {
        return state.pedal_eq.set_control(key, value);
    }
    if let Some(key) = control.strip_prefix("amp_eq_") {
        return state.amp_eq.set_control(key, value);
    }
    if control == "nam_oversampling" || control == "render_nam_oversampling" {
        let live = control == "nam_oversampling";
        let valid = value.is_finite()
            && value.fract() == 0.0
            && (0.0..=8.0).contains(&value)
            && (pedal_engine::nam_oversampling::is_valid_factor(value as u8)
                || (!live && value == 0.0));
        if !valid {
            return Err(
                "NAM oversampling must be 1, 2, 4 or 8 (render also accepts 0, same as live)",
            );
        }
        if live {
            state.nam_oversampling = value as u8;
        } else {
            state.render_nam_oversampling = value as u8;
        }
        return Ok(());
    }
    if control == "nam_quality" || control == "render_nam_quality" {
        let maximum = if control == "nam_quality" { 1.0 } else { 2.0 };
        if !value.is_finite() || value.fract() != 0.0 || !(0.0..=maximum).contains(&value) {
            return Err("invalid NAM size selection");
        }
        if control == "nam_quality" {
            state.nam_quality = value as u8;
        } else {
            state.render_nam_quality = value as u8;
        }
        return Ok(());
    }
    if control == "quality_profile" || control == "render_quality_profile" {
        let maximum = if control == "quality_profile" {
            1.0
        } else {
            3.0
        };
        if !value.is_finite() || value.fract() != 0.0 || !(0.0..=maximum).contains(&value) {
            return Err("invalid quality profile");
        }
        if control == "quality_profile" {
            state.quality_profile = value as u8;
        } else {
            state.render_quality_profile = value as u8;
        }
        return Ok(());
    }
    if matches!(control, "pedal_normalize_enabled" | "amp_normalize_enabled") {
        if value != 0.0 && value != 1.0 {
            return Err("normalization must be 0 or 1");
        }
        if control == "pedal_normalize_enabled" {
            state.pedal_normalize_enabled = value == 1.0;
        } else {
            state.amp_normalize_enabled = value == 1.0;
        }
        return Ok(());
    }
    if control == "calibration_enabled" {
        if value == 1.0 && !state.input_reference_set {
            return Err(
                "Set a measured input reference at your current hardware gain before enabling calibration",
            );
        }
        if value != 0.0 && value != 1.0 {
            return Err("calibration must be 0 or 1");
        }
        state.calibration_enabled = value == 1.0;
        return Ok(());
    }
    if control == "octaver_clean" {
        if value != 0.0 && value != 1.0 {
            return Err("octaver mode must be 0 (analog) or 1 (clean)");
        }
        state.octaver_clean = value == 1.0;
        return Ok(());
    }
    let (target, minimum, maximum) = match control {
        "input_reference_dbu" => (&mut state.input_reference_dbu, -30.0, 30.0),
        "input_trim_db" => (&mut state.input_trim_db, -24.0, 24.0),
        "rig_level_db" => (&mut state.rig_level_db, -24.0, 12.0),
        "pedal_drive_db" => (&mut state.pedal_drive_db, -24.0, 24.0),
        "pedal_mix" => (&mut state.pedal_mix, 0.0, 1.0),
        "pedal_level_db" => (&mut state.pedal_level_db, -24.0, 24.0),
        "amp_bass_db" => (&mut state.amp_bass_db, -12.0, 12.0),
        "amp_mid_db" => (&mut state.amp_mid_db, -12.0, 12.0),
        "amp_treble_db" => (&mut state.amp_treble_db, -12.0, 12.0),
        "amp_volume_db" => (&mut state.amp_volume_db, -24.0, 24.0),
        "amp_tight_hz" => (
            &mut state.amp_tight_hz,
            pedal_engine::dsp::TIGHT_MIN_HZ,
            pedal_engine::dsp::TIGHT_MAX_HZ,
        ),
        "cabinet_gain_db" => (&mut state.cabinet_gain_db, -24.0, 12.0),
        "cabinet_low_cut_hz" => (&mut state.cabinet_low_cut_hz, 20.0, 500.0),
        "cabinet_high_cut_hz" => (&mut state.cabinet_high_cut_hz, 1_000.0, 20_000.0),
        "gate_threshold_db" => (
            &mut state.gate_threshold_db,
            pedal_engine::gate::MIN_THRESHOLD_DB,
            -10.0,
        ),
        "squelch_threshold_db" => (
            &mut state.squelch_threshold_db,
            pedal_engine::squelch::THRESHOLD_RANGE_DB.0,
            pedal_engine::squelch::THRESHOLD_RANGE_DB.1,
        ),
        "dehiss_threshold_db" => (
            &mut state.dehiss_threshold_db,
            pedal_engine::dehiss::THRESHOLD_RANGE_DB.0,
            pedal_engine::dehiss::THRESHOLD_RANGE_DB.1,
        ),
        "dehiss_frequency_hz" => (
            &mut state.dehiss_frequency_hz,
            pedal_engine::dehiss::FREQUENCY_RANGE_HZ.0,
            pedal_engine::dehiss::FREQUENCY_RANGE_HZ.1,
        ),
        "dehiss_range_db" => (
            &mut state.dehiss_range_db,
            pedal_engine::dehiss::RANGE_RANGE_DB.0,
            pedal_engine::dehiss::RANGE_RANGE_DB.1,
        ),
        "gate_hysteresis_db" => (&mut state.gate_hysteresis_db, 0.0, 30.0),
        "gate_hold_ms" => (&mut state.gate_hold_ms, 0.0, 500.0),
        "gate_release_ms" => (&mut state.gate_release_ms, 5.0, 1_000.0),
        "gate_range_db" => (&mut state.gate_range_db, 0.0, 90.0),
        "compressor_threshold_db" => (&mut state.compressor_threshold_db, -48.0, 0.0),
        "compressor_ratio" => (&mut state.compressor_ratio, 1.0, 20.0),
        "compressor_attack_ms" => (
            &mut state.compressor_attack_ms,
            pedal_engine::compressor::ATTACK_RANGE_MS.0,
            pedal_engine::compressor::ATTACK_RANGE_MS.1,
        ),
        "compressor_release_ms" => (
            &mut state.compressor_release_ms,
            pedal_engine::compressor::RELEASE_RANGE_MS.0,
            pedal_engine::compressor::RELEASE_RANGE_MS.1,
        ),
        "compressor_level_db" => (
            &mut state.compressor_level_db,
            pedal_engine::compressor::LEVEL_RANGE_DB.0,
            pedal_engine::compressor::LEVEL_RANGE_DB.1,
        ),
        "compressor_mix" => (&mut state.compressor_mix, 0.0, 1.0),
        "eq_low_db" => (&mut state.eq_low_db, -12.0, 12.0),
        "eq_mid_db" => (&mut state.eq_mid_db, -12.0, 12.0),
        "eq_high_db" => (&mut state.eq_high_db, -12.0, 12.0),
        "chorus_rate_hz" => (&mut state.chorus_rate_hz, 0.05, 8.0),
        "chorus_depth" => (&mut state.chorus_depth, 0.0, 1.0),
        "chorus_mix" => (&mut state.chorus_mix, 0.0, 1.0),
        "flanger_rate_hz" => (&mut state.flanger_rate_hz, 0.05, 5.0),
        "flanger_depth" => (&mut state.flanger_depth, 0.0, 1.0),
        "flanger_feedback" => (&mut state.flanger_feedback, -0.85, 0.85),
        "flanger_mix" => (&mut state.flanger_mix, 0.0, 1.0),
        "tremolo_rate_hz" => (&mut state.tremolo_rate_hz, 0.1, 18.0),
        "tremolo_depth" => (&mut state.tremolo_depth, 0.0, 1.0),
        "octaver_mix" => (&mut state.octaver_mix, 0.0, 1.0),
        "delay_time_ms" => (&mut state.delay_time_ms, 20.0, 1_800.0),
        "delay_feedback" => (&mut state.delay_feedback, 0.0, 0.92),
        "delay_mix" => (&mut state.delay_mix, 0.0, 1.0),
        "reverb_decay_seconds" => (&mut state.reverb_decay_seconds, 0.3, 12.0),
        "reverb_mix" => (&mut state.reverb_mix, 0.0, 1.0),
        _ => return Err("unknown control"),
    };
    if !(minimum..=maximum).contains(&value)
        && !(value == 0.0
            && matches!(
                control,
                "cabinet_low_cut_hz" | "cabinet_high_cut_hz" | "amp_tight_hz"
            ))
    {
        return Err("control value is outside its supported range");
    }
    *target = value;
    if control == "input_reference_dbu" {
        state.input_reference_set = true;
    }
    Ok(())
}

fn valid_slot(request: &Request) -> Option<&str> {
    let slot = request
        .payload
        .get("slot")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("amp");
    (slot == "pre" || slot == "amp").then_some(slot)
}

fn state_response(id: String, state: &EngineState) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "state",
        payload: state.snapshot(),
    }
}

fn presets_response(id: String, presets: &PresetStore) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "presets",
        payload: serde_json::json!({
            "presets": presets.presets.iter().map(|preset| serde_json::json!({
                "id": preset.id,
                "name": preset.name,
                "pre_model": preset.rig.pre_model,
                "model": preset.rig.model,
            })).collect::<Vec<_>>(),
        }),
    }
}

/// Two-step noise-floor LEARN: `{"phase":"start"}`, ~2 s without playing,
/// then `{"phase":"finish"}` sets the input squelch (device setting) and the
/// gate threshold (preset) just above the interface's measured noise floor.
fn noise_floor_learn(context: &ControlContext, request: Request) -> Response {
    let effects = &context.dsp_parameters.effects;
    match request
        .payload
        .get("phase")
        .and_then(serde_json::Value::as_str)
    {
        Some("start") => {
            effects.start_floor_learn();
            let state = context.state.lock().expect("state");
            state_response(request.id, &state)
        }
        Some("finish") => {
            let mut state = context.state.lock().expect("state");
            let Some((squelch_db, gate_db)) = effects.finish_floor_learn(state.gate_hysteresis_db)
            else {
                return control_error(
                    request.id,
                    "noise_floor_learn_failed",
                    "No input was measured. Start LEARN with the engine running.",
                );
            };
            state.squelch_threshold_db = squelch_db;
            if state.gate_threshold_db != gate_db {
                state.gate_threshold_db = gate_db;
                mark_preset_dirty(&mut state);
            }
            context.dsp_parameters.update(&state);
            match pedal_engine::state::save_atomic(&context.state_path, &state) {
                Ok(()) => state_response(request.id, &state),
                Err(error) => control_error(request.id, "save_failed", &error.to_string()),
            }
        }
        _ => control_error(
            request.id,
            "invalid_payload",
            "noise_floor_learn requires phase 'start' or 'finish'",
        ),
    }
}

/// Two-step LEARN for the dehiss threshold, so no request ever blocks:
/// `{"phase":"start"}` begins measuring the idle hiss floor (the player stays
/// silent ~2 s), `{"phase":"finish"}` sets the threshold just above it.
fn dehiss_learn(context: &ControlContext, request: Request) -> Response {
    let effects = &context.dsp_parameters.effects;
    match request
        .payload
        .get("phase")
        .and_then(serde_json::Value::as_str)
    {
        Some("start") => {
            effects.start_dehiss_learn();
            let state = context.state.lock().expect("state");
            state_response(request.id, &state)
        }
        Some("finish") => {
            let Some(threshold) = effects.finish_dehiss_learn() else {
                return control_error(
                    request.id,
                    "dehiss_learn_failed",
                    "No audio was measured. Start LEARN with the engine running.",
                );
            };
            let mut state = context.state.lock().expect("state");
            state.dehiss_threshold_db = threshold;
            mark_preset_dirty(&mut state);
            context.dsp_parameters.update(&state);
            match pedal_engine::state::save_atomic(&context.state_path, &state) {
                Ok(()) => state_response(request.id, &state),
                Err(error) => control_error(request.id, "save_failed", &error.to_string()),
            }
        }
        _ => control_error(
            request.id,
            "invalid_payload",
            "dehiss_learn requires phase 'start' or 'finish'",
        ),
    }
}

fn meters_response(
    id: String,
    telemetry: &RealtimeTelemetry,
    tuner_telemetry: &TunerTelemetry,
) -> Response {
    let snapshot = telemetry.snapshot();
    let (tuner_hz, tuner_confidence) = tuner_telemetry.snapshot();
    let (audio_status, audio_error) = telemetry.audio_status();
    let to_db = |peak: f32| 20.0 * peak.max(0.000_001).log10();
    Response {
        api: API_VERSION,
        id,
        message_type: "meters",
        payload: serde_json::json!({
            "input_db": to_db(snapshot.input_peak),
            "output_db": to_db(snapshot.output_peak),
            "clipped": snapshot.clipped,
            "rig_limiter_reduction_db": snapshot.rig_limiter_reduction_db,
            "output_limiter_reduction_db": snapshot.output_limiter_reduction_db,
            "cpu_percent": snapshot.cpu_percent,
            "worst_cpu_percent": snapshot.worst_cpu_percent,
            "deadline_misses": snapshot.deadline_misses,
            "audio_status": audio_status,
            "audio_error": audio_error,
            "xruns": snapshot.xruns,
            // SCHED_FIFO priorities; 0 means normal (non-realtime) scheduling.
            "audio_realtime_priority": pedal_engine::realtime::audio_priority(),
            "dsp_worker_realtime_priority": pedal_engine::realtime::worker_priority(),
            "tuner_hz": tuner_hz,
            "tuner_confidence": tuner_confidence,
        }),
    }
}

fn preview_response(id: String, path: &Path, duration_seconds: f32) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "preview",
        payload: serde_json::json!({
            "path": path,
            "duration_seconds": duration_seconds,
        }),
    }
}

fn riff_response(id: String, riff: &SavedRiff) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "riff",
        payload: serde_json::to_value(riff).expect("riff metadata serializes"),
    }
}

fn riffs_response(id: String, riffs: &[SavedRiff]) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "riffs",
        payload: serde_json::json!({"riffs": riffs}),
    }
}

fn tempo_response(id: String, estimate: &TempoEstimate) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "tempo_estimate",
        payload: serde_json::to_value(estimate).expect("tempo estimate serializes"),
    }
}

fn waveform_response(id: String, peaks: &[f32]) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "riff_waveform",
        payload: serde_json::json!({"peaks": peaks}),
    }
}

fn riff_buffer_status_response(
    id: String,
    available_seconds: f32,
    capacity_seconds: f32,
    dropped_frames: u64,
) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "riff_buffer_status",
        payload: serde_json::json!({
            "available_seconds": available_seconds,
            "capacity_seconds": capacity_seconds,
            "dropped_frames": dropped_frames,
        }),
    }
}

fn control_error(id: String, code: &str, message: &str) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "error",
        payload: serde_json::json!({"code": code, "message": message}),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn alignment_switch_is_validated_saved_and_measurement_cleared() {
        let mut state = EngineState::default();
        assert!(state.pedal_align_enabled);
        assert!(set_control(&mut state, "pedal_align_enabled", 0.5).is_err());
        assert!(set_control(&mut state, "pedal_align_enabled", f32::NAN).is_err());
        set_control(&mut state, "pedal_align_enabled", 0.0).unwrap();
        let preset = pedal_engine::presets::PresetRig::from_state(&state);
        let mut restored = EngineState::default();
        preset.apply_to(&mut restored);
        assert!(!restored.pedal_align_enabled);
        let old: EngineState = serde_json::from_str("{}").unwrap();
        assert!(old.pedal_align_enabled);
        let mailboxes = mailboxes();
        let path = example_model();
        let request=serde_json::json!({"api":1,"id":"alignment","type":"select_model","payload":{"slot":"pre","path":path}}).to_string();
        assert_eq!(
            handle(&mut state, &request, &mailboxes).message_type,
            "state"
        );
        assert!(state.pedal_alignment.is_some());
        let disk = serde_json::to_string(&state).unwrap();
        assert!(
            serde_json::from_str::<EngineState>(&disk)
                .unwrap()
                .pedal_alignment
                .is_none()
        );
        assert!(state.snapshot()["pedal_alignment"].is_object());
        let clear = r#"{"api":1,"id":"clear","type":"clear_model","payload":{"slot":"pre"}}"#;
        assert_eq!(handle(&mut state, clear, &mailboxes).message_type, "state");
        assert!(state.pedal_alignment.is_none());
    }
    #[test]
    fn cabinet_cut_controls_accept_off_and_valid_ranges() {
        let mut state = EngineState::default();
        for (name, min, max) in [
            ("cabinet_low_cut_hz", 20.0, 500.0),
            ("cabinet_high_cut_hz", 1000.0, 20000.0),
        ] {
            for value in [0.0, min, max] {
                assert!(set_control(&mut state, name, value).is_ok());
            }
            for value in [-1.0, 1.0, min - 1.0, max + 1.0, f32::NAN, f32::INFINITY] {
                assert!(set_control(&mut state, name, value).is_err());
            }
        }
    }

    #[test]
    fn nam_eq_controls_are_validated_and_mark_presets_dirty() {
        let mut state = EngineState {
            preset_id: Some("eq-test".into()),
            ..Default::default()
        };
        let mailboxes = mailboxes();
        let command = |key: &str, value: f32| {
            serde_json::json!({
            "api": 1, "id": "eq", "type": "set_control", "payload": {"control":key, "value":value}
        }).to_string()
        };
        for (key, value) in [("pedal_eq_pre", 1.0), ("amp_eq_band_4_gain_db", -7.5)] {
            assert_eq!(
                handle(&mut state, &command(key, value), &mailboxes).message_type,
                "state"
            );
        }
        assert!(state.pedal_eq.pre);
        assert_eq!(state.amp_eq.bands[4].gain_db, -7.5);
        assert!(state.preset_dirty);
        let before = state.clone();
        assert_eq!(
            handle(&mut state, &command("amp_eq_band_4_q", 0.0), &mailboxes).message_type,
            "error"
        );
        assert_eq!(state, before);
        assert_eq!(state.snapshot()["amp_eq"]["bands"][4]["gain_db"], -7.5);
    }
    #[test]
    fn nam_quality_validation_and_failed_preparation_leave_the_rig_untouched() {
        let mut state = EngineState {
            pre_model: Some(example_model().to_string_lossy().into_owned()),
            model: Some("/missing/amp.nam".into()),
            ..Default::default()
        };
        let mailboxes = mailboxes();
        let request = r#"{"api":1,"id":"size","type":"set_control","payload":{"control":"nam_quality","value":0}}"#;
        let before = state.clone();
        assert_eq!(
            handle(&mut state, request, &mailboxes).message_type,
            "error"
        );
        assert_eq!(state, before);
        let mut active = AmpProcessor::Clean;
        assert!(!mailboxes.pre.swap_pending(&mut active));
        assert!(!mailboxes.amp.swap_pending(&mut active));
        state.model = state.pre_model.clone();
        assert_eq!(
            handle(&mut state, request, &mailboxes).message_type,
            "state"
        );
        assert_eq!(state.nam_quality, 0);
        assert!(mailboxes.pre.swap_pending(&mut active));
        assert!(matches!(active, AmpProcessor::Nam(_)));
        assert!(mailboxes.amp.swap_pending(&mut active));
        for (name, maximum) in [("nam_quality", 1.0), ("render_nam_quality", 2.0)] {
            for invalid in [-1.0, 0.5, maximum + 1.0, f32::NAN, f32::INFINITY] {
                assert!(set_control(&mut state, name, invalid).is_err());
            }
        }
    }

    #[test]
    fn dehiss_controls_validate_ranges_and_round_trip_in_presets() {
        let mut state = pedal_engine::EngineState::default();
        assert!(!state.dehiss_enabled);
        for (name, min, max) in [
            ("dehiss_threshold_db", -100.0, -30.0),
            ("dehiss_frequency_hz", 1_500.0, 8_000.0),
            ("dehiss_range_db", 0.0, 40.0),
        ] {
            super::set_control(&mut state, name, min).unwrap();
            super::set_control(&mut state, name, max).unwrap();
            for invalid in [min - 1.0, max + 1.0, f32::NAN] {
                assert!(
                    super::set_control(&mut state, name, invalid).is_err(),
                    "{name}"
                );
            }
        }
        state.dehiss_enabled = true;
        state.dehiss_threshold_db = -58.0;
        let rig = pedal_engine::presets::PresetRig::from_state(&state);
        let mut restored = pedal_engine::EngineState::default();
        rig.apply_to(&mut restored);
        assert!(restored.dehiss_enabled);
        assert_eq!(restored.dehiss_threshold_db, -58.0);
        let legacy: pedal_engine::presets::PresetRig = serde_json::from_str("{}").unwrap();
        assert!(!legacy.dehiss_enabled, "old presets keep dehiss off");
        assert_eq!(state.snapshot()["dehiss_threshold_db"], -58.0);
    }

    #[test]
    fn amp_tight_accepts_off_or_its_range_and_round_trips_in_presets() {
        let mut state = pedal_engine::EngineState::default();
        assert_eq!(state.amp_tight_hz, 0.0);
        for valid in [0.0, 20.0, 150.0, 300.0] {
            super::set_control(&mut state, "amp_tight_hz", valid).unwrap();
            assert_eq!(state.amp_tight_hz, valid);
        }
        for invalid in [10.0, 301.0, -5.0, f32::NAN] {
            assert!(super::set_control(&mut state, "amp_tight_hz", invalid).is_err());
        }
        state.amp_tight_hz = 120.0;
        let rig = pedal_engine::presets::PresetRig::from_state(&state);
        let mut restored = pedal_engine::EngineState::default();
        rig.apply_to(&mut restored);
        assert_eq!(restored.amp_tight_hz, 120.0);
        let legacy: pedal_engine::presets::PresetRig = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.amp_tight_hz, 0.0, "old presets keep TIGHT off");
        assert_eq!(state.snapshot()["amp_tight_hz"], 120.0);
    }

    #[test]
    fn gate_controls_validate_ranges_and_report_snapshot() {
        let mut state = pedal_engine::EngineState::default();
        for (name, min, max) in [
            ("gate_hysteresis_db", 0.0, 30.0),
            ("gate_hold_ms", 0.0, 500.0),
            ("gate_release_ms", 5.0, 1000.0),
            ("gate_range_db", 0.0, 90.0),
        ] {
            super::set_control(&mut state, name, min).unwrap();
            super::set_control(&mut state, name, max).unwrap();
            for invalid in [min - 1.0, max + 1.0, f32::NAN, f32::INFINITY] {
                assert!(super::set_control(&mut state, name, invalid).is_err());
            }
            assert_eq!(state.snapshot()[name].as_f64(), Some(f64::from(max)));
        }
    }

    #[test]
    fn octave_mode_control_rejects_invalid_values() {
        let mut state = pedal_engine::EngineState::default();
        super::set_control(&mut state, "octaver_clean", 1.0).unwrap();
        assert!(state.octaver_clean);
        for invalid in [-1.0, 0.5, 2.0, f32::NAN] {
            assert!(super::set_control(&mut state, "octaver_clean", invalid).is_err());
            assert!(state.octaver_clean);
        }
        super::set_control(&mut state, "octaver_clean", 0.0).unwrap();
        assert!(!state.octaver_clean);
    }
    use super::*;

    fn example_model() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/NeuralAmpModelerCore/example_models/A2.nam")
            .canonicalize()
            .expect("bundled A2 model")
    }

    fn mailboxes() -> ModelMailboxes {
        ModelMailboxes {
            pre: Arc::new(SwapMailbox::default()),
            amp: Arc::new(SwapMailbox::default()),
            cabinet: Arc::new(SwapMailbox::default()),
        }
    }

    fn handle(state: &mut EngineState, line: &str, mailboxes: &ModelMailboxes) -> Response {
        handle_control_line(state, line, mailboxes, &mut PresetStore::default())
    }

    #[test]
    fn valid_model_is_prepared_before_state_changes() {
        let mailboxes = mailboxes();
        let mut state = EngineState::default();
        let request = serde_json::json!({
            "api": 1,
            "id": "select-1",
            "type": "select_model",
            "payload": {"path": example_model(), "preset": "Test A2"}
        })
        .to_string();

        let response = handle(&mut state, &request, &mailboxes);

        assert_eq!(response.message_type, "state");
        assert_eq!(state.preset, "Test A2");
        assert_eq!(state.model.as_deref(), example_model().to_str());
    }

    #[test]
    fn invalid_model_does_not_replace_current_state() {
        let mailboxes = mailboxes();
        let mut state = EngineState {
            preset: "Known good".to_owned(),
            model: Some("/var/lib/pedal/models/known-good.nam".to_owned()),
            ..EngineState::default()
        };
        let before = state.clone();
        let request = serde_json::json!({
            "api": 1,
            "id": "select-2",
            "type": "select_model",
            "payload": {"path": "/does/not/exist.nam", "preset": "Bad"}
        })
        .to_string();

        let response = handle(&mut state, &request, &mailboxes);

        assert_eq!(response.message_type, "error");
        assert_eq!(response.payload["code"], "invalid_model_path");
        assert_eq!(state, before);
    }

    #[test]
    fn pedal_slot_is_prepared_independently_from_amp_slot() {
        let mailboxes = mailboxes();
        let mut state = EngineState {
            model: Some("/var/lib/pedal/models/amp.nam".to_owned()),
            ..EngineState::default()
        };
        let request = serde_json::json!({
            "api": 1,
            "id": "select-pre",
            "type": "select_model",
            "payload": {"path": example_model(), "preset": "Drive + Amp", "slot": "pre"}
        })
        .to_string();

        let response = handle(&mut state, &request, &mailboxes);

        assert_eq!(response.message_type, "state");
        assert_eq!(state.pre_model.as_deref(), example_model().to_str());
        assert_eq!(
            state.model.as_deref(),
            Some("/var/lib/pedal/models/amp.nam")
        );
        assert!(!state.pre_bypassed);
    }

    #[test]
    fn slots_can_be_bypassed_and_cleared_independently() {
        let mailboxes = mailboxes();
        let mut state = EngineState {
            pre_model: Some("/var/lib/pedal/models/drive.nam".to_owned()),
            model: Some("/var/lib/pedal/models/amp.nam".to_owned()),
            ..EngineState::default()
        };

        let bypass = r#"{"api":1,"id":"bypass-pre","type":"set_slot_bypass","payload":{"slot":"pre","bypassed":true}}"#;
        let response = handle(&mut state, bypass, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert!(state.pre_bypassed);
        assert!(!state.amp_bypassed);

        let clear = r#"{"api":1,"id":"clear-pre","type":"clear_model","payload":{"slot":"pre"}}"#;
        let response = handle(&mut state, clear, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert_eq!(state.pre_model, None);
        assert!(!state.pre_bypassed);
        assert_eq!(
            state.model.as_deref(),
            Some("/var/lib/pedal/models/amp.nam")
        );
    }

    #[test]
    fn adjustable_controls_are_validated_and_returned_in_state() {
        let mailboxes = mailboxes();
        let mut state = EngineState::default();
        let drive = r#"{"api":1,"id":"drive","type":"set_control","payload":{"control":"pedal_drive_db","value":12.5}}"#;
        let response = handle(&mut state, drive, &mailboxes);

        assert_eq!(response.message_type, "state");
        assert_eq!(state.pedal_drive_db, 12.5);
        assert_eq!(response.payload["pedal_drive_db"], 12.5);

        let invalid = r#"{"api":1,"id":"bass","type":"set_control","payload":{"control":"amp_bass_db","value":20}}"#;
        let response = handle(&mut state, invalid, &mailboxes);
        assert_eq!(response.message_type, "error");
        assert_eq!(response.payload["code"], "invalid_control");
        assert_eq!(state.amp_bass_db, 0.0);
    }

    #[test]
    fn effect_switches_controls_and_looper_commands_are_authoritative() {
        let mailboxes = mailboxes();
        let mut state = EngineState::default();
        let bypass = r#"{"api":1,"id":"chorus","type":"set_effect_bypass","payload":{"effect":"chorus","bypassed":false}}"#;
        let response = handle(&mut state, bypass, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert!(state.chorus_enabled);

        let rate = r#"{"api":1,"id":"rate","type":"set_control","payload":{"control":"chorus_rate_hz","value":2.5}}"#;
        handle(&mut state, rate, &mailboxes);
        assert_eq!(state.chorus_rate_hz, 2.5);

        let record = r#"{"api":1,"id":"loop","type":"looper_action","payload":{"action":"record","bpm":120,"bars":2}}"#;
        let response = handle(&mut state, record, &mailboxes);
        assert_eq!(response.payload["looper_mode"], "count_in");
        assert_eq!(state.looper_mode, "count_in");
        assert_eq!(state.looper_bpm, 120);
        assert_eq!(state.looper_bars, 2);
    }

    #[test]
    fn optional_pedals_can_be_added_and_a_new_preset_resets_the_board() {
        let mailboxes = mailboxes();
        let mut state = EngineState {
            pre_model: Some("/var/lib/pedal/models/drive.nam".to_owned()),
            model: Some("/var/lib/pedal/models/amp.nam".to_owned()),
            eq_visible: true,
            delay_visible: true,
            looper_visible: true,
            sample_rate: 44_100,
            buffer_frames: 128,
            calibration_enabled: true,
            input_reference_dbu: 12.0,
            input_reference_set: true,
            input_trim_db: -3.0,
            quality_profile: 0,
            render_quality_profile: 2,
            nam_quality: 0,
            render_nam_quality: 1,
            rig_level_db: -6.0,
            ..EngineState::default()
        };

        let add = r#"{"api":1,"id":"chorus","type":"set_pedal_visible","payload":{"pedal":"chorus","visible":true}}"#;
        let response = handle(&mut state, add, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert!(state.chorus_visible);

        let new_preset = r#"{"api":1,"id":"new","type":"new_preset","payload":{}}"#;
        let response = handle(&mut state, new_preset, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert_eq!(state.preset, "New preset");
        assert!(state.pre_model.is_none());
        assert!(state.model.is_none());
        assert!(!state.eq_visible);
        assert!(!state.chorus_visible);
        assert!(!state.delay_visible);
        assert!(!state.looper_visible);
        assert_eq!(state.sample_rate, 44_100);
        assert_eq!(state.buffer_frames, 128);
        assert!(state.calibration_enabled && state.input_reference_set);
        assert_eq!(state.input_reference_dbu, 12.0);
        assert_eq!(state.input_trim_db, -3.0);
        assert_eq!(state.quality_profile, 0);
        assert_eq!(state.render_quality_profile, 2);
        assert_eq!(state.nam_quality, 0);
        assert_eq!(state.render_nam_quality, 1);
        assert_eq!(state.rig_level_db, 0.0);
    }

    #[test]
    fn tuner_can_be_enabled_without_changing_the_saved_rig() {
        let mailboxes = mailboxes();
        let mut state = EngineState::default();
        let request =
            r#"{"api":1,"id":"tuner","type":"set_tuner_enabled","payload":{"enabled":true}}"#;
        let response = handle(&mut state, request, &mailboxes);
        assert_eq!(response.message_type, "state");
        assert!(state.tuner_enabled);
    }

    #[test]
    fn calibration_requires_explicit_reference_and_quality_is_bounded() {
        let mut state = EngineState::default();
        assert!(set_control(&mut state, "calibration_enabled", 1.0).is_err());
        assert!(set_control(&mut state, "input_reference_dbu", 9.5).is_ok());
        assert!(set_control(&mut state, "calibration_enabled", 1.0).is_ok());
        assert!(state.input_reference_set && state.calibration_enabled);
        assert!(set_control(&mut state, "quality_profile", 2.0).is_err());
        assert!(set_control(&mut state, "render_quality_profile", 2.0).is_ok());
        assert!(set_control(&mut state, "render_quality_profile", 1.5).is_err());
        assert!(set_control(&mut state, "rig_level_db", 13.0).is_err());
    }

    #[test]
    fn nam_slot_normalization_and_gain_controls_validate_reference_ranges() {
        let mut state = EngineState::default();
        for name in ["pedal_normalize_enabled", "amp_normalize_enabled"] {
            assert_eq!(state.snapshot()[name], true);
            set_control(&mut state, name, 0.0).unwrap();
            assert_eq!(state.snapshot()[name], false);
            for invalid in [-1.0, 0.5, 2.0, f32::NAN, f32::INFINITY] {
                assert!(set_control(&mut state, name, invalid).is_err());
                assert_eq!(state.snapshot()[name], false);
            }
            set_control(&mut state, name, 1.0).unwrap();
            assert_eq!(state.snapshot()[name], true);
        }
        for name in ["pedal_drive_db", "pedal_level_db", "amp_volume_db"] {
            for db in [-24.0, 0.0, 24.0] {
                set_control(&mut state, name, db).unwrap();
                assert_eq!(state.snapshot()[name], db);
            }
            for invalid in [-24.1, 24.1, f32::NAN, f32::INFINITY] {
                assert!(set_control(&mut state, name, invalid).is_err());
                assert_eq!(state.snapshot()[name], 24.0);
            }
        }
    }

    #[test]
    fn meter_response_reports_peaks_clipping_cpu_and_xruns() {
        let telemetry = RealtimeTelemetry::default();
        let tuner_telemetry = TunerTelemetry::default();
        telemetry.record_levels(0.5, 0.25, true);
        telemetry.record_limiter_gain(pedal_engine::dsp::LimiterStage::Rig, 0.5);
        telemetry.record_limiter_gain(pedal_engine::dsp::LimiterStage::Output, 0.25);
        telemetry.record_cpu_percent(7.5);
        telemetry.record_xrun();

        let response = meters_response("meters-1".to_owned(), &telemetry, &tuner_telemetry);

        assert_eq!(response.message_type, "meters");
        assert!((response.payload["input_db"].as_f64().expect("input") + 6.0206).abs() < 0.001);
        assert_eq!(response.payload["clipped"], true);
        assert_eq!(response.payload["cpu_percent"], 7.5);
        assert_eq!(response.payload["xruns"], 1);
        assert!(
            (response.payload["rig_limiter_reduction_db"]
                .as_f64()
                .unwrap()
                - 6.0206)
                .abs()
                < 0.001
        );
        assert!(
            (response.payload["output_limiter_reduction_db"]
                .as_f64()
                .unwrap()
                - 12.0412)
                .abs()
                < 0.001
        );
        let next = meters_response("meters-2".to_owned(), &telemetry, &tuner_telemetry);
        assert_eq!(next.payload["rig_limiter_reduction_db"], 0.0);
        assert_eq!(next.payload["output_limiter_reduction_db"], 0.0);
    }

    #[test]
    fn presets_can_be_saved_listed_renamed_and_deleted() {
        let mailboxes = mailboxes();
        let mut state = EngineState {
            pedal_mix: 0.6,
            amp_bass_db: 2.0,
            ..EngineState::default()
        };
        let mut presets = PresetStore::default();

        let save = r#"{"api":1,"id":"save","type":"save_preset","payload":{"name":"  Crunch  "}}"#;
        let response = handle_control_line(&mut state, save, &mailboxes, &mut presets);
        assert_eq!(response.message_type, "state");
        assert_eq!(state.preset, "Crunch");
        assert_eq!(state.preset_id.as_deref(), Some("preset-1"));

        let list = r#"{"api":1,"id":"list","type":"list_presets","payload":{}}"#;
        let response = handle_control_line(&mut state, list, &mailboxes, &mut presets);
        assert_eq!(response.message_type, "presets");
        assert_eq!(response.payload["presets"][0]["name"], "Crunch");

        let rename = r#"{"api":1,"id":"rename","type":"rename_preset","payload":{"id":"preset-1","name":"Lead"}}"#;
        handle_control_line(&mut state, rename, &mailboxes, &mut presets);
        assert_eq!(state.preset, "Lead");

        let delete =
            r#"{"api":1,"id":"delete","type":"delete_preset","payload":{"id":"preset-1"}}"#;
        handle_control_line(&mut state, delete, &mailboxes, &mut presets);
        assert!(presets.presets.is_empty());
        assert_eq!(state.preset_id, None);
    }

    #[test]
    fn preset_load_prepares_all_models_before_replacing_the_rig() {
        let mailboxes = mailboxes();
        let model = example_model().to_string_lossy().into_owned();
        let saved = EngineState {
            preset: "Saved".to_owned(),
            pre_model: Some(model.clone()),
            model: Some(model),
            pedal_drive_db: 11.0,
            amp_treble_db: 4.0,
            ..EngineState::default()
        };
        let mut presets = PresetStore::default();
        presets.save_current("Saved".to_owned(), &saved);
        let mut state = EngineState {
            pedal_drive_db: 0.0,
            amp_treble_db: 0.0,
            ..EngineState::default()
        };

        let load = r#"{"api":1,"id":"load","type":"load_preset","payload":{"id":"preset-1"}}"#;
        let response = handle_control_line(&mut state, load, &mailboxes, &mut presets);

        assert_eq!(response.message_type, "state");
        assert_eq!(state.preset_id.as_deref(), Some("preset-1"));
        assert!(!state.preset_dirty);
        assert_eq!(state.pedal_drive_db, 11.0);
        assert_eq!(state.amp_treble_db, 4.0);
    }

    #[test]
    fn missing_preset_model_leaves_the_current_rig_unchanged() {
        let mailboxes = mailboxes();
        let saved = EngineState {
            pre_model: Some(example_model().to_string_lossy().into_owned()),
            model: Some("/models/missing.nam".to_owned()),
            pedal_drive_db: 18.0,
            ..EngineState::default()
        };
        let mut presets = PresetStore::default();
        presets.save_current("Broken".to_owned(), &saved);
        let mut state = EngineState {
            preset: "Current".to_owned(),
            pedal_drive_db: 3.0,
            ..EngineState::default()
        };
        let before = state.clone();

        let load = r#"{"api":1,"id":"load","type":"load_preset","payload":{"id":"preset-1"}}"#;
        let response = handle_control_line(&mut state, load, &mailboxes, &mut presets);

        assert_eq!(response.message_type, "error");
        assert_eq!(response.payload["code"], "preset_model_failed");
        assert_eq!(state, before);
    }
}
