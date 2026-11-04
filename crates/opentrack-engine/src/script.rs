//! Project-level scripting (M5, RFC-0010): a Rhai-hosted batch surface over
//! the [`Project`] document. Scripts can enumerate the media pool and
//! tracks, add/move/remove events, set track/FX parameters and save — the
//! same operations the edit layer exposes, minus undo (scripts are one
//! replayable batch; the CLI records the script in the report instead).
//!
//! ```rhai
//! // batch example: pan every audio track slightly right, add a limiter
//! for i in 0..project.track_count() {
//!     project.set_track_pan(i, 0.15);
//!     project.add_fx(i, "audio.limiter");
//! }
//! project.save_otp("out.otp");
//! ```

use opentrack_core::EventId;
use opentrack_core::project::{MediaKind, Project};
use opentrack_core::time::Ticks;
use opentrack_core::timeline::{Event, TrackKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a script did (CLI report / CI log).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptReport {
    /// Number of mutating API calls the script performed.
    pub steps: u64,
    /// Lines emitted through `log(...)` / `print(...)`.
    pub log: Vec<String>,
}

/// Script execution failures.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    /// The Rhai engine rejected the script.
    #[error("script error: {0}")]
    Eval(String),
    /// The script asked to save and the filesystem refused.
    #[error("script save failed: {0}")]
    Save(#[from] opentrack_core::save::SaveError),
}

/// Runs `src` against `project` in place. The script sees the live document:
/// every mutating helper registers as one step in the report.
pub fn run(project: &mut Project, src: &str) -> Result<ScriptReport, ScriptError> {
    let steps = Arc::new(AtomicU64::new(0));
    let log: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();

    let mut engine = rhai::Engine::new();

    // `log("...")` — collects into the report (works as a free function and
    // as a method call on `project`, which is the common style in scripts).
    {
        let l1 = Arc::clone(&log);
        engine.register_fn("log", move |s: &str| {
            if let Ok(mut l) = l1.lock() {
                l.push(s.to_string());
            }
        });
        let l2 = Arc::clone(&log);
        engine.register_fn("log", move |_p: &mut Project, s: &str| {
            if let Ok(mut l) = l2.lock() {
                l.push(s.to_string());
            }
        });
    }

    // ---- read surface ---------------------------------------------------
    engine.register_fn("track_count", |p: &mut Project| p.tracks.len() as i64);
    engine.register_fn("track_is_video", |p: &mut Project, i: i64| {
        p.tracks
            .get(i as usize)
            .map(|t| t.is_video())
            .unwrap_or(false)
    });
    engine.register_fn("track_name", |p: &mut Project, i: i64| {
        p.tracks
            .get(i as usize)
            .map(|t| t.name.clone())
            .unwrap_or_default()
    });
    engine.register_fn("event_count", |p: &mut Project, i: i64| {
        p.tracks
            .get(i as usize)
            .map(|t| t.events.len() as i64)
            .unwrap_or(0)
    });
    engine.register_fn("media_count", |p: &mut Project| p.media.assets.len() as i64);
    engine.register_fn("media_label", |p: &mut Project, i: i64| {
        p.media
            .assets
            .get(i as usize)
            .map(|a| a.label.clone())
            .unwrap_or_default()
    });
    engine.register_fn("media_duration_s", |p: &mut Project, label: &str| {
        let tps = p.timebase.ticks_per_second;
        p.media
            .assets
            .iter()
            .find(|a| a.label == label)
            .and_then(|a| match &a.kind {
                MediaKind::Audio { duration, .. } | MediaKind::Video { duration, .. } => {
                    Some(duration.to_seconds_f64(tps))
                }
                MediaKind::Image { .. } => None,
            })
            .unwrap_or(0.0)
    });
    engine.register_fn("duration_s", |p: &mut Project| {
        p.duration().to_seconds_f64(p.timebase.ticks_per_second)
    });

    // ---- mutations --------------------------------------------------------
    let count = |steps: &Arc<AtomicU64>| steps.fetch_add(1, Ordering::SeqCst);

    engine.register_fn("rename_track", {
        let steps = steps.clone();
        move |p: &mut Project, i: i64, name: &str| -> bool {
            match p.tracks.get_mut(i as usize) {
                Some(t) => {
                    t.name = name.to_string();
                    count(&steps);
                    true
                }
                None => false,
            }
        }
    });

    engine.register_fn("add_audio_track", {
        let steps = steps.clone();
        move |p: &mut Project, name: &str| -> i64 {
            let id = p.alloc_id();
            p.tracks.push(opentrack_core::timeline::Track {
                id: opentrack_core::TrackId::from_raw(id),
                name: name.to_string(),
                kind: TrackKind::Audio(Default::default()),
                events: Vec::new(),
                locked: false,
                fx_chain: Default::default(),
            });
            count(&steps);
            (p.tracks.len() - 1) as i64
        }
    });

    engine.register_fn("add_video_track", {
        let steps = steps.clone();
        move |p: &mut Project, name: &str| -> i64 {
            let id = p.alloc_id();
            p.tracks.insert(
                0,
                opentrack_core::timeline::Track {
                    id: opentrack_core::TrackId::from_raw(id),
                    name: name.to_string(),
                    kind: TrackKind::Video(Default::default()),
                    events: Vec::new(),
                    locked: false,
                    fx_chain: Default::default(),
                },
            );
            count(&steps);
            0
        }
    });

    engine.register_fn("add_event", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, label: &str, start_s: f64, len_s: f64| -> bool {
            // Resolve everything that borrows `p` immutably first.
            let Some(asset) = p
                .media
                .assets
                .iter()
                .find(|a| a.label == label || a.label.eq_ignore_ascii_case(label))
                .cloned()
            else {
                return false;
            };
            let tps = p.timebase.ticks_per_second;
            let ev = Event {
                id: EventId::from_raw(p.alloc_id()),
                start: Ticks::from_seconds(start_s.max(0.0), tps),
                length: Ticks::from_seconds(len_s.max(0.0), tps),
                media: asset.id,
                media_in: Ticks::ZERO,
                takes: vec![],
                active_take: 0,
                gain_db: 0.0,
                fade_in: Ticks::ZERO,
                fade_out: Ticks::ZERO,
                reverse: false,
                label: asset.label.clone(),
            };
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let pos = track
                .events
                .partition_point(|e| e.start <= ev.start)
                .min(track.events.len());
            track.events.insert(pos, ev);
            count(&steps);
            true
        }
    });

    engine.register_fn("remove_event", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, ei: i64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            if (ei as usize) >= track.events.len() {
                return false;
            }
            track.events.remove(ei as usize);
            count(&steps);
            true
        }
    });

    engine.register_fn("move_event", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, ei: i64, start_s: f64| -> bool {
            let tps = p.timebase.ticks_per_second;
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(ev) = track.events.get_mut(ei as usize) else {
                return false;
            };
            ev.start = Ticks::from_seconds(start_s.max(0.0), tps);
            track.events.sort_by_key(|e| e.start);
            count(&steps);
            true
        }
    });

    engine.register_fn("set_event_gain_db", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, ei: i64, db: f64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(ev) = track.events.get_mut(ei as usize) else {
                return false;
            };
            ev.gain_db = db as f32;
            count(&steps);
            true
        }
    });

    engine.register_fn("set_event_fades_s", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, ei: i64, fin_s: f64, fout_s: f64| -> bool {
            let tps = p.timebase.ticks_per_second;
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(ev) = track.events.get_mut(ei as usize) else {
                return false;
            };
            ev.fade_in = Ticks::from_seconds(fin_s.max(0.0), tps);
            ev.fade_out = Ticks::from_seconds(fout_s.max(0.0), tps);
            count(&steps);
            true
        }
    });

    engine.register_fn("set_track_gain_db", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, db: f64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            match &mut track.kind {
                TrackKind::Audio(props) => {
                    props.gain_db = db as f32;
                    count(&steps);
                    true
                }
                _ => false,
            }
        }
    });

    engine.register_fn("set_track_pan", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, pan: f64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            match &mut track.kind {
                TrackKind::Audio(props) => {
                    props.pan = pan as f32;
                    count(&steps);
                    true
                }
                _ => false,
            }
        }
    });

    engine.register_fn("set_fx_param", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, fi: i64, param: &str, value: f64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(fx) = track.fx_chain.instances.get_mut(fi as usize) else {
                return false;
            };
            fx.params.insert(
                param.to_string(),
                opentrack_core::fx::FxValue::Float(value as f32),
            );
            count(&steps);
            true
        }
    });

    engine.register_fn("add_fx", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, kind: &str| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(fx) = opentrack_core::fx::FxInstance::with_defaults(kind) else {
                return false;
            };
            track.fx_chain.instances.push(fx);
            count(&steps);
            true
        }
    });

    engine.register_fn("set_fx_enabled", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, fi: i64, on: bool| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            let Some(fx) = track.fx_chain.instances.get_mut(fi as usize) else {
                return false;
            };
            fx.enabled = on;
            count(&steps);
            true
        }
    });

    engine.register_fn("set_composite_level", {
        let steps = steps.clone();
        move |p: &mut Project, ti: i64, level: f64| -> bool {
            let Some(track) = p.tracks.get_mut(ti as usize) else {
                return false;
            };
            match &mut track.kind {
                TrackKind::Video(props) => {
                    props.composite_level = level as f32;
                    count(&steps);
                    true
                }
                _ => false,
            }
        }
    });

    engine.register_fn("add_media", {
        let steps = steps.clone();
        move |p: &mut Project, path: &str, label: &str| -> String {
            let path = std::path::Path::new(path);
            if !path.exists() {
                return String::new();
            }
            // Probe order: audio backends → video (pure Rust + bridge) → still.
            let tps = p.timebase.ticks_per_second;
            let kind = (|| {
                let backends = crate::codec::default_backends();
                if let Ok(info) = crate::codec::open_audio_any(&backends, path).map(|(s, _)| s.spec()) {
                    // How long is the file? Decode cheaply via skip+read on WAV;
                    // for the bridge just trust a generous declared duration.
                    let dur = crate::codec::default_backends()
                        .iter()
                        .find_map(|b| b.probe(path).ok())
                        .and_then(|i| i.duration_secs)
                        .unwrap_or(0.0);
                    return Some(MediaKind::Audio {
                        channels: info.channels,
                        sample_rate: info.sample_rate,
                        duration: Ticks::from_seconds(dur, tps),
                    });
                }
                if let Ok(v) = crate::video::probe_video_any(path) {
                    let fps = if v.fps > 0.5 { v.fps } else { 30.0 };
                    return Some(MediaKind::Video {
                        width: v.width,
                        height: v.height,
                        fps_num: (fps * 1000.0).round() as u32,
                        fps_den: 1000,
                        duration: Ticks::from_seconds(v.duration_secs, tps),
                        has_audio: v.has_audio,
                    });
                }
                if let Ok(still) = crate::composite::decode_still(path) {
                    return Some(MediaKind::Image {
                        width: still.width,
                        height: still.height,
                    });
                }
                None
            })();
            let Some(kind) = kind else {
                return String::new();
            };
            let label = if label.is_empty() {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "media".into())
            } else {
                label.to_string()
            };
            let _id = p.add_media(path.to_path_buf(), Some(label.clone()), kind);
            count(&steps);
            label
        }
    });

    engine.register_fn("save_otp", {
        let steps = steps.clone();
        move |p: &mut Project, path: &str| -> bool {
            match opentrack_core::save::save(p, std::path::Path::new(path)) {
                Ok(()) => {
                    count(&steps);
                    true
                }
                Err(_) => false,
            }
        }
    });

    // Run it. The project travels in the scope as a variable named
    // `project` (documented in RFC-0010).
    let mut scope = rhai::Scope::new();
    scope.push("project", project.clone());

    let result: Result<rhai::Dynamic, _> = engine.eval_with_scope(&mut scope, src);
    if let Err(e) = result {
        return Err(ScriptError::Eval(e.to_string()));
    }
    drop(result);
    // The scope owns a copy; write it back.
    *project = scope.get_value::<Project>("project").ok_or_else(|| {
        ScriptError::Eval("script replaced `project` with a foreign value".into())
    })?;

    Ok(ScriptReport {
        steps: steps.load(Ordering::SeqCst),
        log: log.lock().map(|l| l.clone()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with_tone(dir: &std::path::Path) -> Project {
        let wav = dir.join("tone.wav");
        let samples = crate::dsp::sine(48_000, 48_000, 440.0, 0.5);
        crate::wav::encode_s16(&wav, 1, 48_000, &samples).unwrap();
        let mut p = Project::new("script-test");
        p.add_media(
            wav,
            Some("tone".into()),
            MediaKind::Audio {
                channels: 1,
                sample_rate: 48_000,
                duration: Ticks::from_seconds(1.0, p.timebase.ticks_per_second),
            },
        );
        p
    }

    #[test]
    fn script_builds_and_saves_a_session() {
        let dir = std::env::temp_dir().join(format!("ot_script_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("built.otp");
        let mut p = project_with_tone(&dir);
        let report = run(
            &mut p,
            r#"
            let t = project.add_audio_track("Voiceover");
            project.add_event(t, "tone", 0.0, 1.0);
            project.set_track_pan(t, 0.1);
            project.add_fx(t, "audio.limiter");
            project.set_fx_param(t, 0, "ceiling_db", -3.0);
            project.log("built 1 event");
            project.save_otp("OUT");
            "#
            .replace("OUT", &out.to_string_lossy())
            .as_str(),
        )
        .expect("script runs");
        assert_eq!(report.steps, 6);
        assert_eq!(report.log, vec!["built 1 event"]);
        assert_eq!(p.tracks.len(), 5); // 4 default + 1 added
        assert!(p.tracks[4].events.len() == 1);
        assert_eq!(p.tracks[4].fx_chain.instances[0].kind, "audio.limiter");
        assert!(out.exists(), "script saved the project");
        // and it round-trips
        let reloaded = opentrack_core::save::load(&out).unwrap();
        assert_eq!(reloaded.tracks.len(), 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn script_errors_are_clean() {
        let mut p = Project::new("err");
        let err = run(&mut p, "project.nonsense_call();").unwrap_err();
        assert!(matches!(err, ScriptError::Eval(_)));
        let mut p = Project::new("err2");
        let report = run(&mut p, "project.add_event(99, \"nope\", 0.0, 1.0);").unwrap();
        assert_eq!(report.steps, 0, "failed mutation is not a step");
    }
}
