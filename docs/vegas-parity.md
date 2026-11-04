# Workflow Parity Matrix

How the classic Vegas-era editing workflow maps onto OpenTrack. This matrix is
the public-behavior basis for our clean-room work: every "Vegas workflow"
feature listed here is described from publicly observable behavior (manuals,
tutorials, community discussion), specified in an RFC, then implemented.
Status: ✅ done · 🚧 partial · ⬜ planned · ❌ not planned (with reason).

Legend: RFC links go to `docs/rfc/`.

## Timeline & events

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Unlimited video tracks (stack top-down) | ✅ | `core::timeline`, RFC 0002 |
| Unlimited audio tracks (mix bottom-up) | ✅ | `core::timeline`, RFC 0002 |
| Events placed anywhere on a track (drag-anywhere) | ✅ model / 🚧 drag | RFC 0002 / M1 |
| Event takes (multiple takes per event, switchable) | ✅ model / 🚧 UI | `Event::takes`, M1 |
| Split event at cursor (S) | ✅ model / 🚧 keybind | `EditOp::SplitEvent`, M1 |
| Trim event edges | ✅ model / 🚧 drag | `EditOp::TrimEvent`, M1 |
| Slip event content (media in-point without moving event) | ✅ | `EditOp::SlipEvent` |
| Ripple delete (close the gap) | ✅ model | `EditOp::RippleDelete` |
| Auto crossfade when audio events overlap | ✅ | `edit::overlaps_on_track`, RFC 0004 |
| Fade in/out per event with curve choice | ✅ model | `core::timeline::Fade` |
| Snap to grid + event edges, toggleable threshold | ✅ | `edit::snap_time` |
| Loop region on ruler | ⬜ M2 | RFC 0005 |
| Marker / region list | ⬜ M1 | RFC 0002 §future |
| Track order drag-reorder | ⬜ M1 | — |
| Envelopes (automation) on events/tracks | ⬜ M4 | RFC 0004 |

## Audio mixing

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Per-track volume (dB) | ✅ | `core::bus::AudioBus` |
| Per-track pan (equal-power law) | ✅ | `bus::equal_power_pan` |
| Mute / solo (solo overrides mute) | ✅ | `Track::audible` |
| Master bus with volume | ✅ | `EditOp::SetMaster` |
| Peak metering per track + master | ✅ offline / ⬜ realtime M2 | `dsp::peaks` |
| Bus routing (tracks → custom buses → master) | ⬜ M4 | RFC 0004 |
| 5.1 surround | ❌ not until after M4 | scope |

## Media & codecs

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| WAV decode (PCM 8/16/24/32, float32/64) — pure Rust | ✅ | `engine::wav` |
| Anything else via ffmpeg bridge (runtime-detected) | ✅ | `engine::backend_ffmpeg` |
| Media pool with asset paths + labels | ✅ | `core::project::MediaPool` |
| Offline render to WAV 16-bit | ✅ | `engine::mixer` |
| Video decode (MJPEG AVI intermediate, pure Rust) | ⬜ M3 | RFC 0007 |
| AV1 export via rav1e (pure Rust) | ⬜ M3 | RFC 0007 |
| `.veg` project import | ❌ not planned | clean-room + format risk; `.otp` only |

## Project & workflow

| Workflow behavior | OpenTrack status | Where |
| --- | --- | --- |
| Save/open project (JSON `.otp`, versioned) | ✅ | `core::save`, RFC 0006 |
| Unlimited undo/redo over the whole edit | ✅ | `core::undo::History` |
| Render-as-you-save media manifest | ✅ (paths + roles) | `MediaAsset` |
| External editor round-trip | ⬜ M5 | — |
| Templates | ⬜ M5 | — |

## Explicit non-goals

- **`.veg` file import/export.** The format is proprietary and undocumented;
  parsing it cleanly is not feasible under the clean-room policy, and partial
  guesses would corrupt user media. `.otp` (open, versioned JSON) is our
  interchange format.
- **Skinning to look like any proprietary product.** We share a *workflow*,
  not a brand.
- **DRM-protected media support.** Never.
