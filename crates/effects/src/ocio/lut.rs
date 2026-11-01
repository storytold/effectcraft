use super::cdl::{Cdl, parse_cdl_xml};
use super::{Arc, HashMap, Lut, Mutex, OnceLock, parse_cube};

// ---------------------------------------------------------------- LUT files

/// A loaded transform file.
#[derive(Clone, Debug)]
pub enum FileXform {
    /// A lattice/1D LUT with an optional per-channel piecewise-linear shaper (cineSpace pre-LUT).
    Lut { lut: Lut, shaper: Option<[Vec<(f32, f32)>; 3]> },
    /// CDL corrections by id (in file order).
    Cdl(Vec<(String, Cdl)>),
    /// A matrix with offset (Sony Imageworks `.spimtx`), as a MatrixTransform's.
    Matrix { m: [f64; 16], offset: [f64; 4] },
}

/// Parse a Sony Imageworks `.spimtx` matrix: three rows of four numbers, three matrix columns
/// and an offset in 16-bit code values (÷ 65535).
pub fn parse_spimtx(text: &str) -> Option<FileXform> {
    let v: Vec<f64> = text.split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    if v.len() != 12 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let mut m = [0.0; 16];
    let mut offset = [0.0; 4];
    for (r, row) in v.as_chunks::<4>().0.iter().enumerate() {
        m[r * 4..r * 4 + 3].copy_from_slice(&row[..3]);
        offset[r] = row[3] / 65535.0;
    }
    m[15] = 1.0;
    Some(FileXform::Matrix { m, offset })
}

/// Parse a Sony Imageworks `.spi1d` 1D LUT: `From <min> <max>`, `Length <n>`, `Components
/// <1 | 3>`, then `{` the values `}`.
pub fn parse_spi1d(text: &str) -> Option<Lut> {
    let (mut from, mut len, mut comps) = ([0.0f32, 1.0], 0usize, 1usize);
    let mut values: Vec<f32> = vec![];
    let mut body = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if body {
            if line.starts_with('}') {
                break;
            }
            for t in line.split_whitespace() {
                values.push(t.parse().ok()?);
            }
            continue;
        }
        let mut t = line.split_whitespace();
        match t.next()? {
            "{" => body = true,
            "From" => from = [t.next()?.parse().ok()?, t.next()?.parse().ok()?],
            "Length" => len = t.next()?.parse().ok()?,
            "Components" => comps = t.next()?.parse().ok()?,
            _ => {}
        }
    }
    // A 1D table is interpolated by index, so cap it like the lattices (2²⁴ entries).
    if !(2..=1 << 24).contains(&len)
        || !matches!(comps, 1 | 3)
        || values.len() != len.checked_mul(comps)?
        || from[1] <= from[0]
        || from.iter().any(|v| !v.is_finite())
    {
        return None;
    }
    let d1 = values.chunks_exact(comps).map(|c| if comps == 3 { [c[0], c[1], c[2]] } else { [c[0]; 3] }).collect();
    Some(Lut { n1: len, d1, n3: 0, d3: vec![], dmin: [from[0]; 3], dmax: [from[1]; 3] })
}

/// Parse a Sony Imageworks `.spi3d` lattice: `SPILUT 1.0`, `3 3`, the sizes, then one
/// `r g b  R G B` line per lattice point (indices, then the colour).
pub fn parse_spi3d(text: &str) -> Option<Lut> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next()?.starts_with("SPILUT") {
        return None;
    }
    lines.next()?;
    let size: Vec<usize> = lines.next()?.split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    let n = *size.first()?;
    if size.len() != 3 || size.iter().any(|s| *s != n) || !(2..=129).contains(&n) {
        return None;
    }
    let mut d3 = vec![[0.0f32; 3]; n * n * n];
    let mut seen = 0usize;
    for line in lines {
        let v: Vec<&str> = line.split_whitespace().collect();
        let [r, g, b, cr, cg, cb] = v.as_slice() else { return None };
        let (r, g, b): (usize, usize, usize) = (r.parse().ok()?, g.parse().ok()?, b.parse().ok()?);
        if r >= n || g >= n || b >= n {
            return None;
        }
        // Our lattice is red fastest.
        *d3.get_mut(r + g * n + b * n * n)? = [cr.parse().ok()?, cg.parse().ok()?, cb.parse().ok()?];
        seen += 1;
    }
    (seen == n * n * n).then_some(Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] })
}

/// Parse an Autodesk `.3dl` lattice (integers, blue fastest; an optional input-mesh line).
pub fn parse_3dl(text: &str) -> Option<Lut> {
    let mut mesh: Option<Vec<f64>> = None;
    let mut rows: Vec<[f64; 3]> = vec![];
    let mut out_bits: Option<u32> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with('<') {
            continue;
        }
        if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
            // "Mesh <in bits> <out bits>" (Lustre header); other keywords are ignored.
            let t: Vec<&str> = l.split_whitespace().collect();
            if t[0].eq_ignore_ascii_case("mesh") && t.len() >= 3 {
                out_bits = t[2].parse().ok();
            }
            continue;
        }
        let v: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        if v.len() > 3 && mesh.is_none() && rows.is_empty() {
            mesh = Some(v);
        } else if v.len() == 3 {
            rows.push([v[0], v[1], v[2]]);
        } else {
            return None;
        }
    }
    let is_cube = |k: usize| {
        let n = (k as f64).cbrt().round() as usize;
        n * n * n == k
    };
    // A 3-entry input mesh (3³ lattice) looks like a data row: tell them apart by the count.
    if mesh.is_none() && !is_cube(rows.len()) && rows.len() == 28 {
        mesh = Some(rows.remove(0).to_vec());
    }
    let n = match &mesh {
        Some(m) => m.len(),
        None => (rows.len() as f64).cbrt().round() as usize,
    };
    if n < 2 || rows.len() != n * n * n {
        return None;
    }
    let maxv = rows.iter().flat_map(|r| r.iter()).fold(0.0f64, |m, &v| m.max(v));
    let out_max = match out_bits {
        Some(b) if b > 0 && b < 32 => (1u64 << b) as f64 - 1.0,
        _ => [255.0, 1023.0, 4095.0, 65535.0].into_iter().find(|&m| maxv <= m).unwrap_or(maxv.max(1.0)),
    };
    // .3dl rows: red slowest, blue fastest. Our lattice is red fastest.
    let mut d3 = vec![[0.0f32; 3]; n * n * n];
    for (k, r) in rows.iter().enumerate() {
        let (ri, gi, bi) = (k / (n * n), (k / n) % n, k % n);
        d3[ri + gi * n + bi * n * n] = r.map(|v| (v / out_max) as f32);
    }
    Some(Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] })
}

/// Parse a cineSpace `.csp` LUT (1D or 3D, with its three pre-LUT shapers).
pub fn parse_csp(text: &str) -> Option<(Lut, [Vec<(f32, f32)>; 3])> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if !lines.next()?.starts_with("CSPLUTV100") {
        return None;
    }
    let kind = lines.next()?;
    let mut lines = lines.peekable();
    if lines.peek().is_some_and(|l| l.starts_with("BEGIN METADATA")) {
        for l in lines.by_ref() {
            if l.starts_with("END METADATA") {
                break;
            }
        }
    }
    let nums = |l: &str| -> Option<Vec<f32>> { l.split_whitespace().map(|t| t.parse().ok()).collect() };
    let mut shaper: [Vec<(f32, f32)>; 3] = Default::default();
    for ch in shaper.iter_mut() {
        let n: usize = lines.next()?.parse().ok()?;
        let ins = nums(lines.next()?)?;
        let outs = nums(lines.next()?)?;
        if ins.len() != n || outs.len() != n || n < 2 {
            return None;
        }
        *ch = ins.into_iter().zip(outs).collect();
    }
    if kind.starts_with("3D") {
        let sz = nums(lines.next()?)?;
        if sz.len() != 3 || sz[0] != sz[1] || sz[1] != sz[2] {
            return None;
        }
        let n = sz[0] as usize;
        let mut d3 = Vec::with_capacity(n * n * n);
        for _ in 0..n * n * n {
            let v = nums(lines.next()?)?;
            if v.len() != 3 {
                return None;
            }
            d3.push([v[0], v[1], v[2]]);
        }
        (n >= 2).then_some((Lut { n1: 0, d1: vec![], n3: n, d3, dmin: [0.0; 3], dmax: [1.0; 3] }, shaper))
    } else {
        let n: usize = lines.next()?.parse().ok()?;
        let mut d1 = Vec::with_capacity(n);
        for _ in 0..n {
            let v = nums(lines.next()?)?;
            if v.len() != 3 {
                return None;
            }
            d1.push([v[0], v[1], v[2]]);
        }
        (n >= 2).then_some((Lut { n1: n, d1, n3: 0, d3: vec![], dmin: [0.0; 3], dmax: [1.0; 3] }, shaper))
    }
}

fn shape(curve: &[(f32, f32)], x: f32) -> f32 {
    if curve.len() < 2 {
        return x;
    }
    let i = curve.partition_point(|p| p.0 < x).clamp(1, curve.len() - 1);
    let (a, b) = (curve[i - 1], curve[i]);
    let t = ((x - a.0) / (b.0 - a.0).max(1e-12)).clamp(0.0, 1.0);
    a.1 + (b.1 - a.1) * t
}

fn shape_inv(curve: &[(f32, f32)], y: f32) -> f32 {
    let flipped: Vec<(f32, f32)> = curve.iter().map(|&(a, b)| (b, a)).collect();
    shape(&flipped, y)
}

/// Parse transform text by its format (detected from `ext` or the content).
pub fn parse_file(text: &str, ext: &str) -> Option<FileXform> {
    let ext = ext.to_ascii_lowercase();
    if ext == "cc" || ext == "ccc" || ext == "cdl" || text.contains("<ColorCorrection") {
        let v = parse_cdl_xml(text);
        return (!v.is_empty()).then_some(FileXform::Cdl(v));
    }
    if ext == "csp" || text.trim_start().starts_with("CSPLUTV100") {
        return parse_csp(text).map(|(lut, s)| FileXform::Lut { lut, shaper: Some(s) });
    }
    if ext == "spimtx" {
        return parse_spimtx(text);
    }
    if ext == "spi1d" || text.trim_start().starts_with("Version") {
        return parse_spi1d(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    if ext == "spi3d" || text.trim_start().starts_with("SPILUT") {
        return parse_spi3d(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    if ext == "cube" || text.contains("LUT_3D_SIZE") || text.contains("LUT_1D_SIZE") {
        return parse_cube(text).map(|lut| FileXform::Lut { lut, shaper: None });
    }
    parse_3dl(text).map(|lut| FileXform::Lut { lut, shaper: None })
}

type FileCache = Mutex<HashMap<String, Option<Arc<FileXform>>>>;

/// Load (and cache) a transform from a file path or inline content.
pub fn load_file(src: &str) -> Option<Arc<FileXform>> {
    let src_t = src.trim();
    if src_t.is_empty() {
        return None;
    }
    static CACHE: OnceLock<FileCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(m) = cache.lock()
        && let Some(v) = m.get(src)
    {
        return v.clone();
    }
    let inline = src.contains('\n');
    let parsed = if inline {
        parse_file(src, "")
    } else {
        let ext = std::path::Path::new(src_t).extension().and_then(std::ffi::OsStr::to_str).unwrap_or("");
        std::fs::read_to_string(src_t).ok().and_then(|t| parse_file(&t, ext))
    }
    .map(Arc::new);
    if let Ok(mut m) = cache.lock() {
        m.insert(src.to_string(), parsed.clone());
    }
    parsed
}

impl FileXform {
    pub fn apply(&self, c: [f32; 3], interp: u32, inverse: bool, ccc_id: &str) -> [f32; 3] {
        match self {
            FileXform::Cdl(list) => {
                let cdl = list.iter().find(|(id, _)| !ccc_id.is_empty() && id == ccc_id).or(list.first()).map(|(_, c)| *c).unwrap_or_default();
                let c64 = c.map(|v| v as f64);
                let o = if inverse { cdl.invert(c64, true) } else { cdl.apply(c64, true) };
                o.map(|v| v as f32)
            }
            FileXform::Matrix { m, offset } => crate::ocio_config::matrix_apply(m, offset, c.map(|v| v as f64), inverse).map(|v| v as f32),
            FileXform::Lut { lut, shaper } => {
                let fwd = |c: [f32; 3]| {
                    let c = match shaper {
                        Some(s) => [shape(&s[0], c[0]), shape(&s[1], c[1]), shape(&s[2], c[2])],
                        None => c,
                    };
                    lut.apply_interp(c, interp)
                };
                if !inverse {
                    return fwd(c);
                }
                if lut.n3 == 0 {
                    // 1D: invert each channel exactly through the (monotone) tables.
                    let mut x = c;
                    if lut.n1 > 1 {
                        x = [0, 1, 2].map(|i| {
                            let curve: Vec<(f32, f32)> =
                                (0..lut.n1).map(|k| (lut.d1[k][i], lut.dmin[i] + (lut.dmax[i] - lut.dmin[i]) * k as f32 / (lut.n1 - 1) as f32)).collect();
                            shape(&curve, c[i])
                        });
                    }
                    if let Some(s) = shaper {
                        x = [shape_inv(&s[0], x[0]), shape_inv(&s[1], x[1]), shape_inv(&s[2], x[2])];
                    }
                    return x;
                }
                // 3D: damped fixed-point (Newton with an identity Jacobian) from the target.
                let mut x = c;
                for _ in 0..40 {
                    let y = fwd(x);
                    let e = [c[0] - y[0], c[1] - y[1], c[2] - y[2]];
                    if e.iter().all(|v| v.abs() < 1e-6) {
                        break;
                    }
                    x = [x[0] + e[0], x[1] + e[1], x[2] + e[2]];
                }
                x
            }
        }
    }
}
