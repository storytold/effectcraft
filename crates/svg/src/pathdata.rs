//! Prefix-preserving SVG path and numeric-list parsing is shared with craft-svg.
//! App document budgets and geometry policy remain with the importer.

pub use craft_svg::prefix_path::{MAX_PATH_ELEMENTS, numbers_bounded, parse_path_data, parse_path_data_bounded};

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{PathEl, Point};

    #[test]
    /// Huge arc radii stay bounded.
    fn huge_arc_radius_stays_bounded() {
        let p = parse_path_data("M0 0A1e36 1e36 0 1 1 1e30 0");
        assert!(p.elements().len() < 1000, "got {} elements", p.elements().len());
    }

    #[test]
    fn commands() {
        let p = parse_path_data("M10,10 L20,10 h10 v10 H10 z m5 5 l1-1.5.5.5e1 C 0 0 1 1 2 2 s3 3 4 4 Q1 1 2 2 t3 3 A5 5 0 0 1 20 20 a5 5 0 1010 0");
        let els = p.elements();
        assert_eq!(els[0], PathEl::MoveTo(Point::new(10.0, 10.0)));
        assert_eq!(els[2], PathEl::LineTo(Point::new(30.0, 10.0)));
        assert_eq!(els[3], PathEl::LineTo(Point::new(30.0, 20.0)));
        assert!(els.iter().any(|e| matches!(e, PathEl::ClosePath)));
        // Relative move after Z is relative to the subpath start (10,10).
        assert!(els.contains(&PathEl::MoveTo(Point::new(15.0, 15.0))));
        // "l1-1.5.5.5e1": two implicit linetos (1,-1.5) then (.5,5).
        assert!(els.contains(&PathEl::LineTo(Point::new(16.0, 13.5))));
        assert!(els.contains(&PathEl::LineTo(Point::new(16.5, 18.5))));
        assert!(els.iter().filter(|e| matches!(e, PathEl::CurveTo(..))).count() >= 4);
    }

    #[test]
    fn errors_keep_prefix() {
        let p = parse_path_data("M0 0 L10 0 L10 x10 L0 10");
        assert_eq!(p.elements().len(), 2);
        assert!(parse_path_data("garbage").elements().is_empty());
        assert!(parse_path_data("").elements().is_empty());
    }

    #[test]
    fn number_after_close_stops_instead_of_looping() {
        // "Z9": Z was repeated implicitly without consuming the number, appending close-paths
        // until memory ran out.
        let p = parse_path_data("M0 0 L10 0 L10 10 Z9 C1 2 3 4 5 6");
        assert_eq!(p.elements().len(), 4);
        assert!(parse_path_data("M0 0 L10 0 z 9 9").elements().len() <= 4);
    }

    #[test]
    /// Huge-radius arcs still reach their end point.
    fn huge_radius_arc_still_reaches_end_point() {
        for d in ["M0 0 A1e20 1e20 0 0 1 10 0 L10 10", "M0 0 A1e20 1e20 0 1 1 10 0 L10 10", "M0 0 a1e20 1e20 0 0 1 10 0 L10 10"] {
            let p = parse_path_data(d);
            let reaches = p.elements().iter().any(|e| match e {
                PathEl::LineTo(q) | PathEl::CurveTo(_, _, q) => (q.x - 10.0).abs() < 1e-6 && q.y.abs() < 1e-6,
                _ => false,
            });
            assert!(reaches, "{d}: {:?}", p.elements());
        }
    }

    #[test]
    fn number_lists() {
        assert_eq!(numbers_bounded("0 0,100 50", 8).0, vec![0.0, 0.0, 100.0, 50.0]);
        assert_eq!(numbers_bounded("-1-2.5.5", 8).0, vec![-1.0, -2.5, 0.5]);
        assert_eq!(numbers_bounded("1e2,2E-1", 8).0, vec![100.0, 0.2]);
    }

    #[test]
    /// The element cap stops expansion and reports truncation.
    fn path_element_limit_stops_expansion_and_reports_truncation() {
        let data = format!("M0 0{}", "h1".repeat(MAX_PATH_ELEMENTS + 1));
        let (path, exceeded) = parse_path_data_bounded(&data, 32);
        assert!(exceeded);
        assert!(path.elements().len() <= 32);
    }

    #[test]
    /// The number list cap stops retaining values.
    fn number_list_limit_stops_retaining_values() {
        let (values, exceeded) = numbers_bounded("1 2 3 4", 3);
        assert_eq!(values, [1.0, 2.0, 3.0]);
        assert!(exceeded);
    }
}
