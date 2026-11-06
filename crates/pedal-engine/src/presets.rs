use crate::EngineState;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresetRig {
    pub pedal_align_enabled: bool,
    pub pedal_eq: crate::nam_eq::NamEqSettings,
    pub amp_eq: crate::nam_eq::NamEqSettings,
    pub rig_level_db: f32,
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
    pub amp_volume_db: f32,
    pub cabinet_enabled: bool,
    pub cabinet_ir: Option<String>,
    pub cabinet_gain_db: f32,
    pub cabinet_low_cut_hz: f32,
    pub cabinet_high_cut_hz: f32,
    pub gate_enabled: bool,
    pub gate_threshold_db: f32,
    pub gate_hold_ms: f32,
    pub gate_release_ms: f32,
    pub gate_range_db: f32,
    pub compressor_enabled: bool,
    pub compressor_threshold_db: f32,
    pub compressor_ratio: f32,
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
    pub octaver_mix: f32,
    pub octaver_clean: bool,
    pub delay_enabled: bool,
    pub delay_visible: bool,
    pub delay_time_ms: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,
    pub reverb_enabled: bool,
    pub reverb_visible: bool,
    pub looper_visible: bool,
    pub reverb_decay_seconds: f32,
    pub reverb_mix: f32,
}

impl Default for PresetRig {
    fn default() -> Self {
        Self::from_state(&EngineState::default())
    }
}

impl PresetRig {
    pub fn from_state(state: &EngineState) -> Self {
        Self {
            pedal_align_enabled: state.pedal_align_enabled,
            pedal_eq: state.pedal_eq,
            amp_eq: state.amp_eq,
            rig_level_db: state.rig_level_db,
            pre_bypassed: state.pre_bypassed,
            pre_model: state.pre_model.clone(),
            amp_bypassed: state.amp_bypassed,
            model: state.model.clone(),
            pedal_drive_db: state.pedal_drive_db,
            pedal_mix: state.pedal_mix,
            pedal_level_db: state.pedal_level_db,
            amp_bass_db: state.amp_bass_db,
            amp_mid_db: state.amp_mid_db,
            amp_treble_db: state.amp_treble_db,
            amp_volume_db: state.amp_volume_db,
            cabinet_enabled: state.cabinet_enabled,
            cabinet_ir: state.cabinet_ir.clone(),
            cabinet_gain_db: state.cabinet_gain_db,
            cabinet_low_cut_hz: state.cabinet_low_cut_hz,
            cabinet_high_cut_hz: state.cabinet_high_cut_hz,
            gate_enabled: state.gate_enabled,
            gate_threshold_db: state.gate_threshold_db,
            gate_hold_ms: state.gate_hold_ms,
            gate_release_ms: state.gate_release_ms,
            gate_range_db: state.gate_range_db,
            compressor_enabled: state.compressor_enabled,
            compressor_threshold_db: state.compressor_threshold_db,
            compressor_ratio: state.compressor_ratio,
            eq_enabled: state.eq_enabled,
            eq_visible: state.eq_visible,
            eq_low_db: state.eq_low_db,
            eq_mid_db: state.eq_mid_db,
            eq_high_db: state.eq_high_db,
            chorus_enabled: state.chorus_enabled,
            chorus_visible: state.chorus_visible,
            chorus_rate_hz: state.chorus_rate_hz,
            chorus_depth: state.chorus_depth,
            chorus_mix: state.chorus_mix,
            flanger_enabled: state.flanger_enabled,
            flanger_visible: state.flanger_visible,
            flanger_rate_hz: state.flanger_rate_hz,
            flanger_depth: state.flanger_depth,
            flanger_feedback: state.flanger_feedback,
            flanger_mix: state.flanger_mix,
            tremolo_enabled: state.tremolo_enabled,
            tremolo_visible: state.tremolo_visible,
            tremolo_rate_hz: state.tremolo_rate_hz,
            tremolo_depth: state.tremolo_depth,
            octaver_enabled: state.octaver_enabled,
            octaver_visible: state.octaver_visible,
            octaver_mix: state.octaver_mix,
            octaver_clean: state.octaver_clean,
            delay_enabled: state.delay_enabled,
            delay_visible: state.delay_visible,
            delay_time_ms: state.delay_time_ms,
            delay_feedback: state.delay_feedback,
            delay_mix: state.delay_mix,
            reverb_enabled: state.reverb_enabled,
            reverb_visible: state.reverb_visible,
            looper_visible: state.looper_visible,
            reverb_decay_seconds: state.reverb_decay_seconds,
            reverb_mix: state.reverb_mix,
        }
    }

    pub fn apply_to(&self, state: &mut EngineState) {
        state.pedal_align_enabled = self.pedal_align_enabled;
        state.pedal_eq = self.pedal_eq;
        state.amp_eq = self.amp_eq;
        state.rig_level_db = self.rig_level_db;
        state.pre_bypassed = self.pre_bypassed;
        state.pre_model.clone_from(&self.pre_model);
        state.amp_bypassed = self.amp_bypassed;
        state.model.clone_from(&self.model);
        state.pedal_drive_db = self.pedal_drive_db;
        state.pedal_mix = self.pedal_mix;
        state.pedal_level_db = self.pedal_level_db;
        state.amp_bass_db = self.amp_bass_db;
        state.amp_mid_db = self.amp_mid_db;
        state.amp_treble_db = self.amp_treble_db;
        state.amp_volume_db = self.amp_volume_db;
        state.cabinet_enabled = self.cabinet_enabled;
        state.cabinet_ir.clone_from(&self.cabinet_ir);
        state.cabinet_gain_db = self.cabinet_gain_db;
        state.cabinet_low_cut_hz = self.cabinet_low_cut_hz;
        state.cabinet_high_cut_hz = self.cabinet_high_cut_hz;
        state.gate_enabled = self.gate_enabled;
        state.gate_threshold_db = self.gate_threshold_db;
        state.gate_hold_ms = self.gate_hold_ms;
        state.gate_release_ms = self.gate_release_ms;
        state.gate_range_db = self.gate_range_db;
        state.compressor_enabled = self.compressor_enabled;
        state.compressor_threshold_db = self.compressor_threshold_db;
        state.compressor_ratio = self.compressor_ratio;
        state.eq_enabled = self.eq_enabled;
        state.eq_visible = self.eq_visible;
        state.eq_low_db = self.eq_low_db;
        state.eq_mid_db = self.eq_mid_db;
        state.eq_high_db = self.eq_high_db;
        state.chorus_enabled = self.chorus_enabled;
        state.chorus_visible = self.chorus_visible;
        state.chorus_rate_hz = self.chorus_rate_hz;
        state.chorus_depth = self.chorus_depth;
        state.chorus_mix = self.chorus_mix;
        state.flanger_enabled = self.flanger_enabled;
        state.flanger_visible = self.flanger_visible;
        state.flanger_rate_hz = self.flanger_rate_hz;
        state.flanger_depth = self.flanger_depth;
        state.flanger_feedback = self.flanger_feedback;
        state.flanger_mix = self.flanger_mix;
        state.tremolo_enabled = self.tremolo_enabled;
        state.tremolo_visible = self.tremolo_visible;
        state.tremolo_rate_hz = self.tremolo_rate_hz;
        state.tremolo_depth = self.tremolo_depth;
        state.octaver_enabled = self.octaver_enabled;
        state.octaver_visible = self.octaver_visible;
        state.octaver_mix = self.octaver_mix;
        state.octaver_clean = self.octaver_clean;
        state.delay_enabled = self.delay_enabled;
        state.delay_visible = self.delay_visible;
        state.delay_time_ms = self.delay_time_ms;
        state.delay_feedback = self.delay_feedback;
        state.delay_mix = self.delay_mix;
        state.reverb_enabled = self.reverb_enabled;
        state.reverb_visible = self.reverb_visible;
        state.looper_visible = self.looper_visible;
        state.reverb_decay_seconds = self.reverb_decay_seconds;
        state.reverb_mix = self.reverb_mix;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub rig: PresetRig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresetStore {
    pub version: u8,
    pub next_id: u64,
    pub presets: Vec<Preset>,
}

impl Default for PresetStore {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            presets: Vec::new(),
        }
    }
}

impl PresetStore {
    pub fn save_current(&mut self, name: String, state: &EngineState) -> &Preset {
        let id = format!("preset-{}", self.next_id);
        self.next_id += 1;
        self.presets.push(Preset {
            id,
            name,
            rig: PresetRig::from_state(state),
        });
        self.presets.last().expect("just inserted preset")
    }

    pub fn find(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|preset| preset.id == id)
    }

    pub fn rename(&mut self, id: &str, name: String) -> Option<&Preset> {
        let preset = self.presets.iter_mut().find(|preset| preset.id == id)?;
        preset.name = name;
        Some(preset)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.presets.len();
        self.presets.retain(|preset| preset.id != id);
        self.presets.len() != before
    }
}

pub fn load_or_default(path: &Path) -> PresetStore {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            eprintln!(
                "could not restore presets from {}: {error}; using an empty library",
                path.display()
            );
            PresetStore::default()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => PresetStore::default(),
        Err(error) => {
            eprintln!(
                "could not read presets from {}: {error}; using an empty library",
                path.display()
            );
            PresetStore::default()
        }
    }
}

pub fn save_atomic(path: &Path, store: &PresetStore) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(store).map_err(io::Error::other)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn gate_controls_round_trip_and_legacy_presets_get_defaults() {
        assert_eq!(crate::EngineState::default().gate_threshold_db, -80.0);
        let empty: super::PresetRig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.gate_threshold_db, -80.0);
        let empty_state: crate::EngineState = serde_json::from_str("{}").unwrap();
        assert_eq!(empty_state.gate_threshold_db, -80.0);
        let state = crate::EngineState {
            gate_hold_ms: 75.0,
            gate_release_ms: 225.0,
            gate_range_db: 35.0,
            ..Default::default()
        };
        let rig = super::PresetRig::from_state(&state);
        let encoded = serde_json::to_string(&rig).unwrap();
        let restored: super::PresetRig = serde_json::from_str(&encoded).unwrap();
        let mut applied = crate::EngineState::default();
        restored.apply_to(&mut applied);
        assert_eq!(applied.gate_hold_ms, 75.0);
        assert_eq!(applied.gate_release_ms, 225.0);
        assert_eq!(applied.gate_range_db, 35.0);
        let legacy: super::PresetRig =
            serde_json::from_str(r#"{"gate_threshold_db":-42}"#).unwrap();
        assert_eq!(legacy.gate_threshold_db, -42.0);
        assert_eq!(legacy.gate_hold_ms, 20.0);
        assert_eq!(legacy.gate_release_ms, 50.0);
        assert_eq!(legacy.gate_range_db, 80.0);
    }

    #[test]
    fn clean_octaver_round_trips_and_old_presets_default_to_analog() {
        let legacy: super::PresetRig = serde_json::from_str("{}").unwrap();
        assert!(!legacy.octaver_clean);
        let state = crate::EngineState {
            octaver_clean: true,
            ..Default::default()
        };
        let serialized = serde_json::to_string(&super::PresetRig::from_state(&state)).unwrap();
        let rig: super::PresetRig = serde_json::from_str(&serialized).unwrap();
        let mut restored = crate::EngineState::default();
        rig.apply_to(&mut restored);
        assert!(restored.octaver_clean);
    }
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn store_round_trip_preserves_complete_rig_and_stable_id() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pedal-presets-{unique}.json"));
        let mut store = PresetStore::default();
        let state = EngineState {
            pre_model: Some("/models/drive.nam".to_owned()),
            model: Some("/models/amp.nam".to_owned()),
            pedal_mix: 0.4,
            amp_bass_db: 3.0,
            ..EngineState::default()
        };
        let id = store.save_current("Crunch".to_owned(), &state).id.clone();

        save_atomic(&path, &store).expect("save presets");
        let restored = load_or_default(&path);

        assert_eq!(restored, store);
        assert_eq!(restored.find(&id).expect("preset").rig.pedal_mix, 0.4);
        fs::remove_file(path).expect("remove preset fixture");
    }
}
