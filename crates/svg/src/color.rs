//! SVG paint colours use the shared precision-preserving compatibility parser.
//! Paint references (`none`, `currentColor`, `url`) remain app policy.

pub use craft_svg::color::parse_rgba_compat as parse_color;

#[cfg(test)]
mod tests {
    use super::parse_color;

    #[test]
    /// Non-finite channel values are rejected.
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

#[cfg(test)]
mod shared_precision_tests {
    use super::parse_color;

    #[test]
    fn shared_parser_keeps_float_channels_and_legacy_clamping() {
        assert_eq!(parse_color("rgb(100%,50%,0%)"), Some([1.0, 0.5, 0.0, 1.0]));
        assert_eq!(parse_color("rgba(0,0,255,0.123456789123)"), Some([0.0, 0.0, 1.0, 0.123456789123]));
        assert_eq!(parse_color("rgb(300,-1,0)"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("# fff"), None);
    }
}
