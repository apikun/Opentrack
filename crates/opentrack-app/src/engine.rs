//! Audio engine handle for the GUI.
//!
//! With the default `audio` feature this wraps the cpal-backed
//! [`opentrack_engine::RealtimeEngine`]; without it (headless builds,
//! containers without ALSA headers) every method is a safe no-op and
//! `available()` reports `false` so the shell can fall back to a silent
//! wall-clock playhead.

use opentrack_core::timeline::AudioTrackProps;
use opentrack_core::{Project, TrackId};

/// GUI-facing audio engine with a uniform API in both build configurations.
pub struct AudioEngine {
    #[cfg(feature = "audio")]
    inner: Option<opentrack_engine::RealtimeEngine>,
    /// Why playback is unavailable (no device / no feature), if so.
    pub error: Option<String>,
    /// Audio track ids in graph-slot order (slot = index here).
    track_ids: Vec<TrackId>,
}

impl AudioEngine {
    /// Spawns the engine for `project`; never panics — device problems are
    /// reported through `error` and the shell keeps working silently.
    pub fn spawn(
        project: &Project,
        backends: Vec<std::sync::Arc<dyn opentrack_engine::CodecBackend>>,
    ) -> Self {
        #[cfg(feature = "audio")]
        let (inner, error) = match opentrack_engine::RealtimeEngine::spawn(project, backends) {
            Ok(e) => (Some(e), None),
            Err(e) => {
                eprintln!("opentrack: realtime audio unavailable: {e}");
                (None, Some(e.to_string()))
            }
        };
        #[cfg(not(feature = "audio"))]
        {
            let _ = (project, backends);
        }
        Self {
            #[cfg(feature = "audio")]
            inner,
            #[cfg(feature = "audio")]
            error,
            #[cfg(not(feature = "audio"))]
            error: Some("built without audio output (default feature `audio` is off)".into()),
            track_ids: opentrack_engine::audio_track_slots(project),
        }
    }

    /// Whether realtime audio output is live.
    #[cfg(feature = "audio")]
    pub fn available(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether realtime audio output is live (always false without the
    /// `audio` feature).
    #[cfg(not(feature = "audio"))]
    pub fn available(&self) -> bool {
        false
    }

    /// Rebuilds the mix graph (call whenever the project revision changes).
    pub fn set_project(&mut self, project: &Project) {
        self.track_ids = opentrack_engine::audio_track_slots(project);
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.set_project(project);
        }
    }

    /// Graph slot of a track id (None for video tracks / unknown ids).
    pub fn slot_of(&self, id: TrackId) -> Option<usize> {
        self.track_ids.iter().position(|t| *t == id)
    }

    pub fn track_slots(&self) -> usize {
        self.track_ids.len()
    }

    // ---- transport -----------------------------------------------------

    pub fn play(&self) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.play();
        }
    }

    pub fn pause(&self) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.pause();
        }
    }

    pub fn stop(&self) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.stop();
        }
    }

    pub fn toggle_play(&self) {
        if self.is_playing() {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Seeks (scrubs) to `secs`; keeps playing when playing.
    pub fn seek_secs(&self, secs: f64) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.seek_secs(secs);
        }
        #[cfg(not(feature = "audio"))]
        let _ = secs;
    }

    pub fn set_loop_secs(&self, region: Option<(f64, f64)>) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.set_loop_secs(region);
        }
        #[cfg(not(feature = "audio"))]
        let _ = region;
    }

    // ---- live parameters (lock-free; safe at 60 Hz from the GUI) ---------

    pub fn set_track_props(&self, id: TrackId, props: &AudioTrackProps) {
        #[cfg(feature = "audio")]
        if let (Some(e), Some(slot)) = (&self.inner, self.slot_of(id)) {
            e.set_track_flags(slot, props.mute, props.solo);
            e.set_track_gain(slot, props.gain_db);
            e.set_track_pan(slot, props.pan);
        }
        #[cfg(not(feature = "audio"))]
        let _ = (id, props);
    }

    /// Live gain during a slider drag (lock-free, no history entry).
    pub fn set_track_gain_live(&self, id: TrackId, gain_db: f32) {
        #[cfg(feature = "audio")]
        if let (Some(e), Some(slot)) = (&self.inner, self.slot_of(id)) {
            e.set_track_gain(slot, gain_db);
        }
        #[cfg(not(feature = "audio"))]
        let _ = (id, gain_db);
    }

    /// Live pan during a slider drag (lock-free, no history entry).
    pub fn set_track_pan_live(&self, id: TrackId, pan: f32) {
        #[cfg(feature = "audio")]
        if let (Some(e), Some(slot)) = (&self.inner, self.slot_of(id)) {
            e.set_track_pan(slot, pan);
        }
        #[cfg(not(feature = "audio"))]
        let _ = (id, pan);
    }

    pub fn set_any_solo(&self, any: bool) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.set_any_solo(any);
        }
        #[cfg(not(feature = "audio"))]
        let _ = any;
    }

    pub fn set_master(&self, gain_db: f32, pan: f32, mute: bool) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            e.set_master(gain_db, pan, mute);
        }
        #[cfg(not(feature = "audio"))]
        let _ = (gain_db, pan, mute);
    }

    // ---- polled state ----------------------------------------------------

    /// Playhead the UI should display: the mixer feed position minus the
    /// measured output latency (what the listener hears *now*).
    pub fn position_display_secs(&self) -> f64 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            let sh = e.shared();
            return (sh.position_secs() - sh.latency_secs()).max(0.0);
        }
        0.0
    }

    /// Undisplayed (raw feed) position in seconds.
    pub fn position_secs(&self) -> f64 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().position_secs();
        }
        0.0
    }

    pub fn is_playing(&self) -> bool {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().is_playing();
        }
        false
    }

    pub fn underruns(&self) -> u64 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().underruns();
        }
        0
    }

    pub fn decode_issues(&self) -> u64 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().decode_issues();
        }
        0
    }

    pub fn latency_secs(&self) -> f64 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().latency_secs();
        }
        0.0
    }

    /// Takes and clears a track's peak (linear amplitude since last poll).
    pub fn swap_track_peak(&self, slot: usize) -> f32 {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().swap_track_peak(slot);
        }
        #[cfg(not(feature = "audio"))]
        let _ = slot;
        0.0
    }

    /// Takes and clears the master (L, R) peaks.
    pub fn swap_master_peak(&self) -> (f32, f32) {
        #[cfg(feature = "audio")]
        if let Some(e) = &self.inner {
            return e.shared().swap_master_peak();
        }
        (0.0, 0.0)
    }
}
