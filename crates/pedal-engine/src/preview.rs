use crate::EngineState;
use crate::audio_file::{AudioFile, resample, write_float_wav};
use crate::cabinet::CabinetImpulse;
use crate::dsp::{AmpProcessor, MonoProcessor, RealtimeTelemetry};
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 64;
const DURATION_SECONDS: f32 = 4.25;
const MAX_INPUT_SECONDS: usize = 180;

pub struct PreviewResult {
    pub path: PathBuf,
    pub duration_seconds: f32,
}

pub fn render(state: &EngineState, destination: &Path) -> Result<PreviewResult, String> {
    render_source(state, destination, None, true, None, None)
}

/// Static post-rig matching from the same reference performance for every rig.
/// Not an AGC: it never rides the player's level or alters NAM drive/EQ settings.
pub fn audition_level(state: &EngineState, destination: &Path) -> Result<f32, String> {
    if state.bypassed {
        return Err("Engage the rig before matching its level".into());
    }
    let mut reference = state.clone();
    reference.rig_level_db = 0.0;
    reference.render_quality_profile = 3; // Match the live rig, not a higher export profile.
    reference.render_nam_quality = 2;
    render_source_inner(&reference, destination, None, true, None, None, false)?;
    let audio = AudioFile::load(destination)?;
    let frames = ((DURATION_SECONDS * SAMPLE_RATE as f32) as usize * audio.channels)
        .min(audio.samples.len());
    let signal = &audio.samples[..frames];
    let rms =
        (signal.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / frames.max(1) as f64).sqrt();
    if rms < 0.001 {
        return Err(
            "Reference is too quiet to level-match safely; check the gate and models".into(),
        );
    }
    let peak = signal.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
    let target = (-18.0 - 20.0 * rms.log10()) as f32;
    let headroom = -1.0 - 20.0 * peak.max(1e-9).log10();
    Ok(target.min(headroom).clamp(-24.0, 6.0))
}

pub fn render_source(
    state: &EngineState,
    destination: &Path,
    source: Option<&Path>,
    processed: bool,
    start_seconds: Option<f32>,
    end_seconds: Option<f32>,
) -> Result<PreviewResult, String> {
    render_source_inner(
        state,
        destination,
        source,
        processed,
        start_seconds,
        end_seconds,
        true,
    )
}

fn render_source_inner(
    state: &EngineState,
    destination: &Path,
    source: Option<&Path>,
    processed: bool,
    start_seconds: Option<f32>,
    end_seconds: Option<f32>,
    include_tails: bool,
) -> Result<PreviewResult, String> {
    let (mut samples, channels) = match source {
        Some(path) => {
            let mut audio = AudioFile::load(path)?;
            audio
                .samples
                .truncate(audio.sample_rate as usize * MAX_INPUT_SECONDS * audio.channels);
            if processed {
                (
                    resample(&audio.mono(), 1, audio.sample_rate, SAMPLE_RATE)?,
                    1,
                )
            } else {
                (
                    resample(
                        &audio.samples,
                        audio.channels,
                        audio.sample_rate,
                        SAMPLE_RATE,
                    )?,
                    audio.channels,
                )
            }
        }
        None => (guitar_riff(), 1),
    };
    if samples.is_empty() {
        return Err("WAV contains no audio".into());
    }
    if start_seconds.is_some() || end_seconds.is_some() {
        let start_seconds = start_seconds.unwrap_or(0.0).max(0.0);
        let end_seconds = end_seconds
            .unwrap_or(samples.len() as f32 / channels as f32 / SAMPLE_RATE as f32)
            .max(start_seconds);
        let start = (start_seconds * SAMPLE_RATE as f32).round() as usize * channels;
        let end =
            ((end_seconds * SAMPLE_RATE as f32).round() as usize * channels).min(samples.len());
        if start >= end {
            return Err("the selected preview range is empty".to_owned());
        }
        samples = samples[start..end].to_vec();
    }
    if !processed {
        if channels == 1 {
            write_stereo_wav(destination, &samples, SAMPLE_RATE)?;
        } else {
            write_float_wav(
                destination,
                channels as u16,
                SAMPLE_RATE,
                samples.iter().copied(),
            )?;
        }
        return Ok(PreviewResult {
            path: destination.to_owned(),
            duration_seconds: samples.len() as f32 / channels as f32 / SAMPLE_RATE as f32,
        });
    }

    let pre = load_model(
        state.pre_model.as_deref(),
        state.pre_bypassed || state.bypassed,
        crate::quality::render_nam_size(state),
        true,
    )?;
    let amp = load_model(
        state.model.as_deref(),
        state.amp_bypassed || state.bypassed,
        crate::quality::render_nam_size(state),
        false,
    )?;
    let cabinet = match state.cabinet_ir.as_deref() {
        Some(path) => Some(
            CabinetImpulse::load_with_duration(
                Path::new(path),
                SAMPLE_RATE,
                crate::quality::ir_seconds(if state.render_quality_profile == 3 {
                    state.quality_profile
                } else {
                    state.render_quality_profile
                }),
            )
            .map_err(|error| format!("could not prepare preview cabinet {path}: {error}"))?,
        ),
        None => None,
    };
    // A preview starts with an empty in-memory loop; it still renders the
    // rest of the board exactly as the live engine does.
    let telemetry = Arc::new(RealtimeTelemetry::default());
    let mut processor = crate::rig::build(
        pre,
        amp,
        cabinet,
        crate::rig::RigControls::from_state_at_rate(state, SAMPLE_RATE),
        SAMPLE_RATE,
        BLOCK_FRAMES,
        telemetry,
    );

    let mut stereo = vec![[0.0; 2]; samples.len()];
    for (block, out) in samples
        .chunks_mut(BLOCK_FRAMES)
        .zip(stereo.chunks_mut(BLOCK_FRAMES))
    {
        processor.process_stereo(block, out);
    }
    if include_tails {
        append_tails(&mut processor, state, &mut stereo);
    }
    write_float_wav(destination, 2, SAMPLE_RATE, stereo.iter().flat_map(|f| *f))?;
    Ok(PreviewResult {
        path: destination.to_owned(),
        duration_seconds: stereo.len() as f32 / SAMPLE_RATE as f32,
    })
}

fn append_tails(processor: &mut impl MonoProcessor, s: &EngineState, samples: &mut Vec<[f32; 2]>) {
    if s.bypassed {
        return;
    }
    let delay = if s.delay_enabled {
        s.delay_time_ms / 1000.0
    } else {
        0.0
    };
    let echoes = if s.delay_enabled && s.delay_feedback > 0.01 {
        (0.0001_f32.ln() / s.delay_feedback.clamp(0.01, 0.92).ln()).ceil()
    } else {
        1.0
    };
    let reverb = if s.reverb_enabled {
        s.reverb_decay_seconds * 1.5
    } else {
        0.0
    };
    let cabinet = if s.cabinet_enabled {
        crate::quality::ir_seconds(if s.render_quality_profile == 3 {
            s.quality_profile
        } else {
            s.render_quality_profile
        })
    } else {
        0.0
    };
    let octave = if s.octaver_enabled && s.octaver_clean {
        crate::octaver::clean_latency_frames(SAMPLE_RATE) as f32 / SAMPLE_RATE as f32
    } else {
        0.0
    };
    let modulation =
        if s.chorus_enabled { 0.04 } else { 0.0 } + if s.flanger_enabled { 0.012 } else { 0.0 };
    let maximum = (delay * echoes + reverb + cabinet + octave + modulation).clamp(0.0, 30.0);
    if maximum == 0.0 {
        return;
    }
    let minimum = ((delay + cabinet + octave + 0.25) * SAMPLE_RATE as f32) as usize;
    let limit = (maximum * SAMPLE_RATE as f32).ceil() as usize;
    let mut quiet = 0;
    let mut rendered = 0;
    while rendered < limit {
        let frames = (limit - rendered).min(BLOCK_FRAMES);
        let mut tail = [0.0; BLOCK_FRAMES];
        let mut stereo = [[0.0; 2]; BLOCK_FRAMES];
        processor.process_stereo(&mut tail[..frames], &mut stereo[..frames]);
        samples.extend_from_slice(&stereo[..frames]);
        rendered += frames;
        if stereo[..frames].iter().flatten().all(|s| s.abs() < 0.00001) {
            quiet += frames;
        } else {
            quiet = 0;
        }
        if rendered >= minimum && quiet >= SAMPLE_RATE as usize / 4 {
            break;
        }
    }
    let fade = (SAMPLE_RATE as usize / 100).min(rendered);
    let start = samples.len() - fade;
    for (i, frame) in samples[start..].iter_mut().enumerate() {
        for sample in frame {
            *sample *= 1.0 - i as f32 / fade.saturating_sub(1).max(1) as f32;
        }
    }
}

fn load_model(
    path: Option<&str>,
    bypassed: bool,
    quality: nam_bridge::NamQuality,
    alignment: bool,
) -> Result<AmpProcessor, String> {
    if bypassed || path.is_none() {
        return Ok(AmpProcessor::Clean);
    }
    let path = path.expect("checked above");
    crate::quality::load_nam(
        path,
        f64::from(SAMPLE_RATE),
        BLOCK_FRAMES,
        quality,
        alignment,
    )
    .map(AmpProcessor::Nam)
    .map_err(|error| format!("could not prepare preview model {path}: {error}"))
}

fn guitar_riff() -> Vec<f32> {
    let frames = (SAMPLE_RATE as f32 * DURATION_SECONDS) as usize;
    let mut output = vec![0.0; frames];
    let notes = [
        (0.15, 82.41),
        (0.61, 110.00),
        (1.07, 146.83),
        (1.53, 164.81),
        (1.99, 146.83),
        (2.45, 110.00),
        (2.91, 82.41),
        (3.37, 82.41),
        (3.37, 123.47),
        (3.37, 164.81),
    ];
    for (index, (start, frequency)) in notes.into_iter().enumerate() {
        add_pluck(&mut output, start, frequency, 0x9e37_79b9 ^ index as u32);
    }
    output
}

fn add_pluck(output: &mut [f32], start_seconds: f32, frequency: f32, mut seed: u32) {
    let start = (start_seconds * SAMPLE_RATE as f32) as usize;
    let delay_length = (SAMPLE_RATE as f32 / frequency).round() as usize;
    let mut delay = vec![0.0_f32; delay_length.max(2)];
    for sample in &mut delay {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *sample = ((seed >> 8) as f32 / 16_777_215.0 * 2.0 - 1.0) * 0.11;
    }
    let pluck_frames = (SAMPLE_RATE as f32 * 0.92) as usize;
    for offset in 0..pluck_frames.min(output.len().saturating_sub(start)) {
        let position = offset % delay.len();
        let next = (position + 1) % delay.len();
        let value = delay[position];
        delay[position] = (delay[position] + delay[next]) * 0.4985;
        let attack = (offset as f32 / 96.0).min(1.0);
        output[start + offset] += value * attack;
    }
}

fn write_stereo_wav(path: &Path, mono: &[f32], sample_rate: u32) -> Result<(), String> {
    write_float_wav(path, 2, sample_rate, mono.iter().flat_map(|s| [*s, *s]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_preserves_stereo_float_samples_and_headroom() {
        let stem = format!("pedal-preview-stereo-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{stem}.wav"));
        let output = std::env::temp_dir().join(format!("{stem}-out.wav"));
        let samples: Vec<_> = (0..4800)
            .flat_map(|i| {
                let s = (i as f32 * 0.1).sin() * 1.2;
                [s, -s]
            })
            .collect();
        write_float_wav(&input, 2, 48_000, samples.iter().copied()).unwrap();
        render_source(
            &EngineState::default(),
            &output,
            Some(&input),
            false,
            Some(0.025),
            Some(0.075),
        )
        .unwrap();
        let out = AudioFile::load(&output).unwrap();
        assert_eq!(out.channels, 2);
        assert_eq!(out.samples, &samples[1200 * 2..3600 * 2]);
        assert!(out.samples.iter().any(|s| s.abs() > 1.0));
        let peaks = crate::riff_vault::waveform_peaks(&output, 32).unwrap();
        assert!(peaks.iter().all(|p| *p > 1.0));
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn clean_preview_is_a_non_silent_stereo_wav() {
        let path =
            std::env::temp_dir().join(format!("pedal-preview-test-{}.wav", std::process::id()));
        let result = render(&EngineState::default(), &path).expect("preview should render");
        let bytes = fs::read(&path).expect("preview should be readable");
        fs::remove_file(&path).ok();

        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
        assert!((4.2..4.3).contains(&result.duration_seconds));
    }

    #[test]
    fn dry_wav_input_is_resampled_and_rendered() {
        let stem = format!("pedal-preview-input-test-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{stem}.wav"));
        let output = std::env::temp_dir().join(format!("{stem}-out.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&input, spec).expect("test WAV should open");
        for frame in 0..44_100 {
            let sample =
                ((frame as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 8_000.0) as i16;
            writer.write_sample(sample).expect("sample should write");
        }
        writer.finalize().expect("test WAV should finalize");

        let result = render_source(
            &EngineState::default(),
            &output,
            Some(&input),
            false,
            None,
            None,
        )
        .expect("input should render");
        let bytes = fs::read(&output).expect("output should be readable");
        fs::remove_file(input).ok();
        fs::remove_file(output).ok();

        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert!((0.99..1.01).contains(&result.duration_seconds));
    }

    #[test]
    fn preview_can_render_only_the_selected_take_range() {
        let stem = format!("pedal-preview-slice-test-{}", std::process::id());
        let input = std::env::temp_dir().join(format!("{stem}.wav"));
        let output = std::env::temp_dir().join(format!("{stem}-out.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&input, spec).expect("test WAV should open");
        for frame in 0..48_000 {
            writer
                .write_sample((frame as i16).wrapping_mul(11))
                .expect("sample should write");
        }
        writer.finalize().expect("test WAV should finalize");

        let result = render_source(
            &EngineState::default(),
            &output,
            Some(&input),
            false,
            Some(0.25),
            Some(0.75),
        )
        .expect("selection should render");
        fs::remove_file(input).ok();
        fs::remove_file(output).ok();

        assert!((0.49..0.51).contains(&result.duration_seconds));
    }
}
