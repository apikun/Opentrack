//! Small DSP helpers: tone synthesis, linear resampling, peaks.

/// Renders a mono sine tone (the M0 demo content generator).
pub fn sine(frames: usize, sample_rate: u32, freq_hz: f64, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            (t * freq_hz * std::f64::consts::TAU).sin() as f32 * amplitude
        })
        .collect()
}

/// Naive linear-interpolation resampler for interleaved audio.
///
/// Good enough for the offline M0 mixer; the realtime engine (M2) will move
/// to a windowed-sinc polyphase filter (RFC-0004 §6).
pub fn resample_linear(interleaved: &[f32], channels: u16, from: u32, to: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let in_frames = interleaved.len() / ch;
    if from == to || from == 0 || to == 0 || in_frames == 0 {
        return interleaved.to_vec();
    }
    let out_frames = ((in_frames as f64) * to as f64 / from as f64).floor() as usize;
    let mut out = Vec::with_capacity(out_frames * ch);
    let step = from as f64 / to as f64;
    for o in 0..out_frames {
        let pos = o as f64 * step;
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let i1 = (i0 + 1).min(in_frames - 1);
        for c in 0..ch {
            let a = interleaved[i0 * ch + c];
            let b = interleaved[i1 * ch + c];
            out.push(a + (b - a) * frac);
        }
    }
    out
}

/// Per-channel peak (max absolute sample).
pub fn peaks(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mut peaks = vec![0f32; ch];
    for (i, &s) in interleaved.iter().enumerate() {
        let a = s.abs();
        if a > peaks[i % ch] {
            peaks[i % ch] = a;
        }
    }
    peaks
}

/// Stateful linear resampler for *streaming* decode (realtime engine, M2).
///
/// The offline mixer resamples whole buffers in one shot
/// ([`resample_linear`]); a realtime feeder receives chunks of unknown
/// boundaries, so the interpolation window must carry over between calls.
/// Internally the resampler remembers the *last input frame* of the previous
/// chunk and keeps its absolute stream position, so output values match
/// `resample_linear` over the same concatenated stream (±1 tail frame, which
/// the offline variant drops through floor truncation).
///
/// Chunks must be whole frames (`input.len() % channels == 0`). Feed
/// source-rate chunks to [`Self::process`]; call [`Self::flush`] once the
/// source hit EOF to drain the tail (held last frame, like offline).
pub struct StreamResampler {
    channels: usize,
    step: f64,
    /// Absolute input-frame position of the next output sample.
    next_pos: f64,
    /// Absolute stream index of the carried frame (`-1` = none yet).
    base: i64,
    /// Last input frame of the previous chunk (channel-interleaved).
    carry: Vec<f32>,
    /// Set by [`Self::flush`]; `process` becomes a no-op afterwards.
    drained: bool,
}

impl StreamResampler {
    /// Builds a resampler; passthrough when `from == to`.
    pub fn new(channels: u16, from: u32, to: u32) -> Self {
        let (from, to) = (from.max(1), to.max(1));
        Self {
            channels: channels.max(1) as usize,
            step: from as f64 / to as f64,
            next_pos: 0.0,
            base: -1,
            carry: vec![0.0; channels.max(1) as usize],
            drained: false,
        }
    }

    /// Whether rates are identical (`process` then just copies).
    pub fn is_passthrough(&self) -> bool {
        (self.step - 1.0).abs() < f64::EPSILON
    }

    /// Resamples one interleaved chunk; appends output-rate frames to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        if self.drained {
            return;
        }
        if self.is_passthrough() {
            out.extend_from_slice(input);
            return;
        }
        let in_frames = input.len() / ch;
        if in_frames == 0 {
            return;
        }
        // `self.base` is the absolute index of `carry`; input frame j sits at
        // absolute index base + 1 + j. An output at `p` interpolates frames
        // i0 = floor(p) and i1 = i0 + 1; produce while i1 is available.
        let limit = self.base + in_frames as i64;
        while self.next_pos < limit as f64 {
            let i0 = self.next_pos.floor() as i64;
            let frac = (self.next_pos - i0 as f64) as f32;
            let i1 = i0 + 1; // ≤ limit ⇒ in bounds
            for c in 0..ch {
                let a = if i0 == self.base {
                    self.carry[c]
                } else {
                    input[((i0 - self.base - 1) as usize) * ch + c]
                };
                let b = input[((i1 - self.base - 1) as usize) * ch + c];
                out.push(a + (b - a) * frac);
            }
            self.next_pos += self.step;
        }
        self.carry.clear();
        self.carry.extend_from_slice(&input[(in_frames - 1) * ch..]);
        self.base += in_frames as i64;
    }

    /// Drains the tail after EOF (call once; afterwards `process` is a no-op).
    /// Mirrors the offline resampler's end-of-stream hold.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.drained {
            return;
        }
        self.drained = true;
        if self.is_passthrough() {
            return;
        }
        // Offline produces floor(N * to / from) frames; the streaming tail
        // holds the last carried frame while its position is below N - 1 + 1.
        while self.next_pos < (self.base + 1) as f64 {
            out.extend_from_slice(&self.carry);
            self.next_pos += self.step;
        }
    }
}

#[cfg(test)]
mod stream_resampler_tests {
    use super::*;

    #[test]
    fn passthrough_copies() {
        let mut r = StreamResampler::new(2, 48_000, 48_000);
        let x = vec![0.1, 0.2, 0.3, 0.4];
        let mut out = Vec::new();
        r.process(&x, &mut out);
        r.flush(&mut out);
        assert_eq!(out, x);
    }

    #[test]
    fn upsample_matches_offline_reference() {
        // Ramp of 4 stereo frames, offline reference via resample_linear.
        let x: Vec<f32> = (0..4).flat_map(|i| [i as f32, -i as f32]).collect();
        let mut r = StreamResampler::new(2, 48_000, 96_000);
        let mut out = Vec::new();
        // Frame-aligned awkward chunks (3 frames, then 1) prove state carries.
        assert_eq!(x.len() % 2, 0);
        r.process(&x[..6], &mut out);
        r.process(&x[6..], &mut out);
        r.flush(&mut out);
        let reference = resample_linear(&x, 2, 48_000, 96_000);
        assert!(
            (out.len() as i64 - reference.len() as i64).abs() <= 1,
            "stream {} vs offline {}",
            out.len(),
            reference.len()
        );
        let n = out.len().min(reference.len());
        for i in 0..n {
            let a = out[i];
            let b = reference[i];
            assert!((a - b).abs() < 1e-5, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn single_chunk_matches_offline_exactly() {
        let x: Vec<f32> = (0..100).map(|i| (i as f32) * 0.01).collect();
        let mut r = StreamResampler::new(1, 96_000, 48_000);
        let mut out = Vec::new();
        r.process(&x, &mut out);
        r.flush(&mut out);
        let reference = resample_linear(&x, 1, 96_000, 48_000);
        assert_eq!(out.len(), reference.len());
        for (a, b) in out.iter().zip(reference.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn repeated_small_chunks_stay_finite() {
        let mut r = StreamResampler::new(2, 44_100, 48_000);
        let mut out = Vec::new();
        for k in 0..97 {
            let mut chunk = vec![k as f32 * 0.01; 6]; // 3 stereo frames
            chunk[0] = k as f32; // vary content
            r.process(&chunk, &mut out);
        }
        r.flush(&mut out);
        assert!(out.iter().all(|s| s.is_finite()));
        // Within a frame of the offline count (out is stereo samples).
        let expect_frames = (291.0f64 * 48_000.0 / 44_100.0).floor() as i64;
        let got_frames = out.len() as i64 / 2;
        assert!(
            (got_frames - expect_frames).abs() <= 1,
            "streamed {got_frames} frames vs offline {expect_frames}"
        );
    }

    #[test]
    fn process_after_flush_is_noop() {
        let mut r = StreamResampler::new(1, 48_000, 96_000);
        let mut out = Vec::new();
        r.process(&[1.0, 2.0, 3.0], &mut out);
        r.flush(&mut out);
        let len = out.len();
        r.process(&[4.0, 5.0], &mut out);
        r.flush(&mut out);
        assert_eq!(out.len(), len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_frequency_counts_zero_crossings() {
        let sr = 48_000;
        let n = sr as usize; // 1 second
        let s = sine(n, sr, 440.0, 1.0);
        let crossings = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert_eq!(crossings, 440);
    }

    #[test]
    fn resample_passthrough_and_upsample() {
        // 2 stereo frames ramping 0 -> 1 on both channels
        let x = vec![0.0, 0.0, 1.0, 1.0];
        assert_eq!(resample_linear(&x, 2, 48_000, 48_000), x);
        let y = resample_linear(&x, 2, 48_000, 96_000);
        assert_eq!(y.len(), 8); // 4 frames stereo
        // frame 0 = source frame 0; frame 1 = midpoint; frame 3 = source frame 1
        assert_eq!(&y[0..2], &[0.0, 0.0]);
        assert!((y[2] - 0.5).abs() < 1e-6 && (y[3] - 0.5).abs() < 1e-6);
        assert!((y[6] - 1.0).abs() < 1e-6 && (y[7] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn peaks_finds_channel_maxima() {
        let x = vec![0.1, -0.9, 0.3, 0.5];
        assert_eq!(peaks(&x, 2), vec![0.3, 0.9]);
    }
}
