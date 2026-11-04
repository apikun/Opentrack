- RFC: 0002
- Title: Timeline data model & editing operations
- Status: Accepted (M0 shipped)

## Summary

The timeline is an ordered list of tracks; each track holds events; each event
references a media asset with takes, fades and an in-point. All editing is a
closed set of reversible `EditOp`s executed through `Session`. Time is
integer ticks.

## Motivation

The classic Vegas-era workflow we model from public behavior: unlimited
tracks, events placed anywhere (not glued to neighbors), automatic crossfades
where audio events overlap, per-event fades, takes for alternate readings.
We need a model where *every* user-visible edit is one serializable, reversible
unit — that is what makes undo, autosave journals and (eventually) collab
possible without extra machinery.

## Specification

### Time

- `Ticks(pub i64)` — one timeline instant. `TimeBase { ticks_per_second }`
  resolves ticks↔seconds; `frame_ticks(fps)` gives frame granularity;
  `snap_to_frame` clamps to frame boundaries.
- Negative tick positions are invalid (`EditError::BadPosition`). Splits and
  snaps are pure integer arithmetic — no float drift.

### Tracks & events

- `TrackKind::{Video, Audio}`; video tracks render top-down, audio tracks mix
  bottom-up; video rows sit above audio rows (invariant I2).
- `Track` carries `VideoTrackProps` (height px, opacity, lock, solo, mute,
  `BlendMode`) or `AudioTrackProps` (bus `AudioBus`: gain dB, pan, mute, solo,
  phase invert).
- `Event { position, length, takes: Vec<Take>, active_take, fade_in, fade_out }`
  — an event is a *slot on the timeline*; media lives in takes.
- `Take { media_id, in_point, gain_db, .. }` — in-point means slip edits are
  metadata changes, never data moves.
- Overlapping events on one track are legal; for audio they define a
  crossfade run (`edit::overlaps_on_track` → `Overlap { left, right, start,
  end }`), the same auto-crossfade behavior the workflow is known for.

### Edit operations (closed set, M0)

`AddTrack`, `RemoveTrack`, `AddEvent`, `RemoveEvent`, `MoveEvent` (with
optional track change), `TrimEvent` (edge + new length), `SlipEvent`
(in-point shift), `SplitEvent` (at tick), `RippleDelete` (remove + close gap),
`SetAudioTrackProps`, `SetMaster`.

Every op:

1. validates (returns `EditError` without touching state on failure),
2. captures enough data in itself to `revert` (e.g. `RemoveEvent` embeds the
   removed event; `SplitEvent` embeds both halves' ids),
3. is `serde`-serializable (op-log = autosave journal + future collab).

### Session & snapping

- `Session::exec(op)` → `History::push` (apply-then-record, see RFC 0001);
  `Session` also exposes the ergonomic verbs the GUI uses (`add_track`,
  `add_event`, `split_event`, `trim_event`, …) that build ops internally.
- `snap_time(project, t, ignore, opts)` snaps to: frame grid, event edges of
  *other* events, and time 0; threshold in ticks, `ignore` excludes the event
  being dragged.

## Non-goals (M0)

- Group/compound ops (M1: `EditOp::Batch(Vec<EditOp>)`).
- Automation envelopes (M4, RFC 0004).
- Per-event FX (chain lives on tracks in M0).

## Rationale

Closed op set vs. free mutation: a closed set is auditable, serializable and
testable — 24 unit tests cover apply+revert of every op. Open mutation would
have made undo a snapshot system and the journal meaningless. Takes-as-list on
the event (rather than media versions) matches the observed workflow where a
take switch is a click, not a re-import.

## Acceptance criteria

- Every op has apply + revert unit tests; undo/redo round-trips restore
  bit-identical projects (checked via `.otp` serialization equality).
- `RippleDelete` on a middle event shortens total duration by exactly the
  removed event's length; tests prove it.
- Snapping ignores the dragged event's own edges; test with `ignore`.
- Overlap detection returns the same run whether events are added left-first
  or right-first.
