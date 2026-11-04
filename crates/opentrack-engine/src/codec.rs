//! The codec abstraction: one trait for every decode/probe path
//! (RFC-0003). The pure-Rust backend is always present; the ffmpeg bridge is
//! optional and picked up at runtime.

use std::path::Path;
use std::sync::Arc;

/// Stream/audio format descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioSpec {
    /// Channel count of the decoded stream.
    pub channels: u16,
    /// Native sample rate in Hz.
    pub sample_rate: u32,
    /// Total frames when known.
    pub frames_total: Option<u64>,
}

/// Classified stream metadata.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamKind {
    /// Audio stream.
    Audio {
        /// Channels.
        channels: u16,
        /// Sample rate.
        sample_rate: u32,
    },
    /// Video stream.
    Video {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
        /// FPS when parseable.
        fps: Option<f64>,
    },
    /// Subtitles, data, attachments...
    Other,
}

/// One probed stream.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamInfo {
    /// Classified kind.
    pub kind: StreamKind,
    /// Codec name as reported by the backend ("pcm_s16le", "h264", ...).
    pub codec: String,
}

/// Probe result for a media file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    /// Duration when known.
    pub duration_secs: Option<f64>,
    /// All streams found.
    pub streams: Vec<StreamInfo>,
}

impl MediaInfo {
    /// First audio stream, if any.
    pub fn first_audio(&self) -> Option<(u16, u32)> {
        self.streams.iter().find_map(|s| match &s.kind {
            StreamKind::Audio {
                channels,
                sample_rate,
            } => Some((*channels, *sample_rate)),
            _ => None,
        })
    }
}

/// Codec/backend failures.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// The backend cannot handle this content.
    #[error("unsupported media: {0}")]
    Unsupported(String),
    /// Decode/probe failed mid-stream.
    #[error("decode failed: {0}")]
    Decode(String),
    /// Filesystem/pipe failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// An external tool the bridge needs is missing.
    #[error("tool `{tool}` not found ({hint})")]
    ToolMissing {
        /// Tool binary name.
        tool: String,
        /// What to do about it.
        hint: String,
    },
}

/// A pull-based audio decoder yielding interleaved `f32` frames.
pub trait AudioSource: Send {
    /// Format of the stream being decoded.
    fn spec(&self) -> AudioSpec;

    /// Reads up to `dst.len()` interleaved samples, returning how many
    /// *frames* were read (`0` = end of stream).
    fn read(&mut self, dst: &mut [f32]) -> Result<usize, CodecError>;

    /// Discards `n` frames (default: read and drop). Backends with real
    /// seeking override this.
    fn skip_frames(&mut self, n: u64) -> Result<(), CodecError> {
        let ch = self.spec().channels.max(1) as usize;
        let mut scratch = vec![0f32; 8192 * ch];
        let mut left = n;
        while left > 0 {
            let want = scratch.len().min((left as usize) * ch);
            let frames = self.read(&mut scratch[..want])?;
            if frames == 0 {
                break;
            }
            left -= frames as u64;
        }
        Ok(())
    }
}

/// A media decode backend (pure Rust, ffmpeg bridge, future: native
/// decoders). Backends must be usable from any thread.
pub trait CodecBackend: Send + Sync {
    /// Backend display name.
    fn name(&self) -> &'static str;

    /// Whether this backend claims files with this extension.
    fn supports(&self, path: &Path) -> bool;

    /// Probes a file without decoding.
    fn probe(&self, path: &Path) -> Result<MediaInfo, CodecError>;

    /// Opens an interleaved-f32 audio decoder.
    fn open_audio(&self, path: &Path) -> Result<Box<dyn AudioSource>, CodecError>;
}

/// The default backend chain: pure Rust first, ffmpeg bridge as fallback.
pub fn default_backends() -> Vec<Arc<dyn CodecBackend>> {
    vec![
        Arc::new(PureRustBackend::new()),
        crate::backend_ffmpeg::FfmpegBridge::auto(),
    ]
}

/// Tries every backend in order to open an audio decoder.
pub fn open_audio_any(
    backends: &[Arc<dyn CodecBackend>],
    path: &Path,
) -> Result<(Box<dyn AudioSource>, &'static str), CodecError> {
    let mut last = CodecError::Unsupported("no backend offered".into());
    for b in backends {
        if !b.supports(path) {
            continue;
        }
        match b.open_audio(path) {
            Ok(src) => return Ok((src, b.name())),
            Err(e) => last = e,
        }
    }
    Err(last)
}

// ---------------------------------------------------------------------------
// Pure-Rust backend: WAV in memory
// ---------------------------------------------------------------------------

/// The dependency-free backend. M0 handles WAV (PCM/float); PNG/JPEG stills
/// and MJPEG follow in M2/M3 — see the parity matrix.
pub struct PureRustBackend;

impl PureRustBackend {
    /// Creates the backend.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PureRustBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// In-memory [`AudioSource`] over a fully decoded buffer.
pub struct InMemorySource {
    spec: AudioSpec,
    data: Vec<f32>,
    pos: usize,
}

impl InMemorySource {
    /// Wraps interleaved samples as a source.
    pub fn new(channels: u16, sample_rate: u32, data: Vec<f32>) -> Self {
        Self {
            spec: AudioSpec {
                channels,
                sample_rate,
                frames_total: if channels > 0 {
                    Some((data.len() / channels as usize) as u64)
                } else {
                    None
                },
            },
            data,
            pos: 0,
        }
    }
}

impl AudioSource for InMemorySource {
    fn spec(&self) -> AudioSpec {
        self.spec.clone()
    }

    fn read(&mut self, dst: &mut [f32]) -> Result<usize, CodecError> {
        let left = self.data.len() - self.pos;
        let n = left.min(dst.len());
        dst[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        let frames = if self.spec.channels > 0 {
            n / self.spec.channels as usize
        } else {
            0
        };
        Ok(frames)
    }
}

impl CodecBackend for PureRustBackend {
    fn name(&self) -> &'static str {
        "pure-rust"
    }

    fn supports(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false)
    }

    fn probe(&self, path: &Path) -> Result<MediaInfo, CodecError> {
        let info = crate::wav::read_header(path).map_err(|e| CodecError::Decode(e.to_string()))?;
        Ok(MediaInfo {
            duration_secs: Some(info.duration_secs),
            streams: vec![StreamInfo {
                kind: StreamKind::Audio {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                },
                codec: format!("pcm_s{}le", info.bits),
            }],
        })
    }

    fn open_audio(&self, path: &Path) -> Result<Box<dyn AudioSource>, CodecError> {
        let (info, data) =
            crate::wav::decode_file(path).map_err(|e| CodecError::Decode(e.to_string()))?;
        Ok(Box::new(InMemorySource::new(
            info.channels,
            info.sample_rate,
            data,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_source_reads_and_eof() {
        let mut src = InMemorySource::new(2, 48_000, vec![0.5; 8]);
        assert_eq!(src.spec().frames_total, Some(4));
        let mut buf = [0f32; 6];
        assert_eq!(src.read(&mut buf).unwrap(), 3);
        assert_eq!(src.read(&mut buf).unwrap(), 1);
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }
}
