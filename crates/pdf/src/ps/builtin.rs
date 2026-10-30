use super::eps::find_from;
use super::scanner::Src;
use super::text::encoding_array;
use super::value::{Flow, V, array, matrix_of, matrix_val, new_dict};
use super::{Affine, BezPath, Cap, FillRule, Interp, Join, PathEl, Point, Rc, RefCell, parse_num};

impl Interp {
    pub(super) fn builtin(&mut self, n: &str, src: &mut Src) -> Result<(), Flow> {
        // Every handler leaves the stacks untouched when the operator is outside its family.
        if self.try_stack_op(n)
            || self.try_arithmetic_op(n)
            || self.try_logic_op(n)
            || self.try_control_op(n, src)?
            || self.try_dictionary_op(n)
            || self.try_collection_op(n)
            || self.try_environment_op(n, src)?
            || self.try_graphics_op(n)?
            || self.try_coordinate_op(n)
            || self.try_path_op(n)?
            || self.try_paint_op(n)?
        {
            return Ok(());
        }
        match n {
            // Text (see `text_op`).
            "scalefont" | "makefont" | "selectfont" | "setfont" | "show" | "charpath" | "glyphshow" | "rootfont" | "currentfont" | "setcachedevice"
            | "setcharwidth" | "ashow" | "widthshow" | "awidthshow" | "xshow" | "yshow" | "xyshow" | "kshow" | "cshow" | "stringwidth" | "definefont"
            | "StandardEncoding" | "ISOLatin1Encoding" | "eexec" | "StartData" | "undefinefont" => self.text_op(n, src)?,
            // Images: operands popped, inline data skipped.
            "image" | "imagemask" | "colorimage" => {
                let mut reads_file = false;
                let check = |v: &V, rf: &mut bool| {
                    if matches!(v, V::File) || matches!(v, V::Proc(p) if p.iter().any(|x| matches!(x, V::Name(n) if &**n == "currentfile"))) {
                        *rf = true;
                    }
                };
                if n == "colorimage" {
                    let ncomp = self.popn().max(1.0) as usize;
                    let multi = matches!(self.pop(), V::Bool(true));
                    for _ in 0..if multi { ncomp.min(4) } else { 1 } {
                        let v = self.pop();
                        check(&v, &mut reads_file);
                    }
                    for _ in 0..4 {
                        self.pop();
                    }
                } else {
                    match self.pop() {
                        V::Dict(d) => {
                            if let Some(ds) = d.borrow().get("DataSource") {
                                check(ds, &mut reads_file);
                            }
                        }
                        top => {
                            check(&top, &mut reads_file);
                            for _ in 0..4 {
                                self.pop();
                            }
                        }
                    }
                }
                if reads_file {
                    src.skip_image_data();
                }
                self.b.skip("image");
            }
            _ => {
                if self.unknown.len() < 32 && !self.unknown.iter().any(|u| u == n) {
                    self.unknown.push(n.to_string());
                }
            }
        }
        Ok(())
    }

    fn repeat_proc(&mut self, src: &mut Src) -> Result<(), Flow> {
        let p = self.pop();
        let k = self.popn().clamp(0.0, 1e6) as usize;
        for _ in 0..k {
            self.spend(1)?;
            match self.exec_obj(p.clone(), src) {
                Err(Flow::Exit) => break,
                r => r?,
            }
        }

        Ok(())
    }

    fn for_proc(&mut self, src: &mut Src) -> Result<(), Flow> {
        let p = self.pop();
        let lim = self.popn();
        let inc = self.popn();
        let mut i = self.popn();
        let mut guard = 0;
        while inc != 0.0 && ((inc > 0.0 && i <= lim) || (inc < 0.0 && i >= lim)) && guard < 1_000_000 {
            self.spend(1)?;
            self.push(V::Num(i));
            match self.exec_obj(p.clone(), src) {
                Err(Flow::Exit) => break,
                r => r?,
            }
            i += inc;
            guard += 1;
        }

        Ok(())
    }

    fn loop_proc(&mut self, src: &mut Src) -> Result<(), Flow> {
        let p = self.pop();
        for _ in 0..1_000_000 {
            self.spend(1)?;
            match self.exec_obj(p.clone(), src) {
                Err(Flow::Exit) => break,
                r => r?,
            }
        }

        Ok(())
    }

    fn forall_proc(&mut self, src: &mut Src) -> Result<(), Flow> {
        let p = self.pop();
        match self.pop() {
            V::Dict(d) => {
                let items: Vec<(String, V)> = d.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                for (k, v) in items {
                    self.spend(1)?;
                    self.push(V::Lit(k.into()));
                    self.push(v);
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                }
            }
            V::Str(s) => {
                let bytes = s.borrow().clone();
                for c in bytes {
                    self.spend(1)?;
                    self.push(V::Num(c as f64));
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                }
            }
            a => {
                for v in a.items().unwrap_or_default() {
                    self.spend(1)?;
                    self.push(v);
                    match self.exec_obj(p.clone(), src) {
                        Err(Flow::Exit) => break,
                        r => r?,
                    }
                }
            }
        }

        Ok(())
    }

    fn try_stack_op(&mut self, n: &str) -> bool {
        match n {
            // Stack.
            "pop" => {
                self.pop();
            }
            "exch" => {
                let b = self.pop();
                let a = self.pop();
                self.push(b);
                self.push(a);
            }
            "dup" => {
                let a = self.st.last().cloned().unwrap_or(V::Null);
                self.push(a);
            }
            "copy" => {
                match self.pop() {
                    V::Num(k) => {
                        let k = (k.max(0.0) as usize).min(self.st.len());
                        let tail: Vec<V> = self.st[self.st.len() - k..].to_vec();
                        self.st.extend(tail);
                    }
                    V::Array(dst) => {
                        let s = self.pop().items().unwrap_or_default();
                        let mut d = dst.borrow_mut();
                        for (i, v) in s.into_iter().enumerate() {
                            if i < d.len() {
                                d[i] = v;
                            }
                        }
                        drop(d);
                        self.push(V::Array(dst));
                    }
                    other => self.push(other),
                };
            }
            "index" => {
                let k = self.popn().max(0.0) as usize;
                let v = if k < self.st.len() { self.st[self.st.len() - 1 - k].clone() } else { V::Null };
                self.push(v);
            }
            "roll" => {
                let j = self.popn() as i64;
                let k = (self.popn().max(0.0) as usize).min(self.st.len());
                if k > 0 {
                    let s = self.st.len() - k;
                    let r = j.rem_euclid(k as i64) as usize;
                    self.st[s..].rotate_right(r);
                }
            }
            "clear" => self.st.clear(),
            "count" => self.push(V::Num(self.st.len() as f64)),
            "mark" | "[" | "<<" => self.push(V::Mark),
            "cleartomark" => {
                while let Some(v) = self.st.pop() {
                    if matches!(v, V::Mark) {
                        break;
                    }
                }
            }
            "counttomark" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                self.push(V::Num(k as f64));
            }
            "]" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                let items = self.st.split_off(self.st.len() - k);
                self.st.pop();
                self.push(array(items));
            }
            ">>" => {
                let k = self.st.iter().rev().position(|v| matches!(v, V::Mark)).unwrap_or(self.st.len());
                let items = self.st.split_off(self.st.len() - k);
                self.st.pop();
                let d = new_dict();
                for kv in items.as_chunks::<2>().0 {
                    d.borrow_mut().insert(kv[0].key(), kv[1].clone());
                }
                self.push(V::Dict(d));
            }
            _ => return false,
        }
        true
    }

    fn try_arithmetic_op(&mut self, n: &str) -> bool {
        match n {
            // Arithmetic.
            "add" | "sub" | "mul" | "div" | "idiv" | "mod" | "atan" | "exp" | "max" | "min" => {
                let b = self.popn();
                let a = self.popn();
                let v = match n {
                    "add" => a + b,
                    "sub" => a - b,
                    "mul" => a * b,
                    "div" => {
                        if b == 0.0 {
                            0.0
                        } else {
                            a / b
                        }
                    }
                    // `checked_*` give 0 both for a zero divisor and for the one overflowing
                    // case, `i64::MIN / -1` (`/` and `%` panic on it in every build).
                    "idiv" => (a as i64).checked_div(b as i64).unwrap_or(0) as f64,
                    "mod" => (a as i64).checked_rem(b as i64).unwrap_or(0) as f64,
                    "atan" => a.atan2(b).to_degrees().rem_euclid(360.0),
                    "exp" => a.powf(b),
                    "max" => a.max(b),
                    _ => a.min(b),
                };
                self.push(V::Num(if v.is_finite() { v } else { 0.0 }));
            }
            "neg" | "abs" | "sqrt" | "sin" | "cos" | "ln" | "log" | "cvi" | "cvr" | "round" | "floor" | "ceiling" | "truncate" => {
                let a = match self.pop() {
                    V::Str(s) => parse_num(&s.borrow()).unwrap_or(0.0),
                    v => v.num().unwrap_or(0.0),
                };
                let v = match n {
                    "neg" => -a,
                    "abs" => a.abs(),
                    "sqrt" => a.max(0.0).sqrt(),
                    "sin" => a.to_radians().sin(),
                    "cos" => a.to_radians().cos(),
                    "ln" => a.ln(),
                    "log" => a.log10(),
                    "cvi" | "truncate" => a.trunc(),
                    "round" => (a + 0.5).floor(),
                    "floor" => a.floor(),
                    "ceiling" => a.ceil(),
                    _ => a,
                };
                self.push(V::Num(if v.is_finite() { v } else { 0.0 }));
            }
            _ => return false,
        }
        true
    }

    fn try_logic_op(&mut self, n: &str) -> bool {
        match n {
            // Comparison and logic.
            "eq" | "ne" => {
                let b = self.pop();
                let a = self.pop();
                let eq = match (&a, &b) {
                    (V::Num(x), V::Num(y)) => x == y,
                    (V::Bool(x), V::Bool(y)) => x == y,
                    (V::Null, V::Null) => true,
                    (V::Array(x), V::Array(y)) => Rc::ptr_eq(x, y),
                    (V::Dict(x), V::Dict(y)) => Rc::ptr_eq(x, y),
                    (x, y) => !x.key().is_empty() && x.key() == y.key(),
                };
                self.push(V::Bool(eq == (n == "eq")));
            }
            "gt" | "ge" | "lt" | "le" => {
                let b = self.popn();
                let a = self.popn();
                self.push(V::Bool(match n {
                    "gt" => a > b,
                    "ge" => a >= b,
                    "lt" => a < b,
                    _ => a <= b,
                }));
            }
            "not" => match self.pop() {
                V::Bool(b) => self.push(V::Bool(!b)),
                v => self.push(V::Num(!(v.num().unwrap_or(0.0) as i64) as f64)),
            },
            "and" | "or" | "xor" => {
                let b = self.pop();
                let a = self.pop();
                match (a, b) {
                    (V::Bool(x), V::Bool(y)) => self.push(V::Bool(match n {
                        "and" => x && y,
                        "or" => x || y,
                        _ => x ^ y,
                    })),
                    (x, y) => {
                        let (x, y) = (x.num().unwrap_or(0.0) as i64, y.num().unwrap_or(0.0) as i64);
                        self.push(V::Num(match n {
                            "and" => x & y,
                            "or" => x | y,
                            _ => x ^ y,
                        } as f64));
                    }
                }
            }
            "true" => self.push(V::Bool(true)),
            "false" => self.push(V::Bool(false)),
            "null" => self.push(V::Null),
            _ => return false,
        }
        true
    }

    fn try_control_op(&mut self, n: &str, src: &mut Src) -> Result<bool, Flow> {
        match n {
            // Control.
            "exec" => {
                let v = self.pop();
                self.exec_obj(v, src)?;
            }
            "if" => {
                let p = self.pop();
                if let V::Bool(true) = self.pop() {
                    self.exec_obj(p, src)?;
                }
            }
            "ifelse" => {
                let f = self.pop();
                let t = self.pop();
                let c = matches!(self.pop(), V::Bool(true));
                self.exec_obj(if c { t } else { f }, src)?;
            }
            // Each pass of a loop counts against the budget.
            "repeat" => self.repeat_proc(src)?,
            "for" => self.for_proc(src)?,
            "loop" => self.loop_proc(src)?,
            "forall" => self.forall_proc(src)?,
            "exit" => return Err(Flow::Exit),
            "stop" => return Err(Flow::Stop),
            "stopped" => {
                let p = self.pop();
                let stopped = match self.exec_obj(p, src) {
                    Err(Flow::Quit) => return Err(Flow::Quit),
                    Err(_) => true,
                    Ok(()) => false,
                };
                self.push(V::Bool(stopped));
            }
            "quit" => return Err(Flow::Quit),
            "showpage" | "copypage" => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn try_dictionary_op(&mut self, n: &str) -> bool {
        match n {
            // Dictionaries.
            "def" => {
                let v = self.pop();
                let k = self.pop().key();
                if let Some(d) = self.dicts.last() {
                    d.borrow_mut().insert(k, v);
                }
            }
            "store" => {
                let v = self.pop();
                let k = self.pop().key();
                let d = self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).or(self.dicts.last()).cloned();
                if let Some(d) = d {
                    d.borrow_mut().insert(k, v);
                }
            }
            "load" => {
                let k = self.pop().key();
                let v = self.lookup(&k).unwrap_or(V::Op(k.into()));
                self.push(v);
            }
            "undef" => {
                let k = self.pop().key();
                if let V::Dict(d) = self.pop() {
                    d.borrow_mut().remove(&k);
                }
            }
            "dict" => {
                self.pop();
                self.push(V::Dict(new_dict()));
            }
            "begin" => {
                if let V::Dict(d) = self.pop()
                    && self.dicts.len() < 64
                {
                    self.dicts.push(d);
                }
            }
            "end" => {
                if self.dicts.len() > 1 {
                    self.dicts.pop();
                }
            }
            "currentdict" => {
                let d = self.dicts.last().cloned().unwrap_or_else(new_dict);
                self.push(V::Dict(d));
            }
            "userdict" | "globaldict" | "systemdict" | "statusdict" | "errordict" | "$error" => {
                let d = self.dicts[0].clone();
                self.push(V::Dict(d));
            }
            "where" => {
                let k = self.pop().key();
                match self.dicts.iter().rev().find(|d| d.borrow().contains_key(&k)).cloned() {
                    Some(d) => {
                        self.push(V::Dict(d));
                        self.push(V::Bool(true));
                    }
                    None => self.push(V::Bool(false)),
                }
            }
            "known" => {
                let k = self.pop().key();
                let v = match self.pop() {
                    V::Dict(d) => d.borrow().contains_key(&k),
                    _ => false,
                };
                self.push(V::Bool(v));
            }
            "countdictstack" => self.push(V::Num(self.dicts.len() as f64)),
            "cleardictstack" => self.dicts.truncate(1),
            "findresource" | "findfont" | "findencoding" => {
                let category = if n == "findresource" { self.pop().key() } else { String::new() };
                let key = self.pop().key();
                match (n, category.as_str()) {
                    ("findfont", _) | (_, "Font") => {
                        let f = self.find_font(&key);
                        self.push(V::Dict(f));
                    }
                    ("findencoding", _) | (_, "Encoding") => self.push(encoding_array(&key)),
                    _ => self.push(V::Dict(new_dict())),
                }
            }
            "defineresource" => {
                let category = self.pop().key();
                let v = self.pop();
                let key = self.pop().key();
                if let (V::Dict(d), "Font") = (&v, category.as_str()) {
                    self.font_dir.insert(key, d.clone());
                }
                self.push(v);
            }
            "resourcestatus" => {
                self.pop();
                self.pop();
                self.push(V::Bool(false));
            }
            _ => return false,
        }
        true
    }

    fn try_collection_op(&mut self, n: &str) -> bool {
        match n {
            // Arrays and strings.
            "array" => {
                let k = self.popn().clamp(0.0, 1e6) as usize;
                self.push(array(vec![V::Null; k]));
            }
            "string" => {
                let k = self.popn().clamp(0.0, 1e7) as usize;
                self.push(V::Str(Rc::new(RefCell::new(vec![0; k]))));
            }
            "length" => {
                let l = match self.pop() {
                    V::Str(s) => s.borrow().len(),
                    V::Dict(d) => d.borrow().len(),
                    V::Name(s) | V::Lit(s) => s.len(),
                    v => v.items().map_or(0, |i| i.len()),
                };
                self.push(V::Num(l as f64));
            }
            "get" => {
                let k = self.pop();
                let v = match self.pop() {
                    V::Dict(d) => d.borrow().get(&k.key()).cloned().unwrap_or(V::Null),
                    V::Str(s) => V::Num(s.borrow().get(k.num().unwrap_or(0.0) as usize).copied().unwrap_or(0) as f64),
                    a => a.items().and_then(|i| i.get(k.num().unwrap_or(0.0) as usize).cloned()).unwrap_or(V::Null),
                };
                self.push(v);
            }
            "put" => {
                let v = self.pop();
                let k = self.pop();
                match self.pop() {
                    V::Dict(d) => {
                        d.borrow_mut().insert(k.key(), v);
                    }
                    V::Array(a) => {
                        let i = k.num().unwrap_or(0.0) as usize;
                        if let Some(slot) = a.borrow_mut().get_mut(i) {
                            *slot = v;
                        }
                    }
                    V::Str(s) => {
                        let i = k.num().unwrap_or(0.0) as usize;
                        if let Some(slot) = s.borrow_mut().get_mut(i) {
                            *slot = v.num().unwrap_or(0.0) as u8;
                        }
                    }
                    _ => {}
                }
            }
            "getinterval" => {
                let c = self.popn().max(0.0) as usize;
                let i = self.popn().max(0.0) as usize;
                match self.pop() {
                    V::Str(s) => {
                        let b = s.borrow();
                        let part = b.get(i.min(b.len())..(i + c).min(b.len())).unwrap_or(&[]).to_vec();
                        drop(b);
                        self.push(V::Str(Rc::new(RefCell::new(part))));
                    }
                    a => {
                        let items = a.items().unwrap_or_default();
                        let part = items.get(i.min(items.len())..(i + c).min(items.len())).unwrap_or(&[]).to_vec();
                        self.push(array(part));
                    }
                }
            }
            "putinterval" => {
                self.pop();
                self.pop();
                self.pop();
            }
            "aload" => {
                let a = self.pop();
                for v in a.items().unwrap_or_default() {
                    self.push(v);
                }
                self.push(a);
            }
            "astore" => {
                let a = self.pop();
                if let V::Array(arr) = &a {
                    let k = arr.borrow().len().min(self.st.len());
                    let items = self.st.split_off(self.st.len() - k);
                    *arr.borrow_mut() = items;
                }
                self.push(a);
            }
            "cvx" => match self.pop() {
                V::Array(a) => self.push(V::Proc(Rc::new(a.borrow().clone()))),
                V::Lit(s) => self.push(V::Name(s)),
                V::Str(s) => {
                    // Executable string: tokenize it as a procedure.
                    let bytes = s.borrow().clone();
                    let mut sub = Src { d: &bytes, pos: 0 };
                    let mut body = vec![];
                    while let Some(t) = sub.token(0) {
                        body.push(t);
                    }
                    self.push(V::Proc(Rc::new(body)));
                }
                v => self.push(v),
            },
            "cvlit" => match self.pop() {
                V::Proc(p) => self.push(array(p.as_ref().clone())),
                V::Name(s) => self.push(V::Lit(s)),
                v => self.push(v),
            },
            "cvn" => {
                let k = self.pop().key();
                self.push(V::Lit(k.into()));
            }
            "cvs" => {
                self.pop();
                let k = self.pop().key();
                self.push(V::Str(Rc::new(RefCell::new(k.into_bytes()))));
            }
            "xcheck" => {
                let v = self.pop();
                self.push(V::Bool(matches!(v, V::Proc(_) | V::Name(_) | V::Op(_))));
            }
            "type" => {
                let t = match self.pop() {
                    V::Num(_) => "realtype",
                    V::Bool(_) => "booleantype",
                    V::Name(_) | V::Lit(_) => "nametype",
                    V::Op(_) => "operatortype",
                    V::Str(_) => "stringtype",
                    V::Proc(_) | V::Array(_) => "arraytype",
                    V::Dict(_) => "dicttype",
                    V::Mark => "marktype",
                    V::Null => "nulltype",
                    V::File => "filetype",
                };
                self.push(V::Name(t.into()));
            }
            "bind" | "readonly" | "executeonly" | "noaccess" => {}
            _ => return false,
        }
        true
    }

    fn try_environment_op(&mut self, n: &str, src: &mut Src) -> Result<bool, Flow> {
        match n {
            "languagelevel" => self.push(V::Num(3.0)),
            "version" | "product" => self.push(V::Str(Rc::new(RefCell::new(b"3010".to_vec())))),
            "save" => {
                self.spend(self.gs.path.elements().len())?;
                self.saved.push(self.gs.clone());
                self.gs.depth = self.b.depth();
                self.push(V::Null);
            }
            "restore" => {
                self.pop();
                self.grestore();
            }
            "currentfile" => self.push(V::File),
            "readstring" | "readhexstring" | "readline" => {
                self.pop();
                self.pop();
                self.push(V::Str(Rc::new(RefCell::new(vec![]))));
                self.push(V::Bool(false));
            }
            "flushfile" | "closefile" | "print" | "==" | "=" | "pstack" | "setpagedevice" | "setglobal" | "setpacking" | "setshared" | "setobjectformat"
            | "setsmoothness" => {
                if !matches!(n, "pstack") {
                    self.pop();
                }
            }
            "currentglobal" | "currentpacking" | "currentshared" => self.push(V::Bool(false)),
            // Operators that return values must push them, or the rest of the program reads a
            // shifted stack: Illustrator's EPS prolog queries the device this way and then drew
            // nothing (#955). Values are a plain device's defaults (PostScript Language
            // Reference, 3rd ed., chapter 8).
            "currentflat" => self.push(V::Num(1.0)),
            "currentscreen" => {
                self.push(V::Num(60.0));
                self.push(V::Num(45.0));
                self.push(V::Proc(Rc::new(vec![])));
            }
            "currentblackgeneration" | "currentundercolorremoval" | "currenttransfer" => self.push(V::Proc(Rc::new(vec![]))),
            "currentcolortransfer" => {
                for _ in 0..4 {
                    self.push(V::Proc(Rc::new(vec![])));
                }
            }
            "currentpagedevice" | "currenthalftone" | "currentsystemparams" | "currentuserparams" | "currentdevparams" => {
                if n == "currentdevparams" {
                    self.pop();
                }
                self.push(V::Dict(new_dict()));
            }
            "setuserparams" | "setsystemparams" | "setcolortransfer" | "setblackgeneration" | "setundercolorremoval" => {
                let k = if n == "setcolortransfer" { 4 } else { 1 };
                for _ in 0..k {
                    self.pop();
                }
            }
            // `file status` → false; `string status` → false (no such file).
            "status" => {
                self.pop();
                self.push(V::Bool(false));
            }
            "dictstack" => {
                self.pop();
                let ds: Vec<V> = self.dicts.iter().map(|d| V::Dict(d.clone())).collect();
                self.push(array(ds));
            }
            "maxlength" => {
                let k = match self.pop() {
                    V::Dict(d) => d.borrow().len(),
                    _ => 0,
                };
                self.push(V::Num(k.saturating_add(16) as f64));
            }
            "internaldict" => {
                self.pop();
                let d = self.dicts[0].clone();
                self.push(V::Dict(d));
            }
            "undefineresource" => {
                self.pop();
                self.pop();
            }
            // `[ … /Name pdfmark`: a Distiller annotation; other interpreters discard it.
            "pdfmark" => {
                while let Some(v) = self.st.pop() {
                    if matches!(v, V::Mark) {
                        break;
                    }
                }
            }
            // `source [params] /Name filter`. A SubFileDecode of the current file with a count of
            // 0 reads to its end-of-data string: skip that data (an embedded XMP packet, which
            // otherwise ran as PostScript). Other filters read what their source reads.
            "filter" => {
                let name = self.pop().key();
                let mut skipped = false;
                let source = if name == "SubFileDecode" && !matches!(self.st.last(), Some(V::Dict(_))) {
                    let eod = match self.pop() {
                        V::Str(s) => s.borrow().clone(),
                        _ => vec![],
                    };
                    let count = self.popn();
                    let source = self.pop();
                    if matches!(source, V::File) && count == 0.0 && !eod.is_empty() {
                        src.pos = match find_from(src.d, &eod, src.pos) {
                            Some(p) => p.saturating_add(eod.len()),
                            None => src.d.len(),
                        };
                        skipped = true;
                    }
                    source
                } else {
                    if matches!(self.st.last(), Some(V::Dict(_))) {
                        self.pop();
                    }
                    self.pop()
                };
                // Data already stepped over leaves nothing for `image` to skip.
                self.push(if skipped { V::Null } else { source });
            }
            "vmstatus" => {
                for _ in 0..3 {
                    self.push(V::Num(0.0));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn try_graphics_op(&mut self, n: &str) -> Result<bool, Flow> {
        match n {
            // Graphics state.
            "gsave" => {
                self.spend(self.gs.path.elements().len())?;
                let mut g = self.gs.clone();
                g.depth = self.b.depth();
                self.saved.push(g);
            }
            "grestore" => self.grestore(),
            "grestoreall" => {
                while !self.saved.is_empty() {
                    self.grestore();
                }
            }
            "initgraphics" => {
                self.gs.ctm = Affine::IDENTITY;
                self.gs.lw = 1.0;
                self.gs.color = [0.0; 3];
            }
            "setlinewidth" => self.gs.lw = self.popn(),
            "currentlinewidth" => self.push(V::Num(self.gs.lw)),
            "setlinecap" => self.gs.cap = [Cap::Butt, Cap::Round, Cap::Square][(self.popn().max(0.0) as usize).min(2)],
            "setlinejoin" => self.gs.join = [Join::Miter, Join::Round, Join::Bevel][(self.popn().max(0.0) as usize).min(2)],
            "setmiterlimit" => self.gs.miter = self.popn(),
            "setdash" => {
                let off = self.popn();
                let a: Vec<f64> = self.pop().items().unwrap_or_default().iter().filter_map(V::num).collect();
                self.gs.dash = (!a.is_empty() && a.iter().any(|v| *v > 0.0)).then_some((a, off));
            }
            "setflat" | "setstrokeadjust" | "setoverprint" | "setrenderingintent" | "setcolorrendering" | "sethalftone" | "settransfer" => {
                self.pop();
            }
            // frequency angle proc
            "setscreen" => {
                for _ in 0..3 {
                    self.pop();
                }
            }
            "setgray" => {
                let g = self.popn().clamp(0.0, 1.0);
                self.gs.color = [g; 3];
            }
            "setrgbcolor" => {
                let b = self.popn();
                let g = self.popn();
                let r = self.popn();
                self.gs.color = [r, g, b].map(|v| v.clamp(0.0, 1.0));
            }
            "sethsbcolor" => {
                let v = self.popn().clamp(0.0, 1.0);
                let s = self.popn().clamp(0.0, 1.0);
                let h = self.popn().rem_euclid(1.0) * 6.0;
                let (i, f) = (h.floor() as i32, h.fract());
                let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
                self.gs.color = match i {
                    0 => [v, t, p],
                    1 => [q, v, p],
                    2 => [p, v, t],
                    3 => [p, q, v],
                    4 => [t, p, v],
                    _ => [v, p, q],
                };
            }
            "setcmykcolor" => {
                let k = self.popn().clamp(0.0, 1.0);
                let y = self.popn().clamp(0.0, 1.0);
                let m = self.popn().clamp(0.0, 1.0);
                let c = self.popn().clamp(0.0, 1.0);
                self.gs.color = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
            }
            "setcolorspace" => {
                self.pop();
            }
            "setcolor" => {
                // Device colour by operand count (patterns are not drawn).
                let k = self.st.iter().rev().take_while(|v| matches!(v, V::Num(_))).count().min(4);
                let c: Vec<f64> = self.st.split_off(self.st.len() - k).iter().filter_map(V::num).collect();
                self.gs.color = match c.len() {
                    1 => [c[0]; 3],
                    3 => [c[0], c[1], c[2]],
                    4 => [(1.0 - c[0]) * (1.0 - c[3]), (1.0 - c[1]) * (1.0 - c[3]), (1.0 - c[2]) * (1.0 - c[3])],
                    _ => self.gs.color,
                };
            }
            "currentgray" => self.push(V::Num((self.gs.color[0] + self.gs.color[1] + self.gs.color[2]) / 3.0)),
            "currentrgbcolor" => {
                for c in self.gs.color {
                    self.push(V::Num(c));
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn try_coordinate_op(&mut self, n: &str) -> bool {
        match n {
            // Coordinates.
            "matrix" => self.push(matrix_val(Affine::IDENTITY)),
            "identmatrix" | "defaultmatrix" => {
                self.pop();
                self.push(matrix_val(Affine::IDENTITY));
            }
            "currentmatrix" => {
                self.pop();
                self.push(matrix_val(self.gs.ctm));
            }
            "setmatrix" => {
                let m = self.pop();
                self.gs.ctm = matrix_of(&m).unwrap_or(Affine::IDENTITY);
            }
            "concat" => {
                let m = self.pop();
                if let Some(m) = matrix_of(&m) {
                    self.gs.ctm *= m;
                }
            }
            "concatmatrix" => {
                self.pop();
                let b = self.pop();
                let a = self.pop();
                let (a, b) = (matrix_of(&a).unwrap_or(Affine::IDENTITY), matrix_of(&b).unwrap_or(Affine::IDENTITY));
                self.push(matrix_val(b * a));
            }
            "invertmatrix" => {
                self.pop();
                let a = self.pop();
                self.push(matrix_val(matrix_of(&a).unwrap_or(Affine::IDENTITY).inverse()));
            }
            "translate" | "scale" => {
                let top = self.pop();
                let (m_out, y) = match top {
                    V::Array(_) => (Some(top), self.popn()),
                    v => (None, v.num().unwrap_or(0.0)),
                };
                let x = self.popn();
                let t = if n == "translate" { Affine::translate((x, y)) } else { Affine::scale_non_uniform(x, y) };
                match m_out {
                    Some(_) => self.push(matrix_val(t)),
                    None => self.gs.ctm *= t,
                }
            }
            "rotate" => {
                let top = self.pop();
                match top {
                    V::Array(_) => {
                        let a = self.popn();
                        self.push(matrix_val(Affine::rotate(a.to_radians())));
                    }
                    v => self.gs.ctm *= Affine::rotate(v.num().unwrap_or(0.0).to_radians()),
                }
            }
            "transform" | "itransform" | "dtransform" | "idtransform" => {
                let top = self.pop();
                let (m, y) = match top {
                    V::Array(_) | V::Proc(_) => (matrix_of(&top).unwrap_or(Affine::IDENTITY), self.popn()),
                    v => (self.gs.ctm, v.num().unwrap_or(0.0)),
                };
                let x = self.popn();
                let m = if n.starts_with('i') { m.inverse() } else { m };
                let p = if n.contains('d') {
                    let c = m.as_coeffs();
                    Point::new(c[0] * x + c[2] * y, c[1] * x + c[3] * y)
                } else {
                    m * Point::new(x, y)
                };
                self.push(V::Num(p.x));
                self.push(V::Num(p.y));
            }
            _ => return false,
        }
        true
    }

    fn try_path_op(&mut self, n: &str) -> Result<bool, Flow> {
        match n {
            // Paths.
            "newpath" => {
                self.gs.path = BezPath::new();
                self.gs.cur = None;
                self.gs.start = None;
            }
            "moveto" => {
                let p = self.user_point();
                self.move_to(p)?;
            }
            "rmoveto" => {
                let d = self.user_point();
                let c = self.current_user();
                self.move_to(Point::new(c.x + d.x, c.y + d.y))?;
            }
            "lineto" => {
                let p = self.user_point();
                self.line_to(p)?;
            }
            "rlineto" => {
                let d = self.user_point();
                let c = self.current_user();
                self.line_to(Point::new(c.x + d.x, c.y + d.y))?;
            }
            "curveto" | "rcurveto" => {
                let p3 = self.user_point();
                let p2 = self.user_point();
                let p1 = self.user_point();
                if n == "rcurveto" {
                    let c = self.current_user();
                    let o = |p: Point| Point::new(c.x + p.x, c.y + p.y);
                    self.curve_to(o(p1), o(p2), o(p3))?;
                } else {
                    self.curve_to(p1, p2, p3)?;
                }
            }
            "closepath" => {
                if self.gs.cur.is_some() {
                    self.room(1)?;
                    self.gs.path.close_path();
                    self.gs.cur = self.gs.start;
                }
            }
            "arc" => self.arc(false)?,
            "arcn" => self.arc(true)?,
            "currentpoint" => {
                let c = self.current_user();
                self.push(V::Num(c.x));
                self.push(V::Num(c.y));
            }
            "pathbbox" => {
                self.spend(self.gs.path.elements().len())?;
                let r = kurbo::Shape::bounding_box(&self.gs.path);
                let inv = self.gs.ctm.inverse();
                let (a, b) = (inv * Point::new(r.x0, r.y0), inv * Point::new(r.x1, r.y1));
                for v in [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)] {
                    self.push(V::Num(v));
                }
            }
            "flattenpath" | "reversepath" | "strokepath" | "initclip" => {}
            "clippath" => {
                let p = self.page;
                self.gs.path = kurbo::Shape::to_path(&kurbo::Rect::new(p[0], p[1], p[2], p[3]), 0.1);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn try_paint_op(&mut self, n: &str) -> Result<bool, Flow> {
        match n {
            // Painting.
            "fill" => self.paint(Some(FillRule::NonZero), false),
            "eofill" => self.paint(Some(FillRule::EvenOdd), false),
            "stroke" => self.paint(None, true),
            "clip" | "eoclip" => {
                let rule = if n == "clip" { FillRule::NonZero } else { FillRule::EvenOdd };
                self.spend(self.gs.path.elements().len())?;
                let p = self.gs.path.clone();
                if !p.elements().iter().all(|e| matches!(e, PathEl::MoveTo(_))) {
                    self.b.clip(p, rule);
                }
            }
            "rectfill" | "rectstroke" | "rectclip" => {
                let paths = self.rect_path()?;
                if n == "rectstroke" && matches!(self.st.last(), Some(V::Array(_))) {
                    self.pop();
                }
                let mut all = BezPath::new();
                for p in paths {
                    all.extend(p);
                }
                match n {
                    "rectclip" => self.b.clip(all, FillRule::NonZero),
                    _ => {
                        let saved = std::mem::replace(&mut self.gs.path, all);
                        self.paint(if n == "rectfill" { Some(FillRule::NonZero) } else { None }, n == "rectstroke");
                        self.gs.path = saved;
                    }
                }
            }
            "shfill" => {
                self.pop();
                self.b.skip("smooth shading (shfill)");
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
