# OpenTrack

**A clean-room, open-source NLE inspired by the classic Vegas workflow — rebuilt from scratch in pure Rust.**

OpenTrack reimplements the *workflow* that made Vegas-era editors beloved: unlimited
video and audio tracks, drag-anywhere events with automatic crossfades on overlap,
per-track volume/pan strips, takes, and a straightforward master-bus render.
It is a **from-scratch implementation**: no proprietary code, no decompiled
assets, no protocol/SDK reimplementation — only publicly observable behavior,
documented in RFCs, written by and for the open-source community.

- License: **GPL-3.0-or-later** (see [`LICENSE`](LICENSE))
- Status: **M5 — ecosystem & packaging shipped** (M0 blueprint + M1 interactive editing + M2 realtime audio + M3 video decode/composite/export + M4 FX framework + M5 plugin ABI/scripting/installers — see [`ROADMAP.md`](ROADMAP.md))
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
| `opentrack frame FILE [--at SECS] [-o OUT.bmp]` | Decode one video frame to a BMP (MP4/H.264 and MJPEG-AVI in pure Rust; bridge for the rest) |
| `opentrack render PROJECT [-o OUT] [--rate HZ]` | Offline audio render of a project |
| `opentrack render PROJECT --video OUT.avi [--frames DIR] [--quality N]` | **Video export (M3)**: composited MJPEG-AVI + WAV master (+ optional PNG stills); add `--export-av1` on an `export-av1` feature build |
| `opentrack script FILE.rhai PROJECT.otp` | Run a project-level batch script (Rhai, M5) |
| `opentrack smoke [--minutes N] [--realtime]` | Play an 8-track session through the realtime engine — the M2 acceptance profile (zero underruns) |
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
  opentrack-core     Project/timeline data model, editing ops + undo, buses, FX descriptors
                     + automation lanes, .otp I/O
  opentrack-engine   CodecBackend trait, pure-Rust WAV codec, ffmpeg bridge, video decode
                     (VideoDecoder trait: MP4/H.264 + MJPEG-AVI pure Rust, ffmpeg bridge),
                     CPU compositor, AVI/MJPEG muxer, audio FX + video FX, offline mixer,
                     realtime playback engine, plugin host (RFC-0009), Rhai scripting (RFC-0010)
  opentrack-cli      The `opentrack` binary (demo / new / probe / frame / render / script / smoke)
  opentrack-app      The egui GUI: interactive timeline, composited video preview, FX rack,
                     media pool, transport + loop + peak meters
xtask/              `cargo xtask installer` / `cargo xtask site` (M5 packaging automation)
packaging/          WiX + MSIX (Windows), AppImage assets (Linux), signing runbook (macOS)
docs/
  rfc/               RFC 0001-0011: the blueprint proper
  tutorials/         The tutorial series
  vegas-parity.md    Feature-by-feature parity matrix vs. the Vegas workflow (reviewed 0.4.0)
ARCHITECTURE.md      How the crates fit together
ROADMAP.md           M0..M5 milestones with acceptance criteria
CLEANROOM.md         Clean-room & trademark policy (read before contributing)
ATTRIBUTION.md       Upstream components and licenses
CHANGELOG.md         Release notes per version
```

### What already works (M0 → M5)

- **Timeline model**: tracks (video top-down / audio), events with takes, fades,
  media in-points; Vegas-style **split / trim / slip / move / ripple delete** as
  validated, reversible, serializable edit ops with undo/redo (`opentrack-core::edit`).
- **Interactive editing (M1)**: pointer drag to move, edge-drag to trim,
  alt-drag to slip, `S` to split at the playhead, ripple delete, track strips
  with gain/pan/mute/solo, master fader, snapping, zoom, full undo/redo.
- **Realtime audio playback (M2)**: Space plays the mix through the platform
  device (WASAPI/CoreAudio/ALSA) with the same gain/fade/pan math as the
  offline render; lock-free live parameters, peak meters on every strip and
  the master, loop region (shift+drag the ruler). The audio callback never
  locks, allocates or decodes (RFC-0008).
- **Video pipeline (M3)**: `VideoDecoder` trait with pure-Rust MP4/H.264 and
  **MJPEG-AVI** decode (in-house RIFF walker), ffmpeg bridge for the rest;
  CPU **compositor** (opacity/blend/fades), shared `(MediaId, frame)` LRU
  cache; **export** to MJPEG-AVI (pure-Rust muxer) + WAV master with an exact
  frame-count guarantee, optional PNG stills, optional AV1 (rav1e) behind a
  feature. The preview shows the composited result at 1/1–1/4 scale.
- **FX framework (M4)**: 3-band EQ, compressor, limiter, reverb send on every
  audio track; brightness/contrast, saturation and 1D LUTs on video tracks;
  **automation lanes** per FX parameter; factory presets; offline/realtime
  DSP parity proven bit-exact at the f32 stage.
- **Ecosystem (M5)**: plugin ABI v0 (audio blocks / video frames) with a
  reference host; Rhai **scripting** (`opentrack script`); one-command
  installers via **`cargo xtask installer`** (Windows zip + WiX/MSIX
  scaffolding, Linux AppImage/AppDir, macOS app bundle + dmg).
- **Snapping**: grid + event-edge snapping with configurable threshold.
- **Audio bus math**: equal-power pan law, dBFS gain, mute/solo semantics,
  master bus (`opentrack-core::bus`).
- **Offline audio render**: decode → per-event fades → track FX → track
  gain/pan → master bus → 16-bit PCM WAV, with per-channel peaks.
- **Codec abstraction**: `CodecBackend` trait with a zero-dependency pure-Rust
  WAV backend and an optional ffmpeg/ffprobe bridge for everything else.
- **Project files**: versioned `.otp` JSON (v2 carries FX chains + automation;
  v1 loads unchanged) with forward-version rejection.

### What is deliberately not built yet

GPU/shader compositing (CPU reference first — optimization RFC after parity
tests exist), custom bus routing, markers, dynamic plugin loading (RFC-0009
freezes the v0 contract first), CLAP/VST hosting (research only — RFC-0011),
HDR. Every gap has an RFC or a note: see [`ROADMAP.md`](ROADMAP.md) and
[`docs/vegas-parity.md`](docs/vegas-parity.md).

---

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
opentrack smoke --minutes 10   # realtime engine acceptance profile
```

CI enforces all three on Windows first, then Ubuntu and macOS (Linux CI/heads
need `libasound2-dev` + `pkg-config` for the realtime output). Headless builds
without audio hardware: `cargo build -p opentrack-app --no-default-features`.
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
