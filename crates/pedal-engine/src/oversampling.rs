//! Polyphase half-band IIR oversampler (HIIR structure).
//!
//! A port of Tone3000's `ChainOversampler`. Each 2x stage is a pair of
//! allpass branches (A and B) made of six first-order sections. Factors of
//! 2, 4 and 8 cascade the same stage 1, 2 or 3 times. The passband gain is
//! unity and the reported latency is 0 samples; the physical group delay is
//! a few base-rate samples and is not compensated.
//!
//! Everything is allocation-free after construction, so it is safe to use
//! from a realtime thread.

/// Largest supported oversampling factor.
pub const MAX_FACTOR: usize = 8;

const SECTIONS: usize = 6;

/// Allpass coefficients of branch A, first section first.
const COEFFS_A: [f32; SECTIONS] = [
    0.036_681_502_163_648_017,
    0.274_631_759_379_454_1,
    0.561_098_969_787_919_5,
    0.769_741_833_862_266,
    0.892_260_818_003_878_9,
    0.962_094_548_378_084,
];

/// Allpass coefficients of branch B, first section first.
const COEFFS_B: [f32; SECTIONS] = [
    0.136_547_624_631_957_7,
    0.423_138_617_436_566_67,
    0.677_540_049_974_161_6,
    0.839_889_624_849_638,
    0.931_541_959_963_183_9,
    0.987_816_370_732_897_1,
];

/// Cascade of six first-order allpass sections.
#[derive(Clone, Copy, Default)]
struct AllpassChain {
    x1: [f32; SECTIONS],
    y1: [f32; SECTIONS],
}

impl AllpassChain {
    #[inline]
    fn process(&mut self, coeffs: &[f32; SECTIONS], input: f32) -> f32 {
        let mut x = input;
        for ((a, x1), y1) in coeffs.iter().zip(&mut self.x1).zip(&mut self.y1) {
            let y = a * x + *x1 - a * *y1;
            *x1 = x;
            *y1 = y;
            x = y;
        }
        x
    }
}

/// Filter state of one 2x stage for one channel. Up and down paths are separate.
#[derive(Clone, Copy, Default)]
struct StageState {
    up_a: AllpassChain,
    up_b: AllpassChain,
    down_even: AllpassChain,
    down_odd: AllpassChain,
}

impl StageState {
    /// Doubles the rate: `output.len() == 2 * input.len()`.
    fn upsample(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(output.len(), input.len() * 2);
        for (&x, out) in input.iter().zip(output.chunks_exact_mut(2)) {
            out[0] = self.up_a.process(&COEFFS_A, x);
            out[1] = self.up_b.process(&COEFFS_B, x);
        }
    }

    /// Halves the rate: `input.len() == 2 * output.len()`.
    fn downsample(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len() * 2);
        for (pair, out) in input.chunks_exact(2).zip(output.iter_mut()) {
            let even = self.down_even.process(&COEFFS_B, pair[0]);
            let odd = self.down_odd.process(&COEFFS_A, pair[1]);
            *out = 0.5 * (even + odd);
        }
    }
}

/// Multi-channel oversampler for factors 1, 2, 4 and 8.
pub struct Oversampler {
    factor: usize,
    stages: usize,
    channels: usize,
    max_base_frames: usize,
    /// `channels * stages` entries, indexed `channel * stages + stage`.
    states: Vec<StageState>,
    /// Two ping-pong buffers of `max_base_frames * factor` samples each.
    work: Vec<f32>,
}

impl Oversampler {
    /// Creates an oversampler. `factor` must be 1, 2, 4 or 8; any other value
    /// is rounded down to the nearest valid factor (0 becomes 1, values above
    /// 8 become 8).
    pub fn new(factor: usize, channels: usize, max_base_frames: usize) -> Self {
        let factor = match factor {
            0 | 1 => 1,
            2 | 3 => 2,
            4..=7 => 4,
            _ => MAX_FACTOR,
        };
        let stages = factor.trailing_zeros() as usize;
        Self {
            factor,
            stages,
            channels,
            max_base_frames,
            states: vec![StageState::default(); channels * stages],
            work: vec![0.0; 2 * max_base_frames * factor],
        }
    }

    pub fn factor(&self) -> usize {
        self.factor
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn max_base_frames(&self) -> usize {
        self.max_base_frames
    }

    /// Clears all filter state.
    pub fn reset(&mut self) {
        self.states.fill(StageState::default());
    }

    /// Upsamples `input` (base frames) into `output`
    /// (`input.len() * factor` frames). Inputs longer than `max_base_frames`
    /// are processed in slices; the result is identical.
    pub fn upsample(&mut self, channel: usize, input: &[f32], output: &mut [f32]) {
        debug_assert!(channel < self.channels);
        debug_assert_eq!(output.len(), input.len() * self.factor);
        if self.factor == 1 {
            output.copy_from_slice(input);
            return;
        }
        let step = self.max_base_frames.max(1);
        for (inp, out) in input
            .chunks(step)
            .zip(output.chunks_mut(step * self.factor))
        {
            self.upsample_slice(channel, inp, out);
        }
    }

    /// Downsamples `input` (`output.len() * factor` frames) into `output`
    /// (base frames). Outputs longer than `max_base_frames` are processed in
    /// slices; the result is identical.
    pub fn downsample(&mut self, channel: usize, input: &[f32], output: &mut [f32]) {
        debug_assert!(channel < self.channels);
        debug_assert_eq!(input.len(), output.len() * self.factor);
        if self.factor == 1 {
            output.copy_from_slice(input);
            return;
        }
        let step = self.max_base_frames.max(1);
        for (inp, out) in input
            .chunks(step * self.factor)
            .zip(output.chunks_mut(step))
        {
            self.downsample_slice(channel, inp, out);
        }
    }

    fn upsample_slice(&mut self, channel: usize, input: &[f32], output: &mut [f32]) {
        let cap = self.max_base_frames * self.factor;
        let (ping, pong) = self.work.split_at_mut(cap);
        let states = &mut self.states[channel * self.stages..][..self.stages];
        let mut len = input.len();
        ping[..len].copy_from_slice(input);
        let (mut src, mut dst) = (ping, pong);
        for state in states {
            state.upsample(&src[..len], &mut dst[..len * 2]);
            len *= 2;
            std::mem::swap(&mut src, &mut dst);
        }
        output.copy_from_slice(&src[..len]);
    }

    fn downsample_slice(&mut self, channel: usize, input: &[f32], output: &mut [f32]) {
        let cap = self.max_base_frames * self.factor;
        let (ping, pong) = self.work.split_at_mut(cap);
        let states = &mut self.states[channel * self.stages..][..self.stages];
        let mut len = input.len();
        ping[..len].copy_from_slice(input);
        let (mut src, mut dst) = (ping, pong);
        for state in states.iter_mut().rev() {
            state.downsample(&src[..len], &mut dst[..len / 2]);
            len /= 2;
            std::mem::swap(&mut src, &mut dst);
        }
        output.copy_from_slice(&src[..len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const FS: f64 = 48_000.0;

    fn sine(freq: f64, amp: f64, fs: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * freq * i as f64 / fs).sin()) as f32)
            .collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut s = 0x1234_5678_u32;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 23) as f32 - 1.0
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn round_trip(os: &mut Oversampler, input: &[f32]) -> Vec<f32> {
        let mut up = vec![0.0; input.len() * os.factor()];
        let mut out = vec![0.0; input.len()];
        os.upsample(0, input, &mut up);
        os.downsample(0, &up, &mut out);
        out
    }

    /// In-place radix-2 FFT.
    fn fft(re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let ang = -2.0 * PI / len as f64;
            for start in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = ((ang * k as f64).cos(), (ang * k as f64).sin());
                    let (a, b) = (start + k, start + k + len / 2);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len <<= 1;
        }
    }

    /// Blackman-Harris windowed power spectrum (bins 0..=n/2).
    fn power_spectrum(x: &[f32]) -> Vec<f64> {
        let n = x.len();
        let mut re: Vec<f64> = x
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let p = 2.0 * PI * i as f64 / n as f64;
                let w = 0.35875 - 0.48829 * p.cos() + 0.14128 * (2.0 * p).cos()
                    - 0.01168 * (3.0 * p).cos();
                v as f64 * w
            })
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        (0..=n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
    }

    fn db(ratio: f64) -> f64 {
        10.0 * ratio.max(1e-300).log10()
    }

    #[test]
    fn factor_one_is_bit_exact() {
        let input = noise(300);
        let mut os = Oversampler::new(1, 1, 128);
        let mut up = vec![0.0; 300];
        let mut down = vec![0.0; 300];
        os.upsample(0, &input, &mut up);
        os.downsample(0, &up, &mut down);
        assert_eq!(up, input);
        assert_eq!(down, input);
    }

    #[test]
    fn invalid_factors_are_rounded_down() {
        for (asked, got) in [(0, 1), (3, 2), (5, 4), (7, 4), (16, 8)] {
            assert_eq!(Oversampler::new(asked, 1, 8).factor(), got);
        }
    }

    #[test]
    fn passband_gain_is_unity() {
        let input = sine(997.0, 0.5, FS, 16_384);
        for factor in [2, 4, 8] {
            let mut os = Oversampler::new(factor, 1, 512);
            let out = round_trip(&mut os, &input);
            let gain_db = 20.0 * (rms(&out[8192..]) / rms(&input[8192..])).log10();
            eprintln!("passband factor {factor}: {gain_db:.5} dB");
            assert!(gain_db.abs() < 0.1, "factor {factor}: {gain_db} dB");
        }
    }

    #[test]
    fn round_trip_delay_is_small() {
        let input = noise(8192);
        for factor in [2, 4, 8] {
            let mut os = Oversampler::new(factor, 1, 512);
            let out = round_trip(&mut os, &input);
            let mut best = (0, f64::MIN);
            for lag in 0..=16 {
                let c: f64 = (64..input.len())
                    .map(|n| out[n] as f64 * input[n - lag] as f64)
                    .sum();
                if c > best.1 {
                    best = (lag, c);
                }
            }
            eprintln!("round-trip factor {factor}: best lag {}", best.0);
            assert!(best.0 <= 8, "factor {factor}: lag {}", best.0);
        }
    }

    #[test]
    fn upsampling_rejects_spectral_image() {
        // 10 kHz at 48 kHz, doubled to 96 kHz: the zero-stuffing image sits at
        // 48 kHz - 10 kHz = 38 kHz in the upsampled signal.
        let base_n = 8192;
        let total = 4096 + base_n;
        let bin = 1707.0; // bin-centred in a 16384-point FFT at 96 kHz
        let f = bin * 2.0 * FS / (2 * base_n) as f64;
        let input = sine(f, 0.5, FS, total);
        let mut os = Oversampler::new(2, 1, 1024);
        let mut up = vec![0.0; total * 2];
        os.upsample(0, &input, &mut up);
        let spec = power_spectrum(&up[4096 * 2..]);
        let peak = spec[1707];
        let image = spec[base_n - 1707];
        let rejection = db(peak / image);
        eprintln!("image rejection: {rejection:.1} dB");
        assert!(rejection > 80.0, "{rejection} dB");
    }

    /// Aliasing power (dB re fundamental) of a hard-clipped 4998.05 Hz sine.
    fn clip_aliasing_db(factor: usize) -> f64 {
        const N: usize = 16_384;
        const FUND_BIN: usize = 1706; // 4998.05 Hz at 48 kHz
        let input = sine(FUND_BIN as f64 * FS / N as f64, 2.5, FS, N + 4096);
        let mut os = Oversampler::new(factor, 1, 1024);
        let mut up = vec![0.0; input.len() * factor];
        os.upsample(0, &input, &mut up);
        for v in &mut up {
            *v = v.clamp(-1.0, 1.0);
        }
        let mut out = vec![0.0; input.len()];
        os.downsample(0, &up, &mut out);
        let spec = power_spectrum(&out[4096..]);
        let near_harmonic = |k: usize| {
            let m = (k + FUND_BIN / 2) / FUND_BIN;
            m >= 1 && k.abs_diff(m * FUND_BIN) <= 5
        };
        let fundamental: f64 = (FUND_BIN - 5..=FUND_BIN + 5).map(|k| spec[k]).sum();
        let alias: f64 = (6..spec.len())
            .filter(|&k| !near_harmonic(k))
            .map(|k| spec[k])
            .sum();
        db(alias / fundamental)
    }

    #[test]
    fn oversampling_reduces_clipper_aliasing() {
        let levels: Vec<f64> = [1, 2, 4, 8].iter().map(|&f| clip_aliasing_db(f)).collect();
        eprintln!("aliasing dB at 1x/2x/4x/8x: {levels:.1?}");
        assert!(levels[3] <= levels[0] - 20.0, "{levels:?}");
        for pair in levels.windows(2) {
            assert!(
                pair[1] < pair[0] - 3.0 || pair[1] < -110.0,
                "no improvement: {levels:?}"
            );
        }
    }

    #[test]
    fn block_size_independent() {
        let input = noise(1000);
        let sizes = [37, 64, 1, 200, 3, 128, 99];
        for factor in [2, 4, 8] {
            let mut whole = Oversampler::new(factor, 1, 1024);
            let mut up_ref = vec![0.0; 1000 * factor];
            whole.upsample(0, &input, &mut up_ref);
            let mut down_ref = vec![0.0; 1000];
            whole.downsample(0, &up_ref, &mut down_ref);

            let mut chunked = Oversampler::new(factor, 1, 1024);
            let (mut up, mut down) = (vec![0.0; 1000 * factor], vec![0.0; 1000]);
            let (mut pos, mut i) = (0, 0);
            while pos < 1000 {
                let n = sizes[i % sizes.len()].min(1000 - pos);
                i += 1;
                chunked.upsample(
                    0,
                    &input[pos..pos + n],
                    &mut up[pos * factor..(pos + n) * factor],
                );
                pos += n;
            }
            assert_eq!(up, up_ref, "up factor {factor}");
            let (mut pos, mut i) = (0, 0);
            while pos < 1000 {
                let n = sizes[i % sizes.len()].min(1000 - pos);
                i += 1;
                chunked.downsample(
                    0,
                    &up[pos * factor..(pos + n) * factor],
                    &mut down[pos..pos + n],
                );
                pos += n;
            }
            assert_eq!(down, down_ref, "down factor {factor}");
        }
    }

    #[test]
    fn oversized_calls_are_sliced() {
        let input = noise(1000);
        for factor in [2, 4, 8] {
            let mut small = Oversampler::new(factor, 2, 64);
            let mut big = Oversampler::new(factor, 2, 4096);
            let (mut a, mut b) = (vec![0.0; 1000 * factor], vec![0.0; 1000 * factor]);
            small.upsample(1, &input, &mut a);
            big.upsample(1, &input, &mut b);
            assert_eq!(a, b);
            let (mut da, mut db_) = (vec![0.0; 1000], vec![0.0; 1000]);
            small.downsample(1, &a, &mut da);
            big.downsample(1, &b, &mut db_);
            assert_eq!(da, db_);
        }
    }

    #[test]
    fn reset_restores_initial_state() {
        let input = noise(256);
        let mut os = Oversampler::new(4, 1, 256);
        let mut first = vec![0.0; 1024];
        os.upsample(0, &input, &mut first);
        os.reset();
        let mut second = vec![0.0; 1024];
        os.upsample(0, &input, &mut second);
        assert_eq!(first, second);
    }
}
