use super::*;
use kurbo::Shape as _;

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
    assert!(matches!(try_rasterize(&doc, 16_384, 16_384, 1.0), Err(RasterError::SurfaceLimit(_))), "the refusal is reported, not silent");
}

/// Pattern fills and strokes repeat from the tile origin.
#[test]
fn static_pattern_fill_and_stroke_repeat_with_tile_origin() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="4">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" x="1" width="2" height="2">
                <rect x="1" width="1" height="2" fill="red"/>
                <rect x="2" width="1" height="2" fill="blue"/>
            </pattern></defs>
            <rect id="filled" width="8" height="4" fill="url(#p)"/>
            <path d="M0 3 H8" fill="none" stroke="url(#p)" stroke-width="2"/>
        </svg>"#,
    )
    .unwrap();
    let shapes = doc.flatten();
    assert!(matches!(shapes[0].2.fill.as_ref(), Some(Paint::Pattern(_))));
    assert!(matches!(shapes[1].2.stroke.as_ref().map(|s| &s.paint), Some(Paint::Pattern(_))));

    let img = rasterize(&doc, 8, 4, 1.0);
    // The tile occupies [1, 3): blue wraps into x=0, red starts at x=1.
    assert!(near(px(&img, 0, 1), [0.0, 0.0, 1.0, 1.0], 1e-3));
    assert!(near(px(&img, 1, 1), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 2, 1), [0.0, 0.0, 1.0, 1.0], 1e-3));
    // The same pattern server paints the stroke.
    assert!(near(px(&img, 1, 2), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// objectBoundingBox pattern units map tile and content coordinates to the shape's box.
#[test]
fn object_bounding_box_pattern_units_map_tile_and_content_coordinates() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="8">
            <defs><pattern id="p" patternUnits="objectBoundingBox" width="0.5" height="1" patternContentUnits="objectBoundingBox">
                <rect width="25%" height="100%" fill="green"/>
            </pattern></defs>
            <rect x="10" y="2" width="8" height="4" fill="url(#p)" stroke="url(#p)" stroke-width="1"/>
        </svg>"#,
    )
    .unwrap();
    let shapes = doc.flatten();
    assert!(matches!(shapes[0].2.fill.as_ref(), Some(Paint::Pattern(_))));
    assert!(matches!(shapes[0].2.stroke.as_ref().map(|stroke| &stroke.paint), Some(Paint::Pattern(_))));
    let img = rasterize(&doc, 20, 8, 1.0);
    assert!(near(px(&img, 10, 3), [0.0, 128.0 / 255.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 12, 3)[3], 0.0);
    assert!(near(px(&img, 14, 3), [0.0, 128.0 / 255.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 16, 3)[3], 0.0);
}

/// patternTransform scales tile spacing and content together.
#[test]
fn pattern_transform_scales_tile_spacing_and_its_content_together() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="2">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="2" height="2" patternTransform="scale(2)">
                <rect width="1" height="2" fill="red"/>
            </pattern></defs>
            <rect width="8" height="2" fill="url(#p)"/>
        </svg>"#,
    )
    .unwrap();
    let img = rasterize(&doc, 8, 2, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 1, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 2, 0)[3], 0.0);
    assert_eq!(px(&img, 3, 0)[3], 0.0);
    assert!(near(px(&img, 4, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// A pattern href inherits units and content, and plain `href` wins over `xlink:href`.
#[test]
fn pattern_href_inherits_units_and_content_and_prefers_plain_href() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="2">
            <defs>
                <pattern id="base" patternUnits="userSpaceOnUse" width="2" height="2">
                    <rect width="1" height="2" fill="red"/>
                </pattern>
                <pattern id="other" patternUnits="userSpaceOnUse" width="2" height="2">
                    <rect width="2" height="2" fill="blue"/>
                </pattern>
                <pattern id="derived" href="#base" xlink:href="#other"/>
                <pattern id="xlink-derived" xlink:href="#base"/>
            </defs>
            <rect width="8" height="2" fill="url(#derived)"/>
            <rect x="8" width="8" height="2" fill="url(#xlink-derived)"/>
        </svg>"##,
    )
    .unwrap();
    let img = rasterize(&doc, 16, 2, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 1, 0)[3], 0.0);
    assert!(near(px(&img, 2, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 8, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 9, 0)[3], 0.0);
}

/// Pattern content inherits styles from the pattern's ancestors.
#[test]
fn pattern_content_inherits_styles_from_its_ancestors() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2" fill="red">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="2" height="2">
                <rect width="2" height="2"/>
            </pattern></defs>
            <rect width="2" height="2" fill="url(#p) blue"/>
        </svg>"#,
    )
    .unwrap();
    let img = rasterize(&doc, 2, 2, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// Content reached through `href` inherits styles from the pattern that owns it, not the referrer.
#[test]
fn pattern_href_content_inherits_styles_from_the_owning_pattern() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <defs>
                <g fill="red"><pattern id="base" patternUnits="userSpaceOnUse" width="2" height="2">
                    <rect width="2" height="2"/>
                </pattern></g>
                <pattern id="derived" href="#base"/>
            </defs>
            <rect width="2" height="2" fill="url(#derived)"/>
        </svg>"##,
    )
    .unwrap();
    let img = rasterize(&doc, 2, 2, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// Descriptive children (title, desc) do not count as inheritable pattern content.
#[test]
fn pattern_href_ignores_descriptive_children_when_inheriting_content() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <defs>
                <pattern id="base" patternUnits="userSpaceOnUse" width="2" height="2">
                    <rect width="2" height="2" fill="red"/>
                </pattern>
                <pattern id="derived" href="#base">
                    <title>Documentation only</title>
                    <desc>Still documentation only</desc>
                    <metadata><author>Test</author></metadata>
                </pattern>
            </defs>
            <rect width="2" height="2" fill="url(#derived)"/>
        </svg>"##,
    )
    .unwrap();
    let img = rasterize(&doc, 2, 2, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// The pattern cache is shared for equal viewports but split for different percentage-sized ones.
#[test]
fn cached_patterns_reuse_same_viewport_but_separate_percentage_sized_viewports() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="10" viewBox="0 0 120 10">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="50%" height="100%">
                <rect width="4" height="100%" fill="red"/>
            </pattern></defs>
            <svg width="40" height="10" viewBox="0 0 40 10">
                <rect id="small-one" width="20" height="10" fill="url(#p) blue"/>
                <rect id="small-two" x="20" width="20" height="10" fill="url(#p) green"/>
            </svg>
            <svg x="40" width="80" height="10" viewBox="0 0 80 10">
                <rect id="large" width="80" height="10" fill="url(#p) yellow"/>
            </svg>
        </svg>"##,
    )
    .unwrap();
    let shapes = doc.flatten();
    let Some((_, _, small_one)) = shapes.iter().find(|(_, _, shape)| shape.name == "small-one") else { panic!("small-one expected") };
    let Some(Paint::Pattern(small_one)) = small_one.fill.as_ref() else { panic!("small-one pattern expected") };
    let Some((_, _, small_two)) = shapes.iter().find(|(_, _, shape)| shape.name == "small-two") else { panic!("small-two expected") };
    let Some(Paint::Pattern(small_two)) = small_two.fill.as_ref() else { panic!("small-two pattern expected") };
    let Some((_, _, large)) = shapes.iter().find(|(_, _, shape)| shape.name == "large") else { panic!("large expected") };
    let Some(Paint::Pattern(large)) = large.fill.as_ref() else { panic!("large pattern expected") };
    assert_eq!(small_one.width, 20.0);
    assert_eq!(small_two.width, 20.0);
    assert_eq!(large.width, 40.0);
    assert_eq!(small_one.fallback, Some([0.0, 0.0, 1.0, 1.0]));
    assert_eq!(small_two.fallback, Some([0.0, 128.0 / 255.0, 0.0, 1.0]));
    assert_eq!(large.fallback, Some([1.0, 1.0, 0.0, 1.0]));
    assert!(std::sync::Arc::ptr_eq(&small_one.content, &small_two.content));
    assert!(!std::sync::Arc::ptr_eq(&small_one.content, &large.content));

    let img = rasterize(&doc, 120, 10, 1.0);
    for x in [1, 21, 41, 81] {
        assert!(near(px(&img, x, 5), [1.0, 0.0, 0.0, 1.0], 1e-3), "x={x}");
    }
    for x in [5, 25, 45, 85] {
        assert_eq!(px(&img, x, 5)[3], 0.0, "x={x}");
    }
}

/// Different symbol viewports use distinct pattern cache entries for percentage sizes.
#[test]
fn symbol_viewports_select_distinct_percentage_sized_pattern_cache_entries() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="10">
            <defs>
                <pattern id="p" patternUnits="userSpaceOnUse" width="50%" height="100%">
                    <rect width="4" height="100%" fill="red"/>
                </pattern>
                <symbol id="small-symbol" viewBox="0 0 20 10">
                    <rect id="small-symbol-shape" width="20" height="10" fill="url(#p) blue"/>
                </symbol>
                <symbol id="large-symbol" viewBox="0 0 40 10">
                    <rect id="large-symbol-shape" width="40" height="10" fill="url(#p) green"/>
                </symbol>
            </defs>
            <use href="#small-symbol" width="20" height="10"/>
            <use href="#large-symbol" x="20" width="40" height="10"/>
        </svg>"##,
    )
    .unwrap();
    let shapes = doc.flatten();
    let Some((_, _, small)) = shapes.iter().find(|(_, _, shape)| shape.name == "small-symbol-shape") else { panic!("small symbol shape expected") };
    let Some(Paint::Pattern(small)) = small.fill.as_ref() else { panic!("small symbol pattern expected") };
    let Some((_, _, large)) = shapes.iter().find(|(_, _, shape)| shape.name == "large-symbol-shape") else { panic!("large symbol shape expected") };
    let Some(Paint::Pattern(large)) = large.fill.as_ref() else { panic!("large symbol pattern expected") };
    assert_eq!(small.width, 10.0);
    assert_eq!(large.width, 20.0);

    let img = rasterize(&doc, 60, 10, 1.0);
    assert!(near(px(&img, 1, 5), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 5, 5)[3], 0.0);
    assert!(near(px(&img, 21, 5), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert_eq!(px(&img, 25, 5)[3], 0.0);
    assert!(near(px(&img, 41, 5), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// Bare `url()` and href values without `#` do not resolve to local pattern ids.
#[test]
fn bare_urls_and_hrefs_do_not_resolve_as_local_pattern_ids() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="2">
            <defs>
                <pattern id="target" patternUnits="userSpaceOnUse" width="2" height="2">
                    <rect width="2" height="2" fill="red"/>
                </pattern>
                <pattern id="bare-href" href="target" width="2" height="2"/>
            </defs>
            <rect width="2" height="2" fill="url(target) blue"/>
            <rect x="2" width="2" height="2" fill="url(#bare-href) green"/>
            <rect x="4" width="2" height="2" fill="url(#target)"/>
        </svg>"##,
    )
    .unwrap();
    let shapes = doc.flatten();
    assert_eq!(shapes[0].2.fill, Some(Paint::Color([0.0, 0.0, 1.0])));
    assert_eq!(shapes[1].2.fill, Some(Paint::Color([0.0, 128.0 / 255.0, 0.0])));
    assert!(matches!(shapes[2].2.fill.as_ref(), Some(Paint::Pattern(_))));
    let img = rasterize(&doc, 6, 2, 1.0);
    assert!(near(px(&img, 0, 0), [0.0, 0.0, 1.0, 1.0], 1e-3));
    assert!(near(px(&img, 2, 0), [0.0, 128.0 / 255.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 4, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
}

/// A valid but empty pattern paints nothing rather than its fallback.
#[test]
fn empty_valid_pattern_is_transparent_instead_of_using_its_fallback() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2">
            <defs><pattern id="empty" patternUnits="userSpaceOnUse" width="2" height="2"/></defs>
            <rect width="4" height="2" fill="url(#empty) red"/>
        </svg>"##,
    )
    .unwrap();
    let Some(Paint::Pattern(pattern)) = doc.flatten()[0].2.fill.as_ref() else { panic!("valid pattern expected") };
    assert_eq!(pattern.fallback, Some([1.0, 0.0, 0.0, 1.0]));
    let img = rasterize(&doc, 4, 2, 1.0);
    assert_eq!(px(&img, 0, 0)[3], 0.0);
    assert_eq!(px(&img, 3, 0)[3], 0.0);
}

/// Pattern content attribute work stays bounded across many distinct viewports.
#[test]
fn pattern_content_attribute_work_is_bounded_across_distinct_viewports() {
    let large_attribute = "x".repeat(2 * 1024 * 1024);
    let shapes: String = (0..12)
        .map(|index| format!(r##"<svg x="{index}" width="1" height="1" viewBox="0 0 {} 1"><rect width="1" height="1" fill="url(#p)"/></svg>"##, index + 1))
        .collect();
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="1"><defs>
            <pattern id="p" data-repeat="{large_attribute}" patternUnits="userSpaceOnUse" width="50%" height="100%">
                <rect width="1" height="1" fill="red"/>
            </pattern></defs>{shapes}</svg>"##
    );
    let error = parse(svg.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("<pattern> references scans more than"), "{error}");
}

/// Exceeding the pattern href depth limit uses the paint fallback.
#[test]
fn pattern_href_depth_limit_uses_paint_fallback() {
    let patterns: String = (0..=16)
        .map(|index| {
            if index == 16 {
                format!(r#"<pattern id="p{index}" patternUnits="userSpaceOnUse" width="1" height="1"/>"#)
            } else {
                format!(r##"<pattern id="p{index}" href="#p{}"/>"##, index + 1)
            }
        })
        .collect();
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><defs>{patterns}</defs><rect width="1" height="1" fill="url(#p0) blue"/></svg>"##
    );
    let doc = parse(svg.as_bytes()).unwrap();
    assert_eq!(doc.flatten()[0].2.fill, Some(Paint::Color([0.0, 0.0, 1.0])));
}

/// Ancestor attribute work for patterns stays bounded across many distinct viewports.
#[test]
fn pattern_ancestor_attribute_work_is_bounded_across_distinct_viewports() {
    let large_attribute = "x".repeat(2 * 1024 * 1024);
    let shapes: String = (0..12)
        .map(|index| format!(r##"<svg x="{index}" width="1" height="1" viewBox="0 0 {} 1"><rect width="1" height="1" fill="url(#p)"/></svg>"##, index + 1))
        .collect();
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="1"><defs data-repeat="{large_attribute}">
            <pattern id="p" patternUnits="userSpaceOnUse" width="50%" height="100%">
                <rect width="1" height="1" fill="red"/>
            </pattern></defs>{shapes}</svg>"##
    );
    let error = parse(svg.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("<pattern> references scans more than"), "{error}");
}

/// Pattern tile alpha composites as premultiplied color.
#[test]
fn pattern_tile_alpha_composites_as_premultiplied_color() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1">
                <rect width="1" height="1" fill="red" fill-opacity="0.5"/>
            </pattern></defs>
            <rect width="2" height="2" fill="url(#p)"/>
        </svg>"#,
    )
    .unwrap();
    let img = rasterize(&doc, 2, 2, 1.0);
    assert!(near(px(&img, 0, 0), [0.5, 0.0, 0.0, 0.5], 1e-3));
    assert!(near(px(&img, 1, 1), [0.5, 0.0, 0.0, 0.5], 1e-3));
}

/// An oversized pattern tile uses the fallback without allocating it.
#[test]
fn oversized_pattern_tile_falls_back_without_allocating() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
            <defs><pattern id="huge" patternUnits="userSpaceOnUse" width="1e300" height="1e300">
                <rect width="1" height="1" fill="red"/>
            </pattern></defs>
            <rect width="1" height="1" fill="url(#huge) gold"/>
        </svg>"##,
    )
    .unwrap();
    let img = rasterize(&doc, 1, 1, 1.0);
    assert!(near(px(&img, 0, 0), [1.0, 215.0 / 255.0, 0.0, 1.0], 1e-3));
}

/// Invalid or cyclic patterns use the paint fallback without being expanded.
#[test]
fn invalid_or_cyclic_pattern_uses_paint_fallback_without_expanding() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="2">
            <defs>
                <pattern id="bad" width="0" height="1"/>
                <pattern id="viewbox" width="1" height="1" viewBox="0 0 1 1"/>
                <pattern id="collapsed" patternUnits="userSpaceOnUse" width="1" height="1" patternTransform="scale(0)"/>
                <pattern id="a" href="#b"/>
                <pattern id="b" href="#a"/>
                <pattern id="self" href="#self"/>
            </defs>
            <rect width="2" height="2" fill="url(#bad) blue"/>
            <rect x="2" width="2" height="2" fill="url(#viewbox) red"/>
            <rect x="4" width="2" height="2" fill="url(#collapsed) yellow"/>
            <rect x="6" width="2" height="2" fill="url(#a) green"/>
            <rect x="8" width="2" height="2" fill="url(#self) cyan"/>
        </svg>"##,
    )
    .unwrap();
    let shapes = doc.flatten();
    assert_eq!(shapes[0].2.fill, Some(Paint::Color([0.0, 0.0, 1.0])));
    assert_eq!(shapes[1].2.fill, Some(Paint::Color([1.0, 0.0, 0.0])));
    assert_eq!(shapes[2].2.fill, Some(Paint::Color([1.0, 1.0, 0.0])));
    assert_eq!(shapes[3].2.fill, Some(Paint::Color([0.0, 128.0 / 255.0, 0.0])));
    assert_eq!(shapes[4].2.fill, Some(Paint::Color([0.0, 1.0, 1.0])));
    let img = rasterize(&doc, 10, 2, 1.0);
    assert!(near(px(&img, 0, 0), [0.0, 0.0, 1.0, 1.0], 1e-3));
    assert!(near(px(&img, 3, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 5, 0), [1.0, 1.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 7, 0), [0.0, 128.0 / 255.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 9, 0), [0.0, 1.0, 1.0, 1.0], 1e-3));
}

/// A one-pixel pattern tiles a large rectangle from a single surface.
#[test]
fn one_pixel_pattern_tiles_a_large_rectangle_from_one_surface() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1">
                <rect width="1" height="1" fill="red"/>
            </pattern></defs>
            <rect width="1024" height="1024" fill="url(#p)"/>
        </svg>"#,
    )
    .unwrap();
    let img = rasterize(&doc, 1024, 1024, 1.0);
    assert_eq!((img.width, img.height), (1024, 1024));
    for (x, y) in [(0, 0), (511, 511), (1023, 1023)] {
        assert!(near(px(&img, x, y), [1.0, 0.0, 0.0, 1.0], 1e-3), "({x}, {y})");
    }
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

/// Repeated gradient paints share one cached stop list.
#[test]
fn repeated_gradient_paints_share_their_cached_stop_storage() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2">
            <defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/>
            </linearGradient></defs>
            <rect id="first" width="2" height="2" fill="url(#g)"/>
            <rect id="second" x="2" width="2" height="2" fill="url(#g)"/>
        </svg>"#,
    )
    .unwrap();
    let shapes = doc.flatten();
    let Some((_, _, first)) = shapes.iter().find(|(_, _, shape)| shape.name == "first") else { panic!("first shape expected") };
    let Some(Paint::Gradient(first_gradient)) = first.fill.as_ref() else { panic!("first gradient expected") };
    let Some((_, _, second)) = shapes.iter().find(|(_, _, shape)| shape.name == "second") else { panic!("second shape expected") };
    let Some(Paint::Gradient(second_gradient)) = second.fill.as_ref() else { panic!("second gradient expected") };
    assert!(std::sync::Arc::ptr_eq(&first_gradient.stops, &second_gradient.stops));
}

/// Oversized path geometry fails with a bounded import error.
#[test]
fn oversized_svg_path_geometry_fails_with_a_bounded_import_error() {
    let data = format!("M0 0{}", "h1".repeat(pathdata::MAX_PATH_ELEMENTS + 1));
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><path d="{data}"/></svg>"#);
    let error = parse(svg.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("bounded path-data work budget"), "{error}");

    let points = (0..=pathdata::MAX_PATH_ELEMENTS).map(|x| format!("{x} 0")).collect::<Vec<_>>().join(" ");
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><polyline points="{points}"/></svg>"#);
    let error = parse(svg.as_bytes()).unwrap_err();
    assert!(error.to_string().contains("bounded path-data work budget"), "{error}");
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

/// A symbol viewport sets percentage pattern units and the cache key.
#[test]
fn symbol_viewport_sets_percentage_pattern_units_and_cache_key() {
    let doc = parse(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="10">
            <defs>
                <pattern id="p" patternUnits="userSpaceOnUse" width="50%" height="100%">
                    <rect width="4" height="100%" fill="red"/>
                </pattern>
                <symbol id="small" viewBox="0 0 20 10">
                    <rect id="small-shape" width="20" height="10" fill="url(#p)"/>
                </symbol>
                <symbol id="large" viewBox="0 0 40 10">
                    <rect id="large-shape" width="40" height="10" fill="url(#p)"/>
                </symbol>
            </defs>
            <use href="#small" width="20" height="10"/>
            <use href="#large" x="20" width="40" height="10"/>
        </svg>"##,
    )
    .unwrap();
    let shapes = doc.flatten();
    let Some((_, _, small)) = shapes.iter().find(|(_, _, shape)| shape.name == "small-shape") else { panic!("small symbol shape expected") };
    let Some(Paint::Pattern(small)) = small.fill.as_ref() else { panic!("small symbol pattern expected") };
    let Some((_, _, large)) = shapes.iter().find(|(_, _, shape)| shape.name == "large-shape") else { panic!("large symbol shape expected") };
    let Some(Paint::Pattern(large)) = large.fill.as_ref() else { panic!("large symbol pattern expected") };
    assert_eq!(small.width, 10.0);
    assert_eq!(large.width, 20.0);
}

/// Non-finite opacity values never write NaN pixels.
#[test]
fn non_finite_opacity_values_do_not_write_nan_pixels() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="6" height="2">
            <defs><linearGradient id="g"><stop offset="0" stop-color="red" stop-opacity="NaN"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
            <rect width="2" height="2" fill="red" fill-opacity="NaN"/>
            <rect x="2" width="2" height="2" fill="blue" opacity="1e400"/>
            <rect x="4" width="2" height="2" fill="url(#g)"/>
        </svg>"#,
    )
    .unwrap();
    let img = rasterize(&doc, 6, 2, 1.0);
    assert!(img.data.iter().flatten().all(|channel| channel.is_finite()));
    assert!(near(px(&img, 0, 0), [1.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(near(px(&img, 3, 1), [0.0, 0.0, 1.0, 1.0], 1e-3));
    assert!(px(&img, 4, 1)[3].is_finite());
}

/// Basic shapes whose derived bounds overflow are skipped.
#[test]
fn basic_shapes_with_overflowing_derived_bounds_are_skipped() {
    let doc = parse(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <rect x="1e308" y="0" width="1e308" height="1" fill="red"/>
            <circle cx="1e308" cy="0" r="1e308" fill="blue"/>
            <circle cx="0" cy="0" r="1e300" fill="green"/>
        </svg>"#,
    )
    .unwrap();

    assert!(doc.flatten().is_empty());
    let image = rasterize(&doc, 2, 2, 1.0);
    assert!(image.data.iter().flatten().all(|channel| channel.is_finite()));
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
    // The first element must be the `svg`: an HTML page with an inline SVG is not one.
    assert!(!looks_like_svg(b"<html><body><svg/></body></html>"));
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
fn nested_entities_are_refused_instead_of_expanding_without_bound() {
    // #2129: each top-level `&b;` stays under roxmltree's per-reference cap but adds 12 500
    // nodes, so 40 KB of references expands to 5 million.
    let dtd = format!("<!ENTITY a \"{}\"><!ENTITY b \"{}\">", "<g/>".repeat(50), "&a;".repeat(250));
    let bomb = format!("<?xml version=\"1.0\"?><!DOCTYPE svg [{dtd}]><svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>", "&b;".repeat(400));
    assert!(matches!(crate::parse(bomb.as_bytes()), Err(crate::Error::Xml(_))));
    // An ordinary entity, as Illustrator declares them, still parses.
    let ok = "<?xml version=\"1.0\"?><!DOCTYPE svg [<!ENTITY w \"40\">]><svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"&w;\" height=\"5\"/></svg>";
    assert!(crate::parse(ok.as_bytes()).is_ok());
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
    assert!(!craft_svg::guard::deeper_than(&s, 3) && craft_svg::guard::deeper_than(&s, 2));
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

#[test]
fn at_rule_blocks_are_skipped_by_brace_depth() {
    let mut sheet = StyleSheet::default();
    parse_css("@media print { .a{fill:#ff0000} .b{fill:#0000ff} } .c{fill:red}", &mut sheet).unwrap();
    assert_eq!(sheet.rules.len(), 1);
    assert_eq!(sheet.rules[0].classes, vec!["c".to_string()]);
    let mut sheet = StyleSheet::default();
    parse_css("@media x { @supports y { .c{} } .d{} } .e{fill:red}", &mut sheet).unwrap();
    assert_eq!(sheet.rules.len(), 1);
    assert_eq!(sheet.rules[0].classes, vec!["e".to_string()]);
}

/// Build a `w` x `h` document from `nodes` for sparse-content render tests.
fn sparse_doc(w: f64, h: f64, nodes: Vec<Node>) -> Doc {
    let mut root = Group::new("root");
    root.children = nodes;
    Doc { width: w, height: h, root, skipped: Vec::new() }
}

/// A tiny image on a large canvas is still drawn.
#[test]
fn sparse_minified_image_on_a_large_canvas_is_still_drawn() {
    let image = Image {
        name: "i".into(),
        transform: Affine::scale(1.0 / 8.0),
        width: 8,
        height: 8,
        rgba: std::sync::Arc::new([255u8, 0, 0, 255].repeat(64)),
        opacity: 1.0,
    };
    let doc = sparse_doc(2049.0, 2049.0, vec![Node::Image(image)]);
    let img = try_rasterize(&doc, 2049, 2049, 1.0).expect("sparse image is within budget");
    assert!(px(&img, 0, 0)[3] > 0.99, "the covered pixel must not be dropped by whole-canvas budgeting");
}

/// A small shape with a many-element clip is still drawn.
#[test]
fn sparse_shape_with_a_many_element_clip_is_still_drawn() {
    let mut clip = kurbo::BezPath::new();
    clip.move_to((0.0, 0.0));
    for i in 0..1026 {
        clip.line_to((2.0 * (i % 2) as f64, 2.0 * ((i + 1) % 2) as f64));
    }
    clip.line_to((2.0, 2.0));
    clip.line_to((0.0, 2.0));
    clip.close_path();
    assert!(clip.elements().len() > 1000);
    let mut g = Group::new("clipped");
    g.clip = vec![(clip, FillRule::NonZero)];
    let doc = parse(br#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512"><rect width="2" height="2" fill="red"/></svg>"#).unwrap();
    g.children = doc.root.children.clone();
    let doc = sparse_doc(512.0, 512.0, vec![Node::Group(g)]);
    let img = try_rasterize(&doc, 512, 512, 1.0).expect("sparse clip is within budget");
    assert!(img.data.iter().any(|p| p[3] > 0.5), "clip work is charged by clip area, not canvas area");
}

/// Sparse pattern fills on a 4K canvas all render.
#[test]
fn sparse_pattern_fills_on_a_4k_canvas_all_render() {
    let rects: String = (0..6).map(|i| format!(r#"<rect x="{}" y="0" width="2" height="2" fill="url(#p)"/>"#, i * 4)).collect();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="3840" height="2160">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1"><rect width="1" height="1" fill="red"/></pattern></defs>
            {rects}</svg>"#
    );
    let doc = parse(svg.as_bytes()).unwrap();
    let img = try_rasterize(&doc, 3840, 2160, 1.0).expect("sparse patterns are within budget");
    for i in 0..6 {
        assert!(px(&img, i * 4, 0)[3] > 0.99, "rect {i} must be painted");
    }
}

/// A genuinely huge covered area is still refused.
#[test]
fn genuinely_huge_covered_area_is_still_refused() {
    // A 16392x1 strip squeezed 8:1 across the whole canvas needs 8x8 taps per pixel everywhere.
    let image = Image {
        name: "i".into(),
        transform: Affine::scale_non_uniform(1.0 / 8.0, 2049.0),
        width: 16392,
        height: 1,
        rgba: std::sync::Arc::new([255u8, 0, 0, 255].repeat(16392)),
        opacity: 1.0,
    };
    let doc = sparse_doc(2049.0, 2049.0, vec![Node::Image(image)]);
    assert_eq!(
        try_rasterize(&doc, 2049, 2049, 1.0).err(),
        Some(RasterError::WorkLimit("image sampling")),
        "a fully covered 64-tap canvas exceeds the work cap"
    );
    let img = rasterize(&doc, 2049, 2049, 1.0);
    assert!(img.data.iter().all(|p| p[3] == 0.0), "the lossy wrapper drops the refused image");
}

/// A group of `count` 1x1 red rects, each inside its own tiny clip, spread over a large canvas.
fn tiny_clipped_rects(count: usize) -> Vec<Node> {
    (0..count)
        .map(|i| {
            let (x, y) = ((i % 100) as f64 * 4.0, (i / 100) as f64 * 4.0);
            let mut g = Group::new("clipped");
            g.clip = vec![(kurbo::Rect::new(x, y, x + 2.0, y + 2.0).to_path(0.01), FillRule::NonZero)];
            let mut rect = Group::new("r");
            rect.children = parse(
                format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect x="{x}" y="{y}" width="1" height="1" fill="red"/></svg>"#)
                    .as_bytes(),
            )
            .map(|d| d.root.children)
            .unwrap_or_default();
            g.children = rect.children;
            Node::Group(g)
        })
        .collect()
}

/// Thousands of tiny clipped items on a big canvas cost region-proportional work: a clipped
/// group works on a crop of its clip region, so the render finishes quickly and completely.
#[test]
fn thousands_of_tiny_clips_on_a_large_canvas_finish_quickly() {
    let doc = sparse_doc(2048.0, 2048.0, tiny_clipped_rects(3000));
    let start = std::time::Instant::now();
    let img = try_rasterize(&doc, 2048, 2048, 1.0).expect("tiny clips stay within every budget");
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "region-bounded work: {:?}", start.elapsed());
    assert!(px(&img, 0, 0)[3] > 0.99 && px(&img, 396, 28 * 4)[3] > 0.99);
}

/// Thousands of tiny pattern-filled shapes and tiny images on a big canvas finish quickly because
/// masks and scans cover only each item's region.
#[test]
fn thousands_of_tiny_pattern_and_image_items_on_a_large_canvas_finish_quickly() {
    let mut nodes = Vec::new();
    let rects: String = (0..3000).map(|i| format!(r#"<rect x="{}" y="{}" width="2" height="2" fill="url(#p)"/>"#, (i % 100) * 4, (i / 100) * 4)).collect();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="3840" height="2160">
            <defs><pattern id="p" patternUnits="userSpaceOnUse" width="1" height="1"><rect width="1" height="1" fill="red"/></pattern></defs>
            {rects}</svg>"#
    );
    nodes.extend(parse(svg.as_bytes()).unwrap().root.children);
    for i in 0..3000 {
        nodes.push(Node::Image(Image {
            name: "i".into(),
            transform: Affine::translate(((i % 100) as f64 * 4.0, 100.0 + (i / 100) as f64 * 4.0)),
            width: 1,
            height: 1,
            rgba: std::sync::Arc::new(vec![0, 255, 0, 255]),
            opacity: 1.0,
        }));
    }
    let doc = sparse_doc(3840.0, 2160.0, nodes);
    let start = std::time::Instant::now();
    let img = try_rasterize(&doc, 3840, 2160, 1.0).expect("tiny items stay within every budget");
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "region-bounded work: {:?}", start.elapsed());
    assert!(px(&img, 396, 0)[3] > 0.99 && px(&img, 396, 100)[3] > 0.99);
}

#[test]
fn gradient_stops_shared_across_references_are_not_charged_per_copy() {
    // 4,000 stops referenced by 2,100 shapes would be 8.4 million stops if copied, but resolved
    // gradients share one stop array, so the import keeps only the 4,000 and succeeds.
    let stops: String = (0..4000).map(|i| format!("<stop offset=\"{}\" stop-color=\"red\"/>", i as f64 / 4000.0)).collect();
    let rects = "<rect width=\"1\" height=\"1\" fill=\"url(#g)\"/>".repeat(2100);
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\"><defs><linearGradient id=\"g\">{stops}</linearGradient></defs>{rects}</svg>");
    assert!(parse(svg.as_bytes()).is_ok());
}

#[test]
fn nested_knockout_groups_stop_at_the_draw_budget() {
    // Each knockout level draws its children twice and each non-isolated level twice more:
    // 30 nested levels would be 4^30 draws. The draw budget ends it. The wait is long because the
    // budget is a count of draws, not time: the render took 1 s on a quiet CI machine and over 20 s
    // on a loaded one, and 4^30 draws would never finish at all.
    let mut g = Group::new("leaf");
    g.children.push(Node::Shape(Shape {
        name: "s".into(),
        geom: Geom::Rect { x: 0.0, y: 0.0, w: 4.0, h: 4.0, rx: 0.0, ry: 0.0 },
        transform: Affine::IDENTITY,
        fill: Some(Paint::Color([1.0, 0.0, 0.0])),
        fill_opacity: 1.0,
        fill_rule: FillRule::NonZero,
        stroke: None,
        opacity: 1.0,
    }));
    for _ in 0..30 {
        let mut p = Group::new("k");
        p.knockout = true;
        p.isolated = false;
        p.children.push(Node::Group(g));
        g = p;
    }
    let doc = Doc { width: 4.0, height: 4.0, root: g, skipped: vec![] };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(rasterize(&doc, 4, 4, 1.0).width).ok());
    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(120)).expect("rendering finishes"), 4);
}
