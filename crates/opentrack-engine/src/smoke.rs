//! The CI smoke profile for the realtime engine (M2 acceptance).
//!
//! Builds an 8-track, 48 kHz session of `minutes` minutes of tone events
//! entirely from synthesized WAVs (pure Rust), then plays it through the
//! *real* realtime path — worker decode-ahead, lock-free block queue,
//! [`RenderState`] (the same code the cpal callback runs) — with a simulated
//! output device. The run asserts the transport plays the full span with
//! zero underruns.
//!
//! Two pacing modes:
//! * `realtime = false` (CI): the consumer pumps as fast as the producer
//!   feeds it, so the wall time is CPU-bound (a few seconds for 10 minutes
//!   of content). Dropouts are structurally impossible while the producer
//!   keeps up, so a zero-underrun result demonstrates the mix graph and
//!   transport stay sample-continuous end to end.
//! * `realtime = true` (local soak): the consumer paces itself to wall
//!   clock, reproducing a real device's consumption rate.

use crate::realtime::{BLOCK_MS, RealtimeMixer, RenderState};
use opentrack_core::TrackId;
use opentrack_core::ids::EventId;
use opentrack_core::project::{MediaKind, Project};
use opentrack_core::time::Ticks;
use opentrack_core::timeline::{Event, Track, TrackKind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static SMOKE_RUN: AtomicU32 = AtomicU32::new(0);

/// Summary of one smoke run.
#[derive(Clone, Debug)]
pub struct SmokeReport {
    /// Audio tracks in the session.
    pub tracks: usize,
    /// Timeline length in minutes.
    pub minutes: f64,
    /// Frames actually played by the mixer.
    pub frames: u64,
    /// Callback starvations (must be 0 for a pass).
    pub underruns: u64,
    /// Decode problems reported by the worker (must be 0).
    pub issues: u64,
    /// Wall-clock duration of the run.
    pub wall_secs: f64,
    /// Whether the consumer was paced to real time.
    pub realtime: bool,
}

const TRACKS: usize = 8;
const TONE_SECS: f64 = 30.0;
const RATE: u32 = 48_000;

/// Runs the smoke profile. See the module docs for `realtime` semantics.
pub fn run(minutes: u32, realtime: bool) -> Result<SmokeReport, String> {
    let span = minutes.max(1) as f64 * 60.0;
    // Unique per run: tests may run several smokes in parallel processes'
    // same pid space, and the directory is removed at the end.
    let dir = std::env::temp_dir().join(format!(
        "opentrack-smoke-{}-{}",
        std::process::id(),
        SMOKE_RUN.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let tps = opentrack_core::time::DEFAULT_TICKS_PER_SECOND;
    let tone_ticks = Ticks::from_seconds(TONE_SECS, tps);
    let mut project = Project::new("smoke");
    project.tracks.clear();

    for i in 0..TRACKS {
        let path = dir.join(format!("smoke_tone_{i}.wav"));
        let frames = (RATE as f64 * TONE_SECS) as usize;
        let samples = crate::dsp::sine(frames, RATE, 180.0 + 40.0 * i as f64, 0.4);
        crate::wav::encode_s16(&path, 1, RATE, &samples).map_err(|e| e.to_string())?;
        let media = project.add_media(
            path,
            Some(format!("smoke{i}")),
            MediaKind::Audio {
                channels: 1,
                sample_rate: RATE,
                duration: tone_ticks,
            },
        );
        let mut events = Vec::new();
        let mut t = 0.0f64;
        while t + TONE_SECS <= span + 1e-9 {
            events.push(Event {
                id: EventId::from_raw(project.alloc_id()),
                start: Ticks::from_seconds(t, tps),
                length: tone_ticks,
                media,
                media_in: Ticks::ZERO,
                takes: vec![],
                active_take: 0,
                gain_db: 0.0,
                fade_in: Ticks::ZERO,
                fade_out: Ticks::from_seconds(0.05, tps),
                reverse: false,
                label: format!("e{}", (t / TONE_SECS) as usize),
            });
            t += TONE_SECS;
        }
        let track_id = project.alloc_id();
        project.tracks.push(Track {
            id: TrackId::from_raw(track_id),
            name: format!("Smoke {i}"),
            kind: TrackKind::Audio(Default::default()),
            events,
            locked: false,
            fx_chain: Default::default(),
        });
    }

    let mixer = RealtimeMixer::start(&project, crate::codec::default_backends(), RATE);
    let sh = mixer.shared().clone();
    let mut rs = RenderState::new(&sh);
    let block_frames = (RATE as usize * BLOCK_MS as usize) / 1000;
    let mut out = vec![0f32; block_frames * 2];

    let start = Instant::now();
    let mut played: u64 = 0;
    mixer.play();
    // Wait for the worker to acknowledge Play (it polls every ~2 ms).
    while !sh.is_playing() {
        if start.elapsed() > Duration::from_secs(10) {
            std::fs::remove_dir_all(&dir).ok();
            return Err("smoke: play was never acknowledged".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    loop {
        // Wait for data instead of rendering silence while the worker primes
        // or refills (the pump is not the real-time callback).
        if sh.is_playing() && sh.queued_blocks() == 0 {
            if start.elapsed() > Duration::from_secs(300) {
                mixer.pause();
                std::fs::remove_dir_all(&dir).ok();
                return Err("smoke: producer stalled with an empty queue".into());
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        rs.render(&sh, &mut out);
        played += out.len() as u64 / 2;
        if realtime {
            let target = played as f64 / RATE as f64;
            let wall = start.elapsed().as_secs_f64();
            if wall < target {
                std::thread::sleep(Duration::from_secs_f64(target - wall));
            }
        }
        if !sh.is_playing() && !rs.has_pending() && sh.queued_blocks() == 0 {
            break;
        }
        if start.elapsed() > Duration::from_secs(600) {
            mixer.pause();
            std::fs::remove_dir_all(&dir).ok();
            return Err("smoke: run did not terminate".into());
        }
    }
    let wall = start.elapsed().as_secs_f64();
    let report = SmokeReport {
        tracks: TRACKS,
        minutes: span / 60.0,
        frames: played,
        underruns: sh.underruns(),
        issues: sh.decode_issues(),
        wall_secs: wall,
        realtime,
    };
    mixer.join();
    std::fs::remove_dir_all(&dir).ok();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_report_shape_one_minute() {
        let r = run(1, false).expect("1-minute smoke");
        assert_eq!(r.tracks, TRACKS);
        assert_eq!(r.underruns, 0);
        assert_eq!(r.issues, 0);
        assert!(r.frames >= (60.0 * RATE as f64 * 0.99) as u64);
    }
}
