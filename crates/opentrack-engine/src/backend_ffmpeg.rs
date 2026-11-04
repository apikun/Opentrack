//! Optional ffmpeg/ffprobe bridge.
//!
//! The application code stays pure Rust; when a system `ffmpeg` is present,
//! this backend extends decode coverage to everything ffmpeg supports
//! (H.264/H.265/VP9/AAC/...). See RFC-0003 for the security and portability
//! trade-offs of shelling out.

use super::codec::{
    AudioSource, AudioSpec, CodecBackend, CodecError, MediaInfo, StreamInfo, StreamKind,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;

/// Bridges decode through system ffmpeg/ffprobe binaries.
#[derive(Clone, Debug)]
pub struct FfmpegBridge {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl FfmpegBridge {
    /// Bridge that resolves `ffmpeg`/`ffprobe` from `PATH`.
    pub fn auto() -> Arc<dyn CodecBackend> {
        Arc::new(Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        })
    }

    /// Bridge with explicit binary paths (for bundled installs).
    pub fn with_paths(ffmpeg: PathBuf, ffprobe: PathBuf) -> Self {
        Self { ffmpeg, ffprobe }
    }

    fn tool_ok(&self, tool: &Path, hint: &str) -> Result<(), CodecError> {
        let mut cmd = Self::command(tool);
        cmd.arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.output() {
            Ok(o) if o.status.success() => Ok(()),
            _ => Err(CodecError::ToolMissing {
                tool: tool.display().to_string(),
                hint: hint.to_string(),
            }),
        }
    }

    /// Spawns a child without flashing a console window on Windows.
    fn command(tool: &Path) -> Command {
        let mut cmd = Command::new(tool);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW
            cmd.creation_flags(0x0800_0000);
        }
        let _ = &mut cmd; // `mut` is required on Windows (creation_flags takes &mut)
        cmd
    }

    fn probe_raw(&self, path: &Path) -> Result<serde_json::Value, CodecError> {
        self.tool_ok(
            &self.ffprobe,
            "install ffmpeg/ffprobe or rely on the pure-Rust backend",
        )?;
        let out = Self::command(&self.ffprobe)
            .args([
                "-v",
                "error",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        if !out.status.success() {
            return Err(CodecError::Decode(format!(
                "ffprobe exited with {}",
                out.status
            )));
        }
        serde_json::from_slice(&out.stdout)
            .map_err(|e| CodecError::Decode(format!("ffprobe JSON: {e}")))
    }
}

fn json_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

fn json_u32(v: &serde_json::Value) -> Option<u32> {
    v.as_u64()
        .map(|x| x as u32)
        .or_else(|| v.as_str()?.parse().ok())
}

/// Parses ffprobe ratio strings like `30000/1001`.
fn parse_ratio(s: &str) -> Option<f64> {
    let mut it = s.splitn(2, '/');
    let a: f64 = it.next()?.trim().parse().ok()?;
    let b: f64 = it
        .next()
        .map(|x| x.trim().parse().unwrap_or(1.0))
        .unwrap_or(1.0);
    if b == 0.0 { None } else { Some(a / b) }
}

impl CodecBackend for FfmpegBridge {
    fn name(&self) -> &'static str {
        "ffmpeg-bridge"
    }

    fn supports(&self, _path: &Path) -> bool {
        true // claims everything; open_audio fails gracefully when unusable
    }

    fn probe(&self, path: &Path) -> Result<MediaInfo, CodecError> {
        let v = self.probe_raw(path)?;
        let duration = v
            .pointer("/format/duration")
            .and_then(json_f64)
            .or_else(|| {
                v.pointer("/streams")
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.get("duration").and_then(json_f64))
                            .fold(f64::NAN, f64::max)
                    })
                    .filter(|d| !d.is_nan())
            });
        let mut streams = Vec::new();
        if let Some(arr) = v.pointer("/streams").and_then(|s| s.as_array()) {
            for s in arr {
                let codec = s
                    .get("codec_name")
                    .and_then(|c| c.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                match s.get("codec_type").and_then(|t| t.as_str()) {
                    Some("audio") => streams.push(StreamInfo {
                        kind: StreamKind::Audio {
                            channels: s.get("channels").and_then(json_u32).unwrap_or(2) as u16,
                            sample_rate: s.get("sample_rate").and_then(json_u32).unwrap_or(48_000),
                        },
                        codec,
                    }),
                    Some("video") => streams.push(StreamInfo {
                        kind: StreamKind::Video {
                            width: s.get("width").and_then(json_u32).unwrap_or(0),
                            height: s.get("height").and_then(json_u32).unwrap_or(0),
                            fps: s
                                .get("avg_frame_rate")
                                .and_then(|r| r.as_str())
                                .and_then(parse_ratio),
                        },
                        codec,
                    }),
                    _ => streams.push(StreamInfo {
                        kind: StreamKind::Other,
                        codec,
                    }),
                }
            }
        }
        Ok(MediaInfo {
            duration_secs: duration,
            streams,
        })
    }

    fn open_audio(&self, path: &Path) -> Result<Box<dyn AudioSource>, CodecError> {
        let info = self.probe(path)?;
        let (channels, sample_rate) = info.first_audio().unwrap_or((2, 48_000));
        self.tool_ok(
            &self.ffmpeg,
            "install ffmpeg, or let the pure-Rust backend handle WAV",
        )?;
        let mut child = Self::command(&self.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(path)
            .args([
                "-map",
                "0:a:0",
                "-vn",
                "-f",
                "f32le",
                "-acodec",
                "pcm_f32le",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| CodecError::Decode(format!("spawn ffmpeg: {e}")))?;
        let out = child
            .stdout
            .take()
            .ok_or_else(|| CodecError::Decode("ffmpeg stdout unavailable".into()))?;
        Ok(Box::new(FfmpegAudioSource {
            child: Some(child),
            out,
            spec: AudioSpec {
                channels,
                sample_rate,
                frames_total: None,
            },
            pending: Vec::new(),
            bytes_per_sample: 4,
        }))
    }
}

/// Streaming decoder over ffmpeg's raw `f32le` stdout pipe.
pub struct FfmpegAudioSource {
    child: Option<Child>,
    out: ChildStdout,
    spec: AudioSpec,
    pending: Vec<u8>,
    bytes_per_sample: usize,
}

impl AudioSource for FfmpegAudioSource {
    fn spec(&self) -> AudioSpec {
        self.spec.clone()
    }

    fn read(&mut self, dst: &mut [f32]) -> Result<usize, CodecError> {
        let need = dst.len() * self.bytes_per_sample;
        let mut bytes = std::mem::take(&mut self.pending);
        bytes.reserve(need);
        let mut chunk = [0u8; 16 * 1024];
        while bytes.len() < need {
            let n = self.out.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
        }
        let usable = bytes.len() - bytes.len() % self.bytes_per_sample;
        self.pending = bytes.split_off(usable); // keep partial sample for next call
        bytes.truncate(usable);
        let got = usable / self.bytes_per_sample;
        for (i, s) in bytes.chunks_exact(self.bytes_per_sample).enumerate() {
            dst[i] = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        }
        let ch = self.spec.channels.max(1) as usize;
        Ok(got / ch)
    }
}

impl Drop for FfmpegAudioSource {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_parsing() {
        assert!((parse_ratio("30000/1001").unwrap() - 29.970_029_97).abs() < 1e-6);
        assert!((parse_ratio("30").unwrap() - 30.0).abs() < 1e-9);
        assert!(parse_ratio("0/0").is_none());
    }

    #[test]
    fn bridge_missing_tool_is_clean_error() {
        let bridge = FfmpegBridge::with_paths(
            PathBuf::from("/nonexistent/ffmpeg"),
            PathBuf::from("/nonexistent/ffprobe"),
        );
        let err = bridge
            .probe(Path::new("/nonexistent/file.wav"))
            .unwrap_err();
        assert!(matches!(err, CodecError::ToolMissing { .. }));
    }
}
