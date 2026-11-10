//! Simple-font encodings (ISO 32000-1 §9.6.6 and Annex D): StandardEncoding, WinAnsiEncoding
//! and MacRomanEncoding as code → Unicode tables, and the Latin glyph names those encodings
//! use (with `uniXXXX` / `uXXXX[XX]` names for everything else).

/// Glyph names of the Latin text character set (Annex D.2), with their Unicode values.
const NAMES: &[(&str, u32)] = &[
    ("space", 0x20),
    ("exclam", 0x21),
    ("quotedbl", 0x22),
    ("numbersign", 0x23),
    ("dollar", 0x24),
    ("percent", 0x25),
    ("ampersand", 0x26),
    ("quotesingle", 0x27),
    ("parenleft", 0x28),
    ("parenright", 0x29),
    ("asterisk", 0x2A),
    ("plus", 0x2B),
    ("comma", 0x2C),
    ("hyphen", 0x2D),
    ("period", 0x2E),
    ("slash", 0x2F),
    ("zero", 0x30),
    ("one", 0x31),
    ("two", 0x32),
    ("three", 0x33),
    ("four", 0x34),
    ("five", 0x35),
    ("six", 0x36),
    ("seven", 0x37),
    ("eight", 0x38),
    ("nine", 0x39),
    ("colon", 0x3A),
    ("semicolon", 0x3B),
    ("less", 0x3C),
    ("equal", 0x3D),
    ("greater", 0x3E),
    ("question", 0x3F),
    ("at", 0x40),
    ("bracketleft", 0x5B),
    ("backslash", 0x5C),
    ("bracketright", 0x5D),
    ("asciicircum", 0x5E),
    ("underscore", 0x5F),
    ("grave", 0x60),
    ("braceleft", 0x7B),
    ("bar", 0x7C),
    ("braceright", 0x7D),
    ("asciitilde", 0x7E),
    ("exclamdown", 0xA1),
    ("cent", 0xA2),
    ("sterling", 0xA3),
    ("currency", 0xA4),
    ("yen", 0xA5),
    ("brokenbar", 0xA6),
    ("section", 0xA7),
    ("dieresis", 0xA8),
    ("copyright", 0xA9),
    ("ordfeminine", 0xAA),
    ("guillemotleft", 0xAB),
    ("logicalnot", 0xAC),
    ("registered", 0xAE),
    ("macron", 0xAF),
    ("degree", 0xB0),
    ("plusminus", 0xB1),
    ("twosuperior", 0xB2),
    ("threesuperior", 0xB3),
    ("acute", 0xB4),
    ("mu", 0xB5),
    ("paragraph", 0xB6),
    ("periodcentered", 0xB7),
    ("cedilla", 0xB8),
    ("onesuperior", 0xB9),
    ("ordmasculine", 0xBA),
    ("guillemotright", 0xBB),
    ("onequarter", 0xBC),
    ("onehalf", 0xBD),
    ("threequarters", 0xBE),
    ("questiondown", 0xBF),
    ("Agrave", 0xC0),
    ("Aacute", 0xC1),
    ("Acircumflex", 0xC2),
    ("Atilde", 0xC3),
    ("Adieresis", 0xC4),
    ("Aring", 0xC5),
    ("AE", 0xC6),
    ("Ccedilla", 0xC7),
    ("Egrave", 0xC8),
    ("Eacute", 0xC9),
    ("Ecircumflex", 0xCA),
    ("Edieresis", 0xCB),
    ("Igrave", 0xCC),
    ("Iacute", 0xCD),
    ("Icircumflex", 0xCE),
    ("Idieresis", 0xCF),
    ("Eth", 0xD0),
    ("Ntilde", 0xD1),
    ("Ograve", 0xD2),
    ("Oacute", 0xD3),
    ("Ocircumflex", 0xD4),
    ("Otilde", 0xD5),
    ("Odieresis", 0xD6),
    ("multiply", 0xD7),
    ("Oslash", 0xD8),
    ("Ugrave", 0xD9),
    ("Uacute", 0xDA),
    ("Ucircumflex", 0xDB),
    ("Udieresis", 0xDC),
    ("Yacute", 0xDD),
    ("Thorn", 0xDE),
    ("germandbls", 0xDF),
    ("agrave", 0xE0),
    ("aacute", 0xE1),
    ("acircumflex", 0xE2),
    ("atilde", 0xE3),
    ("adieresis", 0xE4),
    ("aring", 0xE5),
    ("ae", 0xE6),
    ("ccedilla", 0xE7),
    ("egrave", 0xE8),
    ("eacute", 0xE9),
    ("ecircumflex", 0xEA),
    ("edieresis", 0xEB),
    ("igrave", 0xEC),
    ("iacute", 0xED),
    ("icircumflex", 0xEE),
    ("idieresis", 0xEF),
    ("eth", 0xF0),
    ("ntilde", 0xF1),
    ("ograve", 0xF2),
    ("oacute", 0xF3),
    ("ocircumflex", 0xF4),
    ("otilde", 0xF5),
    ("odieresis", 0xF6),
    ("divide", 0xF7),
    ("oslash", 0xF8),
    ("ugrave", 0xF9),
    ("uacute", 0xFA),
    ("ucircumflex", 0xFB),
    ("udieresis", 0xFC),
    ("yacute", 0xFD),
    ("thorn", 0xFE),
    ("ydieresis", 0xFF),
    ("dotlessi", 0x131),
    ("Lslash", 0x141),
    ("lslash", 0x142),
    ("OE", 0x152),
    ("oe", 0x153),
    ("Scaron", 0x160),
    ("scaron", 0x161),
    ("Ydieresis", 0x178),
    ("Zcaron", 0x17D),
    ("zcaron", 0x17E),
    ("florin", 0x192),
    ("circumflex", 0x2C6),
    ("caron", 0x2C7),
    ("breve", 0x2D8),
    ("dotaccent", 0x2D9),
    ("ring", 0x2DA),
    ("ogonek", 0x2DB),
    ("tilde", 0x2DC),
    ("hungarumlaut", 0x2DD),
    ("Omega", 0x3A9),
    ("pi", 0x3C0),
    ("endash", 0x2013),
    ("emdash", 0x2014),
    ("quoteleft", 0x2018),
    ("quoteright", 0x2019),
    ("quotesinglbase", 0x201A),
    ("quotedblleft", 0x201C),
    ("quotedblright", 0x201D),
    ("quotedblbase", 0x201E),
    ("dagger", 0x2020),
    ("daggerdbl", 0x2021),
    ("bullet", 0x2022),
    ("ellipsis", 0x2026),
    ("perthousand", 0x2030),
    ("guilsinglleft", 0x2039),
    ("guilsinglright", 0x203A),
    ("fraction", 0x2044),
    ("Euro", 0x20AC),
    ("trademark", 0x2122),
    ("partialdiff", 0x2202),
    ("Delta", 0x2206),
    ("product", 0x220F),
    ("summation", 0x2211),
    ("minus", 0x2212),
    ("radical", 0x221A),
    ("infinity", 0x221E),
    ("integral", 0x222B),
    ("approxequal", 0x2248),
    ("notequal", 0x2260),
    ("lessequal", 0x2264),
    ("greaterequal", 0x2265),
    ("lozenge", 0x25CA),
    ("fi", 0xFB01),
    ("fl", 0xFB02),
    ("nbspace", 0xA0),
    ("sfthyphen", 0xAD),
];

/// Unicode value of a glyph name (`A`, `eacute`, `uni00E9`, `u1F600`, `f_i`→first part).
pub fn name_to_unicode(name: &str) -> Option<char> {
    let base = name.split('.').next().unwrap_or(name);
    if base.len() == 1 && base.as_bytes()[0].is_ascii_alphabetic() {
        return base.chars().next();
    }
    if let Some((_, u)) = NAMES.iter().find(|(n, _)| *n == base) {
        return char::from_u32(*u);
    }
    // `get(..4)` also insists on a character boundary: a name like `uni\u{20AC}\u{20AC}` used to
    // panic slicing inside a multi-byte character.
    if let Some(x) = base.strip_prefix("uni").and_then(|h| h.get(..4)) {
        return u32::from_str_radix(x, 16).ok().and_then(char::from_u32);
    }
    if let Some(h) = base.strip_prefix('u')
        && (4..=6).contains(&h.len())
    {
        return u32::from_str_radix(h, 16).ok().and_then(char::from_u32);
    }
    // Ligature names (`f_f_i`): the first component.
    if let Some((a, _)) = base.split_once('_') {
        return name_to_unicode(a);
    }
    None
}

/// The glyph name of a Unicode value (Latin names, else `uniXXXX`).
pub fn unicode_to_name(c: char) -> String {
    let u = c as u32;
    if c.is_ascii_alphabetic() {
        return c.to_string();
    }
    match NAMES.iter().find(|(_, v)| *v == u) {
        Some((n, _)) => (*n).to_string(),
        None => format!("uni{u:04X}"),
    }
}

/// A simple font's base encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Standard,
    WinAnsi,
    MacRoman,
}

impl Base {
    pub fn from_name(n: &str) -> Option<Base> {
        match n {
            "StandardEncoding" => Some(Base::Standard),
            "WinAnsiEncoding" => Some(Base::WinAnsi),
            "MacRomanEncoding" => Some(Base::MacRoman),
            _ => None,
        }
    }

    /// The character of a code (`None` for unassigned codes).
    pub fn unicode(self, code: u8) -> Option<char> {
        let u = match self {
            Base::Standard => standard(code)?,
            Base::WinAnsi => win_ansi(code)?,
            Base::MacRoman => mac_roman(code)?,
        };
        char::from_u32(u)
    }

    /// The glyph name of a code.
    pub fn name(self, code: u8) -> Option<String> {
        self.unicode(code).map(unicode_to_name)
    }
}

fn ascii(code: u8) -> Option<u32> {
    (0x20..0x7F).contains(&code).then_some(code as u32)
}

fn standard(code: u8) -> Option<u32> {
    Some(match code {
        0x27 => 0x2019,
        0x60 => 0x2018,
        0x20..=0x7E => code as u32,
        0xA1 => 0xA1,
        0xA2 => 0xA2,
        0xA3 => 0xA3,
        0xA4 => 0x2044,
        0xA5 => 0xA5,
        0xA6 => 0x192,
        0xA7 => 0xA7,
        0xA8 => 0xA4,
        0xA9 => 0x27,
        0xAA => 0x201C,
        0xAB => 0xAB,
        0xAC => 0x2039,
        0xAD => 0x203A,
        0xAE => 0xFB01,
        0xAF => 0xFB02,
        0xB1 => 0x2013,
        0xB2 => 0x2020,
        0xB3 => 0x2021,
        0xB4 => 0xB7,
        0xB6 => 0xB6,
        0xB7 => 0x2022,
        0xB8 => 0x201A,
        0xB9 => 0x201E,
        0xBA => 0x201D,
        0xBB => 0xBB,
        0xBC => 0x2026,
        0xBD => 0x2030,
        0xBF => 0xBF,
        0xC1 => 0x60,
        0xC2 => 0xB4,
        0xC3 => 0x2C6,
        0xC4 => 0x2DC,
        0xC5 => 0xAF,
        0xC6 => 0x2D8,
        0xC7 => 0x2D9,
        0xC8 => 0xA8,
        0xCA => 0x2DA,
        0xCB => 0xB8,
        0xCD => 0x2DD,
        0xCE => 0x2DB,
        0xCF => 0x2C7,
        0xD0 => 0x2014,
        0xE1 => 0xC6,
        0xE3 => 0xAA,
        0xE8 => 0x141,
        0xE9 => 0xD8,
        0xEA => 0x152,
        0xEB => 0xBA,
        0xF1 => 0xE6,
        0xF5 => 0x131,
        0xF8 => 0x142,
        0xF9 => 0xF8,
        0xFA => 0x153,
        0xFB => 0xDF,
        _ => return None,
    })
}

/// Windows code page 1252 (WinAnsiEncoding; bullets for the unused codes, as Annex D notes).
fn win_ansi(code: u8) -> Option<u32> {
    const HIGH: [u32; 32] = [
        0x20AC, 0x2022, 0x201A, 0x192, 0x201E, 0x2026, 0x2020, 0x2021, 0x2C6, 0x2030, 0x160, 0x2039, 0x152, 0x2022, 0x17D, 0x2022, 0x2022, 0x2018, 0x2019,
        0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x2DC, 0x2122, 0x161, 0x203A, 0x153, 0x2022, 0x17E, 0x178,
    ];
    match code {
        0x80..=0x9F => Some(HIGH[(code - 0x80) as usize]),
        0xA0 => Some(0x20),
        0xA1..=0xFF => Some(code as u32),
        _ => ascii(code),
    }
}

fn mac_roman(code: u8) -> Option<u32> {
    const HIGH: [u32; 128] = [
        0xC4, 0xC5, 0xC7, 0xC9, 0xD1, 0xD6, 0xDC, 0xE1, 0xE0, 0xE2, 0xE4, 0xE3, 0xE5, 0xE7, 0xE9, 0xE8, 0xEA, 0xEB, 0xED, 0xEC, 0xEE, 0xEF, 0xF1, 0xF3, 0xF2,
        0xF4, 0xF6, 0xF5, 0xFA, 0xF9, 0xFB, 0xFC, 0x2020, 0xB0, 0xA2, 0xA3, 0xA7, 0x2022, 0xB6, 0xDF, 0xAE, 0xA9, 0x2122, 0xB4, 0xA8, 0x2260, 0xC6, 0xD8,
        0x221E, 0xB1, 0x2264, 0x2265, 0xA5, 0xB5, 0x2202, 0x2211, 0x220F, 0x3C0, 0x222B, 0xAA, 0xBA, 0x3A9, 0xE6, 0xF8, 0xBF, 0xA1, 0xAC, 0x221A, 0x192,
        0x2248, 0x2206, 0xAB, 0xBB, 0x2026, 0x20, 0xC0, 0xC3, 0xD5, 0x152, 0x153, 0x2013, 0x2014, 0x201C, 0x201D, 0x2018, 0x2019, 0xF7, 0x25CA, 0xFF, 0x178,
        0x2044, 0xA4, 0x2039, 0x203A, 0xFB01, 0xFB02, 0x2021, 0xB7, 0x201A, 0x201E, 0x2030, 0xC2, 0xCA, 0xC1, 0xCB, 0xC8, 0xCD, 0xCE, 0xCF, 0xCC, 0xD3, 0xD4,
        0xF8FF, 0xD2, 0xDA, 0xDB, 0xD9, 0x131, 0x2C6, 0x2DC, 0xAF, 0x2D8, 0x2D9, 0x2DA, 0xB8, 0x2DD, 0x2DB, 0x2C7,
    ];
    match code {
        0x80..=0xFF => Some(HIGH[(code - 0x80) as usize]),
        0x27 => Some(0x27),
        _ => ascii(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings_and_names() {
        assert_eq!(Base::WinAnsi.unicode(0x80), Some('€'));
        assert_eq!(Base::WinAnsi.name(0xE9).as_deref(), Some("eacute"));
        assert_eq!(Base::Standard.name(0x27).as_deref(), Some("quoteright"));
        assert_eq!(Base::Standard.name(0xAE).as_deref(), Some("fi"));
        assert_eq!(Base::MacRoman.unicode(0x8E), Some('é'));
        assert_eq!(name_to_unicode("uni20AC"), Some('€'));
        assert_eq!(name_to_unicode("u1F600"), Some('😀'));
        assert_eq!(name_to_unicode("a.sc"), Some('a'));
        assert_eq!(name_to_unicode("f_i"), Some('f'));
        assert_eq!(unicode_to_name('Ω'), "Omega");
        assert_eq!(unicode_to_name('中'), "uni4E2D");
    }

    /// A glyph name like `uni\u{20AC}\u{20AC}` (multi-byte UTF-8 after `uni`) must not slice
    /// inside a character.
    #[test]
    fn multibyte_glyph_names_do_not_panic() {
        assert_eq!(name_to_unicode("uni\u{20AC}\u{20AC}"), None);
        assert_eq!(name_to_unicode("uni\u{20AC}"), None);
    }
}
