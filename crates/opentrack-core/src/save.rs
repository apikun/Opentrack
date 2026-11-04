//! The `.otp` (OpenTrack Project) file format — pretty JSON with a version
//! header. See RFC-0006 for the compatibility policy.

use crate::project::Project;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Current `.otp` container version. Readers accept any version **lower or
/// equal** and reject newer ones with [`SaveError::UnsupportedVersion`].
pub const FORMAT_VERSION: u32 = 1;

/// File extension for OpenTrack projects.
pub const EXTENSION: &str = "otp";

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: u32,
    project: Project,
}

/// Save/load errors.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// Filesystem failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON parse/serialize failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// The file was written by a newer OpenTrack.
    #[error("unsupported .otp version {0} (this build reads v{1} and older)")]
    UnsupportedVersion(u32, u32),
}

/// Writes the project as pretty-printed `.otp` JSON.
pub fn save(p: &Project, path: &Path) -> Result<(), SaveError> {
    let file = ProjectFile {
        format: FORMAT_VERSION,
        project: p.clone(),
    };
    let mut text = serde_json::to_string_pretty(&file)?;
    text.push('\n');
    std::fs::write(path, text)?;
    Ok(())
}

/// Reads a project from `.otp`.
pub fn load(path: &Path) -> Result<Project, SaveError> {
    let text = std::fs::read_to_string(path)?;
    let file: ProjectFile = serde_json::from_str(&text)?;
    if file.format > FORMAT_VERSION {
        return Err(SaveError::UnsupportedVersion(file.format, FORMAT_VERSION));
    }
    Ok(file.project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("opentrack-test-{}-{}.otp", tag, std::process::id()));
        p
    }

    #[test]
    fn roundtrip_preserves_everything() {
        let mut p = Project::new("roundtrip");
        p.master.gain_db = -2.5;
        let media = p.add_media(
            "clip.wav".into(),
            None,
            crate::project::MediaKind::Audio {
                channels: 2,
                sample_rate: 48_000,
                duration: crate::time::Ticks(2_000_000),
            },
        );
        let track = p.tracks.iter().find(|t| t.is_audio()).unwrap().id;
        let ev = crate::timeline::Event {
            id: crate::ids::EventId::from_raw(p.alloc_id()),
            start: crate::time::Ticks(250_000),
            length: crate::time::Ticks(1_000_000),
            media,
            media_in: crate::time::Ticks(100_000),
            takes: vec![],
            active_take: 0,
            gain_db: -3.0,
            fade_in: crate::time::Ticks(50_000),
            fade_out: crate::time::Ticks(100_000),
            reverse: false,
            label: "clip".into(),
        };
        p.track_mut(track).unwrap().events.push(ev);

        let path = temp_path("roundtrip");
        save(&p, &path).expect("save");
        let q = load(&path).expect("load");
        std::fs::remove_file(&path).ok();

        assert_eq!(p, q);
        assert_eq!(q.name, "roundtrip");
        assert_eq!(q.master.gain_db, -2.5);
    }

    #[test]
    fn rejects_newer_versions() {
        let text = format!(
            "{{\"format\":9999,\"project\":{}}}",
            serde_json::to_string(&Project::new("x")).unwrap()
        );
        let path = temp_path("version");
        std::fs::write(&path, text).unwrap();
        let err = load(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, SaveError::UnsupportedVersion(9999, _)));
    }
}
