//! Shared 12 dB/oct state-variable filter (cab cuts, amp TIGHT, compressor
//! detector, DE-MUD band).

/// Second-order state-variable filter coefficients in the
/// topology-preserving (trapezoidal) form. Unlike a direct-form biquad, its
/// state stays well-behaved when the cutoff moves under audio.
#[derive(Clone, Copy)]
pub(crate) struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
    /// Damping, 1/Q.
    k: f32,
}

impl SvfCoefficients {
    /// Butterworth (Q = 1/sqrt(2)): maximally flat low/high-pass.
    pub(crate) fn new(frequency: f32, sample_rate: f32) -> Self {
        Self::with_q(frequency, sample_rate, std::f32::consts::FRAC_1_SQRT_2)
    }

    pub(crate) fn with_q(frequency: f32, sample_rate: f32, q: f32) -> Self {
        let k = 1.0 / q.max(0.05);
        let g = (std::f32::consts::PI * frequency.min(0.45 * sample_rate) / sample_rate).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        Self {
            a1,
            a2,
            a3: g * a2,
            k,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Advances the filter; returns (band v1, low v2).
    #[inline]
    fn step(&mut self, input: f32, c: SvfCoefficients) -> (f32, f32) {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v1, v2)
    }

    /// Returns (low-pass, high-pass).
    #[inline]
    pub(crate) fn tick(&mut self, input: f32, c: SvfCoefficients) -> (f32, f32) {
        let (v1, v2) = self.step(input, c);
        (v2, input - c.k * v1 - v2)
    }

    /// Band-pass with unity gain at the centre frequency.
    #[inline]
    pub(crate) fn band_pass(&mut self, input: f32, c: SvfCoefficients) -> f32 {
        c.k * self.step(input, c).0
    }
}
