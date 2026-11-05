//! File ▸ Save a Copy As XML…: the project as an XML document (`.ecprojx`), the counterpart of
//! After Effects' `.aepx`. It is a lossless, element-per-value encoding of the `.ecproj` JSON, so
//! File ▸ Open reads it back (and saving to a `.ecprojx` path writes XML again):
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <EffectCraftProject encoding="json" version="1">
//!   <obj>
//!     <m n="version"><num>3</num></m>
//!     <m n="items"><arr>…</arr></m>
//!   </obj>
//! </EffectCraftProject>
//! ```
//!
//! `obj`/`m n="key"` (object members), `arr`, `str`, `num`, `bool`, `null`.

use serde_json::{Map, Value};

const ROOT: &str = "EffectCraftProject";

/// How deep `arr`/`obj` elements may nest: the JSON reader's own limit (`serde_json` stops at
/// 128), so every project the JSON form can hold reads from XML too. Without a limit a deeply
/// nested file overflowed the stack and aborted the app instead of failing to open (#779).
const MAX_DEPTH: usize = 128;

/// Whether `text` looks like an XML project.
pub fn is_xml(text: &str) -> bool {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with("<?xml") || t.starts_with(&format!("<{ROOT}"))
}

/// Whether a path names an XML project copy.
pub fn is_xml_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".ecprojx") || p.ends_with(".xml")
}

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => out.push_str(&format!("&#{};", c as u32)),
            c => out.push(c),
        }
    }
}

fn write(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    out.push_str(&pad);
    match v {
        Value::Null => out.push_str("<null/>"),
        Value::Bool(b) => out.push_str(&format!("<bool>{b}</bool>")),
        Value::Number(n) => out.push_str(&format!("<num>{n}</num>")),
        Value::String(s) => {
            out.push_str("<str>");
            esc(s, out);
            out.push_str("</str>");
        }
        Value::Array(a) if a.is_empty() => out.push_str("<arr/>"),
        Value::Array(a) => {
            out.push_str("<arr>\n");
            for x in a {
                write(x, depth + 1, out);
            }
            out.push_str(&pad);
            out.push_str("</arr>");
        }
        Value::Object(m) if m.is_empty() => out.push_str("<obj/>"),
        Value::Object(m) => {
            out.push_str("<obj>\n");
            for (k, x) in m {
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str("<m n=\"");
                esc(k, out);
                out.push_str("\">\n");
                write(x, depth + 2, out);
                out.push_str(&"  ".repeat(depth + 1));
                out.push_str("</m>\n");
            }
            out.push_str(&pad);
            out.push_str("</obj>");
        }
    }
    out.push('\n');
}

/// The project JSON as an XML document.
pub fn to_xml(json: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let mut out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<{ROOT} encoding=\"json\" version=\"1\">\n");
    write(&v, 1, &mut out);
    out.push_str(&format!("</{ROOT}>\n"));
    Ok(out)
}

struct P<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn err<T>(&self, m: &str) -> Result<T, String> {
        Err(format!("XML project: {m} at byte {}", self.i))
    }
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    /// Skip whitespace, comments and processing instructions.
    fn skip(&mut self) {
        loop {
            let t = self.rest();
            let trimmed = t.trim_start();
            self.i += t.len() - trimmed.len();
            if trimmed.starts_with("<?") {
                self.i += trimmed.find("?>").map_or(trimmed.len(), |e| e + 2);
            } else if trimmed.starts_with("<!--") {
                self.i += trimmed.find("-->").map_or(trimmed.len(), |e| e + 3);
            } else {
                return;
            }
        }
    }
    /// An opening tag: (name, attribute `n`, self-closing).
    fn open(&mut self) -> Result<(String, Option<String>, bool), String> {
        self.skip();
        if !self.rest().starts_with('<') || self.rest().starts_with("</") {
            return self.err("expected an element");
        }
        let end = self.rest().find('>').ok_or("XML project: unterminated tag")?;
        let inner = &self.rest()[1..end];
        self.i += end + 1;
        let (inner, closed) = match inner.strip_suffix('/') {
            Some(x) => (x, true),
            None => (inner, false),
        };
        let mut parts = inner.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or_default().to_string();
        let attrs = parts.next().unwrap_or("");
        let n = attrs.find("n=\"").map(|a| {
            let v = &attrs[a + 3..];
            unesc(&v[..v.find('"').unwrap_or(v.len())])
        });
        Ok((name, n, closed))
    }
    fn close(&mut self, name: &str) -> Result<(), String> {
        self.skip();
        let want = format!("</{name}>");
        if self.rest().starts_with(&want) {
            self.i += want.len();
            Ok(())
        } else {
            self.err(&format!("expected {want}"))
        }
    }
    fn text(&mut self) -> String {
        let end = self.rest().find('<').unwrap_or(self.rest().len());
        let t = unesc(&self.rest()[..end]);
        self.i += end;
        t
    }
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return self.err(&format!("elements nested deeper than {MAX_DEPTH} levels"));
        }
        let (name, _, closed) = self.open()?;
        let v = match (name.as_str(), closed) {
            ("null", _) => Value::Null,
            ("arr", true) => Value::Array(vec![]),
            ("obj", true) => Value::Object(Map::new()),
            ("str", true) => Value::String(String::new()),
            ("bool", false) => Value::Bool(self.text().trim() == "true"),
            ("num", false) => serde_json::from_str(self.text().trim()).map_err(|e| format!("XML project: bad number: {e}"))?,
            ("str", false) => Value::String(self.text()),
            ("arr", false) => {
                let mut a = vec![];
                loop {
                    self.skip();
                    if self.rest().starts_with("</") {
                        break;
                    }
                    a.push(self.value(depth + 1)?);
                }
                Value::Array(a)
            }
            ("obj", false) => {
                let mut m = Map::new();
                loop {
                    self.skip();
                    if self.rest().starts_with("</") {
                        break;
                    }
                    let (tag, key, _) = self.open()?;
                    if tag != "m" {
                        return self.err("expected <m n=\"…\">");
                    }
                    let v = self.value(depth + 1)?;
                    self.close("m")?;
                    m.insert(key.unwrap_or_default(), v);
                }
                Value::Object(m)
            }
            (other, _) => return self.err(&format!("unexpected <{other}>")),
        };
        if !closed && name != "null" {
            self.close(&name)?;
        }
        Ok(v)
    }
}

fn unesc(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        rest = &rest[a..];
        let Some(e) = rest.find(';') else { break };
        let ent = &rest[1..e];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            n if n.starts_with("#x") => out.extend(u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32)),
            n if n.starts_with('#') => out.extend(n[1..].parse::<u32>().ok().and_then(char::from_u32)),
            other => {
                out.push('&');
                out.push_str(other);
                out.push(';');
            }
        }
        rest = &rest[e + 1..];
    }
    out.push_str(rest);
    out
}

/// The project JSON of an XML project document.
pub fn from_xml(text: &str) -> Result<String, String> {
    let mut p = P { s: text.trim_start_matches('\u{feff}'), i: 0 };
    let (root, _, _) = p.open()?;
    if root != ROOT {
        return Err(format!("not an EffectCraft XML project (root <{root}>)"));
    }
    let v = p.value(0)?;
    p.close(ROOT)?;
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_through_xml() {
        let j = serde_json::json!({"a": [1, 2.5, -3e-7, true, null, "x<y & \"z\"\n\tq"], "b": {}, "c": [], "d": "", "é": {"n": "ünï"}});
        let xml = to_xml(&j.to_string()).unwrap();
        assert!(is_xml(&xml));
        assert!(xml.contains("<m n=\"a\">"));
        let back: Value = serde_json::from_str(&from_xml(&xml).unwrap()).unwrap();
        assert_eq!(back, j);
        assert!(from_xml("<Other/>").is_err());
    }

    /// #779 (found by the project-file fuzzer): a file of deeply nested elements overflowed the
    /// stack, which aborts the app; it is now refused. Nesting the JSON reader takes still reads.
    #[test]
    fn deep_nesting_is_refused_not_a_stack_overflow() {
        let nested = |n: usize| format!("<{ROOT}>{}<null/>{}</{ROOT}>", "<arr>".repeat(n), "</arr>".repeat(n));
        let e = from_xml(&nested(1_000_000)).unwrap_err();
        assert!(e.contains("nested deeper"), "{e}");
        let ok = from_xml(&nested(100)).unwrap();
        assert!(serde_json::from_str::<Value>(&ok).is_ok());
        let json = format!("{}null{}", "[".repeat(MAX_DEPTH - 1), "]".repeat(MAX_DEPTH - 1));
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&from_xml(&to_xml(&json).unwrap()).unwrap()).unwrap(), v);
    }
}
