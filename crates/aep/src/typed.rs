//! A small typed view over the chunk tree: project items, compositions, layers, the property
//! tree and keyframe lists. The field decoders for headers, values, keyframes, effects, masks, text,
//! markers and footage are in the sibling modules and extend these types.
//!
//! Only facts that were established from our own one-change-per-file corpus and are checked by
//! tests against it are decoded (see `docs/aep-format.md` for the evidence). Everything else
//! stays reachable through the raw [`Chunk`] tree. Views borrow the file and never fail loudly:
//! a chunk that does not have the expected shape yields `None` or is skipped.

use crate::bytes::{be_f64, be_u16, be_u32, text, until_nul};
use crate::chunk::{AepFile, Chunk};

/// The name a user gave a property or group: the `Utf8` inside its `tdsn` chunk. `None` for the
/// sentinel `-_0_/-` ("no custom name") and for empty names.
pub(crate) fn user_name(chunk: &Chunk<'_>) -> Option<String> {
    let name = chunk.child(b"tdsn")?.children().iter().find(|c| c.is(b"Utf8"))?.bytes().map(text)?;
    (!name.is_empty() && name != "-_0_/-").then_some(name)
}

/// The content of a `Utf8` child as text (not trimmed).
pub(crate) fn utf8_child(chunk: &Chunk<'_>) -> Option<String> {
    chunk.child(b"Utf8").and_then(Chunk::bytes).map(text)
}

/// The project: the top-level `LIST Fold`, which holds the project items.
#[derive(Debug, Clone, Copy)]
pub struct Project<'f, 'a> {
    pub(crate) fold: &'f Chunk<'a>,
    /// The top-level `LIST EfdG` (project-level effect definitions), when there is one.
    pub(crate) effect_defs: Option<&'f Chunk<'a>>,
}

impl<'f, 'a> Project<'f, 'a> {
    /// The project view of a file, or `None` when it has no `LIST Fold`.
    pub fn new(file: &'f AepFile<'a>) -> Option<Self> {
        file.list(b"Fold").map(|fold| Self { fold, effect_defs: file.list(b"EfdG") })
    }

    /// The top-level items (the project panel's root; folders hold their children, see
    /// [`Item::children`]).
    pub fn items(&self) -> impl Iterator<Item = Item<'f, 'a>> + 'f {
        self.fold.children().iter().filter(|c| c.is_list(b"Item")).map(|chunk| Item { chunk })
    }

    /// Every item including those inside folders, depth-first, without recursion.
    pub fn all_items(&self) -> Vec<Item<'f, 'a>> {
        let mut out = Vec::new();
        let mut stack: Vec<Item<'f, 'a>> = self.items().collect();
        stack.reverse();
        while let Some(item) = stack.pop() {
            let mut kids: Vec<Item<'f, 'a>> = item.children().collect();
            kids.reverse();
            out.push(item);
            stack.extend(kids);
        }
        out
    }
}

/// What a project item is (the first 16-bit word of its `idta` chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// `1`
    Folder,
    /// `4`
    Composition,
    /// `7`
    Footage,
    /// Any other value.
    Other(u16),
}

/// A project item (`LIST Item`).
#[derive(Debug, Clone, Copy)]
pub struct Item<'f, 'a> {
    pub(crate) chunk: &'f Chunk<'a>,
}

impl<'f, 'a> Item<'f, 'a> {
    /// The underlying chunk.
    pub fn chunk(&self) -> &'f Chunk<'a> {
        self.chunk
    }

    /// The item kind, from `idta`.
    pub fn kind(&self) -> Option<ItemKind> {
        let idta = self.chunk.child(b"idta")?.bytes()?;
        Some(match be_u16(idta, 0)? {
            1 => ItemKind::Folder,
            4 => ItemKind::Composition,
            7 => ItemKind::Footage,
            other => ItemKind::Other(other),
        })
    }

    /// The item name (the `Utf8` child). `None` when absent or empty: a solid's footage item
    /// stores no name there.
    pub fn name(&self) -> Option<String> {
        utf8_child(self.chunk).filter(|n| !n.is_empty())
    }

    /// The items inside a folder (`LIST Sfdr`).
    pub fn children(&self) -> impl Iterator<Item = Item<'f, 'a>> + 'f {
        self.chunk.child_list(b"Sfdr").into_iter().flat_map(|sfdr| sfdr.children().iter()).filter(|c| c.is_list(b"Item")).map(|chunk| Item { chunk })
    }

    /// The composition view when this item has composition data (`cdta`).
    pub fn comp(&self) -> Option<Comp<'f, 'a>> {
        self.chunk.child(b"cdta").map(|_| Comp { item: self.chunk })
    }
}

/// The time base of a composition: one frame lasts `frame_duration` units and a second has
/// `timescale` units. Keyframe times are counted in these units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeBase {
    pub frame_duration: u32,
    pub timescale: u32,
}

impl TimeBase {
    /// Frames per second (`timescale / frame_duration`), `None` for a zero duration.
    pub fn frame_rate(&self) -> Option<f64> {
        (self.frame_duration != 0).then(|| f64::from(self.timescale) / f64::from(self.frame_duration))
    }

    /// Converts time units to seconds, `None` for a zero timescale.
    pub fn seconds(&self, units: i32) -> Option<f64> {
        (self.timescale != 0).then(|| f64::from(units) / f64::from(self.timescale))
    }
}

/// A composition item (an [`Item`] with `cdta`).
#[derive(Debug, Clone, Copy)]
pub struct Comp<'f, 'a> {
    pub(crate) item: &'f Chunk<'a>,
}

impl<'f, 'a> Comp<'f, 'a> {
    /// The time base from `cdta` (`u32` at byte 4 is the frame duration, `u32` at byte 8 the
    /// timescale).
    pub fn time_base(&self) -> Option<TimeBase> {
        let cdta = self.item.child(b"cdta")?.bytes()?;
        Some(TimeBase { frame_duration: be_u32(cdta, 4)?, timescale: be_u32(cdta, 8)? })
    }

    /// The layers, top layer first (`LIST Layr`; the `DLay`/`SLay`/`CLay`/`SecL` lists next to
    /// them are not layers).
    pub fn layers(&self) -> impl Iterator<Item = Layer<'f, 'a>> + 'f {
        self.item.children().iter().filter(|c| c.is_list(b"Layr")).map(|chunk| Layer { chunk })
    }
}

/// A layer (`LIST Layr`).
#[derive(Debug, Clone, Copy)]
pub struct Layer<'f, 'a> {
    pub(crate) chunk: &'f Chunk<'a>,
}

impl<'f, 'a> Layer<'f, 'a> {
    /// The underlying chunk.
    pub fn chunk(&self) -> &'f Chunk<'a> {
        self.chunk
    }

    /// The layer name when it was set (the `Utf8` child); `None` when empty, in which case the
    /// layer shows its source's name.
    pub fn name(&self) -> Option<String> {
        utf8_child(self.chunk).filter(|n| !n.is_empty())
    }

    /// The 3D layer switch: bit `0x04` of byte 38 of `ldta`.
    pub fn is_three_d(&self) -> Option<bool> {
        let ldta = self.chunk.child(b"ldta")?.bytes()?;
        Some(ldta.get(38)? & 0x04 != 0)
    }

    /// The root of the layer's property tree.
    pub fn properties(&self) -> Option<PropGroup<'f, 'a>> {
        self.chunk.child_list(b"tdgp").map(|chunk| PropGroup { chunk })
    }
}

/// A node of the property tree.
#[derive(Debug, Clone, Copy)]
pub enum PropNode<'f, 'a> {
    /// A property group (`LIST tdgp`).
    Group(PropGroup<'f, 'a>),
    /// A property (`LIST tdbs`).
    Property(Property<'f, 'a>),
    /// Any other list that follows a match name (observed: `otst`, the effect instance `sspc`).
    Other(&'f Chunk<'a>),
}

/// A match name with the node it names.
#[derive(Debug, Clone)]
pub struct PropEntry<'f, 'a> {
    pub match_name: String,
    pub node: PropNode<'f, 'a>,
}

/// A property group (`LIST tdgp`): after its `tdsb` and `tdsn` chunks come pairs of a `tdmn`
/// (the 40-byte NUL-padded ASCII match name) and the list it names; a `tdmn` named
/// `ADBE Group End` closes the group and names nothing.
#[derive(Debug, Clone, Copy)]
pub struct PropGroup<'f, 'a> {
    pub(crate) chunk: &'f Chunk<'a>,
}

impl<'f, 'a> PropGroup<'f, 'a> {
    /// A group view of a `LIST tdgp` chunk.
    pub fn from_chunk(chunk: &'f Chunk<'a>) -> Option<Self> {
        chunk.is_list(b"tdgp").then_some(Self { chunk })
    }

    /// The underlying chunk.
    pub fn chunk(&self) -> &'f Chunk<'a> {
        self.chunk
    }

    /// The name the user gave the group (e.g. `Mask 1`, `Group 1`); `None` when it has none.
    pub fn user_name(&self) -> Option<String> {
        user_name(self.chunk)
    }

    /// The group's flags (`tdsb`, a `u32`).
    pub fn flags(&self) -> Option<u32> {
        be_u32(self.chunk.child(b"tdsb")?.bytes()?, 0)
    }

    /// The entries in file order.
    pub fn entries(&self) -> Vec<PropEntry<'f, 'a>> {
        let mut out = Vec::new();
        let mut pending: Option<String> = None;
        for c in self.chunk.children() {
            if c.is(b"tdmn") {
                pending = c.bytes().map(match_name);
            } else if c.id == crate::chunk::LIST
                && let Some(name) = pending.take()
            {
                let node = if c.is_list(b"tdgp") {
                    PropNode::Group(PropGroup { chunk: c })
                } else if c.is_list(b"tdbs") {
                    PropNode::Property(Property { chunk: c })
                } else {
                    PropNode::Other(c)
                };
                out.push(PropEntry { match_name: name, node });
            }
        }
        out
    }

    /// The match names of the entries, in file order.
    pub fn names(&self) -> Vec<String> {
        self.entries().into_iter().map(|e| e.match_name).collect()
    }

    /// The first entry with this match name.
    pub fn get(&self, name: &str) -> Option<PropNode<'f, 'a>> {
        self.entries().into_iter().find(|e| e.match_name == name).map(|e| e.node)
    }

    /// Follows a path of match names through nested groups.
    pub fn find(&self, path: &[&str]) -> Option<PropNode<'f, 'a>> {
        let (first, rest) = path.split_first()?;
        let node = self.get(first)?;
        if rest.is_empty() {
            return Some(node);
        }
        match node {
            PropNode::Group(g) => g.find(rest),
            PropNode::Property(_) | PropNode::Other(_) => None,
        }
    }
}

/// The match name in a `tdmn` body: ASCII up to the first NUL.
fn match_name(body: &[u8]) -> String {
    text(until_nul(body))
}

/// A property (`LIST tdbs`): `tdsb`, `tdsn`, the 124-byte header `tdb4`, then either the static
/// value `cdat` or a keyframe `LIST list`.
#[derive(Debug, Clone, Copy)]
pub struct Property<'f, 'a> {
    pub(crate) chunk: &'f Chunk<'a>,
}

impl<'f, 'a> Property<'f, 'a> {
    /// A property view of a `LIST tdbs` chunk.
    pub fn from_chunk(chunk: &'f Chunk<'a>) -> Option<Self> {
        chunk.is_list(b"tdbs").then_some(Self { chunk })
    }

    /// The underlying chunk.
    pub fn chunk(&self) -> &'f Chunk<'a> {
        self.chunk
    }

    /// The name the user gave the property; `None` when it has none.
    pub fn user_name(&self) -> Option<String> {
        user_name(self.chunk)
    }

    /// The raw 124-byte `tdb4` header.
    pub fn header(&self) -> Option<&'f [u8]> {
        self.chunk.child(b"tdb4")?.bytes()
    }

    /// The number of value components: byte 3 of `tdb4`.
    pub fn dims(&self) -> Option<u8> {
        self.header()?.get(3).copied()
    }

    /// True when the property has keyframes, i.e. its value is a `LIST list` instead of `cdat`.
    /// `tdb4` byte 68 is 1 in exactly these properties.
    pub fn is_animated(&self) -> bool {
        self.chunk.child_list(b"list").is_some()
    }

    /// The static value: the first `dims` big-endian `f64` of `cdat`. `None` for animated
    /// properties and for values that are not `f64` arrays.
    pub fn static_value(&self) -> Option<Vec<f64>> {
        let dims = usize::from(self.dims()?);
        let cdat = self.chunk.child(b"cdat")?.bytes()?;
        (0..dims).map(|i| be_f64(cdat, i.checked_mul(8)?)).collect()
    }

    /// The keyframe list of an animated property, with the record layout the property header
    /// selects (`tdb4` byte 3 = components, byte 7 non-zero = spatial template, unless the record
    /// size tells).
    pub fn keys(&self) -> Option<KeyList<'f>> {
        let list = KeyList::from_chunk(self.chunk.child_list(b"list")?)?;
        let header = self.header();
        // The record size decides where the two templates differ in size: a shape colour's keys
        // are spatial (152 bytes for 4 components) although its `tdb4` byte 7 is 0 (854). For 3
        // components both templates are 128 bytes; byte 7 decides.
        let size = list.header.record_size;
        let layout = header.and_then(|h| {
            let components = *h.get(3)?;
            let n = u32::from(components);
            let (spatial_size, plain_size) = (56 + 24 * n, 8 + 40 * n);
            let spatial = if size == spatial_size && size != plain_size {
                true
            } else if size == plain_size && size != spatial_size {
                false
            } else {
                *h.get(7)? != 0
            };
            Some(KeyLayout { components, spatial })
        });
        Some(KeyList { layout, ..list })
    }
}

/// The header `lhd3` of a `LIST list`: 13 big-endian `u32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListHeader {
    /// Word 0, `0x00d00bee` in every list.
    pub magic: u32,
    /// Word 2: the number of records.
    pub count: u32,
    /// Word 4: the size of one record in bytes.
    pub record_size: u32,
    /// All 13 words.
    pub words: [u32; 13],
}

impl ListHeader {
    /// Parses a 52-byte `lhd3` body.
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() != 52 {
            return None;
        }
        let mut words = [0u32; 13];
        for (i, w) in words.iter_mut().enumerate() {
            *w = be_u32(body, i * 4)?;
        }
        Some(Self { magic: words[0], count: words[2], record_size: words[4], words })
    }
}

/// A `LIST list`: `lhd3` and `ldat`, where `len(ldat) == count * record_size`.
#[derive(Debug, Clone, Copy)]
pub struct KeyList<'f> {
    pub header: ListHeader,
    ldat: &'f [u8],
    layout: Option<KeyLayout>,
}

/// What the property header says about its keyframe records: how many value components each
/// record holds and which of the two record templates applies (see [`KeyframeKind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyLayout {
    /// `tdb4` byte 3.
    pub components: u8,
    /// `tdb4` byte 7 is non-zero: the spatial template (Position, colours) instead of the plain one.
    pub spatial: bool,
}

impl<'f> KeyList<'f> {
    /// A view of a `LIST list` chunk. `None` when `lhd3` is not 52 bytes or `ldat` is not exactly
    /// `count * record_size` bytes (a list without `ldat` must have no records).
    pub fn from_chunk<'a>(chunk: &'f Chunk<'a>) -> Option<Self> {
        if !chunk.is_list(b"list") {
            return None;
        }
        let header = ListHeader::parse(chunk.child(b"lhd3")?.bytes()?)?;
        let ldat: &[u8] = chunk.child(b"ldat").and_then(Chunk::bytes).unwrap_or(&[]);
        let expected = usize::try_from(header.count).ok()?.checked_mul(usize::try_from(header.record_size).ok()?)?;
        (ldat.len() == expected).then_some(Self { header, ldat, layout: None })
    }

    /// The number of records.
    pub fn len(&self) -> usize {
        usize::try_from(self.header.count).unwrap_or(0)
    }

    /// True when there are no records.
    pub fn is_empty(&self) -> bool {
        self.ldat.is_empty()
    }

    /// The records as byte slices.
    pub fn records(&self) -> impl Iterator<Item = &'f [u8]> {
        let size = usize::try_from(self.header.record_size).unwrap_or(0);
        self.ldat.chunks_exact(size.max(1))
    }

    /// The record layout from the property header, `None` for a list that does not belong to a
    /// property (the point list of a path).
    pub fn layout(&self) -> Option<KeyLayout> {
        self.layout
    }

    /// The records of a keyframe list as [`Keyframe`]s.
    pub fn keyframes(&self) -> impl Iterator<Item = Keyframe<'f>> {
        let layout = self.layout;
        self.records().map(move |record| Keyframe { record, layout })
    }
}

/// One keyframe record: the time is the `i32` at byte 0, in the composition's time units
/// ([`TimeBase`]).
#[derive(Debug, Clone, Copy)]
pub struct Keyframe<'f> {
    pub(crate) record: &'f [u8],
    pub(crate) layout: Option<KeyLayout>,
}

impl<'f> Keyframe<'f> {
    /// The whole record.
    pub fn raw(&self) -> &'f [u8] {
        self.record
    }

    /// The keyframe time in time units (layer time).
    pub fn time_units(&self) -> Option<i32> {
        be_u32(self.record, 0).map(|u| i32::from_be_bytes(u.to_be_bytes()))
    }
}
