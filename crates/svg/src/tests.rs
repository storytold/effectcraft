use super::*;

const BASIC: &[u8] = include_bytes!("../tests/fixtures/basic-shapes.svg");
const PATHS: &[u8] = include_bytes!("../tests/fixtures/paths-gradients.svg");

fn px(img: &effectcraft_raster::Image, x: u32, y: u32) -> [f32; 4] {
    img.data[img.idx(x, y)]
}

fn near(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
    (0..4).all(|i| (a[i] - b[i]).abs() <= tol)
}

#[test]
fn gradient_stop_lookup_handles_duplicate_offsets() {
    use crate::raster::eval_stops;
    let stops = [(0.0, [0.0, 0.0, 0.0, 1.0]), (0.5, [1.0, 0.0, 0.0, 1.0]), (0.5, [0.0, 1.0, 0.0, 1.0]), (1.0, [0.0, 0.0, 1.0, 1.0])];
    assert_eq!(eval_stops(&stops, 0.5), [1.0, 0.0, 0.0, 1.0]);
    assert!(near(eval_stops(&stops, 0.5001), [0.0, 1.0, 0.0, 1.0], 3e-4));
    assert_eq!(eval_stops(&stops, 0.75), [0.0, 0.5, 0.5, 1.0]);
}

#[test]
fn raster_surface_budget_rejects_oversized_destination_before_allocation() {
    let doc = parse(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).unwrap();
    // 16,384² RGBA-f32 pixels exceed the bounded destination-surface budget; no giant
    // allocation should be attempted and the caller receives an empty image.
    let img = rasterize(&doc, 16_384, 16_384, 1.0);
    assert!(img.data.is_empty());
    assert_eq!((img.width, img.height), (0, 0));
}

#[test]
fn basic_shapes_structure() {
    let d = parse(BASIC).unwrap();
    assert_eq!((d.width, d.height), (200.0, 100.0));
    assert_eq!(d.pixel_size(), (200, 100));
    // viewBox 400×200 into 200×100.
    assert!((d.root.transform.as_coeffs()[0] - 0.5).abs() < 1e-12);
    let shapes = d.flatten();
    assert_eq!(shapes.len(), 6);
    let (m, o, s) = shapes[1];
    assert_eq!(s.name, "box");
    assert!(matches!(s.geom, Geom::Rect { w, h, rx, .. } if w == 120.0 && h == 80.0 && rx == 12.0));
    assert!((o - 0.8).abs() < 1e-12);
    // translate(20 20) then the viewBox scale.
    let p = m * kurbo::Point::new(10.0, 10.0);
    assert!((p.x - 15.0).abs() < 1e-9 && (p.y - 15.0).abs() < 1e-9);
    let st = s.stroke.as_ref().unwrap();
    assert_eq!(st.width, 6.0);
    assert_eq!(st.paint, Paint::Color([1.0, 1.0, 1.0]));
    assert!(matches!(shapes[2].2.geom, Geom::Ellipse { rx, ry, .. } if rx == 30.0 && ry == 30.0));
    assert_eq!(shapes[3].2.fill_opacity, 0.75);
    assert!(shapes[4].2.fill.is_none());
    assert_eq!(shapes[4].2.stroke.as_ref().unwrap().join, Join::Round);
    assert_eq!(shapes[5].2.stroke.as_ref().unwrap().cap, Cap::Round);
}

#[test]
fn basic_shapes_raster() {
    let d = parse(BASIC).unwrap();
    let img = rasterize(&d, 200, 100, 1.0);
    let bg = [0x20 as f32 / 255.0, 0x28 as f32 / 255.0, 0x30 as f32 / 255.0, 1.0];
    assert!(near(px(&img, 2, 2), bg, 1e-3));
    // Inside the box: tomato at group opacity 0.8 over the background.
    let tomato = [1.0, 99.0 / 255.0, 71.0 / 255.0];
    let want = [0, 1, 2].map(|i| tomato[i] * 0.8 + bg[i] * 0.2);
    let got = px(&img, 45, 35);
    assert!(near(got, [want[0], want[1], want[2], 1.0], 2e-3), "{got:?} vs {want:?}");
    // Scaled ×2: the same point lands at double the coordinates, edges stay sharp.
    let big = rasterize(&d, 400, 200, 2.0);
    assert!(near(px(&big, 90, 70), got, 2e-3));
}

#[test]
fn paths_gradients_css_use() {
    let d = parse(PATHS).unwrap();
    assert_eq!(d.pixel_size(), (160, 120));
    let shapes = d.flatten();
    assert_eq!(shapes.len(), 6, "{:?}", shapes.iter().map(|s| &s.2.name).collect::<Vec<_>>());
    // CSS class → gradient fill.
    let Some(Paint::Gradient(g)) = &shapes[0].2.fill else { panic!("gradient expected") };
    assert!(g.bbox_units);
    assert_eq!(g.stops.len(), 2);
    assert_eq!(shapes[1].2.fill_rule, FillRule::EvenOdd);
    // #wave: id selector beats nothing; stroke from CSS.
    assert!(shapes[2].2.fill.is_none());
    assert_eq!(shapes[2].2.stroke.as_ref().unwrap().width, 5.0);
    // use → the referenced path with the use's fill, translated.
    assert_eq!(shapes[4].2.fill, Some(Paint::Color([128.0 / 255.0, 0.0, 128.0 / 255.0])));
    let p = shapes[4].0 * kurbo::Point::new(0.0, 0.0);
    assert!((p.x - 100.0).abs() < 1e-9 && (p.y - 110.0).abs() < 1e-9);
    assert_eq!(shapes[5].2.stroke.as_ref().unwrap().dash, Some((vec![4.0, 2.0], 0.0)));

    let img = rasterize(&d, 160, 120, 1.0);
    // Vertical gradient: yellow at the top, red at the bottom.
    let top = px(&img, 40, 11);
    let bottom = px(&img, 40, 48);
    assert!(top[1] > 0.7 && bottom[1] < 0.3, "{top:?} {bottom:?}");
    // Even-odd hole.
    assert_eq!(px(&img, 120, 35)[3], 0.0);
    assert!(px(&img, 95, 15)[3] > 0.99);
    // Radial gradient: white centre.
    let c = px(&img, 40, 90);
    assert!(c[0] > 0.9 && c[3] > 0.9, "{c:?}");
}

#[test]
fn root_svg_percentages_use_the_document_viewport() {
    for (attrs, width, height) in [
        ("width=\"100\" height=\"80\"", 100.0, 80.0),
        ("width=\"50%\" height=\"200%\"", 150.0, 300.0),
        ("", 300.0, 150.0),
        ("width=\"100\" height=\"80\" viewBox=\"0 0 200 160\"", 200.0, 160.0),
        ("width=\"NaN%\" height=\"1e400\"", 300.0, 150.0),
    ] {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {attrs}><rect x="25%" y="25%" width="50%" height="50%"/></svg>"#);
        let d = parse(svg.as_bytes()).unwrap();
        let shapes = d.flatten();
        assert_eq!(shapes.len(), 1);
        assert!(
            matches!(shapes[0].2.geom, Geom::Rect { x, y, w, h, .. }
            if x == width / 4.0 && y == height / 4.0 && w == width / 2.0 && h == height / 2.0),
            "{attrs}: {:?}",
            shapes[0].2.geom
        );
    }
}

#[test]
fn nested_svg_percentages_use_the_nearest_viewport() {
    let d = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80" viewBox="0 0 100 80">
            <svg x="10" y="10" width="40" height="20">
                <rect id="inner" width="50%" height="50%" fill="red"/>
                <svg x="50%" width="50%" height="50%">
                    <rect id="deep" width="50%" height="50%" fill="blue"/>
                </svg>
            </svg>
            <rect id="outer" y="60" width="50%" height="10" fill="green"/>
            <svg x="60" width="40" height="20" viewBox="0 0 200 100">
                <rect id="viewbox" width="50%" height="50%"/>
            </svg>
        </svg>"#,
    )
    .unwrap();
    let shapes = d.flatten();
    assert_eq!(shapes.len(), 4);
    for ((_, _, shape), (name, width, height)) in
        shapes.iter().zip([("inner", 20.0, 10.0), ("deep", 10.0, 5.0), ("outer", 50.0, 10.0), ("viewbox", 100.0, 50.0)])
    {
        assert_eq!(shape.name, name);
        assert!(matches!(shape.geom, Geom::Rect { w, h, .. } if w == width && h == height), "{name}: {:?}", shape.geom);
    }
    let img = rasterize(&d, 100, 80, 1.0);
    assert_eq!(px(&img, 15, 15), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(px(&img, 35, 12), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(px(&img, 35, 17)[3], 0.0);
    assert_eq!(px(&img, 45, 65)[3], 1.0);
    assert_eq!(px(&img, 55, 65)[3], 0.0);
}

#[test]
fn non_finite_viewboxes_do_not_panic_or_produce_nan_geometry() {
    for attrs in ["viewBox=\"0 0 1e400 100\"", "viewBox=\"0 0 100 1e400\"", "viewBox=\"-1e400 0 100 100\""] {
        for shape in ["<rect width=\"0%\" height=\"50%\"/>", "<circle r=\"0%\"/>"] {
            let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {attrs}>{shape}<rect width="50%" height="50%"/></svg>"#);
            let d = parse(svg.as_bytes()).unwrap();
            assert_eq!((d.width, d.height), (300.0, 150.0));
            let shapes = d.flatten();
            assert_eq!(shapes.len(), 1);
            assert!(matches!(shapes[0].2.geom, Geom::Rect { x, y, w, h, rx, ry }
                if [x, y, w, h, rx, ry].iter().all(|v| v.is_finite()) && w == 150.0 && h == 75.0));
            assert!(shapes[0].0.as_coeffs().iter().all(|v| v.is_finite()));
        }
    }
}

#[test]
fn non_finite_nested_viewports_do_not_panic_or_produce_nan_geometry() {
    for attrs in ["width=\"1e400\" height=\"10\"", "width=\"10\" height=\"1e400\""] {
        for shape in ["<rect width=\"0%\" height=\"50%\"/>", "<circle r=\"0%\"/>"] {
            let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><svg {attrs}>{shape}<rect width="50%" height="50%"/></svg></svg>"#);
            let d = parse(svg.as_bytes()).unwrap();
            let shapes = d.flatten();
            assert_eq!(shapes.len(), 1);
            assert!(matches!(shapes[0].2.geom, Geom::Rect { x, y, w, h, rx, ry }
                if [x, y, w, h, rx, ry].iter().all(|v| v.is_finite()) && w > 0.0 && h > 0.0));
            assert!(shapes[0].0.as_coeffs().iter().all(|v| v.is_finite()));
        }
    }
}

#[test]
fn zero_size_nested_viewport_draws_nothing() {
    for attrs in ["width=\"0\" height=\"10\"", "width=\"10\" height=\"0\""] {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><svg {attrs}><rect width="50%" height="50%"/></svg></svg>"#);
        assert!(parse(svg.as_bytes()).unwrap().flatten().is_empty(), "{attrs}");
    }
}

#[test]
fn malformed_input() {
    assert!(parse(b"<html/>").is_err());
    assert!(parse(b"not xml").is_err());
    let d = parse(b"<svg xmlns='http://www.w3.org/2000/svg'><path d='M0 0 L 10'/><rect width='-5' height='5'/><text>hi</text></svg>").unwrap();
    assert_eq!(d.pixel_size(), (300, 150));
    assert_eq!(d.skipped, vec!["text".to_string()]);
    assert!(looks_like_svg(BASIC));
    assert!(!looks_like_svg(b"\x89PNG"));
}

#[test]
fn transforms() {
    let m = parse_transform("translate(10,20) rotate(90) scale(2 3)");
    let p = m * kurbo::Point::new(1.0, 1.0);
    assert!((p.x - 7.0).abs() < 1e-9 && (p.y - 22.0).abs() < 1e-9, "{p:?}");
    let m = parse_transform("matrix(1 0 0 1 5 6) skewX(45)");
    let p = m * kurbo::Point::new(0.0, 1.0);
    assert!((p.x - 6.0).abs() < 1e-9 && (p.y - 7.0).abs() < 1e-9);
}

#[test]
fn deep_nesting_errors_instead_of_overflowing_the_stack() {
    // 100 000 nested groups overflowed the XML parser's stack and aborted the process.
    let deep = |n: usize| format!("<svg xmlns=\"http://www.w3.org/2000/svg\">{}<rect width=\"5\" height=\"5\"/>{}</svg>", "<g>".repeat(n), "</g>".repeat(n));
    assert!(matches!(crate::parse(deep(100_000).as_bytes()), Err(crate::Error::Xml(_))));
    assert!(crate::parse(deep(100).as_bytes()).is_ok());
    // Comments, CDATA, quoted '>' and self-closing tags don't count.
    let s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><!-- {} --><![CDATA[{}]]><g title=\"a>b\">{}</g></svg>",
        "<g>".repeat(1000),
        "<g>".repeat(1000),
        "<rect/>".repeat(1000)
    );
    assert_eq!(crate::xml_depth(&s), 2);
    assert!(crate::parse(s.as_bytes()).is_ok());
}

/// How deep `g`'s groups nest, and how many shapes it holds.
fn depth_and_shapes(g: &Group) -> (usize, usize) {
    let mut depth = 0;
    let mut shapes = 0;
    for c in &g.children {
        match c {
            Node::Group(sub) => {
                let (d, s) = depth_and_shapes(sub);
                depth = depth.max(d + 1);
                shapes += s;
            }
            Node::Shape(_) => shapes += 1,
            Node::Image(_) => {}
        }
    }
    (depth, shapes)
}

#[test]
fn branching_use_references_stop_at_the_expansion_budget() {
    // Each group uses the one before twice: 30 levels are 2^30 copies of one rectangle, from a
    // file of a few kilobytes (#1236).
    let mut defs = String::from(r#"<g id="g0"><rect width="1" height="1"/></g>"#);
    for i in 1..=30 {
        defs += &format!(r##"<g id="g{i}"><use href="#g{p}"/><use href="#g{p}" x="1"/></g>"##, p = i - 1);
    }
    let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>{defs}</defs><use href="#g30"/></svg>"##);
    let t0 = std::time::Instant::now();
    let e = parse(svg.as_bytes()).unwrap_err();
    assert!(e.to_string().contains("expand to more than"), "{e}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(10), "{:?}", t0.elapsed());
    // Below the budget the same pattern imports whole: 2^10 rectangles.
    let small = svg.replace(r##"<use href="#g30"/></svg>"##, r##"<use href="#g10"/></svg>"##);
    let doc = parse(small.as_bytes()).unwrap();
    assert_eq!(depth_and_shapes(&doc.root).1, 1024);
}

#[test]
fn mutually_referencing_uses_are_cut_at_the_first_repeat() {
    // A uses B and B uses A, neither inside the other: once each, not down to the depth limit.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>
        <g id="a"><rect width="1" height="1"/><use href="#b"/></g>
        <g id="b"><rect width="2" height="2"/><use href="#a"/></g>
    </defs><use href="#a"/></svg>"##;
    let doc = parse(svg.as_bytes()).unwrap();
    let (depth, shapes) = depth_and_shapes(&doc.root);
    assert_eq!(shapes, 2, "a's and b's rectangle, once each");
    assert!(depth <= 6, "{depth}");
}

#[test]
fn a_symbol_used_a_thousand_times_still_imports() {
    let uses: String = (0..1000).map(|i| format!(r##"<use href="#icon" x="{i}"/>"##)).collect();
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs><symbol id="icon"><rect width="1" height="1"/><circle r="1"/><ellipse rx="1" ry="2"/></symbol></defs>{uses}</svg>"##
    );
    let doc = parse(svg.as_bytes()).unwrap();
    assert_eq!(depth_and_shapes(&doc.root).1, 3000);
}

/// An SVG with one `<style>` sheet and one rectangle per given class list.
fn styled(css: &str, rects: &[&str]) -> Vec<u8> {
    let rects: String = rects.iter().map(|attrs| format!(r#"<rect width="1" height="1" {attrs}/>"#)).collect();
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>{css}</style>{rects}</svg>"#).into_bytes()
}

#[test]
fn comma_selector_lists_share_one_declaration_block() {
    // Before #2140 every selector of a comma list got its own copy of the declaration block, so
    // `a,a,a…{…}` stored selectors × declarations strings; one selector past the cap is refused.
    let sels = vec!["rect"; MAX_CSS_SELECTORS + 1].join(",");
    let decls: String = (0..10).map(|i| format!("p{i}:v;")).collect();
    let svg = styled(&format!("{sels}{{fill:red;{decls}}}"), &[""]);
    let e = parse(&svg).unwrap_err();
    assert!(e.to_string().contains("selectors"), "{e}");
}

#[test]
fn a_style_sheet_with_too_many_declarations_errors() {
    let decls: String = (0..=MAX_CSS_DECLARATIONS).map(|i| format!("p{i}:v;")).collect();
    let svg = styled(&format!("rect{{{decls}}}"), &[""]);
    let e = parse(&svg).unwrap_err();
    assert!(e.to_string().contains("declarations"), "{e}");
}

#[test]
fn selector_lists_keep_the_css_cascade() {
    let green = Some(Paint::Color([0.0, 1.0, 0.0]));
    let blue = Some(Paint::Color([0.0, 0.0, 1.0]));
    let red = Some(Paint::Color([1.0, 0.0, 0.0]));
    let css = "rect, .b, #c { fill: #00ff00 } .b, .d { fill: #0000ff; stroke: #000000; stroke-width: 3 } rect.e, #c { fill: #ff0000 }";
    let svg = styled(css, &["", r#"class="b""#, r#"id="c" class="b""#, r#"class="b" style="fill:#00ff00""#, r##"class="e" fill="#0000ff""##, r#"class="d""#]);
    let doc = parse(&svg).unwrap();
    let shapes = doc.flatten();
    let fills: Vec<_> = shapes.iter().map(|s| s.2.fill.clone()).collect();
    // Type selector; class beats type; id beats class; inline style beats every rule; a later,
    // more specific rule beats the presentation attribute.
    assert_eq!(fills, vec![green.clone(), blue.clone(), red.clone(), green, red, blue]);
    assert!(shapes[0].2.stroke.is_none());
    // The second list's block reaches both of its selectors.
    for i in [1, 5] {
        assert_eq!(shapes[i].2.stroke.as_ref().map(|s| s.width), Some(3.0), "shape {i}");
    }
}

#[test]
fn a_comma_list_stores_its_declarations_once() {
    let sels = vec!["rect"; 1000].join(",");
    let decls: String = (0..50).map(|i| format!("p{i}:v;")).collect();
    let mut sheet = StyleSheet::default();
    parse_css(&format!("{sels}{{{decls}}}"), &mut sheet).unwrap();
    assert_eq!(sheet.rules.len(), 1000);
    assert_eq!(sheet.blocks.len(), 1);
    assert_eq!(sheet.decls, 50);
}

#[test]
fn rules_of_unsupported_selectors_store_no_blocks() {
    let mut sheet = StyleSheet::default();
    parse_css(&"a b{}".repeat(100_000), &mut sheet).unwrap();
    parse_css("a b{fill:red} a>b, rect{fill:blue} c:hover, d{fill:red}", &mut sheet).unwrap();
    assert_eq!(sheet.rules.len(), 2);
    assert_eq!(sheet.blocks.len(), 2);
    assert!(sheet.rules.iter().all(|r| sheet.blocks.get(r.block).is_some_and(|b| b.len() == 1)));
}

#[test]
fn huge_class_lists_count_against_the_matching_budget() {
    // One rule, but a 1 MB class attribute re-split on each of 100 `<use>` copies (#2140).
    let class = "a ".repeat(500_000);
    let uses: String = (0..100).map(|i| format!(r##"<use href="#r" x="{i}"/>"##)).collect();
    let svg = |uses: &str| {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>.z{{fill:red}}</style><defs><rect id="r" width="1" height="1" class="{class}"/></defs>{uses}</svg>"##
        )
    };
    let e = parse(svg(&uses).as_bytes()).unwrap_err();
    assert!(e.to_string().contains("<style> rules"), "{e}");
    let few: String = uses.split_inclusive("/>").take(10).collect();
    assert!(parse(svg(&few).as_bytes()).is_ok());
}

#[test]
fn a_selector_with_many_classes_counts_against_the_matching_budget() {
    // `.z.z.z…` lands in one class bucket, but each match checks every class it lists.
    let rule = ".z".repeat(300_000);
    let uses: String = (0..200).map(|i| format!(r##"<use href="#r" x="{i}"/>"##)).collect();
    let svg = |uses: &str| {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><style>{rule}{{fill:red}}</style><defs><rect id="r" width="1" height="1" class="z"/></defs>{uses}</svg>"##
        )
    };
    let e = parse(svg(&uses).as_bytes()).unwrap_err();
    assert!(e.to_string().contains("<style> rules"), "{e}");
    let few: String = uses.split_inclusive("/>").take(10).collect();
    assert!(parse(svg(&few).as_bytes()).is_ok());
}

#[test]
fn matching_work_past_the_budget_errors_without_doing_it() {
    // Within both sheet caps, yet one rectangle would apply 10 000 x 5 001 declarations.
    let sels = vec!["rect"; 10_000].join(",");
    let decls: String = (0..5_001).map(|i| format!("p{i}:v;")).collect();
    let svg = styled(&format!("{sels}{{{decls}}}"), &[""]);
    let e = parse(&svg).unwrap_err();
    assert!(e.to_string().contains("<style> rules"), "{e}");
    // The same sheet styles an element it does not match at no cost.
    let ok = styled(&format!("{}{{{decls}}}", vec!["circle"; 10_000].join(",")), &[""]);
    assert!(parse(&ok).is_ok());
}

#[test]
fn font_size_percentage_resolves_against_the_parent_font_size() {
    for (body, expected) in [
        (r#"<g font-size="200%"><rect width="1em" height="1em"/></g>"#, 32.0),
        (r#"<g font-size="200%"><rect width="2ex" height="1em"/></g>"#, 32.0),
        (r#"<g font-size="200%"><g font-size="200%"><rect width="1em" height="1em"/></g></g>"#, 64.0),
    ] {
        let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{body}</svg>"#);
        let d = parse(svg.as_bytes()).unwrap();
        let shapes = d.flatten();
        assert_eq!(shapes.len(), 1);
        assert!(matches!(shapes[0].2.geom, Geom::Rect { w, .. } if w == expected), "{body}: {:?}", shapes[0].2.geom);
    }
}
