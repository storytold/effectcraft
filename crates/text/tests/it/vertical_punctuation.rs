use effectcraft_keyframe::TextDoc;
use effectcraft_text::{fonts, kurbo, layout_doc};

#[test]
fn japanese_vertical_punctuation_uses_alternates_at_the_top_right() {
    fonts::scan_system();
    // This integration test owns its font database; probing a native face cannot
    // affect the unit tests that verify fallback selection.
    let face = fonts::face(fonts::fallback_for('国', fonts::resolve("Inter", "Regular").face));
    if !"国、。「」ー".chars().all(|c| face.has_char(c)) || !(face.has_feature(b"vert") || face.has_feature(b"vrt2")) {
        eprintln!("SKIPPED: no Japanese font with vertical alternates available");
        return;
    }
    let horizontal =
        TextDoc { text: "国、。「」ーABC".into(), font: face.info.family.clone(), style: face.info.style.clone(), size: 48.0, ..Default::default() };
    let h = layout_doc(&horizontal);
    let vertical = TextDoc { vertical: true, ..horizontal.clone() };
    let v = layout_doc(&vertical);
    assert_eq!(v.glyphs.len(), h.glyphs.len());
    assert_eq!(v.glyphs[0].gid, h.glyphs[0].gid);
    for i in 1..6 {
        assert_ne!(v.glyphs[i].gid, h.glyphs[i].gid, "{}: vertical form for glyph {i}", face.info.family);
    }
    for i in [1, 2] {
        let g = &v.glyphs[i];
        let ink = kurbo::Shape::bounding_box(&g.path);
        assert!(ink.center().x > 0.35 * g.size, "{}: punctuation on the right: {ink:?}", face.info.family);
        assert!(ink.center().y < g.size / 2.0, "{}: punctuation at the top: {ink:?}", face.info.family);
    }
    // Half-width Latin remains sideways, and horizontal cache entries stay horizontal.
    for i in 6..9 {
        assert_eq!(v.glyphs[i].gid, h.glyphs[i].gid);
    }
    let h_again = layout_doc(&horizontal);
    assert_eq!(h_again.glyphs[1].gid, h.glyphs[1].gid);
    assert_eq!(h_again.glyphs[1].path, h.glyphs[1].path);
    let tracked = {
        let mut d = vertical.clone();
        d.apply_style_all(|s| s.tracking = 200.0);
        layout_doc(&d)
    };
    assert_eq!(tracked.glyphs[1].path, v.glyphs[1].path, "tracking must not move punctuation across its column");
    eprintln!("verified vertical punctuation with {} / {}", face.info.family, face.info.style);
}
