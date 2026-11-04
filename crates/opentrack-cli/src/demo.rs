//! The `opentrack demo` command: builds a real Vegas-style session (two
//! overlapping audio events with fades), saves a `.otp` project, and renders
//! an actual crossfaded mix through the M0 engine. Everything below is real
//! code — the demo output proves the pipeline works end to end.

use opentrack_core::edit::{Session, overlaps_on_track};
use opentrack_core::project::MediaKind;
use opentrack_core::save;
use opentrack_core::time::Ticks;
use opentrack_engine::RenderJob;
use opentrack_engine::codec::default_backends;
use opentrack_engine::dsp;
use std::path::Path;

const DUR_SECS: f64 = 4.0;
const OVERLAP_AT_SECS: f64 = 2.0;

/// Applies an attack/decay envelope to mono samples in place.
fn enveloped(samples: &mut [f32], sample_rate: u32) {
    let attack = (0.15 * sample_rate as f32) as usize;
    let release = (1.2 * sample_rate as f32) as usize;
    let n = samples.len();
    for (i, s) in samples.iter_mut().enumerate() {
        let mut g = 1.0f32;
        if i < attack {
            g *= i as f32 / attack as f32;
        }
        let rem = n - i;
        if rem < release {
            g *= rem as f32 / release as f32;
        }
        *s *= g;
    }
}

fn write_tone(path: &Path, sample_rate: u32, freq: f64, amp: f32) -> Result<(), String> {
    let mut samples = dsp::sine(
        DUR_SECS as usize * sample_rate as usize,
        sample_rate,
        freq,
        amp,
    );
    enveloped(&mut samples, sample_rate);
    opentrack_engine::wav::encode_s16(path, 1, sample_rate, &samples)
        .map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Builds the demo project, saves `demo.otp`, renders `demo_mix.wav`,
/// prints a summary. Returns the output directory.
pub fn run(out_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let sr = 48_000u32;

    println!(
        "opentrack demo - building a Vegas-style session in {}",
        out_dir.display()
    );

    // 1. Synthesize two "stems" (pure Rust: no samples, just math).
    let lead_path = out_dir.join("tone_lead_440.wav");
    let pad_path = out_dir.join("tone_pad_330.wav");
    write_tone(&lead_path, sr, 440.0, 0.8)?;
    write_tone(&pad_path, sr, 329.63, 0.7)?;
    println!(
        "  [1/4] synthesized stems: {} + {}",
        lead_path
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        pad_path
            .file_name()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    );

    // 2. Assemble the project through the *editing* API (same code path the
    //    GUI will use), so undoable ops are exercised too.
    let mut session = Session::new(opentrack_core::Project::new("OpenTrack Demo"));
    let tps = session.project.timebase.ticks_per_second;
    let dur = Ticks::from_seconds(DUR_SECS, tps);

    let a1 = session
        .project
        .tracks
        .iter()
        .find(|t| t.is_audio())
        .map(|t| t.id)
        .expect("default project has audio tracks");

    let lead = session.project.add_media(
        lead_path.clone(),
        Some("Lead 440Hz".into()),
        MediaKind::Audio {
            channels: 1,
            sample_rate: sr,
            duration: dur,
        },
    );
    let pad = session.project.add_media(
        pad_path.clone(),
        Some("Pad 330Hz".into()),
        MediaKind::Audio {
            channels: 1,
            sample_rate: sr,
            duration: dur,
        },
    );

    let lead_ev = session
        .add_event(
            a1,
            lead,
            Ticks::ZERO,
            dur,
            Ticks::ZERO,
            0.0,
            Ticks::from_seconds(0.4, tps),
            Ticks::from_seconds(1.2, tps),
            None,
        )
        .map_err(|e| format!("add_event: {e}"))?;
    let pad_ev = session
        .add_event(
            a1,
            pad,
            Ticks::from_seconds(OVERLAP_AT_SECS, tps),
            dur,
            Ticks::ZERO,
            -2.0,
            Ticks::from_seconds(1.2, tps),
            Ticks::from_seconds(0.4, tps),
            None,
        )
        .map_err(|e| format!("add_event: {e}"))?;
    let _ = (lead_ev, pad_ev);

    let project_path = out_dir.join("demo.otp");
    save::save(&session.project, &project_path).map_err(|e| format!("saving project: {e}"))?;
    println!(
        "  [2/4] project saved: {} ({} track ops in undo history)",
        project_path.display(),
        session.history.len()
    );

    let overlaps = session
        .project
        .tracks
        .iter()
        .flat_map(overlaps_on_track)
        .count();
    println!(
        "        timeline: {} events, {} same-track crossfade overlap(s), {:.1}s",
        session
            .project
            .tracks
            .iter()
            .map(|t| t.events.len())
            .sum::<usize>(),
        overlaps,
        session.project.duration().to_seconds_f64(tps)
    );

    // 3. Offline render through the M0 engine.
    println!("  [3/4] rendering offline mix (pure-Rust WAV backends)...");
    let backends = default_backends();
    let job = RenderJob::new(session.project);
    let rendered = job
        .run_audio(&backends, &mut |p| {
            let pct = (p * 100.0) as u32;
            print!("\r        mixing... {pct}%");
            use std::io::Write as _;
            std::io::stdout().flush().ok();
        })
        .map_err(|e| format!("render: {e}"))?;
    println!();
    for issue in &rendered.issues {
        println!("        warning: {issue:?}");
    }

    let mix_path = out_dir.join("demo_mix.wav");
    job.write_wav(&rendered, &mix_path)
        .map_err(|e| format!("write wav: {e}"))?;

    // 4. Report.
    println!(
        "  [4/4] wrote {} ({} frames, {} Hz stereo)",
        mix_path.display(),
        rendered.samples.len() / 2,
        rendered.settings.sample_rate
    );
    let peak_db = dsp::peaks(&rendered.samples, 2)
        .iter()
        .fold(0f32, |m, p| m.max(*p));
    println!(
        "        master peak: {:.2} dBFS (linear {:.3})",
        if peak_db > 0.0 {
            20.0 * peak_db.log10()
        } else {
            -96.0
        },
        peak_db
    );
    println!();
    println!("Done. Next steps:");
    println!(
        "  * inspect the timeline GUI:  cargo run -p opentrack-app -- --project {}",
        project_path.display()
    );
    println!(
        "  * re-render from the file:   opentrack render {} -o mix2.wav",
        project_path.display()
    );
    println!(
        "  * listen to {} - it is a real crossfaded mix.",
        mix_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_runs_end_to_end() {
        let dir = std::env::temp_dir().join(format!("opentrack-cli-demo-{}", std::process::id()));
        run(&dir).expect("demo should run");
        // outputs exist and are non-trivial
        let otp = std::fs::read(dir.join("demo.otp")).unwrap();
        let mix = std::fs::read(dir.join("demo_mix.wav")).unwrap();
        assert!(otp.len() > 500);
        assert!(mix.len() > 48_000 * 2 * 2); // > 1 s of stereo s16
        // project round-trips
        let p = opentrack_core::save::load(&dir.join("demo.otp")).unwrap();
        assert_eq!(p.name, "OpenTrack Demo");
        assert_eq!(p.tracks.iter().map(|t| t.events.len()).sum::<usize>(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
