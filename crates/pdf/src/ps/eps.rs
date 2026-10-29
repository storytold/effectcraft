use super::{HashMap, Rc, is_white};

/// The PostScript section of an EPS file (DOS EPS binary header handled) and its bounding box
/// (`%%HiResBoundingBox` preferred).
pub fn eps_parts(bytes: &[u8]) -> Option<(&[u8], [f64; 4])> {
    let ps = if bytes.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) && bytes.len() >= 12 {
        let off = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
        bytes.get(off..off.checked_add(len)?.min(bytes.len()))?
    } else {
        bytes
    };
    let find_box = |key: &[u8]| -> Option<[f64; 4]> {
        let mut from = 0;
        while let Some(p) = ps[from..].windows(key.len()).position(|w| w == key) {
            let s = from + p + key.len();
            let e = ps[s..].iter().position(|c| matches!(c, b'\n' | b'\r')).map_or(ps.len(), |q| s + q);
            let v: Vec<f64> = String::from_utf8_lossy(&ps[s..e]).split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if v.len() == 4 && v[2] > v[0] && v[3] > v[1] {
                return Some([v[0], v[1], v[2], v[3]]);
            }
            from = s;
        }
        None
    };
    let bbox = find_box(b"%%HiResBoundingBox:").or_else(|| find_box(b"%%BoundingBox:"))?;
    Some((ps, bbox))
}

pub(super) fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn rfind_before(hay: &[u8], needle: &[u8], to: usize) -> Option<usize> {
    hay.get(..to)?.windows(needle.len()).rposition(|w| w == needle)
}

/// The name after `key` (`/FontName /Name`).
fn name_after(b: &[u8], key: &[u8]) -> Option<String> {
    let p = find_from(b, key, 0)? + key.len();
    let rest = &b[p..];
    let s = rest.iter().position(|c| *c == b'/')? + 1;
    let e = rest[s..].iter().position(|c| is_white(*c) || matches!(c, b'/' | b'[' | b'(' | b'{')).map_or(rest.len(), |q| s + q);
    (e > s).then(|| String::from_utf8_lossy(&rest[s..e]).into_owned())
}

/// Embedded font programs of an EPS file: Type 1 fonts (`… /FontName /X … currentfile eexec
/// … cleartomark`, hexadecimal or binary) and CFF font sets (`… <length> StartData <bytes>`).
pub(super) fn embedded_fonts(ps: &[u8]) -> HashMap<String, (Rc<Vec<u8>>, bool)> {
    let mut out = HashMap::new();
    let mut from = 0;
    while let Some(ee) = find_from(ps, b"eexec", from) {
        from = ee + 5;
        let start = [b"%!PS-AdobeFont".as_slice(), b"%!FontType1"]
            .iter()
            .filter_map(|h| rfind_before(ps, h, ee))
            .max()
            .or_else(|| rfind_before(ps, b"/FontName", ee).map(|p| p.saturating_sub(512)));
        let Some(start) = start else { continue };
        let end = find_from(ps, b"cleartomark", ee).map_or(ps.len(), |p| p + 11);
        if let Some(name) = name_after(&ps[start..ee], b"/FontName") {
            out.insert(name, (Rc::new(ps[start..end].to_vec()), false));
        }
        from = end;
    }
    let mut from = 0;
    while let Some(sd) = find_from(ps, b"StartData", from) {
        from = sd + 9;
        let head = String::from_utf8_lossy(&ps[sd.saturating_sub(64)..sd]).into_owned();
        let Some(len) = head.split_whitespace().last().and_then(|t| t.parse::<usize>().ok()) else { continue };
        let s = sd + 10;
        let Some(data) = ps.get(s..s.saturating_add(len)) else { continue };
        if let Some(name) = crate::cff::font_name(data) {
            out.insert(name, (Rc::new(data.to_vec()), true));
        }
        from = s + len;
    }
    out
}
