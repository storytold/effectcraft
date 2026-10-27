//! Source-to-source rewrite that gives JavaScript After Effects' array maths.
//!
//! AE's legacy expression engine lets `+ - * /` operate on arrays (`value + [10, 0]`,
//! `[1, 2] * 2`). We get the same by rewriting every binary `+ - * /` into a helper call
//! (`__add(a, b)`, `__sub`, `__mul`, `__div`) and unary minus into `__neg(a)`; the helpers fall
//! back to the native operator for numbers and strings, so string concatenation is unchanged.
//! Compound assignments (`x += [1, 0]`) become `x = __add(x, (…))`.
//!
//! The rewriter is a small tokenizer plus a precedence-aware operand/chain parser over bracket
//! groups. It is deliberately lenient: anything it does not understand is copied verbatim, and
//! every newline is preserved so the engine's error line numbers match the user's text.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Ident,
    Num,
    Str,
    Template,
    Regex,
    Punct,
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    text: String,
    /// Whitespace (comments become spaces/newlines) before the token.
    ws: String,
    /// A line break precedes the token.
    nl: bool,
}

impl Tok {
    fn is(&self, p: &str) -> bool {
        self.kind == Kind::Punct && self.text == p
    }
    fn is_kw(&self, k: &[&str]) -> bool {
        self.kind == Kind::Ident && k.contains(&self.text.as_str())
    }
}

enum Item {
    Tok(Tok),
    Group { open: Tok, inner: Vec<Item>, close: Option<Tok> },
}

impl Item {
    fn tok(&self) -> Option<&Tok> {
        if let Item::Tok(t) = self { Some(t) } else { None }
    }
    fn is(&self, p: &str) -> bool {
        self.tok().is_some_and(|t| t.is(p))
    }
    fn group(&self) -> Option<&str> {
        if let Item::Group { open, .. } = self { Some(open.text.as_str()) } else { None }
    }
    fn ws(&self) -> &str {
        match self {
            Item::Tok(t) => &t.ws,
            Item::Group { open, .. } => &open.ws,
        }
    }
    fn nl(&self) -> bool {
        match self {
            Item::Tok(t) => t.nl,
            Item::Group { open, .. } => open.nl,
        }
    }
}

/// Keywords that end an operand chain (statement or lower-precedence syntax).
const DELIM_KW: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "return",
    "switch",
    "throw",
    "try",
    "var",
    "while",
    "with",
    "yield",
    "of",
];
/// Prefix operators spelled as keywords.
const PREFIX_KW: &[&str] = &["typeof", "void", "delete", "await", "new"];
/// Keywords followed by a parenthesised header that is not an operand.
const HEADER_KW: &[&str] = &["if", "while", "for", "switch", "catch", "with"];

const PUNCT: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=",
    "*=", "/=", "%=", "&=", "|=", "^=", "**", "<<", ">>",
];

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$' || (!c.is_ascii() && !c.is_whitespace())
}
fn is_ident_part(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Can a `/` after `prev` start a regular expression literal?
fn regex_allowed(prev: Option<&Tok>) -> bool {
    match prev {
        None => true,
        Some(t) => match t.kind {
            Kind::Num | Kind::Str | Kind::Template | Kind::Regex => false,
            Kind::Ident => t.is_kw(DELIM_KW) || t.is_kw(PREFIX_KW),
            Kind::Punct => !matches!(t.text.as_str(), ")" | "]" | "}" | "++" | "--"),
        },
    }
}

fn skip_string(c: &[char], mut i: usize) -> usize {
    let q = c[i];
    i += 1;
    while i < c.len() && c[i] != q && c[i] != '\n' {
        if c[i] == '\\' {
            i += 1;
        }
        i += 1;
    }
    (i + 1).min(c.len())
}

fn skip_template(c: &[char], mut i: usize) -> usize {
    i += 1;
    let mut depth = 0usize;
    while i < c.len() {
        match c[i] {
            '\\' => i += 1,
            '`' if depth == 0 => return i + 1,
            '$' if c.get(i + 1) == Some(&'{') => {
                depth += 1;
                i += 1;
            }
            '{' if depth > 0 => depth += 1,
            '}' if depth > 0 => depth -= 1,
            '\'' | '"' if depth > 0 => {
                i = skip_string(c, i);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    c.len()
}

fn skip_number(c: &[char], mut i: usize) -> usize {
    if c[i] == '0' && c.get(i + 1).is_some_and(|x| matches!(x, 'x' | 'X' | 'b' | 'B' | 'o' | 'O')) {
        i += 2;
        while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == '_') {
            i += 1;
        }
        return i;
    }
    let mut seen_dot = false;
    while i < c.len() {
        let x = c[i];
        if x.is_ascii_digit() || x == '_' {
            i += 1;
        } else if x == '.' && !seen_dot {
            seen_dot = true;
            i += 1;
        } else if (x == 'e' || x == 'E') && c.get(i + 1).is_some_and(|y| y.is_ascii_digit() || *y == '+' || *y == '-') {
            i += 2;
            while i < c.len() && c[i].is_ascii_digit() {
                i += 1;
            }
            break;
        } else {
            break;
        }
    }
    if c.get(i) == Some(&'n') {
        i += 1;
    }
    i
}

fn skip_regex(c: &[char], mut i: usize) -> usize {
    i += 1;
    let mut class = false;
    while i < c.len() && c[i] != '\n' {
        match c[i] {
            '\\' => i += 1,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => break,
            _ => {}
        }
        i += 1;
    }
    i += 1;
    while i < c.len() && is_ident_part(c[i]) {
        i += 1;
    }
    i.min(c.len())
}

fn punct_len(c: &[char], i: usize) -> usize {
    for p in PUNCT {
        let n = p.chars().count();
        if i + n <= c.len() && c[i..i + n].iter().copied().eq(p.chars()) {
            // `a?.5:b` is a conditional, not optional chaining.
            if *p == "?." && c.get(i + 2).is_some_and(|d| d.is_ascii_digit()) {
                return 1;
            }
            return n;
        }
    }
    1
}

fn tokenize(src: &str) -> (Vec<Tok>, String) {
    let c: Vec<char> = src.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut ws = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            ws.push(ch);
            i += 1;
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            ws.push(' ');
            continue;
        }
        if ch == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                if c[i] == '\n' {
                    ws.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(c.len());
            ws.push(' ');
            continue;
        }
        let start = i;
        let kind = if ch == '"' || ch == '\'' {
            i = skip_string(&c, i);
            Kind::Str
        } else if ch == '`' {
            i = skip_template(&c, i);
            Kind::Template
        } else if ch.is_ascii_digit() || (ch == '.' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            i = skip_number(&c, i);
            Kind::Num
        } else if is_ident_start(ch) {
            while i < c.len() && is_ident_part(c[i]) {
                i += 1;
            }
            Kind::Ident
        } else if ch == '/' && regex_allowed(out.last()) {
            i = skip_regex(&c, i);
            Kind::Regex
        } else {
            i += punct_len(&c, i);
            Kind::Punct
        };
        let nl = ws.contains('\n');
        out.push(Tok { kind, text: c[start..i].iter().collect(), ws: std::mem::take(&mut ws), nl });
    }
    (out, ws)
}

fn closer(open: &str) -> &'static str {
    match open {
        "(" => ")",
        "[" => "]",
        _ => "}",
    }
}

/// Bracket tree. Stray closers stay as plain tokens (the engine reports the syntax error).
fn build(toks: Vec<Tok>) -> Vec<Item> {
    let mut stack: Vec<(Tok, Vec<Item>)> = Vec::new();
    let mut cur: Vec<Item> = Vec::new();
    for t in toks {
        if t.kind == Kind::Punct && matches!(t.text.as_str(), "(" | "[" | "{") {
            stack.push((t, std::mem::take(&mut cur)));
        } else if t.kind == Kind::Punct
            && matches!(t.text.as_str(), ")" | "]" | "}")
            && stack.last().is_some_and(|(o, _)| closer(&o.text) == t.text)
            && let Some((open, parent)) = stack.pop()
        {
            let inner = std::mem::replace(&mut cur, parent);
            cur.push(Item::Group { open, inner, close: Some(t) });
        } else {
            cur.push(Item::Tok(t));
        }
    }
    while let Some((open, parent)) = stack.pop() {
        let inner = std::mem::replace(&mut cur, parent);
        cur.push(Item::Group { open, inner, close: None });
    }
    cur
}

/// A rewritten operand or expression: leading whitespace kept apart so helper calls start where
/// the expression did (keeps automatic semicolon insertion unchanged).
struct Piece {
    ws: String,
    body: String,
}

fn raw(item: &Item) -> Piece {
    match item {
        Item::Tok(t) => Piece { ws: t.ws.clone(), body: t.text.clone() },
        Item::Group { open, inner, close } => {
            let mut body = open.text.clone();
            body.push_str(&rewrite_seq(inner));
            if let Some(c) = close {
                body.push_str(&c.ws);
                body.push_str(&c.text);
            }
            Piece { ws: open.ws.clone(), body }
        }
    }
}

fn starts_operand(items: &[Item], i: usize) -> bool {
    match &items[i] {
        Item::Group { open, .. } => open.text != "{",
        Item::Tok(t) => match t.kind {
            Kind::Num | Kind::Str | Kind::Template | Kind::Regex => true,
            Kind::Ident => !t.is_kw(DELIM_KW),
            Kind::Punct => matches!(t.text.as_str(), "-" | "+" | "!" | "~" | "++" | "--"),
        },
    }
}

/// Parse one unary/postfix operand starting at `i`.
fn operand(items: &[Item], i: usize) -> Option<(Piece, usize)> {
    let first = items.get(i)?;
    if !starts_operand(items, i) {
        return None;
    }
    // Prefix operators.
    if let Some(t) = first.tok()
        && (t.kind == Kind::Punct || t.is_kw(PREFIX_KW))
    {
        let (inner, j) = operand(items, i + 1)?;
        let body = if t.is("-") {
            if items[i + 1].tok().is_some_and(|n| n.kind == Kind::Num) {
                format!("-{}{}", inner.ws, inner.body)
            } else {
                format!("__neg({}{})", inner.ws, inner.body)
            }
        } else {
            let sep = if t.kind == Kind::Ident && inner.ws.is_empty() { " " } else { "" };
            format!("{}{sep}{}{}", t.text, inner.ws, inner.body)
        };
        return Some((Piece { ws: t.ws.clone(), body }, j));
    }
    let mut p = raw(first);
    let mut j = i + 1;
    // `function name(args) { body }` is one atom.
    if first.tok().is_some_and(|t| t.is_kw(&["function"])) {
        if items.get(j).is_some_and(|x| x.is("*")) {
            push(&mut p, &items[j]);
            j += 1;
        }
        if items.get(j).and_then(Item::tok).is_some_and(|t| t.kind == Kind::Ident) {
            push(&mut p, &items[j]);
            j += 1;
        }
        for g in ["(", "{"] {
            if items.get(j).and_then(Item::group) == Some(g) {
                push(&mut p, &items[j]);
                j += 1;
            }
        }
    }
    // Postfix: member access, calls, indexing, tagged templates, `x++`.
    while let Some(it) = items.get(j) {
        let member = it.is(".") || it.is("?.");
        let call = matches!(it.group(), Some("(" | "[")) || it.tok().is_some_and(|t| t.kind == Kind::Template);
        let incr = (it.is("++") || it.is("--")) && !it.nl();
        if !(member || call || incr) {
            break;
        }
        push(&mut p, it);
        j += 1;
        if member
            && let Some(n) = items.get(j)
            && (n.tok().is_some_and(|t| t.kind == Kind::Ident) || matches!(n.group(), Some("(" | "[")))
        {
            push(&mut p, n);
            j += 1;
        }
    }
    // Exponentiation binds tighter than `*` and is right-associative: keep it native.
    if items.get(j).is_some_and(|x| x.is("**"))
        && let Some((rhs, k)) = operand(items, j + 1)
    {
        p.body.push_str(items[j].ws());
        p.body.push_str("**");
        p.body.push_str(&rhs.ws);
        p.body.push_str(&rhs.body);
        j = k;
    }
    Some((p, j))
}

fn push(p: &mut Piece, item: &Item) {
    let r = raw(item);
    p.body.push_str(&r.ws);
    p.body.push_str(&r.body);
}

fn helper(op: &str) -> Option<&'static str> {
    Some(match op {
        "+" => "__add",
        "-" => "__sub",
        "*" => "__mul",
        "/" => "__div",
        _ => return None,
    })
}

/// Fold `a op b op c…` with `* / %` binding tighter than `+ -`.
fn fold(operands: Vec<Piece>, ops: Vec<(String, String)>) -> Piece {
    let mut terms: Vec<Piece> = Vec::new();
    let mut add_ops: Vec<(String, String)> = Vec::new();
    let mut it = operands.into_iter();
    let mut acc = it.next().unwrap_or(Piece { ws: String::new(), body: String::new() });
    for ((op, ws), rhs) in ops.into_iter().zip(it) {
        if op == "+" || op == "-" {
            terms.push(acc);
            add_ops.push((op, ws));
            acc = rhs;
        } else {
            acc = bin(acc, &op, &ws, rhs);
        }
    }
    terms.push(acc);
    let mut it = terms.into_iter();
    let mut acc = it.next().unwrap_or(Piece { ws: String::new(), body: String::new() });
    for ((op, ws), rhs) in add_ops.into_iter().zip(it) {
        acc = bin(acc, &op, &ws, rhs);
    }
    acc
}

fn bin(a: Piece, op: &str, op_ws: &str, b: Piece) -> Piece {
    let body = match helper(op) {
        Some(h) => format!("{h}({}, {op_ws}{}{})", a.body, b.ws, b.body),
        None => format!("({}{op_ws}{op}{}{})", a.body, b.ws, b.body),
    };
    Piece { ws: a.ws, body }
}

fn is_arith(item: &Item) -> bool {
    item.tok().is_some_and(|t| t.kind == Kind::Punct && matches!(t.text.as_str(), "+" | "-" | "*" | "/" | "%"))
}

/// Parse `operand (op operand)*` from `i`.
fn chain(items: &[Item], i: usize) -> Option<(Piece, usize)> {
    let (first, mut j) = operand(items, i)?;
    let mut operands = vec![first];
    let mut ops = Vec::new();
    while let Some(op) = items.get(j).filter(|x| is_arith(x)).and_then(Item::tok) {
        let Some((rhs, k)) = operand(items, j + 1) else { break };
        ops.push((op.text.clone(), op.ws.clone()));
        operands.push(rhs);
        j = k;
    }
    Some((fold(operands, ops), j))
}

/// End of the right-hand side of an assignment starting at `i`.
fn assignment_end(items: &[Item], i: usize) -> usize {
    let mut j = i;
    while j < items.len() {
        let it = &items[j];
        if it.is(";") || it.is(",") {
            break;
        }
        if j > i && it.nl() && !items.get(j - 1).is_some_and(|p| p.tok().is_some_and(|t| t.kind == Kind::Punct && !t.is(")") && !t.is("]"))) {
            break;
        }
        j += 1;
    }
    j
}

fn rewrite_seq(items: &[Item]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < items.len() {
        let it = &items[i];
        if it.tok().is_some_and(|t| t.is_kw(HEADER_KW)) && items.get(i + 1).and_then(Item::group) == Some("(") {
            push_piece(&mut out, raw(it));
            push_piece(&mut out, raw(&items[i + 1]));
            i += 2;
            continue;
        }
        if let Some((p, j)) = chain(items, i) {
            // Compound assignment: `x += v` → `x = __add(x, (v))` (the target is evaluated twice,
            // which only matters for targets with side effects).
            if let Some(op) = items.get(j).and_then(Item::tok).filter(|t| t.kind == Kind::Punct && matches!(t.text.as_str(), "+=" | "-=" | "*=" | "/=")) {
                let end = assignment_end(items, j + 1);
                let rhs = rewrite_seq(&items[j + 1..end]);
                let h = helper(&op.text[..1]).unwrap_or("__add");
                let target = compound_assignment_target(items, i, j);
                out.push_str(&p.ws);
                out.push_str(&format!("{target} ={}{h}({target}, ({rhs}))", op.ws));
                i = end;
                continue;
            }
            push_piece(&mut out, p);
            i = j;
        } else {
            push_piece(&mut out, raw(it));
            i += 1;
        }
    }
    out
}

fn raw_ref(item: &Item) -> String {
    match item {
        Item::Tok(t) => format!("{}{}", t.ws, t.text),
        Item::Group { open, inner, close } => {
            let mut out = format!("{}{}", open.ws, open.text);
            if has_assign(inner) {
                // shortcut: nested assignments stay verbatim (rewriting them would double per level), so arithmetic beside them is not array-aware
                for item in inner {
                    out.push_str(&raw_ref(item));
                }
            } else {
                out.push_str(&rewrite_seq(inner));
            }
            if let Some(close) = close {
                out.push_str(&close.text);
            }
            out
        }
    }
}

fn has_assign(items: &[Item]) -> bool {
    items.iter().any(|it| match it {
        Item::Tok(t) => ASSIGN.contains(&t.text.as_str()),
        Item::Group { inner, .. } => has_assign(inner),
    })
}

fn push_piece(out: &mut String, p: Piece) {
    out.push_str(&p.ws);
    out.push_str(&p.body);
}

/// The deepest nesting an expression may have, in levels of brackets, before and after the
/// rewrite (which nests a call per arithmetic operator). The JavaScript parser and compiler
/// recurse per level and a 2 MiB thread overflows its stack (aborting the app) at about 75
/// nested brackets, so deeper expressions are rejected with an error instead. Constructs that
/// nest without brackets count as part of a level (`too_nested`).
pub const MAX_NESTING: usize = 32;
/// Hard limit on bytes emitted by the array-math source rewrite.
pub const MAX_REWRITTEN_BYTES: usize = 4 * 1024 * 1024;

fn over_rewrite_limit(text: &str) -> bool {
    text.len() > MAX_REWRITTEN_BYTES
}

fn compound_assignment_target(items: &[Item], start: usize, end: usize) -> String {
    // Rewriting the whole left side duplicates nested computed assignment targets. Keep the
    // original target syntax for both reads/writes; source length is bounded above.
    items[start..end].iter().map(raw_ref).collect()
}

/// Stack cost of one level of brackets (also of a template substitution, a regex group and a
/// prefix operator, which cost the parser about as much or are never chained in real code).
const LEVEL: usize = 256;
/// Stack cost of a construct the parser recurses on without a bracket: `?:`, `=>`, assignments,
/// `**`, `||`, labels, `if`/`else`/`while`/`for`/`with`/`do` without a block (about 1/8 of a
/// bracket: 256 of them in a row are allowed; boa overflows from about 750).
const HEAVY: usize = 32;
/// Stack cost of a link in a left-deep chain (binary operators, member access, calls, indexing,
/// the comma operator): the tree is built in a loop but compiled recursively (overflows from
/// about 3 800 links).
const LIGHT: usize = 6;

/// One bracket level of `too_nested`'s walk.
#[derive(Default)]
struct Level {
    /// `[`: commas separate elements (a loop in the parser), not operands.
    array: bool,
    /// Cost of the chains of the current statement or element.
    chain: usize,
    /// Comma operators so far in the current statement.
    commas: usize,
    /// `else` branches in a row: `if … else if … else if …` nests even across `;`.
    elses: usize,
    /// The next token starts a statement.
    fresh: bool,
}

impl Level {
    fn boundary(&mut self) {
        self.chain = 0;
        self.commas = 0;
        self.fresh = true;
    }
}

const ASSIGN: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "**=", "<<=", ">>=", ">>>=", "&=", "|=", "^=", "&&=", "||=", "??="];

/// Can `t` end an expression statement (so a line break after it may end the statement)?
fn ends_value(t: &Tok) -> bool {
    match t.kind {
        Kind::Num | Kind::Str | Kind::Template | Kind::Regex => true,
        Kind::Ident => !t.is_kw(DELIM_KW) && !t.is_kw(PREFIX_KW),
        Kind::Punct => matches!(t.text.as_str(), ")" | "]" | "}" | "++" | "--"),
    }
}

/// Can `t` only start a new statement after a value (automatic semicolon insertion)?
fn starts_statement(t: &Tok) -> bool {
    match t.kind {
        Kind::Num | Kind::Str => true,
        Kind::Ident => !t.is_kw(&["in", "instanceof", "of"]),
        Kind::Punct => matches!(t.text.as_str(), "{" | "!" | "~" | "++" | "--"),
        Kind::Template | Kind::Regex => false,
    }
}

/// Bracket depth inside a template literal (its `${…}` substitutions) or a regex (its groups).
fn inner_depth(text: &str) -> usize {
    let (mut d, mut max) = (0usize, 0usize);
    for c in text.chars() {
        match c {
            '(' | '[' | '{' => d += 1,
            ')' | ']' | '}' => d = d.saturating_sub(1),
            _ => {}
        }
        max = max.max(d);
    }
    max
}

/// The code of a template literal's `${…}` substitutions (the outermost ones).
fn substitutions(template: &str) -> Vec<String> {
    let c: Vec<char> = template.chars().collect();
    let mut out = vec![];
    let mut i = 1;
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '$' if c.get(i + 1) == Some(&'{') => {
                let start = i + 2;
                let (mut j, mut depth) = (start, 0usize);
                while j < c.len() {
                    match c[j] {
                        '{' => depth += 1,
                        '}' if depth == 0 => break,
                        '}' => depth -= 1,
                        '\'' | '"' => {
                            j = skip_string(&c, j);
                            continue;
                        }
                        '`' => {
                            j = skip_template(&c, j);
                            continue;
                        }
                        _ => {}
                    }
                    j += 1;
                }
                out.push(c.get(start..j.min(c.len())).unwrap_or_default().iter().collect());
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Whether `toks` nest deeper than [`MAX_NESTING`] levels: an estimate of the parser's stack
/// use, summing brackets, runs of prefix operators and, per bracket level, the chains of the
/// current statement (or array element) weighted by what each link costs the parser.
fn too_nested(toks: &[Tok]) -> bool {
    too_nested_from(toks, 0)
}

/// `too_nested` for code that already sits `base` deep (a template substitution).
fn too_nested_from(toks: &[Tok], base: usize) -> bool {
    let mut levels = vec![Level::default()];
    let mut prefix = 0usize;
    let mut prev: Option<&Tok> = None;
    for t in toks {
        let p = t.kind == Kind::Punct;
        if let Some(l) = levels.last_mut() {
            if t.nl && prev.is_some_and(ends_value) && starts_statement(t) {
                l.boundary();
            }
            if l.fresh {
                if !t.is_kw(&["else"]) {
                    l.elses = 0;
                }
                l.fresh = false;
            }
        }
        let mut inner = 0;
        if p && matches!(t.text.as_str(), "(" | "[" | "{") {
            // A call or index is a link in its chain too (`f()()()…`).
            if t.text != "{"
                && let Some(l) = levels.last_mut()
            {
                l.chain = l.chain.saturating_add(LIGHT);
            }
            levels.push(Level { array: t.text == "[", ..Level::default() });
        } else if p && matches!(t.text.as_str(), ")" | "]" | "}") {
            if levels.len() > 1 {
                levels.pop();
            }
        } else if let Some(l) = levels.last_mut() {
            if t.is(";") {
                l.boundary();
            } else if t.is(",") {
                l.commas += usize::from(!l.array);
                l.chain = l.commas.saturating_mul(LIGHT);
            } else if t.is_kw(&["else"]) {
                l.elses += 1;
            } else if (p && (matches!(t.text.as_str(), "?" | ":" | "=>" | "**" | "||") || ASSIGN.contains(&t.text.as_str())))
                || t.is_kw(&["if", "while", "for", "with", "do"])
            {
                l.chain = l.chain.saturating_add(HEAVY);
            } else if p || t.is_kw(&["in", "instanceof"]) {
                l.chain = l.chain.saturating_add(LIGHT);
            } else if matches!(t.kind, Kind::Template | Kind::Regex) {
                // A tagged template is a link like a call.
                l.chain = l.chain.saturating_add(LIGHT);
                inner = inner_depth(&t.text);
            }
        }
        let is_prefix = (p && matches!(t.text.as_str(), "!" | "~" | "-" | "+" | "++" | "--")) || t.is_kw(&["typeof", "void", "delete", "new", "await"]);
        prefix = if is_prefix { prefix + 1 } else { 0 };
        let mut cost = (levels.len() - 1 + prefix + inner).saturating_mul(LEVEL).saturating_add(base);
        for l in &levels {
            cost = cost.saturating_add(l.chain).saturating_add(l.elses.saturating_mul(HEAVY));
        }
        if cost > MAX_NESTING * LEVEL {
            return true;
        }
        // The code inside `${…}` is parsed like any other, one level deeper (the depth check
        // above bounds this recursion).
        if t.kind == Kind::Template
            && t.text.contains("${")
            && substitutions(&t.text)
                .iter()
                .any(|sub| too_nested_from(&tokenize(sub).0, cost.saturating_sub(inner.saturating_mul(LEVEL)).saturating_add(LEVEL)))
        {
            return true;
        }
        prev = Some(t);
    }
    false
}

/// An error when `src` nests deeper than [`MAX_NESTING`] levels (before or after the rewrite).
pub fn check_nesting(src: &str) -> Result<(), String> {
    if over_rewrite_limit(src) {
        return Err(too_large());
    }
    let (toks, tail) = tokenize(src);
    if too_nested(&toks) {
        return Err(too_deep());
    }
    let mut out = rewrite_seq(&build(toks));
    out.push_str(&tail);
    if over_rewrite_limit(&out) {
        return Err(too_large());
    }
    if too_nested(&tokenize(&out).0) {
        return Err(too_deep());
    }
    Ok(())
}

fn too_deep() -> String {
    format!("Error: expression is nested too deeply (more than {MAX_NESTING} levels of brackets, or too long a chain of operators)")
}

/// Rewrite an expression's source for array-aware arithmetic. Line breaks are preserved.
/// An expression nested deeper than [`MAX_NESTING`] becomes a `throw` of that error.
pub fn rewrite(src: &str) -> String {
    if over_rewrite_limit(src) {
        return throw_too_large();
    }
    let (toks, tail) = tokenize(src);
    if too_nested(&toks) {
        return throw_too_deep();
    }
    let mut out = rewrite_seq(&build(toks));
    out.push_str(&tail);
    if over_rewrite_limit(&out) {
        return throw_too_large();
    }
    if too_nested(&tokenize(&out).0) {
        return throw_too_deep();
    }
    out
}

fn throw_too_deep() -> String {
    format!("throw new RangeError({:?})", too_deep().trim_start_matches("Error: "))
}

fn too_large() -> String {
    format!("Error: expression exceeds the {MAX_REWRITTEN_BYTES}-byte rewrite limit")
}

fn throw_too_large() -> String {
    format!("throw new RangeError({:?})", too_large().trim_start_matches("Error: "))
}

#[cfg(test)]
mod tests {
    use super::{MAX_NESTING, check_nesting, rewrite};

    /// Compare ignoring spaces (newlines are significant).
    fn n(s: &str) -> String {
        s.replace(' ', "")
    }

    #[test]
    fn precedence_and_unary() {
        assert_eq!(n(&rewrite("a + b * c")), n("__add(a,  __mul(b,  c))"));
        assert_eq!(n(&rewrite("-x * 2")), n("__mul(__neg(x),  2)"));
        assert_eq!(n(&rewrite("-2*x")), n("__mul(-2, x)"));
        assert_eq!(n(&rewrite("a % b + c")), n("__add((a % b),  c)"));
        assert_eq!(n(&rewrite("a - b - c")), n("__sub(__sub(a,  b),  c)"));
    }

    #[test]
    fn calls_members_and_groups() {
        assert_eq!(n(&rewrite("f(a+b)[0] / 2")), n("__div(f(__add(a, b))[0],  2)"));
        assert_eq!(n(&rewrite("thisComp.layer(\"A\").transform.position[0] + 1")), n("__add(thisComp.layer(\"A\").transform.position[0],  1)"));
        assert_eq!(n(&rewrite("x = a ? b + 1 : c")), n("x = a ? __add(b,  1) : c"));
        assert_eq!(n(&rewrite("if (a) -b; else c")), n("if (a) __neg(b); else c"));
        assert_eq!(n(&rewrite("return -x")), n("return __neg(x)"));
    }

    #[test]
    fn strings_regex_comments_and_lines() {
        assert_eq!(n(&rewrite("'a+b' + \"c\"")), n("__add('a+b',  \"c\")"));
        assert_eq!(n(&rewrite("s.replace(/a+b/g, '')")), n("s.replace(/a+b/g, '')"));
        assert_eq!(n(&rewrite("a // x + y\n+ b")), n("__add(a,  \n b)"));
        assert_eq!(rewrite("a /* + */ * b").matches("__mul").count(), 1);
        assert_eq!(rewrite("x\n\n+ 1").lines().count(), 3);
        assert_eq!(n(&rewrite("`${a+b}` + 1")), n("__add(`${a+b}`,  1)"));
    }

    #[test]
    fn compound_assignment_and_functions() {
        assert_eq!(n(&rewrite("x += [1, 0];")), n("x = __add(x, ( [1, 0]));"));
        assert_eq!(n(&rewrite("obj[key] += 1")), n("obj[key] = __add(obj[key], (1))"));
        assert_eq!(n(&rewrite("o[a + b] += 1")), n("o[__add(a, b)] = __add(o[__add(a, b)], (1))"));
        assert_eq!(n(&rewrite("function f(a) { return a * 2 }")), n("function f(a) { return __mul(a,  2) }"));
        assert_eq!(n(&rewrite("var g = (a) => a - 1")), n("var g = (a) => __sub(a,  1)"));
        assert_eq!(n(&rewrite("typeof a + 'x'")), n("__add(typeof a,  'x')"));
        assert_eq!(n(&rewrite("i++ + 1")), n("__add(i++,  1)"));
        assert_eq!(n(&rewrite("2 ** 3 * 2")), n("__mul(2 ** 3,  2)"));
    }

    #[test]
    fn source_expansion_is_bounded_before_rewriting() {
        let src = "x += 1;".repeat(super::MAX_REWRITTEN_BYTES / 6 + 1);
        assert!(check_nesting(&src).unwrap_err().contains("rewrite limit"));
        assert!(rewrite(&src).starts_with("throw new RangeError("));
    }

    #[test]
    fn output_expansion_is_bounded() {
        let src = format!("{} += 1", "a".repeat(super::MAX_REWRITTEN_BYTES / 2));
        assert!(check_nesting(&src).unwrap_err().contains("rewrite limit"));
        assert!(rewrite(&src).starts_with("throw new RangeError("));
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        // Each level costs the JavaScript parser stack: ~60 nested brackets overflowed a 2 MiB
        // thread and aborted the app; 100 000 overflowed the rewriter itself.
        for n in [MAX_NESTING + 1, 100, 100_000] {
            let parens = "(".repeat(n) + "1" + &")".repeat(n);
            let unclosed = "[".repeat(n);
            let sum = vec!["x"; n + 1].join(" + ");
            let nots = "!".repeat(n) + "x";
            for src in [parens, unclosed, sum, nots] {
                assert!(rewrite(&src).starts_with("throw new RangeError("), "{n}");
                assert!(check_nesting(&src).is_err());
                assert!(crate::check_syntax(&src).unwrap_err().contains("nested too deeply"));
            }
        }
        let ok = "(".repeat(MAX_NESTING - 2) + "a + b" + &")".repeat(MAX_NESTING - 2);
        assert!(check_nesting(&ok).is_ok());
        assert!(crate::check_syntax(&ok).is_ok());
        assert!(check_nesting(&vec!["x"; 20].join(" + ")).is_ok());
    }

    #[test]
    fn long_chains_without_brackets_are_an_error_not_a_stack_overflow() {
        // Found by the expression fuzzer (#786): the parser also recurses on chains that need no
        // bracket, so a few hundred `?:`, `=>` or `if`, or a few thousand `.`/`<`/`()` links,
        // aborted the app with a stack overflow; so did 75 nested template substitutions or
        // regex groups (both hidden inside one token), and 6 000 prefix `new`/`++` the rewriter.
        let chains: [(&str, &str); 19] = [
            ("x?x:", ""),
            ("()=>", ""),
            ("a=>", ""),
            ("if(x)", ""),
            ("if(x)x;else ", ""),
            ("x=", ""),
            ("x**", ""),
            ("x||", ""),
            ("x<", ""),
            ("x,", ""),
            ("x.y", ""),
            ("f()", ""),
            ("a:", ""),
            ("new ", ""),
            ("++", ""),
            ("`${", "}`"),
            ("s.replace(/(", ")/, '')"),
            ("`${x??", "}`"),
            ("{a:", "}"),
        ];
        for (open, close) in chains {
            for n in [2_000, 100_000] {
                let src = open.repeat(n) + "1" + &close.repeat(n);
                assert!(rewrite(&src).starts_with("throw new RangeError("), "{open} × {n}");
                assert!(crate::check_syntax(&src).unwrap_err().contains("nested too deeply"), "{open} × {n}");
            }
        }
        // A chain inside one template substitution (the template is one token to the rewriter).
        let src = format!("`${{{}1}}`", "x??".repeat(100_000));
        assert!(crate::check_syntax(&src).unwrap_err().contains("nested too deeply"));
    }

    #[test]
    fn long_real_expressions_still_parse() {
        let lines = |n: usize, line: &str| vec![line; n].join("\n");
        let else_ifs = (0..200).map(|i| format!("if (time < {i}) {i}; else ")).collect::<String>() + "0";
        let numbers = format!("[{}][0]", (0..5000).map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
        let points = format!("createPath([{}], [], [], true)", (0..500).map(|i| format!("[{i}, {}]", i * 2)).collect::<Vec<_>>().join(", "));
        let object = format!("var o = {{{}}}; o.k0", (0..200).map(|i| format!("k{i}: time > {i} ? {i} : -{i}")).collect::<Vec<_>>().join(", "));
        let program = "var x = 0\n".to_string() + &lines(300, "x = x + thisComp.layer(1).transform.position[0] * 2 - 1") + "\nx";
        let ifs = "var y = 0\n".to_string() + &lines(300, "if (time > 1) y = y + 1") + "\ny";
        let chained = vec!["thisComp.layer(1).transform.position.valueAtTime(time - 0.1)[0]"; 30].join(" + ");
        let template = "`${thisLayer.name}: ${Math.floor(time * 10) / 10} s, ${`${index}`} {}`".to_string();
        for src in [else_ifs, numbers, points, object, program, ifs, chained, template] {
            assert!(check_nesting(&src).is_ok(), "{}", &src[..80]);
            assert!(crate::check_syntax(&src).is_ok(), "{}", &src[..80]);
        }
    }
}
