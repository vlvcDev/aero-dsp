//! Shared offline WAV decoding and band-limited conversion. Never call these
//! allocating/file-I/O helpers on the audio thread.
use rubato::{FftFixedInOut, Resampler};
use std::path::Path;

/// Float masters retain headroom and avoid repeated quantization on edits.
pub fn write_float_wav(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    samples: impl IntoIterator<Item = f32>,
) -> Result<(), String> {
    if channels == 0 || sample_rate == 0 {
        return Err("invalid WAV format".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temporary = path.with_extension("wav.new");
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&temporary, spec).map_err(|e| e.to_string())?;
    for sample in samples {
        if !sample.is_finite() {
            return Err("cannot save nonfinite audio".into());
        }
        writer.write_sample(sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    std::fs::File::open(&temporary)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}

pub struct AudioFile {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}

impl AudioFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
        let spec = reader.spec();
        if spec.channels == 0
            || spec.channels > 32
            || spec.sample_rate == 0
            || spec.sample_rate > 384_000
        {
            return Err("unsupported WAV channel count or sample rate".into());
        }
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?,
            hound::SampleFormat::Int if (1..=32).contains(&spec.bits_per_sample) => {
                let scale = (1_u64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|s| s as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?
            }
            _ => return Err("unsupported WAV encoding".into()),
        };
        if samples.iter().any(|s| !s.is_finite()) {
            return Err("WAV contains nonfinite audio samples".into());
        }
        Ok(Self {
            samples,
            channels: spec.channels as usize,
            sample_rate: spec.sample_rate,
        })
    }

    pub fn mono(&self) -> Vec<f32> {
        self.samples
            .chunks_exact(self.channels)
            .map(|frame| frame.iter().sum::<f32>() / self.channels as f32)
            .collect()
    }
}

/// Filtered, delay-compensated conversion with exact rounded frame count.
/// Channels remain independent, including opposite-polarity stereo material.
pub fn resample(
    samples: &[f32],
    channels: usize,
    source_rate: u32,
    destination_rate: u32,
) -> Result<Vec<f32>, String> {
    if channels == 0
        || channels > 32
        || !samples.len().is_multiple_of(channels)
        || source_rate == 0
        || destination_rate == 0
        || source_rate > 384_000
        || destination_rate > 384_000
    {
        return Err("invalid resampling format".into());
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err("cannot resample nonfinite audio".into());
    }
    if source_rate == destination_rate || samples.is_empty() {
        return Ok(samples.to_vec());
    }
    let frames = samples.len() / channels;
    let wanted = ((frames as u64 * u64::from(destination_rate) + u64::from(source_rate) / 2)
        / u64::from(source_rate)) as usize;
    let mut converter = FftFixedInOut::<f32>::new(
        source_rate as usize,
        destination_rate as usize,
        1024,
        channels,
    )
    .map_err(|e| e.to_string())?;
    let delay = converter.output_delay();
    let mut input = converter.input_buffer_allocate(true);
    let mut output = converter.output_buffer_allocate(true);
    let mut result = Vec::with_capacity(wanted * channels);
    let mut consumed = 0;
    let mut produced = 0;
    while produced < wanted + delay {
        let count = converter.input_frames_next();
        for (channel, buffer) in input.iter_mut().enumerate() {
            buffer.fill(0.0);
            for (offset, sample) in buffer.iter_mut().take(count).enumerate() {
                if consumed + offset < frames {
                    *sample = samples[(consumed + offset) * channels + channel];
                }
            }
        }
        let (_, written) = converter
            .process_into_buffer(&input, &mut output, None)
            .map_err(|e| e.to_string())?;
        for frame in 0..written {
            if (delay..delay + wanted).contains(&(produced + frame)) {
                for channel in &output {
                    result.push(channel[frame]);
                }
            }
        }
        consumed += count;
        produced += written;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sine(rate: usize, hz: f64) -> Vec<f32> {
        (0..rate)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5)
            .collect()
    }
    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|s| s * s).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn conversion_rejects_aliases_without_attenuating_passband() {
        let stop = resample(&sine(96_000, 30_000.0), 1, 96_000, 48_000).unwrap();
        let pass = resample(&sine(96_000, 1_000.0), 1, 96_000, 48_000).unwrap();
        assert_eq!(stop.len(), 48_000);
        assert!(rms(&stop[2048..46_000]) < 0.0001);
        assert!((rms(&pass[2048..46_000]) - 0.3535534).abs() < 0.001);
    }
    #[test]
    fn conversion_preserves_channel_polarity_and_alignment() {
        let mut mono = vec![0.0; 4410];
        mono[2205] = 0.5;
        let stereo: Vec<_> = mono.iter().flat_map(|s| [*s, -*s]).collect();
        let out = resample(&stereo, 2, 44_100, 48_000).unwrap();
        assert_eq!(out.len(), 9600);
        assert!(
            out.as_chunks::<2>()
                .0
                .iter()
                .all(|s| (s[0] + s[1]).abs() < 1e-6)
        );
        let peak = out
            .as_chunks::<2>()
            .0
            .iter()
            .enumerate()
            .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
            .unwrap()
            .0;
        // The converter reports integer latency; the filter's fractional
        // group delay can put the largest sample on either adjacent frame.
        assert!(peak.abs_diff(2400) <= 1);
    }
    #[test]
    fn tiny_inputs_and_unity_rate_keep_exact_lengths() {
        assert_eq!(
            resample(&[0.1, -0.2], 2, 48_000, 48_000).unwrap(),
            vec![0.1, -0.2]
        );
        for n in [1, 2, 17, 1023, 1024, 1025] {
            let out = resample(&vec![0.1; n], 1, 44_100, 48_000).unwrap();
            assert_eq!(out.len(), (n as f64 * 48_000.0 / 44_100.0).round() as usize);
            assert!(out.iter().all(|v| v.is_finite()));
        }
    }
}
