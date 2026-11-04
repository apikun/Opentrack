- RFC: 0009
- Title: Plugin ABI v0 (audio blocks / video frames)
- Status: Accepted — in-process reference host shipped (0.4.0); dynamic loading deferred

## Summary

Plugin ABI v0 defines the *contract* OpenTrack makes with third-party
effects, in two domains: audio plugins process f32 blocks in place, video
plugins filter RGBA8 frames in place. 0.4.0 ships the host-side seams and
two builtin example plugins ([`opentrack_engine::plugin`]) so the plumbing
is exercised by tests. Dynamic loading (dlopen/dll of a C-ABI library) is
deliberately **not** shipped yet — the header below is frozen by this RFC
and reviewed before any third-party promise, per the clean-room policy.

## The v0 contract (frozen by this RFC)

### Versioning

* `PLUGIN_ABI_VERSION = 0` — the host refuses plugins exporting any other
  major version; compatible minor growth adds fields *at the end* of every
  struct with `size` guards.
* Handshake: the plugin exports `ot_plugin_abi_version() -> u32`; the host
  loads only when it matches.

### Audio domain

```c
typedef struct ot_audio_plugin {
    const char* name;                       // stable display name
    void*       user;
    // in-place process of one interleaved f32 block
    void (*process)(void* user, float* samples, uint32_t frames,
                    uint32_t channels, uint32_t sample_rate);
    void (*destroy)(void* user);
} ot_audio_plugin;
```

* Blocks are interleaved f32, `[-1, 1]` nominal, and may be any length;
  plugins must be sample-exact stateful (no per-block reset assumptions).
* In-place processing: output length == input length. No bypass, no
  parameter surface in v0 (a future minor version may append a param table).

### Video domain

```c
typedef struct ot_video_frame {
    uint8_t* rgba;        // packed RGBA8, straight alpha, top-down
    uint32_t width, height, stride_bytes;
    double   pts_secs;    // presentation time
} ot_video_frame;

typedef struct ot_video_plugin {
    const char* name;
    void*       user;
    void (*filter)(void* user, ot_video_frame* frame);
    void (*destroy)(void* user);
} ot_video_plugin;
```

* Frames arrive composited (post-FX), full project resolution; plugins may
  mutate color channels in place. Geometry changes (resize/rotate) are
  **out of scope for v0** and must be rejected at load time.

### Host rules

* Plugins run on the master audio bus (after the sum, before encoding /
  output) and post-composite on video (after track FX, before preview
  upload) — one place each, documented, never per-track in v0.
* A misbehaving plugin (panic in the in-process reference host) aborts the
  render with a diagnostic naming the plugin; the dynamic host (future RFC)
  will run plugins out-of-process or with a catch boundary before any
  third-party distribution promise is made.

## In-process reference (shipped)

`opentrack_engine::plugin` implements both traits in Rust
(`AudioBlockPlugin`, `VideoFramePlugin`) plus a `PluginHost` registry and
two examples (`builtin.stereo_width`, `builtin.tint`). The offline mixer
and the compositor accept a host through the same seams the dynamic loader
will use, which is what makes v0 more than a design document.

## Non-goals

* Parameter automation for plugins (v1+).
* MIDI / control surface plugins.
* DAW-style sandboxing (out-of-process hosts) — post-1.0 RFC.
