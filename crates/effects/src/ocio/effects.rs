use super::cdl::Cdl;
use super::lut::load_file;
use super::space::{ACES_CCT, ACES_CG, ACES2065_1, D65, IDENTITY, P3, REC709, REC2020, SPACES, Space, Tf, Xform, matrix, sp, space_names};
use super::{Buf, ColorSpace, EffectCtx, EffectSpec, IntoParallelRefMutIterator, ParallelIterator, Value, invert, mul, mul_vec, p, popup, unpremul};

// ---------------------------------------------------------------- displays, views and looks

/// Displays of the built-in config: (name, display primaries/white, encoding).
pub const DISPLAYS: &[(&str, [[f64; 2]; 3], Tf)] =
    &[("sRGB", REC709, Tf::Srgb), ("Rec.1886 Rec.709", REC709, Tf::Gamma(2.4)), ("Display P3", P3, Tf::Srgb), ("Rec.2100-PQ", REC2020, Tf::Pq)];

pub const VIEWS: &[&str] = &["Standard", "Tone Mapped", "Raw"];

/// Configuration popup: the built-in config, or a custom `.ocio` file (Config File: a path or
/// the config's text; spaces chosen by name, see [`crate::ocio_config`]).
pub const CONFIGS: &[&str] = &["Built-in (EffectCraft minimal)", "Custom (.ocio file)"];

/// Built-in looks: CDLs applied in ACEScct.
pub fn looks() -> Vec<(&'static str, Cdl)> {
    let grey = 0.413_588_402_492_442_1; // ACEScct of 0.18
    let c = |slope: [f64; 3], offset: [f64; 3], sat: f64| Cdl { slope, offset, power: [1.0; 3], sat };
    vec![
        ("Warm", c([1.04, 1.0, 0.94], [0.0; 3], 1.0)),
        ("Cool", c([0.95, 1.0, 1.06], [0.0; 3], 1.0)),
        ("High Contrast", c([1.25; 3], [grey * (1.0 - 1.25); 3], 1.0)),
        ("Low Contrast", c([0.8; 3], [grey * (1.0 - 0.8); 3], 1.0)),
        ("Desaturated", c([1.0; 3], [0.0; 3], 0.6)),
        ("Vivid", c([1.0; 3], [0.0; 3], 1.3)),
    ]
}

/// Extended Reinhard roll-off (white point 16): maps 0..∞ into 0..1, ≈ identity near 0.
pub(super) fn tonemap(v: f64) -> f64 {
    let w2 = 256.0;
    if v <= 0.0 { v } else { v * (1.0 + v / w2) / (1.0 + v) }
}

pub(super) fn tonemap_inv(y: f64) -> f64 {
    if y <= 0.0 {
        return y;
    }
    // Solve v² / w² + v (1 − y) − y = 0.
    let (a, b, c) = (1.0 / 256.0, 1.0 - y, -y);
    (-b + (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
}

/// Display transform of an input-space colour (`inverse`: display code values → input space).
pub fn display_xform(input: &Space, display: usize, view: usize, inverse: bool) -> impl Fn([f32; 3]) -> [f32; 3] + Sync {
    let (_, prims, tf) = DISPLAYS[display.min(DISPLAYS.len() - 1)];
    let disp = Space { name: "display", prims: Some((prims, D65)), tf: Tf::Linear, raw: false };
    let raw = view == 2 || input.raw;
    let m_in = Xform::new(&Space { tf: Tf::Linear, ..*input }, &disp);
    let m_out = Xform::new(&disp, &Space { tf: Tf::Linear, ..*input });
    let in_tf = input.tf;
    move |c: [f32; 3]| {
        if raw {
            return c;
        }
        if !inverse {
            let lin = c.map(|v| in_tf.decode(v as f64) as f32);
            let d = m_in.apply(lin);
            d.map(|v| {
                let v = v as f64;
                let v = if view == 1 { tonemap(v) } else { v };
                tf.encode(v) as f32
            })
        } else {
            let d = c.map(|v| {
                let l = tf.decode(v as f64);
                (if view == 1 { tonemap_inv(l) } else { l }) as f32
            });
            m_out.apply(d).map(|v| in_tf.encode(v as f64) as f32)
        }
    }
}

// ---------------------------------------------------------------- effects

pub(super) fn spec(id: &'static str, name: &'static str, cat: &'static str, params: Vec<crate::ParamSpec>, render: crate::RenderFn) -> EffectSpec {
    EffectSpec { id, name, category: cat, params, render, gpu: false, float: true }
}

pub(super) fn space_param(id: &'static str, name: &'static str, default: u32) -> crate::ParamSpec {
    p(id, name, Value::Enum(default), popup(&space_names()))
}

pub(super) fn direction() -> crate::ParamSpec {
    p("direction", "Direction", Value::Enum(0), popup(&["Forward", "Inverse"]))
}

fn map_px(b: &mut Buf, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    b.img.data.par_iter_mut().for_each(|px| {
        let (c, a) = unpremul(*px);
        if a <= 0.0 {
            return;
        }
        let o = f(c);
        *px = [o[0] * a, o[1] * a, o[2] * a, a];
    });
}

pub(super) fn space_at(i: u32) -> &'static Space {
    &SPACES[(i as usize).min(SPACES.len() - 1)]
}

/// The effects that take Configuration ▸ Custom (a `.ocio` file).
pub(crate) const CUSTOM_CONFIG_FX: [&str; 2] = ["ec.color.ociocolorspace", "ec.color.ociodisplay"];

/// Effect `id`'s transform under Configuration ▸ Custom: the config's conversion from the source
/// space to the destination space (Color Space Transform) or through a display's view (Display
/// Transform), see [`crate::ocio_config`]. `Ok(None)` with the built-in configuration; an error
/// (the pixels pass through) when the config can't be read or has no such colour space.
pub(super) fn custom_xf(id: &str, pr: &crate::Params) -> Result<Option<crate::ocio_config::Xf>, String> {
    use crate::ocio_config::Xf;
    if !CUSTOM_CONFIG_FX.contains(&id) || pr.e("config") != 1 {
        return Ok(None);
    }
    let cfg = crate::ocio_config::load_config(pr.s("configFile")).map_err(|e| format!("Can't read the OCIO config: {e}"))?;
    // A space by its typed name, else by the built-in popup's name.
    let space = |name: &str, popup: u32| {
        let name = if name.trim().is_empty() { space_at(popup).name } else { name.trim() };
        cfg.space(name).ok_or_else(|| format!("The OCIO config has no color space `{name}`"))
    };
    let a = space(pr.s("sourceName"), pr.e("source"))?;
    let inverse = pr.e("direction") == 1;
    if id == "ec.color.ociodisplay" {
        let (dn, vn) = display_view_names(pr);
        let xf = cfg.display_path(a, dn, vn);
        return Ok(Some(if inverse { Xf::Inverse(Box::new(xf)) } else { xf }));
    }
    let z = space(pr.s("destinationName"), pr.e("destination"))?;
    // Inverse: the conversion the other way (OCIO's inverse ColorSpaceTransform).
    Ok(Some(if inverse { cfg.path(z, a) } else { cfg.path(a, z) }))
}

/// The display and view a Display Transform names: the typed names, else the popups'.
fn display_view_names(pr: &crate::Params) -> (&str, &str) {
    let display = DISPLAYS.get(pr.e("display") as usize).map_or("", |d| d.0);
    let view = VIEWS.get(pr.e("view") as usize).copied().unwrap_or("");
    let typed = |k: &str, popup| if pr.s(k).trim().is_empty() { popup } else { pr.s(k).trim() };
    (typed("displayName", display), typed("viewName", view))
}

/// Effect Controls' warning for effect `id`: a custom OCIO configuration that can't be read, has
/// no such colour space, display or view, or uses transforms EffectCraft can't apply (colours
/// pass through those steps, #410). `None` when the effect renders as asked.
pub fn warning(id: &str, pr: &crate::Params) -> Option<String> {
    let xf = match custom_xf(id, pr) {
        Ok(x) => x?,
        Err(e) => return Some(format!("{e}: the colors pass through.")),
    };
    let mut notes = vec![];
    if id == "ec.color.ociodisplay"
        && let Ok(cfg) = crate::ocio_config::load_config(pr.s("configFile"))
    {
        let (dn, vn) = display_view_names(pr);
        if let Some((d, v)) = cfg.view(dn, vn)
            && (!d.eq_ignore_ascii_case(dn) || !v.name.eq_ignore_ascii_case(vn))
        {
            notes.push(format!("The OCIO config has no display `{dn}` with a view `{vn}`: showing {d} / {}.", v.name));
        }
    }
    let missing = xf.unsupported_list();
    if !missing.is_empty() {
        notes.push(format!("Not supported, the colors pass through: {}.", missing.join(", ")));
    }
    (!notes.is_empty()).then(|| notes.join(" "))
}

/// Configuration ▸ Custom: render through the config's transform. `false` with the built-in
/// configuration (the caller renders it).
fn custom_render(id: &str, ctx: &EffectCtx, b: &mut Buf) -> bool {
    match custom_xf(id, ctx.params) {
        Ok(None) => false,
        Ok(Some(xf)) => {
            map_px(b, |c| xf.apply(c.map(|v| v as f64), false).map(|v| v as f32));
            true
        }
        // A config that can't be read, or an unknown space: the pixels pass through.
        Err(_) => true,
    }
}

pub(super) fn ocio_cst(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if custom_render("ec.color.ociocolorspace", ctx, &mut b) {
        return b;
    }
    let (a, z) = (space_at(ctx.params.e("source")), space_at(ctx.params.e("destination")));
    let (a, z) = if ctx.params.e("direction") == 1 { (z, a) } else { (a, z) };
    if a.name == z.name {
        return b;
    }
    let x = Xform::new(a, z);
    map_px(&mut b, |c| x.apply(c));
    b
}

pub(super) fn cdl_from_params(pr: &crate::Params) -> Cdl {
    Cdl {
        slope: [pr.f("slopeRed"), pr.f("slopeGreen"), pr.f("slopeBlue")],
        offset: [pr.f("offsetRed"), pr.f("offsetGreen"), pr.f("offsetBlue")],
        power: [pr.f("powerRed").max(0.0), pr.f("powerGreen").max(0.0), pr.f("powerBlue").max(0.0)],
        sat: pr.f("saturation").max(0.0),
    }
}

pub(super) fn ocio_cdl(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let cdl = cdl_from_params(ctx.params);
    if cdl == Cdl::default() {
        return b;
    }
    let clamp = ctx.params.e("style") == 0;
    let inv = ctx.params.e("direction") == 1;
    map_px(&mut b, |c| {
        let c = c.map(|v| v as f64);
        let o = if inv { cdl.invert(c, clamp) } else { cdl.apply(c, clamp) };
        o.map(|v| v as f32)
    });
    b
}

pub(super) fn ocio_display(ctx: &EffectCtx, mut b: Buf) -> Buf {
    if custom_render("ec.color.ociodisplay", ctx, &mut b) {
        return b;
    }
    let input = space_at(ctx.params.e("source"));
    let f = display_xform(input, ctx.params.e("display") as usize, ctx.params.e("view") as usize, ctx.params.e("direction") == 1);
    map_px(&mut b, f);
    b
}

pub(super) fn ocio_file(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(x) = load_file(ctx.params.s("file")) else { return b };
    let interp = match ctx.params.e("interpolation") {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    let inv = ctx.params.e("direction") == 1;
    let id = ctx.params.s("cccId").to_string();
    map_px(&mut b, |c| x.apply(c, interp, inv, &id));
    b
}

/// Parse a looks string: comma/space separated names, `-Name` = inverse.
pub fn parse_looks(s: &str) -> Vec<(Cdl, bool)> {
    let all = looks();
    s.split([',', ':'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            let (inv, n) = match t.strip_prefix('-') {
                Some(r) => (true, r.trim()),
                None => (false, t.trim_start_matches('+').trim()),
            };
            all.iter().find(|(name, _)| name.eq_ignore_ascii_case(n)).map(|(_, c)| (*c, inv))
        })
        .collect()
}

pub(super) fn ocio_look(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let mut list = parse_looks(ctx.params.s("looks"));
    let pick = ctx.params.e("look");
    if pick > 0
        && let Some((_, c)) = looks().get(pick as usize - 1)
    {
        list.insert(0, (*c, false));
    }
    if list.is_empty() {
        return b;
    }
    let (src, dst) = (space_at(ctx.params.e("source")), space_at(ctx.params.e("destination")));
    let inv = ctx.params.e("direction") == 1;
    let cct = &ACES_CCT;
    let (a, z) = if inv { (dst, src) } else { (src, dst) };
    let to_p = Xform::new(a, cct);
    let from_p = Xform::new(cct, z);
    if inv {
        list.reverse();
    }
    map_px(&mut b, |c| {
        let mut v = to_p.apply(c).map(|x| x as f64);
        for (cdl, li) in &list {
            v = if *li != inv { cdl.invert(v, false) } else { cdl.apply(v, false) };
        }
        from_p.apply(v.map(|x| x as f32))
    });
    b
}

// ---- Color Profile Converter (Utility)

pub const PROFILES: &[&str] = &["Project Working Space", "sRGB IEC61966-2.1", "HDTV (Rec. 709)", "Rec. 2020", "Display P3", "ACEScg", "ACES2065-1"];

pub(super) fn profile(ctx: &EffectCtx, i: u32) -> (Space, bool) {
    let ws = |cs: ColorSpace| match cs {
        ColorSpace::Srgb => sp("sRGB", REC709, D65, Tf::Srgb),
        ColorSpace::Rec709 => sp("Rec.709", REC709, D65, Tf::Gamma(2.4)),
        ColorSpace::Rec2020 => sp("Rec.2020", REC2020, D65, Tf::Gamma(2.4)),
        ColorSpace::DisplayP3 => sp("Display P3", P3, D65, Tf::Srgb),
        ColorSpace::AcesCg => ACES_CG,
        ColorSpace::Aces2065 => ACES2065_1,
        // HDR encodings are output spaces, never a working space.
        ColorSpace::Rec2100Pq | ColorSpace::Rec2100Hlg => sp("Rec.2020", REC2020, D65, Tf::Gamma(2.4)),
    };
    match i {
        0 => (ws(ctx.env.working_space.unwrap_or(ColorSpace::Srgb)), ctx.env.working_linear),
        1 => (ws(ColorSpace::Srgb), false),
        2 => (ws(ColorSpace::Rec709), false),
        3 => (ws(ColorSpace::Rec2020), false),
        4 => (ws(ColorSpace::DisplayP3), false),
        5 => (ACES_CG, true),
        _ => (ACES2065_1, true),
    }
}

/// Gamut compression for the perceptual intent: pull out-of-gamut (negative) colours toward
/// their luminance just enough to bring every channel to ≥ 0, rolling off smoothly.
fn compress_gamut(c: [f64; 3], luma: [f64; 3]) -> [f64; 3] {
    let l = c[0] * luma[0] + c[1] * luma[1] + c[2] * luma[2];
    if l <= 0.0 {
        return c.map(|v| v.max(0.0));
    }
    let mn = c.iter().cloned().fold(f64::INFINITY, f64::min);
    if mn >= 0.0 {
        return c;
    }
    // Fraction of the way toward grey that reaches zero on the most negative channel.
    let t = (l / (l - mn)).clamp(0.0, 1.0);
    c.map(|v| l + (v - l) * t)
}

pub(super) fn profile_converter(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let (inp, in_lin_default) = profile(ctx, ctx.params.e("inputProfile"));
    let (out, out_lin_default) = profile(ctx, ctx.params.e("outputProfile"));
    let in_lin = ctx.params.b("linearizeInputProfile") || in_lin_default && ctx.params.e("inputProfile") == 0;
    let out_lin = ctx.params.b("linearizeOutputProfile") || out_lin_default && ctx.params.e("outputProfile") == 0;
    let intent = ctx.params.e("intent");
    let dec = if in_lin { Tf::Linear } else { inp.tf };
    let enc = if out_lin { Tf::Linear } else { out.tf };
    // Absolute colorimetric keeps the source white (no adaptation); the others adapt.
    let m = if intent == 3 { mul(&invert(&out.to_ref(false)), &inp.to_ref(false)) } else { matrix(&inp, &out) };
    let identity = dec == enc && (0..3).all(|i| (0..3).all(|j| (m[i][j] - IDENTITY[i][j]).abs() < 1e-9));
    if identity {
        return b;
    }
    let out_luma = out.to_ref(true)[1];
    map_px(&mut b, |c| {
        let l = c.map(|v| dec.decode(v as f64));
        let mut o = mul_vec(&m, l);
        match intent {
            0 => o = compress_gamut(o, out_luma),
            2 => o = o.map(|v| v.max(0.0)),
            _ => {}
        }
        o.map(|v| enc.encode(v) as f32)
    });
    b
}

// ---- Color Stabilizer

/// Mean straight colour of a square (half-size `r` layer pixels) around layer point `pt`.
fn sample_mean(b: &Buf, pt: [f64; 2], r: f64) -> [f64; 3] {
    let (cx, cy) = b.to_px(pt);
    let rr = (r * b.scale).max(0.5);
    let (x0, x1) = ((cx - rr).floor().max(0.0) as i64, ((cx + rr).ceil() as i64).min(b.img.width as i64));
    let (y0, y1) = ((cy - rr).floor().max(0.0) as i64, ((cy + rr).ceil() as i64).min(b.img.height as i64));
    let mut acc = [0.0f64; 4];
    for y in y0..y1 {
        for x in x0..x1 {
            let p = b.img.get(x, y);
            for k in 0..4 {
                acc[k] += p[k] as f64;
            }
        }
    }
    if acc[3] <= 1e-9 {
        return [0.0; 3];
    }
    [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3]]
}

/// Monotone piecewise-quadratic curve through (0,0), three samples and (1,1) (linear between
/// them is enough for a stabiliser and stays monotone).
fn curve3(xs: [f64; 3], ys: [f64; 3], v: f64) -> f64 {
    let mut pts = vec![(0.0, 0.0)];
    let mut pairs: Vec<(f64, f64)> = xs.iter().cloned().zip(ys.iter().cloned()).collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (x, y) in pairs {
        if pts.last().is_some_and(|l| x > l.0 + 1e-4) && x < 1.0 - 1e-4 {
            pts.push((x, y));
        }
    }
    pts.push((1.0, 1.0));
    let i = pts.partition_point(|p| p.0 < v).clamp(1, pts.len() - 1);
    let (a, c) = (pts[i - 1], pts[i]);
    a.1 + (c.1 - a.1) * (v - a.0) / (c.0 - a.0).max(1e-9)
}

/// Color Stabilizer's correction of buffer `b` as one piecewise-linear map per channel (points
/// with increasing x; the end segments extend), the same maps `color_stabilizer` applies.
/// `None` without a host or reference frame (the layer passes through). (The GPU effect.)
pub fn color_stabilizer_maps(ctx: &EffectCtx, b: &Buf) -> Option<[Vec<(f64, f64)>; 3]> {
    let reference = ctx.env.host?.self_at(ctx.params.f("referenceFrame"), ctx.env.effect_index)?;
    let r = ctx.params.f("sampleSize").max(0.5);
    let pts = [ctx.params.v2("blackPoint"), ctx.params.v2("midPoint"), ctx.params.v2("whitePoint")];
    let cur = pts.map(|pt| sample_mean(b, pt, r));
    let refv = pts.map(|pt| sample_mean(&reference, pt, r));
    let mode = ctx.params.e("stabilize");
    Some([0, 1, 2].map(|k| {
        let offset = |c0: f64, r0: f64| vec![(0.0, r0 - c0), (1.0, 1.0 + r0 - c0)];
        match mode {
            0 => offset(cur[0][k], refv[0][k]),
            1 => {
                let (c0, c1, r0, r1) = (cur[0][k], cur[2][k], refv[0][k], refv[2][k]);
                if (c1 - c0).abs() < 1e-6 {
                    offset(c0, r0)
                } else if c0 < c1 {
                    vec![(c0, r0), (c1, r1)]
                } else {
                    vec![(c1, r1), (c0, r0)]
                }
            }
            _ => {
                let mut v = vec![(0.0, 0.0)];
                let mut pairs: Vec<(f64, f64)> = (0..3).map(|i| (cur[i][k], refv[i][k])).collect();
                pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (x, y) in pairs {
                    if v.last().is_some_and(|l| x > l.0 + 1e-4) && x < 1.0 - 1e-4 {
                        v.push((x, y));
                    }
                }
                v.push((1.0, 1.0));
                v
            }
        }
    }))
}

pub(super) fn color_stabilizer(ctx: &EffectCtx, mut b: Buf) -> Buf {
    let Some(host) = ctx.env.host else { return b };
    let Some(reference) = host.self_at(ctx.params.f("referenceFrame"), ctx.env.effect_index) else { return b };
    let r = ctx.params.f("sampleSize").max(0.5);
    let pts = [ctx.params.v2("blackPoint"), ctx.params.v2("midPoint"), ctx.params.v2("whitePoint")];
    let cur = pts.map(|pt| sample_mean(&b, pt, r));
    let refv = pts.map(|pt| sample_mean(&reference, pt, r));
    let mode = ctx.params.e("stabilize");
    map_px(&mut b, |c| {
        [0, 1, 2].map(|k| {
            let v = c[k] as f64;
            let o = match mode {
                // Brightness: one sample (black point) — offset each channel.
                0 => v + refv[0][k] - cur[0][k],
                // Levels: black and white samples — linear map per channel.
                1 => {
                    let (c0, c1, r0, r1) = (cur[0][k], cur[2][k], refv[0][k], refv[2][k]);
                    if (c1 - c0).abs() < 1e-6 { v + r0 - c0 } else { r0 + (v - c0) * (r1 - r0) / (c1 - c0) }
                }
                // Curves: three samples — a curve through them per channel.
                _ => curve3([cur[0][k], cur[1][k], cur[2][k]], [refv[0][k], refv[1][k], refv[2][k]], v),
            };
            o as f32
        })
    });
    b
}
