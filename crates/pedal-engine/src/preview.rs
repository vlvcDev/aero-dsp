use crate::EngineState;
use crate::cabinet::{CabinetImpulse, CabinetParameters, CabinetProcessor};
use crate::dsp::{
    AmpControlProcessor, AmpParameters, AmpProcessor, BypassProcessor, MonoProcessor,
    PedalControlProcessor, PedalParameters, RealtimeTelemetry, SafetyProcessor, SerialProcessor,
    SwapMailbox,
};
use crate::effects::{EffectChainProcessor, EffectsParameters};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 64;
const DURATION_SECONDS: f32 = 4.25;
const MAX_INPUT_SECONDS: usize = 60;

pub struct PreviewResult {
    pub path: PathBuf,
    pub duration_seconds: f32,
}

pub fn render(state: &EngineState, destination: &Path) -> Result<PreviewResult, String> {
    render_source(state, destination, None, true)
}

pub fn render_source(
    state: &EngineState,
    destination: &Path,
    source: Option<&Path>,
    processed: bool,
) -> Result<PreviewResult, String> {
    let mut samples = match source {
        Some(path) => read_wav_mono(path)?,
        None => guitar_riff(),
    };
    if !processed {
        write_stereo_wav(destination, &samples, SAMPLE_RATE)?;
        return Ok(PreviewResult {
            path: destination.to_owned(),
            duration_seconds: samples.len() as f32 / SAMPLE_RATE as f32,
        });
    }

    let pre = load_model(
        state.pre_model.as_deref(),
        state.pre_bypassed || state.bypassed,
    )?;
    let amp = load_model(state.model.as_deref(), state.amp_bypassed || state.bypassed)?;
    let pedal_parameters = Arc::new(PedalParameters::new(
        state.pedal_drive_db,
        state.pedal_mix,
        state.pedal_level_db,
    ));
    let amp_parameters = Arc::new(AmpParameters::new(
        state.amp_bass_db,
        state.amp_mid_db,
        state.amp_treble_db,
        state.amp_volume_db,
    ));
    let effects_parameters = Arc::new(EffectsParameters::from_state(state));
    let cabinet_parameters = Arc::new(CabinetParameters::new(
        state.cabinet_enabled,
        state.cabinet_gain_db,
        state.cabinet_low_cut_hz,
        state.cabinet_high_cut_hz,
    ));
    let cabinet = match state.cabinet_ir.as_deref() {
        Some(path) => Some(
            CabinetImpulse::load(Path::new(path), SAMPLE_RATE)
                .map_err(|error| format!("could not prepare preview cabinet {path}: {error}"))?,
        ),
        None => None,
    };
    let pre = PedalControlProcessor::new(pre, pedal_parameters, BLOCK_FRAMES);
    let pre = BypassProcessor::new(pre, Arc::new(AtomicBool::new(state.pre_bypassed)));
    let amp = AmpControlProcessor::new(amp, amp_parameters, SAMPLE_RATE);
    let amp = BypassProcessor::new(amp, Arc::new(AtomicBool::new(state.amp_bypassed)));
    let chain = SerialProcessor::new(pre, amp);
    let chain = CabinetProcessor::new(
        chain,
        cabinet,
        cabinet_parameters,
        Arc::new(SwapMailbox::default()),
        SAMPLE_RATE,
    );
    // A preview starts with an empty in-memory loop; it still renders the
    // rest of the board exactly as the live engine does.
    let chain = EffectChainProcessor::new(chain, effects_parameters, SAMPLE_RATE, 30);
    let chain = BypassProcessor::new(chain, Arc::new(AtomicBool::new(state.bypassed)));
    let telemetry = Arc::new(RealtimeTelemetry::default());
    let mut processor = SafetyProcessor::new(chain, telemetry, SAMPLE_RATE);

    for block in samples.chunks_mut(BLOCK_FRAMES) {
        processor.process(block);
    }
    write_stereo_wav(destination, &samples, SAMPLE_RATE)?;
    Ok(PreviewResult {
        path: destination.to_owned(),
        duration_seconds: samples.len() as f32 / SAMPLE_RATE as f32,
    })
}

fn read_wav_mono(path: &Path) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|error| format!("could not open WAV {}: {error}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        return Err("WAV has an invalid channel count or sample rate".to_owned());
    }
    let interleaved = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|sample| sample.map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 16 => reader
            .samples::<i16>()
            .map(|sample| {
                sample
                    .map(|value| value as f32 / i16::MAX as f32)
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?,
        hound::SampleFormat::Int if spec.bits_per_sample <= 32 => {
            let scale = ((1_i64 << (spec.bits_per_sample - 1)) - 1) as f32;
            reader
                .samples::<i32>()
                .map(|sample| {
                    sample
                        .map(|value| value as f32 / scale)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        _ => return Err("WAV must use 16-, 24-, or 32-bit PCM/float audio".to_owned()),
    };
    let channels = usize::from(spec.channels);
    let mut mono = Vec::with_capacity(interleaved.len() / channels);
    for frame in interleaved.chunks_exact(channels) {
        mono.push(frame.iter().sum::<f32>() / channels as f32);
    }
    if mono.is_empty() {
        return Err("WAV contains no audio".to_owned());
    }
    let limit = spec.sample_rate as usize * MAX_INPUT_SECONDS;
    mono.truncate(limit);
    Ok(resample_linear(&mono, spec.sample_rate, SAMPLE_RATE))
}

fn resample_linear(input: &[f32], source_rate: u32, destination_rate: u32) -> Vec<f32> {
    if source_rate == destination_rate || input.len() < 2 {
        return input.to_vec();
    }
    let output_len = input.len() * destination_rate as usize / source_rate as usize;
    let ratio = source_rate as f64 / destination_rate as f64;
    (0..output_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let lower = position.floor() as usize;
            let upper = (lower + 1).min(input.len() - 1);
            let fraction = (position - lower as f64) as f32;
            input[lower] + (input[upper] - input[lower]) * fraction
        })
        .collect()
}

fn load_model(path: Option<&str>, bypassed: bool) -> Result<AmpProcessor, String> {
    if bypassed || path.is_none() {
        return Ok(AmpProcessor::Clean);
    }
    let path = path.expect("checked above");
    nam_bridge::NamModel::load(path, f64::from(SAMPLE_RATE), BLOCK_FRAMES)
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("wav.new");
    let file = File::create(&temporary).map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    let data_bytes = u32::try_from(mono.len().saturating_mul(4))
        .map_err(|_| "preview is too large for a WAV file".to_owned())?;
    writer
        .write_all(b"RIFF")
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&(36_u32 + data_bytes).to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(b"WAVEfmt ")
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&16_u32.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&1_u16.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&2_u16.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&sample_rate.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&(sample_rate * 4).to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&4_u16.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&16_u16.to_le_bytes())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(b"data")
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&data_bytes.to_le_bytes())
        .map_err(|error| error.to_string())?;
    for sample in mono {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        writer
            .write_all(&pcm.to_le_bytes())
            .and_then(|_| writer.write_all(&pcm.to_le_bytes()))
            .map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let result = render_source(&EngineState::default(), &output, Some(&input), false)
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
}
