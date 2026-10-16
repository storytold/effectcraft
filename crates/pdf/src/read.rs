//! Adapter from the shared PDF syntax API to the existing drawing interpreter's object model.
//! Only reachable objects are retained, references keep their generation, and native parser
//! panics (including lazy object reads) discard the entire partially populated adapter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use craft_pdf::syntax::object::{Dict as PdfDict, MaybeRef, Name, ObjRef, Object, ObjectIdentifier, Stream};
use craft_pdf::syntax::xref::XRef;

use crate::Error;
use crate::object::{Dict, File, Lexer, Obj};

const MAX_OBJECTS: usize = 100_000;
const MAX_VALUES: usize = 1_000_000;
const MAX_DEPTH: usize = 64;
const MAX_BYTES: usize = 512 * 1024 * 1024;

fn limit(what: &str) -> Error {
    Error::Read(format!("PDF exceeds the {what} import limit"))
}

fn allocation() -> Error {
    Error::Read("PDF object storage could not be allocated".into())
}

fn open_options() -> craft_pdf::OpenOptions {
    craft_pdf::OpenOptions {
        decode_limits: craft_pdf::syntax::DecodeLimits::default().with_max_decoded_stream(crate::object::MAX_OBJECT_STREAM),
        ..Default::default()
    }
}

pub(crate) fn open(bytes: &[u8]) -> Result<File, Error> {
    craft_pdf::check_input_size_with(bytes.len(), &open_options()).map_err(|e| Error::Read(e.to_string()))?;
    catch_unwind(AssertUnwindSafe(|| open_inner(bytes))).map_err(|_| Error::Read("PDF parser failed internally".into()))?
}

fn open_inner(bytes: &[u8]) -> Result<File, Error> {
    // Preserve the app's refusal of encrypted PDFs, including empty-user-password files.
    // Classic trailer dictionaries use the existing lexer; xref-stream dictionaries below
    // are read by the shared parser. This does not parse document objects a second time.
    let mut markers = 0;
    for (offset, word) in bytes.windows(7).enumerate() {
        if word == b"trailer" {
            markers += 1;
            if markers > MAX_OBJECTS {
                return Err(limit("trailer count"));
            }
            if let Some(Obj::Dict(dict)) = Lexer::new(bytes, offset + 7).next()
                && dict.contains_key("Encrypt")
            {
                return Err(Error::Encrypted);
            }
        }
    }
    let pdf = craft_pdf::open_slice_with_options(bytes, None, &open_options()).map_err(|error| match error {
        craft_pdf::OpenError::Load(craft_pdf::syntax::LoadPdfError::Decryption(_)) => Error::Encrypted,
        craft_pdf::OpenError::Load(craft_pdf::syntax::LoadPdfError::Invalid) => Error::NoPages,
        other => Error::Read(other.to_string()),
    })?;
    let mut object_streams = Vec::new();
    for (index, object) in pdf.objects().into_iter().enumerate() {
        if index >= MAX_OBJECTS {
            return Err(limit("100000 objects"));
        }
        if let Object::Stream(stream) = object {
            let dict = stream.dict();
            if dict.get::<Name<'_>>(b"Type").is_some_and(|name| name.as_ref() == b"XRef") && dict.contains_key(b"Encrypt") {
                return Err(Error::Encrypted);
            }
            if dict.get::<Name<'_>>(b"Type").is_some_and(|name| name.as_ref() == b"ObjStm") {
                object_streams.try_reserve(1).map_err(|_| allocation())?;
                object_streams.push(stream);
            }
        }
    }
    let mut reader = Reader {
        xref: pdf.xref(),
        pending: VecDeque::new(),
        seen: HashSet::new(),
        values_left: MAX_VALUES,
        bytes_left: MAX_BYTES,
        object_streams,
        recovery: None,
    };
    let root = pdf.xref().root_id();
    let root = reader.reference(ObjRef::new(root.obj_number, root.gen_number))?;
    let mut file = File { objects: HashMap::new(), trailers: vec![Dict::from([("Root".into(), root)])], repairs: Vec::new() };
    while let Some(id) = reader.pending.pop_front() {
        let Some(key) = key(id) else { continue };
        let object = match reader.xref.get::<Object<'_>>(id) {
            Some(value) => reader.object(value, 0)?,
            None => {
                let recovered = reader.recover(id)?;
                if file.repairs.len() < 64 {
                    let action = if recovered.is_some() { "recovered PDF object from an object stream" } else { "unresolved PDF object" };
                    file.repairs.push(format!("{action}: {} {}", id.obj_number, id.gen_number));
                }
                recovered.unwrap_or(Obj::Null)
            }
        };
        file.objects.try_reserve(1).map_err(|_| allocation())?;
        file.objects.insert(key, object);
    }
    Ok(file)
}

fn key(id: ObjectIdentifier) -> Option<(u32, u16)> {
    Some((u32::try_from(id.obj_number).ok()?, u16::try_from(id.gen_number).ok()?))
}

struct Reader<'a> {
    xref: &'a XRef,
    pending: VecDeque<ObjectIdentifier>,
    seen: HashSet<ObjectIdentifier>,
    values_left: usize,
    bytes_left: usize,
    object_streams: Vec<Stream<'a>>,
    recovery: Option<HashMap<u32, Option<Recovered>>>,
}

struct Recovered {
    bytes: Arc<Vec<u8>>,
    start: usize,
    end: usize,
}

impl Reader<'_> {
    fn value(&mut self) -> Result<(), Error> {
        self.values_left = self.values_left.checked_sub(1).ok_or_else(|| limit("1000000 values"))?;
        Ok(())
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<Vec<u8>, Error> {
        self.bytes_left = self.bytes_left.checked_sub(bytes.len()).ok_or_else(|| limit("512 MiB object data"))?;
        let mut out = Vec::new();
        out.try_reserve_exact(bytes.len()).map_err(|_| allocation())?;
        out.extend_from_slice(bytes);
        Ok(out)
    }

    /// The previous importer could recover compressed objects incorrectly marked free in
    /// an otherwise readable xref. Retain that narrow repair only for unresolved references,
    /// with a warning, bounded decoding and the existing content/object lexer.
    fn recover(&mut self, id: ObjectIdentifier) -> Result<Option<Obj>, Error> {
        if id.gen_number != 0 {
            return Ok(None);
        }
        if self.recovery.is_none() {
            let mut recovered: HashMap<u32, Option<Recovered>> = HashMap::new();
            for stream in std::mem::take(&mut self.object_streams) {
                let Some(count) = stream.dict().get::<u32>(b"N") else { continue };
                let Some(first) = stream.dict().get::<u32>(b"First") else { continue };
                if count as usize > MAX_OBJECTS.saturating_sub(recovered.len()) {
                    return Err(limit("100000 recovered objects"));
                }
                let decoded = match stream.decoded_within(self.bytes_left.min(crate::object::MAX_OBJECT_STREAM)) {
                    Ok(data) => data,
                    Err(_) => continue,
                };
                self.bytes_left = self.bytes_left.checked_sub(decoded.len()).ok_or_else(|| limit("512 MiB object data"))?;
                let decoded = Arc::new(decoded.into_owned());
                let mut lexer = Lexer::new(&decoded, 0);
                let mut offsets = Vec::new();
                for _ in 0..count {
                    self.value()?;
                    let (Some(Obj::Num(number)), Some(Obj::Num(offset))) = (lexer.next(), lexer.next()) else { break };
                    if !number.is_finite() || number.fract() != 0.0 || !(0.0..=f64::from(u32::MAX)).contains(&number) {
                        continue;
                    }
                    if !offset.is_finite() || offset.fract() != 0.0 || offset < 0.0 || offset > decoded.len() as f64 {
                        continue;
                    }
                    let Some(start) = (first as usize).checked_add(offset as usize).filter(|start| *start < decoded.len()) else { continue };
                    offsets.try_reserve(1).map_err(|_| allocation())?;
                    offsets.push((start, number as u32));
                }
                offsets.sort_unstable();
                for (index, &(start, number)) in offsets.iter().enumerate() {
                    let end = offsets.get(index + 1).map_or(decoded.len(), |(next, _)| *next);
                    recovered.try_reserve(1).map_err(|_| allocation())?;
                    recovered.entry(number).and_modify(|entry| *entry = None).or_insert_with(|| Some(Recovered { bytes: Arc::clone(&decoded), start, end }));
                }
            }
            self.recovery = Some(recovered);
        }
        let Some(number) = u32::try_from(id.obj_number).ok() else { return Ok(None) };
        let Some(location) = self.recovery.as_ref().and_then(|r| r.get(&number)) else { return Ok(None) };
        let Some(location) = location else { return Err(Error::Read(format!("PDF has ambiguous recovered object {number}"))) };
        let bytes = Arc::clone(&location.bytes);
        let Some(bytes) = bytes.get(location.start..location.end) else { return Ok(None) };
        let mut lexer = Lexer::new(bytes, 0);
        let Some(object) = lexer.next() else { return Ok(None) };
        let object = lexer.maybe_ref(object);
        self.register_recovered(&object, 0)?;
        Ok(Some(object))
    }

    fn register_recovered(&mut self, object: &Obj, depth: usize) -> Result<(), Error> {
        self.value()?;
        if depth > MAX_DEPTH {
            return Err(limit("64 nested objects"));
        }
        let bytes = match object {
            Obj::Name(text) | Obj::Op(text) => text.len(),
            Obj::Str(bytes) => bytes.len(),
            Obj::Array(values) => {
                for value in values {
                    self.register_recovered(value, depth + 1)?;
                }
                0
            }
            Obj::Dict(dict) | Obj::Stream(dict, _) => {
                for (name, value) in dict {
                    self.bytes_left = self.bytes_left.checked_sub(name.len()).ok_or_else(|| limit("512 MiB object data"))?;
                    self.register_recovered(value, depth + 1)?;
                }
                0
            }
            Obj::Ref(number, generation) => {
                if let Ok(number) = i32::try_from(*number) {
                    self.reference(ObjRef::new(number, i32::from(*generation)))?;
                }
                0
            }
            _ => 0,
        };
        self.bytes_left = self.bytes_left.checked_sub(bytes).ok_or_else(|| limit("512 MiB object data"))?;
        Ok(())
    }

    fn name(&mut self, bytes: &[u8]) -> Result<String, Error> {
        // Lossy UTF-8 expansion is at most three output bytes per source byte.
        let bound = bytes.len().checked_mul(3).ok_or_else(|| limit("name length"))?;
        self.bytes_left = self.bytes_left.checked_sub(bound).ok_or_else(|| limit("512 MiB object data"))?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn reference(&mut self, reference: ObjRef) -> Result<Obj, Error> {
        self.value()?;
        let id = ObjectIdentifier::new(reference.obj_number, reference.gen_number);
        let Some((number, generation)) = key(id) else { return Ok(Obj::Null) };
        if !self.seen.contains(&id) {
            if self.seen.len() >= MAX_OBJECTS {
                return Err(limit("100000 referenced objects"));
            }
            self.seen.try_reserve(1).map_err(|_| allocation())?;
            self.pending.try_reserve(1).map_err(|_| allocation())?;
            self.seen.insert(id);
            self.pending.push_back(id);
        }
        Ok(Obj::Ref(number, generation))
    }

    fn maybe(&mut self, object: MaybeRef<Object<'_>>, depth: usize) -> Result<Obj, Error> {
        match object {
            MaybeRef::Ref(reference) => self.reference(reference),
            MaybeRef::NotRef(object) => self.object(object, depth),
        }
    }

    fn dictionary(&mut self, dict: &PdfDict<'_>, depth: usize) -> Result<Dict, Error> {
        let mut out = Dict::new();
        for (name, object) in dict.entries() {
            let name = self.name(name.as_ref())?;
            let object = self.maybe(object, depth + 1)?;
            out.try_reserve(1).map_err(|_| allocation())?;
            out.insert(name, object);
        }
        Ok(out)
    }

    fn object(&mut self, object: Object<'_>, depth: usize) -> Result<Obj, Error> {
        self.value()?;
        if depth > MAX_DEPTH {
            return Err(limit("64 nested objects"));
        }
        Ok(match object {
            Object::Null(_) => Obj::Null,
            Object::Boolean(value) => Obj::Bool(value),
            Object::Number(value) => Obj::Num(value.as_f64()),
            Object::String(value) => Obj::Str(self.bytes(value.as_bytes())?),
            Object::Name(value) => Obj::Name(self.name(value.as_ref())?),
            Object::Array(array) => {
                let mut out = Vec::new();
                for value in array.raw_iter() {
                    let value = self.maybe(value, depth + 1)?;
                    out.try_reserve(1).map_err(|_| allocation())?;
                    out.push(value);
                }
                Obj::Array(out)
            }
            Object::Dict(dict) => Obj::Dict(self.dictionary(&dict, depth)?),
            Object::Stream(stream) => {
                let dict = self.dictionary(stream.dict(), depth)?;
                Obj::Stream(dict, self.bytes(&stream.raw_data())?)
            }
        })
    }
}

#[cfg(test)]
mod limit_tests {
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn shared_reader_preserves_tolerant_array_numbers_and_later_pages() {
        use crate::object::Obj;

        // Array validation accepts these sign/dot-only zero tokens. Previously '+'
        // reached an unchecked lazy-iterator read: native loading returned an internal
        // parser error and WASM could abort before reaching the following object.
        for token in ["+", "-", ".", "+.", "-."] {
            for numeric_kid in [false, true] {
                let kids = if numeric_kid { format!("{token} 3 0 R") } else { "3 0 R".into() };
                let bytes = crate::write::pdf(
                    &[
                        (1, format!("<< /Type /Catalog /Pages 2 0 R /Probe [{token} 3] >>"), None),
                        (2, format!("<< /Type /Pages /Kids [{kids}] /Count 1 >>"), None),
                        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 20] /Resources << >> /Contents 4 0 R >>".into(), None),
                        (4, "<< >>".into(), Some(b"1 0 0 rg 0 0 40 20 re f\n".to_vec())),
                    ],
                    1,
                );
                let file = super::open(&bytes).unwrap();
                assert_eq!(file.catalog().unwrap().get("Probe").and_then(Obj::array), Some([Obj::Num(0.0), Obj::Num(3.0)].as_slice()));
                assert_eq!(crate::page_count(&bytes), 1, "token {token}, numeric kid {numeric_kid}");
                let doc = crate::parse(&bytes).unwrap();
                assert_eq!((doc.width, doc.height), (40.0, 20.0));
                assert_eq!(doc.flatten().len(), 1);
                assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
                assert_eq!(effectcraft_svg::rasterize(&doc, 40, 20, 1.0).get(20, 10), [1.0, 0.0, 0.0, 1.0]);
                assert_eq!(crate::parse_page(&bytes, 1), Err(crate::Error::PageOutOfRange(1)));
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn shared_reader_preserves_the_target_stream_budget() {
        let options = super::open_options();
        let expected = if cfg!(target_pointer_width = "32") { 64 << 20 } else { 256 << 20 };
        assert_eq!(options.decode_limits.max_decoded_stream(), expected);
        assert_eq!(options.max_input_bytes, 512 << 20);
        let bytes = b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 40 20] >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";
        let pdf = craft_pdf::open_slice_with_options(bytes, None, &options).unwrap();
        assert_eq!(pdf.decode_limits().max_decoded_stream(), expected);
    }
}
