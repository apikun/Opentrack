//! # opentrack-app — the OpenTrack GUI shell (egui / eframe)
//!
//! M0 blueprint scope: this window demonstrates the shell layout defined in
//! RFC-0005 (menu bar, media pool, track timeline, status bar) and *reads*
//! real `.otp` projects through `opentrack-core`. Interactive editing
//! (drag/split/trim) is the M1 milestone — the panels below are the exact
//! surfaces those interactions will plug into.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console on Windows release

use eframe::egui;
use opentrack_core::save;
use opentrack_core::{EventId, Project};
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
        "OpenTrack - clean-room Rust NLE (blueprint M0)",
        options,
        Box::new(move |_cc| Ok(Box::new(OpenTrackApp::new(project_arg)))),
    )
}

const HEADER_W: f32 = 150.0;
const RULER_H: f32 = 26.0;
const ROW_H: f32 = 46.0;

const COL_VIDEO_TRACK: egui::Color32 = egui::Color32::from_rgb(52, 86, 132);
const COL_AUDIO_TRACK: egui::Color32 = egui::Color32::from_rgb(34, 94, 74);
const COL_EVENT_VIDEO: egui::Color32 = egui::Color32::from_rgb(86, 138, 214);
const COL_EVENT_AUDIO: egui::Color32 = egui::Color32::from_rgb(70, 190, 130);
const COL_EVENT_SEL: egui::Color32 = egui::Color32::from_rgb(255, 200, 80);
const COL_PLAYHEAD: egui::Color32 = egui::Color32::from_rgb(235, 80, 80);

#[derive(Clone, Debug)]
enum Modal {
    About,
    OpenPath(String),
    SavePath(String),
}

struct OpenTrackApp {
    project: Project,
    project_path: Option<PathBuf>,
    zoom: f32,     // pixels per second
    playhead: f64, // seconds
    playing: bool,
    last_frame_time: Option<f64>,
    selected: Option<EventId>,
    modal: Option<Modal>,
    toast: Option<(String, f64)>,
}

impl OpenTrackApp {
    fn new(project_path: Option<PathBuf>) -> Self {
        let project = project_path
            .as_deref()
            .and_then(|p| save::load(p).ok())
            .unwrap_or_else(|| Project::new("Untitled"));
        Self {
            project,
            project_path,
            zoom: 90.0,
            playhead: 0.0,
            playing: false,
            last_frame_time: None,
            selected: None,
            modal: None,
            toast: None,
        }
    }

    fn toast(&mut self, ctx: &egui::Context, msg: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        self.toast = Some((msg.into(), now));
    }

    fn duration_secs(&self) -> f64 {
        self.project
            .duration()
            .to_seconds_f64(self.project.timebase.ticks_per_second)
    }
}

impl eframe::App for OpenTrackApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Playhead animation (no audio output yet — M2 engine work).
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

        self.draw_menu(ui);
        self.draw_media_pool(ui);
        self.draw_timeline(ui);
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
                        self.project = Project::new("Untitled");
                        self.project_path = None;
                        self.selected = None;
                        ui.close();
                    }
                    if ui.button("Open .otp...").clicked() {
                        self.modal = Some(Modal::OpenPath(String::new()));
                        ui.close();
                    }
                    if ui.button("Save").clicked() {
                        let ctx = ui.ctx().clone();
                        if let Some(p) = self.project_path.clone() {
                            match save::save(&self.project, &p) {
                                Ok(()) => self.toast(&ctx, format!("saved {}", p.display())),
                                Err(e) => self.toast(&ctx, format!("save failed: {e}")),
                            }
                        } else {
                            self.modal = Some(Modal::SavePath(self.project.name.clone() + ".otp"));
                        }
                        ui.close();
                    }
                    if ui.button("Save As...").clicked() {
                        self.modal = Some(Modal::SavePath(self.project.name.clone() + ".otp"));
                        ui.close();
                    }
                    if ui.button("Quit").clicked() {
                        std::process::exit(0);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    ui.label("Editing verbs (split / trim / slip / ripple) land in M1.");
                    ui.label("The opentrack-core Session API is already tested (RFC-0002).");
                    ui.close();
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

    fn draw_media_pool(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("media_pool")
            .resizable(true)
            .default_size(230.0)
            .show(ui, |ui| {
                ui.heading("Media Pool");
                ui.separator();
                if self.project.media.assets.is_empty() {
                    ui.label("No media yet.");
                    ui.small("Import lands in M1 (drag & drop).");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for a in &self.project.media.assets {
                        egui::CollapsingHeader::new(format!("{} ({})", a.label, a.id)).show(
                            ui,
                            |ui| {
                                ui.monospace(a.path.display().to_string());
                            },
                        );
                    }
                });
            });
    }

    fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let mut pending_selection: Option<(EventId, String)> = None;
        egui::CentralPanel::default().show(ui, |ui| {
            let tps = self.project.timebase.ticks_per_second;
            let dur = self.duration_secs();
            let timeline_secs = (dur + 8.0).max(30.0);
            let content_w = HEADER_W + timeline_secs as f32 * self.zoom;
            let content_h = RULER_H + self.project.tracks.len() as f32 * ROW_H + 60.0;

            egui::ScrollArea::both()
                .id_salt("timeline_scroll")
                .show(ui, |ui| {
                    let (response, painter) =
                        ui.allocate_painter(egui::vec2(content_w, content_h), egui::Sense::click());
                    if response.clicked() {
                        self.selected = None;
                    }
                    let o = response.rect.min;

                    // Ruler
                    painter.rect_filled(
                        egui::Rect::from_min_size(o, egui::vec2(content_w, RULER_H)),
                        0.0,
                        egui::Color32::from_rgb(28, 30, 34),
                    );
                    let step: f64 = if self.zoom >= 70.0 {
                        1.0
                    } else if self.zoom >= 35.0 {
                        2.0
                    } else {
                        5.0
                    };
                    let mut t = 0.0f64;
                    while t < timeline_secs {
                        let x = o.x + HEADER_W + t as f32 * self.zoom;
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

                    // Tracks (index 0 = top, the Vegas convention)
                    for (row, track) in self.project.tracks.iter().enumerate() {
                        let y = o.y + RULER_H + row as f32 * ROW_H;
                        let row_rect = egui::Rect::from_min_size(
                            egui::pos2(o.x, y),
                            egui::vec2(content_w, ROW_H - 2.0),
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

                        // Track header
                        painter.text(
                            egui::pos2(o.x + 8.0, y + 8.0),
                            egui::Align2::LEFT_TOP,
                            track.name.clone(),
                            egui::FontId::proportional(13.0),
                            egui::Color32::WHITE,
                        );
                        let (badge, badge_col) = match &track.kind {
                            opentrack_core::TrackKind::Audio(p) => {
                                if p.mute {
                                    ("M", egui::Color32::from_rgb(200, 70, 70))
                                } else if p.solo {
                                    ("S", egui::Color32::from_rgb(220, 190, 70))
                                } else {
                                    ("", egui::Color32::TRANSPARENT)
                                }
                            }
                            _ => ("", egui::Color32::TRANSPARENT),
                        };
                        if !badge.is_empty() {
                            painter.text(
                                egui::pos2(o.x + 12.0, y + 28.0),
                                egui::Align2::LEFT_CENTER,
                                badge,
                                egui::FontId::proportional(11.0),
                                badge_col,
                            );
                        }

                        // Events with fade markers
                        for ev in &track.events {
                            let x0 =
                                o.x + HEADER_W + ev.start.to_seconds_f64(tps) as f32 * self.zoom;
                            let w = (ev.length.to_seconds_f64(tps) as f32 * self.zoom).max(4.0);
                            let er = egui::Rect::from_min_size(
                                egui::pos2(x0, y + 4.0),
                                egui::vec2(w, ROW_H - 10.0),
                            );
                            let sel = self.selected == Some(ev.id);
                            painter.rect_filled(
                                er,
                                3.0,
                                if sel {
                                    COL_EVENT_SEL
                                } else if is_video {
                                    COL_EVENT_VIDEO
                                } else {
                                    COL_EVENT_AUDIO
                                },
                            );
                            painter.rect_stroke(
                                er,
                                3.0,
                                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120)),
                                egui::epaint::StrokeKind::Inside,
                            );
                            let fi = ev.fade_in.to_seconds_f64(tps) as f32 * self.zoom;
                            let fo = ev.fade_out.to_seconds_f64(tps) as f32 * self.zoom;
                            if fi > 1.0 {
                                painter.line_segment(
                                    [
                                        er.left_bottom(),
                                        egui::pos2(er.left() + fi.min(w), er.top()),
                                    ],
                                    egui::Stroke::new(1.5, egui::Color32::from_white_alpha(200)),
                                );
                            }
                            if fo > 1.0 {
                                painter.line_segment(
                                    [
                                        er.right_top(),
                                        egui::pos2(er.right() - fo.min(w), er.bottom()),
                                    ],
                                    egui::Stroke::new(1.5, egui::Color32::from_white_alpha(200)),
                                );
                            }
                            painter.text(
                                egui::pos2(er.left() + 6.0, er.center().y),
                                egui::Align2::LEFT_CENTER,
                                ev.label.clone(),
                                egui::FontId::proportional(11.0),
                                egui::Color32::from_black_alpha(230),
                            );

                            let hit = ui.allocate_rect(er, egui::Sense::click());
                            if hit.clicked() {
                                pending_selection = Some((ev.id, ev.label.clone()));
                            }
                        }
                    }

                    // Playhead
                    if self.playhead <= timeline_secs {
                        let px = o.x + HEADER_W + self.playhead as f32 * self.zoom;
                        painter.line_segment(
                            [egui::pos2(px, o.y), egui::pos2(px, o.y + content_h)],
                            egui::Stroke::new(2.0, COL_PLAYHEAD),
                        );
                    }
                });
        });

        if let Some((id, label)) = pending_selection {
            self.selected = Some(id);
            let ctx = ui.ctx().clone();
            self.toast(
                &ctx,
                format!(
                    "selected '{}' - drag/split/trim arrive with M1 (ROADMAP.md)",
                    label
                ),
            );
        }
    }

    fn draw_status(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Play").clicked() {
                    self.playing = true;
                    ctx.request_repaint();
                }
                if ui.button("Pause").clicked() {
                    self.playing = false;
                }
                ui.label(format_time(self.playhead));
                ui.separator();
                ui.label(format!(
                    "{} | {} tracks | {} events | {} Hz | {:.1}s | {}x{} @ {:.2}fps",
                    self.project.name,
                    self.project.tracks.len(),
                    self.project
                        .tracks
                        .iter()
                        .map(|t| t.events.len())
                        .sum::<usize>(),
                    self.project.sample_rate,
                    self.duration_secs(),
                    self.project.video.width,
                    self.project.video.height,
                    self.project.video.fps(),
                ));
                ui.separator();
                ui.add(
                    egui::Slider::new(&mut self.zoom, 10.0..=400.0)
                        .text("zoom px/s")
                        .logarithmic(true),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some((msg, at)) = &self.toast {
                        if ctx.input(|i| i.time) - at < 4.0 {
                            ui.small(msg.clone());
                        }
                    }
                    ui.colored_label(egui::Color32::from_rgb(150, 200, 150), "M0 blueprint");
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
                        ui.label(format!(
                            "version {} - blueprint milestone M0",
                            env!("CARGO_PKG_VERSION")
                        ));
                        ui.separator();
                        ui.label("A clean-room, GPL-3.0 NLE inspired by the Vegas workflow,");
                        ui.label("rebuilt from scratch in pure Rust.");
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
                            self.project = p;
                            self.project_path = Some(PathBuf::from(&path));
                            self.selected = None;
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
                    match save::save(&self.project, Path::new(&path)) {
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
