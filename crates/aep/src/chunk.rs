//! The lossless chunk tree.

use std::borrow::Cow;

/// The id of a list chunk.
pub const LIST: [u8; 4] = *b"LIST";

/// Non-`LIST` chunk ids whose body is itself a sequence of chunks (observed: each holds one `Utf8`
/// chunk). They are parsed as [`Body::Chunks`] when the body is a well-formed chunk sequence and
/// kept as [`Body::Data`] otherwise.
pub const NESTED_IDS: [[u8; 4]; 3] = [*b"tdsn", *b"pdnm", *b"fnam"];

/// One chunk: a 4-byte id, a body and an optional pad byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk<'a> {
    /// The 4-byte id, as stored (case matters; some ids end in a space, e.g. `seq `).
    pub id: [u8; 4],
    /// The body.
    pub body: Body<'a>,
    /// The pad byte that followed an odd-sized body (normally `0`). `None` when the body is even,
    /// or when an odd body is the last thing of its parent and the file has no pad byte there.
    /// On writing, an odd body that is not the last sibling always gets a pad byte (`0` if this is
    /// `None`); the last sibling gets one only if this is `Some`.
    pub pad: Option<u8>,
}

/// The body of a [`Chunk`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body<'a> {
    /// Opaque bytes (the content of nearly all chunks).
    Data(Cow<'a, [u8]>),
    /// A body that is a sequence of chunks although the id is not `LIST` (see [`NESTED_IDS`]).
    Chunks(Vec<Chunk<'a>>),
    /// A `LIST`: a 4-byte list type followed by chunks.
    List { list_type: [u8; 4], children: Vec<Chunk<'a>> },
    /// A `LIST` whose content is not a well-formed chunk sequence (or is nested deeper than the
    /// limit), kept as bytes after the list type (observed: `LIST btdk` holds raw text).
    OpaqueList { list_type: [u8; 4], data: Cow<'a, [u8]> },
}

impl<'a> Chunk<'a> {
    /// A data chunk with the pad byte its length needs.
    pub fn data(id: [u8; 4], data: impl Into<Cow<'a, [u8]>>) -> Self {
        let data = data.into();
        let pad = if data.len() % 2 == 1 { Some(0) } else { None };
        Self { id, body: Body::Data(data), pad }
    }

    /// A `LIST` chunk.
    pub fn list(list_type: [u8; 4], children: Vec<Chunk<'a>>) -> Self {
        Self { id: LIST, body: Body::List { list_type, children }, pad: None }
    }

    /// The raw bytes of a [`Body::Data`] chunk, or the bytes after the list type of an opaque list.
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.body {
            Body::Data(d) => Some(d),
            Body::OpaqueList { data, .. } => Some(data),
            Body::Chunks(_) | Body::List { .. } => None,
        }
    }

    /// The child chunks of a list or nested chunk; empty for data.
    pub fn children(&self) -> &[Chunk<'a>] {
        match &self.body {
            Body::Chunks(c) | Body::List { children: c, .. } => c,
            Body::Data(_) | Body::OpaqueList { .. } => &[],
        }
    }

    /// Mutable child chunks; empty for data.
    pub fn children_mut(&mut self) -> &mut [Chunk<'a>] {
        match &mut self.body {
            Body::Chunks(c) | Body::List { children: c, .. } => c,
            Body::Data(_) | Body::OpaqueList { .. } => &mut [],
        }
    }

    /// The list type when this is a `LIST` (parsed or opaque).
    pub fn list_type(&self) -> Option<[u8; 4]> {
        match &self.body {
            Body::List { list_type, .. } | Body::OpaqueList { list_type, .. } => Some(*list_type),
            Body::Data(_) | Body::Chunks(_) => None,
        }
    }

    /// True when this is a `LIST` of the given type.
    pub fn is_list(&self, list_type: &[u8; 4]) -> bool {
        self.id == LIST && self.list_type().is_some_and(|t| &t == list_type)
    }

    /// True when this is a chunk (not a `LIST`) with the given id.
    pub fn is(&self, id: &[u8; 4]) -> bool {
        &self.id == id
    }

    /// The first child (non-list) chunk with this id.
    pub fn child(&self, id: &[u8; 4]) -> Option<&Chunk<'a>> {
        self.children().iter().find(|c| c.is(id))
    }

    /// The first child `LIST` of this type.
    pub fn child_list(&self, list_type: &[u8; 4]) -> Option<&Chunk<'a>> {
        self.children().iter().find(|c| c.is_list(list_type))
    }

    /// Visits every chunk below this one depth-first (children before later siblings), without
    /// recursion; the depth counts from 1 for the children of this chunk.
    pub fn descendants(&self) -> Walk<'_, 'a> {
        Walk { stack: vec![(self.children().iter(), 1)] }
    }

    /// The first descendant `LIST` of this type, searching depth-first.
    pub fn find_list(&self, list_type: &[u8; 4]) -> Option<&Chunk<'a>> {
        self.descendants().map(|(_, c)| c).find(|c| c.is_list(list_type))
    }

    /// Copies all borrowed bytes so the tree no longer borrows the input.
    pub fn into_owned(self) -> Chunk<'static> {
        let body = match self.body {
            Body::Data(d) => Body::Data(Cow::Owned(d.into_owned())),
            Body::Chunks(c) => Body::Chunks(c.into_iter().map(Chunk::into_owned).collect()),
            Body::List { list_type, children } => Body::List { list_type, children: children.into_iter().map(Chunk::into_owned).collect() },
            Body::OpaqueList { list_type, data } => Body::OpaqueList { list_type, data: Cow::Owned(data.into_owned()) },
        };
        Chunk { id: self.id, body, pad: self.pad }
    }
}

/// A parsed `.aep`: the `RIFX` container's chunks and the bytes after its declared end.
///
/// Writing it back with [`crate::write`] reproduces the input byte for byte (the container size
/// and every list size are recomputed from the tree and equal what the file declared).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AepFile<'a> {
    /// The top-level chunks (after the `Egg!` form type).
    pub chunks: Vec<Chunk<'a>>,
    /// The bytes after the container's declared end. After Effects puts an XMP packet there.
    pub trailer: Cow<'a, [u8]>,
}

impl<'a> AepFile<'a> {
    /// The first top-level chunk with this id.
    pub fn chunk(&self, id: &[u8; 4]) -> Option<&Chunk<'a>> {
        self.chunks.iter().find(|c| c.is(id))
    }

    /// The first top-level `LIST` of this type.
    pub fn list(&self, list_type: &[u8; 4]) -> Option<&Chunk<'a>> {
        self.chunks.iter().find(|c| c.is_list(list_type))
    }

    /// The XMP packet in the trailer (from `<?xpacket begin` to the end of `<?xpacket end ...?>`).
    pub fn xmp(&self) -> Option<&[u8]> {
        let start = find(&self.trailer, b"<?xpacket begin")?;
        let rest = self.trailer.get(start..)?;
        let end_marker = find(rest, b"<?xpacket end")?;
        let tail = rest.get(end_marker..)?;
        let close = find(tail, b"?>")?;
        rest.get(..end_marker.checked_add(close)?.checked_add(2)?)
    }

    /// Visits every chunk depth-first with its nesting depth (the top level is 1), without recursion.
    pub fn walk(&self) -> Walk<'_, 'a> {
        Walk { stack: vec![(self.chunks.iter(), 1)] }
    }

    /// Copies all borrowed bytes so the file no longer borrows the input.
    pub fn into_owned(self) -> AepFile<'static> {
        AepFile { chunks: self.chunks.into_iter().map(Chunk::into_owned).collect(), trailer: Cow::Owned(self.trailer.into_owned()) }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Depth-first iterator over all chunks, see [`AepFile::walk`].
pub struct Walk<'f, 'a> {
    stack: Vec<(std::slice::Iter<'f, Chunk<'a>>, usize)>,
}

impl<'f, 'a> Iterator for Walk<'f, 'a> {
    type Item = (usize, &'f Chunk<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (iter, depth) = self.stack.last_mut()?;
            let depth = *depth;
            match iter.next() {
                Some(chunk) => {
                    let kids = chunk.children();
                    if !kids.is_empty() {
                        self.stack.push((kids.iter(), depth.saturating_add(1)));
                    }
                    return Some((depth, chunk));
                }
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}
