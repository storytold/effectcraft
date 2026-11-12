//! What a composition's pixels can depend on, from the project in memory: the items it draws
//! and, when expressions are among them, what those expressions can read (#3184). The disk
//! cache keys a comp's frames by exactly these items, so an edit elsewhere in the project keeps
//! them.
//!
//! An expression reaches its own comp's layers (`thisComp`, `thisLayer`), the sources of the
//! layers it can read (`layer.source`, a precomp's layers through it), and other items by name:
//! `comp("Name")` and `footage("Name")`. A name given as a plain string literal is followed; a
//! computed name (`comp("Shot " + index)`), the object model's constructors and internals
//! (`new Comp(7)`, `__h`), dynamic code (`eval`, `Function`) and anything the scanner can't read
//! for sure (a regular expression literal, a template string, an escape in a name) count as
//! reaching every item of the project. The scan errs on the side of reaching more: an item
//! wrongly left out would serve a stale frame, one wrongly let in only renders again.
//!
//! Known limit (`ponytail`): an expression that builds the names of the object model's private
//! fields at run time into a variable and writes them with a computed key it reads from that
//! variable could forge another comp's id. That is deliberate tampering with the internals, not
//! an expression one writes; the scan refuses every direct form of it (`__` names, the
//! constructors, `Object`, `JSON`, `call`/`apply`/`bind`, computed writes and computed keys).
//! Upgrade: have the expression host record which items an evaluation reached.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Comp, ItemId, ItemKind, LayerId, Node, Project, PropGroup, Value};

/// What an expression's text can reach outside its own comp and the layers it can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExprReach {
    /// Only items named by string literals: comps by `comp("…")`, footage by `footage("…")`.
    Names { comps: Vec<String>, footage: Vec<String> },
    /// Any item of the project.
    Anything,
}

/// Names that hand an expression another item by id, run code built at run time, or give it the
/// tools to forge the object model's private fields.
const REACHES_ANYTHING: &[&str] = &[
    "Comp",
    "Footage",
    "Layer",
    "Prop",
    "Markers",
    "eval",
    "Function",
    "globalThis",
    "this",
    "self",
    "window",
    "arguments",
    "constructor",
    "prototype",
    "Object",
    "Reflect",
    "Proxy",
    "JSON",
    "call",
    "apply",
    "bind",
    "class",
    "with",
    "import",
    "require",
    "fromCharCode",
    "fromCodePoint",
    "getOwnPropertyDescriptor",
    "getOwnPropertyDescriptors",
    "getPrototypeOf",
    "setPrototypeOf",
    "defineProperty",
];

/// Keywords after which a `/` starts a regular expression.
const BEFORE_REGEX: &[&str] = &["return", "typeof", "instanceof", "in", "of", "new", "delete", "void", "throw", "case", "do", "else", "yield", "await"];

/// Keywords whose `( … )` a regular expression may follow (`if (a) /x/.test(s)`).
const CONTROL: &[&str] = &["if", "while", "for", "with"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok<'a> {
    Ident(&'a str),
    /// A string literal's text between its quotes; `plain` when it has no escape.
    Str {
        text: &'a str,
        plain: bool,
    },
    Num,
    Punct(u8),
}

/// The tokens of `text`, or `None` when it can't be read for sure.
fn tokens(text: &str) -> Option<Vec<Tok<'_>>> {
    let b = text.as_bytes();
    let mut out: Vec<Tok<'_>> = Vec::new();
    // Per open `(`: whether a control keyword is before it; whether the last `)` closed one
    // (only looked at right after a `)`).
    let mut parens: Vec<bool> = Vec::new();
    let mut control_close = false;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match c {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => i += 1,
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|c| *c != b'\n') {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text.get(i + 2..)?.find("*/")?;
                i += 2 + end + 2;
            }
            b'/' => {
                if regex_may_start(out.last(), control_close) {
                    return None;
                }
                out.push(Tok::Punct(c));
                i += 1;
            }
            b'"' | b'\'' => {
                let start = i + 1;
                let mut j = start;
                let mut plain = true;
                loop {
                    match *b.get(j)? {
                        q if q == c => break,
                        b'\n' | b'\r' => return None,
                        b'\\' => {
                            // Only escapes that can't spell a name: `\n`, `\t`, a quote...
                            if !matches!(b.get(j + 1)?, b'n' | b'r' | b't' | b'b' | b'f' | b'v' | b'\'' | b'"' | b'\\') {
                                return None;
                            }
                            plain = false;
                            j += 2;
                        }
                        _ => j += 1,
                    }
                }
                let s = text.get(start..j)?;
                if s.contains("__") {
                    return None;
                }
                out.push(Tok::Str { text: s, plain });
                i = j + 1;
            }
            b'0'..=b'9' => {
                while b.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.') {
                    i += 1;
                }
                out.push(Tok::Num);
            }
            b'.' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                i += 1;
                while b.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'.') {
                    i += 1;
                }
                out.push(Tok::Num);
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                let start = i;
                while b.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$') {
                    i += 1;
                }
                out.push(Tok::Ident(text.get(start..i)?));
            }
            // A template string (code inside `${}`), an escaped identifier, a name in another
            // alphabet: not followed.
            b'`' | b'\\' => return None,
            c if !c.is_ascii() => return None,
            c => {
                match c {
                    b'(' => parens.push(matches!(out.last(), Some(Tok::Ident(k)) if CONTROL.contains(k))),
                    b')' => control_close = parens.pop().unwrap_or(false),
                    _ => {}
                }
                out.push(Tok::Punct(c));
                i += 1;
            }
        }
    }
    Some(out)
}

/// Whether a `/` after `prev` may start a regular expression literal (which the scanner doesn't
/// read: its text could hide a quote).
fn regex_may_start(prev: Option<&Tok<'_>>, control_close: bool) -> bool {
    match prev {
        None => true,
        Some(Tok::Ident(k)) => BEFORE_REGEX.contains(k),
        Some(Tok::Num | Tok::Str { .. }) => false,
        Some(Tok::Punct(b')')) => control_close,
        Some(Tok::Punct(b']')) => false,
        Some(Tok::Punct(_)) => true,
    }
}

/// What the expression `text` can reach by name, or [`ExprReach::Anything`].
pub fn expression_reach(text: &str) -> ExprReach {
    expression_names(text).map_or(ExprReach::Anything, |(comps, footage)| ExprReach::Names { comps, footage })
}

/// The comp names and footage names an expression references by literal name.
/// Returns `None` when it can reach anything by a computed name or cannot be tokenised.
fn expression_names(text: &str) -> Option<(Vec<String>, Vec<String>)> {
    let toks = tokens(text)?;
    let (mut comps, mut footage) = (Vec::new(), Vec::new());
    // Per open `[`: a computed key (`{[k]: v}`), a computed member (`o[k]`).
    let mut brackets: Vec<(bool, bool)> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let prev = i.checked_sub(1).and_then(|p| toks.get(p));
        let next = |n: usize| toks.get(i + n);
        match *t {
            Tok::Ident(name) if name.starts_with("__") || REACHES_ANYTHING.contains(&name) => return None,
            Tok::Ident(name @ ("comp" | "footage")) => {
                // Only a call with a plain literal, not a property of something else.
                let (Some(Tok::Punct(b'(')), Some(Tok::Str { text, plain: true }), Some(Tok::Punct(b')'))) = (next(1), next(2), next(3)) else { return None };
                if matches!(prev, Some(Tok::Punct(b'.'))) {
                    return None;
                }
                if name == "comp" { comps.push((*text).to_string()) } else { footage.push((*text).to_string()) }
            }
            Tok::Punct(b'[') => {
                let key = matches!(prev, Some(Tok::Punct(b'{' | b',')) | Some(Tok::Ident("get" | "set" | "static" | "async")) | Some(Tok::Punct(b'*')));
                let member = matches!(prev, Some(Tok::Ident(_) | Tok::Str { .. } | Tok::Num | Tok::Punct(b')' | b']' | b'.')));
                brackets.push((key, member));
            }
            Tok::Str { .. } if brackets.last().is_some_and(|(_, member)| *member) => return None,
            Tok::Punct(b']') => {
                let (key, _) = brackets.pop().unwrap_or((true, true));
                // A computed key (`{[k]: v}`, `{[k]() {}}`) or a write through brackets (`o[k] = v`,
                // `o[k] ||= v`, `for (o[k] in …)`, `[o[k]] = v`) could name a private field.
                if key && matches!(next(1), Some(Tok::Punct(b':' | b'('))) {
                    return None;
                }
                if assigns(&toks, i + 1) || matches!(next(1), Some(Tok::Ident("in" | "of"))) {
                    return None;
                }
            }
            // A destructuring write (`({a: o[k]} = v)`).
            Tok::Punct(b'}') if assigns(&toks, i + 1) => return None,
            _ => {}
        }
    }
    Some((comps, footage))
}

/// Whether the tokens from `at` are an assignment operator (`=`, `+=`, `>>>=`, `??=`...), not a
/// comparison (`==`, `<=`) or an arrow (`=>`).
fn assigns(toks: &[Tok<'_>], at: usize) -> bool {
    let mut op = Vec::new();
    for t in toks.iter().skip(at).take(5) {
        match t {
            Tok::Punct(c) if b"+-*/%&|^<>?=".contains(c) => op.push(*c),
            _ => break,
        }
    }
    let Some(eq) = op.iter().position(|c| *c == b'=') else { return false };
    let (before, after) = op.split_at(eq);
    if after.get(1).is_some_and(|c| *c == b'=' || *c == b'>') {
        return false;
    }
    matches!(before, b"" | b"+" | b"-" | b"*" | b"/" | b"%" | b"&" | b"|" | b"^" | b"**" | b"<<" | b">>" | b">>>" | b"&&" | b"||" | b"??")
}

/// The items a comp's pixels can depend on ([`Project::pixel_reach`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PixelReach {
    /// The items whose state counts; `None`: every item of the project.
    pub items: Option<BTreeSet<ItemId>>,
    /// Expressions are among them: they can read every layer of the comps they reach (hidden
    /// and silent ones too) and the markers, which therefore count as well (#1083).
    pub expressions: bool,
    /// The names expressions look items up by (`true`: a comp's, `false`: footage's), each with
    /// the ids of the items of that kind that have it now, in id order (the first one is what
    /// the lookup finds; another item taking or giving up the name changes it).
    pub names: Vec<(bool, String, Vec<ItemId>)>,
}

/// Each property of a group and its subgroups (iterative: no depth limit to hit).
fn each_prop<'a>(g: &'a PropGroup, f: &mut dyn FnMut(&'a crate::Property) -> Option<()>) -> Option<()> {
    let mut stack = vec![g];
    while let Some(g) = stack.pop() {
        for n in &g.children {
            match n {
                Node::Prop(p) => f(p)?,
                Node::Group(sub) => stack.push(sub),
            }
        }
    }
    Some(())
}

impl Project {
    /// The items comp `comp` draws ([`Self::silent_layers`] aside, with `heard`) and the
    /// footage its precomp layers swap in (Essential Graphics Media Replacement); with an
    /// expression in any of its comps, everything those expressions can reach
    /// ([`expression_reach`]). Memory only: no file is looked at.
    pub fn pixel_reach(&self, comp: ItemId, heard: &crate::Heard) -> PixelReach {
        let Some(drawn) = self.reach_walk(comp, &|id, c| self.silent_layers(id, c, heard), &mut |_| Some(vec![])) else { return PixelReach::every() };
        let has_expression = |g: &PropGroup| each_prop(g, &mut |p| p.expr.is_none().then_some(())).is_none();
        if !drawn.iter().filter_map(|id| self.comp(*id)).any(|c| c.layers.iter().any(|l| has_expression(&l.props))) {
            return PixelReach { items: Some(drawn), expressions: false, names: vec![] };
        }
        // Expressions: every layer of a comp they reach counts, and what they name.
        let mut names: BTreeMap<(bool, String), Vec<ItemId>> = BTreeMap::new();
        let mut scan = |p: &crate::Property| match p.expr.as_ref().map(|e| expression_reach(&e.text)) {
            None => Some(vec![]),
            Some(ExprReach::Anything) => None,
            Some(ExprReach::Names { comps, footage }) => {
                let mut found = vec![];
                for (is_comp, name) in comps.into_iter().map(|n| (true, n)).chain(footage.into_iter().map(|n| (false, n))) {
                    let ids = self.named(is_comp, &name);
                    found.extend(ids.iter().copied());
                    names.insert((is_comp, name), ids);
                }
                Some(found)
            }
        };
        let Some(mut items) = self.reach_walk(comp, &|_, _| BTreeSet::new(), &mut scan) else { return PixelReach::every() };
        // A source's data is read by its name (`layer.source.sourceData`).
        let footage_names: BTreeSet<&str> =
            items.iter().filter_map(|id| self.item(*id)).filter(|i| matches!(i.kind, ItemKind::Footage(_))).map(|i| i.name.as_str()).collect();
        let same_name: Vec<ItemId> =
            self.items.values().filter(|i| matches!(i.kind, ItemKind::Footage(_)) && footage_names.contains(i.name.as_str())).map(|i| i.id).collect();
        items.extend(same_name);
        PixelReach { items: Some(items), expressions: true, names: names.into_iter().map(|((c, n), ids)| (c, n, ids)).collect() }
    }

    /// The items of one kind (`is_comp`: comps, else footage) named `name`, in id order.
    fn named(&self, is_comp: bool, name: &str) -> Vec<ItemId> {
        self.items
            .values()
            .filter(|i| i.name == name && if is_comp { matches!(i.kind, ItemKind::Comp(_)) } else { matches!(i.kind, ItemKind::Footage(_)) })
            .map(|i| i.id)
            .collect()
    }

    /// The items reached from `comp` through its layers (but those `quiet` names) and the items
    /// `scan` returns for each of their properties; `None` when something reaches every item.
    /// (A stack and a visited set: each item is visited once, cycles included.)
    fn reach_walk(
        &self,
        comp: ItemId,
        quiet: &dyn Fn(ItemId, &Comp) -> BTreeSet<LayerId>,
        scan: &mut dyn FnMut(&crate::Property) -> Option<Vec<ItemId>>,
    ) -> Option<BTreeSet<ItemId>> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![comp];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(c) = self.comp(id) else { continue };
            let quiet = quiet(id, c);
            for l in c.layers.iter().filter(|l| !quiet.contains(&l.id)) {
                // Every source item: a 3D model's too (it is drawn, and `layer.source` reads its
                // name and size).
                if let Some(item) = l.source.item() {
                    stack.push(item);
                }
                stack.extend(self.media_replacements(l)?);
                let mut reached = vec![];
                each_prop(&l.props, &mut |p| {
                    reached.extend(scan(p)?);
                    Some(())
                })?;
                stack.extend(reached);
            }
        }
        Some(seen)
    }

    /// The footage a precomp layer swaps in through its Essential Properties (Media Replacement):
    /// the items its overridden properties name. `None` when they can't be listed (an
    /// expression or keyframes choose the item over time).
    fn media_replacements(&self, layer: &crate::Layer) -> Option<Vec<ItemId>> {
        let over = crate::essential::overridden(layer);
        let Some(g) = crate::essential::group(layer).filter(|_| !over.is_empty()) else { return Some(vec![]) };
        let mut out = vec![];
        each_prop(g, &mut |p| {
            if !over.contains(&p.uid) {
                return Some(());
            }
            if p.expr.is_some() || !p.keys.is_empty() {
                return None;
            }
            if let Value::Scalar(id) = p.value {
                let item = ItemId(id.max(0.0) as u64);
                if matches!(self.item(item).map(|i| &i.kind), Some(ItemKind::Footage(_))) {
                    out.push(item);
                }
            }
            Some(())
        })?;
        Some(out)
    }
}

impl PixelReach {
    /// Every item of the project, with expressions.
    fn every() -> PixelReach {
        PixelReach { items: None, expressions: true, names: vec![] }
    }
}

#[cfg(test)]
mod tests;
