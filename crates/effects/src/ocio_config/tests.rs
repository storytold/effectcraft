use super::*;

const CONFIG: &str = r#"
ocio_profile_version: 2

name: Test Config   # a comment
search_path: "luts:other"
roles:
  scene_linear: lin
  color_timing: "log2"

displays:
  sRGB:
    - !<View> {name: Standard, colorspace: gamma22}
    - !<View> {name: Raw, colorspace: raw}

colorspaces:
  - !<ColorSpace>
    name: lin
    family: ""
    description: |
      The reference.
      Linear.
    isdata: false

  - !<ColorSpace>
    name: gamma22
    aliases: [g22, "gamma 2.2"]
    to_scene_reference: !<ExponentTransform> {value: [2.2, 2.2, 2.2, 1]}

  - !<ColorSpace>
    name: half
    from_reference: !<MatrixTransform> {matrix: [0.5, 0, 0, 0,
                                                0, 0.5, 0, 0,
                                                0, 0, 0.5, 0,
                                                0, 0, 0, 1], offset: [0.1, 0.1, 0.1, 0]}

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
    name: lut
    to_reference: !<FileTransform> {src: double.cube, interpolation: linear}

  - !<ColorSpace>
    name: raw
    isdata: true

  - !<ColorSpace>
    name: weird
    to_reference: !<ExposureContrastTransform> {exposure: 1}
"#;

fn close(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < tol)
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ec-ocio-{name}-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("luts")).unwrap();
    dir
}

#[test]
fn yaml_subset() {
    let y = parse_yaml("a: 1\nb:\n  - x\n  - {k: [1, 2], s: 'q: r'}\nc: !<T> {v: 3}\nd:\n- 4\n- 5\n");
    assert_eq!(y.get("a").and_then(Yaml::str), Some("1"));
    assert_eq!(y.get("b").unwrap().seq().len(), 2);
    assert_eq!(y.get("b").unwrap().seq()[1].get("k").unwrap().nums(), vec![1.0, 2.0]);
    assert_eq!(y.get("b").unwrap().seq()[1].get("s").and_then(Yaml::str), Some("q: r"));
    assert_eq!(y.get("c").unwrap().tag(), Some("T"));
    assert_eq!(y.get("c").unwrap().get("v").and_then(Yaml::str), Some("3"));
    assert_eq!(y.get("d").unwrap().seq().len(), 2, "a sequence at the key's indent");
}

/// Hostile nesting (flow brackets and block indentation) is cut off instead of exhausting
/// the stack.
#[test]
fn yaml_nesting_is_bounded() {
    let flow = format!("a: {}\nb: 1\n", "[".repeat(200_000));
    assert!(matches!(parse_yaml(&flow), Yaml::Map(_)));
    let block: String = (0..2_000).map(|i| format!("{}k{i}:\n", " ".repeat(i))).collect();
    assert!(matches!(parse_yaml(&block), Yaml::Map(_)));
    let dashes: String = (0..2_000).map(|i| format!("{}- x{i}:\n", " ".repeat(2 * i))).collect();
    let _ = parse_yaml(&dashes);
    assert!(Config::parse(&flow, Path::new(".")).is_err());
}

#[test]
fn parses_spaces_roles_displays_and_converts() {
    let dir = temp_dir("basic");
    // A 1D LUT that doubles (0..1 → 0..2).
    std::fs::write(dir.join("luts/double.cube"), "LUT_1D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n0 0 0\n2 2 2\n").unwrap();
    let c = Config::parse(CONFIG, &dir).unwrap();
    assert_eq!(c.name, "Test Config");
    assert_eq!(c.search_path, vec!["luts", "other"]);
    assert_eq!(c.space_names(), vec!["lin", "gamma22", "half", "log2", "affine", "lut", "raw", "weird"]);
    // Roles and aliases resolve.
    assert_eq!(c.space("scene_linear").unwrap().name, "lin");
    assert_eq!(c.space("G22").unwrap().name, "gamma22");
    assert_eq!(c.space("gamma 2.2").unwrap().name, "gamma22");
    assert_eq!(c.view("sRGB", "Standard").unwrap().1.colorspace, "gamma22");
    let sp = |n: &str| c.space(n).unwrap();
    // gamma22 → lin: decode with the exponent.
    let g = c.convert(sp("gamma22"), sp("lin"), [0.5, 0.25, 1.0]);
    assert!(close(g, [0.5f64.powf(2.2), 0.25f64.powf(2.2), 1.0], 1e-9), "{g:?}");
    // lin → gamma22: the inverse (only to_reference given).
    let back = c.convert(sp("lin"), sp("gamma22"), g);
    assert!(close(back, [0.5, 0.25, 1.0], 1e-9), "{back:?}");
    // The display's view is that conversion.
    assert!(close(c.display_path(sp("lin"), "sRGB", "Standard").apply(g, false), [0.5, 0.25, 1.0], 1e-9));
    // lin → half: matrix and offset; and back through the inverse matrix.
    let h = c.convert(sp("lin"), sp("half"), [0.4, 0.6, 1.0]);
    assert!(close(h, [0.3, 0.4, 0.6], 1e-9), "{h:?}");
    assert!(close(c.convert(sp("half"), sp("lin"), h), [0.4, 0.6, 1.0], 1e-9));
    // lin → log2: a group (×2 then log2): 0.5 → 0; 2 → 2.
    let l = c.convert(sp("lin"), sp("log2"), [0.5, 2.0, 4.0]);
    assert!(close(l, [0.0, 2.0, 3.0], 1e-9), "{l:?}");
    assert!(close(c.convert(sp("log2"), sp("lin"), l), [0.5, 2.0, 4.0], 1e-9));
    // LogAffine (inverse direction as to_reference) round-trips.
    let a = c.convert(sp("lin"), sp("affine"), [0.18, 0.5, 1.0]);
    let expect = [0.18f64, 0.5, 1.0].map(|v| 0.5 * (2.0 * v + 0.01).log10() + 0.2);
    assert!(close(a, expect, 1e-9), "{a:?} vs {expect:?}");
    assert!(close(c.convert(sp("affine"), sp("lin"), a), [0.18, 0.5, 1.0], 1e-9));
    // FileTransform found on the search path.
    let f = c.convert(sp("lut"), sp("lin"), [0.25, 0.5, 0.0]);
    assert!(close(f, [0.5, 1.0, 0.0], 1e-6), "{f:?}");
    // Data spaces pass through; unsupported transforms are reported.
    assert_eq!(c.convert(sp("raw"), sp("gamma22"), [0.3, 0.3, 0.3]), [0.3, 0.3, 0.3]);
    assert_eq!(c.unsupported(), vec!["ExposureContrastTransform".to_string()]);
    assert_eq!(c.path(sp("lin"), sp("weird")).unsupported_list(), vec!["ExposureContrastTransform".to_string()]);
    assert!(c.path(sp("lin"), sp("half")).unsupported_list().is_empty());
    // Loading from a file path (cached), and errors.
    std::fs::write(dir.join("config.ocio"), CONFIG).unwrap();
    let loaded = load_config(&dir.join("config.ocio").to_string_lossy()).unwrap();
    assert_eq!(loaded.spaces.len(), 8);
    assert!(load_config("/no/such/config.ocio").is_err());
    assert!(Config::parse("just: text\n", &dir).is_err());
}

/// The OCIO Color Space and Display Transform effects with Configuration ▸ Custom.
#[test]
fn effects_use_a_custom_config() {
    use effectcraft_keyframe::Value;
    let cfg = "ocio_profile_version: 1\nroles:\n  default: lin\ndisplays:\n  Monitor:\n    - !<View> {name: Video, colorspace: g2}\ncolorspaces:\n  - !<ColorSpace>\n    name: lin\n  - !<ColorSpace>\n    name: g2\n    to_reference: !<ExponentTransform> {value: [2, 2, 2, 1]}\n";
    let img = crate::Image::filled(2, 2, [0.5, 0.25, 1.0, 1.0]);
    let fx = |id: &str, extra: &[(&str, Value)]| {
        let mut v = vec![("config", Value::Enum(1)), ("configFile", Value::Str(cfg.into()))];
        v.extend(extra.iter().cloned());
        crate::run_fx(id, &v, img.clone(), 0.0, crate::EffectEnv::default()).img.data[0]
    };
    let o = fx("ec.color.ociocolorspace", &[("sourceName", Value::Str("g2".into())), ("destinationName", Value::Str("default".into()))]);
    assert!((o[0] - 0.25).abs() < 1e-6 && (o[1] - 0.0625).abs() < 1e-6 && (o[2] - 1.0).abs() < 1e-6, "{o:?}");
    // Inverse direction.
    let o =
        fx("ec.color.ociocolorspace", &[("sourceName", Value::Str("g2".into())), ("destinationName", Value::Str("lin".into())), ("direction", Value::Enum(1))]);
    assert!((o[0] - 0.5f32.sqrt()).abs() < 1e-6, "{o:?}");
    // Display ▸ view → its colour space.
    let o = fx(
        "ec.color.ociodisplay",
        &[("sourceName", Value::Str("lin".into())), ("displayName", Value::Str("Monitor".into())), ("viewName", Value::Str("Video".into()))],
    );
    assert!((o[0] - 0.5f32.sqrt()).abs() < 1e-6, "{o:?}");
    // An unreadable config leaves pixels alone.
    let o = crate::run_fx(
        "ec.color.ociocolorspace",
        &[("config", Value::Enum(1)), ("configFile", Value::Str("/missing.ocio".into()))],
        img.clone(),
        0.0,
        crate::EffectEnv::default(),
    );
    assert_eq!(o.img.data, img.data);
}

/// A config laid out like Blender 4.x / 5.x's (written for this test): the reference space is
/// CIE XYZ with the E white, the D65 spaces adapt through a `.spimtx` matrix, sRGB is a
/// ColorSpaceTransform plus an ExponentWithLinearTransform, the display-referred spaces meet
/// the scene through the default view transform, and a log space uses an
/// AllocationTransform.
fn blender_like(dir: &Path) -> Config {
    use effectcraft_color::space::{invert, rgb_to_xyz};
    let e_to_d65 = crate::ocio::bradford([1.0 / 3.0, 1.0 / 3.0], D65);
    let rows = |m: [[f64; 3]; 3], sep: &str, end: &str| m.iter().map(|r| format!("{}{sep}{}{sep}{}{end}", r[0], r[1], r[2])).collect::<Vec<_>>();
    std::fs::write(dir.join("luts/e_to_d65.spimtx"), rows(e_to_d65, " ", " 0\n").concat()).unwrap();
    let xyz_to_709 = rows(invert(&rgb_to_xyz(REC709, D65)), ", ", ", 0").join(", ");
    let text = format!(
        r#"ocio_profile_version: 2.4
search_path: luts
roles:
  reference: Linear CIE-XYZ E
  scene_linear: Linear Rec.709
  cie_xyz_d65_interchange: XYZ D65 Display
default_view_transform: Standard
view_transforms:
  - !<ViewTransform>
    name: Standard
    from_scene_reference: !<FileTransform> {{src: e_to_d65.spimtx, interpolation: linear}}
displays:
  sRGB:
    - !<View> {{name: Standard, colorspace: sRGB}}
    - !<View> {{name: Display, view_transform: Standard, display_colorspace: sRGB Display}}
    - !<View> {{name: Log, colorspace: Log Encoding}}
    - !<View> {{name: Log Brighter, colorspace: Log Encoding, looks: +Brighter}}
    - !<View> {{name: Log Darker, colorspace: Log Encoding, looks: "-Brighter, Missing"}}
    - !<View> {{name: Raw, colorspace: Non-Color}}
display_colorspaces:
  - !<ColorSpace>
    name: XYZ D65 Display
    isdata: false
  - !<ColorSpace>
    name: sRGB Display
    from_display_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: cie_xyz_d65_interchange, dst: Linear Rec.709}}
        - !<ExponentWithLinearTransform> {{gamma: 2.4, offset: 0.055, direction: inverse}}
colorspaces:
  - !<ColorSpace>
    name: Linear CIE-XYZ E
    aliases: [Linear CIE-XYZ I-E]
  - !<ColorSpace>
    name: Linear CIE-XYZ D65
    from_scene_reference: !<FileTransform> {{src: e_to_d65.spimtx, interpolation: linear}}
  - !<ColorSpace>
    name: Linear Rec.709
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ I-E, dst: Linear CIE-XYZ D65}}
        - !<MatrixTransform> {{matrix: [{xyz_to_709}, 0, 0, 0, 1]}}
  - !<ColorSpace>
    name: sRGB
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Linear Rec.709}}
        - !<ExponentWithLinearTransform> {{gamma: 2.4, offset: 0.055, direction: inverse}}
  - !<ColorSpace>
    name: Log Encoding
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Linear Rec.709}}
        - !<AllocationTransform> {{allocation: lg2, vars: [-12, 12]}}
  - !<ColorSpace>
    name: ACEScg
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Linear CIE-XYZ D65}}
        - !<BuiltinTransform> {{style: "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD", direction: inverse}}
  - !<ColorSpace>
    name: Clipped
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Linear Rec.709}}
        - !<RangeTransform> {{min_in_value: 0, min_out_value: 0}}
  - !<ColorSpace>
    name: Graded
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Linear Rec.709}}
        - !<GradingToneTransform>
         shadows: {{rgb: [0.2, 0.2, 0.2], master: 0.35, start: 0.4, pivot: 0.1}}
  - !<ColorSpace>
    name: Loop
    from_scene_reference: !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Loop}}
  - !<ColorSpace>
    name: Fan
    from_scene_reference: !<GroupTransform>
      children:
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Fan}}
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Fan}}
        - !<ColorSpaceTransform> {{src: Linear CIE-XYZ E, dst: Fan}}
  - !<ColorSpace>
    name: Non-Color
    isdata: true
looks:
  - !<Look>
    name: Brighter
    process_space: Log Encoding
    transform: !<CDLTransform> {{offset: [0.05, 0.05, 0.05]}}
"#
    );
    Config::parse(&text, dir).unwrap()
}

/// The sRGB encoding (IEC 61966-2-1).
fn srgb(v: f64) -> f64 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// #410: with Blender's config, Linear Rec.709 → sRGB got no sRGB curve and a blue cast:
/// ColorSpaceTransform and ExponentWithLinearTransform passed colours through, so the
/// colours stayed in the E-white XYZ reference. Every route to sRGB now encodes greys to
/// neutral sRGB values (0.18 → 0.4614).
#[test]
fn blender_like_config_converts_linear_rec709_to_srgb() {
    let dir = temp_dir("blender");
    let c = blender_like(&dir);
    assert!(c.unsupported().iter().all(|t| t == "GradingToneTransform"), "{:?}", c.unsupported());
    let sp = |n: &str| c.space(n).unwrap();
    let lin = sp("scene_linear");
    let routes = [
        ("Color Space Transform", c.path(lin, sp("sRGB"))),
        ("display-referred sRGB", c.path(lin, sp("sRGB Display"))),
        ("v1 view", c.display_path(lin, "sRGB", "Standard")),
        ("v2 view", c.display_path(lin, "sRGB", "Display")),
    ];
    for (route, x) in &routes {
        assert!(x.unsupported_list().is_empty(), "{route}: {:?}", x.unsupported_list());
        for v in [0.0, 0.001, 0.18, 0.5, 1.0, 4.0] {
            let o = x.apply([v; 3], false);
            assert!(close(o, [srgb(v); 3], 2e-5), "{route}: {v} → {o:?}, want {}", srgb(v));
        }
        let red = x.apply([1.0, 0.0, 0.0], false);
        assert!(close(red, [1.0, 0.0, 0.0], 1e-4), "{route}: red {red:?}");
        // And back.
        assert!(close(x.apply([srgb(0.18); 3], true), [0.18; 3], 1e-6), "{route}");
    }
    // AllocationTransform lg2 and a look in its process space; data passes through.
    let log = |v: f64| (v.log2() + 12.0) / 24.0;
    let o = c.display_path(lin, "sRGB", "Log").apply([0.18; 3], false);
    assert!(close(o, [log(0.18); 3], 1e-6), "{o:?}");
    let o = c.display_path(lin, "sRGB", "Log Brighter").apply([0.18; 3], false);
    assert!(close(o, [log(0.18) + 0.05; 3], 1e-6), "{o:?}");
    let darker = c.display_path(lin, "sRGB", "Log Darker");
    assert!(close(darker.apply([0.18; 3], false), [log(0.18) - 0.05; 3], 1e-6));
    assert_eq!(darker.unsupported_list(), vec!["Look `Missing` (not in the config)".to_string()]);
    assert_eq!(c.display_path(lin, "sRGB", "Raw").apply([0.3, 2.0, -1.0], false), [0.3, 2.0, -1.0]);
    // BuiltinTransform: the built-in config's Linear Rec.709 → ACEScg.
    let want = crate::ocio::Xform::new(&sp_builtin("Linear Rec.709 (sRGB)"), &sp_builtin("ACEScg")).apply([1.0, 0.25, 0.0]);
    let got = c.convert(lin, sp("ACEScg"), [1.0, 0.25, 0.0]);
    assert!(close(got, want.map(f64::from), 1e-4), "{got:?} vs {want:?}");
    // RangeTransform with only a minimum clamps below and shifts nothing.
    let o = c.convert(lin, sp("Clipped"), [-0.5, 0.5, 2.0]);
    assert!(close(o, [0.0, 0.5, 2.0], 1e-6), "{o:?}");
    // Unsupported steps are listed (and pass through); loops stop.
    assert_eq!(c.path(lin, sp("Graded")).unsupported_list(), vec!["GradingToneTransform".to_string()]);
    assert!(c.path(lin, sp("Loop")).unsupported_list()[0].contains("nested"));
    let fan = c.path(lin, sp("Fan"));
    assert!(!fan.unsupported_list().is_empty());
    assert!(matches!(&fan, Xf::Group(v) if v.len() <= MAX_OPS + 1));
}

fn sp_builtin(name: &str) -> Space {
    *crate::ocio::SPACES.iter().find(|s| s.name == name).unwrap()
}

/// ExponentWithLinearTransform is the sRGB curve at gamma 2.4 / offset 0.055, continuous at
/// its break point and inverted exactly; mirror mirrors negatives.
#[test]
fn moncurve_matches_srgb_and_inverts() {
    for v in [-0.2, 0.0, 0.002, 0.039, 0.0393, 0.2, 0.5, 1.0, 3.0] {
        let lin = moncurve(v, 2.4, 0.055, false, false);
        assert!((moncurve(lin, 2.4, 0.055, true, false) - v).abs() < 1e-12, "{v}");
        if v >= 0.0 {
            assert!((moncurve(srgb(v), 2.4, 0.055, false, false) - v).abs() < 2e-5, "{v}");
        }
    }
    let xb = 0.055 / 1.4;
    assert!((moncurve(xb - 1e-12, 2.4, 0.055, false, false) - moncurve(xb + 1e-12, 2.4, 0.055, false, false)).abs() < 1e-9);
    assert_eq!(moncurve(-0.5, 2.4, 0.055, false, true), -moncurve(0.5, 2.4, 0.055, false, false));
    assert_eq!(moncurve(-0.5, 1.0, 0.0, false, false), 0.0);
}

#[test]
fn builtin_styles() {
    for style in ["IDENTITY", "DISPLAY - CIE-XYZ-D65_to_sRGB", "display - cie-xyz-d65_to_rec.2100-pq", "ACEScc_to_ACES2065-1", "ACEScct_to_ACES2065-1"] {
        assert!(builtin(style).is_some(), "{style}");
    }
    assert!(builtin("ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0").is_none());
    // D65 white in XYZ is display white.
    let xyz = rgb_white_xyz();
    for style in ["DISPLAY - CIE-XYZ-D65_to_sRGB", "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709", "DISPLAY - CIE-XYZ-D65_to_DisplayP3"] {
        let o = builtin(style).unwrap().apply(xyz, false);
        assert!(close(o, [1.0; 3], 1e-6), "{style}: {o:?}");
    }
    // PQ: 1.0 = 100 nits → code value ≈ 0.508.
    let pq = builtin("DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ").unwrap().apply(xyz, false);
    assert!((pq[0] - 0.508).abs() < 1e-3, "{pq:?}");
    // ACEScc and ACEScct of 0.18 (ACES2065-1 grey stays grey).
    for (style, code) in [("ACEScc_to_ACES2065-1", 0.413_588_402_492_442_1), ("ACEScct_to_ACES2065-1", 0.413_588_402_492_442_1)] {
        let o = builtin(style).unwrap().apply([code; 3], false);
        assert!(close(o, [0.18; 3], 1e-6), "{style}: {o:?}");
    }
}

fn rgb_white_xyz() -> [f64; 3] {
    [D65[0] / D65[1], 1.0, (1.0 - D65[0] - D65[1]) / D65[1]]
}
