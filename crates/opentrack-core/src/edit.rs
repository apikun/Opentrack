//! High-level editing operations: build ops, validate, apply, snap.
//!
//! [`Session`] bundles a [`Project`] with a [`History`] and exposes the
//! Vegas-style edit verbs: split, trim, slip, move, ripple delete. All
//! operations validate first (media bounds, locked tracks) and then go
//! through the reversible [`EditOp`] path, so anything the session does can
//! be undone.

use crate::ids::{EventId, MediaId, TrackId};
use crate::project::{MediaKind, Project};
use crate::time::Ticks;
use crate::timeline::{AudioTrackProps, Event, Track, TrackKind};
use crate::undo::{EditOp, History};

/// Default number of remembered edit steps.
pub const DEFAULT_HISTORY_LIMIT: usize = 256;

/// Everything that can go wrong in an edit.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EditError {
    /// Track id does not exist.
    #[error("track {0} not found")]
    TrackNotFound(TrackId),
    /// Event id does not exist.
    #[error("event {0} not found")]
    EventNotFound(EventId),
    /// Media id does not exist.
    #[error("media {0} not found")]
    MediaNotFound(MediaId),
    /// Track is locked against edits.
    #[error("track is locked")]
    TrackLocked,
    /// Trim/slip would read before the start of the source media.
    #[error("edit reaches before media start")]
    BeyondMediaStart,
    /// Trim/slip would read past the end of the source media.
    #[error("edit reaches past media end")]
    BeyondMediaEnd,
    /// Split point is not strictly inside the event.
    #[error("split point is outside the event")]
    BadSplitPoint,
    /// Trim would produce a zero/negative-length event.
    #[error("trim would empty the event")]
    EmptyTrim,
    /// A stored op no longer matches project state (id drift).
    #[error("operation is stale")]
    StaleOp,
    /// FX registry key does not exist.
    #[error("unknown FX kind `{0}`")]
    UnknownFxKind(String),
    /// Chain position is out of bounds.
    #[error("FX chain index {0} out of bounds")]
    FxIndexOutOfBounds(usize),
}

/// Snapping configuration (Vegas-style: grid + event edges).
#[derive(Clone, Debug)]
pub struct SnapOptions {
    /// Master switch.
    pub enabled: bool,
    /// Maximum snap distance in ticks.
    pub threshold: Ticks,
    /// Optional grid interval.
    pub grid: Option<Ticks>,
    /// Snap to event edges on all tracks.
    pub edges: bool,
}

impl SnapOptions {
    /// Sensible defaults for a timebase: enabled, 1/12 s threshold, edges on.
    pub fn for_timebase(ticks_per_second: i64) -> Self {
        Self {
            enabled: true,
            threshold: Ticks((ticks_per_second / 12).max(1)),
            grid: None,
            edges: true,
        }
    }
}

impl Default for SnapOptions {
    fn default() -> Self {
        Self::for_timebase(crate::time::DEFAULT_TICKS_PER_SECOND)
    }
}

struct Best {
    value: Ticks,
    dist: i64,
}

impl Best {
    fn consider(&mut self, t: Ticks, cand: Ticks, threshold: i64) {
        let d = (cand.0 - t.0).abs();
        if d <= threshold && d < self.dist {
            self.dist = d;
            self.value = cand.max(Ticks::ZERO);
        }
    }
}

/// Returns `t` snapped to the nearest candidate (zero, grid lines, event
/// edges) within `opts.threshold`, or `t` itself when nothing is close.
pub fn snap_time(p: &Project, t: Ticks, ignore: Option<EventId>, opts: &SnapOptions) -> Ticks {
    if !opts.enabled {
        return t;
    }
    let mut best = Best {
        value: t,
        dist: i64::MAX,
    };
    best.consider(t, Ticks::ZERO, opts.threshold.0);
    if let Some(g) = opts.grid
        && g.0 > 0
    {
        let k = (t.0 as f64 / g.0 as f64).round() as i64;
        for c in [k - 1, k, k + 1] {
            if c >= 0 {
                best.consider(t, Ticks(c * g.0), opts.threshold.0);
            }
        }
    }
    if opts.edges {
        for track in &p.tracks {
            for e in &track.events {
                if Some(e.id) == ignore {
                    continue;
                }
                best.consider(t, e.start, opts.threshold.0);
                best.consider(t, e.end(), opts.threshold.0);
            }
        }
    }
    best.value
}

/// An overlap region between two events on one track.
///
/// OpenTrack renders overlaps as crossfades (Vegas behavior): the event that
/// starts earlier is on the left of the fade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlap {
    /// The earlier-starting event.
    pub left: EventId,
    /// The later-starting event.
    pub right: EventId,
    /// First tick of the overlap.
    pub start: Ticks,
    /// Last tick (exclusive) of the overlap.
    pub end: Ticks,
}

/// Detects all overlap (auto-crossfade) regions on a track.
pub fn overlaps_on_track(track: &Track) -> Vec<Overlap> {
    let mut sorted: Vec<&Event> = track.events.iter().collect();
    sorted.sort_by_key(|e| e.start);
    let mut out = Vec::new();
    for (i, a) in sorted.iter().enumerate() {
        for b in &sorted[i + 1..] {
            if b.start >= a.end() {
                break; // sorted by start: nothing further can overlap `a`
            }
            let start = a.start.max(b.start);
            let end = a.end().min(b.end());
            if end > start {
                let (left, right) = if a.start <= b.start {
                    (a.id, b.id)
                } else {
                    (b.id, a.id)
                };
                out.push(Overlap {
                    left,
                    right,
                    start,
                    end,
                });
            }
        }
    }
    out
}

/// An interactive editing session: project + undo history.
pub struct Session {
    /// The project being edited.
    pub project: Project,
    /// Undo/redo stack.
    pub history: History,
}

impl Session {
    /// Opens a session for `project`.
    pub fn new(project: Project) -> Self {
        Self {
            project,
            history: History::new(DEFAULT_HISTORY_LIMIT),
        }
    }

    /// Executes (applies + records) an op.
    pub fn exec(&mut self, op: EditOp) -> Result<(), EditError> {
        self.history.push(op, &mut self.project)
    }

    /// Undo; returns the op label when something was undone.
    pub fn undo(&mut self) -> Result<Option<&'static str>, EditError> {
        self.history.undo(&mut self.project)
    }

    /// Redo; returns the op label when something was redone.
    pub fn redo(&mut self) -> Result<Option<&'static str>, EditError> {
        self.history.redo(&mut self.project)
    }

    /// Adds a track at `index` (or on top when `None`).
    pub fn add_track(
        &mut self,
        name: &str,
        kind: TrackKind,
        index: Option<usize>,
    ) -> Result<TrackId, EditError> {
        let id = TrackId::from_raw(self.project.alloc_id());
        let track = Track {
            id,
            name: name.to_string(),
            kind,
            events: Vec::new(),
            locked: false,
            fx_chain: Default::default(),
        };
        let index = index.unwrap_or(0);
        self.exec(EditOp::AddTrack { index, track })?;
        Ok(id)
    }

    /// Places a media instance on a track.
    ///
    /// Validates media bounds when the asset declares a duration.
    #[allow(clippy::too_many_arguments)]
    pub fn add_event(
        &mut self,
        track: TrackId,
        media: MediaId,
        start: Ticks,
        length: Ticks,
        media_in: Ticks,
        gain_db: f32,
        fade_in: Ticks,
        fade_out: Ticks,
        label: Option<String>,
    ) -> Result<EventId, EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let asset = self
            .project
            .media(media)
            .ok_or(EditError::MediaNotFound(media))?;
        let label = label.unwrap_or_else(|| asset.label.clone());
        if let Some(dur) = declared_duration(&asset.kind)
            && media_in + length > dur
        {
            return Err(EditError::BeyondMediaEnd);
        }
        let id = EventId::from_raw(self.project.alloc_id());
        let event = Event {
            id,
            start,
            length,
            media,
            media_in,
            takes: vec![],
            active_take: 0,
            gain_db,
            fade_in,
            fade_out,
            reverse: false,
            label,
        };
        self.exec(EditOp::AddEvent { track, event })?;
        Ok(id)
    }

    /// Splits an event at `at` (strictly inside). Left keeps the original id
    /// and `fade_in`; right gets a new id and keeps `fade_out`.
    pub fn split_event(
        &mut self,
        track: TrackId,
        event: EventId,
        at: Ticks,
    ) -> Result<[EventId; 2], EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let original = t
            .events
            .iter()
            .find(|e| e.id == event)
            .cloned()
            .ok_or(EditError::EventNotFound(event))?;
        if at <= original.start || at >= original.end() {
            return Err(EditError::BadSplitPoint);
        }
        let right_len = original.end().saturating_sub(at);
        let mut left = original.clone();
        left.length = at.saturating_sub(original.start);
        left.fade_out = Ticks::ZERO;
        let mut right = original.clone();
        right.id = EventId::from_raw(self.project.alloc_id());
        right.start = at;
        right.length = right_len;
        right.media_in = original.media_in + left.length;
        right.fade_in = Ticks::ZERO;
        let right_id = right.id;
        self.exec(EditOp::SplitEvent {
            track,
            original,
            left,
            right,
        })?;
        Ok([event, right_id])
    }

    /// Trims either edge. Head trims slide `media_in` so content follows the
    /// edge (standard NLE trim). Snaps first when `snap` is given.
    pub fn trim_event(
        &mut self,
        track: TrackId,
        event: EventId,
        new_start: Option<Ticks>,
        new_end: Option<Ticks>,
        snap: Option<&SnapOptions>,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let e = t
            .events
            .iter()
            .find(|e| e.id == event)
            .cloned()
            .ok_or(EditError::EventNotFound(event))?;
        let media = self
            .project
            .media(e.media)
            .cloned()
            .ok_or(EditError::MediaNotFound(e.media))?;

        let mut ns = new_start.unwrap_or(e.start);
        let mut ne = new_end.unwrap_or(e.end());
        if let Some(s) = snap {
            ns = snap_time(&self.project, ns, Some(event), s);
            ne = snap_time(&self.project, ne, Some(event), s);
        }
        if ne <= ns {
            return Err(EditError::EmptyTrim);
        }
        let delta = ns.0 - e.start.0; // may be negative (head pull-left)
        let new_media_in_i = e.media_in.0 + delta;
        if new_media_in_i < 0 {
            return Err(EditError::BeyondMediaStart);
        }
        let new_media_in = Ticks(new_media_in_i);
        let new_length = Ticks(ne.0 - ns.0);
        if let Some(dur) = declared_duration(&media.kind)
            && new_media_in + new_length > dur
        {
            return Err(EditError::BeyondMediaEnd);
        }
        self.exec(EditOp::TrimEvent {
            track,
            event,
            old_start: e.start,
            old_length: e.length,
            old_media_in: e.media_in,
            new_start: ns,
            new_length,
            new_media_in,
        })?;
        Ok(())
    }

    /// Slips the content under the event (moves `media_in` only).
    pub fn slip_event(
        &mut self,
        track: TrackId,
        event: EventId,
        delta: Ticks,
        extend: bool,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let e = t
            .events
            .iter()
            .find(|e| e.id == event)
            .cloned()
            .ok_or(EditError::EventNotFound(event))?;
        let media = self
            .project
            .media(e.media)
            .cloned()
            .ok_or(EditError::MediaNotFound(e.media))?;
        let raw = if extend {
            e.media_in.0.saturating_add(delta.0)
        } else {
            (e.media_in.0 - delta.0).max(0)
        };
        let new_media_in = Ticks(raw);
        if let Some(dur) = declared_duration(&media.kind)
            && new_media_in + e.length > dur
        {
            return Err(EditError::BeyondMediaEnd);
        }
        self.exec(EditOp::SlipEvent {
            track,
            event,
            old_media_in: e.media_in,
            new_media_in,
        })?;
        Ok(())
    }

    /// Moves an event to a new start on the same track (snap-aware).
    pub fn move_event(
        &mut self,
        track: TrackId,
        event: EventId,
        mut new_start: Ticks,
        snap: Option<&SnapOptions>,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let e = t
            .events
            .iter()
            .find(|e| e.id == event)
            .cloned()
            .ok_or(EditError::EventNotFound(event))?;
        if let Some(s) = snap {
            new_start = snap_time(&self.project, new_start, Some(event), s);
        }
        self.exec(EditOp::MoveEvent {
            track,
            event,
            old_start: e.start,
            new_start,
            old_media_in: e.media_in,
            new_media_in: e.media_in,
        })?;
        Ok(())
    }

    /// Removes an event and closes the gap by pulling later events on the
    /// same track left (per-track ripple; project-wide ripple is M3).
    pub fn ripple_delete(&mut self, track: TrackId, event: EventId) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        let removed = t
            .events
            .iter()
            .find(|e| e.id == event)
            .cloned()
            .ok_or(EditError::EventNotFound(event))?;
        let shifts: Vec<(EventId, Ticks)> = t
            .events
            .iter()
            .filter(|e| e.start >= removed.end())
            .map(|e| (e.id, e.start))
            .collect();
        self.exec(EditOp::RippleDelete {
            track,
            removed,
            shifts,
        })?;
        Ok(())
    }

    /// Sets an audio track's strip state (volume / pan / mute / solo).
    pub fn set_audio_track_props(
        &mut self,
        track: TrackId,
        props: AudioTrackProps,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        let TrackKind::Audio(old) = t.kind.clone() else {
            return Err(EditError::StaleOp);
        };
        self.exec(EditOp::SetAudioTrackProps {
            track,
            old,
            new: props,
        })?;
        Ok(())
    }

    /// Sets the master bus state.
    pub fn set_master(&mut self, master: crate::bus::AudioBus) -> Result<(), EditError> {
        let old = self.project.master.clone();
        self.exec(EditOp::SetMaster { old, new: master })?;
        Ok(())
    }

    // ---- M4: FX chains & automation ---------------------------------------

    fn fx_track_mut(&mut self, track: TrackId) -> Result<&mut Track, EditError> {
        let t = self
            .project
            .track_mut(track)
            .ok_or(EditError::TrackNotFound(track))?;
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        Ok(t)
    }

    /// Adds an FX instance (by registry kind, with defaults) to a track's
    /// chain at the end. Rejected for unknown kinds.
    pub fn add_fx(&mut self, track: TrackId, kind: &str) -> Result<(), EditError> {
        let Some(instance) = crate::fx::FxInstance::with_defaults(kind) else {
            return Err(EditError::UnknownFxKind(kind.to_string()));
        };
        let t = self.fx_track_mut(track)?;
        let index = t.fx_chain.instances.len();
        self.exec(EditOp::AddFx {
            track,
            index,
            instance,
        })?;
        Ok(())
    }

    /// Removes the FX instance at chain position `index`.
    pub fn remove_fx(&mut self, track: TrackId, index: usize) -> Result<(), EditError> {
        let t = self.fx_track_mut(track)?;
        let Some(instance) = t.fx_chain.instances.get(index).cloned() else {
            return Err(EditError::FxIndexOutOfBounds(index));
        };
        self.exec(EditOp::RemoveFx {
            track,
            index,
            instance,
        })?;
        Ok(())
    }

    /// Replaces one FX instance's parameters / enabled flag.
    pub fn set_fx(
        &mut self,
        track: TrackId,
        index: usize,
        new: crate::fx::FxInstance,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        let Some(old) = t.fx_chain.instances.get(index).cloned() else {
            return Err(EditError::StaleOp);
        };
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        self.exec(EditOp::SetFx {
            track,
            index,
            old,
            new,
        })?;
        Ok(())
    }

    /// Moves an FX instance from chain position `from` to `to`.
    pub fn move_fx(&mut self, track: TrackId, from: usize, to: usize) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        let n = t.fx_chain.instances.len();
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        if from >= n || to >= n {
            return Err(EditError::StaleOp);
        }
        self.exec(EditOp::MoveFx { track, from, to })?;
        Ok(())
    }

    /// Adds / replaces / removes one automation point on an FX parameter.
    pub fn set_auto_point(
        &mut self,
        track: TrackId,
        fx_index: usize,
        param: &str,
        tick: i64,
        value: Option<f32>,
    ) -> Result<(), EditError> {
        let t = self
            .project
            .track(track)
            .ok_or(EditError::TrackNotFound(track))?;
        let Some(fx) = t.fx_chain.instances.get(fx_index) else {
            return Err(EditError::StaleOp);
        };
        let old_point = fx
            .automation
            .get(param)
            .and_then(|lane| lane.points.iter().find(|p| p.tick == tick))
            .map(|p| (p.tick, p.value));
        if t.locked {
            return Err(EditError::TrackLocked);
        }
        self.exec(EditOp::SetAutoPoint {
            track,
            fx_index,
            param: param.to_string(),
            old_point,
            new_point: value.map(|v| (tick, v)),
        })?;
        Ok(())
    }
}

/// Declared duration of an asset, when its kind carries one.
pub fn declared_duration(kind: &MediaKind) -> Option<Ticks> {
    match kind {
        MediaKind::Audio { duration, .. } | MediaKind::Video { duration, .. } => Some(*duration),
        MediaKind::Image { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::AudioBus;

    fn audio_session() -> (Session, TrackId, MediaId) {
        let mut p = Project::new("s");
        let media = p.add_media(
            "tone.wav".into(),
            Some("tone".into()),
            MediaKind::Audio {
                channels: 2,
                sample_rate: 48_000,
                duration: Ticks(4_000_000),
            },
        );
        let track = p.tracks.iter().find(|t| t.is_audio()).unwrap().id;
        (Session::new(p), track, media)
    }

    #[test]
    fn add_split_and_undo() {
        let (mut s, t, m) = audio_session();
        let ev = s
            .add_event(
                t,
                m,
                Ticks(0),
                Ticks(4_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let [l, r] = s.split_event(t, ev, Ticks(1_500_000)).unwrap();
        assert_eq!(s.project.event(l).unwrap().length, Ticks(1_500_000));
        let re = s.project.event(r).unwrap();
        assert_eq!(re.start, Ticks(1_500_000));
        assert_eq!(re.media_in, Ticks(1_500_000));
        assert_eq!(s.project.track(t).unwrap().events.len(), 2);
        s.undo().unwrap();
        assert_eq!(s.project.track(t).unwrap().events.len(), 1);
        assert_eq!(s.project.event(ev).unwrap().length, Ticks(4_000_000));
        s.redo().unwrap();
        assert_eq!(s.project.track(t).unwrap().events.len(), 2);
    }

    #[test]
    fn trim_respects_media_bounds() {
        let (mut s, t, m) = audio_session();
        let ev = s
            .add_event(
                t,
                m,
                Ticks(500_000),
                Ticks(3_000_000),
                Ticks(500_000),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        // head trim left is fine while media_in stays >= 0
        s.trim_event(t, ev, Some(Ticks(0)), None, None).unwrap();
        let e = s.project.event(ev).unwrap();
        assert_eq!(e.start, Ticks(0));
        assert_eq!(e.media_in, Ticks(0));
        // pulling the head further left would go before media start
        let err = s.trim_event(t, ev, Some(Ticks(0)) /* no-op */, None, None);
        assert!(err.is_ok());
        // bad: extend head left with no media headroom → error via negative delta
        s.project.event_mut(ev).unwrap().media_in = Ticks(0);
        s.project.event_mut(ev).unwrap().start = Ticks(1_000_000);
        let err = s.trim_event(t, ev, Some(Ticks(500_000)), None, None);
        assert_eq!(err.unwrap_err(), EditError::BeyondMediaStart);
    }

    #[test]
    fn move_snap_to_edge() {
        let (mut s, t, m) = audio_session();
        let a = s
            .add_event(
                t,
                m,
                Ticks(0),
                Ticks(1_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let b = s
            .add_event(
                t,
                m,
                Ticks(3_500_000),
                Ticks(500_000),
                Ticks(500_000),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let opts = SnapOptions {
            threshold: Ticks(200_000),
            ..SnapOptions::default()
        };
        // b moves near a's end → snaps flush
        s.move_event(t, b, Ticks(1_050_000), Some(&opts)).unwrap();
        assert_eq!(s.project.event(b).unwrap().start, Ticks(1_000_000));
        // ignore = a shouldn't snap to itself
        let _ = a;
    }

    #[test]
    fn overlap_detection() {
        let (mut s, t, m) = audio_session();
        let a = s
            .add_event(
                t,
                m,
                Ticks(0),
                Ticks(2_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let b = s
            .add_event(
                t,
                m,
                Ticks(1_000_000),
                Ticks(2_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let ov = overlaps_on_track(s.project.track(t).unwrap());
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].left, a);
        assert_eq!(ov[0].right, b);
        assert_eq!(ov[0].start, Ticks(1_000_000));
        assert_eq!(ov[0].end, Ticks(2_000_000));
    }

    #[test]
    fn ripple_delete_closes_gap() {
        let (mut s, t, m) = audio_session();
        let _a = s
            .add_event(
                t,
                m,
                Ticks(0),
                Ticks(1_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        let b = s
            .add_event(
                t,
                m,
                Ticks(2_000_000),
                Ticks(1_000_000),
                Ticks(0),
                0.0,
                Ticks::ZERO,
                Ticks::ZERO,
                None,
            )
            .unwrap();
        s.ripple_delete(t, _a).unwrap();
        assert_eq!(s.project.event(b).unwrap().start, Ticks(1_000_000));
        s.undo().unwrap();
        assert_eq!(s.project.event(b).unwrap().start, Ticks(2_000_000));
        assert_eq!(s.project.track(t).unwrap().events.len(), 2);
    }

    #[test]
    fn master_and_track_props_undo() {
        let (mut s, t, _m) = audio_session();
        s.set_audio_track_props(
            t,
            AudioTrackProps {
                gain_db: -3.0,
                pan: 0.5,
                ..Default::default()
            },
        )
        .unwrap();
        s.set_master(AudioBus {
            gain_db: -1.0,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(s.project.master.gain_db, -1.0);
        s.undo().unwrap();
        assert_eq!(s.project.master.gain_db, 0.0);
        s.undo().unwrap();
        match &s.project.track(t).unwrap().kind {
            TrackKind::Audio(p) => assert_eq!(p.gain_db, 0.0),
            _ => panic!("expected audio track"),
        }
    }
}
