//! GPU colour-family effects vs the CPU effects (the oracle): direct on a buffer at full and
//! half resolution, as adjustment, and composited at 8 and 32 bpc.

use effectcraft_keyframe::Value;

use crate::tests::{c, effect_case, n};

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn on() -> Value {
    Value::Bool(true)
}

#[test]
fn color_balance_vibrance_tritone_mixer() {
    effect_case("ec.color.colorbalance", &[("shadowRed", n(40.0)), ("midGreen", n(-30.0)), ("hiBlue", n(60.0))]);
    effect_case("ec.color.colorbalance", &[("shadowBlue", n(-50.0)), ("midRed", n(70.0)), ("hiGreen", n(25.0)), ("preserveLuminosity", on())]);
    effect_case("ec.color.vibrance", &[("vibrance", n(60.0)), ("saturation", n(-20.0))]);
    effect_case("ec.color.vibrance", &[("vibrance", n(-40.0)), ("saturation", n(35.0))]);
    effect_case("ec.color.tritone", &[("midtones", c(0.6, 0.3, 0.5)), ("shadows", c(0.0, 0.1, 0.3)), ("blend", n(30.0))]);
    effect_case("ec.color.tritone", &[("highlights", c(1.0, 0.9, 0.6))]);
    effect_case("ec.color.channelmixer", &[("rr", n(80.0)), ("rg", n(30.0)), ("gb", n(-40.0)), ("bc", n(10.0)), ("br", n(50.0))]);
    effect_case("ec.color.channelmixer", &[("rr", n(40.0)), ("rg", n(40.0)), ("rb", n(20.0)), ("rc", n(-5.0)), ("monochrome", on())]);
}

#[test]
fn black_white_colorama_selective_color() {
    effect_case("ec.color.blackwhite", &[]);
    effect_case("ec.color.blackwhite", &[("reds", n(120.0)), ("blues", n(-50.0)), ("cyans", n(200.0)), ("tint", on())]);
    effect_case("ec.color.blackwhite", &[("yellows", n(300.0)), ("greens", n(-200.0)), ("magentas", n(0.0))]);
    effect_case("ec.color.colorama", &[]);
    effect_case("ec.color.colorama", &[("inputPhase/mode", e(4)), ("inputPhase/phaseShift", n(75.0)), ("outputCycle/cycles", n(2.5)), ("blend", n(40.0))]);
    effect_case("ec.color.colorama", &[("inputPhase/mode", e(8)), ("outputCycle/cycles", n(1.3))]);
    effect_case(
        "ec.color.selectivecolor",
        &[
            ("details/reds/redsCyan", n(60.0)),
            ("details/blues/bluesYellow", n(-40.0)),
            ("details/neutrals/neutralsBlack", n(20.0)),
            ("details/whites/whitesMagenta", n(30.0)),
        ],
    );
    effect_case(
        "ec.color.selectivecolor",
        &[
            ("method", e(1)),
            ("details/greens/greensMagenta", n(50.0)),
            ("details/yellows/yellowsCyan", n(-30.0)),
            ("details/blacks/blacksYellow", n(40.0)),
            ("details/magentas/magentasBlack", n(25.0)),
        ],
    );
}

#[test]
fn lumetri() {
    effect_case(
        "ec.color.lumetri",
        &[
            ("basicCorrection/whiteBalance/temperature", n(30.0)),
            ("basicCorrection/whiteBalance/tint", n(-20.0)),
            ("basicCorrection/tone/exposure", n(0.5)),
            ("basicCorrection/tone/contrast", n(25.0)),
            ("basicCorrection/tone/highlights", n(-40.0)),
            ("basicCorrection/tone/shadows", n(30.0)),
            ("basicCorrection/tone/whites", n(10.0)),
            ("basicCorrection/tone/blacks", n(-15.0)),
            ("basicCorrection/saturation", n(130.0)),
        ],
    );
    effect_case(
        "ec.color.lumetri",
        &[
            ("creative/lookIntensity", n(80.0)),
            ("creative/adjustments/fadedFilm", n(30.0)),
            ("creative/adjustments/vibrance", n(40.0)),
            ("creative/adjustments/creativeSaturation", n(70.0)),
            ("creative/adjustments/shadowTint", c(0.3, 0.5, 0.7)),
            ("creative/adjustments/highlightTint", c(0.7, 0.55, 0.4)),
            ("creative/adjustments/tintBalance", n(-30.0)),
            ("curves/curveMasterMidtones", n(30.0)),
            ("curves/curveRedShadows", n(-20.0)),
            ("curves/curveBlueHighlights", n(40.0)),
        ],
    );
    effect_case(
        "ec.color.lumetri",
        &[
            ("colorWheels/shadowsWheel", c(0.4, 0.5, 0.6)),
            ("colorWheels/highlightsWheel", c(0.6, 0.5, 0.45)),
            ("vignette/vignetteAmount", n(-2.0)),
            ("vignette/vignetteMidpoint", n(30.0)),
            ("vignette/vignetteRoundness", n(40.0)),
            ("creative/adjustments/sharpen", n(50.0)),
        ],
    );
    effect_case(
        "ec.color.lumetri",
        &[
            ("vignette/vignetteAmount", n(1.5)),
            ("vignette/vignetteRoundness", n(-60.0)),
            ("vignette/vignetteFeather", n(20.0)),
            ("creative/adjustments/sharpen", n(-40.0)),
        ],
    );
}

#[test]
fn linear_color_key() {
    effect_case("ec.key.linearcolor", &[("keyColor", c(0.6, 0.5, 0.4)), ("tolerance", n(15.0)), ("softness", n(20.0))]);
    effect_case("ec.key.linearcolor", &[("keyColor", c(0.2, 0.8, 0.3)), ("matchColors", e(1)), ("keyOperation", e(1)), ("view", e(2))]);
    effect_case("ec.key.linearcolor", &[("keyColor", c(0.7, 0.3, 0.6)), ("matchColors", e(2)), ("tolerance", n(5.0)), ("softness", n(30.0))]);
}

#[test]
fn key_light() {
    let screen = ("screenColour", c(0.3, 0.75, 0.35));
    effect_case("ec.keying.keylight", std::slice::from_ref(&screen));
    effect_case(
        "ec.keying.keylight",
        &[
            // A low gain against a pure screen keeps the raw matte away from 0 (the screen
            // removal divides by it; near 0 it amplifies the blurs' float rounding).
            ("screenColour", c(0.0, 1.0, 0.0)),
            ("screenGain", n(60.0)),
            ("screenBalance", n(30.0)),
            ("screenPreblur", n(3.0)),
            ("screenMatte/clipBlack", n(10.0)),
            ("screenMatte/clipWhite", n(80.0)),
            ("screenMatte/clipRollback", n(2.5)),
            ("screenMatte/screenSoftness", n(3.0)),
            ("screenMatte/replaceMethod", e(3)),
            ("screenMatte/replaceColour", c(0.6, 0.4, 0.3)),
            ("unpremultiplyResult", Value::Bool(false)),
        ],
    );
    effect_case(
        "ec.keying.keylight",
        &[
            ("screenColour", c(0.2, 0.4, 0.8)),
            ("lockBiasesTogether", Value::Bool(false)),
            ("alphaBias", c(0.5, 0.45, 0.6)),
            ("despillBias", c(0.55, 0.5, 0.45)),
            ("screenMatte/clipWhite", n(70.0)),
            ("screenMatte/screenDespotBlack", n(2.0)),
            ("screenMatte/screenDespotWhite", n(1.5)),
            ("screenMatte/screenShrinkGrow", n(-1.5)),
            ("screenMatte/replaceMethod", e(2)),
            ("foregroundColourCorrection/enableColourCorrection", on()),
            ("foregroundColourCorrection/saturation", n(120.0)),
            ("foregroundColourCorrection/contrast", n(15.0)),
            ("edgeColourCorrection/enableEdgeColourCorrection", on()),
            ("edgeColourCorrection/edgeSoftness", n(2.0)),
            ("edgeColourCorrection/edgeHardness", n(30.0)),
            ("edgeColourCorrection/edgeBrightness", n(-20.0)),
            ("sourceCrops/cropLeft", n(13.0)),
            ("sourceCrops/cropBottom", n(7.0)),
            ("insideMask/sourceAlpha", e(1)),
        ],
    );
    // Soft Colour without Screen Softness (after the blur, the screen-removed foreground of
    // nearly transparent pixels, divided by a raw matte near 1e-4, amplifies float rounding).
    effect_case(
        "ec.keying.keylight",
        &[
            screen.clone(),
            ("screenMatte/clipBlack", n(15.0)),
            ("screenMatte/clipRollback", n(1.5)),
            ("screenMatte/replaceColour", c(0.6, 0.4, 0.3)),
            ("insideMask/sourceAlpha", e(0)),
        ],
    );
    for view in [1, 2, 4, 7, 9] {
        effect_case("ec.keying.keylight", &[screen.clone(), ("view", e(view)), ("screenMatte/screenShrinkGrow", n(1.0)), ("screenMatte/replaceMethod", e(1))]);
    }
}
