//! Encapsulated PostScript: a small PostScript interpreter (PostScript Language Reference, 3rd
//! edition, and the EPSF 3.0 specification) covering what exported vector EPS files use: the
//! operand and dictionary stacks, procedures and `def`, arithmetic, control flow, arrays and
//! dictionaries, the graphics state, path construction (including arcs) and painting, clipping
//! and device colours, and text (`findfont` / `selectfont`, `show` and its variants, `charpath`,
//! `stringwidth`, re-encoded fonts) with embedded Type 1 and CFF programs or the bundled
//! stand-in fonts. Images are skipped (their inline data is stepped over), as is
//! Level 3 smooth shading (`shfill`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use effectcraft_svg::{Affine, BezPath, Cap, FillRule, Join, Paint, Stroke};
use kurbo::{PathEl, Point};

use crate::build::{Builder, MAX_PATH};
use crate::font::Font;
use crate::object::{ascii85, is_white, parse_num};

mod builtin;
mod eps;
mod exec;
mod paths;
mod scanner;
mod text;
mod value;

pub use eps::eps_parts;
use value::{DictRef, GState, V};

/// A drawable font's cache key: its base font name and the glyph names it draws with.
type FontKey = (Rc<str>, Glyphs);

/// The glyph names a drawable font gives codes 0 to 255, shared with the values that hold them:
/// the first 256 entries of its encoding ([`Font::standalone`] reads no more), the one glyph
/// `glyphshow` draws as code 65, or its program's own.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Glyphs {
    Encoding(Vec<Option<Rc<str>>>),
    One(Rc<str>),
    Builtin,
}

/// A name hashed and compared by the identity of its `Rc`, not its text. The fast font-cache key
/// holds the name objects (so their addresses stay valid while cached) but does not hash the glyph
/// names' bytes, which can be long and are read on every text operation.
#[derive(Clone)]
struct NamePtr(Option<Rc<str>>);

impl NamePtr {
    fn addr(&self) -> usize {
        self.0.as_ref().map_or(0, |r| Rc::as_ptr(r) as *const () as usize)
    }
}

impl PartialEq for NamePtr {
    fn eq(&self, other: &Self) -> bool {
        self.addr() == other.addr()
    }
}
impl Eq for NamePtr {}
impl Hash for NamePtr {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.addr().hash(h);
    }
}

/// A fast, pointer-identity font-cache key: the base name, a discriminant (0 built-in, 1 encoding,
/// 2 one glyph), the single glyph, and the encoding's glyph-name objects.
type FastKey = (NamePtr, u8, NamePtr, Box<[NamePtr]>);

pub(crate) struct Interp {
    pub b: Builder,
    st: Vec<V>,
    dicts: Vec<DictRef>,
    gs: GState,
    saved: Vec<GState>,
    budget: usize,
    /// What is left of the 256 MiB runtime allocation budget; charged before each allocation it
    /// covers and never refunded.
    allocation_budget: usize,
    /// Nested procedure calls (recursion is cut off at [`MAX_CALL_DEPTH`]).
    calls: usize,
    /// Fonts by name (Font Directory): `definefont`d and `findfont`-made dictionaries.
    font_dir: DictRef,
    /// Embedded font programs found in the file (name → (program, CFF)): one copy each, taken
    /// before the program runs and bounded by the file's length; each drawable font built from one
    /// is charged the program's length for the parsed copy it keeps.
    embedded: HashMap<String, (Rc<Vec<u8>>, bool)>,
    /// Fonts built for drawing, by base font and the glyph names of their encoding, which the
    /// key shares with the dictionaries that hold them.
    font_cache: HashMap<FontKey, Rc<Font>>,
    /// A pointer-identity accelerator in front of `font_cache`: a font whose base and glyph-name
    /// objects are the same as before is found without hashing their text. Cleared at 1,024
    /// entries; `font_cache` is the truth.
    font_fast: HashMap<FastKey, Rc<Font>>,
    page: [f64; 4],
    pub unknown: Vec<String>,
}

#[cfg(test)]
mod allocation_tests;
