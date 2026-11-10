//! CSS colour values (CSS Color Module Level 3): keywords, `#rgb[a]`, `#rrggbb[aa]`, `rgb[a]()`,
//! `hsl[a]()`.

/// The CSS colour keywords (CSS Color Module Level 3, §4.3 extended colour keywords).
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xF0F8FF),
    ("antiquewhite", 0xFAEBD7),
    ("aqua", 0x00FFFF),
    ("aquamarine", 0x7FFFD4),
    ("azure", 0xF0FFFF),
    ("beige", 0xF5F5DC),
    ("bisque", 0xFFE4C4),
    ("black", 0x000000),
    ("blanchedalmond", 0xFFEBCD),
    ("blue", 0x0000FF),
    ("blueviolet", 0x8A2BE2),
    ("brown", 0xA52A2A),
    ("burlywood", 0xDEB887),
    ("cadetblue", 0x5F9EA0),
    ("chartreuse", 0x7FFF00),
    ("chocolate", 0xD2691E),
    ("coral", 0xFF7F50),
    ("cornflowerblue", 0x6495ED),
    ("cornsilk", 0xFFF8DC),
    ("crimson", 0xDC143C),
    ("cyan", 0x00FFFF),
    ("darkblue", 0x00008B),
    ("darkcyan", 0x008B8B),
    ("darkgoldenrod", 0xB8860B),
    ("darkgray", 0xA9A9A9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xA9A9A9),
    ("darkkhaki", 0xBDB76B),
    ("darkmagenta", 0x8B008B),
    ("darkolivegreen", 0x556B2F),
    ("darkorange", 0xFF8C00),
    ("darkorchid", 0x9932CC),
    ("darkred", 0x8B0000),
    ("darksalmon", 0xE9967A),
    ("darkseagreen", 0x8FBC8F),
    ("darkslateblue", 0x483D8B),
    ("darkslategray", 0x2F4F4F),
    ("darkslategrey", 0x2F4F4F),
    ("darkturquoise", 0x00CED1),
    ("darkviolet", 0x9400D3),
    ("deeppink", 0xFF1493),
    ("deepskyblue", 0x00BFFF),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1E90FF),
    ("firebrick", 0xB22222),
    ("floralwhite", 0xFFFAF0),
    ("forestgreen", 0x228B22),
    ("fuchsia", 0xFF00FF),
    ("gainsboro", 0xDCDCDC),
    ("ghostwhite", 0xF8F8FF),
    ("gold", 0xFFD700),
    ("goldenrod", 0xDAA520),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xADFF2F),
    ("honeydew", 0xF0FFF0),
    ("hotpink", 0xFF69B4),
    ("indianred", 0xCD5C5C),
    ("indigo", 0x4B0082),
    ("ivory", 0xFFFFF0),
    ("khaki", 0xF0E68C),
    ("lavender", 0xE6E6FA),
    ("lavenderblush", 0xFFF0F5),
    ("lawngreen", 0x7CFC00),
    ("lemonchiffon", 0xFFFACD),
    ("lightblue", 0xADD8E6),
    ("lightcoral", 0xF08080),
    ("lightcyan", 0xE0FFFF),
    ("lightgoldenrodyellow", 0xFAFAD2),
    ("lightgray", 0xD3D3D3),
    ("lightgreen", 0x90EE90),
    ("lightgrey", 0xD3D3D3),
    ("lightpink", 0xFFB6C1),
    ("lightsalmon", 0xFFA07A),
    ("lightseagreen", 0x20B2AA),
    ("lightskyblue", 0x87CEFA),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xB0C4DE),
    ("lightyellow", 0xFFFFE0),
    ("lime", 0x00FF00),
    ("limegreen", 0x32CD32),
    ("linen", 0xFAF0E6),
    ("magenta", 0xFF00FF),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66CDAA),
    ("mediumblue", 0x0000CD),
    ("mediumorchid", 0xBA55D3),
    ("mediumpurple", 0x9370DB),
    ("mediumseagreen", 0x3CB371),
    ("mediumslateblue", 0x7B68EE),
    ("mediumspringgreen", 0x00FA9A),
    ("mediumturquoise", 0x48D1CC),
    ("mediumvioletred", 0xC71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xF5FFFA),
    ("mistyrose", 0xFFE4E1),
    ("moccasin", 0xFFE4B5),
    ("navajowhite", 0xFFDEAD),
    ("navy", 0x000080),
    ("oldlace", 0xFDF5E6),
    ("olive", 0x808000),
    ("olivedrab", 0x6B8E23),
    ("orange", 0xFFA500),
    ("orangered", 0xFF4500),
    ("orchid", 0xDA70D6),
    ("palegoldenrod", 0xEEE8AA),
    ("palegreen", 0x98FB98),
    ("paleturquoise", 0xAFEEEE),
    ("palevioletred", 0xDB7093),
    ("papayawhip", 0xFFEFD5),
    ("peachpuff", 0xFFDAB9),
    ("peru", 0xCD853F),
    ("pink", 0xFFC0CB),
    ("plum", 0xDDA0DD),
    ("powderblue", 0xB0E0E6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xFF0000),
    ("rosybrown", 0xBC8F8F),
    ("royalblue", 0x4169E1),
    ("saddlebrown", 0x8B4513),
    ("salmon", 0xFA8072),
    ("sandybrown", 0xF4A460),
    ("seagreen", 0x2E8B57),
    ("seashell", 0xFFF5EE),
    ("sienna", 0xA0522D),
    ("silver", 0xC0C0C0),
    ("skyblue", 0x87CEEB),
    ("slateblue", 0x6A5ACD),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xFFFAFA),
    ("springgreen", 0x00FF7F),
    ("steelblue", 0x4682B4),
    ("tan", 0xD2B48C),
    ("teal", 0x008080),
    ("thistle", 0xD8BFD8),
    ("tomato", 0xFF6347),
    ("turquoise", 0x40E0D0),
    ("violet", 0xEE82EE),
    ("wheat", 0xF5DEB3),
    ("white", 0xFFFFFF),
    ("whitesmoke", 0xF5F5F5),
    ("yellow", 0xFFFF00),
    ("yellowgreen", 0x9ACD32),
];

/// Parse a CSS colour into RGBA 0..1 (`transparent` is RGBA 0). `None` for anything else
/// (`none`, `currentColor` and `url()` are handled by the caller).
pub fn parse_color(s: &str) -> Option<[f64; 4]> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if lower == "transparent" {
        return Some([0.0; 4]);
    }
    if let Some(h) = lower.strip_prefix('#') {
        let d: Vec<u32> = h.chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        let v = |i: usize| d[i] as f64 / 15.0;
        let w = |i: usize| (d[i] * 16 + d[i + 1]) as f64 / 255.0;
        return Some(match d.len() {
            3 => [v(0), v(1), v(2), 1.0],
            4 => [v(0), v(1), v(2), v(3)],
            6 => [w(0), w(2), w(4), 1.0],
            8 => [w(0), w(2), w(4), w(6)],
            _ => return None,
        });
    }
    if let Some(args) = func(&lower, "rgba").or_else(|| func(&lower, "rgb")) {
        let a: Vec<&str> = split_args(args);
        if a.len() < 3 {
            return None;
        }
        let comp = |t: &str| -> Option<f64> {
            Some(if let Some(p) = t.strip_suffix('%') {
                p.trim().parse::<f64>().ok().filter(|v| v.is_finite())? / 100.0
            } else {
                t.parse::<f64>().ok().filter(|v| v.is_finite())? / 255.0
            })
        };
        let alpha = a.get(3).map_or(Some(1.0), |t| alpha(t))?;
        return Some([comp(a[0])?.clamp(0.0, 1.0), comp(a[1])?.clamp(0.0, 1.0), comp(a[2])?.clamp(0.0, 1.0), alpha]);
    }
    if let Some(args) = func(&lower, "hsla").or_else(|| func(&lower, "hsl")) {
        let a: Vec<&str> = split_args(args);
        if a.len() < 3 {
            return None;
        }
        let h = a[0].trim_end_matches("deg").parse::<f64>().ok().filter(|v| v.is_finite())?.rem_euclid(360.0) / 360.0;
        let s = a[1].trim_end_matches('%').parse::<f64>().ok().filter(|v| v.is_finite())?.clamp(0.0, 100.0) / 100.0;
        let l = a[2].trim_end_matches('%').parse::<f64>().ok().filter(|v| v.is_finite())?.clamp(0.0, 100.0) / 100.0;
        let al = a.get(3).map_or(Some(1.0), |t| alpha(t))?;
        let [r, g, b] = hsl(h, s, l);
        return Some([r, g, b, al]);
    }
    NAMED.iter().find(|(n, _)| *n == lower).map(|(_, v)| [((v >> 16) & 255) as f64 / 255.0, ((v >> 8) & 255) as f64 / 255.0, (v & 255) as f64 / 255.0, 1.0])
}

fn func<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    s.strip_prefix(name)?.trim_start().strip_prefix('(')?.strip_suffix(')')
}

fn split_args(s: &str) -> Vec<&str> {
    s.split(|c: char| c == ',' || c == '/' || c.is_whitespace()).filter(|t| !t.is_empty()).collect()
}

fn alpha(t: &str) -> Option<f64> {
    Some(
        if let Some(p) = t.strip_suffix('%') {
            p.parse::<f64>().ok().filter(|v| v.is_finite())? / 100.0
        } else {
            t.parse::<f64>().ok().filter(|v| v.is_finite())?
        }
        .clamp(0.0, 1.0),
    )
}

/// CSS HSL → RGB (CSS Color 3 §4.2.4).
fn hsl(h: f64, s: f64, l: f64) -> [f64; 3] {
    let m2 = if l <= 0.5 { l * (s + 1.0) } else { l + s - l * s };
    let m1 = l * 2.0 - m2;
    let hue = |mut h: f64| {
        if h < 0.0 {
            h += 1.0;
        }
        if h > 1.0 {
            h -= 1.0;
        }
        if h * 6.0 < 1.0 {
            m1 + (m2 - m1) * h * 6.0
        } else if h * 2.0 < 1.0 {
            m2
        } else if h * 3.0 < 2.0 {
            m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0
        } else {
            m1
        }
    };
    [hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::parse_color;

    #[test]
    fn non_finite_rejected() {
        for s in [
            "rgb(nan,0,0)",
            "rgb(inf,0,0)",
            "rgba(0,0,0,nan)",
            "rgba(0,0,0,inf)",
            "hsl(nan,100%,50%)",
            "hsl(120,nan%,50%)",
            "hsl(120,100%,inf%)",
            "hsl(120,100%,50%,nan)",
        ] {
            assert_eq!(parse_color(s), None, "{s}");
        }
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#f00"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("#00ff0080").map(|c| (c[1], (c[3] * 255.0).round())), Some((1.0, 128.0)));
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("rgba(0,0,255,0.5)"), Some([0.0, 0.0, 1.0, 0.5]));
        assert_eq!(parse_color("rgb(100%, 50%, 0%)"), Some([1.0, 0.5, 0.0, 1.0]));
        assert_eq!(parse_color("hsl(120, 100%, 50%)"), Some([0.0, 1.0, 0.0, 1.0]));
        assert_eq!(parse_color("CornflowerBlue"), Some([100.0 / 255.0, 149.0 / 255.0, 237.0 / 255.0, 1.0]));
        assert_eq!(parse_color("transparent"), Some([0.0; 4]));
        assert_eq!(parse_color("nonsense"), None);
    }
}
