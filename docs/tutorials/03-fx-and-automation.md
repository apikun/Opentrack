# Tutorial 3 — FX & automation (M4)

**Goal:** compress a voice track, grade a video track, and automate a
parameter.

## The FX rack

The right panel's **FX Rack** section edits one track's chain at a time
(pick the track in the combo). Per instance:

- **on** toggles bypass,
- **↑ / ↓** reorder the chain,
- **remove** deletes it,
- presets apply the factory bank (e.g. *Podcast* for the compressor),
- sliders come straight from the descriptor table.

## Audio FX

| FX | Kind | Notes |
| --- | --- | --- |
| Gain | `audio.gain` | static per-track trim |
| 3-Band EQ | `audio.eq3` | low/mid/high shelves + sweepable mid |
| Compressor | `audio.compressor` | threshold/ratio/attack/release/makeup |
| Limiter | `audio.limiter` | brickwall at the ceiling |
| Reverb Send | `audio.reverb` | Schroeder room, size/damping |

Realtime playback and the offline render run the *same* processors in the
*same* block size — what you hear is what `opentrack render` writes, bit
for bit (tested).

## Automation

Every numeric parameter has an **A** button in the rack. Toggle it to create
a lane, then `+ here` drops a point at the playhead; `×` removes points.
Between points the value interpolates (linear by default). Automation is
sampled per processing block — during playback *and* render.

## Video FX

Video tracks get brightness/contrast, saturation and the baked 1D LUTs
(none / film fade / crush blacks). They apply per event before blending, so
opacity and blend modes still composite the graded result.

Next: [04-render-and-export.md](04-render-and-export.md).
