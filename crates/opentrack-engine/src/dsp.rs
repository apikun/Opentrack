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
