# Contributing to OpenTrack

First: read [`CLEANROOM.md`](CLEANROOM.md). It is short, and contributing
means agreeing to it.

## Quick start

```bash
git clone https://github.com/apikun/Opentrack.git
cd Opentrack
cargo test --workspace          # everything must be green before you start
cargo run -p opentrack-cli -- demo     # see the engine work
cargo run -p opentrack-app             # see the GUI shell
```

Toolchain: stable Rust (MSRV 1.85, see `rust-toolchain.toml`), edition 2024.
No other build dependencies — the pure-Rust path needs nothing but `cargo`.

## Ground rules

1. **Every behavior change starts as an RFC** in `docs/rfc/` (process in
   [`docs/rfc/README.md`](docs/rfc/README.md)). Bug fixes and test-only PRs
   are exempt.
2. **Every mutation of the model goes through `EditOp`** — the GUI must not
   edit `Project` fields directly (see ARCHITECTURE.md §4, invariant I7).
3. **New code needs tests.** The model crate (`opentrack-core`) targets
   unit-test coverage of every op's apply *and* revert path.
4. **No warnings.** CI runs `cargo clippy --workspace --all-targets -- -D
   warnings` and `cargo fmt --all -- --check`. Run both locally.
5. **One PR, one concern.** If you find an unrelated bug, file an issue.

## Commit & PR style

- Conventional Commits prefixes: `feat:`, `fix:`, `docs:`, `refactor:`,
  `test:`, `ci:`, `chore:`.
- Sign your commits (DCO): `git commit -s` adds a `Signed-off-by` line
  attesting you have the right to submit the code under GPL-3.0-or-later.
- PR description: what changed, why, how you tested it. Screenshots/GIFs for
  GUI changes. Reference the RFC (`RFC 0002`) if one exists.
- CI must be green (Windows → Ubuntu → macOS) before review.

## Code layout map

| Change type | Where it lives |
| --- | --- |
| Timeline/editing behavior | `crates/opentrack-core/src/{edit,undo,timeline}.rs` |
| Audio bus math | `crates/opentrack-core/src/bus.rs` |
| Project file format | `crates/opentrack-core/src/save.rs` (+ RFC 0006) |
| Decoding / encoding | `crates/opentrack-engine/src/{codec,wav,backend_ffmpeg}.rs` |
| Offline mixing | `crates/opentrack-engine/src/mixer.rs` (+ `dsp.rs`) |
| CLI commands | `crates/opentrack-cli/src/main.rs` |
| GUI | `crates/opentrack-app/src/main.rs` |

## Testing notes

- `cargo test --workspace` runs model, engine and CLI tests. The demo render
  test generates audio in-memory; it does not touch the filesystem outside
  `tempfile`.
- The ffmpeg bridge tests deliberately assert a *clean error* when ffmpeg is
  absent, so they pass in CI containers without ffmpeg.
- GUI changes: `cargo clippy -p opentrack-app` + the `gui-compile` CI job must
  pass; manual smoke test on at least one platform.

## Reporting bugs

Open an issue with: OpenTrack version/commit, OS, minimal steps to reproduce,
expected vs actual. For `.otp` files that fail to load, attach them if you
can (they are plain JSON — redact paths you care about).

## Security

Report vulnerabilities privately to the maintainers (see the repo contact in
the README) rather than filing a public issue. Timeline/render bugs that
crash the app are regular issues.

## Packaging (M5)

From a clean clone, one command builds the release and packages an
installer for the host platform:

```bash
cargo xtask installer      # or: cargo run -p xtask -- installer
```

Windows additionally ships WiX/MSIX scaffolding under `packaging/windows/`
(`make-msix.ps1` needs the Windows SDK); macOS signing/notarization steps
are documented in `packaging/macos/README.md`.
