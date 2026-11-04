# Changelog

All notable changes to OpenTrack are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com); versions are tagged `vX.Y.Z`.

## [0.2.0] — M1: interactive editing + video preview

### Added

- **Interactive timeline editing** (`opentrack-app`):
  - pointer drag to move events (with grid + edge snapping and a live ghost
    preview), drag the left/right edge to trim (media-follows-edge head trims),
    alt-drag to slip content without moving the event;
  - `S` splits the selected event at the playhead, `Delete` ripple-deletes
    (toggleable "Ripple" — when off, the gap stays);
  - track strips are real widgets: gain (dB) and pan sliders with drag-commit,
    Mute/Solo buttons — all through reversible `EditOp`s;
  - master volume slider in the status bar; snap/ripple toggles;
  - full undo/redo (`Ctrl+Z` / `Ctrl+Y`, buttons in Edit menu + status bar);
  - media import by drag & drop anywhere onto the window (WAV / MP4 / images),
    "Place at playhead" from the pool creates the right kind of track when
    none exists;
  - keyboard transport: Space play/pause, Home/End, arrow keys step one frame,
    `+`/`-` and Ctrl+wheel zoom; click/drag the ruler to scrub.
- **Video preview pane**: decodes the video event under the playhead through a
  **pure-Rust H.264 path** — no ffmpeg required:
  - new `opentrack-engine::video`: MP4/MOV demux (craft-media `isobmff`),
    H.264 decode (`filmcraft-h264`), YUV 4:2:0 → RGBA conversion honoring
    BT.601/709 matrix + limited/full range, streaming GOP decoding (O(1)
    amortized during forward playback), frame cache, clean errors;
  - non-H.264 containers probe fine and report codec info; decode reports
    `Unsupported` cleanly.
- **CLI**: `opentrack frame FILE [--at SECS] [-o OUT.bmp]` exports a decoded
  frame as a dependency-free 24-bit BMP; `opentrack probe` now also reports
  MP4/MOV video streams from the pure-Rust demuxer.
- **Docs**: `ATTRIBUTION.md` (upstream licenses), third-party code policy in
  `CLEANROOM.md`, `CHANGELOG.md` (this file).

### Changed

- MSRV is now **1.95** (edition 2024, matching the video dependency stack).
- `opentrack-engine` gained two pinned git dependencies (`craft-media`,
  `filmcraft-h264`), both MIT OR Apache-2.0 — see `ATTRIBUTION.md`.

### Deferred (from the M1 plan, tracked for M1.1)

- Multi-selection / group moves (needs an `EditOp::Batch` compound op),
  cross-track event moves, take switching UI, crossfade drag handles,
  autosave journal, zoom-to-selection.

## [0.1.0] — M0: blueprint milestone

- Timeline data model: unlimited video/audio tracks, events with takes,
  fades, media in-points; integer-tick timebase.
- Reversible `EditOp` set with undo/redo, snapping, overlap/crossfade model.
- Audio buses: dB gain, equal-power pan, mute/solo, master bus.
- Offline audio render → 16-bit PCM WAV with per-channel peaks.
- Codec abstraction: pure-Rust WAV backend + optional ffmpeg bridge.
- Versioned `.otp` project format with atomic saves.
- egui shell: timeline view, media pool, open/save.
- CLI: `demo` / `new` / `probe` / `render` / `gui`.
- Blueprint: ARCHITECTURE, ROADMAP M0–M5, CLEANROOM policy, RFC 0001–0007.
- CI (Windows-first 3-OS matrix) and tag-driven release workflow.
