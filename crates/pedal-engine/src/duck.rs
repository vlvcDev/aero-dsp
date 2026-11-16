//! Ducking for time effects: the wet signal drops while you play and swells
//! back once you stop, so fast runs stay articulate and the space fills the
//! gaps. The detector listens to the dry signal reaching the effect.

/// Ducking starts once the dry signal rises above this level...
const START_DB: f32 = -54.0;
/// ...and reaches full depth this many dB above it.
const SPAN_DB: f32 = 24.0;
/// Wet-signal reduction at DUCK = 100% while playing.
pub const MAX_DEPTH_DB: f32 = 24.0;

pub struct Ducker {
    envelope: f32,
    gain_db: f32,
    env_attack: f32,
    env_release: f32,
    duck_down: f32,
    swell_up: f32,
}

fn coefficient(seconds: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (seconds * sample_rate)).exp()
}

impl Ducker {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            envelope: 0.0,
            gain_db: 0.0,
            env_attack: coefficient(0.001, rate),
            env_release: coefficient(0.060, rate),
            // Fast enough to clear the way for the next pick attack, slow
            // enough to swell (not jump) back in after the last note.
            duck_down: coefficient(0.010, rate),
            swell_up: coefficient(0.350, rate),
        }
    }

    /// Feeds one dry sample and returns the gain for the wet signal.
    /// `amount` 0 always settles on exactly 1.0.
    #[inline]
    pub fn tick(&mut self, dry: f32, amount: f32) -> f32 {
        let level = dry.abs();
        let rate = if level > self.envelope {
            self.env_attack
        } else {
            self.env_release
        };
        self.envelope += (level - self.envelope) * rate;
        let target = if amount <= 0.0 || self.envelope <= 1e-9 {
            0.0
        } else {
            let over = ((20.0 * self.envelope.log10() - START_DB) / SPAN_DB).clamp(0.0, 1.0);
            -MAX_DEPTH_DB * amount.min(1.0) * over
        };
        let rate = if target < self.gain_db {
            self.duck_down
        } else {
            self.swell_up
        };
        self.gain_db += (target - self.gain_db) * rate;
        if self.gain_db > -0.01 {
            self.gain_db = 0.0;
            return 1.0;
        }
        10f32.powf(self.gain_db / 20.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ducks_while_playing_and_swells_back_after() {
        let mut duck = Ducker::new(48_000);
        let mut gain = 1.0;
        for i in 0..48_000 {
            let dry = 0.2 * (i as f32 * 0.05).sin();
            gain = duck.tick(dry, 1.0);
        }
        assert!(gain < 0.1, "should be ~-24 dB while playing, got {gain}");
        for _ in 0..240_000 {
            gain = duck.tick(0.0, 1.0);
        }
        assert_eq!(gain, 1.0);
    }

    #[test]
    fn zero_amount_is_unity() {
        let mut duck = Ducker::new(48_000);
        for i in 0..4_800 {
            assert_eq!(duck.tick((i as f32).sin(), 0.0), 1.0);
        }
    }
}
