- RFC: 0004
- Title: Audio buses, mixing & DSP roadmap
- Status: Accepted (M0: buses + offline mixdown shipped; realtime M2, FX M4)

## Summary

Every audio track owns an `AudioBus` (gain dB, pan, mute/solo, phase invert).
The offline mixer renders events through fades → bus → master with an
equal-power pan law. Realtime playback (M2) and the FX framework (M4) extend
this graph without changing its shape.

## Motivation

The workflow's audio model from public behavior is deliberately simple:
a volume slider and a pan knob per track, fades on event edges, a master
fader — and it must behave identically offline and (eventually) in realtime.
Getting the *math* fixed early, tested, and shared by both paths is the whole
game.

## Specification

### Bus math (shipped, `core::bus`)

- Gain: `db_to_linear(db)` clamped to `[-60, +12] dB`; `linear_to_db` inverse.
- Pan: equal-power law, `pan ∈ [-1, +1]` → `(cos·θ, sin·θ)` style pair
  `equal_power_pan(pan) -> (gain_l, gain_r)`; center = both 1.0/√2 so mono
  power is preserved, hard L/R = full amplitude on one side.
- Audibility: `Track::audible(any_solo)` — if *any* track solos, only soloed
  tracks sound; otherwise mute wins per track.

### Offline mixdown (shipped, `engine::mixer`)

```
for track in audio tracks (bottom-up), if audible:
  for event on track (by position):
    src = open_audio(take.media, AudioSpec { ch, rate })   // RFC 0003
    apply fade_in / fade_out on the event edge regions     // curves below
    apply take.gain_db → track gain_db → equal-power pan
  sum into track accumulator
master = Σ tracks × master.gain_db → clamp soft
report  per-channel peaks (dsp::peaks) with the rendered buffer
```

- Fade curves: linear, fast (x²), smooth (sin²) — selected per event edge.
- Output: interleaved f32 → `wav::encode_s16` → WAV; `RenderedAudio`
  carries the buffer + `Vec<f32>` peaks; `MixIssue` collects non-fatal
  notes (missing media → silence + issue, render continues).

### Realtime (M2 plan)

- The same bus math runs in the cpal callback. `History` revision numbers
  gate rebuilding the mix graph; parameter changes (gain/pan/fades) are
  atomic loads inside the callback — no locks, no allocation.
- Metering: callback publishes peaks to atomics; GUI polls at display rate.

### FX (M4 plan)

- FX chains live on tracks (`core::fx`): descriptors with typed
  `ParamSpec`s (`Domain::{Linear, Logarithmic, ...}`), instances hold
  parameter values validated against specs.
- Contract: `process(&mut [f32], params, sr)` for audio FX; realtime and
  offline paths share the implementation; golden tests enforce bit-equality
  between the two at the f32 stage.

## Non-goals

- Surround buses (post-M4).
- Bus sends/returns before M4 (M0 has master only).
- Loudness normalization (M4 candidate, needs EBU R128 study RFC first).

## Rationale

Equal-power over linear pan: keeping perceived level constant when panning is
what the workflow does audibly; linear pan is a one-line change behind the
same function if a future RFC wants a "law" parameter. f32 end-to-end with a
single s16 quantization at file write keeps render parity trivially testable.

## Acceptance criteria

- Unit tests: pan center yields (0.7071, 0.7071) ±1e-4; hard right yields
  (0, 1); dB clamps at both ends; mute+solo matrix matches the truth table.
- Demo render: two overlapping events on one track produce a measurable
  crossfade (midpoint amplitude ≈ sum of edges), peaks reported match a
  reference scan of the buffer.
- Resampler: 44.1k→48k of a 1 kHz sine keeps frequency within 1 cent and
  peak level within 0.1 dB.
