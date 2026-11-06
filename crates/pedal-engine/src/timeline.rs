//! Sample-clocked stereo timeline core. Prepare assets and arrangements off the
//! audio thread. Rendering only reads prepared memory; no files, locks or timers.
use std::{path::Path, sync::Arc};

pub struct Asset {
    samples: Vec<[f32; 2]>,
    mono: bool,
}
impl Asset {
    pub fn load(path: &Path, rate: u32) -> Result<Self, String> {
        let header = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        if header.spec().sample_rate == 0
            || header.duration() as u64 > header.spec().sample_rate as u64 * 600
        {
            return Err("timeline asset exceeds 10 minutes or has an invalid rate".into());
        }
        drop(header);
        let audio = crate::audio_file::AudioFile::load(path)?;
        if !(1..=2).contains(&audio.channels) {
            return Err("timeline assets must be mono or stereo".into());
        }
        let samples =
            crate::audio_file::resample(&audio.samples, audio.channels, audio.sample_rate, rate)?;
        Ok(Self {
            samples: samples
                .chunks_exact(audio.channels)
                .map(|f| [f[0], f[audio.channels - 1]])
                .collect(),
            mono: audio.channels == 1,
        })
    }
    pub fn frames(&self) -> u64 {
        self.samples.len() as u64
    }
}

pub struct Clip {
    asset: Arc<Asset>,
    start: u64,
    source_in: u64,
    length: u64,
    fade_in: u64,
    fade_out: u64,
}
impl Clip {
    /// Frame positions are non-destructive: no source audio is sliced or copied.
    pub fn new(
        asset: Arc<Asset>,
        start: u64,
        source_in: u64,
        length: u64,
        fade_in: u64,
        fade_out: u64,
    ) -> Result<Self, String> {
        if length == 0
            || source_in
                .checked_add(length)
                .is_none_or(|end| end > asset.frames())
            || start.checked_add(length).is_none()
        {
            return Err("clip range is outside its source or timeline".into());
        }
        Ok(Self {
            asset,
            start,
            source_in,
            length,
            fade_in: fade_in.min(length),
            fade_out: fade_out.min(length),
        })
    }
    fn sample(&self, frame: u64) -> Option<[f32; 2]> {
        let offset = frame.checked_sub(self.start)?;
        if offset >= self.length {
            return None;
        }
        let mut gain = 1.0_f32;
        if self.fade_in > 1 && offset < self.fade_in {
            gain *= offset as f32 / (self.fade_in - 1) as f32;
        }
        let remaining = self.length - 1 - offset;
        if self.fade_out > 1 && remaining < self.fade_out {
            gain *= remaining as f32 / (self.fade_out - 1) as f32;
        }
        let s = self.asset.samples[(self.source_in + offset) as usize];
        Some([s[0] * gain, s[1] * gain])
    }
}

pub struct Track {
    control: Option<Arc<TrackControl>>,
    current: [f32; 4],
    target: [f32; 4],
    pub clips: Vec<Clip>,
    gain: f32,
    pan: f32,
    pub muted: bool,
    pub solo: bool,
}
impl Track {
    pub fn new(clips: Vec<Clip>, gain: f32, pan: f32) -> Result<Self, String> {
        if !gain.is_finite()
            || !(0.0..=4.0).contains(&gain)
            || !pan.is_finite()
            || !(-1.0..=1.0).contains(&pan)
        {
            return Err("invalid track gain or pan".into());
        }
        let mut track = Self {
            clips,
            gain,
            pan,
            muted: false,
            solo: false,
            control: None,
            current: [0.0; 4],
            target: [0.0; 4],
        };
        let a = track.gains(true);
        let b = track.gains(false);
        track.current = [a[0], a[1], b[0], b[1]];
        track.target = track.current;
        Ok(track)
    }
    fn gains(&self, mono: bool) -> [f32; 2] {
        if mono {
            let angle = (self.pan + 1.0) * std::f32::consts::FRAC_PI_4;
            [angle.cos() * self.gain, angle.sin() * self.gain]
        } else {
            // Stereo balance retains the image at center; never folds L into R.
            [
                self.gain * (1.0 - self.pan.max(0.0)),
                self.gain * (1.0 + self.pan.min(0.0)),
            ]
        }
    }
}

pub struct Mixer {
    smoothing: f32,
    tracks: Vec<Track>,
    position: u64,
    end: u64,
    playing: bool,
    loop_range: Option<(u64, u64)>,
}
impl Mixer {
    pub fn new(mut tracks: Vec<Track>) -> Self {
        let solo = tracks.iter().any(|t| t.solo);
        for track in &mut tracks {
            if track.muted || (solo && !track.solo) {
                track.current = [0.0; 4];
                track.target = [0.0; 4];
            }
        }
        let end = tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(|c| c.start + c.length)
            .max()
            .unwrap_or(0);
        Self {
            tracks,
            smoothing: 1.0 - (-1.0_f32 / 480.0).exp(),
            position: 0,
            end,
            playing: false,
            loop_range: None,
        }
    }
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn playing(&self) -> bool {
        self.playing
    }
    pub fn play(&mut self) {
        self.playing = self.position < self.end;
    }
    pub fn pause(&mut self) {
        self.playing = false;
    }
    pub fn seek(&mut self, frame: u64) {
        self.position = frame.min(self.end);
    }
    pub fn set_loop(&mut self, range: Option<(u64, u64)>) -> Result<(), String> {
        if range.is_some_and(|(a, b)| a >= b || b > self.end) {
            return Err("invalid loop range".into());
        }
        self.loop_range = range;
        Ok(())
    }
    /// Writes a stereo bus, retaining float headroom. Master output protection
    /// belongs after guitar/click/timeline summing, not independently per track.
    pub fn render(&mut self, output: &mut [[f32; 2]]) {
        output.fill([0.0; 2]);
        if !self.playing {
            return;
        }
        for track in &mut self.tracks {
            if let Some(control) = &track.control {
                let packed = control.levels.load(Ordering::Relaxed);
                track.gain = f32::from_bits(packed as u32);
                track.pan = f32::from_bits((packed >> 32) as u32);
                let flags = control.flags.load(Ordering::Relaxed);
                track.muted = flags & 1 != 0;
                track.solo = flags & 2 != 0;
            }
        }
        let solo = self.tracks.iter().any(|t| t.solo);
        for track in &mut self.tracks {
            let a = track.gains(true);
            let b = track.gains(false);
            track.target = if track.muted || (solo && !track.solo) {
                [0.0; 4]
            } else {
                [a[0], a[1], b[0], b[1]]
            };
        }
        let mut offset = 0;
        let mut gains = [[0.0; 4]; 1024];
        while offset < output.len() {
            if let Some((start, end)) = self.loop_range
                && self.position >= end
            {
                self.position = start;
            }
            if self.position >= self.end {
                self.playing = false;
                break;
            }
            let boundary = self.loop_range.map_or(self.end, |(_, end)| end);
            let count = (output.len() - offset)
                .min(1024)
                .min((boundary - self.position) as usize);
            for track in &mut self.tracks {
                for frame_gain in &mut gains[..count] {
                    for i in 0..4 {
                        track.current[i] += (track.target[i] - track.current[i]) * self.smoothing;
                        if (track.target[i] - track.current[i]).abs() < 1e-8 {
                            track.current[i] = track.target[i];
                        }
                    }
                    *frame_gain = track.current;
                }
                for clip in &track.clips {
                    let first = clip.start.max(self.position);
                    let last = (clip.start + clip.length).min(self.position + count as u64);
                    for position in first..last {
                        let i = (position - self.position) as usize;
                        let s = clip.sample(position).expect("validated overlap");
                        let at = if clip.asset.mono { 0 } else { 2 };
                        output[offset + i][0] += s[0] * gains[i][at];
                        output[offset + i][1] += s[1] * gains[i][at + 1];
                    }
                }
            }
            self.position += count as u64;
            offset += count;
        }
        if self.loop_range.is_none() && self.position >= self.end {
            self.playing = false;
        }
    }
}

/// Place a recorded take earlier by a measured round-trip delay. Near time zero,
/// preserve source data and trim its leading compensated region non-destructively.
/// A UI timestamp or an interface knob position is not a latency measurement.
pub fn compensated_placement(start: u64, length: u64, delay_frames: u64) -> (u64, u64, u64) {
    let source_in = delay_frames.saturating_sub(start).min(length);
    (
        start.saturating_sub(delay_frames),
        source_in,
        length - source_in,
    )
}

use crate::dsp::{MonoProcessor, SwapMailbox};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

struct Session {
    name: String,
    preset: String,
    sample_rate: u32,
    countin_remaining: u64,
    countin_total: u64,
    bpm: f64,
    beats: u64,
    mixer: Mixer,
    generation: u64,
    recording: Vec<[f32; 3]>,
    recorded: usize,
    recording_start: u64,
    delay_frames: u64,
}
impl Session {
    fn empty(generation: u64) -> Self {
        Self {
            mixer: Mixer::new(vec![]),
            name: String::new(),
            preset: String::new(),
            sample_rate: 48000,
            countin_remaining: 0,
            countin_total: 0,
            bpm: 120.0,
            beats: 4,
            generation,
            recording: vec![],
            recorded: 0,
            recording_start: 0,
            delay_frames: 0,
        }
    }
}
#[derive(Default)]
pub struct Transport {
    pending_generation: AtomicU64,
    looping: AtomicBool,
    interrupted: AtomicBool,
    pending_take: Mutex<Option<Session>>,
    click: AtomicBool,
    countin: AtomicU64,
    track_controls: Mutex<HashMap<String, Arc<TrackControl>>>,
    mailbox: SwapMailbox<Session>,
    control: Mutex<()>,
    generation: AtomicU64,
    acknowledged: AtomicU64,
    position: AtomicU64,
    playing: AtomicBool,
    recording: AtomicBool,
    live: AtomicBool,
    rate: AtomicU32,
}
#[derive(Default)]
struct TrackControl {
    levels: AtomicU64,
    flags: AtomicU32,
}
impl TrackControl {
    fn update(&self, value: &Value) -> Result<(), String> {
        let gain = value["volume"].as_f64().unwrap_or(1.0) as f32;
        let pan = value["pan"].as_f64().unwrap_or(0.0) as f32;
        if !gain.is_finite()
            || !(0.0..=4.0).contains(&gain)
            || !pan.is_finite()
            || !(-1.0..=1.0).contains(&pan)
        {
            return Err("invalid track gain/pan".into());
        }
        self.levels.store(
            (gain.to_bits() as u64) | ((pan.to_bits() as u64) << 32),
            Ordering::Relaxed,
        );
        self.flags.store(
            u32::from(value["muted"].as_bool().unwrap_or(false))
                | (u32::from(value["solo"].as_bool().unwrap_or(false)) << 1),
            Ordering::Relaxed,
        );
        Ok(())
    }
}
impl Transport {
    // A timed-out control request may still be acknowledged on the next audio
    // block. Collect it before staging another command so its take is not retired
    // and discarded by the next mailbox handoff.
    fn recover_pending(&self) -> Result<(), String> {
        let generation = self.pending_generation.load(Ordering::Acquire);
        if generation == 0 {
            return Ok(());
        }
        if self.acknowledged.load(Ordering::Acquire) != generation {
            return Err(
                "Waiting for the previous transport command; restore audio before retrying".into(),
            );
        }
        if let Some(session) = self.mailbox.take_retired().filter(|s| s.recorded > 0) {
            *self
                .pending_take
                .lock()
                .map_err(|_| "pending take lock poisoned")? = Some(session);
        }
        self.pending_generation.store(0, Ordering::Release);
        Ok(())
    }
    pub fn update_tracks(&self, payload: &Value) -> Result<Value, String> {
        let controls = self
            .track_controls
            .lock()
            .map_err(|_| "track controls poisoned")?;
        for track in payload["tracks"]
            .as_array()
            .ok_or("tracks must be an array")?
        {
            if let Some(control) = track["id"].as_str().and_then(|id| controls.get(id)) {
                control.update(track)?;
            }
        }
        Ok(self.status())
    }
    pub fn status(&self) -> Value {
        json!({"position_frames": self.position.load(Ordering::Acquire),
            "interrupted": self.interrupted.load(Ordering::Acquire),
            "count_in_frames": self.countin.load(Ordering::Acquire),
            "sample_rate": self.rate.load(Ordering::Acquire),
            "playing": self.playing.load(Ordering::Acquire),
            "recording": self.recording.load(Ordering::Acquire),
            "audio_ready": self.live.load(Ordering::Acquire)})
    }
    pub fn set_click(&self, payload: &Value) -> Result<Value, String> {
        self.click.store(
            payload["enabled"].as_bool().ok_or("enabled is required")?,
            Ordering::Relaxed,
        );
        Ok(self.status())
    }
    pub fn set_looping(&self, payload: &Value) -> Result<Value, String> {
        self.looping.store(
            payload["enabled"].as_bool().ok_or("enabled is required")?,
            Ordering::Relaxed,
        );
        Ok(self.status())
    }
    fn swap(&self, session: Session) -> Result<Option<Session>, String> {
        self.recover_pending()?;
        if !self.live.load(Ordering::Acquire) {
            return Err("audio engine is not ready".into());
        }
        let generation = session.generation;
        self.pending_generation.store(generation, Ordering::Release);
        self.mailbox.stage(session);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while self.acknowledged.load(Ordering::Acquire) != generation {
            if std::time::Instant::now() >= until {
                return Err("audio transport did not acknowledge the command".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let retired = self.mailbox.take_retired();
        self.pending_generation.store(0, Ordering::Release);
        Ok(retired)
    }
    pub fn start(&self, payload: &Value) -> Result<Value, String> {
        let _guard = self.control.lock().map_err(|_| "transport lock poisoned")?;
        self.recover_pending()?;
        if self.recording.load(Ordering::Acquire)
            || self
                .pending_take
                .lock()
                .map_err(|_| "pending take lock poisoned")?
                .is_some()
        {
            return Err("Stop and save the current recording first".into());
        }
        let rate = self.rate.load(Ordering::Acquire);
        if rate == 0 || !self.live.load(Ordering::Acquire) {
            return Err("audio engine is not ready".into());
        }
        let frames = |ms: u64| {
            ms.checked_mul(rate as u64)
                .map(|n| n / 1000)
                .ok_or("timeline duration overflow")
        };
        let mut assets = HashMap::<String, Arc<Asset>>::new();
        let mut tracks = Vec::new();
        let mut controls = HashMap::new();
        let list = payload["tracks"]
            .as_array()
            .ok_or("tracks must be an array")?;
        if list.len() > 64 {
            return Err("maximum 64 tracks".into());
        }
        let mut memory_frames = 0u64;
        let mut clip_count = 0;
        for track in list {
            let mut clips = Vec::new();
            for clip in track["clips"].as_array().ok_or("clips must be an array")? {
                clip_count += 1;
                if clip_count > 4096 {
                    return Err("maximum 4096 clips".into());
                }
                let path = clip["path"].as_str().ok_or("clip path is required")?;
                if !Path::new(path).is_absolute() {
                    return Err("clip path must be absolute".into());
                }
                let asset = if let Some(asset) = assets.get(path) {
                    asset.clone()
                } else {
                    // Bound decoded storage for the first in-memory mixer version.
                    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 256 * 1024 * 1024
                    {
                        return Err("audio asset exceeds 256 MiB".into());
                    }
                    let asset = Arc::new(Asset::load(Path::new(path), rate)?);
                    memory_frames += asset.frames();
                    if memory_frames > rate as u64 * 600 {
                        return Err("timeline exceeds 10 minutes of unique decoded audio".into());
                    }
                    assets.insert(path.to_owned(), asset.clone());
                    asset
                };
                let start = frames(clip["start_ms"].as_u64().ok_or("start_ms is required")?)?;
                let source = frames(
                    clip["source_in_ms"]
                        .as_u64()
                        .ok_or("source_in_ms is required")?,
                )?;
                let length = match clip["length_ms"].as_u64() {
                    Some(ms) => frames(ms)?,
                    None => asset.frames().saturating_sub(source),
                };
                // Saved UI durations are rounded to milliseconds. Permit only
                // that final rounding difference, not arbitrary invalid ranges.
                let available = asset.frames().saturating_sub(source);
                if length > available + (rate as u64 / 1000 + 1) {
                    return Err("clip exceeds source audio".into());
                }
                clips.push(Clip::new(
                    asset,
                    start,
                    source,
                    length.min(available),
                    frames(clip["fade_in_ms"].as_u64().unwrap_or(0))?,
                    frames(clip["fade_out_ms"].as_u64().unwrap_or(0))?,
                )?);
            }
            let mut prepared = Track::new(
                clips,
                track["volume"].as_f64().unwrap_or(1.0) as f32,
                track["pan"].as_f64().unwrap_or(0.0) as f32,
            )?;
            prepared.muted = track["muted"].as_bool().unwrap_or(false);
            prepared.solo = track["solo"].as_bool().unwrap_or(false);
            if let Some(id) = track["id"].as_str() {
                let control = Arc::new(TrackControl::default());
                control.update(track)?;
                prepared.control = Some(control.clone());
                controls.insert(id.to_owned(), control);
            }
            tracks.push(prepared);
        }
        let mut mixer = Mixer::new(tracks);
        mixer.smoothing = 1.0 - (-1.0 / (0.01 * rate as f32)).exp();
        let record = payload["record"].as_bool().unwrap_or(false);
        if record {
            mixer.end = u64::MAX;
        }
        let start = frames(payload["start_ms"].as_u64().unwrap_or(0))?;
        mixer.seek(if !record && start >= mixer.end {
            0
        } else {
            start
        });
        self.looping.store(
            payload["loop"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        if !record && payload["loop"].as_bool().unwrap_or(false) && mixer.end > 0 {
            mixer.set_loop(Some((0, mixer.end)))?;
        }
        mixer.play();
        let delay_ms = payload["recording_delay_ms"].as_u64().unwrap_or(0);
        if delay_ms > 2000 {
            return Err("recording compensation must be 0–2000 ms".into());
        }
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let bpm = payload["bpm"].as_f64().unwrap_or(120.0);
        if !(30.0..=300.0).contains(&bpm) {
            return Err("BPM must be 30–300".into());
        }
        let beats = payload["beats_per_bar"].as_u64().unwrap_or(4).clamp(1, 12);
        let bars = if record {
            payload["count_in_bars"].as_u64().unwrap_or(0).min(4)
        } else {
            0
        };
        let countin = (bars as f64 * beats as f64 * 60.0 * rate as f64 / bpm).round() as u64;
        self.click.store(
            payload["click"].as_bool().unwrap_or(false),
            Ordering::Relaxed,
        );
        self.swap(Session {
            mixer,
            name: payload["name"]
                .as_str()
                .unwrap_or("Timeline take")
                .chars()
                .take(64)
                .collect(),
            preset: payload["preset"]
                .as_str()
                .unwrap_or("Timeline rig")
                .chars()
                .take(256)
                .collect(),
            sample_rate: rate,
            countin_remaining: countin,
            countin_total: countin,
            bpm,
            beats,
            generation,
            recording: if record {
                vec![[0.0; 3]; rate as usize * 180]
            } else {
                vec![]
            },
            recorded: 0,
            recording_start: start,
            delay_frames: frames(delay_ms)?,
        })?;
        *self
            .track_controls
            .lock()
            .map_err(|_| "track controls poisoned")? = controls;
        Ok(self.status())
    }
    pub fn stop(&self, directory: &Path) -> Result<Value, String> {
        let _guard = self.control.lock().map_err(|_| "transport lock poisoned")?;
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        if self.live.load(Ordering::Acquire) {
            let previous = self.swap(Session::empty(generation))?;
            if let Some(session) = previous.filter(|s| s.recorded > 0) {
                *self
                    .pending_take
                    .lock()
                    .map_err(|_| "pending take lock poisoned")? = Some(session);
            }
        }
        let mut pending = self
            .pending_take
            .lock()
            .map_err(|_| "pending take lock poisoned")?;
        let mut result = self.status();
        if let Some(session) = pending.as_ref() {
            let rate = session.sample_rate;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let id = format!("riff-timeline-{stamp}");
            let clean = directory.join(format!("{id}-clean.wav"));
            let wet = directory.join(format!("{id}-rig.wav"));
            let samples = &session.recording[..session.recorded];
            crate::audio_file::write_float_wav(&clean, 1, rate, samples.iter().map(|s| s[0]))?;
            crate::audio_file::write_float_wav(
                &wet,
                2,
                rate,
                samples.iter().flat_map(|s| [s[1], s[2]]),
            )?;
            let (start, source, length) = compensated_placement(
                session.recording_start,
                session.recorded as u64,
                session.delay_frames,
            );
            crate::riff_vault::write_metadata(
                directory,
                &crate::riff_vault::SavedRiff {
                    source_duration_seconds: Some(session.recorded as f32 / rate as f32),
                    id: id.clone(),
                    name: session.name.clone(),
                    created_at_ms: (stamp / 1_000_000) as u64,
                    duration_seconds: session.recorded as f32 / rate as f32,
                    clean_path: clean.to_string_lossy().into_owned(),
                    processed_path: wet.to_string_lossy().into_owned(),
                    preset: session.preset.clone(),
                },
            )?;
            result["take"] = json!({"id":id,"clean_path": clean, "processed_path": wet,
                "duration_seconds": session.recorded as f64 / rate as f64,
                "timeline_start_ms": start * 1000 / rate as u64,
                "source_in_ms": source * 1000 / rate as u64,
                "length_ms": length * 1000 / rate as u64});
            pending.take();
        }
        Ok(result)
    }
}

pub struct TimelineProcessor<P> {
    source: P,
    transport: Arc<Transport>,
    session: Session,
    limiter_gain: f32,
}
impl<P> TimelineProcessor<P> {
    pub fn new(source: P, transport: Arc<Transport>, rate: u32) -> Self {
        transport.rate.store(rate, Ordering::Release);
        transport.live.store(true, Ordering::Release);
        Self {
            source,
            transport,
            session: Session::empty(0),
            limiter_gain: 1.0,
        }
    }
}
impl<P> Drop for TimelineProcessor<P> {
    fn drop(&mut self) {
        if self.session.recorded > 0
            && let Ok(mut pending) = self.transport.pending_take.lock()
        {
            self.transport.interrupted.store(true, Ordering::Release);
            *pending = Some(std::mem::replace(&mut self.session, Session::empty(0)));
        }
        self.transport.live.store(false, Ordering::Release);
        self.transport.playing.store(false, Ordering::Release);
        self.transport.recording.store(false, Ordering::Release);
    }
}
impl<P: MonoProcessor> MonoProcessor for TimelineProcessor<P> {
    fn discontinuity(&mut self) {
        self.session.mixer.pause();
        self.transport.playing.store(false, Ordering::Release);
        self.transport.interrupted.store(true, Ordering::Release);
    }
    fn process(&mut self, samples: &mut [f32]) {
        let mut out = [[0.0; 2]; 1024];
        for chunk in samples.chunks_mut(1024) {
            self.process_stereo(chunk, &mut out[..chunk.len()]);
            for (s, frame) in chunk.iter_mut().zip(&out) {
                *s = (frame[0] + frame[1]) * 0.5;
            }
        }
    }
    fn process_stereo(&mut self, input: &mut [f32], output: &mut [[f32; 2]]) {
        if self.transport.mailbox.swap_pending(&mut self.session) && self.session.mixer.playing() {
            self.transport.interrupted.store(false, Ordering::Release);
        }
        if self.session.recording.is_empty() && self.session.mixer.end > 0 {
            self.session.mixer.loop_range = if self.transport.looping.load(Ordering::Relaxed) {
                Some((0, self.session.mixer.end))
            } else {
                None
            };
        }
        let mut backing = [[0.0; 2]; 1024];
        let mut clean = [0.0; 1024];
        for (input, output) in input.chunks_mut(1024).zip(output.chunks_mut(1024)) {
            clean[..input.len()].copy_from_slice(input);
            self.source.process_stereo(input, output);
            let recording = !self.session.recording.is_empty() && self.session.mixer.playing();
            let counting = self.session.countin_remaining.min(input.len() as u64) as usize;
            backing[..counting].fill([0.0; 2]);
            let start_position = self.session.mixer.position();
            let count_elapsed = self.session.countin_total - self.session.countin_remaining;
            let active = self.session.mixer.playing();
            self.session
                .mixer
                .render(&mut backing[counting..input.len()]);
            self.session.countin_remaining -= counting as u64;
            let release =
                1.0 - (-1.0 / (0.08 * self.transport.rate.load(Ordering::Relaxed) as f32)).exp();
            for (i, frame) in output.iter_mut().enumerate() {
                if recording
                    && i >= counting
                    && self.session.recorded < self.session.recording.len()
                {
                    self.session.recording[self.session.recorded] = [clean[i], frame[0], frame[1]];
                    self.session.recorded += 1;
                }
                for c in 0..2 {
                    frame[c] += backing[i][c];
                    if !frame[c].is_finite() {
                        frame[c] = 0.0;
                    }
                }
                if active && (i < counting || self.transport.click.load(Ordering::Relaxed)) {
                    let mut position = if i < counting {
                        count_elapsed + i as u64
                    } else {
                        start_position + (i - counting) as u64
                    };
                    if i >= counting
                        && let Some((start, end)) = self.session.mixer.loop_range
                        && position >= end
                    {
                        position = start + (position - start) % (end - start);
                    }
                    let rate = self.transport.rate.load(Ordering::Relaxed) as f64;
                    let beat_frames = rate * 60.0 / self.session.bpm;
                    let phase = position as f64 % beat_frames;
                    if phase < rate * 0.03 {
                        let beat = (position as f64 / beat_frames).floor() as u64;
                        let hz = if beat.is_multiple_of(self.session.beats) {
                            1760.0
                        } else {
                            1180.0
                        };
                        let click = (std::f64::consts::TAU * hz * phase / rate).sin()
                            * (1.0 - phase / (rate * 0.03))
                            * 0.18;
                        frame[0] += click as f32;
                        frame[1] += click as f32;
                    }
                }
                let peak = frame[0].abs().max(frame[1].abs());
                let target = if peak > 0.891251 {
                    0.891251 / peak
                } else {
                    1.0
                };
                if target < self.limiter_gain {
                    self.limiter_gain = target;
                } else {
                    self.limiter_gain += (target - self.limiter_gain) * release;
                }
                for s in frame {
                    *s *= self.limiter_gain;
                }
            }
            if recording && self.session.recorded == self.session.recording.len() {
                self.session.mixer.pause();
            }
        }
        self.transport
            .position
            .store(self.session.mixer.position(), Ordering::Release);
        self.transport
            .countin
            .store(self.session.countin_remaining, Ordering::Release);
        self.transport
            .playing
            .store(self.session.mixer.playing(), Ordering::Release);
        self.transport
            .recording
            .store(!self.session.recording.is_empty(), Ordering::Release);
        self.transport
            .acknowledged
            .store(self.session.generation, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct StereoGuitar;
    impl MonoProcessor for StereoGuitar {
        fn process(&mut self, samples: &mut [f32]) {
            for s in samples {
                *s *= 2.0;
            }
        }
        fn process_stereo(&mut self, input: &mut [f32], output: &mut [[f32; 2]]) {
            for (s, f) in input.iter().zip(output) {
                *f = [*s * 2.0, -*s];
            }
        }
    }
    #[test]
    fn transport_load_record_stop_and_recovery_export_real_stereo_files() {
        let root = std::env::temp_dir().join(format!(
            "aero-native-transport-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let transport = Arc::new(Transport::default());
        let running = Arc::new(AtomicBool::new(true));
        let started = Arc::new(AtomicBool::new(false));
        let worker = {
            let t = transport.clone();
            let running = running.clone();
            let started = started.clone();
            std::thread::spawn(move || {
                let mut p = TimelineProcessor::new(StereoGuitar, t, 1000);
                started.store(true, Ordering::Release);
                while running.load(Ordering::Acquire) {
                    p.process_stereo(&mut [0.1; 16], &mut [[0.0; 2]; 16]);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        while !started.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let result = transport
            .start(&json!({"tracks":[],"record":true,"start_ms":100,"recording_delay_ms":20}))
            .unwrap();
        assert_eq!(result["recording"], true);
        // A device shutdown retains the completed portion for an explicit save.
        running.store(false, Ordering::Release);
        worker.join().unwrap();
        let saved = transport.stop(&root).unwrap();
        let take = &saved["take"];
        assert_eq!(take["timeline_start_ms"], 80);
        let clean =
            crate::audio_file::AudioFile::load(Path::new(take["clean_path"].as_str().unwrap()))
                .unwrap();
        let wet =
            crate::audio_file::AudioFile::load(Path::new(take["processed_path"].as_str().unwrap()))
                .unwrap();
        assert_eq!(wet.channels, 2);
        assert_eq!(clean.samples.len() * 2, wet.samples.len());
        assert!(clean.samples.iter().all(|s| *s == 0.1));
        assert!(
            wet.samples
                .as_chunks::<2>()
                .0
                .iter()
                .all(|s| *s == [0.2, -0.1])
        );
        assert!(transport.stop(&root).unwrap().get("take").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_countin_recording_and_backing_use_one_clock_and_separate_taps() {
        let transport = Arc::new(Transport::default());
        let mut processor = TimelineProcessor::new(StereoGuitar, transport.clone(), 48000);
        let mut mixer = Mixer::new(vec![Track::new(vec![clip(0, false)], 0.2, 0.0).unwrap()]);
        mixer.end = u64::MAX;
        mixer.play();
        processor.session = Session {
            mixer,
            name: String::new(),
            preset: String::new(),
            sample_rate: 48000,
            generation: 1,
            recording: vec![[0.0; 3]; 32],
            recorded: 0,
            recording_start: 0,
            delay_frames: 0,
            countin_total: 5,
            countin_remaining: 5,
            bpm: 120.0,
            beats: 4,
        };
        transport.click.store(true, Ordering::Relaxed);
        let mut out = [[0.0; 2]; 12];
        processor.process_stereo(&mut [0.1; 12], &mut out);
        assert_eq!(processor.session.recorded, 7);
        assert_eq!(processor.session.mixer.position(), 7);
        for f in &processor.session.recording[..7] {
            assert_eq!(*f, [0.1, 0.2, -0.1]);
        }
        assert!(out[6][0] > 0.35, "backing did not reach monitor output");
        assert_eq!(transport.status()["count_in_frames"], 0);
        transport.pending_generation.store(9, Ordering::Release);
        transport.mailbox.stage(Session::empty(9));
        processor.process_stereo(&mut [0.0; 2], &mut [[0.0; 2]; 2]);
        transport.recover_pending().unwrap();
        assert_eq!(
            transport
                .pending_take
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .recorded,
            7
        );
    }
    #[test]
    fn atomic_track_updates_smooth_without_restarting_transport() {
        let control = Arc::new(TrackControl::default());
        control.update(&json!({"volume":1.0,"pan":0.0})).unwrap();
        let mut track = Track::new(vec![clip(0, false)], 1.0, 0.0).unwrap();
        track.control = Some(control.clone());
        let mut m = Mixer::new(vec![track]);
        m.set_loop(Some((0, 8))).unwrap();
        m.play();
        m.render(&mut [[0.0; 2]; 4]);
        control.update(&json!({"volume":0.0,"pan":0.0})).unwrap();
        let mut out = [[0.0; 2]; 4];
        m.render(&mut out);
        assert!(out[0][0] < 1.0 && out[0][0] > 0.99);
        assert!(out[3][0] < out[0][0]);
        assert_eq!(m.position(), 8);
    }
    fn clip(start: u64, mono: bool) -> Clip {
        Clip::new(
            Arc::new(Asset {
                samples: vec![[1.0, -0.5]; 8],
                mono,
            }),
            start,
            0,
            8,
            0,
            0,
        )
        .unwrap()
    }
    #[test]
    fn stereo_and_offsets_are_exact_across_callback_boundaries() {
        let mut m = Mixer::new(vec![Track::new(vec![clip(3, false)], 0.5, 0.0).unwrap()]);
        m.play();
        let mut a = [[0.0; 2]; 5];
        let mut b = [[0.0; 2]; 7];
        m.render(&mut a);
        m.render(&mut b);
        assert_eq!(&a[..3], &[[0.0; 2]; 3]);
        assert_eq!(a[3], [0.5, -0.25]);
        assert_eq!(&b[..6], &[[0.5, -0.25]; 6]);
        assert_eq!(b[6], [0.0; 2]);
        assert_eq!(m.position(), 11);
        assert!(!m.playing());
    }
    #[test]
    fn loop_wraps_inside_a_block_and_pause_freezes_clock() {
        let mut m = Mixer::new(vec![Track::new(vec![clip(0, false)], 1.0, 0.0).unwrap()]);
        m.set_loop(Some((2, 5))).unwrap();
        m.seek(4);
        m.play();
        m.render(&mut [[0.0; 2]; 5]);
        assert_eq!(m.position(), 3);
        m.pause();
        let mut out = [[1.0; 2]; 4];
        m.render(&mut out);
        assert_eq!(out, [[0.0; 2]; 4]);
        assert_eq!(m.position(), 3);
    }
    #[test]
    fn mono_pan_solo_and_gain_are_applied() {
        let mut a = Track::new(vec![clip(0, true)], 0.5, -1.0).unwrap();
        a.solo = true;
        let b = Track::new(vec![clip(0, false)], 1.0, 0.0).unwrap();
        let mut m = Mixer::new(vec![a, b]);
        m.play();
        let mut out = [[0.0; 2]; 1];
        m.render(&mut out);
        assert_eq!(out[0], [0.5, 0.0]);
    }
    #[test]
    fn invalid_ranges_and_controls_are_rejected() {
        assert!(Track::new(vec![], f32::NAN, 0.0).is_err());
        let c = clip(0, true);
        assert!(Clip::new(c.asset, 0, 5, 10, 0, 0).is_err());
        assert_eq!(compensated_placement(48000, 96000, 480), (47520, 0, 96000));
        assert_eq!(compensated_placement(100, 1000, 480), (0, 380, 620));
        assert_eq!(compensated_placement(0, 100, 480), (0, 100, 0));
    }
}
