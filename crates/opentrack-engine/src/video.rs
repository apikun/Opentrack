//! MP4/H.264 video decoding for the M1 preview pane.
//!
//! The demuxer ([`craft_media`] `isobmff`) and the H.264 decoder
//! ([`filmcraft_h264`]) are external pure-Rust crates, both dual-licensed
//! MIT OR Apache-2.0 (see `ATTRIBUTION.md` in the repository root). This
//! module is the glue: container → avcC → decoder → planar 4:2:0 → packed
//! RGBA8 with a small LRU frame cache, plus a dependency-free BMP writer so
//! the CLI can export frames headlessly.
//!
//! Scope (M1): progressive 8-bit 4:2:0 H.264 in MP4/MOV. HEVC/AV1/ProRes
//! probe fine (stream info) but do not decode yet — RFC-0007 tracks the
//! video roadmap. Audio playback is M2.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use craft_media::isobmff::{self, CodecConfig, Mp4File, TrackKind};
use filmcraft_h264::Picture;
use thiserror::Error;

/// Video decode / probe errors.
#[derive(Debug, Error)]
pub enum VideoError {
    /// File-level I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Container could not be parsed or has no decodable video track.
    #[error("container: {0}")]
    Container(String),
    /// The video codec is recognized but not decodable in this build.
    #[error("codec unsupported for decode: {0}")]
    Unsupported(String),
    /// The H.264 bitstream rejected a sample.
    #[error("h264: {0}")]
    H264(#[from] filmcraft_h264::Error),
}

/// Static description of a video file (probe result).
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    /// Decoded frame width in pixels.
    pub width: u32,
    /// Decoded frame height in pixels.
    pub height: u32,
    /// Number of samples (frames) on the video track.
    pub frame_count: usize,
    /// Average frame rate (frames / media duration).
    pub fps: f64,
    /// Media duration in seconds.
    pub duration_secs: f64,
    /// Container codec tag (`h264`, `hevc`, `av1`, `vp9`, `jpeg`, ...).
    pub codec: String,
    /// Whether the container also carries an audio track.
    pub has_audio: bool,
}

/// One decoded frame, packed RGBA8 (straight alpha, row-major, top-down).
#[derive(Clone, Debug)]
pub struct RgbaFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
    /// Presentation time in seconds.
    pub pts_secs: f64,
    /// True for an IDR/random-access frame.
    pub key: bool,
}

/// Cache budget for decoded frames (256 MiB of RGBA).
const CACHE_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// Random-access MP4/H.264 video decoder with an internal frame cache.
///
/// Seeks restart decoding at the keyframe at or before the target sample and
/// decode forward (correct for closed GOPs; open-GOP files may show brief
/// artifacts after a seek — a documented M1 limitation). Backward seeks get a
/// fresh decoder so stale DPB state can never leak into a new position.
pub struct VideoDecoder {
    file: Mp4File,
    src: std::fs::File,
    path: PathBuf,
    track: usize,
    timescale: u32,
    avcc: Vec<u8>,
    dec: filmcraft_h264::Decoder,
    info: VideoInfo,
    /// Sample index of the keyframe the current decode run started at.
    run_key_idx: Option<usize>,
    /// Last sample index fed in the current run.
    run_last_idx: Option<i64>,
    /// Decoded pictures of the current run not yet consumed by a request.
    pending: Vec<Picture>,
    cache: HashMap<i64, std::sync::Arc<RgbaFrame>>,
    cache_order: VecDeque<i64>,
    cache_bytes: usize,
}

impl VideoDecoder {
    /// Opens `path` and probes it; fails cleanly when there is no decodable
    /// H.264 video track.
    pub fn open(path: &Path) -> Result<Self, VideoError> {
        let f = std::fs::File::open(path)?;
        // `File` implements `ByteSource`; `open` takes it by value, but `&T:
        // ByteSource` too — pass a reference so `self.src` stays available.
        let file = isobmff::open(&f)
            .map_err(|e| VideoError::Container(format!("{}: {e}", path.display())))?;
        let track = file
            .track_of_kind(TrackKind::Video)
            .ok_or_else(|| VideoError::Container("no video track".into()))?;
        let t = &file.tracks[track];

        let codec = match t.codec() {
            Some(c) => codec_tag(c),
            None => "unknown".to_string(),
        };
        let (w, h) = match t.video() {
            Some(v) => (v.width as u32, v.height as u32),
            None => (0, 0),
        };
        let duration_secs = if t.timescale > 0 {
            t.duration as f64 / t.timescale as f64
        } else {
            0.0
        };
        let frame_count = t.samples.len();
        let fps = if duration_secs > 0.0 {
            frame_count as f64 / duration_secs
        } else {
            0.0
        };
        let info = VideoInfo {
            width: w,
            height: h,
            frame_count,
            fps,
            duration_secs,
            codec,
            has_audio: file.track_of_kind(TrackKind::Audio).is_some(),
        };

        let avcc = match t.codec() {
            Some(CodecConfig::Avc(cfg)) => cfg.to_bytes(),
            _ => Vec::new(),
        };
        let timescale = t.timescale;

        let dec = new_decoder(&avcc)?;

        Ok(Self {
            file,
            src: f,
            path: path.to_path_buf(),
            track,
            timescale,
            avcc,
            dec,
            info,
            run_key_idx: None,
            run_last_idx: None,
            pending: Vec::new(),
            cache: HashMap::new(),
            cache_order: VecDeque::new(),
            cache_bytes: 0,
        })
    }

    /// Probe result captured at open time.
    pub fn info(&self) -> &VideoInfo {
        &self.info
    }

    /// Path this decoder was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decodes the frame presented at `time_secs` (cached when possible).
    ///
    /// Sequential forward requests continue the current GOP run and only
    /// decode the newly needed samples (O(1) amortized per frame); any other
    /// access pattern restarts at the keyframe at or before the target.
    pub fn decode_at(&mut self, time_secs: f64) -> Result<std::sync::Arc<RgbaFrame>, VideoError> {
        let tick = (time_secs * self.timescale as f64).round() as i64;
        let tick = tick.max(0);
        if let Some(hit) = self.cache.get(&tick) {
            return Ok(hit.clone());
        }

        let t = &self.file.tracks[self.track];
        let idx = t
            .sample_at_pts(tick)
            .ok_or_else(|| VideoError::Container("no sample at requested time".into()))?;
        let key_idx = t.sync_sample_before(idx);

        // Continue the run only when the request is strictly forward inside
        // the same GOP; everything else (backward, GOP jump) gets a fresh
        // decoder so stale DPB state can never leak into a new position.
        let continuing = self.run_key_idx == Some(key_idx)
            && self.run_last_idx.is_some_and(|last| (idx as i64) > last);
        if !continuing {
            self.dec = new_decoder(&self.avcc)?;
            self.pending.clear();
            self.run_key_idx = Some(key_idx);
            self.run_last_idx = None;
        }

        // Feed the samples the request needs: a continuing run only decodes
        // what is new since `run_last_idx`; a fresh run starts at the
        // keyframe itself (inclusive).
        let first: i64 = if continuing {
            self.run_last_idx.unwrap_or(idx as i64)
        } else {
            key_idx as i64 - 1
        };
        for i in (first + 1)..=(idx as i64) {
            let i = i as usize;
            let sample = self
                .file
                .read_sample(&self.src, self.track, i)
                .map_err(|e| VideoError::Container(format!("read sample {i}: {e}")))?;
            let pts = t.samples[i].pts;
            self.pending.extend(self.dec.decode(&sample, pts)?);
            self.run_last_idx = Some(i as i64);
        }

        // A restart (fresh decoder) must drain the DPB so B-frames held back
        // by reorder delay surface; a continuing run keeps them pending.
        if !continuing {
            self.pending.extend(self.dec.flush());
        }

        // Pick the picture presented at the request: the latest decoded
        // picture at or before `tick` (or the earliest available when the
        // request precedes every picture). Decoded pictures stay in
        // `pending` — later forward requests pick newer ones from the same
        // run without re-decoding.
        let chosen = self
            .pending
            .iter()
            .filter(|p| p.pts <= tick)
            .max_by_key(|p| p.pts)
            .or_else(|| self.pending.iter().min_by_key(|p| p.pts))
            .cloned();
        let Some(pic) = chosen else {
            return Err(VideoError::Container(
                "decoder produced no picture for the requested time".into(),
            ));
        };
        // Bound pending memory (playback only ever looks forward; backward
        // access restarts the run and clears it anyway).
        if self.pending.len() > 600 {
            let excess = self.pending.len() - 600;
            self.pending.drain(..excess);
        }

        let rgba = yuv420_to_rgba(&pic);
        let frame = std::sync::Arc::new(RgbaFrame {
            width: pic.width,
            height: pic.height,
            rgba,
            pts_secs: pic.pts as f64 / self.timescale as f64,
            key: pic.key,
        });
        self.cache_insert(tick, frame.clone());
        Ok(frame)
    }

    fn cache_insert(&mut self, tick: i64, frame: std::sync::Arc<RgbaFrame>) {
        self.cache_bytes += frame.rgba.len();
        self.cache_order.push_back(tick);
        self.cache.insert(tick, frame);
        while self.cache_bytes > CACHE_BUDGET_BYTES {
            let Some(oldest) = self.cache_order.pop_front() else {
                break;
            };
            if let Some(f) = self.cache.remove(&oldest) {
                self.cache_bytes = self.cache_bytes.saturating_sub(f.rgba.len());
            }
        }
    }
}

fn new_decoder(avcc: &[u8]) -> Result<filmcraft_h264::Decoder, VideoError> {
    let mut dec = filmcraft_h264::Decoder::new();
    if !avcc.is_empty() {
        dec.configure_avcc(avcc)?;
    }
    Ok(dec)
}

fn codec_tag(c: &CodecConfig) -> String {
    use CodecConfig as C;
    match c {
        C::Avc(_) => "h264".into(),
        C::Hevc(_) => "hevc".into(),
        C::Av1(_) => "av1".into(),
        C::Vp9(_) => "vp9".into(),
        C::ProRes { .. } => "prores".into(),
        C::Jpeg { .. } => "mjpeg".into(),
        C::Dnx { .. } => "dnx".into(),
        C::Apv(_) => "apv".into(),
        C::Aac(_) => "aac".into(),
        _ => "other".into(),
    }
}

/// Probes a file through the MP4/MOV path without keeping a decoder open.
pub fn probe_video(path: &Path) -> Result<VideoInfo, VideoError> {
    VideoDecoder::open(path).map(|d| d.info().clone())
}

/// Converts a planar 4:2:0 picture to packed RGBA8 with bilinear chroma
/// upsampling. Matrix selection follows the VUI (H.273 code points):
/// BT.709 for matrix=1 (and unspecified HD), BT.601 for matrix=5/6 and
/// unspecified SD; range handling honours `full_range`.
pub fn yuv420_to_rgba(pic: &Picture) -> Vec<u8> {
    let w = pic.width as usize;
    let h = pic.height as usize;
    let mut out = vec![0u8; w * h * 4];
    if w == 0 || h == 0 {
        return out;
    }

    // Matrix coefficients (H.273): 1 = BT.709, 5/6 = BT.601; unspecified
    // defaults to BT.709 for HD and BT.601 for SD (ffmpeg's convention).
    let hd = pic.width >= 1280 || pic.height >= 720;
    let bt709 = match pic.color.matrix {
        1 => true,
        5 | 6 => false,
        _ => hd,
    };
    // Limited (16-235 / 16-240) vs full range expansion coefficients.
    let (kr_exp, kb_exp) = if bt709 {
        (1.5748_f64, 1.8556)
    } else {
        (1.402, 1.772)
    };
    let (y_k, uv_k, u_g, v_g) = if pic.color.full_range {
        if bt709 {
            (1.0_f64, 1.0, 0.1874, 0.4681)
        } else {
            (1.0, 1.0, 0.3441, 0.7141)
        }
    } else if bt709 {
        (1.164, 255.0 / 224.0, 0.213, 0.533)
    } else {
        (1.164, 255.0 / 224.0, 0.391, 0.813)
    };

    let cw = pic.chroma_width as usize;
    let ch = pic.chroma_height as usize;
    let ys = pic.y_stride;
    let uvs = pic.uv_stride;

    #[inline]
    fn plane_at(plane: &[u8], stride: usize, x: usize, y: usize) -> f64 {
        plane.get(y * stride + x).copied().unwrap_or(128) as f64
    }

    // Bilinear chroma: chroma sample (cx, cy) covers output pixels
    // [2cx, 2cx+2) x [2cy, 2cy+2); sample position is the block center.
    for y in 0..h {
        let fy = ((y as f64 + 0.5) / 2.0 - 0.5)
            .max(0.0)
            .min((ch.max(1) - 1) as f64);
        let cy0 = fy.floor() as usize;
        let cy1 = (cy0 + 1).min(ch.saturating_sub(1));
        let wy = fy - cy0 as f64;
        let row = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            let fx = ((x as f64 + 0.5) / 2.0 - 0.5)
                .max(0.0)
                .min((cw.max(1) - 1) as f64);
            let cx0 = fx.floor() as usize;
            let cx1 = (cx0 + 1).min(cw.saturating_sub(1));
            let wx = fx - cx0 as f64;

            let u00 = plane_at(&pic.u, uvs, cx0, cy0);
            let u10 = plane_at(&pic.u, uvs, cx1, cy0);
            let u01 = plane_at(&pic.u, uvs, cx0, cy1);
            let u11 = plane_at(&pic.u, uvs, cx1, cy1);
            let v00 = plane_at(&pic.v, uvs, cx0, cy0);
            let v10 = plane_at(&pic.v, uvs, cx1, cy0);
            let v01 = plane_at(&pic.v, uvs, cx0, cy1);
            let v11 = plane_at(&pic.v, uvs, cx1, cy1);
            let u_top = u00 + (u10 - u00) * wx;
            let u_bot = u01 + (u11 - u01) * wx;
            let u = u_top + (u_bot - u_top) * wy - 128.0;
            let v_top = v00 + (v10 - v00) * wx;
            let v_bot = v01 + (v11 - v01) * wx;
            let v = v_top + (v_bot - v_top) * wy - 128.0;

            let yv = plane_at(&pic.y, ys, x, y);
            let yy = (yv - if pic.color.full_range { 0.0 } else { 16.0 }) * y_k;

            let r = yy + kr_exp * v * uv_k / 1.0;
            let b = yy + kb_exp * u * uv_k / 1.0;
            let g = yy - u_g * u * uv_k - v_g * v * uv_k;

            row[x * 4] = r.round().clamp(0.0, 255.0) as u8;
            row[x * 4 + 1] = g.round().clamp(0.0, 255.0) as u8;
            row[x * 4 + 2] = b.round().clamp(0.0, 255.0) as u8;
            row[x * 4 + 3] = 255;
        }
    }
    out
}

/// Encodes an RGBA frame as a 24-bit uncompressed BMP (bottom-up BGR).
/// Dependency-free so the CLI can export stills anywhere.
pub fn encode_bmp(frame: &RgbaFrame) -> Result<Vec<u8>, VideoError> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let row_bytes = w * 3;
    let pad = (4 - row_bytes % 4) % 4;
    let stride = row_bytes + pad;
    let data_size = stride * h;
    let file_size = 54 + data_size;

    let mut out = Vec::with_capacity(file_size);
    // BITMAPFILEHEADER
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    out.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset
    // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // positive = bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes()); // 72 dpi
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in (0..h).rev() {
        for x in 0..w {
            let p = (y * w + x) * 4;
            if p + 2 < frame.rgba.len() {
                out.push(frame.rgba[p + 2]); // B
                out.push(frame.rgba[p + 1]); // G
                out.push(frame.rgba[p]); // R
            } else {
                out.extend_from_slice(&[0, 0, 0]);
            }
        }
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    Ok(out)
}

/// Writes a frame to `path` as BMP.
pub fn write_bmp(frame: &RgbaFrame, path: &Path) -> Result<(), VideoError> {
    let bytes = encode_bmp(frame)?;
    let mut f = std::fs::File::create(path)?;
    f.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generates a small H.264 MP4 fixture with ffmpeg; returns None when
    /// ffmpeg or libx264 is unavailable (CI containers without ffmpeg skip).
    fn fixture(dir: &Path) -> Option<PathBuf> {
        let out = dir.join("clip.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=duration=1:size=320x240:rate=10",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-g",
                "5",
                "-pix_fmt",
                "yuv420p",
                "-an",
                out.to_str()?,
            ])
            .status()
            .ok()?
            .success();
        ok.then_some(out)
    }

    #[test]
    fn probe_and_decode_fixture() {
        let dir = std::env::temp_dir();
        let Some(path) = fixture(&dir) else {
            eprintln!("ffmpeg/libx264 unavailable - skipping");
            return;
        };
        let mut dec = VideoDecoder::open(&path).expect("open fixture");
        let info = dec.info().clone();
        assert_eq!(info.codec, "h264");
        assert_eq!((info.width, info.height), (320, 240));
        assert!(info.frame_count >= 9, "expected ~10 frames, got {info:?}");
        assert!((info.fps - 10.0).abs() < 0.5);
        assert!(!info.has_audio);

        // First frame and a mid-GOP frame decode to the right size, opaque.
        let f0 = dec.decode_at(0.0).expect("decode t=0");
        assert_eq!((f0.width, f0.height), (320, 240));
        assert_eq!(f0.rgba.len(), 320 * 240 * 4);
        assert!(f0.rgba.iter().skip(3).step_by(4).all(|&a| a == 255));
        assert!(f0.key, "t=0 starts at an IDR");

        let fmid = dec.decode_at(0.55).expect("decode t=0.55");
        assert_eq!(fmid.rgba.len(), 320 * 240 * 4);
        // testsrc2 has colorful content; some pixel must be non-gray.
        assert!(
            fmid.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[0].abs_diff(p[1]) > 8 || p[1].abs_diff(p[2]) > 8),
            "frame looks flat - color conversion is broken"
        );

        // Backward seek must not corrupt state (fresh decoder path).
        let fback = dec.decode_at(0.05).expect("decode backwards");
        assert_eq!(fback.rgba.len(), 320 * 240 * 4);

        // Cached hit returns the same frame.
        let cached = dec.decode_at(0.05).expect("cache hit");
        assert_eq!(cached.rgba, fback.rgba);
    }

    #[test]
    fn bmp_roundtrip_header() {
        let frame = RgbaFrame {
            width: 2,
            height: 2,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 255],
            pts_secs: 0.0,
            key: true,
        };
        let bmp = encode_bmp(&frame).expect("encode");
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize,
            bmp.len()
        );
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        assert_eq!(
            u32::from_le_bytes(bmp[18..22].try_into().unwrap()) as i32,
            2
        );
        assert_eq!(
            u32::from_le_bytes(bmp[22..26].try_into().unwrap()) as i32,
            2
        );
        // Bottom-up: first stored row is the LAST image row (blue, black).
        assert_eq!(&bmp[54..57], &[255, 0, 0]); // BGR blue
        assert_eq!(&bmp[57..60], &[0, 0, 0]); // BGR black
    }

    #[test]
    fn yuv_grey_yields_grey_rgb() {
        // 4x4 all-neutral Y=U=V (limited range mid grey).
        let pic = Picture {
            width: 4,
            height: 4,
            chroma_width: 2,
            chroma_height: 2,
            y: vec![128; 16],
            u: vec![128; 4],
            v: vec![128; 4],
            y_stride: 4,
            uv_stride: 2,
            pts: 0,
            poc: 0,
            key: true,
            color: filmcraft_h264::ColorInfo {
                full_range: false,
                primaries: 1,
                transfer: 1,
                matrix: 1,
            },
            sar: (0, 0),
            draft: false,
        };
        let rgba = yuv420_to_rgba(&pic);
        for px in rgba.as_chunks::<4>().0 {
            // Y=128 in limited range maps to (128-16)*255/219 ≈ 130.
            assert!(
                (px[0] as i32 - 130).abs() <= 2 && (px[1] as i32 - 130).abs() <= 2,
                "neutral YUV must stay neutral, got rgb({}, {}, {})",
                px[0],
                px[1],
                px[2]
            );
        }
    }

    #[test]
    fn non_mp4_fails_clean() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("ot_notmp4_{}.txt", std::process::id()));
        std::fs::write(&dir, b"definitely not an mp4 file").unwrap();
        let err = match VideoDecoder::open(&dir) {
            Err(e) => e,
            Ok(_) => panic!("non-mp4 must fail to open"),
        };
        assert!(matches!(err, VideoError::Container(_)), "got {err:?}");
        std::fs::remove_file(&dir).ok();
    }
}
