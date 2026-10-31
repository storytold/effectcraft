//! A small indenting XML writer and roxmltree helpers.

use roxmltree::Node;

/// Escape text for an attribute value or element content.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 forbids are dropped.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// A number as Motion writes it (no exponent, no trailing zeros, `-0` → `0`).
pub(crate) fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 {
        return "0".into();
    }
    let s = format!("{r:.9}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

pub(crate) struct Writer {
    out: String,
    depth: usize,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::at_depth(0)
    }

    /// A writer whose elements start `depth` tabs in (a fragment spliced in later).
    pub fn at_depth(depth: usize) -> Writer {
        Writer { out: String::new(), depth }
    }

    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push('\t');
        }
    }

    fn attrs(&mut self, attrs: &[(&str, String)]) {
        for (k, v) in attrs {
            self.out.push(' ');
            self.out.push_str(k);
            self.out.push_str("=\"");
            self.out.push_str(&esc(v));
            self.out.push('"');
        }
    }

    pub fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    pub fn open(&mut self, name: &str, attrs: &[(&str, String)]) {
        self.indent();
        self.out.push('<');
        self.out.push_str(name);
        self.attrs(attrs);
        self.out.push_str(">\n");
        self.depth += 1;
    }

    pub fn close(&mut self, name: &str) {
        self.depth = self.depth.saturating_sub(1);
        self.indent();
        self.out.push_str("</");
        self.out.push_str(name);
        self.out.push_str(">\n");
    }

    pub fn empty(&mut self, name: &str, attrs: &[(&str, String)]) {
        self.indent();
        self.out.push('<');
        self.out.push_str(name);
        self.attrs(attrs);
        self.out.push_str("/>\n");
    }

    pub fn text(&mut self, name: &str, text: &str) {
        self.indent();
        self.out.push_str(&format!("<{name}>{}</{name}>\n", esc(text)));
    }

    pub fn finish(self) -> String {
        self.out
    }
}

/// Child elements of `n` with tag `name`.
pub(crate) fn children<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

pub(crate) fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Trimmed text of child element `name`.
pub(crate) fn child_text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name).and_then(|c| c.text()).map(str::trim)
}

pub(crate) fn child_f64(n: Node, name: &str) -> Option<f64> {
    child_text(n, name).and_then(|t| t.parse().ok()).filter(|v: &f64| v.is_finite())
}

pub(crate) fn attr_f64(n: Node, name: &str) -> Option<f64> {
    n.attribute(name).and_then(|t| t.trim().parse().ok()).filter(|v: &f64| v.is_finite())
}

/// The `parameter` child of `n` named `name`.
pub(crate) fn param<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == "parameter" && c.attribute("name") == Some(name))
}

/// Follow a path of parameter names (`["Properties", "Transform", "Position"]`).
pub(crate) fn param_path<'a, 'i>(n: Node<'a, 'i>, path: &[&str]) -> Option<Node<'a, 'i>> {
    let mut cur = n;
    for p in path {
        cur = param(cur, p)?;
    }
    Some(cur)
}
