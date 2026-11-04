# OpenTrack

**A clean-room, open-source NLE inspired by the classic Vegas workflow — rebuilt from scratch in pure Rust.**

OpenTrack reimplements the *workflow* that made Vegas-era editors beloved: unlimited
video and audio tracks, drag-anywhere events with automatic crossfades on overlap,
per-track volume/pan strips, takes, and a straightforward master-bus render.
It is a **from-scratch implementation**: no proprietary code, no decompiled
assets, no protocol/SDK reimplementation — only publicly observable behavior,
documented in RFCs, written by and for the open-source community.

- License: **GPL-3.0-or-later** (see [`LICENSE`](LICENSE))
- Status: **M1 — interactive editing + video preview** (working data model + offline audio render + editable timeline + pure-Rust H.264 preview)
- Platform: **Windows first**, then Linux/macOS (CI matrix in [`.github/workflows/ci.yml`](.github/workflows/ci.yml))

> **Disclaimer** — OpenTrack is not affiliated with, endorsed by, or derived from
> Sony Creative Software or MAGIX Software GmbH. "Vegas" is a trademark of MAGIX;
> it is referenced in this repository solely to describe workflow compatibility
> (nominative fair use). See [`CLEANROOM.md`](CLEANROOM.md) for the full policy.

---

## Quickstart

```bash
git clone https://github.com/apikun/Opentrack.git
cd Opentrack
cargo run -p opentrack-cli -- demo          # builds a real project and renders a real mix
cargo run -p opentrack-app                  # opens the egui shell (timeline GUI)
```

The demo synthesizes two WAV stems with pure Rust, assembles a Vegas-style session
through the *editing API* (the same `Session` verbs the GUI will use), saves a
`demo.otp` project file, and renders a crossfaded stereo master:

```
opentrack demo - building a Vegas-style session in opentrack-demo
  [1/4] synthesized stems: tone_lead_440.wav + tone_pad_330.wav
  [2/4] project saved: opentrack-demo/demo.otp (4 track ops in undo history)
        timeline: 2 events, 1 same-track crossfade overlap(s), 6.0s
  [3/4] rendering offline mix (pure-Rust WAV backends)...
  [4/4] wrote opentrack-demo/demo_mix.wav (288000 frames, 48000 Hz stereo)
        master peak: -4.15 dBFS (linear 0.620)
```

### CLI reference

| Command | Purpose |
| --- | --- |
| `opentrack demo [DIR]` | Build + render the bundled demo session |
| `opentrack new NAME [DIR]` | Create an empty project skeleton (`NAME.otp`) |
| `opentrack probe FILE` | Identify media through the backend chain (pure-Rust first, then ffmpeg) |
| `opentrack frame FILE [--at SECS] [-o OUT.bmp]` | Decode one video frame to a BMP (pure-Rust H.264, no ffmpeg) |
| `opentrack render PROJECT [-o OUT] [--rate HZ]` | Offline audio render of a project |
| `opentrack gui` | Print how to launch the egui shell |

Load the demo into the GUI: `cargo run -p opentrack-app -- --project opentrack-demo/demo.otp`

### Prebuilt binaries

Release builds (CLI + GUI, Windows first) are attached to every
[GitHub Release](https://github.com/apikun/Opentrack/releases), produced by
[`.github/workflows/release.yml`](.github/workflows/release.yml) on each `v*` tag.

---

## Workspace layout

```
crates/
  opentrack-core     Project/timeline data model, editing ops + undo, buses, .otp I/O
  opentrack-engine   CodecBackend trait, pure-Rust WAV codec, ffmpeg bridge,
                     pure-Rust MP4/H.264 video decode, offline mixer
  opentrack-cli      The `opentrack` binary (demo / new / probe / frame / render)
  opentrack-app      The egui GUI: interactive timeline, video preview, media pool
docs/
  rfc/               RFC 0001-0007: the blueprint proper
  vegas-parity.md    Feature-by-feature parity matrix vs. the Vegas workflow
ARCHITECTURE.md      How the crates fit together
ROADMAP.md           M0..M5 milestones with acceptance criteria
CLEANROOM.md         Clean-room & trademark policy (read before contributing)
ATTRIBUTION.md       Upstream components and licenses (filmcraft-h264, craft-media)
CHANGELOG.md         Release notes per version
```

### What already works (M0 + M1)

- **Timeline model**: tracks (video top-down / audio), events with takes, fades,
  media in-points; Vegas-style **split / trim / slip / move / ripple delete** as
  validated, reversible, serializable edit ops with undo/redo (`opentrack-core::edit`).
- **Interactive editing (M1)**: pointer drag to move, edge-drag to trim,
  alt-drag to slip, `S` to split at the playhead, ripple delete, track strips
  with gain/pan/mute/solo, master fader, snapping, zoom, full undo/redo —
  all through the same `Session` op pipeline (`opentrack-app`).
- **Video preview (M1)**: the pane under the playhead decodes **H.264/MP4
  in pure Rust** — MP4/MOV demux + AVC decode + YUV→RGBA — no ffmpeg needed.
  Frame-accurate scrubbing; streaming GOP decode keeps playback O(1) per frame.
- **Snapping**: grid + event-edge snapping with configurable threshold.
- **Audio bus math**: equal-power pan law, dBFS gain, mute/solo semantics,
  master bus (`opentrack-core::bus`).
- **Offline audio render**: decode → per-event fades → track gain/pan →
  master bus → 16-bit PCM WAV, with per-channel peak reporting (`opentrack-engine::mixer`).
- **Codec abstraction**: `CodecBackend` trait with a zero-dependency pure-Rust
  WAV backend (PCM 8/16/24/32, float 32/64, extensible) and an optional
  ffmpeg/ffprobe bridge for everything else.
- **Project files**: versioned `.otp` JSON with forward-version rejection.

### What is deliberately not built yet

Video compositing/multi-clip render and video export (M3), realtime audio
playback (M2), FX DSP (M4/M5), multi-select/cross-track moves (M1.1). Every
gap has an RFC and a milestone:
see [`ROADMAP.md`](ROADMAP.md) and [`docs/vegas-parity.md`](docs/vegas-parity.md).

---

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI enforces all three on Windows first, then Ubuntu and macOS.
MSRV: **1.95** (edition 2024). Third-party components and their licenses:
[`ATTRIBUTION.md`](ATTRIBUTION.md).

## Contributing

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and especially [`CLEANROOM.md`](CLEANROOM.md).
Substantive changes go through the RFC process in [`docs/rfc/README.md`](docs/rfc/README.md).

## Design documents

| RFC | Title |
| --- | --- |
| [0001](docs/rfc/0001-architecture-overview.md) | Architecture overview & crate boundaries |
| [0002](docs/rfc/0002-timeline-data-model.md) | Timeline data model & editing operations |
| [0003](docs/rfc/0003-codec-backends.md) | Codec backends (pure Rust + ffmpeg bridge) |
| [0004](docs/rfc/0004-audio-bus-dsp.md) | Audio buses, mixing & DSP roadmap |
| [0005](docs/rfc/0005-gui-egui-shell.md) | GUI shell (egui) layout & interaction plan |
| [0006](docs/rfc/0006-project-file-format.md) | `.otp` project file format |
| [0007](docs/rfc/0007-video-render-pipeline.md) | Video compositing & render pipeline |
