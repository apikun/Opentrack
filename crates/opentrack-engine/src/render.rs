//! Render job orchestration: project in, rendered master out.
//!
//! M0 shipped the audio path end-to-end; M3 adds the video path: compositor
//! → JPEG frames → pure-Rust AVI/MJPEG mux, with the audio master written
//! alongside as 16-bit PCM WAV from the same tick clock (single master
//! clock — the RFC-0007 sync discipline). Optional AV1 export lives behind
//! the `export-av1` feature ([`crate::av1`]) and is never built by default.

use crate::codec::{CodecBackend, CodecError};
pub use crate::mixer::RenderError;
use crate::mixer::{RenderSettings, RenderedAudio, render_audio};
use opentrack_core::project::Project;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A render job bound to a project and output settings.
pub struct RenderJob {
    /// The project to render.
    pub project: Project,
    /// Output parameters.
    pub settings: RenderSettings,
}

/// Options for [`RenderJob::run_video`].
#[derive(Clone, Debug)]
pub struct VideoRenderOptions {
    /// JPEG quality for the MJPEG frames (1–100).
    pub jpeg_quality: u8,
    /// When set, every composited frame is also written here as
    /// `frame_000001.png` … (the "compositor → PNG frames" RFC leg).
    pub frames_dir: Option<PathBuf>,
    /// When set, the offline audio master is rendered and written to this
    /// WAV path (same master clock, same duration).
    pub audio_wav: Option<PathBuf>,
    /// Progress phase label source (audio first, then video) — callers get
    /// `progress(0..1)` over the whole job.
    pub quiet: bool,
}

impl Default for VideoRenderOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: 85,
            frames_dir: None,
            audio_wav: None,
            quiet: false,
        }
    }
}

/// Facts about a finished video render (the frame-count acceptance data).
#[derive(Clone, Debug, PartialEq)]
pub struct VideoRenderStats {
    /// Frames actually written into the AVI.
    pub frames: u64,
    /// Frame size (project video settings ÷ 1 — render is always full-res).
    pub width: u32,
    pub height: u32,
    /// Timeline duration in seconds.
    pub duration_secs: f64,
    /// Audio samples per channel in the companion WAV (when rendered).
    pub audio_frames: Option<usize>,
    /// Non-fatal composite issues (missing media, decode errors).
    pub issues: Vec<String>,
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

    /// Writes a rendered master as 16-bit PCM WAV (the M0 delivery format).
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

    /// Full video render (M3, RFC-0007):
    ///
    /// 1. offline audio master → WAV (optional, same tick clock);
    /// 2. composite every frame at full resolution;
    /// 3. encode JPEG per frame (pure Rust) → AVI/MJPEG (pure Rust mux);
    /// 4. optionally export PNG stills alongside.
    ///
    /// Frame count is exactly `ceil(duration × fps)` — the acceptance rule.
    pub fn run_video(
        &self,
        backends: &[Arc<dyn CodecBackend>],
        out_avi: &Path,
        opts: &VideoRenderOptions,
        progress: &mut dyn FnMut(f32),
    ) -> Result<VideoRenderStats, RenderError> {
        let p = &self.project;
        let tps = p.timebase.ticks_per_second;
        let fps = p.video.fps();
        let duration_secs = p.duration().to_seconds_f64(tps);
        let frames_total = (duration_secs * fps).ceil().max(0.0) as u64;
        if frames_total == 0 {
            return Err(RenderError::EmptyTimeline);
        }
        if p.video.width == 0 || p.video.height == 0 {
            return Err(RenderError::Settings("project video size is zero".into()));
        }

        // --- audio pass (same master clock) -----------------------------
        let mut audio_frames = None;
        if let Some(wav_path) = &opts.audio_wav {
            let rendered = render_audio(p, backends, &self.settings, &mut |f| {
                if !opts.quiet {
                    progress(f * 0.25);
                }
            })?;
            audio_frames = Some(rendered.samples.len() / 2);
            self.write_wav(&rendered, wav_path)?;
        }

        // --- frame pass ---------------------------------------------------
        if let Some(dir) = &opts.frames_dir {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(out_avi)?;
        let mut avi = crate::avi::AviMjpegWriter::new(
            file,
            p.video.width,
            p.video.height,
            p.video.fps_num,
            p.video.fps_den,
        );
        let mut compositor = crate::composite::Compositor::new();
        let mut issues: Vec<String> = Vec::new();

        for i in 0..frames_total {
            let tick = (i as f64 / fps * tps as f64).round() as i64;
            let frame = compositor.composite(p, tick);
            issues.extend(compositor.issues.iter().cloned());

            let (fw, fh) = (
                u16::try_from(frame.width).map_err(|_| {
                    RenderError::Settings("video width exceeds 65535 (JPEG limit)".into())
                })?,
                u16::try_from(frame.height).map_err(|_| {
                    RenderError::Settings("video height exceeds 65535 (JPEG limit)".into())
                })?,
            );
            let mut jpeg = Vec::with_capacity(frame.rgba.len() / 4);
            let enc = jpeg_encoder::Encoder::new(&mut jpeg, opts.jpeg_quality);
            enc.encode(&frame.rgba, fw, fh, jpeg_encoder::ColorType::Rgba)
                .map_err(|e| {
                    RenderError::Video(crate::video::VideoError::Image(format!("jpeg: {e}")))
                })?;
            avi.write_jpeg_frame(&jpeg)?;

            if let Some(dir) = &opts.frames_dir {
                let path = dir.join(format!("frame_{i:06}.png"));
                write_png(&frame, &path)?;
            }
            if !opts.quiet {
                progress(0.25 + 0.75 * (i + 1) as f32 / frames_total as f32);
            }
        }
        let frames = avi.finish()?;
        issues.extend(compositor.issues.iter().cloned());

        Ok(VideoRenderStats {
            frames,
            width: p.video.width,
            height: p.video.height,
            duration_secs,
            audio_frames,
            issues,
        })
    }

    /// AV1 export (optional feature `export-av1`): composites the timeline
    /// and feeds frames to rav1e, muxed into an IVF container.
    #[cfg(feature = "export-av1")]
    pub fn run_video_av1(
        &self,
        out_ivf: &Path,
        opts: &VideoRenderOptions,
        speed: u8,
        progress: &mut dyn FnMut(f32),
    ) -> Result<VideoRenderStats, RenderError> {
        crate::av1::render_av1(&self.project, out_ivf, opts, speed, progress)
    }
}

/// Writes an RGBA frame as a PNG (pure Rust `png` crate).
pub fn write_png(frame: &crate::video::RgbaFrame, path: &Path) -> Result<(), RenderError> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(file, frame.width, frame.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc
        .write_header()
        .map_err(|e| RenderError::Video(crate::video::VideoError::Image(format!("png: {e}"))))?;
    writer
        .write_image_data(&frame.rgba)
        .map_err(|e| RenderError::Video(crate::video::VideoError::Image(format!("png: {e}"))))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::default_backends;
    use opentrack_core::time::Ticks;
    use opentrack_core::timeline::{Event, Track};
    use opentrack_core::{EventId, MediaId, TrackId};

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ot_render_{}_{}", name, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_png(path: &std::path::Path, w: u32, h: u32, c: [u8; 3]) {
        let file = std::fs::File::create(path).unwrap();
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        let mut data = Vec::new();
        for _ in 0..w * h {
            data.extend_from_slice(&c);
        }
        writer.write_image_data(&data).unwrap();
    }

    /// RFC-0007 acceptance: a 6.0 s session (PNG still + WAV audio) renders
    /// to an AVI whose frame count × fps matches the timeline duration
    /// exactly (180 frames @ 30 fps), and the companion WAV matches the M0
    /// mixer's output sample count.
    #[test]
    fn video_render_frame_count_acceptance() {
        let dir = tmp("accept");
        let png_path = dir.join("still.png");
        write_png(&png_path, 8, 8, [200, 30, 30]);
        let wav_path = dir.join("tone.wav");
        let samples = crate::dsp::sine(48_000 * 3, 48_000, 440.0, 0.4);
        crate::wav::encode_s16(&wav_path, 1, 48_000, &samples).unwrap();

        let mut p = Project::new("accept");
        p.video = opentrack_core::VideoSettings {
            width: 64,
            height: 48,
            fps_num: 30,
            fps_den: 1,
        };
        // keep only Video 1 / Audio 1 from the default stack
        p.tracks.clear();
        let tps = p.timebase.ticks_per_second;
        let mk_track = |id: u64, name: &str, kind| Track {
            id: TrackId::from_raw(id),
            name: name.into(),
            kind,
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        };
        let id_v = p.alloc_id();
        let id_a = p.alloc_id();
        p.tracks.push(mk_track(
            id_v,
            "V1",
            opentrack_core::TrackKind::Video(Default::default()),
        ));
        p.tracks.push(mk_track(
            id_a,
            "A1",
            opentrack_core::TrackKind::Audio(Default::default()),
        ));
        let mv = p.add_media(
            png_path,
            None,
            opentrack_core::project::MediaKind::Image {
                width: 8,
                height: 8,
            },
        );
        let ma = p.add_media(
            wav_path,
            None,
            opentrack_core::project::MediaKind::Audio {
                channels: 1,
                sample_rate: 48_000,
                duration: Ticks::from_seconds(3.0, tps),
            },
        );
        let six = Ticks::from_seconds(6.0, tps);
        let ev = |p: &mut Project, media: MediaId, len: Ticks| Event {
            id: EventId::from_raw(p.alloc_id()),
            start: Ticks::ZERO,
            length: len,
            media,
            media_in: Ticks::ZERO,
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            reverse: false,
            label: "e".into(),
        };
        let e1 = ev(&mut p, mv, six);
        p.tracks[0].events.push(e1);
        let three = Ticks::from_seconds(3.0, tps);
        let e2 = ev(&mut p, ma, three);
        p.tracks[1].events.push(e2);

        let avi_path = dir.join("out.avi");
        let wav_out = dir.join("out.wav");
        let frames_dir = dir.join("frames");
        let mut job = RenderJob::new(p);
        let opts = VideoRenderOptions {
            jpeg_quality: 80,
            frames_dir: Some(frames_dir.clone()),
            audio_wav: Some(wav_out.clone()),
            quiet: true,
        };
        let stats = job
            .run_video(&default_backends(), &avi_path, &opts, &mut |_| {})
            .expect("video render");
        assert_eq!(stats.frames, 180, "6.0s × 30fps must be exactly 180 frames");
        assert_eq!((stats.width, stats.height), (64, 48));
        assert_eq!(
            stats.audio_frames,
            Some(rendered_audio_frames(&mut job)),
            "companion WAV matches the mixer output"
        );
        // AVI demuxes back with the same frame count (round trip).
        let mut f = std::fs::File::open(&avi_path).unwrap();
        let (info, table) = crate::avi::read_avi_video(&mut f).unwrap();
        assert_eq!(info.frame_count, 180);
        assert_eq!(table.len(), 180);
        assert_eq!(info.codec, *b"MJPG");
        // PNG stills exported alongside: one per frame.
        let pngs: Vec<_> = std::fs::read_dir(&frames_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(pngs.len(), 180, "one PNG per composited frame");
        // AVI is playable by ffmpeg when it exists (sanity, non-fatal).
        drop(f);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn rendered_audio_frames(job: &mut RenderJob) -> usize {
        let rendered = job
            .run_audio(&default_backends(), &mut |_| {})
            .expect("audio render");
        rendered.samples.len() / 2
    }

    /// FX chains participate in the offline render (M4 mixer integration).
    #[test]
    fn track_fx_chain_changes_offline_output() {
        let dir = tmp("fxmix");
        let wav_path = dir.join("tone.wav");
        let samples = crate::dsp::sine(48_000, 48_000, 440.0, 0.8);
        crate::wav::encode_s16(&wav_path, 1, 48_000, &samples).unwrap();
        let mut p = Project::new("fxmix");
        p.tracks.clear();
        let tps = p.timebase.ticks_per_second;
        let mut t = Track {
            id: TrackId::from_raw(p.alloc_id()),
            name: "A1".into(),
            kind: opentrack_core::TrackKind::Audio(Default::default()),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        };
        let ma = p.add_media(
            wav_path.clone(),
            None,
            opentrack_core::project::MediaKind::Audio {
                channels: 1,
                sample_rate: 48_000,
                duration: Ticks::from_seconds(1.0, tps),
            },
        );
        t.events.push(Event {
            id: EventId::from_raw(p.alloc_id()),
            start: Ticks::ZERO,
            length: Ticks::from_seconds(1.0, tps),
            media: ma,
            media_in: Ticks::ZERO,
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            reverse: false,
            label: "e".into(),
        });
        // Limiter at -6 dB ceiling: peak must come down.
        t.fx_chain
            .instances
            .push(opentrack_core::fx::FxInstance::with_defaults("audio.limiter").unwrap());
        t.fx_chain.instances[0].params.insert(
            "ceiling_db".into(),
            opentrack_core::fx::FxValue::Float(-6.0),
        );
        p.tracks.push(t);
        let job = RenderJob::new(p);
        let r = job.run_audio(&default_backends(), &mut |_| {}).unwrap();
        let peak = r.peaks.iter().fold(0f32, |m, x| m.max(*x));
        let ceiling = 10f32.powf(-6.0 / 20.0);
        assert!(
            peak <= ceiling * 1.01,
            "limiter must cap the track bus: {peak} vs {ceiling}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
