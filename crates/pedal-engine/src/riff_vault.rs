//! Power-safe Riff Vault exports from a bounded in-memory rolling buffer.
//!
//! The audio callback pushes paired DI/wet blocks into a preallocated bounded
//! queue. A background consumer owns ring writes; snapshots cannot block audio.
//! Queue overruns are counted and replaced by silence, never compressed time.

use crate::dsp::MonoProcessor;
use rustfft::{FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const CAPTURE_FRAMES: usize = 128;
const CAPTURE_BLOCKS: usize = 2048;

struct CaptureBlock {
    right: [f32; CAPTURE_FRAMES],
    start: u64,
    len: usize,
    clean: [f32; CAPTURE_FRAMES],
    wet: [f32; CAPTURE_FRAMES],
}

struct CaptureQueue {
    blocks: crossbeam_queue::ArrayQueue<CaptureBlock>,
    submitted: AtomicU64,
    dropped: AtomicU64,
}

/// Keep three minutes of clean and processed guitar so a player can decide on
/// the actual take after the performance instead of having to hit record first.
pub const DEFAULT_SECONDS: u32 = 180;

pub struct RiffVault {
    right: Vec<f32>,
    clean: Vec<f32>,
    processed: Vec<f32>,
    write_index: usize,
    filled: usize,
    sample_rate: u32,
    capture: Arc<CaptureQueue>,
    captured_until: u64,
    worker_started: bool,
}

impl RiffVault {
    pub fn new(sample_rate: u32, seconds: u32) -> Self {
        let frames = (sample_rate as usize)
            .saturating_mul(seconds as usize)
            .max(1);
        Self {
            right: vec![0.0; frames],
            clean: vec![0.0; frames],
            processed: vec![0.0; frames],
            write_index: 0,
            filled: 0,
            sample_rate,
            capture: Arc::new(CaptureQueue {
                blocks: crossbeam_queue::ArrayQueue::new(CAPTURE_BLOCKS),
                submitted: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
            }),
            captured_until: 0,
            worker_started: false,
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
            self.right[(self.write_index + offset) % capacity] = *sample;
        }
        self.write_index = (self.write_index + samples.len()) % self.clean.len();
        self.filled = (self.filled + samples.len()).min(self.clean.len());
    }

    // Only called with the vault lock, never from the real-time thread.
    fn drain_capture(&mut self) {
        let completed = self.capture.submitted.load(Ordering::Acquire);
        // Bound work even when a producer continues streaming during a save.
        for _ in 0..CAPTURE_BLOCKS {
            let Some(block) = self.capture.blocks.pop() else {
                break;
            };
            let missing = block.start.saturating_sub(self.captured_until);
            self.record_gap(missing);
            self.record_clean(&block.clean[..block.len]);
            self.record_processed(&block.wet[..block.len]);
            for i in 0..block.len {
                let at = (self.write_index + self.right.len() - block.len % self.right.len() + i)
                    % self.right.len();
                self.right[at] = block.right[i];
            }
            self.captured_until = block.start + block.len as u64;
        }
        // Account for a trailing overrun even if the performer stops playing.
        self.record_gap(completed.saturating_sub(self.captured_until));
        self.captured_until = self.captured_until.max(completed);
    }

    fn record_gap(&mut self, frames: u64) {
        let mut remaining = frames.min(self.clean.len() as u64) as usize;
        let zeros = [0.0; CAPTURE_FRAMES];
        while remaining > 0 {
            let count = remaining.min(CAPTURE_FRAMES);
            self.record_clean(&zeros[..count]);
            self.record_processed(&zeros[..count]);
            remaining -= count;
        }
    }

    pub fn dropped_frames(&self) -> u64 {
        self.capture.dropped.load(Ordering::Relaxed)
    }

    fn snapshot(&mut self) -> (Vec<f32>, Vec<f32>, u32) {
        self.drain_capture();
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

    fn snapshot_stereo(&mut self) -> (Vec<f32>, Vec<[f32; 2]>, u32) {
        let (clean, left, rate) = self.snapshot();
        let start = if self.filled == self.clean.len() {
            self.write_index
        } else {
            0
        };
        let wet = left
            .into_iter()
            .enumerate()
            .map(|(i, l)| [l, self.right[(start + i) % self.right.len()]])
            .collect();
        (clean, wet, rate)
    }

    pub fn capacity_seconds(&self) -> f32 {
        self.clean.len() as f32 / self.sample_rate as f32
    }

    pub fn processed_peaks(&self, point_count: usize) -> Vec<f32> {
        if self.filled == 0 {
            return Vec::new();
        }
        let point_count = point_count.clamp(32, 512);
        let frames_per_point = self.filled.div_ceil(point_count).max(1);
        let start = if self.filled == self.processed.len() {
            self.write_index
        } else {
            0
        };
        (0..self.filled)
            .step_by(frames_per_point)
            .map(|offset| {
                let end = (offset + frames_per_point).min(self.filled);
                (offset..end).fold(0.0_f32, |peak, index| {
                    let at = (start + index) % self.processed.len();
                    peak.max(self.processed[at].abs()).max(self.right[at].abs())
                })
            })
            .collect()
    }
}

pub struct RiffVaultProcessor<P> {
    processor: P,
    capture: Arc<CaptureQueue>,
}

impl<P> RiffVaultProcessor<P> {
    pub fn new(processor: P, vault: Arc<Mutex<RiffVault>>) -> Self {
        let mut state = vault.lock().expect("capture initialization");
        let capture = Arc::clone(&state.capture);
        if !state.worker_started {
            let weak = Arc::downgrade(&vault);
            std::thread::Builder::new()
                .name("guitar-capture".into())
                .spawn(move || {
                    loop {
                        let Some(vault) = weak.upgrade() else {
                            break;
                        };
                        if let Ok(mut state) = vault.lock() {
                            state.drain_capture();
                        }
                        drop(vault);
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                })
                .expect("start guitar capture worker");
            state.worker_started = true;
        }
        Self { processor, capture }
    }
}

impl<P: MonoProcessor> MonoProcessor for RiffVaultProcessor<P> {
    fn process_stereo(&mut self, samples: &mut [f32], output: &mut [[f32; 2]]) {
        for (input, out) in samples
            .chunks_mut(CAPTURE_FRAMES)
            .zip(output.chunks_mut(CAPTURE_FRAMES))
        {
            let mut block = CaptureBlock {
                start: self.capture.submitted.load(Ordering::Relaxed),
                len: input.len(),
                clean: [0.0; CAPTURE_FRAMES],
                wet: [0.0; CAPTURE_FRAMES],
                right: [0.0; CAPTURE_FRAMES],
            };
            block.clean[..input.len()].copy_from_slice(input);
            self.processor.process_stereo(input, out);
            for (i, f) in out.iter().enumerate() {
                block.wet[i] = f[0];
                block.right[i] = f[1];
            }
            let completed = block.start + block.len as u64;
            if self.capture.blocks.push(block).is_err() {
                self.capture
                    .dropped
                    .fetch_add(input.len() as u64, Ordering::Relaxed);
            }
            self.capture.submitted.store(completed, Ordering::Release);
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        for samples in samples.chunks_mut(CAPTURE_FRAMES) {
            let mut block = CaptureBlock {
                right: [0.0; CAPTURE_FRAMES],
                start: self.capture.submitted.load(Ordering::Relaxed),
                len: samples.len(),
                clean: [0.0; CAPTURE_FRAMES],
                wet: [0.0; CAPTURE_FRAMES],
            };
            block.clean[..samples.len()].copy_from_slice(samples);
            self.processor.process(samples);
            block.wet[..samples.len()].copy_from_slice(samples);
            block.right[..samples.len()].copy_from_slice(samples);
            let completed = block.start + block.len as u64;
            if self.capture.blocks.push(block).is_err() {
                self.capture
                    .dropped
                    .fetch_add(samples.len() as u64, Ordering::Relaxed);
            }
            self.capture.submitted.store(completed, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedRiff {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_duration_seconds: Option<f32>,
    pub id: String,
    pub name: String,
    pub created_at_ms: u64,
    pub duration_seconds: f32,
    pub clean_path: String,
    pub processed_path: String,
    pub preset: String,
}

/// A deliberately conservative, offline onset-based estimate. It is intended
/// to propose a project tempo from a rhythmic first take, never to replace a
/// player's tempo choice without confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoEstimate {
    pub bpm: u16,
    pub confidence: f32,
    /// Other musically plausible candidates are useful when a player is
    /// strumming eighths or sixteenths and the recording is inherently
    /// ambiguous between, for example, 80 and 160 BPM.
    pub candidates: Vec<TempoCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoCandidate {
    pub bpm: u16,
    pub confidence: f32,
}

pub fn estimate_tempo(path: &Path) -> Result<TempoEstimate, String> {
    let (samples, sample_rate) = read_stereo_wav(path)?;
    estimate_tempo_from_samples(&samples, sample_rate)
        .ok_or_else(|| "not enough rhythmic guitar signal to estimate BPM".to_owned())
}

/// Downsample a saved mono take into peak amplitudes for a responsive waveform
/// view. The values are intentionally un-normalized: a quiet take should look
/// quiet rather than being stretched to fill the lane.
pub fn waveform_peaks(path: &Path, point_count: usize) -> Result<Vec<f32>, String> {
    let audio = crate::audio_file::AudioFile::load(path)?;
    // Peak across channels, not their sum: stereo phase must not hide audio.
    let samples: Vec<_> = audio
        .samples
        .chunks_exact(audio.channels)
        .map(|frame| frame.iter().fold(0.0_f32, |p, s| p.max(s.abs())))
        .collect();
    if samples.is_empty() {
        return Err("the take contains no audio samples".to_owned());
    }
    let point_count = point_count.clamp(32, 512);
    let frames_per_point = samples.len().div_ceil(point_count).max(1);
    Ok(samples
        .chunks(frames_per_point)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
        })
        .collect())
}

/// This intentionally runs only after a take is saved, never in the audio
/// callback.  Guitar's sustained notes make a plain amplitude envelope a poor
/// onset signal, so this follows a compact version of a conventional music
/// information retrieval pipeline:
///
/// 1. spectral flux finds changes in harmonic content at pick attacks;
/// 2. adaptive normalization rejects a change in level or sustained feedback;
/// 3. periodicity and phase agreement rank plausible tempos.
///
/// It does not claim to infer a tempo from a freely played lead line. In that
/// situation `None` is much more useful than a confident but arbitrary BPM.
fn estimate_tempo_from_samples(samples: &[f32], sample_rate: u32) -> Option<TempoEstimate> {
    const ANALYSIS_RATE: u32 = 16_000;
    const FFT_SIZE: usize = 1024;
    const HOP_SIZE: usize = 256;
    const MIN_BPM: u16 = 55;
    const MAX_BPM: u16 = 220;

    let analysis = resample_for_tempo(samples, sample_rate, ANALYSIS_RATE);
    if analysis.len() < ANALYSIS_RATE as usize * 6 {
        return None;
    }
    let flux = spectral_flux(&analysis, FFT_SIZE, HOP_SIZE);
    let onsets = adaptive_onset_envelope(&flux);
    if onsets.len() < 160 || count_onset_peaks(&onsets) < 8 {
        return None;
    }
    let energy = onsets.iter().map(|value| value * value).sum::<f32>();
    if energy <= 0.000_001 {
        return None;
    }

    let frames_per_second = ANALYSIS_RATE as f32 / HOP_SIZE as f32;
    let mut ranked = Vec::new();
    for bpm in MIN_BPM..=MAX_BPM {
        let lag = (frames_per_second * 60.0 / bpm as f32).round() as usize;
        if lag < 2 || lag >= onsets.len() / 2 {
            continue;
        }
        let periodicity = (lag..onsets.len())
            .map(|index| onsets[index] * onsets[index - lag])
            .sum::<f32>()
            / energy;
        let phase = phase_agreement(&onsets, lag);
        // Tempo is customarily communicated within a musically useful range.
        // This is deliberately a gentle prior, not a hard cap: fast metal and
        // slow ballads still win when their beat evidence is strong.
        let register_prior = if !(65..=190).contains(&bpm) { 0.9 } else { 1.0 };
        ranked.push((bpm, (periodicity * 0.72 + phase * 0.28) * register_prior));
    }
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    let (bpm, score) = *ranked.first()?;
    let runner_up = ranked
        .iter()
        .skip(1)
        .find(|(other_bpm, _)| other_bpm.abs_diff(bpm) >= 4)
        .map(|(_, score)| *score)
        .unwrap_or(0.0);
    let separation = ((score - runner_up) / score.max(0.000_1)).clamp(0.0, 1.0);
    let confidence = (score * 1.65 + separation * 0.42).clamp(0.0, 0.94);
    if confidence < 0.28 {
        return None;
    }

    let mut candidates = Vec::new();
    for (candidate_bpm, candidate_score) in ranked {
        if candidates
            .iter()
            .all(|candidate: &TempoCandidate| candidate.bpm.abs_diff(candidate_bpm) >= 4)
        {
            candidates.push(TempoCandidate {
                bpm: candidate_bpm,
                confidence: (candidate_score / score.max(0.000_1)).clamp(0.0, 1.0),
            });
        }
        if candidates.len() == 3 {
            break;
        }
    }
    Some(TempoEstimate {
        bpm,
        confidence,
        candidates,
    })
}

fn resample_for_tempo(samples: &[f32], sample_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 {
        return Vec::new();
    }
    let length =
        (samples.len() as u128 * target_rate as u128 / sample_rate as u128).max(1) as usize;
    let ratio = sample_rate as f64 / target_rate as f64;
    let mut previous_input = 0.0_f32;
    let mut high_pass = 0.0_f32;
    (0..length)
        .map(|index| {
            let position = index as f64 * ratio;
            let lower = position.floor() as usize;
            let upper = (lower + 1).min(samples.len() - 1);
            let fraction = (position - lower as f64) as f32;
            let input = samples[lower] + (samples[upper] - samples[lower]) * fraction;
            // Remove slow pickup/handling rumble while retaining pick attack.
            high_pass = input - previous_input + 0.995 * high_pass;
            previous_input = input;
            high_pass
        })
        .collect()
}

fn spectral_flux(samples: &[f32], fft_size: usize, hop_size: usize) -> Vec<f32> {
    if samples.len() < fft_size {
        return Vec::new();
    }
    let window = (0..fft_size)
        .map(|index| 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / fft_size as f32).cos())
        .collect::<Vec<_>>();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let mut frame = vec![Complex32::new(0.0, 0.0); fft_size];
    let mut previous = vec![0.0_f32; fft_size / 2 + 1];
    let mut flux = Vec::with_capacity((samples.len() - fft_size) / hop_size + 1);
    for start in (0..=samples.len() - fft_size).step_by(hop_size) {
        for (index, cell) in frame.iter_mut().enumerate() {
            *cell = Complex32::new(samples[start + index] * window[index], 0.0);
        }
        fft.process(&mut frame);
        let mut change = 0.0;
        // Ignore DC and the top edge of the spectrum; both tend to emphasize
        // interface noise more than a guitar's pick attack.
        for bin in 2..fft_size / 2 {
            let magnitude = frame[bin].norm().ln_1p();
            change += (magnitude - previous[bin]).max(0.0);
            previous[bin] = magnitude;
        }
        flux.push(change / (fft_size / 2) as f32);
    }
    flux
}

fn adaptive_onset_envelope(flux: &[f32]) -> Vec<f32> {
    if flux.is_empty() {
        return Vec::new();
    }
    let mut sums = vec![0.0_f32; flux.len() + 1];
    let mut squares = vec![0.0_f32; flux.len() + 1];
    for (index, value) in flux.iter().enumerate() {
        sums[index + 1] = sums[index] + value;
        squares[index + 1] = squares[index] + value * value;
    }
    // Roughly one second at 16 kHz / 256 hop. This adapts naturally when the
    // guitarist goes from quiet chords to a louder section.
    let radius = 31;
    flux.iter()
        .enumerate()
        .map(|(index, value)| {
            let start = index.saturating_sub(radius);
            let end = (index + radius + 1).min(flux.len());
            let count = (end - start) as f32;
            let mean = (sums[end] - sums[start]) / count;
            let variance = ((squares[end] - squares[start]) / count - mean * mean).max(0.0);
            (*value - mean - variance.sqrt() * 0.25).max(0.0)
        })
        .collect()
}

fn count_onset_peaks(onsets: &[f32]) -> usize {
    if onsets.len() < 3 {
        return 0;
    }
    let mean = onsets.iter().sum::<f32>() / onsets.len() as f32;
    let deviation = (onsets
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / onsets.len() as f32)
        .sqrt();
    let threshold = mean + deviation * 0.35;
    let mut peaks = 0;
    let mut last_peak = None;
    for index in 1..onsets.len() - 1 {
        if onsets[index] >= threshold
            && onsets[index] >= onsets[index - 1]
            && onsets[index] > onsets[index + 1]
            && last_peak.is_none_or(|previous| index.saturating_sub(previous) >= 12)
        {
            peaks += 1;
            last_peak = Some(index);
        }
    }
    peaks
}

fn phase_agreement(onsets: &[f32], lag: usize) -> f32 {
    let onset_mean = onsets.iter().sum::<f32>() / onsets.len() as f32;
    let mut best = 0.0_f32;
    for phase in 0..lag {
        let mut total = 0.0;
        let mut count = 0;
        for center in (phase..onsets.len()).step_by(lag) {
            let start = center.saturating_sub(1);
            let end = (center + 2).min(onsets.len());
            total += onsets[start..end].iter().copied().fold(0.0_f32, f32::max);
            count += 1;
        }
        if count > 0 {
            best = best.max(total / count as f32);
        }
    }
    (best / onset_mean.max(0.000_1)).clamp(0.0, 1.0)
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
        .snapshot_stereo();
    if clean.is_empty() {
        return Err("play for a moment before saving a riff".to_owned());
    }
    let duration_seconds = clean.len() as f32 / sample_rate as f32;
    let start_seconds = trim_start_seconds
        .unwrap_or(0.0)
        .clamp(0.0, duration_seconds);
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
    crate::audio_file::write_float_wav(
        &processed_path,
        2,
        sample_rate,
        processed.iter().flat_map(|f| *f),
    )?;
    let saved = SavedRiff {
        source_duration_seconds: Some(clean.len() as f32 / sample_rate as f32),
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
    riffs.sort_by_key(|riff| std::cmp::Reverse(riff.created_at_ms));
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
    let (mut clean, sample_rate) = read_stereo_wav(clean_source)?;
    let processed = crate::audio_file::AudioFile::load(processed_source)?;
    if sample_rate != processed.sample_rate {
        return Err("the clean and processed take files do not match".to_owned());
    }
    let source_seconds =
        source_duration(clean_source).unwrap_or(clean.len() as f32 / sample_rate as f32);
    let frames = processed.samples.len() / processed.channels;
    clean.resize(frames, 0.0);
    let duration = frames as f32 / sample_rate as f32;
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
    crate::audio_file::write_float_wav(
        &processed_path,
        processed.channels as u16,
        sample_rate,
        processed.samples[start_frame * processed.channels..end_frame * processed.channels]
            .iter()
            .copied(),
    )?;
    let saved = SavedRiff {
        source_duration_seconds: Some((source_seconds - start).clamp(0.0, end - start)),
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

/// Original DI extent, excluding silence appended for a rendered tail.
pub fn source_duration(clean_path: &Path) -> Option<f32> {
    let name = clean_path
        .file_name()?
        .to_str()?
        .strip_suffix("-clean.wav")?;
    let bytes = fs::read(clean_path.with_file_name(format!("{name}.json"))).ok()?;
    let riff: SavedRiff = serde_json::from_slice(&bytes).ok()?;
    riff.source_duration_seconds
        .filter(|s| s.is_finite() && *s >= 0.0)
}

pub(crate) fn write_metadata(directory: &Path, saved: &SavedRiff) -> Result<(), String> {
    let destination = directory.join(format!("{}.json", saved.id));
    let temporary = destination.with_extension("json.new");
    let bytes = serde_json::to_vec_pretty(saved).map_err(|error| error.to_string())?;
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(temporary, destination).map_err(|error| error.to_string())
}

fn write_stereo_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    crate::audio_file::write_float_wav(path, 2, sample_rate, samples.iter().flat_map(|s| [*s, *s]))
}

fn read_stereo_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let audio = crate::audio_file::AudioFile::load(path)?;
    Ok((audio.mono(), audio.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Passthrough;
    use crate::metronome::{MetronomeParameters, MetronomeProcessor};

    #[test]
    fn snapshot_contention_does_not_drop_or_misalign_capture() {
        struct Double;
        impl MonoProcessor for Double {
            fn process(&mut self, samples: &mut [f32]) {
                for sample in samples {
                    *sample *= 2.0;
                }
            }
        }
        let vault = Arc::new(Mutex::new(RiffVault::new(48000, 1)));
        let mut p = RiffVaultProcessor::new(Double, vault.clone());
        let mut guard = vault.lock().unwrap();
        // Simulate a long save holding the ring lock, while audio keeps arriving.
        for index in 0..100 {
            p.process(&mut [index as f32; 64]);
        }
        let (clean, wet, _) = guard.snapshot();
        assert_eq!(clean.len(), 6400);
        assert_eq!(guard.dropped_frames(), 0);
        for (index, (dry, wet)) in clean.iter().zip(wet).enumerate() {
            assert_eq!(*dry, (index / 64) as f32);
            assert_eq!(wet, dry * 2.0);
        }
    }

    #[test]
    fn queue_overrun_is_counted_and_preserves_elapsed_time() {
        let vault = Arc::new(Mutex::new(RiffVault::new(48000, 10)));
        let mut p = RiffVaultProcessor::new(Passthrough, vault.clone());
        let mut guard = vault.lock().unwrap();
        for _ in 0..CAPTURE_BLOCKS + 3 {
            p.process(&mut [0.25; CAPTURE_FRAMES]);
        }
        let (clean, wet, _) = guard.snapshot();
        assert_eq!(guard.dropped_frames(), (3 * CAPTURE_FRAMES) as u64);
        assert_eq!(clean.len(), (CAPTURE_BLOCKS + 3) * CAPTURE_FRAMES);
        assert_eq!(clean, wet);
        assert!(
            clean[CAPTURE_BLOCKS * CAPTURE_FRAMES..]
                .iter()
                .all(|s| *s == 0.0)
        );
        p.process(&mut [0.5; CAPTURE_FRAMES]);
        let (next, _, _) = guard.snapshot();
        assert_eq!(next.len(), clean.len() + CAPTURE_FRAMES);
        assert_eq!(*next.last().unwrap(), 0.5);
    }

    #[test]
    fn float_takes_round_trip_and_legacy_pcm_takes_remain_readable() {
        let path =
            std::env::temp_dir().join(format!("pedal-take-formats-{}.wav", std::process::id()));
        let samples = [1.25, -1.4, 0.0000001, 0.0];
        write_stereo_wav(&path, &samples, 48_000).unwrap();
        let (loaded, rate) = read_stereo_wav(&path).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(loaded, samples);
        for bits in [16, 24] {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: bits,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            let sample = 1_i32 << (bits - 3);
            writer.write_sample(sample).unwrap();
            writer.write_sample(sample).unwrap();
            writer.finalize().unwrap();
            assert_eq!(read_stereo_wav(&path).unwrap().0, vec![0.25]);
        }
        std::fs::remove_file(path).unwrap();
    }

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

    #[test]
    fn output_only_metronome_is_not_printed_into_a_take() {
        let vault = Arc::new(Mutex::new(RiffVault::new(48_000, 1)));
        let captured_guitar = RiffVaultProcessor::new(Passthrough, Arc::clone(&vault));
        let parameters = Arc::new(MetronomeParameters::new(true, 120));
        let mut output = MetronomeProcessor::new(captured_guitar, parameters, 48_000);
        let mut samples = [0.0_f32; 64];

        output.process(&mut samples);

        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
        let (clean, processed, _) = vault.lock().expect("vault").snapshot();
        assert!(clean.iter().all(|sample| *sample == 0.0));
        assert!(processed.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn onset_estimator_finds_a_regular_click_pulse() {
        let sample_rate = 48_000;
        let seconds = 12;
        let mut samples = vec![0.0; sample_rate as usize * seconds];
        // 120 BPM: one pick-like transient every 0.5 seconds.
        for start in (0..samples.len()).step_by((sample_rate / 2) as usize) {
            for offset in 0..16 {
                if let Some(sample) = samples.get_mut(start + offset) {
                    *sample = 1.0 - offset as f32 / 16.0;
                }
            }
        }
        let estimate = estimate_tempo_from_samples(&samples, sample_rate).expect("tempo");
        assert!(
            (estimate.bpm as i32 - 120).abs() <= 1,
            "estimated {} BPM",
            estimate.bpm
        );
        assert!(estimate.confidence > 0.2);
    }

    #[test]
    fn spectral_estimator_handles_varied_guitar_like_pick_attacks() {
        let sample_rate = 48_000;
        let seconds = 18;
        let mut samples = vec![0.0; sample_rate as usize * seconds];
        // A deliberately non-click-like source: each beat starts with a short
        // bright pick transient then sustains a differently loud harmonic
        // guitar-ish note. The old amplitude-rise detector was especially
        // sensitive to those changing sustain levels.
        for (beat, start) in (0..samples.len())
            .step_by((sample_rate / 2) as usize)
            .enumerate()
        {
            let gain = [0.32, 0.72, 0.46, 0.61][beat % 4];
            for offset in 0..(sample_rate / 5) as usize {
                let index = start + offset;
                if index >= samples.len() {
                    break;
                }
                let time = offset as f32 / sample_rate as f32;
                let sustain = (-(time * 13.0)).exp()
                    * ((std::f32::consts::TAU * 220.0 * time).sin()
                        + 0.38 * (std::f32::consts::TAU * 440.0 * time).sin());
                let pick = (-(time * 150.0)).exp() * (std::f32::consts::TAU * 3_200.0 * time).sin();
                samples[index] += gain * (sustain + 0.28 * pick);
            }
        }
        let estimate = estimate_tempo_from_samples(&samples, sample_rate).expect("tempo");
        assert!(
            (estimate.bpm as i32 - 120).abs() <= 2,
            "estimated {} BPM",
            estimate.bpm
        );
        assert!(estimate.confidence > 0.28);
        assert!(!estimate.candidates.is_empty());
    }

    #[test]
    fn spectral_estimator_declines_a_sustained_note() {
        let sample_rate = 48_000;
        let samples = (0..sample_rate as usize * 10)
            .map(|index| {
                (std::f32::consts::TAU * 220.0 * index as f32 / sample_rate as f32).sin() * 0.45
            })
            .collect::<Vec<_>>();
        assert!(estimate_tempo_from_samples(&samples, sample_rate).is_none());
    }
}
