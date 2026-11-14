//! RBJ low shelf, peaking mid, and high shelf; 10 ms coefficient smoothing.
//! Formula reference: https://www.w3.org/TR/audio-eq-cookbook/
//!
//! Voicing follows the reference NeuralAmpModelerPlugin tone stack (as
//! Tone3000 does), so dB settings carry over from NAM/Tone3000: bass shelf at
//! 150 Hz, mid bell at 425 Hz, treble shelf at 1.8 kHz. The mid bell widens
//! when boosting (Q 0.7) and narrows when cutting (Q 1.5) so pushed mids
//! don't turn honky; at 0 dB both are exactly flat, so the switch is seamless.
const FREQUENCIES: [f64; 3] = [150.0, 425.0, 1_800.0];
const MID_Q_BOOST: f64 = 0.7;
const MID_Q_CUT: f64 = 1.5;
const UNITY: [f64; 5] = [1.0, 0.0, 0.0, 0.0, 0.0];
struct Biquad {
    coefficients: [f64; 5],
    target: [f64; 5],
    x: [f64; 2],
    y: [f64; 2],
    smoothing: f64,
}
impl Biquad {
    fn new(rate: f64) -> Self {
        Self {
            coefficients: UNITY,
            target: UNITY,
            x: [0.0; 2],
            y: [0.0; 2],
            smoothing: 1.0 - (-1.0 / (0.01 * rate)).exp(),
        }
    }
    fn tick(&mut self, input: f32) -> f32 {
        for (c, t) in self.coefficients.iter_mut().zip(self.target) {
            *c += (t - *c) * self.smoothing;
        }
        let [b0, b1, b2, a1, a2] = self.coefficients;
        let input = f64::from(input);
        let output = b0 * input + b1 * self.x[0] + b2 * self.x[1] - a1 * self.y[0] - a2 * self.y[1];
        self.x = [input, self.x[0]];
        self.y = [output, self.y[0]];
        output as f32
    }
}

pub struct ToneEq {
    filters: [Biquad; 3],
    gains: [f32; 3],
    rate: f64,
}
impl ToneEq {
    pub fn new(rate: u32, gains: [f32; 3]) -> Self {
        let mut eq = Self {
            filters: std::array::from_fn(|_| Biquad::new(f64::from(rate))),
            gains: [f32::NAN; 3],
            rate: f64::from(rate),
        };
        eq.set_gains(gains);
        for f in &mut eq.filters {
            f.coefficients = f.target;
        }
        eq
    }
    pub fn set_gains(&mut self, gains: [f32; 3]) {
        if self.gains == gains {
            return;
        }
        self.gains = gains;
        for (i, filter) in self.filters.iter_mut().enumerate() {
            let db = gains[i].clamp(-24.0, 24.0) as f64;
            if db.abs() < 1e-9 {
                filter.target = UNITY;
                continue;
            }
            let a = 10.0_f64.powf(db / 40.0);
            let frequency = FREQUENCIES[i].min(self.rate * 0.4);
            let w = std::f64::consts::TAU * frequency / self.rate;
            let c = w.cos();
            let alpha = w.sin() / 2.0_f64.sqrt();
            let beta = 2.0 * a.sqrt() * alpha;
            let (b, a0) = match i {
                0 => (
                    [
                        a * ((a + 1.0) - (a - 1.0) * c + beta),
                        2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                        a * ((a + 1.0) - (a - 1.0) * c - beta),
                        -2.0 * ((a - 1.0) + (a + 1.0) * c),
                        (a + 1.0) + (a - 1.0) * c - beta,
                    ],
                    (a + 1.0) + (a - 1.0) * c + beta,
                ),
                1 => {
                    let q = if db > 0.0 { MID_Q_BOOST } else { MID_Q_CUT };
                    let alpha = w.sin() / (2.0 * q);
                    (
                        [
                            1.0 + alpha * a,
                            -2.0 * c,
                            1.0 - alpha * a,
                            -2.0 * c,
                            1.0 - alpha / a,
                        ],
                        1.0 + alpha / a,
                    )
                }
                _ => (
                    [
                        a * ((a + 1.0) + (a - 1.0) * c + beta),
                        -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                        a * ((a + 1.0) + (a - 1.0) * c - beta),
                        2.0 * ((a - 1.0) - (a + 1.0) * c),
                        (a + 1.0) - (a - 1.0) * c - beta,
                    ],
                    (a + 1.0) - (a - 1.0) * c + beta,
                ),
            };
            filter.target = b.map(|v| v / a0);
        }
    }
    pub fn tick(&mut self, mut sample: f32) -> f32 {
        for filter in &mut self.filters {
            sample = filter.tick(sample);
        }
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response(gains: [f32; 3], hz: f32) -> f32 {
        let mut eq = ToneEq::new(48_000, gains);
        let mut input = 0.0;
        let mut output = 0.0;
        for n in 0..24_000 {
            let s = (std::f32::consts::TAU * hz * n as f32 / 48_000.0).sin() * 0.1;
            let y = eq.tick(s);
            if n > 12_000 {
                input += s * s;
                output += y * y;
            }
        }
        10.0 * (output / input).log10()
    }
    #[test]
    fn controls_have_expected_shelf_and_peak_responses() {
        assert!((response([6.0, 0.0, 0.0], 20.0) - 6.0).abs() < 0.1);
        assert!(response([6.0, 0.0, 0.0], 5000.0).abs() < 0.1);
        assert!((response([0.0, 6.0, 0.0], 425.0) - 6.0).abs() < 0.02);
        assert!((response([0.0, -6.0, 0.0], 425.0) + 6.0).abs() < 0.02);
        assert!((response([0.0, 0.0, -6.0], 18_000.0) + 6.0).abs() < 0.1);
    }
    #[test]
    fn unity_is_exact_and_extreme_sweeps_remain_finite() {
        let mut eq = ToneEq::new(48_000, [0.0; 3]);
        for x in [0.0, 0.3, -0.2, 0.4] {
            assert_eq!(eq.tick(x), x);
        }
        for i in 0..48000 {
            if i % 64 == 0 {
                eq.set_gains(if i % 128 == 0 {
                    [24.0, -24.0, 24.0]
                } else {
                    [-24.0, 24.0, -24.0]
                });
            }
            assert!(eq.tick((i as f32 * 0.1).sin() * 0.01).is_finite());
        }
    }
    #[test]
    fn voicing_matches_the_nam_tone_stack() {
        // Shelves reach half their gain at the corner frequency.
        assert!((response([6.0, 0.0, 0.0], 150.0) - 3.0).abs() < 0.1);
        assert!((response([0.0, 0.0, 6.0], 1_800.0) - 3.0).abs() < 0.1);
        // The mid bell is wider when boosting than when cutting: one octave
        // above 425 Hz a +6 dB boost keeps more of its gain than a -6 dB cut.
        let boost = response([0.0, 6.0, 0.0], 850.0);
        let cut = -response([0.0, -6.0, 0.0], 850.0);
        assert!(boost > 2.0 && cut < 1.5, "boost {boost} cut {cut}");
    }
}
