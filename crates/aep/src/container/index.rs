//! A read-only, flat index of the chunk tree for readers that only look (the importer).
//!
//! [`TreeIndex`] holds every chunk of a file in one `Vec` in depth-first order (a chunk, then its
//! whole subtree, then its next sibling). Each entry keeps its id, list type, pad byte, the bytes of
//! its body (borrowed from the input) and the index one past its subtree, so:
//! - a chunk's subtree is the index range `at + 1 .. end`,
//! - its direct children are found by stepping from `at + 1` to each child's `end`,
//! - the whole tree is one array, grown a few times while it is built and freed at once; it never
//!   holds more slots than the input has room for chunk headers or the chunk limit allows.
//!
//! [`crate::container::parse`] builds the owned [`Chunk`] tree instead (one `Vec` per list), which
//! the exporter and editors need. [`TreeIndex::to_file`] and [`Node::to_chunk`] convert the index
//! into that tree where an owned one is needed.
//!
//! [`TreeIndex::parse_with_limits`] walks the input exactly as the [`Chunk`] parser does: the same
//! chunks are counted against [`Limits::max_chunks`] in the same order, the same lists are kept
//! opaque at [`Limits::max_depth`] or when damaged, and the same errors come back. A test compares
//! the two on the corpus and on generated and damaged trees. Two errors are its own, and both are
//! stricter than the [`Chunk`] parser (which would abort or could not address the tree):
//! [`AepError::TooLarge`] when the array cannot grow (a failed `try_reserve`), and
//! [`AepError::TooManyChunks`] for a tree of more than `u32::MAX` chunks.
//!
//! An `.aepx` is indexed by its own reader (`container/aepx.rs`), which decodes every body into
//! one buffer and fills the entries as it reads; the index then borrows that buffer
//! (`TreeIndex::from_drafts`). It gives the tree [`crate::container::read_aepx`] gives.
//!
//! [`Node`] is the handle the typed view ([`crate::container::Project`] and the types below it)
//! reads through: a chunk of either tree. The view works the same on both.

use std::borrow::Cow;
use std::fmt;

use crate::container::bytes::be_u32;
use crate::container::chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS, Walk};
use crate::container::error::AepError;
use crate::container::read::{Limits, container_end, id4};

/// What an entry's body is (the [`Body`] variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Data,
    /// [`Body::Chunks`].
    Nested,
    List,
    OpaqueList,
}

/// One chunk of a [`TreeIndex`].
#[derive(Debug, Clone, Copy)]
struct Entry<'a> {
    /// The body bytes of a data chunk, or the bytes after the list type of an opaque list; empty
    /// for a list or nested chunk (their content is their children).
    data: &'a [u8],
    id: [u8; 4],
    /// The list type of a `LIST` (parsed or opaque); zero otherwise.
    list_type: [u8; 4],
    /// One past the last entry of this chunk's subtree.
    end: u32,
    kind: Kind,
    pad: Option<u8>,
}

impl<'a> Entry<'a> {
    /// An entry for a chunk whose body is not read yet.
    fn placeholder(id: [u8; 4]) -> Self {
        Self { data: &[], id, list_type: [0; 4], end: 0, kind: Kind::Data, pad: None }
    }
}

/// An entry of an index whose bytes are still being written (the `.aepx` reader decodes into a
/// buffer that grows, so it keeps positions, not slices): the body is `start..start + len` of
/// that buffer. The fields are those of [`Entry`]; [`TreeIndex::from_drafts`] turns the list
/// into an index once the buffer is final.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Draft {
    pub(crate) start: usize,
    pub(crate) len: usize,
    pub(crate) id: [u8; 4],
    pub(crate) list_type: [u8; 4],
    pub(crate) end: u32,
    pub(crate) kind: Kind,
    pub(crate) pad: Option<u8>,
}

impl Draft {
    /// A data chunk with no body yet.
    pub(crate) fn placeholder(id: [u8; 4]) -> Self {
        Self { start: 0, len: 0, id, list_type: [0; 4], end: 0, kind: Kind::Data, pad: None }
    }
}

/// The chunk tree of a file as one flat, read-only array (see the module documentation).
#[derive(Clone)]
pub struct TreeIndex<'a> {
    nodes: Vec<Entry<'a>>,
    trailer: &'a [u8],
}

impl fmt::Debug for TreeIndex<'_> {
    /// The sizes only: the chunk count and the trailer length.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeIndex").field("chunks", &self.nodes.len()).field("trailer", &self.trailer.len()).finish()
    }
}

/// An index that does not fit a `u32` (more than 4 billion chunks) is refused as too many chunks.
fn index_u32(at: usize) -> Result<u32, AepError> {
    u32::try_from(at).map_err(|_| AepError::TooManyChunks)
}

impl<'a> TreeIndex<'a> {
    /// Indexes an `.aep` with the default [`Limits`].
    ///
    /// # Errors
    /// The errors of [`crate::container::parse`], for the same inputs.
    pub fn parse(data: &'a [u8]) -> Result<Self, AepError> {
        Self::parse_with_limits(data, Limits::default())
    }

    /// Indexes an `.aep` with explicit [`Limits`].
    ///
    /// # Errors
    /// The errors of [`crate::container::parse_with_limits`], for the same inputs and limits; also
    /// [`AepError::TooManyChunks`] for a tree of more than `u32::MAX` chunks and
    /// [`AepError::TooLarge`] when the array cannot be allocated.
    pub fn parse_with_limits(data: &'a [u8], limits: Limits) -> Result<Self, AepError> {
        let end = container_end(data)?;
        // Every chunk header takes 8 bytes of its own and is counted once, so the tree never has
        // more chunks than this.
        let bound = (end.saturating_sub(12) / 8).min(limits.max_chunks);
        let mut builder = Builder { data, limits, count: 0, nodes: Vec::new(), bound };
        // A small start; [`Builder::push`] doubles it as chunks arrive, never past the bound, so
        // the array stays proportional to the chunk count (a file of a few large chunks gets a few
        // slots, not one per 32 bytes). `try_reserve_exact` so it fails nothing here.
        let _ = builder.nodes.try_reserve_exact(bound.min(INITIAL_SLOTS));
        builder.sequence(12, end, 1)?;
        let trailer = data.get(end..).unwrap_or(&[]);
        Ok(Self { nodes: builder.nodes, trailer })
    }

    /// The index of the entries `drafts` (in depth-first order, see [`Draft`]) whose bodies and
    /// trailer are ranges of `buffer`. A range outside `buffer` reads as empty. The list is turned
    /// into entries; a [`Draft`] and an [`Entry`] have the same size and alignment, so the standard
    /// library reuses the list's allocation for them.
    pub(crate) fn from_drafts(buffer: &'a [u8], drafts: Vec<Draft>, trailer: (usize, usize)) -> Self {
        let slice = |start: usize, len: usize| start.checked_add(len).and_then(|end| buffer.get(start..end)).unwrap_or(&[]);
        let nodes =
            drafts.into_iter().map(|d| Entry { data: slice(d.start, d.len), id: d.id, list_type: d.list_type, end: d.end, kind: d.kind, pad: d.pad }).collect();
        let trailer = buffer.get(trailer.0..trailer.1).unwrap_or(&[]);
        Self { nodes, trailer }
    }

    /// Indexes an already parsed (or built) chunk tree. The index borrows the file's bytes.
    ///
    /// # Errors
    /// [`AepError::TooManyChunks`] for a tree of more than `u32::MAX` chunks.
    pub fn from_file(file: &'a AepFile<'_>) -> Result<Self, AepError> {
        // Sized once, to the chunks the tree holds.
        let mut nodes: Vec<Entry<'a>> = Vec::new();
        nodes.try_reserve_exact(file.walk().count()).map_err(|_| AepError::TooLarge)?;
        // Each frame: the siblings still to visit and the entry whose subtree they fill.
        let mut stack: Vec<(std::slice::Iter<'a, Chunk<'_>>, Option<usize>)> = vec![(file.chunks.iter(), None)];
        while let Some((iter, parent)) = stack.last_mut() {
            let parent = *parent;
            match iter.next() {
                Some(chunk) => {
                    let at = nodes.len();
                    // The entry after this one must still have a `u32` index.
                    let next = index_u32(at.checked_add(1).ok_or(AepError::TooManyChunks)?)?;
                    let (kind, data, list_type) = match &chunk.body {
                        Body::Data(d) => (Kind::Data, &d[..], [0; 4]),
                        Body::Chunks(_) => (Kind::Nested, &[][..], [0; 4]),
                        Body::List { list_type, .. } => (Kind::List, &[][..], *list_type),
                        Body::OpaqueList { list_type, data } => (Kind::OpaqueList, &data[..], *list_type),
                    };
                    nodes.push(Entry { data, id: chunk.id, list_type, end: next, kind, pad: chunk.pad });
                    let kids = chunk.children();
                    if !kids.is_empty() {
                        stack.push((kids.iter(), Some(at)));
                    }
                }
                None => {
                    stack.pop();
                    if let Some(p) = parent {
                        let end = index_u32(nodes.len())?;
                        if let Some(e) = nodes.get_mut(p) {
                            e.end = end;
                        }
                    }
                }
            }
        }
        Ok(Self { nodes, trailer: &file.trailer[..] })
    }

    /// The number of chunks in the tree (at every depth).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True when the container holds no chunks.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The bytes after the container's declared end (see [`AepFile::trailer`]).
    pub fn trailer(&self) -> &'a [u8] {
        self.trailer
    }

    /// The top-level chunks.
    pub fn chunks(&self) -> Nodes<'_, 'a> {
        Nodes(NodesRepr::Index { tree: self, next: 0, end: u32::try_from(self.nodes.len()).unwrap_or(u32::MAX) })
    }

    /// The first top-level chunk with this id.
    pub fn chunk(&self, id: &[u8; 4]) -> Option<Node<'_, 'a>> {
        self.chunks().find(|c| c.is(id))
    }

    /// The first top-level `LIST` of this type.
    pub fn list(&self, list_type: &[u8; 4]) -> Option<Node<'_, 'a>> {
        self.chunks().find(|c| c.is_list(list_type))
    }

    /// Visits every chunk depth-first with its nesting depth (the top level is 1), as
    /// [`AepFile::walk`] does.
    pub fn walk(&self) -> NodeWalk<'_, 'a> {
        NodeWalk(WalkRepr::Index { tree: self, next: 0, end: u32::try_from(self.nodes.len()).unwrap_or(u32::MAX), open: Vec::new() })
    }

    /// The owned chunk tree (the bodies stay borrowed from the input): what
    /// [`crate::container::parse`] gives for the same file.
    pub fn to_file(&self) -> AepFile<'a> {
        AepFile { chunks: self.build(0, u32::try_from(self.nodes.len()).unwrap_or(u32::MAX)), trailer: Cow::Borrowed(self.trailer) }
    }

    /// The entry at `at`, `None` past the end.
    fn entry(&self, at: u32) -> Option<&Entry<'a>> {
        self.nodes.get(usize::try_from(at).ok()?)
    }

    /// The end of the subtree at `at`; never before `at + 1`, so walks always move on.
    fn end_of(&self, at: u32) -> u32 {
        self.entry(at).map_or(u32::MAX, |e| e.end).max(at.saturating_add(1))
    }

    /// The owned chunks for the siblings that start at `next` and fill `next..end`, without
    /// recursion.
    fn build(&self, next: u32, end: u32) -> Vec<Chunk<'a>> {
        struct Frame<'a> {
            /// The list or nested chunk being filled; `None` for the outermost range.
            at: Option<u32>,
            kids: Vec<Chunk<'a>>,
            next: u32,
            end: u32,
        }
        let mut stack = vec![Frame { at: None, kids: Vec::new(), next, end }];
        loop {
            let Some(top) = stack.last_mut() else { return Vec::new() };
            if top.next < top.end {
                let at = top.next;
                top.next = self.end_of(at);
                let Some(e) = self.entry(at).copied() else {
                    top.next = top.end;
                    continue;
                };
                match e.kind {
                    Kind::List | Kind::Nested => {
                        let sub_end = self.end_of(at);
                        stack.push(Frame { at: Some(at), kids: Vec::new(), next: at.saturating_add(1), end: sub_end });
                    }
                    Kind::Data => top.kids.push(Chunk { id: e.id, body: Body::Data(Cow::Borrowed(e.data)), pad: e.pad }),
                    Kind::OpaqueList => {
                        top.kids.push(Chunk { id: e.id, body: Body::OpaqueList { list_type: e.list_type, data: Cow::Borrowed(e.data) }, pad: e.pad });
                    }
                }
            } else {
                let Some(frame) = stack.pop() else { return Vec::new() };
                let Some(at) = frame.at else { return frame.kids };
                let Some(e) = self.entry(at).copied() else { continue };
                let body = match e.kind {
                    Kind::Nested => Body::Chunks(frame.kids),
                    Kind::List | Kind::Data | Kind::OpaqueList => Body::List { list_type: e.list_type, children: frame.kids },
                };
                let chunk = Chunk { id: e.id, body, pad: e.pad };
                match stack.last_mut() {
                    Some(parent) => parent.kids.push(chunk),
                    None => return vec![chunk],
                }
            }
        }
    }
}

/// The slots a [`TreeIndex`] starts with (32 KiB); it doubles from there.
const INITIAL_SLOTS: usize = 1024;

/// Builds a [`TreeIndex`]; mirrors `read::Parser` step for step (see the module documentation).
struct Builder<'a> {
    data: &'a [u8],
    limits: Limits,
    count: usize,
    nodes: Vec<Entry<'a>>,
    /// The most chunks the input can hold under the limits: `nodes` never reserves past it.
    bound: usize,
}

impl<'a> Builder<'a> {
    /// Appends an entry. A full array grows by its length (at least 16), but never past
    /// [`Self::bound`]; an allocation the target cannot make is an error, not an abort.
    fn push(&mut self, entry: Entry<'a>) -> Result<(), AepError> {
        let len = self.nodes.len();
        if len == self.nodes.capacity() {
            let more = len.max(16).min(self.bound.saturating_sub(len)).max(1);
            self.nodes.try_reserve_exact(more).map_err(|_| AepError::TooLarge)?;
        }
        self.nodes.push(entry);
        Ok(())
    }

    /// Appends the sibling chunks that exactly fill `start..end`, each followed by its subtree.
    fn sequence(&mut self, start: usize, end: usize, depth: usize) -> Result<(), AepError> {
        let mut pos = start;
        while pos < end {
            let short = AepError::Malformed { offset: pos, what: "bytes left over that are too short for a chunk header" };
            let body_start = pos.checked_add(8).filter(|b| *b <= end).ok_or(short)?;
            let id = id4(self.data, pos).ok_or(AepError::Malformed { offset: pos, what: "short read" })?;
            let size = be_u32(self.data, pos + 4).ok_or(AepError::Malformed { offset: pos, what: "short read" })? as usize;
            let avail = end - body_start;
            if size > avail {
                return Err(AepError::ChunkTooLarge { offset: pos, size: size as u64, available: avail as u64 });
            }
            self.count = self.count.saturating_add(1);
            if self.count > self.limits.max_chunks {
                return Err(AepError::TooManyChunks);
            }
            let at = self.nodes.len();
            // This entry and the one after it must have `u32` indices.
            index_u32(at.checked_add(1).ok_or(AepError::TooManyChunks)?)?;
            self.push(Entry::placeholder(id))?;
            let body_end = body_start + size;
            self.body(at, id, body_start, body_end, depth)?;
            let mut next = body_end;
            let mut pad = None;
            if size % 2 == 1 && next < end {
                pad = Some(self.data.get(next).copied().ok_or(AepError::Malformed { offset: next, what: "missing pad byte" })?);
                next += 1;
            }
            let subtree_end = index_u32(self.nodes.len())?;
            if let Some(e) = self.nodes.get_mut(at) {
                e.pad = pad;
                e.end = subtree_end;
            }
            pos = next;
        }
        Ok(())
    }

    /// Like [`Self::sequence`] but a structural problem yields `false` and removes what the
    /// attempt appended (the caller keeps bytes). The chunk count keeps the attempt's chunks, as
    /// the [`Chunk`] parser's does.
    fn tentative(&mut self, start: usize, end: usize, depth: usize) -> Result<bool, AepError> {
        let mark = self.nodes.len();
        match self.sequence(start, end, depth) {
            Ok(()) => Ok(true),
            Err(e) if e.is_structural() => {
                self.nodes.truncate(mark);
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    /// The input bytes `start..end` (empty when out of range).
    fn slice(&self, start: usize, end: usize) -> &'a [u8] {
        self.data.get(start..end).unwrap_or(&[])
    }

    /// Fills in the body of the entry at `at` once the parser knows what it is.
    fn set(&mut self, at: usize, kind: Kind, data: &'a [u8], list_type: [u8; 4]) {
        if let Some(e) = self.nodes.get_mut(at) {
            e.kind = kind;
            e.data = data;
            e.list_type = list_type;
        }
    }

    /// Fills in the entry at `at` for the body `start..end` and appends its subtree.
    fn body(&mut self, at: usize, id: [u8; 4], start: usize, end: usize, depth: usize) -> Result<(), AepError> {
        let len = end - start;
        if id == LIST && len >= 4 {
            let list_type = id4(self.data, start).ok_or(AepError::Malformed { offset: start, what: "short read" })?;
            if depth < self.limits.max_depth && self.tentative(start + 4, end, depth + 1)? {
                self.set(at, Kind::List, &[], list_type);
            } else {
                let data = self.slice(start + 4, end);
                self.set(at, Kind::OpaqueList, data, list_type);
            }
            return Ok(());
        }
        if NESTED_IDS.contains(&id) && len >= 8 && depth < self.limits.max_depth {
            let mark = self.nodes.len();
            if self.tentative(start, end, depth + 1)? && self.nodes.len() > mark {
                self.set(at, Kind::Nested, &[], [0; 4]);
                return Ok(());
            }
        }
        let data = self.slice(start, end);
        self.set(at, Kind::Data, data, [0; 4]);
        Ok(())
    }
}

/// A chunk of either tree: an owned [`Chunk`] or an entry of a [`TreeIndex`]. The typed view
/// reads through it, so it works the same on both. Cheap to copy.
#[derive(Clone, Copy)]
pub struct Node<'f, 'a>(NodeRepr<'f, 'a>);

#[derive(Clone, Copy)]
enum NodeRepr<'f, 'a> {
    Chunk(&'f Chunk<'a>),
    Index(&'f TreeIndex<'a>, u32),
}

impl<'f, 'a> From<&'f Chunk<'a>> for Node<'f, 'a> {
    /// The handle for this value.
    fn from(chunk: &'f Chunk<'a>) -> Self {
        Node(NodeRepr::Chunk(chunk))
    }
}

/// Prints the chunk as [`Chunk`]'s `Debug` does, whichever tree it is in, without copying the
/// subtree.
impl fmt::Debug for Node<'_, '_> {
    /// The chunk as `Chunk`'s `Debug` prints it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            NodeRepr::Chunk(c) => fmt::Debug::fmt(c, f),
            NodeRepr::Index(..) => f.debug_struct("Chunk").field("id", &self.id()).field("body", &BodyDebug(*self)).field("pad", &self.pad()).finish(),
        }
    }
}

/// The body of an index [`Node`], printed as [`Body`]'s `Debug` prints it.
struct BodyDebug<'f, 'a>(Node<'f, 'a>);

/// The children of a [`Node`], printed as a list.
struct ChildrenDebug<'f, 'a>(Node<'f, 'a>);

impl fmt::Debug for BodyDebug<'_, '_> {
    /// The body as `Body`'s `Debug` prints it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(e) = self.0.entry() else { return f.write_str("?") };
        match e.kind {
            Kind::Data => f.debug_tuple("Data").field(&e.data).finish(),
            Kind::Nested => f.debug_tuple("Chunks").field(&ChildrenDebug(self.0)).finish(),
            Kind::List => f.debug_struct("List").field("list_type", &e.list_type).field("children", &ChildrenDebug(self.0)).finish(),
            Kind::OpaqueList => f.debug_struct("OpaqueList").field("list_type", &e.list_type).field("data", &e.data).finish(),
        }
    }
}

impl fmt::Debug for ChildrenDebug<'_, '_> {
    /// The children as a list.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.children()).finish()
    }
}

impl<'f, 'a> Node<'f, 'a> {
    /// The entry at `at`, `None` past the end.
    fn entry(self) -> Option<&'f Entry<'a>> {
        match self.0 {
            NodeRepr::Index(tree, at) => tree.entry(at),
            NodeRepr::Chunk(_) => None,
        }
    }

    /// The [`Chunk`] when this node is one of an owned tree; `None` for an index entry (see
    /// [`Self::to_chunk`]).
    pub fn as_chunk(self) -> Option<&'f Chunk<'a>> {
        match self.0 {
            NodeRepr::Chunk(c) => Some(c),
            NodeRepr::Index(..) => None,
        }
    }

    /// The chunk and its subtree as an owned [`Chunk`] (bodies stay borrowed).
    pub fn to_chunk(self) -> Chunk<'a> {
        match self.0 {
            NodeRepr::Chunk(c) => c.clone(),
            NodeRepr::Index(tree, at) => tree.build(at, tree.end_of(at)).into_iter().next().unwrap_or_else(|| Chunk::data(self.id(), &[][..])),
        }
    }

    /// The 4-byte id (see [`Chunk::id`]).
    pub fn id(self) -> [u8; 4] {
        match self.0 {
            NodeRepr::Chunk(c) => c.id,
            NodeRepr::Index(..) => self.entry().map_or([0; 4], |e| e.id),
        }
    }

    /// The pad byte (see [`Chunk::pad`]).
    pub fn pad(self) -> Option<u8> {
        match self.0 {
            NodeRepr::Chunk(c) => c.pad,
            NodeRepr::Index(..) => self.entry().and_then(|e| e.pad),
        }
    }

    /// The raw bytes of a data chunk, or the bytes after the list type of an opaque list (see
    /// [`Chunk::bytes`]).
    pub fn bytes(self) -> Option<&'f [u8]> {
        match self.0 {
            NodeRepr::Chunk(c) => c.bytes(),
            NodeRepr::Index(..) => self.entry().filter(|e| matches!(e.kind, Kind::Data | Kind::OpaqueList)).map(|e| e.data),
        }
    }

    /// The child chunks of a list or nested chunk; none for data.
    pub fn children(self) -> Nodes<'f, 'a> {
        match self.0 {
            NodeRepr::Chunk(c) => Nodes(NodesRepr::Slice(c.children().iter())),
            NodeRepr::Index(tree, at) => {
                let end = match tree.entry(at) {
                    Some(e) if matches!(e.kind, Kind::List | Kind::Nested) => tree.end_of(at),
                    _ => at.saturating_add(1),
                };
                Nodes(NodesRepr::Index { tree, next: at.saturating_add(1), end })
            }
        }
    }

    /// The list type when this is a `LIST` (parsed or opaque).
    pub fn list_type(self) -> Option<[u8; 4]> {
        match self.0 {
            NodeRepr::Chunk(c) => c.list_type(),
            NodeRepr::Index(..) => self.entry().filter(|e| matches!(e.kind, Kind::List | Kind::OpaqueList)).map(|e| e.list_type),
        }
    }

    /// True when this is a `LIST` of the given type.
    pub fn is_list(self, list_type: &[u8; 4]) -> bool {
        match self.0 {
            NodeRepr::Chunk(c) => c.is_list(list_type),
            NodeRepr::Index(..) => self.entry().is_some_and(|e| e.id == LIST && matches!(e.kind, Kind::List | Kind::OpaqueList) && &e.list_type == list_type),
        }
    }

    /// True when this is a `LIST` whose content could not be read as chunks.
    pub fn is_opaque_list(self) -> bool {
        match self.0 {
            NodeRepr::Chunk(c) => c.is_opaque_list(),
            NodeRepr::Index(..) => self.entry().is_some_and(|e| e.kind == Kind::OpaqueList),
        }
    }

    /// True when this chunk has the given id.
    pub fn is(self, id: &[u8; 4]) -> bool {
        self.id() == *id
    }

    /// The first child chunk with this id.
    pub fn child(self, id: &[u8; 4]) -> Option<Self> {
        self.children().find(|c| c.is(id))
    }

    /// The first child `LIST` of this type.
    pub fn child_list(self, list_type: &[u8; 4]) -> Option<Self> {
        self.children().find(|c| c.is_list(list_type))
    }

    /// Every chunk below this one depth-first (children before later siblings), with the depth
    /// counted from 1 for the children of this chunk.
    pub fn descendants(self) -> NodeWalk<'f, 'a> {
        match self.0 {
            NodeRepr::Chunk(c) => NodeWalk(WalkRepr::Chunk(c.descendants())),
            NodeRepr::Index(tree, at) => {
                let end = match tree.entry(at) {
                    Some(e) if matches!(e.kind, Kind::List | Kind::Nested) => tree.end_of(at),
                    _ => at.saturating_add(1),
                };
                NodeWalk(WalkRepr::Index { tree, next: at.saturating_add(1), end, open: Vec::new() })
            }
        }
    }

    /// The first descendant `LIST` of this type, searching depth-first.
    pub fn find_list(self, list_type: &[u8; 4]) -> Option<Self> {
        match self.0 {
            NodeRepr::Chunk(c) => c.find_list(list_type).map(Node::from),
            // The subtree is a contiguous range in depth-first order: a plain scan.
            NodeRepr::Index(tree, at) => {
                let first = usize::try_from(at).ok()?.checked_add(1)?;
                let end = usize::try_from(tree.end_of(at)).ok()?.min(tree.nodes.len());
                let range = tree.nodes.get(first..end)?;
                let i = range.iter().position(|e| e.id == LIST && matches!(e.kind, Kind::List | Kind::OpaqueList) && &e.list_type == list_type)?;
                let found = u32::try_from(first.checked_add(i)?).ok()?;
                Some(Node(NodeRepr::Index(tree, found)))
            }
        }
    }
}

/// The children of a [`Node`] (or the top-level chunks of a [`TreeIndex`]), in file order.
#[derive(Clone)]
pub struct Nodes<'f, 'a>(NodesRepr<'f, 'a>);

#[derive(Clone)]
enum NodesRepr<'f, 'a> {
    Slice(std::slice::Iter<'f, Chunk<'a>>),
    Index { tree: &'f TreeIndex<'a>, next: u32, end: u32 },
}

impl<'f, 'a> Iterator for Nodes<'f, 'a> {
    type Item = Node<'f, 'a>;

    /// The next item, `None` at the end of the range.
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.0 {
            NodesRepr::Slice(it) => it.next().map(Node::from),
            NodesRepr::Index { tree, next, end } => {
                if *next >= *end {
                    return None;
                }
                let (tree, at) = (*tree, *next);
                tree.entry(at)?;
                *next = tree.end_of(at);
                Some(Node(NodeRepr::Index(tree, at)))
            }
        }
    }
}

/// A depth-first walk over [`Node`]s with their depth; see [`Node::descendants`] and
/// [`TreeIndex::walk`].
pub struct NodeWalk<'f, 'a>(WalkRepr<'f, 'a>);

enum WalkRepr<'f, 'a> {
    Chunk(Walk<'f, 'a>),
    /// `open` holds the subtree ends of the entries above the next one (its ancestors inside the
    /// walked range); their count is the next entry's depth minus one.
    Index {
        tree: &'f TreeIndex<'a>,
        next: u32,
        end: u32,
        open: Vec<u32>,
    },
}

impl<'f, 'a> Iterator for NodeWalk<'f, 'a> {
    type Item = (usize, Node<'f, 'a>);

    /// The next item, `None` at the end of the range.
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.0 {
            WalkRepr::Chunk(w) => w.next().map(|(d, c)| (d, Node::from(c))),
            WalkRepr::Index { tree, next, end, open } => {
                if *next >= *end {
                    return None;
                }
                let (tree, at) = (*tree, *next);
                tree.entry(at)?;
                while open.last().is_some_and(|e| *e <= at) {
                    open.pop();
                }
                let depth = open.len().saturating_add(1);
                let sub_end = tree.end_of(at);
                if sub_end > at.saturating_add(1) {
                    open.push(sub_end);
                }
                *next = at.saturating_add(1);
                Some((depth, Node(NodeRepr::Index(tree, at))))
            }
        }
    }
}

/// The top level of either tree: what the project settings and the version are read from.
#[derive(Clone, Copy)]
pub(crate) enum Root<'f, 'a> {
    File(&'f AepFile<'a>),
    Index(&'f TreeIndex<'a>),
}

impl<'f, 'a> From<&'f AepFile<'a>> for Root<'f, 'a> {
    /// The handle for this value.
    fn from(file: &'f AepFile<'a>) -> Self {
        Root::File(file)
    }
}

impl<'f, 'a> From<&'f TreeIndex<'a>> for Root<'f, 'a> {
    /// The handle for this value.
    fn from(index: &'f TreeIndex<'a>) -> Self {
        Root::Index(index)
    }
}

impl<'f, 'a> Root<'f, 'a> {
    /// The top-level chunks.
    pub(crate) fn chunks(self) -> Nodes<'f, 'a> {
        match self {
            Root::File(f) => Nodes(NodesRepr::Slice(f.chunks.iter())),
            Root::Index(t) => t.chunks(),
        }
    }

    /// The first top-level chunk with this id.
    pub(crate) fn chunk(self, id: &[u8; 4]) -> Option<Node<'f, 'a>> {
        self.chunks().find(|c| c.is(id))
    }

    /// The first top-level `LIST` of this type.
    pub(crate) fn list(self, list_type: &[u8; 4]) -> Option<Node<'f, 'a>> {
        self.chunks().find(|c| c.is_list(list_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{parse_with_limits, write};

    /// A tiny deterministic generator (xorshift64*), so the tests need no extra crate.
    struct Rng(u64);

    impl Rng {
        /// The next pseudo-random number (xorshift64*).
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        /// A number below `n` (0 for `n == 0`).
        fn below(&mut self, n: u64) -> u64 {
            if n == 0 { 0 } else { self.next() % n }
        }
    }

    /// A random tree: lists, nested-id chunks holding a `Utf8`, data of odd and even sizes, opaque
    /// lists, empty lists, deep chains.
    fn tree(rng: &mut Rng, depth: usize, budget: &mut usize) -> Vec<Chunk<'static>> {
        let n = rng.below(6);
        let mut out = Vec::new();
        for _ in 0..n {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let pick = rng.below(10);
            let chunk = if depth < 12 && pick < 4 {
                let types: [&[u8; 4]; 4] = [b"Fold", b"tdgp", b"tdbs", b"Layr"];
                let t = *types[usize::try_from(rng.below(4)).expect("small")];
                Chunk::list(t, tree(rng, depth + 1, budget))
            } else if pick == 4 {
                let ids: [&[u8; 4]; 3] = [b"tdsn", b"pdnm", b"fnam"];
                let id = *ids[usize::try_from(rng.below(3)).expect("small")];
                let text = vec![b'a'; usize::try_from(rng.below(5)).expect("small")];
                Chunk { id, body: Body::Chunks(vec![Chunk::data(*b"Utf8", text)]), pad: None }
            } else if pick == 5 {
                let raw = vec![0x5a; usize::try_from(rng.below(9)).expect("small")];
                Chunk { id: LIST, body: Body::OpaqueList { list_type: *b"btdk", data: raw.into() }, pad: None }
            } else {
                let ids: [&[u8; 4]; 4] = [b"cdat", b"tdmn", b"Utf8", b"idta"];
                let id = *ids[usize::try_from(rng.below(4)).expect("small")];
                Chunk::data(id, vec![u8::try_from(rng.below(256)).expect("byte"); usize::try_from(rng.below(13)).expect("small")])
            };
            out.push(chunk);
        }
        out
    }

    /// Both readers on the same bytes and limits: the same result, errors included; and the
    /// index's walk visits the same chunks at the same depths as the tree's.
    fn same(bytes: &[u8], limits: Limits) {
        let old = parse_with_limits(bytes, limits);
        let new = TreeIndex::parse_with_limits(bytes, limits);
        match (&old, &new) {
            (Ok(file), Ok(index)) => {
                assert_eq!(&index.to_file(), file, "the index converts to the parsed tree");
                let a: Vec<(usize, Chunk<'_>)> = file.walk().map(|(d, c)| (d, c.clone())).collect();
                let b: Vec<(usize, Chunk<'_>)> = index.walk().map(|(d, n)| (d, n.to_chunk())).collect();
                assert_eq!(a, b, "the walks agree");
                assert_eq!(index.len(), a.len());
                // Indexing the parsed tree gives the same index.
                let again = TreeIndex::from_file(file).expect("index");
                assert_eq!(again.to_file(), *file);
            }
            (Err(a), Err(b)) => assert_eq!(a, b, "the same error"),
            _ => panic!("one reader failed: {:?} vs {:?}", old.as_ref().err(), new.as_ref().err()),
        }
    }

    /// The default limits and four tight ones (depth 1 and 3, 7 and 40 chunks).
    fn limits() -> [Limits; 5] {
        [
            Limits::default(),
            Limits { max_depth: 1, max_chunks: 10_000_000 },
            Limits { max_depth: 3, max_chunks: 10_000_000 },
            Limits { max_depth: 64, max_chunks: 7 },
            Limits { max_depth: 64, max_chunks: 40 },
        ]
    }

    /// The index matches the chunk parser on generated and damaged trees.
    #[test]
    fn the_index_matches_the_chunk_parser_on_generated_and_damaged_trees() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..400 {
            let mut budget = 120;
            let chunks = tree(&mut rng, 0, &mut budget);
            let file = AepFile { chunks, trailer: Cow::Borrowed(b"<?xpacket begin?>x<?xpacket end?>") };
            let bytes = write(&file).expect("write");
            for l in limits() {
                same(&bytes, l);
            }
            // Damage: overwrite a few bytes past the container header (sizes, ids, list types).
            for _ in 0..4 {
                let mut bad = bytes.clone();
                for _ in 0..1 + rng.below(3) {
                    let at = 12 + usize::try_from(rng.below(u64::try_from(bad.len().saturating_sub(12)).expect("len"))).expect("index");
                    if let Some(b) = bad.get_mut(at) {
                        *b = u8::try_from(rng.below(256)).expect("byte");
                    }
                }
                for l in limits() {
                    same(&bad, l);
                }
            }
            // Truncated inputs (the container size then no longer fits).
            let cut = bytes.len().saturating_sub(usize::try_from(rng.below(8)).expect("small") + 1);
            same(&bytes[..cut], Limits::default());
        }
    }

    /// Header errors are the same.
    #[test]
    fn header_errors_are_the_same() {
        for bytes in [&b""[..], b"RIFX", b"RIFX\0\0\0\x04Egg", b"RIFX\0\0\0\x04Fnord", b"RIFX\0\0\0\x08Egg!", b"RIFX\0\0\0\x03Egg!", b"RIFF\0\0\0\x04Egg!"] {
            same(bytes, Limits::default());
        }
    }

    /// The typed accessors give the same answers through either tree.
    #[test]
    fn nodes_answer_alike_on_both_trees() {
        let mut rng = Rng(42);
        for _ in 0..100 {
            let mut budget = 200;
            let file = AepFile { chunks: tree(&mut rng, 0, &mut budget), trailer: Cow::Borrowed(&[]) };
            let bytes = write(&file).expect("write");
            let parsed = crate::container::parse(&bytes).expect("parse");
            let index = TreeIndex::parse(&bytes).expect("index");
            let a: Vec<Node<'_, '_>> = parsed.walk().map(|(_, c)| Node::from(c)).collect();
            let b: Vec<Node<'_, '_>> = index.walk().map(|(_, n)| n).collect();
            assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(&b) {
                assert_eq!((x.id(), x.pad(), x.bytes(), x.list_type(), x.is_opaque_list()), (y.id(), y.pad(), y.bytes(), y.list_type(), y.is_opaque_list()));
                assert_eq!(x.children().map(Node::to_chunk).collect::<Vec<_>>(), y.children().map(Node::to_chunk).collect::<Vec<_>>());
                for t in [b"Fold", b"tdgp", b"tdbs", b"Layr", b"btdk"] {
                    assert_eq!(x.find_list(t).map(Node::to_chunk), y.find_list(t).map(Node::to_chunk));
                    assert_eq!(x.child_list(t).map(Node::to_chunk), y.child_list(t).map(Node::to_chunk));
                    assert_eq!(x.is_list(t), y.is_list(t));
                }
                for id in [b"cdat", b"Utf8", b"tdsn"] {
                    assert_eq!(x.child(id).map(Node::to_chunk), y.child(id).map(Node::to_chunk));
                }
                let dx: Vec<(usize, Chunk<'_>)> = x.descendants().map(|(d, n)| (d, n.to_chunk())).collect();
                let dy: Vec<(usize, Chunk<'_>)> = y.descendants().map(|(d, n)| (d, n.to_chunk())).collect();
                assert_eq!(dx, dy);
                assert_eq!(format!("{x:?}"), format!("{y:?}"));
            }
        }
    }

    /// The index never reserves more than the input or the limit allows.
    #[test]
    fn the_index_never_reserves_more_than_the_input_or_the_limit_allows() {
        // 1000 empty headers: the most chunks the bytes can hold.
        let mut data = b"RIFX\0\0\0\0Egg!".to_vec();
        data.extend(std::iter::repeat_n(*b"abcd\0\0\0\0", 1000).flatten());
        let declared = u32::try_from(data.len() - 8).expect("small");
        data[4..8].copy_from_slice(&declared.to_be_bytes());
        let index = TreeIndex::parse(&data).expect("index");
        assert_eq!(index.len(), 1000);
        // The start (1024 slots) is cut to what the input can hold.
        assert!(index.nodes.capacity() <= 1000, "{} slots for at most 1000 headers", index.nodes.capacity());
        // A short tail or a header that runs past the end does not raise the bound.
        let mut big = data.clone();
        big[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(TreeIndex::parse(&big).is_err() && crate::container::parse(&big).is_err());
        // Under a tighter limit the parse fails as the chunk parser does, and the guess never
        // exceeds the limit.
        let tight = Limits { max_depth: 64, max_chunks: 10 };
        assert_eq!(TreeIndex::parse_with_limits(&data, tight).err(), Some(AepError::TooManyChunks));
        let small = Limits { max_depth: 64, max_chunks: 1000 };
        let index = TreeIndex::parse_with_limits(&data, small).expect("exactly at the limit");
        assert_eq!(index.len(), 1000);
        // A limit below what the input holds bounds the reservation: 100 chunks, never more slots.
        let mut wide = b"RIFX\0\0\0\0Egg!".to_vec();
        wide.extend_from_slice(b"LIST");
        wide.extend_from_slice(&u32::try_from(4 + 8 * 99).expect("small").to_be_bytes());
        wide.extend_from_slice(b"Fold");
        wide.extend(std::iter::repeat_n(*b"abcd\0\0\0\0", 99).flatten());
        let declared = u32::try_from(wide.len() - 8).expect("small");
        wide[4..8].copy_from_slice(&declared.to_be_bytes());
        let index = TreeIndex::parse_with_limits(&wide, Limits { max_depth: 64, max_chunks: 100 }).expect("100 chunks");
        assert_eq!(index.len(), 100);
        assert!(index.nodes.capacity() <= 100);
        // `from_file` is sized exactly.
        let file = crate::container::parse(&wide).expect("parse");
        assert_eq!(TreeIndex::from_file(&file).expect("index").nodes.capacity(), 100);
    }

    /// A file of `n` data chunks of `size` bytes each.
    fn flat(n: usize, size: usize) -> Vec<u8> {
        let mut data = b"RIFX\0\0\0\0Egg!".to_vec();
        for _ in 0..n {
            data.extend_from_slice(b"cdat");
            data.extend_from_slice(&u32::try_from(size).expect("small").to_be_bytes());
            data.extend(std::iter::repeat_n(0u8, size));
        }
        let declared = u32::try_from(data.len() - 8).expect("small");
        data[4..8].copy_from_slice(&declared.to_be_bytes());
        data
    }

    /// The index grows with the chunk count not the byte size.
    #[test]
    fn the_index_grows_with_the_chunk_count_not_the_byte_size() {
        // 4 MB in 4 chunks: a few slots, not one per 32 bytes (that would be 131 072 slots, 4 MB).
        let big = flat(4, 1 << 20);
        let index = TreeIndex::parse(&big).expect("index");
        assert_eq!(index.len(), 4);
        assert!(index.nodes.capacity() <= INITIAL_SLOTS, "{} slots for 4 chunks", index.nodes.capacity());
        // Many small chunks: at most twice the count (it doubles).
        let many = flat(10_000, 2);
        let index = TreeIndex::parse(&many).expect("index");
        assert_eq!(index.len(), 10_000);
        assert!(index.nodes.capacity() <= 2 * 10_000, "{} slots for 10 000 chunks", index.nodes.capacity());
        same(&big, Limits::default());
        same(&many, Limits::default());
    }

    /// `LIST Fold` around `inner`, followed by 6 bytes too short for a header, so the list's
    /// sequence fails and the list is kept opaque.
    fn broken_level(inner: &[u8]) -> Vec<u8> {
        let mut body = b"Fold".to_vec();
        body.extend_from_slice(inner);
        body.extend_from_slice(&[0xee; 6]);
        let mut out = b"LIST".to_vec();
        out.extend_from_slice(&u32::try_from(body.len()).expect("small").to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Failed attempts count once and leave nothing behind.
    #[test]
    fn failed_attempts_count_once_and_leave_nothing_behind() {
        let mut wide = b"Fold".to_vec();
        wide.extend(std::iter::repeat_n(*b"abcd\0\0\0\0", 1000).flatten());
        let mut level = b"LIST".to_vec();
        level.extend_from_slice(&u32::try_from(wide.len()).expect("small").to_be_bytes());
        level.extend_from_slice(&wide);
        for _ in 0..50 {
            level = broken_level(&level);
        }
        let mut data = b"RIFX\0\0\0\0Egg!".to_vec();
        data.extend_from_slice(&level);
        let declared = u32::try_from(data.len() - 8).expect("small");
        data[4..8].copy_from_slice(&declared.to_be_bytes());
        let headers = 1 + 50 + 1000;
        let index = TreeIndex::parse_with_limits(&data, Limits { max_depth: 64, max_chunks: headers }).expect("parse");
        assert_eq!(TreeIndex::parse_with_limits(&data, Limits { max_depth: 64, max_chunks: headers - 1 }).err(), Some(AepError::TooManyChunks));
        // Only the outermost list is left, opaque: every failed attempt was rolled back.
        assert_eq!(index.len(), 1);
        let top = index.chunks().next().expect("one top-level chunk");
        assert!(top.is_opaque_list() && top.bytes().map(<[u8]>::len) == Some(level.len() - 12));
        assert_eq!(write(&index.to_file()).expect("write"), data);
        same(&data, Limits::default());
    }

    /// Depth beyond the limit stays opaque without recursion past it.
    #[test]
    fn depth_beyond_the_limit_stays_opaque_without_recursion_past_it() {
        // 5000 nested lists: far deeper than the limit; everything below depth 64 stays bytes.
        let mut level = b"LIST\0\0\0\x04tdgp".to_vec();
        for _ in 0..5000 {
            let mut out = b"LIST".to_vec();
            out.extend_from_slice(&u32::try_from(level.len() + 4).expect("small").to_be_bytes());
            out.extend_from_slice(b"tdgp");
            out.extend_from_slice(&level);
            level = out;
        }
        let mut data = b"RIFX\0\0\0\0Egg!".to_vec();
        data.extend_from_slice(&level);
        let declared = u32::try_from(data.len() - 8).expect("small");
        data[4..8].copy_from_slice(&declared.to_be_bytes());
        let index = TreeIndex::parse(&data).expect("parse");
        assert_eq!(index.len(), 64);
        assert_eq!(index.walk().map(|(d, _)| d).max(), Some(64));
        assert!(index.walk().last().is_some_and(|(_, n)| n.is_opaque_list()));
        same(&data, Limits::default());
        // Converting and indexing a deep owned tree uses no recursion either.
        let file = index.to_file();
        assert_eq!(TreeIndex::from_file(&file).expect("index").len(), 64);
    }
}
