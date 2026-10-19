//! Minimal SFNT (TrueType / OpenType / collection) header reader for font discovery: reads only
//! the table directory, `name` and `OS/2` of each face, so scanning a system font folder does not
//! load whole files.

use std::io::{Read, Seek, SeekFrom};

/// Names and style attributes of one face in a font file.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceNames {
    pub index: u32,
    pub family: String,
    pub style: String,
    pub full_name: String,
    pub weight: u16,
    pub italic: bool,
    /// The family name in another language than English (CJK, Arabic… fonts), when the font
    /// has one (Settings ▸ Type ▸ Show Font Names in English off shows it).
    pub native_family: Option<String>,
    /// The PostScript name (name id 6, e.g. `YuGothic-Bold`): what After Effects scripts and
    /// `.aep`-derived data call a font.
    pub postscript: Option<String>,
    /// A variable font: it has an `fvar` table with at least one axis (the font menu's
    /// Variable Fonts filter).
    pub variable: bool,
    /// The named instances of a variable font (`fvar`): `Light`, `Black`, … with their axis
    /// values, so a style name or an instance's PostScript name selects that design-space position.
    pub instances: Vec<NamedInstance>,
}

/// One named instance of a variable font (an `fvar` instance record).
#[derive(Clone, Debug, PartialEq)]
pub struct NamedInstance {
    /// The subfamily name (`Black`, `Bold Italic`).
    pub style: String,
    /// The instance's PostScript name (`Figtree-Black`): the one the font names, or else the one
    /// Adobe Technical Note #5902 derives from the family's PostScript prefix and the style.
    pub postscript: String,
    /// Axis values in user units, one per axis, sorted by tag (the order `TextDoc` keeps).
    pub coords: Vec<(String, f32)>,
}

impl NamedInstance {
    /// The instance's weight class: the `wght` coordinate, or `fallback` without a weight axis.
    pub fn weight(&self, fallback: u16) -> u16 {
        self.coords.iter().find(|(t, _)| t == "wght").map_or(fallback, |(_, v)| v.round().clamp(1.0, 1000.0) as u16)
    }

    /// The instance is italic: its name says so, or its `ital` / `slnt` axis is off zero.
    pub fn italic(&self, fallback: bool) -> bool {
        let s = self.style.to_ascii_lowercase();
        let axis = |tag: &str| self.coords.iter().find(|(t, _)| t == tag).map(|(_, v)| *v);
        if s.contains("italic") || s.contains("oblique") || axis("ital").is_some_and(|v| v >= 0.5) || axis("slnt").is_some_and(|v| v != 0.0) {
            return true;
        }
        fallback && axis("ital").is_none() && axis("slnt").is_none()
    }
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(o)?, *b.get(o + 1)?, *b.get(o + 2)?, *b.get(o + 3)?]))
}

fn read_at<R: Read + Seek>(r: &mut R, off: u64, len: usize) -> Option<Vec<u8>> {
    if len > 4 << 20 {
        return None;
    }
    r.seek(SeekFrom::Start(off)).ok()?;
    let mut v = vec![0u8; len];
    r.read_exact(&mut v).ok()?;
    Some(v)
}

/// Read the names of every face in a font file (a single font or a collection).
pub fn read_faces<R: Read + Seek>(r: &mut R) -> Vec<FaceNames> {
    let Some(head) = read_at(r, 0, 12) else { return Vec::new() };
    let offsets: Vec<u32> = if &head[0..4] == b"ttcf" {
        let n = be32(&head, 8).unwrap_or(0).min(256) as usize;
        let Some(t) = read_at(r, 12, n * 4) else { return Vec::new() };
        (0..n).filter_map(|i| be32(&t, i * 4)).collect()
    } else {
        vec![0]
    };
    offsets.iter().enumerate().filter_map(|(i, &o)| read_face(r, o as u64, i as u32)).collect()
}

/// Names from in-memory font data.
pub fn read_faces_bytes(data: &[u8]) -> Vec<FaceNames> {
    read_faces(&mut std::io::Cursor::new(data))
}

fn read_face<R: Read + Seek>(r: &mut R, off: u64, index: u32) -> Option<FaceNames> {
    let h = read_at(r, off, 12)?;
    let tag = &h[0..4];
    if tag != [0, 1, 0, 0] && tag != b"OTTO" && tag != b"true" {
        return None;
    }
    let n = be16(&h, 4)? as usize;
    let dir = read_at(r, off + 12, n * 16)?;
    let mut name = None;
    let mut os2 = None;
    let mut fvar = None;
    for i in 0..n {
        let rec = &dir[i * 16..i * 16 + 16];
        let (o, l) = (be32(rec, 8)? as u64, be32(rec, 12)? as usize);
        match &rec[0..4] {
            b"name" => name = Some((o, l)),
            b"OS/2" => os2 = Some((o, l)),
            b"fvar" => fvar = Some((o, l)),
            _ => {}
        }
    }
    let (no, nl) = name?;
    let nb = read_at(r, no, nl)?;
    let get = |id: u16| name_string(&nb, id);
    let family = get(16).or_else(|| get(1))?;
    let style = get(17).or_else(|| get(2)).unwrap_or_else(|| "Regular".into());
    let full_name = get(4).unwrap_or_else(|| format!("{family} {style}"));
    let native_family = native_name(&nb, 16).or_else(|| native_name(&nb, 1)).filter(|n| *n != family);
    let postscript = get(6).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let (mut weight, mut italic) = (400u16, false);
    if let Some((oo, ol)) = os2
        && let Some(ob) = read_at(r, oo, ol.min(78))
    {
        weight = be16(&ob, 4).unwrap_or(400);
        italic = be16(&ob, 62).is_some_and(|fs| fs & 1 != 0);
    }
    let sl = style.to_ascii_lowercase();
    italic |= sl.contains("italic") || sl.contains("oblique");
    // fvar header: version (4), axesArrayOffset (2), reserved (2), axisCount (2).
    let variable = fvar.filter(|(_, l)| *l >= 10).and_then(|(fo, _)| read_at(r, fo, 10)).and_then(|fb| be16(&fb, 8)).is_some_and(|axes| axes > 0);
    let instances = if variable {
        let prefix = get(25).map(|p| ps_part(&p)).filter(|p| !p.is_empty()).unwrap_or_else(|| ps_part(&family));
        fvar.and_then(|(fo, fl)| read_at(r, fo, fl)).map(|fb| named_instances(&fb, &nb, &prefix)).unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(FaceNames { index, family, style, full_name, weight, italic, native_family, postscript, variable, instances })
}

/// Most named instances read from one face: real fonts have a few dozen at most, so this only
/// bounds a hostile `instanceCount`.
const MAX_INSTANCES: usize = 512;

/// `s` with every character outside `A–Z a–z 0–9` removed (PostScript name parts, Adobe Technical
/// Note #5902).
fn ps_part(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// The named instances of an `fvar` table (OpenType `fvar`: a 16-byte header, `axisCount` axis
/// records of `axisSize` bytes, then `instanceCount` instance records of `instanceSize` bytes:
/// subfamily name id, flags, one Fixed 16.16 coordinate per axis, and the PostScript name id when
/// the record has room for it). Records that point outside the table or name no style are skipped.
fn named_instances(fb: &[u8], nb: &[u8], ps_prefix: &str) -> Vec<NamedInstance> {
    let (Some(axes_off), Some(axis_count), Some(axis_size), Some(count), Some(size)) = (be16(fb, 4), be16(fb, 8), be16(fb, 10), be16(fb, 12), be16(fb, 14))
    else {
        return Vec::new();
    };
    let (axes_off, axis_count, axis_size, size) = (axes_off as usize, axis_count as usize, axis_size as usize, size as usize);
    if axis_count == 0 || axis_size < 20 || size < 4 + 4 * axis_count {
        return Vec::new();
    }
    // Offsets in checked arithmetic: on 32-bit targets (wasm) the products of hostile 16-bit
    // counts and sizes can overflow `usize`.
    let at_of = |base: usize, i: usize, stride: usize| i.checked_mul(stride).and_then(|o| o.checked_add(base));
    let tags: Option<Vec<String>> = (0..axis_count)
        .map(|a| {
            let at = at_of(axes_off, a, axis_size)?;
            fb.get(at..at.checked_add(4)?).map(|t| String::from_utf8_lossy(t).trim_end().to_string())
        })
        .collect();
    let (Some(tags), Some(first)) = (tags, at_of(axes_off, axis_count, axis_size)) else { return Vec::new() };
    let mut out: Vec<NamedInstance> = Vec::new();
    for i in 0..(count as usize).min(MAX_INSTANCES) {
        let Some(rec) = at_of(first, i, size).and_then(|at| fb.get(at..at.checked_add(size)?)) else { break };
        let Some(style) = be16(rec, 0).and_then(|id| name_string(nb, id)).map(|s| s.trim().to_string()) else { continue };
        let coords: Option<Vec<(String, f32)>> =
            tags.iter().enumerate().map(|(a, t)| be32(rec, 4 + 4 * a).map(|v| (t.clone(), v as i32 as f32 / 65536.0))).collect();
        let Some(mut coords) = coords else { continue };
        coords.sort_by(|a, b| a.0.cmp(&b.0));
        let named_ps = (size >= 6 + 4 * axis_count)
            .then(|| be16(rec, 4 + 4 * axis_count))
            .flatten()
            .filter(|&id| id != 0xFFFF)
            .and_then(|id| name_string(nb, id))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let postscript = named_ps.unwrap_or_else(|| format!("{ps_prefix}-{}", ps_part(&style)));
        if !out.iter().any(|o| o.style.eq_ignore_ascii_case(&style)) {
            out.push(NamedInstance { style, postscript, coords });
        }
    }
    out
}

/// A Windows Unicode name-table string by id in a language other than US English.
fn native_name(b: &[u8], id: u16) -> Option<String> {
    let count = be16(b, 2)? as usize;
    let storage = be16(b, 4)? as usize;
    for i in 0..count {
        let r = 6 + i * 12;
        let (plat, enc, lang, nid, len, off) =
            (be16(b, r)?, be16(b, r + 2)?, be16(b, r + 4)?, be16(b, r + 6)?, be16(b, r + 8)? as usize, be16(b, r + 10)? as usize);
        if nid != id || plat != 3 || !(enc == 1 || enc == 10) || lang == 0x409 {
            continue;
        }
        let raw = b.get(storage + off..storage + off + len)?;
        let u: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        let s = String::from_utf16_lossy(&u);
        if !s.trim().is_empty() {
            return Some(s);
        }
    }
    None
}

/// A name-table string by id, preferring Windows Unicode English, then any Unicode, then Mac Roman.
fn name_string(b: &[u8], id: u16) -> Option<String> {
    let count = be16(b, 2)? as usize;
    let storage = be16(b, 4)? as usize;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (plat, enc, lang, nid, len, off) =
            (be16(b, r)?, be16(b, r + 2)?, be16(b, r + 4)?, be16(b, r + 6)?, be16(b, r + 8)? as usize, be16(b, r + 10)? as usize);
        if nid != id {
            continue;
        }
        let raw = b.get(storage + off..storage + off + len)?;
        let (rank, s) = match plat {
            3 if enc == 1 || enc == 10 => {
                let u: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                (if lang == 0x409 { 0 } else { 1 }, String::from_utf16_lossy(&u))
            }
            0 => {
                let u: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                (2, String::from_utf16_lossy(&u))
            }
            1 if enc == 0 => (3, raw.iter().map(|&c| if c < 128 { c as char } else { '?' }).collect()),
            _ => continue,
        };
        if best.as_ref().is_none_or(|(r0, _)| rank < *r0) {
            best = Some((rank, s));
        }
    }
    best.map(|(_, s)| s).filter(|s| !s.trim().is_empty())
}

/// Synthetic font files for tests: just `name` and `fvar`, enough for the scan and the resolver.
#[cfg(test)]
pub(crate) mod test_font {
    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }

    /// A variable font of family `family` (name ids 1 and 2, style `Light`), one `wght` axis
    /// 100..900 (default 300) and the named `instances` (style, `wght`, PostScript name), each
    /// with its own name ids; `with_ps_ids` gives the instance records room for a PostScript name
    /// id (names without one get 0xFFFF, "none").
    pub(crate) fn variable(family: &str, instances: &[(&str, f32, Option<&str>)], with_ps_ids: bool) -> Vec<u8> {
        let mut strings: Vec<(u16, String)> = vec![(1, family.into()), (2, "Light".into())];
        let mut recs = Vec::new();
        for (i, (style, w, ps)) in instances.iter().enumerate() {
            let sid = 256 + 2 * i as u16;
            strings.push((sid, (*style).into()));
            let pid = ps.map(|p| {
                strings.push((sid + 1, p.into()));
                sid + 1
            });
            let mut r = u16s(&[sid, 0]);
            r.extend(((w * 65536.0) as i32).to_be_bytes());
            if with_ps_ids {
                r.extend(pid.unwrap_or(0xFFFF).to_be_bytes());
            }
            recs.push(r);
        }
        let enc: Vec<(u16, Vec<u8>)> = strings.iter().map(|(id, s)| (*id, s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect())).collect();
        let mut name = u16s(&[0, enc.len() as u16, 6 + 12 * enc.len() as u16]);
        let mut off = 0;
        for (id, st) in &enc {
            name.extend(u16s(&[3, 1, 0x409, *id, st.len() as u16, off]));
            off += st.len() as u16;
        }
        enc.iter().for_each(|(_, st)| name.extend(st));
        let isize = if with_ps_ids { 10 } else { 8 };
        let mut fvar = u16s(&[1, 0, 16, 2, 1, 20, instances.len() as u16, isize]);
        // wght: min 100, default 300, max 900 (Fixed 16.16), flags 0, axis name id 256.
        fvar.extend(b"wght".iter().copied().chain(u16s(&[100, 0, 300, 0, 900, 0, 0, 256])));
        recs.iter().for_each(|r| fvar.extend(r));
        let tables: [(&[u8; 4], Vec<u8>); 2] = [(b"fvar", fvar), (b"name", name)];
        let mut out = u16s(&[1, 0, 2, 0, 0, 0]);
        let mut body: Vec<u8> = vec![];
        for (tag, data) in &tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend(((12 + 32 + body.len()) as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
        }
        out.extend(body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bundled_names() {
        let f = read_faces_bytes(crate::fonts::INTER_BOLD);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].family, "Inter");
        assert_eq!(f[0].style, "Bold");
        assert_eq!(f[0].weight, 700);
        assert!(!f[0].italic);
        let i = read_faces_bytes(crate::fonts::INTER_ITALIC);
        assert!(i[0].italic, "{:?}", i[0]);
        let n = read_faces_bytes(crate::fonts::NOTO_SERIF_REGULAR);
        assert_eq!(n[0].family, "Noto Serif");
        assert!(read_faces_bytes(b"not a font").is_empty());
        assert!(!f[0].variable && !n[0].variable, "the bundled fonts are static");
    }

    /// A font file of just `name` (family, Regular) and, when `fvar` is given, an `fvar` table
    /// with that many axes (or a truncated one with `Some(None)`).
    fn names_font(family: &str, fvar: Option<Option<u16>>) -> Vec<u8> {
        let u16s = |v: &[u16]| -> Vec<u8> { v.iter().flat_map(|x| x.to_be_bytes()).collect() };
        let strings: Vec<Vec<u8>> = [family, "Regular"].iter().map(|s| s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()).collect();
        let mut name = u16s(&[0, 2, 30]);
        let mut off = 0;
        for (id, st) in strings.iter().enumerate() {
            name.extend(u16s(&[3, 1, 0x409, id as u16 + 1, st.len() as u16, off]));
            off += st.len() as u16;
        }
        strings.iter().for_each(|st| name.extend(st));
        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![];
        match fvar {
            Some(Some(n)) => {
                // Header, then `n` axis records: `wght` pinned at 400 (min = default = max).
                let mut t = u16s(&[1, 0, 16, 2, n, 20, 0, 4 + 4 * n]);
                (0..n).for_each(|_| t.extend(b"wght".iter().copied().chain(u16s(&[0x190, 0, 0x190, 0, 0x190, 0, 0, 256]))));
                tables.push((b"fvar", t));
            }
            Some(None) => tables.push((b"fvar", u16s(&[1, 0, 16]))),
            None => {}
        }
        tables.push((b"name", name));
        let mut out = u16s(&[1, 0, tables.len() as u16, 0, 0, 0]);
        let mut offset = 12 + 16 * tables.len();
        let mut body: Vec<u8> = vec![];
        for (tag, data) in &tables {
            out.extend(*tag);
            out.extend([0u8; 4]);
            out.extend((offset as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            body.extend(data);
            offset += data.len();
        }
        out.extend(body);
        out
    }

    /// The scan marks a face with an `fvar` table that declares axes as variable, reading only
    /// the table directory and the fvar header; a static font, an fvar without axes and a
    /// truncated fvar are not.
    #[test]
    fn detects_variable_faces_from_fvar() {
        let v = read_faces_bytes(&names_font("Var Test", Some(Some(1))));
        assert_eq!((v.len(), v[0].family.as_str(), v[0].variable), (1, "Var Test", true), "{v:?}");
        assert!(!read_faces_bytes(&names_font("Static Test", None))[0].variable);
        assert!(!read_faces_bytes(&names_font("No Axes", Some(Some(0))))[0].variable);
        assert!(!read_faces_bytes(&names_font("Cut", Some(None)))[0].variable);
    }

    /// #1563: the scan reads a variable font's named instances: the style name, the axis values
    /// and the PostScript name the font gives it, or the one Adobe Technical Note #5902 derives
    /// (family prefix, `-`, style without spaces) when it gives none.
    #[test]
    fn reads_named_instances_from_fvar() {
        let f = read_faces_bytes(&test_font::variable(
            "Fig Test",
            &[("Light", 300.0, Some("FigTest-Light")), ("Semi Bold", 600.0, None), ("Black", 900.0, Some("FigTest-Black"))],
            true,
        ));
        assert_eq!(f.len(), 1);
        let i = &f[0].instances;
        assert_eq!(i.iter().map(|x| x.style.as_str()).collect::<Vec<_>>(), ["Light", "Semi Bold", "Black"], "{i:?}");
        assert_eq!(i[2].coords, vec![("wght".to_string(), 900.0)]);
        assert_eq!(i[2].postscript, "FigTest-Black");
        assert_eq!(i[1].postscript, "FigTest-SemiBold", "derived when the font names none");
        assert_eq!((i[1].weight(400), i[1].italic(false)), (600, false));
        // Instance records without the PostScript name id field.
        let short = read_faces_bytes(&test_font::variable("Fig Test", &[("Black", 900.0, None)], false));
        assert_eq!(short[0].instances[0].postscript, "FigTest-Black");
        // A static font has none.
        assert!(read_faces_bytes(crate::fonts::INTER_BOLD)[0].instances.is_empty());
    }

    /// Hostile `fvar` counts: records past the end of the table are dropped, never read out of
    /// bounds, and a huge instance count is bounded.
    #[test]
    fn named_instances_survive_truncated_and_hostile_fvar() {
        let mut data = test_font::variable("Cut Test", &[("Black", 900.0, None)], false);
        // instanceCount (fvar offset 44 + 12) to 0xFFFF: only the one real record is read.
        data[44 + 12] = 0xFF;
        data[44 + 13] = 0xFF;
        let f = read_faces_bytes(&data);
        assert_eq!(f.len(), 1);
        assert!(f[0].instances.len() <= 1, "{:?}", f[0].instances);
        // An axis array offset past the table: no instances, but the face still scans.
        data[44 + 4] = 0xFF;
        let f = read_faces_bytes(&data);
        assert!(f[0].variable && f[0].instances.is_empty(), "{:?}", f[0]);
    }
}
