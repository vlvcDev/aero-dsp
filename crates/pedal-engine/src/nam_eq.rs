//! Per-NAM six-band EQ. Channel-strip roles/routing reference TONE3000 BlockEq;
//! original Rust implementation of RBJ cookbook filters. See docs/nam-eq.md.
use crate::dsp::MonoProcessor;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering::SeqCst},
};

const UNITY: [f64; 5] = [1.0, 0.0, 0.0, 0.0, 0.0];

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Band {
    pub freq_hz: f32,
    pub gain_db: f32,
    pub q: f32,
    /// Only the first/last band can switch from shelf to cut.
    pub cut: bool,
}
impl Default for Band {
    fn default() -> Self {
        Self {
            freq_hz: 1000.0,
            gain_db: 0.0,
            q: 1.0,
            cut: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamEqSettings {
    pub enabled: bool,
    pub pre: bool,
    pub bands: [Band; 6],
}
impl Default for NamEqSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            pre: false,
            bands: std::array::from_fn(|i| Band {
                freq_hz: [100.0, 250.0, 650.0, 1600.0, 3500.0, 8000.0][i],
                q: [0.71, 1.0, 1.0, 1.0, 1.4, 0.71][i],
                ..Band::default()
            }),
        }
    }
}
impl NamEqSettings {
    pub fn set_control(&mut self, key: &str, value: f32) -> Result<(), &'static str> {
        if !value.is_finite() {
            return Err("EQ value must be finite");
        }
        if matches!(key, "enabled" | "pre" | "reset") {
            if value != 0.0 && value != 1.0 {
                return Err("EQ switch must be 0 or 1");
            }
            match key {
                "enabled" => self.enabled = value == 1.0,
                "pre" => self.pre = value == 1.0,
                "reset" if value == 1.0 => *self = Self::default(),
                _ => (),
            }
            return Ok(());
        }
        let (index, field) = key
            .strip_prefix("band_")
            .and_then(|s| s.split_once('_'))
            .ok_or("unknown NAM EQ control")?;
        let index = index.parse::<usize>().map_err(|_| "invalid EQ band")?;
        let band = self.bands.get_mut(index).ok_or("invalid EQ band")?;
        let (target, min, max) = match field {
            "freq_hz" => (&mut band.freq_hz, 20.0, 20000.0),
            "gain_db" => (&mut band.gain_db, -15.0, 15.0),
            "q" => (&mut band.q, 0.1, 10.0),
            "cut" if index == 0 || index == 5 => {
                if value != 0.0 && value != 1.0 {
                    return Err("EQ cut must be 0 or 1");
                }
                band.cut = value == 1.0;
                return Ok(());
            }
            _ => return Err("unknown NAM EQ control"),
        };
        if !(min..=max).contains(&value) {
            return Err("EQ value outside supported range");
        }
        *target = value;
        Ok(())
    }
}

fn bounded(x: f32, lo: f64, hi: f64, fallback: f64) -> f64 {
    if x.is_finite() {
        f64::from(x).clamp(lo, hi)
    } else {
        fallback.clamp(lo, hi)
    }
}
fn coefficients(b: Band, index: usize, rate: u32) -> [f64; 5] {
    let cut = b.cut && (index == 0 || index == 5);
    let gain = bounded(b.gain_db, -15.0, 15.0, 0.0);
    if !cut && gain.abs() < 0.000001 {
        return UNITY;
    }
    let rate = f64::from(rate.max(1000));
    let w =
        std::f64::consts::TAU * bounded(b.freq_hz, 20.0, (rate * 0.49).min(20000.0), 1000.0) / rate;
    let c = w.cos();
    let alpha = w.sin() / (2.0 * bounded(b.q, 0.1, 10.0, 0.71));
    let a = 10.0_f64.powf(gain / 40.0);
    let beta = 2.0 * a.sqrt() * alpha;
    let (v, a0) = if cut {
        let sign = if index == 0 { 1.0 } else { -1.0 };
        let x = (1.0 + sign * c) / 2.0;
        ([x, -sign * 2.0 * x, x, -2.0 * c, 1.0 - alpha], 1.0 + alpha)
    } else if index == 0 {
        (
            [
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            ],
            (a + 1.0) + (a - 1.0) * c + beta,
        )
    } else if index == 5 {
        (
            [
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            ],
            (a + 1.0) - (a - 1.0) * c + beta,
        )
    } else {
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
    };
    v.map(|x| x / a0)
}

/// Single control-thread writer. Audio reads once per block, never spins or locks.
/// A revision check rejects torn updates; it retains the previous valid bank.
pub struct NamEqParameters {
    revision: AtomicU64,
    values: [AtomicU64; 31],
}
impl NamEqParameters {
    pub fn new(s: &NamEqSettings, rate: u32) -> Self {
        let p = Self {
            revision: AtomicU64::new(0),
            values: std::array::from_fn(|_| AtomicU64::new(0)),
        };
        p.update(s, rate);
        p
    }
    pub fn update(&self, s: &NamEqSettings, rate: u32) {
        // All filter math runs on the control/loading thread.
        let bank: [[f64; 5]; 6] = std::array::from_fn(|i| {
            if s.enabled {
                coefficients(s.bands[i], i, rate)
            } else {
                UNITY
            }
        });
        self.revision.fetch_add(1, SeqCst);
        for (target, value) in self.values.iter().zip(bank.into_iter().flatten()) {
            target.store(value.to_bits(), SeqCst);
        }
        self.values[30].store(u64::from(s.pre), SeqCst);
        self.revision.fetch_add(1, SeqCst);
    }
    fn read(&self) -> Option<([[f64; 5]; 6], bool)> {
        let before = self.revision.load(SeqCst);
        if !before.is_multiple_of(2) {
            return None;
        }
        let bank = std::array::from_fn(|i| {
            std::array::from_fn(|j| f64::from_bits(self.values[i * 5 + j].load(SeqCst)))
        });
        let pre = self.values[30].load(SeqCst) != 0;
        (before == self.revision.load(SeqCst)).then_some((bank, pre))
    }
}

#[derive(Clone, Copy)]
struct Filter {
    c: [f64; 5],
    target: [f64; 5],
    x: [f64; 2],
    y: [f64; 2],
    remaining: u32,
}
impl Filter {
    fn new(c: [f64; 5]) -> Self {
        Self {
            c,
            target: c,
            x: [0.0; 2],
            y: [0.0; 2],
            remaining: 0,
        }
    }
    fn target(&mut self, c: [f64; 5], frames: u32) {
        if c != self.target {
            self.target = c;
            self.remaining = frames;
        }
    }
    fn tick(&mut self, x: f32) -> f32 {
        if self.remaining == 0 && self.c == UNITY {
            return x;
        }
        if self.remaining > 0 {
            for (c, t) in self.c.iter_mut().zip(self.target) {
                *c += (t - *c) / f64::from(self.remaining);
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                self.c = self.target;
            }
        }
        let x = if x.is_finite() { f64::from(x) } else { 0.0 };
        let [b0, b1, b2, a1, a2] = self.c;
        let y = b0 * x + b1 * self.x[0] + b2 * self.x[1] - a1 * self.y[0] - a2 * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [if y.abs() < 1e-30 { 0.0 } else { y }, self.y[0]];
        if !y.is_finite() || y.abs() > f64::from(f32::MAX) {
            self.x = [0.0; 2];
            self.y = [0.0; 2];
            return 0.0;
        }
        if self.remaining == 0 && self.c == UNITY {
            self.x = [0.0; 2];
            self.y = [0.0; 2];
        }
        y as f32
    }
}

pub struct NamEqProcessor<P> {
    inner: P,
    parameters: Arc<NamEqParameters>,
    before: [Filter; 6],
    after: [Filter; 6],
    ramp: u32,
}
impl<P> NamEqProcessor<P> {
    pub fn new(inner: P, parameters: Arc<NamEqParameters>, rate: u32) -> Self {
        let (bank, pre) = parameters.read().unwrap_or(([UNITY; 6], false));
        Self {
            inner,
            parameters,
            before: if pre { bank } else { [UNITY; 6] }.map(Filter::new),
            after: if pre { [UNITY; 6] } else { bank }.map(Filter::new),
            ramp: (rate / 100).max(1),
        }
    }
    fn process_bank(bank: &mut [Filter; 6], samples: &mut [f32]) {
        for f in bank {
            if f.remaining == 0 && f.c == UNITY {
                continue;
            }
            for x in samples.iter_mut() {
                *x = f.tick(*x);
            }
        }
    }
}
impl<P: MonoProcessor> MonoProcessor for NamEqProcessor<P> {
    fn dry_wet_alignment(&self) -> Option<nam_bridge::alignment::AlignmentEstimate> {
        self.inner.dry_wet_alignment()
    }
    fn model_metadata(&self) -> Option<nam_bridge::ModelMetadata> {
        self.inner.model_metadata()
    }
    fn discontinuity(&mut self) {
        self.inner.discontinuity();
        for f in self.before.iter_mut().chain(&mut self.after) {
            f.x = [0.0; 2];
            f.y = [0.0; 2];
        }
    }
    fn process(&mut self, samples: &mut [f32]) {
        if let Some((bank, pre)) = self.parameters.read() {
            for (i, c) in bank.into_iter().enumerate() {
                self.before[i].target(if pre { c } else { UNITY }, self.ramp);
                self.after[i].target(if pre { UNITY } else { c }, self.ramp);
            }
        }
        Self::process_bank(&mut self.before, samples);
        self.inner.process(samples);
        Self::process_bank(&mut self.after, samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Identity;
    impl MonoProcessor for Identity {
        fn process(&mut self, _: &mut [f32]) {}
    }
    struct Saturate;
    impl MonoProcessor for Saturate {
        fn process(&mut self, s: &mut [f32]) {
            for x in s {
                *x = (*x * 4.0).tanh();
            }
        }
    }
    #[test]
    fn concurrent_publication_never_exposes_a_partial_bank() {
        let a = NamEqSettings::default();
        let mut b = a;
        b.pre = true;
        for band in &mut b.bands {
            band.gain_db = -6.0;
        }
        let expected_a = NamEqParameters::new(&a, 48000).read().unwrap();
        let expected_b = NamEqParameters::new(&b, 48000).read().unwrap();
        let p = Arc::new(NamEqParameters::new(&a, 48000));
        let writer = Arc::clone(&p);
        let thread = std::thread::spawn(move || {
            for i in 0..2000 {
                writer.update(if i % 2 == 0 { &a } else { &b }, 48000);
            }
        });
        for _ in 0..10000 {
            if let Some(bank) = p.read() {
                assert!(bank == expected_a || bank == expected_b);
            }
        }
        thread.join().unwrap();
    }

    #[test]
    #[ignore = "manual release-build CPU measurement, not a timing assertion"]
    fn nam_eq_processing_benchmark() {
        for active in [false, true] {
            let mut s = NamEqSettings::default();
            if active {
                for (i, b) in s.bands.iter_mut().enumerate() {
                    b.gain_db = if i % 2 == 0 { 3.0 } else { -3.0 };
                }
            }
            let mut p =
                NamEqProcessor::new(Identity, Arc::new(NamEqParameters::new(&s, 48000)), 48000);
            let original: [f32; 64] = std::array::from_fn(|i| (i as f32 * 0.1).sin() * 0.01);
            let started = std::time::Instant::now();
            let mut worst = std::time::Duration::ZERO;
            let mut missed = 0;
            for _ in 0..7500 {
                let t = std::time::Instant::now();
                let mut x = original;
                p.process(std::hint::black_box(&mut x));
                std::hint::black_box(x);
                let elapsed = t.elapsed();
                worst = worst.max(elapsed);
                if elapsed.as_secs_f64() > 64.0 / 48000.0 {
                    missed += 1;
                }
            }
            println!(
                "Six-band EQ active={active}: 10s input, total {:?}, worst block {worst:?}, deadline misses {missed}",
                started.elapsed()
            );
        }
    }
    fn response(settings: NamEqSettings, hz: f32, rate: u32) -> f32 {
        let mut p = NamEqProcessor::new(
            Identity,
            Arc::new(NamEqParameters::new(&settings, rate)),
            rate,
        );
        let mut input = 0.0;
        let mut output = 0.0;
        for n in 0..rate {
            let x = (std::f32::consts::TAU * hz * n as f32 / rate as f32).sin() * 0.01;
            let mut s = [x];
            p.process(&mut s);
            if n > rate / 2 {
                input += x * x;
                output += s[0] * s[0];
            }
        }
        10.0_f32 * (output / input).log10()
    }
    #[test]
    fn measured_shelves_bells_and_cuts_match_their_settings() {
        for rate in [44100, 48000, 96000] {
            let mut s = NamEqSettings::default();
            s.bands[0].gain_db = 6.0;
            assert!((response(s, 20.0, rate) - 6.0).abs() < 0.15);
            s = NamEqSettings::default();
            s.bands[3].gain_db = -9.0;
            assert!((response(s, 1600.0, rate) + 9.0).abs() < 0.03);
            s = NamEqSettings::default();
            s.bands[5].freq_hz = 3500.0;
            s.bands[5].gain_db = 6.0;
            assert!((response(s, 18000.0, rate) - 6.0).abs() < 0.1);
            s = NamEqSettings::default();
            s.bands[0].cut = true;
            s.bands[0].freq_hz = 200.0;
            assert!(response(s, 20.0, rate) < -39.0);
            assert!(response(s, 2000.0, rate).abs() < 0.1);
            s = NamEqSettings::default();
            s.bands[5].cut = true;
            s.bands[5].freq_hz = 2000.0;
            assert!(response(s, 16000.0, rate) < -35.0);
        }
    }
    #[test]
    fn flat_and_disabled_are_exact_and_pre_changes_distortion_not_dry_mix() {
        let input: Vec<_> = (0..4096).map(|n| (n as f32 * 0.1).sin() * 0.1).collect();
        let render = |s: NamEqSettings| {
            let mut p =
                NamEqProcessor::new(Saturate, Arc::new(NamEqParameters::new(&s, 48000)), 48000);
            let mut x = input.clone();
            p.process(&mut x);
            x
        };
        let mut s = NamEqSettings::default();
        let flat = render(s);
        assert_eq!(
            flat,
            input.iter().map(|x| (*x * 4.0).tanh()).collect::<Vec<_>>()
        );
        s.bands[2].gain_db = 12.0;
        let post = render(s);
        s.pre = true;
        let pre = render(s);
        assert!(pre.iter().zip(&post).any(|(a, b)| (a - b).abs() > 0.01));
        s.enabled = false;
        assert_eq!(flat, render(s));
        s.enabled = true;
        for pre in [false, true] {
            s.pre = pre;
            let eq =
                NamEqProcessor::new(Saturate, Arc::new(NamEqParameters::new(&s, 48000)), 48000);
            let mut pedal = crate::dsp::PedalControlProcessor::new(
                eq,
                Arc::new(crate::dsp::PedalParameters::new(12.0, 0.0, 0.0)),
                4096,
                48000,
            );
            let mut dry = input.clone();
            pedal.process(&mut dry);
            assert_eq!(dry, input);
        }
    }
    #[test]
    fn transitions_are_block_invariant_and_return_to_exact_bypass() {
        fn run(block: usize) -> Vec<f32> {
            let mut s = NamEqSettings::default();
            let params = Arc::new(NamEqParameters::new(&s, 48000));
            let mut p = NamEqProcessor::new(Identity, params.clone(), 48000);
            let mut output = Vec::new();
            for phase in 0..4 {
                s.bands[2].gain_db = 9.0;
                s.enabled = phase != 3;
                s.pre = phase == 2;
                params.update(&s, 48000);
                let mut x: Vec<_> = (0..4800)
                    .map(|n| ((n + phase * 4800) as f32 * 0.04).sin() * 0.01)
                    .collect();
                for chunk in x.chunks_mut(block) {
                    p.process(chunk);
                }
                output.extend(x);
            }
            assert!(output.iter().all(|x| x.is_finite() && x.abs() < 0.1));
            assert!(output.windows(2).all(|p| (p[1] - p[0]).abs() < 0.02));
            for (n, x) in output.iter().enumerate().skip(15000) {
                assert_eq!(*x, (n as f32 * 0.04).sin() * 0.01);
            }
            output
        }
        assert_eq!(run(64), run(37));
    }
    #[test]
    fn extreme_updates_and_bad_saved_values_do_not_poison_filters() {
        let mut s = NamEqSettings::default();
        let params = Arc::new(NamEqParameters::new(&s, 48000));
        let mut p = NamEqProcessor::new(Identity, params.clone(), 48000);
        for i in 0..300 {
            for b in &mut s.bands {
                b.freq_hz = if i % 2 == 0 { 20.0 } else { 20000.0 };
                b.gain_db = if i % 3 == 0 { 15.0 } else { -15.0 };
                b.q = if i % 2 == 0 { 0.1 } else { 10.0 };
                b.cut = true;
            }
            params.update(&s, 48000);
            let mut x = [0.000001; 64];
            p.process(&mut x);
            assert!(x.iter().all(|x| x.is_finite()));
        }
        s.bands[0].freq_hz = f32::NAN;
        s.bands[5].q = f32::INFINITY;
        params.update(&s, 48000);
        let mut x = [f32::NAN; 64];
        p.process(&mut x);
        assert!(x.iter().all(|x| x.is_finite()));
    }
    #[test]
    fn control_bounds_and_preset_legacy_defaults() {
        let mut s = NamEqSettings::default();
        for (key, value) in [
            ("band_6_q", 1.0),
            ("band_2_cut", 1.0),
            ("pre", 0.5),
            ("band_0_q", 0.0),
            ("band_0_gain_db", f32::NAN),
            ("band_1_freq_hz", 20001.0),
        ] {
            let old = s;
            assert!(s.set_control(key, value).is_err());
            assert_eq!(s, old);
        }
        s.set_control("band_2_gain_db", 6.0).unwrap();
        s.pre = true;
        let state = crate::EngineState {
            pedal_eq: s,
            ..Default::default()
        };
        let saved = serde_json::to_string(&crate::presets::PresetRig::from_state(&state)).unwrap();
        let preset: crate::presets::PresetRig = serde_json::from_str(&saved).unwrap();
        let mut restored = crate::EngineState::default();
        preset.apply_to(&mut restored);
        assert_eq!(restored.pedal_eq, s);
        let old: crate::presets::PresetRig = serde_json::from_str("{}").unwrap();
        assert_eq!(old.amp_eq, NamEqSettings::default());
        s.set_control("reset", 1.0).unwrap();
        assert_eq!(s, NamEqSettings::default());
    }
}
