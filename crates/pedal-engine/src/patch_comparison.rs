//! Offline comparison rendering; never called from the audio callback.
use crate::{
    audio_file::{AudioFile, resample, write_float_wav},
    node_document::RigDocument,
    node_rig::Frame,
};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// Match stereo RMS over frames whose source peak exceeds 0.001 (-60 dBFS).
/// Attenuate to the lower RMS, then cap each full render at 0.89 peak.
/// No active input or silent output disables RMS matching.
pub fn match_levels(source: &AudioFile, a: &mut [Frame], b: &mut [Frame]) -> (f32, f32, bool) {
    let mut sums = [0.0f64; 2];
    let mut count = 0usize;
    for (i, frame) in source.samples.chunks_exact(source.channels).enumerate() {
        if i >= a.len() || i >= b.len() {
            break;
        }
        if frame.iter().any(|x| x.abs() > 0.001) {
            for (sum, out) in sums.iter_mut().zip([a[i], b[i]]) {
                *sum += out.iter().map(|x| (*x as f64).powi(2)).sum::<f64>();
            }
            count += 2;
        }
    }
    let rms = sums.map(|sum| {
        if count == 0 {
            0.0
        } else {
            (sum / count as f64).sqrt()
        }
    });
    let matched = rms.iter().all(|x| *x > 1e-9 && x.is_finite());
    let target = rms[0].min(rms[1]);
    let mut gains = if matched {
        [(target / rms[0]) as f32, (target / rms[1]) as f32]
    } else {
        [1.0, 1.0]
    };
    let peaks =
        [&*a, &*b].map(|output| output.iter().flatten().fold(0.0f32, |p, x| p.max(x.abs())));
    let adjusted_peak = (peaks[0] * gains[0]).max(peaks[1] * gains[1]);
    let headroom = if adjusted_peak > 0.89 {
        0.89 / adjusted_peak
    } else {
        1.0
    };
    for (output, gain) in [a, b].into_iter().zip(gains.iter_mut()) {
        *gain *= headroom;
        for sample in output.iter_mut().flatten() {
            *sample *= *gain;
        }
    }
    (gains[0], gains[1], matched)
}

pub fn render(
    a: &RigDocument,
    a_dir: &Path,
    b: &RigDocument,
    b_dir: &Path,
    source_path: &Path,
    preview_path: &Path,
) -> Result<serde_json::Value, String> {
    if !(8_000..=192_000).contains(&a.sample_rate) || !(8_000..=192_000).contains(&b.sample_rate) {
        return Err("Unsupported patch sample rate".into());
    }
    if a.sample_rate != b.sample_rate {
        return Err("Both patches must use the same sample rate".into());
    }
    if std::fs::metadata(source_path)
        .map_err(|e| format!("Cannot open preview WAV: {e}"))?
        .len()
        > 256 * 1024 * 1024
    {
        return Err("Preview WAV exceeds 256 MiB".into());
    }
    let mut source = AudioFile::load(source_path)?;
    if !(1..=2).contains(&source.channels) {
        return Err("Preview needs a mono or stereo WAV".into());
    }
    source
        .samples
        .truncate(source.sample_rate as usize * source.channels * 30);
    source.samples = resample(
        &source.samples,
        source.channels,
        source.sample_rate,
        a.sample_rate,
    )?;
    source.sample_rate = a.sample_rate;
    let mut left = a.render(a_dir, &source, 3.0)?;
    let mut right = b.render(b_dir, &source, 3.0)?;
    let (ga, gb, matched) = match_levels(&source, &mut left, &mut right);
    let dir = preview_path.with_extension("comparisons");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Files older than 24 hours are disposable previews; current pairs remain stable.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("patch-ab-")
                && name.ends_with(".wav")
                && entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age.as_secs() > 86_400)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let stem = format!(
        "patch-ab-{}-{stamp}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let ap = dir.join(format!("{stem}-a.wav"));
    let bp = dir.join(format!("{stem}-b.wav"));
    write_float_wav(&ap, 2, a.sample_rate, left.iter().flatten().copied())?;
    if let Err(e) = write_float_wav(&bp, 2, a.sample_rate, right.iter().flatten().copied()) {
        let _ = std::fs::remove_file(&ap);
        return Err(e);
    }
    Ok(
        serde_json::json!({"a_path":ap,"b_path":bp,"duration_seconds":left.len() as f32 / a.sample_rate as f32,"a_attenuation_db":20.0*ga.log10(),"b_attenuation_db":20.0*gb.log10(),"matched":matched,"matching":"Fixed attenuation matches stereo RMS over frames where source peak exceeds -60 dBFS; a common attenuation caps both full renders at 0.89 peak. Silence disables RMS matching.","cleanup":"Preview pairs older than 24 hours are removed on a later comparison render."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_source_rms_matches_by_attenuation_and_peak_is_capped() {
        let source = AudioFile {
            samples: vec![0.0, 0.1, 0.1],
            channels: 1,
            sample_rate: 48000,
        };
        let mut a = [[0.0; 2], [0.8; 2], [0.8; 2], [4.0; 2]];
        let mut b = [[0.0; 2], [0.4; 2], [0.4; 2], [0.0; 2]];
        let (ga, gb, matched) = match_levels(&source, &mut a, &mut b);
        assert!(matched);
        assert!((ga - 0.2225).abs() < 1e-5);
        assert!((gb - 0.445).abs() < 1e-5);
        assert!((a[3][0] - 0.89).abs() < 1e-6);
        let silent = AudioFile {
            samples: vec![0.0; 3],
            channels: 1,
            sample_rate: 48000,
        };
        assert!(!match_levels(&silent, &mut a, &mut b).2);
    }
}
