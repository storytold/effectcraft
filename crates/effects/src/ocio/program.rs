use super::cdl::Cdl;
use super::effects::{DISPLAYS, cdl_from_params, custom_xf, looks, parse_looks, profile, space_at};
use super::lut::{FileXform, load_file};
use super::space::{ACES_CCT, D65, IDENTITY, Space, Tf, Xform, matrix};
use super::{Arc, EffectCtx, Lut, invert, mul};

// ---- Colour programs (the GPU compositor's colour-management kernels)

/// One per-pixel step of a colour transform, in the order the CPU effect applies them (see
/// [`color_program`]).
#[derive(Clone, Debug)]
pub enum ColorOp {
    /// Transfer function, encoded → linear.
    Decode(Tf),
    /// Transfer function, linear → encoded.
    Encode(Tf),
    /// Linear-RGB matrix.
    Matrix([[f32; 3]; 3]),
    /// ASC CDL (forward or exact inverse; `clamp` = the v1.2 style).
    Cdl { cdl: Cdl, clamp: bool, inverse: bool },
    /// The Tone Mapped view's roll-off (or its inverse).
    Tonemap { inverse: bool },
    /// Color Profile Converter's perceptual gamut compression toward luminance `luma`.
    Gamut([f64; 3]),
    /// Negative channels to 0 (the saturation intent).
    Max0,
    /// A LUT (0 nearest, 1 trilinear, 2 tetrahedral) with an optional per-channel shaper, forward
    /// or inverse (as `FileXform::apply`).
    Lut { lut: Arc<Lut>, shaper: Option<Arc<[Vec<(f32, f32)>; 3]>>, interp: u32, inverse: bool },
    /// `m × (c + pre) + post` (a custom config's MatrixTransform and RangeTransform).
    Affine { m: [[f32; 3]; 3], pre: [f32; 3], post: [f32; 3] },
    /// `max(c, 0) ^ p` per channel (ExponentTransform).
    Pow([f32; 3]),
    /// LogTransform / LogAffineTransform, forward or inverse (as `ocio_config::Xf::apply`).
    LogAffine { base: f32, log_slope: [f32; 3], log_offset: [f32; 3], lin_slope: [f32; 3], lin_offset: [f32; 3], inverse: bool },
    /// ExponentWithLinearTransform, forward (decode) or inverse (as
    /// [`crate::ocio_config::moncurve`]).
    MonCurve { gamma: [f32; 3], offset: [f32; 3], inverse: bool, mirror: bool },
    /// Clamp each channel to `lo..=hi` (a clamping RangeTransform; ±`f32::MAX` = unbounded).
    Clamp { lo: [f32; 3], hi: [f32; 3] },
}

/// The ops of a matrix-and-offset transform (MatrixTransform, `.spimtx`), forward or inverse
/// (as [`crate::ocio_config::matrix_apply`]).
fn matrix_ops(m: &[f64; 16], offset: &[f64; 4], inverse: bool, ops: &mut Vec<ColorOp>) {
    if !inverse {
        let m = [0, 1, 2].map(|r| [m[r * 4] as f32, m[r * 4 + 1] as f32, m[r * 4 + 2] as f32]);
        ops.push(ColorOp::Affine { m, pre: [0.0; 3], post: [offset[0] as f32, offset[1] as f32, offset[2] as f32] });
    } else if let Some(inv) = crate::ocio_config::mat3_inverse(m) {
        let m = inv.map(|r| r.map(|v| v as f32));
        ops.push(ColorOp::Affine { m, pre: [-offset[0] as f32, -offset[1] as f32, -offset[2] as f32], post: [0.0; 3] });
    }
}

/// The ops of a LUT / CDL / matrix file transform (as [`FileXform::apply`]).
fn file_ops(x: &FileXform, interp: u32, inverse: bool, ccc_id: &str, ops: &mut Vec<ColorOp>) {
    match x {
        FileXform::Cdl(list) => {
            let cdl = list.iter().find(|(i, _)| !ccc_id.is_empty() && i == ccc_id).or(list.first()).map(|(_, c)| *c).unwrap_or_default();
            ops.push(ColorOp::Cdl { cdl, clamp: true, inverse });
        }
        FileXform::Lut { lut, shaper } => {
            ops.push(ColorOp::Lut { lut: Arc::new(lut.clone()), shaper: shaper.clone().map(Arc::new), interp, inverse });
        }
        FileXform::Matrix { m, offset } => matrix_ops(m, offset, inverse, ops),
    }
}

/// The ops of a custom config's transform `x` (as `Xf::apply`).
fn config_xf_ops(x: &crate::ocio_config::Xf, inverse: bool, ops: &mut Vec<ColorOp>) {
    use crate::ocio_config::Xf;
    let f3 = |v: &[f64; 3]| v.map(|x| x as f32);
    match x {
        Xf::Inverse(x) => config_xf_ops(x, !inverse, ops),
        Xf::Matrix { m, offset } => matrix_ops(m, offset, inverse, ops),
        Xf::File { xf: Some(x), interp, ccc, .. } => file_ops(x, *interp, inverse, ccc, ops),
        Xf::ExponentLinear { gamma, offset, mirror } => ops.push(ColorOp::MonCurve { gamma: f3(gamma), offset: f3(offset), inverse, mirror: *mirror }),
        // A BuiltinTransform's curve: forward decodes.
        Xf::Curve(tf) => ops.push(if inverse { ColorOp::Encode(*tf) } else { ColorOp::Decode(*tf) }),
        Xf::Named(_) => {}
        Xf::File { xf: None, .. } | Xf::Unsupported(_) => {}
        Xf::Exponent(e) => ops.push(ColorOp::Pow([0, 1, 2].map(|k| (if inverse { 1.0 / e[k].max(1e-9) } else { e[k] }) as f32))),
        Xf::Log { base } => {
            ops.push(ColorOp::LogAffine { base: *base as f32, log_slope: [1.0; 3], log_offset: [0.0; 3], lin_slope: [1.0; 3], lin_offset: [0.0; 3], inverse })
        }
        Xf::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset } => ops.push(ColorOp::LogAffine {
            base: *base as f32,
            log_slope: f3(log_slope),
            log_offset: f3(log_offset),
            lin_slope: f3(lin_slope),
            lin_offset: f3(lin_offset),
            inverse,
        }),
        Xf::Cdl(cdl) => ops.push(ColorOp::Cdl { cdl: *cdl, clamp: true, inverse }),
        Xf::Range(r) => {
            let (k, off, lo, hi) = r.parts(inverse);
            let k = k as f32;
            ops.push(ColorOp::Affine { m: [[k, 0.0, 0.0], [0.0, k, 0.0], [0.0, 0.0, k]], pre: [0.0; 3], post: [off as f32; 3] });
            if lo.is_finite() || hi.is_finite() {
                let bound = |v: f64| (v as f32).clamp(-f32::MAX, f32::MAX);
                ops.push(ColorOp::Clamp { lo: [bound(lo); 3], hi: [bound(hi); 3] });
            }
        }
        Xf::Group(list) => {
            if inverse {
                list.iter().rev().for_each(|x| config_xf_ops(x, true, ops));
            } else {
                list.iter().for_each(|x| config_xf_ops(x, false, ops));
            }
        }
    }
}

/// How a colour program meets premultiplied pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Straight {
    /// `unpremul` (colour 0 below alpha 1e-6); fully transparent pixels untouched (OCIO effects).
    Unpremul,
    /// Colour ÷ alpha; fully transparent pixels untouched (`Image::map_straight`, Apply Color LUT).
    Divide,
}

fn xform_ops(x: &Xform, ops: &mut Vec<ColorOp>) {
    if x.dec != Tf::Linear {
        ops.push(ColorOp::Decode(x.dec));
    }
    if !x.ident {
        ops.push(ColorOp::Matrix(x.m));
    }
    if x.enc != Tf::Linear {
        ops.push(ColorOp::Encode(x.enc));
    }
}

/// Effect `id`'s per-pixel transform as a list of [`ColorOp`]s that reproduces the CPU effect
/// (empty = the layer passes through; custom OCIO configs compile their transforms too). `None`
/// for another effect.
pub fn color_program(id: &str, ctx: &EffectCtx) -> Option<(Vec<ColorOp>, Straight)> {
    let pr = ctx.params;
    let mut ops = vec![];
    // Configuration ▸ Custom compiles the config's transforms (a config that can't be read, or
    // names unknown spaces, passes the pixels through).
    match custom_xf(id, pr) {
        Ok(None) => {}
        Ok(Some(xf)) => {
            config_xf_ops(&xf, false, &mut ops);
            return Some((ops, Straight::Unpremul));
        }
        Err(_) => return Some((ops, Straight::Unpremul)),
    }
    match id {
        "ec.utility.applylut" => {
            if let Some(lut) = crate::utility::load_lut(pr.s("lut")) {
                ops.push(ColorOp::Lut { lut, shaper: None, interp: 2, inverse: false });
            }
            return Some((ops, Straight::Divide));
        }
        "ec.color.ociocolorspace" => {
            let (a, z) = (space_at(pr.e("source")), space_at(pr.e("destination")));
            let (a, z) = if pr.e("direction") == 1 { (z, a) } else { (a, z) };
            if a.name != z.name {
                xform_ops(&Xform::new(a, z), &mut ops);
            }
        }
        "ec.color.ociocdl" => {
            let cdl = cdl_from_params(pr);
            if cdl != Cdl::default() {
                ops.push(ColorOp::Cdl { cdl, clamp: pr.e("style") == 0, inverse: pr.e("direction") == 1 });
            }
        }
        "ec.color.ociodisplay" => {
            let input = space_at(pr.e("source"));
            let (display, view, inverse) = (pr.e("display") as usize, pr.e("view") as usize, pr.e("direction") == 1);
            let (_, prims, tf) = DISPLAYS[display.min(DISPLAYS.len() - 1)];
            if view != 2 && !input.raw {
                let disp = Space { name: "display", prims: Some((prims, D65)), tf: Tf::Linear, raw: false };
                let lin = Space { tf: Tf::Linear, ..*input };
                if !inverse {
                    ops.push(ColorOp::Decode(input.tf));
                    xform_ops(&Xform::new(&lin, &disp), &mut ops);
                    if view == 1 {
                        ops.push(ColorOp::Tonemap { inverse: false });
                    }
                    ops.push(ColorOp::Encode(tf));
                } else {
                    ops.push(ColorOp::Decode(tf));
                    if view == 1 {
                        ops.push(ColorOp::Tonemap { inverse: true });
                    }
                    xform_ops(&Xform::new(&disp, &lin), &mut ops);
                    ops.push(ColorOp::Encode(input.tf));
                }
            }
        }
        "ec.color.ociofile" => {
            if let Some(x) = load_file(pr.s("file")) {
                let interp = match pr.e("interpolation") {
                    0 => 0,
                    1 => 1,
                    _ => 2,
                };
                file_ops(&x, interp, pr.e("direction") == 1, pr.s("cccId"), &mut ops);
            }
        }
        "ec.color.ociolook" => {
            let mut list = parse_looks(pr.s("looks"));
            let pick = pr.e("look");
            if pick > 0
                && let Some((_, c)) = looks().get(pick as usize - 1)
            {
                list.insert(0, (*c, false));
            }
            if !list.is_empty() {
                let (src, dst) = (space_at(pr.e("source")), space_at(pr.e("destination")));
                let inv = pr.e("direction") == 1;
                let cct = &ACES_CCT;
                let (a, z) = if inv { (dst, src) } else { (src, dst) };
                if inv {
                    list.reverse();
                }
                xform_ops(&Xform::new(a, cct), &mut ops);
                for (cdl, li) in &list {
                    ops.push(ColorOp::Cdl { cdl: *cdl, clamp: false, inverse: *li != inv });
                }
                xform_ops(&Xform::new(cct, z), &mut ops);
            }
        }
        "ec.utility.colorprofileconverter" => {
            let (inp, in_lin_default) = profile(ctx, pr.e("inputProfile"));
            let (out, out_lin_default) = profile(ctx, pr.e("outputProfile"));
            let in_lin = pr.b("linearizeInputProfile") || in_lin_default && pr.e("inputProfile") == 0;
            let out_lin = pr.b("linearizeOutputProfile") || out_lin_default && pr.e("outputProfile") == 0;
            let intent = pr.e("intent");
            let dec = if in_lin { Tf::Linear } else { inp.tf };
            let enc = if out_lin { Tf::Linear } else { out.tf };
            let m = if intent == 3 { mul(&invert(&out.to_ref(false)), &inp.to_ref(false)) } else { matrix(&inp, &out) };
            let identity = dec == enc && (0..3).all(|i| (0..3).all(|j| (m[i][j] - IDENTITY[i][j]).abs() < 1e-9));
            if !identity {
                ops.push(ColorOp::Decode(dec));
                ops.push(ColorOp::Matrix(m.map(|r| r.map(|v| v as f32))));
                match intent {
                    0 => ops.push(ColorOp::Gamut(out.to_ref(true)[1])),
                    2 => ops.push(ColorOp::Max0),
                    _ => {}
                }
                ops.push(ColorOp::Encode(enc));
            }
        }
        _ => return None,
    }
    Some((ops, Straight::Unpremul))
}
