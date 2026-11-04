- RFC: 0001
- Title: Architecture overview & crate boundaries
- Status: Accepted (M0 shipped)

## Summary

OpenTrack is a cargo workspace of four crates with strictly one-way
dependencies: `opentrack-core` (model) ← `opentrack-engine` (media) ←
`opentrack-cli` (CLI) ← `opentrack-app` (GUI). This RFC fixes the boundaries,
the dependency direction, and the reasons for each split.

## Motivation

An NLE dies by entanglement: if the timeline model knows about the GPU, the
audio callback knows about the GUI, or the file format knows about the mixer,
testing any one thing requires all the others. We want:

- headless testing of the full edit→save→render path in CI (no display, no
  audio device, no ffmpeg);
- the freedom to replace the GUI toolkit later without touching the model;
- the option to embed the engine in other front-ends (CLI render farm,
  server-side rendering) unchanged.

## Specification

### Crates

| Crate | Owns | May depend on |
| --- | --- | --- |
| `opentrack-core` | `Project`, timeline, edit ops, undo, buses, FX descriptors, `.otp` I/O | `serde`, `serde_json`, `thiserror` |
| `opentrack-engine` | `CodecBackend`/`AudioSource` traits, WAV codec, ffmpeg bridge, DSP, offline mixer, `RenderJob` | core |
| `opentrack-cli` | `opentrack` binary: `demo`/`new`/`probe`/`render`/`gui` | core, engine |
| `opentrack-app` | eframe/egui shell | core, engine |

Rules:

1. Dependencies point strictly left→right in the table; `core` never appears
   as a dependency of anything below it in the stack, and nothing above
   `engine` may be referenced by it.
2. `core` performs no process spawning and no filesystem access beyond the
   `.otp` read/write path.
3. The GUI is a *view over the model*: it constructs `EditOp`s and paints
   `Project`. All mutation flows through `Session::exec`.

### Mutation pipeline

```
UI event → EditOp → Session::exec → History { apply, push } → Project
                                   ↘ Err → project untouched, toast/flash
```

`History::push` applies the op before recording; a failed apply leaves no
trace on either the project or the undo stack. `EditOp::revert` rebuilds the
pre-state from data embedded in the op (e.g. `RemoveEvent` stores the whole
event), so undo needs no snapshots.

### Codec backend chain

`open_audio_any(path)` walks a `Vec<Arc<dyn CodecBackend>>` — pure-Rust WAV
backend first, then the ffmpeg bridge when its executables are found at
runtime — and uses the first backend whose `can_open(probe)` accepts. If none
accepts, the caller gets a `CodecError::Unsupported` carrying what each
backend reported.

## Non-goals

- No plugin ABI yet (M5, separate RFC).
- No async runtime in M0; `RenderJob` is synchronous by design.
- No IPC/remote session; `EditOp` serialization makes it *possible*, not
  promised.

## Rationale

Why not one crate? Compile times and API discipline. Why is `engine` separate
from `core`? The model must stay expressible without any media backend
(project files reference media by path; decode only happens at render/play).
Why egui in its own `app` crate rather than inside `cli`? The CLI ships
headless to servers; the GUI drags in eframe — different dependency worlds.

## Acceptance criteria

- `cargo tree -e normal` shows no back-edge from `core` to anything else.
- `cargo test --workspace` passes on a machine with no ffmpeg installed
  (bridge tests assert a clean error, not a panic).
- `opentrack demo` end-to-end: build session via `Session` verbs, save `.otp`,
  render WAV, report peaks — zero non-Rust runtime deps.

## Implementation notes (shipped)

Workspace layout and invariants I1–I7 live in
[`ARCHITECTURE.md`](../../ARCHITECTURE.md); they mirror this RFC and both
reference each other so they cannot drift silently.
