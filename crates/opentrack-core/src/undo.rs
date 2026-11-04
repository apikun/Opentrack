//! Reversible editing operations and the undo stack.
//!
//! Every edit is an [`EditOp`] value that knows how to apply *and* revert
//! itself against a [`Project`]. Because ops are plain serializable data, an
//! undo history can later be persisted or replayed (macro feature, M5).

use crate::bus::AudioBus;
use crate::edit::EditError;
use crate::ids::{EventId, TrackId};
use crate::project::Project;
use crate::time::Ticks;
use crate::timeline::{AudioTrackProps, Event, Track};
use serde::{Deserialize, Serialize};

/// A reversible, serializable editing operation.
///
/// Variants deliberately embed full before/after snapshots (tracks, events)
/// so that undo is exact and ops can be replayed or persisted later; the
/// size asymmetry between variants is inherent to that design.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditOp {
    /// Insert a whole track at `index`.
    AddTrack {
        /// Insert position (clamped to `0..=tracks.len()` on apply).
        index: usize,
        /// The complete track to insert.
        track: Track,
    },
    /// Delete a track (payload stores the removed track for revert).
    RemoveTrack {
        /// Index the track was removed from.
        index: usize,
        /// The removed track.
        track: Track,
    },
    /// Insert an event on a track (events stay sorted by start).
    AddEvent {
        /// Owning track.
        track: TrackId,
        /// The event to insert.
        event: Event,
    },
    /// Remove an event (payload stores it for revert).
    RemoveEvent {
        /// Owning track.
        track: TrackId,
        /// The removed event.
        event: Event,
    },
    /// Same-track reposition of an event.
    MoveEvent {
        /// Owning track.
        track: TrackId,
        /// Event id.
        event: EventId,
        /// Start before the op.
        old_start: Ticks,
        /// Start after the op.
        new_start: Ticks,
        /// Media offset before the op.
        old_media_in: Ticks,
        /// Media offset after the op (unchanged for pure moves).
        new_media_in: Ticks,
    },
    /// Resize an event from either edge (head trim shifts `media_in`).
    TrimEvent {
        /// Owning track.
        track: TrackId,
        /// Event id.
        event: EventId,
        /// Start before the op.
        old_start: Ticks,
        /// Length before the op.
        old_length: Ticks,
        /// Media offset before the op.
        old_media_in: Ticks,
        /// Start after the op.
        new_start: Ticks,
        /// Length after the op.
        new_length: Ticks,
        /// Media offset after the op.
        new_media_in: Ticks,
    },
    /// Change what part of the media plays without moving the event.
    SlipEvent {
        /// Owning track.
        track: TrackId,
        /// Event id.
        event: EventId,
        /// Media offset before the op.
        old_media_in: Ticks,
        /// Media offset after the op.
        new_media_in: Ticks,
    },
    /// Split one event into two at a point (Vegas `S` key).
    SplitEvent {
        /// Owning track.
        track: TrackId,
        /// The event as it was before the split.
        original: Event,
        /// Left half (keeps the original id, keeps `fade_in`).
        left: Event,
        /// Right half (new id, keeps the original `fade_out`).
        right: Event,
    },
    /// Ripple delete: remove an event and pull later events on the same
    /// track left to close the gap.
    RippleDelete {
        /// Owning track.
        track: TrackId,
        /// The removed event.
        removed: Event,
        /// Old start positions of the shifted events.
        shifts: Vec<(EventId, Ticks)>,
    },
    /// Replace an audio track's strip state (gain/pan/mute/solo).
    SetAudioTrackProps {
        /// Owning track.
        track: TrackId,
        /// Props before the op.
        old: AudioTrackProps,
        /// Props after the op.
        new: AudioTrackProps,
    },
    /// Replace the master bus state.
    SetMaster {
        /// Master state before the op.
        old: AudioBus,
        /// Master state after the op.
        new: AudioBus,
    },
}

impl EditOp {
    /// Short human label for menus / history UI.
    pub fn label(&self) -> &'static str {
        match self {
            EditOp::AddTrack { .. } => "Add Track",
            EditOp::RemoveTrack { .. } => "Delete Track",
            EditOp::AddEvent { .. } => "Add Event",
            EditOp::RemoveEvent { .. } => "Delete Event",
            EditOp::MoveEvent { .. } => "Move Event",
            EditOp::TrimEvent { .. } => "Trim Event",
            EditOp::SlipEvent { .. } => "Slip Event",
            EditOp::SplitEvent { .. } => "Split Event",
            EditOp::RippleDelete { .. } => "Ripple Delete",
            EditOp::SetAudioTrackProps { .. } => "Track Volume/Pan",
            EditOp::SetMaster { .. } => "Master Bus",
        }
    }

    /// Applies the op to the project (idempotent given matching state).
    pub fn apply(&self, p: &mut Project) -> Result<(), EditError> {
        match self.clone() {
            EditOp::AddTrack { index, track } => {
                let i = index.min(p.tracks.len());
                p.tracks.insert(i, track);
            }
            EditOp::RemoveTrack { index, .. } => {
                if index >= p.tracks.len() {
                    return Err(EditError::StaleOp);
                }
                p.tracks.remove(index);
            }
            EditOp::AddEvent { track, event } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let pos = t
                    .events
                    .partition_point(|e| e.start <= event.start)
                    .min(t.events.len());
                t.events.insert(pos, event);
            }
            EditOp::RemoveEvent { track, event } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let Some(pos) = t.events.iter().position(|e| e.id == event.id) else {
                    return Err(EditError::StaleOp);
                };
                t.events.remove(pos);
            }
            EditOp::MoveEvent {
                track,
                event,
                new_start,
                new_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.start = new_start;
                e.media_in = new_media_in;
                let _ = track;
            }
            EditOp::TrimEvent {
                event,
                new_start,
                new_length,
                new_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.start = new_start;
                e.length = new_length;
                e.media_in = new_media_in;
            }
            EditOp::SlipEvent {
                event,
                new_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.media_in = new_media_in;
            }
            EditOp::SplitEvent {
                track,
                original,
                left,
                right,
            } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let Some(pos) = t.events.iter().position(|e| e.id == original.id) else {
                    return Err(EditError::StaleOp);
                };
                t.events[pos] = left;
                let rpos = t
                    .events
                    .partition_point(|e| e.start <= right.start)
                    .min(t.events.len());
                t.events.insert(rpos, right);
            }
            EditOp::RippleDelete {
                track,
                removed,
                shifts,
            } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let Some(pos) = t.events.iter().position(|e| e.id == removed.id) else {
                    return Err(EditError::StaleOp);
                };
                t.events.remove(pos);
                for (id, old_start) in &shifts {
                    if let Some(e) = t.events.iter_mut().find(|e| e.id == *id) {
                        e.start = e.start.saturating_sub(removed.length);
                        let _ = old_start;
                    }
                }
            }
            EditOp::SetAudioTrackProps { track, new, .. } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                match &mut t.kind {
                    crate::timeline::TrackKind::Audio(props) => *props = new,
                    crate::timeline::TrackKind::Video(_) => return Err(EditError::StaleOp),
                }
            }
            EditOp::SetMaster { new, .. } => p.master = new,
        }
        Ok(())
    }

    /// Reverts the op, restoring the captured previous state.
    pub fn revert(&self, p: &mut Project) -> Result<(), EditError> {
        match self.clone() {
            EditOp::AddTrack { index, track } => {
                if index >= p.tracks.len() || p.tracks[index].id != track.id {
                    return Err(EditError::StaleOp);
                }
                p.tracks.remove(index);
            }
            EditOp::RemoveTrack { index, track } => {
                let i = index.min(p.tracks.len());
                p.tracks.insert(i, track);
            }
            EditOp::AddEvent { track, event } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let Some(pos) = t.events.iter().position(|e| e.id == event.id) else {
                    return Err(EditError::StaleOp);
                };
                t.events.remove(pos);
            }
            EditOp::RemoveEvent { track, event } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let pos = t
                    .events
                    .partition_point(|e| e.start <= event.start)
                    .min(t.events.len());
                t.events.insert(pos, event);
            }
            EditOp::MoveEvent {
                event,
                old_start,
                old_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.start = old_start;
                e.media_in = old_media_in;
            }
            EditOp::TrimEvent {
                event,
                old_start,
                old_length,
                old_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.start = old_start;
                e.length = old_length;
                e.media_in = old_media_in;
            }
            EditOp::SlipEvent {
                event,
                old_media_in,
                ..
            } => {
                let e = p.event_mut(event).ok_or(EditError::EventNotFound(event))?;
                e.media_in = old_media_in;
            }
            EditOp::SplitEvent {
                track,
                original,
                left,
                right,
            } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                let Some(lpos) = t.events.iter().position(|e| e.id == left.id) else {
                    return Err(EditError::StaleOp);
                };
                t.events[lpos] = original;
                if let Some(rpos) = t.events.iter().position(|e| e.id == right.id) {
                    t.events.remove(rpos);
                }
            }
            EditOp::RippleDelete {
                track,
                removed,
                shifts,
            } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                for (id, old_start) in &shifts {
                    if let Some(e) = t.events.iter_mut().find(|e| e.id == *id) {
                        e.start = *old_start;
                    }
                }
                let pos = t
                    .events
                    .partition_point(|e| e.start <= removed.start)
                    .min(t.events.len());
                t.events.insert(pos, removed);
            }
            EditOp::SetAudioTrackProps { track, old, .. } => {
                let t = p.track_mut(track).ok_or(EditError::TrackNotFound(track))?;
                match &mut t.kind {
                    crate::timeline::TrackKind::Audio(props) => *props = old,
                    crate::timeline::TrackKind::Video(_) => return Err(EditError::StaleOp),
                }
            }
            EditOp::SetMaster { old, .. } => p.master = old,
        }
        Ok(())
    }
}

/// Undo/redo stacks with a bounded history.
///
/// Besides the stacks, [`History`] carries a monotonically increasing
/// `revision`: every applied/undone/redone op bumps it. Consumers that watch
/// the project (the realtime engine's mix graph, autosave, ...) diff this
/// cheap integer instead of polling the whole document.
#[derive(Debug, Default)]
pub struct History {
    undo_stack: Vec<EditOp>,
    redo_stack: Vec<EditOp>,
    limit: usize,
    revision: u64,
}

impl History {
    /// A history that remembers the most recent `limit` ops (min 1).
    pub fn new(limit: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            limit: limit.max(1),
            revision: 0,
        }
    }

    /// Monotonic change counter: bumped by every `push`/`undo`/`redo`.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Applies `op` and records it, clearing the redo branch.
    pub fn push(&mut self, op: EditOp, p: &mut Project) -> Result<(), EditError> {
        op.apply(p)?;
        self.undo_stack.push(op);
        self.redo_stack.clear();
        if self.undo_stack.len() > self.limit {
            self.undo_stack.remove(0);
        }
        self.revision += 1;
        Ok(())
    }

    /// Undoes the newest op; returns its label.
    pub fn undo(&mut self, p: &mut Project) -> Result<Option<&'static str>, EditError> {
        let Some(op) = self.undo_stack.pop() else {
            return Ok(None);
        };
        let label = op.label();
        op.revert(p)?;
        self.redo_stack.push(op);
        self.revision += 1;
        Ok(Some(label))
    }

    /// Redoes the most recently undone op; returns its label.
    pub fn redo(&mut self, p: &mut Project) -> Result<Option<&'static str>, EditError> {
        let Some(op) = self.redo_stack.pop() else {
            return Ok(None);
        };
        let label = op.label();
        op.apply(p)?;
        self.undo_stack.push(op);
        self.revision += 1;
        Ok(Some(label))
    }

    /// Whether an undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether a redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Number of recorded ops.
    pub fn len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.undo_stack.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use crate::timeline::TrackKind;

    fn add_track_op(p: &mut Project) -> EditOp {
        let id = p.alloc_id();
        EditOp::AddTrack {
            index: p.tracks.len(),
            track: Track {
                id: crate::TrackId::from_raw(id),
                name: "T".into(),
                kind: TrackKind::Audio(Default::default()),
                events: vec![],
                locked: false,
            },
        }
    }

    #[test]
    fn revision_bumps_on_push_undo_redo() {
        let mut p = Project::new("r");
        let mut h = History::new(16);
        assert_eq!(h.revision(), 0);
        h.push(add_track_op(&mut p), &mut p).unwrap();
        let r1 = h.revision();
        assert_eq!(r1, 1);
        h.undo(&mut p).unwrap();
        let r2 = h.revision();
        assert_eq!(r2, 2);
        h.redo(&mut p).unwrap();
        assert_eq!(h.revision(), 3);
        // failed ops must not bump
        let stale = EditOp::SetMaster {
            old: p.master.clone(),
            new: p.master.clone(),
        };
        // SetMaster always applies; use a stale track op instead
        let bad = EditOp::RemoveTrack {
            index: 999,
            track: p.tracks[0].clone(),
        };
        assert!(h.push(bad, &mut p).is_err());
        assert!(h.push(stale, &mut p).is_ok());
        assert_eq!(h.revision(), 4);
    }
}
