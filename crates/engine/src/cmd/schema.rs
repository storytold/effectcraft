use super::params::bad;
use super::{CommandSpec, Result, Value};

// ---------- parameter validation (agents) ----------

/// Top-level keys accepted by a params doc such as `{layers: [id|name|#n], add?, toggle?}` or
/// `{time? (s) | frame? | timecode?}`; `None` when the doc isn't a `{…}` key list.
pub fn accepted_params(doc: &str) -> Option<Vec<String>> {
    let body = doc.trim().strip_prefix('{')?;
    // Top-level pieces up to the matching close brace, split at `,` and `|`; `:` starts a value.
    let (mut keys, mut cur, mut depth, mut in_value) = (Vec::new(), String::new(), 0i32, false);
    let push = |cur: &mut String, keys: &mut Vec<String>| {
        let k: String = cur.trim().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
        cur.clear();
    };
    for c in body.chars() {
        match c {
            '{' | '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            '}' if depth == 0 => {
                push(&mut cur, &mut keys);
                return Some(keys);
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&mut cur, &mut keys);
                in_value = false;
            }
            '|' if depth == 0 && !in_value => push(&mut cur, &mut keys),
            ':' if depth == 0 => {
                push(&mut cur, &mut keys);
                in_value = true;
            }
            _ if depth == 0 && !in_value => cur.push(c),
            _ => {}
        }
    }
    None
}

/// The JSON Schema of a command's parameters, derived from its params doc: one property per
/// accepted key (described by its piece of the doc), no others. MCP `describe_command` and
/// `list_commands {schemas: true}` serve it. The doc parser lives in `craft-command`.
pub fn params_schema(spec: &CommandSpec) -> Value {
    craft_command::schema::from_doc(spec.params).value
}

/// The same schema from a bare params doc (a registry row from `craft-command`, or a dynamic command).
pub fn params_schema_doc(doc: &str) -> Value {
    craft_command::schema::from_doc(doc).value
}

/// Same check as [`check_params`], for a command that has no local row (a registry row or a dynamic command).
pub fn check_params_doc(id: &str, doc: &str, params: &Value) -> Result<()> {
    let (Some(obj), Some(accepted)) = (params.as_object(), accepted_params(doc)) else { return Ok(()) };
    let ok = |k: &str| ALWAYS_OK.contains(&k) || accepted.iter().any(|a| a == k || ALIASES.iter().any(|(param, alias)| param == a && *alias == k));
    let unknown: Vec<&str> = obj.keys().map(String::as_str).filter(|k| !ok(k)).collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let list = if accepted.is_empty() { "none".to_string() } else { accepted.join(", ") };
    Err(bad(id, format!("unknown parameter(s) {}; accepted: {list} (doc: {doc})", unknown.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", "))))
}

/// Keys every command understands (comp targeting, gesture merging) and accepted aliases.
const ALWAYS_OK: &[&str] = &["comp", "merge"];

const ALIASES: &[(&str, &str)] = &[("layers", "layer"), ("layer", "layers"), ("time", "frame"), ("frameRate", "fps"), ("properties", "property")];

/// Reject unknown top-level parameters (typos, guessed names) with the list of accepted keys.
pub fn check_params(spec: &CommandSpec, params: &Value) -> Result<()> {
    check_params_doc(spec.id, spec.params, params)
}
