//! roxmltree helpers.

use roxmltree::Node;

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
