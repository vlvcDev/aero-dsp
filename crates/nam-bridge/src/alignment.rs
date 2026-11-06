//! Conservative load-time capture alignment. Original implementation using a
//! linear sweep/correlation; behavioral reference: TONE3000 model-latency.md.
//! This estimates an integer correlation peak, not broadband phase correction.
use serde::{Deserialize, Serialize};

pub const MAX_DELAY_SECONDS: f64 = 0.010;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlignmentStatus {
    Accepted,
    Uncertain,
    Silent,
    Inverted,
    AtLimit,
    LevelDependent,
    ProbeFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AlignmentEstimate {
    pub status: AlignmentStatus,
    pub delay_samples: u32,
    /// Normalized correlation, NOT a calibrated probability of correctness.
    pub correlation: f32,
    pub peak_ratio: f32,
    pub lobe_ratio: f32,
}
impl AlignmentEstimate {
    pub fn unavailable(status: AlignmentStatus) -> Self {
        Self {
            status,
            delay_samples: 0,
            correlation: 0.0,
            peak_ratio: 0.0,
            lobe_ratio: 0.0,
        }
    }
    pub fn applied_delay(self) -> usize {
        if self.status == AlignmentStatus::Accepted {
            self.delay_samples as usize
        } else {
            0
        }
    }
}

pub fn max_delay(rate: f64) -> usize {
    (rate * MAX_DELAY_SECONDS).ceil() as usize
}

fn sweep(rate: f64, amplitude: f32) -> Vec<f32> {
    let n = (rate * 0.1).round() as usize;
    let fade = (rate * 0.005).round() as usize;
    let duration = n as f64 / rate;
    let high = 20000.0_f64.min(rate * 0.45);
    (0..n)
        .map(|i| {
            let t = i as f64 / rate;
            let phase =
                std::f64::consts::TAU * (30.0 * t + (high - 30.0) * t * t / (2.0 * duration));
            let distance = i.min(n - 1 - i);
            let envelope = if distance < fade {
                0.5 - 0.5 * (std::f64::consts::PI * distance as f64 / fade as f64).cos()
            } else {
                1.0
            };
            (phase.sin() * envelope * f64::from(amplitude)) as f32
        })
        .collect()
}

fn analyze(input: &[f32], output: &[f32], rate: f64) -> AlignmentEstimate {
    let limit = max_delay(rate);
    if input.is_empty()
        || output.len() < input.len() + limit
        || output.iter().any(|x| !x.is_finite())
    {
        return AlignmentEstimate::unavailable(AlignmentStatus::ProbeFailed);
    }
    let mean = output.iter().map(|x| f64::from(*x)).sum::<f64>() / output.len() as f64;
    let y: Vec<f64> = output.iter().map(|x| f64::from(*x) - mean).collect();
    let x: Vec<f64> = input.iter().map(|x| f64::from(*x)).collect();
    let energy_x = x.iter().map(|x| x * x).sum::<f64>();
    let energy_y = y.iter().map(|y| y * y).sum::<f64>();
    if energy_x < 1e-12 || energy_y / (y.len() as f64) < 1e-14 {
        return AlignmentEstimate::unavailable(AlignmentStatus::Silent);
    }
    let scores: Vec<f64> = (0..=limit)
        .map(|lag| x.iter().zip(&y[lag..]).map(|(x, y)| x * y).sum())
        .collect();
    let best = (0..=limit)
        .max_by(|&a, &b| scores[a].abs().total_cmp(&scores[b].abs()))
        .unwrap_or(0);
    let peak = scores[best].abs();
    let guard = (rate * 0.001).round() as usize;
    let mut distant = 0.0_f64;
    let mut nearby = 0.0_f64;
    for (lag, score) in scores.iter().enumerate() {
        let distance = lag.abs_diff(best);
        if distance > guard {
            distant = distant.max(score.abs());
        } else if distance >= 2 {
            nearby = nearby.max(score.abs());
        }
    }
    let local_energy = y[best..best + x.len()].iter().map(|y| y * y).sum::<f64>();
    let correlation = (peak / (energy_x * local_energy).sqrt().max(1e-20)).clamp(0.0, 1.0) as f32;
    let peak_ratio = (peak / distant.max(1e-20)).min(1000000.0) as f32;
    let lobe_ratio = (peak / nearby.max(1e-20)).min(1000000.0) as f32;
    let status = if best >= limit.saturating_sub(1) {
        AlignmentStatus::AtLimit
    } else if correlation < 0.35 || peak_ratio < 2.0 || lobe_ratio < 1.2 {
        AlignmentStatus::Uncertain
    } else if scores[best] < 0.0 {
        AlignmentStatus::Inverted
    } else {
        AlignmentStatus::Accepted
    };
    AlignmentEstimate {
        status,
        delay_samples: best as u32,
        correlation,
        peak_ratio,
        lobe_ratio,
    }
}

/// Runs only on an isolated analysis instance. Any processing failure rejects
/// compensation; the real model is loaded separately and never hears this probe.
pub fn measure<E>(
    rate: f64,
    block: usize,
    mut process: impl FnMut(&mut [f32]) -> Result<(), E>,
) -> AlignmentEstimate {
    if !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) || block == 0 {
        return AlignmentEstimate::unavailable(AlignmentStatus::ProbeFailed);
    }
    let mut estimates = Vec::with_capacity(2);
    for amplitude in [0.03, 0.1] {
        let mut silence = vec![0.0; (rate * 0.1).round() as usize];
        for chunk in silence.chunks_mut(block) {
            if process(chunk).is_err() {
                return AlignmentEstimate::unavailable(AlignmentStatus::ProbeFailed);
            }
        }
        let input = sweep(rate, amplitude);
        let mut output = input.clone();
        output.resize(input.len() + max_delay(rate), 0.0);
        for chunk in output.chunks_mut(block) {
            if process(chunk).is_err() {
                return AlignmentEstimate::unavailable(AlignmentStatus::ProbeFailed);
            }
        }
        estimates.push(analyze(&input, &output, rate));
    }
    let a = estimates[0];
    let mut b = estimates[1];
    if a.status != AlignmentStatus::Accepted {
        return a;
    }
    if b.status != AlignmentStatus::Accepted {
        return b;
    }
    // Deliberately require identical integer estimates across the two levels.
    if a.delay_samples != b.delay_samples {
        b.status = AlignmentStatus::LevelDependent;
    }
    b.correlation = b.correlation.min(a.correlation);
    b.peak_ratio = b.peak_ratio.min(a.peak_ratio);
    b.lobe_ratio = b.lobe_ratio.min(a.lobe_ratio);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    fn delayed(delay: usize, inverted: bool, nonlinear: bool, block: usize) -> AlignmentEstimate {
        let mut history = vec![0.0; delay + 1];
        let mut cursor = 0;
        measure(48000.0, block, |samples| {
            for x in samples {
                history[cursor] = *x;
                *x = history[(cursor + 1) % history.len()];
                if nonlinear {
                    *x = (*x * 15.0).tanh();
                }
                if inverted {
                    *x = -*x;
                }
                *x = *x * 0.3 + 0.02; // gain and DC must not bias the peak
                cursor = (cursor + 1) % history.len();
            }
            Ok::<_, ()>(())
        })
    }
    #[test]
    fn finds_zero_and_injected_delays_with_gain_dc_and_distortion() {
        for delay in [0, 1, 17, 37, 127] {
            let result = delayed(delay, false, true, 64);
            assert_eq!(result.status, AlignmentStatus::Accepted, "{result:?}");
            assert_eq!(result.applied_delay(), delay);
            assert_eq!(result, delayed(delay, false, true, 37));
        }
    }
    #[test]
    fn rejects_silence_nonfinite_polarity_boundary_and_errors() {
        for value in [0.0, 0.1, f32::NAN] {
            let result = measure(48000.0, 64, |s| {
                s.fill(value);
                Ok::<_, ()>(())
            });
            assert_ne!(result.status, AlignmentStatus::Accepted);
            assert_eq!(result.applied_delay(), 0);
        }
        assert_eq!(
            delayed(17, true, false, 64).status,
            AlignmentStatus::Inverted
        );
        assert_eq!(
            delayed(480, false, false, 64).status,
            AlignmentStatus::AtLimit
        );
        assert_eq!(
            measure(48000.0, 64, |_| Err(())).status,
            AlignmentStatus::ProbeFailed
        );
        assert_eq!(
            measure(48000.0, 0, |_| Ok::<_, ()>(())).status,
            AlignmentStatus::ProbeFailed
        );
    }
    #[test]
    fn rejects_competing_lobes_and_level_dependent_delay() {
        let x = sweep(48000.0, 0.1);
        let mut y = vec![0.0; x.len() + 480];
        for (i, v) in x.iter().enumerate() {
            y[i + 17] += *v;
            y[i + 25] -= *v;
        }
        assert_eq!(analyze(&x, &y, 48000.0).status, AlignmentStatus::Uncertain);
        let mut ring = [0.0; 128];
        let mut cursor = 0;
        let mut frames = 0;
        let result = measure(48000.0, 64, |s| {
            for x in s {
                let delay = if frames < 10080 { 17 } else { 37 };
                ring[cursor] = *x;
                *x = ring[(cursor + 128 - delay) % 128];
                cursor = (cursor + 1) % 128;
                frames += 1;
            }
            Ok::<_, ()>(())
        });
        assert_eq!(result.status, AlignmentStatus::LevelDependent, "{result:?}");
    }
}
