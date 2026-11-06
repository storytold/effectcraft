//! Shadings the render tree has no gradient for (ISO 32000-1 §8.7.4.5): function-based
//! shadings (type 1) and the mesh shadings — free-form and lattice-form Gouraud-shaded triangle
//! meshes (types 4 and 5), Coons patch meshes (type 6) and tensor-product patch meshes (type 7).
//! Patches are tessellated into triangles; triangles are rasterised with their colours (or the
//! parametric `t` of a shading `Function`) interpolated across them, into an image node in page
//! space (2×2 supersampled, so the mesh's outer edges are anti-aliased without seams between
//! triangles).

use effectcraft_svg::{Affine, Image as SvgImage};
use kurbo::Point;

use crate::color::{Cs, Func, color_space};
use crate::object::{Dict, File, Obj, decode_stream};

/// The longest side of a rasterised shading, in image pixels.
const MAX_SIDE: f64 = 1024.0;
/// The most image pixels per PDF point.
const MAX_SCALE: f64 = 4.0;
/// The most triangles one shading draws.
const MAX_TRIANGLES: usize = 2_000_000;
/// How many times over the mesh raster may be covered by triangle bounding boxes in all.
const MAX_RASTER_PASSES: usize = 64;

/// A mesh vertex: page-space position and its value (RGB, or `t` in `[0]` with a function).
#[derive(Clone, Copy, Debug, PartialEq)]
struct V {
    p: Point,
    c: [f64; 3],
}

/// How vertex values become colours.
struct Colors {
    cs: Cs,
    /// With a function: `t` (domain `[t0, t1]`) → RGB through a 1024-entry table.
    lut: Option<(f64, f64, Vec<[u8; 3]>)>,
}

impl Colors {
    fn new(file: &File, sh: &Dict) -> (Colors, usize) {
        let cs = file.get(sh, "ColorSpace").map(|c| color_space(file, c, None)).unwrap_or(Cs::Rgb);
        let n = cs.components().max(1);
        match file.get(sh, "Function") {
            Some(fo) => {
                let func = Func::read(file, fo);
                let dom = file.get(sh, "Domain").map(|d| file.nums(d)).filter(|d| d.len() == 2).unwrap_or_else(|| vec![0.0, 1.0]);
                let (t0, t1) = (dom[0], dom[1]);
                let lut = (0..1024)
                    .map(|i| {
                        let t = t0 + (t1 - t0) * i as f64 / 1023.0;
                        to8(cs.to_rgb(&func.eval(&[t])))
                    })
                    .collect();
                (Colors { cs, lut: Some((t0, t1, lut)) }, 1)
            }
            None => (Colors { cs, lut: None }, n),
        }
    }

    /// A vertex value from its components (RGB, or `t`).
    fn value(&self, comps: &[f64]) -> [f64; 3] {
        match self.lut {
            Some(_) => [comps.first().copied().unwrap_or(0.0), 0.0, 0.0],
            None => self.cs.to_rgb(comps),
        }
    }

    /// An interpolated value → RGB8.
    fn rgb(&self, v: [f64; 3]) -> [u8; 3] {
        match &self.lut {
            Some((t0, t1, lut)) => {
                let k = if t1 != t0 { (v[0] - t0) / (t1 - t0) } else { 0.0 };
                let k = if k.is_finite() { k.clamp(0.0, 1.0) } else { 0.0 };
                lut[(k * 1023.0).round() as usize]
            }
            None => to8(v),
        }
    }
}

fn to8(c: [f64; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Big-endian bit reader over mesh data.
struct Bits<'a> {
    data: &'a [u8],
    bit: usize,
}

impl Bits<'_> {
    fn left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bit)
    }

    fn read(&mut self, n: usize) -> Option<u64> {
        if n == 0 || n > 32 || self.left() < n {
            return None;
        }
        let mut v = 0u64;
        for _ in 0..n {
            let byte = self.data[self.bit / 8];
            v = (v << 1) | ((byte >> (7 - self.bit % 8)) & 1) as u64;
            self.bit += 1;
        }
        Some(v)
    }

    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }
}

/// The layout of a mesh's vertex data (§8.7.4.5.5 Table 83 and following).
struct Layout {
    bpc: usize,
    bpcomp: usize,
    bpf: usize,
    /// Value components per vertex (1 with a function).
    n: usize,
    decode: Vec<f64>,
}

impl Layout {
    fn read(file: &File, sh: &Dict, n: usize) -> Option<Layout> {
        let bpc = file.get_num(sh, "BitsPerCoordinate")? as usize;
        let bpcomp = file.get_num(sh, "BitsPerComponent")? as usize;
        let bpf = file.get_num(sh, "BitsPerFlag").unwrap_or(8.0) as usize;
        let decode = file.get(sh, "Decode").map(|d| file.nums(d)).unwrap_or_default();
        let ok = |b: usize| matches!(b, 1 | 2 | 4 | 8 | 12 | 16 | 24 | 32);
        if !ok(bpc) || !ok(bpcomp) || !matches!(bpf, 2 | 4 | 8) || decode.len() < 4 + 2 * n {
            return None;
        }
        Some(Layout { bpc, bpcomp, bpf, n, decode })
    }

    fn scale(raw: u64, bits: usize, lo: f64, hi: f64) -> f64 {
        let max = ((1u64 << bits) - 1) as f64;
        lo + raw as f64 * (hi - lo) / max
    }

    fn point(&self, r: &mut Bits) -> Option<Point> {
        let x = r.read(self.bpc)?;
        let y = r.read(self.bpc)?;
        let d = &self.decode;
        Some(Point::new(Self::scale(x, self.bpc, d[0], d[1]), Self::scale(y, self.bpc, d[2], d[3])))
    }

    fn comps(&self, r: &mut Bits) -> Option<Vec<f64>> {
        (0..self.n)
            .map(|i| {
                let v = r.read(self.bpcomp)?;
                Some(Self::scale(v, self.bpcomp, self.decode[4 + 2 * i], self.decode[5 + 2 * i]))
            })
            .collect()
    }
}

/// A shading rasterised into an image node in page space, for `sh` or a shading-pattern fill.
/// `sh` is the shading dictionary or stream, `to_page` maps shading space to page space and
/// `page_box` limits the image (page space). `None` with a note in `skip` when nothing draws.
pub(crate) fn shading_image(file: &File, sh: &Obj, to_page: Affine, page_box: [f64; 4], skip: &mut Vec<String>) -> Option<SvgImage> {
    let sh_obj = file.resolve(sh);
    let d = sh_obj.dict()?;
    let ty = file.get_num(d, "ShadingType").unwrap_or(0.0) as i32;
    if !to_page.as_coeffs().iter().all(|v| v.is_finite()) || to_page.determinant().abs() < 1e-12 {
        return None;
    }
    let tris = match ty {
        1 => return function_shading(file, d, to_page, page_box, skip),
        4..=7 => {
            let Obj::Stream(_, raw) = sh_obj else {
                skip.push(format!("shading type {ty} (no data)"));
                return None;
            };
            let Some(data) = decode_stream(file, d, raw) else {
                skip.push(format!("shading type {ty} (filters)"));
                return None;
            };
            let (colors, n) = Colors::new(file, d);
            let Some(layout) = Layout::read(file, d, n) else {
                skip.push(format!("shading type {ty} (layout)"));
                return None;
            };
            let mut r = Bits { data: &data, bit: 0 };
            let tris = match ty {
                4 => free_form(&mut r, &layout, &colors, to_page),
                5 => {
                    let per_row = file.get_num(d, "VerticesPerRow").unwrap_or(0.0) as usize;
                    lattice(&mut r, &layout, &colors, to_page, per_row)
                }
                _ => patches(&mut r, &layout, &colors, to_page, ty == 7, page_box),
            };
            (tris, colors)
        }
        _ => {
            skip.push(format!("shading type {ty}"));
            return None;
        }
    };
    let (tris, colors) = tris;
    if tris.is_empty() {
        return None;
    }
    rasterize(&tris, &colors, page_box)
}

/// Type 4: free-form triangles with edge flags (0 starts a triangle; 1 and 2 share an edge with
/// the previous one).
fn free_form(r: &mut Bits, l: &Layout, colors: &Colors, m: Affine) -> Vec<[V; 3]> {
    let mut out = vec![];
    let mut pending: Vec<V> = vec![];
    let mut last: Option<[V; 3]> = None;
    while out.len() < MAX_TRIANGLES {
        let Some(flag) = r.read(l.bpf) else { break };
        let (Some(p), Some(c)) = (l.point(r), l.comps(r)) else { break };
        r.align();
        let v = V { p: m * p, c: colors.value(&c) };
        // A triangle being started takes the next two vertices whatever their flags.
        if !pending.is_empty() || flag == 0 {
            pending.push(v);
            if pending.len() == 3 {
                let t = [pending[0], pending[1], pending[2]];
                out.push(t);
                last = Some(t);
                pending.clear();
            }
            continue;
        }
        let Some([a, b, c]) = last else { continue };
        let t = if flag == 1 { [b, c, v] } else { [a, c, v] };
        out.push(t);
        last = Some(t);
    }
    out
}

/// Type 5: a lattice of `per_row` vertices per row, two triangles per cell.
fn lattice(r: &mut Bits, l: &Layout, colors: &Colors, m: Affine, per_row: usize) -> Vec<[V; 3]> {
    if !(2..=MAX_TRIANGLES).contains(&per_row) {
        return vec![];
    }
    let mut rows: Vec<Vec<V>> = vec![];
    let mut row = vec![];
    while let (Some(p), Some(c)) = (l.point(r), l.comps(r)) {
        r.align();
        // Vertices kept, the unfinished row's included, stay within the triangle limit.
        if (rows.len() * per_row).saturating_add(row.len()) >= MAX_TRIANGLES {
            break;
        }
        row.push(V { p: m * p, c: colors.value(&c) });
        if row.len() == per_row {
            rows.push(std::mem::take(&mut row));
            if rows.len() * per_row > MAX_TRIANGLES {
                break;
            }
        }
    }
    let mut out = vec![];
    for w in rows.windows(2) {
        for i in 0..per_row - 1 {
            let (a, b, c, d) = (w[0][i], w[0][i + 1], w[1][i], w[1][i + 1]);
            out.push([a, b, c]);
            out.push([b, d, c]);
        }
    }
    out
}

fn bez(p: [Point; 4], t: f64) -> Point {
    let s = 1.0 - t;
    let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
    Point::new(a * p[0].x + b * p[1].x + c * p[2].x + d * p[3].x, a * p[0].y + b * p[1].y + c * p[2].y + d * p[3].y)
}

/// Types 6 and 7: Coons and tensor-product patches. The 12 boundary points run around the
/// patch from its first corner (`p00 p01 p02 p03 p13 p23 p33 p32 p31 p30 p20 p10`); tensor
/// patches add `p11 p12 p22 p21`. Corner colours belong to `p00`, `p03`, `p33` and `p30`.
/// Flags 1–3 reuse the previous patch's second, third or fourth side as the first.
fn patches(r: &mut Bits, l: &Layout, colors: &Colors, m: Affine, tensor: bool, page_box: [f64; 4]) -> Vec<[V; 3]> {
    let mut out = vec![];
    let mut prev: Option<([Point; 12], [[f64; 3]; 4])> = None;
    while out.len() < MAX_TRIANGLES {
        let Some(flag) = r.read(l.bpf) else { break };
        let mut lp = [Point::ZERO; 12];
        let mut cs = [[0.0; 3]; 4];
        let first = match (flag, &prev) {
            (0, _) => 0,
            (1..=3, Some((pp, pc))) => {
                let (k, ci) = match flag {
                    1 => (3, 1),
                    2 => (6, 2),
                    _ => (9, 3),
                };
                for i in 0..4 {
                    lp[i] = pp[(k + i) % 12];
                }
                cs[0] = pc[ci];
                cs[1] = pc[(ci + 1) % 4];
                4
            }
            _ => break,
        };
        let mut ok = true;
        for p in lp.iter_mut().skip(first) {
            match l.point(r) {
                Some(q) => *p = q,
                None => ok = false,
            }
        }
        let mut inner = [Point::ZERO; 4];
        if tensor {
            for p in &mut inner {
                match l.point(r) {
                    Some(q) => *p = q,
                    None => ok = false,
                }
            }
        }
        for c in cs.iter_mut().skip(if first == 0 { 0 } else { 2 }) {
            match l.comps(r) {
                Some(v) => *c = colors.value(&v),
                None => ok = false,
            }
        }
        r.align();
        if !ok {
            break;
        }
        // Shading space → page space before tessellating (the rasteriser works in page space).
        let lp_page = lp.map(|p| m * p);
        let inner_page = inner.map(|p| m * p);
        // The 4×4 grid g[i][j]: row i = 0 is p00 … p03.
        let mut g = [[Point::ZERO; 4]; 4];
        g[0] = [lp_page[0], lp_page[1], lp_page[2], lp_page[3]];
        g[1][3] = lp_page[4];
        g[2][3] = lp_page[5];
        g[3] = [lp_page[9], lp_page[8], lp_page[7], lp_page[6]];
        g[2][0] = lp_page[10];
        g[1][0] = lp_page[11];
        g[1][1] = inner_page[0];
        g[1][2] = inner_page[1];
        g[2][2] = inner_page[2];
        g[2][1] = inner_page[3];
        tessellate(&g, tensor, cs, page_box, &mut out);
        prev = Some((lp, cs));
    }
    out
}

/// A patch → `n`×`n` cells of two triangles; `u` runs along `p00 → p03`, `v` along `p00 → p30`.
fn tessellate(g: &[[Point; 4]; 4], tensor: bool, cs: [[f64; 3]; 4], page_box: [f64; 4], out: &mut Vec<[V; 3]>) {
    let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
    for p in g.iter().flatten() {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    if !(lo.x.is_finite() && hi.x.is_finite() && lo.y.is_finite() && hi.y.is_finite()) {
        return;
    }
    // Patches entirely off the page draw nothing.
    if hi.x < page_box[0] || lo.x > page_box[2] || hi.y < page_box[1] || lo.y > page_box[3] {
        return;
    }
    let size = (hi.x - lo.x).max(hi.y - lo.y).min(page_box[2] - page_box[0] + page_box[3] - page_box[1]);
    let n = ((size * MAX_SCALE / 6.0).ceil() as usize).clamp(2, 48);
    let eval = |u: f64, v: f64| -> Point {
        if tensor {
            let rows = g.map(|row| bez(row, u));
            bez(rows, v)
        } else {
            let cb = bez(g[0], u);
            let ct = bez(g[3], u);
            let cl = bez([g[0][0], g[1][0], g[2][0], g[3][0]], v);
            let cr = bez([g[0][3], g[1][3], g[2][3], g[3][3]], v);
            let corner = |a: Point, w: f64| a.to_vec2() * w;
            let s = cb.to_vec2() * (1.0 - v) + ct.to_vec2() * v + cl.to_vec2() * (1.0 - u) + cr.to_vec2() * u
                - corner(g[0][0], (1.0 - u) * (1.0 - v))
                - corner(g[0][3], u * (1.0 - v))
                - corner(g[3][0], (1.0 - u) * v)
                - corner(g[3][3], u * v);
            s.to_point()
        }
    };
    let color = |u: f64, v: f64| -> [f64; 3] {
        // c0 (0,0), c1 (1,0), c2 (1,1), c3 (0,1).
        let mut c = [0.0; 3];
        for (k, ck) in c.iter_mut().enumerate() {
            *ck = cs[0][k] * (1.0 - u) * (1.0 - v) + cs[1][k] * u * (1.0 - v) + cs[2][k] * u * v + cs[3][k] * (1.0 - u) * v;
        }
        c
    };
    let mut grid = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
            grid.push(V { p: eval(u, v), c: color(u, v) });
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = grid[j * (n + 1) + i];
            let b = grid[j * (n + 1) + i + 1];
            let c = grid[(j + 1) * (n + 1) + i];
            let d = grid[(j + 1) * (n + 1) + i + 1];
            out.push([a, b, c]);
            out.push([b, d, c]);
        }
    }
}

/// The page-space raster frame for content covering `lo..hi`: origin (top-left, page space),
/// scale (pixels per point) and size, or `None` when it misses the page.
fn frame(lo: Point, hi: Point, page_box: [f64; 4]) -> Option<(Point, f64, usize, usize)> {
    let x0 = lo.x.max(page_box[0]);
    let y0 = lo.y.max(page_box[1]);
    let x1 = hi.x.min(page_box[2]);
    let y1 = hi.y.min(page_box[3]);
    let finite = x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite();
    if !finite || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let s = (MAX_SIDE / (x1 - x0).max(y1 - y0)).min(MAX_SCALE);
    let w = ((x1 - x0) * s).ceil().max(1.0) as usize;
    let h = ((y1 - y0) * s).ceil().max(1.0) as usize;
    Some((Point::new(x0, y1), s, w, h))
}

/// The image node for an RGBA8 raster with origin `o` (page space, top-left) and `s` px/pt.
fn image_node(rgba: Vec<u8>, w: usize, h: usize, o: Point, s: f64) -> SvgImage {
    SvgImage {
        name: "Shading".into(),
        transform: Affine::new([1.0 / s, 0.0, 0.0, -1.0 / s, o.x, o.y]),
        width: w as u32,
        height: h as u32,
        rgba: std::sync::Arc::new(rgba),
        opacity: 1.0,
    }
}

/// Gouraud-shade the triangles (page space) into an image: 2×2 samples per pixel, later
/// triangles over earlier ones.
fn rasterize(tris: &[[V; 3]], colors: &Colors, page_box: [f64; 4]) -> Option<SvgImage> {
    let (mut lo, mut hi) = (Point::new(f64::MAX, f64::MAX), Point::new(f64::MIN, f64::MIN));
    for v in tris.iter().flatten() {
        if v.p.x.is_finite() && v.p.y.is_finite() {
            lo = Point::new(lo.x.min(v.p.x), lo.y.min(v.p.y));
            hi = Point::new(hi.x.max(v.p.x), hi.y.max(v.p.y));
        }
    }
    let (o, s, w, h) = frame(lo, hi, page_box)?;
    // Supersampled buffer: value + covered.
    let (sw, sh) = (w * 2, h * 2);
    let ss = s * 2.0;
    let mut val = vec![[0.0f32; 3]; sw * sh];
    let mut cov = vec![false; sw * sh];
    // Samples tested across all triangles: overlapping triangles that each cover the raster
    // would otherwise test it once per triangle. Past the budget the later triangles are left out.
    let mut work = (sw * sh).saturating_mul(MAX_RASTER_PASSES).saturating_add(tris.len().saturating_mul(16));
    for t in tris {
        let q = t.map(|v| Point::new((v.p.x - o.x) * ss, (o.y - v.p.y) * ss));
        let area = (q[1].x - q[0].x) * (q[2].y - q[0].y) - (q[2].x - q[0].x) * (q[1].y - q[0].y);
        if area.abs() < 1e-12 || !area.is_finite() {
            continue;
        }
        let bx0 = q.iter().map(|p| p.x).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let by0 = q.iter().map(|p| p.y).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let bx1 = (q.iter().map(|p| p.x).fold(f64::MIN, f64::max).ceil().max(0.0) as usize).min(sw);
        let by1 = (q.iter().map(|p| p.y).fold(f64::MIN, f64::max).ceil().max(0.0) as usize).min(sh);
        let Some(left) = work.checked_sub(bx1.saturating_sub(bx0).saturating_mul(by1.saturating_sub(by0))) else { break };
        work = left;
        let eps = 1e-9 * area.abs();
        for y in by0..by1 {
            for x in bx0..bx1 {
                let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);
                let w0 = ((q[1].x - p.x) * (q[2].y - p.y) - (q[2].x - p.x) * (q[1].y - p.y)) / area;
                let w1 = ((q[2].x - p.x) * (q[0].y - p.y) - (q[0].x - p.x) * (q[2].y - p.y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < -eps || w1 < -eps || w2 < -eps {
                    continue;
                }
                let i = y * sw + x;
                for k in 0..3 {
                    val[i][k] = (t[0].c[k] * w0 + t[1].c[k] * w1 + t[2].c[k] * w2) as f32;
                }
                cov[i] = true;
            }
        }
    }
    // Resolve: average the covered samples' colours, alpha = covered fraction.
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f64; 3];
            let mut n = 0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = (2 * y + dy) * sw + 2 * x + dx;
                if cov[i] {
                    let c = colors.rgb(val[i].map(f64::from));
                    for k in 0..3 {
                        acc[k] += c[k] as f64;
                    }
                    n += 1;
                }
            }
            if n > 0 {
                let o = (y * w + x) * 4;
                for k in 0..3 {
                    rgba[o + k] = (acc[k] / n as f64).round() as u8;
                }
                rgba[o + 3] = (n * 255 / 4) as u8;
            }
        }
    }
    Some(image_node(rgba, w, h, o, s))
}

/// Type 1: `Function(x, y)` over `Domain` `[x0 x1 y0 y1]`, mapped by `Matrix`.
fn function_shading(file: &File, d: &Dict, to_page: Affine, page_box: [f64; 4], skip: &mut Vec<String>) -> Option<SvgImage> {
    let Some(fo) = file.get(d, "Function") else {
        skip.push("shading type 1 (no function)".into());
        return None;
    };
    let func = Func::read(file, fo);
    if func == Func::Unsupported {
        skip.push("shading function".into());
        return None;
    }
    let cs = file.get(d, "ColorSpace").map(|c| color_space(file, c, None)).unwrap_or(Cs::Rgb);
    let dom = file.get(d, "Domain").map(|x| file.nums(x)).filter(|v| v.len() == 4).unwrap_or_else(|| vec![0.0, 1.0, 0.0, 1.0]);
    let mtx = file.get(d, "Matrix").map(|x| file.nums(x)).filter(|v| v.len() == 6).map(|v| Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]));
    let full = to_page * mtx.unwrap_or(Affine::IDENTITY);
    if full.determinant().abs() < 1e-12 || !full.as_coeffs().iter().all(|v| v.is_finite()) {
        return None;
    }
    let corners = [(dom[0], dom[2]), (dom[1], dom[2]), (dom[1], dom[3]), (dom[0], dom[3])].map(|(x, y)| full * Point::new(x, y));
    let lo = Point::new(corners.iter().map(|p| p.x).fold(f64::MAX, f64::min), corners.iter().map(|p| p.y).fold(f64::MAX, f64::min));
    let hi = Point::new(corners.iter().map(|p| p.x).fold(f64::MIN, f64::max), corners.iter().map(|p| p.y).fold(f64::MIN, f64::max));
    let (o, s, w, h) = frame(lo, hi, page_box)?;
    let inv = full.inverse();
    let (xa, xb) = (dom[0].min(dom[1]), dom[0].max(dom[1]));
    let (ya, yb) = (dom[2].min(dom[3]), dom[2].max(dom[3]));
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            // 2×2 samples: anti-aliased domain edges.
            let mut acc = [0.0f64; 3];
            let mut n = 0;
            for (dx, dy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let page = Point::new(o.x + (x as f64 + dx) / s, o.y - (y as f64 + dy) / s);
                let q = inv * page;
                if q.x < xa - 1e-9 || q.x > xb + 1e-9 || q.y < ya - 1e-9 || q.y > yb + 1e-9 {
                    continue;
                }
                let c = cs.to_rgb(&func.eval(&[q.x, q.y]));
                for k in 0..3 {
                    acc[k] += c[k];
                }
                n += 1;
            }
            if n > 0 {
                let i = (y * w + x) * 4;
                let c = to8(acc.map(|v| v / n as f64));
                rgba[i..i + 3].copy_from_slice(&c);
                rgba[i + 3] = (n * 255 / 4) as u8;
            }
        }
    }
    Some(image_node(rgba, w, h, o, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_reader_and_alignment() {
        let mut r = Bits { data: &[0b1010_0000, 0xFF], bit: 0 };
        assert_eq!(r.read(3), Some(0b101));
        r.align();
        assert_eq!(r.read(8), Some(0xFF));
        assert_eq!(r.read(1), None);
        assert_eq!(r.read(33), None);
    }

    #[test]
    fn bezier_ends() {
        let p = [Point::new(0.0, 0.0), Point::new(1.0, 2.0), Point::new(2.0, 2.0), Point::new(3.0, 0.0)];
        assert_eq!(bez(p, 0.0), p[0]);
        assert_eq!(bez(p, 1.0), p[3]);
    }
}
