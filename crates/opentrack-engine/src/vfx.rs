//! Video FX (M4): brightness/contrast, saturation and 1D LUTs, applied to
//! packed RGBA8. CPU reference implementations — the GPU/shader path is an
//! optimization RFC that lands *after* these pass their golden tests
//! (RFC-0007 non-goal: compositing in shaders before parity tests exist).
//!
//! All math is per-pixel straight-alpha (the color channels are touched, the
//! alpha channel is preserved). Parameters come from the [`FxInstance`]
//! model in `opentrack-core`, with per-parameter automation sampled at the
//! timeline tick being composited.

use opentrack_core::fx::{FxChain, FxValue};

/// Applies brightness/contrast: `brightness` in `[-1, 1]` adds
/// `brightness × 255`; `contrast` in `[-1, 1]` scales the distance from
/// mid-gray by `1 + contrast`.
pub fn apply_brightness_contrast(rgba: &mut [u8], brightness: f32, contrast: f32) {
    let add = brightness * 255.0;
    let scale = (1.0 + contrast).max(0.0);
    for px in rgba.as_chunks_mut::<4>().0 {
        for ch in &mut px[..3] {
            let v = *ch as f32;
            *ch = ((v - 128.0) * scale + 128.0 + add)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// Applies saturation: `amount` in `[-1, 1]` maps to a multiplier of
/// `1 + amount` (−1 → grayscale, 0 → neutral, +1 → double saturation),
/// pivoting on the BT.601 luma.
pub fn apply_saturation(rgba: &mut [u8], amount: f32) {
    let mul = 1.0 + amount;
    for px in rgba.as_chunks_mut::<4>().0 {
        let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
        let gray = 0.299 * r + 0.587 * g + 0.114 * b;
        px[0] = (gray + (r - gray) * mul).round().clamp(0.0, 255.0) as u8;
        px[1] = (gray + (g - gray) * mul).round().clamp(0.0, 255.0) as u8;
        px[2] = (gray + (b - gray) * mul).round().clamp(0.0, 255.0) as u8;
    }
}

/// Bakes a 256-entry 1D LUT for the builtin curve `id`
/// (`0` = identity, `1` = film fade, `2` = crush blacks).
pub fn lut_curve(id: i64) -> [u8; 256] {
    let mut lut = [0u8; 256];
    for (i, out) in lut.iter_mut().enumerate() {
        let v = i as f32 / 255.0;
        let mapped = match id {
            1 => {
                // Film fade: lifted blacks + gently rolled whites.
                let lifted = v * 0.92 + 0.08;
                let rolled = lifted + 0.06 * (1.0 - lifted) * lifted;
                rolled.clamp(0.0, 1.0)
            }
            2 => {
                // Crush blacks: quadratic toe that meets identity at 0.35.
                let crushed = if v < 0.35 { v * v / 0.35 } else { v };
                crushed.clamp(0.0, 1.0)
            }
            _ => v,
        };
        *out = (mapped * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

/// Applies a 1D LUT to RGB with a linear `mix` (0 = original, 1 = fully
/// graded). The same LUT applies to every channel (1D, not 3D).
pub fn apply_lut_1d(rgba: &mut [u8], lut: &[u8; 256], mix: f32) {
    let mix = mix.clamp(0.0, 1.0);
    for px in rgba.as_chunks_mut::<4>().0 {
        for ch in 0..3 {
            let graded = lut[px[ch] as usize] as f32;
            px[ch] = (px[ch] as f32 * (1.0 - mix) + graded * mix)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
}

/// Applies a track's video FX chain to a packed RGBA8 buffer.
///
/// Parameters are sampled at `tick` (project ticks), so automation lanes
/// drive the look over time. Disabled instances are skipped. Unknown kinds
/// are ignored (forward compatibility with newer registries).
pub fn apply_chain_video(chain: &FxChain, rgba: &mut [u8], tick: i64, tps: i64) {
    for fx in &chain.instances {
        if !fx.enabled {
            continue;
        }
        let at = |param: &str| -> Option<f32> { fx.value_at(param, tick) };
        let _ = tps; // reserved for per-frame-rate params; keeps signature stable
        match fx.kind.as_str() {
            "video.brightness_contrast" => {
                apply_brightness_contrast(
                    rgba,
                    at("brightness").unwrap_or(0.0),
                    at("contrast").unwrap_or(0.0),
                );
            }
            "video.saturation" => {
                apply_saturation(rgba, at("amount").unwrap_or(0.0));
            }
            "video.lut" => {
                let curve = fx
                    .params
                    .get("curve")
                    .and_then(|v| match v {
                        FxValue::Int(i) => Some(*i),
                        _ => None,
                    })
                    .unwrap_or(0);
                let automated = at("curve");
                let curve = automated.map(|f| f as i64).unwrap_or(curve);
                let lut = lut_curve(curve);
                apply_lut_1d(rgba, &lut, at("mix").unwrap_or(1.0));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels() -> Vec<u8> {
        // 2 px: mid-gray and a saturated red
        vec![128, 128, 128, 255, 200, 40, 40, 255]
    }

    #[test]
    fn neutral_params_are_identity() {
        let mut p = pixels();
        apply_brightness_contrast(&mut p, 0.0, 0.0);
        apply_saturation(&mut p, 0.0);
        assert_eq!(p, pixels());
    }

    #[test]
    fn brightness_adds_and_contrast_scales() {
        let mut p = vec![128u8, 128, 128, 255];
        apply_brightness_contrast(&mut p, 0.5, 0.0); // +127.5
        assert!(p[0] >= 253, "brightness lifts to white: {}", p[0]);
        let mut q = vec![200u8, 128, 128, 255];
        apply_brightness_contrast(&mut q, 0.0, 1.0); // distance from 128 ×2
        assert_eq!(q[0], 255);
        assert_eq!(q[1], 128);
        let mut r = vec![200u8, 128, 128, 255];
        apply_brightness_contrast(&mut r, 0.0, -1.0); // distance ×0
        assert_eq!(r[0], 128);
    }

    #[test]
    fn full_negative_saturation_grayscales() {
        let mut p = pixels();
        apply_saturation(&mut p, -1.0);
        let px = &p[..4];
        let gray0 = px[0];
        assert_eq!(px[1], gray0);
        assert_eq!(px[2], gray0);
    }

    #[test]
    fn lut_curves_are_monotonic_and_identity_is_exact() {
        let id = lut_curve(0);
        assert_eq!(id, {
            let mut a = [0u8; 256];
            for (i, v) in a.iter_mut().enumerate() {
                *v = i as u8;
            }
            a
        });
        for id in [1, 2] {
            let lut = lut_curve(id);
            for w in lut.windows(2) {
                assert!(w[0] <= w[1], "curve {id} must be monotonic");
            }
        }
        let fade = lut_curve(1);
        assert!(fade[0] > 0, "film fade lifts blacks");
        let crush = lut_curve(2);
        assert!(crush[64] < 64, "crush blacks pulls the toe down");
    }

    #[test]
    fn lut_mix_blends_linearly() {
        let lut = lut_curve(1);
        let mut p = vec![0u8, 0, 0, 255];
        apply_lut_1d(&mut p, &lut, 0.5);
        assert_eq!(p[0], (0.5 * lut[0] as f32).round() as u8);
        assert_eq!(p[3], 255, "alpha untouched");
    }

    #[test]
    fn chain_applies_enabled_instances_with_automation() {
        use opentrack_core::fx::{AutomationLane, FxInstance};
        let mut chain = FxChain::default();
        let mut sat = FxInstance::with_defaults("video.saturation").unwrap();
        // automate amount 0 → -1 between tick 0 and 1000
        let lane = AutomationLane {
            points: vec![
                opentrack_core::fx::AutoPoint {
                    tick: 0,
                    value: 0.0,
                },
                opentrack_core::fx::AutoPoint {
                    tick: 1000,
                    value: -1.0,
                },
            ],
            curve: opentrack_core::fx::AutoCurve::Linear,
        };
        sat.automation.insert("amount".into(), lane);
        chain.instances.push(sat);
        chain
            .instances
            .push(FxInstance::with_defaults("video.brightness_contrast").unwrap());
        chain.instances[1].enabled = false;

        let tps = 1000i64;
        let mut p = vec![200u8, 40, 40, 255];
        apply_chain_video(&chain, &mut p, 0, tps); // amount 0 → untouched
        assert_eq!(p, vec![200, 40, 40, 255]);
        apply_chain_video(&chain, &mut p, 1000, tps); // amount −1 → gray
        let g = p[0];
        assert_eq!(p[1], g);
        assert_eq!(p[2], g);
    }
}
