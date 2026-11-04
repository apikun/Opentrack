- RFC: 0007
- Title: Video compositing & render pipeline
- Status: Draft (target milestone M3) — decode half shipped early in M1 (0.2.0)

## Summary

Video enters OpenTrack in M3: a `VideoDecoder` trait with a pure-Rust
Motion-JPEG AVI path, an ffmpeg bridge for the rest; a CPU compositor that
stacks tracks top-down honoring opacity/blend/fades; GPU presentation in the
egui canvas; and a render pipeline that can produce a pure-Rust AVI/MJPEG
intermediate plus optional AV1 (rav1e) export behind a feature flag.

## Motivation

The workflow people expect: drop stills/clips on video tracks, stack them,
fade them, and render. Clean-room constraints push us to define the *simplest
honest* pipeline: MJPEG (each frame is a JPEG) is decodable and encodable in
pure Rust with existing crates, making a zero-native-dependency video path
real rather than aspirational; everything else delegates to ffmpeg.

## Specification (proposal)

### Decode

```rust
pub trait VideoDecoder: Send {
    fn info(&self) -> VideoInfo;                    // w,h,fps,frame_count
    fn seek_frame(&mut self, frame: u64) -> Result<(), VideoError>;
    fn next_frame(&mut self) -> Result<Frame, VideoError>;  // RGBA8
}
```

> **Amendment (M1 / 0.2.0) — decode shipped ahead of this RFC.** The preview
> path landed early with a shape slightly different from the trait above:
> `opentrack-engine::video::VideoDecoder` is a concrete type (not yet the
> trait), opened from an MP4/MOV, with `decode_at(time_secs)` random access,
> a streaming GOP decode run (O(1) amortized forward), an LRU frame cache,
> and our own YUV 4:2:0 → RGBA conversion honoring BT.601/709 + range.
> The demuxer is `craft-media`'s isobmff and the AVC decoder is
> `filmcraft-h264` — both MIT OR Apache-2.0, pinned, attributed in
> `ATTRIBUTION.md`. The trait consolidation, MJPEG-AVI path and the rest of
> this RFC remain M3 work.

- `PureRustBackend`: AVI container walker (RIFF/`movi` list), MJPEG frames →
  JPEG baseline decode → RGBA8. JPEG decode may use a pure-Rust crate
  (e.g. zune-jpeg class); nothing links to libav.
- `FfmpegBridge`: `ffmpeg -i in -f rawvideo -pix_fmt rgba -` piped, same
  trait, same errors-as-values discipline as audio (RFC 0003).
- Frame cache: `HashMap<(MediaId, u64), CachedFrame>` with byte budget
  (default 256 MiB) and LRU eviction; keyed access keeps seeks cheap.

### Composite

For output frame F at time t (project tick → frame via `TimeBase`):

1. Collect visible video events (top-down track order).
2. Per event: source frame = (media in-point + (t − event.position)) →
   decoder frame index; apply fade_in/out → alpha multiplier.
3. Blend onto canvas by `BlendMode` (M0 model ships Normal; M3 adds
   Add/Multiply/Screen/Crossfade) with per-track `opacity`.
4. Preview scales: composite at 1/2 or 1/4 into the same canvas size, then
   upscale at presentation — never re-decode at more than preview size when
   the user asks for it.

### Render

- Master clock: one tick source; every stage derives frames/audio from it
  (no per-stage clocks — the sync discipline).
- Pipeline: composite RGBA frames → JPEG encode → AVI/MJPEG mux (pure Rust,
  RIFF writer reusing the WAV chunk discipline) + WAV audio from the M0
  mixer → optional `--export av1` runs frames through rav1e in a
  `feature = "export-av1"` build (documented as optional; CI keeps the
  default build pure).
- Frame-accurate acceptance: output frame count == ceil(duration × fps).

### Playback (M3)

- Decode-ahead ring of N uploaded textures (N≈8); egui paints the newest
  presentable frame; audio (M2 clock) leads, video follows with a documented
  per-platform latency constant. Drops are counted and shown in the status
  bar, never hidden.

## Non-goals

- Hardware encode/decode in M3 (NVENC/VAAPI study RFC later).
- HDR / wide gamut (post-M4).
- Compositing in shaders for M3 — CPU reference first, GPU acceleration is
  an optimization RFC after parity tests exist.

## Rationale

Why RGBA8 CPU compositing first: parity-testable against a golden frame set,
trivially unit-testable (blend math is pure functions), and fast enough at
1080p 1/2-preview on any modern machine. Why MJPEG first: every frame is an
independent JPEG — no inter-frame state, no B-frames, crash-safe, and the
same JPEG codecs serve both directions.

## Acceptance criteria

- Golden tests: known 4-frame MJPEG AVI → RGBA8 buffers match references
  byte-for-byte after decode.
- Composite test: two stacked events with fades produce the documented
  alpha ramp at the documented ticks.
- Render test: 6.0 s session at 30 fps → AVI with exactly 180 frames +
  WAV whose sample count matches the M0 mixer output.
- Feature-gated: default `cargo build --no-default-features` (CI) still
  builds and passes everything without rav1e.
