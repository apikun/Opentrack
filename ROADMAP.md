# OpenTrack Roadmap

Status markers: ✅ shipped · 🚧 in progress · ⬜ planned. Each milestone has
explicit acceptance criteria; a milestone is done when every criterion has a
demonstration (test, CI job, or reproducible command) — not before.

Milestone ordering follows the clean-room policy in
[`CLEANROOM.md`](CLEANROOM.md): everything below is specified from publicly
observable workflow behavior and community RFCs, never from proprietary
internals.

---

## M0 — Blueprint & skeleton ✅ (this release)

Working data model, offline audio render, GUI shell, project files.

- [x] Cargo workspace: `core` / `engine` / `cli` / `app` with one-way deps
- [x] Integer-tick timeline: tracks (video/audio), events, takes, fades
- [x] Edit ops + undo/redo: add/remove track & event, move, trim, slip, split,
      ripple delete, track props, master bus (`opentrack-core::undo`)
- [x] Snapping: grid + event edges, configurable threshold, ignore-set
- [x] Overlap detection → same-track crossfade model
- [x] Audio bus math: dB gain, equal-power pan, mute/solo, master bus
- [x] Codec abstraction: `CodecBackend` trait, pure-Rust WAV backend
      (PCM 8/16/24/32 + float32/64), optional ffmpeg/ffprobe bridge
- [x] Offline audio render → 16-bit PCM WAV with per-channel peaks
- [x] `.otp` versioned project format (save/load, forward-version refusal)
- [x] egui shell: timeline view (tracks, events, fades, playhead, zoom),
      media pool, open/save, `--project` flag
- [x] CLI: `demo` / `new` / `probe` / `render` / `gui`
- [x] CI matrix: Windows (primary) → Ubuntu → macOS, fmt + clippy `-D warnings`
      + tests + GUI compile check
- [x] Release pipeline: tag → binaries for Windows/Linux/macOS (CLI + GUI)

**Acceptance:** `cargo test --workspace` green on all 3 CI platforms;
`opentrack demo` renders a crossfaded stereo master end-to-end with zero
non-Rust dependencies.

## M1 — Interactive timeline editing 🚧 (next)

Make the GUI a real editor, not a viewer.

- [ ] Pointer-driven event move / trim / slip with drag previews
- [ ] Split at playhead or click point (S key), snapping on by default
- [ ] Ripple delete (R), auto-ripple toggle
- [ ] Multi-selection (shift/ctrl click), group move, group delete
- [ ] Take switching from the context menu
- [ ] Track header widgets: gain slider, pan knob, mute/solo, arm
- [ ] Crossfade drag handles on overlap runs; fade curve picker
- [ ] Zoom to selection; vertical zoom; timeline ruler scrub
- [ ] Autosave + crash-recovery journal (`.otp.autosave` + op log replay)

**Acceptance:** a user can assemble the `demo` session entirely with the mouse
in under two minutes, undo any mistake, and save a `.otp` that re-renders
identical output (`opentrack render`).

## M2 — Realtime audio playback ⬜

- [ ] cpal output thread; prebuilt mix graph updated on `History` revision
- [ ] Lock-free parameter smoothing (gain/pan) from GUI thread
- [ ] Playhead-locked transport: play/pause/stop/loop region
- [ ] Peak meters on track headers + master (atomic publish, GUI poll)
- [ ] Audition scrubbing with buffered lookahead
- [ ] Latency compensation constant, documented per platform

**Acceptance:** 48 kHz stereo session with 8 audio tracks plays continuously
for 10 minutes on Windows with no dropouts in the CI smoke profile.

## M3 — Video: decode, composite, render ⬜

- [ ] `VideoDecoder` trait; pure-Rust decode path for Motion-JPEG AVI
      intermediate (see RFC 0007), ffmpeg bridge for everything else
- [ ] Frame cache keyed by `(MediaId, frame)` with byte budget + eviction
- [ ] Compositor: track stacking order, per-track opacity, blend modes
      (already modeled: `BlendMode`), event fades → alpha ramps
- [ ] GPU presentation of the composited frame in the egui canvas
- [ ] Timeline video preview resolution scaling (1/1, 1/2, 1/4)
- [ ] Render pipeline: compositor → PNG/JPEG frames + WAV →
      pure-Rust AVI/MJPEG muxer; optional rav1e AV1 export behind a feature
- [ ] Audio/video sync discipline: single master clock, frame-accurate seeks

**Acceptance:** M1-built session containing PNG stills + MJPEG clips + WAV
audio renders to a playable AVI whose frame count × fps matches the timeline
duration exactly.

## M4 — FX framework ⬜

- [ ] Parameter model shipped in M0 (`FxDescriptor`, `ParamSpec`, `Domain`)
      extended with automation lanes per parameter
- [ ] Audio FX: Track EQ (3-band), compressor, limiter, reverb send — DSP
      module-by-module with offline + realtime parity tests
- [ ] Video FX: brightness/contrast, saturation, 1D LUTs; GPU path via
      egui-paintable shaders, CPU reference implementation for tests
- [ ] FX chain UI: per-track rack, drag reorder, bypass, presets
- [ ] Render parity: realtime FX bit-match offline FX at 32-bit float stage

**Acceptance:** golden-file tests prove realtime and offline DSP produce
identical buffers; every stock FX has docs and a preset bank.

## M5 — Ecosystem & packaging ⬜

- [ ] Plugin ABI v0 (audio: process f32 blocks; video: filter RGBA8 frames)
      — design via RFC process, frozen before any third-party promise
- [ ] Scripting surface (project-level Lua/Rhai) for batch ops
- [ ] Installer/MSIX for Windows, AppImage for Linux, signed notarized dmg
- [ ] Per-platform VST/iPlug-style hosting evaluation (research RFC only)
- [ ] Documentation site; tutorial video series; parity matrix 100% reviewed

**Acceptance:** a fresh contributor can build an installer for their platform
from a clean clone with one command.

---

## Beyond

Ordering decided by community RFCs: collaboration (crdt-backed op log), HDR
video, surround buses, hardware control surfaces, project templates.

## cadence

Milestones M1+ are time-boxed, not feature-boxed: whatever is not done rolls
into the next milestone, and the ROADMAP is updated in the same PR as the
code so it never rots.
