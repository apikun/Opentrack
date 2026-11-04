//! Track and event types — the Vegas-style timeline model.
//!
//! A [`Track`] owns an ordered set of [`Event`]s. An event is a *media
//! instance*: a reference into the media pool plus a position, a length, and
//! per-event state (gain, fades, takes). Multiple events may overlap on one
//! track; overlap regions render as crossfades (see RFC-0002 §4).

use crate::ids::{EventId, MediaId, TrackId};
use crate::time::Ticks;
use serde::{Deserialize, Serialize};

/// Compositing mode of a video track against everything below it.
///
/// Track order matters: index 0 in the project's track list is the *top*
/// video track (the Vegas convention — later tracks composite underneath).
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum BlendMode {
    /// Standard alpha composite.
    #[default]
    Normal,
    /// Additive light.
    Add,
    /// Multiply (multiply-darken).
    Multiply,
    /// Screen (lighten).
    Screen,
}

/// Properties of a video track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoTrackProps {
    /// 0.0–1.0 opacity of the composite contribution.
    pub composite_level: f32,
    /// Blend mode against lower tracks.
    pub blend: BlendMode,
}

impl Default for VideoTrackProps {
    fn default() -> Self {
        Self {
            composite_level: 1.0,
            blend: BlendMode::Normal,
        }
    }
}

/// Properties of an audio track (Vegas-style track strip: Volume / Pan).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioTrackProps {
    /// Track gain in dBFS (-inf..+12 clamp is applied by the mixer).
    pub gain_db: f32,
    /// Pan position, -1.0 = hard left, +1.0 = hard right, 0 = center.
    pub pan: f32,
    /// Mute flag.
    pub mute: bool,
    /// Solo flag (any solo mutes every non-solo audio track).
    pub solo: bool,
}

impl Default for AudioTrackProps {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// What kind of media a track carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum TrackKind {
    /// Carries video events; composites top-down.
    Video(VideoTrackProps),
    /// Carries audio events; mixes into the master bus.
    Audio(AudioTrackProps),
}

/// An alternate take attached to an event (Vegas-style multiple takes).
///
/// The event's own `media` / `media_in` / `length` fields always mirror the
/// *active* take; `takes` stores the alternates so the user can switch back
/// without re-linking media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Take {
    /// Media asset used by this take.
    pub media: MediaId,
    /// Offset into the source media where playback starts.
    pub media_in: Ticks,
    /// Take length.
    pub length: Ticks,
    /// Human label shown in the take picker.
    pub name: String,
}

/// A media instance on a track — the unit users drag, trim and fade.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Unique event id.
    pub id: EventId,
    /// Timeline position of the event head.
    pub start: Ticks,
    /// Timeline duration.
    pub length: Ticks,
    /// Media asset (always mirrors the active take).
    pub media: MediaId,
    /// Offset into the source media where playback starts.
    pub media_in: Ticks,
    /// Alternate takes, oldest first.
    pub takes: Vec<Take>,
    /// Index into `takes` of the active take (when non-empty).
    pub active_take: usize,
    /// Per-event audio gain in dBFS.
    pub gain_db: f32,
    /// Linear fade-in length from the event head.
    pub fade_in: Ticks,
    /// Linear fade-out length into the event tail.
    pub fade_out: Ticks,
    /// Play the source backwards (video: reversed frames, audio: reversed
    /// samples). The mixer resolves this against the source in M2.
    pub reverse: bool,
    /// Optional user label; defaults to the asset label.
    pub label: String,
}

impl Event {
    /// Exclusive end position (`start + length`).
    pub fn end(&self) -> Ticks {
        self.start + self.length
    }

    /// Returns `true` when the timeline position `t` lies inside the event.
    pub fn covers(&self, t: Ticks) -> bool {
        self.start <= t && t < self.end()
    }

    /// The active take, if any takes were recorded.
    pub fn active_take(&self) -> Option<&Take> {
        self.takes.get(self.active_take)
    }

    /// Installs `take` as the active take, mirroring its values onto the
    /// event's top-level fields.
    pub fn switch_to_take(&mut self, index: usize) -> bool {
        if index >= self.takes.len() {
            return false;
        }
        let t = self.takes[index].clone();
        self.media = t.media;
        self.media_in = t.media_in;
        self.length = t.length;
        self.active_take = index;
        true
    }
}

/// A timeline track.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Unique track id.
    pub id: TrackId,
    /// Display name ("Audio 1", "Video 2", ...).
    pub name: String,
    /// Kind + per-kind properties.
    pub kind: TrackKind,
    /// Events on this track, kept sorted by `start`.
    pub events: Vec<Event>,
    /// Locked tracks reject edits (enforced by `opentrack-core::edit`).
    pub locked: bool,
    /// FX chain applied to the track bus (audio) / composited output
    /// (video), in processing order (M4).
    #[serde(default)]
    pub fx_chain: crate::fx::FxChain,
}

impl Track {
    /// `true` for video tracks.
    pub fn is_video(&self) -> bool {
        matches!(self.kind, TrackKind::Video(_))
    }

    /// `true` for audio tracks.
    pub fn is_audio(&self) -> bool {
        matches!(self.kind, TrackKind::Audio(_))
    }

    /// Audibility rule: an audio track is audible unless muted, or unless
    /// another audio track is soloed and this one is not.
    pub fn audible(&self, any_solo: bool) -> bool {
        match &self.kind {
            TrackKind::Audio(p) => {
                if any_solo {
                    p.solo && !p.mute
                } else {
                    !p.mute
                }
            }
            TrackKind::Video(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_covers_range_half_open() {
        let e = Event {
            id: EventId(1),
            start: Ticks(100),
            length: Ticks(50),
            media: MediaId(1),
            media_in: Ticks(0),
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks(0),
            fade_out: Ticks(0),
            reverse: false,
            label: "x".into(),
        };
        assert!(e.covers(Ticks(100)));
        assert!(e.covers(Ticks(149)));
        assert!(!e.covers(Ticks(150)));
        assert_eq!(e.end(), Ticks(150));
    }

    #[test]
    fn solo_logic() {
        let audio = |solo: bool, mute: bool| Track {
            id: TrackId(0),
            name: "t".into(),
            kind: TrackKind::Audio(AudioTrackProps {
                solo,
                mute,
                ..Default::default()
            }),
            events: vec![],
            locked: false,
            fx_chain: Default::default(),
        };
        assert!(audio(false, false).audible(false));
        assert!(!audio(false, false).audible(true)); // someone else solos
        assert!(audio(true, false).audible(true));
        assert!(!audio(true, true).audible(true)); // solo + mute loses
        assert!(!audio(false, true).audible(false));
    }
}
