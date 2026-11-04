# OpenTrack Roadmap

Status markers: ✅ shipped · 🚧 in progress · ⬜ planned. Each milestone has
explicit acceptance criteria; a milestone is done when every criterion has a
demonstration (test, CI job, or reproducible command) — not before.

Milestone ordering follows the clean-room policy in
[`CLEANROOM.md`](CLEANROOM.md): everything below is specified from publicly
observable workflow behavior and community RFCs, never from proprietary
internals.

---

## M0 — Blueprint & skeleton ✅ (0.1.0)

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

## M1 — Interactive timeline editing ✅ (0.2.0)

Make the GUI a real editor, not a viewer.

- [x] Pointer-driven event **move** with drag previews (ghost) and snapping
- [x] Edge-drag **trim** (media follows the head), clamped to media bounds
- [x] Alt-drag **slip** (content shifts without moving the event)
- [x] **Split at playhead or click point** (`S`), snapping honored
- [x] **Ripple delete** (`Del`), auto-ripple toggle in the status bar
- [x] Track header widgets: gain (dB) + pan sliders (commit on release),
      mute/solo buttons — every change is a reversible `SetAudioTrackProps` op
- [x] Master fader (`SetMaster`) in the status bar
- [x] Ruler scrub (click/drag); playhead frame-stepping with arrow keys;
      Space transport; Home/End; `+`/`-` and Ctrl+wheel zoom
- [x] Media pool import via drag & drop (WAV / MP4 / images) with probing;
      "Place at playhead" auto-creates a matching track when none exists
- [x] Full undo/redo (`Ctrl+Z` / `Ctrl+Y`) over every one of these actions
- [x] **Video preview pane** — pure-Rust MP4/H.264 decode under the playhead
      (pulled forward from M3: demux + AVC decode + YUV→RGBA, streaming GOP
      decode, frame cache; see RFC 0007 and `ATTRIBUTION.md`)
- [ ] Multi-selection (shift/ctrl click), group move, group delete
      (needs an `EditOp::Batch` compound op) — *deferred to M1.1*
- [ ] Cross-track event moves (same compound-op dependency) — *deferred to M1.1*
- [ ] Take switching from the context menu — *deferred to M1.1*
- [ ] Crossfade drag handles on overlap runs; fade curve picker — *M1.1*
- [ ] Zoom to selection; vertical zoom — *M1.1*
- [ ] Autosave + crash-recovery journal (`.otp.autosave` + op log replay) — *M1.1*

**Acceptance:** a user can assemble the `demo` session entirely with the mouse
in under two minutes, undo any mistake, and save a `.otp` that re-renders
identical output (`opentrack render`). — demonstrated by the shipped op
pipeline tests; video preview decode verified by fixture tests in
`opentrack-engine::video` and the `opentrack frame` CLI round-trip.

## M2 — Realtime audio playback ✅ (0.3.0)

- [x] Worker-thread mix graph rebuilt on `History` revision (decode-ahead per
      track: `AudioSource` chunks → stateful `StreamResampler` → stereo,
      event gain + fades pre-applied)
- [x] Lock-free block handoff (hand-rolled bounded SPSC over `Arc<MixBlock>`,
      depth 3 ≈ 60 ms); generation-tagged queues make seek/rebuild glitch-free
- [x] Lock-free parameter updates from the GUI thread (dB×100 / pan×1000 /
      flag atomics) with per-sample smoothing — no zipper noise
- [x] Playhead-locked transport: play/pause/stop, loop region (shift+drag the
      ruler), auto-stop at the timeline end, play-at-end restarts from the top
- [x] Peak meters on track headers + master (atomic `fetch_max` publish, GUI
      poll with ballistics, green/yellow/red zones)
- [x] Audition scrubbing: seeks re-prime buffered lookahead; audio chases the
      playhead while dragging
- [x] Latency compensation: displayed playhead = feed position − device
      latency; per-host period estimates documented (RFC-0008)
- [x] Device layer: cpal behind the optional `out-cpal` feature (WASAPI /
      CoreAudio / ALSA); headless builds compile the engine without it
- [x] `opentrack smoke [--minutes N] [--realtime]` — the acceptance profile:
      8 tracks × 48 kHz played through the real mix graph + callback code with
      zero underruns

**Acceptance:** 48 kHz stereo session with 8 audio tracks plays continuously
for 10 minutes with no dropouts — demonstrated by `realtime::smoke_eight_tracks_ten_minutes`
and `opentrack smoke` in CI (producer-paced; `--realtime` paces to wall clock
for local soak tests). Realtime-vs-offline parity is proven by test
(`realtime_mix_matches_offline`, max diff < 1e-4). Note: device-rate playback
(WASAPI/CoreAudio) is exercised on hardware by the GUI; CI validates the
deterministic mix-graph/transport path.

## M3 — Video: decode, composite, render ✅ (0.4.0)

- [x] Pure-Rust H.264 decode for preview (shipped early in M1):
      MP4/MOV demux (`craft-media` isobmff) + `filmcraft-h264` + our
      YUV 4:2:0 → RGBA conversion — see `ATTRIBUTION.md` for licenses
- [x] `VideoDecoder` trait consolidation (RFC-0007: info/seek_frame/
      next_frame → RGBA8); pure-Rust Motion-JPEG AVI decode (in-house RIFF
      walker + zune-jpeg); ffmpeg bridge for everything else
- [x] Frame cache keyed by `(MediaId, frame)` with 256 MiB budget + LRU
      eviction (`opentrack-engine::frame_cache`)
- [x] Compositor: track stacking order (index 0 = top), per-track opacity,
      blend modes (Normal/Add/Multiply/Screen), event fades → alpha ramps
- [x] Composited frame presentation in the egui canvas (CPU composite →
      texture upload; the shader path is an explicit post-M4 optimization
      RFC, per RFC-0007 non-goals)
- [x] Preview resolution scaling (1/1, 1/2, 1/4) with a GUI toggle
- [x] Render pipeline: compositor → JPEG frames → pure-Rust AVI/MJPEG
      muxer (+ WAV master from the same tick clock, + optional PNG stills);
      optional rav1e AV1 export behind `--features export-av1`
- [x] A/V sync discipline: single master tick clock (audio samples and
      video frames both derive from it); frame-accurate seeks via keyframe
      runs (MP4) and exact frame tables (AVI)

**Acceptance (demonstrated in tests):** a session with PNG stills + MJPEG
clips + WAV audio renders to a playable AVI whose frame count × fps matches
the timeline duration exactly — `render::tests::
video_render_frame_count_acceptance` (6.0 s @ 30 fps → exactly 180 frames,
companion WAV sample count matches the M0 mixer output, AVI demuxes back
with 180 frames, 180 PNG stills exported).

## M4 — FX framework ✅ (0.4.0)

- [x] Parameter model (`FxDescriptor`, `ParamSpec`, `Domain`) extended with
      automation lanes per parameter (`AutomationLane`, linear/constant
      curves, sampled at block ticks; `.otp` v2, v1 files load as-is)
- [x] Audio FX (offline mixer runs tracks through `AudioChainRunner` in
      fixed 128-frame blocks — the same runner the realtime engine feeds):
      3-band shelving/peaking EQ (RBJ biquads), feed-forward compressor,
      brickwall limiter, Schroeder reverb send
- [x] Video FX, CPU reference implementation: brightness/contrast,
      saturation, 1D LUTs (3 baked curves) applied per video track in the
      compositor; the GPU/shader path remains a post-M4 optimization RFC
      (RFC-0007 non-goal — CPU first, parity tests exist)
- [x] FX chain UI: per-track rack in the right panel — add from the
      registry (domain-filtered), reorder (↑/↓), bypass toggle, remove,
      parameter sliders from `ParamSpec`, factory presets, automation
      points add/remove at the playhead
- [x] Render parity: realtime and offline DSP share the exact processor
      types at the f32 stage; golden test proves the same stream processed
      in one call vs many `FX_BLOCK_FRAMES` blocks is bit-identical
      (`afx::tests::block_chunking_is_bit_identical_parity`)

**Acceptance (demonstrated in tests):** the chunking-parity golden file
proves identical f32 buffers across call patterns; every stock FX has a
descriptor, docs and a preset bank (`opentrack_core::fx::presets`).

## M5 — Ecosystem & packaging ✅ (0.4.0)

- [x] Plugin ABI v0 designed via RFC-0009 (audio: in-place f32 blocks;
      video: in-place RGBA8 frames) with the in-process reference host +
      two builtin example plugins shipped; dynamic loading stays behind
      its own RFC before any third-party promise
- [x] Scripting surface (Rhai, RFC-0010): `opentrack script FILE.rhai
      PROJECT.otp` for batch ops over the media pool/tracks/events/FX
- [x] Installers: `cargo xtask installer` — Windows zip (+ WiX `.wxs` and
      MSIX `make-msix.ps1` scaffolding), Linux `.AppDir`/AppImage with
      `.desktop` entry, macOS `.app` + `hdiutil` dmg (signing/notarization
      documented in `packaging/macos/README.md` — identity-bound steps are
      out of CI's reach by design)
- [x] VST/iPlug-style hosting evaluation as research RFC-0011 only
      (recommendation: CLAP first, never VST2)
- [x] Documentation site (`cargo xtask site` → static `docs/site/`),
      tutorial series (`docs/tutorials/`), parity matrix reviewed
      (`docs/vegas-parity.md`)

**Acceptance (demonstrated):** a fresh contributor builds an installer for
their platform from a clean clone with one command — `cargo xtask
installer` (verified on Linux: release build + `OpenTrack.AppDir`).

---

## Beyond

Ordering decided by community RFCs: collaboration (crdt-backed op log), HDR
video, surround buses, hardware control surfaces, project templates.

## cadence

Milestones M1+ are time-boxed, not feature-boxed: whatever is not done rolls
into the next milestone, and the ROADMAP is updated in the same PR as the
code so it never rots.
