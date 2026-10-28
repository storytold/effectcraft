use super::{Buf, Image, IndexedParallelIterator, IntoParallelRefIterator, IntoParallelRefMutIterator, ParallelIterator, ParallelSliceMut, Px, unpremul};

// ------------------------------------------------------------------ band rasteriser

const BAND: usize = 16;

/// How rasterised items combine with what is already in the target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Acc {
    /// Premultiplied "over" (later items on top).
    Over,
    /// Additive (glows, light).
    Add,
}

/// Rasterise many small items into `img` (buffer pixels). `bbox` gives an item's pixel bounds
/// `[x0, y0, x1, y1]` (or `None` to skip), `shade` its premultiplied colour at a pixel centre.
/// Items are bucketed into horizontal bands rendered in parallel; within a band they are drawn
/// in slice order.
pub(crate) fn raster<T: Sync>(
    img: &mut Image,
    items: &[T],
    bbox: impl Fn(&T) -> Option<[f32; 4]> + Sync,
    shade: impl Fn(&T, f32, f32) -> Option<Px> + Sync,
    acc: Acc,
) {
    let (w, hgt) = (img.width as usize, img.height as usize);
    if w == 0 || hgt == 0 || items.is_empty() {
        return;
    }
    let nb = hgt.div_ceil(BAND);
    let boxes: Vec<Option<[i32; 4]>> = items
        .par_iter()
        .map(|it| {
            let b = bbox(it)?;
            if !(b[0].is_finite() && b[1].is_finite() && b[2].is_finite() && b[3].is_finite()) {
                return None;
            }
            let x0 = b[0].floor().max(0.0) as i32;
            let y0 = b[1].floor().max(0.0) as i32;
            let x1 = (b[2].ceil() as i32).min(w as i32 - 1);
            let y1 = (b[3].ceil() as i32).min(hgt as i32 - 1);
            if x1 < x0 || y1 < y0 || b[2] < 0.0 || b[3] < 0.0 {
                return None;
            }
            Some([x0, y0, x1, y1])
        })
        .collect();
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (i, b) in boxes.iter().enumerate() {
        if let Some(b) = b {
            for band in (b[1] as usize / BAND)..=(b[3] as usize / BAND).min(nb - 1) {
                buckets[band].push(i as u32);
            }
        }
    }
    img.data.par_chunks_mut(w * BAND).enumerate().for_each(|(band, chunk)| {
        let by0 = band * BAND;
        let rows = chunk.len() / w;
        for &i in &buckets[band] {
            let Some(b) = boxes[i as usize] else { continue };
            let it = &items[i as usize];
            let ya = (b[1] as usize).max(by0);
            let yb = (b[3] as usize).min(by0 + rows - 1);
            for y in ya..=yb {
                let row = &mut chunk[(y - by0) * w..(y - by0 + 1) * w];
                for x in b[0] as usize..=b[2] as usize {
                    if let Some(c) = shade(it, x as f32 + 0.5, y as f32 + 0.5) {
                        let d = &mut row[x];
                        match acc {
                            Acc::Over => {
                                let k = 1.0 - c[3];
                                *d = [c[0] + d[0] * k, c[1] + d[1] * k, c[2] + d[2] * k, c[3] + d[3] * k];
                            }
                            Acc::Add => {
                                *d = [d[0] + c[0], d[1] + c[1], d[2] + c[2], (d[3] + c[3]).min(1.0)];
                            }
                        }
                    }
                }
            }
        }
    });
}

// ------------------------------------------------------------------ sprites

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Shape {
    /// Soft round dot (Gaussian-like falloff).
    Soft,
    /// Hard anti-aliased disc.
    Disc,
    /// Lit sphere (upper-left light, specular).
    Sphere,
    /// Disc fading linearly to its rim.
    Faded,
    /// Thin ring with a highlight.
    Bubble,
    /// Four-point star.
    Star,
    /// Rotated square (angle in `rot`).
    Square,
    /// Triangle (angle in `rot`).
    Tri,
    /// Segment from (x, y) by (dx, dy), half-width `r`.
    Line { dx: f32, dy: f32 },
}

/// A particle sprite in buffer pixels; `c` is straight colour + opacity.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub c: [f32; 4],
    pub shape: Shape,
    pub rot: f32,
}

impl Sprite {
    pub(crate) fn new(x: f32, y: f32, r: f32, c: [f32; 4], shape: Shape) -> Sprite {
        Sprite { x, y, r, c, shape, rot: 0.0 }
    }
}

/// Where a sprite's colour comes from when it depends on the layer's pixels (resolved by
/// [`SpritePlan::resolve`] on the CPU, per pixel by the GPU compositor). Positions are buffer
/// pixels.
#[derive(Clone, Copy, Debug)]
pub enum Tint {
    /// The sprite's own colour.
    Fixed,
    /// CC Rainfall's refracting drops: where the layer shows (alpha > 0), colour × (layer
    /// colour × 0.6 + 0.5) sampled at (x, y).
    Refract { x: f64, y: f64 },
    /// CC Star Burst: the layer's colour at (x, y), opacity × its alpha (none where clear).
    Source { x: f64, y: f64 },
    /// CC Hair: the root's colour at (x, y) mixed with the hair colour (`inherit` = 1 keeps the
    /// root's), × `shade`, + `spec`; no strand where the root's alpha is below ½.
    Root { x: f64, y: f64, hair: [f32; 3], inherit: f32, shade: f32, spec: f32 },
}

/// How a sprite layer meets the original layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Post {
    /// `combine` with this mode (0 Composite … 5 sprites only).
    Combine(u32),
    /// The sprites alone.
    Replace,
    /// The sprites, mixed toward the original by this fraction.
    Lerp(f32),
    /// The sprites over opaque black.
    OverBlack,
}

/// What a sprite-drawing effect draws this frame: built on the CPU (closed-form or from the
/// simulation state), rasterised by [`SpritePlan::finish`] or the GPU compositor.
#[derive(Clone, Debug)]
pub struct SpritePlan {
    pub sprites: Vec<Sprite>,
    /// Per sprite (empty = all [`Tint::Fixed`]).
    pub tints: Vec<Tint>,
    pub acc: Acc,
    pub post: Post,
}

impl SpritePlan {
    /// Resolve layer-dependent colours against the layer `src`.
    pub fn resolve(&mut self, src: &Image) {
        let tints = std::mem::take(&mut self.tints);
        self.sprites.par_iter_mut().zip(tints.par_iter()).for_each(|(sp, t)| match *t {
            Tint::Fixed => {}
            Tint::Refract { x, y } => {
                let (sc, sa) = unpremul(src.sample_bilinear(x, y));
                if sa > 0.0 {
                    let c = sp.c;
                    sp.c = [(sc[0] * 0.6 + 0.5) * c[0], (sc[1] * 0.6 + 0.5) * c[1], (sc[2] * 0.6 + 0.5) * c[2], c[3]];
                }
            }
            Tint::Source { x, y } => {
                let (c, a) = unpremul(src.sample_bilinear(x, y));
                sp.c = if a <= 0.0 { [0.0; 4] } else { [c[0], c[1], c[2], a * sp.c[3]] };
            }
            Tint::Root { x, y, hair, inherit, shade, spec } => {
                let root = src.sample_bilinear(x, y);
                if root[3] < 0.5 {
                    sp.c = [0.0; 4];
                    return;
                }
                let (rc, _) = unpremul(root);
                let base: [f32; 3] = std::array::from_fn(|k| rc[k] + (hair[k] - rc[k]) * (1.0 - inherit));
                sp.c = [base[0] * shade + spec, base[1] * shade + spec, base[2] * shade + spec, sp.c[3]];
            }
        });
    }

    /// Rasterise (after [`SpritePlan::resolve`]) and meet the original layer `b`.
    pub fn finish(&self, b: Buf) -> Buf {
        let fx = splat(b.img.width, b.img.height, &self.sprites, self.acc);
        self.finish_with(b, fx)
    }

    /// [`finish`](Self::finish) with a prepared sprite image (the motion-blurred average of
    /// several frames, #1195).
    pub fn finish_with(&self, b: Buf, fx: Image) -> Buf {
        self.post.apply(b, fx)
    }
}

impl Post {
    /// Meet the sprite image `fx` with the original layer `b` (see [`SpritePlan::finish`]).
    pub(super) fn apply(self, mut b: Buf, fx: Image) -> Buf {
        b.img = match self {
            Post::Combine(m) => combine(&b.img, &fx, m),
            Post::Replace => fx,
            Post::Lerp(t) => {
                let mut o = fx;
                o.data.par_iter_mut().zip(b.img.data.par_iter()).for_each(|(o, s0)| *o = crate::util::lerp4(*o, *s0, t));
                o
            }
            Post::OverBlack => combine(&Image::filled(b.img.width, b.img.height, [0.0, 0.0, 0.0, 1.0]), &fx, 0),
        };
        b
    }
}

fn sprite_bbox(s: &Sprite) -> Option<[f32; 4]> {
    if s.c[3] <= 1e-5 || s.r <= 0.0 {
        return None;
    }
    let r = s.r.max(0.5) + 1.0;
    Some(match s.shape {
        Shape::Line { dx, dy } => [s.x.min(s.x + dx) - r, s.y.min(s.y + dy) - r, s.x.max(s.x + dx) + r, s.y.max(s.y + dy) + r],
        Shape::Star => [s.x - r * 2.0, s.y - r * 2.0, s.x + r * 2.0, s.y + r * 2.0],
        _ => [s.x - r, s.y - r, s.x + r, s.y + r],
    })
}

#[inline]
pub(super) fn aa(edge: f32) -> f32 {
    (edge + 0.5).clamp(0.0, 1.0)
}

fn sprite_shade(s: &Sprite, x: f32, y: f32) -> Option<Px> {
    // Sub-pixel sprites keep their energy: draw at radius 0.5 with reduced opacity.
    let (r, fade) = if s.r < 0.5 { (0.5, s.r / 0.5) } else { (s.r, 1.0) };
    let (dx, dy) = (x - s.x, y - s.y);
    let mut rgb = [s.c[0], s.c[1], s.c[2]];
    let cov = match s.shape {
        Shape::Soft => {
            let d2 = (dx * dx + dy * dy) / (r * r);
            if d2 >= 1.0 {
                return None;
            }
            let q = 1.0 - d2;
            q * q
        }
        Shape::Disc => aa(r - (dx * dx + dy * dy).sqrt()),
        Shape::Faded => {
            let d = (dx * dx + dy * dy).sqrt() / r;
            (1.0 - d).max(0.0).powf(1.5)
        }
        Shape::Sphere => {
            let d = (dx * dx + dy * dy).sqrt();
            let cv = aa(r - d);
            if cv <= 0.0 {
                return None;
            }
            let (nx, ny) = (dx / r, dy / r);
            let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
            let (lx, ly, lz) = (-0.48, -0.58, 0.66);
            let diff = (nx * lx + ny * ly + nz * lz).max(0.0);
            let rz = 2.0 * diff * nz - lz;
            let spec = rz.max(0.0).powi(20) * 0.6;
            let k = 0.2 + 0.8 * diff;
            rgb = [rgb[0] * k + spec, rgb[1] * k + spec, rgb[2] * k + spec];
            cv
        }
        Shape::Bubble => {
            let d = (dx * dx + dy * dy).sqrt() / r;
            if d > 1.0 + 1.0 / r {
                return None;
            }
            let ring = (1.0 - ((d - 0.9) / 0.12).powi(2)).max(0.0);
            let hl = (1.0 - (((dx / r) + 0.4).powi(2) + ((dy / r) + 0.4).powi(2)) / 0.04).max(0.0);
            (0.12 * (d <= 1.0) as u8 as f32 + ring * 0.8 + hl).min(1.0)
        }
        Shape::Star => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = ((dx * cs + dy * sn) / r, (-dx * sn + dy * cs) / r);
            let d = (ux * ux + uy * uy).sqrt();
            let core = (1.0 - d).max(0.0).powi(2);
            let sx = (1.0 - ux.abs() / 2.0).max(0.0) * (1.0 - (uy.abs() * 6.0)).max(0.0);
            let sy = (1.0 - uy.abs() / 2.0).max(0.0) * (1.0 - (ux.abs() * 6.0)).max(0.0);
            (core + sx + sy).min(1.0)
        }
        Shape::Square => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            let e = r * 0.75;
            aa(e - ux.abs()) * aa(e - uy.abs())
        }
        Shape::Tri => {
            let (sn, cs) = s.rot.sin_cos();
            let (ux, uy) = (dx * cs + dy * sn, -dx * sn + dy * cs);
            // Equilateral triangle with circumradius r: three half-planes.
            let e = r * 0.5;
            let mut m = f32::INFINITY;
            for k in 0..3 {
                let a = std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::TAU / 3.0;
                let (ns, nc) = a.sin_cos();
                m = m.min(e - (ux * nc + uy * ns));
            }
            aa(m)
        }
        Shape::Line { dx: lx, dy: ly } => {
            let l2 = lx * lx + ly * ly;
            let t = if l2 > 0.0 { ((dx * lx + dy * ly) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (dx - lx * t, dy - ly * t);
            aa(r - (px * px + py * py).sqrt())
        }
    };
    let a = cov * s.c[3] * fade;
    if a <= 1e-6 {
        return None;
    }
    Some([rgb[0] * a, rgb[1] * a, rgb[2] * a, a])
}

/// Draw sprites into a new transparent image of `w`×`h`.
pub(crate) fn splat(w: u32, hgt: u32, sprites: &[Sprite], acc: Acc) -> Image {
    let mut img = Image::new(w, hgt);
    raster(&mut img, sprites, sprite_bbox, sprite_shade, acc);
    img
}

/// Combine an effect layer `fx` with the original `src` (both premultiplied, same size):
/// 0 Composite (fx over src), 1 Screen, 2 Add, 3 Lighten, 4 Darken, 5 fx only.
pub(crate) fn combine(src: &Image, fx: &Image, mode: u32) -> Image {
    let mut out = fx.clone();
    out.data.par_iter_mut().zip(src.data.par_iter()).for_each(|(o, s)| {
        let f = *o;
        *o = match mode {
            0 => {
                let k = 1.0 - f[3];
                [f[0] + s[0] * k, f[1] + s[1] * k, f[2] + s[2] * k, f[3] + s[3] * k]
            }
            1 => [s[0] + f[0] - s[0] * f[0], s[1] + f[1] - s[1] * f[1], s[2] + f[2] - s[2] * f[2], s[3] + f[3] - s[3] * f[3]],
            2 => [s[0] + f[0], s[1] + f[1], s[2] + f[2], (s[3] + f[3]).min(1.0)],
            3 => [s[0].max(f[0]), s[1].max(f[1]), s[2].max(f[2]), s[3] + f[3] - s[3] * f[3]],
            4 => {
                let (fc, fa) = unpremul(f);
                let m = |c: f32, k: usize| c - (c - c.min(fc[k] * s[3])) * fa;
                [m(s[0], 0), m(s[1], 1), m(s[2], 2), s[3]]
            }
            _ => f,
        };
    });
    out
}
