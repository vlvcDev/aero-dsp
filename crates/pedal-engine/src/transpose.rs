//! Full-wet stereo STFT pitch shifting. All storage and FFT plans are built off-thread.
//! Original implementation of phase-difference instantaneous-frequency estimation:
//! https://www.audiolabs-erlangen.com/content/05_fau/professor/00_mueller/06_projects/90_siamus/2016_DriedgerMueller_TSMOverview_AppliedSciences_ePrint.pdf
//! Independent input-bin phases avoid detuning when shifted bins collide. This basic
//! phase vocoder can smear attacks and change partial amplitudes; it is not formant preserving.
use crate::node_rig::{Frame, StereoNode};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::f32::consts::{PI, TAU};
use std::sync::Arc;

struct Channel {
    input: Vec<f32>,
    overlap: Vec<f32>,
    output: Vec<f32>,
    previous: Vec<f32>,
    phase: Vec<f32>,
    phase_started: Vec<bool>,
}
impl Channel {
    fn new(n: usize, hop: usize) -> Self {
        Self {
            input: vec![0.; n],
            overlap: vec![0.; n],
            output: vec![0.; hop],
            previous: vec![0.; n / 2 + 1],
            phase: vec![0.; n / 2 + 1],
            phase_started: vec![false; n / 2 + 1],
        }
    }
    fn reset(&mut self) {
        self.input.fill(0.);
        self.overlap.fill(0.);
        self.output.fill(0.);
        self.previous.fill(0.);
        self.phase.fill(0.);
        self.phase_started.fill(false);
    }
}
struct Vocoder {
    n: usize,
    hop: usize,
    cursor: usize,
    ratio: f32,
    window: Vec<f32>,
    channels: [Channel; 2],
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    spectrum: Vec<Complex32>,
    synthesis: Vec<Complex32>,
    scratch: Vec<Complex32>,
}
impl Vocoder {
    fn new(rate: u32, base: usize, shift: i32) -> Self {
        // Power-of-two sizes keep processing cost predictable at every supported rate.
        let n = (rate as usize * base / 48000).max(512).next_power_of_two();
        let hop = n / 8;
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        Self {
            n,
            hop,
            cursor: 0,
            ratio: 2f32.powf(shift as f32 / 12.),
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (TAU * i as f32 / n as f32).cos())
                .collect(),
            channels: [Channel::new(n, hop), Channel::new(n, hop)],
            forward,
            inverse,
            spectrum: vec![Complex32::default(); n],
            synthesis: vec![Complex32::default(); n],
            scratch: vec![Complex32::default(); scratch_len],
        }
    }
    fn transform(&mut self) {
        let n = self.n;
        let hop = self.hop;
        for channel in &mut self.channels {
            for i in 0..n {
                self.spectrum[i] = Complex32::new(channel.input[i] * self.window[i], 0.);
            }
            self.forward
                .process_with_scratch(&mut self.spectrum, &mut self.scratch);
            self.synthesis.fill(Complex32::default());
            for k in 0..=n / 2 {
                let bin = self.spectrum[k];
                let phase = bin.arg();
                let expected = TAU * k as f32 * hop as f32 / n as f32;
                let delta = (phase - channel.previous[k] - expected + PI).rem_euclid(TAU) - PI;
                channel.previous[k] = phase;
                // Keep each input partial's frequency even when several bins map to one output bin.
                // Averaging frequencies on collision detunes simultaneous notes.
                if !channel.phase_started[k] && bin.norm_sqr() > 1e-12 {
                    // Preserve the measured inter-channel phase at onset, including
                    // after leading silence. Multiplying it by ratio rotates stereo.
                    channel.phase[k] = phase;
                    channel.phase_started[k] = true;
                } else {
                    channel.phase[k] =
                        (channel.phase[k] + (expected + delta) * self.ratio).rem_euclid(TAU);
                }
                let destination = (k as f32 * self.ratio).round() as usize;
                let frequency = (expected + delta) * self.ratio / hop as f32;
                if destination > 0 && destination < n / 2 && frequency > 0. && frequency < PI {
                    self.synthesis[destination] +=
                        Complex32::from_polar(bin.norm(), channel.phase[k]);
                }
            }
            // Unit-ratio reconstruction also retains the real DC and Nyquist bins.
            if self.ratio == 1.0 {
                self.synthesis.copy_from_slice(&self.spectrum);
            }
            for k in 1..n / 2 {
                self.synthesis[n - k] = self.synthesis[k].conj();
            }
            self.inverse
                .process_with_scratch(&mut self.synthesis, &mut self.scratch);
            // Eight overlapping periodic Hann windows have a squared-window sum of three.
            for i in 0..n {
                channel.overlap[i] += self.synthesis[i].re * self.window[i] / (3. * n as f32);
            }
            channel.output.copy_from_slice(&channel.overlap[..hop]);
            channel.overlap.copy_within(hop.., 0);
            channel.overlap[n - hop..].fill(0.);
            channel.input.copy_within(hop.., 0);
        }
    }
    fn tick(&mut self, input: Frame) -> Frame {
        let mut out = [0.; 2];
        for c in 0..2 {
            out[c] = self.channels[c].output[self.cursor];
            self.channels[c].input[self.n - self.hop + self.cursor] = input[c];
        }
        self.cursor += 1;
        if self.cursor == self.hop {
            self.transform();
            self.cursor = 0;
        }
        out
    }
    fn reset(&mut self) {
        self.cursor = 0;
        for c in &mut self.channels {
            c.reset();
        }
    }
}

pub struct TransposeNode {
    vocoder: Option<Vocoder>,
}
impl TransposeNode {
    pub fn new(rate: u32, semitones: i32, quality: &str) -> Result<Self, String> {
        if !(-12..=12).contains(&semitones) {
            return Err("Semitones must be between -12 and 12".into());
        }
        let base = match quality {
            "fast" => 2048,
            "smooth" => 4096,
            _ => return Err("Quality must be fast or smooth".into()),
        };
        if !(8000..=192000).contains(&rate) {
            return Err("Sample rate must be 8000–192000 Hz".into());
        }
        Ok(Self {
            vocoder: if semitones == 0 {
                None
            } else {
                Some(Vocoder::new(rate, base, semitones))
            },
        })
    }
}
impl StereoNode for TransposeNode {
    fn latency_samples(&self) -> usize {
        self.vocoder.as_ref().map_or(0, |v| v.n)
    }
    fn tick(&mut self, input: Frame) -> Frame {
        self.vocoder.as_mut().map_or(input, |v| v.tick(input))
    }
    fn process_block(&mut self, frames: &mut [Frame]) {
        if let Some(v) = &mut self.vocoder {
            for frame in frames {
                *frame = v.tick(*frame);
            }
        }
    }
    fn reset(&mut self) {
        if let Some(v) = &mut self.vocoder {
            v.reset();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const RATE: f32 = 48000.0;
    fn magnitude(frames: &[Frame], hz: f32, channel: usize) -> f32 {
        let angle = std::f64::consts::TAU * hz as f64 / RATE as f64;
        let step = rustfft::num_complex::Complex64::from_polar(1.0, angle);
        let mut oscillator = rustfft::num_complex::Complex64::new(1.0, 0.0);
        let mut sum = rustfft::num_complex::Complex64::new(0.0, 0.0);
        for f in frames {
            sum += oscillator * f[channel] as f64;
            oscillator *= step;
        }
        (sum.norm() / frames.len() as f64) as f32
    }

    #[test]
    fn validates_and_zero_is_exact() {
        assert!(TransposeNode::new(7999, 0, "fast").is_err());
        assert!(TransposeNode::new(192001, 0, "fast").is_err());
        assert!(TransposeNode::new(48000, 13, "fast").is_err());
        assert!(TransposeNode::new(48000, 0, "bad").is_err());
        let mut n = TransposeNode::new(48000, 0, "fast").unwrap();
        let mut x = [[0.2, -0.3], [1.0, -1.0]];
        let original = x;
        n.process_block(&mut x);
        assert_eq!(x, original);
        assert_eq!(n.latency_samples(), 0);
    }
    #[test]
    fn single_pitch_partition_reset_and_channels() {
        for shift in [-12, -2] {
            let source: Vec<Frame> = (0..96000)
                .map(|i| {
                    let x = 0.1 * (std::f32::consts::TAU * 220.0 * i as f32 / RATE).sin();
                    [x, x]
                })
                .collect();
            let mut a = source.clone();
            let mut b = source.clone();
            let mut one = TransposeNode::new(48000, shift, "fast").unwrap();
            let mut two = TransposeNode::new(48000, shift, "fast").unwrap();
            one.process_block(&mut a);
            for c in b.chunks_mut(37) {
                two.process_block(c);
            }
            assert!(
                a.iter()
                    .zip(&b)
                    .all(|(x, y)| (x[0] - y[0]).abs() < 1e-5 && (x[1] - y[1]).abs() < 1e-5)
            );
            // Identical channels must remain identical.
            assert!(a.iter().all(|f| (f[0] - f[1]).abs() < 0.001));
            let target = 220.0 * 2f32.powf(shift as f32 / 12.0);
            assert!(
                magnitude(&a[48000..], target, 0) > 0.03,
                "shift {shift}: {}",
                magnitude(&a[48000..], target, 0)
            );
            one.reset();
            let mut reset = source.clone();
            one.process_block(&mut reset);
            assert!(
                a.iter()
                    .zip(&reset)
                    .all(|(x, y)| (x[0] - y[0]).abs() < 1e-5)
            );
            let mut isolated = source;
            for f in &mut isolated {
                f[1] = 0.0;
            }
            two.reset();
            two.process_block(&mut isolated);
            assert!(isolated.iter().all(|f| f[1].abs() < 1e-6));
        }
    }
    #[test]
    fn low_guitar_chord_pitches_within_five_cents() {
        for quality in ["fast", "smooth"] {
            for shift in [-12, -2, 12] {
                let notes = [82.4069, 110.0, 146.8324, 195.9977];
                let mut audio: Vec<Frame> = (0..144000)
                    .map(|i| {
                        let x = notes
                            .iter()
                            .map(|hz| 0.1 * (std::f32::consts::TAU * hz * i as f32 / RATE).sin())
                            .sum();
                        [x, x]
                    })
                    .collect();
                TransposeNode::new(48000, shift, quality)
                    .unwrap()
                    .process_block(&mut audio);
                for note in notes {
                    let target = note * 2f32.powf(shift as f32 / 12.0);
                    // Search the surrounding semitone: strong energy at the target alone
                    // cannot establish that the strongest partial is correctly tuned.
                    let (cents, peak) = (-50i32..=50)
                        .map(|c| {
                            (
                                c,
                                magnitude(
                                    &audio[96000..],
                                    target * 2f32.powf(c as f32 / 1200.0),
                                    0,
                                ),
                            )
                        })
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .unwrap();
                    assert!(
                        cents.abs() <= 5,
                        "{quality}, {shift} semitones, {target} Hz: {cents} cents"
                    );
                    assert!(
                        peak > 0.01,
                        "{quality}, {shift} semitones, {target} Hz: partial lost ({peak})"
                    );
                }
            }
        }
    }
    #[test]
    fn octave_up_rejects_above_nyquist_and_preserves_audible_tone() {
        for quality in ["fast", "smooth"] {
            let mut high: Vec<Frame> = (0..96000)
                .map(|i| {
                    let x = 0.1 * (std::f32::consts::TAU * 16000.0 * i as f32 / RATE).sin();
                    [x, x]
                })
                .collect();
            let mut low: Vec<Frame> = (0..96000)
                .map(|i| {
                    let x = 0.1 * (std::f32::consts::TAU * 4000.0 * i as f32 / RATE).sin();
                    [x, x]
                })
                .collect();
            TransposeNode::new(48000, 12, quality)
                .unwrap()
                .process_block(&mut high);
            TransposeNode::new(48000, 12, quality)
                .unwrap()
                .process_block(&mut low);
            let rms = (high[48000..].iter().map(|f| f[0] * f[0]).sum::<f32>() / 48000.).sqrt();
            assert!(rms < 0.001, "{quality}: aliased high tone RMS {rms}");
            assert!(magnitude(&low[48000..], 8000., 0) > 0.02);
        }
    }
    #[test]
    fn unit_ratio_overlap_normalization_and_reported_delay() {
        let mut v = Vocoder::new(48000, 2048, 0);
        let input: Vec<Frame> = (0..12000)
            .map(|i| {
                let a = 0.2 * (i as f32 * 0.129).sin() + 0.07;
                [a, -a]
            })
            .collect();
        let output: Vec<Frame> = input.iter().map(|&f| v.tick(f)).collect();
        for i in v.n..input.len() {
            assert!(
                (output[i][0] - input[i - v.n][0]).abs() < 0.0002,
                "frame {i}"
            );
        }
    }
    #[test]
    fn individual_samples_match_irregular_blocks() {
        let mut sample = TransposeNode::new(48000, 7, "fast").unwrap();
        let mut blocks = TransposeNode::new(48000, 7, "fast").unwrap();
        let source: Vec<Frame> = (0..16000)
            .map(|i| {
                [
                    (i as f32 * 0.129).sin() * 0.2,
                    (i as f32 * 0.087).sin() * 0.1,
                ]
            })
            .collect();
        let expected: Vec<Frame> = source.iter().map(|&f| sample.tick(f)).collect();
        let mut actual = source;
        let mut position = 0;
        let mut state = 123u32;
        while position < actual.len() {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let end = (position + 1 + (state as usize % 511)).min(actual.len());
            blocks.process_block(&mut actual[position..end]);
            position = end;
        }
        assert_eq!(expected, actual);
        assert!(
            actual
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() < 1.)
        );
    }
    #[test]
    fn stereo_phase_and_gain_survive_silent_onset() {
        for gain in [-1.0, 0.5] {
            let mut node = TransposeNode::new(48000, -2, "fast").unwrap();
            let mut audio: Vec<Frame> = (0..48000)
                .map(|i| {
                    let x = if i < 5000 {
                        0.
                    } else {
                        0.1 * (TAU * 220. * (i - 5000) as f32 / RATE).sin()
                    };
                    [x, x * gain]
                })
                .collect();
            node.process_block(&mut audio);
            let error = audio[12000..]
                .iter()
                .map(|f| (f[1] - gain * f[0]).powi(2))
                .sum::<f32>()
                / 36000.;
            assert!(
                error.sqrt() < 0.001,
                "gain {gain}: stereo error {}",
                error.sqrt()
            );
        }
    }
}
