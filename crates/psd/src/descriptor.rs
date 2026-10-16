//! The descriptor ("action descriptor") structure: typed key/value trees used by text layers,
//! layer effects, fill layers and newer adjustment layers (spec: "Descriptor structure").

use crate::read::Reader;
use crate::{Error, Result};

/// A descriptor: a class and a list of keyed items (in file order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Descriptor {
    pub class_name: String,
    pub class_id: String,
    pub items: Vec<(String, DValue)>,
}

/// One descriptor value.
#[derive(Clone, Debug, PartialEq)]
pub enum DValue {
    Descriptor(Descriptor),
    List(Vec<DValue>),
    Double(f64),
    /// Unit (`#Pxl`, `#Prc`, `#Ang`, `#Pnt`…) and value.
    UnitFloat(String, f64),
    UnitFloats(String, Vec<f64>),
    Text(String),
    /// Enumerated value: type and value ids.
    Enum(String, String),
    Integer(i32),
    LargeInteger(i64),
    Bool(bool),
    Class(String, String),
    Raw(Vec<u8>),
    Reference(Vec<RefItem>),
    /// An object array (`ObAr`): `count` objects of one class stored column-wise, each item a
    /// list of `count` values (unit floats).
    ObjectArray(u32, Descriptor),
}

/// An item of a reference (`obj `).
#[derive(Clone, Debug, PartialEq)]
pub enum RefItem {
    Property(String, String),
    Class(String),
    Enum(String, String, String),
    Offset(String, i32),
    Identifier(i32),
    Index(i32),
    Name(String, String),
}

impl Descriptor {
    pub fn get(&self, key: &str) -> Option<&DValue> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn obj(&self, key: &str) -> Option<&Descriptor> {
        match self.get(key)? {
            DValue::Descriptor(d) => Some(d),
            _ => None,
        }
    }
    /// A number: doubles, unit floats, integers.
    pub fn num(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            DValue::Double(v) | DValue::UnitFloat(_, v) => Some(*v),
            DValue::Integer(v) => Some(*v as f64),
            DValue::LargeInteger(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            DValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            DValue::Text(s) => Some(s),
            _ => None,
        }
    }
    /// The value id of an enumerated item.
    pub fn enum_value(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            DValue::Enum(_, v) => Some(v),
            _ => None,
        }
    }
    pub fn raw(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            DValue::Raw(v) => Some(v),
            _ => None,
        }
    }
    /// An RGB colour object (`RGBC` with `Rd  `/`Grn `/`Bl  ` in 0..255) as 0..1 values; also
    /// reads grayscale (`Gry `) colours.
    pub fn color(&self, key: &str) -> Option<[f64; 3]> {
        let c = self.obj(key)?;
        if let (Some(r), Some(g), Some(b)) = (c.num("Rd  "), c.num("Grn "), c.num("Bl  ")) {
            return Some([r / 255.0, g / 255.0, b / 255.0]);
        }
        if let (Some(r), Some(g), Some(b)) = (c.num("redFloat"), c.num("greenFloat"), c.num("blueFloat")) {
            return Some([r, g, b]);
        }
        c.num("Gry ").map(|g| {
            let v = 1.0 - g / 100.0;
            [v, v, v]
        })
    }
}

/// A key or class id: length-prefixed string, or a 4-character code when the length is 0.
pub(crate) fn read_id(r: &mut Reader) -> Result<String> {
    let n = r.u32()? as usize;
    let n = if n == 0 { 4 } else { n };
    let b = r.bytes(n)?;
    Ok(String::from_utf8_lossy(b).into_owned())
}

pub(crate) fn read_unicode(r: &mut Reader) -> Result<String> {
    let n = r.u32()? as usize;
    let mut v = Vec::with_capacity(n.min(r.remaining() / 2));
    for _ in 0..n {
        v.push(r.u16()?);
    }
    while v.last() == Some(&0) {
        v.pop();
    }
    Ok(String::from_utf16_lossy(&v))
}

/// Read a descriptor (after its version field).
pub fn read_descriptor(r: &mut Reader) -> Result<Descriptor> {
    read_descriptor_at(r, 0)
}

/// [`read_descriptor`] nested `depth` values deep: the limit in [`read_value`] counts nested
/// descriptors too, so a file can't recurse until the stack overflows.
fn read_descriptor_at(r: &mut Reader, depth: usize) -> Result<Descriptor> {
    let class_name = read_unicode(r)?;
    let class_id = read_id(r)?;
    let n = r.u32()? as usize;
    if n > 100_000 {
        return Err(Error::Invalid("descriptor item count".into()));
    }
    let mut items = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        let key = read_id(r)?;
        let ty = r.tag()?;
        let v = read_value(r, &ty, depth)?;
        items.push((key, v));
    }
    Ok(Descriptor { class_name, class_id, items })
}

fn read_value(r: &mut Reader, ty: &[u8; 4], depth: usize) -> Result<DValue> {
    if depth > 64 {
        return Err(Error::Invalid("descriptor nesting".into()));
    }
    Ok(match ty {
        b"Objc" | b"GlbO" => DValue::Descriptor(read_descriptor_at(r, depth + 1)?),
        b"VlLs" => {
            let n = r.u32()? as usize;
            if n > 1_000_000 {
                return Err(Error::Invalid("descriptor list length".into()));
            }
            let mut v = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                let t = r.tag()?;
                v.push(read_value(r, &t, depth + 1)?);
            }
            DValue::List(v)
        }
        b"ObAr" => {
            let n = r.u32()?;
            DValue::ObjectArray(n, read_descriptor_at(r, depth + 1)?)
        }
        b"doub" => DValue::Double(r.f64_finite()?),
        b"UntF" => {
            let unit = String::from_utf8_lossy(&r.tag()?).into_owned();
            DValue::UnitFloat(unit, r.f64_finite()?)
        }
        b"UnFl" => {
            let unit = String::from_utf8_lossy(&r.tag()?).into_owned();
            let n = r.u32()? as usize;
            let mut v = Vec::with_capacity(n.min(1024));
            for _ in 0..n {
                v.push(r.f64_finite()?);
            }
            DValue::UnitFloats(unit, v)
        }
        b"TEXT" => DValue::Text(read_unicode(r)?),
        b"enum" => {
            let t = read_id(r)?;
            DValue::Enum(t, read_id(r)?)
        }
        b"long" => DValue::Integer(r.u32()? as i32),
        b"comp" => DValue::LargeInteger(r.u64()? as i64),
        b"bool" => DValue::Bool(r.u8()? != 0),
        b"type" | b"GlbC" => {
            let name = read_unicode(r)?;
            DValue::Class(name, read_id(r)?)
        }
        b"alis" | b"tdta" | b"Pth " => {
            let n = r.u32()? as usize;
            DValue::Raw(r.bytes(n)?.to_vec())
        }
        b"obj " => {
            let n = r.u32()? as usize;
            let mut items = Vec::with_capacity(n.min(16));
            for _ in 0..n {
                let t = r.tag()?;
                items.push(match &t {
                    b"prop" => {
                        let _name = read_unicode(r)?;
                        let class = read_id(r)?;
                        RefItem::Property(class, read_id(r)?)
                    }
                    b"Clss" => {
                        let _name = read_unicode(r)?;
                        RefItem::Class(read_id(r)?)
                    }
                    b"Enmr" => {
                        let _name = read_unicode(r)?;
                        let class = read_id(r)?;
                        let ty = read_id(r)?;
                        RefItem::Enum(class, ty, read_id(r)?)
                    }
                    b"rele" => {
                        let _name = read_unicode(r)?;
                        let class = read_id(r)?;
                        RefItem::Offset(class, r.u32()? as i32)
                    }
                    b"Idnt" => RefItem::Identifier(r.u32()? as i32),
                    b"indx" => RefItem::Index(r.u32()? as i32),
                    b"name" => {
                        let _name = read_unicode(r)?;
                        let class = read_id(r)?;
                        RefItem::Name(class, read_unicode(r)?)
                    }
                    other => return Err(Error::Invalid(format!("reference item `{}`", String::from_utf8_lossy(other)))),
                });
            }
            DValue::Reference(items)
        }
        other => return Err(Error::Invalid(format!("descriptor type `{}`", String::from_utf8_lossy(other)))),
    })
}

// ---------------------------------------------------------------- writing

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_id(out: &mut Vec<u8>, id: &str) {
    if id.len() == 4 {
        put_u32(out, 0);
    } else {
        put_u32(out, id.len() as u32);
    }
    out.extend_from_slice(id.as_bytes());
}

/// A length-prefixed UTF-16BE string (the length counts code units, including the NUL when
/// `terminated`).
pub(crate) fn put_utf16(out: &mut Vec<u8>, s: &str, terminated: bool) {
    let v: Vec<u16> = s.encode_utf16().chain(terminated.then_some(0)).collect();
    put_u32(out, v.len() as u32);
    for c in v {
        put_u16(out, c);
    }
}

/// A descriptor Unicode string: NUL-terminated.
pub(crate) fn put_unicode(out: &mut Vec<u8>, s: &str) {
    put_utf16(out, s, true);
}

/// Serialise a descriptor (without the version field).
pub fn write_descriptor(out: &mut Vec<u8>, d: &Descriptor) {
    put_unicode(out, &d.class_name);
    put_id(out, &d.class_id);
    put_u32(out, d.items.len() as u32);
    for (k, v) in &d.items {
        put_id(out, k);
        write_value(out, v);
    }
}

fn write_value(out: &mut Vec<u8>, v: &DValue) {
    match v {
        DValue::Descriptor(d) => {
            out.extend_from_slice(b"Objc");
            write_descriptor(out, d);
        }
        DValue::List(l) => {
            out.extend_from_slice(b"VlLs");
            put_u32(out, l.len() as u32);
            for x in l {
                write_value(out, x);
            }
        }
        DValue::ObjectArray(n, d) => {
            out.extend_from_slice(b"ObAr");
            put_u32(out, *n);
            write_descriptor(out, d);
        }
        DValue::Double(x) => {
            out.extend_from_slice(b"doub");
            out.extend_from_slice(&x.to_be_bytes());
        }
        DValue::UnitFloat(u, x) => {
            out.extend_from_slice(b"UntF");
            out.extend_from_slice(&tag4(u));
            out.extend_from_slice(&x.to_be_bytes());
        }
        DValue::UnitFloats(u, xs) => {
            out.extend_from_slice(b"UnFl");
            out.extend_from_slice(&tag4(u));
            put_u32(out, xs.len() as u32);
            for x in xs {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        DValue::Text(s) => {
            out.extend_from_slice(b"TEXT");
            put_unicode(out, s);
        }
        DValue::Enum(t, e) => {
            out.extend_from_slice(b"enum");
            put_id(out, t);
            put_id(out, e);
        }
        DValue::Integer(i) => {
            out.extend_from_slice(b"long");
            out.extend_from_slice(&i.to_be_bytes());
        }
        DValue::LargeInteger(i) => {
            out.extend_from_slice(b"comp");
            out.extend_from_slice(&i.to_be_bytes());
        }
        DValue::Bool(b) => {
            out.extend_from_slice(b"bool");
            out.push(*b as u8);
        }
        DValue::Class(n, c) => {
            out.extend_from_slice(b"type");
            put_unicode(out, n);
            put_id(out, c);
        }
        DValue::Raw(b) => {
            out.extend_from_slice(b"tdta");
            put_u32(out, b.len() as u32);
            out.extend_from_slice(b);
        }
        DValue::Reference(items) => {
            out.extend_from_slice(b"obj ");
            put_u32(out, items.len() as u32);
            for it in items {
                match it {
                    RefItem::Property(c, k) => {
                        out.extend_from_slice(b"prop");
                        put_unicode(out, "");
                        put_id(out, c);
                        put_id(out, k);
                    }
                    RefItem::Class(c) => {
                        out.extend_from_slice(b"Clss");
                        put_unicode(out, "");
                        put_id(out, c);
                    }
                    RefItem::Enum(c, t, e) => {
                        out.extend_from_slice(b"Enmr");
                        put_unicode(out, "");
                        put_id(out, c);
                        put_id(out, t);
                        put_id(out, e);
                    }
                    RefItem::Offset(c, o) => {
                        out.extend_from_slice(b"rele");
                        put_unicode(out, "");
                        put_id(out, c);
                        out.extend_from_slice(&o.to_be_bytes());
                    }
                    RefItem::Identifier(i) => {
                        out.extend_from_slice(b"Idnt");
                        out.extend_from_slice(&i.to_be_bytes());
                    }
                    RefItem::Index(i) => {
                        out.extend_from_slice(b"indx");
                        out.extend_from_slice(&i.to_be_bytes());
                    }
                    RefItem::Name(c, n) => {
                        out.extend_from_slice(b"name");
                        put_unicode(out, "");
                        put_id(out, c);
                        put_unicode(out, n);
                    }
                }
            }
        }
    }
}

fn tag4(s: &str) -> [u8; 4] {
    let mut t = *b"    ";
    for (i, b) in s.bytes().take(4).enumerate() {
        t[i] = b;
    }
    t
}

/// Builder helpers for descriptors (tests and the writer).
impl Descriptor {
    pub fn new(class_id: &str) -> Descriptor {
        Descriptor { class_name: String::new(), class_id: class_id.into(), items: vec![] }
    }
    pub fn with(mut self, key: &str, v: DValue) -> Descriptor {
        self.items.push((key.into(), v));
        self
    }
    /// An `RGBC` colour object from 0..1 values.
    pub fn rgb(c: [f64; 3]) -> Descriptor {
        Descriptor::new("RGBC").with("Rd  ", DValue::Double(c[0] * 255.0)).with("Grn ", DValue::Double(c[1] * 255.0)).with("Bl  ", DValue::Double(c[2] * 255.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeply_nested_descriptors_error_instead_of_overflowing_the_stack() {
        // Each level: empty class name, a 4-byte class id, one item `key` of type `Objc`. The
        // depth limit restarted at every nested descriptor, so 100 000 levels (2.4 MB) recursed
        // until the stack overflowed.
        let level: Vec<u8> = [&0u32.to_be_bytes()[..], &0u32.to_be_bytes(), b"null", &1u32.to_be_bytes(), &0u32.to_be_bytes(), b"key ", b"Objc"].concat();
        let bytes = level.repeat(100_000);
        assert!(matches!(read_descriptor(&mut Reader::new(&bytes)), Err(Error::Invalid(_))));
    }

    #[test]
    fn round_trip() {
        let d = Descriptor::new("null")
            .with("Txt ", DValue::Text("Héllo".into()))
            .with("Clr ", DValue::Descriptor(Descriptor::rgb([1.0, 0.5, 0.0])))
            .with("Opct", DValue::UnitFloat("#Prc".into(), 75.0))
            .with("Md  ", DValue::Enum("BlnM".into(), "Mltp".into()))
            .with("enab", DValue::Bool(true))
            .with("longKeyName", DValue::List(vec![DValue::Integer(3), DValue::Double(1.5)]))
            .with("EngineData", DValue::Raw(b"<< >>".to_vec()))
            .with("ref", DValue::Reference(vec![RefItem::Enum("Lyr ".into(), "Ordn".into(), "Trgt".into()), RefItem::Index(2)]));
        let mut out = vec![];
        write_descriptor(&mut out, &d);
        let back = read_descriptor(&mut Reader::new(&out)).unwrap();
        assert_eq!(back.class_id, "null");
        assert_eq!(back.text("Txt "), Some("Héllo"));
        assert_eq!(back.color("Clr "), Some([1.0, 0.5, 0.0]));
        assert_eq!(back.num("Opct"), Some(75.0));
        assert_eq!(back.enum_value("Md  "), Some("Mltp"));
        assert_eq!(back.bool("enab"), Some(true));
        assert_eq!(back.raw("EngineData"), Some(&b"<< >>"[..]));
        assert_eq!(back, d);
    }
}
