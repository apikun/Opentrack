# Tutorial 5 — Scripting & plugins (M5)

**Goal:** batch-edit a project with a script; understand the plugin story.

## Scripting (Rhai)

```bash
opentrack script batch.rhai project.otp
```

with `batch.rhai`:

```rhai
// every audio track: slight right pan + a -1 dB limiter
for i in 0..project.track_count() {
    if !project.track_is_video(i) {
        project.set_track_pan(i, 0.15);
        project.add_fx(i, "audio.limiter");
        project.set_fx_param(i, 0, "ceiling_db", -1.0);
    }
}
project.log("batch done");
```

The script sees the live document (`project`), can enumerate tracks/media,
add/move/remove events, tune FX, and save. See RFC-0010 for the full
surface. Scripts are sandboxed batches — no filesystem or network beyond
`save_otp`.

## Plugins (ABI v0)

OpenTrack's plugin contract (RFC-0009) has two domains:

- **audio**: process interleaved f32 blocks in place (master bus);
- **video**: filter packed RGBA8 frames in place (post-composite).

0.4.0 ships the in-process reference host and two builtin examples
(`builtin.stereo_width`, `builtin.tint`); dynamically loaded third-party
plugins come after the host gains a crash boundary (future RFC — the v0
header is frozen today so that work can start the same way ours did:
read the contract, write the host).

## Tutorial video series

Each tutorial in this folder doubles as the outline for one video lesson:
build shot, edit moves on screen, and a rendered before/after. Scripts for
the demos live in the repo (`opentrack-cli/src/demo.rs` builds the sample
session programmatically).
