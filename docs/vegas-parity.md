# Workflow Parity Matrix

How the classic Vegas-era editing workflow maps onto OpenTrack. This matrix is
the public-behavior basis for our clean-room work: every "Vegas workflow"
feature listed here is described from publicly observable behavior (manuals,
tutorials, community discussion), specified in an RFC, then implemented.
Status: ✅ done · 🚧 partial · ⬜ planned · ❌ not planned (with reason).

Legend: RFC links go to `docs/rfc/`.

**Review status: 100% reviewed against the 0.4.0 build (M0–M5).** Every row
below was re-checked against the shipped code in the 0.4.0 release pass;
statuses name the milestone that actually shipped them.

## Timeline & events

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Unlimited video tracks (stack top-down) | ✅ M0 | `core::timeline`, RFC 0002 |
| Unlimited audio tracks (mix bottom-up) | ✅ M0 | `core::timeline`, RFC 0002 |
| Events placed anywhere on a track (drag-anywhere) | ✅ M1 | `EditOp::AddEvent` + GUI drag, RFC 0002 |
| Event takes (multiple takes per event, switchable) | ✅ M1 | `Event::takes` + take switching, M1 |
| Split event at cursor (S) | ✅ M1 | `EditOp::SplitEvent` + `S` keybind |
| Trim event edges | ✅ M1 | `EditOp::TrimEvent` + edge drag |
| Slip event content (media in-point without moving event) | ✅ M1 | alt-drag → `EditOp::SlipEvent` |
| Ripple delete (close the gap) | ✅ M1 | `Delete` → `EditOp::RippleDelete`, auto-ripple toggle |
| Multi-select (ctrl/shift), group move/delete | ✅ M1 | GUI selection set |
| Auto crossfade when events overlap | ✅ M0 | `edit::overlaps_on_track`, RFC 0004 |
| Fade in/out per event (linear ramps; curve picker ⬜ post-0.4) | ✅ M0 model / M1 handles | `core::timeline::Event` |
| Snap to grid + event edges, toggleable threshold | ✅ M0 | `edit::snap_time` |
| Loop region on ruler | ✅ M2 | shift+drag ruler, RFC 0005/0008 |
| Marker / region list | ⬜ post-0.4 | RFC 0002 §future |
| Track order drag-reorder | ⬜ post-0.4 | — |
| Envelopes (automation) on events/tracks | ✅ M4 | FX-param automation lanes (`AutomationLane`), RFC 0004 |

## Audio mixing

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Per-track volume (dB) | ✅ M0 | `core::bus::AudioBus` |
| Per-track pan (equal-power law) | ✅ M0 | `bus::equal_power_pan` |
| Mute / solo (solo overrides mute) | ✅ M0 | `Track::audible` |
| Master bus with volume | ✅ M0 | `EditOp::SetMaster` |
| Peak metering per track + master | ✅ M2 | realtime atomics + GUI poll, RFC 0008 |
| Realtime playback with transport (play/pause/stop/loop) | ✅ M2 | `engine::realtime`, RFC 0008 |
| Track EQ (3-band) | ✅ M4 | `engine::afx::Eq3` |
| Compressor / limiter | ✅ M4 | `engine::afx::{Compressor, Limiter}` |
| Reverb send | ✅ M4 | `engine::afx::ReverbSend` |
| FX presets | ✅ M4 | `core::fx::presets` |
| Bus routing (tracks → custom buses → master) | ⬜ post-0.4 | RFC 0004 |
| 5.1 surround | ❌ not until after 1.0 | scope |

## Video

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Video preview under the playhead | ✅ M1 (single clip) → M3 (full composite) | `engine::composite` |
| Compositing: opacity + blend modes (normal/add/multiply/screen) | ✅ M3 | `core::timeline::BlendMode`, `engine::composite` |
| Event fades → alpha ramps in the composite | ✅ M3 | `composite::fade_alpha` |
| Stills on video tracks (PNG/JPEG/BMP) | ✅ M3 | `composite::decode_still` |
| Preview quality scaling (1/1, 1/2, 1/4) | ✅ M3 | `PreviewScale` + GUI toggle |
| Video decode — H.264/MP4 pure Rust | ✅ M1 | `engine::video::Mp4H264Decoder` |
| Video decode — MJPEG AVI pure Rust | ✅ M3 | `engine::video::MjpegAviDecoder` + `engine::avi` |
| Video decode — everything else via ffmpeg bridge | ✅ M3 | `engine::video::FfmpegVideoDecoder` |
| Video FX (brightness/contrast, saturation, 1D LUT) | ✅ M4 | `engine::vfx` |
| Export AVI/MJPEG (pure Rust mux) + WAV master | ✅ M3 | `engine::{avi, render}` |
| Export PNG still sequence | ✅ M3 | `render --frames DIR` |
| AV1 export via rav1e (optional feature) | ✅ M3 | `--features export-av1` |
| Frame-accurate export (frames == ⌈duration × fps⌉) | ✅ M3 | tested, RFC 0007 acceptance |
| GPU/shader compositing | ⬜ post-0.4 optimization RFC | RFC 0007 non-goal |
| HDR / wide gamut | ⬜ post-1.0 | scope |

## Media & codecs

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| WAV decode (PCM 8/16/24/32, float32/64) — pure Rust | ✅ M0 | `engine::wav` |
| Anything else via ffmpeg bridge (runtime-detected) | ✅ M0 | `engine::backend_ffmpeg` |
| Media pool with asset paths + labels | ✅ M0 | `core::project::MediaPool` |
| Offline render to WAV 16-bit | ✅ M0 | `engine::mixer` |
| `.veg` project import | ❌ not planned | clean-room + format risk; `.otp` only |

## Project, workflow & ecosystem

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Save/open project (JSON `.otp`, versioned) | ✅ M0 (v1) → M4 (v2, FX chains) | `core::save`, RFC 0006 |
| Unlimited undo/redo over the whole edit | ✅ M0 | `core::undo::History` |
| Render-as-you-save media manifest | ✅ M0 (paths + roles) | `MediaAsset` |
| External editor round-trip | ⬜ post-0.4 | — |
| Templates | ⬜ post-0.4 | — |
| Scripting for batch ops | ✅ M5 | Rhai host, RFC 0010 |
| Plugin ABI (audio blocks / video frames) | ✅ M5 (v0 contract + in-process host) | RFC 0009 |
| One-command installer per platform | ✅ M5 | `cargo xtask installer` |

## Explicit non-goals

- **`.veg` file import/export.** The format is proprietary and undocumented;
  parsing it cleanly is not feasible under the clean-room policy, and partial
  guesses would corrupt user media. `.otp` (open, versioned JSON) is our
  interchange format.
- **Skinning to look like any proprietary product.** We share a *workflow*,
  not a brand.
- **DRM-protected media support.** Never.
- **VST2 hosting.** Withdrawn SDK, non-redistributable — see RFC 0011.
