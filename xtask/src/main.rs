//! `cargo xtask` — OpenTrack build & packaging automation (M5).
//!
//! One command from a clean clone to an installable artifact for the host
//! platform (the M5 acceptance rule):
//!
//! ```text
//! cargo xtask installer          # build release + package for this OS
//! cargo xtask installer --out D  # choose the output directory
//! cargo xtask site               # (re)generate the docs site into docs/site
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = args.get(1..).unwrap_or(&[]);
    match cmd {
        "installer" => cmd_installer(rest),
        "site" => crate::site::generate(),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!(
            "unknown command `{other}` (try `cargo xtask help`)"
        )),
    }
}

const USAGE: &str = "\
xtask — OpenTrack build automation

USAGE:
    cargo xtask <COMMAND>

COMMANDS:
    installer [--out DIR]     build release binaries and package an
                              installer for the host platform:
                              Windows -> opentrack-x86_64-windows.zip
                                         (+ .wixproj/.wxs for MSI/MSIX)
                              Linux   -> .AppImage (when appimagetool exists)
                                         otherwise a ready-to-run .AppDir
                                         plus .desktop entry
                              macOS   -> OpenTrack.app bundle + .dmg
    site                      regenerate docs/site (static HTML)
    help                      print this help
";

fn cmd_installer(rest: &[String]) -> Result<(), String> {
    let mut out_dir = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--out" => {
                out_dir = rest.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            other => return Err(format!("unknown installer option `{other}`")),
        }
    }
    let root = project_root()?;
    let out = out_dir.unwrap_or_else(|| root.join("dist"));
    std::fs::create_dir_all(&out).map_err(|e| format!("mkdir {}: {e}", out.display()))?;

    // 1. Release build of both binaries. On Linux without ALSA headers the
    // realtime-audio feature cannot link; degrade to a silent GUI binary
    // (documented) instead of failing the whole packaging run.
    status("building release binaries (CLI + GUI)…")?;
    let full = Command::new(cargo())
        .args([
            "build",
            "--release",
            "-p",
            "opentrack-cli",
            "-p",
            "opentrack-app",
        ])
        .current_dir(&root)
        .status();
    match full {
        Ok(st) if st.success() => {}
        _ => {
            status(
                "full build failed (missing ALSA headers?) — retrying with the GUI audio feature off",
            )?;
            run_cmd(
                Command::new(cargo())
                    .args(["build", "--release", "-p", "opentrack-cli"])
                    .current_dir(&root),
            )?;
            run_cmd(
                Command::new(cargo())
                    .args([
                        "build",
                        "--release",
                        "-p",
                        "opentrack-app",
                        "--no-default-features",
                    ])
                    .current_dir(&root),
            )?;
        }
    }
    let bin = bin_dir(&root);
    let cli = bin.join(if cfg!(windows) {
        "opentrack.exe"
    } else {
        "opentrack"
    });
    let app = bin.join(if cfg!(windows) {
        "opentrack-app.exe"
    } else {
        "opentrack-app"
    });
    for f in [&cli, &app] {
        if !f.exists() {
            return Err(format!("expected binary missing: {}", f.display()));
        }
    }

    match std::env::consts::OS {
        "windows" => crate::pack::windows(&root, &out, &cli, &app),
        "linux" => crate::pack::linux(&root, &out, &cli, &app),
        "macos" => crate::pack::macos(&root, &out, &cli, &app),
        other => Err(format!("no installer recipe for `{other}` yet")),
    }
}

fn status(msg: &str) -> Result<(), String> {
    println!("xtask: {msg}");
    Ok(())
}

fn run_cmd(cmd: &mut Command) -> Result<(), String> {
    let st = cmd
        .status()
        .map_err(|e| format!("spawn {:?}: {e}", cmd.get_program()))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("command {:?} failed with {st}", cmd.get_program()))
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn project_root() -> Result<PathBuf, String> {
    // xtask runs via `cargo run -p xtask`, so CARGO_MANIFEST_DIR is xtask/.
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map(|p| p.parent().expect("xtask parent").to_path_buf())
        .map_err(|_| "CARGO_MANIFEST_DIR not set (run via `cargo xtask`)".into())
}

/// Resolves where `cargo build --release` put host binaries: prefer the
/// bare `target/release` (no explicit triple), fall back to the
/// `target/<triple>/release` layout.
pub fn bin_dir(root: &Path) -> PathBuf {
    let plain = root.join("target/release");
    if plain.join("opentrack").exists() || plain.join("opentrack.exe").exists() {
        return plain;
    }
    // Ask cargo for the host triple.
    if let Ok(out) = Command::new("rustc").args(["-vV"]).output()
        && let Ok(text) = String::from_utf8(out.stdout)
        && let Some(host) = text.lines().find_map(|l| l.strip_prefix("host: "))
        && let t = root.join("target").join(host.trim()).join("release")
        && (t.join("opentrack").exists() || t.join("opentrack.exe").exists())
    {
        return t;
    }
    plain
}

mod pack {
    //! Per-platform installer recipes.

    use super::{run_cmd, status};
    use std::path::{Path, PathBuf};

    pub(crate) fn version() -> String {
        // Single source of truth: the workspace manifest.
        let manifest = include_str!("../../Cargo.toml");
        manifest
            .lines()
            .skip_while(|l| *l != "[workspace.package]")
            .take_while(|l| !l.starts_with('['))
            .find_map(|l| l.strip_prefix("version = "))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_else(|| "0.0.0".into())
    }

    fn copy_readme(root: &Path, stage: &Path) -> Result<(), String> {
        for f in ["README.md", "LICENSE"] {
            let src = root.join(f);
            if src.exists() {
                std::fs::copy(&src, stage.join(f)).map_err(|e| format!("copy {f}: {e}"))?;
            }
        }
        Ok(())
    }

    /// Windows: zip with the binaries + the WiX/MSIX scaffolding (the MSIX
    /// itself is built by `packaging/windows/make-msix.ps1` on a Windows
    /// box with the Windows SDK; the zip alone is the supported artifact).
    pub fn windows(root: &Path, out: &Path, cli: &PathBuf, app: &PathBuf) -> Result<(), String> {
        let stage = out.join("opentrack-win-stage");
        let _ = std::fs::remove_dir_all(&stage);
        std::fs::create_dir_all(&stage).map_err(|e| format!("mkdir stage: {e}"))?;
        std::fs::copy(cli, stage.join("opentrack.exe")).map_err(|e| format!("copy cli: {e}"))?;
        std::fs::copy(app, stage.join("opentrack-app.exe"))
            .map_err(|e| format!("copy app: {e}"))?;
        copy_readme(root, &stage)?;
        // WiX + MSIX sources ship with the zip so packagers can go further.
        let wix_dir = root.join("packaging/windows");
        if wix_dir.exists() {
            copy_dir(&wix_dir, &stage.join("packaging/windows"))?;
        }
        let zip = out.join(format!("opentrack-x86_64-windows-{}.zip", version()));
        let _ = std::fs::remove_file(&zip);
        let st = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Compress-Archive -Path '{}/*' -DestinationPath '{}'",
                    stage.display(),
                    zip.display()
                ),
            ])
            .status()
            .map_err(|e| format!("Compress-Archive: {e}"))?;
        if !st.success() {
            return Err("Compress-Archive failed".into());
        }
        let _ = std::fs::remove_dir_all(&stage);
        status(&format!("wrote {}", zip.display()))?;
        status("MSIX: run packaging/windows/make-msix.ps1 on Windows (needs the Windows SDK)")
    }

    /// Linux: an .AppDir (always) + .AppImage when appimagetool exists.
    pub fn linux(root: &Path, out: &Path, cli: &PathBuf, app: &PathBuf) -> Result<(), String> {
        let appdir = out.join("OpenTrack.AppDir");
        let _ = std::fs::remove_dir_all(&appdir);
        std::fs::create_dir_all(appdir.join("usr/bin")).map_err(|e| format!("mkdir: {e}"))?;
        std::fs::create_dir_all(appdir.join("usr/share/icons/hicolor/256x256/apps"))
            .map_err(|e| format!("mkdir icons: {e}"))?;
        std::fs::copy(cli, appdir.join("usr/bin/opentrack")).map_err(|e| format!("copy: {e}"))?;
        std::fs::copy(app, appdir.join("usr/bin/opentrack-app"))
            .map_err(|e| format!("copy: {e}"))?;
        copy_readme(root, &appdir)?;
        // .desktop entry (AppImage spec: the file at the AppDir root).
        std::fs::write(
            appdir.join("opentrack.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=OpenTrack\nComment=Clean-room Rust NLE\nExec=opentrack-app\nIcon=opentrack\nCategories=AudioVideo;Video;AudioVideoEditing;\nTerminal=false\nStartupWMClass=opentrack\nVersion={}\n",
                version()
            ),
        )
        .map_err(|e| format!("desktop: {e}"))?;
        std::fs::write(appdir.join("AppRun"), APPRUN_SCRIPT).map_err(|e| format!("AppRun: {e}"))?;
        make_executable(&appdir.join("AppRun"))?;
        make_executable(&appdir.join("usr/bin/opentrack"))?;
        make_executable(&appdir.join("usr/bin/opentrack-app"))?;
        // Icon (SVG → the AppImage convention allows .svg directly).
        let icon = root.join("packaging/linux/opentrack.svg");
        if icon.exists() {
            std::fs::copy(
                &icon,
                appdir.join("usr/share/icons/hicolor/256x256/apps/opentrack.svg"),
            )
            .map_err(|e| format!("icon: {e}"))?;
            std::fs::copy(&icon, appdir.join("opentrack.svg")).map_err(|e| format!("icon: {e}"))?;
        }
        // Try an AppImage when the tool is present.
        let tool = which("appimagetool");
        if let Some(tool) = tool {
            let appimage = out.join(format!("OpenTrack-{}-x86_64.AppImage", version()));
            let _ = std::fs::remove_file(&appimage);
            run_cmd(
                std::process::Command::new(tool)
                    .arg(&appdir)
                    .arg(&appimage)
                    .env("ARCH", "x86_64"),
            )?;
            status(&format!("wrote {}", appimage.display()))?;
        } else {
            status(
                "appimagetool not found — shipping the .AppDir (run `appimagetool dist/OpenTrack.AppDir` to finish)",
            )?;
        }
        Ok(())
    }

    /// macOS: .app bundle + .dmg via hdiutil (codesigning/notarization is a
    /// documented, identity-bound step — see packaging/macos/README.md).
    pub fn macos(root: &Path, out: &Path, cli: &PathBuf, app: &PathBuf) -> Result<(), String> {
        let bundle = out.join("OpenTrack.app");
        let _ = std::fs::remove_dir_all(&bundle);
        for sub in ["Contents/MacOS", "Contents/Resources"] {
            std::fs::create_dir_all(bundle.join(sub)).map_err(|e| format!("mkdir: {e}"))?;
        }
        std::fs::copy(cli, bundle.join("Contents/MacOS/opentrack"))
            .map_err(|e| format!("copy: {e}"))?;
        std::fs::copy(app, bundle.join("Contents/MacOS/opentrack-app"))
            .map_err(|e| format!("copy: {e}"))?;
        std::fs::write(
            bundle.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>OpenTrack</string>
  <key>CFBundleDisplayName</key><string>OpenTrack</string>
  <key>CFBundleIdentifier</key><string>io.github.apikun.OpenTrack</string>
  <key>CFBundleVersion</key><string>{}</string>
  <key>CFBundleShortVersionString</key><string>{}</string>
  <key>CFBundleExecutable</key><string>opentrack-app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
"#,
                version(),
                version()
            ),
        )
        .map_err(|e| format!("Info.plist: {e}"))?;
        copy_readme(root, &bundle.join("Contents/Resources"))?;
        make_executable(&bundle.join("Contents/MacOS/opentrack"))?;
        make_executable(&bundle.join("Contents/MacOS/opentrack-app"))?;
        status(&format!("built {}", bundle.display()))?;
        // dmg when hdiutil exists (always, on macOS).
        let dmg = out.join(format!("OpenTrack-{}-aarch64.dmg", version()));
        let _ = std::fs::remove_file(&dmg);
        let st = std::process::Command::new("hdiutil")
            .args([
                "create",
                "-volname",
                "OpenTrack",
                "-srcfolder",
                &bundle.display().to_string(),
                "-ov",
                &dmg.display().to_string(),
            ])
            .status()
            .map_err(|e| format!("hdiutil: {e}"))?;
        if st.success() {
            status(&format!("wrote {}", dmg.display()))?;
        } else {
            status("hdiutil failed — the .app bundle is still valid")?;
        }
        status("notarization: see packaging/macos/README.md (needs an Apple identity)")
    }

    const APPRUN_SCRIPT: &str = r#"#!/bin/sh
HERE="$(dirname "$(readlink -f "${0}")")"
exec "$HERE/usr/bin/opentrack-app" "$@"
"#;

    fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
        for entry in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
            let entry = entry.map_err(|e| format!("entry: {e}"))?;
            let ty = entry.file_type().map_err(|e| format!("type: {e}"))?;
            let to = dst.join(entry.file_name());
            if ty.is_dir() {
                copy_dir(&entry.path(), &to)?;
            } else {
                std::fs::copy(entry.path(), &to).map_err(|e| format!("copy: {e}"))?;
            }
        }
        Ok(())
    }

    fn make_executable(path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod {}: {e}", path.display()))?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    fn which(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    }
}

mod site {
    //! Static docs-site generator (M5): plain, dependency-free HTML.

    pub fn generate() -> Result<(), String> {
        let root = std::env::var("CARGO_MANIFEST_DIR")
            .map(std::path::PathBuf::from)
            .map(|p| p.parent().expect("root").to_path_buf())
            .map_err(|_| "run via `cargo xtask site`".to_string())?;
        let out = root.join("docs/site");
        std::fs::create_dir_all(&out).map_err(|e| format!("mkdir: {e}"))?;
        std::fs::write(out.join("index.html"), index_html()).map_err(|e| format!("index: {e}"))?;
        std::fs::write(out.join("style.css"), style_css()).map_err(|e| format!("css: {e}"))?;
        // mirror the RFCs + tutorials as plain text links
        for sub in ["docs/rfc", "docs/tutorials"] {
            let src = root.join(sub);
            if src.exists() {
                for entry in std::fs::read_dir(&src).map_err(|e| format!("read: {e}"))? {
                    let entry = entry.map_err(|e| format!("entry: {e}"))?;
                    if entry.path().extension().is_some_and(|e| e == "md") {
                        std::fs::create_dir_all(out.join(sub)).ok();
                        let name = entry.file_name();
                        std::fs::copy(entry.path(), out.join(sub).join(&name))
                            .map_err(|e| format!("copy {sub}: {e}"))?;
                    }
                }
            }
        }
        println!("xtask: docs site written to {}", out.display());
        Ok(())
    }

    fn index_html() -> String {
        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>OpenTrack — a clean-room Rust NLE</title>
<link rel="stylesheet" href="style.css">
</head>
<body>
<header>
<h1>OpenTrack</h1>
<p>A clean-room, pure-Rust reimplementation of the classic Vegas-style NLE workflow. GPL-3.0.</p>
</header>
<main>
<section>
<h2>Milestones</h2>
<ul>
<li><strong>M0</strong> — blueprint, data model, offline audio render, GUI shell ✅</li>
<li><strong>M1</strong> — interactive timeline editing + video preview ✅</li>
<li><strong>M2</strong> — realtime audio playback (cpal, lock-free, peak meters) ✅</li>
<li><strong>M3</strong> — video decode/composite/render: MJPEG-AVI in pure Rust, ffmpeg bridge, CPU compositor, frame-accurate export ✅</li>
<li><strong>M4</strong> — FX framework: 3-band EQ, compressor, limiter, reverb; brightness/contrast/saturation/1D-LUT; automation lanes; parity-tested ✅</li>
<li><strong>M5</strong> — plugin ABI v0, Rhai scripting, installers via <code>cargo xtask installer</code>, docs site ✅</li>
</ul>
</section>
<section>
<h2>Docs</h2>
<ul>
<li><a href="https://github.com/apikun/Opentrack/blob/main/README.md">README</a></li>
<li><a href="https://github.com/apikun/Opentrack/blob/main/ARCHITECTURE.md">Architecture</a></li>
<li>RFCs: <a href="rfc/index.html">index</a></li>
<li><a href="tutorials/00-getting-started.md">Getting started tutorial</a></li>
</ul>
</section>
<section>
<h2>Quick start</h2>
<pre><code>git clone https://github.com/apikun/Opentrack.git
cd Opentrack
cargo xtask installer   # build + package for your platform
cargo run -p opentrack-app</code></pre>
</section>
</main>
<footer>OpenTrack {} · GPL-3.0-or-later · built with egui, cpal, zune-jpeg, rhai</footer>
</body>
</html>
"#,
            crate::pack::version()
        )
    }

    fn style_css() -> &'static str {
        "body{font-family:system-ui,sans-serif;max-width:52rem;margin:2rem auto;padding:0 1rem;color:#1c1c22;background:#fafafa}
header h1{margin-bottom:0}
code,pre{background:#eee;border-radius:4px;padding:.1em .35em;font-size:.95em}
pre{padding:.8em;overflow:auto}
li{margin:.3em 0}
footer{margin-top:3rem;color:#777;font-size:.9em}"
    }
}
