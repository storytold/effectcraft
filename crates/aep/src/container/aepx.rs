//! Reading the XML form of a project (`.aepx`) into the same chunk tree as an `.aep`.
//!
//! An `.aepx` is the `RIFX` chunk tree written as XML: one element per chunk, named by the chunk
//! id (or the list type of a `LIST`), with the body as hex in a `bdata` attribute. A few chunks
//! are written as text instead (`Utf8` as `<string>`, the numbers `numS` and `ppSn`, the footage
//! alias `alas` as `<fileReference>`), and the XMP packet that follows the container of an `.aep`
//! is the element `<ProjectXMPMetadata>`. The mapping, with the cases it was found on, is in
//! `docs/aep-format.md` (section 1.1).
//!
//! The reader is a small XML tokenizer for exactly that subset: elements, attributes, text, the
//! five predefined entities and character references, CDATA, comments and processing
//! instructions. A document type declaration is refused (no entity expansion). It runs without
//! recursion; the nesting depth and the element count are bounded by [`Limits`], and everything
//! it allocates is bounded by the size of the input.

use std::borrow::Cow;

use crate::container::chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS};
use crate::container::error::AepError;
use crate::container::read::{Limits, parse};

/// The root element of an `.aepx`.
const ROOT: &[u8] = b"AfterEffectsProject";
/// The element that holds the XMP packet (the trailer of an `.aep`).
const XMP: &[u8] = b"ProjectXMPMetadata";
/// The element of a `Utf8` chunk.
const STRING: &[u8] = b"string";
/// The element of an `alas` chunk (footage alias), written as attributes instead of JSON.
const FILE_REFERENCE: &[u8] = b"fileReference";

/// List types whose body is not a chunk sequence: the `.aepx` writes them like data chunks (with
/// `bdata`), the `.aep` as an opaque `LIST`.
const OPAQUE_LIST_TYPES: [[u8; 4]; 1] = [*b"btdk"];

/// True when `data` looks like an `.aepx`: optional UTF-8 byte order mark and white space, then
/// an XML declaration or the `AfterEffectsProject` root element.
pub fn is_aepx(data: &[u8]) -> bool {
    let rest = data.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(data);
    let start = rest.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(rest.len());
    let rest = rest.get(start..).unwrap_or(&[]);
    rest.starts_with(b"<?xml") || rest.starts_with(b"<AfterEffectsProject")
}

/// Reads an `.aep` or an `.aepx`, chosen by content: a `RIFX` file goes to [`parse`], an XML file
/// (see [`is_aepx`]) to [`read_aepx`].
///
/// # Errors
/// The errors of [`parse`] or [`read_aepx`].
pub fn parse_any(data: &[u8]) -> Result<AepFile<'_>, AepError> {
    if is_aepx(data) { read_aepx(data) } else { parse(data) }
}

/// Reads an `.aepx` with the default [`Limits`] into the chunk tree an `.aep` of the same project
/// gives.
///
/// # Errors
/// [`AepError::Xml`] for input that is not well-formed XML of the `.aepx` subset (bad UTF-8, an
/// unknown entity, an unclosed or mismatched tag, invalid or odd-length hex, a number that does
/// not parse, a document type declaration, a root element other than `AfterEffectsProject`);
/// [`AepError::TooDeep`] / [`AepError::TooManyChunks`] beyond the limits.
pub fn read_aepx(data: &[u8]) -> Result<AepFile<'static>, AepError> {
    read_aepx_with_limits(data, Limits::default())
}

/// Like [`read_aepx`] with explicit [`Limits`]: `max_depth` bounds the element nesting (the root
/// element's children are depth 1), `max_chunks` the number of elements.
///
/// # Errors
/// See [`read_aepx`].
pub fn read_aepx_with_limits(data: &[u8], limits: Limits) -> Result<AepFile<'static>, AepError> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t.as_bytes(),
        Err(e) => return Err(xml(e.valid_up_to(), "the file is not valid UTF-8")),
    };
    Reader { data: text, pos: 0, limits, count: 0, stack: Vec::new(), top: None, trailer: None }.run()
}

fn xml(offset: usize, what: &'static str) -> AepError {
    AepError::Xml { offset, what }
}

/// What an open element becomes when it closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The root element: its children are the top-level chunks.
    Root,
    /// A chunk with this id (or a `LIST` with this list type).
    Chunk([u8; 4]),
    /// A `Utf8` chunk.
    Utf8,
    /// An `alas` chunk built from the attributes.
    Alias,
    /// The XMP packet (only directly below the root).
    Xmp,
    /// An element without a chunk counterpart (a name that is no chunk id); skipped with its content.
    Skip,
}

struct Frame {
    kind: Kind,
    /// Offset of the start tag (for errors).
    at: usize,
    /// The name as written, to match the end tag.
    name: (usize, usize),
    /// The decoded `bdata` attribute.
    bdata: Option<Vec<u8>>,
    /// The attributes of a `fileReference`, in document order.
    attrs: Vec<(String, String)>,
    /// Collected text (only for `string` and the XMP packet).
    text: Vec<u8>,
    children: Vec<Chunk<'static>>,
}

struct Reader<'d> {
    data: &'d [u8],
    pos: usize,
    limits: Limits,
    count: usize,
    stack: Vec<Frame>,
    /// The top-level chunks once the root element has closed.
    top: Option<Vec<Chunk<'static>>>,
    trailer: Option<Vec<u8>>,
}

impl Reader<'_> {
    fn run(mut self) -> Result<AepFile<'static>, AepError> {
        if self.data.starts_with(&[0xef, 0xbb, 0xbf]) {
            self.pos = 3;
        }
        while self.pos < self.data.len() {
            if self.rest().starts_with(b"<") {
                self.markup()?;
            } else {
                self.text()?;
            }
        }
        if let Some(open) = self.stack.last() {
            return Err(xml(open.at, "an element is not closed before the end of the file"));
        }
        let chunks = self.top.ok_or_else(|| xml(self.data.len(), "no AfterEffectsProject root element"))?;
        Ok(AepFile { chunks, trailer: Cow::Owned(self.trailer.unwrap_or_default()) })
    }

    fn rest(&self) -> &[u8] {
        self.data.get(self.pos..).unwrap_or(&[])
    }

    /// The offset of `needle` at or after `from`.
    fn find(&self, from: usize, needle: &[u8]) -> Option<usize> {
        let hay = self.data.get(from..)?;
        if needle.is_empty() || hay.len() < needle.len() {
            return None;
        }
        let first = *needle.first()?;
        let mut i = 0;
        while let Some(p) = hay.get(i..)?.iter().position(|b| *b == first) {
            let at = i.checked_add(p)?;
            if hay.get(at..)?.starts_with(needle) {
                return from.checked_add(at);
            }
            i = at.checked_add(1)?;
        }
        None
    }

    /// Skips to just after `close`, which must follow.
    fn skip_past(&mut self, close: &[u8], what: &'static str) -> Result<usize, AepError> {
        let at = self.find(self.pos, close).ok_or(xml(self.pos, what))?;
        self.pos = at.saturating_add(close.len());
        Ok(at)
    }

    fn markup(&mut self) -> Result<(), AepError> {
        let rest = self.rest();
        if rest.starts_with(b"<?") {
            self.skip_past(b"?>", "a processing instruction is not closed")?;
        } else if rest.starts_with(b"<!--") {
            self.skip_past(b"-->", "a comment is not closed")?;
        } else if rest.starts_with(b"<![CDATA[") {
            let start = self.pos.saturating_add(9);
            self.pos = start;
            let end = self.skip_past(b"]]>", "a CDATA section is not closed")?;
            let content = self.data.get(start..end).unwrap_or(&[]);
            let at = start;
            match self.stack.last_mut() {
                Some(frame) => {
                    if matches!(frame.kind, Kind::Utf8 | Kind::Xmp) {
                        frame.text.extend_from_slice(content);
                    }
                }
                None => return Err(xml(at, "CDATA outside the root element")),
            }
        } else if rest.starts_with(b"<!") {
            return Err(xml(self.pos, "document type declarations are not supported"));
        } else if rest.starts_with(b"</") {
            self.end_tag()?;
        } else {
            self.start_tag()?;
        }
        Ok(())
    }

    fn is_name_byte(b: u8) -> bool {
        !(b.is_ascii_whitespace() || matches!(b, b'<' | b'>' | b'/' | b'=' | b'"' | b'\'' | b'&'))
    }

    fn name(&mut self) -> Result<(usize, usize), AepError> {
        let start = self.pos;
        let len = self.rest().iter().position(|b| !Self::is_name_byte(*b)).unwrap_or(self.rest().len());
        if len == 0 {
            return Err(xml(start, "a tag or attribute has no name"));
        }
        self.pos = start.saturating_add(len);
        Ok((start, self.pos))
    }

    fn skip_ws(&mut self) {
        let n = self.rest().iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(self.rest().len());
        self.pos = self.pos.saturating_add(n);
    }

    fn slice(&self, (a, b): (usize, usize)) -> &[u8] {
        self.data.get(a..b).unwrap_or(&[])
    }

    fn start_tag(&mut self) -> Result<(), AepError> {
        let at = self.pos;
        self.pos = self.pos.saturating_add(1);
        let name = self.name()?;
        let mut attrs = Vec::new();
        let self_closing = loop {
            self.skip_ws();
            let rest = self.rest();
            if rest.starts_with(b"/>") {
                self.pos = self.pos.saturating_add(2);
                break true;
            }
            if rest.starts_with(b">") {
                self.pos = self.pos.saturating_add(1);
                break false;
            }
            if rest.is_empty() {
                return Err(xml(at, "a start tag is not closed"));
            }
            let key = self.name()?;
            self.skip_ws();
            if !self.rest().starts_with(b"=") {
                return Err(xml(self.pos, "an attribute has no value"));
            }
            self.pos = self.pos.saturating_add(1);
            self.skip_ws();
            let quote = *self.rest().first().ok_or(xml(self.pos, "a start tag is not closed"))?;
            if quote != b'"' && quote != b'\'' {
                return Err(xml(self.pos, "an attribute value is not quoted"));
            }
            let vstart = self.pos.saturating_add(1);
            let vend = self.find(vstart, &[quote]).ok_or(xml(vstart, "an attribute value is not closed"))?;
            let raw = self.data.get(vstart..vend).unwrap_or(&[]);
            if raw.contains(&b'<') {
                return Err(xml(vstart, "`<` inside an attribute value"));
            }
            // A value without `&` is its own decoding (most of them, every `bdata` After Effects
            // writes): borrow it instead of copying it.
            let value = if raw.contains(&b'&') {
                let mut value = Vec::new();
                decode_entities(raw, vstart, &mut value)?;
                Cow::Owned(value)
            } else {
                Cow::Borrowed(raw)
            };
            self.pos = vend.saturating_add(1);
            attrs.push((key, value));
        };
        let kind = self.kind_of(name, at)?;
        if kind != Kind::Root {
            self.count = self.count.saturating_add(1);
            if self.count > self.limits.max_chunks {
                return Err(AepError::TooManyChunks);
            }
            // The root's children are depth 1, like the top-level chunks of an `.aep`.
            if self.stack.len() > self.limits.max_depth {
                return Err(AepError::TooDeep);
            }
        }
        let mut frame = Frame { kind, at, name, bdata: None, attrs: Vec::new(), text: Vec::new(), children: Vec::new() };
        if matches!(kind, Kind::Chunk(_)) {
            if let Some((key, value)) = attrs.iter().find(|(k, _)| self.slice(*k) == b"bdata") {
                frame.bdata = Some(decode_hex(value, key.1)?);
            }
        } else if kind == Kind::Alias {
            frame.attrs = attrs.iter().map(|(k, v)| (String::from_utf8_lossy(self.slice(*k)).into_owned(), String::from_utf8_lossy(v).into_owned())).collect();
        }
        self.stack.push(frame);
        if self_closing {
            self.close()?;
        }
        Ok(())
    }

    /// Classifies an element by its name and where it is.
    fn kind_of(&self, name: (usize, usize), at: usize) -> Result<Kind, AepError> {
        let n = self.slice(name);
        let parent = match self.stack.last() {
            None => {
                if self.top.is_some() {
                    return Err(xml(at, "a second root element"));
                }
                if n != ROOT {
                    return Err(xml(at, "the root element is not AfterEffectsProject"));
                }
                return Ok(Kind::Root);
            }
            Some(p) => p.kind,
        };
        Ok(match parent {
            Kind::Skip | Kind::Utf8 | Kind::Xmp | Kind::Alias => Kind::Skip,
            Kind::Root | Kind::Chunk(_) => {
                if n == STRING {
                    Kind::Utf8
                } else if n == FILE_REFERENCE {
                    Kind::Alias
                } else if n == XMP && parent == Kind::Root {
                    Kind::Xmp
                } else if let Some(id) = chunk_id(n) {
                    Kind::Chunk(id)
                } else {
                    Kind::Skip
                }
            }
        })
    }

    fn end_tag(&mut self) -> Result<(), AepError> {
        let at = self.pos;
        self.pos = self.pos.saturating_add(2);
        let name = self.name()?;
        self.skip_ws();
        if !self.rest().starts_with(b">") {
            return Err(xml(at, "an end tag is not closed"));
        }
        self.pos = self.pos.saturating_add(1);
        let open = self.stack.last().ok_or(xml(at, "an end tag without a start tag"))?;
        if self.slice(open.name) != self.slice(name) {
            return Err(xml(at, "an end tag does not match its start tag"));
        }
        self.close()
    }

    /// Closes the innermost element and hands its chunk to the parent.
    fn close(&mut self) -> Result<(), AepError> {
        let frame = self.stack.pop().ok_or(xml(self.pos, "an end tag without a start tag"))?;
        let chunk = match frame.kind {
            Kind::Root => {
                self.top = Some(frame.children);
                return Ok(());
            }
            Kind::Skip => return Ok(()),
            Kind::Xmp => {
                if self.trailer.is_none() {
                    self.trailer = Some(frame.text);
                }
                return Ok(());
            }
            Kind::Utf8 => Chunk::data(*b"Utf8", frame.text),
            Kind::Alias => Chunk::data(*b"alas", alias_json(&frame.attrs).into_bytes()),
            Kind::Chunk(id) => chunk_of(id, frame)?,
        };
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(chunk);
        }
        Ok(())
    }

    fn text(&mut self) -> Result<(), AepError> {
        let start = self.pos;
        let len = self.rest().iter().position(|b| *b == b'<').unwrap_or(self.rest().len());
        let end = start.saturating_add(len);
        self.pos = end;
        let raw = self.data.get(start..end).unwrap_or(&[]);
        match self.stack.last_mut() {
            Some(frame) if matches!(frame.kind, Kind::Utf8 | Kind::Xmp) => decode_entities(raw, start, &mut frame.text),
            Some(_) => Ok(()),
            None if raw.iter().all(u8::is_ascii_whitespace) => Ok(()),
            None => Err(xml(start, "text outside the root element")),
        }
    }
}

/// The chunk id of an element name: 1 to 4 printable ASCII bytes, padded with spaces (`Pin` is
/// the list type `Pin `).
fn chunk_id(name: &[u8]) -> Option<[u8; 4]> {
    if name.is_empty() || name.len() > 4 || !name.iter().all(|b| b.is_ascii_graphic()) {
        return None;
    }
    let mut id = [b' '; 4];
    for (d, s) in id.iter_mut().zip(name) {
        *d = *s;
    }
    Some(id)
}

/// The chunk of a closed element named by a chunk id.
fn chunk_of(id: [u8; 4], frame: Frame) -> Result<Chunk<'static>, AepError> {
    if let Some(bytes) = frame.bdata {
        if OPAQUE_LIST_TYPES.contains(&id) {
            let pad = if bytes.len() % 2 == 1 { Some(0) } else { None };
            return Ok(Chunk { id: LIST, body: Body::OpaqueList { list_type: id, data: Cow::Owned(bytes) }, pad });
        }
        return Ok(Chunk::data(id, bytes));
    }
    // Numbers written as text: `<numS><string>0</string></numS>` and `<ppSn><string>150.000000000</string></ppSn>`.
    if &id == b"numS" || &id == b"ppSn" {
        let text = match frame.children.as_slice() {
            [only] if only.is(b"Utf8") => only.bytes().unwrap_or(&[]),
            _ => return Err(xml(frame.at, "a numS or ppSn element without its number")),
        };
        let text = std::str::from_utf8(text).map_err(|_| xml(frame.at, "a number is not text"))?.trim();
        let body = if &id == b"numS" {
            // Only `0` is sampled, so whether the field is signed is open: accept both readings of 4 bytes.
            let n = text.parse::<i64>().ok().filter(|n| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(n));
            let n = n.ok_or(xml(frame.at, "numS is not a 32-bit integer"))?;
            (n as u32).to_be_bytes().to_vec()
        } else {
            text.parse::<f64>().map_err(|_| xml(frame.at, "ppSn is not a number"))?.to_be_bytes().to_vec()
        };
        return Ok(Chunk::data(id, body));
    }
    if NESTED_IDS.contains(&id) {
        if frame.children.is_empty() {
            return Ok(Chunk::data(id, Vec::new()));
        }
        return Ok(Chunk { id, body: Body::Chunks(frame.children), pad: None });
    }
    Ok(Chunk::list(id, frame.children))
}

/// Appends `raw` with the predefined entities and character references decoded.
fn decode_entities(raw: &[u8], offset: usize, out: &mut Vec<u8>) -> Result<(), AepError> {
    let mut i = 0;
    while let Some(rest) = raw.get(i..) {
        let Some(amp) = rest.iter().position(|b| *b == b'&') else {
            out.extend_from_slice(rest);
            break;
        };
        out.extend_from_slice(rest.get(..amp).unwrap_or(&[]));
        let at = offset.saturating_add(i).saturating_add(amp);
        let after = rest.get(amp.saturating_add(1)..).unwrap_or(&[]);
        let semi = after.iter().take(12).position(|b| *b == b';').ok_or(xml(at, "an entity is not terminated"))?;
        let name = after.get(..semi).unwrap_or(&[]);
        let ch = match name {
            b"amp" => '&',
            b"lt" => '<',
            b"gt" => '>',
            b"quot" => '"',
            b"apos" => '\'',
            _ => {
                let code = if let Some(hex) = name.strip_prefix(b"#x").or_else(|| name.strip_prefix(b"#X")) {
                    std::str::from_utf8(hex).ok().and_then(|h| u32::from_str_radix(h, 16).ok())
                } else if let Some(dec) = name.strip_prefix(b"#") {
                    std::str::from_utf8(dec).ok().and_then(|d| d.parse::<u32>().ok())
                } else {
                    return Err(xml(at, "an unknown entity"));
                };
                code.filter(|c| *c != 0).and_then(char::from_u32).ok_or(xml(at, "an invalid character reference"))?
            }
        };
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        i = i.saturating_add(amp).saturating_add(semi).saturating_add(2);
    }
    Ok(())
}

/// Decodes a `bdata` value (hex digits, two per byte).
fn decode_hex(text: &[u8], offset: usize) -> Result<Vec<u8>, AepError> {
    decode_hex_at_most(text, offset, MAX_BDATA)
}

/// The most bytes one `bdata` may decode to: what the 32-bit size field of a chunk can hold (a
/// larger body could not be written back as an `.aep` either).
const MAX_BDATA: usize = u32::MAX as usize;

/// [`decode_hex`] with the decoded-size limit as a parameter (so tests can reach it).
fn decode_hex_at_most(text: &[u8], offset: usize, max: usize) -> Result<Vec<u8>, AepError> {
    if text.len() % 2 == 1 {
        return Err(xml(offset, "bdata has an odd number of hex digits"));
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let (pairs, _) = text.as_chunks::<2>();
    if pairs.len() > max {
        return Err(AepError::TooLarge);
    }
    // Nothing is reserved for a value that does not decode: one cheap pass checks every digit first.
    // Blocks without an early exit inside, so the check vectorises; it stops at the first bad block.
    if !text.chunks(4096).all(|block| block.iter().fold(true, |ok, b| ok & b.is_ascii_hexdigit())) {
        return Err(xml(offset, "bdata holds a character that is not a hex digit"));
    }
    // Sized once (collecting into `Option<Vec>` has no size hint and regrows step by step), and
    // without aborting when the memory is not there.
    let mut out = Vec::new();
    out.try_reserve_exact(pairs.len()).map_err(|_| xml(offset, "bdata is too large to hold in memory"))?;
    for [hi, lo] in pairs {
        match (digit(*hi), digit(*lo)) {
            (Some(h), Some(l)) => out.push((h << 4) | l),
            _ => return Err(xml(offset, "bdata holds a character that is not a hex digit")),
        }
    }
    Ok(out)
}

/// The JSON body of an `alas` chunk from the attributes of a `fileReference`, keys in document
/// order (alphabetical in every sample, as in the `.aep`): the two `ascendcount_*` values as
/// numbers, `platform` `Win` as `1`, `target_is_folder` `0` / `1` as `false` / `true`, everything
/// else as a string.
fn alias_json(attrs: &[(String, String)]) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in attrs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        json_str(&mut out, key);
        out.push(':');
        let number = value.parse::<i64>().ok();
        match (key.as_str(), value.as_str()) {
            ("ascendcount_base" | "ascendcount_target", _) if number.is_some() => out.push_str(value),
            ("platform", "Win") => out.push('1'),
            ("platform", _) if number.is_some() => out.push_str(value),
            ("target_is_folder", "0" | "false") => out.push_str("false"),
            ("target_is_folder", "1" | "true") => out.push_str("true"),
            _ => json_str(&mut out, value),
        }
    }
    out.push('}');
    out
}

fn json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> String {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AfterEffectsProject xmlns=\"x\">{body}</AfterEffectsProject>\n")
    }

    #[test]
    fn maps_elements_to_chunks() {
        let x = doc(concat!(
            "<head bdata=\"00ff\"/><Pin><sspc bdata=\"01\"/></Pin><Pefl>\n</Pefl>",
            "<tdsn><string>A &amp; B&#x41;</string></tdsn><numS><string>-3</string></numS>",
            "<ppSn><string>150.000000000</string></ppSn><btdk bdata=\"414243\"/><Comment>x<a bdata=\"00\"/></Comment>",
            "<ProjectXMPMetadata><![CDATA[<?xpacket begin?>]]></ProjectXMPMetadata>"
        ));
        let f = read_aepx(x.as_bytes()).unwrap();
        assert_eq!(f.trailer.as_ref(), b"<?xpacket begin?>");
        let ids: Vec<_> = f.chunks.iter().map(|c| (c.id, c.list_type())).collect();
        assert_eq!(
            ids,
            vec![
                (*b"head", None),
                (LIST, Some(*b"Pin ")),
                (LIST, Some(*b"Pefl")),
                (*b"tdsn", None),
                (*b"numS", None),
                (*b"ppSn", None),
                (LIST, Some(*b"btdk"))
            ]
        );
        assert_eq!(f.chunks[1].children()[0].bytes(), Some(&[1u8][..]));
        assert_eq!(f.chunks[1].children()[0].pad, Some(0));
        assert_eq!(f.chunks[3].children()[0].bytes(), Some(&b"A & BA"[..]));
        assert_eq!(f.chunks[4].bytes(), Some(&(-3i32).to_be_bytes()[..]));
        assert_eq!(f.chunks[5].bytes(), Some(&150f64.to_be_bytes()[..]));
        assert_eq!(f.chunks[6].bytes(), Some(&b"ABC"[..]));
        // The tree writes as a valid `.aep` that parses back to itself.
        let bin = crate::container::write(&f).unwrap();
        assert_eq!(parse(&bin).unwrap(), f);
    }

    #[test]
    fn file_reference_becomes_alias_json() {
        let x = doc(concat!(
            "<Als2><fileReference ascendcount_base=\"1\" ascendcount_target=\"1\" fullpath=\"C:\\a\\&quot;b.png\" ",
            "platform=\"Win\" server_name=\"N\" server_volume_name=\"V\" target_is_folder=\"0\"/></Als2>"
        ));
        let f = read_aepx(x.as_bytes()).unwrap();
        let alas = f.chunks[0].children()[0].bytes().unwrap();
        assert_eq!(
            std::str::from_utf8(alas).unwrap(),
            r#"{"ascendcount_base":1,"ascendcount_target":1,"fullpath":"C:\\a\\\"b.png","platform":1,"server_name":"N","server_volume_name":"V","target_is_folder":false}"#
        );
    }

    #[test]
    fn nums_takes_signed_and_unsigned_32_bit_values() {
        for (text, bytes) in [("-2147483648", [0x80, 0, 0, 0]), ("4294967295", [0xff; 4]), (" 7 ", [0, 0, 0, 7])] {
            let f = read_aepx(doc(&format!("<numS><string>{text}</string></numS>")).as_bytes()).unwrap();
            assert_eq!(f.chunks[0].bytes(), Some(&bytes[..]), "{text}");
        }
        for text in ["-2147483649", "4294967296", "1.5"] {
            assert!(read_aepx(doc(&format!("<numS><string>{text}</string></numS>")).as_bytes()).is_err(), "{text}");
        }
    }

    #[test]
    fn detection() {
        assert!(is_aepx(b"\xef\xbb\xbf  <?xml version=\"1.0\"?>"));
        assert!(is_aepx(b"<AfterEffectsProject>"));
        assert!(!is_aepx(b"RIFX\0\0\0\x04Egg!"));
        assert!(parse_any(b"RIFX\0\0\0\x04Egg!").is_ok());
        assert!(parse_any(doc("").as_bytes()).is_ok());
    }

    #[test]
    fn bad_input_is_an_error() {
        let bad = [
            doc("<a bdata=\"0\"/>"),
            doc("<a bdata=\"zz\"/>"),
            doc("<string>&nbsp;</string>"),
            doc("<string>&#0;</string>"),
            doc("<string>&amp</string>"),
            doc("<a>"),
            doc("<a></b>"),
            doc("<numS><string>x</string></numS>"),
            doc("<numS></numS>"),
            doc("<a b></a>"),
            doc("<a b=c></a>"),
            "<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"b\">]><AfterEffectsProject/>".into(),
            "<?xml version=\"1.0\"?><Other/>".into(),
            "<?xml version=\"1.0\"?>".into(),
            "<AfterEffectsProject/><AfterEffectsProject/>".into(),
            "<AfterEffectsProject/>junk".into(),
            "<AfterEffectsProject><!-- x".into(),
        ];
        for b in &bad {
            assert!(matches!(read_aepx(b.as_bytes()), Err(AepError::Xml { .. })), "{b}");
        }
        assert!(matches!(read_aepx(b"<AfterEffectsProject>\xff</AfterEffectsProject>"), Err(AepError::Xml { .. })));
    }

    /// Attribute values decode entities only where they have them.
    #[test]
    fn attribute_values_decode_entities_only_where_they_have_them() {
        // An entity inside `bdata` still decodes before the hex does.
        let f = read_aepx(doc("<a bdata=\"&#x30;1&#65;b\"/>").as_bytes()).unwrap();
        assert_eq!(f.chunks[0].bytes(), Some(&[0x01, 0xab][..]));
        // A bad entity in any attribute, even of an element that is skipped, is still an error.
        for bad in ["<a other=\"&nbsp;\"/>", "<Unknown x=\"&bogus;\"/>", "<a bdata=\"00\" note=\"&amp\"/>"] {
            assert!(matches!(read_aepx(doc(bad).as_bytes()), Err(AepError::Xml { .. })), "{bad}");
        }
        // Without `&` the value is taken as written.
        let f = read_aepx(doc("<a bdata=\"0aFf\" x=\"y\"/>").as_bytes()).unwrap();
        assert_eq!(f.chunks[0].bytes(), Some(&[0x0a, 0xff][..]));
    }

    /// A large bdata with a bad digit is refused before it is sized.
    #[test]
    fn a_large_bdata_with_a_bad_digit_is_refused_before_it_is_sized() {
        // 10 MB of hex that is bad from the first pair (and once more at the very end).
        let mut text = b"zz".to_vec();
        text.extend(std::iter::repeat_n(*b"00", 5_000_000).flatten());
        assert_eq!(decode_hex(&text, 3), Err(xml(3, "bdata holds a character that is not a hex digit")));
        let mut late = vec![b'0'; 10_000_000];
        late.extend_from_slice(b"0g");
        assert_eq!(decode_hex(&late, 3), Err(xml(3, "bdata holds a character that is not a hex digit")));
        let x = doc(&format!("<a bdata=\"{}\"/>", std::str::from_utf8(&text).unwrap()));
        assert!(matches!(read_aepx(x.as_bytes()), Err(AepError::Xml { .. })));
    }

    /// Bdata decodes up to the size limit and not past it.
    #[test]
    fn bdata_decodes_up_to_the_size_limit_and_not_past_it() {
        assert_eq!(decode_hex_at_most(b"00ff10", 0, 3), Ok(vec![0x00, 0xff, 0x10]));
        assert_eq!(decode_hex_at_most(b"00ff10", 0, 2), Err(AepError::TooLarge));
        assert_eq!(decode_hex_at_most(b"", 0, 0), Ok(Vec::new()));
        assert_eq!(decode_hex_at_most(b"00", 0, 0), Err(AepError::TooLarge));
        // The odd-length error still comes first.
        assert_eq!(decode_hex_at_most(b"001", 0, 0), Err(xml(0, "bdata has an odd number of hex digits")));
        // The real limit is what a chunk's 32-bit size field holds.
        assert_eq!(MAX_BDATA as u64, u64::from(u32::MAX));
    }

    /// Decode hex matches the collecting version.
    #[test]
    fn decode_hex_matches_the_collecting_version() {
        /// The collecting decoder this one replaced, as the reference.
        fn old(text: &[u8], offset: usize) -> Result<Vec<u8>, AepError> {
            if text.len() % 2 == 1 {
                return Err(xml(offset, "bdata has an odd number of hex digits"));
            }
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            let (pairs, _) = text.as_chunks::<2>();
            pairs
                .iter()
                .map(|[hi, lo]| digit(*hi).zip(digit(*lo)).map(|(h, l)| (h << 4) | l))
                .collect::<Option<Vec<u8>>>()
                .ok_or(xml(offset, "bdata holds a character that is not a hex digit"))
        }
        let alphabet = b"0123456789abcdefABCDEFgG \0";
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for len in 0..200usize {
            for _ in 0..20 {
                let text: Vec<u8> = (0..len)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        // Mostly hex digits, now and then a byte that is not one.
                        let i = (seed % 1000) as usize;
                        alphabet[if i < 990 { i % 22 } else { 22 + i % 4 }]
                    })
                    .collect();
                let new = decode_hex(&text, 7);
                assert_eq!(new, old(&text, 7));
                if let Ok(v) = new {
                    assert_eq!(v.capacity(), v.len());
                }
            }
        }
    }

    #[test]
    fn limits_hold() {
        let deep = format!("<AfterEffectsProject>{}{}</AfterEffectsProject>", "<a>".repeat(100_000), "</a>".repeat(100_000));
        assert_eq!(read_aepx(deep.as_bytes()), Err(AepError::TooDeep));
        let ok = format!("<AfterEffectsProject>{}{}</AfterEffectsProject>", "<a>".repeat(64), "</a>".repeat(64));
        assert!(read_aepx(ok.as_bytes()).is_ok());
        let many = format!("<AfterEffectsProject>{}</AfterEffectsProject>", "<a/>".repeat(10));
        assert_eq!(read_aepx_with_limits(many.as_bytes(), Limits { max_depth: 64, max_chunks: 9 }), Err(AepError::TooManyChunks));
    }
}
