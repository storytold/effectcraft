use super::{HashMap, Rc};

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

/// Embedded font programs of an EPS file: Type 1 fonts (`… /FontName /X … currentfile eexec
/// … cleartomark`, hexadecimal or binary) and CFF font sets (`… <length> StartData <bytes>`).
pub(super) fn embedded_fonts(ps: &[u8]) -> HashMap<String, (Rc<Vec<u8>>, bool)> {
    let mut out = HashMap::new();
    for font in craft_eps::embedded_font_programs(ps) {
        let name = if font.cff { crate::cff::font_name(&font.program) } else { font.name };
        if let Some(name) = name {
            out.insert(name, (Rc::new(font.program), font.cff));
        }
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
