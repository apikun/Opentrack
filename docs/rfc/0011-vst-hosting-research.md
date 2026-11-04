- RFC: 0011
- Title: VST / iPlug-style hosting evaluation (research only)
- Status: Research — no code, no promise

## Summary

The ROADMAP's M5 lists "per-platform VST/iPlug-style hosting evaluation
(research RFC only)". This document is exactly that: what hosting VST2/VST3
or CLAP plugins would cost OpenTrack, what the clean-room and licensing
constraints are, and what a decision would need. **Nothing here ships.**

## License reality check

* **VST2**: the VST2 SDK was withdrawn by Steinberg in 2018; the license
  was never GPL-compatible for redistribution in the first place. Any VST2
  support would need the user to provide the SDK headers locally and would
  be permanently un-distributable. Practical verdict: do not pursue VST2.
* **VST3**: SDK is dual-licensed GPLv3-able (Steinberg VST3 license, GPL-3
  compatible path exists), but the ABI is large (module scan, class
  factories, parameter objects, host callbacks). A clean-room re-derivation
  of the *observable* host/plugin behavior is possible in principle; the
  engineering cost is high and the payoff overlaps with our own
  RFC-0009 ABI.
* **CLAP**: MIT-licensed SDK with a C header, actively maintained, and
  designed for exactly this use case (open-source DAW hosting). If OpenTrack
  ever hosts third-party effects, CLAP is the natural first target.

## Technical surface a host would need (from our side, all missing today)

1. Process-thread model: our realtime engine runs one audio thread with
   lock-free control (RFC-0008); a hosted plugin needs parameter queues,
   latency reporting wired into the master clock, and a crash boundary
   (sandboxed plugin process or `catch_unwind` + kill).
2. Parameter automation integration: our automation lanes are internal
   (`FxInstance`); a host needs bidirectional param sync + GUI embedding or
   a generic parameter UI.
3. Plugin sandbox: RFC-0009 explicitly defers out-of-process hosting; VST3/
   CLAP hosting **requires** that boundary first (native code, untrusted).

## Recommendation

1. Grow the *internal* plugin story (RFC-0009 v1: parameters, GUI-less
   param surface, dynamic loading with a catch boundary).
2. When third-party effects matter, implement **CLAP hosting** behind an
   optional feature (MIT SDK, clean-room compatible) — not VST2/3.
3. Re-evaluate VST3 only if a contributor demonstrates demand the CLAP
   path cannot serve.

Decision owner: maintainer. Status of this RFC does not gate any milestone.
