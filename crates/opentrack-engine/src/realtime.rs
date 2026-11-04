//! Realtime audio playback (M2, RFC-0008).
//!
//! Architecture (all device-independent parts are unit-tested; the only
//! platform-specific code is the thin cpal adapter behind `out-cpal`):
//!
//! ```text
//! GUI thread                      worker thread                 audio callback (RT)
//! ─────────                       ─────────────                 ───────────────────
//! engine.set_project()  ──mpsc──▶ rebuild graph
//! engine.play()         ──mpsc──▶ prime blocks ─┐
//! engine.set_*()  ──atomics──┐                   ▼
//!                            │            decode-ahead per track (AudioSource
//!                            │            + StreamResampler + event gain/fades)
//!                            │                   │ push Arc<MixBlock>
//!                            │                   ▼
//!                            └─ read       Spsc<block> (lock-free, depth 3)
//!                                                │ pop
//!                                                ▼
//!              ◀── position/peaks atomics ── RenderState::render():
//!                                            strip gain/pan smoothing + master,
//!                                            loop wrap, underrun + peak meters
//! ```
//!
//! Invariants:
//! * the audio callback never locks (except once per generation change when a
//!   seek/rebuild swaps the block queue), never allocates, never decodes;
//! * the worker thread is the single producer, the callback the single
//!   consumer of each block queue;
//! * `Shared.position` is written by the callback during playback and by the
//!   worker on seek/stop — the GUI only reads it.

use crate::codec::{AudioSource, CodecBackend, CodecError};
use crate::dsp::StreamResampler;
use opentrack_core::TrackId;
use opentrack_core::bus::{clamp_gain_db, db_to_linear, equal_power_pan};
use opentrack_core::project::{MediaKind, Project};
use opentrack_core::time::Ticks;
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::path::PathBuf;
use std::sync::atomic::{
    AtomicBool, AtomicI32, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Block length the worker produces (20 ms at 48 kHz; derived from rate).
pub const BLOCK_MS: u32 = 20;
/// Block-queue depth: 3 × 20 ms ≈ 60 ms of decode-ahead buffering.
pub const QUEUE_DEPTH: usize = 3;
/// How far ahead of the mix position event decoders are opened (1 s).
pub const OPEN_AHEAD_SECS: f64 = 1.0;
/// Hard cap on realtime track slots (atomics are preallocated at spawn).
pub const MAX_TRACK_SLOTS: usize = 128;

// ---------------------------------------------------------------------------
// Lock-free SPSC queue (Lamport, power-of-two capacity)
// ---------------------------------------------------------------------------

struct SpscInner<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: AtomicUsize, // consumer cursor
    tail: AtomicUsize, // producer cursor
}

// SAFETY: the queue implements the SPSC protocol — exactly one thread may
// call `Producer::push` and exactly one `Consumer::pop` for a given queue
// instance; the Release/Acquire pairs on head/tail publish slot writes.
unsafe impl<T: Send> Send for SpscInner<T> {}
unsafe impl<T: Send> Sync for SpscInner<T> {}

impl<T> Drop for SpscInner<T> {
    fn drop(&mut self) {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        for i in head..tail {
            // SAFETY: every slot in head..tail was initialized by the producer.
            let v = unsafe { self.slots[i & self.mask].get_mut().assume_init_read() };
            drop(v);
        }
    }
}

/// Bounded lock-free single-producer/single-consumer queue.
pub struct Spsc<T>(Arc<SpscInner<T>>);

/// Producing half of a [`Spsc`].
pub struct Producer<T>(Arc<SpscInner<T>>);
/// Consuming half of a [`Spsc`].
pub struct Consumer<T>(Arc<SpscInner<T>>);

impl<T> Clone for Producer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Spsc<T> {
    /// Creates a queue with at least `cap` slots (rounded up to a power of 2).
    pub fn with_capacity(cap: usize) -> Self {
        let cap = cap.max(1).next_power_of_two();
        let slots: Box<[UnsafeCell<MaybeUninit<T>>]> = (0..cap)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self(Arc::new(SpscInner {
            slots,
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }))
    }

    /// Splits into the single-producer and single-consumer handles.
    pub fn split(&self) -> (Producer<T>, Consumer<T>) {
        (Producer(self.0.clone()), Consumer(self.0.clone()))
    }
}

impl<T> SpscInner<T> {
    fn len(&self) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let t = self.tail.load(Ordering::Acquire);
        t.wrapping_sub(h)
    }
}

impl<T> Producer<T> {
    /// Pushes a value; `Err(v)` when the queue is full.
    pub fn push(&self, v: T) -> Result<(), T> {
        let inner = &self.0;
        let h = inner.head.load(Ordering::Acquire);
        let t = inner.tail.load(Ordering::Relaxed);
        if t.wrapping_sub(h) > inner.mask {
            return Err(v);
        }
        // SAFETY: slot t is free (checked above) and only this thread writes it.
        unsafe {
            (*inner.slots[t & inner.mask].get()).write(v);
        }
        inner.tail.store(t.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Number of queued items.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the queue has no items.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total slot count.
    pub fn capacity(&self) -> usize {
        self.0.mask + 1
    }
}

impl<T> Consumer<T> {
    /// Pops the oldest value, or `None` when empty.
    pub fn pop(&self) -> Option<T> {
        let inner = &self.0;
        let t = inner.tail.load(Ordering::Acquire);
        let h = inner.head.load(Ordering::Relaxed);
        if h == t {
            return None;
        }
        // SAFETY: slot h was initialized by the producer and is exclusively
        // owned by the consumer until head advances.
        let v = unsafe { (*inner.slots[h & inner.mask].get()).assume_init_read() };
        inner.head.store(h.wrapping_add(1), Ordering::Release);
        Some(v)
    }

    /// Number of queued items.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the queue has no items.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T> Clone for Consumer<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// ---------------------------------------------------------------------------
// Shared state: transport, parameters, meters (all lock-free)
// ---------------------------------------------------------------------------

/// One mixed block handed from the worker to the audio callback.
///
/// `tracks[slot]` holds stereo-interleaved audio *after* event gain and
/// fades but *before* the track strip (gain/pan/mute) and master bus — those
/// are applied live in the callback with smoothing.
pub struct MixBlock {
    /// Queue generation this block belongs to (stale blocks are dropped).
    pub generation: u64,
    /// Timeline frame of the first sample.
    pub start_frame: u64,
    /// Timeline frame following the block (loop wrap already applied by the
    /// producer).
    pub end_frame: u64,
    /// Frames per track buffer.
    pub frames: usize,
    /// Per-slot stereo-interleaved audio (`frames * 2` samples).
    pub tracks: Vec<Vec<f32>>,
}

/// Atomics shared by GUI, worker and audio callback.
pub struct Shared {
    rate: u32,
    track_count: AtomicUsize,
    playing: AtomicBool,
    ready: AtomicBool,
    finished: AtomicBool,
    position: AtomicU64,
    generation: AtomicU64,
    end_frames: AtomicU64,
    loop_enabled: AtomicBool,
    loop_start: AtomicU64,
    loop_end: AtomicU64,
    queues: Mutex<Consumer<Arc<MixBlock>>>,
    queued: AtomicUsize,
    underruns: AtomicU64,
    issues: AtomicU64,
    latency_us: AtomicU64,
    any_solo: AtomicBool,
    master_gain_x100: AtomicI32,
    master_pan_x1000: AtomicI32,
    master_mute: AtomicBool,
    track_gain_x100: Vec<AtomicI32>,
    track_pan_x1000: Vec<AtomicI32>,
    track_flags: Vec<AtomicU8>,
    peaks_track: Vec<AtomicU32>,
    peak_master_l: AtomicU32,
    peak_master_r: AtomicU32,
}

impl Shared {
    fn new(track_count: usize, rate: u32) -> Self {
        let n = track_count.min(MAX_TRACK_SLOTS);
        let (_prod, cons) = Spsc::<Arc<MixBlock>>::with_capacity(QUEUE_DEPTH).split();
        Self {
            rate,
            track_count: AtomicUsize::new(n),
            playing: AtomicBool::new(false),
            ready: AtomicBool::new(true),
            finished: AtomicBool::new(false),
            position: AtomicU64::new(0),
            generation: AtomicU64::new(1),
            end_frames: AtomicU64::new(0),
            loop_enabled: AtomicBool::new(false),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            queues: Mutex::new(cons),
            queued: AtomicUsize::new(0),
            underruns: AtomicU64::new(0),
            issues: AtomicU64::new(0),
            latency_us: AtomicU64::new(0),
            any_solo: AtomicBool::new(false),
            master_gain_x100: AtomicI32::new(0),
            master_pan_x1000: AtomicI32::new(0),
            master_mute: AtomicBool::new(false),
            track_gain_x100: (0..MAX_TRACK_SLOTS).map(|_| AtomicI32::new(0)).collect(),
            track_pan_x1000: (0..MAX_TRACK_SLOTS).map(|_| AtomicI32::new(0)).collect(),
            track_flags: (0..MAX_TRACK_SLOTS).map(|_| AtomicU8::new(0)).collect(),
            peaks_track: (0..MAX_TRACK_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            peak_master_l: AtomicU32::new(0),
            peak_master_r: AtomicU32::new(0),
        }
    }

    /// Engine sample rate (the output device's rate).
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Number of audio track slots in the current mix graph.
    pub fn track_count(&self) -> usize {
        self.track_count.load(Ordering::Acquire)
    }

    /// Whether audio is currently being played.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    /// Whether the producer has primed its buffers (startup/seek settle).
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Whether playback reached the timeline end without a loop region.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Current playback position in seconds (feed position, no latency trim).
    pub fn position_secs(&self) -> f64 {
        self.position.load(Ordering::Acquire) as f64 / self.rate as f64
    }

    /// Current playback position in frames.
    pub fn position_frames(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    /// Blocks currently buffered between worker and callback.
    pub fn queued_blocks(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }

    /// Callback starvations observed so far (diagnostics).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Acquire)
    }

    /// Non-fatal decode problems observed by the worker (diagnostics).
    pub fn decode_issues(&self) -> u64 {
        self.issues.load(Ordering::Acquire)
    }

    /// Output latency estimate in seconds (device buffer + host period).
    pub fn latency_secs(&self) -> f64 {
        self.latency_us.load(Ordering::Acquire) as f64 / 1_000_000.0
    }

    /// Sets the output latency estimate (called by the device adapter).
    pub fn set_latency_secs(&self, secs: f64) {
        self.latency_us.store(
            (secs.max(0.0) * 1_000_000.0).round() as u64,
            Ordering::Release,
        );
    }

    /// Takes and clears the peak a track slot reached since the last poll.
    pub fn swap_track_peak(&self, slot: usize) -> f32 {
        f32::from_bits(self.peaks_track[slot.min(MAX_TRACK_SLOTS - 1)].swap(0, Ordering::AcqRel))
    }

    /// Takes and clears the master peaks since the last poll.
    pub fn swap_master_peak(&self) -> (f32, f32) {
        (
            f32::from_bits(self.peak_master_l.swap(0, Ordering::AcqRel)),
            f32::from_bits(self.peak_master_r.swap(0, Ordering::AcqRel)),
        )
    }

    /// Live track strip update (lock-free, callable from the GUI thread).
    pub fn set_track_params(&self, slot: usize, gain_db: f32, pan: f32, mute: bool, solo: bool) {
        if slot >= MAX_TRACK_SLOTS {
            return;
        }
        self.track_gain_x100[slot].store(
            (clamp_gain_db(gain_db) * 100.0).round() as i32,
            Ordering::Release,
        );
        self.track_pan_x1000[slot].store(
            (pan.clamp(-1.0, 1.0) * 1000.0).round() as i32,
            Ordering::Release,
        );
        let mut flags = 0u8;
        if mute {
            flags |= 1;
        }
        if solo {
            flags |= 2;
        }
        self.track_flags[slot].store(flags, Ordering::Release);
    }

    /// Live master bus update (lock-free, callable from the GUI thread).
    pub fn set_master_params(&self, gain_db: f32, pan: f32, mute: bool) {
        self.master_gain_x100.store(
            (clamp_gain_db(gain_db) * 100.0).round() as i32,
            Ordering::Release,
        );
        self.master_pan_x1000.store(
            (pan.clamp(-1.0, 1.0) * 1000.0).round() as i32,
            Ordering::Release,
        );
        self.master_mute.store(mute, Ordering::Release);
    }

    /// Updates the solo context used to interpret track solo flags.
    pub fn set_any_solo(&self, any: bool) {
        self.any_solo.store(any, Ordering::Release);
    }

    /// Pushes the project's strip/master state into the atomics (rebuild).
    fn reset_params_from(&self, project: &Project) {
        let mut any_solo = false;
        let mut slot = 0usize;
        for track in &project.tracks {
            let opentrack_core::timeline::TrackKind::Audio(props) = &track.kind else {
                continue;
            };
            if slot >= MAX_TRACK_SLOTS {
                break;
            }
            any_solo |= props.solo;
            self.set_track_params(slot, props.gain_db, props.pan, props.mute, props.solo);
            slot += 1;
        }
        self.set_any_solo(any_solo);
        self.set_master_params(
            project.master.gain_db,
            project.master.pan,
            project.master.mute,
        );
        self.track_count.store(slot, Ordering::Release);
        self.end_frames.store(
            ticks_to_frames(
                project.duration(),
                project.timebase.ticks_per_second,
                self.rate,
            )
            .max(0) as u64,
            Ordering::Release,
        );
    }

    fn install_queue(&self, cons: Consumer<Arc<MixBlock>>) {
        *self.queues.lock().expect("queue mutex poisoned") = cons;
        self.queued.store(0, Ordering::Release);
    }

    fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }
}

// ---------------------------------------------------------------------------
// Worker-side mix graph
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct EventSpec {
    /// Timeline start in engine frames.
    start: i64,
    /// Event length in engine frames (≥ 1).
    length: i64,
    /// Source offset in seconds (media_in) — resolved to the source rate at open.
    media_in_secs: f64,
    /// Source file path.
    path: PathBuf,
    /// Per-event linear gain (offline-identical).
    gain: f32,
    /// Fade-in length in engine frames.
    fade_in: usize,
    /// Fade-out length in engine frames.
    fade_out: usize,
}

struct ActiveEvent {
    spec: usize,
    dec: Box<dyn AudioSource>,
    res: StreamResampler,
    /// Event-local output frames already delivered.
    produced: i64,
    /// Stereo-interleaved output frames not yet delivered.
    buf: Vec<f32>,
    /// Scratch for one decode+resample round.
    tmp: Vec<f32>,
    src_ch: usize,
    eof: bool,
}

#[derive(Default)]
struct TrackGraph {
    events: Vec<EventSpec>,
    active: Vec<ActiveEvent>,
    next_open: usize,
}

/// The prebuilt mix graph: one [`TrackGraph`] per audio track slot.
struct Graph {
    tracks: Vec<TrackGraph>,
}

impl Graph {
    fn new(project: &Project, rate: u32) -> Self {
        let tps = project.timebase.ticks_per_second;
        let mut tracks = Vec::new();
        for track in &project.tracks {
            let opentrack_core::timeline::TrackKind::Audio(_) = &track.kind else {
                continue;
            };
            let mut events = Vec::new();
            for ev in &track.events {
                let audio_ok = match project.media(ev.media).map(|a| &a.kind) {
                    Some(MediaKind::Audio { .. }) => true,
                    Some(MediaKind::Video { has_audio, .. }) => *has_audio,
                    _ => false,
                };
                if !audio_ok {
                    continue;
                }
                let Some(asset) = project.media(ev.media) else {
                    continue;
                };
                events.push(EventSpec {
                    start: ticks_to_frames(ev.start, tps, rate),
                    length: ticks_to_frames(ev.length, tps, rate).max(1),
                    media_in_secs: ev.media_in.to_seconds_f64(tps),
                    path: asset.path.clone(),
                    gain: db_to_linear(ev.gain_db.clamp(-96.0, 24.0)),
                    fade_in: ticks_to_frames(ev.fade_in, tps, rate).max(0) as usize,
                    fade_out: ticks_to_frames(ev.fade_out, tps, rate).max(0) as usize,
                });
            }
            events.sort_by_key(|e| e.start);
            tracks.push(TrackGraph {
                events,
                active: Vec::new(),
                next_open: 0,
            });
        }
        Self { tracks }
    }

    /// Repositions the graph: drops active decoders and re-derives the open
    /// cursor so events overlapping `pos` get (re)opened on the next fill.
    fn seek(&mut self, pos: i64) {
        for tg in &mut self.tracks {
            tg.active.clear();
            tg.next_open = tg
                .events
                .iter()
                .position(|e| e.start + e.length > pos)
                .unwrap_or(tg.events.len());
        }
    }
}

fn ticks_to_frames(t: Ticks, tps: i64, sample_rate: u32) -> i64 {
    (t.to_seconds_f64(tps) * sample_rate as f64).round() as i64
}

/// Opens a decoder for an event and skips to its media in-point.
fn open_event(
    ev: &EventSpec,
    backends: &[Arc<dyn CodecBackend>],
    rate: u32,
) -> Result<ActiveEvent, CodecError> {
    let (mut src, _) = crate::codec::open_audio_any(backends, &ev.path)?;
    let spec = src.spec();
    let src_ch = (spec.channels.max(1)) as usize;
    let skip = (ev.media_in_secs * spec.sample_rate as f64)
        .round()
        .max(0.0) as u64;
    src.skip_frames(skip)?;
    Ok(ActiveEvent {
        spec: 0,
        dec: src,
        res: StreamResampler::new(src_ch as u16, spec.sample_rate, rate),
        produced: 0,
        buf: Vec::new(),
        tmp: Vec::new(),
        src_ch,
        eof: false,
    })
}

/// Decodes + resamples one chunk into the event's pending buffer.
fn decode_more(a: &mut ActiveEvent) {
    if a.eof {
        return;
    }
    let mut chunk = vec![0f32; 4096 * a.src_ch];
    let read = a.dec.read(&mut chunk).unwrap_or(0);
    if read == 0 {
        a.eof = true;
    }
    if read > 0 {
        a.res.process(&chunk[..read], &mut a.tmp);
    }
    if a.eof {
        a.res.flush(&mut a.tmp);
    }
    // Expand whatever channel count we got into stereo.
    let ch = a.src_ch;
    let frames = a.tmp.len() / ch;
    a.buf.reserve(frames * 2);
    for f in 0..frames {
        let l = a.tmp[f * ch];
        let r = if ch > 1 { a.tmp[f * ch + 1] } else { l };
        a.buf.push(l);
        a.buf.push(r);
    }
    a.tmp.clear();
}

/// Linear fade identical to the offline mixer (parity requirement).
fn fade_at(i: usize, len: usize, fade_in: usize, fade_out: usize) -> f32 {
    crate::mixer::fade_gain(i, len, fade_in, fade_out)
}

/// Fills `out` (stereo, `frames * 2`) with the track's contribution to the
/// timeline window `[pos, pos + frames)` — event gain and fades pre-applied.
fn fill_track(
    tg: &mut TrackGraph,
    pos: i64,
    frames: usize,
    out: &mut Vec<f32>,
    backends: &[Arc<dyn CodecBackend>],
    sh: &Shared,
    rate: u32,
) {
    out.clear();
    out.resize(frames * 2, 0.0);
    let window_end = pos + frames as i64;

    // Retire events that ended before the window.
    tg.active
        .retain(|a| tg.events[a.spec].start + tg.events[a.spec].length > pos);

    // Open upcoming decoders (bounded lookahead so opens never happen
    // inside the event's own first block).
    let open_horizon = window_end + (OPEN_AHEAD_SECS * rate as f64) as i64;
    while tg.next_open < tg.events.len() && tg.events[tg.next_open].start < open_horizon {
        let idx = tg.next_open;
        tg.next_open += 1;
        if tg.events[idx].start + tg.events[idx].length <= pos {
            continue; // already over before the window
        }
        match open_event(&tg.events[idx], backends, rate) {
            Ok(mut a) => {
                a.spec = idx;
                tg.active.push(a);
            }
            Err(_) => {
                sh.issues.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    for ai in 0..tg.active.len() {
        let spec_idx = tg.active[ai].spec;
        // Capture the event params before taking the mutable borrow.
        let (ev_gain, ev_fi, ev_fo, ev_start, ev_len) = {
            let ev = &tg.events[spec_idx];
            (ev.gain, ev.fade_in, ev.fade_out, ev.start, ev.length)
        };
        if ev_start + ev_len <= pos || ev_start >= window_end {
            continue;
        }
        let len = ev_len as usize;
        let from = (ev_start.max(pos) - ev_start) as usize;
        let to = ((ev_start + ev_len).min(window_end) - ev_start) as usize;

        let a = &mut tg.active[ai];
        // Decode until the requested range is available (or EOF).
        let mut guard = 0;
        while !a.eof && a.produced + ((a.buf.len() / 2) as i64) < to as i64 {
            decode_more(a);
            guard += 1;
            if guard > 4096 {
                break; // pathological decoder; never spin forever
            }
        }
        let avail = a.produced + (a.buf.len() / 2) as i64;
        let lo = (from as i64).max(a.produced);
        let hi = (to as i64).min(avail);
        if hi > lo {
            let k0 = (lo - a.produced) as usize;
            for i in lo..hi {
                let k = k0 + (i - lo) as usize;
                let g = ev_gain * fade_at(i as usize, len, ev_fi, ev_fo);
                // Timeline frame of sample i is ev_start + i (≥ pos).
                let o = (ev_start - pos + i) as usize * 2;
                out[o] += a.buf[k * 2] * g;
                out[o + 1] += a.buf[k * 2 + 1] * g;
            }
            let consumed = (hi - lo) as usize;
            a.buf.drain(..consumed * 2);
            a.produced += consumed as i64;
        }
    }
}

// ---------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------

enum WorkerMsg {
    Rebuild(Arc<Project>),
    Play,
    Pause,
    Stop,
    Seek(f64),
    SetLoop(Option<(f64, f64)>),
    Shutdown,
}

struct Worker {
    sh: Arc<Shared>,
    backends: Vec<Arc<dyn CodecBackend>>,
    rate: u32,
    graph: Graph,
    prod: Producer<Arc<MixBlock>>,
    playing: bool,
    next_start: u64,
    loop_on: bool,
    loop_start: u64,
    loop_end: u64,
}

impl Worker {
    fn block_frames(&self) -> usize {
        ((self.rate * BLOCK_MS) / 1000).max(64) as usize
    }

    fn handle(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Shutdown => {}
            WorkerMsg::Rebuild(project) => {
                self.graph = Graph::new(&project, self.rate);
                self.sh.reset_params_from(&project);
                let pos = self.sh.position.load(Ordering::Acquire);
                self.reprime(pos);
            }
            WorkerMsg::Play => {
                self.sh.finished.store(false, Ordering::Release);
                if self.prod.is_empty() {
                    let pos = self.sh.position.load(Ordering::Acquire);
                    self.next_start = pos;
                    self.sh.position.store(pos, Ordering::Release);
                }
                self.playing = true;
                self.sh.playing.store(true, Ordering::Release);
                self.sh.ready.store(false, Ordering::Release);
            }
            WorkerMsg::Pause => {
                self.playing = false;
                self.sh.playing.store(false, Ordering::Release);
                self.sh.ready.store(true, Ordering::Release);
            }
            WorkerMsg::Stop => {
                self.playing = false;
                self.sh.playing.store(false, Ordering::Release);
                self.seek_internal(0.0);
            }
            WorkerMsg::Seek(secs) => self.seek_internal(secs),
            WorkerMsg::SetLoop(region) => match region {
                Some((a, b)) if b > a => {
                    self.loop_on = true;
                    self.loop_start = (a.max(0.0) * self.rate as f64).round() as u64;
                    self.loop_end = (b * self.rate as f64).round() as u64;
                    self.sh.loop_enabled.store(true, Ordering::Release);
                    self.sh.loop_start.store(self.loop_start, Ordering::Release);
                    self.sh.loop_end.store(self.loop_end, Ordering::Release);
                }
                _ => {
                    self.loop_on = false;
                    self.sh.loop_enabled.store(false, Ordering::Release);
                }
            },
        }
    }

    fn seek_internal(&mut self, secs: f64) {
        let frames = (secs.max(0.0) * self.rate as f64).round() as u64;
        self.swap_queue();
        self.sh.position.store(frames, Ordering::Release);
        self.sh.finished.store(false, Ordering::Release);
        self.graph.seek(frames as i64);
        self.next_start = frames;
        self.sh.ready.store(true, Ordering::Release);
    }

    /// Installs a fresh block queue, keeping both halves connected: the
    /// consumer goes into `Shared` (callback picks it up on the generation
    /// bump) and the producer stays with the worker.
    fn swap_queue(&mut self) {
        self.sh.bump_generation();
        let (prod, cons) = Spsc::<Arc<MixBlock>>::with_capacity(QUEUE_DEPTH).split();
        self.sh.install_queue(cons);
        self.prod = prod;
    }

    /// Drops stale queue + stream state and re-primes from `pos`.
    fn reprime(&mut self, pos: u64) {
        self.swap_queue();
        self.sh.position.store(pos, Ordering::Release);
        self.sh.finished.store(false, Ordering::Release);
        self.graph.seek(pos as i64);
        self.next_start = pos;
        self.sh.ready.store(false, Ordering::Release);
    }

    /// Produces blocks while the queue has room. Runs on every worker wake.
    fn produce(&mut self) {
        if !self.playing {
            return;
        }
        let block_len = self.block_frames();
        let end_frames = self.sh.end_frames.load(Ordering::Acquire);
        while self.prod.len() < QUEUE_DEPTH {
            // Loop wrap (worker-side, before building the block).
            if self.loop_on && self.loop_end > self.loop_start && self.next_start >= self.loop_end {
                self.next_start = self.loop_start;
            }
            if !self.loop_on && self.next_start >= end_frames {
                self.playing = false;
                self.sh.playing.store(false, Ordering::Release);
                self.sh.finished.store(true, Ordering::Release);
                break;
            }
            // Truncate the block at the loop boundary so the wrap happens
            // between blocks.
            let frames = if self.loop_on
                && self.loop_end > self.loop_start
                && self.next_start < self.loop_end
                && self.next_start + block_len as u64 > self.loop_end
            {
                (self.loop_end - self.next_start) as usize
            } else {
                block_len
            };
            if frames == 0 {
                break;
            }
            let block = self.build_block(self.next_start, frames);
            match self.prod.push(Arc::new(block)) {
                Ok(()) => {
                    self.sh.queued.fetch_add(1, Ordering::AcqRel);
                    self.next_start += frames as u64;
                }
                Err(_) => break, // queue full (should not happen; len checked)
            }
        }
        // Primes are complete once the queue is full (or nothing more to play).
        if self.playing
            && !self.sh.ready.load(Ordering::Acquire)
            && (self.prod.len() >= QUEUE_DEPTH || (!self.loop_on && self.next_start >= end_frames))
        {
            self.sh.ready.store(true, Ordering::Release);
        }
    }

    fn build_block(&mut self, start: u64, frames: usize) -> MixBlock {
        let generation = self.sh.generation.load(Ordering::Acquire);
        let mut tracks = Vec::with_capacity(self.graph.tracks.len());
        for tg in &mut self.graph.tracks {
            let mut buf = Vec::with_capacity(frames * 2);
            fill_track(
                tg,
                start as i64,
                frames,
                &mut buf,
                &self.backends,
                &self.sh,
                self.rate,
            );
            tracks.push(buf);
        }
        MixBlock {
            generation,
            start_frame: start,
            end_frame: start + frames as u64,
            frames,
            tracks,
        }
    }
}

fn worker_main(
    rx: Receiver<WorkerMsg>,
    sh: Arc<Shared>,
    backends: Vec<Arc<dyn CodecBackend>>,
    project: Arc<Project>,
    rate: u32,
) {
    let graph = Graph::new(&project, rate);
    sh.reset_params_from(&project);
    let prod = install_worker_queue(&sh);
    let mut w = Worker {
        sh,
        backends,
        rate,
        graph,
        prod,
        playing: false,
        next_start: 0,
        loop_on: false,
        loop_start: 0,
        loop_end: 0,
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(WorkerMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(msg) => w.handle(msg),
            Err(RecvTimeoutError::Timeout) => {}
        }
        w.produce();
    }
}

/// Installs a fresh queue and returns its producer half.
fn install_worker_queue(sh: &Shared) -> Producer<Arc<MixBlock>> {
    let (prod, cons) = Spsc::<Arc<MixBlock>>::with_capacity(QUEUE_DEPTH).split();
    sh.install_queue(cons);
    prod
}

// ---------------------------------------------------------------------------
// Render state (audio callback side — also used by tests and the smoke pump)
// ---------------------------------------------------------------------------

/// The realtime mixer's output stage: pops prebuilt blocks, applies track
/// strip + master gains with per-sample smoothing, publishes peaks, and
/// maintains the transport position.
///
/// The same code path runs inside the cpal callback (feature `out-cpal`) and
/// in `opentrack smoke` / tests, guaranteeing that what is tested is what
/// ships.
pub struct RenderState {
    queue: Option<(Consumer<Arc<MixBlock>>, u64)>,
    pending: Option<(Arc<MixBlock>, usize)>,
    strip: Vec<(f32, f32)>,
    master: (f32, f32),
}

impl RenderState {
    /// Creates a render state for `track_count` slots, seeded with the
    /// strip/master targets already published in `sh` (so a fresh stream
    /// does not ramp from silence — the smoothing only reacts to *changes*).
    pub fn new(sh: &Shared) -> Self {
        let n = sh.track_count().min(MAX_TRACK_SLOTS);
        let mut strip = Vec::with_capacity(n);
        for slot in 0..n {
            strip.push(Self::strip_targets(sh, slot));
        }
        let master = Self::master_targets(sh);
        Self {
            queue: None,
            pending: None,
            strip,
            master,
        }
    }

    fn queue_for(&mut self, sh: &Shared) -> &Consumer<Arc<MixBlock>> {
        let generation = sh.generation.load(Ordering::Acquire);
        let stale = match &self.queue {
            Some((_, g)) => *g != generation,
            None => true,
        };
        if stale {
            let cons = sh.queues.lock().expect("queue mutex poisoned").clone();
            self.queue = Some((cons, generation));
            let n = sh.track_count().min(MAX_TRACK_SLOTS);
            self.strip.resize(n, (0.0, 0.0));
            self.pending = None;
        }
        &self.queue.as_ref().expect("queue just set").0
    }

    fn strip_targets(sh: &Shared, slot: usize) -> (f32, f32) {
        let gain = db_to_linear(clamp_gain_db(
            sh.track_gain_x100[slot].load(Ordering::Relaxed) as f32 / 100.0,
        ));
        let pan = sh.track_pan_x1000[slot].load(Ordering::Relaxed) as f32 / 1000.0;
        let flags = sh.track_flags[slot].load(Ordering::Relaxed);
        let mute = flags & 1 != 0;
        let solo = flags & 2 != 0;
        if mute || (sh.any_solo.load(Ordering::Relaxed) && !solo) {
            return (0.0, 0.0);
        }
        let (l, r) = equal_power_pan(pan);
        (gain * l, gain * r)
    }

    fn master_targets(sh: &Shared) -> (f32, f32) {
        if sh.master_mute.load(Ordering::Relaxed) {
            return (0.0, 0.0);
        }
        let g = db_to_linear(clamp_gain_db(
            sh.master_gain_x100.load(Ordering::Relaxed) as f32 / 100.0,
        ));
        let (l, r) = equal_power_pan(sh.master_pan_x1000.load(Ordering::Relaxed) as f32 / 1000.0);
        (g * l, g * r)
    }

    /// Renders the next `out.len() / 2` frames of the realtime mix into
    /// `out` (stereo-interleaved). Advances transport state and publishes
    /// peak meters. Never blocks.
    pub fn render(&mut self, sh: &Shared, out: &mut [f32]) {
        out.fill(0.0);
        let total = out.len() / 2;
        if total == 0 {
            return;
        }
        let mut done = 0usize;
        let mut starved = false;

        while done < total {
            if !sh.is_playing() && !sh.finished.load(Ordering::Acquire) {
                break;
            }
            if self.pending.is_none() {
                let cons = self.queue_for(sh).clone();
                match cons.pop() {
                    Some(block) => {
                        let generation = sh.generation.load(Ordering::Acquire);
                        if block.generation != generation {
                            continue; // stale block from a replaced queue
                        }
                        let pos = sh.position_frames();
                        if block.start_frame != pos {
                            // Producer wrapped the loop (backward) or a gap
                            // appeared (forward): adopt the block's timeline.
                            sh.position.store(block.start_frame, Ordering::Release);
                        }
                        self.pending = Some((block, 0));
                    }
                    None => {
                        starved = sh.ready.load(Ordering::Acquire) && sh.is_playing();
                        break;
                    }
                }
            }
            let Some((block, off)) = self.pending.take() else {
                unreachable!("pending set above");
            };
            let n = (block.frames - off).min(total - done);
            self.mix_segment(sh, &block, off, n, done, out);
            done += n;
            let off = off + n;
            if off == block.frames {
                sh.queued.fetch_sub(1, Ordering::AcqRel);
                sh.position.store(block.end_frame, Ordering::Release);
            } else {
                self.pending = Some((block, off));
            }
        }

        if done < total {
            // out already zeroed; publish the starvation (once per call).
            if starved {
                sh.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Mixes `n` frames of `block` (from `off`) into `out` at frame `at`,
    /// applying smoothed strip and master gains and publishing peaks.
    fn mix_segment(
        &mut self,
        sh: &Shared,
        block: &MixBlock,
        off: usize,
        n: usize,
        at: usize,
        out: &mut [f32],
    ) {
        if n == 0 {
            return;
        }
        for slot in 0..self.strip.len() {
            let Some(data) = block.tracks.get(slot) else {
                continue;
            };
            let (tl, tr) = Self::strip_targets(sh, slot);
            let (mut gl, mut gr) = self.strip[slot];
            let dl = (tl - gl) / n as f32;
            let dr = (tr - gr) / n as f32;
            let mut pl = 0f32;
            let mut pr = 0f32;
            for i in 0..n {
                gl += dl;
                gr += dr;
                let s = (off + i) * 2;
                let l = data.get(s).copied().unwrap_or(0.0) * gl;
                let r = data.get(s + 1).copied().unwrap_or(0.0) * gr;
                let o = (at + i) * 2;
                out[o] += l;
                out[o + 1] += r;
                pl = pl.max(l.abs());
                pr = pr.max(r.abs());
            }
            self.strip[slot] = (gl, gr);
            sh.peaks_track[slot].fetch_max(pl.max(pr).to_bits(), Ordering::Release);
        }

        // Master bus with smoothing.
        let (tml, tmr) = Self::master_targets(sh);
        let (mut ml, mut mr) = self.master;
        let dl = (tml - ml) / n as f32;
        let dr = (tmr - mr) / n as f32;
        let mut pm_l = 0f32;
        let mut pm_r = 0f32;
        for i in 0..n {
            ml += dl;
            mr += dr;
            let o = (at + i) * 2;
            out[o] *= ml;
            out[o + 1] *= mr;
            pm_l = pm_l.max(out[o].abs());
            pm_r = pm_r.max(out[o + 1].abs());
        }
        self.master = (ml, mr);
        sh.peak_master_l
            .fetch_max(pm_l.to_bits(), Ordering::Release);
        sh.peak_master_r
            .fetch_max(pm_r.to_bits(), Ordering::Release);
    }

    /// Whether a partially consumed block is buffered.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

// ---------------------------------------------------------------------------
// Public engine handle (device-independent)
// ---------------------------------------------------------------------------

/// Failures of the realtime output device layer.
#[derive(Debug, thiserror::Error)]
pub enum AudioOutError {
    /// No default output device exists (CI, containers, headless machines).
    #[error("no audio output device found")]
    NoDevice,
    /// The device exposes no usable configuration.
    #[error("audio output configuration: {0}")]
    Config(String),
    /// The stream failed to build or start.
    #[error("audio stream: {0}")]
    Stream(String),
    /// This binary was built without an audio backend (`audio` feature off).
    #[error("built without audio output (rebuild with default features)")]
    Unavailable,
}

/// Estimated output latency contributed by each host API *besides* the
/// device buffer size — shared-mode period values as typically reported by
/// the OS. The device adapter measures the fixed buffer size when available
/// and adds this estimate; see `ARCHITECTURE.md` § Realtime audio.
pub fn estimated_output_latency_secs(host_id: &str) -> f64 {
    match host_id {
        "Wasapi" => 0.010,    // WASAPI shared-mode period
        "CoreAudio" => 0.010, // default HAL IO proc
        "ALSA" => 0.020,      // two-period default
        "Jack" => 0.005,      // one 128-frame period @ 24k? conservative
        "Oboe" => 0.010,
        _ => 0.020,
    }
}

/// The realtime engine without a device: worker thread + shared state, for
/// tests, the CLI smoke profile, and hosts without audio hardware.
pub struct RealtimeMixer {
    shared: Arc<Shared>,
    tx: Option<Sender<WorkerMsg>>,
    handle: Option<std::thread::JoinHandle<()>>,
    rate: u32,
}

impl RealtimeMixer {
    /// Spawns the engine worker for `project` at `sample_rate`.
    pub fn start(
        project: &Project,
        backends: Vec<Arc<dyn CodecBackend>>,
        sample_rate: u32,
    ) -> Self {
        let shared = Arc::new(Shared::new(
            project.tracks.iter().filter(|t| t.is_audio()).count(),
            sample_rate,
        ));
        let (tx, rx) = mpsc::channel();
        let project = Arc::new(project.clone());
        let sh = shared.clone();
        let handle = std::thread::Builder::new()
            .name("opentrack-audio-worker".into())
            .spawn(move || worker_main(rx, sh, backends, project, sample_rate))
            .expect("spawn opentrack audio worker");
        Self {
            shared,
            tx: Some(tx),
            handle: Some(handle),
            rate: sample_rate,
        }
    }

    /// Shared transport/param/meter state.
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Engine sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn send(&self, msg: WorkerMsg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(msg);
        }
    }

    /// Rebuilds the mix graph from a new project revision.
    pub fn set_project(&self, project: &Project) {
        self.send(WorkerMsg::Rebuild(Arc::new(project.clone())));
    }

    /// Starts playback from the current position (primes buffers first).
    pub fn play(&self) {
        self.send(WorkerMsg::Play);
    }

    /// Pauses playback, keeping the position and buffered audio.
    pub fn pause(&self) {
        self.send(WorkerMsg::Pause);
    }

    /// Stops playback and returns the playhead to 0.
    pub fn stop(&self) {
        self.send(WorkerMsg::Stop);
    }

    /// Seeks (scrub) to a timeline position in seconds.
    pub fn seek_secs(&self, secs: f64) {
        self.send(WorkerMsg::Seek(secs));
    }

    /// Enables/disables the loop region (seconds; ignored when `b <= a`).
    pub fn set_loop_secs(&self, region: Option<(f64, f64)>) {
        self.send(WorkerMsg::SetLoop(region));
    }

    /// Live track strip gain (lock-free; applies with smoothing).
    pub fn set_track_gain(&self, slot: usize, gain_db: f32) {
        self.shared.track_gain_x100[slot.min(MAX_TRACK_SLOTS - 1)].store(
            (clamp_gain_db(gain_db) * 100.0).round() as i32,
            Ordering::Release,
        );
    }

    /// Live track strip pan (lock-free; applies with smoothing).
    pub fn set_track_pan(&self, slot: usize, pan: f32) {
        self.shared.track_pan_x1000[slot.min(MAX_TRACK_SLOTS - 1)].store(
            (pan.clamp(-1.0, 1.0) * 1000.0).round() as i32,
            Ordering::Release,
        );
    }

    /// Live mute/solo flags for a track slot.
    pub fn set_track_flags(&self, slot: usize, mute: bool, solo: bool) {
        let mut flags = 0u8;
        if mute {
            flags |= 1;
        }
        if solo {
            flags |= 2;
        }
        self.shared.track_flags[slot.min(MAX_TRACK_SLOTS - 1)].store(flags, Ordering::Release);
    }

    /// Updates the solo context (any track soloed).
    pub fn set_any_solo(&self, any: bool) {
        self.shared.set_any_solo(any);
    }

    /// Live master bus update.
    pub fn set_master(&self, gain_db: f32, pan: f32, mute: bool) {
        self.shared.set_master_params(gain_db, pan, mute);
    }

    /// Shuts the worker down and joins it (consumes the mixer).
    pub fn join(mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WorkerMsg::Shutdown);
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RealtimeMixer {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(WorkerMsg::Shutdown);
        }
    }
}

/// Slot order of audio tracks (graph order == GUI strip order).
pub fn audio_track_slots(project: &Project) -> Vec<TrackId> {
    project
        .tracks
        .iter()
        .filter(|t| t.is_audio())
        .map(|t| t.id)
        .collect()
}

// ---------------------------------------------------------------------------
// cpal device adapter (feature `out-cpal`)
// ---------------------------------------------------------------------------

#[cfg(feature = "out-cpal")]
mod output {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, StreamConfig};

    /// The realtime engine attached to the platform output device
    /// (WASAPI on Windows, CoreAudio on macOS, ALSA on Linux).
    pub struct RealtimeEngine {
        mixer: RealtimeMixer,
        _stream: cpal::Stream,
    }

    impl RealtimeEngine {
        /// Spawns the engine worker + output stream for `project`.
        ///
        /// The graph always runs at the *device's* sample rate (no runtime
        /// resampling against the hardware); sources resample through
        /// [`StreamResampler`] in the worker.
        pub fn spawn(
            project: &Project,
            backends: Vec<Arc<dyn CodecBackend>>,
        ) -> Result<Self, AudioOutError> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or(AudioOutError::NoDevice)?;
            let (fmt, config) = pick_config(&device, project.sample_rate)?;
            let rate = config.sample_rate.0;
            let mixer = RealtimeMixer::start(project, backends, rate);
            let shared = mixer.shared().clone();
            let channels = config.channels as usize;

            // Measured device-buffer latency + documented host period.
            let buffer_latency = match config.buffer_size {
                cpal::BufferSize::Fixed(n) => n as f64 / rate as f64,
                cpal::BufferSize::Default => 0.0,
            };
            shared.set_latency_secs(
                buffer_latency + super::estimated_output_latency_secs(&format!("{:?}", host.id())),
            );

            let mut state = RenderState::new(&shared);
            let mut scratch: Vec<f32> = Vec::new();
            let err_host = host.id();
            let err_fn = move |err| {
                eprintln!("opentrack audio ({err_host:?}): stream error: {err}");
            };
            let stream = match fmt {
                SampleFormat::F32 => {
                    let sh = shared.clone();
                    device.build_output_stream(
                        &config,
                        move |data: &mut [f32], _info| {
                            let frames = data.len() / channels.max(1);
                            scratch.clear();
                            scratch.resize(frames * 2, 0.0);
                            state.render(&sh, &mut scratch);
                            if channels == 2 {
                                data.copy_from_slice(&scratch);
                            } else {
                                for (i, frame) in scratch.as_chunks::<2>().0.iter().enumerate() {
                                    for c in 0..channels {
                                        data[i * channels + c] = match c {
                                            0 => frame[0],
                                            1 => frame[1],
                                            _ => (frame[0] + frame[1]) * 0.5,
                                        };
                                    }
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                }
                other => {
                    mixer.join();
                    return Err(AudioOutError::Config(format!(
                        "unsupported sample format {other:?}"
                    )));
                }
            }
            .map_err(|e| AudioOutError::Stream(e.to_string()))?;
            stream
                .play()
                .map_err(|e| AudioOutError::Stream(e.to_string()))?;
            Ok(Self {
                mixer,
                _stream: stream,
            })
        }

        /// Shared transport/param/meter state.
        pub fn shared(&self) -> &Arc<Shared> {
            self.mixer.shared()
        }

        /// Rebuilds the mix graph from a new project revision.
        pub fn set_project(&self, project: &Project) {
            self.mixer.set_project(project);
        }

        /// Starts playback from the current position.
        pub fn play(&self) {
            self.mixer.play();
        }

        /// Pauses playback, keeping the position.
        pub fn pause(&self) {
            self.mixer.pause();
        }

        /// Stops playback and returns the playhead to 0.
        pub fn stop(&self) {
            self.mixer.stop();
        }

        /// Seeks to a timeline position in seconds.
        pub fn seek_secs(&self, secs: f64) {
            self.mixer.seek_secs(secs);
        }

        /// Enables/disables the loop region (seconds).
        pub fn set_loop_secs(&self, region: Option<(f64, f64)>) {
            self.mixer.set_loop_secs(region);
        }

        /// Live track strip gain (lock-free; applies with smoothing).
        pub fn set_track_gain(&self, slot: usize, gain_db: f32) {
            self.mixer.set_track_gain(slot, gain_db);
        }

        /// Live track strip pan (lock-free; applies with smoothing).
        pub fn set_track_pan(&self, slot: usize, pan: f32) {
            self.mixer.set_track_pan(slot, pan);
        }

        /// Live mute/solo flags for a track slot.
        pub fn set_track_flags(&self, slot: usize, mute: bool, solo: bool) {
            self.mixer.set_track_flags(slot, mute, solo);
        }

        /// Updates the solo context (any track soloed).
        pub fn set_any_solo(&self, any: bool) {
            self.mixer.set_any_solo(any);
        }

        /// Live master bus update.
        pub fn set_master(&self, gain_db: f32, pan: f32, mute: bool) {
            self.mixer.set_master(gain_db, pan, mute);
        }
    }

    /// Picks the best output configuration: f32 + stereo + preferred rate.
    fn pick_config(
        device: &cpal::Device,
        preferred_rate: u32,
    ) -> Result<(SampleFormat, StreamConfig), AudioOutError> {
        let ranges: Vec<cpal::SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map_err(|e| AudioOutError::Config(e.to_string()))?
            .collect();
        let best = ranges
            .iter()
            .max_by_key(|r| {
                let fmt = u32::from(r.sample_format() == SampleFormat::F32) * 4;
                let ch = u32::from(r.channels() == 2) * 4;
                let (lo, hi) = (r.min_sample_rate().0, r.max_sample_rate().0);
                let rate = u32::from(lo <= preferred_rate && preferred_rate <= hi) * 2;
                fmt + ch + rate
            })
            .ok_or_else(|| {
                AudioOutError::Config("device exposes no output configurations".into())
            })?;
        let fmt = best.sample_format();
        let (lo, hi) = (best.min_sample_rate().0, best.max_sample_rate().0);
        let rate = preferred_rate.clamp(lo, hi);
        let config: StreamConfig = (*best).with_sample_rate(cpal::SampleRate(rate)).into();
        Ok((fmt, config))
    }
}

#[cfg(feature = "out-cpal")]
pub use output::RealtimeEngine;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav;

    fn write_tone(path: &std::path::Path, freq: f64, amp: f32, frames: usize) {
        let samples = crate::dsp::sine(frames, 48_000, freq, amp);
        wav::encode_s16(path, 1, 48_000, &samples).unwrap();
    }

    /// Builds a project with `n` audio tracks; each holds one tone event at
    /// t=0 with the given length.
    fn tone_project(dir: &std::path::Path, n: usize, len_secs: f64) -> Project {
        let mut p = Project::new("rt-test");
        p.tracks.clear();
        let tps = p.timebase.ticks_per_second;
        let dur = Ticks::from_seconds(len_secs, tps);
        for i in 0..n {
            let path = dir.join(format!("tone_{i}.wav"));
            write_tone(
                &path,
                220.0 * (i + 1) as f64,
                0.5,
                48_000 * len_secs as usize,
            );
            let media = p.add_media(
                path,
                Some(format!("tone{i}")),
                MediaKind::Audio {
                    channels: 1,
                    sample_rate: 48_000,
                    duration: dur,
                },
            );
            let id = p.alloc_id();
            let event_id = p.alloc_id();
            p.tracks.push(opentrack_core::timeline::Track {
                id: TrackId::from_raw(id),
                name: format!("T{i}"),
                kind: opentrack_core::timeline::TrackKind::Audio(Default::default()),
                events: vec![opentrack_core::timeline::Event {
                    id: opentrack_core::EventId::from_raw(event_id),
                    start: Ticks::ZERO,
                    length: dur,
                    media,
                    media_in: Ticks::ZERO,
                    takes: vec![],
                    active_take: 0,
                    gain_db: 0.0,
                    fade_in: Ticks::ZERO,
                    fade_out: Ticks::ZERO,
                    reverse: false,
                    label: format!("e{i}"),
                }],
                locked: false,
            });
        }
        p
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("opentrack-rt-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Drives the realtime graph through [`RenderState`] exactly like the
    /// device callback would, waiting for the producer instead of letting
    /// the consumer starve. Returns the underrun count (always 0 here).
    fn pump(
        mixer: &RealtimeMixer,
        rs: &mut RenderState,
        mut collect: Option<&mut Vec<f32>>,
    ) -> u64 {
        let sh = mixer.shared();
        let block = mixer.sample_rate() as usize * BLOCK_MS as usize / 1000;
        let mut out = vec![0f32; block * 2];
        let deadline = std::time::Instant::now() + Duration::from_secs(300);
        // The worker polls commands every ~2 ms; never consume before it
        // acknowledged Play (otherwise the pump would exit on a stopped
        // transport).
        while !sh.is_playing() {
            if std::time::Instant::now() > deadline {
                panic!("play was never acknowledged by the worker");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        loop {
            // Non-RT pump: wait for data instead of rendering silence, so
            // collected frames are exactly the played audio and the underrun
            // counter stays meaningful (the real callback renders silence).
            if sh.is_playing() && sh.queued_blocks() == 0 {
                if std::time::Instant::now() > deadline {
                    panic!("producer stalled: queue empty while playing");
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            rs.render(sh, &mut out);
            if let Some(c) = collect.as_deref_mut() {
                c.extend_from_slice(&out);
            }
            if !sh.is_playing() && !rs.has_pending() && sh.queued_blocks() == 0 {
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("pump did not terminate");
            }
        }
        sh.underruns()
    }

    // ---- SPSC queue -------------------------------------------------------

    #[test]
    fn spsc_push_pop_and_wrap() {
        let q = Spsc::<usize>::with_capacity(3); // pow2 → 4 slots
        let (prod, cons) = q.split();
        assert_eq!(prod.capacity(), 4);
        let mut expect = 0usize;
        let mut pushed = 0usize;
        while pushed < 7 {
            match prod.push(pushed) {
                Ok(()) => pushed += 1,
                Err(_) => {
                    assert_eq!(cons.pop(), Some(expect));
                    expect += 1;
                }
            }
        }
        while expect < pushed {
            assert_eq!(cons.pop(), Some(expect));
            expect += 1;
        }
        assert_eq!(cons.pop(), None);
    }

    #[test]
    fn spsc_threads_checksum() {
        let q = Spsc::<u64>::with_capacity(64);
        let (prod, cons) = q.split();
        let n = 100_000u64;
        let producer = std::thread::spawn(move || {
            for i in 0..n {
                while prod.push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });
        let consumer = std::thread::spawn(move || {
            let mut sum = 0u64;
            let mut got = 0u64;
            while got < n {
                if let Some(v) = cons.pop() {
                    sum = sum.wrapping_add(v);
                    got += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
            sum
        });
        producer.join().unwrap();
        let sum = consumer.join().unwrap();
        assert_eq!(sum, n * (n - 1) / 2);
    }

    // ---- parity: realtime mix == offline mix ------------------------------

    #[test]
    fn realtime_mix_matches_offline() {
        let dir = tempdir("parity");
        let mut p = tone_project(&dir, 2, 1.0);
        // Overlap the second event 0.5s into the first with crossfades.
        let tps = p.timebase.ticks_per_second;
        let ev2 = p.tracks[1].events[0].id;
        {
            let ev = p.event_mut(ev2).unwrap();
            ev.start = Ticks::from_seconds(0.5, tps);
            ev.fade_in = Ticks::from_seconds(0.5, tps);
        }
        let ev1 = p.tracks[0].events[0].id;
        {
            let ev = p.event_mut(ev1).unwrap();
            ev.fade_out = Ticks::from_seconds(0.5, tps);
        }

        let backends = crate::codec::default_backends();
        let offline = crate::mixer::render_audio(
            &p,
            &backends,
            &crate::mixer::RenderSettings::default(),
            &mut |_| {},
        )
        .unwrap();

        let mixer = RealtimeMixer::start(&p, backends.clone(), 48_000);
        let mut rs = RenderState::new(mixer.shared());
        mixer.play();
        let mut got = Vec::new();
        let underruns = pump(&mixer, &mut rs, Some(&mut got));
        mixer.join();

        assert_eq!(underruns, 0, "parity run must not starve");
        let end_frames = offline.samples.len() / 2;
        got.truncate(end_frames * 2);
        assert_eq!(got.len(), offline.samples.len(), "frame count must match");
        let mut max_diff = 0f32;
        for (a, b) in got.iter().zip(offline.samples.iter()) {
            max_diff = max_diff.max((a - b).abs());
        }
        assert!(max_diff < 1e-4, "realtime vs offline diff {max_diff}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- transport ---------------------------------------------------------

    #[test]
    fn transport_play_pause_seek_stop() {
        let dir = tempdir("transport");
        let p = tone_project(&dir, 1, 2.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let sh = mixer.shared().clone();
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];

        fn drive(sh: &Shared, rs: &mut RenderState, out: &mut [f32], target: f64) -> f64 {
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                rs.render(sh, out);
                if sh.position_secs() >= target {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            sh.position_secs()
        }

        mixer.play();
        let pos = drive(&sh, &mut rs, &mut out, 0.5);
        assert!(
            pos >= 0.5,
            "playhead must advance under the callback: {pos}"
        );

        mixer.pause();
        std::thread::sleep(Duration::from_millis(80));
        for _ in 0..5 {
            rs.render(&sh, &mut out);
        }
        let frozen = sh.position_secs();
        std::thread::sleep(Duration::from_millis(60));
        for _ in 0..5 {
            rs.render(&sh, &mut out);
        }
        assert_eq!(sh.position_secs(), frozen, "paused playhead is frozen");
        assert!(!sh.is_playing());

        mixer.seek_secs(0.1);
        let mut waited = 0;
        while (sh.position_secs() - 0.1).abs() > 0.05 && waited < 100 {
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
        }
        assert!((sh.position_secs() - 0.1).abs() <= 0.05);

        mixer.play();
        let pos = drive(&sh, &mut rs, &mut out, 0.2);
        assert!(
            pos >= 0.2,
            "resumed playback must advance from the seek point"
        );
        mixer.stop();
        let mut waited = 0;
        while sh.position_secs() != 0.0 && waited < 100 {
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
        }
        assert_eq!(sh.position_secs(), 0.0, "stop returns to 0");
        assert!(!sh.is_playing());
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn playback_stops_at_end() {
        let dir = tempdir("endstop");
        let p = tone_project(&dir, 1, 1.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let mut rs = RenderState::new(mixer.shared());
        mixer.play();
        let underruns = pump(&mixer, &mut rs, None);
        let sh = mixer.shared();
        assert!(!sh.is_playing(), "transport must stop at the end");
        assert!(sh.is_finished());
        let end =
            ticks_to_frames(p.duration(), p.timebase.ticks_per_second, 48_000) as f64 / 48_000.0;
        assert!((sh.position_secs() - end).abs() < 0.05);
        assert_eq!(underruns, 0);
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn loop_region_wraps() {
        let dir = tempdir("loop");
        let p = tone_project(&dir, 1, 3.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let sh = mixer.shared().clone();
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];
        mixer.set_loop_secs(Some((0.5, 1.0)));
        mixer.play();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut prev = 0f64;
        let mut wrapped = false;
        let mut max_pos = 0f64;
        while std::time::Instant::now() < deadline {
            rs.render(&sh, &mut out);
            let pos = sh.position_secs();
            max_pos = max_pos.max(pos);
            if pos < prev - 1e-6 {
                wrapped = true;
            }
            prev = pos;
            if wrapped && pos >= 0.75 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        mixer.pause();
        assert!(wrapped, "loop wrap must move the playhead backwards");
        assert!(
            max_pos <= 1.0 + 960.0 / 48_000.0 + 1e-6,
            "playhead must not pass the loop end + one block: {max_pos}"
        );
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- live parameters ---------------------------------------------------

    #[test]
    fn live_gain_change_smooths_without_zipper() {
        let dir = tempdir("smooth");
        let p = tone_project(&dir, 1, 4.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let sh = mixer.shared().clone();
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];
        mixer.play();
        let mut peak_per_block = Vec::new();
        for _ in 0..5 {
            rs.render(&sh, &mut out);
            peak_per_block.push(out.iter().fold(0f32, |m, s| m.max(s.abs())));
        }
        let before = *peak_per_block.last().unwrap();
        mixer.set_track_gain(0, -60.0); // ≈ silence
        for _ in 0..10 {
            rs.render(&sh, &mut out);
            peak_per_block.push(out.iter().fold(0f32, |m, s| m.max(s.abs())));
        }
        mixer.pause();
        let after = &peak_per_block[5..];
        assert!(
            after[0] <= before * 1.05,
            "no gain jump: {before} -> {}",
            after[0]
        );
        assert!(*after.last().unwrap() < 0.01, "gain must reach ~silence");
        assert!(
            after.windows(2).all(|w| w[1] <= w[0] + 0.05),
            "monotonic fade"
        );
        assert!(out.iter().all(|s| s.is_finite()));
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mute_and_solo_silence_in_realtime() {
        let dir = tempdir("rtsolo");
        let p = tone_project(&dir, 2, 1.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let sh = mixer.shared().clone();
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];
        mixer.play();
        while sh.queued_blocks() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..3 {
            rs.render(&sh, &mut out);
        }
        let loud = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        // Clear accumulated peaks, then solo track 0 → track 1 silenced.
        let _ = sh.swap_track_peak(0);
        let _ = sh.swap_track_peak(1);
        mixer.set_track_flags(0, false, true);
        mixer.set_any_solo(true);
        while sh.queued_blocks() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..5 {
            rs.render(&sh, &mut out);
        }
        // The first block after the toggle still carries the 20 ms smoothing
        // ramp toward silence — discard it before asserting.
        let _ = sh.swap_track_peak(0);
        let _ = sh.swap_track_peak(1);
        while sh.queued_blocks() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..5 {
            rs.render(&sh, &mut out);
        }
        let peak0 = sh.swap_track_peak(0);
        let peak1 = sh.swap_track_peak(1);
        mixer.pause();
        assert!(loud > 0.2);
        assert!(peak0 > 0.2, "soloed track audible: {peak0}");
        assert!(
            peak1.abs() < 1e-6,
            "non-soloed track silenced (smoothing residue): {peak1}"
        );
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- meters ------------------------------------------------------------

    #[test]
    fn peak_meters_report_and_reset() {
        let dir = tempdir("meters");
        let p = tone_project(&dir, 1, 1.0);
        let mixer = RealtimeMixer::start(&p, crate::codec::default_backends(), 48_000);
        let sh = mixer.shared().clone();
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];
        mixer.play();
        while sh.queued_blocks() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        for _ in 0..10 {
            rs.render(&sh, &mut out);
        }
        mixer.pause();
        let t = sh.swap_track_peak(0);
        let (ml, mr) = sh.swap_master_peak();
        // Mono tone duplicated to stereo, equal-power pan at center: every
        // channel sees 0.5 × √½ at the strip and again × √½ at the master
        // (the offline mixer publishes exactly the same values).
        let strip = 0.5 * std::f32::consts::FRAC_1_SQRT_2;
        let master = strip * std::f32::consts::FRAC_1_SQRT_2;
        assert!((t - strip).abs() < 0.05, "track peak: {t} vs {strip}");
        assert!(
            (ml - master).abs() < 0.05,
            "master L peak: {ml} vs {master}"
        );
        assert!(
            (mr - master).abs() < 0.05,
            "master R peak: {mr} vs {master}"
        );
        let (ml2, _) = sh.swap_master_peak();
        assert_eq!(ml2, 0.0, "peak read resets the meter");
        mixer.join();
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- underrun accounting ------------------------------------------------

    #[test]
    fn underrun_counted_when_starved() {
        let sh = Shared::new(1, 48_000);
        sh.playing.store(true, Ordering::Release);
        sh.ready.store(true, Ordering::Release);
        let mut rs = RenderState::new(&sh);
        let mut out = vec![0f32; 960 * 2];
        rs.render(&sh, &mut out);
        assert_eq!(sh.underruns(), 1);
        // Not playing → no counting.
        sh.playing.store(false, Ordering::Release);
        rs.render(&sh, &mut out);
        assert_eq!(sh.underruns(), 1);
    }

    // ---- CI smoke profile (acceptance analog) --------------------------------

    #[test]
    fn smoke_eight_tracks_ten_minutes() {
        let report = crate::smoke::run(10, false).expect("smoke profile");
        assert_eq!(report.tracks, 8);
        assert_eq!(report.underruns, 0, "smoke run must be dropout-free");
        assert_eq!(report.issues, 0, "smoke session must decode cleanly");
        let expected = (report.minutes * 60.0 * 48_000.0).round() as u64;
        assert!(
            (report.frames as i64 - expected as i64).abs() < 48_000,
            "rendered {} frames, expected ~{expected}",
            report.frames
        );
    }
}
