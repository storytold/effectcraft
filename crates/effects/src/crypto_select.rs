//! Editing a Cryptomatte effect's Selection text from a pick in the viewer (#473): replace it
//! with the picked ID, add it, or remove it, in the grammar `selection_hashes` reads.

use crate::channel3d::{selection_hashes, selection_names};

/// How a pick changes the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickMode {
    Replace,
    Add,
    Remove,
}

/// A name as a selection token: quoted (with `\` escapes) when it has a comma, a quote or a
/// backslash, which the selection parser would otherwise split on or read as syntax.
fn quote(name: &str) -> String {
    if !name.contains([',', '"', '\'', '\\']) {
        return name.to_string();
    }
    let mut out = String::from("\"");
    for c in name.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The selection token for the ID `hash` named `name` in `manifest`: the (quoted) name when it
/// reads back as exactly that ID, else `<hex>`. That covers unnamed IDs, names with `*` (which
/// would select by wildcard), names that look like `<hex>`, names with surrounding spaces (the
/// parser trims them) and names shared by several IDs.
pub fn token_for(name: Option<&str>, hash: u32, manifest: Option<&[(String, u32)]>) -> String {
    match name {
        Some(n) if !n.is_empty() && !n.contains('*') && selection_hashes(&quote(n), manifest) == [hash] => quote(n),
        _ => format!("<{hash:08x}>"),
    }
}

/// `sel` edited by a pick of the ID `hash` whose token is `token` (see [`token_for`]). The text
/// stays as typed when the edit wouldn't change what it selects: Replace when it selects just
/// that ID, Add when it already selects it, Remove when no name or `<hex>` token selects it (a
/// wildcard that matches it stays). Other tokens (wildcards included) keep their text,
/// re-quoted where needed, separated by `, `.
pub fn edit_selection(sel: &str, token: &str, hash: u32, mode: PickMode, manifest: Option<&[(String, u32)]>) -> String {
    let tokens: Vec<String> = selection_names(sel).into_iter().filter(|t| !t.is_empty()).collect();
    let selects = |t: &String| !t.contains('*') && selection_hashes(&quote(t), manifest).contains(&hash);
    let kept: Vec<String> = match mode {
        PickMode::Replace if selection_hashes(sel, manifest) == [hash] => return sel.to_string(),
        PickMode::Replace => return token.to_string(),
        PickMode::Add if selection_hashes(sel, manifest).contains(&hash) => return sel.to_string(),
        PickMode::Add => tokens.iter().map(|t| quote(t)).chain([token.to_string()]).collect(),
        PickMode::Remove if !tokens.iter().any(selects) => return sel.to_string(),
        PickMode::Remove => tokens.iter().filter(|t| !selects(t)).map(|t| quote(t)).collect(),
    };
    kept.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use effectcraft_raster::channels3d::crypto_hash;

    fn manifest() -> Vec<(String, u32)> {
        ["bunny", "Green, Glossy", r#"say "hi""#, r"back\slash", "  pad ", "star*", "<0000abcd>", "twin"]
            .iter()
            .map(|n| (n.to_string(), crypto_hash(n)))
            .chain([("twin".to_string(), 7)])
            .collect()
    }

    /// Every name's token reads back as exactly its ID, through the effect's own parser.
    #[test]
    fn tokens_round_trip_through_selection_hashes() {
        let m = manifest();
        for (n, h) in &m {
            let t = token_for(Some(n), *h, Some(&m));
            assert_eq!(selection_hashes(&t, Some(&m)), [*h], "{n:?} → {t:?}");
        }
        assert_eq!(token_for(Some("bunny"), crypto_hash("bunny"), Some(&m)), "bunny");
        assert_eq!(token_for(Some("Green, Glossy"), crypto_hash("Green, Glossy"), Some(&m)), r#""Green, Glossy""#);
        assert_eq!(token_for(Some(r#"say "hi""#), crypto_hash(r#"say "hi""#), Some(&m)), r#""say \"hi\"""#);
        // Wildcard-looking, hash-looking, empty, unnamed and shared names: the hash.
        assert_eq!(token_for(Some("star*"), crypto_hash("star*"), Some(&m)), format!("<{:08x}>", crypto_hash("star*")));
        assert_eq!(token_for(Some("<0000abcd>"), crypto_hash("<0000abcd>"), Some(&m)), format!("<{:08x}>", crypto_hash("<0000abcd>")));
        assert_eq!(token_for(Some(""), 5, Some(&m)), "<00000005>");
        assert_eq!(token_for(Some("  pad "), crypto_hash("  pad "), Some(&m)), format!("<{:08x}>", crypto_hash("  pad ")));
        assert_eq!(token_for(None, 0xdead_beef, None), "<deadbeef>");
        assert_eq!(token_for(Some("twin"), 7, Some(&m)), "<00000007>", "the name selects the other twin");
    }

    #[test]
    fn add_and_remove_keep_other_tokens_and_skip_duplicates() {
        let m = manifest();
        let (b, g) = (crypto_hash("bunny"), crypto_hash("Green, Glossy"));
        let tok = |n: &str| token_for(Some(n), crypto_hash(n), Some(&m));
        let edit = |sel: &str, n: &str, mode| edit_selection(sel, &tok(n), crypto_hash(n), mode, Some(&m));
        assert_eq!(edit("anything, else", "bunny", PickMode::Replace), "bunny");
        assert_eq!(edit(" bunny ", "bunny", PickMode::Replace), " bunny ", "same meaning: unchanged");
        assert_eq!(edit("", "bunny", PickMode::Add), "bunny");
        assert_eq!(edit("bunny", "Green, Glossy", PickMode::Add), r#"bunny, "Green, Glossy""#);
        // Already selected: by name, by hash, by a wildcard. The text stays as typed.
        assert_eq!(edit("bunny,floor", "bunny", PickMode::Add), "bunny,floor");
        assert_eq!(edit(&format!("<{b:08x}>"), "bunny", PickMode::Add), format!("<{b:08x}>"));
        assert_eq!(edit("bun*", "bunny", PickMode::Add), "bun*");
        // Other tokens keep their meaning: quoted where needed, wildcards stay wildcards.
        assert_eq!(edit(r#"'a, b', fl*, "x, *""#, "bunny", PickMode::Add), r#""a, b", fl*, "x, *", bunny"#);
        assert_eq!(edit(r#""Green, Glossy", bunny"#, "bunny", PickMode::Remove), r#""Green, Glossy""#);
        assert_eq!(edit(&format!("bunny, <{b:08x}>, x, bunny"), "bunny", PickMode::Remove), "x");
        assert_eq!(edit(&format!("<{g:08x}>"), "Green, Glossy", PickMode::Remove), "");
        // Not selected, or only by a wildcard: unchanged.
        assert_eq!(edit("a ,b", "bunny", PickMode::Remove), "a ,b");
        assert_eq!(edit("bun*, x", "bunny", PickMode::Remove), "bun*, x");
        for sel in [r#""a, b", fl*, bunny"#, r#"x, "Green, Glossy", "say \"hi\"", "back\\slash""#] {
            let before = selection_hashes(sel, Some(&m));
            let after = selection_hashes(&edit(sel, "  pad ", PickMode::Add), Some(&m));
            assert_eq!(after, [before, vec![crypto_hash("  pad ")]].concat(), "{sel}");
        }
    }
}
