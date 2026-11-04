//! A complete, dependency-free WAV (RIFF) reader and writer.
//!
//! Supports PCM 8/16/24/32-bit and IEEE float 32/64, plus
//! `WAVE_FORMAT_EXTENSIBLE` containers. This is the fallback decode path of
//! the pure-Rust backend and the output path of the offline renderer.

use std::path::Path;

/// WAV metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct WavInfo {
    /// Channel count.
    pub channels: u16,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Bits per sample.
    pub bits: u16,
    /// WAVE format tag (1 = PCM, 3 = IEEE float).
    pub format_tag: u16,
    /// Total frames (samples per channel).
    pub frames: u64,
    /// Duration in seconds.
    pub duration_secs: f64,
}

/// WAV codec errors.
#[derive(Debug, thiserror::Error)]
pub enum WavError {
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Not a RIFF/WAVE file.
    #[error("not a WAV file (bad RIFF/WAVE header)")]
    NotWav,
    /// Header present but codec/format unsupported.
    #[error("unsupported WAV format: {0}")]
    Unsupported(String),
    /// File ends mid-chunk or mid-frame.
    #[error("truncated WAV data")]
    Truncated,
}

const FMT_PCM: u16 = 1;
const FMT_FLOAT: u16 = 3;
const FMT_EXTENSIBLE: u16 = 0xFFFE;

fn u16le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Parses chunk bounds of a WAVE file body. Returns `(fmt bytes, data range)`.
fn parse_chunks(buf: &[u8]) -> Result<(Option<&[u8]>, std::ops::Range<usize>), WavError> {
    if buf.len() < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut fmt = None;
    let mut data: Option<std::ops::Range<usize>> = None;
    let mut pos = 12usize;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size = u32le(buf, pos + 4) as usize;
        let start = pos + 8;
        let end = start.checked_add(size).ok_or(WavError::Truncated)?;
        if end > buf.len() {
            return Err(WavError::Truncated);
        }
        match id {
            b"fmt " => fmt = Some(&buf[start..end]),
            b"data" if data.is_none() => data = Some(start..end),
            _ => {}
        }
        pos = end + (size & 1); // chunks are word-aligned
    }
    let fmt = fmt.ok_or_else(|| WavError::Unsupported("missing fmt chunk".into()))?;
    let data = data.ok_or_else(|| WavError::Unsupported("missing data chunk".into()))?;
    Ok((Some(fmt), data))
}

fn effective_tag(fmt: &[u8]) -> Result<u16, WavError> {
    let tag = u16le(fmt, 0);
    if tag != FMT_EXTENSIBLE {
        return Ok(tag);
    }
    // WAVE_FORMAT_EXTENSIBLE: cbSize >= 22 and SubFormat GUID at offset 24.
    if fmt.len() < 40 || u16le(fmt, 16) < 22 {
        return Err(WavError::Unsupported("bad extensible header".into()));
    }
    Ok(u16le(fmt, 24))
}

fn info_from(fmt: &[u8], data_len: usize) -> Result<WavInfo, WavError> {
    if fmt.len() < 16 {
        return Err(WavError::Unsupported("short fmt chunk".into()));
    }
    let tag = effective_tag(fmt)?;
    let channels = u16le(fmt, 2);
    let sample_rate = u32le(fmt, 4);
    let bits = u16le(fmt, 14);
    if channels == 0 {
        return Err(WavError::Unsupported("zero channels".into()));
    }
    let bytes_per_frame = (bits as usize / 8) * channels as usize;
    if bytes_per_frame == 0 {
        return Err(WavError::Unsupported("zero bits per sample".into()));
    }
    let frames = data_len / bytes_per_frame;
    Ok(WavInfo {
        channels,
        sample_rate,
        bits,
        format_tag: tag,
        frames: frames as u64,
        duration_secs: if sample_rate == 0 {
            0.0
        } else {
            frames as f64 / sample_rate as f64
        },
    })
}

/// Reads only the header (cheap probe path).
pub fn read_header(path: &Path) -> Result<WavInfo, WavError> {
    let buf = std::fs::read(path)?;
    let (fmt, data) = parse_chunks(&buf)?;
    info_from(fmt.expect("parse_chunks returns fmt"), data.len())
}

/// Decodes the whole file to interleaved `f32` in `[-1, 1]`.
pub fn decode_file(path: &Path) -> Result<(WavInfo, Vec<f32>), WavError> {
    let buf = std::fs::read(path)?;
    let (fmt, data) = parse_chunks(&buf)?;
    let fmt = fmt.expect("parse_chunks returns fmt");
    let info = info_from(fmt, data.len())?;
    let tag = effective_tag(fmt)?;
    let bytes = &buf[data];
    let bps = info.bits as usize / 8;
    let frame_bytes = bps * info.channels as usize;
    let mut out = Vec::with_capacity(info.frames as usize * info.channels as usize);

    match (tag, info.bits) {
        (FMT_PCM, 8) => {
            for s in bytes {
                out.push((*s as f32 - 128.0) / 128.0);
            }
        }
        (FMT_PCM, 16) => {
            for c in bytes.chunks_exact(2) {
                out.push(i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0);
            }
        }
        (FMT_PCM, 24) => {
            for c in bytes.chunks_exact(3) {
                let v = ((c[2] as i32) << 24 | (c[1] as i32) << 16 | (c[0] as i32) << 8) >> 8;
                out.push(v as f32 / 8388608.0);
            }
        }
        (FMT_PCM, 32) => {
            for c in bytes.chunks_exact(4) {
                out.push(i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2147483648.0);
            }
        }
        (FMT_FLOAT, 32) => {
            for c in bytes.chunks_exact(4) {
                out.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
        }
        (FMT_FLOAT, 64) => {
            for c in bytes.chunks_exact(8) {
                out.push(
                    f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32,
                );
            }
        }
        _ => {
            return Err(WavError::Unsupported(format!(
                "format tag {tag} at {} bits",
                info.bits
            )));
        }
    }
    let _ = frame_bytes;
    Ok((info, out))
}

/// Encodes interleaved `f32` samples as 16-bit PCM WAV (the M0 render
/// master format).
pub fn encode_s16(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), WavError> {
    if channels == 0 {
        return Err(WavError::Unsupported("zero channels".into()));
    }
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let data_len = pcm.len() as u32;
    let mut out = Vec::with_capacity(pcm.len() + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&FMT_PCM.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&(channels * 2).to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&pcm);
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("opentrack-wav-{}-{}.wav", tag, std::process::id()));
        p
    }

    #[test]
    fn roundtrip_s16() {
        let path = tmp("rt");
        let src: Vec<f32> = (0..480)
            .map(|i| ((i as f64 / 480.0) * std::f64::consts::TAU * 3.0).sin() as f32 * 0.5)
            .collect();
        encode_s16(&path, 2, 48_000, &src).unwrap();
        let (info, data) = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.bits, 16);
        assert_eq!(info.format_tag, FMT_PCM);
        assert_eq!(info.frames, 240);
        assert!((info.duration_secs - 0.005).abs() < 1e-9);
        assert_eq!(data.len(), 480);
        // quantization: within 1/32767 of source
        for (a, b) in src.iter().zip(&data) {
            assert!((a - b).abs() < 4.0 / 32768.0);
        }
    }

    #[test]
    fn header_only_probe_matches_decode() {
        let path = tmp("hdr");
        encode_s16(&path, 1, 44_100, &[0.25; 441]).unwrap();
        let h = read_header(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(h.frames, 441);
        assert_eq!(h.channels, 1);
        assert!((h.duration_secs - 0.01).abs() < 1e-9);
    }

    #[test]
    fn garbage_is_rejected() {
        let path = tmp("bad");
        std::fs::write(&path, b"NOTAWAV....").unwrap();
        let err = decode_file(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, WavError::NotWav));
    }
}
