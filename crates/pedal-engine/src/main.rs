use pedal_engine::audio::{AudioConfig, probe, run_processor_with_telemetry};
use pedal_engine::cabinet::{CabinetImpulse, CabinetParameters, CabinetProcessor};
use pedal_engine::dsp::{
    AmpControlProcessor, AmpParameters, AmpProcessor, BypassProcessor, PedalControlProcessor,
    PedalParameters, RealtimeTelemetry, SafetyProcessor, SerialProcessor, SwapMailbox,
    SwappableProcessor,
};
use pedal_engine::effects::{EffectChainProcessor, EffectsParameters, LooperMode};
use pedal_engine::metronome::{MetronomeParameters, MetronomeProcessor};
use pedal_engine::presets::PresetStore;
use pedal_engine::riff_vault::{DEFAULT_SECONDS, RiffVault, RiffVaultProcessor, SavedRiff};
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
    pedal: Arc<PedalParameters>,
    amp: Arc<AmpParameters>,
    cabinet: Arc<CabinetParameters>,
    effects: Arc<EffectsParameters>,
    tuner: Arc<TunerParameters>,
    metronome: Arc<MetronomeParameters>,
}

#[derive(Clone)]
struct ControlContext {
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
            pedal: Arc::new(PedalParameters::new(
                state.pedal_drive_db,
                state.pedal_mix,
                state.pedal_level_db,
            )),
            amp: Arc::new(AmpParameters::new(
                state.amp_bass_db,
                state.amp_mid_db,
                state.amp_treble_db,
                state.amp_volume_db,
            )),
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
            .update(state.pedal_drive_db, state.pedal_mix, state.pedal_level_db);
        self.amp.update(
            state.amp_bass_db,
            state.amp_mid_db,
            state.amp_treble_db,
            state.amp_volume_db,
        );
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
    if audio_is_enabled() {
        spawn_audio_worker(
            Arc::clone(&state),
            Arc::clone(&bypass_flags),
            Arc::clone(&dsp_parameters),
            Arc::clone(&telemetry),
            Arc::clone(&tuner_telemetry),
            Arc::clone(&model_mailboxes),
            Arc::clone(&riff_vault),
            &state_path,
        );
    } else {
        eprintln!("audio disabled by PEDAL_AUDIO_ENABLED=0");
    }
    let control_context = ControlContext {
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
        let response = if let Some(request) = special_request
            && request.message_type == "get_meters"
        {
            meters_response(request.id, &context.telemetry, &context.tuner_telemetry)
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
                ),
                Err(_) => control_error(request.id, "riff_buffer_failed", "riff buffer lock poisoned"),
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
            let clean_path = request.payload.get("clean_path").and_then(serde_json::Value::as_str);
            let processed_path = request.payload.get("processed_path").and_then(serde_json::Value::as_str);
            let start = request.payload.get("trim_start_seconds").and_then(serde_json::Value::as_f64);
            let end = request.payload.get("trim_end_seconds").and_then(serde_json::Value::as_f64);
            let name = request.payload.get("name").and_then(serde_json::Value::as_str);
            let preset = context.state.lock().expect("control state lock poisoned").preset.clone();
            match (clean_path, processed_path, start, end) {
                (Some(clean), Some(processed), Some(start), Some(end)) => match pedal_engine::riff_vault::trim_saved(
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
                },
                _ => control_error(request.id, "invalid_payload", "trim_riff requires take paths and start/end seconds"),
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
            match pedal_engine::preview::render_source(
                &preview_state,
                &context.preview_path,
                source,
                processed,
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

fn spawn_audio_worker(
    state: Arc<Mutex<EngineState>>,
    bypass_flags: Arc<BypassFlags>,
    dsp_parameters: Arc<DspParameters>,
    telemetry: Arc<RealtimeTelemetry>,
    tuner_telemetry: Arc<TunerTelemetry>,
    model_mailboxes: Arc<ModelMailboxes>,
    riff_vault: Arc<Mutex<RiffVault>>,
    state_path: &Path,
) {
    let state_path = state_path.to_owned();
    thread::Builder::new()
        .name("pedal-audio".to_owned())
        .spawn(move || {
            if let Err(error) = run_audio_worker(
                &state,
                bypass_flags,
                dsp_parameters,
                telemetry,
                tuner_telemetry,
                model_mailboxes,
                riff_vault,
                &state_path,
            ) {
                eprintln!("audio engine stopped: {error}");
            }
        })
        .expect("could not start audio worker");
}

fn run_audio_worker(
    shared_state: &Arc<Mutex<EngineState>>,
    bypass_flags: Arc<BypassFlags>,
    dsp_parameters: Arc<DspParameters>,
    telemetry: Arc<RealtimeTelemetry>,
    tuner_telemetry: Arc<TunerTelemetry>,
    model_mailboxes: Arc<ModelMailboxes>,
    riff_vault: Arc<Mutex<RiffVault>>,
    state_path: &Path,
) -> Result<(), String> {
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
    );
    let (amp, amp_failed) = load_restored_model(
        "amp",
        amp_model_path.as_deref(),
        negotiated.sample_rate,
        negotiated.period_frames,
    );
    let (cabinet, cabinet_failed) =
        load_restored_cabinet(cabinet_ir_path.as_deref(), negotiated.sample_rate);
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
    let pre = SwappableProcessor::new(pre, Arc::clone(&model_mailboxes.pre));
    let pre = PedalControlProcessor::new(
        pre,
        Arc::clone(&dsp_parameters.pedal),
        negotiated.period_frames as usize,
    );
    let pre = BypassProcessor::new(pre, Arc::clone(&bypass_flags.pre));
    let amp = SwappableProcessor::new(amp, Arc::clone(&model_mailboxes.amp));
    let amp =
        AmpControlProcessor::new(amp, Arc::clone(&dsp_parameters.amp), negotiated.sample_rate);
    let amp = BypassProcessor::new(amp, Arc::clone(&bypass_flags.amp));
    let chain = SerialProcessor::new(pre, amp);
    let chain = CabinetProcessor::new(
        chain,
        cabinet,
        Arc::clone(&dsp_parameters.cabinet),
        Arc::clone(&model_mailboxes.cabinet),
        negotiated.sample_rate,
    );
    let chain = EffectChainProcessor::new(
        chain,
        Arc::clone(&dsp_parameters.effects),
        negotiated.sample_rate,
        30,
    );
    let chain = BypassProcessor::new(chain, Arc::clone(&bypass_flags.all));
    let chain = TunerTapProcessor::new(
        chain,
        Arc::clone(&dsp_parameters.tuner),
        tuner_telemetry,
        negotiated.sample_rate,
    );
    let chain = MetronomeProcessor::new(
        chain,
        Arc::clone(&dsp_parameters.metronome),
        negotiated.sample_rate,
    );
    let processor = SafetyProcessor::new(chain, Arc::clone(&telemetry), negotiated.sample_rate);
    let mut processor = RiffVaultProcessor::new(processor, riff_vault);
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
) -> (AmpProcessor, bool) {
    let Some(path) = path else {
        return (AmpProcessor::Clean, false);
    };
    match nam_bridge::NamModel::load(path, f64::from(sample_rate), buffer_frames as usize) {
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

fn load_restored_cabinet(path: Option<&str>, sample_rate: u32) -> (Option<CabinetImpulse>, bool) {
    let Some(path) = path else {
        return (None, false);
    };
    match CabinetImpulse::load(Path::new(path), sample_rate) {
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
        let pre = match prepare_preset_model(preset.rig.pre_model.as_deref(), state) {
            Ok(model) => model,
            Err(error) => return control_error(request.id, "preset_model_failed", &error),
        };
        let amp = match prepare_preset_model(preset.rig.model.as_deref(), state) {
            Ok(model) => model,
            Err(error) => return control_error(request.id, "preset_model_failed", &error),
        };
        let cabinet = match prepare_preset_cabinet(preset.rig.cabinet_ir.as_deref(), state) {
            Ok(cabinet) => cabinet,
            Err(error) => return control_error(request.id, "preset_cabinet_failed", &error),
        };
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
        let impulse = match CabinetImpulse::load(path, state.sample_rate) {
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
        if let Err(message) = set_control(state, control, value) {
            return control_error(request.id, "invalid_control", message);
        }
        mark_preset_dirty(state);
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

    let prepared = match nam_bridge::NamModel::load(
        path,
        f64::from(state.sample_rate),
        state.buffer_frames.max(128) as usize,
    ) {
        Ok(model) => model,
        Err(error) => {
            return control_error(request.id, "model_load_failed", &error.to_string());
        }
    };
    if slot == "pre" {
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

fn prepare_preset_model(path: Option<&str>, state: &EngineState) -> Result<AmpProcessor, String> {
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
    nam_bridge::NamModel::load(
        path_ref,
        f64::from(state.sample_rate),
        state.buffer_frames.max(128) as usize,
    )
    .map(AmpProcessor::Nam)
    .map_err(|error| error.to_string())
}

fn prepare_preset_cabinet(
    path: Option<&str>,
    state: &EngineState,
) -> Result<Option<CabinetImpulse>, String> {
    path.map(|path| CabinetImpulse::load(Path::new(path), state.sample_rate))
        .transpose()
}

fn set_control(state: &mut EngineState, control: &str, value: f32) -> Result<(), &'static str> {
    let (target, minimum, maximum) = match control {
        "pedal_drive_db" => (&mut state.pedal_drive_db, 0.0, 24.0),
        "pedal_mix" => (&mut state.pedal_mix, 0.0, 1.0),
        "pedal_level_db" => (&mut state.pedal_level_db, -24.0, 12.0),
        "amp_bass_db" => (&mut state.amp_bass_db, -12.0, 12.0),
        "amp_mid_db" => (&mut state.amp_mid_db, -12.0, 12.0),
        "amp_treble_db" => (&mut state.amp_treble_db, -12.0, 12.0),
        "amp_volume_db" => (&mut state.amp_volume_db, -24.0, 12.0),
        "cabinet_gain_db" => (&mut state.cabinet_gain_db, -24.0, 12.0),
        "cabinet_low_cut_hz" => (&mut state.cabinet_low_cut_hz, 20.0, 500.0),
        "cabinet_high_cut_hz" => (&mut state.cabinet_high_cut_hz, 1_000.0, 20_000.0),
        "gate_threshold_db" => (&mut state.gate_threshold_db, -80.0, -10.0),
        "compressor_threshold_db" => (&mut state.compressor_threshold_db, -48.0, 0.0),
        "compressor_ratio" => (&mut state.compressor_ratio, 1.0, 20.0),
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
    if !(minimum..=maximum).contains(&value) {
        return Err("control value is outside its supported range");
    }
    *target = value;
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

fn meters_response(
    id: String,
    telemetry: &RealtimeTelemetry,
    tuner_telemetry: &TunerTelemetry,
) -> Response {
    let snapshot = telemetry.snapshot();
    let (tuner_hz, tuner_confidence) = tuner_telemetry.snapshot();
    let to_db = |peak: f32| 20.0 * peak.max(0.000_001).log10();
    Response {
        api: API_VERSION,
        id,
        message_type: "meters",
        payload: serde_json::json!({
            "input_db": to_db(snapshot.input_peak),
            "output_db": to_db(snapshot.output_peak),
            "clipped": snapshot.clipped,
            "cpu_percent": snapshot.cpu_percent,
            "xruns": snapshot.xruns,
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

fn riff_buffer_status_response(id: String, available_seconds: f32, capacity_seconds: f32) -> Response {
    Response {
        api: API_VERSION,
        id,
        message_type: "riff_buffer_status",
        payload: serde_json::json!({
            "available_seconds": available_seconds,
            "capacity_seconds": capacity_seconds,
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
    fn meter_response_reports_peaks_clipping_cpu_and_xruns() {
        let telemetry = RealtimeTelemetry::default();
        let tuner_telemetry = TunerTelemetry::default();
        telemetry.record_levels(0.5, 0.25, true);
        telemetry.record_cpu_percent(7.5);
        telemetry.record_xrun();

        let response = meters_response("meters-1".to_owned(), &telemetry, &tuner_telemetry);

        assert_eq!(response.message_type, "meters");
        assert!((response.payload["input_db"].as_f64().expect("input") + 6.0206).abs() < 0.001);
        assert_eq!(response.payload["clipped"], true);
        assert_eq!(response.payload["cpu_percent"], 7.5);
        assert_eq!(response.payload["xruns"], 1);
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
