# Tutorial 4 — Rendering & export (M3)

**Goal:** turn the timeline into deliverables.

## Audio only

```bash
opentrack render project.otp -o master.wav --rate 48000
```

Same math as realtime playback: fades, track FX, pan, master bus.

## Video (MJPEG-AVI + WAV)

```bash
opentrack render project.otp --video out.avi
```

Writes `out.avi` (Motion-JPEG, playable everywhere) and `out.wav` (the
audio master) — both derived from one tick clock, so they stay in sync.
The frame count is exact: `ceil(duration × fps)` is asserted by the render
job and checked against the demuxed container in tests.

Add `--frames stills/` to also write one PNG per composited frame, and
`--quality 90` to raise MJPEG quality.

## AV1 (optional build)

```bash
cargo build --release -p opentrack-cli --features export-av1
opentrack render project.otp --export-av1 -o out.ivf
```

## Installers

```bash
cargo xtask installer
```

produces a platform installer in `dist/` (zip/WiX+MSIX scaffolding on
Windows, AppDir/AppImage on Linux, .app/.dmg on macOS).

Next: [05-scripting-and-plugins.md](05-scripting-and-plugins.md).
