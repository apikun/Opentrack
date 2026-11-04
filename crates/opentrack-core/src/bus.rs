//! Audio bus math: gain, pan law, master bus state.

use serde::{Deserialize, Serialize};

/// Smallest dB value treated as silence (below this, gain is hard zero).
pub const SILENCE_DB: f32 = -96.0;
/// Maximum gain the UI allows on a bus/track strip (+12 dB).
pub const MAX_GAIN_DB: f32 = 12.0;
/// Minimum gain the UI allows on a bus/track strip (-60 dB).
pub const MIN_GAIN_DB: f32 = -60.0;

/// Converts dBFS to a linear amplitude factor (0 at/below [`SILENCE_DB`]).
pub fn db_to_linear(db: f32) -> f32 {
    if db <= SILENCE_DB {
        0.0
    } else {
        10.0f32.powf(db / 20.0)
    }
}

/// Converts a linear amplitude factor to dBFS (`-inf`-ish at silence).
pub fn linear_to_db(lin: f32) -> f32 {
    if lin <= 0.0 {
        SILENCE_DB - 1.0
    } else {
        20.0 * lin.log10()
    }
}

/// Clamps a UI gain value into the allowed strip range.
pub fn clamp_gain_db(db: f32) -> f32 {
    db.clamp(MIN_GAIN_DB, MAX_GAIN_DB)
}

/// Equal-power pan law (the Vegas-style constant-loudness curve).
///
/// Returns the (left, right) linear multipliers for a pan position in
/// `[-1.0, +1.0]`. Center yields `√2/2` per side so total *power* is
/// preserved as audio is panned across the field.
pub fn equal_power_pan(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let theta = (p + 1.0) * std::f32::consts::FRAC_PI_4;
    (theta.cos(), theta.sin())
}

/// State of an audio bus (track strips share this shape today; assignable
/// buses arrive in M4 — see RFC-0004).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioBus {
    /// Bus gain in dBFS.
    pub gain_db: f32,
    /// Bus pan, -1..+1.
    pub pan: f32,
    /// Mute flag.
    pub mute: bool,
}

impl Default for AudioBus {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
        }
    }
}

impl AudioBus {
    /// Effective (left, right) linear multipliers including gain, pan and
    /// mute (mute → hard silence).
    pub fn stereo_gains(&self) -> (f32, f32) {
        if self.mute {
            return (0.0, 0.0);
        }
        let g = db_to_linear(clamp_gain_db(self.gain_db));
        let (l, r) = equal_power_pan(self.pan);
        (g * l, g * r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn db_conversions() {
        assert!(close(db_to_linear(0.0), 1.0));
        assert!(close(db_to_linear(6.0), 1.9953));
        assert!(close(db_to_linear(-6.0), 0.5012));
        assert_eq!(db_to_linear(-120.0), 0.0);
        assert!(close(linear_to_db(0.5), -6.0206));
    }

    #[test]
    fn equal_power_center_preserves_power() {
        let (l, r) = equal_power_pan(0.0);
        assert!(close(l, std::f32::consts::FRAC_1_SQRT_2));
        assert!(close(r, std::f32::consts::FRAC_1_SQRT_2));
        // total power ≈ 1
        assert!(close(l * l + r * r, 1.0));
    }

    #[test]
    fn equal_power_edges_are_hard() {
        let (l, r) = equal_power_pan(-1.0);
        assert!(close(l, 1.0) && close(r, 0.0));
        let (l, r) = equal_power_pan(1.0);
        assert!(close(l, 0.0) && close(r, 1.0));
    }

    #[test]
    fn bus_mute_is_silence() {
        let mut b = AudioBus::default();
        let (l, r) = b.stereo_gains();
        assert!(l > 0.0 && r > 0.0);
        b.mute = true;
        assert_eq!(b.stereo_gains(), (0.0, 0.0));
    }

    #[test]
    fn gain_clamped() {
        assert_eq!(clamp_gain_db(99.0), MAX_GAIN_DB);
        assert_eq!(clamp_gain_db(-99.0), MIN_GAIN_DB);
    }
}
