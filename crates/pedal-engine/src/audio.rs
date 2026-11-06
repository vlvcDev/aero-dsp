use std::cmp::Reverse;

pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const PREFERRED_PERIOD_FRAMES: &[u32] = &[64, 128];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub name: String,
    pub description: String,
    pub can_capture: bool,
    pub can_playback: bool,
}

impl DeviceDescriptor {
    pub fn is_duplex(&self) -> bool {
        self.can_capture && self.can_playback
    }

    fn searchable_name(&self) -> String {
        format!("{} {}", self.name, self.description).to_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    pub requested_device: Option<String>,
    pub requested_capture_device: Option<String>,
    pub requested_playback_device: Option<String>,
    pub preferred_device_terms: Vec<String>,
    pub sample_rate: u32,
    pub period_candidates: Vec<u32>,
    pub capture_channels: u32,
    pub playback_channels: u32,
    pub guitar_input_channel: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            requested_device: None,
            requested_capture_device: None,
            requested_playback_device: None,
            preferred_device_terms: vec!["scarlett".to_owned(), "usb audio".to_owned()],
            sample_rate: DEFAULT_SAMPLE_RATE,
            period_candidates: PREFERRED_PERIOD_FRAMES.to_vec(),
            capture_channels: 2,
            playback_channels: 2,
            guitar_input_channel: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedAudio {
    pub device: DeviceDescriptor,
    pub sample_rate: u32,
    pub period_frames: u32,
    pub capture_channels: u32,
    pub playback_channels: u32,
    pub streams_linked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStats {
    pub blocks: u64,
    pub frames: u64,
    pub recoveries: u64,
    pub timed_out: bool,
}

pub fn select_device(
    devices: &[DeviceDescriptor],
    requested: Option<&str>,
    preferred_terms: &[String],
) -> Option<DeviceDescriptor> {
    select_endpoint(devices, requested, preferred_terms, |device| {
        device.is_duplex()
    })
}

fn select_endpoint(
    devices: &[DeviceDescriptor],
    requested: Option<&str>,
    preferred_terms: &[String],
    supports: impl Fn(&DeviceDescriptor) -> bool,
) -> Option<DeviceDescriptor> {
    let eligible: Vec<&DeviceDescriptor> =
        devices.iter().filter(|device| supports(device)).collect();

    if let Some(requested) = requested.filter(|value| !value.eq_ignore_ascii_case("auto")) {
        let requested = requested.to_lowercase();
        if let Some(exact) = eligible
            .iter()
            .find(|device| device.name.eq_ignore_ascii_case(&requested))
        {
            return Some((*exact).clone());
        }
        return eligible
            .into_iter()
            .find(|device| device.searchable_name().contains(&requested))
            .cloned();
    }

    eligible
        .into_iter()
        .enumerate()
        .max_by_key(|(index, device)| {
            let searchable = device.searchable_name();
            let preference = preferred_terms
                .iter()
                .position(|term| searchable.contains(&term.to_lowercase()))
                .map_or(0, |position| preferred_terms.len() - position);
            (preference, Reverse(*index))
        })
        .map(|(_, device)| device.clone())
}

#[cfg(feature = "alsa-backend")]
mod alsa_backend {
    use super::{AudioConfig, DeviceDescriptor, NegotiatedAudio, StreamStats, select_endpoint};
    use crate::dsp::{
        InterleavedLayout, MonoProcessor, Passthrough, RealtimeTelemetry, process_interleaved_i32,
    };
    use alsa::device_name::HintIter;
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::{Direction, ValueOr};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    pub fn enumerate_pcm_devices() -> Result<Vec<DeviceDescriptor>, String> {
        let hints = HintIter::new_str(None, "pcm").map_err(|error| error.to_string())?;
        let mut devices: BTreeMap<String, DeviceDescriptor> = BTreeMap::new();

        for hint in hints {
            let Some(name) = hint.name else { continue };
            if name == "null" {
                continue;
            }
            let entry = devices
                .entry(name.clone())
                .or_insert_with(|| DeviceDescriptor {
                    name,
                    description: hint.desc.clone().unwrap_or_default().replace('\n', " "),
                    can_capture: false,
                    can_playback: false,
                });
            match hint.direction {
                Some(Direction::Capture) => entry.can_capture = true,
                Some(Direction::Playback) => entry.can_playback = true,
                None => {
                    entry.can_capture = true;
                    entry.can_playback = true;
                }
            }
            if entry.description.is_empty() {
                entry.description = hint.desc.unwrap_or_default().replace('\n', " ");
            }
        }

        Ok(devices.into_values().collect())
    }

    pub fn probe(config: &AudioConfig) -> Result<NegotiatedAudio, String> {
        open(config).map(|(_, _, negotiated)| negotiated)
    }

    pub fn run_loopback(
        config: &AudioConfig,
        block_limit: Option<u64>,
        duration_limit: Option<Duration>,
    ) -> Result<StreamStats, String> {
        let mut processor = Passthrough;
        run_processor(config, &mut processor, block_limit, duration_limit)
    }

    pub fn run_processor<P: MonoProcessor + ?Sized>(
        config: &AudioConfig,
        processor: &mut P,
        block_limit: Option<u64>,
        duration_limit: Option<Duration>,
    ) -> Result<StreamStats, String> {
        run_processor_with_telemetry(config, processor, block_limit, duration_limit, None)
    }

    pub fn run_processor_with_telemetry<P: MonoProcessor + ?Sized>(
        config: &AudioConfig,
        processor: &mut P,
        block_limit: Option<u64>,
        duration_limit: Option<Duration>,
        telemetry: Option<&RealtimeTelemetry>,
    ) -> Result<StreamStats, String> {
        let (capture, playback, negotiated) = open(config)?;
        if config.guitar_input_channel >= config.capture_channels as usize {
            return Err(format!(
                "guitar input channel {} is outside the {}-channel capture stream",
                config.guitar_input_channel, config.capture_channels
            ));
        }

        let capture_io = capture.io_i32().map_err(|error| error.to_string())?;
        let playback_io = playback.io_i32().map_err(|error| error.to_string())?;
        let frames = negotiated.period_frames as usize;
        let capture_channels = negotiated.capture_channels as usize;
        let playback_channels = negotiated.playback_channels as usize;
        let mut input = vec![0_i32; frames * capture_channels];
        let mut output = vec![0_i32; frames * playback_channels];
        let mut mono_workspace = vec![0.0_f32; frames];
        let mut stats = StreamStats {
            blocks: 0,
            frames: 0,
            recoveries: 0,
            timed_out: false,
        };
        let started_at = Instant::now();

        if negotiated.streams_linked {
            prime_linked_playback(&playback, &playback_io, playback_channels, frames)?;
        } else {
            capture.start().map_err(|error| error.to_string())?;
        }
        loop {
            if duration_limit.is_some_and(|limit| started_at.elapsed() >= limit) {
                stats.timed_out = true;
                return Ok(stats);
            }
            match capture.wait(Some(20)) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    stats.recoveries += 1;
                    processor.discontinuity();
                    if let Some(telemetry) = telemetry {
                        telemetry.record_xrun();
                    }
                    capture
                        .try_recover(error, true)
                        .map_err(|e| e.to_string())?;
                    continue;
                }
            }
            let captured = match capture_io.readi(&mut input) {
                Ok(captured) => captured,
                Err(error) => {
                    stats.recoveries += 1;
                    processor.discontinuity();
                    if let Some(telemetry) = telemetry {
                        telemetry.record_xrun();
                    }
                    capture
                        .try_recover(error, true)
                        .map_err(|e| e.to_string())?;
                    continue;
                }
            };

            let processing_started = Instant::now();
            process_interleaved_i32(
                &input,
                &mut output,
                &mut mono_workspace,
                InterleavedLayout {
                    frames: captured,
                    capture_channels,
                    playback_channels,
                    guitar_input_channel: config.guitar_input_channel,
                },
                processor,
            )
            .map_err(str::to_owned)?;
            if let Some(telemetry) = telemetry.filter(|_| captured > 0) {
                let block_seconds = captured as f32 / negotiated.sample_rate as f32;
                let cpu_percent =
                    processing_started.elapsed().as_secs_f32() / block_seconds * 100.0;
                telemetry.record_cpu_percent(cpu_percent);
            }

            let mut written = 0;
            while written < captured {
                if duration_limit.is_some_and(|limit| started_at.elapsed() >= limit) {
                    stats.timed_out = true;
                    return Ok(stats);
                }
                match playback.wait(Some(20)) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        stats.recoveries += 1;
                        processor.discontinuity();
                        if let Some(telemetry) = telemetry {
                            telemetry.record_xrun();
                        }
                        playback
                            .try_recover(error, true)
                            .map_err(|e| e.to_string())?;
                        continue;
                    }
                }
                match playback_io
                    .writei(&output[written * playback_channels..captured * playback_channels])
                {
                    Ok(0) => return Err("ALSA playback accepted zero frames".to_owned()),
                    Ok(count) => written += count,
                    Err(error) => {
                        stats.recoveries += 1;
                        processor.discontinuity();
                        if let Some(telemetry) = telemetry {
                            telemetry.record_xrun();
                        }
                        playback
                            .try_recover(error, true)
                            .map_err(|e| e.to_string())?;
                    }
                }
            }

            stats.blocks += 1;
            stats.frames += captured as u64;
            if block_limit.is_some_and(|limit| stats.blocks >= limit) {
                return Ok(stats);
            }
        }
    }

    fn prime_linked_playback(
        playback: &PCM,
        playback_io: &alsa::pcm::IO<'_, i32>,
        channels: usize,
        period_frames: usize,
    ) -> Result<(), String> {
        let hardware = playback
            .hw_params_current()
            .map_err(|error| error.to_string())?;
        let buffer_frames = usize::try_from(
            hardware
                .get_buffer_size()
                .map_err(|error| error.to_string())?,
        )
        .map_err(|_| "ALSA returned an invalid playback buffer size".to_owned())?;
        let software = playback
            .sw_params_current()
            .map_err(|error| error.to_string())?;
        software
            .set_start_threshold(buffer_frames as i64)
            .map_err(|error| error.to_string())?;
        software
            .set_avail_min(period_frames as i64)
            .map_err(|error| error.to_string())?;
        playback
            .sw_params(&software)
            .map_err(|error| error.to_string())?;

        let silence = vec![0_i32; period_frames * channels];
        let mut primed = 0;
        while primed < buffer_frames {
            let frames = (buffer_frames - primed).min(period_frames);
            let written = playback_io
                .writei(&silence[..frames * channels])
                .map_err(|error| error.to_string())?;
            if written == 0 {
                return Err("ALSA playback accepted zero frames while priming".to_owned());
            }
            primed += written;
        }
        Ok(())
    }

    fn open(config: &AudioConfig) -> Result<(PCM, PCM, NegotiatedAudio), String> {
        let devices = enumerate_pcm_devices()?;
        let shared_request = config.requested_device.as_deref();
        let capture_device = select_endpoint(
            &devices,
            config
                .requested_capture_device
                .as_deref()
                .or(shared_request),
            &config.preferred_device_terms,
            |device| device.can_capture,
        )
        .ok_or_else(|| "no matching ALSA capture device found".to_owned())?;
        let playback_device = select_endpoint(
            &devices,
            config
                .requested_playback_device
                .as_deref()
                .or(shared_request),
            &config.preferred_device_terms,
            |device| device.can_playback,
        )
        .ok_or_else(|| "no matching ALSA playback device found".to_owned())?;

        let mut failures = Vec::new();
        for period_frames in &config.period_candidates {
            match open_period(&capture_device, &playback_device, config, *period_frames) {
                Ok(opened) => return Ok(opened),
                Err(error) => failures.push(format!("{period_frames} frames: {error}")),
            }
        }
        Err(format!(
            "{} -> {} could not meet the requested audio configuration ({})",
            capture_device.name,
            playback_device.name,
            failures.join("; ")
        ))
    }

    fn open_period(
        capture_device: &DeviceDescriptor,
        playback_device: &DeviceDescriptor,
        config: &AudioConfig,
        period_frames: u32,
    ) -> Result<(PCM, PCM, NegotiatedAudio), String> {
        let capture =
            PCM::new(&capture_device.name, Direction::Capture, false).map_err(|e| e.to_string())?;
        let playback = PCM::new(&playback_device.name, Direction::Playback, false)
            .map_err(|e| e.to_string())?;

        let capture_period = configure_pcm(
            &capture,
            config.capture_channels,
            config.sample_rate,
            period_frames,
        )?;
        let playback_period = configure_pcm(
            &playback,
            config.playback_channels,
            config.sample_rate,
            period_frames,
        )?;

        if capture_period != playback_period {
            return Err(format!(
                "capture negotiated {capture_period} frames but playback negotiated {playback_period}"
            ));
        }

        let streams_linked =
            capture_device.name == playback_device.name && capture.link(&playback).is_ok();
        capture.prepare().map_err(|error| error.to_string())?;
        playback.prepare().map_err(|error| error.to_string())?;

        let device = if capture_device.name == playback_device.name {
            capture_device.clone()
        } else {
            DeviceDescriptor {
                name: format!("{} -> {}", capture_device.name, playback_device.name),
                description: format!(
                    "{} capture to {} playback",
                    capture_device.description, playback_device.description
                ),
                can_capture: true,
                can_playback: true,
            }
        };
        let negotiated = NegotiatedAudio {
            device,
            sample_rate: config.sample_rate,
            period_frames: capture_period,
            capture_channels: config.capture_channels,
            playback_channels: config.playback_channels,
            streams_linked,
        };
        Ok((capture, playback, negotiated))
    }

    fn configure_pcm(
        pcm: &PCM,
        channels: u32,
        sample_rate: u32,
        period_frames: u32,
    ) -> Result<u32, String> {
        let params = HwParams::any(pcm).map_err(|error| error.to_string())?;
        params
            .set_access(Access::RWInterleaved)
            .map_err(|e| e.to_string())?;
        params
            .set_format(Format::s32())
            .map_err(|e| e.to_string())?;
        params.set_channels(channels).map_err(|e| e.to_string())?;
        let actual_rate = params
            .set_rate_near(sample_rate, ValueOr::Nearest)
            .map_err(|error| error.to_string())?;
        if actual_rate != sample_rate {
            return Err(format!(
                "requested {sample_rate} Hz, device offered {actual_rate} Hz"
            ));
        }
        let actual_period = params
            .set_period_size_near(i64::from(period_frames), ValueOr::Nearest)
            .map_err(|error| error.to_string())?;
        params
            .set_buffer_size_near(actual_period * 3)
            .map_err(|error| error.to_string())?;
        pcm.hw_params(&params).map_err(|error| error.to_string())?;
        u32::try_from(actual_period).map_err(|_| "ALSA returned an invalid period size".to_owned())
    }
}

#[cfg(feature = "alsa-backend")]
pub use alsa_backend::{
    enumerate_pcm_devices, probe, run_loopback, run_processor, run_processor_with_telemetry,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, description: &str, duplex: bool) -> DeviceDescriptor {
        DeviceDescriptor {
            name: name.to_owned(),
            description: description.to_owned(),
            can_capture: duplex,
            can_playback: true,
        }
    }

    #[test]
    fn auto_selection_prefers_scarlett() {
        let devices = vec![
            device("default", "Built-in Audio", true),
            device("hw:CARD=USB", "Scarlett 2i2 USB", true),
        ];
        let selected = select_device(
            &devices,
            None,
            &AudioConfig::default().preferred_device_terms,
        )
        .expect("expected an audio device");
        assert_eq!(selected.name, "hw:CARD=USB");
    }

    #[test]
    fn explicit_selection_wins_over_preference() {
        let devices = vec![
            device("studio", "Scarlett 2i2 USB", true),
            device("portable", "Generic USB Audio", true),
        ];
        let selected = select_device(
            &devices,
            Some("portable"),
            &AudioConfig::default().preferred_device_terms,
        )
        .expect("expected explicitly selected device");
        assert_eq!(selected.name, "portable");
    }

    #[test]
    fn explicit_name_wins_over_an_earlier_description_match() {
        let devices = vec![
            device("default", "Default output through PipeWire", true),
            device("pipewire", "PipeWire Sound Server", true),
        ];
        let selected =
            select_device(&devices, Some("pipewire"), &[]).expect("expected the exact device name");
        assert_eq!(selected.name, "pipewire");
    }

    #[test]
    fn playback_only_devices_are_not_selected() {
        let devices = vec![device("speaker", "USB Audio", false)];
        assert!(select_device(&devices, None, &["usb".to_owned()]).is_none());
    }
}
