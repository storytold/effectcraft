//! GPU colour management vs the CPU effects (the oracle): Apply Color LUT, the OCIO effects,
//! Color Profile Converter and Lumetri's Input LUT / Look, direct on a buffer (full and half
//! resolution, as adjustment) and composited at 8 and 32 bpc.

use effectcraft_keyframe::Value;

use effectcraft_project::BitDepth;
use effectcraft_render::RenderOpts;
use effectcraft_time::Tick;

use crate::tests::{Scene, check, compare_at, effect_case, effect_direct, n, opts, set};

/// [`effect_case`] for nearest-neighbour lattice lookups: an 8 bpc input that lands exactly on a
/// rounding boundary between two lattice points may pick the other one (the GPU's division is
/// not correctly rounded), so up to `allow` of the composited pixels may differ.
fn case_allow(id: &str, vals: &[(&str, Value)], allow: f64) {
    effect_direct(id, vals, false);
    for depth in [BitDepth::Bpc8, BitDepth::Bpc32] {
        let mut s = Scene::new(depth);
        let bg = s.solid([0.15, 0.1, 0.2], 97, 61);
        s.push(bg);
        let mut l = s.footage(70, 44);
        s.effect(&mut l, id, vals);
        set(&mut l, "transform/rotation", Value::Scalar(8.0));
        s.push(l);
        check(&format!("{id} {depth:?}"), compare_at(&s, opts(), Tick::ZERO), allow);
        check(&format!("{id} {depth:?} half"), compare_at(&s, RenderOpts { scale: 0.5, ..opts() }, Tick::ZERO), allow);
    }
}

fn e(v: u32) -> Value {
    Value::Enum(v)
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}

/// A graded lattice: `f` over an `n`³ grid (red fastest).
fn lattice(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
    let mut out = String::new();
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let c = f([r, g, b].map(|i| i as f32 / (n - 1) as f32));
                out += &format!("{} {} {}\n", c[0], c[1], c[2]);
            }
        }
    }
    out
}

fn grade(c: [f32; 3]) -> [f32; 3] {
    [c[0].powf(0.8) * 1.05, c[1] * 0.9 + 0.08 * (c[0] * 5.0).sin(), c[2] * 0.7 + c[0] * 0.2 + 0.03]
}

fn cube_3d() -> String {
    format!("TITLE \"grade\"\nLUT_3D_SIZE 9\n{}", lattice(9, grade))
}

fn cube_1d_3d() -> String {
    let mut s = String::from("LUT_1D_SIZE 16\nLUT_3D_SIZE 7\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1.2 1.2 1.2\n");
    for i in 0..16 {
        let v = (i as f32 / 15.0).powf(0.6);
        s += &format!("{} {} {}\n", v, v * 0.95, v.powf(1.2));
    }
    s + &lattice(7, grade)
}

fn cube_1d() -> String {
    let mut s = String::from("LUT_1D_SIZE 32\n");
    for i in 0..32 {
        let v = i as f32 / 31.0;
        s += &format!("{} {} {}\n", v.powf(0.7), v.powf(1.3), 0.05 + v * 0.9);
    }
    s
}

fn csp() -> String {
    let mut s = String::from("CSPLUTV100\n3D\n\n3\n0 0.5 1\n0 0.3 1\n2\n0 1\n0 1\n4\n0 0.2 0.6 1\n0 0.4 0.7 1\n\n5 5 5\n");
    for b in 0..5 {
        for g in 0..5 {
            for r in 0..5 {
                let c = grade([r, g, b].map(|i| i as f32 / 4.0));
                s += &format!("{} {} {}\n", c[0], c[1], c[2]);
            }
        }
    }
    s
}

#[test]
fn apply_color_lut() {
    effect_case("ec.utility.applylut", &[("lut", s(&cube_3d()))]);
    effect_case("ec.utility.applylut", &[("lut", s(&cube_1d_3d()))]);
    effect_case("ec.utility.applylut", &[("lut", s(&cube_1d()))]);
    effect_case("ec.utility.applylut", &[("lut", s("/nonexistent/grade.cube"))]);
}

#[test]
fn ocio_file_transform() {
    for interp in 1..4 {
        effect_case("ec.color.ociofile", &[("file", s(&cube_3d())), ("interpolation", e(interp))]);
    }
    case_allow("ec.color.ociofile", &[("file", s(&cube_3d())), ("interpolation", e(0))], 0.01);
    case_allow("ec.color.ociofile", &[("file", s(&cube_1d_3d())), ("interpolation", e(0))], 0.01);
    effect_case("ec.color.ociofile", &[("file", s(&cube_1d())), ("direction", e(1))]);
    effect_case("ec.color.ociofile", &[("file", s(&cube_3d())), ("direction", e(1))]);
    effect_case("ec.color.ociofile", &[("file", s(&csp())), ("interpolation", e(1))]);
    effect_case("ec.color.ociofile", &[("file", s(&csp())), ("direction", e(1))]);
    let ccc = r#"<ColorCorrectionCollection><ColorCorrection id="a"><SOPNode><Slope>1.2 1 0.9</Slope><Offset>0.02 0 -0.01</Offset><Power>1.1 0.9 1</Power></SOPNode></ColorCorrection>
        <ColorCorrection id='b'><SOPNode><Slope>0.8 0.8 0.8</Slope></SOPNode><SatNode><Saturation>1.4</Saturation></SatNode></ColorCorrection></ColorCorrectionCollection>"#;
    effect_case("ec.color.ociofile", &[("file", s(ccc))]);
    effect_case("ec.color.ociofile", &[("file", s(ccc)), ("cccId", s("b")), ("direction", e(1))]);
}

#[test]
fn ocio_cdl_space_display_look() {
    let cdl = [("slopeRed", n(1.3)), ("offsetGreen", n(0.05)), ("powerBlue", n(1.4)), ("saturation", n(1.3))];
    effect_case("ec.color.ociocdl", &cdl);
    effect_case("ec.color.ociocdl", &[cdl.as_slice(), &[("style", e(1)), ("offsetRed", n(-0.1))]].concat());
    effect_case("ec.color.ociocdl", &[cdl.as_slice(), &[("direction", e(1))]].concat());
    effect_case("ec.color.ociocdl", &[cdl.as_slice(), &[("direction", e(1)), ("style", e(1))]].concat());
    let spaces = effectcraft_effects::find("ec.color.ociocolorspace").unwrap();
    let names = match &spaces.params.iter().find(|p| p.id == "source").unwrap().ui {
        effectcraft_project::ParamUi::Popup { options } => options.clone(),
        _ => vec![],
    };
    for (a, z) in [("sRGB", "ACEScg"), ("ACEScct", "Display P3"), ("Gamma 2.4 Rec.2020", "ACES2065-1"), ("CIE-XYZ-D65", "sRGB"), ("Raw", "sRGB")] {
        let i = |n: &str| e(names.iter().position(|x| x == n).unwrap() as u32);
        effect_case("ec.color.ociocolorspace", &[("source", i(a)), ("destination", i(z))]);
        effect_case("ec.color.ociocolorspace", &[("source", i(a)), ("destination", i(z)), ("direction", e(1))]);
    }
    for display in 0..4 {
        for view in 0..2 {
            effect_case("ec.color.ociodisplay", &[("display", e(display)), ("view", e(view))]);
        }
    }
    // (Inverting PQ and the roll-off together reaches values near 1000 whose matrix mix cancels
    // in the small channels: f32 agrees to ~1e-5 of the pixel's largest channel there.)
    effect_case("ec.color.ociodisplay", &[("display", e(3)), ("direction", e(1))]);
    effect_case("ec.color.ociodisplay", &[("display", e(2)), ("view", e(1)), ("direction", e(1))]);
    effect_case("ec.color.ociodisplay", &[("display", e(1)), ("direction", e(1))]);
    effect_case("ec.color.ociolook", &[("look", e(3))]);
    effect_case("ec.color.ociolook", &[("look", e(1)), ("looks", s("Vivid, -Cool"))]);
    effect_case("ec.color.ociolook", &[("looks", s("High Contrast")), ("direction", e(1))]);
}

#[test]
fn color_profile_converter() {
    for (inp, out, intent) in [(0, 1, 1), (1, 3, 0), (4, 5, 2), (6, 2, 3), (5, 1, 0), (3, 4, 1)] {
        effect_case("ec.utility.colorprofileconverter", &[("inputProfile", e(inp)), ("outputProfile", e(out)), ("intent", e(intent))]);
    }
    effect_case(
        "ec.utility.colorprofileconverter",
        &[("inputProfile", e(1)), ("outputProfile", e(3)), ("linearizeInputProfile", Value::Bool(true)), ("linearizeOutputProfile", Value::Bool(true))],
    );
}

#[test]
fn lumetri_luts_and_looks() {
    for look in 2..8 {
        effect_case("ec.color.lumetri", &[("creative/look", e(look))]);
    }
    effect_case("ec.color.lumetri", &[("creative/look", e(4)), ("creative/lookIntensity", n(60.0)), ("basicCorrection/tone/exposure", n(0.3))]);
    effect_case("ec.color.lumetri", &[("creative/look", e(1)), ("creative/lookFile", s(&cube_3d()))]);
    effect_case(
        "ec.color.lumetri",
        &[("basicCorrection/inputLut", e(1)), ("basicCorrection/inputLutFile", s(&cube_1d_3d())), ("basicCorrection/saturation", n(120.0))],
    );
    effect_case(
        "ec.color.lumetri",
        &[("basicCorrection/inputLut", e(1)), ("basicCorrection/inputLutFile", s(&cube_1d())), ("creative/look", e(1)), ("creative/lookFile", s(&cube_3d()))],
    );
}

/// A custom config: exponent, matrix + offset, group (matrix, log), log-affine (inverse),
/// range, CDL and data spaces, and a display.
const CONFIG: &str = r#"ocio_profile_version: 2
roles:
  scene_linear: lin
displays:
  Monitor:
    - !<View> {name: Video, colorspace: gamma22}
    - !<View> {name: Log, colorspace: log2}
colorspaces:
  - !<ColorSpace>
    name: lin
  - !<ColorSpace>
    name: gamma22
    to_scene_reference: !<ExponentTransform> {value: [2.2, 2.2, 2.2, 1]}
  - !<ColorSpace>
    name: half
    from_reference: !<MatrixTransform> {matrix: [0.5, 0.1, 0, 0, 0, 0.5, 0, 0, 0.05, 0, 0.5, 0, 0, 0, 0, 1], offset: [0.1, 0.1, 0.1, 0]}
  - !<ColorSpace>
    name: log2
    from_reference: !<GroupTransform>
      children:
        - !<MatrixTransform> {matrix: [2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 1]}
        - !<LogTransform> {base: 2}
  - !<ColorSpace>
    name: affine
    to_reference: !<LogAffineTransform> {base: 10, logSideSlope: 0.5, logSideOffset: 0.2, linSideSlope: 2, linSideOffset: 0.01, direction: inverse}
  - !<ColorSpace>
    name: range
    from_reference: !<RangeTransform> {min_in_value: 0.1, max_in_value: 0.9, min_out_value: 0, max_out_value: 1}
  - !<ColorSpace>
    name: graded
    to_reference: !<CDLTransform> {slope: [1.2, 1, 0.9], offset: [0.02, 0, -0.01], power: [1.1, 0.9, 1], sat: 1.2}
  - !<ColorSpace>
    name: raw
    isdata: true
"#;

#[test]
fn ocio_custom_config() {
    let cfg = [("config", e(1)), ("configFile", s(CONFIG))];
    for (a, z) in [
        ("gamma22", "lin"),
        ("lin", "gamma22"),
        ("gamma22", "half"),
        ("half", "lin"),
        ("lin", "log2"),
        ("log2", "graded"),
        ("affine", "lin"),
        ("lin", "affine"),
        ("range", "gamma22"),
        ("lin", "range"),
        ("graded", "lin"),
        ("raw", "half"),
        ("lin", "missing"),
    ] {
        for dir in 0..2 {
            let vals = [cfg.as_slice(), &[("sourceName", s(a)), ("destinationName", s(z)), ("direction", e(dir))]].concat();
            effect_case("ec.color.ociocolorspace", &vals);
        }
    }
    for (view, dir) in [("Video", 0), ("Log", 0), ("Log", 1)] {
        let vals = [cfg.as_slice(), &[("sourceName", s("half")), ("displayName", s("Monitor")), ("viewName", s(view)), ("direction", e(dir))]].concat();
        effect_case("ec.color.ociodisplay", &vals);
    }
    effect_case("ec.color.ociocolorspace", &[("config", e(1)), ("configFile", s("/missing.ocio"))]);
}

/// A config with the OCIO v2 transforms of Blender's (#410): an E-white XYZ reference,
/// ColorSpaceTransform, ExponentWithLinearTransform, display-referred spaces through the
/// default view transform, AllocationTransform, BuiltinTransform (a matrix, the PQ and ACEScc
/// curves), a minimum-only RangeTransform and a look in a view.
const CONFIG_V2: &str = r#"ocio_profile_version: 2
roles:
  reference: XYZ E
  scene_linear: Linear Rec.709
default_view_transform: Standard
view_transforms:
  - !<ViewTransform>
    name: Standard
    from_scene_reference: !<MatrixTransform> {matrix: [0.9531, -0.0266, 0.0239, 0, -0.0382, 1.0288, 0.0094, 0, 0.0026, -0.0030, 1.0893, 0, 0, 0, 0, 1]}
displays:
  sRGB:
    - !<View> {name: Standard, view_transform: Standard, display_colorspace: sRGB Display}
    - !<View> {name: Log, colorspace: Log, looks: +Lift}
    - !<View> {name: PQ, view_transform: Standard, display_colorspace: PQ Display}
display_colorspaces:
  - !<ColorSpace>
    name: XYZ D65
  - !<ColorSpace>
    name: sRGB Display
    from_display_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: XYZ D65, dst: Linear Rec.709}
        - !<ExponentWithLinearTransform> {gamma: 2.4, offset: 0.055, direction: inverse}
  - !<ColorSpace>
    name: PQ Display
    from_display_reference: !<BuiltinTransform> {style: DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ}
colorspaces:
  - !<ColorSpace>
    name: XYZ E
  - !<ColorSpace>
    name: Linear Rec.709
    from_scene_reference: !<GroupTransform>
      children:
        - !<MatrixTransform> {matrix: [0.9531, -0.0266, 0.0239, 0, -0.0382, 1.0288, 0.0094, 0, 0.0026, -0.0030, 1.0893, 0, 0, 0, 0, 1]}
        - !<MatrixTransform> {matrix: [3.2410, -1.5374, -0.4986, 0, -0.9692, 1.8760, 0.0416, 0, 0.0556, -0.2040, 1.0570, 0, 0, 0, 0, 1]}
  - !<ColorSpace>
    name: sRGB
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: XYZ E, dst: Linear Rec.709}
        - !<ExponentWithLinearTransform> {gamma: 2.4, offset: 0.055, direction: inverse}
  - !<ColorSpace>
    name: Log
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: XYZ E, dst: Linear Rec.709}
        - !<AllocationTransform> {allocation: lg2, vars: [-10, 6.5]}
  - !<ColorSpace>
    name: ACEScg
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: XYZ E, dst: XYZ D65}
        - !<BuiltinTransform> {style: "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD", direction: inverse}
  - !<ColorSpace>
    name: ACEScc
    to_scene_reference: !<GroupTransform>
      children:
        - !<BuiltinTransform> {style: ACEScc_to_ACES2065-1}
        - !<MatrixTransform> {matrix: [0.9525, 0, 0.0001, 0, 0.3439, 0.7281, -0.0721, 0, 0, 0, 1.0088, 0, 0, 0, 0, 1]}
  - !<ColorSpace>
    name: Floor
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {src: XYZ E, dst: sRGB}
        - !<RangeTransform> {min_in_value: 0.1, min_out_value: 0.1}
  - !<ColorSpace>
    name: Uniform
    from_scene_reference: !<AllocationTransform> {allocation: uniform, vars: [-0.5, 2]}
looks:
  - !<Look>
    name: Lift
    process_space: Log
    transform: !<CDLTransform> {offset: [0.05, 0.02, 0], sat: 0.8}
"#;

#[test]
fn ocio_custom_config_v2() {
    let cfg = [("config", e(1)), ("configFile", s(CONFIG_V2))];
    for (a, z) in [
        ("Linear Rec.709", "sRGB"),
        ("Linear Rec.709", "sRGB Display"),
        ("sRGB", "Log"),
        ("ACEScg", "sRGB"),
        ("ACEScc", "Linear Rec.709"),
        ("sRGB", "Floor"),
        ("Linear Rec.709", "Uniform"),
    ] {
        for dir in 0..2 {
            let vals = [cfg.as_slice(), &[("sourceName", s(a)), ("destinationName", s(z)), ("direction", e(dir))]].concat();
            effect_case("ec.color.ociocolorspace", &vals);
        }
    }
    for (view, dir) in [("Standard", 0), ("Standard", 1), ("Log", 0), ("Log", 1), ("PQ", 0)] {
        let vals = [cfg.as_slice(), &[("sourceName", s("Linear Rec.709")), ("displayName", s("sRGB")), ("viewName", s(view)), ("direction", e(dir))]].concat();
        effect_case("ec.color.ociodisplay", &vals);
    }
}
