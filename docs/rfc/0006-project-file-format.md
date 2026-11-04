- RFC: 0006
- Title: `.otp` project file format
- Status: Accepted (M0 shipped)

## Summary

`.otp` (OpenTrack Project) is a UTF-8 JSON document, versioned by a top-level
integer. Files from *newer* format versions are refused with a clear error;
older versions load through explicit migration steps. Nothing is binary,
nothing is compressed, diffs stay readable.

## Motivation

The workflow needs a dependable project file, and the clean-room policy rules
out reverse-engineering proprietary formats (see
[`docs/vegas-parity.md`](../vegas-parity.md) non-goals). An open JSON format
gives us: trivially debuggable sessions, no format drift surprises, git-friendly
diffs for template projects, and a zero-dependency read/write path.

## Specification

### Envelope

```json
{
  "format_version": 1,
  "project": { ... }
}
```

- `format_version: u32` — bumped **only** when reading old files would be
  ambiguous; additive fields do *not* bump it.
- Load rule (invariant I5): `file.version > FORMAT_VERSION` →
  `SaveError::VersionTooNew { found, supported }` — refuse, never guess.
  `file.version < FORMAT_VERSION` → run the registered migration ladder
  (v0→v1→…→current), each step a pure function with tests.

### `project` object (v1)

| Field | Type | Notes |
| --- | --- | --- |
| `name` | string | display name |
| `created_at` / `modified_at` | RFC 3339 string | updated on save |
| `next_id` | u64 | monotonic ID allocator watermark |
| `timebase` | `{ ticks_per_second: i64 }` | default 48000 |
| `video` | `{ width, height, fps_num, fps_den, .. }` | `VideoSettings` |
| `media_pool` | array of `MediaAsset` | id, path, label, kind, duration |
| `tracks` | array of `Track` | kind, id, name, props, events |
| `master` | `AudioBus` | gain dB, pan, mute, solo |

- IDs (`TrackId`, `EventId`, `MediaId`, `TakeId`) serialize as their u64
  values inside a typed newtype; `next_id` guarantees freshness after load.
- `Event` serializes with position/length as ticks (i64), takes as an array
  with an `active_take` index, fades as `{ length_ticks, curve }`.

### Rules

1. Unknown fields are **ignored** on load (forward-compatible additions).
2. Paths are stored as given (relative preferred); missing media on load is
   a warning surfaced per asset, not an error — offline render reports it as
   a `MixIssue` and renders silence.
3. Save is atomic: write `path.otp.tmp`, then rename (crash-safe).
4. Serialization is the *same* code path as tests: undo round-trip tests
   compare projects by serializing both and comparing strings — format and
   model cannot drift apart silently.

## Non-goals

- Embedding media in the project file (archive/bundle = M5 RFC if wanted).
- Binary formats for size; JSON gz is a user-level choice, not ours.
- Multi-document/pointer formats (project references are plain paths).

## Rationale

JSON + serde over anything custom: zero dependencies, immutable tooling,
ecosystem-wide tooling for free. Refuse-newer over best-effort-load: silently
mis-loading a session destroys work; a loud error plus upgrade instructions
destroys nothing.

## Acceptance criteria

- Round-trip: build a project → save → load → save again → byte-identical
  (map ordering aside, tested with canonical serde ordering).
- A file with `format_version: 999` errors with both versions in the message.
- An unknown extra field in `project` loads without warning.
- Crash test: truncated `.tmp` file is ignored; the real file is untouched.
