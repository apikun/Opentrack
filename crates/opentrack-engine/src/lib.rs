//! # opentrack-engine
//!
//! OpenTrack's media engine: the [`codec`] backend abstraction (pure-Rust
//! default + optional ffmpeg bridge, RFC-0003), a real pure-Rust WAV codec,
//! pure-Rust MP4/H.264 video decode for preview ([`video`], RFC-0007
//! amendment), DSP helpers, and the offline audio mixer that turns a project
//! timeline into a rendered master (RFC-0004).
//!
//! Shipped: audio-only offline render (M0) + video preview decode (M1).
//! The video compositor and export pipeline are specced in RFC-0007 (M3).

pub mod backend_ffmpeg;
pub mod codec;
pub mod dsp;
pub mod mixer;
pub mod render;
pub mod video;
pub mod wav;

pub use backend_ffmpeg::FfmpegBridge;
pub use codec::{
    AudioSource, AudioSpec, CodecBackend, CodecError, InMemorySource, MediaInfo, PureRustBackend,
    StreamInfo, StreamKind, default_backends, open_audio_any,
};
pub use mixer::{MixIssue, RenderSettings, RenderedAudio, render_audio};
pub use render::{RenderError, RenderJob};
pub use video::{RgbaFrame, VideoDecoder, VideoError, VideoInfo, probe_video, write_bmp};
