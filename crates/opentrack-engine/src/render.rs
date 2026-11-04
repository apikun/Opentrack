//! Render job orchestration: project in, rendered master out.
//!
//! M0 ships the audio path end-to-end. The video path is intentionally a
//! documented stub — the compositor design lives in RFC-0007 and lands in M3.

use crate::codec::{CodecBackend, CodecError};
pub use crate::mixer::RenderError;
use crate::mixer::{RenderSettings, RenderedAudio, render_audio};
use opentrack_core::project::Project;
use std::path::Path;
use std::sync::Arc;

/// A render job bound to a project and output settings.
pub struct RenderJob {
    /// The project to render.
    pub project: Project,
    /// Output parameters.
    pub settings: RenderSettings,
}

impl RenderJob {
    /// Builds a job from a project with default settings.
    pub fn new(project: Project) -> Self {
        Self {
            project,
            settings: RenderSettings::default(),
        }
    }

    /// Offline audio render; `progress` receives 0.0–1.0 per track.
    pub fn run_audio(
        &self,
        backends: &[Arc<dyn CodecBackend>],
        progress: &mut dyn FnMut(f32),
    ) -> Result<RenderedAudio, RenderError> {
        render_audio(&self.project, backends, &self.settings, progress)
    }

    /// Writes a rendered master as 16-bit PCM WAV (the M0 delivery format;
    /// AV1/AVI export arrives with the M3 video pipeline).
    pub fn write_wav(&self, rendered: &RenderedAudio, path: &Path) -> Result<(), RenderError> {
        crate::wav::encode_s16(
            path,
            rendered.settings.channels,
            rendered.settings.sample_rate,
            &rendered.samples,
        )
        .map_err(|e| RenderError::Codec(CodecError::Decode(e.to_string())))?;
        Ok(())
    }

    /// Video render — planned, not implemented. Kept as an explicit,
    /// well-labeled stub instead of being silently missing.
    pub fn run_video(&self) -> Result<RenderedAudio, RenderError> {
        Err(RenderError::NotYetImplemented(
            "the video compositor is specified in RFC-0007 and scheduled for milestone M3",
        ))
    }
}
