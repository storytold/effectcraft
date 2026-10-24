//! Item ids and labels, footage items (solids and files), precomp linkage, and the name and size
//! of a layer's source.

use crate::bytes::{be_f32, be_f64, be_u32, cstr, le_u32, text};
use crate::header::Rational;
use crate::typed::{Item, ItemKind, Layer, Project};

/// The footage data of an item (`LIST Pin ` with `sspc`, `opti` and, for files, `Als2/alas`).
#[derive(Debug, Clone, PartialEq)]
pub struct Footage {
    /// The 4-character file type tag at byte 22 of `sspc` (`Soli` for a solid, `png!` for a png).
    pub file_type: String,
    /// The width in pixels (`u32` at byte 30 of `sspc`).
    pub width: u32,
    /// The height in pixels (`u32` at byte 34 of `sspc`).
    pub height: u32,
    /// The display name stored in `opti` (the item's own `Utf8` name is empty for footage): at byte
    /// 26 for a solid and at byte 58 for a file.
    pub name: Option<String>,
    /// True for a solid (tag `Soli` at the start of `opti`).
    pub is_solid: bool,
    /// The colour of a solid as ARGB floats 0..1 (`opti` bytes 10, 14, 18, 22); a red solid is
    /// `[1, 1, 0, 0]`.
    pub solid_argb: Option<[f32; 4]>,
    /// The file path from the alias JSON (`Als2/alas`, key `fullpath`); none for solids. For an
    /// image sequence it is the sequence **folder**.
    pub path: Option<String>,
    /// The duration in seconds as stored (`sspc` `u32` at byte 38 over `u32` at byte 42; unreduced):
    /// 0/1 for stills, 48000/48000 for the 1 s WAV (the sample rate is the denominator), 3/30 for
    /// the three-frame image sequence (the frame rate is the denominator).
    pub duration: Option<Rational>,
    /// The native frame rate (`sspc` `u32` at byte 56): 30 for the image sequence, 0 for stills,
    /// audio and solids (also 0 when the byte is missing).
    pub native_frame_rate: u32,
    /// The bits per pixel (`sspc` `u32` at byte 60): 32 for an RGBA still, 24 for an RGB sequence,
    /// 0 for solids and audio.
    pub bits_per_pixel: u32,
    /// The size in bytes of the source file(s) (`sspc` `u32` at byte 208): 149 for the png, 96044
    /// for the WAV (44 + 48000 x 2), 504 for three 168-byte png files.
    pub file_size: u32,
    /// The audio sample rate (`sspc` `f64` at byte 160, 48000.0 for the WAV); `None` for footage
    /// without audio.
    pub audio_sample_rate: Option<f64>,
    /// True when the alias points at a folder (`"target_is_folder":true`): an image sequence is ONE
    /// item whose alias is its folder and whose `opti` holds the first file name.
    pub is_sequence: bool,
}

impl Footage {
    /// The number of frames of an image sequence (`duration * native_frame_rate`: 3/30 s at 30 fps is
    /// 3); `None` for anything else.
    pub fn sequence_frame_count(&self) -> Option<u32> {
        if !self.is_sequence {
            return None;
        }
        let d = self.duration?;
        let frames = i128::from(d.num).checked_mul(i128::from(self.native_frame_rate))?;
        let den = i128::from(d.den);
        if den <= 0 || frames % den != 0 {
            return None;
        }
        u32::try_from(frames / den).ok()
    }

    /// The solid's colour as fractions R, G, B.
    pub fn solid_rgb(&self) -> Option<[f32; 3]> {
        let [_, r, g, b] = self.solid_argb?;
        Some([r, g, b])
    }
}

/// The boolean value of `"key"` in a flat JSON object (`true` / `false`); `None` when missing.
pub(crate) fn json_bool(json: &[u8], key: &str) -> Option<bool> {
    let needle = format!("\"{key}\":");
    let needle = needle.as_bytes();
    let at = json.windows(needle.len()).position(|w| w == needle)?;
    let rest = json.get(at.checked_add(needle.len())?..)?;
    if rest.starts_with(b"true") {
        Some(true)
    } else if rest.starts_with(b"false") {
        Some(false)
    } else {
        None
    }
}

/// The string value of `"key"` in a flat JSON object, with the standard escapes decoded.
/// Returns `None` when the key is missing or the value is not a string.
pub(crate) fn json_string(json: &[u8], key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let needle = needle.as_bytes();
    let at = json.windows(needle.len()).position(|w| w == needle)?;
    let mut i = at.checked_add(needle.len())?;
    let skip_ws = |i: &mut usize| {
        while json.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
            *i = i.saturating_add(1);
        }
    };
    skip_ws(&mut i);
    if json.get(i) != Some(&b':') {
        return None;
    }
    i = i.saturating_add(1);
    skip_ws(&mut i);
    if json.get(i) != Some(&b'"') {
        return None;
    }
    i = i.saturating_add(1);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let c = *json.get(i)?;
        i = i.saturating_add(1);
        match c {
            b'"' => return Some(text(&out)),
            b'\\' => {
                let e = *json.get(i)?;
                i = i.saturating_add(1);
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        let hex = json.get(i..i.checked_add(4)?)?;
                        let code = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                        i = i.saturating_add(4);
                        let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    other => out.push(other),
                }
            }
            other => out.push(other),
        }
    }
}

/// The numbering of an image sequence item (353, 603).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceInfo {
    /// The file name before the frame number (`seq_`, `shot_`): the first `Utf8` after the alias.
    pub prefix: String,
    /// The file name after the frame number (`.png`, `.exr`): the second `Utf8` after the alias.
    pub suffix: String,
    /// The first and last frame number (`sspc` `u32` at 172 and 176: 1 and 3 in 353, 101 and 103
    /// in 603).
    pub start: u32,
    pub end: u32,
    /// The digits of the frame number (`sspc` `u32` at 180: 3 for `seq_001`, 4 for `shot_0101`).
    pub digits: u32,
}

impl SequenceInfo {
    /// The file name of frame `n` (`shot_0101.exr`).
    pub fn file_name(&self, n: u32) -> String {
        let width = usize::try_from(self.digits.min(16)).unwrap_or(0);
        format!("{}{n:0width$}{}", self.prefix, self.suffix)
    }
}

impl<'f, 'a> Item<'f, 'a> {
    /// The numbering of an image sequence (an item whose alias is a folder); `None` for anything
    /// else.
    pub fn sequence_info(&self) -> Option<SequenceInfo> {
        let f = self.footage()?;
        if !f.is_sequence {
            return None;
        }
        let pin = self.chunk.child_list(b"Pin ")?;
        let sspc = pin.child(b"sspc")?.bytes()?;
        let kids = pin.children();
        let after = kids.iter().position(|c| c.is_list(b"Als2"))?;
        let mut names = kids.iter().skip(after + 1).take_while(|c| !c.is(b"opti")).filter(|c| c.is(b"Utf8")).filter_map(|c| c.bytes()).map(text);
        let prefix = names.next()?;
        let suffix = names.next()?;
        Some(SequenceInfo { prefix, suffix, start: be_u32(sspc, 172)?, end: be_u32(sspc, 176)?, digits: be_u32(sspc, 180)? })
    }

    /// The rate a clip is conformed to (Interpret Footage ▸ Conform to frame rate; `sspc` `u32` at
    /// byte 146): 30 for the 25 fps mp4 conformed to 30 (645), `None` when not conformed (0 in 644,
    /// 606) or for an item without footage data. The effective rate is then also at byte 150 while
    /// the stored duration (bytes 38 / 42) and byte 56 stay native. A conformed image sequence
    /// stores the new rate in bytes 42, 56 and 150 instead and keeps 0 here (615).
    pub fn conform_frame_rate(&self) -> Option<u32> {
        let sspc = self.chunk.child_list(b"Pin ")?.child(b"sspc")?.bytes()?;
        be_u32(sspc, 146).filter(|r| *r > 0)
    }

    /// The item id: the big-endian `u32` at byte 16 of `idta` (what a layer's source id refers to).
    pub fn id(&self) -> Option<u32> {
        be_u32(self.chunk.child(b"idta")?.bytes()?, 16)
    }

    /// The label colour index, byte 58 of `idta` (15 for a composition made by precomposing).
    pub fn label(&self) -> Option<u8> {
        self.chunk.child(b"idta")?.bytes()?.get(58).copied()
    }

    /// The ids of the compositions this item uses as precomp layers: the little-endian `u32` of
    /// each `idpi` chunk.
    pub fn used_item_ids(&self) -> Vec<u32> {
        self.chunk.children().iter().filter(|c| c.is(b"idpi")).filter_map(|c| le_u32(c.bytes()?, 0)).collect()
    }

    /// The footage data of a footage item.
    pub fn footage(&self) -> Option<Footage> {
        if self.kind()? != ItemKind::Footage {
            return None;
        }
        let pin = self.chunk.child_list(b"Pin ")?;
        let sspc = pin.child(b"sspc")?.bytes()?;
        let opti = pin.child(b"opti")?.bytes()?;
        let is_solid = opti.get(..4) == Some(&b"Soli"[..]);
        let alias = pin.child_list(b"Als2").and_then(|als| als.child(b"alas")).and_then(|alas| alas.bytes());
        let path = alias.and_then(|json| json_string(json, "fullpath"));
        let is_sequence = alias.and_then(|json| json_bool(json, "target_is_folder")).unwrap_or(false);
        let solid_argb = if is_solid { Some([be_f32(opti, 10)?, be_f32(opti, 14)?, be_f32(opti, 18)?, be_f32(opti, 22)?]) } else { None };
        Some(Footage {
            file_type: cstr(sspc, 22, 4)?,
            width: be_u32(sspc, 30)?,
            height: be_u32(sspc, 34)?,
            name: cstr(opti, if is_solid { 26 } else { 58 }, 256).filter(|n| !n.is_empty()),
            is_solid,
            solid_argb,
            path,
            duration: be_u32(sspc, 38).zip(be_u32(sspc, 42)).map(|(num, den)| Rational { num: i64::from(num), den: i64::from(den) }),
            native_frame_rate: be_u32(sspc, 56).unwrap_or(0),
            bits_per_pixel: be_u32(sspc, 60).unwrap_or(0),
            file_size: be_u32(sspc, 208).unwrap_or(0),
            audio_sample_rate: be_f64(sspc, 160).filter(|r| r.is_finite() && *r > 0.0),
            is_sequence,
        })
    }

    /// The name this item shows in the project: the item name, or for footage the name stored in
    /// `opti`.
    pub fn display_name(&self) -> Option<String> {
        self.name().or_else(|| self.footage().and_then(|f| f.name))
    }
}

impl<'f, 'a> Project<'f, 'a> {
    /// The item with this id (`idta` byte 16), searching every item including those in folders.
    pub fn item_by_id(&self, id: u32) -> Option<Item<'f, 'a>> {
        self.all_items().into_iter().find(|i| i.id() == Some(id))
    }

    /// The item a layer is made from (its source id), e.g. the solid, the footage or the precomp.
    pub fn layer_source(&self, layer: &Layer<'f, 'a>) -> Option<Item<'f, 'a>> {
        self.item_by_id(layer.header()?.source_id?)
    }

    /// The name a layer shows: its own name, or its source's name when it has none.
    pub fn layer_display_name(&self, layer: &Layer<'f, 'a>) -> Option<String> {
        layer.name().or_else(|| self.layer_source(layer)?.display_name())
    }

    /// The pixel size of a layer's source: a footage item's width and height, or a precomp's
    /// composition size.
    pub fn layer_source_size(&self, layer: &Layer<'f, 'a>) -> Option<(u32, u32)> {
        let source = self.layer_source(layer)?;
        match source.kind()? {
            ItemKind::Footage => source.footage().map(|f| (f.width, f.height)),
            ItemKind::Composition => source.comp()?.header().map(|h| (u32::from(h.width), u32::from(h.height))),
            ItemKind::Folder | ItemKind::Other(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::json_string;

    #[test]
    fn reads_a_string_with_escapes() {
        let json = br#"{"a":1,"fullpath":"C:\\dir\\f\u00e9.png","b":"x"}"#;
        assert_eq!(json_string(json, "fullpath").as_deref(), Some(r"C:\dir\fé.png"));
        assert_eq!(json_string(json, "b").as_deref(), Some("x"));
        assert_eq!(json_string(json, "missing"), None);
        assert_eq!(json_string(br#"{"a" : "spaced"}"#, "a").as_deref(), Some("spaced"));
    }

    #[test]
    fn damaged_json_never_panics() {
        for json in [
            &br#"{"fullpath":"#[..],
            br#"{"fullpath":"abc"#,
            br#"{"fullpath":"ab\"#,
            br#"{"fullpath":"\u12"}"#,
            br#"{"fullpath":"\uzzzz"}"#,
            br#"{"fullpath":5}"#,
            br#"{"fullpath"}"#,
            b"\"fullpath\"",
            b"",
            b"\"",
            br#"{"fullpath":"\ud800"}"#,
        ] {
            let _ = json_string(json, "fullpath");
        }
        assert_eq!(json_string(br#"{"fullpath":"\ud800"}"#, "fullpath").as_deref(), Some("\u{fffd}"));
    }
}
