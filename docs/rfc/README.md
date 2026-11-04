# RFCs — OpenTrack's blueprint process

OpenTrack is blueprint-first: behavior is specified here, reviewed, and only
then implemented. RFCs are the project's memory — if you want to know *why*
something is the way it is, start here.

## Index

| RFC | Title | Status |
| --- | --- | --- |
| [0001](0001-architecture-overview.md) | Architecture overview & crate boundaries | Accepted (M0) |
| [0002](0002-timeline-data-model.md) | Timeline data model & editing operations | Accepted (M0) |
| [0003](0003-codec-backends.md) | Codec backends: pure Rust + ffmpeg bridge | Accepted (M0) |
| [0004](0004-audio-bus-dsp.md) | Audio buses, mixing & DSP roadmap | Accepted (M0) |
| [0005](0005-gui-egui-shell.md) | GUI shell (egui) layout & interaction plan | Accepted (M0; M1 interactions amended) |
| [0006](0006-project-file-format.md) | `.otp` project file format | Accepted (M0) |
| [0007](0007-video-render-pipeline.md) | Video compositing & render pipeline | Draft — decode half shipped early (M1/0.2.0) |
| [0008](0008-realtime-audio.md) | Realtime audio playback engine | Implemented (M2/0.3.0) |

## Lifecycle

```
Draft ──▶ Accepted ──▶ Implemented ──▶ (Superseded)
   │
   └──▶ Rejected (with reasons kept on file)
```

- **Draft**: PR against `docs/rfc/`. Numbering: next integer, never reused.
- **Accepted**: at least one maintainer review + one non-author review; the
  clean-room checklist in [`CLEANROOM.md`](../../CLEANROOM.md) §2 must pass.
- **Implemented**: the RFC's acceptance criteria demonstrably pass. The RFC
  then records the shipped status; behavioral changes afterwards require a
  new RFC or an explicit amendment section.
- **Superseded**: leave the file, mark it, link the replacement.

## Writing one

Copy `0000-template` style from any existing RFC: Summary, Motivation,
Specification (the behavior, in our own words), Non-goals, Rationale,
Acceptance criteria, Open questions. Keep it implementable by someone who has
never seen the product it is inspired by — that is the whole clean-room point.
