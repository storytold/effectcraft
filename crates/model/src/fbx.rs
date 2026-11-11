//! FBX 7.x scenes, binary and ASCII.
//!
//! Written from public descriptions of the format (the binary node-record layout, the ASCII
//! syntax, and the scene objects of the Autodesk FBX SDK documentation): no importer source was
//! read.
//!
//! - **Binary**: the `Kaydara FBX Binary` header and version, node records with 32-bit (before
//!   7.5) or 64-bit offsets, every property type (`Y C I F D L S R` and the `f d l i b` arrays,
//!   zlib-compressed or not). Object names `Name\0\x01Class` become `Class::Name` as in ASCII.
//! - **ASCII**: `Key: value, value { children }` with `;` comments and `*N { a: … }` arrays.
//! - **Scene**: `Geometry` meshes (`Vertices`, `PolygonVertexIndex` polygons fan-triangulated,
//!   the first `LayerElementNormal`, `LayerElementUV` and `LayerElementMaterial` with every
//!   mapping and reference mode), `Model` nodes with the SDK transform (translation, rotation
//!   offset and pivot, pre/post rotation, Euler rotation in its order, scaling offset and pivot,
//!   scaling) and geometric transforms, `Material` (Lambert/Phong mapped onto
//!   metallic-roughness), `Texture` and `Video` (embedded or relative image files), joined by
//!   `Connections`. `GlobalSettings` axes turn the scene to +Y up.
//!
//! Not imported: FBX 6.x and older, animation stacks, skin deformers, blend shapes, cameras and
//! lights, NURBS and patches. Units stay the file's own (like OBJ).

use std::collections::HashMap;
use std::sync::Arc;

use crate::obj::srgb_to_linear;
use crate::{AlphaMode, Mat4, Material, Mesh, Model, ModelError, Node, Primitive, Result, TexRef, Texture, perr, vec3};

/// The binary file's magic (`Kaydara FBX Binary`, two spaces, NUL, 0x1A, 0x00).
pub const BINARY_MAGIC: &[u8] = b"Kaydara FBX Binary  \0\x1a\0";

/// Deepest node nesting accepted (real files nest well under 10 levels).
const MAX_DEPTH: usize = 64;
/// Largest decoded array accepted (bytes).
const MAX_ARRAY_BYTES: usize = 1 << 30;
/// Most memory one import may keep: parsed arrays as stored (8 bytes per element, whatever the
/// wire type) plus the meshes built from them (#1234).
const MAX_IMPORT_BYTES: usize = 1 << 31;

/// What is left of an import's memory budget.
struct Budget(std::cell::Cell<usize>);

impl Budget {
    fn new(bytes: usize) -> Budget {
        Budget(std::cell::Cell::new(bytes))
    }
    /// Take `bytes` (None: a size that overflowed) from the budget, or fail the import before
    /// anything that large is allocated.
    fn take(&self, bytes: Option<usize>, what: &str) -> Result<()> {
        let left = self.0.get();
        match bytes.filter(|&b| b <= left) {
            Some(b) => {
                self.0.set(left - b);
                Ok(())
            }
            None => Err(perr(format!("FBX: {what} would take the import past its {} MiB memory limit", MAX_IMPORT_BYTES >> 20))),
        }
    }
}

/// Bytes a mesh keeps.
fn mesh_bytes(m: &Mesh) -> Option<usize> {
    m.primitives.iter().try_fold(0usize, |acc, p| {
        let n = p.positions.len().checked_mul(12)?.checked_add(p.normals.len().checked_mul(12)?)?.checked_add(p.uvs.len().checked_mul(8)?)?;
        let n = n.checked_add(p.tangents.len().checked_mul(16)?)?.checked_add(p.joints.len().checked_mul(8)?)?.checked_add(p.weights.len().checked_mul(16)?)?;
        acc.checked_add(n)?.checked_add(p.indices.len().checked_mul(4)?)
    })
}

/// Bytes the values under `nodes` keep. Used for ASCII files, which hold array elements as
/// one `Prop` each and are parsed before they can be counted (the text itself bounds them).
fn value_bytes(nodes: &[FbxNode]) -> Option<usize> {
    nodes.iter().try_fold(0usize, |acc, n| {
        let own = n.props.iter().try_fold(0usize, |a, p| {
            let extra = match p {
                Prop::Ints(v) => v.len().checked_mul(8)?,
                Prop::Floats(v) => v.len().checked_mul(8)?,
                Prop::Str(v) => v.len(),
                Prop::Bytes(v) => v.len(),
                Prop::Int(_) | Prop::Float(_) => 0,
            };
            a.checked_add(std::mem::size_of::<Prop>())?.checked_add(extra)
        })?;
        acc.checked_add(own)?.checked_add(value_bytes(&n.children)?)
    })
}

/// One property value of an FBX node.
#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Ints(Vec<i64>),
    Floats(Vec<f64>),
}

impl Prop {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Prop::Int(i) => Some(*i as f64),
            Prop::Float(f) => Some(*f),
            _ => None,
        }
    }
    fn as_i64(&self) -> Option<i64> {
        match self {
            Prop::Int(i) => Some(*i),
            Prop::Float(f) if f.is_finite() => Some(*f as i64),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Prop::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// A node of the FBX document tree (the same for binary and ASCII files).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FbxNode {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<FbxNode>,
}

impl FbxNode {
    pub fn child(&self, name: &str) -> Option<&FbxNode> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a FbxNode> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
    fn prop(&self, i: usize) -> Option<&Prop> {
        self.props.get(i)
    }
    fn str_at(&self, i: usize) -> Option<&str> {
        self.prop(i).and_then(Prop::as_str)
    }
    /// The node's array as floats: a binary array property, an ASCII `a:` child, or the plain
    /// value list.
    pub fn floats(&self) -> Vec<f64> {
        if let Some(a) = self.child("a") {
            return a.floats();
        }
        let mut out = vec![];
        for p in &self.props {
            match p {
                Prop::Floats(v) => out.extend_from_slice(v),
                Prop::Ints(v) => out.extend(v.iter().map(|&i| i as f64)),
                Prop::Int(i) => out.push(*i as f64),
                Prop::Float(f) => out.push(*f),
                _ => {}
            }
        }
        out
    }
    /// The node's array as integers (see [`FbxNode::floats`]).
    pub fn ints(&self) -> Vec<i64> {
        if let Some(a) = self.child("a") {
            return a.ints();
        }
        let mut out = vec![];
        for p in &self.props {
            match p {
                Prop::Ints(v) => out.extend_from_slice(v),
                Prop::Floats(v) => out.extend(v.iter().filter(|f| f.is_finite()).map(|&f| f as i64)),
                p => out.extend(p.as_i64()),
            }
        }
        out
    }
}

/// Read an FBX file (binary or ASCII) into its top-level nodes and the file version
/// (e.g. 7400).
pub fn read(bytes: &[u8]) -> Result<(u32, Vec<FbxNode>)> {
    read_within(bytes, &Budget::new(MAX_IMPORT_BYTES))
}

fn read_within(bytes: &[u8], budget: &Budget) -> Result<(u32, Vec<FbxNode>)> {
    if bytes.starts_with(BINARY_MAGIC) {
        read_binary(bytes, budget)
    } else {
        let (version, nodes) = read_ascii(&String::from_utf8_lossy(bytes))?;
        budget.take(value_bytes(&nodes), "its values")?;
        Ok((version, nodes))
    }
}

// ---------------------------------------------------------------------------------------------
// Binary

struct Bin<'a> {
    d: &'a [u8],
    /// 64-bit record offsets (version 7500 and later).
    wide: bool,
    budget: &'a Budget,
}

impl Bin<'_> {
    fn take(&self, pos: &mut usize, n: usize) -> Result<&[u8]> {
        let end = pos.checked_add(n).ok_or_else(|| perr("FBX: offset overflow"))?;
        let s = self.d.get(*pos..end).ok_or_else(|| perr(format!("FBX: truncated at byte {pos}")))?;
        *pos = end;
        Ok(s)
    }
    fn arr<const N: usize>(&self, pos: &mut usize) -> Result<[u8; N]> {
        let s = self.take(pos, N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn u32(&self, pos: &mut usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr(pos)?))
    }
    fn offset(&self, pos: &mut usize) -> Result<u64> {
        if self.wide { Ok(u64::from_le_bytes(self.arr(pos)?)) } else { Ok(self.u32(pos)? as u64) }
    }
    fn to_usize(v: u64) -> Result<usize> {
        usize::try_from(v).map_err(|_| perr("FBX: offset out of range"))
    }

    /// Records until `end` or a null record.
    fn list(&self, pos: &mut usize, end: usize, depth: usize) -> Result<Vec<FbxNode>> {
        if depth > MAX_DEPTH {
            return Err(perr("FBX: nodes nested too deeply"));
        }
        let mut out = vec![];
        while *pos < end {
            match self.record(pos, depth)? {
                Some(n) => out.push(n),
                None => break,
            }
        }
        Ok(out)
    }

    /// One node record; `None` for the null record that closes a list.
    fn record(&self, pos: &mut usize, depth: usize) -> Result<Option<FbxNode>> {
        let start = *pos;
        let end = Self::to_usize(self.offset(pos)?)?;
        let nprops = self.offset(pos)?;
        let plen = Self::to_usize(self.offset(pos)?)?;
        let nlen = self.arr::<1>(pos)?[0] as usize;
        if end == 0 && nprops == 0 && plen == 0 && nlen == 0 {
            return Ok(None);
        }
        if end <= start || end > self.d.len() {
            return Err(perr(format!("FBX: bad record end {end} at byte {start}")));
        }
        let name = String::from_utf8_lossy(self.take(pos, nlen)?).into_owned();
        let pstart = *pos;
        let pend = pstart.checked_add(plen).filter(|&e| e <= end).ok_or_else(|| perr(format!("FBX: `{name}`: property list past the record")))?;
        // Every property is at least one byte, so a count above the list length is corrupt.
        if nprops > plen as u64 {
            return Err(perr(format!("FBX: `{name}`: {nprops} properties in {plen} bytes")));
        }
        let mut props = Vec::with_capacity(nprops as usize);
        for _ in 0..nprops {
            props.push(self.prop(pos, pend)?);
        }
        *pos = pend;
        let children = if *pos < end { self.list(pos, end, depth + 1)? } else { vec![] };
        *pos = end;
        Ok(Some(FbxNode { name, props, children }))
    }

    fn prop(&self, pos: &mut usize, pend: usize) -> Result<Prop> {
        let sub = Bin { d: self.d.get(..pend).ok_or_else(|| perr("FBX: truncated property"))?, wide: self.wide, budget: self.budget };
        let t = sub.arr::<1>(pos)?[0];
        Ok(match t {
            b'Y' => Prop::Int(i16::from_le_bytes(sub.arr(pos)?) as i64),
            b'C' => Prop::Int((sub.arr::<1>(pos)?[0] != 0) as i64),
            b'I' => Prop::Int(i32::from_le_bytes(sub.arr(pos)?) as i64),
            b'L' => Prop::Int(i64::from_le_bytes(sub.arr(pos)?)),
            b'F' => Prop::Float(f32::from_le_bytes(sub.arr(pos)?) as f64),
            b'D' => Prop::Float(f64::from_le_bytes(sub.arr(pos)?)),
            b'S' => {
                let n = sub.u32(pos)? as usize;
                Prop::Str(object_name(sub.take(pos, n)?))
            }
            b'R' => {
                let n = sub.u32(pos)? as usize;
                Prop::Bytes(sub.take(pos, n)?.to_vec())
            }
            b'f' | b'd' | b'l' | b'i' | b'b' => {
                let len = sub.u32(pos)? as usize;
                let enc = sub.u32(pos)?;
                let clen = sub.u32(pos)? as usize;
                let size = match t {
                    b'd' | b'l' => 8,
                    b'f' | b'i' => 4,
                    _ => 1,
                };
                let want = len.checked_mul(size).filter(|&n| n <= MAX_ARRAY_BYTES).ok_or_else(|| perr("FBX: array too large"))?;
                // Elements are kept as f64/i64 (booleans and 32-bit values widen), so the budget
                // is charged for what is kept, not for the wire bytes.
                sub.budget.take(len.checked_mul(8), "an array")?;
                let data = sub.take(pos, clen)?;
                let raw = match enc {
                    0 => data.to_vec(),
                    1 => miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, want).map_err(|e| perr(format!("FBX: array zlib: {e:?}")))?,
                    e => return Err(perr(format!("FBX: unknown array encoding {e}"))),
                };
                let raw = raw.get(..want).ok_or_else(|| perr("FBX: array shorter than its length"))?;
                match t {
                    b'f' => Prop::Floats(raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c) as f64).collect()),
                    b'd' => Prop::Floats(raw.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c)).collect()),
                    b'l' => Prop::Ints(raw.as_chunks::<8>().0.iter().map(|c| i64::from_le_bytes(*c)).collect()),
                    b'i' => Prop::Ints(raw.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c) as i64).collect()),
                    _ => Prop::Ints(raw.iter().map(|&b| (b != 0) as i64).collect()),
                }
            }
            t => return Err(perr(format!("FBX: unknown property type 0x{t:02x}"))),
        })
    }
}

/// Binary object names are `Name\0\x01Class`; ASCII writes `Class::Name`.
fn object_name(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    match s.split_once("\0\x01") {
        Some((name, class)) => format!("{class}::{name}"),
        None => s.into_owned(),
    }
}

fn read_binary(bytes: &[u8], budget: &Budget) -> Result<(u32, Vec<FbxNode>)> {
    let mut pos = BINARY_MAGIC.len();
    let version = Bin { d: bytes, wide: false, budget }.u32(&mut pos)?;
    let bin = Bin { d: bytes, wide: version >= 7500, budget };
    let mut nodes = vec![];
    while pos < bytes.len() {
        match bin.record(&mut pos, 0) {
            Ok(Some(n)) => nodes.push(n),
            Ok(None) => break,
            // A damaged tail (footer, padding) after complete sections is ignored.
            Err(_) if nodes.iter().any(|n| n.name == "Objects") => break,
            Err(e) => return Err(e),
        }
    }
    Ok((version, nodes))
}

// ---------------------------------------------------------------------------------------------
// ASCII

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Key(String),
    Val(Prop),
    /// `*N` array length (the values follow in an `a:` child).
    Count,
    Comma,
    Open,
    Close,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = vec![];
    let mut it = text.char_indices().peekable();
    while let Some(&(i, c)) = it.peek() {
        match c {
            c if c.is_whitespace() => {
                it.next();
            }
            ';' => {
                for (_, c) in it.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            ',' => {
                it.next();
                out.push(Tok::Comma);
            }
            '{' => {
                it.next();
                out.push(Tok::Open);
            }
            '}' => {
                it.next();
                out.push(Tok::Close);
            }
            '"' => {
                it.next();
                let start = i + 1;
                let mut end = text.len();
                for (j, c) in it.by_ref() {
                    if c == '"' {
                        end = j;
                        break;
                    }
                }
                out.push(Tok::Val(Prop::Str(text.get(start..end).unwrap_or("").replace("&quot;", "\""))));
            }
            _ => {
                let start = i;
                let mut end = text.len();
                while let Some(&(j, c)) = it.peek() {
                    if c.is_whitespace() || matches!(c, ',' | '{' | '}' | '"' | ';' | ':') {
                        end = j;
                        break;
                    }
                    it.next();
                }
                let word = text.get(start..end).unwrap_or("");
                if it.peek().is_some_and(|&(_, c)| c == ':') {
                    it.next();
                    out.push(Tok::Key(word.to_string()));
                } else if word.starts_with('*') {
                    out.push(Tok::Count);
                } else if let Ok(i) = word.parse::<i64>() {
                    out.push(Tok::Val(Prop::Int(i)));
                } else if let Ok(f) = word.parse::<f64>() {
                    out.push(Tok::Val(Prop::Float(f)));
                } else if !word.is_empty() {
                    out.push(Tok::Val(Prop::Str(word.to_string())));
                } else {
                    // A stray `:` with no key.
                    it.next();
                }
            }
        }
    }
    out
}

fn ascii_list(toks: &[Tok], i: &mut usize, depth: usize) -> Result<Vec<FbxNode>> {
    if depth > MAX_DEPTH {
        return Err(perr("FBX: nodes nested too deeply"));
    }
    let mut out = vec![];
    while let Some(t) = toks.get(*i) {
        *i += 1;
        match t {
            Tok::Close if depth > 0 => return Ok(out),
            Tok::Key(name) => {
                let mut node = FbxNode { name: name.clone(), ..FbxNode::default() };
                loop {
                    match toks.get(*i) {
                        Some(Tok::Val(v)) => node.props.push(v.clone()),
                        Some(Tok::Count | Tok::Comma) => {}
                        _ => break,
                    }
                    *i += 1;
                }
                if toks.get(*i) == Some(&Tok::Open) {
                    *i += 1;
                    node.children = ascii_list(toks, i, depth + 1)?;
                }
                out.push(node);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn read_ascii(text: &str) -> Result<(u32, Vec<FbxNode>)> {
    let toks = tokenize(text);
    let mut i = 0;
    let nodes = ascii_list(&toks, &mut i, 0)?;
    let version = nodes
        .iter()
        .find(|n| n.name == "FBXHeaderExtension")
        .and_then(|h| h.child("FBXVersion"))
        .and_then(|v| v.prop(0))
        .and_then(Prop::as_i64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    Ok((version, nodes))
}

// ---------------------------------------------------------------------------------------------
// Scene

/// `Properties70` entries by name (`P: name, type, label, flags, values…`).
struct Props70<'a>(HashMap<&'a str, &'a FbxNode>);

impl<'a> Props70<'a> {
    fn of(node: &'a FbxNode) -> Props70<'a> {
        let mut m = HashMap::new();
        if let Some(p) = node.child("Properties70") {
            for e in p.children_named("P") {
                if let Some(name) = e.str_at(0) {
                    m.entry(name).or_insert(e);
                }
            }
        }
        Props70(m)
    }
    fn values(&self, name: &str) -> Vec<f64> {
        self.0.get(name).map(|e| e.props.iter().skip(4).filter_map(Prop::as_f64).collect()).unwrap_or_default()
    }
    fn f(&self, name: &str) -> Option<f64> {
        self.values(name).first().copied().filter(|v| v.is_finite())
    }
    fn v3(&self, name: &str, default: [f64; 3]) -> [f64; 3] {
        match self.values(name).as_slice() {
            [x, y, z, ..] if x.is_finite() && y.is_finite() && z.is_finite() => [*x, *y, *z],
            _ => default,
        }
    }
}

/// An Euler rotation (degrees) in FBX rotation order (`eEulerXYZ` = 0 rotates about X first).
fn euler(d: [f64; 3], order: i64) -> Mat4 {
    let (x, y, z) = (Mat4::rotate_x(d[0]), Mat4::rotate_y(d[1]), Mat4::rotate_z(d[2]));
    match order {
        1 => y * z * x,
        2 => x * z * y,
        3 => z * x * y,
        4 => y * x * z,
        5 => x * y * z,
        _ => z * y * x,
    }
}

fn translate(t: [f64; 3]) -> Mat4 {
    Mat4::translate(vec3(t[0], t[1], t[2]))
}

/// A Model's local transform: `T · Roff · Rp · Rpre · R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹`.
fn model_local(p: &Props70) -> Mat4 {
    let order = p.f("RotationOrder").map_or(0, |o| o as i64);
    let active = p.f("RotationActive").is_some_and(|a| a != 0.0);
    let (pre, post) =
        if active { (euler(p.v3("PreRotation", [0.0; 3]), 0), euler(p.v3("PostRotation", [0.0; 3]), 0).transpose()) } else { (Mat4::IDENTITY, Mat4::IDENTITY) };
    let rp = p.v3("RotationPivot", [0.0; 3]);
    let sp = p.v3("ScalingPivot", [0.0; 3]);
    let s = p.v3("Lcl Scaling", [1.0; 3]);
    translate(p.v3("Lcl Translation", [0.0; 3]))
        * translate(p.v3("RotationOffset", [0.0; 3]))
        * translate(rp)
        * pre
        * euler(p.v3("Lcl Rotation", [0.0; 3]), order)
        * post
        * translate([-rp[0], -rp[1], -rp[2]])
        * translate(p.v3("ScalingOffset", [0.0; 3]))
        * translate(sp)
        * Mat4::scale(vec3(s[0], s[1], s[2]))
        * translate([-sp[0], -sp[1], -sp[2]])
}

/// The geometric transform of a Model (applies to its geometry, not to its children).
fn geometric(p: &Props70) -> Option<Mat4> {
    let t = p.v3("GeometricTranslation", [0.0; 3]);
    let r = p.v3("GeometricRotation", [0.0; 3]);
    let s = p.v3("GeometricScaling", [1.0; 3]);
    if t == [0.0; 3] && r == [0.0; 3] && s == [1.0; 3] {
        return None;
    }
    Some(translate(t) * euler(r, 0) * Mat4::scale(vec3(s[0], s[1], s[2])))
}

/// The file's axis system → +Y up, +Z front, +X right.
fn axis_matrix(settings: Option<&FbxNode>) -> Mat4 {
    let p = settings.map(Props70::of);
    let get = |name: &str, d: f64| p.as_ref().and_then(|p| p.f(name)).unwrap_or(d);
    let axis = |a: f64, s: f64| -> Option<[f64; 4]> {
        let a = a as i64;
        let sign = if s < 0.0 { -1.0 } else { 1.0 };
        let mut v = [0.0; 4];
        *v.get_mut(usize::try_from(a).ok().filter(|&a| a < 3)?)? = sign;
        Some(v)
    };
    let (Some(right), Some(up), Some(front)) = (
        axis(get("CoordAxis", 0.0), get("CoordAxisSign", 1.0)),
        axis(get("UpAxis", 1.0), get("UpAxisSign", 1.0)),
        axis(get("FrontAxis", 2.0), get("FrontAxisSign", 1.0)),
    ) else {
        return Mat4::IDENTITY;
    };
    let m = Mat4([right, up, front, [0.0, 0.0, 0.0, 1.0]]);
    // Two axes on the same file axis: not a rotation.
    let used = |k: usize| right[k] != 0.0 || up[k] != 0.0 || front[k] != 0.0;
    if (0..3).all(used) { m } else { Mat4::IDENTITY }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mapping {
    PolygonVertex,
    Vertex,
    Polygon,
    AllSame,
}

/// A geometry layer element (normals, UVs, material indices).
struct LayerData {
    mapping: Mapping,
    data: Vec<f64>,
    index: Option<Vec<i64>>,
    width: usize,
}

impl LayerData {
    fn of(geom: &FbxNode, element: &str, data: &str, index: &str, width: usize) -> Option<LayerData> {
        let e = geom.child(element)?;
        let mapping = match e.child("MappingInformationType").and_then(|m| m.str_at(0)).unwrap_or("ByPolygonVertex") {
            "ByPolygonVertex" => Mapping::PolygonVertex,
            "ByVertex" | "ByVertice" | "ByControlPoint" => Mapping::Vertex,
            "ByPolygon" => Mapping::Polygon,
            "AllSame" => Mapping::AllSame,
            _ => return None,
        };
        let reference = e.child("ReferenceInformationType").and_then(|m| m.str_at(0)).unwrap_or("Direct");
        let index = (reference != "Direct").then(|| e.child(index).map(FbxNode::ints)).flatten();
        Some(LayerData { mapping, data: e.child(data)?.floats(), index, width })
    }
    fn get(&self, pv: usize, v: usize, poly: usize) -> Option<&[f64]> {
        let i = match self.mapping {
            Mapping::PolygonVertex => pv,
            Mapping::Vertex => v,
            Mapping::Polygon => poly,
            Mapping::AllSame => 0,
        };
        let j = match &self.index {
            Some(ix) => usize::try_from(*ix.get(i)?).ok()?,
            None => i,
        };
        let a = j.checked_mul(self.width)?;
        self.data.get(a..a.checked_add(self.width)?)
    }
}

fn f32s<const N: usize>(s: &[f64]) -> [f32; N] {
    let mut a = [0.0f32; N];
    for (o, v) in a.iter_mut().zip(s) {
        *o = if v.is_finite() { *v as f32 } else { 0.0 };
    }
    a
}

/// A Mesh geometry with its polygons triangulated; `materials[k]` is the model-level material
/// index of the geometry's material slot `k`.
fn build_mesh(name: &str, geom: &FbxNode, materials: &[usize], with_normal_maps: &dyn Fn(usize) -> bool) -> Option<Mesh> {
    let positions: Vec<[f32; 3]> = geom.child("Vertices")?.floats().as_chunks::<3>().0.iter().map(|c| f32s::<3>(c)).collect();
    let pvi = geom.child("PolygonVertexIndex")?.ints();
    let normals = LayerData::of(geom, "LayerElementNormal", "Normals", "NormalsIndex", 3);
    let uvs = LayerData::of(geom, "LayerElementUV", "UV", "UVIndex", 2);
    let mats = geom.child("LayerElementMaterial").and_then(|e| {
        let mapping = e.child("MappingInformationType").and_then(|m| m.str_at(0)).unwrap_or("AllSame");
        let ids = e.child("Materials")?.ints();
        Some((mapping == "ByPolygon", ids))
    });

    type Key = (u32, [u32; 3], [u32; 2]);
    let mut prims: Vec<(Option<usize>, Primitive, HashMap<Key, u32>, Vec<bool>)> = vec![];
    let mut corners: Vec<(usize, usize)> = vec![];
    let mut poly = 0usize;
    for (pv, &raw) in pvi.iter().enumerate() {
        // The last corner of a polygon is stored as `-index - 1`.
        let (v, last) = if raw < 0 { (!raw, true) } else { (raw, false) };
        corners.push((pv, usize::try_from(v).unwrap_or(usize::MAX)));
        if !last {
            continue;
        }
        let poly_corners = std::mem::take(&mut corners);
        let p_index = poly;
        poly += 1;
        if poly_corners.len() < 3 || poly_corners.iter().any(|&(_, v)| v >= positions.len()) {
            continue;
        }
        let slot = mats.as_ref().and_then(|(by_poly, ids)| {
            let k = if *by_poly { *ids.get(p_index)? } else { *ids.first()? };
            materials.get(usize::try_from(k).ok()?).copied()
        });
        let slot = slot.or_else(|| materials.first().copied());
        let at = match prims.iter().position(|p| p.0 == slot) {
            Some(i) => i,
            None => {
                prims.push((slot, Primitive { material: slot, ..Default::default() }, HashMap::new(), vec![]));
                prims.len() - 1
            }
        };
        let Some((_, prim, map, need)) = prims.get_mut(at) else { continue };
        let mut ids = Vec::with_capacity(poly_corners.len());
        for &(pv, v) in &poly_corners {
            let n = normals.as_ref().and_then(|l| l.get(pv, v, p_index)).map(f32s::<3>);
            let t = uvs.as_ref().and_then(|l| l.get(pv, v, p_index)).map(f32s::<2>);
            let key: Key = (v as u32, n.unwrap_or([f32::NAN; 3]).map(f32::to_bits), t.unwrap_or([f32::NAN; 2]).map(f32::to_bits));
            let id = *map.entry(key).or_insert_with(|| {
                prim.positions.push(positions.get(v).copied().unwrap_or_default());
                prim.normals.push(n.unwrap_or_default());
                prim.uvs.push(t.unwrap_or_default());
                need.push(n.is_none());
                (prim.positions.len() - 1) as u32
            });
            ids.push(id);
        }
        for i in 2..ids.len() {
            prim.indices.extend([ids[0], ids[i - 1], ids[i]]);
        }
    }
    let mut mesh = Mesh { name: name.to_string(), primitives: vec![] };
    for (_, mut p, _, need) in prims {
        if p.indices.is_empty() {
            continue;
        }
        if need.iter().any(|&b| b) {
            let given = p.normals.clone();
            p.compute_normals();
            for ((n, g), need) in p.normals.iter_mut().zip(given).zip(&need) {
                if !need {
                    *n = g;
                }
            }
        }
        if uvs.is_none() {
            p.uvs.clear();
        } else {
            // FBX's v axis points up the image; textures are stored top row first.
            for uv in p.uvs.iter_mut() {
                uv[1] = 1.0 - uv[1];
            }
        }
        if p.material.is_some_and(with_normal_maps) {
            p.compute_tangents();
        }
        mesh.primitives.push(p);
    }
    (!mesh.primitives.is_empty()).then_some(mesh)
}

/// Lambert/Phong surface properties onto metallic-roughness.
fn material(name: &str, node: &FbxNode) -> Material {
    let p = Props70::of(node);
    let phong = node.child("ShadingModel").and_then(|s| s.str_at(0)).is_some_and(|s| s.eq_ignore_ascii_case("phong"));
    let factor = p.f("DiffuseFactor").unwrap_or(1.0);
    let d = p.v3("DiffuseColor", p.v3("Diffuse", [0.8; 3]));
    let alpha = match p.f("Opacity") {
        Some(o) => o,
        None => {
            let tc = p.v3("TransparentColor", [0.0; 3]);
            1.0 - (tc[0] + tc[1] + tc[2]) / 3.0 * p.f("TransparencyFactor").unwrap_or(0.0)
        }
    }
    .clamp(0.0, 1.0);
    let lin = |c: f64| srgb_to_linear((c * factor).clamp(0.0, 1.0) as f32);
    let e = p.v3("EmissiveColor", [0.0; 3]);
    let ef = p.f("EmissiveFactor").unwrap_or(1.0);
    let roughness = if phong {
        let ns = p.f("ShininessExponent").or_else(|| p.f("Shininess")).unwrap_or(20.0).max(0.0);
        (2.0 / (ns + 2.0)).sqrt() as f32
    } else {
        1.0
    };
    Material {
        name: name.to_string(),
        base_color: [lin(d[0]), lin(d[1]), lin(d[2]), alpha as f32],
        metallic: 0.0,
        roughness,
        emissive: e.map(|c| srgb_to_linear((c * ef).clamp(0.0, 1.0) as f32)),
        alpha_mode: if alpha < 0.999 { AlphaMode::Blend } else { AlphaMode::Opaque },
        ..Material::default()
    }
}

/// Decode standard base64 (ASCII FBX embeds `Video` content this way); `None` on bad input.
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn short_name(full: &str) -> &str {
    full.split_once("::").map_or(full, |(_, n)| n)
}

/// Parse an FBX file into a model. `resolve(uri)` reads texture files next to it.
pub fn parse(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>) -> Result<Model> {
    parse_within(bytes, resolve, MAX_IMPORT_BYTES)
}

/// [`parse`] keeping at most `limit` bytes of arrays and meshes.
fn parse_within(bytes: &[u8], resolve: &dyn Fn(&str) -> Option<Vec<u8>>, limit: usize) -> Result<Model> {
    let budget = Budget::new(limit);
    let (version, doc) = read_within(bytes, &budget)?;
    if version != 0 && version < 7000 {
        return Err(ModelError::Unsupported(format!("FBX {}.{} (only FBX 7.x files are read)", version / 1000, version % 1000 / 100)));
    }
    let top = |name: &str| doc.iter().find(|n| n.name == name);
    let objects = top("Objects").ok_or_else(|| perr("FBX: no Objects section"))?;
    let by_id: HashMap<i64, &FbxNode> = objects.children.iter().filter_map(|o| Some((o.prop(0)?.as_i64()?, o))).collect();
    let kind = |id: i64| by_id.get(&id).map(|n| n.name.as_str());

    // Connections: (child, parent, property) in file order.
    let mut links: Vec<(i64, i64, Option<&str>)> = vec![];
    if let Some(c) = top("Connections") {
        for e in c.children_named("C") {
            let (Some(a), Some(b)) = (e.prop(1).and_then(Prop::as_i64), e.prop(2).and_then(Prop::as_i64)) else { continue };
            links.push((a, b, e.str_at(3)));
        }
    }

    let model_ids: Vec<i64> = objects.children.iter().filter(|o| o.name == "Model").filter_map(|o| o.prop(0)?.as_i64()).collect();
    let mut node_of: HashMap<i64, usize> = HashMap::new();
    let mut nodes = vec![Node { name: "FBX axes".into(), matrix: Some(axis_matrix(top("GlobalSettings"))), ..Node::default() }];
    for &id in &model_ids {
        if node_of.contains_key(&id) {
            continue;
        }
        let Some(o) = by_id.get(&id) else { continue };
        let name = short_name(o.str_at(1).unwrap_or("")).to_string();
        node_of.insert(id, nodes.len());
        nodes.push(Node { name, matrix: Some(model_local(&Props70::of(o))), ..Node::default() });
    }

    // Hierarchy: the first Model→Model link is the parent; anything else hangs off the root.
    let mut parent: HashMap<usize, usize> = HashMap::new();
    for &(c, p, _) in &links {
        if let (Some(&ci), Some(&pi)) = (node_of.get(&c), node_of.get(&p))
            && ci != pi
        {
            parent.entry(ci).or_insert(pi);
        }
    }
    for i in 1..nodes.len() {
        let p = parent.get(&i).copied().unwrap_or(0);
        if let Some(n) = nodes.get_mut(p) {
            n.children.push(i);
        }
    }

    // Materials (in first-use order) and their textures.
    let mut materials: Vec<Material> = vec![];
    let mut textures: Vec<Arc<Texture>> = vec![];
    let mut material_of: HashMap<i64, usize> = HashMap::new();
    let mut texture_of: HashMap<i64, Option<usize>> = HashMap::new();
    let mut load_texture = |tid: i64, textures: &mut Vec<Arc<Texture>>| -> Option<usize> {
        if let Some(t) = texture_of.get(&tid) {
            return *t;
        }
        let t = (|| {
            let tex = by_id.get(&tid)?;
            let embedded = links.iter().filter(|l| l.1 == tid && kind(l.0) == Some("Video")).find_map(|l| {
                let content = by_id.get(&l.0)?.child("Content")?;
                match content.props.first()? {
                    Prop::Bytes(b) if !b.is_empty() => Some(b.clone()),
                    Prop::Str(_) => {
                        let s: String = content.props.iter().filter_map(Prop::as_str).collect();
                        base64(&s).filter(|b| !b.is_empty())
                    }
                    _ => None,
                }
            });
            let bytes = embedded.or_else(|| {
                ["RelativeFilename", "FileName"].iter().find_map(|k| {
                    let f = tex.child(k)?.str_at(0)?.replace('\\', "/");
                    resolve(&f).or_else(|| resolve(f.rsplit('/').next()?))
                })
            })?;
            let t = Texture::decode(&bytes).ok()?;
            textures.push(Arc::new(t));
            Some(textures.len() - 1)
        })();
        texture_of.insert(tid, t);
        t
    };
    let mut material_index = |mid: i64, materials: &mut Vec<Material>, textures: &mut Vec<Arc<Texture>>| -> Option<usize> {
        if let Some(&m) = material_of.get(&mid) {
            return Some(m);
        }
        let node = by_id.get(&mid)?;
        let mut m = material(short_name(node.str_at(1).unwrap_or("")), node);
        for &(tid, _, prop) in links.iter().filter(|l| l.1 == mid && kind(l.0) == Some("Texture")) {
            let slot = match prop {
                Some("DiffuseColor") => &mut m.base_color_tex,
                Some("NormalMap") => &mut m.normal_tex,
                Some("EmissiveColor") => &mut m.emissive_tex,
                _ => continue,
            };
            if slot.is_none() {
                *slot = load_texture(tid, textures).map(|texture| TexRef { texture, uv_set: 0 });
            }
        }
        if m.emissive_tex.is_some() && m.emissive == [0.0; 3] {
            m.emissive = [1.0; 3];
        }
        materials.push(m);
        material_of.insert(mid, materials.len() - 1);
        Some(materials.len() - 1)
    };

    // One mesh per Model with Mesh geometry (material slots are per Model). Models instancing the
    // same geometry with the same materials share one mesh rather than each building a copy.
    let mut meshes: Vec<Mesh> = vec![];
    let mut mesh_of: HashMap<(i64, Vec<usize>), usize> = HashMap::new();
    for &id in &model_ids {
        let Some(&ni) = node_of.get(&id) else { continue };
        let Some(gid) = links.iter().find(|l| l.1 == id && kind(l.0) == Some("Geometry")).map(|l| l.0) else { continue };
        let Some(geom) = by_id.get(&gid) else { continue };
        let mats: Vec<usize> =
            links.iter().filter(|l| l.1 == id && kind(l.0) == Some("Material")).filter_map(|l| material_index(l.0, &mut materials, &mut textures)).collect();
        let name = nodes.get(ni).map(|n| n.name.clone()).unwrap_or_default();
        let mesh_index = match mesh_of.get(&(gid, mats.clone())) {
            Some(&i) => i,
            None => {
                let normal_mapped: Vec<bool> = materials.iter().map(|m| m.normal_tex.is_some()).collect();
                let Some(mesh) = build_mesh(&name, geom, &mats, &|m| normal_mapped.get(m).copied().unwrap_or(false)) else { continue };
                budget.take(mesh_bytes(&mesh), &format!("mesh `{name}`"))?;
                meshes.push(mesh);
                mesh_of.insert((gid, mats), meshes.len() - 1);
                meshes.len() - 1
            }
        };
        let geo = by_id.get(&id).and_then(|o| geometric(&Props70::of(o)));
        match geo {
            Some(matrix) => {
                nodes.push(Node { name: format!("{name} geometry"), matrix: Some(matrix), mesh: Some(mesh_index), ..Node::default() });
                let gi = nodes.len() - 1;
                if let Some(n) = nodes.get_mut(ni) {
                    n.children.push(gi);
                }
            }
            None => {
                if let Some(n) = nodes.get_mut(ni) {
                    n.mesh = Some(mesh_index);
                }
            }
        }
    }
    if meshes.is_empty() {
        return Err(perr("FBX file without meshes"));
    }
    Ok(Model { meshes, nodes, roots: vec![0], materials, textures, ..Model::default() })
}

#[cfg(test)]
mod tests;
