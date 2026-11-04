//! FX chains, automation lanes and the built-in effect registry.
//!
//! M0 shipped the descriptors; M4 ships the DSP ([`crate::fx`] feeds the
//! engine's registry), per-track chains, parameter automation and presets.
//! The descriptor table is the contract the GUI's FX picker, the engine's
//! DSP dispatch and the scripting surface all consume.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One automation point: a tick position and a value in the parameter's
/// domain (f32 — bool/int params are not automatable).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoPoint {
    /// Timeline position.
    pub tick: i64,
    /// Parameter value at that position.
    pub value: f32,
}

/// Interpolation between automation points.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutoCurve {
    /// Straight line between neighboring points (default).
    #[default]
    Linear,
    /// Hold the previous point's value until the next one (step).
    Constant,
}

/// An automation lane for one FX parameter.
///
/// Empty lanes mean "use the static parameter value". Points must stay
/// sorted by `tick` (enforced by [`AutomationLane::insert`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    /// Control points, sorted by `tick`.
    pub points: Vec<AutoPoint>,
    /// Interpolation between points.
    pub curve: AutoCurve,
}

impl AutomationLane {
    /// Inserts (or replaces at the same tick) a point, keeping sort order.
    pub fn insert(&mut self, tick: i64, value: f32) {
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => self.points[i].value = value,
            Err(i) => self.points.insert(i, AutoPoint { tick, value }),
        }
    }

    /// Removes the point at `tick`; `true` when one was removed.
    pub fn remove(&mut self, tick: i64) -> bool {
        match self.points.binary_search_by_key(&tick, |p| p.tick) {
            Ok(i) => {
                self.points.remove(i);
                true
            }
            Err(_) => false,
        }
    }

    /// Moves a point (removes + reinserts); `true` when the point existed.
    pub fn move_point(&mut self, from: i64, to: i64, new_value: Option<f32>) -> bool {
        let Some(p) = self.points.iter().find(|p| p.tick == from).copied() else {
            return false;
        };
        self.remove(from);
        self.insert(to, new_value.unwrap_or(p.value));
        true
    }

    /// `true` when the lane has no points.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Evaluates the lane at `tick`.
    ///
    /// Before the first point → that point's value; after the last → the
    /// last point's value; between neighbors → linear (or held) blend.
    pub fn sample(&self, tick: i64) -> Option<f32> {
        let pts = &self.points;
        let first = pts.first()?;
        if tick <= first.tick {
            return Some(first.value);
        }
        let last = pts.last()?;
        if tick >= last.tick {
            return Some(last.value);
        }
        let i = pts.partition_point(|p| p.tick <= tick) - 1;
        let a = pts[i];
        let b = pts[i + 1];
        match self.curve {
            AutoCurve::Constant => Some(a.value),
            AutoCurve::Linear => {
                let f = (tick - a.tick) as f32 / (b.tick - a.tick).max(1) as f32;
                Some(a.value + (b.value - a.value) * f)
            }
        }
    }
}

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

impl FxValue {
    /// Numeric view of the value (Float; Int as f32) for automation.
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            FxValue::Float(f) => Some(*f),
            FxValue::Int(i) => Some(*i as f32),
            _ => None,
        }
    }
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
    /// Per-parameter automation lanes; an absent / empty lane means the
    /// static [`Self::params`] value applies (M4).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub automation: BTreeMap<String, AutomationLane>,
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
            automation: BTreeMap::new(),
        })
    }

    /// Effective parameter value at `tick`: the automation lane when one has
    /// points, otherwise the static value, otherwise the descriptor default.
    pub fn value_at(&self, param: &str, tick: i64) -> Option<f32> {
        if let Some(lane) = self.automation.get(param)
            && let Some(v) = lane.sample(tick)
        {
            return Some(v);
        }
        self.params.get(param).and_then(|v| v.as_f32())
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

    const COMP_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "threshold_db",
            default: FxValue::Float(-18.0),
            min: Some(-60.0),
            max: Some(0.0),
        },
        ParamSpec {
            name: "ratio",
            default: FxValue::Float(4.0),
            min: Some(1.0),
            max: Some(20.0),
        },
        ParamSpec {
            name: "attack_ms",
            default: FxValue::Float(10.0),
            min: Some(0.1),
            max: Some(200.0),
        },
        ParamSpec {
            name: "release_ms",
            default: FxValue::Float(120.0),
            min: Some(5.0),
            max: Some(1000.0),
        },
        ParamSpec {
            name: "makeup_db",
            default: FxValue::Float(0.0),
            min: Some(-12.0),
            max: Some(24.0),
        },
    ];

    const LIMITER_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "ceiling_db",
            default: FxValue::Float(-1.0),
            min: Some(-12.0),
            max: Some(0.0),
        },
        ParamSpec {
            name: "release_ms",
            default: FxValue::Float(50.0),
            min: Some(5.0),
            max: Some(500.0),
        },
    ];

    const REVERB_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "send",
            default: FxValue::Float(0.25),
            min: Some(0.0),
            max: Some(1.0),
        },
        ParamSpec {
            name: "size",
            default: FxValue::Float(0.5),
            min: Some(0.05),
            max: Some(1.0),
        },
        ParamSpec {
            name: "damping",
            default: FxValue::Float(0.4),
            min: Some(0.0),
            max: Some(1.0),
        },
    ];

    const LUT_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "curve",
            default: FxValue::Int(0),
            min: Some(0.0),
            max: Some(2.0),
        },
        ParamSpec {
            name: "mix",
            default: FxValue::Float(1.0),
            min: Some(0.0),
            max: Some(1.0),
        },
    ];

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

    /// Feed-forward compressor (M4 DSP).
    pub const AUDIO_COMPRESSOR: FxDescriptor = FxDescriptor {
        kind: "audio.compressor",
        label: "Compressor",
        domain: Domain::Audio,
        params: COMP_PARAMS,
    };

    /// Brickwall limiter (M4 DSP).
    pub const AUDIO_LIMITER: FxDescriptor = FxDescriptor {
        kind: "audio.limiter",
        label: "Limiter",
        domain: Domain::Audio,
        params: LIMITER_PARAMS,
    };

    /// Schroeder reverb send/return (M4 DSP).
    pub const AUDIO_REVERB: FxDescriptor = FxDescriptor {
        kind: "audio.reverb",
        label: "Reverb Send",
        domain: Domain::Audio,
        params: REVERB_PARAMS,
    };

    /// Brightness/contrast (M4 DSP).
    pub const VIDEO_BRIGHTNESS_CONTRAST: FxDescriptor = FxDescriptor {
        kind: "video.brightness_contrast",
        label: "Brightness / Contrast",
        domain: Domain::Video,
        params: BC_PARAMS,
    };

    /// Saturation (M4 DSP).
    pub const VIDEO_SATURATION: FxDescriptor = FxDescriptor {
        kind: "video.saturation",
        label: "Saturation",
        domain: Domain::Video,
        params: SAT_PARAMS,
    };

    /// 1D LUT / curve (M4 DSP; `curve` selects a baked 256-entry LUT:
    /// 0 = none, 1 = film fade, 2 = crush blacks).
    pub const VIDEO_LUT: FxDescriptor = FxDescriptor {
        kind: "video.lut",
        label: "1D LUT",
        domain: Domain::Video,
        params: LUT_PARAMS,
    };

    /// All builtin descriptors, in registry order.
    pub fn all() -> &'static [&'static FxDescriptor] {
        &[
            &AUDIO_GAIN,
            &AUDIO_EQ3,
            &AUDIO_COMPRESSOR,
            &AUDIO_LIMITER,
            &AUDIO_REVERB,
            &VIDEO_BRIGHTNESS_CONTRAST,
            &VIDEO_SATURATION,
            &VIDEO_LUT,
        ]
    }

    /// Looks up a descriptor by kind key.
    pub fn descriptor(kind: &str) -> Option<&'static FxDescriptor> {
        all().iter().copied().find(|d| d.kind == kind)
    }

    /// Descriptors in one media domain.
    pub fn in_domain(domain: Domain) -> Vec<&'static FxDescriptor> {
        all()
            .iter()
            .copied()
            .filter(|d| d.domain == domain)
            .collect()
    }
}

/// Built-in factory presets per FX kind (M4): `(name, params)` where the map
/// holds only the values that differ from the descriptor defaults.
pub mod presets {
    use super::{FxValue, ParamValueMap};

    fn m(pairs: &[(&str, FxValue)]) -> ParamValueMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// Every preset for `kind`, in display order. Owned because preset maps
    /// are constructed at runtime (they mix defaults with overrides).
    pub fn for_kind(kind: &str) -> Vec<(&'static str, ParamValueMap)> {
        match kind {
            "audio.eq3" => vec![
                ("Flat", m(&[])),
                (
                    "Vocal Air",
                    m(&[
                        ("high_db", FxValue::Float(4.0)),
                        ("mid_freq_hz", FxValue::Float(2500.0)),
                        ("mid_db", FxValue::Float(-2.0)),
                    ]),
                ),
                (
                    "Telephone",
                    m(&[
                        ("low_db", FxValue::Float(-14.0)),
                        ("mid_freq_hz", FxValue::Float(1800.0)),
                        ("mid_db", FxValue::Float(8.0)),
                        ("high_db", FxValue::Float(-10.0)),
                    ]),
                ),
            ],
            "audio.compressor" => vec![
                (
                    "Podcast",
                    m(&[
                        ("threshold_db", FxValue::Float(-20.0)),
                        ("ratio", FxValue::Float(6.0)),
                        ("makeup_db", FxValue::Float(4.0)),
                    ]),
                ),
                (
                    "Glue",
                    m(&[
                        ("threshold_db", FxValue::Float(-12.0)),
                        ("ratio", FxValue::Float(2.0)),
                        ("attack_ms", FxValue::Float(25.0)),
                    ]),
                ),
            ],
            "audio.limiter" => vec![
                ("Master -1 dB", m(&[])),
                (
                    "Broadcast -6 dB",
                    m(&[("ceiling_db", FxValue::Float(-6.0))]),
                ),
            ],
            "audio.reverb" => vec![
                (
                    "Room",
                    m(&[
                        ("size", FxValue::Float(0.3)),
                        ("damping", FxValue::Float(0.6)),
                    ]),
                ),
                (
                    "Plate",
                    m(&[
                        ("size", FxValue::Float(0.7)),
                        ("damping", FxValue::Float(0.2)),
                    ]),
                ),
            ],
            "video.brightness_contrast" => vec![
                ("Neutral", m(&[])),
                ("Punch", m(&[("contrast", FxValue::Float(0.25))])),
                ("Lift", m(&[("brightness", FxValue::Float(0.15))])),
            ],
            "video.saturation" => vec![
                ("Neutral", m(&[])),
                ("B&W", m(&[("amount", FxValue::Float(-1.0))])),
                ("Vivid", m(&[("amount", FxValue::Float(0.5))])),
            ],
            "video.lut" => vec![
                ("None", m(&[])),
                ("Film Fade", m(&[("curve", FxValue::Int(1))])),
                ("Crush Blacks", m(&[("curve", FxValue::Int(2))])),
            ],
            _ => Vec::new(),
        }
    }
}

/// Convenience alias for a parameter map.
pub type ParamValueMap = BTreeMap<String, FxValue>;

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

    #[test]
    fn lane_sample_interpolates_and_extends() {
        let mut lane = AutomationLane::default();
        assert!(lane.sample(0).is_none(), "empty lane samples none");
        lane.insert(0, 0.0);
        lane.insert(100, 1.0);
        assert_eq!(lane.sample(-50), Some(0.0), "before first point");
        assert_eq!(lane.sample(0), Some(0.0));
        assert_eq!(lane.sample(50), Some(0.5), "linear midpoint");
        assert_eq!(lane.sample(200), Some(1.0), "after last point");
        lane.curve = AutoCurve::Constant;
        assert_eq!(lane.sample(99), Some(0.0), "constant holds previous");
    }

    #[test]
    fn lane_insert_keeps_order_and_replaces() {
        let mut lane = AutomationLane::default();
        lane.insert(100, 1.0);
        lane.insert(0, 0.0);
        lane.insert(100, 2.0);
        assert_eq!(
            lane.points.iter().map(|p| p.tick).collect::<Vec<_>>(),
            vec![0, 100]
        );
        assert_eq!(lane.sample(100), Some(2.0));
        assert!(lane.remove(0));
        assert!(!lane.remove(0));
        assert!(lane.move_point(100, 50, Some(9.0)));
        assert_eq!(lane.sample(50), Some(9.0));
    }

    #[test]
    fn value_at_prefers_automation() {
        let mut fx = FxInstance::with_defaults("audio.gain").unwrap();
        assert_eq!(fx.value_at("gain_db", 123), Some(0.0));
        fx.automation
            .entry("gain_db".into())
            .or_default()
            .insert(0, -6.0);
        assert_eq!(fx.value_at("gain_db", 999), Some(-6.0));
    }

    #[test]
    fn presets_exist_for_new_fx() {
        assert!(presets::for_kind("audio.eq3").len() >= 2);
        assert!(presets::for_kind("video.lut").len() >= 2);
        assert!(presets::for_kind("nope").is_empty());
        assert_eq!(builtin::in_domain(Domain::Video).len(), 3);
    }
}
