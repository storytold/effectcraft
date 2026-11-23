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
    /// Identifier with the legacy unnamed `null` class.
    Identifier(i32),
    /// Index with the legacy unnamed `null` class.
    Index(i32),
    /// Identifier retaining the original class and its encoding.
    IdentifierWithClass(craft_psd::descriptor::Class, i32),
    /// Index retaining the original class and its encoding.
    IndexWithClass(craft_psd::descriptor::Class, i32),
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

/// Preserve the application parser's per-container limits in the shared parser.
pub(crate) const PARSE_LIMITS: craft_psd::descriptor::ParseLimits =
    craft_psd::descriptor::ParseLimits { max_descriptor_items: 100_000, max_list_items: 1_000_000 };

/// Read a descriptor (after its version field) with [`craft_psd::descriptor::Descriptor::parse_prefix`].
/// The public [`Descriptor`] / [`DValue`] types stay this crate's (engine interchange uses them).
pub fn read_descriptor(r: &mut Reader) -> Result<Descriptor> {
    let rest = r.data.get(r.pos..).unwrap_or(&[]);
    let (d, used) = craft_psd::descriptor::Descriptor::parse_prefix_with_limits(rest, PARSE_LIMITS).map_err(desc_err)?;
    r.skip(used)?;
    Ok(from_craft_descriptor(d))
}

pub(crate) fn desc_err(e: craft_psd::PsdError) -> Error {
    match e {
        craft_psd::PsdError::UnexpectedEof { .. } => Error::Truncated,
        craft_psd::PsdError::Unsupported(s) => Error::Unsupported(s),
        other => Error::Invalid(other.to_string()),
    }
}

fn id_str(id: &craft_psd::descriptor::Id) -> String {
    String::from_utf8_lossy(id.as_bytes()).into_owned()
}

fn unit_str(u: &[u8; 4]) -> String {
    String::from_utf8_lossy(u).into_owned()
}

pub(crate) fn from_craft_descriptor(d: craft_psd::descriptor::Descriptor) -> Descriptor {
    Descriptor {
        class_name: d.name.to_string_lossy(),
        class_id: id_str(&d.class_id),
        items: d.items.into_iter().map(|(k, v)| (id_str(&k), from_craft_value(v))).collect(),
    }
}

fn from_craft_value(v: craft_psd::descriptor::Value) -> DValue {
    use craft_psd::descriptor::Value;
    match v {
        Value::Descriptor(d) | Value::GlobalObject(d) => DValue::Descriptor(from_craft_descriptor(d)),
        Value::List(l) => DValue::List(l.into_iter().map(from_craft_value).collect()),
        Value::Double(x) => DValue::Double(if x.is_finite() { x } else { 0.0 }),
        Value::UnitFloat { unit, value } => DValue::UnitFloat(unit_str(&unit), if value.is_finite() { value } else { 0.0 }),
        Value::UnitFloats { unit, values } => DValue::UnitFloats(unit_str(&unit), values.into_iter().map(|x| if x.is_finite() { x } else { 0.0 }).collect()),
        Value::Text(s) => DValue::Text(s.to_string_lossy()),
        Value::Enumerated { type_id, value } => DValue::Enum(id_str(&type_id), id_str(&value)),
        Value::Integer(i) => DValue::Integer(i),
        Value::LargeInteger(i) => DValue::LargeInteger(i),
        Value::Boolean(b) => DValue::Bool(b),
        Value::Class(c) | Value::GlobalClass(c) => DValue::Class(c.name.to_string_lossy(), id_str(&c.class_id)),
        Value::Alias(b) | Value::Path(b) | Value::RawData(b) => DValue::Raw(b),
        Value::ObjectArray(o) => DValue::ObjectArray(o.prefix, from_craft_descriptor(o.body)),
        Value::Reference(items) => DValue::Reference(items.into_iter().map(from_craft_ref).collect()),
    }
}

fn legacy_reference_class(class: &craft_psd::descriptor::Class) -> bool {
    class.name.0 == [0] && class.class_id == craft_psd::descriptor::Id::Code(*b"null")
}

fn from_craft_ref(it: craft_psd::descriptor::ReferenceItem) -> RefItem {
    use craft_psd::descriptor::ReferenceItem;
    match it {
        ReferenceItem::Property { class, key } => RefItem::Property(id_str(&class.class_id), id_str(&key)),
        ReferenceItem::Class(c) => RefItem::Class(id_str(&c.class_id)),
        ReferenceItem::Enumerated { class, type_id, value } => RefItem::Enum(id_str(&class.class_id), id_str(&type_id), id_str(&value)),
        ReferenceItem::Offset { class, offset } => RefItem::Offset(id_str(&class.class_id), offset),
        ReferenceItem::Identifier { class, id } => {
            if legacy_reference_class(&class) {
                RefItem::Identifier(id as i32)
            } else {
                RefItem::IdentifierWithClass(class, id as i32)
            }
        }
        ReferenceItem::Index { class, index } => {
            if legacy_reference_class(&class) {
                RefItem::Index(index as i32)
            } else {
                RefItem::IndexWithClass(class, index as i32)
            }
        }
        ReferenceItem::Name { class, name } => RefItem::Name(id_str(&class.class_id), name.to_string_lossy()),
    }
}

// ---------------------------------------------------------------- writing

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
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

use craft_psd::descriptor::borrowed::{ClassRef, DescriptorView, IdRef, ReferenceRef, ReferencesView, UnicodeRef, ValueRef, ValuesView};

fn borrowed_id(id: &str) -> IdRef<'_> {
    match id.as_bytes() {
        [a, b, c, d] => IdRef::Code([*a, *b, *c, *d]),
        bytes => IdRef::String(bytes),
    }
}
fn borrowed_class<'a>(name: &'a str, id: &'a str) -> ClassRef<'a> {
    ClassRef { name: UnicodeRef::Text(name, true), id: borrowed_id(id) }
}
fn preserved_class(class: &craft_psd::descriptor::Class) -> ClassRef<'_> {
    ClassRef {
        name: UnicodeRef::Units(&class.name.0),
        id: match &class.class_id {
            craft_psd::descriptor::Id::Code(code) => IdRef::Code(*code),
            craft_psd::descriptor::Id::Str(bytes) => IdRef::String(bytes),
        },
    }
}
impl DescriptorView for Descriptor {
    fn class(&self) -> ClassRef<'_> {
        borrowed_class(&self.class_name, &self.class_id)
    }
    fn item(&self, index: usize) -> Option<(IdRef<'_>, ValueRef<'_>)> {
        self.items.get(index).map(|(k, v)| (borrowed_id(k), borrowed_value(v)))
    }
}
impl ValuesView for DValue {
    fn value(&self, index: usize) -> Option<ValueRef<'_>> {
        match self {
            DValue::List(items) => items.get(index).map(borrowed_value),
            _ => None,
        }
    }
}
impl ReferencesView for DValue {
    fn reference(&self, index: usize) -> Option<ReferenceRef<'_>> {
        let DValue::Reference(items) = self else { return None };
        Some(match items.get(index)? {
            RefItem::Property(c, k) => ReferenceRef::Property(borrowed_class("", c), borrowed_id(k)),
            RefItem::Class(c) => ReferenceRef::Class(borrowed_class("", c)),
            RefItem::Enum(c, t, v) => ReferenceRef::Enumerated(borrowed_class("", c), borrowed_id(t), borrowed_id(v)),
            RefItem::Offset(c, n) => ReferenceRef::Offset(borrowed_class("", c), *n),
            RefItem::Identifier(n) => ReferenceRef::Identifier(borrowed_class("", "null"), *n),
            RefItem::Index(n) => ReferenceRef::Index(borrowed_class("", "null"), *n),
            RefItem::IdentifierWithClass(c, n) => ReferenceRef::Identifier(preserved_class(c), *n),
            RefItem::IndexWithClass(c, n) => ReferenceRef::Index(preserved_class(c), *n),
            RefItem::Name(c, n) => ReferenceRef::Name(borrowed_class("", c), UnicodeRef::Text(n, true)),
        })
    }
}
fn borrowed_value(value: &DValue) -> ValueRef<'_> {
    match value {
        DValue::Descriptor(d) => ValueRef::Descriptor(d),
        DValue::List(_) => ValueRef::List(value),
        DValue::ObjectArray(n, d) => ValueRef::ObjectArray(*n, d),
        DValue::Double(n) => ValueRef::Double(*n),
        DValue::UnitFloat(u, n) => ValueRef::UnitFloat(tag4(u), *n),
        DValue::UnitFloats(u, ns) => ValueRef::UnitFloats(tag4(u), ns),
        DValue::Text(s) => ValueRef::Text(UnicodeRef::Text(s, true)),
        DValue::Enum(t, v) => ValueRef::Enumerated(borrowed_id(t), borrowed_id(v)),
        DValue::Integer(n) => ValueRef::Integer(*n),
        DValue::LargeInteger(n) => ValueRef::LargeInteger(*n),
        DValue::Bool(b) => ValueRef::Boolean(*b),
        DValue::Class(n, c) => ValueRef::Class(borrowed_class(n, c)),
        DValue::Raw(b) => ValueRef::RawData(b),
        DValue::Reference(_) => ValueRef::Reference(value),
    }
}

/// Serialise a descriptor (without the version field), appending to out.
pub fn write_descriptor(out: &mut Vec<u8>, d: &Descriptor) {
    craft_psd::descriptor::borrowed::append_descriptor(out, d);
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

#[cfg(test)]
mod writer_wire_tests {
    use super::*;
    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }
    fn check(value: DValue, payload: Vec<u8>) {
        let d = Descriptor::new("warp").with("time", value);
        let mut got = vec![0xaa, 0xbb];
        write_descriptor(&mut got, &d);
        // Empty name plus one NUL, Code(warp), one item, Code(time).
        let mut want = hex("aa bb 00 00 00 01 00 00 00 00 00 00 77 61 72 70 00 00 00 01 00 00 00 00 74 69 6d 65");
        want.extend(payload);
        assert_eq!(got, want);
    }
    #[test]
    fn all_scalar_wire_tags_keep_raw_bits_ids_and_unicode() {
        for (v, bytes) in [
            (DValue::Double(f64::from_bits(0x7ff8_1234_5678_9abc)), "64 6f 75 62 7f f8 12 34 56 78 9a bc"),
            (DValue::Double(-0.0), "64 6f 75 62 80 00 00 00 00 00 00 00"),
            (DValue::UnitFloat("x".into(), f64::INFINITY), "55 6e 74 46 78 20 20 20 7f f0 00 00 00 00 00 00"),
            (DValue::UnitFloats("abcde".into(), vec![-0.0, 1.0]), "55 6e 46 6c 61 62 63 64 00 00 00 02 80 00 00 00 00 00 00 00 3f f0 00 00 00 00 00 00"),
            (DValue::Text("A\0\0".into()), "54 45 58 54 00 00 00 04 00 41 00 00 00 00 00 00"),
            (DValue::Enum("list".into(), "".into()), "65 6e 75 6d 00 00 00 00 6c 69 73 74 00 00 00 00"),
            (DValue::Integer(i32::MIN), "6c 6f 6e 67 80 00 00 00"),
            (DValue::LargeInteger(i64::MIN), "63 6f 6d 70 80 00 00 00 00 00 00 00"),
            (DValue::Bool(true), "62 6f 6f 6c 01"),
            (DValue::Class("A".into(), "longer".into()), "74 79 70 65 00 00 00 02 00 41 00 00 00 00 00 06 6c 6f 6e 67 65 72"),
            (DValue::Raw(vec![0, 255, 3]), "74 64 74 61 00 00 00 03 00 ff 03"),
        ] {
            check(v, hex(bytes));
        }
    }
    #[test]
    fn children_and_object_array_prefix_keep_their_own_counts() {
        let empty = Descriptor::new("null");
        let body = hex("00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 00");
        let mut object = b"Objc".to_vec();
        object.extend(&body);
        check(DValue::Descriptor(empty.clone()), object);
        let mut array = hex("4f 62 41 72 12 34 56 78");
        array.extend(&body);
        check(DValue::ObjectArray(0x1234_5678, empty), array);
        check(DValue::List(vec![DValue::List(vec![]), DValue::Bool(false)]), hex("56 6c 4c 73 00 00 00 02 56 6c 4c 73 00 00 00 00 62 6f 6f 6c 00"));
    }
    #[test]
    fn reference_variants_preserve_class_encoding_and_signed_fields() {
        let class = hex("00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c");
        for (r, tag, tail) in [
            (RefItem::Property("null".into(), "warp".into()), "prop", hex("00 00 00 00 77 61 72 70")),
            (RefItem::Class("null".into()), "Clss", vec![]),
            (RefItem::Enum("null".into(), "time".into(), "list".into()), "Enmr", hex("00 00 00 00 74 69 6d 65 00 00 00 00 6c 69 73 74")),
            (RefItem::Offset("null".into(), -1), "rele", vec![255; 4]),
            (RefItem::Identifier(-1), "Idnt", vec![255; 4]),
            (RefItem::Index(-1), "indx", vec![255; 4]),
            (RefItem::Name("null".into(), "A\0".into()), "name", hex("00 00 00 03 00 41 00 00 00 00")),
        ] {
            let mut payload = hex("6f 62 6a 20 00 00 00 01");
            payload.extend(tag.as_bytes());
            payload.extend(&class);
            payload.extend(tail);
            check(DValue::Reference(vec![r]), payload);
        }
        let raw = craft_psd::descriptor::Class {
            name: craft_psd::descriptor::UnicodeString(vec![0xd800, 0, 0]),
            class_id: craft_psd::descriptor::Id::Str(b"warp".to_vec()),
        };
        for (r, tag) in [(RefItem::IdentifierWithClass(raw.clone(), 7), "Idnt"), (RefItem::IndexWithClass(raw, 7), "indx")] {
            let mut payload = hex("6f 62 6a 20 00 00 00 01");
            payload.extend(tag.as_bytes());
            payload.extend(hex("00 00 00 03 d8 00 00 00 00 00 00 00 00 04 77 61 72 70 00 00 00 07"));
            check(DValue::Reference(vec![r]), payload);
        }
    }
    #[test]
    fn borrowed_large_payload_and_duplicate_items_remain_in_order() {
        let raw = vec![0x65; 1024 * 1024];
        let d = Descriptor::new("null").with("same", DValue::Raw(raw)).with("same", DValue::Integer(9));
        let Some((_, ValueRef::RawData(bytes))) = d.item(0) else {
            panic!("raw");
        };
        let DValue::Raw(original) = &d.items[0].1 else {
            panic!("raw");
        };
        assert_eq!(bytes.as_ptr(), original.as_ptr());
        let mut out = vec![];
        write_descriptor(&mut out, &d);
        assert_eq!(&out[out.len() - 12..], &hex("73 61 6d 65 6c 6f 6e 67 00 00 00 09"));
    }

    #[test]
    fn unicode_byte_ids_and_split_unit_tags_are_literal() {
        check(DValue::Text("\u{1f600}".into()), hex("54 45 58 54 00 00 00 03 d8 3d de 00 00 00"));
        check(DValue::UnitFloat("abc\u{e9}".into(), 1.0), hex("55 6e 74 46 61 62 63 c3 3f f0 00 00 00 00 00 00"));
        let d = Descriptor::new("\u{e9}\u{e9}").with("\u{1f600}", DValue::Bool(false));
        let mut out = vec![0xa5];
        write_descriptor(&mut out, &d);
        assert_eq!(out, hex("a5 00 00 00 01 00 00 00 00 00 00 c3 a9 c3 a9 00 00 00 01 00 00 00 00 f0 9f 98 80 62 6f 6f 6c 00"));
    }

    #[test]
    fn nonempty_children_resume_each_parent_after_the_child() {
        let child = || Descriptor::new("null").with("leaf", DValue::Integer(7)).with("done", DValue::Bool(true));
        check(
            DValue::List(vec![DValue::Descriptor(child()), DValue::ObjectArray(0x1234_5678, child()), DValue::Bool(false)]),
            hex("56 6c 4c 73 00 00 00 03
                 4f 62 6a 63 00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 02
                 00 00 00 00 6c 65 61 66 6c 6f 6e 67 00 00 00 07
                 00 00 00 00 64 6f 6e 65 62 6f 6f 6c 01
                 4f 62 41 72 12 34 56 78 00 00 00 01 00 00 00 00 00 00 6e 75 6c 6c 00 00 00 02
                 00 00 00 00 6c 65 61 66 6c 6f 6e 67 00 00 00 07
                 00 00 00 00 64 6f 6e 65 62 6f 6f 6c 01
                 62 6f 6f 6c 00"),
        );
    }

    #[test]
    fn large_lists_and_unit_float_slices_are_borrowed() {
        let values = DValue::List((0..65536).map(DValue::Integer).collect());
        let ValueRef::List(view) = borrowed_value(&values) else { panic!("list") };
        assert_eq!(view as *const dyn ValuesView as *const (), &values as *const DValue as *const ());
        assert!(matches!(view.value(65535), Some(ValueRef::Integer(65535))));
        assert!(view.value(65536).is_none());
        let units = DValue::UnitFloats("#Pxl".into(), vec![-0.0; 65536]);
        let ValueRef::UnitFloats(_, slice) = borrowed_value(&units) else { panic!("units") };
        let DValue::UnitFloats(_, original) = &units else { panic!("units") };
        assert_eq!(slice.as_ptr(), original.as_ptr());
        let d = Descriptor::new("null").with("list", values).with("unit", units);
        let mut out = vec![];
        write_descriptor(&mut out, &d);
        // Header18 + list key/tag/count16, 65536 scalar tag/value pairs,
        // unit key/tag/unit/count20, 65536 raw doubles.
        assert_eq!(out.len(), 18 + 16 + 65536 * 8 + 20 + 65536 * 8);
        assert_eq!(&out[30..34], &[0, 1, 0, 0]);
        assert_eq!(&out[out.len() - 8..], &[128, 0, 0, 0, 0, 0, 0, 0]);
    }
}
