//! Plugin ABI v0 (M5, RFC-0009): the *in-process reference* host for the two
//! plugin domains the ABI will freeze:
//!
//! * audio — process f32 blocks in place (interleaved, one call per block);
//! * video — filter RGBA8 frames in place ([`crate::video::RgbaFrame`]).
//!
//! The dynamically-loaded C ABI (function tables, version handshake, error
//! codes) is *specified* in RFC-0009 and frozen **before any third-party
//! promise**; this module ships the host-side seams and two builtin example
//! plugins so the plumbing is exercised by tests. It deliberately does not
//! dlopen anything yet — dynamic loading lands behind its own RFC with a
//! stable, reviewed header.

use crate::video::RgbaFrame;

/// ABI version this host implements (RFC-0009).
pub const PLUGIN_ABI_VERSION: u32 = 0;

/// An audio-domain plugin: processes interleaved f32 blocks in place.
pub trait AudioBlockPlugin: Send {
    /// Stable plugin name (shown in the host UI).
    fn name(&self) -> &str;
    /// Processes one interleaved block in place.
    fn process(&mut self, block: &mut [f32], channels: usize, sample_rate: u32);
}

/// A video-domain plugin: filters packed RGBA8 frames in place.
pub trait VideoFramePlugin: Send {
    /// Stable plugin name (shown in the host UI).
    fn name(&self) -> &str;
    /// Filters one frame in place.
    fn filter(&mut self, frame: &mut RgbaFrame);
}

/// The plugin host: ordered chains of audio/video plugins applied at the
/// master bus (audio) and post-composite (video).
#[derive(Default)]
pub struct PluginHost {
    audio: Vec<Box<dyn AudioBlockPlugin>>,
    video: Vec<Box<dyn VideoFramePlugin>>,
}

impl PluginHost {
    /// An empty host.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an audio plugin (chain order = call order).
    pub fn register_audio(&mut self, p: Box<dyn AudioBlockPlugin>) {
        self.audio.push(p);
    }

    /// Registers a video plugin (chain order = call order).
    pub fn register_video(&mut self, p: Box<dyn VideoFramePlugin>) {
        self.video.push(p);
    }

    /// Registered audio plugin count.
    pub fn audio_count(&self) -> usize {
        self.audio.len()
    }

    /// Registered video plugin count.
    pub fn video_count(&self) -> usize {
        self.video.len()
    }

    /// Applies the audio chain to one interleaved block.
    pub fn process_audio(&mut self, block: &mut [f32], channels: usize, sample_rate: u32) {
        for p in &mut self.audio {
            p.process(block, channels, sample_rate);
        }
    }

    /// Applies the video chain to one frame.
    pub fn filter_video(&mut self, frame: &mut RgbaFrame) {
        for p in &mut self.video {
            p.filter(frame);
        }
    }
}

/// Example audio plugin: stereo width via M/S processing (mono-safe).
pub struct StereoWidth {
    /// 0 = mono fold, 1 = unchanged, >1 = widened.
    pub width: f32,
}

impl AudioBlockPlugin for StereoWidth {
    fn name(&self) -> &str {
        "builtin.stereo_width"
    }

    fn process(&mut self, block: &mut [f32], channels: usize, _sample_rate: u32) {
        if channels != 2 {
            return; // stereo-only in the reference host (documented)
        }
        let w = self.width.clamp(0.0, 4.0);
        if (w - 1.0).abs() < 1e-6 {
            return;
        }
        for f in block.as_chunks_mut::<2>().0 {
            let mid = (f[0] + f[1]) * 0.5;
            let side = (f[0] - f[1]) * 0.5 * w;
            f[0] = mid + side;
            f[1] = mid - side;
        }
    }
}

/// Example video plugin: uniform tint toward a target color.
pub struct VideoTint {
    /// Tint color.
    pub color: [u8; 3],
    /// Blend strength 0..1.
    pub strength: f32,
}

impl VideoFramePlugin for VideoTint {
    fn name(&self) -> &str {
        "builtin.tint"
    }

    fn filter(&mut self, frame: &mut RgbaFrame) {
        let s = self.strength.clamp(0.0, 1.0);
        if s <= 0.0 {
            return;
        }
        for px in frame.rgba.as_chunks_mut::<4>().0 {
            for (c, tint) in self.color.iter().enumerate() {
                px[c] = (px[c] as f32 * (1.0 - s) + *tint as f32 * s)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_width_folds_to_mono_at_zero() {
        let mut host = PluginHost::new();
        host.register_audio(Box::new(StereoWidth { width: 0.0 }));
        let mut block = vec![0.8, 0.2, 0.6, 0.4];
        host.process_audio(&mut block, 2, 48_000);
        assert_eq!(block[0], block[1]);
        assert_eq!(block[2], block[3]);
    }

    #[test]
    fn tint_blends_and_counts() {
        let mut host = PluginHost::new();
        host.register_video(Box::new(VideoTint {
            color: [255, 255, 255],
            strength: 0.5,
        }));
        assert_eq!(host.video_count(), 1);
        let mut frame = RgbaFrame {
            width: 1,
            height: 1,
            rgba: vec![100, 100, 100, 255],
            pts_secs: 0.0,
            key: true,
        };
        host.filter_video(&mut frame);
        assert_eq!(frame.rgba[0], 178, "0.5×100 + 0.5×255");
        assert_eq!(frame.rgba[3], 255, "alpha untouched");
    }

    #[test]
    fn abi_version_is_declared() {
        // Guards the RFC-0009 contract: the host declares what it speaks.
        assert_eq!(PLUGIN_ABI_VERSION, 0);
    }
}
