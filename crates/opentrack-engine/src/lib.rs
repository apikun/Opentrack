//! # opentrack-engine
//!
//! OpenTrack's media engine: the [`codec`] backend abstraction (pure-Rust
//! default + optional ffmpeg bridge, RFC-0003), a real pure-Rust WAV codec,
//! video decode behind the [`video`] [`video::VideoDecoder`] trait (MPEG-4
//! AVC in pure Rust, Motion-JPEG AVI in pure Rust, everything else via the
//! ffmpeg bridge — RFC-0007), the CPU compositor ([`composite`]) and render
//! pipeline ([`render`], [`avi`]), audio FX ([`afx`]) and video FX
//! ([`vfx`]) with the M4 parity discipline, the realtime playback engine
//! ([`realtime`], RFC-0008) with its CI smoke profile ([`smoke`]), the
//! plugin host ([`plugin`], RFC-0009) and the scripting surface
//! ([`script`], RFC-0010).
//!
//! Shipped: audio offline render (M0) + interactive editing & video preview
//! decode (M1) + realtime audio (M2) + video composite/export (M3) + FX
//! framework (M4) + plugin ABI v0 reference, scripting, packaging (M5).

pub mod afx;
#[cfg(feature = "export-av1")]
pub mod av1;
pub mod avi;
pub mod backend_ffmpeg;
pub mod codec;
pub mod composite;
pub mod dsp;
pub mod frame_cache;
pub mod mixer;
pub mod plugin;
pub mod realtime;
pub mod render;
pub mod script;
pub mod smoke;
pub mod vfx;
pub mod video;
pub mod wav;

pub use backend_ffmpeg::FfmpegBridge;
pub use codec::{
    AudioSource, AudioSpec, CodecBackend, CodecError, InMemorySource, MediaInfo, PureRustBackend,
    StreamInfo, StreamKind, default_backends, open_audio_any,
};
pub use composite::{CompositeStats, Compositor, PreviewScale};
pub use frame_cache::{DEFAULT_BUDGET_BYTES, FrameCache};
pub use mixer::{MixIssue, RenderSettings, RenderedAudio, render_audio};
pub use plugin::{
    AudioBlockPlugin, PLUGIN_ABI_VERSION, PluginHost, StereoWidth, VideoFramePlugin, VideoTint,
};
#[cfg(feature = "out-cpal")]
pub use realtime::RealtimeEngine;
pub use realtime::{
    AudioOutError, Consumer, MixBlock, Producer, RealtimeMixer, RenderState, Shared, Spsc,
    audio_track_slots, estimated_output_latency_secs,
};
pub use render::{RenderError, RenderJob, VideoRenderOptions, VideoRenderStats};
pub use script::{ScriptError, ScriptReport};
pub use smoke::{SmokeReport, run as run_smoke};
pub use vfx::{
    apply_brightness_contrast, apply_chain_video, apply_lut_1d, apply_saturation, lut_curve,
};
pub use video::{
    FfmpegVideoDecoder, MjpegAviDecoder, Mp4H264Decoder, RgbaFrame, VideoDecoder, VideoError,
    VideoInfo, decode_jpeg_rgba, open_video_any, probe_video, probe_video_any, write_bmp,
};
