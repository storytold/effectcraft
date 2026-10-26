//! WGSL rules naga accepts but stricter front-ends (Tint, i.e. the browser's
//! WebGPU, which receives the WGSL source verbatim on wasm) reject.
//!
//! WGSL gives the bitwise operators `&`, `|` and `^` no precedence against any other
//! binary operator: `a * b ^ c` must be written `(a * b) ^ c`, and `a & b == c` as
//! `(a & b) == c`. Only a chain of the same bitwise operator may go unparenthesised.

use std::fs;
use std::path::Path;

#[derive(Clone, Copy, PartialEq)]
enum Tok<'a> {
    Operand,
    Open,
    Close,
    Op(&'a str),
    /// A unary prefix (`!`, `~`): the operator after it cannot be binary.
    Prefix,
    /// Ends an expression: `,` `;` `{` `}` `:` `=` compound assignment, `->`, `@`.
    Sep,
}

const BITWISE: [&str; 3] = ["&", "|", "^"];

const OPS: [&str; 18] = ["&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "<", ">", "+", "-", "*", "/", "%", "&", "|", "^"];

/// Drop comments and template argument lists (`vec3<f32>`, `array<vec4<u32>>`), whose
/// angle brackets are not relational operators.
fn strip(src: &str) -> String {
    let mut out = drop_comments(src);
    loop {
        let (next, changed) = drop_template_lists_once(&out);
        out = next;
        if !changed {
            return out;
        }
    }
}

fn drop_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < b.len() && !b[i..].starts_with(b"*/") {
                out.push(if b[i] == b'\n' { '\n' } else { ' ' });
                i += 1;
            }
            i += 2;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// One pass over `out` dropping every template argument list. Returns whether one was dropped.
fn drop_template_lists_once(out: &str) -> (String, bool) {
    let mut changed = false;
    let mut next = String::with_capacity(out.len());
    let c: Vec<char> = out.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if let Some(end) = template_list_end(&c, i) {
            i = end;
            changed = true;
            continue;
        }
        next.push(c[i]);
        i += 1;
    }
    (next, changed)
}

/// If `c[i]` opens a template argument list after an identifier, the index just past its `>`.
fn template_list_end(c: &[char], i: usize) -> Option<usize> {
    let prev = i.checked_sub(1).and_then(|p| c.get(p))?;
    if c.get(i) != Some(&'<') || !(prev.is_alphanumeric() || *prev == '_') {
        return None;
    }
    let mut j = i + 1;
    while let Some(&ch) = c.get(j) {
        if !(ch.is_alphanumeric() || ch == '_' || ch == ',' || ch == ' ') {
            break;
        }
        j += 1;
    }
    (c.get(j) == Some(&'>')).then_some(j + 1)
}

/// Scans the token starting at `*at` (not whitespace) and advances `*at` past it.
fn next_token<'a>(s: &'a str, at: &mut usize) -> Tok<'a> {
    let b = s.as_bytes();
    let Some(&ch) = b.get(*at) else {
        return Tok::Sep;
    };
    let mut i = *at;
    let tok = if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.' {
        // Identifier, member access or number (`1e-6`, `0x9E3779B9u`).
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
            if (b[i] == b'e' || b[i] == b'E')
                && b[start].is_ascii_digit()
                && !s[start..i].starts_with("0x")
                && i + 1 < b.len()
                && (b[i + 1] == b'-' || b[i + 1] == b'+')
            {
                i += 1;
            }
            i += 1;
        }
        Tok::Operand
    } else if ch == b'(' || ch == b'[' {
        i += 1;
        Tok::Open
    } else if ch == b')' || ch == b']' {
        i += 1;
        Tok::Close
    } else if b[i..].starts_with(b"->") || b[i..].starts_with(b"++") || b[i..].starts_with(b"--") {
        i += 2;
        Tok::Sep
    } else if matches!(ch, b',' | b';' | b'{' | b'}' | b':' | b'@') {
        i += 1;
        Tok::Sep
    } else if let Some(op) = OPS.iter().find(|op| b[i..].starts_with(op.as_bytes())) {
        i += op.len();
        if b.get(i) == Some(&b'=') && !matches!(*op, "==" | "!=" | "<=" | ">=") {
            i += 1;
            Tok::Sep // compound assignment
        } else {
            Tok::Op(op)
        }
    } else if ch == b'=' {
        i += 1;
        Tok::Sep
    } else {
        i += 1;
        Tok::Prefix
    };
    *at = i;
    tok
}

/// Records a binary operator at the current parenthesis level, reporting it if it mixes with
/// one already seen there.
fn note_operator<'a>(level: &mut Vec<&'a str>, op: &'a str, line: usize, out: &mut Vec<(usize, String)>) {
    let mixes = |a: &str, b: &str| a != b && (BITWISE.contains(&a) || BITWISE.contains(&b));
    if let Some(other) = level.iter().find(|o| mixes(o, op)) {
        out.push((line, format!("mixing '{other}' and '{op}' requires parentheses")));
    }
    level.push(op);
}

/// `(line, message)` for every bitwise operator mixed with another binary operator at
/// the same parenthesis level.
fn violations(src: &str) -> Vec<(usize, String)> {
    let s = strip(src);
    let b = s.as_bytes();
    let mut out = vec![];
    // Binary operators seen at each open parenthesis level.
    let mut levels: Vec<Vec<&str>> = vec![vec![]];
    let mut prev = Tok::Sep;
    let (mut i, mut line) = (0, 1);
    while i < b.len() {
        let ch = b[i];
        if ch == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let tok = next_token(&s, &mut i);
        match tok {
            Tok::Open => levels.push(vec![]),
            Tok::Close => {
                if levels.len() > 1 {
                    levels.pop();
                }
            }
            Tok::Sep => {
                if let Some(l) = levels.last_mut() {
                    l.clear();
                }
            }
            Tok::Op(op) if matches!(prev, Tok::Operand | Tok::Close) => {
                let level = levels.last_mut().expect("level");
                note_operator(level, op, line, &mut out);
            }
            _ => {}
        }
        prev = tok;
    }
    out
}

#[test]
fn checker_flags_unparenthesised_bitwise_mixes() {
    assert_eq!(violations("let h = u32(x) * 3u ^ u32(y) * 5u;").len(), 2);
    assert_eq!(violations("let a = b & c == d;").len(), 1);
    assert!(violations("let h = (u32(x) * 3u) ^ (u32(y) * 5u);").is_empty());
    assert!(violations("let h = a ^ b ^ c; let m = f32(h & 0xFFFFu) * (1.0 / 65536.0) - 0.5;").is_empty());
    assert!(violations("let v: vec3<f32> = vec3<f32>(1e-6, -x, a < b); if (a < b && c > d) { h ^= h >> 16u; }").is_empty());
    assert!(violations("var p = &x; let y = -a & b;").is_empty());
}

#[test]
fn shaders_parenthesise_bitwise_operators() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
    let mut bad = vec![];
    let mut files = 0;
    for entry in fs::read_dir(&dir).expect("shader dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "wgsl") {
            files += 1;
            let src = fs::read_to_string(&path).expect("shader source");
            for (line, msg) in violations(&src) {
                bad.push(format!("{}:{line}: {msg}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
            }
        }
    }
    assert!(files > 10, "found the shaders: {files}");
    assert!(bad.is_empty(), "WGSL a strict front-end rejects:\n{}", bad.join("\n"));
}
