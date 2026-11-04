//! The project document: media pool, tracks, buses, settings.

use crate::bus::AudioBus;
use crate::ids::{EventId, MediaId, TrackId};
use crate::time::{Ticks, TimeBase};
use crate::timeline::{AudioTrackProps, Event, Track, TrackKind, VideoTrackProps};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Video output settings for the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSettings {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// FPS numerator (e.g. 30000 for 29.97).
    pub fps_num: u32,
    /// FPS denominator (e.g. 1001 for 29.97).
    pub fps_den: u32,
}

impl VideoSettings {
    /// Effective frames per second.
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 {
            return 30.0;
        }
        self.fps_num as f64 / self.fps_den as f64
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps_num: 30,
            fps_den: 1,
        }
    }
}

/// One media file registered in the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    /// Pool id.
    pub id: MediaId,
    /// File path (absolute in M0; relocation/relative paths are roadmap).
    pub path: PathBuf,
    /// Human label (defaults to the file stem).
    pub label: String,
    /// Probed/stream metadata.
    pub kind: MediaKind,
}

/// Metadata for a pooled media asset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    /// Audio-only asset.
    Audio {
        /// Channel count.
        channels: u16,
        /// Sample rate in Hz.
        sample_rate: u32,
        /// Declared duration in project ticks.
        duration: Ticks,
    },
    /// Video asset (optionally with audio).
    Video {
        /// Frame width.
        width: u32,
        /// Frame height.
        height: u32,
        /// FPS numerator.
        fps_num: u32,
        /// FPS denominator.
        fps_den: u32,
        /// Declared duration in project ticks.
        duration: Ticks,
        /// Whether a usable audio stream exists.
        has_audio: bool,
    },
    /// Still image.
    Image {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

/// All media referenced by the project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaPool {
    /// Assets, in insertion order.
    pub assets: Vec<MediaAsset>,
}

impl MediaPool {
    /// Looks up an asset by id.
    pub fn get(&self, id: MediaId) -> Option<&MediaAsset> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Looks up an asset path by id.
    pub fn path_of(&self, id: MediaId) -> Option<&PathBuf> {
        self.get(id).map(|a| &a.path)
    }
}

/// The OpenTrack project document (serialized as `.otp`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Project name.
    pub name: String,
    /// Tick↔second mapping.
    pub timebase: TimeBase,
    /// Audio engine sample rate (Hz).
    pub sample_rate: u32,
    /// Video output settings.
    pub video: VideoSettings,
    /// Media pool.
    pub media: MediaPool,
    /// Tracks in display order: index 0 is the *top* video track.
    pub tracks: Vec<Track>,
    /// Master audio bus.
    pub master: AudioBus,
    /// ID allocator cursor; always greater than every issued id.
    pub next_id: u64,
}

impl Project {
    /// A fresh project with the default track stack
    /// (Video 2 / Video 1 / Audio 2 / Audio 1 — top video first).
    pub fn new(name: &str) -> Self {
        let mut p = Project {
            name: name.to_string(),
            timebase: TimeBase::default_base(),
            sample_rate: 48_000,
            video: VideoSettings::default(),
            media: MediaPool::default(),
            tracks: Vec::new(),
            master: AudioBus::default(),
            next_id: 1,
        };
        let defaults: [(&str, TrackKind); 4] = [
            ("Video 2", TrackKind::Video(VideoTrackProps::default())),
            ("Video 1", TrackKind::Video(VideoTrackProps::default())),
            ("Audio 2", TrackKind::Audio(AudioTrackProps::default())),
            ("Audio 1", TrackKind::Audio(AudioTrackProps::default())),
        ];
        for (name, kind) in defaults {
            let id = p.alloc_id();
            p.tracks.push(Track {
                id: TrackId::from_raw(id),
                name: name.to_string(),
                kind,
                events: Vec::new(),
                locked: false,
                fx_chain: Default::default(),
            });
        }
        p
    }

    /// Allocates the next unique id.
    pub fn alloc_id(&mut self) -> u64 {
        let v = self.next_id;
        self.next_id += 1;
        v
    }

    /// Track lookup by id.
    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Mutable track lookup by id.
    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Finds which track (and index within it) owns `event`.
    pub fn locate_event(&self, event: EventId) -> Option<TrackId> {
        for t in &self.tracks {
            if t.events.iter().any(|e| e.id == event) {
                return Some(t.id);
            }
        }
        None
    }

    /// Event lookup by id across all tracks.
    pub fn event(&self, event: EventId) -> Option<&Event> {
        self.tracks
            .iter()
            .flat_map(|t| &t.events)
            .find(|e| e.id == event)
    }

    /// Mutable event lookup by id across all tracks.
    pub fn event_mut(&mut self, event: EventId) -> Option<&mut Event> {
        self.tracks
            .iter_mut()
            .flat_map(|t| &mut t.events)
            .find(|e| e.id == event)
    }

    /// Media asset lookup.
    pub fn media(&self, id: MediaId) -> Option<&MediaAsset> {
        self.media.get(id)
    }

    /// Registers a new asset in the pool and returns its id.
    pub fn add_media(&mut self, path: PathBuf, label: Option<String>, kind: MediaKind) -> MediaId {
        let id = MediaId::from_raw(self.alloc_id());
        let label = label.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "media".to_string())
        });
        self.media.assets.push(MediaAsset {
            id,
            path,
            label,
            kind,
        });
        id
    }

    /// Timeline end: the max event end across all tracks (zero when empty).
    pub fn duration(&self) -> Ticks {
        self.tracks
            .iter()
            .flat_map(|t| &t.events)
            .map(|e| e.end())
            .max()
            .unwrap_or(Ticks::ZERO)
    }

    /// `true` when any audio track is soloed.
    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| match &t.kind {
            crate::timeline::TrackKind::Audio(p) => p.solo,
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_TICKS_PER_SECOND;

    #[test]
    fn default_track_stack_and_ids() {
        let p = Project::new("test");
        assert_eq!(p.tracks.len(), 4);
        assert!(p.tracks[0].is_video());
        assert!(p.tracks[3].is_audio());
        // ids are unique
        let ids: std::collections::HashSet<u64> = p.tracks.iter().map(|t| t.id.0).collect();
        assert_eq!(ids.len(), 4);
        assert!(p.next_id > 4);
    }

    #[test]
    fn duration_is_max_event_end() {
        let mut p = Project::new("test");
        let a1 = p.tracks.iter().find(|t| t.is_audio()).unwrap().id;
        let media = p.add_media(
            PathBuf::from("x.wav"),
            None,
            MediaKind::Audio {
                channels: 2,
                sample_rate: 48_000,
                duration: Ticks(10_000_000),
            },
        );
        let tps = p.timebase.ticks_per_second;
        let ev = Event {
            id: EventId::from_raw(p.alloc_id()),
            start: Ticks::from_seconds(2.0, tps),
            length: Ticks::from_seconds(3.5, tps),
            media,
            media_in: Ticks::ZERO,
            takes: vec![],
            active_take: 0,
            gain_db: 0.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
            reverse: false,
            label: "e".into(),
        };
        p.track_mut(a1).unwrap().events.push(ev);
        assert_eq!(p.duration(), Ticks(5_500_000));
        assert_eq!(p.timebase.ticks_per_second, DEFAULT_TICKS_PER_SECOND);
    }
}
