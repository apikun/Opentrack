//! FX chains and the built-in effect registry.
//!
//! M0 ships the *descriptors* and the per-track/per-event chain data. The DSP
//! implementations land in M4 (audio) and M5 (video) — see RFC-0004 and the
//! parity matrix. The descriptor table is the contract the GUI's FX picker
//! and the engine's DSP registry will both consume.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A parameter value on an FX instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FxValue {
    /// Floating parameter (most gain/frequency controls).
    Float(f32),
    /// Integer parameter (quality, mode selectors).
    Int(i64),
    /// Boolean toggle (enable phase invert, ...).
    Bool(bool),
    /// Text parameter (presets, notes).
    Text(String),
}

/// One effect instance in a chain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FxInstance {
    /// Registry key, e.g. `"audio.gain"` — see [`builtin`].
    pub kind: String,
    /// Bypass flag.
    pub enabled: bool,
    /// Parameter values by name; missing keys fall back to the descriptor
    /// default.
    pub params: BTreeMap<String, FxValue>,
}

impl FxInstance {
    /// A fresh instance of `kind` with all descriptor defaults applied.
    pub fn with_defaults(kind: &str) -> Option<Self> {
        let d = builtin::descriptor(kind)?;
        let mut params = BTreeMap::new();
        for p in d.params {
            params.insert(p.name.to_string(), p.default.clone());
        }
        Some(Self {
            kind: kind.to_string(),
            enabled: true,
            params,
        })
    }
}

/// An ordered chain of effects on a track or event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FxChain {
    /// Effects, in processing order (top of list first).
    pub instances: Vec<FxInstance>,
}

impl FxChain {
    /// `true` when the chain has no instances.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
}

/// Declared parameter of a builtin effect.
pub struct ParamSpec {
    /// Parameter key used in `FxInstance::params`.
    pub name: &'static str,
    /// Default value.
    pub default: FxValue,
    /// Inclusive minimum (Float only).
    pub min: Option<f32>,
    /// Inclusive maximum (Float only).
    pub max: Option<f32>,
}

/// Declared builtin effect.
pub struct FxDescriptor {
    /// Registry key.
    pub kind: &'static str,
    /// Human label for the FX picker.
    pub label: &'static str,
    /// Which media domain the effect applies to.
    pub domain: Domain,
    /// Parameter table.
    pub params: &'static [ParamSpec],
}

/// Media domain of an effect.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Domain {
    /// Audio-domain effect.
    Audio,
    /// Video-domain effect.
    Video,
}

/// Built-in effect registry (M0: descriptors only; DSP in M4/M5).
pub mod builtin {
    use super::{Domain, FxDescriptor, FxValue, ParamSpec};

    const GAIN_PARAMS: &[ParamSpec] = &[ParamSpec {
        name: "gain_db",
        default: FxValue::Float(0.0),
        min: Some(-24.0),
        max: Some(24.0),
    }];

    const EQ3_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "low_db",
            default: FxValue::Float(0.0),
            min: Some(-18.0),
            max: Some(18.0),
        },
        ParamSpec {
            name: "mid_db",
            default: FxValue::Float(0.0),
            min: Some(-18.0),
            max: Some(18.0),
        },
        ParamSpec {
            name: "high_db",
            default: FxValue::Float(0.0),
            min: Some(-18.0),
            max: Some(18.0),
        },
        ParamSpec {
            name: "mid_freq_hz",
            default: FxValue::Float(1000.0),
            min: Some(120.0),
            max: Some(8000.0),
        },
    ];

    const BC_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "brightness",
            default: FxValue::Float(0.0),
            min: Some(-1.0),
            max: Some(1.0),
        },
        ParamSpec {
            name: "contrast",
            default: FxValue::Float(0.0),
            min: Some(-1.0),
            max: Some(1.0),
        },
    ];

    const SAT_PARAMS: &[ParamSpec] = &[ParamSpec {
        name: "amount",
        default: FxValue::Float(0.0),
        min: Some(-1.0),
        max: Some(1.0),
    }];

    /// Per-event/track audio gain.
    pub const AUDIO_GAIN: FxDescriptor = FxDescriptor {
        kind: "audio.gain",
        label: "Gain",
        domain: Domain::Audio,
        params: GAIN_PARAMS,
    };

    /// Three-band EQ (M4 DSP).
    pub const AUDIO_EQ3: FxDescriptor = FxDescriptor {
        kind: "audio.eq3",
        label: "3-Band EQ",
        domain: Domain::Audio,
        params: EQ3_PARAMS,
    };

    /// Brightness/contrast (M5 DSP).
    pub const VIDEO_BRIGHTNESS_CONTRAST: FxDescriptor = FxDescriptor {
        kind: "video.brightness_contrast",
        label: "Brightness / Contrast",
        domain: Domain::Video,
        params: BC_PARAMS,
    };

    /// Saturation (M5 DSP).
    pub const VIDEO_SATURATION: FxDescriptor = FxDescriptor {
        kind: "video.saturation",
        label: "Saturation",
        domain: Domain::Video,
        params: SAT_PARAMS,
    };

    /// All builtin descriptors, in registry order.
    pub fn all() -> &'static [&'static FxDescriptor] {
        &[
            &AUDIO_GAIN,
            &AUDIO_EQ3,
            &VIDEO_BRIGHTNESS_CONTRAST,
            &VIDEO_SATURATION,
        ]
    }

    /// Looks up a descriptor by kind key.
    pub fn descriptor(kind: &str) -> Option<&'static FxDescriptor> {
        all().iter().copied().find(|d| d.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_gets_defaults() {
        let fx = FxInstance::with_defaults("audio.gain").unwrap();
        assert_eq!(fx.params.get("gain_db"), Some(&FxValue::Float(0.0)));
        assert!(fx.enabled);
    }

    #[test]
    fn unknown_kind_is_none() {
        assert!(FxInstance::with_defaults("nope.nope").is_none());
    }
}
