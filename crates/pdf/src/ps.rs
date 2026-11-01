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

pub(crate) struct Interp {
    pub b: Builder,
    st: Vec<V>,
    dicts: Vec<DictRef>,
    gs: GState,
    saved: Vec<GState>,
    budget: usize,
    /// Nested procedure calls (recursion is cut off at [`MAX_CALL_DEPTH`]).
    calls: usize,
    /// Fonts by name: `definefont`d dictionaries.
    font_dir: HashMap<String, DictRef>,
    /// Embedded font programs found in the file (name → (program, CFF)).
    embedded: HashMap<String, (Rc<Vec<u8>>, bool)>,
    /// Fonts built for drawing, by base font and encoding.
    font_cache: HashMap<String, Rc<Font>>,
    page: [f64; 4],
    pub unknown: Vec<String>,
}
