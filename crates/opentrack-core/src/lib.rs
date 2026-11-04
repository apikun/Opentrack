//! # opentrack-core
//!
//! The headless heart of OpenTrack: the project/timeline data model, editing
//! operations with undo, the audio bus graph, and the `.otp` project file.
//!
//! This crate is deliberately UI-free and codec-free so it can be unit-tested
//! everywhere, embedded in tools, and reused by the GUI, the CLI and the
//! render engine alike.
//!
//! Design pillars (see `docs/rfc/0002-timeline-data-model.md`):
//!
//! * **Track/Event model** — the Vegas-style timeline: tracks hold *events*
//!   (media instances in time), not clips-in-sequence.
//! * **Ticks** — all times are `i64` ticks against a project timebase, so the
//!   model is frame-rate independent and sample-exact math stays in the engine.
//! * **Value-semantics undo** — every edit is captured as a serializable
//!   [`undo::EditOp`] that knows how to apply *and* revert itself.
//! * **Clean-room** — nothing here is derived from any proprietary NLE source;
//!   behaviors are reimplemented from publicly observable workflow.

pub mod bus;
pub mod edit;
pub mod fx;
pub mod ids;
pub mod project;
pub mod save;
pub mod time;
pub mod timeline;
pub mod undo;

pub use ids::{EventId, MediaId, TrackId};
pub use project::{MediaAsset, MediaKind, MediaPool, Project, VideoSettings};
pub use time::{DEFAULT_TICKS_PER_SECOND, Ticks};
pub use timeline::{BlendMode, Event, Take, Track, TrackKind};
