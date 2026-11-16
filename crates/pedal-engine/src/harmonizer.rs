//! Scale-aware harmonizer: a second voice that follows your playing a fixed
//! number of scale steps away, so a third above stays inside the key (minor
//! or major third as the scale demands) instead of a fixed pitch shift.
//!
//! The note is tracked on the clean DI (YIN on a 4x decimated copy, which is
//! cheap and accurate before distortion adds harmonics). The voice itself is
//! the post-cab tone pitch-shifted, so it sounds like a second guitar through
//! the same rig. Everything is allocated in `new`; nothing allocates in the
//! audio callback.

use crate::svf::{Svf, SvfCoefficients};
use signalsmith_stretch::Stretch;

/// (name, semitone offsets of the seven degrees).
pub const SCALES: [(&str, [i32; 7]); 6] = [
    ("MINOR", [0, 2, 3, 5, 7, 8, 10]),
    ("MAJOR", [0, 2, 4, 5, 7, 9, 11]),
    ("HARMONIC MINOR", [0, 2, 3, 5, 7, 8, 11]),
    ("PHRYGIAN", [0, 1, 3, 5, 7, 8, 10]),
    ("DORIAN", [0, 2, 3, 5, 7, 9, 10]),
    ("PHRYGIAN DOMINANT", [0, 1, 4, 5, 7, 8, 10]),
];

/// Lowest tracked fundamental (Hz): below a 6-string's low B.
const MIN_HZ: f32 = 55.0;
/// Highest tracked fundamental (Hz): above the 24th fret of the high E.
const MAX_HZ: f32 = 1_400.0;
/// YIN cumulative-mean threshold; lower is stricter.
const YIN_THRESHOLD: f32 = 0.15;
/// Quieter clean input than this keeps the last note (lets the tail ring).
const VOICED_RMS: f32 = 0.001;

/// Shift-voice latency in frames: the FFT window, ~21 ms at any rate.
pub fn latency_frames(sample_rate: u32) -> usize {
    ((sample_rate as usize * 1024 / 48_000).max(256)).next_power_of_two()
}

/// Semitones between `note` and the note `steps` scale degrees away in the
/// given key. Notes outside the scale are harmonized in parallel with the
/// scale note just below them.
pub fn harmony_shift(note: i32, key: i32, scale: &[i32; 7], steps: i32) -> i32 {
    let class = (note - key).rem_euclid(12);
    let degree = (0..7).rev().find(|&d| scale[d] <= class).unwrap_or(0);
    let target = degree as i32 + steps;
    scale[target.rem_euclid(7) as usize] + 12 * target.div_euclid(7) - scale[degree]
}

#[derive(Clone, Copy)]
pub struct HarmonizerSettings {
    pub enabled: bool,
    pub key: i32,
    pub scale: usize,
    pub steps: i32,
    pub mix: f32,
}

/// Monophonic YIN pitch tracker on a decimated copy of the clean input.
struct PitchTracker {
    filters: [Svf; 2],
    coefficients: SvfCoefficients,
    factor: usize,
    phase: usize,
    ring: Vec<f32>,
    write: usize,
    since_hop: usize,
    hop: usize,
    frame: Vec<f32>,
    cmnd: Vec<f32>,
    window: usize,
    tau_min: usize,
    tau_max: usize,
    rate: f32,
    note: Option<i32>,
}

impl PitchTracker {
    fn new(sample_rate: u32) -> Self {
        let factor = (sample_rate as f32 / 12_000.0).round().max(1.0) as usize;
        let rate = sample_rate as f32 / factor as f32;
        let tau_max = (rate / MIN_HZ).ceil() as usize;
        // Two periods of the lowest note keeps YIN reliable on low strings.
        let window = tau_max + 32;
        let length = window + tau_max + 2;
        Self {
            filters: [Svf::default(); 2],
            coefficients: SvfCoefficients::new(1_800.0, sample_rate as f32),
            factor,
            phase: 0,
            ring: vec![0.0; length],
            write: 0,
            since_hop: 0,
            hop: (rate * 0.004) as usize,
            frame: vec![0.0; length],
            cmnd: vec![0.0; tau_max + 2],
            window,
            tau_min: (rate / MAX_HZ).floor().max(2.0) as usize,
            tau_max,
            rate,
            note: None,
        }
    }

    fn reset(&mut self) {
        self.filters = [Svf::default(); 2];
        self.ring.fill(0.0);
        self.note = None;
    }

    fn push(&mut self, samples: &[f32]) {
        for &sample in samples {
            let (low, _) = self.filters[0].tick(sample, self.coefficients);
            let (low, _) = self.filters[1].tick(low, self.coefficients);
            self.phase += 1;
            if self.phase < self.factor {
                continue;
            }
            self.phase = 0;
            self.ring[self.write] = low;
            self.write = (self.write + 1) % self.ring.len();
            self.since_hop += 1;
            if self.since_hop >= self.hop {
                self.since_hop = 0;
                self.detect();
            }
        }
    }

    /// Fundamental frequency of the newest window, or `None` if unvoiced.
    fn frequency(&mut self) -> Option<f32> {
        let n = self.ring.len();
        for (i, slot) in self.frame.iter_mut().enumerate() {
            *slot = self.ring[(self.write + i) % n];
        }
        let w = self.window;
        let reference = &self.frame[n - w..];
        let power = reference.iter().map(|s| s * s).sum::<f32>() / w as f32;
        if power.sqrt() < VOICED_RMS {
            return None;
        }
        let mut running = 0.0;
        self.cmnd[0] = 1.0;
        for tau in 1..=self.tau_max {
            let lagged = &self.frame[n - w - tau..n - tau];
            let d: f32 = reference
                .iter()
                .zip(lagged)
                .map(|(a, b)| (a - b) * (a - b))
                .sum();
            running += d;
            self.cmnd[tau] = if running > 0.0 {
                d * tau as f32 / running
            } else {
                1.0
            };
        }
        let mut tau = (self.tau_min..self.tau_max).find(|&t| self.cmnd[t] < YIN_THRESHOLD)?;
        while tau + 1 < self.tau_max && self.cmnd[tau + 1] < self.cmnd[tau] {
            tau += 1;
        }
        let (a, b, c) = (self.cmnd[tau - 1], self.cmnd[tau], self.cmnd[tau + 1]);
        let curve = a - 2.0 * b + c;
        let offset = if curve > 0.0 {
            (0.5 * (a - c) / curve).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some(self.rate / (tau as f32 + offset))
    }

    fn detect(&mut self) {
        let Some(hz) = self.frequency() else { return };
        let midi = 69.0 + 12.0 * (hz / 440.0).log2();
        // Hysteresis: bends and vibrato don't flip the harmony note.
        if self.note.is_none_or(|n| (midi - n as f32).abs() > 0.7) {
            self.note = Some(midi.round() as i32);
        }
    }
}

pub struct Harmonizer {
    shifter: Stretch,
    latency: usize,
    warmup: usize,
    running: bool,
    gain: f32,
    step: f32,
    tracker: PitchTracker,
}

impl Harmonizer {
    pub fn new(sample_rate: u32) -> Self {
        let window = latency_frames(sample_rate);
        let shifter = Stretch::new(1, window, window / 8);
        let latency = shifter.input_latency() + shifter.output_latency();
        Self {
            shifter,
            latency,
            warmup: 0,
            running: false,
            gain: 0.0,
            step: 1.0 / (0.010 * sample_rate.max(1) as f32),
            tracker: PitchTracker::new(sample_rate),
        }
    }

    /// The note currently being harmonized (MIDI), if any.
    pub fn note(&self) -> Option<i32> {
        self.tracker.note
    }

    /// Feed the clean DI so the harmony knows which note you're playing.
    pub fn track(&mut self, clean: &[f32]) {
        if self.running {
            self.tracker.push(clean);
        }
    }

    /// Adds the harmony voice (built from the post-cab tone) to both channels.
    pub fn process(
        &mut self,
        left: &mut [f32],
        mut right: Option<&mut [f32]>,
        settings: HarmonizerSettings,
    ) {
        let want = settings.enabled && settings.mix > 0.0;
        if want && !self.running {
            // Flush history from before the bypass instead of resetting
            // (which would be allowed, but warmup keeps the voice silent
            // until fresh audio has passed through the window anyway).
            self.warmup = self.latency * 2;
            self.tracker.reset();
        }
        self.running = want || self.gain > 0.0;
        if !self.running {
            return;
        }
        let scale = &SCALES[settings.scale.min(SCALES.len() - 1)].1;
        let note = self.tracker.note;
        let semitones = note.map_or(0, |n| {
            harmony_shift(n, settings.key, scale, settings.steps)
        });
        let mut offset = 0;
        while offset < left.len() {
            let end = (offset + 128).min(left.len());
            let mut mid = [0.0; 128];
            let mut wet = [0.0; 128];
            let len = end - offset;
            for i in 0..len {
                mid[i] = match right.as_deref() {
                    Some(r) => 0.5 * (left[offset + i] + r[offset + i]),
                    None => left[offset + i],
                };
            }
            self.shifter
                .set_transpose_factor_semitones(semitones as f32, None);
            self.shifter.process(&mid[..len], &mut wet[..len]);
            for i in 0..len {
                self.warmup = self.warmup.saturating_sub(1);
                let target = if want && self.warmup == 0 && note.is_some() {
                    settings.mix.min(1.0)
                } else {
                    0.0
                };
                self.gain += (target - self.gain).clamp(-self.step, self.step);
                let voice = wet[i] * self.gain;
                left[offset + i] += voice;
                if let Some(r) = right.as_deref_mut() {
                    r[offset + i] += voice;
                }
            }
            offset = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const MINOR: [i32; 7] = SCALES[0].1;

    #[test]
    fn thirds_follow_the_key() {
        // E minor, third up: E->G (3), F#->A (3), G->B (4), B->D (3), C->E (4).
        let e = 4;
        assert_eq!(harmony_shift(40, e, &MINOR, 2), 3);
        assert_eq!(harmony_shift(42, e, &MINOR, 2), 3);
        assert_eq!(harmony_shift(43, e, &MINOR, 2), 4);
        assert_eq!(harmony_shift(47, e, &MINOR, 2), 3);
        assert_eq!(harmony_shift(48, e, &MINOR, 2), 4);
        // Fifth up from B in E minor is F# (7); from C it's G (7); from F# it's C (6).
        assert_eq!(harmony_shift(47, e, &MINOR, 4), 7);
        assert_eq!(harmony_shift(42, e, &MINOR, 4), 6);
        // Octaves and downward intervals.
        assert_eq!(harmony_shift(52, e, &MINOR, 7), 12);
        assert_eq!(harmony_shift(52, e, &MINOR, -2), -4); // E -> C below
        assert_eq!(harmony_shift(52, e, &MINOR, -7), -12);
    }

    #[test]
    fn displayed_latency_matches_the_shifter() {
        for rate in [44_100, 48_000, 96_000] {
            assert_eq!(Harmonizer::new(rate).latency, latency_frames(rate));
        }
    }

    fn sine(hz: f32, seconds: f32) -> Vec<f32> {
        (0..(48_000.0 * seconds) as usize)
            .map(|i| 0.2 * (TAU * hz * i as f32 / 48_000.0).sin())
            .collect()
    }

    fn magnitude(signal: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in signal.iter().enumerate() {
            let p = TAU * hz * i as f32 / 48_000.0;
            re += s * p.cos();
            im += s * p.sin();
        }
        (re * re + im * im).sqrt() / signal.len() as f32
    }

    #[test]
    fn tracks_low_and_high_guitar_notes() {
        for (hz, midi) in [(61.74, 35), (82.41, 40), (329.63, 64), (1_046.5, 84)] {
            let mut tracker = PitchTracker::new(48_000);
            // A little second harmonic, like a real string.
            let audio: Vec<f32> = sine(hz, 0.3)
                .iter()
                .enumerate()
                .map(|(i, s)| s + 0.1 * (TAU * 2.0 * hz * i as f32 / 48_000.0).sin())
                .collect();
            tracker.push(&audio);
            assert_eq!(tracker.note, Some(midi), "{hz} Hz");
        }
    }

    #[test]
    fn adds_a_diatonic_third_above_the_played_note() {
        let settings = HarmonizerSettings {
            enabled: true,
            key: 4,
            scale: 0,
            steps: 2,
            mix: 1.0,
        };
        // G (196 Hz) in E minor: third up is B (246.94 Hz, +4 semitones).
        let mut harmonizer = Harmonizer::new(48_000);
        let input = sine(196.0, 1.0);
        let mut output = input.clone();
        for (clean, block) in input.chunks(64).zip(output.chunks_mut(64)) {
            harmonizer.process(block, None, settings);
            harmonizer.track(clean);
        }
        assert_eq!(harmonizer.note(), Some(55));
        let tail = &output[24_000..];
        assert!(magnitude(tail, 246.94) > 0.03, "missing harmony");
        assert!(magnitude(tail, 233.08) < 0.01, "minor third leaked");
        assert!(magnitude(tail, 196.0) > 0.08, "dry must stay");
    }

    #[test]
    fn disabled_is_exact_passthrough() {
        let mut harmonizer = Harmonizer::new(48_000);
        let settings = HarmonizerSettings {
            enabled: false,
            key: 0,
            scale: 0,
            steps: 2,
            mix: 1.0,
        };
        let input = sine(220.0, 0.1);
        let mut output = input.clone();
        harmonizer.track(&input);
        harmonizer.process(&mut output, None, settings);
        assert_eq!(output, input);
    }
}
