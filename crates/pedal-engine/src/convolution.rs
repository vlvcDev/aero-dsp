//! Zero-added-latency convolution: direct early taps and a partitioned FFT tail.
//! All plans, spectra, history, and scratch are prepared before playback.
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::sync::Arc;

const PARTITION: usize = 128;
const FFT_SIZE: usize = PARTITION * 2;

pub struct Convolver {
    head: Vec<f32>,
    history: [f32; PARTITION],
    position: usize,
    filters: Vec<Vec<Complex32>>,
    spectra: Vec<Vec<Complex32>>,
    spectrum_position: usize,
    input: Vec<Complex32>,
    sum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    overlap: [f32; PARTITION],
    tail: [f32; PARTITION],
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
}

impl Convolver {
    /// Clear history without rebuilding FFT plans or allocating on the callback.
    pub fn clear(&mut self) {
        self.history.fill(0.0);
        self.position = 0;
        self.spectrum_position = 0;
        for spectrum in &mut self.spectra {
            spectrum.fill(Complex32::default());
        }
        self.input.fill(Complex32::default());
        self.sum.fill(Complex32::default());
        self.scratch.fill(Complex32::default());
        self.overlap.fill(0.0);
        self.tail.fill(0.0);
    }
    pub fn new(impulse: &[f32]) -> Self {
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(FFT_SIZE);
        let inverse = planner.plan_fft_inverse(FFT_SIZE);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let mut scratch = vec![Complex32::default(); scratch_len];
        let filters: Vec<_> = impulse
            .get(PARTITION..)
            .unwrap_or_default()
            .chunks(PARTITION)
            .map(|part| {
                let mut spectrum = vec![Complex32::default(); FFT_SIZE];
                for (frequency, sample) in spectrum.iter_mut().zip(part) {
                    frequency.re = *sample;
                }
                forward.process_with_scratch(&mut spectrum, &mut scratch);
                spectrum
            })
            .collect();
        Self {
            head: impulse[..impulse.len().min(PARTITION)].to_vec(),
            history: [0.0; PARTITION],
            position: 0,
            spectra: vec![vec![Complex32::default(); FFT_SIZE]; filters.len()],
            filters,
            spectrum_position: 0,
            input: vec![Complex32::default(); FFT_SIZE],
            sum: vec![Complex32::default(); FFT_SIZE],
            scratch,
            overlap: [0.0; PARTITION],
            tail: [0.0; PARTITION],
            forward,
            inverse,
        }
    }

    pub fn tick(&mut self, sample: f32) -> f32 {
        self.history[self.position] = sample;
        let mut output = self.tail[self.position];
        for (tap, coefficient) in self.head.iter().enumerate() {
            output += coefficient * self.history[(self.position + PARTITION - tap) % PARTITION];
        }
        self.input[self.position] = Complex32::new(sample, 0.0);
        self.position += 1;
        if self.position == PARTITION {
            self.position = 0;
            if !self.filters.is_empty() {
                self.finish_partition();
            }
        }
        output
    }

    fn finish_partition(&mut self) {
        self.input[PARTITION..].fill(Complex32::default());
        self.forward
            .process_with_scratch(&mut self.input, &mut self.scratch);
        self.spectra[self.spectrum_position].copy_from_slice(&self.input);
        self.sum.fill(Complex32::default());
        for (part, filter) in self.filters.iter().enumerate() {
            let spectrum = &self.spectra
                [(self.spectrum_position + self.spectra.len() - part) % self.spectra.len()];
            for ((sum, sample), coefficient) in self.sum.iter_mut().zip(spectrum).zip(filter) {
                *sum += sample * coefficient;
            }
        }
        self.inverse
            .process_with_scratch(&mut self.sum, &mut self.scratch);
        for i in 0..PARTITION {
            self.tail[i] = self.sum[i].re / FFT_SIZE as f32 + self.overlap[i];
            self.overlap[i] = self.sum[i + PARTITION].re / FFT_SIZE as f32;
        }
        self.spectrum_position = (self.spectrum_position + 1) % self.spectra.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agrees_with_direct_fir_across_all_partition_boundaries() {
        for length in [1, 127, 128, 129, 255, 256, 257, 1200, 9600] {
            let impulse: Vec<_> = (0..length)
                .map(|i| (i as f32 * 1.71).sin() * (-0.003 * i as f32).exp() * 0.1)
                .collect();
            let input: Vec<_> = (0..1800).map(|i| (i as f32 * 0.319).sin() * 0.3).collect();
            let mut convolver = Convolver::new(&impulse);
            for n in 0..input.len() + length {
                let actual = convolver.tick(input.get(n).copied().unwrap_or(0.0));
                let expected: f32 = impulse
                    .iter()
                    .enumerate()
                    .filter_map(|(k, h)| n.checked_sub(k).and_then(|i| input.get(i)).map(|x| x * h))
                    .sum();
                assert!(
                    (actual - expected).abs() < 2e-5,
                    "IR {length}, frame {n}: {actual} != {expected}"
                );
            }
        }
    }
    #[test]
    fn preserves_late_reflections_without_adding_latency() {
        let mut impulse = vec![0.0; 9600];
        impulse[0] = 1.0;
        impulse[2400] = 0.5;
        impulse[9599] = -0.2;
        let mut c = Convolver::new(&impulse);
        for (i, expected) in impulse.iter().enumerate() {
            assert!((c.tick(if i == 0 { 1.0 } else { 0.0 }) - expected).abs() < 1e-6);
        }
    }
}
