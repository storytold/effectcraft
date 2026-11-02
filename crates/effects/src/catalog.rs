//! The public effects catalogue (`docs/effects.md`), generated from the registry.
//!
//! Regenerate with `UPDATE_DOCS=1 cargo test -p effectcraft-effects --lib effects_doc_is_current`.

use crate::{CATEGORIES, registry};

/// Effects whose implementation is known to fall short of the reference behaviour:
/// `(effect id, what is missing)`. Everything else in the registry is implemented in full as far
/// as the public behaviour documentation describes it.
pub const PARTIAL: &[(&str, &str)] = &[];

/// Whether Effect Controls shows parameter (or twirl-down group) `param` (spec id path,
/// `group/param`) of an instance of effect `effect`, given the instance's current values
/// (`value(spec id)`). After Effects shows only the controls a popup selects: Levels' channel,
/// Hue/Saturation's Channel Control, the camera system of the card effects, and so on. Every
/// parameter stays animatable and addressable; this is presentation only.
pub fn param_shown(effect: &str, param: &str, value: &dyn Fn(&str) -> Option<effectcraft_keyframe::Value>) -> bool {
    let e = |id: &str| value(id).map(|v| v.as_enum()).unwrap_or(0);
    let b = |id: &str| value(id).is_some_and(|v| v.as_bool());
    match effect {
        "ec.color.levels" => {
            let ch = e("channel") as usize;
            for (i, pre) in crate::color_fx::LEVELS_CHANNEL_PREFIX.iter().enumerate() {
                if param.starts_with(pre) {
                    return ch == i + 1;
                }
            }
            let master = ["inBlack", "inWhite", "gamma", "outBlack", "outWhite", "clipToOutputBlack", "clipToOutputWhite"];
            !master.contains(&param) || ch == 0
        }
        "ec.color.huesaturation" => {
            let ch = e("channelControl") as usize;
            for (i, (pre, _, _)) in crate::color_fx::HUESAT_RANGES.iter().enumerate() {
                if param.starts_with(pre) {
                    return ch == i + 1;
                }
            }
            match param {
                "hue" | "saturation" | "lightness" => ch == 0,
                "colorizeSaturation" | "colorizeLightness" => b("colorize"),
                _ => true,
            }
        }
        "ec.color.selectivecolor" => match param.strip_prefix("details/") {
            Some(rest) => {
                let g = rest.split('/').next().unwrap_or("");
                crate::color2::SC_GROUPS.iter().position(|x| *x == g).is_none_or(|i| i == e("colors") as usize)
            }
            None => true,
        },
        "ec.sim.carddance" | "ec.transition.cardwipe" => crate::card3d::shown(param, &e).unwrap_or(true),
        "ec.sim.shatter" => match param {
            "shape/customShatterMap" | "shape/whiteTilesFixed" => e("shape/pattern") == 5,
            _ => crate::card3d::shown(param, &e).unwrap_or(true),
        },
        _ => {
            let _ = b("");
            true
        }
    }
}

/// Controls of GPU-accelerated effects that only the CPU implements: while any of them is away
/// from its default the effect renders on the CPU. (Prefixes ending in `/` cover a whole group.)
const CPU_ONLY_CONTROLS: &[(&str, &[&str])] = &[
    ("ec.color.lumetri", &["highDynamicRange", "curves/rgbCurves/", "curves/hueSaturationCurves/", "hslSecondary/"]),
    (
        "ec.color.colorama",
        &["inputPhase/addPhase", "outputCycle/usePresetPalette", "modify/", "pixelSelection/matchingMode", "masking/maskingMode", "masking/compositeOverLayer"],
    ),
    (
        "ec.keying.keylight",
        &[
            "foregroundColourCorrection/colourSuppression",
            "foregroundColourCorrection/suppressionAmount",
            "foregroundColourCorrection/colourBalanceSaturation",
            "edgeColourCorrection/edgeColourSuppression",
            "edgeColourCorrection/edgeSuppressionAmount",
            "edgeColourCorrection/edgeColourBalanceSaturation",
            "sourceCrops/xMethod",
            "sourceCrops/yMethod",
            "sourceCrops/edgeColourAlpha",
        ],
    ),
    ("ec.noise.addgrain", &["viewingMode", "preset", "color/redBalance", "color/greenBalance", "color/blueBalance", "previewRegion/"]),
    ("ec.blur.cameralens", &["irisProperties/diffractionFringe", "blurMap/blurMapLayer"]),
    ("ec.noise.removegrain", &["temporalFiltering/enabled"]),
    ("ec.distort.liquify", &["viewFreezeAreaMask", "viewMesh"]),
    ("ec.distort.ccpowerpin", &["unstretch"]),
    ("ec.matte.refinesoft", &["reduceChatter", "useMotionBlur", "invert"]),
    ("ec.matte.refinehard", &["reduceChatter", "useMotionBlur", "invert"]),
];

/// Whether the GPU kernel of effect `id` covers the instance's current settings (see
/// [`CPU_ONLY_CONTROLS`]; plus Cell Pattern's HQ variants and Turbulent Displace's locked
/// pinning).
pub fn gpu_supported(id: &str, ctx: &crate::EffectCtx) -> bool {
    match id {
        "ec.generate.cellpattern" if (6..=10).contains(&ctx.params.e("cellPattern")) => return false,
        "ec.distort.turbulentdisplace" if ctx.params.e("pinning") >= 8 => return false,
        // #1195: CC Particle World's motion trails (the averaged shutter samples) render on the
        // CPU kernel until the GPU particle pass supports them.
        "ec.sim.ccparticleworld" if ctx.env.shutter.filter(|(a, _, _)| *a > 0.0).is_some() => return false,
        _ => {}
    }
    let Some((_, controls)) = CPU_ONLY_CONTROLS.iter().find(|(e, _)| *e == id) else { return true };
    let Some(spec) = crate::find(id) else { return true };
    spec.params
        .iter()
        .filter(|ps| controls.iter().any(|c| if c.ends_with('/') { ps.id.starts_with(c) } else { ps.id == *c }))
        .all(|ps| ctx.params.get(ps.id).is_none_or(|v| *v == crate::default_value(ps, ctx.layer_size)))
}

/// Implementation status of effect `id` (`Implemented` or `Partial: …`).
pub fn status(id: &str) -> String {
    match PARTIAL.iter().find(|(e, _)| *e == id) {
        Some((_, note)) => format!("Partial: {note}"),
        None => "Implemented".to_string(),
    }
}

/// `docs/effects.md`.
pub fn effects_markdown() -> String {
    let r = registry();
    let mut s = String::new();
    s.push_str("# Effects\n\n");
    s.push_str("<!-- Generated from the effect registry (crates/effects/src/catalog.rs); do not edit by hand.\n");
    s.push_str("     Regenerate: UPDATE_DOCS=1 cargo test -p effectcraft-effects --lib effects_doc_is_current -->\n\n");
    let params: usize = r.iter().map(|e| e.params.len()).sum();
    let gpu = r.iter().filter(|e| e.gpu).count();
    let float = r.iter().filter(|e| e.float).count();
    let partial = r.iter().filter(|e| PARTIAL.iter().any(|(id, _)| *id == e.id)).count();
    s.push_str(&format!(
        "EffectCraft ships {} effects with {} parameters in total, grouped into the same categories as the Effects & Presets \
         panel. Every effect is our own implementation, written from public behaviour descriptions and standard \
         image-processing literature. Parameter names, order, popup options, units, defaults and ranges follow the \
         reference application so that projects, expressions and muscle memory carry over.\n\n",
        r.len(),
        params
    ));
    s.push_str(&format!(
        "- **GPU**: {gpu} effects also run on the GPU compositor with identical results.\n\
         - **32**: {float} effects process 32-bit float (HDR, overbright) pixels without clamping.\n\
         - **Status**: {} implemented in full, {partial} partial (what is missing is listed).\n\n",
        r.len() - partial
    ));
    s.push_str("Effects are addressed by id (`ec.<category>.<name>`) or display name in commands, scripts and the control channel, and every parameter by its id (`effects/#1/blurriness`).\n\n");
    for cat in CATEGORIES {
        let list: Vec<_> = r.iter().filter(|e| e.category == *cat).collect();
        if list.is_empty() {
            continue;
        }
        s.push_str(&format!("## {cat}\n\n| Effect | Id | Params | GPU | 32 | Status |\n|---|---|---:|:-:|:-:|---|\n"));
        for e in list {
            s.push_str(&format!(
                "| {} | `{}` | {} | {} | {} | {} |\n",
                e.name.replace('|', "\\|"),
                e.id,
                e.params.iter().filter(|p| !matches!(p.ui, effectcraft_project::ParamUi::Hidden)).count(),
                if e.gpu { "GPU" } else { "" },
                if e.float { "32" } else { "" },
                status(e.id).replace('|', "\\|"),
            ));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn effects_doc_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/effects.md");
        let want = super::effects_markdown();
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(path, &want).unwrap();
            return;
        }
        // A Windows checkout may use CRLF; catalog content must still match exactly.
        let have = std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "docs/effects.md is stale: UPDATE_DOCS=1 cargo test -p effectcraft-effects --lib effects_doc_is_current");
    }

    #[test]
    fn cpu_only_controls_keep_effects_off_the_gpu() {
        use effectcraft_keyframe::Value;
        let ctx_with = |id: &str, set: &[(&str, Value)]| {
            let s = crate::find(id).unwrap();
            let mut values: std::collections::HashMap<String, Value> =
                s.params.iter().map(|p| (p.id.to_string(), crate::default_value(p, [64.0, 64.0]))).collect();
            for (k, v) in set {
                values.insert(k.to_string(), v.clone());
            }
            crate::Params { values }
        };
        let ok = |id: &str, set: &[(&str, Value)]| {
            let params = ctx_with(id, set);
            let ctx = crate::EffectCtx { params: &params, time: 0.0, layer_size: [64.0, 64.0], seed: 0, adjustment: false, env: Default::default() };
            super::gpu_supported(id, &ctx)
        };
        assert!(ok("ec.color.lumetri", &[]));
        assert!(!ok("ec.color.lumetri", &[("hslSecondary/correction/saturation", Value::Scalar(50.0))]));
        assert!(!ok("ec.noise.addgrain", &[("preset", Value::Enum(2))]));
        // #1195: CC Particle World's motion trails (shutter samples) render on the CPU.
        {
            let params = ctx_with("ec.sim.ccparticleworld", &[]);
            let env = crate::EffectEnv { frame_rate: 30.0, shutter: Some((180.0, -90.0, 16)), ..Default::default() };
            let ctx = crate::EffectCtx { params: &params, time: 0.0, layer_size: [64.0, 64.0], seed: 0, adjustment: false, env };
            assert!(!super::gpu_supported("ec.sim.ccparticleworld", &ctx));
        }
        assert!(ok("ec.sim.ccparticleworld", &[]));
        assert!(ok("ec.generate.cellpattern", &[("cellPattern", Value::Enum(3))]));
        assert!(!ok("ec.generate.cellpattern", &[("cellPattern", Value::Enum(9))]));
        assert!(!ok("ec.distort.turbulentdisplace", &[("pinning", Value::Enum(9))]));
        assert!(ok("ec.distort.ccpowerpin", &[]));
        assert!(!ok("ec.distort.ccpowerpin", &[("unstretch", Value::Bool(true))]));
        assert!(ok("ec.blur.gaussian", &[]));
    }

    #[test]
    fn popups_choose_the_shown_controls() {
        use effectcraft_keyframe::Value;
        let with = |pairs: &'static [(&'static str, u32)]| move |id: &str| pairs.iter().find(|(k, _)| *k == id).map(|(_, v)| Value::Enum(*v));
        let shown = |e: &str, p: &str, pairs: &'static [(&'static str, u32)]| super::param_shown(e, p, &with(pairs));
        // Levels: RGB shows the master controls only; Red shows the red ones.
        assert!(shown("ec.color.levels", "inBlack", &[]));
        assert!(!shown("ec.color.levels", "redInBlack", &[]));
        assert!(shown("ec.color.levels", "redInBlack", &[("channel", 1)]));
        assert!(!shown("ec.color.levels", "inBlack", &[("channel", 1)]));
        assert!(shown("ec.color.levels", "channel", &[("channel", 1)]));
        // Hue/Saturation: Master or one colour range.
        assert!(shown("ec.color.huesaturation", "hue", &[]));
        assert!(shown("ec.color.huesaturation", "bluesHue", &[("channelControl", 5)]));
        assert!(!shown("ec.color.huesaturation", "redsHue", &[("channelControl", 5)]));
        // Card effects: the camera system's group.
        assert!(shown("ec.sim.carddance", "cameraPosition", &[]));
        assert!(!shown("ec.sim.carddance", "cornerPins", &[]));
        assert!(shown("ec.transition.cardwipe", "cornerPins/upperLeftCorner", &[("cameraSystem", 1)]));
        assert!(!shown("ec.sim.shatter", "shape/customShatterMap", &[]));
        // Selective Color: the chosen range's Details group.
        assert!(shown("ec.color.selectivecolor", "details/reds", &[]));
        assert!(!shown("ec.color.selectivecolor", "details/blues/bluesCyan", &[]));
        assert!(shown("ec.color.selectivecolor", "details", &[]));
        // Everything else is always shown.
        assert!(shown("ec.blur.gaussian", "blurriness", &[]));
    }

    #[test]
    fn partial_notes_name_registered_effects() {
        for (id, note) in super::PARTIAL {
            assert!(crate::find(id).is_some(), "PARTIAL names unknown effect {id}");
            assert!(!note.trim().is_empty());
        }
    }
}
