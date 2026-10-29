use super::*;
use crate::{EffectEnv, run_fx};
use effectcraft_raster::Image;

fn px(id: &str, vals: &[(&str, Value)], c: [f32; 4]) -> [f32; 4] {
    run_fx(id, vals, Image::filled(2, 2, c), 0.0, EffectEnv::default()).img.get(0, 0)
}
fn close(a: [f32; 4], b: [f32; 4], eps: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
}

/// Projects saved before Use Legacy / the Photo Filter menu existed keep rendering as before.
#[test]
fn old_saves_get_legacy_values_for_added_params() {
    use effectcraft_project::build::Ids;
    let mut next = 1;
    for (id, param, want) in
        [("ec.color.brightnesscontrast", "useLegacy", Value::Bool(true)), ("ec.color.photofilter", "filter", Value::Enum(PHOTO_FILTER_CUSTOM))]
    {
        let spec = crate::find(id).unwrap();
        let mut g = crate::instantiate(spec, &mut Ids(&mut next), spec.name, [10.0, 10.0]);
        g.children.retain(|c| c.match_id() != param);
        crate::migrate::upgrade_instance(spec, &mut g, &mut Ids(&mut next), [10.0, 10.0]);
        assert_eq!(g.get(param).unwrap().value, want, "{id}");
    }
}

/// After Effects 2026 renders of full-frame solids with Tint at its defaults (sRGB working
/// space, not linearised, 32 bpc, 8-bit capture): Rec. 601 weights on the encoded values.
#[test]
fn tint_defaults_match_after_effects_greys() {
    for (c, want) in [([1.0, 0.0, 0.0], 76.0), ([0.0, 1.0, 0.0], 150.0), ([0.0, 0.0, 1.0], 29.0), ([0.5, 0.5, 0.5], 128.0)] {
        let o = px("ec.color.tint", &[], [c[0], c[1], c[2], 1.0]);
        let want = want / 255.0;
        assert!(close(o, [want, want, want, 1.0], 1.0 / 255.0), "{c:?}: {o:?} vs {want}");
    }
}

#[test]
fn colorama_reads_get_phase_from() {
    // Same intensity, different red: Intensity gives one colour, Red gives two.
    let a = [0.6, 0.2, 0.2, 1.0];
    let b = [0.2, 0.2, 0.6, 1.0];
    let red = |c| px("ec.color.colorama", &[("inputPhase/mode", Value::Enum(1))], c);
    assert!(!close(red(a), red(b), 1e-3));
    // Zero maps everything to the start of the cycle (red).
    assert!(close(px("ec.color.colorama", &[("inputPhase/mode", Value::Enum(9))], b), [1.0, 0.0, 0.0, 1.0], 1e-5));
    // Interpolate Palette off posterizes: two nearby inputs land on the same colour.
    let off = |v: f32| px("ec.color.colorama", &[("outputCycle/interpolatePalette", Value::Bool(false))], [v, v, v, 1.0]);
    assert_eq!(off(0.20), off(0.22));
}

#[test]
fn brightness_contrast_modern_keeps_black_and_white_and_legacy_is_linear() {
    let bc = |vals: &[(&str, Value)], v: f32| px("ec.color.brightnesscontrast", vals, [v, v, v, 1.0])[0];
    let up = [("brightness", num(50.0)), ("contrast", num(40.0))];
    assert_eq!(bc(&up, 0.0), 0.0);
    assert!((bc(&up, 1.0) - 1.0).abs() < 1e-6);
    assert!(bc(&up, 0.4) > 0.4);
    let legacy = [("brightness", num(50.0)), ("useLegacy", Value::Bool(true))];
    assert!((bc(&legacy, 0.2) - 0.7).abs() < 1e-5);
    assert!((bc(&legacy, 0.0) - 0.5).abs() < 1e-5);
}

/// After Effects non-legacy ramps (kasper9n, #681). B0 C-100 matched B0 C-50 and is omitted.
#[test]
fn brightness_contrast_modern_matches_ae_ramp_samples() {
    let u8 = |v: f32| (v * 255.0).round() as i32;
    let go = |b: f64, c: f64, x: u8| {
        u8(px("ec.color.brightnesscontrast", &[("brightness", num(b)), ("contrast", num(c))], [x as f32 / 255.0, x as f32 / 255.0, x as f32 / 255.0, 1.0])[0])
    };
    // (brightness, contrast, input, AE 8-bit)
    let samples: &[(f64, f64, u8, u8)] = &[
        (0.0, 100.0, 64, 40),
        (0.0, 100.0, 128, 129),
        (0.0, 100.0, 192, 217),
        (0.0, 50.0, 64, 53),
        (0.0, 50.0, 192, 205),
        (0.0, -50.0, 64, 77),
        (100.0, 0.0, 128, 209),
        (50.0, 0.0, 128, 171),
        (106.0, -18.0, 128, 214),
    ];
    for &(b, c, x, ae) in samples {
        let got = go(b, c, x);
        assert!((got - ae as i32).abs() <= 10, "B{b} C{c} in {x}: got {got} AE {ae}");
    }
    // Old g=4 S-curve put Contrast 100 at ~8/255 here.
    assert!(go(0.0, 100.0, 64) >= 30);
}

/// After Effects 2026: Tritone Highlights [1, 1, 0.8], Midtones [0.6, 0.2, 0.2], Shadows
/// [0, 0, 0.3]. On a red solid with Blend With Original 25 (case 718) AE gives (132, 23, 46);
/// on a grey ramp (case 815) levels 0, 63.9, 127.6, 191.4 give (0, 0, 77), (77, 26, 64),
/// (153, 51, 51), (204, 153, 128).
#[test]
fn tritone_matches_after_effects() {
    let colours =
        [("highlights", Value::Color([1.0, 1.0, 0.8, 1.0])), ("midtones", Value::Color([0.6, 0.2, 0.2, 1.0])), ("shadows", Value::Color([0.0, 0.0, 0.3, 1.0]))];
    let near = |o: [f32; 4], ae: [f32; 3]| (0..3).all(|i| (o[i] * 255.0 - ae[i]).abs() <= 1.0);
    let mut red = colours.to_vec();
    red.push(("blend", num(25.0)));
    let o = px("ec.color.tritone", &red, [1.0, 0.0, 0.0, 1.0]);
    assert!(near(o, [132.0, 23.0, 46.0]), "{:?}", o.map(|v| v * 255.0));
    for (level, ae) in [(0.1f32, [0.0, 0.0, 77.0]), (63.9, [77.0, 26.0, 64.0]), (127.6, [153.0, 51.0, 51.0]), (191.4, [204.0, 153.0, 128.0])] {
        let v = level / 255.0;
        let o = px("ec.color.tritone", &colours, [v, v, v, 1.0]);
        assert!(near(o, ae), "level {level}: {:?} vs {ae:?}", o.map(|v| v * 255.0));
    }
}

/// After Effects 2026, Tint Color [1, 0.5, 0]: a red solid at Reds 10 (case 717) and yellow
/// and white solids at the default sliders (case 825).
#[test]
fn black_white_tint_matches_after_effects() {
    let tint = [("tint", Value::Bool(true)), ("tintColor", Value::Color([1.0, 0.5, 0.0, 1.0]))];
    let near = |o: [f32; 4], ae: [f32; 3]| (0..3).all(|i| (o[i] * 255.0 - ae[i]).abs() <= 1.5);
    let mut reds = tint.to_vec();
    reds.push(("reds", num(10.0)));
    let o = px("ec.color.blackwhite", &reds, [1.0, 0.0, 0.0, 1.0]);
    assert!(near(o, [43.0, 21.0, 0.0]), "{:?}", o.map(|v| v * 255.0));
    let o = px("ec.color.blackwhite", &tint, [1.0, 1.0, 0.0, 1.0]);
    assert!(near(o, [255.0, 129.0, 2.0]), "{:?}", o.map(|v| v * 255.0));
    let o = px("ec.color.blackwhite", &tint, [1.0, 1.0, 1.0, 1.0]);
    assert!(near(o, [255.0, 255.0, 255.0]), "{:?}", o.map(|v| v * 255.0));
    // Hostile grays stay finite.
    for g in [f32::NAN, -1.0, 0.0, 4.0] {
        assert!(bw_tint([1.0, 0.5, 0.0], g).iter().all(|v| v.is_finite()));
    }
}

/// After Effects 2026 ramps (RENDERER round 2, session R8): negative Brightness and Contrast
/// are the inverses of the positive curves. (brightness, contrast, input, AE 8-bit)
#[test]
fn negative_brightness_and_contrast_invert_the_positive_curves() {
    let go = |b: f64, c: f64, x: u8| {
        let v = x as f32 / 255.0;
        (px("ec.color.brightnesscontrast", &[("brightness", num(b)), ("contrast", num(c))], [v, v, v, 1.0])[0] * 255.0).round() as i32
    };
    let samples: &[(f64, f64, u8, u8)] = &[
        (-40.0, 0.0, 64, 50),
        (-40.0, 0.0, 128, 99),
        (-40.0, 0.0, 191, 155),
        (-40.0, 0.0, 234, 207),
        (0.0, -50.0, 21, 28),
        (0.0, -50.0, 64, 76),
        (0.0, -50.0, 191, 179),
        (0.0, -50.0, 234, 227),
        (40.0, 0.0, 128, 162),
        (0.0, 50.0, 64, 52),
    ];
    for &(b, c, x, ae) in samples {
        let got = go(b, c, x);
        assert!((got - ae as i32).abs() <= 7, "B{b} C{c} in {x}: got {got} AE {ae}");
    }
}

/// #1463: Black & White's sliders set the gray of each pure hue and blend between
/// neighbouring hues; neutral pixels keep their value, overbright ones too, and an overbright
/// colour stays finite (it used to divide by zero).
#[test]
fn black_white_sliders_set_the_gray_of_each_hue() {
    let bw = |vals: &[(&str, Value)], c: [f32; 3]| px("ec.color.blackwhite", vals, [c[0], c[1], c[2], 1.0]);
    let gray = |v: f32| [v, v, v, 1.0];
    // The defaults: Reds 40, Yellows 60, Greens 40, Cyans 60, Blues 20, Magentas 80 (%).
    let hues = [
        ([1.0, 0.0, 0.0], 0.4),
        ([1.0, 1.0, 0.0], 0.6),
        ([0.0, 1.0, 0.0], 0.4),
        ([0.0, 1.0, 1.0], 0.6),
        ([0.0, 0.0, 1.0], 0.2),
        ([1.0, 0.0, 1.0], 0.8),
        // Half as bright, half the gray; orange is halfway from Reds to Yellows.
        ([0.5, 0.0, 0.0], 0.2),
        ([1.0, 0.5, 0.0], 0.5),
        // Neutral pixels keep their value.
        ([0.3, 0.3, 0.3], 0.3),
        ([4.0, 4.0, 4.0], 4.0),
        // An overbright red: its smallest channel plus its chroma at Reds' 40 %.
        ([1.5, 0.5, 0.5], 0.9),
    ];
    for (c, want) in hues {
        let o = bw(&[], c);
        assert!(close(o, gray(want), 1e-5), "{c:?}: {o:?}, want {want}");
    }
    assert!(close(bw(&[("reds", num(120.0))], [1.0, 0.0, 0.0]), gray(1.2), 1e-5));
    assert!(close(bw(&[("reds", num(-200.0))], [1.0, 0.0, 0.0]), gray(0.0), 1e-5));
    assert!(close(bw(&[("blues", num(300.0))], [0.1, 0.1, 0.3]), gray(0.7), 1e-5));
}

/// #1463: faint colour noise stays faint. The shadows of compressed footage carry a code or
/// two of colour noise, which Black & White turned into bright and black blocks (the HSL
/// saturation it scaled is near 1 for any tinted near-black pixel). At 8, 16 and 32 bpc the
/// gray of a dark ramp with ±2 steps of colour noise stays within 2 steps of the clean ramp's.
#[test]
fn black_white_keeps_faint_colour_noise_faint() {
    // A step of 8 bpc, of 16 bpc (After Effects' 0..32768), and a small 32-bpc float one.
    for step in [1.0 / 255.0, 1.0 / 32768.0, 1e-3_f32] {
        let (w, h) = (64u32, 4u32);
        let (mut clean, mut noisy) = (Image::new(w, h), Image::new(w, h));
        for y in 0..h {
            for x in 0..w {
                // A dark ramp up to a quarter gray, in whole steps...
                let v = (x as f32 / w as f32 * 0.25 / step).round() * step;
                // ...and up to ±2 steps of noise per channel, mixed differently in each pixel.
                let n = |k: u32| (((x * 7 + y * 13 + k * 2) % 5) as f32 - 2.0) * step;
                clean.set(x, y, [v, v, v, 1.0]);
                noisy.set(x, y, [(v + n(0)).max(0.0), (v + n(1)).max(0.0), (v + n(2)).max(0.0), 1.0]);
            }
        }
        let bw = |img| run_fx("ec.color.blackwhite", &[], img, 0.0, EffectEnv::default()).img;
        let (a, b) = (bw(clean), bw(noisy));
        let worst = a.data.iter().zip(&b.data).map(|(p, q)| (p[0] - q[0]).abs()).fold(0.0, f32::max);
        assert!(worst <= 2.0 * step + 1e-6, "step {step}: the noise moved the gray by {} steps", worst / step);
    }
}

#[test]
fn levels_clips_black_and_white_independently() {
    let lv = |vals: &[(&str, Value)], v: f32| px("ec.color.levels", vals, [v, v, v, 1.0])[0];
    let base = [("inBlack", num(0.2)), ("inWhite", num(0.6)), ("outBlack", num(0.1)), ("outWhite", num(0.9))];
    assert!((lv(&base, 0.9) - 0.9).abs() < 1e-5);
    let mut no_white = base.to_vec();
    no_white.push(("clipToOutputWhite", Value::Enum(0)));
    assert!(lv(&no_white, 0.9) > 0.9);
    assert!((lv(&no_white, 0.0) - 0.1).abs() < 1e-5);
    // The hidden switch of older projects turns both off.
    let mut legacy = base.to_vec();
    legacy.push(("noClip", Value::Bool(true)));
    assert!(lv(&legacy, 0.9) > 0.9);
}

#[test]
fn exposure_individual_channels_and_bypass() {
    let g = [0.5, 0.5, 0.5, 1.0];
    let one = px("ec.color.exposure", &[("channels", Value::Enum(1)), ("red/redExposure", num(1.0))], g);
    assert!(one[0] > 0.6 && (one[1] - 0.5).abs() < 1e-5 && (one[2] - 0.5).abs() < 1e-5);
    // Master values are ignored in Individual Channels mode.
    assert!(close(px("ec.color.exposure", &[("channels", Value::Enum(1)), ("master/exposure", num(2.0))], g), g, 1e-5));
    // Bypass Linear Light Conversion: +1 stop doubles the raw value.
    let raw = px("ec.color.exposure", &[("master/exposure", num(1.0)), ("bypassLinearLight", Value::Bool(true))], [0.25, 0.25, 0.25, 1.0]);
    assert!((raw[0] - 0.5).abs() < 1e-5);
    // Negative results are mirrored, not clipped.
    let neg =
        px("ec.color.exposure", &[("master/offset", num(-0.5)), ("bypassLinearLight", Value::Bool(true)), ("master/gamma", num(2.0))], [0.25, 0.25, 0.25, 1.0]);
    assert!((neg[0] + 0.5).abs() < 1e-4, "{neg:?}");
}

/// After Effects' Exposure ranges for all twelve sliders; a saved value outside them (a gamma
/// of 0.05, valid before, as in Photoshop) survives loading and still renders.
#[test]
fn exposure_has_after_effects_ranges_and_keeps_old_values() {
    use effectcraft_project::build::Ids;
    let spec = crate::find("ec.color.exposure").unwrap();
    let want = [(-100.0, 100.0, -4.0, 4.0, 2), (-2.0, 2.0, -0.5, 0.5, 4), (0.1, 10.0, 0.1, 10.0, 2)];
    let sliders: Vec<_> = spec
        .params
        .iter()
        .filter_map(|p| match p.ui {
            ParamUi::Slider { min, max, slider_min, slider_max, decimals } => Some((p.id, (min, max, slider_min, slider_max, decimals))),
            _ => None,
        })
        .collect();
    assert_eq!(sliders.len(), 12);
    for (i, (id, r)) in sliders.iter().enumerate() {
        assert_eq!(*r, want[i % 3], "{id}");
    }
    let mut next = 1;
    let mut g = crate::instantiate(spec, &mut Ids(&mut next), spec.name, [10.0, 10.0]);
    let gamma = g.prop_mut("master/gamma").unwrap();
    gamma.value = num(0.05);
    gamma.ui = slider(0.01, 9.99, 0.1, 3.0, 2);
    crate::migrate::upgrade_instance(spec, &mut g, &mut Ids(&mut next), [10.0, 10.0]);
    let gamma = g.prop_mut("master/gamma").unwrap();
    assert_eq!(gamma.value, num(0.05));
    assert!(matches!(gamma.ui, ParamUi::Slider { min, .. } if min == 0.1));
    let o = px("ec.color.exposure", &[("master/gamma", num(0.05)), ("bypassLinearLight", Value::Bool(true))], [0.5, 0.5, 0.5, 1.0]);
    assert!((o[0] - 0.5f32.powf(20.0)).abs() < 1e-9, "{o:?}");
}

/// Exposure on a red [1, 0, 0] solid against After Effects 2026 renders (sRGB working space,
/// not linearised, 32 bpc, 8-bit captures): the linear-light conversion is a pure 2.2 power.
/// The piecewise sRGB curve gave 2 / 7 / 39 for the three smallest Offsets.
#[test]
fn exposure_matches_after_effects_gamma_2_2() {
    let red = [1.0, 0.0, 0.0, 1.0];
    let code = |o: [f32; 4], i: usize| o[i] * 255.0;
    // Master Offset only: G and B carry the curve; R goes over 1 (255 in the 8-bit capture).
    for (offset, ae) in [(0.0005, 8.0), (0.002, 15.0), (0.02, 43.0), (0.1, 90.0)] {
        let o = px("ec.color.exposure", &[("master/offset", num(offset))], red);
        for i in [1, 2] {
            assert!((code(o, i) - ae).abs() <= 1.0, "offset {offset}: {} vs AE {ae}", code(o, i));
        }
        assert!(o[0] >= 1.0, "{o:?}");
    }
    // Exposure −2.25: AE 126; with Bypass Linear Light Conversion (gain on encoded values) 54.
    let o = px("ec.color.exposure", &[("master/exposure", num(-2.25))], red);
    assert!((code(o, 0) - 126.0).abs() <= 1.0 && o[1] == 0.0, "{o:?}");
    let o = px("ec.color.exposure", &[("master/exposure", num(-2.25)), ("bypassLinearLight", Value::Bool(true))], red);
    assert!((code(o, 0) - 54.0).abs() <= 1.0 && o[1] == 0.0, "{o:?}");
    // Default settings leave 32-bpc values alone, negative and over-range ones included.
    let hdr = [-0.25, 0.5, 2.0, 1.0];
    assert!(close(px("ec.color.exposure", &[], hdr), hdr, 1e-5));
    // A negative Offset pushes black below zero, mirrored, rather than clipping or NaN.
    let o = px("ec.color.exposure", &[("master/offset", num(-0.02))], [0.0, 0.0, 0.0, 1.0]);
    assert!((o[0] + 0.02f32.powf(1.0 / 2.2)).abs() < 1e-5, "{o:?}");
}

#[test]
fn change_to_color_change_modes_tolerances_and_matte() {
    let red = [0.8, 0.1, 0.1, 1.0];
    let green = [0.1, 0.8, 0.1, 1.0];
    let base = [("from", col(0.8, 0.1, 0.1)), ("to", col(0.1, 0.1, 0.8))];
    // Hue only: red turns blue with its own lightness / saturation; green is untouched.
    let o = px("ec.color.changetocolor", &base, red);
    assert!(o[2] > 0.7 && o[0] < 0.2, "{o:?}");
    assert!(close(px("ec.color.changetocolor", &base, green), green, 1e-5));
    // The matte is white where matched, black elsewhere.
    let mut m = base.to_vec();
    m.push(("viewCorrectionMatte", Value::Bool(true)));
    assert!(close(px("ec.color.changetocolor", &m, red), [1.0, 1.0, 1.0, 1.0], 1e-5));
    assert!(close(px("ec.color.changetocolor", &m, green), [0.0, 0.0, 0.0, 1.0], 1e-5));
    // A lightness tolerance of 0 rejects a lighter red.
    let mut strict = m.clone();
    strict.push(("toleranceGroup/lightness", num(0.0)));
    strict.push(("softness", num(0.0)));
    assert!(close(px("ec.color.changetocolor", &strict, [1.0, 0.5, 0.5, 1.0]), [0.0, 0.0, 0.0, 1.0], 1e-5));
    // Hue, Lightness & Saturation, set: a matched pixel becomes the To colour.
    let mut all = base.to_vec();
    all.push(("change", Value::Enum(3)));
    assert!(close(px("ec.color.changetocolor", &all, red), [0.1, 0.1, 0.8, 1.0], 1e-4));
}

#[test]
fn leave_color_tolerance_softness_and_match_mode() {
    let green = [0.1, 0.8, 0.1, 1.0];
    let lc = |vals: &[(&str, Value)]| {
        let mut v = vec![("amount", num(100.0))];
        v.extend_from_slice(vals);
        px("ec.color.leavecolor", &v, green)
    };
    let g = luminance(0.1, 0.8, 0.1);
    assert!(close(lc(&[]), [g, g, g, 1.0], 1e-5));
    assert!(close(lc(&[("tolerance", num(100.0))]), green, 1e-6));
    // Softness keeps part of the colour.
    let s = lc(&[("tolerance", num(10.0)), ("edgeSoftness", num(100.0))]);
    assert!(s[1] > g + 0.05 && s[1] < 0.8);
    // Using Hue: a dark red still matches pure red.
    let dark = px("ec.color.leavecolor", &[("amount", num(100.0)), ("tolerance", num(5.0)), ("matchColors", Value::Enum(1))], [0.4, 0.0, 0.0, 1.0]);
    assert!(close(dark, [0.4, 0.0, 0.0, 1.0], 1e-6));
}

#[test]
fn photo_filter_presets_and_custom() {
    let g = [0.5, 0.5, 0.5, 1.0];
    let cool = px("ec.color.photofilter", &[("filter", Value::Enum(3)), ("preserveLuminosity", Value::Bool(false))], g);
    assert!(cool[2] > cool[0], "a cooling filter is blue: {cool:?}");
    let custom = |c| px("ec.color.photofilter", &[("filter", Value::Enum(PHOTO_FILTER_CUSTOM)), ("color", c)], g);
    assert!(custom(col(0.0, 1.0, 0.0))[1] > custom(col(1.0, 0.0, 0.0))[1]);
    // The Color swatch is ignored by the presets.
    let warm = |c| px("ec.color.photofilter", &[("color", c)], g);
    assert_eq!(warm(col(0.0, 1.0, 0.0)), warm(col(1.0, 0.0, 0.0)));
}

#[test]
fn color_balance_preserve_luminosity_and_black_stretch() {
    let g = [0.5, 0.5, 0.5, 1.0];
    let o = px("ec.color.colorbalance", &[("midRed", num(60.0)), ("preserveLuminosity", Value::Bool(true))], g);
    assert!((luminance(o[0], o[1], o[2]) - 0.5).abs() < 1e-4 && o[0] > o[2]);
    let dark = [0.1, 0.1, 0.1, 1.0];
    assert!(px("ec.color.gammapedestalgain", &[("blackStretch", num(4.0))], dark)[0] > 0.15);
    assert!(close(px("ec.color.gammapedestalgain", &[], dark), dark, 1e-6));
}

#[test]
fn levels_channel_controls_act_on_their_channel() {
    let g = [0.5, 0.5, 0.5, 1.0];
    assert!(close(px("ec.color.levels", &[("channel", Value::Enum(1))], g), g, 1e-6), "the popup alone changes nothing");
    let r = px("ec.color.levels", &[("redOutWhite", num(0.5))], g);
    assert!(close(r, [0.25, 0.5, 0.5, 1.0], 1e-5), "{r:?}");
    let b = px("ec.color.levels", &[("blueGamma", num(2.0)), ("inWhite", num(0.5))], [0.25, 0.25, 0.25, 1.0]);
    assert!(close(b, [0.5, 0.5, 0.5f32.sqrt(), 1.0], 1e-5), "RGB first, then blue: {b:?}");
    let a = px("ec.color.levels", &[("alphaOutWhite", num(0.5))], [0.5, 0.5, 0.5, 1.0]);
    assert!(close(a, [0.25, 0.25, 0.25, 0.5], 1e-5), "{a:?}");
    // Per-channel clipping follows its own popups.
    let hot = [0.9, 0.9, 0.9, 1.0];
    let c = px("ec.color.levels", &[("greenInWhite", num(0.5))], hot)[1];
    let nc = px("ec.color.levels", &[("greenInWhite", num(0.5)), ("greenClipToOutputWhite", Value::Enum(0))], hot)[1];
    assert!((c - 1.0).abs() < 1e-6 && (nc - 1.8).abs() < 1e-5, "{c} {nc}");
}

#[test]
fn hue_saturation_ranges_adjust_their_hues_only() {
    let red = [0.8, 0.1, 0.1, 1.0];
    let blue = [0.1, 0.1, 0.8, 1.0];
    let grey = [0.5, 0.5, 0.5, 1.0];
    let v = [("redsSaturation", num(-100.0))];
    let r = px("ec.color.huesaturation", &v, red);
    assert!((r[0] - r[1]).abs() < 1e-4, "reds desaturated: {r:?}");
    assert!(close(px("ec.color.huesaturation", &v, blue), blue, 1e-5));
    assert!(close(px("ec.color.huesaturation", &v, grey), grey, 1e-5));
    // Blues hue +120°: blue becomes red.
    let b = px("ec.color.huesaturation", &[("bluesHue", num(120.0))], blue);
    assert!(b[0] > 0.7 && b[2] < 0.2, "{b:?}");
    // Fall-off: a hue 15° past the range end (Reds: −15..15, fall-off 30) gets half.
    let orange_ish = {
        let (r, g, b) = hsl_to_rgb(30.0 / 360.0, 0.8, 0.5);
        [r, g, b, 1.0]
    };
    let o = px("ec.color.huesaturation", &[("redsLightness", num(100.0))], orange_ish);
    let (_, _, l) = rgb_to_hsl(o[0], o[1], o[2]);
    assert!((l - 0.75).abs() < 0.01, "half weight: {l}");
    // Moving the range changes which hues it takes.
    let o2 = px("ec.color.huesaturation", &[("redsLightness", num(100.0)), ("redsRangeEnd", num(40.0))], orange_ish);
    assert!(rgb_to_hsl(o2[0], o2[1], o2[2]).2 > 0.99);
    let params = crate::Params { values: [("redsHue".to_string(), num(10.0))].into_iter().collect() };
    let ctx = EffectCtx { params: &params, time: 0.0, layer_size: [1.0, 1.0], seed: 0, adjustment: false, env: Default::default() };
    assert!(!huesat_ranges_identity(&ctx));
}

#[test]
fn fill_mask_all_masks_feather_and_invert() {
    let sq =
        |x0: f64, x1: f64| crate::MaskShape { name: String::new(), points: vec![[x0, 0.0], [x1, 0.0], [x1, 10.0], [x0, 10.0]], closed: true, inverted: false };
    let masks = [sq(0.0, 5.0), sq(15.0, 20.0)];
    let env = EffectEnv { masks: &masks, ..Default::default() };
    let grey = Image::filled(20, 10, [0.5, 0.5, 0.5, 1.0]);
    let run = |vals: &[(&str, Value)]| run_fx("ec.generate.fill", vals, grey.clone(), 0.0, env).img;
    let one = run(&[("fillMask", num(1.0))]);
    assert_eq!(one.get(2, 5), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(one.get(10, 5), [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(one.get(17, 5), [0.5, 0.5, 0.5, 1.0]);
    let all = run(&[("allMasks", Value::Bool(true))]);
    assert_eq!(all.get(17, 5), [1.0, 0.0, 0.0, 1.0]);
    let inv = run(&[("fillMask", num(1.0)), ("invert", Value::Bool(true))]);
    assert_eq!(inv.get(2, 5), [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(inv.get(10, 5), [1.0, 0.0, 0.0, 1.0]);
    let soft = run(&[("fillMask", num(1.0)), ("horizontalFeather", num(6.0))]);
    let v = soft.get(6, 5)[1];
    assert!(v > 0.05 && v < 0.5, "feathered edge {v}");
    // Without masks, Fill fills the layer as before.
    let plain = run_fx("ec.generate.fill", &[("fillMask", num(1.0))], grey.clone(), 0.0, EffectEnv::default()).img;
    assert_eq!(plain.get(10, 5), [1.0, 0.0, 0.0, 1.0]);
}

/// After Effects 2026: Fill (0, 0.5, 1) at Opacity 50 % on an opaque layer (a Gradient Ramp
/// on a red solid) renders premultiplied (0, 0.251, 0.502) at alpha 0.502 everywhere: the
/// fill colour at half the layer's alpha, nothing of the ramp left.
#[test]
fn fill_opacity_is_the_layers_opacity_as_in_after_effects() {
    let mut img = Image::new(16, 4);
    for x in 0..16 {
        let v = x as f32 / 15.0;
        for y in 0..4 {
            img.set(x, y, [v, 0.0, 0.0, 1.0]);
        }
    }
    let vals = [("color", Value::Color([0.0, 0.5, 1.0, 1.0])), ("opacity", num(50.0))];
    let out = run_fx("ec.generate.fill", &vals, img, 0.0, EffectEnv::default()).img;
    for x in [0, 7, 15] {
        let p = out.get(x, 2);
        let ae = [0.0, 0.251, 0.502, 0.502];
        assert!(p.iter().zip(ae).all(|(v, a)| (v - a).abs() < 1.0 / 255.0), "x {x}: {p:?} vs AE {ae:?}");
    }
    // A half-transparent pixel ends at a quarter alpha, still the fill colour.
    let half = run_fx("ec.generate.fill", &vals, Image::filled(2, 2, [0.2, 0.2, 0.2, 0.5]), 0.0, EffectEnv::default()).img;
    assert_eq!(half.get(0, 0), [0.0, 0.125, 0.25, 0.25]);
}

#[test]
fn colorama_palettes_modify_selection_masking_and_add_phase() {
    let grey = [0.5, 0.5, 0.5, 1.0];
    let red = [1.0, 0.0, 0.0, 1.0];
    // Ramp Grayscale: intensity 0.5 maps to mid grey.
    let g = px("ec.color.colorama", &[("outputCycle/usePresetPalette", Value::Enum(1))], grey);
    assert!((g[0] - 0.5).abs() < 0.01 && (g[0] - g[2]).abs() < 1e-5, "{g:?}");
    // Custom palette: two stops, black at 0 and blue at 0.5.
    let custom = [("outputCycle/usePresetPalette", Value::Enum(8)), ("outputCycle/palette", Value::Str("0:0,0,0 0.5:0,0,1".into()))];
    let c = px("ec.color.colorama", &custom, grey);
    assert!(c[2] > 0.99 && c[0] < 0.01, "{c:?}");
    // Modify Lightness keeps the hue of the pixel (red stays red).
    let l = px("ec.color.colorama", &[("modify/modify", Value::Enum(5)), ("outputCycle/usePresetPalette", Value::Enum(1))], red);
    assert!(l[0] > l[1] + 0.2 && l[1] < 0.01 + l[2] + 1e-4, "{l:?}");
    // Pixel Selection: only colours near Matching Color change.
    let sel = [
        ("pixelSelection/matchingMode", Value::Enum(1)),
        ("pixelSelection/matchingColor", col(1.0, 0.0, 0.0)),
        ("pixelSelection/matchingTolerance", num(10.0)),
    ];
    assert!(close(px("ec.color.colorama", &sel, grey), grey, 1e-6));
    assert!(!close(px("ec.color.colorama", &sel, red), red, 1e-3));
    // Composite Over Layer off: unselected pixels vanish.
    let mut off = sel.to_vec();
    off.push(("masking/compositeOverLayer", Value::Bool(false)));
    assert_eq!(px("ec.color.colorama", &off, grey)[3], 0.0);
    // Modify Alpha takes the palette alpha.
    let alpha = [
        ("outputCycle/usePresetPalette", Value::Enum(8)),
        ("outputCycle/palette", Value::Str("0:1,1,1,0.25".into())),
        ("modify/modifyAlpha", Value::Bool(true)),
    ];
    assert!((px("ec.color.colorama", &alpha, grey)[3] - 0.25).abs() < 1e-5);
    // Palette parsing and evaluation.
    let p = ColoramaPalette::parse("0.5:0,0,1 0:1,0,0").unwrap();
    assert_eq!(p.eval(0.25, true), [0.5, 0.0, 0.5, 1.0]);
    assert_eq!(p.eval(0.75, true), [0.5, 0.0, 0.5, 1.0], "wraps around");
    assert_eq!(p.eval(0.25, false), [1.0, 0.0, 0.0, 1.0]);
}
