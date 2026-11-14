//! Shared 12 dB/oct state-variable filter (cab cuts, amp TIGHT).

/// Second-order (12 dB/oct) Butterworth state-variable filter coefficients,
/// in the topology-preserving (trapezoidal) form. Unlike a direct-form
/// biquad, its state stays well-behaved when the cutoff moves under audio.
#[derive(Clone, Copy)]
pub(crate) struct SvfCoefficients {
    a1: f32,
    a2: f32,
    a3: f32,
}

impl SvfCoefficients {
    pub(crate) const DAMPING: f32 = std::f32::consts::SQRT_2; // 1/Q, Butterworth

    pub(crate) fn new(frequency: f32, sample_rate: f32) -> Self {
        let g = (std::f32::consts::PI * frequency.min(0.45 * sample_rate) / sample_rate).tan();
        let a1 = 1.0 / (1.0 + g * (g + Self::DAMPING));
        let a2 = g * a1;
        Self { a1, a2, a3: g * a2 }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    /// Returns (low-pass, high-pass).
    #[inline]
    pub(crate) fn tick(&mut self, input: f32, c: SvfCoefficients) -> (f32, f32) {
        let v3 = input - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, input - SvfCoefficients::DAMPING * v1 - v2)
    }
}
