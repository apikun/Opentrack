# OpenTrack Architecture

OpenTrack is a clean-room, pure-Rust NLE inspired by the classic Vegas workflow.
This document describes how the workspace is organized, what each crate owns,
and the invariants that keep the layers honest. For the milestone plan see
[`ROADMAP.md`](ROADMAP.md); for the clean-room/legal policy see
[`CLEANROOM.md`](CLEANROOM.md).

---

## 1. Guiding principles

1. **Model first, pixels later.** The editing model (`opentrack-core`) never
   depends on rendering, audio DSP, or GUI crates. A headless CI run can build,
   mutate, save and render a full session without ever opening a window.
2. **Every mutation is an op.** The GUI does not edit `Project` fields directly;
   it executes `EditOp`s through `Session`, which owns the undo history. This
   makes undo/redo, serialization of the edit log, and future collaborative
   editing fall out of the same machinery.
3. **Time is integer ticks.** All timeline positions are `Ticks(i64)` resolved
   against a project `TimeBase` (ticks per second). No floating-point
   accumulators on the timeline — splits and snaps stay frame-exact.
4. **Codecs live behind traits.** The engine speaks to media through
   `CodecBackend` / `AudioSource` traits. Pure-Rust backends are the default;
   an ffmpeg bridge is a runtime-detected optional extra, never a build
   dependency.
5. **Windows first, but not Windows only.** CI and release builds run on
   Windows (MSVC) first, then Linux and macOS. Platform-specific code is
   isolated in the engine backends and the GUI shell.

## 2. Crate layout

```
opentrack (cargo workspace)
│
├── opentrack-core     The editing model. No I/O beyond .otp save/load.
│     project.rs       Project, MediaPool, MediaAsset, VideoSettings
│     timeline.rs      Track, Event, Take, track props, blend modes
│     time.rs          Ticks, TimeBase (integer timeline clock)
│     ids.rs           Typed newtype IDs (TrackId, EventId, MediaId, ...)
│     undo.rs          EditOp enum + apply/revert + History (undo/redo)
│     edit.rs          Session facade, snapping, overlap detection
│     bus.rs           AudioBus, gain dB math, equal-power pan law
│     fx.rs            FX descriptors, parameter specs, FX chains
│     save.rs          .otp JSON I/O with version gating
│
├── opentrack-engine   Media plumbing. Knows core, nothing about GUI.
│     codec.rs         CodecBackend + AudioSource traits, backend chain
│     wav.rs           Pure-Rust WAV demux/decode + PCM s16 encode
│     backend_ffmpeg.rs Optional ffmpeg/ffprobe bridge (runtime probed)
│     dsp.rs           Sine synthesis, linear resampler, peak metering
│     mixer.rs         Offline audio mixdown: fades → track bus → master
│     render.rs        RenderJob orchestration (audio now, video in M3)
│
├── opentrack-cli      The `opentrack` binary: demo / new / probe / render / gui
│
└── opentrack-app      The egui (eframe) GUI shell: timeline view, media pool,
                      project open/save, playhead animation
```

Dependency direction (enforced by review, one-way only):

```
opentrack-app ──▶ opentrack-cli ──▶ opentrack-engine ──▶ opentrack-core
        └──────────────────────────────────────────────────────┘
                 (the app links core directly for Session/Project)
```

`opentrack-core` depends only on `serde`, `serde_json` and `thiserror`.
`opentrack-engine` additionally depends on nothing but `core` + std at build
time (ffmpeg is invoked as an external process, not linked).

## 3. Data flow

### 3.1 Editing (GUI or CLI)

```
user action ──▶ EditOp (validated data)
                 │ Session::exec()
                 ▼
          op.apply(&mut Project)      ──▶ success: op pushed to History
                 │                       failure: Project untouched, Err returned
                 ▼
          Project (single source of truth)
                 │
                 ├──▶ save.rs → demo.otp (versioned JSON)
                 └──▶ engine render / GUI paint
```

`History::push` applies first and only records the op on success, so the undo
stack can never desynchronize from the project state. `EditOp::revert` rebuilds
the prior state from data captured inside the op itself (removed events are
stored back verbatim), so undo does not require snapshots.

### 3.2 Offline audio render

```
Project ──▶ for each audio track (bottom-up):
              for each event:
                decode take's media  (CodecBackend chain)
                resample to render rate            (dsp::resample_linear)
                apply event fades                  (linear / fast / smooth curves)
                apply track gain dB + equal-power pan   (core::bus)
              sum, honor mute/solo semantics
            ──▶ master bus gain
            ──▶ peak report per channel                (dsp::peaks)
            ──▶ 16-bit PCM WAV write                   (wav::encode_s16)
```

Rendering is synchronous and single-threaded in M0; the `RenderJob` facade keeps
the door open for a graph scheduler without touching call sites.

### 3.3 Codec backend chain

`open_audio_any` walks a backend list (pure-Rust WAV first, then the ffmpeg
bridge if the executables are found on `PATH`) and returns the first backend
that can `probe` the file. This keeps zero-dependency builds real while letting
users with ffmpeg installed open anything.

## 4. Key invariants

| # | Invariant | Enforced by |
| --- | --- | --- |
| I1 | Timeline positions are `Ticks(i64)`; negative positions are rejected | `EditError::BadPosition`, op validation |
| I2 | Video tracks order top-down, audio tracks bottom-up, video above audio | `Project::add_track` insertion policy |
| I3 | Events on a track may overlap; overlap runs on audio become crossfades | `edit::overlaps_on_track` |
| I4 | Every op is reversible with data captured at exec time | `EditOp::revert`, unit tests |
| I5 | `.otp` files from newer format versions are refused, never guessed | `save::load` version gate |
| I6 | Pure-Rust build (no ffmpeg present) compiles, tests and renders | CI runs without ffmpeg installed |
| I7 | GUI never mutates `Project` outside `Session::exec` | code review + crate boundaries |

## 5. Threading & realtime (looking ahead)

M0 is synchronous by design. The intended shape for M2+:

- **Engine thread** owns the `Project` render snapshot; the GUI thread posts
  `EditOp`s and receives state revisions (never shared mutable state).
- **Audio callback** (cpal) pulls from a lock-free prebuilt mix graph; the GUI
  metering path reads atomically published peaks.
- **Video playback** (M3) decodes ahead into a small ring of GPU-uploaded
  textures keyed by `(MediaId, frame)`.

None of this changes the model: `EditOp` remains the only mutation channel.

## 6. Versioning

- Workspace version is `0.x` until M2; breaking model changes bump the minor.
- `.otp` format version is independent (`FORMAT_VERSION` in `save.rs`) and only
  ever increments.
- Releases are tagged `vX.Y.Z` per [Keep a Changelog](https://keepachangelog.com)
  once a CHANGELOG exists (M1).
