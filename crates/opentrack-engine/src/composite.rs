//! The CPU video compositor (M3, RFC-0007): stacks video tracks top-down
//! (project track index 0 = top), honoring per-track opacity and blend
//! modes, per-event fade alpha ramps, the video FX chains (M4) and the
//! preview resolution scale.
//!
//! Frame sourcing goes through [`crate::open_video_any`] decoders pooled per
//! media + a shared [`FrameCache`] keyed `(MediaId, frame)`; still images
//! decode once and stay resident. Compositing math is pure functions over
//! packed RGBA8, so it is golden-testable (see `tests`).

use std::collections::HashMap;

use opentrack_core::timeline::{BlendMode, TrackKind};
use opentrack_core::{MediaId, Project};

use crate::frame_cache::FrameCache;
use crate::video::{RgbaFrame, VideoDecoder, VideoError, open_video_any};

/// Preview resolution scale (RFC-0007: composite small, upscale at
/// presentation — never decode bigger than asked).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PreviewScale {
    /// Full project resolution.
    Full,
    /// Half width/height (quarter the pixels).
    Half,
    /// Quarter width/height.
    Quarter,
}

impl PreviewScale {
    /// Canvas divisor (1, 2 or 4).
    pub fn divisor(self) -> u32 {
        match self {
            PreviewScale::Full => 1,
            PreviewScale::Half => 2,
            PreviewScale::Quarter => 4,
        }
    }

    /// Next scale, wrapping (used by the GUI toggle).
    pub fn next(self) -> Self {
        match self {
            PreviewScale::Full => PreviewScale::Half,
            PreviewScale::Half => PreviewScale::Quarter,
            PreviewScale::Quarter => PreviewScale::Full,
        }
    }
}

/// Counters surfaced in the GUI status bar (RFC-0007: drops are counted and
/// shown, never hidden).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CompositeStats {
    /// Frames decoded from media since the compositor was created.
    pub decoded: u64,
    /// Frame lookups served from the shared cache.
    pub cache_hits: u64,
    /// Frames that could not be produced (decode errors, missing media).
    pub dropped: u64,
}

/// Decodes a still image (PNG / JPEG / BMP) to packed RGBA8.
pub fn decode_still(path: &std::path::Path) -> Result<RgbaFrame, VideoError> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let dec = png::Decoder::new(std::io::Cursor::new(&bytes));
        let mut reader = dec
            .read_info()
            .map_err(|e| VideoError::Image(format!("png: {e}")))?;
        let buf_size = reader
            .output_buffer_size()
            .ok_or_else(|| VideoError::Image("png: no output buffer size".into()))?;
        let mut buf = vec![0; buf_size];
        let info = reader
            .next_frame(&mut buf)
            .map_err(|e| VideoError::Image(format!("png: {e}")))?;
        let (w, h) = (info.width, info.height);
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::Rgb => expand_rgb(&buf, w, h),
            png::ColorType::Grayscale => expand_gray(&buf, w, h),
            png::ColorType::GrayscaleAlpha => expand_gray_alpha(&buf, w, h),
            other => {
                return Err(VideoError::Image(format!(
                    "png: unsupported color type {other:?}"
                )));
            }
        };
        return Ok(RgbaFrame {
            width: w,
            height: h,
            rgba,
            pts_secs: 0.0,
            key: true,
        });
    }
    if bytes.starts_with(b"BM") {
        return decode_bmp(&bytes);
    }
    // JPEG (still) — same decoder as the MJPEG path.
    let (w, h, rgba) = crate::video::decode_jpeg_rgba(&bytes)?;
    Ok(RgbaFrame {
        width: w,
        height: h,
        rgba,
        pts_secs: 0.0,
        key: true,
    })
}

fn expand_rgb(buf: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for px in buf.as_chunks::<3>().0 {
        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    out
}

fn expand_gray(buf: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for &g in buf {
        out.extend_from_slice(&[g, g, g, 255]);
    }
    out
}

fn expand_gray_alpha(buf: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for ga in buf.as_chunks::<2>().0 {
        out.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
    }
    out
}

/// Minimal 24/32-bit uncompressed BMP reader (matches `encode_bmp` output).
fn decode_bmp(bytes: &[u8]) -> Result<RgbaFrame, VideoError> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err(VideoError::Image("bmp: truncated header".into()));
    }
    let offset = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    let w = i32::from_le_bytes(bytes[18..22].try_into().unwrap());
    let h = i32::from_le_bytes(bytes[22..26].try_into().unwrap());
    let bpp = u16::from_le_bytes(bytes[28..30].try_into().unwrap());
    if !(bpp == 24 || bpp == 32) || w <= 0 || h == 0 {
        return Err(VideoError::Image(format!(
            "bmp: unsupported format (bpp {bpp})"
        )));
    }
    let width = w as usize;
    let height = h.unsigned_abs() as usize;
    let bottom_up = h > 0;
    let row_bytes = width * (bpp as usize / 8);
    let pad = (4 - row_bytes % 4) % 4;
    let mut rgba = vec![0u8; width * height * 4];
    for row in 0..height {
        let src_row = if bottom_up { height - 1 - row } else { row };
        let start = offset + src_row * (row_bytes + pad);
        let Some(src) = bytes.get(start..start + row_bytes) else {
            return Err(VideoError::Image("bmp: truncated pixels".into()));
        };
        for (x, px) in src.chunks_exact(bpp as usize / 8).take(width).enumerate() {
            let (b, g, r) = (px[0], px[1], px[2]);
            let a = if bpp == 32 {
                *px.get(3).unwrap_or(&255)
            } else {
                255
            };
            let d = &mut rgba[(row * width + x) * 4..(row * width + x) * 4 + 4];
            d[0] = r;
            d[1] = g;
            d[2] = b;
            d[3] = a;
        }
    }
    Ok(RgbaFrame {
        width: width as u32,
        height: height as u32,
        rgba,
        pts_secs: 0.0,
        key: true,
    })
}

/// Composites one project frame at `tick`.
///
/// Owns the decoder pool + shared frame cache so repeated calls (scrubbing,
/// playback, rendering) stay warm.
pub struct Compositor {
    scale: PreviewScale,
    decoders: HashMap<MediaId, Box<dyn VideoDecoder>>,
    stills: HashMap<MediaId, std::sync::Arc<RgbaFrame>>,
    cache: FrameCache,
    stats: CompositeStats,
    /// Last non-fatal problems (missing media, decode failures) — one per
    /// frame composite, cleared per call.
    pub issues: Vec<String>,
}

impl Compositor {
    /// Builds a compositor with the default 256 MiB frame budget.
    pub fn new() -> Self {
        Self::with_budget(crate::frame_cache::DEFAULT_BUDGET_BYTES)
    }

    /// Builds a compositor with an explicit frame-cache budget (tests).
    pub fn with_budget(bytes: usize) -> Self {
        Self {
            scale: PreviewScale::Full,
            decoders: HashMap::new(),
            stills: HashMap::new(),
            cache: FrameCache::new(bytes),
            stats: CompositeStats::default(),
            issues: Vec::new(),
        }
    }

    /// Preview scale (project resolution ÷ divisor).
    pub fn scale(&self) -> PreviewScale {
        self.scale
    }

    /// Sets the preview scale (the GUI never re-decodes bigger than asked).
    pub fn set_scale(&mut self, scale: PreviewScale) {
        self.scale = scale;
    }

    /// Cache statistics snapshot.
    pub fn stats(&self) -> CompositeStats {
        self.stats
    }

    /// Drops all pooled decoders/caches (used when media files change).
    pub fn reset(&mut self) {
        self.decoders.clear();
        self.stills.clear();
        self.stats = CompositeStats::default();
    }

    /// Composites the project frame at timeline position `tick`.
    pub fn composite(&mut self, project: &Project, tick: i64) -> RgbaFrame {
        self.issues.clear();
        let div = self.scale.divisor();
        let cw = (project.video.width / div).max(1);
        let ch = (project.video.height / div).max(1);
        let mut canvas = vec![0u8; (cw * ch * 4) as usize];
        let tps = project.timebase.ticks_per_second;

        // Bottom video track first: track list index 0 is the *top* (Vegas
        // convention), so iterate reversed.
        let video_tracks: Vec<_> = project
            .tracks
            .iter()
            .filter(|t| matches!(t.kind, TrackKind::Video(_)))
            .collect();
        for track in video_tracks.iter().rev() {
            let TrackKind::Video(props) = &track.kind else {
                unreachable!()
            };
            let track_alpha = props.composite_level.clamp(0.0, 1.0);
            if track_alpha <= 0.0 {
                continue;
            }
            for event in &track.events {
                if !event.covers(opentrack_core::Ticks(tick)) {
                    continue;
                }
                let Some(frame) = self.source_frame(project, event, tick, tps) else {
                    continue;
                };
                // Fade ramps → alpha multiplier (documented linear ramps).
                let fade = fade_alpha(event, tick, tps);
                let alpha = fade * track_alpha;
                if alpha <= 0.0 {
                    continue;
                }
                // Video FX chain (M4) on the event's contribution before
                // blending (per-event; the track rack order = chain order).
                if !track.fx_chain.is_empty() {
                    let mut layer = frame.rgba.as_slice().to_vec();
                    crate::vfx::apply_chain_video(&track.fx_chain, &mut layer, tick, tps);
                    blend_onto(
                        &mut canvas,
                        cw,
                        ch,
                        &RgbaFrame {
                            width: frame.width,
                            height: frame.height,
                            rgba: layer,
                            pts_secs: frame.pts_secs,
                            key: frame.key,
                        },
                        alpha,
                        props.blend,
                    );
                } else {
                    blend_onto(&mut canvas, cw, ch, &frame, alpha, props.blend);
                }
            }
        }
        RgbaFrame {
            width: cw,
            height: ch,
            rgba: canvas,
            pts_secs: tick as f64 / tps as f64,
            key: true,
        }
    }

    /// Fetches the source frame for `event` at timeline `tick`.
    fn source_frame(
        &mut self,
        project: &Project,
        event: &opentrack_core::timeline::Event,
        tick: i64,
        tps: i64,
    ) -> Option<std::sync::Arc<RgbaFrame>> {
        let Some(asset) = project.media(event.media) else {
            self.issues
                .push(format!("missing media asset {}", event.media));
            self.stats.dropped += 1;
            return None;
        };
        let rel = (tick - event.start.0).max(0);
        let src_tick = if event.reverse {
            (event.length.0 - rel - 1).max(0) + event.media_in.0
        } else {
            rel + event.media_in.0
        };

        match &asset.kind {
            opentrack_core::project::MediaKind::Image { .. } => {
                if let Some(hit) = self.stills.get(&event.media) {
                    return Some(hit.clone());
                }
                match decode_still(&asset.path) {
                    Ok(f) => {
                        let f = std::sync::Arc::new(f);
                        self.stills.insert(event.media, f.clone());
                        Some(f)
                    }
                    Err(e) => {
                        self.issues.push(format!("'{}': {e}", asset.label));
                        self.stats.dropped += 1;
                        None
                    }
                }
            }
            opentrack_core::project::MediaKind::Video {
                fps_num, fps_den, ..
            } => {
                let fps = if *fps_den > 0 {
                    *fps_num as f64 / *fps_den as f64
                } else {
                    0.0
                }
                .max(1.0 / 1000.0);
                let src_secs = src_tick as f64 / tps as f64;
                let frame_idx = (src_secs * fps).round().max(0.0) as u64;
                if let Some(hit) = self.cache.get(event.media, frame_idx) {
                    self.stats.cache_hits += 1;
                    return Some(hit);
                }
                let dec = match self.decoders.entry(event.media) {
                    std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                    std::collections::hash_map::Entry::Vacant(v) => {
                        match open_video_any(&asset.path) {
                            Ok(d) => v.insert(d),
                            Err(e) => {
                                self.issues.push(format!("'{}': {e}", asset.label));
                                self.stats.dropped += 1;
                                return None;
                            }
                        }
                    }
                };
                match dec.seek_frame(frame_idx).and_then(|_| dec.next_frame()) {
                    Ok(f) => {
                        self.stats.decoded += 1;
                        self.cache.put(event.media, frame_idx, f.clone());
                        Some(f)
                    }
                    Err(e) => {
                        self.issues.push(format!("'{}': {e}", asset.label));
                        self.stats.dropped += 1;
                        None
                    }
                }
            }
            opentrack_core::project::MediaKind::Audio { .. } => None, // audio-only asset
        }
    }
}

impl Default for Compositor {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear fade ramp multiplier for `event` at `tick` in `[0, 1]`.
pub fn fade_alpha(event: &opentrack_core::timeline::Event, tick: i64, _tps: i64) -> f32 {
    let rel = tick - event.start.0;
    let len = event.length.0.max(1);
    let mut a = 1.0f32;
    if event.fade_in.0 > 0 && rel < event.fade_in.0 {
        a *= rel as f32 / event.fade_in.0 as f32;
    }
    if event.fade_out.0 > 0 {
        let rem = event.end().0 - tick;
        if rem < event.fade_out.0 {
            a *= rem as f32 / event.fade_out.0 as f32;
        }
    }
    let _ = len;
    a.clamp(0.0, 1.0)
}

/// Blits `src` onto `dst` letterbox-fit (bilinear) with a global `alpha`
/// multiplier and a [`BlendMode`].
pub fn blend_onto(dst: &mut [u8], dw: u32, dh: u32, src: &RgbaFrame, alpha: f32, mode: BlendMode) {
    let (sw, sh) = (src.width.max(1), src.height.max(1));
    let scale = (dw as f64 / sw as f64).min(dh as f64 / sh as f64);
    let out_w = ((sw as f64 * scale).round() as u32).clamp(1, dw);
    let out_h = ((sh as f64 * scale).round() as u32).clamp(1, dh);
    let ox = (dw.saturating_sub(out_w)) / 2;
    let oy = (dh.saturating_sub(out_h)) / 2;

    for y in 0..out_h {
        // bilinear sample rows
        let fy = (y as f64 + 0.5) / scale - 0.5;
        let sy = fy.max(0.0).min((sh - 1) as f64);
        let y0 = sy.floor() as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = (sy - y0 as f64) as f32;
        let drow = &mut dst[((oy + y) * dw + ox) as usize * 4..][..out_w as usize * 4];
        for x in 0..out_w {
            let fx = (x as f64 + 0.5) / scale - 0.5;
            let sx = fx.max(0.0).min((sw - 1) as f64);
            let x0 = sx.floor() as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let wx = (sx - x0 as f64) as f32;
            let at = |px: u32, py: u32| -> [f32; 4] {
                let o = ((py * sw + px) * 4) as usize;
                [
                    src.rgba[o] as f32,
                    src.rgba[o + 1] as f32,
                    src.rgba[o + 2] as f32,
                    src.rgba[o + 3] as f32,
                ]
            };
            let (p00, p10, p01, p11) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            let mut s = [0f32; 4];
            for c in 0..4 {
                let top = p00[c] + (p10[c] - p00[c]) * wx;
                let bot = p01[c] + (p11[c] - p01[c]) * wx;
                s[c] = top + (bot - top) * wy;
            }
            let a = (s[3] / 255.0).clamp(0.0, 1.0) * alpha;
            if a <= 0.0 {
                continue;
            }
            let d = &mut drow[(x * 4) as usize..(x * 4 + 4) as usize];
            for c in 0..3 {
                let sv = s[c];
                let dv = d[c] as f32;
                let mixed = match mode {
                    BlendMode::Normal => sv * a + dv * (1.0 - a),
                    BlendMode::Add => (dv + sv * a).min(255.0),
                    BlendMode::Multiply => {
                        let m = dv * sv / 255.0;
                        m * a + dv * (1.0 - a)
                    }
                    BlendMode::Screen => {
                        let m = 255.0 - (255.0 - dv) * (255.0 - sv) / 255.0;
                        m * a + dv * (1.0 - a)
                    }
                };
                d[c] = mixed.round().clamp(0.0, 255.0) as u8;
            }
            // Destination stays opaque (canvas alpha is presentation-only).
            d[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentrack_core::time::Ticks;
    use opentrack_core::timeline::{Event, Track, VideoTrackProps};
    use opentrack_core::{EventId, MediaId, TrackId};

    fn solid(w: u32, h: u32, c: [u8; 3]) -> RgbaFrame {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        RgbaFrame {
            width: w,
            height: h,
            rgba,
            pts_secs: 0.0,
            key: true,
        }
    }

    #[test]
    fn blend_normal_alpha_lerp() {
        // black canvas, red source at alpha 0.5 → mid red-ish
        let mut dst = vec![0u8; 4];
        blend_onto(
            &mut dst,
            1,
            1,
            &solid(1, 1, [255, 0, 0]),
            0.5,
            BlendMode::Normal,
        );
        assert_eq!(dst, [128, 0, 0, 255]);
    }

    #[test]
    fn blend_modes_match_reference_math() {
        let mut add = vec![100u8, 100, 100, 255];
        blend_onto(
            &mut add,
            1,
            1,
            &solid(1, 1, [200, 200, 200]),
            0.5,
            BlendMode::Add,
        );
        assert_eq!(add[0], 200, "100 + 200×0.5");
        let mut mul = vec![128u8, 128, 128, 255];
        blend_onto(
            &mut mul,
            1,
            1,
            &solid(1, 1, [128, 128, 128]),
            1.0,
            BlendMode::Multiply,
        );
        assert_eq!(mul[0], 64, "128×128/255");
        let mut scr = vec![128u8, 128, 128, 255];
        blend_onto(
            &mut scr,
            1,
            1,
            &solid(1, 1, [128, 128, 128]),
            1.0,
            BlendMode::Screen,
        );
        let want: f32 = 255.0 - (255.0 - 128.0) * (255.0 - 128.0) / 255.0;
        assert_eq!(scr[0], want.round() as u8);
    }

    #[test]
    fn blit_letterboxes_and_centers() {
        let mut dst = vec![0u8; 8 * 4 * 4]; // 8×4 black
        blend_onto(
            &mut dst,
            8,
            4,
            &solid(4, 4, [255, 255, 255]),
            1.0,
            BlendMode::Normal,
        );
        // center columns lit, outer columns black
        let px = |x: u32, y: u32| &dst[((y * 8 + x) * 4) as usize..((y * 8 + x) * 4 + 3) as usize];
        assert_eq!(px(0, 0), [0, 0, 0]);
        assert_eq!(px(2, 1), [255, 255, 255]);
        assert_eq!(px(5, 1), [255, 255, 255]);
    }

    #[test]
    fn fade_alpha_ramps_documented() {
        let tps = 1000i64;
        let ev = Event {
            id: EventId(1),
            start: Ticks(0),
            length: Ticks(1000),
            media: MediaId(1),
            media_in: Ticks(0),
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks(250),
            fade_out: Ticks(500),
            reverse: false,
            label: "e".into(),
        };
        assert!((fade_alpha(&ev, 0, tps) - 0.0).abs() < 1e-6);
        assert!((fade_alpha(&ev, 125, tps) - 0.5).abs() < 1e-6);
        assert!((fade_alpha(&ev, 500, tps) - 1.0).abs() < 1e-6);
        assert!((fade_alpha(&ev, 750, tps) - 0.5).abs() < 1e-6);
        assert!((fade_alpha(&ev, 999, tps) - 1.0 / 500.0).abs() < 1e-3);
    }

    /// End-to-end composite test with stills: two stacked image events on
    /// two video tracks, top track half opacity → documented blend.
    #[test]
    fn composites_stacked_stills_with_track_opacity() {
        let dir = std::env::temp_dir().join(format!("ot_comp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // red 2×2 PNG on top track (opacity 0.5), blue 2×2 PNG below
        let red_path = dir.join("red.png");
        let blue_path = dir.join("blue.png");
        write_png(&red_path, 2, 2, [255, 0, 0]);
        write_png(&blue_path, 2, 2, [0, 0, 255]);

        let mut project = Project::new("comp-test");
        project.video.width = 4;
        project.video.height = 4;
        project.video.fps_num = 30;
        project.tracks.clear();
        let t0 = TrackId::from_raw(project.alloc_id());
        project.tracks.push(Track {
            id: t0,
            name: "V2".into(),
            kind: TrackKind::Video(VideoTrackProps {
                composite_level: 0.5,
                blend: BlendMode::Normal,
            }),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        });
        let t1 = TrackId::from_raw(project.alloc_id());
        project.tracks.push(Track {
            id: t1,
            name: "V1".into(),
            kind: TrackKind::Video(VideoTrackProps {
                composite_level: 1.0,
                blend: BlendMode::Normal,
            }),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        });
        let m0 = project.add_media(
            red_path.clone(),
            None,
            opentrack_core::project::MediaKind::Image {
                width: 2,
                height: 2,
            },
        );
        let m1 = project.add_media(
            blue_path.clone(),
            None,
            opentrack_core::project::MediaKind::Image {
                width: 2,
                height: 2,
            },
        );
        let mk = |id: u64, media: MediaId| Event {
            id: EventId::from_raw(id),
            start: Ticks(0),
            length: Ticks(1000),
            media,
            media_in: Ticks(0),
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks(0),
            fade_out: Ticks(0),
            reverse: false,
            label: "e".into(),
        };
        let id0 = project.alloc_id();
        let id1 = project.alloc_id();
        project.tracks[0].events.push(mk(id0, m0));
        project.tracks[1].events.push(mk(id1, m1));

        let mut comp = Compositor::with_budget(1024);
        let frame = comp.composite(&project, 500);
        assert_eq!((frame.width, frame.height), (4, 4));
        // center pixel: 0.5×red over blue = (128, 0, 128)
        let o = ((2 * 4 + 2) * 4) as usize;
        assert_eq!(&frame.rgba[o..o + 3], &[128, 0, 128]);
        // corner pixels: still inside both blitted (fit-to-canvas) rects
        let o = 0;
        assert_eq!(&frame.rgba[o..o + 3], &[128, 0, 128]);
        std::fs::remove_dir_all(&dir).ok();
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
}
