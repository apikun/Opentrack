- RFC: 0005
- Title: GUI shell (egui) layout & interaction plan
- Status: Accepted (M0 shell shipped; editing interactions = M1)

## Summary

The GUI is an egui/eframe application that is a *view over the model*: it
paints `Project` and issues `EditOp`s. M0 ships the shell — timeline
visualization (tracks, events, fades, playhead, zoom), media pool,
open/save, transport-free. M1 adds direct manipulation.

## Motivation

Immediate-mode GUI keeps the state discipline honest: there is no shadow copy
of the timeline to keep in sync — every frame reads the project, and every
interaction produces an op. egui is pure Rust, immediate-mode, GPU-backed,
and ships without a system webview; good fit for a 60 fps timeline canvas.

## Specification (M0 shell)

### Window layout

```
┌────────────────────────────────────────────────────────────┐
│ menu: File(New/Open/Save/SaveAs/Exit) · Help(About)        │
├──────────────┬─────────────────────────────────────────────┤
│ media pool   │ ruler (time marks, click to move playhead)  │
│ (assets,     ├─────────────────────────────────────────────┤
│  label,      │ video tracks (top-down)                     │
│  add button) │ ─────────────────────────────────────────── │
│              │ audio tracks (bottom-up)                    │
├──────────────┴─────────────────────────────────────────────┤
│ status: project name · duration · zoom · hints             │
└────────────────────────────────────────────────────────────┘
```

- Track header column: 150 px (`HEADER_W`); rows 46 px (`ROW_H`); ruler 26 px.
- Track colors: video blue family, audio green family; selected event amber;
  playhead red. Fades drawn as translucent ramps on event corners.
- Zoom: pixels-per-second in a 0.05…400 range; ctrl+wheel; ruler drag =
  playhead scrub. Playhead animates (request_repaint) in the demo session.

### Mutation discipline

- The GUI never mutates `Project` directly (invariant I7). Menus and modal
  dialogs construct `EditOp`s / `Session` verbs (`add_track`, `add_event`,
  `set_master`…). Errors surface as toasts, not panics.
- Open/Save: `core::save` only; `--project PATH` CLI flag loads on start.

### M1 interaction plan (next milestone, same shell)

- Drag event = `MoveEvent` preview while dragging (ghost), commit on release
  through `Session::exec` so undo works naturally.
- Edge handles = `TrimEvent`; alt-drag = `SlipEvent`; S = split at cursor;
  Del = `RippleDelete` (toggle auto-ripple); shift-click = multi-select
  (M1: `EditOp::Batch`).
- Keyboard map kept in one `Keymap` table so docs and UI hints stay truthful.

## Non-goals

- Docking/panels system (revisit if the workflow demands it post-M2).
- Theming beyond the M0 palette; skinning to mimic any product is a
  clean-room non-goal.
- Frame-accurate video preview (M3, RFC 0007).

## Rationale

egui vs. alternatives: GTK-rs bindings drag in C deps (breaks "nothing but
cargo"); winit+wgpu from scratch costs months of widget work; Slint's
declarative model fights the read-project-every-frame loop. egui's cost —
retaining some GUI-side state (selection, drag) outside the model — is
confined to ephemeral view state that never needs persistence.

## Acceptance criteria

- `cargo run -p opentrack-app -- --project opentrack-demo/demo.otp` renders
  the demo session: both tracks, both events, visible fade ramps, moving
  playhead, media pool entries with labels.
- Open/Save round-trip through the GUI produces a `.otp` byte-identical to
  the CLI's save of the same session.
- CI `gui-compile` job builds `opentrack-app` on all three platforms (no
  display needed at build time).
