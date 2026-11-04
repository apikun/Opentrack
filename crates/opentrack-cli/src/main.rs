//! `opentrack` — command line entry point.
//!
//! Commands:
//!
//! ```text
//! opentrack demo [DIR]          build + render the bundled demo session
//! opentrack new NAME [DIR]      create an empty project skeleton
//! opentrack probe FILE          identify media via the backend chain
//! opentrack frame FILE [--at SECS] [-o OUT.bmp]
//!                               decode one video frame (pure-Rust H.264)
//! opentrack render PROJECT [-o OUT.wav] [--rate HZ]
//! opentrack smoke [--minutes N] [--realtime]
//!                               realtime-engine dropout smoke profile
//! opentrack gui                 print how to launch the egui shell
//! ```

use opentrack_core::save;
use opentrack_engine::RenderJob;
use opentrack_engine::codec::default_backends;
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
    frame FILE [--at SECS] [-o OUT.bmp]
                                   decode one video frame to a BMP (pure-Rust H.264)
    render PROJECT [-o OUT] [--rate HZ] [--video OUT.avi] [--frames DIR]
                                   [--quality N] [--export-av1]
                                   offline render; --video adds the M3
                                   compositor -> MJPEG-AVI pipeline (+ WAV
                                   master + optional PNG stills); --export-av1
                                   needs the `export-av1` feature build
    script FILE.rhai PROJECT.otp   run a project-level batch script (Rhai,
                                   RFC-0010) and save the result
    smoke [--minutes N] [--realtime]
                                   play an 8-track session through the realtime
                                   engine (M2 acceptance: zero underruns)
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
        "frame" => cmd_frame(rest),
        "render" => cmd_render(rest),
        "script" => cmd_script(rest),
        "smoke" => cmd_smoke(rest),
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
    // Pure-Rust video probe (MP4/H.264 + MJPEG-AVI without ffmpeg; anything
    // else through the ffmpeg bridge when the tools exist).
    match opentrack_engine::probe_video_any(path) {
        Ok(v) => {
            any_ok = true;
            println!(
                "  {:<14} duration {:.3}s streams: [video {}x{} {:.2}fps {}{} frames]{}",
                "video",
                v.duration_secs,
                v.width,
                v.height,
                v.fps,
                v.codec,
                v.frame_count,
                if v.has_audio { " +audio" } else { "" }
            );
        }
        Err(e) => println!("  {:<14}-> {e}", "video"),
    }
    if any_ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!("no backend could probe {}", path.display()))
    }
}

fn cmd_frame(rest: &[String]) -> Result<ExitCode, String> {
    let mut file: Option<PathBuf> = None;
    let mut at = 0.0f64;
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--at" | "-t" => {
                at = rest
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .ok_or("--at expects seconds")?;
                i += 2;
            }
            "-o" | "--out" => {
                out = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => {
                if file.is_none() {
                    file = Some(PathBuf::from(other));
                }
                i += 1;
            }
        }
    }
    let file = file.ok_or("usage: opentrack frame FILE [--at SECS] [-o OUT.bmp]")?;
    let out = out.unwrap_or_else(|| PathBuf::from("frame.bmp"));

    let mut dec = opentrack_engine::open_video_any(&file)
        .map_err(|e| format!("open {}: {e}", file.display()))?;
    let info = dec.info();
    let fps = if info.fps > 0.5 { info.fps } else { 30.0 };
    dec.seek_frame((at * fps).round().max(0.0) as u64)
        .map_err(|e| format!("seek to {at:.3}s: {e}"))?;
    let frame = dec
        .next_frame()
        .map_err(|e| format!("decode at {at:.3}s: {e}"))?;
    opentrack_engine::write_bmp(&frame, &out)
        .map_err(|e| format!("write {}: {e}", out.display()))?;
    println!(
        "wrote {} ({}x{} at {:.3}s, codec {}, key={})",
        out.display(),
        frame.width,
        frame.height,
        frame.pts_secs,
        info.codec,
        frame.key
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_smoke(rest: &[String]) -> Result<ExitCode, String> {
    let mut minutes = 10u32;
    let mut realtime = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--minutes" | "-m" => {
                minutes = rest
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .ok_or("--minutes expects a number")?;
                i += 2;
            }
            "--realtime" => {
                realtime = true;
                i += 1;
            }
            other => {
                return Err(format!("unknown smoke option `{other}`"));
            }
        }
    }
    let report = opentrack_engine::run_smoke(minutes, realtime)?;
    println!(
        "opentrack smoke: {} tracks x {:.0} min @ 48 kHz ({})",
        report.tracks,
        report.minutes,
        if report.realtime {
            "real-time paced"
        } else {
            "as fast as the producer feeds"
        }
    );
    println!(
        "  played {:.2}s ({} frames) in {:.1}s wall, underruns {}, decode issues {}",
        report.frames as f64 / 48_000.0,
        report.frames,
        report.wall_secs,
        report.underruns,
        report.issues
    );
    if report.underruns == 0 && report.issues == 0 {
        println!("  PASS: transport played the full span with zero dropouts");
        Ok(ExitCode::SUCCESS)
    } else {
        Err("smoke FAILED: dropouts or decode issues detected".into())
    }
}

fn cmd_script(rest: &[String]) -> Result<ExitCode, String> {
    let mut script_path: Option<PathBuf> = None;
    let mut project_path: Option<PathBuf> = None;
    for arg in rest {
        if project_path.is_none() && script_path.is_none() {
            script_path = Some(PathBuf::from(arg));
        } else if project_path.is_none() {
            project_path = Some(PathBuf::from(arg));
        }
    }
    let script_path = script_path.ok_or("usage: opentrack script FILE.rhai PROJECT.otp")?;
    let project_path = project_path.ok_or("usage: opentrack script FILE.rhai PROJECT.otp")?;
    let src = std::fs::read_to_string(&script_path)
        .map_err(|e| format!("read {}: {e}", script_path.display()))?;
    let mut project = save::load(&project_path).map_err(|e| format!("load: {e}"))?;
    let before = project
        .duration()
        .to_seconds_f64(project.timebase.ticks_per_second);
    let report =
        opentrack_engine::script::run(&mut project, &src).map_err(|e| format!("script: {e}"))?;
    let after = project
        .duration()
        .to_seconds_f64(project.timebase.ticks_per_second);
    // Persist the mutated project back over the input path (scripts that
    // want a copy call save_otp themselves first).
    save::save(&project, &project_path).map_err(|e| format!("save: {e}"))?;
    println!(
        "script {} applied {} step(s) over {} ({:.3}s -> {:.3}s timeline)",
        script_path.display(),
        report.steps,
        project_path.display(),
        before,
        after
    );
    for line in &report.log {
        println!("  script: {line}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_render(rest: &[String]) -> Result<ExitCode, String> {
    let mut project_path: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut rate = 48_000u32;
    let mut video: Option<PathBuf> = None;
    let mut frames_dir: Option<PathBuf> = None;
    let mut quality = 85u8;
    let mut av1 = false;
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
            "--video" => {
                video = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--frames" => {
                frames_dir = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--quality" => {
                quality = rest
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .ok_or("--quality expects 1..100")?;
                i += 2;
            }
            "--export-av1" => {
                av1 = true;
                i += 1;
            }
            other => {
                if project_path.is_none() {
                    project_path = Some(PathBuf::from(other));
                }
                i += 1;
            }
        }
    }
    let project_path = project_path
        .ok_or("usage: opentrack render PROJECT [-o OUT] [--rate HZ] [--video OUT.avi] [--frames DIR] [--quality N] [--export-av1]")?;
    let project = save::load(&project_path).map_err(|e| format!("load: {e}"))?;

    let backends = default_backends();
    let mut job = RenderJob::new(project);
    job.settings.sample_rate = rate;

    // Video path (M3): composite → JPEG → AVI/MJPEG (+ PNG stills + WAV).
    if let Some(avi_path) = &video {
        let opts = opentrack_engine::VideoRenderOptions {
            jpeg_quality: quality,
            frames_dir: frames_dir.clone(),
            audio_wav: Some(
                out.clone()
                    .unwrap_or_else(|| avi_path.with_extension("wav")),
            ),
            quiet: false,
        };
        let stats = job
            .run_video(&backends, avi_path, &opts, &mut |f| {
                eprint!("\rrendering... {:.0}%", f * 100.0);
            })
            .map_err(|e| format!("video render: {e}"))?;
        eprintln!();
        let expect = (stats.duration_secs * job.project.video.fps()).ceil() as u64;
        println!(
            "video {} ({}x{}, {} frames = {:.3}s × {} fps{}), audio {} frames",
            avi_path.display(),
            stats.width,
            stats.height,
            stats.frames,
            stats.duration_secs,
            job.project.video.fps(),
            if stats.frames == expect {
                ""
            } else {
                " FRAME COUNT MISMATCH"
            },
            stats.audio_frames.unwrap_or(0)
        );
        for issue in &stats.issues {
            eprintln!("warning: {issue}");
        }
        if stats.frames != expect {
            return Err(format!(
                "frame-count acceptance failed: {} != ceil({:.3} × {})",
                stats.frames,
                stats.duration_secs,
                job.project.video.fps()
            ));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // AV1 export (feature-gated).
    #[cfg(feature = "export-av1")]
    if av1 {
        let opts = opentrack_engine::VideoRenderOptions {
            jpeg_quality: quality,
            frames_dir: frames_dir.clone(),
            audio_wav: None,
            quiet: false,
        };
        let ivf = out
            .clone()
            .unwrap_or_else(|| project_path.with_extension("ivf"));
        let stats = job
            .run_video_av1(&ivf, &opts, 6, &mut |f| {
                eprint!("\rencoding AV1... {:.0}%", f * 100.0);
            })
            .map_err(|e| format!("av1 render: {e}"))?;
        eprintln!();
        println!(
            "av1 {} ({}x{}, {} frames)",
            ivf.display(),
            stats.width,
            stats.height,
            stats.frames
        );
        return Ok(ExitCode::SUCCESS);
    }
    #[cfg(not(feature = "export-av1"))]
    if av1 {
        return Err(
            "--export-av1 requires a build with --features export-av1 (default builds stay pure)"
                .into(),
        );
    }
    let _ = (av1, frames_dir, quality);

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
