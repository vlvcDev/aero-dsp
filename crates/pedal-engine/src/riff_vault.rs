//! Power-safe Riff Vault exports from a bounded in-memory rolling buffer.
//!
//! The audio callback only takes a non-blocking lock and copies samples into a
//! preallocated ring. Saving clones a consistent snapshot on the control
//! thread, then writes files after releasing the lock.

use crate::dsp::MonoProcessor;
use hound::{SampleFormat, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Keep three minutes of clean and processed guitar so a player can decide on
/// the actual take after the performance instead of having to hit record first.
pub const DEFAULT_SECONDS: u32 = 180;

pub struct RiffVault {
    clean: Vec<f32>,
    processed: Vec<f32>,
    write_index: usize,
    filled: usize,
    sample_rate: u32,
}

impl RiffVault {
    pub fn new(sample_rate: u32, seconds: u32) -> Self {
        let frames = (sample_rate as usize)
            .saturating_mul(seconds as usize)
            .max(1);
        Self {
            clean: vec![0.0; frames],
            processed: vec![0.0; frames],
            write_index: 0,
            filled: 0,
            sample_rate,
        }
    }

    fn record_clean(&mut self, samples: &[f32]) {
        let capacity = self.clean.len();
        for (offset, sample) in samples.iter().enumerate() {
            self.clean[(self.write_index + offset) % capacity] = *sample;
        }
    }

    fn record_processed(&mut self, samples: &[f32]) {
        let capacity = self.processed.len();
        for (offset, sample) in samples.iter().enumerate() {
            self.processed[(self.write_index + offset) % capacity] = *sample;
        }
        self.write_index = (self.write_index + samples.len()) % self.clean.len();
        self.filled = (self.filled + samples.len()).min(self.clean.len());
    }

    fn snapshot(&self) -> (Vec<f32>, Vec<f32>, u32) {
        let start = if self.filled == self.clean.len() {
            self.write_index
        } else {
            0
        };
        let ordered = |ring: &[f32]| {
            (0..self.filled)
                .map(|offset| ring[(start + offset) % ring.len()])
                .collect::<Vec<_>>()
        };
        (
            ordered(&self.clean),
            ordered(&self.processed),
            self.sample_rate,
        )
    }

    pub fn available_seconds(&self) -> f32 {
        self.filled as f32 / self.sample_rate as f32
    }

    pub fn capacity_seconds(&self) -> f32 {
        self.clean.len() as f32 / self.sample_rate as f32
    }
}

pub struct RiffVaultProcessor<P> {
    processor: P,
    vault: Arc<Mutex<RiffVault>>,
}

impl<P> RiffVaultProcessor<P> {
    pub fn new(processor: P, vault: Arc<Mutex<RiffVault>>) -> Self {
        Self { processor, vault }
    }
}

impl<P: MonoProcessor> MonoProcessor for RiffVaultProcessor<P> {
    fn process(&mut self, samples: &mut [f32]) {
        // A save may briefly hold the lock while snapshotting; skip that tiny
        // block rather than ever waiting in the real-time callback.
        let captured_clean = self
            .vault
            .try_lock()
            .map(|mut vault| vault.record_clean(samples))
            .is_ok();
        self.processor.process(samples);
        if captured_clean {
            if let Ok(mut vault) = self.vault.try_lock() {
                vault.record_processed(samples);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRiff {
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub duration_seconds: f32,
    pub clean_path: String,
    pub processed_path: String,
    pub preset: String,
}

pub fn save_latest(
    vault: &Arc<Mutex<RiffVault>>,
    directory: &Path,
    requested_name: Option<&str>,
    preset: &str,
    trim_start_seconds: Option<f32>,
    trim_end_seconds: Option<f32>,
) -> Result<SavedRiff, String> {
    let (clean, processed, sample_rate) = vault
        .lock()
        .map_err(|_| "riff vault lock poisoned".to_owned())?
        .snapshot();
    if clean.is_empty() {
        return Err("play for a moment before saving a riff".to_owned());
    }
    let duration_seconds = clean.len() as f32 / sample_rate as f32;
    let start_seconds = trim_start_seconds.unwrap_or(0.0).clamp(0.0, duration_seconds);
    let end_seconds = trim_end_seconds
        .unwrap_or(duration_seconds)
        .clamp(start_seconds, duration_seconds);
    let start_frame = (start_seconds * sample_rate as f32).round() as usize;
    let end_frame = (end_seconds * sample_rate as f32).round() as usize;
    let clean = clean[start_frame..end_frame].to_vec();
    let processed = processed[start_frame..end_frame].to_vec();
    if clean.is_empty() {
        return Err("choose a non-empty portion of the guitar take".to_owned());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    let id = format!("riff-{created_at_ms}");
    let name = requested_name
        .filter(|name| !name.trim().is_empty())
        .map(str::trim)
        .unwrap_or("Untitled Riff")
        .chars()
        .take(64)
        .collect::<String>();
    let clean_path = directory.join(format!("{id}-clean.wav"));
    let processed_path = directory.join(format!("{id}-rig.wav"));
    write_stereo_wav(&clean_path, &clean, sample_rate)?;
    write_stereo_wav(&processed_path, &processed, sample_rate)?;
    let saved = SavedRiff {
        id: id.clone(),
        name,
        created_at_ms,
        duration_seconds: clean.len() as f32 / sample_rate as f32,
        clean_path: clean_path.display().to_string(),
        processed_path: processed_path.display().to_string(),
        preset: preset.to_owned(),
    };
    write_metadata(directory, &saved)?;
    Ok(saved)
}

pub fn list(directory: &Path) -> Result<Vec<SavedRiff>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error.to_string()),
    };
    let mut riffs = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<SavedRiff>(&bytes).ok())
        .collect::<Vec<_>>();
    riffs.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    Ok(riffs)
}

/// Creates a new trimmed take rather than modifying the source files.  That
/// preserves the original capture for later comping or re-editing.
pub fn trim_saved(
    clean_source: &Path,
    processed_source: &Path,
    directory: &Path,
    requested_name: Option<&str>,
    preset: &str,
    trim_start_seconds: f32,
    trim_end_seconds: f32,
) -> Result<SavedRiff, String> {
    let (clean, sample_rate) = read_stereo_wav(clean_source)?;
    let (processed, processed_rate) = read_stereo_wav(processed_source)?;
    if sample_rate != processed_rate || clean.len() != processed.len() {
        return Err("the clean and processed take files do not match".to_owned());
    }
    let duration = clean.len() as f32 / sample_rate as f32;
    let start = trim_start_seconds.clamp(0.0, duration);
    let end = trim_end_seconds.clamp(start, duration);
    let start_frame = (start * sample_rate as f32).round() as usize;
    let end_frame = (end * sample_rate as f32).round() as usize;
    if start_frame >= end_frame {
        return Err("choose a non-empty portion of the guitar take".to_owned());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    let id = format!("riff-{created_at_ms}");
    let name = requested_name
        .filter(|name| !name.trim().is_empty())
        .map(str::trim)
        .unwrap_or("Trimmed Riff")
        .chars()
        .take(64)
        .collect::<String>();
    let clean_path = directory.join(format!("{id}-clean.wav"));
    let processed_path = directory.join(format!("{id}-rig.wav"));
    write_stereo_wav(&clean_path, &clean[start_frame..end_frame], sample_rate)?;
    write_stereo_wav(&processed_path, &processed[start_frame..end_frame], sample_rate)?;
    let saved = SavedRiff {
        id: id.clone(),
        name,
        created_at_ms,
        duration_seconds: (end_frame - start_frame) as f32 / sample_rate as f32,
        clean_path: clean_path.display().to_string(),
        processed_path: processed_path.display().to_string(),
        preset: preset.to_owned(),
    };
    write_metadata(directory, &saved)?;
    Ok(saved)
}

fn write_metadata(directory: &Path, saved: &SavedRiff) -> Result<(), String> {
    let destination = directory.join(format!("{}.json", saved.id));
    let temporary = destination.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(saved).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn write_stereo_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let temporary: PathBuf = path.with_extension("wav.new");
    let mut writer = WavWriter::create(&temporary, spec).map_err(|error| error.to_string())?;
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_sample(value)
            .map_err(|error| error.to_string())?;
        writer
            .write_sample(value)
            .map_err(|error| error.to_string())?;
    }
    writer.finalize().map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn read_stereo_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path).map_err(|error| error.to_string())?;
    let spec = reader.spec();
    if spec.channels != 2 || spec.sample_format != SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err("only 16-bit stereo WAV takes can be trimmed".to_owned());
    }
    let samples = reader
        .samples::<i16>()
        .enumerate()
        .filter_map(|(index, sample)| (index % 2 == 0).then_some(sample))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|sample| sample as f32 / i16::MAX as f32)
        .collect();
    Ok((samples, spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;

    #[test]
    fn ring_snapshot_keeps_the_latest_samples_in_order() {
        let vault = Arc::new(Mutex::new(RiffVault::new(4, 1)));
        let mut processor = RiffVaultProcessor::new(Passthrough, Arc::clone(&vault));
        let mut first = [1.0, 2.0, 3.0];
        processor.process(&mut first);
        let mut second = [4.0, 5.0];
        processor.process(&mut second);
        let (clean, processed, _) = vault.lock().expect("vault").snapshot();
        assert_eq!(clean, vec![2.0, 3.0, 4.0, 5.0]);
        assert_eq!(processed, clean);
    }
}
