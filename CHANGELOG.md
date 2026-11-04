# Changelog

All notable changes to OpenTrack are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com); versions are tagged `vX.Y.Z`.

## [0.4.0] — M3 + M4 + M5: video pipeline, FX framework, ecosystem

Milestones M3 (video decode/composite/render), M4 (FX framework) and M5
(ecosystem & packaging) shipped together in this release.

### Added — M3: video decode, composite, render (RFC-0007)

- **`VideoDecoder` trait consolidation**: every video source now speaks the
  RFC-0007 contract (`info` / `seek_frame` / `next_frame` → RGBA8).
- **Pure-Rust Motion-JPEG AVI decode** (`engine::avi` + `MjpegAviDecoder`):
  an in-house RIFF walker (chunk walker, `strh`/`strf` parse, `movi` frame
  table, `idx1` fallback) plus `zune-jpeg` per frame with an LRU cache.
- **ffmpeg bridge video decoder**: `ffmpeg -f rawvideo -pix_fmt rgba -` pipe
  with `ffprobe`-based probing; frame-accurate seeks respawn with input-side
  `-ss`. `open_video_any` dispatches by container sniff (RIFF → AVI, `ftyp`
  → MP4/MOV, else bridge).
- **CPU compositor** (`engine::composite`): video tracks stacked top-down,
  per-track opacity, blend modes (Normal/Add/Multiply/Screen), event fades →
  alpha ramps, letterbox-fit bilinear blit, stills (PNG/JPEG/BMP) decoded
  once and cached.
- **Shared frame cache** (`engine::frame_cache`): `(MediaId, frame)` keyed
  LRU with a 256 MiB budget and hit/miss/eviction stats surfaced in the GUI.
- **Video render pipeline** (`RenderJob::run_video`): compositor → JPEG
  (pure Rust) → **pure-Rust AVI/MJPEG muxer** (streaming RIFF writer with
  size patching + `idx1`), companion WAV master from the same tick clock,
  optional PNG stills per frame; frame-count acceptance is tested (6.0 s @
  30 fps → exactly 180 frames, WAV matches the mixer output).
- **Optional AV1 export** behind `--features export-av1` (rav1e → IVF);
  default builds stay pure.
- **GUI**: the preview pane now shows the *composited* frame (all video
  tracks) with a 1/1–1/4 scale toggle and decoded/cache/dropped counters.

### Added — M4: FX framework (RFC-0004 completion)

- **Automation lanes** (`core::fx::AutomationLane`): per-FX-parameter
  control points with linear/constant interpolation, sampled at processing
  block ticks; `.otp` format v2 (v1 files load unchanged via serde defaults).
- **Audio FX DSP** (`engine::afx`): 3-band shelving/peaking EQ (RBJ
  biquads), feed-forward compressor, brickwall limiter, Schroeder reverb
  send — wired into the offline mixer (fixed 128-frame blocks, automation
  sampled per block) through the same `AudioChainRunner` the realtime
  engine uses.
- **Parity guarantee**: offline and realtime share the processor types at
  the f32 stage; a golden test proves one-call vs many-block processing is
  bit-identical (`afx::tests::block_chunking_is_bit_identical_parity`).
- **Video FX** (`engine::vfx`): brightness/contrast, saturation, baked 1D
  LUTs (identity / film fade / crush blacks) — CPU reference implementations
  applied per video track in the compositor.
- **FX registry growth**: `audio.compressor`, `audio.limiter`,
  `audio.reverb`, `video.lut` join the M0 descriptors; factory **presets**
  (`core::fx::presets`) for every FX.
- **FX rack UI**: per-track chain (domain-filtered add, reorder ↑/↓, bypass,
  remove), parameter sliders from `ParamSpec`, preset buttons, automation
  points add/remove at the playhead.

### Added — M5: ecosystem & packaging

- **Plugin ABI v0** (RFC-0009): the frozen audio-blocks/video-frames
  contract, an in-process reference host (`engine::plugin`) and two builtin
  example plugins (`builtin.stereo_width`, `builtin.tint`). Dynamic loading
  stays behind its own future RFC.
- **Scripting** (RFC-0010): Rhai-hosted batch surface over the project
  document; `opentrack script FILE.rhai PROJECT.otp` runs and saves.
- **Installers**: new `xtask` crate — `cargo xtask installer` builds the
  release and packages the host platform (Windows zip + WiX/MSIX
  scaffolding, Linux `.AppDir`/AppImage with `.desktop`, macOS `.app` +
  dmg); `cargo xtask site` regenerates the static docs site.
- **Packaging assets**: WiX source, MSIX manifest + script, AppImage
  AppRun/desktop/icon, macOS signing/notarization runbook.
- **Research**: RFC-0011 evaluates VST/iPlug-style hosting (verdict: CLAP
  first, never VST2).
- **Docs**: parity matrix 100% reviewed against 0.4.0; tutorial series in
  `docs/tutorials/`.

### Changed

- `VideoDecoder` is now a trait; the M1 concrete MP4/H.264 decoder is
  `Mp4H264Decoder` (was `VideoDecoder`).
- The offline mixer processes tracks in fixed 128-frame blocks (needed for
  FX-chain parity); mix output is bit-compatible for FX-free projects
  modulo associativity-level rounding.
- Workspace version 0.4.0; `.otp` FORMAT_VERSION 2.

## [0.3.0] — M2: realtime audio playback

### Added

- **Realtime audio engine** (`opentrack-engine::realtime`, RFC-0008): the
  timeline plays through the platform output device with the same mix math as
  the offline renderer.
  - worker thread decodes ahead per audio track (backend-chain decoders →
    stateful streaming resampler → stereo, event gain + fades pre-applied) and
    hands 20 ms blocks to the output callback through a hand-rolled lock-free
    bounded SPSC queue (3 blocks ≈ 60 ms);
  - the audio callback never locks, allocates or decodes; seek/rebuild swap
    generation-tagged queues so scrubbing never mixes stale audio;
  - **live parameters**: track gain/pan/mute/solo and the master bus apply
    immediately from the GUI (fixed-point atomics + per-sample smoothing — no
    zipper noise, no locks);
  - **transport**: play/pause/stop, loop region, auto-stop at the timeline
    end, play-at-end restarts from the top; the playhead is the audio clock
    minus documented output latency, so picture and sound stay aligned;
  - **peak meters** on every audio track strip and the master bus (atomic
    publish, GUI poll with ballistics, green/yellow/red zones);
  - realtime vs **offline parity** is tested (same crossfade session, max
    sample diff < 1e-4).
- **Device layer**: `cpal` (WASAPI / CoreAudio / ALSA) behind the optional
  engine feature `out-cpal`; the GUI enables it by default via its `audio`
  feature, headless builds (`--no-default-features`) compile the full engine
  and degrade to a silent playhead with a status-bar reason.
- **`opentrack smoke [--minutes N] [--realtime]`**: the M2 acceptance profile —
  an 8-track, 48 kHz, N-minute session played through the *real* mix graph and
  callback code (device simulated) with zero underruns; `--realtime` paces to
  wall clock for local soak testing.
- **GUI**: transport bar (Play/Pause/Stop/Loop + region display), loop region
  via shift+drag on the ruler (right-click clears), track meters, master
  meter, audio status/latency readout; `History` gained a `revision()` counter
  the GUI diffs to rebuild the engine graph.
- **Docs**: RFC-0008 (realtime engine), ARCHITECTURE §3.4 + invariants I8–I10,
  ROADMAP M2 acceptance notes.

### Platform notes

- Linux CI/builds need `libasound2-dev` + `pkg-config` (added to CI). Windows
  and macOS need nothing new.

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
