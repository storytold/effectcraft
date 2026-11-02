//! GPU generators of part C (Lightning, Advanced Lightning, Beam, Lens Flare, Radio
//! Waves, Vegas, Stroke, Scribble, Write-on, Paint Bucket, Eyedropper Fill, CC Glue Gun, CC
//! Threads, Audio Spectrum, Audio Waveform, Basic Text, Path Text) vs the CPU effects (the
//! oracle): direct on a buffer with masks, another layer and audio at full and half resolution
//! and as adjustment, and composited at 8 and 32 bpc.

use effectcraft_keyframe::Value;
use effectcraft_time::Tick;

use crate::tests::fx_pixel2::{e, env_case, env_case_allow, env_case_at, off, on, pt};
use crate::tests::{c, n};

fn layer(id: u64) -> Value {
    Value::Layer(Some(id))
}

fn text(s: &str) -> Value {
    Value::Str(s.into())
}

#[test]
fn lightning() {
    env_case("ec.obsolete.lightning", &[]);
    env_case(
        "ec.obsolete.lightning",
        &[("branching", n(0.9)), ("rebranching", n(0.6)), ("detailLevel", n(4.0)), ("blendingMode", e(1)), ("pullForce", n(40.0))],
    );
    env_case("ec.obsolete.lightning", &[("blendingMode", e(2)), ("rerunAtEachFrame", on()), ("fixedEndpoint", on()), ("coreWidth", n(0.8))]);
    env_case("ec.generate.advancedlightning", &[]);
    env_case("ec.generate.advancedlightning", &[("lightningType", e(4)), ("forking", n(60.0)), ("decayMainCore", on()), ("expertSettings/coreDrain", n(40.0))]);
    env_case(
        "ec.generate.advancedlightning",
        &[("alphaObstacle", n(6.0)), ("compositeOnOriginal", off()), ("expertSettings/fractalType", e(2)), ("glowSettings/glowRadius", n(10.0))],
    );
    env_case("ec.generate.advancedlightning", &[("lightningType", e(7)), ("glowSettings/glowRadius", n(0.0)), ("coreSettings/coreRadius", n(1.5))]);
}

#[test]
fn closed_form() {
    env_case("ec.generate.beam", &[]);
    env_case("ec.generate.beam", &[("length", n(70.0)), ("time", n(40.0)), ("softness", n(0.0)), ("perspective3d", off()), ("compositeOnOriginal", off())]);
    env_case("ec.generate.beam", &[("startThickness", n(3.0)), ("endThickness", n(20.0)), ("insideColor", c(0.3, 0.9, 1.0))]);
    env_case("ec.generate.lensflare", &[]);
    env_case("ec.generate.lensflare", &[("lensType", e(1)), ("flareCenter", pt(50.0, 30.0)), ("blendWithOriginal", n(30.0))]);
    env_case("ec.generate.lensflare", &[("lensType", e(2)), ("flareBrightness", n(160.0))]);
    env_case("ec.generate.ccgluegun", &[]);
    env_case("ec.generate.ccgluegun", &[("density", n(12.0)), ("strokeWidth", n(40.0)), ("style", e(1)), ("strength", n(60.0))]);
    let t = |vals: &[(&str, Value)]| env_case_allow("ec.generate.ccthreads", vals, THREAD_TIES);
    t(&[]);
    t(&[("texture", n(0.0))]);
    // Generic values: no ties.
    env_case("ec.generate.ccthreads", &[("width", n(13.1)), ("height", n(17.3)), ("overlaps", n(2.0)), ("direction", n(23.0)), ("texture", n(0.0))]);
    env_case(
        "ec.generate.ccthreads",
        &[("width", n(9.3)), ("height", n(11.1)), ("coverage", n(55.3)), ("shadowing", n(90.0)), ("texture", n(40.0)), ("center", pt(33.7, 21.9))],
    );
}

/// CC Threads decides per pixel which thread band it lies in and which fibre (the position
/// across / along the thread times 8 / 64, truncated) shades it: at parameter values that put
/// pixel centres exactly on a band edge or fibre boundary (the defaults do), f32 and the CPU's
/// f64 can land on either side.
const THREAD_TIES: f64 = 0.05;

#[test]
fn radio_waves() {
    let t = Tick::from_seconds_f64(1.3);
    env_case_at("ec.generate.radiowaves", &[("waveMotion/expansion", n(30.0))], 0.0, t);
    env_case_at(
        "ec.generate.radiowaves",
        &[
            ("polygon/sides", n(5.0)),
            ("polygon/star", on()),
            ("waveMotion/spin", n(30.0)),
            ("waveStroke/profile", e(4)),
            ("waveMotion/velocity", n(20.0)),
            ("waveMotion/expansion", n(25.0)),
        ],
        0.0,
        t,
    );
    env_case_at(
        "ec.generate.radiowaves",
        &[("waveType", e(1)), ("imageContour/sourceLayer", layer(9)), ("waveStroke/fadeOutTime", n(1.0)), ("waveMotion/expansion", n(10.0))],
        0.0,
        t,
    );
    env_case_at(
        "ec.generate.radiowaves",
        &[
            ("waveType", e(2)),
            ("waveMask/mask", n(1.0)),
            ("waveMotion/reflection", on()),
            ("waveMotion/velocity", n(40.0)),
            ("waveMotion/expansion", n(15.0)),
            ("waveStroke/profile", e(5)),
        ],
        0.0,
        t,
    );
}

#[test]
fn strokes() {
    env_case("ec.generate.stroke", &[]);
    env_case("ec.generate.stroke", &[("allMasks", on()), ("spacing", n(80.0)), ("paintStyle", e(1)), ("end", n(70.0)), ("brushSize", n(6.0))]);
    env_case("ec.generate.stroke", &[("path", n(3.0)), ("paintStyle", e(2)), ("start", n(20.0)), ("brushHardness", n(10.0))]);
    env_case("ec.generate.scribble", &[]);
    env_case("ec.generate.scribble", &[("scribble", e(1)), ("fillType", e(2)), ("edgeOptions/endCap", e(2)), ("edgeOptions/join", e(1)), ("composite", e(0))]);
    env_case(
        "ec.generate.scribble",
        &[("mask", n(2.0)), ("fillType", e(1)), ("edgeOptions/join", e(2)), ("wiggleType", e(2)), ("end", n(60.0)), ("composite", e(2))],
    );
    env_case("ec.generate.scribble", &[("mask", n(3.0))]);
    env_case("ec.generate.vegas", &[]);
    env_case("ec.generate.vegas", &[("stroke", e(1)), ("path", n(3.0)), ("blendMode", e(2)), ("segments", n(12.0)), ("randomPhase", on())]);
    env_case("ec.generate.vegas", &[("inputLayer", layer(9)), ("blendMode", e(3)), ("shorterContoursHave", e(1)), ("threshold", n(30.0))]);
    env_case("ec.generate.vegas", &[("stroke", e(1)), ("path", n(1.0)), ("blendMode", e(0)), ("width", n(5.0)), ("hardness", n(0.9))]);
    env_case("ec.generate.writeon", &[]);
    env_case("ec.generate.writeon", &[("brushPosition", pt(20.0, 30.0)), ("brushSize", n(14.0)), ("paintStyle", e(1))]);
    env_case("ec.generate.writeon", &[("paintStyle", e(2)), ("brushHardness", n(20.0)), ("brushOpacity", n(60.0))]);
}

#[test]
fn fills() {
    env_case("ec.generate.paintbucket", &[]);
    env_case(
        "ec.generate.paintbucket",
        &[("fillPoint", pt(10.0, 10.0)), ("fillSelector", e(1)), ("tolerance", n(60.0)), ("stroke", e(4)), ("strokeWidth", n(4.0))],
    );
    env_case("ec.generate.paintbucket", &[("fillPoint", pt(35.0, 22.0)), ("fillSelector", e(2)), ("stroke", e(1)), ("color", c(0.2, 0.9, 0.4))]);
    env_case("ec.generate.paintbucket", &[("stroke", e(2)), ("spreadRadius", n(2.5)), ("tolerance", n(120.0)), ("blendingMode", e(5))]);
    env_case("ec.generate.paintbucket", &[("stroke", e(3)), ("tolerance", n(90.0)), ("blendingMode", e(9))]);
    env_case("ec.generate.paintbucket", &[("viewThreshold", on()), ("tolerance", n(80.0))]);
    env_case("ec.generate.paintbucket", &[("tolerance", n(100.0)), ("blendingMode", e(8)), ("opacity", n(70.0))]);
    env_case("ec.generate.eyedropperfill", &[]);
    env_case(
        "ec.generate.eyedropperfill",
        &[("samplePoint", pt(12.0, 30.0)), ("sampleRadius", n(6.0)), ("averagePixelColors", e(3)), ("blendWithOriginal", n(30.0))],
    );
    env_case("ec.generate.eyedropperfill", &[("sampleRadius", n(9.0)), ("averagePixelColors", e(1)), ("maintainOriginalAlpha", on())]);
    env_case("ec.generate.eyedropperfill", &[("sampleRadius", n(4.0)), ("averagePixelColors", e(2))]);
}

#[test]
fn audio() {
    let a = || ("audioLayer", layer(9));
    env_case("ec.generate.audiospectrum", &[a()]);
    env_case("ec.generate.audiospectrum", &[a(), ("displayOptions", e(1)), ("sideOptions", e(2)), ("compositeOnOriginal", on())]);
    env_case("ec.generate.audiospectrum", &[a(), ("displayOptions", e(2)), ("path", n(3.0)), ("softness", n(0.0))]);
    env_case("ec.generate.audiowaveform", &[a()]);
    env_case("ec.generate.audiowaveform", &[a(), ("displayOptions", e(0)), ("waveformOptions", e(2)), ("usePolarPath", on())]);
    env_case("ec.generate.audiowaveform", &[a(), ("displayOptions", e(2)), ("path", n(1.0)), ("compositeOnOriginal", on())]);
}

#[test]
fn text_effects() {
    env_case("ec.obsolete.basictext", &[]);
    env_case("ec.obsolete.basictext", &[("text", text("Hi\nthere")), ("fillAndStroke/displayOptions", e(2)), ("alignment", e(0)), ("size", n(14.0))]);
    env_case("ec.obsolete.basictext", &[("fillAndStroke/displayOptions", e(3)), ("fillAndStroke/strokeWidth", n(5.0)), ("compositeOnOriginal", on())]);
    env_case("ec.obsolete.basictext", &[("fillAndStroke/displayOptions", e(1)), ("fillAndStroke/strokeWidth", n(1.0)), ("size", n(20.0))]);
    env_case("ec.obsolete.pathtext", &[]);
    env_case(
        "ec.obsolete.pathtext",
        &[("pathOptions/shapeType", e(1)), ("advanced/visibleCharacters", n(2.5)), ("advanced/fadeTime", n(50.0)), ("character/size", n(14.0))],
    );
    env_case(
        "ec.obsolete.pathtext",
        &[
            ("pathOptions/customPath", n(3.0)),
            ("fillAndStroke/displayOptions", e(2)),
            ("advanced/jitterSettings/rotationJitterMax", n(20.0)),
            ("compositeOnOriginal", on()),
        ],
    );
}
