//! # opentrack-engine
//!
//! OpenTrack's media engine: the [`codec`] backend abstraction (pure-Rust
//! default + optional ffmpeg bridge, RFC-0003), a real pure-Rust WAV codec,
//! pure-Rust MP4/H.264 video decode for preview ([`video`], RFC-0007
//! amendment), DSP helpers, the offline audio mixer that turns a project
//! timeline into a rendered master (RFC-0004), and the realtime playback
//! engine ([`realtime`], RFC-0008) with its CI smoke profile ([`smoke`]).
//!
//! Shipped: audio-only offline render (M0) + video preview decode (M1) +
//! realtime audio playback (M2). The video compositor and export pipeline
//! are specced in RFC-0007 (M3).

pub mod backend_ffmpeg;
pub mod codec;
pub mod dsp;
pub mod mixer;
pub mod realtime;
pub mod render;
pub mod smoke;
pub mod video;
pub mod wav;

pub use backend_ffmpeg::FfmpegBridge;
pub use codec::{
    AudioSource, AudioSpec, CodecBackend, CodecError, InMemorySource, MediaInfo, PureRustBackend,
    StreamInfo, StreamKind, default_backends, open_audio_any,
};
pub use mixer::{MixIssue, RenderSettings, RenderedAudio, render_audio};
#[cfg(feature = "out-cpal")]
pub use realtime::RealtimeEngine;
pub use realtime::{
    AudioOutError, Consumer, MixBlock, Producer, RealtimeMixer, RenderState, Shared, Spsc,
    audio_track_slots, estimated_output_latency_secs,
};
pub use render::{RenderError, RenderJob};
pub use smoke::{SmokeReport, run as run_smoke};
pub use video::{RgbaFrame, VideoDecoder, VideoError, VideoInfo, probe_video, write_bmp};
