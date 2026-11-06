use crate::EngineState;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub fn load(path: &Path) -> io::Result<EngineState> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

pub fn load_or_default(path: &Path) -> EngineState {
    match load(path) {
        Ok(state) => state,
        Err(error) if error.kind() == io::ErrorKind::NotFound => EngineState::default(),
        Err(error) => {
            eprintln!(
                "could not restore persistent engine state from {}: {error}; using safe defaults",
                path.display()
            );
            EngineState::default()
        }
    }
}

pub fn save_atomic(path: &Path, state: &EngineState) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;

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
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_state_path(test_name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pedal-{test_name}-{}-{unique}.json",
            std::process::id()
        ))
    }

    #[test]
    fn saved_rig_survives_a_fresh_process_state() {
        let path = temporary_state_path("offline-restart");
        let expected = EngineState {
            bypassed: false,
            preset: "Portable Clean".to_owned(),
            pre_bypassed: true,
            pre_model: Some("/var/lib/pedal/models/drive.nam".to_owned()),
            amp_bypassed: false,
            model: Some("/var/lib/pedal/models/portable-clean.nam".to_owned()),
            pedal_drive_db: 9.0,
            pedal_mix: 0.75,
            amp_bass_db: 2.5,
            amp_volume_db: -3.0,
            gate_hold_ms: 65.0,
            gate_release_ms: 175.0,
            gate_range_db: 40.0,
            nam_quality: 0,
            render_nam_quality: 1,
            pedal_align_enabled: false,
            pedal_eq: crate::nam_eq::NamEqSettings {
                pre: true,
                ..Default::default()
            },
            amp_eq: crate::nam_eq::NamEqSettings {
                enabled: false,
                ..Default::default()
            },
            ..EngineState::default()
        };

        save_atomic(&path, &expected).expect("save state");
        let restored = load_or_default(&path);

        assert_eq!(restored, expected);
        fs::remove_file(path).expect("remove test state");
    }

    #[test]
    fn corrupt_state_uses_safe_defaults() {
        let path = temporary_state_path("corrupt-state");
        fs::write(&path, b"not json").expect("write corrupt state");

        assert_eq!(load_or_default(&path), EngineState::default());
        fs::remove_file(path).expect("remove test state");
    }
}
