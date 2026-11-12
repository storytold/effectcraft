//! A cheap check that a project file will read back, without parsing it.
//!
//! `serde_json` writes a non-finite float as `null` and refuses to parse nesting deeper than 128
//! containers, so a project holding either would be saved to a file that can't be opened. Walking
//! the value once, writing nothing, finds both without building the project again.

use std::fmt;

use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple, SerializeTupleStruct, SerializeTupleVariant,
    Serializer,
};

/// serde_json refuses to parse the 128th nested array/object (its recursion limit is 128).
const MAX_DEPTH: u32 = 127;

const NOT_FINITE: &str = "a number that isn't finite (NaN or infinity)";
const TOO_DEEP: &str = "nesting deeper than 127 levels";

/// Why a value can't be written to a project file.
#[derive(Debug)]
pub(crate) struct Unstorable(String);

impl fmt::Display for Unstorable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unstorable {}

impl ser::Error for Unstorable {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Unstorable(msg.to_string())
    }
}

/// Walk `value` as `serde_json` would write it, producing no text and allocating only on error.
pub(crate) fn check<T: Serialize + ?Sized>(value: &T) -> Result<(), Unstorable> {
    let mut walker = Walker { depth: 0 };
    value.serialize(&mut walker)
}

/// The current nesting depth of the value being walked.
struct Walker {
    depth: u32,
}

impl Walker {
    /// Enter `levels` nested containers, refusing anything serde_json couldn't parse back.
    fn enter(&mut self, levels: u32) -> Result<(), Unstorable> {
        let depth = self.depth.checked_add(levels).ok_or_else(|| Unstorable(TOO_DEEP.into()))?;
        if depth > MAX_DEPTH {
            return Err(Unstorable(TOO_DEEP.into()));
        }
        self.depth = depth;
        Ok(())
    }
    fn leave(&mut self, levels: u32) {
        self.depth = self.depth.saturating_sub(levels);
    }
}

/// One compound value being walked; leaving it when it ends keeps the depth exact.
struct Compound<'a> {
    walker: &'a mut Walker,
    levels: u32,
}

impl<'a> Compound<'a> {
    fn new(walker: &'a mut Walker, levels: u32) -> Result<Self, Unstorable> {
        walker.enter(levels)?;
        Ok(Self { walker, levels })
    }
    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        value.serialize(&mut *self.walker)
    }
    fn finish(self) -> Result<(), Unstorable> {
        self.walker.leave(self.levels);
        Ok(())
    }
}

impl SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unstorable> {
        self.value(key)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, _key: &'static str, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = Unstorable;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, _key: &'static str, value: &T) -> Result<(), Unstorable> {
        self.value(value)
    }
    fn end(self) -> Result<(), Unstorable> {
        self.finish()
    }
}

impl<'a> Serializer for &'a mut Walker {
    type Ok = ();
    type Error = Unstorable;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_bool(self, _v: bool) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_i8(self, _v: i8) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_i16(self, _v: i16) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_i32(self, _v: i32) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_i64(self, _v: i64) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_i128(self, _v: i128) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_u8(self, _v: u8) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_u16(self, _v: u16) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_u32(self, _v: u32) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_u64(self, _v: u64) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_u128(self, _v: u128) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), Unstorable> {
        if v.is_finite() { Ok(()) } else { Err(Unstorable(NOT_FINITE.into())) }
    }
    fn serialize_f64(self, v: f64) -> Result<(), Unstorable> {
        if v.is_finite() { Ok(()) } else { Err(Unstorable(NOT_FINITE.into())) }
    }
    fn serialize_char(self, _v: char) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_str(self, _v: &str) -> Result<(), Unstorable> {
        Ok(())
    }
    /// serde_json writes bytes as an array of numbers: one level of nesting.
    fn serialize_bytes(self, _v: &[u8]) -> Result<(), Unstorable> {
        Compound::new(self, 1)?.finish()
    }
    /// Writes text for a `Display` value; it is a scalar, not a container.
    fn collect_str<T: fmt::Display + ?Sized>(self, _v: &T) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Unstorable> {
        value.serialize(&mut *self)
    }
    fn serialize_unit(self) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_unit_variant(self, _name: &'static str, _index: u32, _variant: &'static str) -> Result<(), Unstorable> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<(), Unstorable> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _name: &'static str, _index: u32, _variant: &'static str, value: &T) -> Result<(), Unstorable> {
        let mut compound = Compound::new(self, 1)?;
        compound.value(value)?;
        compound.finish()
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 1)
    }
    fn serialize_tuple(self, _len: usize) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 1)
    }
    fn serialize_tuple_struct(self, _name: &'static str, _len: usize) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 1)
    }
    fn serialize_tuple_variant(self, _name: &'static str, _index: u32, _variant: &'static str, _len: usize) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 2)
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 1)
    }
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 1)
    }
    fn serialize_struct_variant(self, _name: &'static str, _index: u32, _variant: &'static str, _len: usize) -> Result<Compound<'a>, Unstorable> {
        Compound::new(self, 2)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::check;

    #[test]
    fn refuses_non_finite_numbers_anywhere() {
        assert!(check(&1.5f64).is_ok());
        assert!(check(&vec![Some(1.0f64), None]).is_ok());
        assert!(check(&BTreeMap::from([("a".to_string(), 2.0f32)])).is_ok(), "finite floats in a map");
        assert!(check(&f64::NAN).is_err());
        assert!(check(&f64::INFINITY).is_err());
        assert!(check(&vec![Some(f64::NEG_INFINITY)]).is_err(), "inside an option in a list");
        assert!(check(&BTreeMap::from([("a".to_string(), f32::NAN)])).is_err(), "as a map value");
    }

    #[test]
    fn depth_limit_matches_serde_json() {
        for n in [127, 128] {
            let mut v = serde_json::Value::Null;
            for _ in 0..n {
                v = serde_json::Value::Array(vec![v]);
            }
            let text = serde_json::to_string(&v).unwrap();
            let reads_back = serde_json::from_str::<serde_json::Value>(&text).is_ok();
            assert_eq!(check(&v).is_ok(), reads_back, "depth {n} agrees with serde_json");
            assert_eq!(check(&v).is_ok(), n == 127, "depth {n}");
        }
    }

    /// `n` nested lists around bytes, which serde_json writes as one more array.
    struct BytesUnder(u32);

    impl serde::Serialize for BytesUnder {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeSeq;
            let Some(inner) = self.0.checked_sub(1) else {
                return s.serialize_bytes(&[1]);
            };
            let mut seq = s.serialize_seq(Some(1))?;
            seq.serialize_element(&BytesUnder(inner))?;
            seq.end()
        }
    }

    #[test]
    fn bytes_count_as_one_level() {
        for n in [126, 127] {
            let text = serde_json::to_string(&BytesUnder(n)).unwrap();
            let reads_back = serde_json::from_str::<serde_json::Value>(&text).is_ok();
            assert_eq!(check(&BytesUnder(n)).is_ok(), reads_back, "depth {n} agrees with serde_json");
            assert_eq!(check(&BytesUnder(n)).is_ok(), n == 126, "depth {n}");
        }
    }
}
