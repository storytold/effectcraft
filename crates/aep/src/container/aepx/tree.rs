//! The `.aepx` reader of the owned chunk tree ([`super::read_aepx`]): it builds the tree element by
//! element. The importer uses the index reader in `aepx.rs` instead; the tests compare the two on
//! generated, damaged and corpus input (the same tree and the same errors).

use std::borrow::Cow;

use super::{FILE_REFERENCE, OPAQUE_LIST_TYPES, ROOT, STRING, XMP, alias_json, chunk_id, decode_entities, decode_hex, xml};
use crate::container::chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS};
use crate::container::error::AepError;
use crate::container::read::Limits;

/// [`super::read_aepx_with_limits`].
pub(super) fn read(data: &[u8], limits: Limits) -> Result<AepFile<'static>, AepError> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t.as_bytes(),
        Err(e) => return Err(xml(e.valid_up_to(), "the file is not valid UTF-8")),
    };
    Reader { data: text, pos: 0, limits, count: 0, stack: Vec::new(), top: None, trailer: None }.run()
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
