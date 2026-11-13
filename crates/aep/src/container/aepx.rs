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
//!
//! There are two readers. [`read_aepx`] builds the owned tree element by element
//! (`aepx/tree.rs`). The importer, which only reads, uses `index_aepx`: it decodes every body
//! (hex, text, the alias JSON, the numbers) into one buffer and records the chunks as entries of
//! the flat index ([`Draft`]s, in depth-first order, bodies as ranges of the buffer), which
//! [`crate::container::TreeIndex::from_drafts`] turns into an index that borrows the buffer.
//! The tests hold both to the same tree and the same errors.

use std::borrow::Cow;

use crate::container::chunk::{AepFile, LIST, NESTED_IDS};
use crate::container::error::AepError;
use crate::container::index::{Draft, Kind as EntryKind};
use crate::container::read::{Limits, parse};

mod tree;

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
    tree::read(data, limits)
}

/// An `.aepx` read for the flat index: every body decoded into one buffer, and the chunks as
/// [`Draft`]s in depth-first order with their bodies as ranges of that buffer.
/// [`crate::container::TreeIndex::from_drafts`] makes the index, which borrows the buffer.
pub(crate) struct AepxDrafts {
    /// The decoded bodies (hex, text, alias JSON, numbers) and the XMP packet.
    pub(crate) buffer: Vec<u8>,
    pub(crate) nodes: Vec<Draft>,
    /// The XMP packet: a range of `buffer` (empty without one).
    pub(crate) trailer: (usize, usize),
}

/// Reads an `.aepx` into [`AepxDrafts`]: the tree [`read_aepx_with_limits`] gives, with the same
/// errors, without one allocation per chunk.
///
/// # Errors
/// See [`read_aepx_with_limits`]. When memory runs out, two errors come up in more cases than
/// there: [`AepError::TooLarge`] when the chunk list cannot grow (the owned-tree reader would
/// abort), and [`AepError::Xml`] "bdata is too large to hold in memory" also when the shared body
/// buffer cannot grow (the owned-tree reader reserves each value on its own).
pub(crate) fn index_aepx(data: &[u8], limits: Limits) -> Result<AepxDrafts, AepError> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t.as_bytes(),
        Err(e) => return Err(xml(e.valid_up_to(), "the file is not valid UTF-8")),
    };
    // Every counted element takes at least 3 bytes (`<a>` in a document cut short), so there are
    // never more chunks than this.
    let bound = (text.len() / 3).min(limits.max_chunks);
    let mut nodes = Vec::new();
    let _ = nodes.try_reserve_exact(bound.min(INITIAL_SLOTS));
    // Hex is most of an `.aepx` and decodes to half its length: one buffer of that size usually
    // holds everything. A failed reservation is not an error here; the buffer grows as it goes.
    let mut buffer = Vec::new();
    let _ = buffer.try_reserve_exact(text.len() / 2);
    let reader = Reader { data: text, pos: 0, limits, count: 0, stack: Vec::new(), root_closed: false, trailer: None, buffer, nodes, bound, attrs: Vec::new() };
    reader.run()
}

/// The slots the chunk list starts with; it doubles from there (as the array of
/// [`crate::container::TreeIndex`] does).
const INITIAL_SLOTS: usize = 1024;

fn xml(offset: usize, what: &'static str) -> AepError {
    AepError::Xml { offset, what }
}

/// The pad byte of a body of `len` bytes, as [`crate::container::Chunk::data`] sets it.
fn pad_of(len: usize) -> Option<u8> {
    if len % 2 == 1 { Some(0) } else { None }
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
    /// The chunk's entry in the list (for `Chunk`, `Utf8` and `Alias`).
    entry: Option<usize>,
    /// The buffer length when the element opened: its body starts here.
    mark: usize,
    /// The end of the decoded `bdata` attribute in the buffer.
    bdata_end: Option<usize>,
    /// The attributes of a `fileReference`, in document order.
    attrs: Vec<(String, String)>,
}

struct Reader<'d> {
    data: &'d [u8],
    pos: usize,
    limits: Limits,
    count: usize,
    stack: Vec<Frame>,
    /// True once the root element has closed.
    root_closed: bool,
    trailer: Option<(usize, usize)>,
    buffer: Vec<u8>,
    nodes: Vec<Draft>,
    /// The most chunks the input can hold under the limits: `nodes` never reserves past it.
    bound: usize,
    /// The attributes of the current start tag; kept to reuse its allocation.
    attrs: Vec<((usize, usize), Cow<'d, [u8]>)>,
}

impl<'d> Reader<'d> {
    fn run(mut self) -> Result<AepxDrafts, AepError> {
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
        if !self.root_closed {
            return Err(xml(self.data.len(), "no AfterEffectsProject root element"));
        }
        Ok(AepxDrafts { buffer: self.buffer, nodes: self.nodes, trailer: self.trailer.unwrap_or((0, 0)) })
    }

    fn rest(&self) -> &'d [u8] {
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
            match self.stack.last() {
                Some(frame) => {
                    if matches!(frame.kind, Kind::Utf8 | Kind::Xmp) {
                        self.buffer.extend_from_slice(content);
                    }
                }
                None => return Err(xml(start, "CDATA outside the root element")),
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

    fn slice(&self, (a, b): (usize, usize)) -> &'d [u8] {
        self.data.get(a..b).unwrap_or(&[])
    }

    /// Reads the attributes of a start tag into `attrs`; true when the tag closes itself.
    fn attributes(&mut self, at: usize, attrs: &mut Vec<((usize, usize), Cow<'d, [u8]>)>) -> Result<bool, AepError> {
        loop {
            self.skip_ws();
            let rest = self.rest();
            if rest.starts_with(b"/>") {
                self.pos = self.pos.saturating_add(2);
                return Ok(true);
            }
            if rest.starts_with(b">") {
                self.pos = self.pos.saturating_add(1);
                return Ok(false);
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
        }
    }

    fn start_tag(&mut self) -> Result<(), AepError> {
        let at = self.pos;
        self.pos = self.pos.saturating_add(1);
        let name = self.name()?;
        let mut attrs = std::mem::take(&mut self.attrs);
        attrs.clear();
        let self_closing = self.attributes(at, &mut attrs)?;
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
        let mark = self.buffer.len();
        let mut frame = Frame { kind, at, name, entry: None, mark, bdata_end: None, attrs: Vec::new() };
        match kind {
            Kind::Chunk(id) => {
                if let Some((key, value)) = attrs.iter().find(|(k, _)| self.slice(*k) == b"bdata") {
                    decode_hex_into(value, key.1, MAX_BDATA, &mut self.buffer)?;
                    frame.bdata_end = Some(self.buffer.len());
                }
                frame.entry = Some(self.open_entry(id)?);
            }
            Kind::Utf8 => frame.entry = Some(self.open_entry(*b"Utf8")?),
            Kind::Alias => {
                frame.attrs =
                    attrs.iter().map(|(k, v)| (String::from_utf8_lossy(self.slice(*k)).into_owned(), String::from_utf8_lossy(v).into_owned())).collect();
                frame.entry = Some(self.open_entry(*b"alas")?);
            }
            Kind::Root | Kind::Xmp | Kind::Skip => {}
        }
        self.attrs = attrs;
        self.stack.push(frame);
        if self_closing {
            self.close()?;
        }
        Ok(())
    }

    /// Appends the entry of a chunk whose element just opened; its body is filled in when it
    /// closes. The list grows by its length (at least 16) but never past [`Self::bound`]; an
    /// allocation the target cannot make is an error, not an abort.
    fn open_entry(&mut self, id: [u8; 4]) -> Result<usize, AepError> {
        let at = self.nodes.len();
        // This entry and the one after it must have `u32` indices.
        u32::try_from(at.checked_add(1).ok_or(AepError::TooManyChunks)?).map_err(|_| AepError::TooManyChunks)?;
        if at == self.nodes.capacity() {
            // Past the bound (which only a wrong guess could reach) it keeps doubling.
            let room = self.bound.saturating_sub(at);
            let more = if room == 0 { at.max(16) } else { at.max(16).min(room) };
            self.nodes.try_reserve_exact(more).map_err(|_| AepError::TooLarge)?;
        }
        self.nodes.push(Draft::placeholder(id));
        Ok(at)
    }

    /// Fills in the entry at `at` once its element has closed; its subtree is what follows it.
    fn finish(&mut self, at: usize, id: [u8; 4], kind: EntryKind, list_type: [u8; 4], body: (usize, usize), pad: Option<u8>) -> Result<(), AepError> {
        let end = u32::try_from(self.nodes.len()).map_err(|_| AepError::TooManyChunks)?;
        if let Some(d) = self.nodes.get_mut(at) {
            *d = Draft { start: body.0, len: body.1.saturating_sub(body.0), id, list_type, end, kind, pad };
        }
        Ok(())
    }

    /// Classifies an element by its name and where it is.
    fn kind_of(&self, name: (usize, usize), at: usize) -> Result<Kind, AepError> {
        let n = self.slice(name);
        let parent = match self.stack.last() {
            None => {
                if self.root_closed {
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

    /// Closes the innermost element and fills in its chunk's entry.
    fn close(&mut self) -> Result<(), AepError> {
        let frame = self.stack.pop().ok_or(xml(self.pos, "an end tag without a start tag"))?;
        match frame.kind {
            Kind::Root => self.root_closed = true,
            Kind::Skip => {}
            Kind::Xmp => {
                // Only the first packet is kept; a later one is dropped from the buffer (its
                // children are skipped, so nothing else was written after it opened).
                if self.trailer.is_none() {
                    self.trailer = Some((frame.mark, self.buffer.len()));
                } else {
                    self.buffer.truncate(frame.mark);
                }
            }
            Kind::Utf8 => {
                let at = frame.entry.ok_or(xml(frame.at, "an element lost its chunk"))?;
                let end = self.buffer.len();
                self.finish(at, *b"Utf8", EntryKind::Data, [0; 4], (frame.mark, end), pad_of(end.saturating_sub(frame.mark)))?;
            }
            Kind::Alias => {
                let at = frame.entry.ok_or(xml(frame.at, "an element lost its chunk"))?;
                self.buffer.extend_from_slice(alias_json(&frame.attrs).as_bytes());
                let end = self.buffer.len();
                self.finish(at, *b"alas", EntryKind::Data, [0; 4], (frame.mark, end), pad_of(end.saturating_sub(frame.mark)))?;
            }
            Kind::Chunk(id) => self.close_chunk(id, &frame)?,
        }
        Ok(())
    }

    /// The chunk of a closed element named by a chunk id (what the owned reader's `chunk_of`
    /// built): `bdata` wins over children, `numS` / `ppSn` hold their number, a nested id holds its
    /// children (or nothing), any other name is a `LIST` of that type.
    fn close_chunk(&mut self, id: [u8; 4], frame: &Frame) -> Result<(), AepError> {
        let at = frame.entry.ok_or(xml(frame.at, "an element lost its chunk"))?;
        let after = at.checked_add(1).ok_or(AepError::TooManyChunks)?;
        if let Some(bdata_end) = frame.bdata_end {
            // The children were read (and counted, and checked) but are not part of the chunk.
            self.nodes.truncate(after);
            self.buffer.truncate(bdata_end);
            let body = (frame.mark, bdata_end);
            let pad = pad_of(bdata_end.saturating_sub(frame.mark));
            if OPAQUE_LIST_TYPES.contains(&id) {
                return self.finish(at, LIST, EntryKind::OpaqueList, id, body, pad);
            }
            return self.finish(at, id, EntryKind::Data, [0; 4], body, pad);
        }
        // Numbers written as text: `<numS><string>0</string></numS>` and `<ppSn><string>150.000000000</string></ppSn>`.
        if &id == b"numS" || &id == b"ppSn" {
            let missing = || xml(frame.at, "a numS or ppSn element without its number");
            // Exactly one child chunk, a `Utf8`: the entry after this one, whose subtree runs to the end.
            let only = self.nodes.get(after).copied().filter(|c| usize::try_from(c.end).ok() == Some(self.nodes.len())).ok_or_else(missing)?;
            if only.id != *b"Utf8" {
                return Err(missing());
            }
            let text = match only.kind {
                EntryKind::Data | EntryKind::OpaqueList => only.start.checked_add(only.len).and_then(|end| self.buffer.get(only.start..end)).unwrap_or(&[]),
                EntryKind::List | EntryKind::Nested => &[],
            };
            let text = std::str::from_utf8(text).map_err(|_| xml(frame.at, "a number is not text"))?.trim();
            let body: [u8; 8];
            let bytes: &[u8] = if &id == b"numS" {
                // Only `0` is sampled, so whether the field is signed is open: accept both readings of 4 bytes.
                let n = text.parse::<i64>().ok().filter(|n| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(n));
                let n = n.ok_or(xml(frame.at, "numS is not a 32-bit integer"))?;
                body = u64::from(n as u32).to_be_bytes();
                body.get(4..).unwrap_or(&[])
            } else {
                body = text.parse::<f64>().map_err(|_| xml(frame.at, "ppSn is not a number"))?.to_be_bytes();
                &body
            };
            self.nodes.truncate(after);
            self.buffer.truncate(frame.mark);
            self.buffer.extend_from_slice(bytes);
            let end = self.buffer.len();
            return self.finish(at, id, EntryKind::Data, [0; 4], (frame.mark, end), pad_of(bytes.len()));
        }
        let empty = (frame.mark, frame.mark);
        if NESTED_IDS.contains(&id) {
            if self.nodes.len() == after {
                return self.finish(at, id, EntryKind::Data, [0; 4], empty, None);
            }
            return self.finish(at, id, EntryKind::Nested, [0; 4], empty, None);
        }
        self.finish(at, LIST, EntryKind::List, id, empty, None)
    }

    fn text(&mut self) -> Result<(), AepError> {
        let start = self.pos;
        let len = self.rest().iter().position(|b| *b == b'<').unwrap_or(self.rest().len());
        let end = start.saturating_add(len);
        self.pos = end;
        let raw = self.data.get(start..end).unwrap_or(&[]);
        match self.stack.last() {
            Some(frame) if matches!(frame.kind, Kind::Utf8 | Kind::Xmp) => decode_entities(raw, start, &mut self.buffer),
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

/// Decodes a `bdata` value (hex digits, two per byte) into a new `Vec` of exactly its size.
fn decode_hex(text: &[u8], offset: usize) -> Result<Vec<u8>, AepError> {
    decode_hex_at_most(text, offset, MAX_BDATA)
}

/// The most bytes one `bdata` may decode to: what the 32-bit size field of a chunk can hold (a
/// larger body could not be written back as an `.aep` either).
const MAX_BDATA: usize = u32::MAX as usize;

/// [`decode_hex`] with the decoded-size limit as a parameter (so tests can reach it).
fn decode_hex_at_most(text: &[u8], offset: usize, max: usize) -> Result<Vec<u8>, AepError> {
    // Checked before anything is reserved, then sized once (collecting into `Option<Vec>` has no
    // size hint and regrows step by step), and without aborting when the memory is not there.
    let pairs = check_hex(text, offset, max)?;
    let mut out = Vec::new();
    out.try_reserve_exact(pairs).map_err(|_| xml(offset, "bdata is too large to hold in memory"))?;
    decode_checked(text, offset, &mut out)?;
    Ok(out)
}

/// Appends the bytes of a `bdata` value (hex digits, two per byte, at most `max` bytes) to `out`.
/// On an error `out` is unchanged.
fn decode_hex_into(text: &[u8], offset: usize, max: usize, out: &mut Vec<u8>) -> Result<(), AepError> {
    let pairs = check_hex(text, offset, max)?;
    // Grown at most once per value (amortised over a shared buffer), and without aborting when the
    // memory is not there.
    out.try_reserve(pairs).map_err(|_| xml(offset, "bdata is too large to hold in memory"))?;
    decode_checked(text, offset, out)
}

/// Appends the bytes of a `bdata` value that [`check_hex`] accepted. On an error (a digit the
/// check let through, which it does not) `out` is unchanged.
fn decode_checked(text: &[u8], offset: usize, out: &mut Vec<u8>) -> Result<(), AepError> {
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mark = out.len();
    for [hi, lo] in text.as_chunks::<2>().0 {
        match (digit(*hi), digit(*lo)) {
            (Some(h), Some(l)) => out.push((h << 4) | l),
            _ => {
                out.truncate(mark);
                return Err(xml(offset, "bdata holds a character that is not a hex digit"));
            }
        }
    }
    Ok(())
}

/// Checks a `bdata` value before anything is reserved for it: an even number of hex digits, at
/// most `max` bytes. Returns the number of bytes it decodes to.
fn check_hex(text: &[u8], offset: usize, max: usize) -> Result<usize, AepError> {
    if text.len() % 2 == 1 {
        return Err(xml(offset, "bdata has an odd number of hex digits"));
    }
    let pairs = text.len() / 2;
    if pairs > max {
        return Err(AepError::TooLarge);
    }
    // One cheap pass checks every digit first. Blocks without an early exit inside, so the check
    // vectorises; it stops at the first bad block.
    if !text.chunks(4096).all(|block| block.iter().fold(true, |ok, b| ok & b.is_ascii_hexdigit())) {
        return Err(xml(offset, "bdata holds a character that is not a hex digit"));
    }
    Ok(pairs)
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
    use crate::container::index::TreeIndex;

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

    /// A tiny deterministic generator (xorshift64*), so the tests need no extra crate.
    struct Rng(u64);

    impl Rng {
        /// The next pseudo-random number.
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        /// A number below `n` (0 for `n == 0`).
        fn below(&mut self, n: usize) -> usize {
            if n == 0 { 0 } else { usize::try_from(self.next() % n as u64).unwrap() }
        }

        /// One of `items`.
        fn pick<'s>(&mut self, items: &[&'s str]) -> &'s str {
            items[self.below(items.len())]
        }
    }

    /// `n` random bytes as hex, upper or lower case.
    fn hex(rng: &mut Rng, n: usize) -> String {
        let upper = rng.below(4) == 0;
        (0..n).map(|_| if upper { format!("{:02X}", rng.below(256)) } else { format!("{:02x}", rng.below(256)) }).collect()
    }

    /// Random `.aepx` content: every element form the reader knows (data, lists, nested ids,
    /// strings, numbers, `btdk`, file references, skipped elements, XMP packets), `bdata` next to
    /// children, bodies that read as chunk sequences, entities, CDATA, comments and stray text.
    fn content(rng: &mut Rng, depth: usize, budget: &mut usize, out: &mut String) {
        for _ in 0..rng.below(6) {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            match rng.below(20) {
                0..=3 => {
                    let id = rng.pick(&["cdat", "tdmn", "head", "Utf8", "idta", "LIST", "a"]);
                    let n = rng.below(9);
                    out.push_str(&format!("<{id} bdata=\"{}\"/>", hex(rng, n)));
                }
                4..=6 if depth < 10 => {
                    let id = rng.pick(&["Fold", "tdgp", "Layr", "Pin", "LIST", "btdk", "sspc"]);
                    let attr = if rng.below(5) == 0 { format!(" bdata=\"{}\"", hex(rng, 3)) } else { String::new() };
                    out.push_str(&format!("<{id}{attr}>"));
                    content(rng, depth + 1, budget, out);
                    out.push_str(&format!("</{id}>"));
                }
                7 => {
                    let id = rng.pick(&["tdsn", "pdnm", "fnam"]);
                    match rng.below(4) {
                        0 => out.push_str(&format!("<{id}/>")),
                        1 => out.push_str(&format!("<{id}><string>name &amp; &#x41;</string></{id}>")),
                        // A body that is itself a chunk sequence (`Utf8`, 1 byte, pad): still data.
                        2 => out.push_str(&format!("<{id} bdata=\"557466380000000161{}\"/>", if rng.below(2) == 0 { "00" } else { "" })),
                        _ => out.push_str(&format!("<{id}><string>a</string><string>b</string></{id}>")),
                    }
                }
                8 => {
                    let text = rng.pick(&["plain", "A &lt; B", "<![CDATA[x<y]]>", "a<!-- c -->b", "", "&#233;t&#xE9;", "x<Skip>gone</Skip>y", "\r\n"]);
                    out.push_str(&format!("<string>{text}</string>"));
                }
                9 => {
                    let id = rng.pick(&["numS", "ppSn"]);
                    let inner = rng.pick(&[
                        "<string>0</string>",
                        "<string> 7 </string>",
                        "<string>-2147483648</string>",
                        "<string>150.000000000</string>",
                        "<string>x</string>",
                        "",
                        "<string>1</string><string>2</string>",
                        "<Utf8 bdata=\"31\"/>",
                        "<cdat bdata=\"31\"/>",
                        "<Comment>3</Comment><string>4</string>",
                    ]);
                    let attr = if rng.below(6) == 0 { " bdata=\"00000002\"" } else { "" };
                    out.push_str(&format!("<{id}{attr}>{inner}</{id}>"));
                }
                10 => {
                    let n = rng.pick(&["", "41", "414243", "4c495354000000000000"]);
                    out.push_str(&format!("<btdk bdata=\"{n}\"/>"));
                }
                11 => {
                    let platform = rng.pick(&["Win", "1", "Mac"]);
                    out.push_str(&format!(
                        "<Als2><fileReference ascendcount_base=\"1\" fullpath=\"C:\\a\\&quot;b&#x9;.png\" platform=\"{platform}\" target_is_folder=\"{}\"/></Als2>",
                        rng.below(3)
                    ));
                }
                12 if depth < 10 => {
                    out.push_str("<Comment note=\"&amp;\">text<a bdata=\"00\"/>");
                    content(rng, depth + 1, budget, out);
                    out.push_str("</Comment>");
                }
                13 => {
                    let packet = rng.pick(&["<![CDATA[<?xpacket begin?>x<?xpacket end?>]]>", "plain &amp; text", ""]);
                    out.push_str(&format!("<ProjectXMPMetadata>{packet}</ProjectXMPMetadata>"));
                }
                14 => out.push_str(rng.pick(&["<!-- comment -->", "<?pi x?>", "\n  ", "stray"])),
                _ => {
                    let n = rng.below(5);
                    out.push_str(&format!("<tdb4 bdata=\"{}\" x='y'></tdb4>", hex(rng, n)));
                }
            }
        }
    }

    /// The default limits and six tight ones (depth 0, 1 and 3; 0, 7 and 40 chunks).
    fn limit_sets() -> [Limits; 7] {
        [
            Limits { max_depth: 0, max_chunks: 10_000_000 },
            Limits { max_depth: 64, max_chunks: 0 },
            Limits::default(),
            Limits { max_depth: 1, max_chunks: 10_000_000 },
            Limits { max_depth: 3, max_chunks: 10_000_000 },
            Limits { max_depth: 64, max_chunks: 7 },
            Limits { max_depth: 64, max_chunks: 40 },
        ]
    }

    /// The index reader and the owned-tree reader give the same `Result` (the tree, or the error),
    /// and the index walks like the index of the owned tree.
    fn same(bytes: &[u8], limits: Limits) {
        let old = read_aepx_with_limits(bytes, limits);
        let new = index_aepx(bytes, limits).map(|d| TreeIndex::from_drafts(&d.buffer, d.nodes, d.trailer).to_file().into_owned());
        assert_eq!(new, old, "{}", String::from_utf8_lossy(bytes));
        if let Ok(file) = &old {
            let AepxDrafts { buffer, nodes, trailer } = index_aepx(bytes, limits).unwrap();
            let index = TreeIndex::from_drafts(&buffer, nodes, trailer);
            let expected = TreeIndex::from_file(file).unwrap();
            assert_eq!(index.len(), expected.len());
            assert_eq!(index.trailer(), expected.trailer());
            let a: Vec<String> = index.walk().map(|(d, n)| format!("{d} {n:?}")).collect();
            let b: Vec<String> = expected.walk().map(|(d, n)| format!("{d} {n:?}")).collect();
            assert_eq!(a, b);
        }
    }

    /// The two readers give the same tree and errors on generated and damaged documents.
    #[test]
    fn the_two_readers_agree_on_generated_and_damaged_documents() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let noise = b"<>/=\"'&;#x0aZ \n![]-?";
        for _ in 0..600 {
            let mut budget = 80;
            let mut body = String::new();
            content(&mut rng, 0, &mut budget, &mut body);
            let x = doc(&body);
            for l in limit_sets() {
                same(x.as_bytes(), l);
            }
            for _ in 0..4 {
                let mut bad = x.clone().into_bytes();
                for _ in 0..1 + rng.below(3) {
                    let at = rng.below(bad.len());
                    bad[at] = noise[rng.below(noise.len())];
                }
                for l in limit_sets() {
                    same(&bad, l);
                }
            }
            let cut = rng.below(x.len());
            same(&x.as_bytes()[..cut], Limits::default());
            // Bytes that are not UTF-8, and a multi-byte character cut in half.
            let mut bad = x.clone().into_bytes();
            let at = rng.below(bad.len());
            bad.insert(at, 0xff);
            same(&bad, Limits::default());
            let open = format!("{}<string>é", x.trim_end().trim_end_matches("</AfterEffectsProject>")).into_bytes();
            same(&open[..open.len() - 1], Limits::default());
        }
    }

    /// The flat chunk list never reserves more slots than the input or the chunk limit allows.
    #[test]
    fn the_chunk_list_is_bounded_by_the_input_and_the_limit() {
        let many = format!("<AfterEffectsProject>{}</AfterEffectsProject>", "<a/>".repeat(10_000));
        let d = index_aepx(many.as_bytes(), Limits::default()).unwrap();
        assert_eq!(d.nodes.len(), 10_000);
        assert!(d.nodes.capacity() <= 2 * 10_000, "{} slots for 10 000 chunks", d.nodes.capacity());
        // The buffer is reserved once, at half the input, and holds nothing here.
        assert!(d.buffer.is_empty() && d.buffer.capacity() <= many.len() / 2);
        // At the limit the list never holds more slots than the limit.
        let d = index_aepx(many.as_bytes(), Limits { max_depth: 64, max_chunks: 10_000 }).unwrap();
        assert!(d.nodes.capacity() <= 10_000);
        let few = format!("<AfterEffectsProject>{}</AfterEffectsProject>", "<a/>".repeat(100));
        let d = index_aepx(few.as_bytes(), Limits { max_depth: 64, max_chunks: 100 }).unwrap();
        assert!(d.nodes.capacity() <= 100, "{} slots for at most 100 chunks", d.nodes.capacity());
        assert_eq!(index_aepx(few.as_bytes(), Limits { max_depth: 64, max_chunks: 99 }).err(), Some(AepError::TooManyChunks));
        // A tiny input cannot hold more than a third of its bytes in elements.
        let tiny = doc("<a/>");
        assert!(index_aepx(tiny.as_bytes(), Limits::default()).unwrap().nodes.capacity() <= tiny.len() / 3);
    }

    /// What a closed element drops is gone from the buffer and the chunk list, but was counted.
    #[test]
    fn dropped_content_leaves_nothing_behind_but_still_counts() {
        // `bdata` wins over the children: they are read, counted and dropped.
        let x = doc("<a bdata=\"01\"><b bdata=\"0203\"/><string>xyz</string><tdsn><string>q</string></tdsn></a>");
        let d = index_aepx(x.as_bytes(), Limits::default()).unwrap();
        assert_eq!((d.buffer.as_slice(), d.nodes.len()), (&[1u8][..], 1));
        assert_eq!(index_aepx(x.as_bytes(), Limits { max_depth: 64, max_chunks: 4 }).err(), Some(AepError::TooManyChunks));
        assert!(index_aepx(x.as_bytes(), Limits { max_depth: 64, max_chunks: 5 }).is_ok());
        // A child's error is raised although the child would be dropped.
        assert!(matches!(index_aepx(doc("<a bdata=\"01\"><numS></numS></a>").as_bytes(), Limits::default()), Err(AepError::Xml { .. })));
        // Only the first XMP packet stays in the buffer.
        let packets = doc(&"<ProjectXMPMetadata>packet</ProjectXMPMetadata>".repeat(1000));
        let d = index_aepx(packets.as_bytes(), Limits::default()).unwrap();
        assert_eq!((d.buffer.as_slice(), d.trailer), (&b"packet"[..], (0, 6)));
        // A number keeps its 4 or 8 bytes, not its text.
        let d = index_aepx(doc("<numS><string>   7   </string></numS><ppSn><string>1.5</string></ppSn>").as_bytes(), Limits::default()).unwrap();
        assert_eq!(d.buffer, [&[0, 0, 0, 7][..], &1.5f64.to_be_bytes()[..]].concat());
        assert_eq!(d.nodes.len(), 2);
    }

    /// Bodies keep what the element says they are, even where reading the same bytes as an `.aep`
    /// would decide otherwise: a nested id whose `bdata` is a chunk sequence stays data, an empty
    /// `btdk` stays an opaque list, an empty list at the deepest level stays a list.
    #[test]
    fn bodies_keep_the_kind_the_element_gives_them() {
        let x = doc("<tdsn bdata=\"5574663800000001610000\"/><btdk bdata=\"\"/><btdk bdata=\"4c4953540000000474657374\"/>");
        let AepxDrafts { buffer, nodes, trailer } = index_aepx(x.as_bytes(), Limits::default()).unwrap();
        let index = TreeIndex::from_drafts(&buffer, nodes, trailer);
        let top: Vec<_> = index.chunks().collect();
        assert_eq!(top.len(), 3);
        assert_eq!(top[0].bytes().map(<[u8]>::len), Some(11));
        assert!(top[0].children().next().is_none());
        assert!(top[1].is_opaque_list() && top[1].bytes() == Some(&[][..]));
        assert!(top[2].is_opaque_list() && top[2].bytes().map(<[u8]>::len) == Some(12));
        // 64 levels: the innermost (empty) list is a list, not opaque.
        let deep = format!("<AfterEffectsProject>{}{}</AfterEffectsProject>", "<Fold>".repeat(64), "</Fold>".repeat(64));
        let AepxDrafts { buffer, nodes, trailer } = index_aepx(deep.as_bytes(), Limits::default()).unwrap();
        let index = TreeIndex::from_drafts(&buffer, nodes, trailer);
        assert_eq!(index.walk().map(|(d, _)| d).max(), Some(64));
        assert!(index.walk().all(|(_, n)| n.is_list(b"Fold") && !n.is_opaque_list()));
        same(x.as_bytes(), Limits::default());
        same(deep.as_bytes(), Limits::default());
    }

    /// Hostile input is refused by both readers with the same errors.
    #[test]
    fn hostile_input_is_refused_alike_by_both_readers() {
        let hostile = [
            format!("<AfterEffectsProject>{}{}</AfterEffectsProject>", "<a>".repeat(100_000), "</a>".repeat(100_000)),
            format!("<AfterEffectsProject>{}", "<string>".repeat(1000)),
            doc(&format!("<a bdata=\"{}0\"/>", "00".repeat(100_000))),
            doc(&format!("<a bdata=\"{}zz\"/>", "00".repeat(100_000))),
            doc("<numS><string>4294967296</string></numS>"),
            doc("<ppSn><string>1e400x</string></ppSn>"),
            doc("<string>&#xD800;</string>"),
            doc("<string>&#99999999999;</string>"),
            doc(&format!("<Fold>{}", "<b/>".repeat(1000))),
            format!("<AfterEffectsProject>{}</AfterEffectsProject><x/>", "<a/>".repeat(10)),
            "<AfterEffectsProject><a bdata=\"00".to_string(),
            "<AfterEffectsProject><![CDATA[x".to_string(),
        ];
        for x in &hostile {
            assert!(read_aepx(x.as_bytes()).is_err(), "{}", &x[..x.len().min(80)]);
            for l in limit_sets() {
                same(x.as_bytes(), l);
            }
        }
    }

    /// Every `.aepx` of the fixture directory gives the same tree through both readers (skipped
    /// without `EFFECTCRAFT_AEP_FIXTURES`).
    #[test]
    fn every_fixture_aepx_gives_the_same_tree_through_both_readers() {
        let Some(dir) = std::env::var_os("EFFECTCRAFT_AEP_FIXTURES").filter(|d| !d.is_empty()) else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<std::path::PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aepx")).collect();
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap();
            for l in limit_sets() {
                let index = index_aepx(&bytes, l).map(|d| TreeIndex::from_drafts(&d.buffer, d.nodes, d.trailer).to_file().into_owned());
                assert_eq!(index, read_aepx_with_limits(&bytes, l), "{}", path.display());
            }
        }
    }
}
