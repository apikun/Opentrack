# RFC 0008 — Realtime audio playback engine

- **Status:** Implemented (M2 / 0.3.0)
- **Related:** RFC-0003 (codec backends), RFC-0004 (audio buses & DSP),
  ROADMAP M2

## Summary

OpenTrack plays the timeline in real time through the platform output device
(WASAPI / CoreAudio / ALSA) with the same mix math as the offline renderer.
The design separates *decode-ahead* (a worker thread), *transport state*
(lock-free atomics), and *output* (a tiny, allocation-free render function
shared by the device callback, the tests and the CI smoke profile).

## Motivation

M0 shipped offline rendering; M1 shipped interactive editing with a silent
wall-clock playhead. Editing without hearing is not editing. The M2 goals:

1. Play/pause/stop/loop transport locked to the audio clock (the playhead is
   where the audio *is*, not where a frame timer hopes it is).
2. Track gain/pan/mute/solo and master changes apply **live**, without
   rebuilding the graph, without locks, and without zipper noise.
3. Peak meters on every track strip and the master bus.
4. Scrubbing that auditions audio with buffered lookahead.
5. A dropout-free 10-minute 8-track session as the CI-verifiable acceptance
   criterion.

## Specification

### Threading model

```
GUI thread            worker thread ("opentrack-audio-worker")      audio callback (RT)
──────────            ───────────────────────────────────────       ───────────────────
set_project() ─mpsc─▶ rebuild graph (EventSpecs, decoders lazy)
play()/pause()/stop() ─mpsc─▶ transport transitions
seek_secs() ─mpsc─▶ generation++, new block queue, re-prime
set_* params ──atomics──▶ (read every block by the callback)
                                │ decode-ahead per audio track:
                                │ AudioSource chunks → StreamResampler →
                                │ stereo, event gain + fades pre-applied
                                ▼
                          Spsc<Arc<MixBlock>> (depth 3 ≈ 60 ms)
                                │ pop
                                ▼
              ◀─ position/peaks atomics ─ RenderState::render():
                                          strip gain/pan smoothing (per-sample
                                          linear ramp), master smoothing, loop
                                          wrap, underrun counting, peak meters
```

Invariants:

- The callback **never locks** (one exception: a single mutex acquisition per
  *generation change*, i.e. per seek/rebuild, to swap the block queue),
  never allocates, and never decodes.
- The worker is the single producer, the callback the single consumer of each
  block queue (hand-rolled bounded SPSC over `Arc<MixBlock>`).
- `Shared.position` is written by the callback while playing and by the worker
  on seek/stop; the GUI only reads it.

### Mix graph

The worker keeps one `TrackGraph` per audio track slot: a sorted list of
`EventSpec`s (start/length in engine frames, gain, fade lengths — all
converted exactly like the offline mixer), plus lazily opened decoders.
Events overlapping the mix window are decoded in chunks, resampled with a
stateful linear `StreamResampler` (chunk-boundary-transparent, bit-identical
to the offline resampler over the same stream), expanded to stereo, and
pre-multiplied by event gain and linear fades. What lands in a block is
therefore "post-event, pre-strip" audio; the strip and master gains are
applied live by the callback.

The graph is rebuilt whenever the GUI observes a new `History::revision()`
(a monotonic counter bumped by every applied/undone/redone op). A rebuild
keeps the transport position and re-primes buffers; it never drops to a
stopped state.

### Parameters

All live parameters are fixed-point atomics (dB ×100, pan ×1000, flag bits).
The callback reads targets per block and ramps per sample from the last
applied value — no zipper noise, no locks, GUI writes at 60 Hz are safe.
Mute/solo semantics match the offline mixer: audible = `!mute && (!any_solo ||
solo)`.

### Transport and loop

Blocks are strictly ordered: each block records its `start_frame` and
`end_frame`, and the callback advances `position` to `end_frame` of every
fully consumed block. The worker tracks its own cursor (`next_start`) and:

- wraps to `loop_start` when the cursor reaches `loop_end` (blocks are
  truncated at the loop boundary so the wrap happens between blocks);
- stops the transport (`playing=false`, `finished=true`) when the cursor
  reaches the end of the timeline without a loop.

The callback detects a backward jump (loop wrap) by comparing
`block.start_frame` to `position` and adopts the block's timeline. Seek bumps
a **generation** counter and installs a fresh queue; blocks from an old
generation are dropped by the callback, never mixed.

### Underruns and priming

The callback counts an *underrun* whenever it is playing, primed, and finds
an empty queue (it renders silence). Play/seek/re-build prime the queue to
its full depth before setting the `ready` flag, so startup and scrubbing do
not count as dropouts. `RenderState::render` is shared verbatim by the
device callback and the smoke profile — what is tested is what ships.

### Latency compensation

The displayed playhead is `feed_position − output_latency`, i.e. what the
listener hears *now*; the video preview uses the same value so picture and
sound stay aligned. `output_latency` = device buffer (measured when the host
reports a fixed buffer size) + a documented per-host period estimate:

| Host API  | Estimated period | Notes |
| --------- | ---------------- | ----- |
| WASAPI    | 10 ms            | shared-mode period; Windows primary target |
| CoreAudio | 10 ms            | default HAL IOProc |
| ALSA      | 20 ms            | two-period default |
| others    | 20 ms            | conservative default |

(`opentrack_engine::realtime::estimated_output_latency_secs`.)

### Device layer

`cpal` is an **optional** dependency behind the engine feature `out-cpal`
(enabled by the app's default `audio` feature). The adapter prefers an f32
stereo configuration at the project rate and otherwise picks the closest
supported rate — the mix graph always runs at the *device* rate and sources
resample in the worker. Non-stereo devices are fed a stereo downmix scatter.
Headless builds compile the whole engine with `--no-default-features` and
report playback as unavailable instead of failing.

## Non-goals (M2)

- Gapless *loop* wraps: the loop re-primes buffers at the wrap, which can
  leave a short (< 60 ms) hole; closing it needs dual-cursor pre-roll (M2.1).
- A real sample-accurate device-latency measurement on WASAPI (the 10 ms
  constant is a documented estimate).
- Automation lanes, FX inserts, GPU video presentation (M3/M4).

## Acceptance criteria

1. `cargo test --workspace` green on all three CI platforms, including:
   - realtime vs **offline parity**: the same 2-track crossfade session
     rendered through `RenderState` matches `render_audio` (max diff < 1e-4);
   - transport: play/pause/freeze, seek, stop-to-zero, end-of-timeline stop;
   - loop: wrap moves the playhead backwards and never passes loop end + 1
     block;
   - smoothing: a −60 dB gain change fades monotonically to silence with no
     upward jump;
   - meters: track/master peaks equal the analytically expected amplitude and
     reset on read;
   - underruns: counted only when playing + primed + empty queue.
2. `opentrack smoke --minutes 10` — the 8-track, 48 kHz, 10-minute session —
   plays the full span through the *real* mix graph and callback code with
   **zero underruns and zero decode issues**, in CI (producer-paced) and
   locally (`--realtime`, wall-clock paced).
3. The GUI plays the demo session audibly on Windows/macOS/Linux with the
   default build; a machine without audio hardware still opens the shell and
   shows the reason in the status bar.

## Open questions

- When the loop region crosses a compressed-source boundary, decode cost at
  the wrap may spike; a pre-rolled shadow cursor is the M2.1 candidate.
- Per-track ring *sample* buffers (instead of block queues) would cut
  allocation churn; deferred until profiling says it matters.
