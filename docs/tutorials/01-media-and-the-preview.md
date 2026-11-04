# Tutorial 1 — Media, import & the composited preview

**Goal:** put your own video/stills/audio on the timeline and see them
composite.

## Import

Drag files from your file manager anywhere onto the window, or use
*File → Import media*. OpenTrack imports:

| Content | Pure-Rust decode | Bridge |
| --- | --- | --- |
| WAV audio (PCM 8/16/24/32, float) | ✅ | — |
| MP4/MOV with H.264 video | ✅ | — |
| MJPEG-AVI video | ✅ (M3) | — |
| PNG / JPEG / BMP stills | ✅ (M3) | — |
| Anything else (MP3, MKV, HEVC…) | — | ✅ ffmpeg |

`opentrack probe FILE` shows what the backend chain detected.

## Place & preview

Double-click an asset in the pool (or use its *place* button) to put it at
the playhead on the right kind of track. The preview pane shows the
**composited** frame: every video track, top-down, with each track's
opacity, blend mode and event fades — not just one clip.

The `scale 1/n` button switches preview resolution (1/1 → 1/2 → 1/4); lower
scales composite fewer pixels and decode the same sources, so they help on
big projects.

## Stacking & blending

Video track 1 is the *bottom* of the stack; tracks above composite onto it.
Select a track in the **FX Rack** panel and use `set_composite_level`-style
controls through the track header, or script it (tutorial 4). Fade handles
on each event ramp its alpha.

Next: [02-editing-like-vegas.md](02-editing-like-vegas.md).
