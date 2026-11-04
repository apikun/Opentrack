//! `opentrack` — command line entry point.
//!
//! Commands (M0 blueprint scope):
//!
//! ```text
//! opentrack demo [DIR]          build + render the bundled demo session
//! opentrack new NAME [DIR]      create an empty project skeleton
//! opentrack probe FILE          identify media via the backend chain
//! opentrack render PROJECT [-o OUT.wav] [--rate HZ]
//! opentrack gui                 print how to launch the egui shell
//! ```

use opentrack_core::save;
use opentrack_engine::RenderJob;
use opentrack_engine::codec::default_backends;
use opentrack_engine::mixer::RenderSettings;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod demo;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
opentrack - a clean-room, pure-Rust NLE (Vegas-style workflow)

USAGE:
    opentrack <COMMAND> [ARGS]

COMMANDS:
    demo [DIR]                     build + render the bundled demo session
    new NAME [DIR]                 create an empty project (NAME.otp)
    probe FILE                     identify media via the backend chain
    render PROJECT [-o OUT] [--rate HZ]
                                   offline audio render of a .otp project
    gui                            how to launch the egui shell
    version                        print version
    help                           print this help

PROJECT FILES:
    .otp = pretty JSON, versioned (docs/rfc/0006-project-file-format.md)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            eprintln!("run `opentrack help` for usage");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = args.get(1..).unwrap_or(&[]);
    match cmd {
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            println!("opentrack {VERSION} (blueprint M0, GPL-3.0-or-later)");
            Ok(ExitCode::SUCCESS)
        }
        "demo" => {
            let dir = rest
                .first()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("opentrack-demo"));
            demo::run(&dir)?;
            Ok(ExitCode::SUCCESS)
        }
        "new" => cmd_new(rest),
        "probe" => cmd_probe(rest),
        "render" => cmd_render(rest),
        "gui" => {
            println!("The egui shell is the `opentrack-app` crate. Launch it with:");
            println!("  cargo run -p opentrack-app");
            println!("or with a project preloaded:");
            println!("  cargo run -p opentrack-app -- --project path/to/project.otp");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

fn cmd_new(rest: &[String]) -> Result<ExitCode, String> {
    let name = rest
        .first()
        .ok_or_else(|| "usage: opentrack new NAME [DIR]".to_string())?;
    let dir = rest
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.{}", opentrack_core::save::EXTENSION));
    let project = opentrack_core::Project::new(name);
    save::save(&project, &path).map_err(|e| format!("save: {e}"))?;
    println!(
        "created {} ({} tracks, {} Hz, {}x{} @ {} fps)",
        path.display(),
        project.tracks.len(),
        project.sample_rate,
        project.video.width,
        project.video.height,
        project.video.fps()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_probe(rest: &[String]) -> Result<ExitCode, String> {
    let path = Path::new(rest.first().ok_or("usage: opentrack probe FILE")?);
    if !path.exists() {
        return Err(format!("file not found: {}", path.display()));
    }
    let backends = default_backends();
    let mut any_ok = false;
    for b in &backends {
        print!("  {:<14}", b.name());
        match b.probe(path) {
            Ok(info) => {
                any_ok = true;
                if let Some(d) = info.duration_secs {
                    print!(" duration {d:.3}s ");
                }
                print!("streams:");
                for s in &info.streams {
                    match &s.kind {
                        opentrack_engine::StreamKind::Audio {
                            channels,
                            sample_rate,
                        } => {
                            print!(" [audio {}ch {}Hz {}]", channels, sample_rate, s.codec);
                        }
                        opentrack_engine::StreamKind::Video { width, height, fps } => {
                            print!(
                                " [video {}x{} {} {}]",
                                width,
                                height,
                                fps.map(|f| format!("{f:.2}fps"))
                                    .unwrap_or_else(|| "?".into()),
                                s.codec
                            );
                        }
                        opentrack_engine::StreamKind::Other => print!(" [other {}]", s.codec),
                    }
                }
                println!();
            }
            Err(e) => println!("-> {e}"),
        }
    }
    if any_ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!("no backend could probe {}", path.display()))
    }
}

fn cmd_render(rest: &[String]) -> Result<ExitCode, String> {
    let mut project_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut rate = 48_000u32;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "-o" | "--out" => {
                out = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--rate" => {
                rate = rest
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .ok_or("--rate expects HZ")?;
                i += 2;
            }
            other => {
                if project_path.is_none() {
                    project_path = Some(PathBuf::from(other));
                }
                i += 1;
            }
        }
    }
    let project_path =
        project_path.ok_or("usage: opentrack render PROJECT [-o OUT] [--rate HZ]")?;
    let project = save::load(&project_path).map_err(|e| format!("load: {e}"))?;

    let backends = default_backends();
    let job = RenderJob::new(project);
    let job = RenderJob {
        settings: RenderSettings {
            sample_rate: rate,
            ..RenderSettings::default()
        },
        ..job
    };
    let rendered = job
        .run_audio(&backends, &mut |_| {})
        .map_err(|e| format!("render: {e}"))?;
    for issue in &rendered.issues {
        eprintln!("warning: {issue:?}");
    }
    let out = out.unwrap_or_else(|| project_path.with_extension("wav"));
    job.write_wav(&rendered, &out)
        .map_err(|e| format!("write: {e}"))?;
    let peak = rendered.peaks.iter().fold(0f32, |m, p| m.max(*p));
    println!(
        "rendered {} -> {} ({:.1}s, {} Hz stereo, peak {:.2} dBFS)",
        project_path.display(),
        out.display(),
        rendered.samples.len() as f64 / 2.0 / rendered.settings.sample_rate as f64,
        rendered.settings.sample_rate,
        if peak > 0.0 {
            20.0 * peak.log10()
        } else {
            -96.0
        }
    );
    Ok(ExitCode::SUCCESS)
}
