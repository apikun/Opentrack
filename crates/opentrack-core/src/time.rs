//! Rational, frame-rate-independent time.
//!
//! The timeline works in integer *ticks* against a project-wide
//! `ticks_per_second` base (default 1,000,000 — microsecond resolution).
//! Frame/sample conversions happen only at the edges (GUI drawing, engine
//! mixing), keeping the model free of rounding drift.

use serde::{Deserialize, Serialize};
use std::ops::{Add, Sub};

/// Default project timebase: 1,000,000 ticks per second (µs resolution).
pub const DEFAULT_TICKS_PER_SECOND: i64 = 1_000_000;

/// A duration or position on the timeline, in ticks.
///
/// A newtype serializes transparently as its `i64` payload, so `.otp` files
/// stay compact and readable.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Ticks(pub i64);

impl Ticks {
    /// Zero ticks.
    pub const ZERO: Ticks = Ticks(0);

    /// Builds ticks from floating-point seconds.
    pub fn from_seconds(secs: f64, ticks_per_second: i64) -> Ticks {
        Ticks((secs * ticks_per_second as f64).round() as i64).max(Ticks::ZERO)
    }

    /// Converts to floating-point seconds.
    pub fn to_seconds_f64(self, ticks_per_second: i64) -> f64 {
        self.0 as f64 / ticks_per_second as f64
    }

    /// Saturating addition (timeline times never go negative by convention).
    pub fn saturating_add(self, other: Ticks) -> Ticks {
        Ticks(self.0.saturating_add(other.0))
    }

    /// Saturating subtraction, clamped at zero.
    pub fn saturating_sub(self, other: Ticks) -> Ticks {
        Ticks(self.0.saturating_sub(other.0)).max(Ticks::ZERO)
    }

    /// Returns `true` when this is exactly zero.
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl Add for Ticks {
    type Output = Ticks;
    fn add(self, rhs: Ticks) -> Ticks {
        self.saturating_add(rhs)
    }
}

impl Sub for Ticks {
    type Output = Ticks;
    fn sub(self, rhs: Ticks) -> Ticks {
        self.saturating_sub(rhs)
    }
}

/// The project-wide mapping between ticks and wall time.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeBase {
    /// Ticks per second.
    pub ticks_per_second: i64,
}

impl TimeBase {
    /// Creates a timebase with the given resolution.
    pub const fn new(ticks_per_second: i64) -> Self {
        Self { ticks_per_second }
    }

    /// The default timebase (microsecond ticks).
    pub const fn default_base() -> Self {
        Self::new(DEFAULT_TICKS_PER_SECOND)
    }

    /// Converts seconds to ticks.
    pub fn ticks_from_seconds(&self, secs: f64) -> Ticks {
        Ticks::from_seconds(secs, self.ticks_per_second)
    }

    /// Converts ticks to seconds.
    pub fn seconds(&self, t: Ticks) -> f64 {
        t.to_seconds_f64(self.ticks_per_second)
    }

    /// Duration of a single frame at `fps`, rounded to the nearest tick
    /// (minimum 1 tick).
    pub fn frame_ticks(&self, fps: f64) -> Ticks {
        if fps <= 0.0 {
            return Ticks(1);
        }
        Ticks(((self.ticks_per_second as f64) / fps).round() as i64).max(Ticks(1))
    }

    /// Snaps a tick position to the nearest frame boundary at `fps`.
    pub fn snap_to_frame(&self, t: Ticks, fps: f64) -> Ticks {
        let f = self.frame_ticks(fps);
        if f.is_zero() {
            return t;
        }
        Ticks((t.0 as f64 / f.0 as f64).round() as i64 * f.0).max(Ticks::ZERO)
    }
}

impl Default for TimeBase {
    fn default() -> Self {
        Self::default_base()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_round_trip() {
        let tb = TimeBase::default_base();
        let t = Ticks::from_seconds(1.5, tb.ticks_per_second);
        assert_eq!(t, Ticks(1_500_000));
        assert!((tb.seconds(t) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn negative_seconds_clamp_to_zero() {
        let t = Ticks::from_seconds(-3.0, 1_000_000);
        assert_eq!(t, Ticks::ZERO);
    }

    #[test]
    fn frame_ticks_at_2997() {
        let tb = TimeBase::default_base();
        let f = tb.frame_ticks(30.0);
        assert_eq!(f, Ticks(33_333));
        let snapped = tb.snap_to_frame(Ticks(40_000), 30.0);
        assert_eq!(snapped, Ticks(33_333));
    }

    #[test]
    fn saturating_math() {
        assert_eq!(Ticks(2).saturating_sub(Ticks(5)), Ticks::ZERO);
        assert_eq!(Ticks(2) + Ticks(3), Ticks(5));
    }
}
