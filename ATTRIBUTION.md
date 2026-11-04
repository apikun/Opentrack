# Attribution & Third-Party Code

OpenTrack is GPL-3.0-or-later. It builds on the following third-party,
pure-Rust components. Both are dual-licensed MIT OR Apache-2.0, which is
GPL-3.0-compatible; this file records the provenance and license choices
required by those licenses.

## craft-media (bitstream + isobmff)

- **Source:** <https://github.com/storytold/craft-libs> — crate `craft-media`,
  pinned at rev `c5a1cbfb5f9049f9270955caffb5bee5997aebf7` (the same pin the
  upstream h264 crate is tested against).
- **Used for:** MSB-first bit I/O (H.264 NAL parsing) and the ISO BMFF
  (MP4/MOV) demuxer that feeds samples to our decoder.
- **License:** MIT OR Apache-2.0 (copyright "ArtCraft Team and the
  craft-libs contributors").
- **Note:** this dependency carries the **Apache-2.0 patent grant and NOTICE
  obligations**; OpenTrack redistributes it unmodified as a git dependency
  and acknowledges it here per NOTICE requirements.

## filmcraft-h264

- **Source:** <https://github.com/storytold/filmcraft> — crate
  `filmcraft-h264`, pinned at rev `c9be14245a6c9da585ff906a6e0ca9c8200652ea`.
- **Used for:** the clean-room H.264/AVC decoder (CAVLC + CABAC, baseline/
  main/high, 8-bit 4:2:0). OpenTrack calls its public `Decoder` API and
  converts the resulting planar pictures to RGBA in our own code
  (`opentrack-engine::video`).
- **License:** MIT OR Apache-2.0 (copyright the FilmCraft/ArtCraft authors).

## How this fits the clean-room policy

[`CLEANROOM.md`](CLEANROOM.md) forbids proprietary or GPL-incompatible
code. FilmCraft and craft-libs are open-source projects (MIT OR Apache-2.0)
that publish their own clean-room provenance ("written from public specs");
reusing them is ordinary open-source composition, fully compatible with our
policy and our GPL-3.0-or-later license. Their behavior specifications were
derived from public standards (ITU-T H.264, ISO/IEC 14496), not from any
proprietary NLE.

## Runtime-optional, not bundled

- **ffmpeg / ffprobe** (GPL/LGPL, external): never linked or bundled. When
  the user has them installed, `opentrack-engine::backend_ffmpeg` shells out
  for formats the pure-Rust path does not cover. Without them OpenTrack
  still builds, tests and renders.

## zune-jpeg + zune-core

- **Source:** <https://github.com/etemesi254/zune-image> (crates
  `zune-jpeg`, `zune-core`).
- **Used for:** baseline JPEG decoding in the M3 video pipeline (MJPEG-AVI
  frames and JPEG stills → RGBA8).
- **License:** MIT OR Apache-2.0.
- **Version:** 0.5.x (see `Cargo.lock` for the exact build).

## jpeg-encoder

- **Source:** <https://github.com/vstroebel/jpeg-encoder>.
- **Used for:** baseline JPEG encoding for the pure-Rust AVI/MJPEG export.
- **License:** MIT OR Apache-2.0.
- **Version:** 0.6.x.

## png

- **Source:** <https://github.com/image-rs/image-png>.
- **Used for:** PNG still decode (compositor) and PNG frame export (render
  pipeline).
- **License:** MIT OR Apache-2.0.
- **Version:** 0.18.x.

## rhai

- **Source:** <https://github.com/rhaiscript/rhai>.
- **Used for:** the project-level scripting surface (RFC-0010), sandboxed
  VM, `sync` feature.
- **License:** MIT OR Apache-2.0.
- **Version:** 1.19+.

## rav1e (optional, feature `export-av1`)

- **Source:** <https://github.com/xiph/rav1e>.
- **Used for:** optional AV1 export (RFC-0007). Not compiled by default;
  default builds never link it.
- **License:** BSD-2-Clause.
