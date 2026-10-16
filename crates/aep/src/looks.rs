//! Bounded reader for Magic Bullet Looks' serialized MBL2 state in an AEP.
//! Tool parameters stay raw until their render semantics are calibrated.

use effectcraft_keyframe::Value;
use effectcraft_project::{Node, ParamUi, Project, PropGroup, Property};

const MAX_STATE: usize = 1024 * 1024;
const MAX_TOOLS: usize = 64;
const MAX_PARAMS: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LookTool {
    pub code: [u8; 4],
    pub version: u32,
    pub params: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LookState {
    /// Exact MBL2 payload, including the embedded thumbnail and trailing bytes.
    pub raw: Vec<u8>,
    pub version: u32,
    pub tools: Vec<LookTool>,
    pub preview_size: Option<(u32, u32)>,
}

fn u32be(b: &[u8]) -> Result<u32, String> {
    let bytes: [u8; 4] = b.try_into().map_err(|_| "truncated MBL2 integer")?;
    Ok(u32::from_be_bytes(bytes))
}

pub(super) fn parse_hex(encoded: &str) -> Result<LookState, String> {
    if encoded.len() > MAX_STATE * 2 || !encoded.len().is_multiple_of(2) {
        return Err("invalid or oversized MBL2 hex state".into());
    }
    fn nibble(b: u8) -> Result<u8, String> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            b'A'..=b'F' => Ok(b - b'A' + 10),
            _ => Err("invalid MBL2 hex digit".into()),
        }
    }
    let mut raw = Vec::with_capacity(encoded.len() / 2);
    for pair in encoded.as_bytes().as_chunks::<2>().0 {
        raw.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    parse(&raw)
}

/// Parse a complete MBL2 payload. The final four bytes are an opaque trailer.
pub(super) fn parse(raw: &[u8]) -> Result<LookState, String> {
    if raw.len() < 12 || raw.len() > MAX_STATE || !raw.starts_with(b"MBL2") {
        return Err("invalid or oversized MBL2 state".into());
    }
    let version = u32be(&raw[4..8])?;
    if version != 18 {
        return Err(format!("unsupported MBL2 version {version}"));
    }
    let mut at = 8usize;
    let mut tools = Vec::new();
    let mut declared_tools = None;
    let mut preview_size = None;
    while at + 8 <= raw.len() {
        let tag = &raw[at..at + 4];
        let size = u32be(&raw[at + 4..at + 8])? as usize;
        let start = at + 8;
        let end = start.checked_add(size).filter(|end| *end <= raw.len()).ok_or("truncated MBL2 chunk")?;
        let value = &raw[start..end];
        match tag {
            b"#tls" => {
                let n = u32be(value)? as usize;
                if n > MAX_TOOLS {
                    return Err("too many MBL2 tools".into());
                }
                declared_tools = Some(n);
            }
            b"imag" => {
                if value.len() < 12 {
                    return Err("short MBL2 preview".into());
                }
                let (w, h, stride) = (u32be(&value[0..4])?, u32be(&value[4..8])?, u32be(&value[8..12])?);
                let expected = (h as usize).checked_mul(stride as usize).and_then(|n| n.checked_add(12)).ok_or("oversized MBL2 preview")?;
                if w == 0 || h == 0 || stride < w.saturating_mul(4) || expected != value.len() {
                    return Err("invalid MBL2 preview dimensions".into());
                }
                preview_size = Some((w, h));
            }
            b"tool" => {
                if tools.len() == MAX_TOOLS || value.len() < 12 {
                    return Err("invalid MBL2 tool".into());
                }
                let code: [u8; 4] = value[0..4].try_into().map_err(|_| "short MBL2 tool code")?;
                let version = u32be(&value[4..8])?;
                let count = u32be(&value[8..12])? as usize;
                if count > MAX_PARAMS || count.checked_mul(8).and_then(|n| n.checked_add(12)) != Some(value.len()) {
                    return Err("invalid MBL2 parameter vector".into());
                }
                let mut params = Vec::with_capacity(count);
                for bytes in value[12..].as_chunks::<8>().0 {
                    let v = f64::from_be_bytes(*bytes);
                    if !v.is_finite() {
                        return Err("nonfinite MBL2 parameter".into());
                    }
                    params.push(v);
                }
                tools.push(LookTool { code, version, params });
            }
            _ => {}
        }
        at = end;
    }
    if raw.len() - at != 4 || declared_tools != Some(tools.len()) {
        return Err("incomplete MBL2 tool chain".into());
    }
    Ok(LookState { raw: raw.to_vec(), version, tools, preview_size })
}

impl LookState {
    /// An editable raw tool tree. This group deliberately has no native renderer.
    pub(super) fn editable_group(&self, project: &mut Project, name: &str, losses: &mut Vec<String>, context: &str) -> PropGroup {
        losses.push(format!("{context}: MB LookSuite3 raw tools are editable but have no calibrated native renderer"));
        let mut group = PropGroup::new(project.alloc(), "MB LookSuite3", name);
        let mut hex = String::with_capacity(self.raw.len() * 2);
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for b in &self.raw {
            hex.push(DIGITS[(b >> 4) as usize] as char);
            hex.push(DIGITS[(b & 15) as usize] as char);
        }
        let mut original = Property::new(project.alloc(), "mbl2Raw", "Original MBL2 State", Value::Str(hex));
        original.ui = ParamUi::Hidden;
        original.static_only = true;
        group.children.push(Node::Prop(original));
        for (index, tool) in self.tools.iter().enumerate() {
            let code = String::from_utf8_lossy(&tool.code);
            let mut node = PropGroup::new(
                project.alloc(),
                &format!("mbl2Tool{index}"),
                &format!(
                    "{} ({code})",
                    match &tool.code {
                        b"spex" => "Spot Exposure",
                        b"lenv" => "Lens Vignette",
                        _ => "Unknown Looks Tool",
                    }
                ),
            );
            node.children.push(Node::Prop(Property::new(project.alloc(), "rawCode", "Raw Code", Value::Str(code.into_owned())).with_ui(ParamUi::Hidden)));
            node.children
                .push(Node::Prop(Property::new(project.alloc(), "rawVersion", "Raw Version", Value::Scalar(tool.version as f64)).with_ui(ParamUi::Hidden)));
            for (n, value) in tool.params.iter().enumerate() {
                node.children.push(Node::Prop(Property::new(project.alloc(), &format!("p{n:02}"), &format!("Raw Parameter {n:02}"), Value::Scalar(*value))));
            }
            group.children.push(Node::Group(node));
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_raw_tools_and_rejects_truncation() {
        let mut raw = b"MBL2".to_vec();
        raw.extend_from_slice(&18u32.to_be_bytes());
        raw.extend_from_slice(b"#tls\0\0\0\x04\0\0\0\x02");
        for (code, values) in [(b"spex", &[0.2_f64, 0.0][..]), (b"lenv", &[50.0_f64, 1.0][..])] {
            raw.extend_from_slice(b"tool");
            raw.extend_from_slice(&(12u32 + values.len() as u32 * 8).to_be_bytes());
            raw.extend_from_slice(code);
            raw.extend_from_slice(&0u32.to_be_bytes());
            raw.extend_from_slice(&(values.len() as u32).to_be_bytes());
            for value in values {
                raw.extend_from_slice(&value.to_be_bytes());
            }
        }
        raw.extend_from_slice(&[0; 4]);
        let state = parse(&raw).unwrap();
        assert_eq!(state.tools.len(), 2);
        assert_eq!(state.tools[0].params, [0.2, 0.0]);
        let mut project = Project::default();
        let mut losses = Vec::new();
        let group = state.editable_group(&mut project, "Looks", &mut losses, "test");
        assert_eq!(group.sub("mbl2Tool0").and_then(|g| g.get("p00")).map(|p| p.value.as_f64()), Some(0.2));
        assert_eq!(group.sub("mbl2Tool1").and_then(|g| g.get("rawCode")).map(|p| &p.value), Some(&Value::Str("lenv".into())));
        assert_eq!(losses.len(), 1);
        assert!(parse(&raw[..raw.len() - 1]).is_err());
        let encoded: String =
            raw.iter().flat_map(|b| [b"0123456789abcdef"[(b >> 4) as usize] as char, b"0123456789abcdef"[(b & 15) as usize] as char]).collect();
        assert_eq!(parse_hex(&encoded).unwrap().raw, raw);
        assert!(parse_hex("4d424c3g").is_err());
    }

    #[test]
    fn corpus_looks_state_when_supplied() {
        let Ok(path) = std::env::var("EFFECTCRAFT_LOOKS_IR") else { return };
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        fn look(node: &serde_json::Value) -> Option<&str> {
            if node.get("match_name").and_then(serde_json::Value::as_str) == Some("MB LookSuite3") {
                return node.get("mbl2_state_hex").and_then(serde_json::Value::as_str);
            }
            match node {
                serde_json::Value::Array(a) => a.iter().find_map(look),
                serde_json::Value::Object(o) => o.values().find_map(look),
                _ => None,
            }
        }
        let state = parse_hex(look(&doc).unwrap()).unwrap();
        assert_eq!(state.raw.len(), 28_729);
        assert_eq!(state.preview_size, Some((147, 48)));
        assert_eq!(state.tools.iter().map(|t| t.code).collect::<Vec<_>>(), [*b"spex", *b"lenv"]);
        assert_eq!(state.tools[0].params[0], 0.2);
        assert_eq!(state.tools[1].params, [50.0, 50.0, 50.0, 0.0, 0.0, 1.0, 0.0]);
    }
}
