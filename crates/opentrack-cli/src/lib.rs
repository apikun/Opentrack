//! Library surface of the CLI crate so integration tests (and the demo
//! generator embedded in CI) can drive the same code paths as the binary.

pub mod demo;

/// The `opentrack` binary version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
