use super::{Affine, BezPath, Cap, HashMap, Join, Point, Rc, RefCell};

pub(crate) type DictRef = Rc<RefCell<HashMap<String, V>>>;

#[derive(Clone)]
pub(crate) enum V {
    Num(f64),
    Bool(bool),
    /// An executable name.
    Name(Rc<str>),
    /// A literal name (`/name`).
    Lit(Rc<str>),
    /// A built-in operator, as `load` returns it for a name no dictionary defines.
    Op(Rc<str>),
    Str(Rc<RefCell<Vec<u8>>>),
    Proc(Rc<Vec<V>>),
    Array(Rc<RefCell<Vec<V>>>),
    Dict(DictRef),
    Mark,
    Null,
    /// `currentfile`.
    File,
}

impl V {
    pub(super) fn num(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            _ => None,
        }
    }
    /// Run `f` on the text this value names as a key: a name's text and a UTF-8 string's bytes in
    /// place, so looking a key up copies nothing; a number, a boolean or a string that isn't UTF-8
    /// as a short-lived conversion; anything else as the empty key.
    pub(super) fn with_key<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        match self {
            V::Name(n) | V::Lit(n) | V::Op(n) => f(n),
            V::Str(s) => {
                let b = s.borrow();
                match std::str::from_utf8(&b) {
                    Ok(t) => f(t),
                    Err(_) => f(&String::from_utf8_lossy(&b)),
                }
            }
            V::Num(n) => f(&format!("{n}")),
            V::Bool(b) => f(if *b { "true" } else { "false" }),
            _ => f(""),
        }
    }
    /// A copy of the text this value names as a key ([`V::with_key`]).
    pub(super) fn key(&self) -> String {
        self.with_key(str::to_owned)
    }
    /// The most bytes [`V::key`] allocates: a string's worst-case lossy UTF-8 copy (each invalid
    /// byte becomes at most three), a name's length, at most 512 for a number (f64's
    /// non-exponential decimal form stays under it), 5 for a boolean and none for an empty key.
    pub(super) fn key_bytes(&self) -> usize {
        match self {
            V::Str(s) => s.borrow().len().saturating_mul(3),
            V::Name(n) | V::Lit(n) | V::Op(n) => n.len(),
            V::Num(_) => 512,
            V::Bool(_) => 5,
            _ => 0,
        }
    }
    /// Return the array/procedure length without cloning its payload; other values have no items.
    pub(super) fn item_count(&self) -> usize {
        match self {
            V::Array(a) => a.borrow().len(),
            V::Proc(p) => p.len(),
            _ => 0,
        }
    }

    pub(super) fn items(&self) -> Option<Vec<V>> {
        match self {
            V::Array(a) => Some(a.borrow().clone()),
            V::Proc(p) => Some(p.as_ref().clone()),
            _ => None,
        }
    }
}

pub(super) enum Flow {
    Exit,
    Stop,
    Quit,
}

#[derive(Clone)]
pub(super) struct GState {
    pub(crate) ctm: Affine,
    pub(crate) color: [f64; 3],
    pub(crate) lw: f64,
    pub(crate) cap: Cap,
    pub(crate) join: Join,
    pub(crate) miter: f64,
    pub(crate) dash: Option<(Vec<f64>, f64)>,
    pub(crate) path: BezPath,
    pub(crate) cur: Option<Point>,
    pub(crate) start: Option<Point>,
    pub(crate) depth: usize,
    /// The current font dictionary.
    pub(crate) font: Option<DictRef>,
}

pub(super) fn new_dict() -> DictRef {
    Rc::new(RefCell::new(HashMap::new()))
}

pub(super) fn array(v: Vec<V>) -> V {
    V::Array(Rc::new(RefCell::new(v)))
}

/// Read a six-number array or procedure as a matrix. A PostScript matrix is exactly six numbers,
/// so a different length is rejected without scanning the whole thing: the read is O(1), not O(len)
/// for an operand a program controls the size of (`concat`, `setmatrix`, `makefont`, a font's
/// stored matrix read on every text operation).
pub(super) fn matrix_of(v: &V) -> Option<Affine> {
    let read = |items: &[V]| {
        let [a, b, c, d, e, f] = items else { return None };
        Some(Affine::new([a.num()?, b.num()?, c.num()?, d.num()?, e.num()?, f.num()?]))
    };
    match v {
        V::Array(a) => read(&a.borrow()),
        V::Proc(p) => read(p),
        _ => None,
    }
}

pub(super) fn matrix_val(m: Affine) -> V {
    array(m.as_coeffs().iter().map(|c| V::Num(*c)).collect())
}
