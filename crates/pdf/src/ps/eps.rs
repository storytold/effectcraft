use super::{HashMap, Rc, is_white};

/// The PostScript section of an EPS file (DOS EPS binary header handled) and its bounding box
/// (`%%HiResBoundingBox` preferred), read with `craft-eps`: the header comments, and the trailer
/// for a box deferred with `(atend)`. A binary header that points outside the file is refused.
pub fn eps_parts(bytes: &[u8]) -> Option<(&[u8], [f64; 4])> {
    let ps = craft_eps::sections(bytes).ok()?.postscript;
    Some((ps, craft_eps::Comments::read(ps).page_box()?))
}

pub(super) fn find_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Every position of `needle` in `hay`, in order (one pass over the file).
fn positions(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(p) = find_from(hay, needle, from) {
        out.push(p);
        from = p + needle.len();
    }
    out
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
    // The headers are found once, in one pass each; every `eexec` then takes the last header
    // before it that no earlier `eexec` has used, so no prefix of the file is searched again.
    let mut heads = positions(ps, b"%!PS-AdobeFont");
    heads.extend(positions(ps, b"%!FontType1"));
    heads.sort_unstable();
    let names = positions(ps, b"/FontName");
    let mut from = 0;
    let mut used = 0;
    while let Some(ee) = find_from(ps, b"eexec", from) {
        from = ee + 5;
        let last_before = |v: &[usize]| v[..v.partition_point(|&p| p < ee)].last().copied().filter(|&p| p >= used);
        // The fallback look-back stops at the previous font's end, so it cannot pick up its header.
        let start = last_before(&heads).or_else(|| last_before(&names).map(|p| p.saturating_sub(512).max(used)));
        let Some(start) = start else { continue };
        let end = find_from(ps, b"cleartomark", ee).map_or(ps.len(), |p| p + 11);
        if let Some(name) = name_after(&ps[start..ee], b"/FontName") {
            out.insert(name, (Rc::new(ps[start..end].to_vec()), false));
        }
        from = end;
        used = end;
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

#[cfg(test)]
mod tests {
    use super::embedded_fonts;

    #[test]
    fn font_discovery_looks_back_once_per_header() {
        // 20,000 `eexec` words before any font header: each used to search the whole prefix
        // for a header again. The font after them is still found.
        let mut ps = b"%!PS-Adobe-3.0 EPSF-3.0\n".to_vec();
        ps.extend_from_slice("eexec ".repeat(20_000).as_bytes());
        ps.extend_from_slice(b"\n%!PS-AdobeFont-1.0: Real\n/FontName /Real def\ncurrentfile eexec\n0000\ncleartomark\n");
        let fonts = embedded_fonts(&ps);
        assert_eq!(fonts.len(), 1);
        assert!(fonts.contains_key("Real"));
    }

    #[test]
    fn header_less_font_after_a_short_one_does_not_take_its_header() {
        let a = "%!PS-AdobeFont-1.0: A\n/FontName /A def\ncurrentfile eexec\n0000\ncleartomark\n";
        let b = "/FontName /B def\ncurrentfile eexec\n0000\ncleartomark\n";
        let ps = format!("%!PS-Adobe-3.0 EPSF-3.0\n{a}{b}");
        let fonts = embedded_fonts(ps.as_bytes());
        let (data, _) = fonts.get("B").expect("second font found");
        assert!(!String::from_utf8_lossy(data).contains("AdobeFont"), "{}", String::from_utf8_lossy(data));
    }
}
