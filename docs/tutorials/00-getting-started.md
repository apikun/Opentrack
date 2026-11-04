# Tutorial 0 — Getting started

**Goal:** go from a clean clone to a playing, edited, saved project.

1. **Build**
   ```bash
   git clone https://github.com/apikun/Opentrack.git && cd Opentrack
   cargo build --release -p opentrack-cli -p opentrack-app
   ```
   (Or get an installable artifact right away: `cargo xtask installer`.)

2. **Make some media to play with** — the bundled demo generates a real
   project with real audio:
   ```bash
   cargo run -p opentrack-cli -- demo
   ```

3. **Open it in the GUI**
   ```bash
   cargo run -p opentrack-app -- --project opentrack-demo/demo.otp
   ```

4. **Play**: `Space` toggles playback. The playhead is the audio clock, the
   track strips show live peak meters, and the loop region (shift+drag on
   the ruler) cycles.

5. **Edit**: drag events to move, drag their edges to trim, alt-drag to
   slip, `S` splits at the playhead, `Delete` ripple-deletes, `Ctrl+Z`
   undoes. Snapping is on by default.

6. **Save**: `Ctrl+S` writes the `.otp` (open JSON, versioned).

Next: [01-media-and-the-preview.md](01-media-and-the-preview.md).
