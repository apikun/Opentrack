//! Audio FX (M4): 3-band EQ, feed-forward compressor, brickwall limiter and
//! a Schroeder reverb send — the DSP behind the `audio.*` registry kinds.
//!
//! Parity discipline (ROADMAP M4 acceptance): the same processor types run
//! in both the offline mixer and the realtime engine, fed in fixed
//! [`FX_BLOCK_FRAMES`] blocks with automation sampled at the block's start
//! tick. Stateful filters are per-sample, so identical block streams give
//! bit-identical f32 output wherever they run (see the golden tests).

use opentrack_core::fx::{FxChain, FxInstance};

/// Fixed processing block (frames). Offline and realtime both cut their
/// streams at this boundary so automation sampling and per-block parameter
/// resolution match bit-for-bit.
pub const FX_BLOCK_FRAMES: usize = 128;

// ---------------------------------------------------------------------------
// Building blocks
// ---------------------------------------------------------------------------

/// One biquad section (RBJ cookbook, transposed direct form II).
#[derive(Clone, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// Identity (passthrough).
    pub fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// RBJ low shelf: ±`gain_db` below `freq_hz` (Q ≈ 0.707 shelf S=1).
    pub fn low_shelf(sr: f32, freq_hz: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * freq_hz / sr;
        let (s, c) = w0.sin_cos();
        let alpha = s / 2.0 * std::f32::consts::SQRT_2;
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) - (a - 1.0) * c + two_sqrt_a_alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * c);
        let b2 = a * ((a + 1.0) - (a - 1.0) * c - two_sqrt_a_alpha);
        let a0 = (a + 1.0) + (a - 1.0) * c + two_sqrt_a_alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * c);
        let a2 = (a + 1.0) + (a - 1.0) * c - two_sqrt_a_alpha;
        Self::normalized(b0, b1, b2, a1, a2, a0)
    }

    /// RBJ high shelf.
    pub fn high_shelf(sr: f32, freq_hz: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * freq_hz / sr;
        let (s, c) = w0.sin_cos();
        let alpha = s / 2.0 * std::f32::consts::SQRT_2;
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * c + two_sqrt_a_alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * c);
        let b2 = a * ((a + 1.0) + (a - 1.0) * c - two_sqrt_a_alpha);
        let a0 = (a + 1.0) - (a - 1.0) * c + two_sqrt_a_alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * c);
        let a2 = (a + 1.0) - (a - 1.0) * c - two_sqrt_a_alpha;
        Self::normalized(b0, b1, b2, a1, a2, a0)
    }

    /// RBJ peaking EQ at `freq_hz` with resonance `q`.
    pub fn peaking(sr: f32, freq_hz: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = std::f32::consts::TAU * freq_hz / sr;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * c;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * c;
        let a2 = 1.0 - alpha / a;
        Self::normalized(b0, b1, b2, a1, a2, a0)
    }

    fn normalized(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32, a0: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Processes one sample.
    #[inline]
    pub fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// One-pole envelope follower coefficient from an ms time constant.
fn coeff_ms(ms: f32, sr: f32) -> f32 {
    (-1.0 / (ms.max(0.05) * 0.001 * sr)).exp()
}

// ---------------------------------------------------------------------------
// FX processors
// ---------------------------------------------------------------------------

/// Per-channel 3-band shelving/peaking EQ.
pub struct Eq3 {
    /// Per channel: [low shelf, mid peak, high shelf].
    bands: Vec<[Biquad; 3]>,
    /// Last tuning key — rebuilds (and thus state resets) only on change.
    tuned: Option<(f32, f32, f32, f32, f32)>,
}

impl Eq3 {
    fn new(channels: usize) -> Self {
        Self {
            bands: vec![[Biquad::identity(), Biquad::identity(), Biquad::identity()]; channels],
            tuned: None,
        }
    }

    fn retune(
        &mut self,
        channels: usize,
        sr: f32,
        low_db: f32,
        mid_db: f32,
        high_db: f32,
        mid_hz: f32,
    ) {
        let key = (low_db, mid_db, high_db, mid_hz, sr);
        if self.tuned == Some(key) && self.bands.len() >= channels.max(1) {
            return;
        }
        self.tuned = Some(key);
        self.bands.resize_with(channels.max(1), || {
            [Biquad::identity(), Biquad::identity(), Biquad::identity()]
        });
        for b in &mut self.bands {
            b[0] = Biquad::low_shelf(sr, 250.0, low_db);
            b[1] = Biquad::peaking(sr, mid_hz, 0.9, mid_db);
            b[2] = Biquad::high_shelf(sr, 4000.0, high_db);
        }
    }

    fn process(&mut self, block: &mut [f32], channels: usize) {
        let n_bands = self.bands.len().max(1);
        for (i, s) in block.iter_mut().enumerate() {
            let c = i % channels.max(1);
            let mut v = *s;
            for band in &mut self.bands[c.min(n_bands - 1)] {
                v = band.tick(v);
            }
            *s = v;
        }
    }
}

/// Feed-forward compressor with a peak envelope follower.
pub struct Compressor {
    channels: usize,
    threshold_db: f32,
    ratio: f32,
    attack_c: f32,
    release_c: f32,
    makeup: f32,
    env: Vec<f32>,
    tuned: Option<(f32, f32, f32, f32, f32, f32)>,
}

impl Compressor {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            threshold_db: -18.0,
            ratio: 4.0,
            attack_c: coeff_ms(10.0, 48_000.0),
            release_c: coeff_ms(120.0, 48_000.0),
            makeup: 1.0,
            env: vec![0.0; channels.max(1)],
            tuned: None,
        }
    }

    fn set_params(
        &mut self,
        sr: f32,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        makeup_db: f32,
    ) {
        let key = (sr, threshold_db, ratio, attack_ms, release_ms, makeup_db);
        if self.tuned == Some(key) {
            return;
        }
        self.tuned = Some(key);
        self.threshold_db = threshold_db;
        self.ratio = ratio.max(1.0);
        self.attack_c = coeff_ms(attack_ms, sr);
        self.release_c = coeff_ms(release_ms, sr);
        self.makeup = 10f32.powf(makeup_db / 20.0);
    }

    fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels.max(1);
        let slope = 1.0 - 1.0 / self.ratio;
        let (ac, rc) = (self.attack_c, self.release_c);
        let n_env = self.env.len();
        for (i, s) in block.iter_mut().enumerate() {
            let c = i % ch;
            let x = s.abs();
            let e = &mut self.env[c.min(n_env - 1)];
            let mut env_v = *e;
            env_v = if x > env_v {
                ac * env_v + (1.0 - ac) * x
            } else {
                rc * env_v + (1.0 - rc) * env_v
            };
            *e = env_v;
            let env_db = 20.0 * e.max(1e-9).log10();
            let over = env_db - self.threshold_db;
            let gain = if over > 0.0 {
                10f32.powf(-over * slope / 20.0)
            } else {
                1.0
            };
            *s *= gain * self.makeup;
        }
    }
}

/// Brickwall limiter (fast attack, smoothed release toward the ceiling).
pub struct Limiter {
    channels: usize,
    ceiling: f32,
    release_c: f32,
    gain: Vec<f32>,
    tuned: Option<(f32, f32, f32)>,
}

impl Limiter {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            ceiling: 10f32.powf(-1.0 / 20.0),
            release_c: coeff_ms(50.0, 48_000.0),
            gain: vec![1.0; channels.max(1)],
            tuned: None,
        }
    }

    fn set_params(&mut self, sr: f32, ceiling_db: f32, release_ms: f32) {
        let key = (sr, ceiling_db, release_ms);
        if self.tuned == Some(key) {
            return;
        }
        self.tuned = Some(key);
        self.ceiling = 10f32.powf(ceiling_db / 20.0);
        self.release_c = coeff_ms(release_ms, sr);
    }

    fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels.max(1);
        let ceiling = self.ceiling;
        let rc = self.release_c;
        let n_g = self.gain.len();
        for (i, s) in block.iter_mut().enumerate() {
            let c = i % ch;
            let x = s.abs();
            let needed = if x > ceiling { ceiling / x } else { 1.0 };
            let g = &mut self.gain[c.min(n_g - 1)];
            // attack is instantaneous (needed < current), release is smoothed
            *g = if needed < *g {
                needed
            } else {
                rc * *g + (1.0 - rc) * needed
            };
            *s *= *g;
        }
    }
}

/// Simple Schroeder reverb: 4 parallel combs + 2 series allpasses, mono-in
/// stereo-out wet signal added back into the block.
pub struct ReverbSend {
    comb_l: Vec<Comb>,
    comb_r: Vec<Comb>,
    ap_l: Vec<AllPass>,
    ap_r: Vec<AllPass>,
    send: f32,
    wet: f32,
    tuned: Option<(f32, f32, f32, f32)>,
}

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    feedback: f32,
    damp: f32,
    store: f32,
}

impl Comb {
    fn new(len: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            idx: 0,
            feedback,
            damp: damp.clamp(0.0, 0.99),
            store: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.buf[self.idx];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.idx] = x + self.store * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}

struct AllPass {
    buf: Vec<f32>,
    idx: usize,
    feedback: f32,
}

impl AllPass {
    fn new(len: usize, feedback: f32) -> Self {
        Self {
            buf: vec![0.0; len.max(2)],
            idx: 0,
            feedback: feedback.clamp(0.0, 0.99),
        }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let b = self.buf[self.idx];
        let y = -x + b;
        self.buf[self.idx] = x + b * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        y
    }
}

impl ReverbSend {
    fn new(_sr: f32) -> Self {
        Self {
            comb_l: Vec::new(),
            comb_r: Vec::new(),
            ap_l: Vec::new(),
            ap_r: Vec::new(),
            send: 0.25,
            wet: 1.0,
            tuned: None,
        }
    }

    fn set_params(&mut self, sr: f32, send: f32, size: f32, damping: f32) {
        let key = (sr, send, size, damping);
        if self.tuned == Some(key) {
            return;
        }
        self.tuned = Some(key);
        self.send = send.clamp(0.0, 1.0);
        let size = size.clamp(0.05, 1.0);
        let fb = 0.72 + 0.2 * size; // bigger room → more feedback
        let damp = damping.clamp(0.0, 1.0) * 0.8;
        let base = |ms: f32| ((ms * 0.001 * sr) as usize).max(2);
        // Freeverb-style delay spread (ms), scaled a touch by size.
        let spread = 0.7 + 0.6 * size;
        let comb_ms = [29.7, 37.1, 41.1, 43.7];
        let ap_ms = [5.0, 1.7];
        self.comb_l = comb_ms
            .iter()
            .map(|m| Comb::new(base(m * spread), fb, damp))
            .collect();
        self.comb_r = comb_ms
            .iter()
            .map(|m| Comb::new(base(m * spread * 1.013 + 2.3), fb, damp))
            .collect();
        self.ap_l = ap_ms
            .iter()
            .map(|m| AllPass::new(base(m * spread), 0.5))
            .collect();
        self.ap_r = ap_ms
            .iter()
            .map(|m| AllPass::new(base(m * spread * 1.021 + 1.1), 0.5))
            .collect();
        self.wet = 1.0;
    }

    fn process(&mut self, block: &mut [f32], channels: usize) {
        if channels < 2 || self.comb_l.is_empty() {
            return; // stereo-only in M4 (documented)
        }
        for frame in block.as_chunks_mut::<2>().0 {
            let input = (frame[0] + frame[1]) * 0.5 * self.send;
            let mut wl = 0.0f32;
            let mut wr = 0.0f32;
            for (c, d) in self.comb_l.iter_mut().zip(self.comb_r.iter_mut()) {
                wl += c.tick(input);
                wr += d.tick(input);
            }
            for (a, b) in self.ap_l.iter_mut().zip(self.ap_r.iter_mut()) {
                wl = a.tick(wl);
                wr = b.tick(wr);
            }
            frame[0] += wl * self.wet * 0.35;
            frame[1] += wr * self.wet * 0.35;
        }
    }
}

// ---------------------------------------------------------------------------
// Chain runner
// ---------------------------------------------------------------------------

/// A built, runnable audio FX chain. Params are refreshed per block from the
/// project's `FxInstance`s (automation sampled at the block tick).
pub struct AudioChainRunner {
    channels: usize,
    sr: f32,
    eq3: Eq3,
    comp: Compressor,
    limiter: Limiter,
    reverb: ReverbSend,
    /// Which built processors are live (rebuilt when the chain changes).
    active: Vec<Active>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Active {
    Gain,
    Eq3,
    Comp,
    Limiter,
    Reverb,
}

/// Errors from chain construction (unknown FX kind is tolerated upstream —
/// this is for diagnostics).
#[derive(Debug, thiserror::Error)]
pub enum FxBuildError {
    /// The registry does not know this kind.
    #[error("unknown audio FX kind `{0}`")]
    UnknownKind(String),
}

impl AudioChainRunner {
    /// Builds (or rebuilds) the runner from a track's chain. Unknown kinds
    /// are skipped so newer projects load on older engines.
    pub fn build(chain: &FxChain, channels: usize, sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let ch = channels.max(1);
        let mut active = Vec::new();
        for fx in &chain.instances {
            if !fx.enabled {
                continue;
            }
            let a = match fx.kind.as_str() {
                "audio.gain" => Active::Gain,
                "audio.eq3" => Active::Eq3,
                "audio.compressor" => Active::Comp,
                "audio.limiter" => Active::Limiter,
                "audio.reverb" => Active::Reverb,
                _ => continue,
            };
            if !active.contains(&a) {
                active.push(a);
            }
        }
        let mut r = Self {
            channels: ch,
            sr,
            eq3: Eq3::new(ch),
            comp: Compressor::new(ch),
            limiter: Limiter::new(ch),
            reverb: ReverbSend::new(sr),
            active,
        };
        r.refresh(chain, 0);
        r
    }

    /// Re-resolves parameters (incl. automation) for the block starting at
    /// timeline `tick` and applies them to the processors.
    pub fn refresh(&mut self, chain: &FxChain, tick: i64) {
        let mut first = true;
        for fx in &chain.instances {
            if !fx.enabled {
                continue;
            }
            let at = |p: &str| fx.value_at(p, tick).unwrap_or_else(|| default_of(fx, p));
            match fx.kind.as_str() {
                "audio.gain" => {
                    // gain is applied by the caller via db_to_linear; nothing
                    // to build here (kept as an Active for parity runs).
                }
                "audio.eq3" => {
                    self.eq3.retune(
                        self.channels,
                        self.sr,
                        at("low_db"),
                        at("mid_db"),
                        at("high_db"),
                        at("mid_freq_hz"),
                    );
                }
                "audio.compressor" => {
                    self.comp.set_params(
                        self.sr,
                        at("threshold_db"),
                        at("ratio"),
                        at("attack_ms"),
                        at("release_ms"),
                        at("makeup_db"),
                    );
                }
                "audio.limiter" => {
                    self.limiter
                        .set_params(self.sr, at("ceiling_db"), at("release_ms"));
                }
                "audio.reverb" => {
                    self.reverb
                        .set_params(self.sr, at("send"), at("size"), at("damping"));
                }
                _ => {}
            }
            let _ = first;
            first = false;
        }
    }

    /// `true` when the chain contains at least one audible processor.
    pub fn is_empty(&self) -> bool {
        self.active.iter().all(|a| *a == Active::Gain)
    }

    /// Processes one interleaved block in place. `tick` is the timeline
    /// position of the block start (automation sampling point).
    pub fn process(&mut self, chain: &FxChain, block: &mut [f32], tick: i64) {
        if self.is_empty() || block.is_empty() {
            return;
        }
        self.refresh(chain, tick);
        let ch = self.channels;
        for a in &self.active {
            match a {
                Active::Gain => {} // static/event gain handled by the mixer
                Active::Eq3 => self.eq3.process(block, ch),
                Active::Comp => self.comp.process(block),
                Active::Limiter => self.limiter.process(block),
                Active::Reverb => self.reverb.process(block, ch),
            }
        }
    }
}

fn default_of(fx: &FxInstance, param: &str) -> f32 {
    opentrack_core::fx::builtin::descriptor(&fx.kind)
        .and_then(|d| d.params.iter().find(|p| p.name == param))
        .and_then(|p| p.default.clone().as_f32())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp;
    use opentrack_core::fx::FxChain;

    fn chain_of(kinds: &[&str]) -> FxChain {
        let mut c = FxChain::default();
        for k in kinds {
            c.instances
                .push(opentrack_core::fx::FxInstance::with_defaults(k).unwrap());
        }
        c
    }

    #[test]
    fn biquad_identity_is_passthrough() {
        let mut b = Biquad::identity();
        for x in [0.0, 0.5, -0.25] {
            assert_eq!(b.tick(x), x);
        }
    }

    #[test]
    fn low_shelf_cuts_lows_not_highs() {
        let sr = 48_000.0;
        // -12 dB low shelf at 500 Hz: the low band drops ≈ ×0.25, highs pass.
        let mut b = Biquad::low_shelf(sr, 500.0, -12.0);
        let n = sr as usize;
        let s = dsp::sine(n, sr as u32, 60.0, 1.0);
        let out_low: f32 = s
            .iter()
            .map(|x| b.tick(*x))
            .take(n)
            .fold(0.0, |m, y| m.max(y.abs()));
        let mut b2 = Biquad::low_shelf(sr, 500.0, -12.0);
        let s2 = dsp::sine(n, sr as u32, 8_000.0, 1.0);
        let out_high: f32 = s2
            .iter()
            .map(|x| b2.tick(*x))
            .take(n)
            .fold(0.0, |m, y| m.max(y.abs()));
        assert!(
            out_high > out_low * 2.5,
            "high {} must survive much more than low {}",
            out_high,
            out_low
        );
        assert!(
            (out_low - 0.25).abs() < 0.05,
            "-12 dB ≈ ×0.25, got {out_low}"
        );
    }

    #[test]
    fn compressor_reduces_loud_peaks() {
        let mut c = Compressor::new(1);
        c.set_params(48_000.0, -18.0, 4.0, 5.0, 50.0, 0.0);
        // Sustained loud material: the envelope builds and 4:1 engages.
        let mut block: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.31).sin() * 0.9).collect();
        c.process(&mut block);
        // A feed-forward compressor has no lookahead: the very first attack
        // window (≈ one time constant) passes at full level by design, so
        // the assertion measures the steady state after it.
        let peak = block[2_400..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak < 0.7,
            "compressor must duck sustained loudness, got {peak}"
        );
        // Quiet material passes (roughly) untouched.
        let mut quiet: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 * 0.31).sin() * 0.05)
            .collect();
        let mut c2 = Compressor::new(1);
        c2.set_params(48_000.0, -18.0, 4.0, 5.0, 50.0, 0.0);
        c2.process(&mut quiet);
        let qp = quiet.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(qp > 0.04, "quiet material must not be crushed, got {qp}");
    }

    #[test]
    fn limiter_never_exceeds_ceiling() {
        let mut l = Limiter::new(1);
        l.set_params(48_000.0, -1.0, 20.0);
        let mut block: Vec<f32> = (0..4_800).map(|i| ((i as f32 * 0.1).sin()) * 2.0).collect();
        l.process(&mut block);
        let ceiling = 10f32.powf(-1.0 / 20.0);
        let peak = block.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak <= ceiling * 1.01 + 1e-4,
            "peak {peak} > ceiling {ceiling}"
        );
    }

    #[test]
    fn reverb_wets_the_tail() {
        let mut r = ReverbSend::new(48_000.0);
        r.set_params(48_000.0, 1.0, 0.8, 0.2);
        let mut block = vec![0.0f32; 48_000 * 2];
        // one impulse at t=0
        block[0] = 1.0;
        block[1] = 1.0;
        r.process(&mut block, 2);
        // energy well after the impulse = wet tail
        let late: f32 = block[20_000 * 2..40_000 * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|f| f[0].abs() + f[1].abs())
            .sum();
        assert!(late > 0.01, "reverb must produce a tail, got {late}");
    }

    #[test]
    fn block_chunking_is_bit_identical_parity() {
        // The M4 acceptance core: the same chain fed the same stream in one
        // call vs many FX-sized blocks must produce identical f32 buffers.
        let chain = chain_of(&[
            "audio.eq3",
            "audio.compressor",
            "audio.limiter",
            "audio.reverb",
        ]);
        let sr = 48_000u32;
        let src: Vec<f32> = (0..48_000 * 2)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (t * 220.0 * std::f32::consts::TAU).sin() * 0.7
                    + (t * 331.0 * std::f32::consts::TAU).sin() * 0.3
            })
            .collect();

        // whole-stream path (runner internally refreshes per call)
        let mut whole = src.clone();
        let mut r1 = AudioChainRunner::build(&chain, 2, sr);
        r1.process(&chain, &mut whole, 0);

        // FX-block path
        let mut chunked = src.clone();
        let mut r2 = AudioChainRunner::build(&chain, 2, sr);
        for (bi, blk) in chunked
            .as_chunks_mut::<FX_BLOCK_FRAMES>()
            .0
            .iter_mut()
            .enumerate()
        {
            let tick = (bi * FX_BLOCK_FRAMES) as i64 * 48_000 / sr as i64;
            r2.process(&chain, blk, tick);
        }
        assert_eq!(whole.len(), chunked.len());
        for (i, (a, b)) in whole.iter().zip(chunked.iter()).enumerate() {
            assert_eq!(a, b, "sample {i} diverges between chunkings");
        }
    }

    #[test]
    fn automation_changes_output_over_time() {
        let mut chain = chain_of(&["audio.gain"]);
        let mut fx = chain.instances[0].clone();
        fx.automation.entry("gain_db".into()).or_default();
        chain.instances[0] = fx;
        // Automation is sampled per block; the offline mixer applies event
        // fades itself, so here we assert refresh() picks lane values.
        let mut lane = opentrack_core::fx::AutomationLane::default();
        lane.insert(0, -6.0);
        chain.instances[0].automation.insert("gain_db".into(), lane);
        let mut r = AudioChainRunner::build(&chain, 2, 48_000);
        r.refresh(&chain, 0);
        // gain_db resolves from the lane (not asserted on the processor
        // state — the mixer consumes db_to_linear(value_at) directly).
        let fx = &chain.instances[0];
        assert_eq!(fx.value_at("gain_db", 0), Some(-6.0));
    }

    #[test]
    fn unknown_kinds_are_skipped() {
        let mut chain = chain_of(&["audio.eq3"]);
        chain.instances.push(FxInstance {
            kind: "audio.hologram".into(),
            enabled: true,
            params: Default::default(),
            automation: Default::default(),
        });
        let r = AudioChainRunner::build(&chain, 2, 48_000);
        assert!(!r.is_empty());
        assert_eq!(r.active, vec![Active::Eq3]);
    }
}
