//! The offline audio mixer: turns a project timeline into a rendered stereo
//! master (RFC-0004). Tracks are mixed in fixed [`FX_BLOCK_FRAMES`] blocks
//! so the M4 FX chains (per-track, automation-sampled at each block's start
//! tick) process offline exactly like the realtime engine — bit-identical
//! output, which the golden-file tests pin.

use crate::codec::{AudioSource, CodecBackend, CodecError};
use opentrack_core::bus::{clamp_gain_db, db_to_linear, equal_power_pan};
use opentrack_core::project::{MediaKind, Project};
use opentrack_core::time::Ticks;
use opentrack_core::{EventId, TrackKind};
use std::sync::Arc;

/// Output parameters of an offline render.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    /// Master sample rate.
    pub sample_rate: u32,
    /// Output channel count (M0: stereo only).
    pub channels: u16,
    /// Extra silence after the last event.
    pub tail_seconds: f64,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            tail_seconds: 0.0,
        }
    }
}

/// A non-fatal problem encountered while mixing (reported, not rendered).
#[derive(Clone, Debug, PartialEq)]
pub struct MixIssue {
    /// Event that caused the issue.
    pub event: EventId,
    /// Human-readable description.
    pub message: String,
}

/// The rendered master.
#[derive(Clone, Debug)]
pub struct RenderedAudio {
    /// Settings used for this render.
    pub settings: RenderSettings,
    /// Interleaved stereo master in `[-1, 1]` (may exceed before you limit it).
    pub samples: Vec<f32>,
    /// Per-channel peak.
    pub peaks: Vec<f32>,
    /// Warnings collected during the mix.
    pub issues: Vec<MixIssue>,
}

/// Offline render failures.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// No events to render.
    #[error("timeline is empty - nothing to render")]
    EmptyTimeline,
    /// Video pipeline failure (M3 renderer).
    #[error("video render error: {0}")]
    Video(#[from] crate::video::VideoError),
    /// AVI mux failure.
    #[error("avi: {0}")]
    Avi(#[from] crate::avi::AviError),
    /// Decoder failure.
    #[error("codec error: {0}")]
    Codec(#[from] crate::codec::CodecError),
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Bad render settings.
    #[error("invalid settings: {0}")]
    Settings(String),
}

/// Reads a source to completion (M0 keeps decoded audio in memory; streaming
/// decode is an M2 engine goal — RFC-0003 §7).
fn read_all(src: &mut dyn AudioSource) -> Result<Vec<f32>, CodecError> {
    let ch = src.spec().channels.max(1) as usize;
    let mut out = Vec::new();
    let mut buf = vec![0f32; 8192 * ch];
    loop {
        let frames = src.read(&mut buf)?;
        if frames == 0 {
            break;
        }
        out.extend_from_slice(&buf[..frames * ch]);
    }
    Ok(out)
}

/// Linear fade gain at sample `i` of a `len`-sample event. Shared with the
/// realtime engine so fades are bit-identical online and offline.
pub(crate) fn fade_gain(i: usize, len: usize, fade_in: usize, fade_out: usize) -> f32 {
    let mut g = 1.0f32;
    if fade_in > 0 && i < fade_in {
        g *= i as f32 / fade_in as f32;
    }
    if fade_out > 0 {
        let rem = len - i;
        if rem < fade_out {
            g *= rem as f32 / fade_out as f32;
        }
    }
    g
}

fn ticks_to_frames(t: Ticks, tps: i64, sample_rate: u32) -> usize {
    ((t.to_seconds_f64(tps)) * sample_rate as f64).round() as usize
}

/// One event decoded + resampled to the render rate, ready to be spread
/// across the mix blocks.
struct PreparedEvent {
    data: Vec<f32>,
    start_frame: usize,
    len_frames: usize,
    fade_in: usize,
    fade_out: usize,
    gain: f32,
}

/// Renders every audible audio track through event/track FX/master processing
/// into a stereo master.
///
/// Crossfades: events overlapping on one track each carry their own fades,
/// and the mixer simply sums both — so set fades on the overlap edges for a
/// Vegas-style crossfade (the auto-crossfade edit op that trims + sets fades
/// automatically lands in M2, RFC-0002 §4).
pub fn render_audio(
    project: &Project,
    backends: &[Arc<dyn CodecBackend>],
    settings: &RenderSettings,
    progress: &mut dyn FnMut(f32),
) -> Result<RenderedAudio, RenderError> {
    if settings.channels != 2 {
        return Err(RenderError::Settings(
            "M0 renders stereo masters only".into(),
        ));
    }
    let tps = project.timebase.ticks_per_second;
    let sr = settings.sample_rate.max(1);
    let tail = (settings.tail_seconds.max(0.0) * sr as f64).round() as usize;
    let total_frames = ticks_to_frames(project.duration(), tps, sr) + tail;
    if total_frames == 0 {
        return Err(RenderError::EmptyTimeline);
    }

    let mut out = vec![0f32; total_frames * 2];
    let mut issues = Vec::new();
    let any_solo = project.any_solo();

    let audio_tracks: Vec<_> = project
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio(_)))
        .collect();
    let total_tracks = audio_tracks.len().max(1);

    for (ti, track) in audio_tracks.iter().enumerate() {
        let TrackKind::Audio(props) = &track.kind else {
            unreachable!()
        };
        let audible = track.audible(any_solo);
        let t_gain = db_to_linear(clamp_gain_db(props.gain_db));
        let (p_l, p_r) = equal_power_pan(props.pan);

        if audible {
            // Decode + resample every event once (per track).
            let mut prepared: Vec<PreparedEvent> = Vec::new();
            for event in &track.events {
                if let Some(pe) = prepare_event(project, event, backends, tps, sr, &mut issues) {
                    prepared.push(pe);
                }
            }
            // Track FX chain (M4): processed in fixed blocks with automation
            // sampled at each block's start tick — identical to realtime.
            let mut runner = crate::afx::AudioChainRunner::build(&track.fx_chain, 2, sr);
            let chain = track.fx_chain.clone();
            let block_frames = crate::afx::FX_BLOCK_FRAMES;
            let mut block_start = 0usize;
            while block_start < total_frames {
                let end = (block_start + block_frames).min(total_frames);
                let n = end - block_start;
                let mut block = vec![0f32; n * 2];
                for ev in &prepared {
                    spread_event(ev, block_start, &mut block, total_frames);
                }
                if !chain.is_empty() {
                    let tick = (block_start as f64 / sr as f64 * tps as f64).round() as i64;
                    runner.process(&chain, &mut block, tick);
                }
                for (j, s) in block.iter().enumerate() {
                    let dst = block_start * 2 + j;
                    out[dst] += s * t_gain * if j % 2 == 0 { p_l } else { p_r };
                }
                block_start = end;
            }
        }
        progress((ti + 1) as f32 / total_tracks as f32);
    }

    // Master bus.
    let (m_l, m_r) = project.master.stereo_gains();
    for s in out.as_chunks_mut::<2>().0 {
        s[0] *= m_l;
        s[1] *= m_r;
    }
    let peaks = crate::dsp::peaks(&out, 2);

    Ok(RenderedAudio {
        settings: settings.clone(),
        samples: out,
        peaks,
        issues,
    })
}

/// Copies one prepared event's contribution into a mix block (fades
/// included). `block` covers frames `[block_start, block_start + n)` of the
/// timeline at output rate, stereo-interleaved.
fn spread_event(ev: &PreparedEvent, block_start: usize, block: &mut [f32], total_frames: usize) {
    let n = block.len() / 2;
    let ev_end = ev.start_frame + ev.len_frames;
    let from = ev.start_frame.max(block_start);
    let to = ev_end.min(block_start + n).min(total_frames);
    if from >= to {
        return;
    }
    for dst in from..to {
        let i = dst - ev.start_frame;
        if i >= ev.len_frames || i * 2 + 1 >= ev.data.len() {
            break; // source ended early (short file / EOF)
        }
        let g = ev.gain * fade_gain(i, ev.len_frames, ev.fade_in, ev.fade_out);
        let b = (dst - block_start) * 2;
        block[b] += ev.data[i * 2] * g;
        block[b + 1] += ev.data[i * 2 + 1] * g;
    }
}

/// Decodes + resamples one event's source to the render rate.
fn prepare_event(
    project: &Project,
    event: &opentrack_core::timeline::Event,
    backends: &[Arc<dyn CodecBackend>],
    tps: i64,
    sr: u32,
    issues: &mut Vec<MixIssue>,
) -> Option<PreparedEvent> {
    let issue = |event: EventId, message: String, issues: &mut Vec<MixIssue>| {
        issues.push(MixIssue { event, message });
    };

    let Some(asset) = project.media(event.media) else {
        issue(
            event.id,
            format!("missing media asset {}", event.media),
            issues,
        );
        return None;
    };
    let audio_ok = match &asset.kind {
        MediaKind::Audio { .. } => true,
        MediaKind::Video { has_audio, .. } => *has_audio,
        MediaKind::Image { .. } => false,
    };
    if !audio_ok {
        issue(
            event.id,
            format!("'{}' carries no audio", asset.label),
            issues,
        );
        return None;
    }

    // Open a decoder (pure-rust backend handles WAV; bridge handles the rest).
    let (mut src, _backend_name) = match crate::codec::open_audio_any(backends, &asset.path) {
        Ok(pair) => pair,
        Err(e) => {
            issue(event.id, format!("'{}': {e}", asset.label), issues);
            return None;
        }
    };
    let src_spec = src.spec();

    // Skip to the event's in-point at the *source* rate, then read fully.
    let skip = ticks_to_frames(event.media_in, tps, src_spec.sample_rate);
    if let Err(e) = src.skip_frames(skip as u64) {
        issue(
            event.id,
            format!("'{}': seek failed: {e}", asset.label),
            issues,
        );
        return None;
    }
    let raw = match read_all(src.as_mut()) {
        Ok(d) => d,
        Err(e) => {
            issue(
                event.id,
                format!("'{}': decode failed: {e}", asset.label),
                issues,
            );
            return None;
        }
    };
    let src_ch = (src_spec.channels as usize).max(1);
    let data = if src_spec.sample_rate != sr {
        crate::dsp::resample_linear(&raw, src_ch as u16, src_spec.sample_rate, sr)
    } else {
        raw
    };
    // Normalize everything to stereo (the M0 mix bus is stereo-only).
    let data = match src_ch {
        1 => data.iter().flat_map(|s| [*s, *s]).collect::<Vec<f32>>(),
        2 => data,
        _ => {
            let frames = data.len() / src_ch;
            let mut v = Vec::with_capacity(frames * 2);
            for f in 0..frames {
                v.push(data[f * src_ch]);
                v.push(data[f * src_ch + 1]);
            }
            v
        }
    };

    Some(PreparedEvent {
        data,
        start_frame: ticks_to_frames(event.start, tps, sr),
        len_frames: ticks_to_frames(event.length, tps, sr).max(1),
        fade_in: ticks_to_frames(event.fade_in, tps, sr),
        fade_out: ticks_to_frames(event.fade_out, tps, sr),
        gain: db_to_linear(event.gain_db.clamp(-96.0, 24.0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav;
    use opentrack_core::project::MediaKind;

    fn write_tone(path: &std::path::Path, freq: f64, amp: f32) {
        let samples = crate::dsp::sine(48_000, 48_000, freq, amp);
        wav::encode_s16(path, 1, 48_000, &samples).unwrap();
    }

    fn project_with_two_overlapping_tones(dir: &std::path::Path) -> Project {
        let a_path = dir.join("tone_a.wav");
        let b_path = dir.join("tone_b.wav");
        write_tone(&a_path, 440.0, 0.8);
        write_tone(&b_path, 660.0, 0.8);

        let mut p = Project::new("mix-test");
        p.tracks.clear();
        let t1_id = opentrack_core::TrackId::from_raw(p.alloc_id());
        p.tracks.push(opentrack_core::timeline::Track {
            id: t1_id,
            name: "A".into(),
            kind: TrackKind::Audio(Default::default()),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        });
        let t2_id = opentrack_core::TrackId::from_raw(p.alloc_id());
        p.tracks.push(opentrack_core::timeline::Track {
            id: t2_id,
            name: "B".into(),
            kind: TrackKind::Audio(Default::default()),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        });

        let dur = Ticks::from_seconds(1.0, p.timebase.ticks_per_second);
        let ma = p.add_media(
            a_path,
            Some("A".into()),
            MediaKind::Audio {
                channels: 1,
                sample_rate: 48_000,
                duration: dur,
            },
        );
        let mb = p.add_media(
            b_path,
            Some("B".into()),
            MediaKind::Audio {
                channels: 1,
                sample_rate: 48_000,
                duration: dur,
            },
        );

        let ev = |id: u64, media, start, len, gain: f32, fi, fo| opentrack_core::timeline::Event {
            id: opentrack_core::EventId::from_raw(id),
            start,
            length: len,
            media,
            media_in: Ticks::ZERO,
            takes: vec![],
            active_take: 0,
            gain_db: gain,
            fade_in: fi,
            fade_out: fo,
            reverse: false,
            label: "e".into(),
        };
        let tps = p.timebase.ticks_per_second;
        let id1 = p.alloc_id();
        p.track_mut(t1_id).unwrap().events.push(ev(
            id1,
            ma,
            Ticks::ZERO,
            dur,
            0.0,
            Ticks::ZERO,
            Ticks::from_seconds(0.5, tps), // fade-out over second half
        ));
        let id2 = p.alloc_id();
        p.track_mut(t2_id).unwrap().events.push(ev(
            id2,
            mb,
            Ticks::from_seconds(0.5, tps), // overlaps A for 0.5 s
            dur,
            0.0,
            Ticks::from_seconds(0.5, tps),
            Ticks::ZERO,
        ));
        p
    }

    #[test]
    fn crossfade_mix_sums_and_fades() {
        let dir = std::env::temp_dir().join(format!("opentrack-mix-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = project_with_two_overlapping_tones(&dir);
        let backends = crate::codec::default_backends();
        let settings = RenderSettings::default();
        let rendered =
            render_audio(&p, &backends, &settings, &mut |_| {}).expect("mix should succeed");

        // 1.5 s stereo at 48 kHz
        assert_eq!(rendered.samples.len(), 48_000 * 3);
        assert!(rendered.issues.is_empty(), "issues: {:?}", rendered.issues);

        let window_max = |from_s: f64, to_s: f64| -> f32 {
            let a = (from_s * 48_000.0) as usize * 2;
            let b = ((to_s * 48_000.0) as usize * 2).min(rendered.samples.len());
            rendered.samples[a..b]
                .iter()
                .fold(0f32, |m, s| m.max(s.abs()))
        };

        // Early window (0.10–0.20 s): only track A (440 Hz) — must be audible.
        assert!(window_max(0.10, 0.20) > 0.05, "early window silent");

        // Mid window (0.60–0.70 s): crossfade region — A fading out, B fading
        // in at 50%: both contribute, signal must be present.
        assert!(window_max(0.60, 0.70) > 0.01, "crossfade window silent");

        // Both sources are mono and panned center: channel peaks stay equal.
        assert!((rendered.peaks[0] - rendered.peaks[1]).abs() < 1e-3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mute_and_solo_silence_tracks() {
        let dir = std::env::temp_dir().join(format!("opentrack-solo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = project_with_two_overlapping_tones(&dir);
        // Solo only the first audio track
        for t in p.tracks.iter_mut() {
            if let TrackKind::Audio(props) = &mut t.kind {
                props.solo = t.name == "A";
            }
        }
        let backends = crate::codec::default_backends();
        let r = render_audio(&p, &backends, &RenderSettings::default(), &mut |_| {}).unwrap();
        assert!(r.peaks.iter().all(|pk| *pk > 0.0));
        // Now mute everything on A and silence it: only B (non-soloed → muted) → silence
        for t in p.tracks.iter_mut() {
            if let TrackKind::Audio(props) = &mut t.kind {
                props.solo = false;
                props.mute = true;
            }
        }
        let r2 = render_audio(&p, &backends, &RenderSettings::default(), &mut |_| {}).unwrap();
        assert!(r2.peaks.iter().all(|pk| *pk == 0.0));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn master_gain_scales_output() {
        let dir = std::env::temp_dir().join(format!("opentrack-master-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut p = project_with_two_overlapping_tones(&dir);
        p.master.gain_db = -20.0; // ×0.1
        let backends = crate::codec::default_backends();
        let quiet = render_audio(&p, &backends, &RenderSettings::default(), &mut |_| {}).unwrap();
        p.master.gain_db = 0.0;
        let loud = render_audio(&p, &backends, &RenderSettings::default(), &mut |_| {}).unwrap();
        assert!(loud.peaks[0] > quiet.peaks[0] * 5.0);

        std::fs::remove_dir_all(&dir).ok();
    }
}
