- RFC: 0003
- Title: Codec backends: pure Rust + ffmpeg bridge
- Status: Accepted (M0 shipped for audio)

## Summary

All media access goes through two traits — `AudioSource` (a resample-to-target
stream) and `CodecBackend` (probes + opens sources). The default backend chain
is: `PureRustBackend` (zero-dependency WAV) → `FfmpegBridge` (runtime-detected
ffmpeg/ffprobe). Nothing is required at build time except Rust.

## Motivation

The workflow needs "drop any file in, it plays". A pure-Rust core keeps the
promise that a fresh clone builds and renders with nothing but `cargo` (CI
proof: Linux CI has no ffmpeg). At the same time, hand-writing H.264 decode is
a decade of work — so everything we cannot do natively delegates to the
user's ffmpeg, if present, as a *bridge*, not a dependency.

## Specification

### Traits

```rust
pub trait AudioSource: Send {
    fn spec(&self) -> AudioSpec;                       // channels, sample_rate
    fn next_block(&mut self, max_frames: usize)
        -> Result<Vec<f32>, CodecError>;               // interleaved f32
}

pub trait CodecBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn probe(&self, path: &Path) -> Result<MediaInfo, CodecError>;
    fn open_audio(&self, path: &Path, target: &AudioSpec)
        -> Result<Box<dyn AudioSource>, CodecError>;
}
```

- `MediaInfo` lists streams (`StreamInfo { kind, codec_name, channels,
  sample_rate, .. }`); `first_audio()` picks the render default.
- Backends must be **fail-clean**: absent tool ≠ panic. `FfmpegBridge` on a
  machine without ffmpeg returns `CodecError::MissingTool("ffmpeg")`; the
  chain then simply reports `Unsupported` overall. (Tested.)

### PureRustBackend (M0 scope)

WAV demux/decode: RIFF parsing, `fmt`/`data` chunk discovery, PCM
8/16/24/32-bit and IEEE float 32/64, mono→stereo and channel duplication,
linear resampling to the target rate (`dsp::resample_linear`). Encoding side:
16-bit PCM stereo (`wav::encode_s16`) for render output. Extensible by
adding decoders; each gets a round-trip test.

### FfmpegBridge

- Located once per call via `PATH` (or explicit paths); runs
  `ffprobe -print_format json -show_streams` for probing, and
  `ffmpeg -i in -f f32le -acodec pcm_f32le -` piped to stdout for decode,
  streaming into `FfmpegAudioSource`.
- std-only (no shell-escape surprises, args passed as argv array).

### Backend chain

`default_backends()` = `[PureRustBackend, (FfmpegBridge::auto() if found)]`.
`open_audio_any(paths, spec)` walks the chain and returns the first success,
aggregating per-backend errors for diagnostics (`CodecError::Unsupported`).

## Non-goals

- No linking of libav* — process bridge only.
- No writing of compressed formats in M0 (video render pipeline = RFC 0007).
- No metadata tagging/writing.

## Rationale

Trait + chain (vs. enum of codecs): new backends (rav1e export, MJPEG-AVI
video in M3) slot in without touching call sites. Probe-before-open keeps
error messages honest. f32 pipeline everywhere above decode: one conversion
point, no per-backend gain staging.

## Acceptance criteria

- Round-trip tests: synthesize PCM in every supported bit depth → decode →
  encode s16 → re-decode, sample counts and peaks match.
- With ffmpeg absent, `cargo test --workspace` still passes (bridge tests
  assert `MissingTool`-derived clean error).
- `opentrack probe <wav>` prints stream info from the pure-Rust backend even
  when ffmpeg is installed (chain order = pure Rust first).
