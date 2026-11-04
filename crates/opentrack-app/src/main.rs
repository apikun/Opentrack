//! # opentrack-app — the OpenTrack GUI (egui / eframe)
//!
//! M1 scope: the timeline became an *editor* — pointer drag to move and trim
//! events (with snapping), alt-drag to slip, `S` splits at the playhead,
//! `Delete` ripple-deletes, track strips carry gain/pan/mute/solo, the media
//! pool imports via drag & drop, and a preview pane decodes video under the
//! playhead through the pure-Rust H.264 path (RFC-0005 / RFC-0007).
//! Everything mutates the model through `Session` `EditOp`s — undo/redo is
//! exact (Ctrl+Z / Ctrl+Y).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console on Windows release

use eframe::egui;
use opentrack_core::edit::{Session, SnapOptions, snap_time};
use opentrack_core::save;
use opentrack_core::timeline::AudioTrackProps;
use opentrack_core::undo::EditOp;
use opentrack_core::{EventId, MediaId, MediaKind, Project, Ticks, TrackId, TrackKind};
use opentrack_engine::VideoDecoder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn main() -> Result<(), eframe::Error> {
    let args: Vec<String> = std::env::args().collect();
    let project_arg = args
        .iter()
        .position(|a| a == "--project")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_app_id("opentrack"),
        ..Default::default()
    };
    eframe::run_native(
        "OpenTrack - clean-room Rust NLE (M1)",
        options,
        Box::new(move |_cc| Ok(Box::new(OpenTrackApp::new(project_arg)))),
    )
}

const HEADER_W: f32 = 182.0;
const RULER_H: f32 = 26.0;
const ROW_H: f32 = 50.0;
const EDGE_PX: f32 = 7.0;
const PREVIEW_MAX_H: f32 = 180.0;

const COL_VIDEO_TRACK: egui::Color32 = egui::Color32::from_rgb(52, 86, 132);
const COL_AUDIO_TRACK: egui::Color32 = egui::Color32::from_rgb(34, 94, 74);
const COL_EVENT_VIDEO: egui::Color32 = egui::Color32::from_rgb(86, 138, 214);
const COL_EVENT_AUDIO: egui::Color32 = egui::Color32::from_rgb(70, 190, 130);
const COL_EVENT_SEL: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const COL_PLAYHEAD: egui::Color32 = egui::Color32::from_rgb(235, 80, 80);
fn col_ghost() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(255, 240, 180, 120)
}

#[derive(Clone, Debug)]
enum Modal {
    About,
    OpenPath(String),
    SavePath(String),
}

/// What the user is currently doing with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Move,
    TrimLeft,
    TrimRight,
    Slip,
}

#[derive(Clone, Debug)]
struct DragState {
    event: EventId,
    track: TrackId,
    kind: DragKind,
    orig_start: Ticks,
    orig_length: Ticks,
    orig_media_in: Ticks,
    /// Pointer time (ticks) at grab; zero for trims.
    grab_ticks: i64,
    preview_start: Ticks,
    preview_length: Ticks,
    preview_media_in: Ticks,
}

impl DragState {
    fn new(event: EventId, track: TrackId, ev_start: Ticks, len: Ticks, media_in: Ticks) -> Self {
        Self {
            event,
            track,
            kind: DragKind::Move,
            orig_start: ev_start,
            orig_length: len,
            orig_media_in: media_in,
            grab_ticks: 0,
            preview_start: ev_start,
            preview_length: len,
            preview_media_in: media_in,
        }
    }
}

/// One decoded-video texture plus the decoder feeding it.
struct PreviewPane {
    media: MediaId,
    label: String,
    dec: VideoDecoder,
    tex: egui::TextureHandle,
    tex_key: String,
    err: Option<String>,
}

/// Interactions collected while drawing the timeline, applied afterwards so
/// the project is never mutated mid-paint.
enum TlEvent {
    Select(Option<EventId>),
    BeginDrag {
        event: EventId,
        track: TrackId,
        kind: DragKind,
        grab_ticks: i64,
    },
    UpdateDrag {
        pointer_ticks: i64,
    },
    EndDrag,
    Scrub(f64),
    ToggleMute(TrackId),
    ToggleSolo(TrackId),
    CommitGain(TrackId, f32),
    CommitPan(TrackId, f32),
}

struct OpenTrackApp {
    session: Session,
    project_path: Option<PathBuf>,
    zoom: f32,     // pixels per second
    playhead: f64, // seconds
    playing: bool,
    last_frame_time: Option<f64>,
    selected: Option<EventId>,
    drag: Option<DragState>,
    modal: Option<Modal>,
    toast: Option<(String, f64)>,
    snap: bool,
    ripple: bool,
    preview: Option<PreviewPane>,
    pool_selection: Option<MediaId>,
    gain_drag: HashMap<TrackId, f32>,
    pan_drag: HashMap<TrackId, f32>,
}

impl OpenTrackApp {
    fn new(project_path: Option<PathBuf>) -> Self {
        let project = project_path
            .as_deref()
            .and_then(|p| save::load(p).ok())
            .unwrap_or_else(|| Project::new("Untitled"));
        Self {
            session: Session::new(project),
            project_path,
            zoom: 90.0,
            playhead: 0.0,
            playing: false,
            last_frame_time: None,
            selected: None,
            drag: None,
            modal: None,
            toast: None,
            snap: true,
            ripple: true,
            preview: None,
            pool_selection: None,
            gain_drag: HashMap::new(),
            pan_drag: HashMap::new(),
        }
    }

    // ---- model helpers -------------------------------------------------

    fn project(&self) -> &Project {
        &self.session.project
    }

    fn tps(&self) -> i64 {
        self.session.project.timebase.ticks_per_second
    }

    fn fps(&self) -> f64 {
        self.session.project.video.fps()
    }

    fn frame_ticks(&self) -> Ticks {
        self.session.project.timebase.frame_ticks(self.fps())
    }

    fn duration_secs(&self) -> f64 {
        self.session.project.duration().to_seconds_f64(self.tps())
    }

    fn snap_opts(&self) -> SnapOptions {
        let mut o = SnapOptions::for_timebase(self.tps());
        o.enabled = self.snap;
        o
    }

    fn playhead_ticks(&self) -> Ticks {
        Ticks::from_seconds(self.playhead, self.tps())
    }

    fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        self.toast = Some((msg.into(), now));
    }

    fn exec(&mut self, ctx: &egui::Context, op: EditOp) {
        let label = op.label();
        if let Err(e) = self.session.exec(op) {
            self.toast(ctx, format!("{label}: {e}"));
        }
    }

    /// Declared source duration of a media asset, in project ticks.
    fn media_duration(&self, media: MediaId) -> Option<Ticks> {
        self.project().media(media).and_then(|a| match &a.kind {
            MediaKind::Audio { duration, .. } | MediaKind::Video { duration, .. } => {
                Some(*duration)
            }
            MediaKind::Image { .. } => None,
        })
    }

    // ---- edit verbs ------------------------------------------------------

    fn split_selected(&mut self, ctx: &egui::Context) {
        let Some(id) = self.selected else {
            self.toast(ctx, "split: select an event first");
            return;
        };
        let Some(track) = self.project().locate_event(id) else {
            return;
        };
        match self.session.split_event(track, id, self.playhead_ticks()) {
            Ok(_) => self.toast(ctx, "split event"),
            Err(e) => self.toast(ctx, format!("split: {e}")),
        }
    }

    fn delete_selected(&mut self, ctx: &egui::Context) {
        let Some(id) = self.selected else {
            self.toast(ctx, "delete: select an event first");
            return;
        };
        let Some(track) = self.project().locate_event(id) else {
            return;
        };
        let Some(removed) = self.project().event(id).cloned() else {
            return;
        };
        let shifts: Vec<(EventId, Ticks)> = if self.ripple {
            self.project()
                .track(track)
                .map(|t| {
                    t.events
                        .iter()
                        .filter(|e| e.start >= removed.end())
                        .map(|e| (e.id, e.start))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.selected = None;
        self.exec(
            ctx,
            EditOp::RippleDelete {
                track,
                removed,
                shifts,
            },
        );
    }

    fn add_track(&mut self, ctx: &egui::Context, kind: TrackKind) {
        let n_video = self
            .project()
            .tracks
            .iter()
            .filter(|t| t.is_video())
            .count();
        let n_audio = self.project().tracks.len() - n_video;
        let (name, index) = match kind {
            TrackKind::Video(_) => (format!("Video {}", n_video + 1), Some(0)),
            TrackKind::Audio(_) => (format!("Audio {}", n_audio + 1), None),
        };
        match self.session.add_track(&name, kind, index) {
            Ok(_) => self.toast(ctx, format!("added track {name}")),
            Err(e) => self.toast(ctx, format!("add track: {e}")),
        }
    }

    /// Imports one dropped file into the media pool by probing it.
    fn import_file(&mut self, ctx: &egui::Context, path: &Path) {
        let tps = self.tps();
        // 1) audio through the backend chain (pure-Rust WAV, then ffmpeg)?
        let audio = opentrack_engine::default_backends().iter().find_map(|b| {
            b.probe(path).ok().map(|info| {
                let a = info.first_audio();
                MediaKind::Audio {
                    channels: a.map(|(c, _)| c).unwrap_or(2),
                    sample_rate: a.map(|(_, r)| r).unwrap_or(48_000),
                    duration: Ticks::from_seconds(info.duration_secs.unwrap_or(0.0), tps),
                }
            })
        });
        let kind = audio.or_else(|| {
            // 2) video through the pure-Rust MP4/H.264 probe?
            VideoDecoder::open(path).ok().map(|dec| {
                let v = dec.info().clone();
                let fps = if v.fps > 0.5 { v.fps } else { 30.0 };
                MediaKind::Video {
                    width: v.width,
                    height: v.height,
                    fps_num: (fps * 1000.0).round() as u32,
                    fps_den: 1000,
                    duration: Ticks::from_seconds(v.duration_secs, tps),
                    has_audio: v.has_audio,
                }
            })
        });
        let kind = kind.or_else(|| {
            // 3) still image? (decode lands in M3; pool entry only)
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            matches!(
                ext.as_deref(),
                Some("png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp")
            )
            .then_some(MediaKind::Image {
                width: 0,
                height: 0,
            })
        });
        match kind {
            Some(kind) => {
                let label = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "media".into());
                let id =
                    self.session
                        .project
                        .add_media(path.to_path_buf(), Some(label.clone()), kind);
                self.pool_selection = Some(id);
                self.toast(ctx, format!("imported {label}"));
            }
            None => self.toast(ctx, format!("unrecognized media: {}", path.display())),
        }
    }

    /// Places a pool asset at the (snapped) playhead, creating a matching
    /// track when none exists.
    fn place_asset(&mut self, ctx: &egui::Context, media: MediaId) {
        let Some(asset) = self.project().media(media).cloned() else {
            return;
        };
        let tps = self.tps();
        let frame = self.frame_ticks();
        let want_video = matches!(
            asset.kind,
            MediaKind::Video { .. } | MediaKind::Image { .. }
        );
        let duration = match &asset.kind {
            MediaKind::Audio { duration, .. } | MediaKind::Video { duration, .. } => {
                (*duration).max(frame)
            }
            MediaKind::Image { .. } => Ticks::from_seconds(5.0, tps),
        };
        let kind = if want_video {
            TrackKind::Video(Default::default())
        } else {
            TrackKind::Audio(Default::default())
        };
        let found = self
            .project()
            .tracks
            .iter()
            .find(|t| t.is_video() == want_video)
            .map(|t| t.id);
        let track = match found {
            Some(id) => id,
            None => match self.session.add_track(
                if want_video { "Video 1" } else { "Audio 1" },
                kind,
                None,
            ) {
                Ok(id) => id,
                Err(e) => {
                    self.toast(ctx, format!("place: {e}"));
                    return;
                }
            },
        };
        let start = snap_time(
            self.project(),
            self.playhead_ticks(),
            None,
            &self.snap_opts(),
        );
        match self.session.add_event(
            track,
            media,
            start,
            duration,
            Ticks::ZERO,
            0.0,
            Ticks::ZERO,
            Ticks::ZERO,
            None,
        ) {
            Ok(id) => {
                self.selected = Some(id);
                self.toast(ctx, format!("placed {}", asset.label));
            }
            Err(e) => self.toast(ctx, format!("place: {e}")),
        }
    }

    // ---- global input ----------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let keys = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::Z),
                (i.modifiers.command || i.modifiers.ctrl) && i.key_pressed(egui::Key::Y),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::Delete),
                i.key_pressed(egui::Key::Home),
                i.key_pressed(egui::Key::End),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
            )
        });
        let (undo, redo, space, s, del, home, end, left, right, zin, zout) = keys;
        if undo {
            match self.session.undo() {
                Ok(Some(label)) => self.toast(ctx, format!("undo: {label}")),
                Ok(None) => {}
                Err(e) => self.toast(ctx, format!("undo: {e}")),
            }
        }
        if redo {
            match self.session.redo() {
                Ok(Some(label)) => self.toast(ctx, format!("redo: {label}")),
                Ok(None) => {}
                Err(e) => self.toast(ctx, format!("redo: {e}")),
            }
        }
        if space {
            self.playing = !self.playing;
        }
        if s {
            self.split_selected(ctx);
        }
        if del {
            self.delete_selected(ctx);
        }
        let frame = self.frame_ticks().to_seconds_f64(self.tps());
        if home {
            self.playhead = 0.0;
        }
        if end {
            self.playhead = self.duration_secs();
        }
        if left {
            self.playhead = (self.playhead - frame).max(0.0);
        }
        if right {
            self.playhead += frame;
        }
        if zin {
            self.zoom = (self.zoom * 1.25).min(400.0);
        }
        if zout {
            self.zoom = (self.zoom / 1.25).max(10.0);
        }
    }

    /// Ctrl+wheel zoom (egui reports pinch/ctrl+wheel as `zoom_delta`).
    fn handle_zoom_wheel(&mut self, ctx: &egui::Context) {
        let z = ctx.input(|i| i.zoom_delta());
        if (z - 1.0).abs() > f32::EPSILON {
            self.zoom = (self.zoom * z).clamp(10.0, 400.0);
        }
    }

    /// Drag & drop file import.
    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for p in dropped {
            self.import_file(ctx, &p);
        }
    }

    // ---- video preview ---------------------------------------------------

    /// The video event under the playhead: (media, label, source seconds).
    fn video_target(&self) -> Option<(MediaId, String, f64)> {
        let tps = self.tps();
        let ph = self.playhead_ticks();
        for t in self.project().tracks.iter().filter(|t| t.is_video()) {
            for ev in &t.events {
                if ev.covers(ph) {
                    let rel = Ticks(ph.0 - ev.start.0);
                    let src = Ticks(ev.media_in.0 + rel.0).to_seconds_f64(tps);
                    return Some((ev.media, ev.label.clone(), src));
                }
            }
        }
        None
    }

    fn sync_preview(&mut self, ctx: &egui::Context) {
        let target = self.video_target();
        let media = target.as_ref().map(|(m, _, _)| *m);

        // Drop the pane when no video event is under the playhead.
        if media.is_none() {
            self.preview = None;
            return;
        }

        // Re-open the decoder when the media under the playhead changed.
        let reopen = !matches!(&self.preview, Some(p) if Some(p.media) == media);
        if reopen {
            self.preview = None;
            let asset = media.and_then(|m| self.project().media(m).cloned());
            if let (Some((_, label, _)), Some(asset)) = (target.clone(), asset) {
                match VideoDecoder::open(&asset.path) {
                    Ok(dec) => {
                        let tex = ctx.load_texture(
                            "video_preview",
                            egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]),
                            egui::TextureOptions::LINEAR,
                        );
                        self.preview = Some(PreviewPane {
                            media: asset.id,
                            label,
                            dec,
                            tex,
                            tex_key: String::new(),
                            err: None,
                        });
                    }
                    Err(e) => {
                        if let Some(p) = &mut self.preview {
                            p.err = Some(format!("open: {e}"));
                        }
                    }
                }
            }
        }

        if let (Some(p), Some((_, _, src))) = (&mut self.preview, target) {
            // Quantize the source time to whole source frames so the decoder
            // cache hits while scrubbing.
            let info = p.dec.info();
            let fps = if info.fps > 0.5 { info.fps } else { 30.0 };
            let q = (src * fps).round() / fps;
            let key = format!("{:.3}", q);
            if p.tex_key != key {
                match p.dec.decode_at(q) {
                    Ok(frame) => {
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        );
                        p.tex.set(img, egui::TextureOptions::LINEAR);
                        p.tex_key = key;
                        p.err = None;
                    }
                    Err(e) => p.err = Some(format!("decode: {e}")),
                }
            }
        }
    }

    // ---- timeline interaction application ---------------------------------

    fn apply_tl(&mut self, ctx: &egui::Context, e: TlEvent) {
        match e {
            TlEvent::Select(id) => self.selected = id,
            TlEvent::BeginDrag {
                event,
                track,
                kind,
                grab_ticks,
            } => {
                let Some(ev) = self.project().event(event).cloned() else {
                    return;
                };
                let mut d = DragState::new(event, track, ev.start, ev.length, ev.media_in);
                d.kind = kind;
                d.grab_ticks = grab_ticks;
                self.drag = Some(d);
            }
            TlEvent::UpdateDrag { pointer_ticks } => {
                let Some(mut d) = self.drag.clone() else {
                    return;
                };
                let frame = self.frame_ticks();
                let media_dur = self.media_duration_of_drag(&d);
                let opts = self.snap_opts();
                match d.kind {
                    DragKind::Move => {
                        let cand = Ticks((pointer_ticks - d.grab_ticks).max(0));
                        let end_cand = Ticks(cand.0 + d.orig_length.0);
                        let s1 = snap_time(self.project(), cand, Some(d.event), &opts);
                        let s2 = Ticks(
                            snap_time(self.project(), end_cand, Some(d.event), &opts)
                                .0
                                .saturating_sub(d.orig_length.0)
                                .max(0),
                        );
                        d.preview_start = if (s1.0 - cand.0).abs() <= (s2.0 - cand.0).abs() {
                            s1
                        } else {
                            s2
                        };
                    }
                    DragKind::TrimLeft => {
                        let min_len = frame.0.max(1);
                        let mut s = snap_time(
                            self.project(),
                            Ticks(pointer_ticks.max(0)),
                            Some(d.event),
                            &opts,
                        );
                        // Never move the head past the media start…
                        let min_start = d.orig_start.0 - d.orig_media_in.0;
                        // …nor collapse the event below one frame.
                        if s.0 > d.orig_start.0 + d.orig_length.0 - min_len {
                            s = Ticks(d.orig_start.0 + d.orig_length.0 - min_len);
                        }
                        s = Ticks(s.0.max(min_start));
                        d.preview_start = s;
                        d.preview_length = Ticks(d.orig_start.0 + d.orig_length.0 - s.0);
                        d.preview_media_in = Ticks(d.orig_media_in.0 + (s.0 - d.orig_start.0));
                    }
                    DragKind::TrimRight => {
                        let min_len = frame.0.max(1);
                        let mut e = snap_time(
                            self.project(),
                            Ticks(pointer_ticks.max(0)),
                            Some(d.event),
                            &opts,
                        );
                        if e.0 < d.orig_start.0 + min_len {
                            e = Ticks(d.orig_start.0 + min_len);
                        }
                        if let Some(dur) = media_dur {
                            let max_end = d.orig_start.0 + (dur.0 - d.orig_media_in.0);
                            if e.0 > max_end {
                                e = Ticks(max_end);
                            }
                        }
                        d.preview_length = Ticks((e.0 - d.orig_start.0).max(min_len));
                    }
                    DragKind::Slip => {
                        let delta = pointer_ticks - d.grab_ticks;
                        let mut mi = d.orig_media_in.0 - delta;
                        if let Some(dur) = media_dur {
                            mi = mi.clamp(0, (dur.0 - d.orig_length.0).max(0));
                        } else {
                            mi = mi.max(0);
                        }
                        d.preview_media_in = Ticks(mi);
                    }
                }
                self.drag = Some(d);
            }
            TlEvent::EndDrag => {
                let Some(d) = self.drag.take() else { return };
                let changed_start = d.preview_start != d.orig_start;
                let changed_len = d.preview_length != d.orig_length;
                let changed_in = d.preview_media_in != d.orig_media_in;
                match d.kind {
                    DragKind::Move => {
                        if changed_start {
                            self.exec(
                                ctx,
                                EditOp::MoveEvent {
                                    track: d.track,
                                    event: d.event,
                                    old_start: d.orig_start,
                                    new_start: d.preview_start,
                                    old_media_in: d.orig_media_in,
                                    new_media_in: d.orig_media_in,
                                },
                            );
                        }
                    }
                    DragKind::TrimLeft | DragKind::TrimRight => {
                        let (ns, ne) = match d.kind {
                            DragKind::TrimLeft => (Some(d.preview_start), None),
                            DragKind::TrimRight => {
                                (None, Some(Ticks(d.orig_start.0 + d.preview_length.0)))
                            }
                            DragKind::Move | DragKind::Slip => unreachable!(),
                        };
                        if (changed_start || changed_len)
                            && let Err(e) = self.session.trim_event(
                                d.track,
                                d.event,
                                ns,
                                ne,
                                Some(&self.snap_opts()),
                            )
                        {
                            self.toast(ctx, format!("trim: {e}"));
                        }
                    }
                    DragKind::Slip => {
                        if changed_in {
                            self.exec(
                                ctx,
                                EditOp::SlipEvent {
                                    track: d.track,
                                    event: d.event,
                                    old_media_in: d.orig_media_in,
                                    new_media_in: d.preview_media_in,
                                },
                            );
                        }
                    }
                }
            }
            TlEvent::Scrub(secs) => {
                self.playhead = secs.max(0.0);
            }
            TlEvent::ToggleMute(track) => {
                self.mutate_audio_props(ctx, track, |p| p.mute = !p.mute);
            }
            TlEvent::ToggleSolo(track) => {
                self.mutate_audio_props(ctx, track, |p| p.solo = !p.solo);
            }
            TlEvent::CommitGain(track, gain) => {
                self.mutate_audio_props(ctx, track, |p| p.gain_db = gain);
            }
            TlEvent::CommitPan(track, pan) => {
                self.mutate_audio_props(ctx, track, |p| p.pan = pan);
            }
        }
    }

    fn media_duration_of_drag(&self, d: &DragState) -> Option<Ticks> {
        self.project()
            .locate_event(d.event)
            .and_then(|tid| self.project().track(tid))
            .and_then(|t| t.events.iter().find(|e| e.id == d.event))
            .and_then(|e| self.media_duration(e.media))
    }

    fn mutate_audio_props(
        &mut self,
        ctx: &egui::Context,
        track: TrackId,
        f: impl FnOnce(&mut AudioTrackProps),
    ) {
        let Some(t) = self.project().track(track) else {
            return;
        };
        let TrackKind::Audio(old) = t.kind.clone() else {
            return;
        };
        let mut new = old.clone();
        f(&mut new);
        self.exec(ctx, EditOp::SetAudioTrackProps { track, old, new });
    }
}

impl eframe::App for OpenTrackApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Playhead animation (audio output is M2; the preview advances).
        let now = ctx.input(|i| i.time);
        if self.playing {
            if let Some(prev) = self.last_frame_time {
                self.playhead += now - prev;
            }
            let end = self.duration_secs();
            if self.playhead >= end {
                self.playhead = 0.0;
                self.playing = false;
            }
            ctx.request_repaint();
        }
        self.last_frame_time = Some(now);

        self.handle_keys(&ctx);
        self.handle_zoom_wheel(&ctx);
        self.handle_dropped_files(&ctx);
        self.sync_preview(&ctx);

        self.draw_menu(ui);
        self.draw_right_panel(ui, &ctx);
        self.draw_timeline(ui, &ctx);
        self.draw_status(ui, &ctx);
        self.draw_modal(&ctx);
    }
}

impl OpenTrackApp {
    fn draw_menu(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("menu").show(ui, |ui| {
            egui::menu::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() {
                        self.session = Session::new(Project::new("Untitled"));
                        self.project_path = None;
                        self.selected = None;
                        self.preview = None;
                        ui.close();
                    }
                    if ui.button("Open .otp...").clicked() {
                        self.modal = Some(Modal::OpenPath(String::new()));
                        ui.close();
                    }
                    if ui.button("Save").clicked() {
                        if let Some(p) = self.project_path.clone() {
                            match save::save(&self.session.project, &p) {
                                Ok(()) => self.toast(ui.ctx(), format!("saved {}", p.display())),
                                Err(e) => self.toast(ui.ctx(), format!("save failed: {e}")),
                            }
                        } else {
                            self.modal =
                                Some(Modal::SavePath(self.project().name.clone() + ".otp"));
                        }
                        ui.close();
                    }
                    if ui.button("Save As...").clicked() {
                        self.modal = Some(Modal::SavePath(self.project().name.clone() + ".otp"));
                        ui.close();
                    }
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let can_undo = self.session.history.can_undo();
                    let can_redo = self.session.history.can_redo();
                    if ui
                        .add_enabled(can_undo, egui::Button::new("Undo  Ctrl+Z"))
                        .clicked()
                    {
                        if let Ok(Some(label)) = self.session.undo() {
                            let c = ui.ctx().clone();
                            self.toast(&c, format!("undo: {label}"));
                        }
                        ui.close();
                    }
                    if ui
                        .add_enabled(can_redo, egui::Button::new("Redo  Ctrl+Y"))
                        .clicked()
                    {
                        if let Ok(Some(label)) = self.session.redo() {
                            let c = ui.ctx().clone();
                            self.toast(&c, format!("redo: {label}"));
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Split at Playhead  (S)").clicked() {
                        let c = ui.ctx().clone();
                        self.split_selected(&c);
                        ui.close();
                    }
                    let del_label = if self.ripple {
                        "Ripple Delete  (Del)"
                    } else {
                        "Delete (gap stays)  (Del)"
                    };
                    if ui.button(del_label).clicked() {
                        let c = ui.ctx().clone();
                        self.delete_selected(&c);
                        ui.close();
                    }
                });
                ui.menu_button("Insert", |ui| {
                    if ui.button("Video Track (top)").clicked() {
                        let c = ui.ctx().clone();
                        self.add_track(&c, TrackKind::Video(Default::default()));
                        ui.close();
                    }
                    if ui.button("Audio Track (bottom)").clicked() {
                        let c = ui.ctx().clone();
                        self.add_track(&c, TrackKind::Audio(Default::default()));
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About OpenTrack").clicked() {
                        self.modal = Some(Modal::About);
                        ui.close();
                    }
                });
            });
        });
    }

    fn draw_right_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut place: Option<MediaId> = None;
        egui::Panel::right("right_panel")
            .resizable(true)
            .default_size(250.0)
            .show(ui, |ui| {
                ui.heading("Preview");
                ui.separator();
                self.draw_preview_body(ui);
                ui.add_space(6.0);
                ui.heading("Media Pool");
                ui.separator();
                place = self.draw_pool_body(ui);
            });
        if let Some(m) = place {
            self.place_asset(ctx, m);
        }
    }

    fn draw_preview_body(&mut self, ui: &mut egui::Ui) {
        match &mut self.preview {
            Some(p) => {
                let avail = ui.available_width();
                if let Some(err) = &p.err {
                    ui.colored_label(egui::Color32::from_rgb(230, 120, 120), err.clone());
                }
                ui.add(
                    egui::Image::new(&p.tex)
                        .max_size(egui::vec2(avail, PREVIEW_MAX_H))
                        .shrink_to_fit(),
                );
                let src = p.tex_key.parse::<f64>().unwrap_or(0.0) / 1000.0;
                ui.small(format!(
                    "{} | source {src:.2}s | {} {}x{}",
                    p.label,
                    p.dec.info().codec,
                    p.dec.info().width,
                    p.dec.info().height
                ));
            }
            None => {
                ui.label("No video under the playhead.");
                ui.small("Drop an .mp4/.mov (H.264) file anywhere to import,");
                ui.small("place it on a video track, then scrub the ruler.");
            }
        }
    }

    /// Media pool; returns a "place at playhead" request if one was clicked.
    fn draw_pool_body(&mut self, ui: &mut egui::Ui) -> Option<MediaId> {
        let assets: Vec<_> = self.session.project.media.assets.clone();
        let mut place = None;
        if assets.is_empty() {
            ui.label("No media yet.");
            ui.small("Drag & drop WAV / MP4 / images anywhere.");
        }
        egui::ScrollArea::vertical()
            .id_salt("pool_scroll")
            .show(ui, |ui| {
                for a in &assets {
                    ui.horizontal(|ui| {
                        let sel = self.pool_selection == Some(a.id);
                        if ui.selectable_label(sel, a.label.clone()).clicked() {
                            self.pool_selection = Some(a.id);
                        }
                        let kind = match &a.kind {
                            MediaKind::Audio { channels, .. } => format!("audio {channels}ch"),
                            MediaKind::Video {
                                width,
                                height,
                                has_audio,
                                ..
                            } => {
                                format!(
                                    "video {width}x{height}{}",
                                    if *has_audio { "+a" } else { "" }
                                )
                            }
                            MediaKind::Image { .. } => "image".to_string(),
                        };
                        ui.small(kind);
                    });
                    ui.monospace(
                        a.path
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| a.path.display().to_string()),
                    );
                    if ui.small_button("Place at playhead").clicked() {
                        place = Some(a.id);
                    }
                    ui.separator();
                }
            });
        place
    }

    fn draw_timeline(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut tl: Vec<TlEvent> = Vec::new();
        let mut hover_edge = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let tps = self.tps();
                let dur = self.duration_secs();
                let timeline_secs = (dur + 8.0).max(30.0);
                let lanes_w = timeline_secs as f32 * self.zoom;
                let content_h = RULER_H + self.project().tracks.len() as f32 * ROW_H + 60.0;

                egui::ScrollArea::vertical()
                    .id_salt("timeline_v")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // ---- track header column (fixed on h-scroll) ----
                            ui.vertical(|ui| {
                                ui.set_width(HEADER_W);
                                ui.allocate_space(egui::vec2(HEADER_W, RULER_H));
                                for track in &self.session.project.tracks {
                                    ui.horizontal(|ui| {
                                        ui.set_min_height(ROW_H - 2.0);
                                        ui.set_width(HEADER_W - 8.0);
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(track.name.clone()).size(11.0),
                                            );
                                            match &track.kind {
                                                TrackKind::Audio(props) => {
                                                    ui.horizontal(|ui| {
                                                        let mut gain = self
                                                            .gain_drag
                                                            .get(&track.id)
                                                            .copied()
                                                            .unwrap_or(props.gain_db);
                                                        let gr = ui.add_sized(
                                                            [70.0, 14.0],
                                                            egui::Slider::new(
                                                                &mut gain,
                                                                -60.0..=12.0,
                                                            )
                                                            .custom_formatter(|v, _| {
                                                                format!("{v:+.1}dB")
                                                            }),
                                                        );
                                                        if gr.changed() {
                                                            self.gain_drag.insert(track.id, gain);
                                                        }
                                                        if gr.drag_stopped() {
                                                            self.gain_drag.remove(&track.id);
                                                            tl.push(TlEvent::CommitGain(
                                                                track.id, gain,
                                                            ));
                                                        }
                                                        let mut pan = self
                                                            .pan_drag
                                                            .get(&track.id)
                                                            .copied()
                                                            .unwrap_or(props.pan);
                                                        let pr = ui.add_sized(
                                                            [52.0, 14.0],
                                                            egui::Slider::new(&mut pan, -1.0..=1.0)
                                                                .step_by(0.05),
                                                        );
                                                        if pr.changed() {
                                                            self.pan_drag.insert(track.id, pan);
                                                        }
                                                        if pr.drag_stopped() {
                                                            self.pan_drag.remove(&track.id);
                                                            tl.push(TlEvent::CommitPan(
                                                                track.id, pan,
                                                            ));
                                                        }
                                                    });
                                                    ui.horizontal(|ui| {
                                                        let m_fill = if props.mute {
                                                            egui::Color32::from_rgb(200, 70, 70)
                                                        } else {
                                                            egui::Color32::from_rgb(60, 60, 66)
                                                        };
                                                        if ui
                                                            .add_sized(
                                                                [16.0, 14.0],
                                                                egui::Button::new("M").fill(m_fill),
                                                            )
                                                            .clicked()
                                                        {
                                                            tl.push(TlEvent::ToggleMute(track.id));
                                                        }
                                                        let s_fill = if props.solo {
                                                            egui::Color32::from_rgb(220, 190, 70)
                                                        } else {
                                                            egui::Color32::from_rgb(60, 60, 66)
                                                        };
                                                        if ui
                                                            .add_sized(
                                                                [16.0, 14.0],
                                                                egui::Button::new("S").fill(s_fill),
                                                            )
                                                            .clicked()
                                                        {
                                                            tl.push(TlEvent::ToggleSolo(track.id));
                                                        }
                                                        ui.small(format!(
                                                            "{:+.1}dB / {:+.2}",
                                                            props.gain_db, props.pan
                                                        ));
                                                    });
                                                }
                                                TrackKind::Video(_) => {
                                                    ui.small("video");
                                                }
                                            }
                                        });
                                    });
                                    ui.add_space(2.0);
                                }
                            });

                            // ---- lanes (horizontal scroll) ----
                            egui::ScrollArea::horizontal()
                                .id_salt("timeline_h")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let (response, painter) = ui.allocate_painter(
                                        egui::vec2(lanes_w, content_h),
                                        egui::Sense::click(),
                                    );
                                    if response.clicked() {
                                        tl.push(TlEvent::Select(None));
                                    }
                                    let o = response.rect.min;

                                    // Ruler (painted, then interactive)
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(o, egui::vec2(lanes_w, RULER_H)),
                                        0.0,
                                        egui::Color32::from_rgb(28, 30, 34),
                                    );
                                    let step: f64 = if self.zoom >= 70.0 {
                                        1.0
                                    } else if self.zoom >= 35.0 {
                                        2.0
                                    } else if self.zoom >= 14.0 {
                                        5.0
                                    } else {
                                        10.0
                                    };
                                    let mut t = 0.0f64;
                                    while t < timeline_secs {
                                        let x = o.x + t as f32 * self.zoom;
                                        painter.line_segment(
                                            [
                                                egui::pos2(x, RULER_H * 0.55 + o.y),
                                                egui::pos2(x, RULER_H + o.y),
                                            ],
                                            egui::Stroke::new(1.0, egui::Color32::GRAY),
                                        );
                                        painter.text(
                                            egui::pos2(x + 3.0, o.y + 4.0),
                                            egui::Align2::LEFT_TOP,
                                            format_time(t),
                                            egui::FontId::monospace(11.0),
                                            egui::Color32::from_rgb(190, 190, 190),
                                        );
                                        t += step;
                                    }
                                    let ruler_resp = ui.allocate_rect(
                                        egui::Rect::from_min_size(o, egui::vec2(lanes_w, RULER_H)),
                                        egui::Sense::click_and_drag(),
                                    );
                                    if (ruler_resp.dragged() || ruler_resp.clicked())
                                        && let Some(p) = ruler_resp.interact_pointer_pos()
                                    {
                                        let secs = ((p.x - o.x) / self.zoom).max(0.0) as f64;
                                        tl.push(TlEvent::Scrub(secs));
                                    }

                                    // Tracks + events
                                    for (row, track) in
                                        self.session.project.tracks.iter().enumerate()
                                    {
                                        let y = o.y + RULER_H + row as f32 * ROW_H;
                                        let row_rect = egui::Rect::from_min_size(
                                            egui::pos2(o.x, y),
                                            egui::vec2(lanes_w, ROW_H - 2.0),
                                        );
                                        let is_video = track.is_video();
                                        painter.rect_filled(
                                            row_rect,
                                            2.0,
                                            if is_video {
                                                COL_VIDEO_TRACK
                                            } else {
                                                COL_AUDIO_TRACK
                                            },
                                        );
                                        for ev in &track.events {
                                            let dragging = self
                                                .drag
                                                .as_ref()
                                                .is_some_and(|d| d.event == ev.id);
                                            let (ex, ew, alpha, label_suffix) = if dragging {
                                                let d = self.drag.as_ref().unwrap();
                                                (
                                                    o.x + d.preview_start.to_seconds_f64(tps)
                                                        as f32
                                                        * self.zoom,
                                                    (d.preview_length.to_seconds_f64(tps) as f32
                                                        * self.zoom)
                                                        .max(4.0),
                                                    1.0_f32,
                                                    match d.kind {
                                                        DragKind::Slip => format!(
                                                            "  (slip {:+.2}s)",
                                                            (d.preview_media_in.0
                                                                - d.orig_media_in.0)
                                                                as f64
                                                                / tps as f64
                                                        ),
                                                        _ => String::new(),
                                                    },
                                                )
                                            } else {
                                                (
                                                    o.x + ev.start.to_seconds_f64(tps) as f32
                                                        * self.zoom,
                                                    (ev.length.to_seconds_f64(tps) as f32
                                                        * self.zoom)
                                                        .max(4.0),
                                                    if self.drag.is_some() { 0.35 } else { 1.0 },
                                                    String::new(),
                                                )
                                            };
                                            let er = egui::Rect::from_min_size(
                                                egui::pos2(ex, y + 4.0),
                                                egui::vec2(ew, ROW_H - 10.0),
                                            );
                                            let base = if self.selected == Some(ev.id) && !dragging
                                            {
                                                COL_EVENT_SEL
                                            } else if is_video {
                                                COL_EVENT_VIDEO
                                            } else {
                                                COL_EVENT_AUDIO
                                            };
                                            let fill = if dragging {
                                                col_ghost()
                                            } else {
                                                base.gamma_multiply(alpha)
                                            };
                                            painter.rect_filled(er, 3.0, fill);
                                            painter.rect_stroke(
                                                er,
                                                3.0,
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_black_alpha(120),
                                                ),
                                                egui::epaint::StrokeKind::Inside,
                                            );
                                            // fade ramps
                                            let fi =
                                                ev.fade_in.to_seconds_f64(tps) as f32 * self.zoom;
                                            let fo =
                                                ev.fade_out.to_seconds_f64(tps) as f32 * self.zoom;
                                            if fi > 1.0 {
                                                painter.line_segment(
                                                    [
                                                        er.left_bottom(),
                                                        egui::pos2(
                                                            er.left() + fi.min(ew),
                                                            er.top(),
                                                        ),
                                                    ],
                                                    egui::Stroke::new(
                                                        1.5,
                                                        egui::Color32::from_white_alpha(200),
                                                    ),
                                                );
                                            }
                                            if fo > 1.0 {
                                                painter.line_segment(
                                                    [
                                                        er.right_top(),
                                                        egui::pos2(
                                                            er.right() - fo.min(ew),
                                                            er.bottom(),
                                                        ),
                                                    ],
                                                    egui::Stroke::new(
                                                        1.5,
                                                        egui::Color32::from_white_alpha(200),
                                                    ),
                                                );
                                            }
                                            painter.text(
                                                egui::pos2(er.left() + 6.0, er.center().y),
                                                egui::Align2::LEFT_CENTER,
                                                format!("{}{label_suffix}", ev.label),
                                                egui::FontId::proportional(11.0),
                                                egui::Color32::from_black_alpha(230),
                                            );
                                            if !ev.takes.is_empty() {
                                                painter.text(
                                                    egui::pos2(er.right() - 4.0, er.center().y),
                                                    egui::Align2::RIGHT_CENTER,
                                                    format!("takes {}", ev.takes.len() + 1),
                                                    egui::FontId::proportional(9.0),
                                                    egui::Color32::from_black_alpha(180),
                                                );
                                            }

                                            // interaction
                                            let hit =
                                                ui.allocate_rect(er, egui::Sense::click_and_drag());
                                            if hit.drag_started() {
                                                tl.push(TlEvent::Select(Some(ev.id)));
                                                let kind = hit
                                                    .interact_pointer_pos()
                                                    .and_then(|p| {
                                                        let px = p.x;
                                                        if (px - er.left()).abs() <= EDGE_PX
                                                            && ew > 3.0 * EDGE_PX
                                                        {
                                                            Some(DragKind::TrimLeft)
                                                        } else if (px - er.right()).abs() <= EDGE_PX
                                                            && ew > 3.0 * EDGE_PX
                                                        {
                                                            Some(DragKind::TrimRight)
                                                        } else {
                                                            None
                                                        }
                                                    })
                                                    .unwrap_or_else(|| {
                                                        if ui.input(|i| i.modifiers.alt) {
                                                            DragKind::Slip
                                                        } else {
                                                            DragKind::Move
                                                        }
                                                    });
                                                let grab = hit
                                                    .interact_pointer_pos()
                                                    .map(|p| {
                                                        let secs = (p.x - o.x).max(0.0) as f64
                                                            / self.zoom as f64;
                                                        Ticks::from_seconds(secs, tps).0
                                                    })
                                                    .unwrap_or(0);
                                                tl.push(TlEvent::BeginDrag {
                                                    event: ev.id,
                                                    track: track.id,
                                                    kind,
                                                    grab_ticks: grab,
                                                });
                                            } else if hit.dragged()
                                                && self
                                                    .drag
                                                    .as_ref()
                                                    .is_some_and(|d| d.event == ev.id)
                                            {
                                                if let Some(p) = hit.interact_pointer_pos() {
                                                    let secs = (p.x - o.x).max(0.0) as f64
                                                        / self.zoom as f64;
                                                    tl.push(TlEvent::UpdateDrag {
                                                        pointer_ticks: Ticks::from_seconds(
                                                            secs, tps,
                                                        )
                                                        .0,
                                                    });
                                                }
                                            } else if hit.drag_stopped()
                                                && self
                                                    .drag
                                                    .as_ref()
                                                    .is_some_and(|d| d.event == ev.id)
                                            {
                                                tl.push(TlEvent::EndDrag);
                                            } else if hit.clicked() && !dragging {
                                                tl.push(TlEvent::Select(Some(ev.id)));
                                            }
                                            if (hit.hovered() && ew > 3.0 * EDGE_PX)
                                                && let Some(p) = hit.interact_pointer_pos()
                                                && ((p.x - er.left()).abs() <= EDGE_PX
                                                    || (p.x - er.right()).abs() <= EDGE_PX)
                                            {
                                                hover_edge = true;
                                            }
                                        }
                                    }

                                    // Playhead
                                    if self.playhead <= timeline_secs {
                                        let px = o.x + self.playhead as f32 * self.zoom;
                                        painter.line_segment(
                                            [egui::pos2(px, o.y), egui::pos2(px, o.y + content_h)],
                                            egui::Stroke::new(2.0, COL_PLAYHEAD),
                                        );
                                    }
                                });
                        });
                    });
            });

        for e in tl {
            self.apply_tl(ctx, e);
        }
        if hover_edge {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
    }

    fn draw_status(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let play_label = if self.playing { "Pause" } else { "Play" };
                if ui.button(play_label).clicked() {
                    self.playing = !self.playing;
                }
                ui.label(format_time(self.playhead));
                ui.separator();
                let can_undo = self.session.history.can_undo();
                let can_redo = self.session.history.can_redo();
                if ui
                    .add_enabled(can_undo, egui::Button::new("Undo"))
                    .clicked()
                    && let Ok(Some(label)) = self.session.undo()
                {
                    self.toast(ctx, format!("undo: {label}"));
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("Redo"))
                    .clicked()
                    && let Ok(Some(label)) = self.session.redo()
                {
                    self.toast(ctx, format!("redo: {label}"));
                }
                ui.separator();
                ui.checkbox(&mut self.snap, "Snap");
                ui.checkbox(&mut self.ripple, "Ripple");
                ui.separator();
                let mut master = self.session.project.master.gain_db;
                let mr = ui.add_sized([80.0, 16.0], egui::Slider::new(&mut master, -60.0..=12.0));
                if mr.drag_stopped() {
                    let old = self.session.project.master.clone();
                    let mut new = old.clone();
                    new.gain_db = master;
                    self.exec(ctx, EditOp::SetMaster { old, new });
                }
                ui.label("master");
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut self.zoom, 10.0..=400.0)
                        .text("zoom")
                        .logarithmic(true),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((msg, at)) = &self.toast
                        && ctx.input(|i| i.time) - at < 4.0
                    {
                        ui.small(msg.clone());
                    }
                    ui.colored_label(egui::Color32::from_rgb(150, 200, 150), "M1");
                });
            });
            ui.add_space(4.0);
        });
    }

    fn draw_modal(&mut self, ctx: &egui::Context) {
        match self.modal.clone() {
            Some(Modal::About) => {
                let mut open = true;
                egui::Window::new("About OpenTrack")
                    .open(&mut open)
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.heading("OpenTrack");
                        ui.label(format!("version {}", env!("CARGO_PKG_VERSION")));
                        ui.separator();
                        ui.label("A clean-room, GPL-3.0 NLE inspired by the Vegas workflow,");
                        ui.label("rebuilt from scratch in pure Rust.");
                        ui.label("M1: interactive timeline editing + pure-Rust video preview.");
                        ui.add_space(4.0);
                        ui.small("OpenTrack is not affiliated with, endorsed by, or derived from");
                        ui.small("Sony Creative Software or MAGIX Software GmbH. \"Vegas\" is a");
                        ui.small("trademark of MAGIX; it is referenced here only to describe");
                        ui.small("workflow compatibility, under nominative fair use.");
                        ui.add_space(4.0);
                        ui.hyperlink("https://github.com/apikun/Opentrack");
                    });
                if !open {
                    self.modal = None;
                }
            }
            Some(Modal::OpenPath(_)) => {
                let mut path = match &self.modal {
                    Some(Modal::OpenPath(p)) => p.clone(),
                    _ => String::new(),
                };
                let mut action = false;
                let mut close = false;
                egui::Window::new("Open project")
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Path:");
                            ui.text_edit_singleline(&mut path);
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Load").clicked() {
                                action = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
                if action {
                    match save::load(Path::new(&path)) {
                        Ok(p) => {
                            self.session = Session::new(p);
                            self.project_path = Some(PathBuf::from(&path));
                            self.selected = None;
                            self.preview = None;
                            self.playhead = 0.0;
                            self.toast(ctx, format!("loaded {path}"));
                        }
                        Err(e) => self.toast(ctx, format!("load failed: {e}")),
                    }
                    self.modal = None;
                } else if close {
                    self.modal = None;
                } else {
                    self.modal = Some(Modal::OpenPath(path));
                }
            }
            Some(Modal::SavePath(_)) => {
                let mut path = match &self.modal {
                    Some(Modal::SavePath(p)) => p.clone(),
                    _ => String::new(),
                };
                let mut action = false;
                let mut close = false;
                egui::Window::new("Save project")
                    .collapsible(false)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Path:");
                            ui.text_edit_singleline(&mut path);
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                action = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
                if action {
                    match save::save(&self.session.project, Path::new(&path)) {
                        Ok(()) => {
                            self.project_path = Some(PathBuf::from(&path));
                            self.toast(ctx, format!("saved {path}"));
                        }
                        Err(e) => self.toast(ctx, format!("save failed: {e}")),
                    }
                    self.modal = None;
                } else if close {
                    self.modal = None;
                } else {
                    self.modal = Some(Modal::SavePath(path));
                }
            }
            None => {}
        }
    }
}

fn format_time(secs: f64) -> String {
    let m = (secs / 60.0).floor();
    let s = secs - m * 60.0;
    format!("{:02}:{:05.2}", m, s)
}
