use super::eps::embedded_fonts;
use super::scanner::Src;
use super::text::{encoding_array, encoding_bytes};
use super::value::{DictRef, Flow, GState, V, matrix_val, new_dict};
use super::{Affine, BezPath, Builder, Cap, HashMap, Interp, Join, Rc};

/// The deepest procedure nesting run (deeper recursion stops the program).
const MAX_CALL_DEPTH: usize = 256;

/// The most copies of the graphics state `gsave` and `save` keep at once. Documents nest a
/// handful; deeper nesting is a runaway program.
pub(super) const MAX_SAVE_DEPTH: usize = 1024;

/// The most operands retained on the operand stack.
pub(super) const MAX_STACK: usize = 100_000;

/// The longest name or dictionary key, in bytes, that a program may make or keep at runtime
/// ([`Interp::name_fits`]); a longer one stops it, as `limitcheck` does in a conforming
/// interpreter (PostScript's documented limit is 127 characters, PostScript Language Reference,
/// 3rd ed., Appendix B). Names read from the file are not capped, since inline image data the
/// interpreter cannot step over reaches the scanner as names; running one longer than this is
/// still constant work, because it cannot be a defined name and is not looked up.
pub(super) const MAX_NAME: usize = 255;

/// What a program keeps at runtime, cumulatively: compound payloads, kept text, dictionary
/// entries, key scans, drawable fonts and their embedded programs, dash copies. Source tokens,
/// paths and drawn output (bounded by the operation budget and path limit) and container overhead
/// are outside it.
const MAX_ALLOCATION: usize = 256 * 1024 * 1024;

/// What a dictionary keeps for an entry besides its key's text: the key's `String` and the value's
/// slot. The table's spare buckets and control bytes are container overhead, outside the budget
/// like a vector's spare capacity.
pub(super) const DICT_ENTRY: usize = std::mem::size_of::<(String, V)>();

impl Interp {
    /// Create an interpreter with independent operation and cumulative runtime-payload budgets.
    pub fn new(page: [f64; 4]) -> Interp {
        Interp {
            b: Builder::new(),
            st: vec![],
            dicts: vec![new_dict()],
            gs: GState {
                ctm: Affine::IDENTITY,
                color: [0.0; 3],
                lw: 1.0,
                cap: Cap::Butt,
                join: Join::Miter,
                miter: 10.0,
                dash: None,
                path: BezPath::new(),
                cur: None,
                start: None,
                depth: 1,
                font: None,
            },
            saved: vec![],
            budget: 8_000_000,
            allocation_budget: MAX_ALLOCATION,
            calls: 0,
            font_dir: new_dict(),
            embedded: HashMap::new(),
            font_cache: HashMap::new(),
            font_fast: HashMap::new(),
            page,
            unknown: vec![],
        }
    }

    pub fn run(&mut self, data: &[u8]) {
        self.embedded = embedded_fonts(data);
        let mut src = Src::new(data);
        while let Some(t) = src.token(0) {
            if let Err(Flow::Quit) = self.exec_token(t, &mut src) {
                break;
            }
        }
    }

    /// Stop the program when a name made or kept at runtime (`cvn`, a new dictionary key, a font
    /// name from a string) of `len` bytes would be longer than [`MAX_NAME`]. A name read from the
    /// file is not checked when it is read, pushed or run (the file bounds them, and running one is
    /// bounded in [`Self::exec_name`]); kept as a new key, it is.
    pub(super) fn name_fits(&mut self, len: usize) -> Result<(), Flow> {
        if len <= MAX_NAME {
            return Ok(());
        }
        self.b.skip("PostScript (name length limit)");
        Err(Flow::Quit)
    }

    pub(super) fn pop(&mut self) -> V {
        self.st.pop().unwrap_or(V::Null)
    }

    pub(super) fn popn(&mut self) -> f64 {
        self.pop().num().unwrap_or(0.0)
    }

    /// Retain one operand if it fits; bulk callers must preflight their complete result.
    pub(super) fn push(&mut self, v: V) {
        if self.st.len() < MAX_STACK {
            self.st.push(v);
        }
    }

    /// Refuse a bulk result before any operands are copied when `n` more slots would exceed the cap.
    pub(super) fn stack_room(&mut self, n: usize) -> Result<(), Flow> {
        if self.st.len().saturating_add(n) <= MAX_STACK {
            return Ok(());
        }
        self.b.skip("PostScript (operand stack limit)");
        Err(Flow::Quit)
    }

    /// Charge before allocating: discarded or overwritten objects do not replenish the budget.
    pub(super) fn allocate(&mut self, bytes: usize) -> Result<(), Flow> {
        match self.allocation_budget.checked_sub(bytes) {
            Some(left) => {
                self.allocation_budget = left;
                Ok(())
            }
            None => {
                self.b.skip("PostScript (allocation limit)");
                Err(Flow::Quit)
            }
        }
    }

    /// Charge runtime value slots by their in-memory size, saturating oversized byte counts.
    pub(super) fn allocate_items(&mut self, n: usize) -> Result<(), Flow> {
        self.allocate(n.saturating_mul(std::mem::size_of::<V>()))
    }

    /// Charge then snapshot array/procedure slots shallowly, returning no items for other values.
    pub(super) fn items(&mut self, v: &V) -> Result<Vec<V>, Flow> {
        self.allocate_items(v.item_count())?;
        Ok(v.items().unwrap_or_default())
    }

    /// A copy of the key text `v` names (what `cvs` keeps), its exact length charged before the
    /// copy is made. The text is read in place for a name or a UTF-8 string; a number, a boolean
    /// or a string that isn't UTF-8 is converted first.
    pub(super) fn converted_key<T: for<'a> From<&'a str>>(&mut self, v: &V) -> Result<T, Flow> {
        v.with_key(|t| self.copied_text(t))
    }

    /// [`Self::converted_key`] for text that becomes a name (`cvn`, an operator `load` makes, a
    /// font's name): longer than [`MAX_NAME`] stops the program instead.
    pub(super) fn converted_name<T: for<'a> From<&'a str>>(&mut self, v: &V) -> Result<T, Flow> {
        v.with_key(|t| {
            self.name_fits(t.len())?;
            self.copied_text(t)
        })
    }

    /// A copy of `t`, its length charged first.
    fn copied_text<T: for<'a> From<&'a str>>(&mut self, t: &str) -> Result<T, Flow> {
        self.allocate(t.len())?;
        Ok(T::from(t))
    }

    /// The name `v` stands for: a name's own text, shared at no cost, else a copy of its key text
    /// ([`Self::converted_name`]).
    pub(super) fn name_of(&mut self, v: &V) -> Result<Rc<str>, Flow> {
        match v {
            V::Name(n) | V::Lit(n) | V::Op(n) => Ok(n.clone()),
            other => self.converted_name(other),
        }
    }

    /// Charge a copy of the dictionary key `k` before it is made: the copy `forall` hands out of
    /// every key.
    pub(super) fn charge_key(&mut self, k: &str) -> Result<(), Flow> {
        self.allocate(k.len())
    }

    /// Charge a new dictionary entry under the key `k` before it is made: the key's text and the
    /// entry's slot ([`DICT_ENTRY`]).
    pub(super) fn charge_entry(&mut self, k: &str) -> Result<(), Flow> {
        self.allocate(k.len().saturating_add(DICT_ENTRY))
    }

    /// Charge a key-bearing operand's scan before it is hashed or compared as a dictionary key, a
    /// lookup name or in `eq`: a string at its worst-case lossy UTF-8 length, a name at its own
    /// length ([`V::key_bytes`]). Hashing or comparing reads every byte, so a long one would
    /// otherwise do work that scales with its length for a single operation (a lookup or an
    /// overwrite in a loop).
    pub(super) fn charge_scan(&mut self, v: &V) -> Result<(), Flow> {
        match v {
            V::Str(_) | V::Name(_) | V::Lit(_) | V::Op(_) => self.allocate(v.key_bytes()),
            _ => Ok(()),
        }
    }

    /// Put `v` in `d` under the key `k` names. A key `d` already has keeps its entry, and only its
    /// value is replaced, charging the scan that finds it ([`Self::charge_scan`]). A new key, at
    /// most [`MAX_NAME`] bytes, is an entry the dictionary keeps, which reads the key once as it is
    /// stored and is charged in full ([`Self::charge_entry`]).
    pub(super) fn dict_insert(&mut self, d: &DictRef, k: &V, v: V) -> Result<(), Flow> {
        k.with_key(|t| {
            if d.borrow().contains_key(t) {
                self.charge_scan(k)?;
                if let Some(slot) = d.borrow_mut().get_mut(t) {
                    *slot = v;
                }
                return Ok(());
            }
            self.name_fits(t.len())?;
            self.charge_entry(t)?;
            d.borrow_mut().insert(t.to_owned(), v);
            Ok(())
        })
    }

    /// Put the font dictionary `d` in Font Directory under the key `k` names ([`Self::dict_insert`]).
    pub(super) fn define_font(&mut self, k: &V, d: V) -> Result<(), Flow> {
        let dir = self.font_dir.clone();
        self.dict_insert(&dir, k, d)
    }

    /// The innermost dictionary on the dictionary stack that defines the key `k` names.
    pub(super) fn defining_dict(&self, k: &V) -> Option<DictRef> {
        k.with_key(|t| self.dicts.iter().rev().find(|d| d.borrow().contains_key(t)).cloned())
    }

    /// Check room for, and charge, a copy of the graphics state (`gsave`, `save`): at most
    /// [`MAX_SAVE_DEPTH`] are kept at once, their path elements count as operations, and their
    /// dash arrays are charged by their numbers.
    pub(super) fn charge_gstate_copy(&mut self) -> Result<(), Flow> {
        if self.saved.len() >= MAX_SAVE_DEPTH {
            self.b.skip("PostScript (graphics state depth limit)");
            return Err(Flow::Quit);
        }
        self.spend(self.gs.path.elements().len())?;
        let dash = self.gs.dash.as_ref().map_or(0, |(a, _)| a.len());
        self.allocate(dash.saturating_mul(std::mem::size_of::<f64>()))
    }

    /// Precharge the copy an equality key makes ([`Self::charge_scan`]): a string's worst-case lossy
    /// UTF-8 copy, a name's own length.
    pub(super) fn comparison_key(&mut self, v: &V) -> Result<String, Flow> {
        self.charge_scan(v)?;
        Ok(v.key())
    }

    /// A new matrix array, its six slots charged before it is made.
    pub(super) fn matrix(&mut self, m: Affine) -> Result<V, Flow> {
        self.allocate_items(6)?;
        Ok(matrix_val(m))
    }

    /// Push a new matrix array ([`Self::matrix`]).
    pub(super) fn push_matrix(&mut self, m: Affine) -> Result<(), Flow> {
        let v = self.matrix(m)?;
        self.push(v);
        Ok(())
    }

    /// Charge the slots and fresh name text before constructing or publishing an encoding array.
    pub(super) fn push_encoding(&mut self, name: &str) -> Result<(), Flow> {
        // Keep the legacy choice: ISO Latin-1 and other requested encodings approximate WinAnsi.
        let base = if name == "StandardEncoding" { crate::encoding::Base::Standard } else { crate::encoding::Base::WinAnsi };
        self.allocate(encoding_bytes(base))?;
        self.push(encoding_array(base));
        Ok(())
    }

    pub(super) fn lookup(&self, k: &str) -> Option<V> {
        self.dicts.iter().rev().find_map(|d| d.borrow().get(k).cloned())
    }

    /// A token read from the file: procedures are pushed, names executed.
    fn exec_token(&mut self, t: V, src: &mut Src) -> Result<(), Flow> {
        match t {
            V::Name(n) => self.exec_name(&n, src),
            other => {
                self.push(other);
                Ok(())
            }
        }
    }

    /// Execute a value as an object (procedures run).
    pub(super) fn exec_obj(&mut self, v: V, src: &mut Src) -> Result<(), Flow> {
        match v {
            V::Proc(p) => {
                if self.calls >= MAX_CALL_DEPTH {
                    self.b.skip("PostScript (recursion limit)");
                    return Err(Flow::Quit);
                }
                self.calls += 1;
                let mut r = Ok(());
                for x in p.iter() {
                    match x {
                        V::Name(n) => {
                            r = self.exec_name(n, src);
                            if r.is_err() {
                                break;
                            }
                        }
                        V::Op(op) => {
                            r = self.spend(1).and_then(|()| self.builtin(op, src));
                            if r.is_err() {
                                break;
                            }
                        }
                        // Other items count against the budget as names do.
                        other => {
                            r = self.spend(1);
                            if r.is_err() {
                                break;
                            }
                            self.push(other.clone());
                        }
                    }
                }
                self.calls -= 1;
                r
            }
            // An executable name can run another (`exec` of `/exec cvx`), so names count too.
            V::Name(n) => {
                if self.calls >= MAX_CALL_DEPTH {
                    self.b.skip("PostScript (recursion limit)");
                    return Err(Flow::Quit);
                }
                self.calls += 1;
                let r = self.exec_name(&n, src);
                self.calls -= 1;
                r
            }
            V::Op(op) => {
                self.spend(1)?;
                self.builtin(&op, src)
            }
            other => {
                self.push(other);
                Ok(())
            }
        }
    }

    fn exec_name(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        self.spend(1)?;
        // A name longer than any key a dictionary can hold ([`MAX_NAME`]) is no defined name and no
        // operator, so it is not looked up — hashing it against every dictionary on the stack would
        // scale with its length for one operation. It is noted like any unknown operator below.
        if n.len() <= MAX_NAME
            && let Some(v) = self.lookup(n)
        {
            return match v {
                V::Proc(_) => self.exec_obj(v, src),
                // An operator stored under another name (`/mo /moveto load def`, as Illustrator's
                // EPS prolog does for every path operator) runs that operator. It pushed the
                // operator's name instead, so such files drew nothing (#955).
                V::Op(op) => self.builtin(&op, src),
                // An executable name (`/y /x cvx def`) runs what that name is bound to.
                V::Name(_) => self.exec_obj(v, src),
                other => {
                    self.push(other);
                    Ok(())
                }
            };
        }
        self.builtin(n, src)
    }

    /// Count `n` operations against the budget, and stop the program when fewer are left. An
    /// operator that makes, reads or copies path elements counts each one as an operation.
    pub(super) fn spend(&mut self, n: usize) -> Result<(), Flow> {
        match self.budget.checked_sub(n) {
            Some(left) => {
                self.budget = left;
                Ok(())
            }
            None => {
                self.b.skip("PostScript (operation limit)");
                Err(Flow::Quit)
            }
        }
    }
}
