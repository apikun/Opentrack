- RFC: 0010
- Title: Project-level scripting (Rhai)
- Status: Accepted — shipped in 0.4.0

## Summary

Batch operations on a project document get a scripting surface hosted by
Rhai (pure Rust, MIT OR Apache-2.0, sandboxed VM): enumerate the media pool
and tracks, add/move/remove events, set track/FX parameters, save. Lua was
considered and rejected for 0.4: Rhai is a single pure-Rust dependency and
its VM is sandboxed by construction (no filesystem/network access beyond
what the host explicitly registers).

## Surface (shipped)

The script sees one live variable, `project`, with these methods:

Reads:

* `track_count() -> i64`, `track_is_video(i) -> bool`, `track_name(i) -> String`
* `event_count(track) -> i64`
* `media_count() -> i64`, `media_label(i) -> String`,
  `media_duration_s(label) -> f64`
* `duration_s() -> f64`

Mutations (each is one report step):

* `add_media(path, label) -> String` — registers a file in the pool
  (probe order: audio backends → video pure-Rust/bridge → still) and
  returns its label (empty on failure); the label is what `add_event`
  references
* `add_audio_track(name) -> i64` (appends), `add_video_track(name) -> i64`
  (inserts at the top — video track 0 is the top track)
* `rename_track(i, name)`
* `add_event(track, media_label, start_s, len_s) -> bool`
* `remove_event(track, index) -> bool`
* `move_event(track, index, new_start_s) -> bool`
* `set_event_gain_db(track, index, db)`, `set_event_fades_s(track, index, in, out)`
* `set_track_gain_db(track, db)`, `set_track_pan(track, pan)`
* `set_composite_level(track, level)` (video tracks)
* `add_fx(track, kind) -> bool`, `set_fx_enabled(track, fx, on)`,
  `set_fx_param(track, fx, param, value)`
* `save_otp(path) -> bool`

Plus `log(text)` (free function *or* method style) which lands in the run
report.

## Execution model

* Scripts are **batches, not live**: they run to completion against a
  snapshot of the document and then the result is written back. No
  callbacks into the UI, no undo integration (a script run is one
  journalable action recorded by the CLI report).
* The CLI entry point is `opentrack script FILE.rhai PROJECT.otp`; the run
  saves the mutated project over the input path (scripts that want copies
  call `save_otp` themselves first).
* Sandboxing: the Rhai engine is created with defaults (no file/network
  access); only the registered methods above can touch the world.

## Example

```rhai
// pan every audio track slightly right and brickwall-limit it
for i in 0..project.track_count() {
    if !project.track_is_video(i) {
        project.set_track_pan(i, 0.15);
        project.add_fx(i, "audio.limiter");
        project.set_fx_param(i, 0, "ceiling_db", -1.0);
    }
}
project.log("batch: panned + limited");
```

## Non-goals

* Per-frame/per-sample scripting (this is a *batch* surface).
* GUI automation.
* Lua/WASM hosts (revisit when a contributor needs an existing Lua lib).
