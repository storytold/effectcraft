//! STL (stereolithography) meshes, binary and ASCII.
//!
//! Written from the public description of the format (3D Systems' StereoLithography Interface
//! Specification, 1989, and its widely documented colour extensions):
//!
//! - Binary: an 80-byte header, a little-endian `u32` facet count, then 50 bytes per facet: a
//!   normal and three vertices (12 `f32`) and a 16-bit attribute word. Bytes after the last
//!   facet are ignored.
//! - ASCII: `solid name`, then `facet normal nx ny nz` / `outer loop` / `vertex x y z`… /
//!   `endloop` / `endfacet`, closed by `endsolid`; several solids may follow each other.
//!   Loops with more than three vertices are fan-triangulated.
//! - Colour (binary only): with `COLOR=r g b a` (four bytes) in the header (Materialise Magics),
//!   that is the part's colour and a facet whose attribute has bit 15 clear has its own colour
//!   (red in bits 0–4, green 5–9, blue 10–14); otherwise (VisCAM / SolidView) a facet with bit 15
//!   set has its own colour (blue in bits 0–4, green 5–9, red 10–14). Each colour becomes a
//!   plain dielectric material; uncoloured files get none (the renderer's default material).
//!
//! STL has no units, no UVs and no shared vertices: every facet keeps its own three vertices
//! with the face normal from its counter-clockwise winding (the stored normal is only used when
//! the triangle is degenerate), which gives the faceted look of an STL part. STL files are
//! Z-up (3D printing and CAD); they are turned to the +Y-up convention of [`crate::Model`]
//! (`(x, y, z)` → `(x, z, −y)`, a proper rotation, so winding is kept). Facets with non-finite
//! coordinates are skipped.

use std::collections::HashMap;

use crate::{Material, Mesh, Model, Primitive, Result, cross, normalize, obj::srgb_to_linear, perr, sub};

/// Header (80) plus facet count (4).
const HEADER: usize = 84;
/// Bytes per binary facet.
const FACET: usize = 50;

/// Parse an STL file (binary or ASCII, detected from the contents).
pub fn parse(bytes: &[u8]) -> Result<Model> {
    let count = bytes.get(80..HEADER).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes);
    let needed = count.and_then(|n| (n as usize).checked_mul(FACET)).and_then(|n| n.checked_add(HEADER));
    // A binary file's size is exact; many binary headers still begin with `solid`, so the size
    // decides first, then the ASCII keyword.
    if needed.is_some_and(|n| n == bytes.len()) {
        return binary(bytes);
    }
    if looks_ascii(bytes) {
        return ascii(bytes);
    }
    if needed.is_some_and(|n| n <= bytes.len()) {
        return binary(bytes);
    }
    Err(perr("not an STL file (truncated binary facets or no `solid` keyword)"))
}

fn looks_ascii(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(512)).unwrap_or(&[]);
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start();
    t.get(..5).is_some_and(|k| k.eq_ignore_ascii_case("solid")) && (t.contains("facet") || t.contains("endsolid"))
}

/// The Z-up → Y-up turn.
fn y_up(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

/// Facets grouped into primitives by colour, in order of first use.
#[derive(Default)]
struct Builder {
    /// (colour key, primitive); `None` = uncoloured.
    prims: Vec<(Option<[u8; 3]>, Primitive)>,
    /// Colour → index into `prims` (up to 32 768 colours, so not a linear search).
    slots: HashMap<Option<[u8; 3]>, usize>,
    facets: usize,
}

impl Builder {
    /// Add one triangle (file coordinates); `stored` is the file's facet normal.
    fn tri(&mut self, color: Option<[u8; 3]>, stored: [f32; 3], v: [[f32; 3]; 3]) {
        if v.iter().flatten().any(|c| !c.is_finite()) {
            return;
        }
        let v = v.map(y_up);
        let n = normalize(cross(sub(v[1], v[0]), sub(v[2], v[0])))
            .or_else(|| stored.iter().all(|c| c.is_finite()).then(|| normalize(y_up(stored))).flatten())
            .unwrap_or([0.0, 1.0, 0.0]);
        let slot = *self.slots.entry(color).or_insert_with(|| {
            self.prims.push((color, Primitive::default()));
            self.prims.len() - 1
        });
        let Some((_, p)) = self.prims.get_mut(slot) else { return };
        for c in v {
            p.indices.push(p.positions.len() as u32);
            p.positions.push(c);
            p.normals.push(n);
        }
        self.facets += 1;
    }

    fn finish(self, name: String) -> Result<Model> {
        if self.facets == 0 {
            return Err(perr("STL file without facets"));
        }
        let mut materials = vec![];
        let mut mesh = Mesh { name, primitives: vec![] };
        for (color, mut p) in self.prims {
            if let Some(c) = color {
                let lin = c.map(|x| srgb_to_linear(x as f32 / 255.0));
                let name = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
                materials.push(Material { name, ..Material::plain([lin[0], lin[1], lin[2], 1.0], 0.0, 0.5) });
                p.material = Some(materials.len() - 1);
            }
            mesh.primitives.push(p);
        }
        // Uncoloured facets next to coloured ones: a neutral grey of their own.
        if !materials.is_empty() {
            for p in mesh.primitives.iter_mut().filter(|p| p.material.is_none()) {
                materials.push(Material { name: "STL".into(), ..Material::plain([0.6, 0.6, 0.6, 1.0], 0.0, 0.5) });
                p.material = Some(materials.len() - 1);
            }
        }
        Ok(Model::single(mesh, materials))
    }
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    b.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map_or(f32::NAN, f32::from_le_bytes)
}

/// A 5-bit channel expanded to 8 bits.
fn five(x: u16) -> u8 {
    let x = (x & 31) as u8;
    (x << 3) | (x >> 2)
}

/// The Magics `COLOR=` part colour in the header (RGB; alpha is ignored).
fn header_color(header: &[u8]) -> Option<[u8; 3]> {
    let at = header.windows(6).position(|w| w == b"COLOR=")?;
    let c = header.get(at + 6..at + 10)?;
    Some([*c.first()?, *c.get(1)?, *c.get(2)?])
}

fn binary(bytes: &[u8]) -> Result<Model> {
    let header = bytes.get(..80).ok_or_else(|| perr("STL header truncated"))?;
    let magics = header_color(header);
    let body = bytes.get(HEADER..).unwrap_or(&[]);
    let count = bytes.get(80..HEADER).and_then(|b| <[u8; 4]>::try_from(b).ok()).map_or(0, u32::from_le_bytes) as usize;
    let mut b = Builder::default();
    for f in body.as_chunks::<FACET>().0.iter().take(count) {
        let n = [f32_at(f, 0), f32_at(f, 4), f32_at(f, 8)];
        let v = [0usize, 1, 2].map(|k| [f32_at(f, 12 + 12 * k), f32_at(f, 16 + 12 * k), f32_at(f, 20 + 12 * k)]);
        let attr = f.get(48..50).and_then(|a| <[u8; 2]>::try_from(a).ok()).map_or(0, u16::from_le_bytes);
        let color = match magics {
            Some(part) => Some(if attr & 0x8000 == 0 { [five(attr), five(attr >> 5), five(attr >> 10)] } else { part }),
            None => (attr & 0x8000 != 0).then(|| [five(attr >> 10), five(attr >> 5), five(attr)]),
        };
        b.tri(color, n, v);
    }
    let text = String::from_utf8_lossy(header);
    let name = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    b.finish(name.strip_prefix("solid").unwrap_or(name).trim().to_string())
}

/// Three numbers from the rest of an ASCII line.
fn three<'a>(it: &mut impl Iterator<Item = &'a str>) -> Option<[f32; 3]> {
    Some([it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?])
}

fn ascii(bytes: &[u8]) -> Result<Model> {
    let text = String::from_utf8_lossy(bytes);
    let mut b = Builder::default();
    let mut name: Option<String> = None;
    let mut normal = [0.0f32; 3];
    let mut loop_v: Vec<[f32; 3]> = vec![];
    for (ln, line) in text.lines().enumerate() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        let Some(key) = it.next() else { continue };
        let bad = || perr(format!("STL line {}: expected three numbers", ln + 1));
        match key.to_ascii_lowercase().as_str() {
            "solid" if name.is_none() => {
                name = Some(line.get(5..).unwrap_or("").trim().to_string());
            }
            "facet" => {
                // `facet normal nx ny nz`
                normal = it.next().and_then(|_| three(&mut it)).unwrap_or([0.0; 3]);
                loop_v.clear();
            }
            "vertex" => loop_v.push(three(&mut it).ok_or_else(bad)?),
            "endloop" | "endfacet" => {
                if let Some((&first, rest)) = loop_v.split_first() {
                    for w in rest.windows(2) {
                        if let [a, c] = w {
                            b.tri(None, normal, [first, *a, *c]);
                        }
                    }
                }
                loop_v.clear();
            }
            _ => {}
        }
    }
    b.finish(name.unwrap_or_default())
}
