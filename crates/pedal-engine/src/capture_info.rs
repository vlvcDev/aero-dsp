//! Facts about a NAM capture that help choose and stage it: whether it
//! already includes a speaker cabinet (so a cab IR on top would filter the
//! sound twice), and its small-signal gain (a practical proxy for how
//! saturated it is: prog/djent rhythm tones tend to sit around +35..+50 dB,
//! the most extreme high-gain captures above +60 dB).
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Reference input for the gain measurement: white noise at -80 dBFS RMS,
/// about an interface's idle floor and well inside every capture's
/// small-signal region.
pub const GAIN_PROBE_DBFS: f32 = -80.0;

/// Gear types (from the capture's `metadata.gear_type`, or the TONE3000
/// sidecar's `gear`) that already contain a speaker cabinet.
fn gear_includes_cab(gear: &str) -> bool {
    matches!(
        gear.trim().to_ascii_lowercase().replace('_', "-").as_str(),
        "amp-cab" | "full-rig"
    )
}

/// An explicit "no cab" in the file name overrides a contradictory tag.
fn name_says_no_cab(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    ["no-cab", "no_cab", "nocab", "no cab"]
        .iter()
        .any(|marker| name.contains(marker))
}

/// The capture's declared gear type, if any.
pub fn gear_type(path: &Path) -> Option<String> {
    let from_model = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|model| model["metadata"]["gear_type"].as_str().map(str::to_owned));
    from_model.or_else(|| {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(".json");
        std::fs::read(sidecar)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|side| side["gear"].as_str().map(str::to_owned))
    })
}

pub fn includes_cab(path: &Path) -> bool {
    !name_says_no_cab(path) && gear_type(path).is_some_and(|gear| gear_includes_cab(&gear))
}

/// Memoized [`includes_cab`] for state snapshots (a capture file is parsed
/// once per path per process). Control thread only.
pub fn includes_cab_cached(path: &str) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().ok().and_then(|c| c.get(path).copied()) {
        return known;
    }
    let value = includes_cab(Path::new(path));
    if let Ok(mut c) = cache.lock() {
        c.insert(path.to_owned(), value);
    }
    value
}

/// Small-signal gain in dB: output RMS for a [`GAIN_PROBE_DBFS`] white-noise
/// input, minus the input level. Loads the model (not realtime safe; ~0.1-0.5 s).
pub fn small_signal_gain_db(path: &Path) -> Result<f32, String> {
    const RATE: usize = 48_000;
    let mut model = nam_bridge::NamModel::load_with_quality(
        path,
        RATE as f64,
        256,
        nam_bridge::NamQuality::Full,
    )
    .map_err(|e| e.to_string())?;
    let amplitude = 10.0_f32.powf(GAIN_PROBE_DBFS / 20.0) * 3.0_f32.sqrt();
    let mut seed = 0x5eed_u32;
    let mut signal: Vec<f32> = (0..RATE)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amplitude
        })
        .collect();
    for block in signal.chunks_mut(256) {
        model.process(block).map_err(|e| e.to_string())?;
    }
    // Skip the first half second (receptive field / settling); measure AC.
    let tail = &signal[RATE / 2..];
    let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
    let power = tail
        .iter()
        .map(|s| (f64::from(*s) - mean).powi(2))
        .sum::<f64>()
        / tail.len() as f64;
    if !power.is_finite() {
        return Err("capture produced non-finite output".into());
    }
    Ok((10.0 * power.max(1e-30).log10()) as f32 - GAIN_PROBE_DBFS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cab_detection_follows_gear_type_with_a_name_override() {
        for (gear, cab) in [
            ("amp_cab", true),
            ("amp-cab", true),
            ("full-rig", true),
            ("Full_Rig", true),
            ("amp", false),
            ("pedal", false),
        ] {
            assert_eq!(gear_includes_cab(gear), cab, "{gear}");
        }
        assert!(name_says_no_cab(Path::new("/x/Twin_clean_NO-CAB.nam")));
        assert!(!name_says_no_cab(Path::new("/x/Twin_4x12_SM57.nam")));
    }

    #[test]
    fn gain_is_measured_for_a_real_capture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../vendor/NeuralAmpModelerCore/example_models/A2.nam");
        let gain = small_signal_gain_db(&path).unwrap();
        // Matches the earlier library scan of this file (+21.8 dB).
        assert!((gain - 21.8).abs() < 1.0, "{gain}");
        assert!(small_signal_gain_db(Path::new("/nonexistent.nam")).is_err());
    }
}
