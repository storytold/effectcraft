use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, ShapePath, TextDoc, Value};
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, MaskMode, Node, Project, PropGroup};
use effectcraft_text::layout_doc;
use effectcraft_time::{FrameRate, Tick};

use super::*;

fn setup(w: u32, h: u32) -> (Project, ItemId, Comp) {
    let mut p = Project::default();
    let comp = Comp::new(w, h, FrameRate::FPS_30, Tick::from_seconds_f64(4.0));
    let cid = p.add_item("Comp", Label::Sandstone, None, ItemKind::Comp(comp.clone().into()));
    (p, cid, comp)
}

fn text_layer(p: &mut Project, comp: &Comp, doc: TextDoc) -> Layer {
    let mut l = build::layer(p, comp, "Text", LayerSource::Text, (comp.width, comp.height), None);
    l.props.prop_mut("text/sourceText").unwrap().value = Value::Text(Box::new(doc));
    l
}

/// Add an animator with `kinds` and selectors (`range`/`wiggly`/`expression`); returns its index.
fn animator(p: &mut Project, l: &mut Layer, kinds: &[&str], sels: &[&str]) -> usize {
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let props = kinds.iter().flat_map(|k| build::text_anim_props(&mut ids, k, false)).collect();
    let mut g = build::text_animator(&mut ids, "Animator", props);
    let sg = g.sub_mut("selectors").unwrap();
    for (i, k) in sels.iter().enumerate() {
        if i == 0 && *k == "range" {
            continue;
        }
        if i == 0 {
            sg.children.clear();
        }
        sg.children.push(build::text_selector(&mut ids, k, k).unwrap().into());
    }
    p.next_id = next;
    let anims = l.props.group_mut("text/animators").unwrap();
    anims.children.push(g.into());
    anims.children.len()
}

fn set(l: &mut Layer, path: &str, v: Value) {
    l.props.prop_mut(path).unwrap_or_else(|| panic!("no {path}")).value = v;
}

fn ctx<'a>(p: &'a Project, cid: ItemId, t: f64) -> EvalCtx<'a> {
    let comp = p.comp(cid).unwrap();
    EvalCtx::new(p, cid, comp, Tick::from_seconds_f64(t))
}

fn selection(p: &Project, cid: ItemId, t: f64, anim: usize) -> Vec<f64> {
    let c = ctx(p, cid, t);
    let l = &p.comp(cid).unwrap().layers[0];
    let doc = source_text(&c, l).unwrap();
    let lay = layout_doc(&doc);
    let a = l.props.group(&format!("text/animators/#{anim}")).unwrap();
    animator_selection(&c, l, a, &lay).into_iter().map(|v| (v[0] * 1000.0).round() / 1000.0).collect()
}

fn doc(text: &str, size: f64) -> TextDoc {
    TextDoc { text: text.into(), size, ..Default::default() }
}

#[test]
fn range_based_on_words_and_lines() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, doc("AB CD\nEF", 30.0));
    let a = animator(&mut p, &mut l, &["opacity"], &["range"]);
    set(&mut l, "text/animators/#1/selectors/#1/advanced/basedOn", Value::Enum(2));
    set(&mut l, "text/animators/#1/selectors/#1/advanced/smoothness", Value::Scalar(0.0));
    set(&mut l, "text/animators/#1/selectors/#1/end", Value::Scalar(34.0));
    p.comp_mut(cid).unwrap().layers.push(l);
    // Glyphs: A B ␠ C D E F (the newline has no glyph). Word 1 only.
    let s = selection(&p, cid, 0.0, a);
    assert_eq!(s, vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0], "{s:?}");
    // Lines: the first half = line 1.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/selectors/#1/advanced/basedOn", Value::Enum(3));
    set(l, "text/animators/#1/selectors/#1/end", Value::Scalar(50.0));
    let s = selection(&p, cid, 0.0, a);
    assert_eq!(s, vec![1.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0], "{s:?}");
    // Characters excluding spaces: 6 units, first half = A B C.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/selectors/#1/advanced/basedOn", Value::Enum(1));
    let s = selection(&p, cid, 0.0, a);
    assert_eq!(s[0..2], [1.0, 1.0]);
    assert_eq!(s[3], 1.0);
    assert_eq!(s[4..], [0.0, 0.0, 0.0]);
    // Index units: start 1, end 3 characters.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/selectors/#1/advanced/basedOn", Value::Enum(0));
    set(l, "text/animators/#1/selectors/#1/advanced/units", Value::Enum(1));
    set(l, "text/animators/#1/selectors/#1/start", Value::Scalar(1.0));
    set(l, "text/animators/#1/selectors/#1/end", Value::Scalar(3.0));
    let s = selection(&p, cid, 0.0, a);
    assert_eq!(s, vec![0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0], "{s:?}");
}

#[test]
fn range_start_end_and_offset_are_capped() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, doc("ABCD", 30.0));
    let a = animator(&mut p, &mut l, &["opacity"], &["range"]);
    set(&mut l, "text/animators/#1/selectors/#1/advanced/smoothness", Value::Scalar(0.0));
    // End 300% is capped at 100% (Offset -150% at -100%): the range is -100%..0% and selects nothing.
    // Uncapped it would be -150%..150% and select everything.
    set(&mut l, "text/animators/#1/selectors/#1/end", Value::Scalar(300.0));
    set(&mut l, "text/animators/#1/selectors/#1/offset", Value::Scalar(-150.0));
    p.comp_mut(cid).unwrap().layers.push(l);
    assert_eq!(selection(&p, cid, 0.0, a), vec![0.0; 4]);
    // Start -200% is capped at 0%: with Offset 100% the range is 100%..150% and selects nothing.
    // Uncapped it would be -100%..150% and select everything.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/selectors/#1/start", Value::Scalar(-200.0));
    set(l, "text/animators/#1/selectors/#1/end", Value::Scalar(50.0));
    set(l, "text/animators/#1/selectors/#1/offset", Value::Scalar(100.0));
    assert_eq!(selection(&p, cid, 0.0, a), vec![0.0; 4]);
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/selectors/#1/offset", Value::Scalar(0.0));
    assert_eq!(selection(&p, cid, 0.0, a), vec![1.0, 1.0, 0.0, 0.0]);
}

#[test]
fn selector_modes_and_randomize() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, doc("ABCDEFGH", 30.0));
    let a = animator(&mut p, &mut l, &["opacity"], &["range", "range"]);
    let s1 = "text/animators/#1/selectors/#1";
    let s2 = "text/animators/#1/selectors/#2";
    set(&mut l, &format!("{s1}/end"), Value::Scalar(50.0));
    set(&mut l, &format!("{s2}/end"), Value::Scalar(25.0));
    set(&mut l, &format!("{s2}/advanced/mode"), Value::Enum(1)); // subtract
    p.comp_mut(cid).unwrap().layers.push(l);
    assert_eq!(selection(&p, cid, 0.0, a), vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    let modes = [
        (2, vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        (4, vec![1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
        (5, vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
    ];
    for (m, want) in modes {
        set(&mut p.comp_mut(cid).unwrap().layers[0], &format!("{s2}/advanced/mode"), Value::Enum(m));
        assert_eq!(selection(&p, cid, 0.0, a), want, "mode {m}");
    }
    // A lone Subtract selector inverts.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    if let Some(Node::Group(g)) = l.props.group_mut("text/animators/#1/selectors").map(|g| &mut g.children[1]) {
        g.enabled = false;
    }
    set(l, &format!("{s1}/advanced/mode"), Value::Enum(1));
    assert_eq!(selection(&p, cid, 0.0, a), vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    // Randomize Order: still half selected, but not the first half; seed changes it.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, &format!("{s1}/advanced/mode"), Value::Enum(0));
    set(l, &format!("{s1}/advanced/randomize"), Value::Bool(true));
    let r1 = selection(&p, cid, 0.0, a);
    assert_eq!(r1.iter().sum::<f64>(), 4.0);
    assert_ne!(r1, vec![1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(r1, selection(&p, cid, 0.0, a));
    set(&mut p.comp_mut(cid).unwrap().layers[0], &format!("{s1}/advanced/randomSeed"), Value::Scalar(5.0));
    assert_ne!(selection(&p, cid, 0.0, a), r1);
}

#[test]
fn wiggly_selector_is_deterministic_and_time_dependent() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, doc("WIGGLE", 40.0));
    animator(&mut p, &mut l, &["position"], &["range", "wiggly"]);
    set(&mut l, "text/animators/#1/properties/position", Value::Vec3([0.0, 50.0, 0.0]));
    p.comp_mut(cid).unwrap().layers.push(l);
    let l = &p.comp(cid).unwrap().layers[0];
    let a = glyph_paths(&ctx(&p, cid, 0.5), l);
    let b = glyph_paths(&ctx(&p, cid, 0.5), l);
    let c = glyph_paths(&ctx(&p, cid, 1.3), l);
    let ys = |v: &[(BezPath, CharXf)]| v.iter().map(|g| g.1.offset[1]).collect::<Vec<_>>();
    assert_eq!(ys(&a), ys(&b));
    assert_ne!(ys(&a), ys(&c));
    // Within ±50 and not all identical across characters (correlation 50%).
    assert!(ys(&a).iter().all(|y| y.abs() <= 50.0 + 1e-9));
    assert!(ys(&a).windows(2).any(|w| (w[0] - w[1]).abs() > 1e-6));
    // The layer cache keys a wiggly text layer by time.
    let k1 = crate::cache::layer_key(&ctx(&p, cid, 0.5), l, 1.0, false, false);
    let k2 = crate::cache::layer_key(&ctx(&p, cid, 1.3), l, 1.0, false, false);
    assert_ne!(k1, k2);
    // Renders identically twice.
    let r1 = crate::render_frame(&p, cid, Tick::from_seconds_f64(0.5), 0.5);
    let r2 = crate::render_frame(&p, cid, Tick::from_seconds_f64(0.5), 0.5);
    assert_eq!(r1.data, r2.data);
}

fn bounds_of(paths: &[(BezPath, CharXf)]) -> kurbo::Rect {
    let v: Vec<BezPath> = paths.iter().map(|p| p.0.clone()).collect();
    effectcraft_path::bounds(&v).unwrap()
}

#[test]
fn tracking_line_anchor_and_character_offset() {
    let (mut p, cid, comp) = setup(600, 200);
    let mut l = text_layer(&mut p, &comp, doc("ABCD", 40.0));
    animator(&mut p, &mut l, &["tracking", "lineAnchor"], &["range"]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let w0 = bounds_of(&glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]));
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/animators/#1/properties/tracking", Value::Scalar(500.0));
    let w1 = bounds_of(&glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]));
    // 4 chars × 500/1000 em × 40 px: the last glyph moves ~ 3.5 × 20 px further (before & after).
    assert!((w1.width() - w0.width() - 3.0 * 20.0).abs() < 1.5, "{} {}", w0.width(), w1.width());
    assert!((w1.x0 - w0.x0 - 10.0).abs() < 1.0, "first char shifts by half its tracking: {} {}", w0.x0, w1.x0);
    // Line Anchor 100%: tracking grows to the left.
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/animators/#1/properties/lineAnchor", Value::Scalar(100.0));
    let w2 = bounds_of(&glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]));
    assert!((w2.x1 - w0.x1 + 10.0).abs() < 1.0, "{} {}", w0.x1, w2.x1);

    // Character Offset 1: "ABCD" draws as "BCDE".
    let (mut p, cid, comp) = setup(600, 200);
    let mut l = text_layer(&mut p, &comp, doc("AZ9", 40.0));
    animator(&mut p, &mut l, &["characterOffset"], &["range"]);
    set(&mut l, "text/animators/#1/properties/characterOffset", Value::Scalar(1.0));
    set(&mut l, "text/animators/#1/properties/characterAlignment", Value::Enum(3));
    p.comp_mut(cid).unwrap().layers.push(l);
    let got = bounds_of(&glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]));
    let (mut q, qid, qc) = setup(600, 200);
    let l2 = text_layer(&mut q, &qc, doc("BA0", 40.0));
    q.comp_mut(qid).unwrap().layers.push(l2);
    let want = bounds_of(&glyph_paths(&ctx(&q, qid, 0.0), &q.comp(qid).unwrap().layers[0]));
    assert!((got.width() - want.width()).abs() < 0.01 && (got.height() - want.height()).abs() < 0.01, "{got:?} {want:?}");
    assert_eq!(substitute('z', &CharXf { char_offset: 2.0, ..Default::default() }), Some('b'));
    assert_eq!(substitute('-', &CharXf { char_offset: 2.0, ..Default::default() }), None);
    assert_eq!(substitute('-', &CharXf { char_offset: 2.0, char_range: 1, ..Default::default() }), Some('/'));
    assert_eq!(substitute('a', &CharXf { char_value: Some((66.0, 1.0)), ..Default::default() }), Some('B'));
}

/// Without a Line Anchor, tracking grows each line from where its paragraph's alignment pins it:
/// left text grows right, centred text around its centre, right text to the left (#146).
#[test]
fn tracking_grows_from_each_paragraphs_alignment() {
    use effectcraft_keyframe::{Justify, ParaStyle};
    let tracked = |d: TextDoc, amount: f64| {
        let (mut p, cid, comp) = setup(600, 300);
        let mut l = text_layer(&mut p, &comp, d);
        animator(&mut p, &mut l, &["tracking"], &["range"]);
        set(&mut l, "text/animators/#1/properties/tracking", Value::Scalar(amount));
        p.comp_mut(cid).unwrap().layers.push(l);
        glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0])
    };
    // 4 chars × 500/1000 em × 40 px: 20 px of tracking each, half before and half after.
    for (justify, x0, centre, x1) in [(Justify::Left, 10.0, 40.0, 70.0), (Justify::Center, -30.0, 0.0, 30.0), (Justify::Right, -70.0, -40.0, -10.0)] {
        let d = TextDoc { justify, ..doc("ABCD", 40.0) };
        let (w0, w1) = (bounds_of(&tracked(d.clone(), 0.0)), bounds_of(&tracked(d, 500.0)));
        let got = [w1.x0 - w0.x0, w1.center().x - w0.center().x, w1.x1 - w0.x1];
        assert!(got.iter().zip([x0, centre, x1]).all(|(g, w)| (g - w).abs() < 1.0), "{justify:?}: moved {got:?}, want {:?}", [x0, centre, x1]);
    }
    // Each paragraph by its own alignment: a left paragraph above a right-aligned one.
    let mut d = doc("ABCD\nABCD", 40.0);
    let left = d.base_para();
    d.set_paras(vec![left.clone(), ParaStyle { justify: Justify::Right, ..left }]);
    let lines = |paths: Vec<(BezPath, CharXf)>| -> [kurbo::Rect; 2] {
        let (a, b): (Vec<_>, Vec<_>) = paths.into_iter().partition(|g| bounds_of(std::slice::from_ref(g)).center().y < 10.0);
        [bounds_of(&a), bounds_of(&b)]
    };
    let ([a0, b0], [a1, b1]) = (lines(tracked(d.clone(), 0.0)), lines(tracked(d, 500.0)));
    assert!((a1.x0 - a0.x0 - 10.0).abs() < 1.0, "the left paragraph grows right: {} {}", a0.x0, a1.x0);
    assert!((b1.x1 - b0.x1 + 10.0).abs() < 1.0, "the right paragraph grows left: {} {}", b0.x1, b1.x1);
}

#[test]
fn anchor_grouping_line_rotates_around_line_centre() {
    let (mut p, cid, comp) = setup(600, 300);
    let mut l = text_layer(&mut p, &comp, doc("HHHH", 40.0));
    animator(&mut p, &mut l, &["rotation"], &["range"]);
    set(&mut l, "text/animators/#1/properties/rotation", Value::Scalar(90.0));
    p.comp_mut(cid).unwrap().layers.push(l);
    let per_char = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    let cx: Vec<f64> = per_char.iter().map(|g| effectcraft_path::bounds(std::slice::from_ref(&g.0)).unwrap().center().x).collect();
    assert!(cx.windows(2).all(|w| w[1] > w[0] + 10.0), "characters rotate in place: {cx:?}");
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/moreOptions/anchorGrouping", Value::Enum(2));
    let line = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    let cx: Vec<f64> = line.iter().map(|g| effectcraft_path::bounds(std::slice::from_ref(&g.0)).unwrap().center().x).collect();
    assert!(cx.windows(2).all(|w| (w[1] - w[0]).abs() < 1.0), "the line turns as one: {cx:?}");
    let cy: Vec<f64> = line.iter().map(|g| effectcraft_path::bounds(std::slice::from_ref(&g.0)).unwrap().center().y).collect();
    assert!(cy.windows(2).all(|w| w[1] > w[0] + 10.0), "{cy:?}");
}

fn mask(p: &mut Project, l: &mut Layer, pts: &[[f64; 2]], closed: bool) {
    let mut next = p.next_id;
    let mut ids = Ids(&mut next);
    let sp = ShapePath { vertices: pts.to_vec(), in_tangents: vec![[0.0; 2]; pts.len()], out_tangents: vec![[0.0; 2]; pts.len()], closed, feather: Vec::new() };
    let g = build::mask(&mut ids, "Mask 1", sp, MaskMode::None, [255, 255, 0]);
    p.next_id = next;
    let masks = l.props.group_mut("masks").unwrap();
    masks.children.push(g.into());
}

#[test]
fn path_text_follows_a_mask() {
    let (mut p, cid, comp) = setup(400, 400);
    let mut l = text_layer(&mut p, &comp, doc("PATH", 30.0));
    // A vertical path going down at x = 100.
    mask(&mut p, &mut l, &[[100.0, 20.0], [100.0, 380.0]], false);
    set(&mut l, "text/pathOptions/path", Value::Enum(1));
    set(&mut l, "text/pathOptions/firstMargin", Value::Scalar(40.0));
    p.comp_mut(cid).unwrap().layers.push(l);
    let g = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    assert_eq!(g.len(), 4);
    let bs: Vec<kurbo::Rect> = g.iter().map(|x| effectcraft_path::bounds(std::slice::from_ref(&x.0)).unwrap()).collect();
    // Perpendicular: glyphs stand on the path, rotated 90° (to the right of x = 100), in order down.
    for b in &bs {
        assert!(b.x0 > 99.0 && b.x1 < 100.0 + 30.0, "{b:?}");
        assert!(b.height() < 30.0);
    }
    assert!(bs.windows(2).all(|w| w[1].center().y > w[0].center().y));
    assert!((bs[0].y0 - 60.0).abs() < 4.0, "first margin 40 from the start at y = 20: {:?}", bs[0]);
    // Reverse Path: starts from the bottom.
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/pathOptions/reversePath", Value::Bool(true));
    let g = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    let b0 = effectcraft_path::bounds(std::slice::from_ref(&g[0].0)).unwrap();
    assert!(b0.center().y > 300.0 && b0.x1 < 101.0, "{b0:?}");
    // Not perpendicular: glyphs stay upright.
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/pathOptions/perpendicular", Value::Bool(false));
    let g = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    let b0 = effectcraft_path::bounds(std::slice::from_ref(&g[0].0)).unwrap();
    assert!(b0.height() > b0.width(), "{b0:?}");
    // Force alignment spreads to the margins.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/pathOptions/reversePath", Value::Bool(false));
    set(l, "text/pathOptions/forceAlignment", Value::Bool(true));
    set(l, "text/pathOptions/firstMargin", Value::Scalar(0.0));
    let g = glyph_paths(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0]);
    let first = effectcraft_path::bounds(std::slice::from_ref(&g[0].0)).unwrap().center().y;
    let last = effectcraft_path::bounds(std::slice::from_ref(&g[3].0)).unwrap().center().y;
    assert!(first < 40.0 && last > 360.0, "{first} {last}");
}

fn alpha_bbox(img: &crate::Image) -> Option<[i64; 4]> {
    let mut b: Option<[i64; 4]> = None;
    for y in 0..img.height as i64 {
        for x in 0..img.width as i64 {
            if img.get(x, y)[3] > 0.2 {
                b = Some(match b {
                    None => [x, y, x, y],
                    Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)],
                });
            }
        }
    }
    b
}

#[test]
fn render_key_pixels_fill_hue_blur_and_fill_stroke_order() {
    let (mut p, cid, comp) = setup(200, 100);
    let mut l = text_layer(&mut p, &comp, TextDoc { text: "I".into(), size: 90.0, fill: [1.0, 0.0, 0.0, 1.0], style: "Bold".into(), ..Default::default() });
    set(&mut l, "transform/position", Value::Vec3([90.0, 80.0, 0.0]));
    animator(&mut p, &mut l, &["fillHue"], &["range"]);
    set(&mut l, "text/animators/#1/properties/fillHue", Value::Scalar(120.0));
    p.comp_mut(cid).unwrap().layers.push(l);
    let img = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    let b = alpha_bbox(&img).unwrap();
    let c = img.get((b[0] + b[2]) / 2, (b[1] + b[3]) / 2);
    assert!(c[1] > 0.9 && c[0] < 0.1, "hue +120° turns red green: {c:?}");
    // Fill Opacity 0 hides the fill.
    let mut l = p.comp(cid).unwrap().layers[0].clone();
    animator(&mut p, &mut l, &["fillOpacity"], &["range"]);
    set(&mut l, "text/animators/#2/properties/fillOpacity", Value::Scalar(0.0));
    p.comp_mut(cid).unwrap().layers[0] = l;
    let img = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    assert!(alpha_bbox(&img).is_none());
    p.comp_mut(cid).unwrap().layers[0].props.group_mut("text/animators").unwrap().children.pop();
    // Blur spreads the glyph.
    let sharp = alpha_bbox(&crate::render_frame(&p, cid, Tick::ZERO, 1.0)).unwrap();
    let mut l = p.comp(cid).unwrap().layers[0].clone();
    animator(&mut p, &mut l, &["blur"], &["range"]);
    set(&mut l, "text/animators/#2/properties/blur", Value::Vec2([20.0, 0.0]));
    p.comp_mut(cid).unwrap().layers[0] = l;
    let img = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    let soft = img.get(sharp[0] - 4, (sharp[1] + sharp[3]) / 2);
    assert!(soft[3] > 0.05, "blur reaches outside the sharp edge: {soft:?}");
    let core = img.get((sharp[0] + sharp[2]) / 2, (sharp[1] + sharp[3]) / 2);
    assert!(core[3] < 0.99, "and softens the core: {core:?}");
}

#[test]
fn fill_stroke_modes_order_layers() {
    // Two overlapping characters ("AA" with negative tracking), stroke on.
    let (mut p, cid, comp) = setup(300, 150);
    let d = TextDoc {
        text: "OO".into(),
        size: 100.0,
        tracking: -400.0,
        fill: [1.0, 0.0, 0.0, 1.0],
        stroke: [0.0, 0.0, 1.0, 1.0],
        apply_stroke: true,
        stroke_width: 6.0,
        stroke_over_fill: false,
        ..Default::default()
    };
    let mut l = text_layer(&mut p, &comp, d);
    set(&mut l, "transform/position", Value::Vec3([60.0, 110.0, 0.0]));
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = ctx(&p, cid, 0.0);
    let geom = text_geom(&c, &p.comp(cid).unwrap().layers[0]).unwrap();
    assert_eq!(geom.glyphs.len(), 2);
    let order = |fs: u32| {
        let mut g = text_geom(&c, &p.comp(cid).unwrap().layers[0]).unwrap();
        g.fill_stroke = fs;
        passes(&g)
    };
    assert_eq!(order(1), vec![(0, true), (1, true), (0, false), (1, false)]);
    assert_eq!(order(2), vec![(0, false), (1, false), (0, true), (1, true)]);
    assert_eq!(order(0), vec![(0, true), (0, false), (1, true), (1, false)]);
    // Inter-character blending: Multiply darkens where the characters overlap.
    let normal = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/moreOptions/interCharBlend", Value::Enum(2));
    let multiply = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    assert_ne!(normal.data, multiply.data);
}

#[test]
fn per_character_3d_projects_characters() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, TextDoc { text: "MM".into(), size: 80.0, justify: effectcraft_keyframe::Justify::Center, ..Default::default() });
    set(&mut l, "transform/position", Value::Vec3([200.0, 130.0, 0.0]));
    l.switches.three_d = true;
    animator(&mut p, &mut l, &["position", "rotation"], &["range"]);
    p.comp_mut(cid).unwrap().layers.push(l);
    let flat = alpha_bbox(&crate::render_frame(&p, cid, Tick::ZERO, 1.0)).unwrap();
    // Per-character 3D off: Z position is ignored.
    set(&mut p.comp_mut(cid).unwrap().layers[0], "text/animators/#1/properties/position", Value::Vec3([0.0, 0.0, -300.0]));
    let off = alpha_bbox(&crate::render_frame(&p, cid, Tick::ZERO, 1.0)).unwrap();
    assert_eq!(flat, off);
    // On: characters come towards the camera and get bigger.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    let t = l.props.sub_mut("text").unwrap();
    t.get_mut("perChar3d").unwrap().value = Value::Bool(true);
    let near = alpha_bbox(&crate::render_frame(&p, cid, Tick::ZERO, 1.0)).unwrap();
    assert!(near[2] - near[0] > (flat[2] - flat[0]) + 20, "{flat:?} {near:?}");
    assert_eq!(per_char_planes(&ctx(&p, cid, 0.0), &p.comp(cid).unwrap().layers[0], 1.0).len(), 2);
    // Y rotation 90° turns every character edge-on: nothing visible.
    let l = &mut p.comp_mut(cid).unwrap().layers[0];
    set(l, "text/animators/#1/properties/position", Value::Vec3([0.0; 3]));
    let mut next = p.next_id;
    let ry = build::text_anim_props(&mut Ids(&mut next), "rotation", true).remove(1);
    p.next_id = next;
    let props: &mut PropGroup = p.comp_mut(cid).unwrap().layers[0].props.group_mut("text/animators/#1/properties").unwrap();
    props.children.push(ry.into());
    props.get_mut("rotationY").unwrap().value = Value::Scalar(90.0);
    let img = crate::render_frame(&p, cid, Tick::ZERO, 1.0);
    let covered = |img: &crate::Image| img.data.iter().filter(|p| p[3] > 0.2).count();
    let vis = covered(&img);
    let flat_px = {
        let mut q = p.clone();
        q.comp_mut(cid).unwrap().layers[0].props.group_mut("text/animators/#1/properties").unwrap().get_mut("rotationY").unwrap().value = Value::Scalar(0.0);
        covered(&crate::render_frame(&q, cid, Tick::ZERO, 1.0))
    };
    assert!(vis * 10 < flat_px, "edge-on characters are thin slivers: {vis} of {flat_px}");
    // 45°: narrower than flat but visible.
    let props = p.comp_mut(cid).unwrap().layers[0].props.group_mut("text/animators/#1/properties").unwrap();
    props.get_mut("rotationY").unwrap().value = Value::Scalar(60.0);
    let b = alpha_bbox(&crate::render_frame(&p, cid, Tick::ZERO, 1.0)).unwrap();
    assert!(b[2] - b[0] < flat[2] - flat[0] && b[2] > b[0] + 10, "{b:?} {flat:?}");
}

#[test]
fn keyframed_selector_animates() {
    let (mut p, cid, comp) = setup(400, 200);
    let mut l = text_layer(&mut p, &comp, doc("ABCD", 40.0));
    animator(&mut p, &mut l, &["opacity"], &["range"]);
    l.props.prop_mut("text/animators/#1/selectors/#1/start").unwrap().keys =
        vec![Keyframe::new(Tick::ZERO, Value::Scalar(0.0)), Keyframe::new(Tick::from_seconds_f64(1.0), Value::Scalar(100.0))];
    p.comp_mut(cid).unwrap().layers.push(l);
    assert_eq!(selection(&p, cid, 0.0, 1), vec![1.0; 4]);
    assert_eq!(selection(&p, cid, 0.5, 1), vec![0.0, 0.0, 1.0, 1.0]);
    assert_eq!(selection(&p, cid, 1.0, 1), vec![0.0; 4]);
}

#[test]
fn caret_follows_animators_and_path_text() {
    let (mut p, cid, comp) = setup(400, 400);
    let mut l = text_layer(&mut p, &comp, doc("ABCD", 30.0));
    let a = animator(&mut p, &mut l, &["position"], &["range"]);
    set(&mut l, &format!("text/animators/#{a}/properties/position"), Value::Vec3([0.0, 50.0, 0.0]));
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = ctx(&p, cid, 0.0);
    let ly = &p.comp(cid).unwrap().layers[0];
    let lay = layout_doc(&source_text(&c, ly).unwrap());
    let maps = caret_maps(&c, ly, lay.chars);
    assert_eq!(maps.len(), 5);
    for (ci, m) in maps.iter().enumerate() {
        let (top, _) = lay.caret(ci);
        let q = m.apply(vec2(top.x, top.y));
        assert!((q.x - top.x).abs() < 1e-6 && (q.y - top.y - 50.0).abs() < 1e-6, "caret {ci}: {top:?} → {q:?}");
    }
    // On a vertical path (x = 100, running down) the caret turns with the characters and sits
    // on the path.
    let (mut p, cid, comp) = setup(400, 400);
    let mut l = text_layer(&mut p, &comp, doc("PATH", 30.0));
    mask(&mut p, &mut l, &[[100.0, 20.0], [100.0, 380.0]], false);
    set(&mut l, "text/pathOptions/path", Value::Enum(1));
    p.comp_mut(cid).unwrap().layers.push(l);
    let c = ctx(&p, cid, 0.0);
    let ly = &p.comp(cid).unwrap().layers[0];
    let lay = layout_doc(&source_text(&c, ly).unwrap());
    let maps = caret_maps(&c, ly, lay.chars);
    let (t, b) = lay.caret(2);
    let (qt, qb) = (maps[2].apply(vec2(t.x, t.y)), maps[2].apply(vec2(b.x, b.y)));
    assert!((qt.y - qb.y).abs() < 1.0 && (qt.x - qb.x).abs() > 20.0, "caret across the path: {qt:?} {qb:?}");
    assert!(qb.x < 100.5 && qt.x > 100.0, "baseline end on the path, top to its right: {qt:?} {qb:?}");
    let glyph_c = glyph_paths(&c, ly).iter().map(|g| effectcraft_path::bounds(std::slice::from_ref(&g.0)).unwrap().center().y).collect::<Vec<_>>();
    assert!(qt.y > glyph_c[1] && qt.y < glyph_c[2], "between the 2nd and 3rd characters: {} in {glyph_c:?}", qt.y);
}

/// Glyphs that touch or overlap are drawn as one outline, as After Effects draws a line of text:
/// no light seam where their anti-aliased edges meet (the crossbars of "ff", #415).
#[test]
fn touching_glyphs_join_without_a_seam() {
    use kurbo::Shape;
    let glyph = |x0: f64, x1: f64, fill: [f32; 4], reversed: bool| {
        let path = kurbo::Rect::new(x0, 4.0, x1, 16.0).to_path(0.1);
        PlacedGlyph {
            local: if reversed { path.reverse_subpaths() } else { path },
            m: Mat4::IDENTITY,
            xf: CharXf::default(),
            fill,
            stroke: [0.0; 4],
            stroke_width: 0.0,
            apply_fill: true,
            char_index: 0,
            rest: [0.0; 2],
        }
    };
    let draw = |glyphs: Vec<PlacedGlyph>, blend: BlendMode| {
        let geom = TextGeom { doc: TextDoc::default(), glyphs, fill_stroke: 0, blend };
        let mut img = crate::Image::new(24, 20);
        draw_glyphs(&geom, &mut img, &Mat3::IDENTITY, 1.0, None);
        img
    };
    let (white, red) = ([1.0; 4], [1.0, 0.0, 0.0, 1.0]);
    // The shared edge runs through the middle of pixel column 10.
    let seam = draw(vec![glyph(4.0, 10.5, white, false), glyph(10.5, 17.0, white, false)], BlendMode::Normal).get(10, 10);
    assert!(seam[3] > 0.999 && seam[0] > 0.999, "touching glyphs leave no seam: {seam:?}");
    // Outlines wound the other way (another font, a mirrored character) still add up.
    let seam = draw(vec![glyph(4.0, 10.5, white, false), glyph(10.5, 17.0, white, true)], BlendMode::Normal).get(10, 10);
    assert!(seam[3] > 0.999, "opposite windings join too: {seam:?}");
    let overlap = draw(vec![glyph(4.0, 12.0, white, false), glyph(8.0, 17.0, white, true)], BlendMode::Normal).get(10, 10);
    assert!(overlap[3] > 0.999, "and their overlap stays filled: {overlap:?}");
    // Characters of other colours, or blended with each other, are still drawn one by one.
    let two = draw(vec![glyph(4.0, 10.5, white, false), glyph(10.5, 17.0, red, false)], BlendMode::Normal).get(10, 10);
    assert!((two[3] - 0.75).abs() < 0.02, "{two:?}");
    let multiply = draw(vec![glyph(4.0, 10.5, white, false), glyph(10.5, 17.0, white, false)], BlendMode::Multiply).get(10, 10);
    assert!(multiply[3] < 0.9, "{multiply:?}");
}

/// #1011: what dragging a Range Selector bar to fraction `f` of the text writes.
#[test]
fn range_bar_drag_values() {
    // Percentage: the bar's place less the Offset, within 0..100.
    assert!((range_value_of(0.5, 0.0, false, 7) - 50.0).abs() < 1e-9);
    assert!((range_value_of(0.5, 0.25, false, 7) - 25.0).abs() < 1e-9, "Offset 25%: the bar at 50% is Start 25%");
    assert_eq!(range_value_of(0.1, 0.25, false, 7), 0.0, "never below 0");
    assert_eq!(range_value_of(1.0, -0.5, false, 7), 100.0, "nor above 100");
    // Index: whole units, the nearest boundary (3.4 of 7 characters → 3, 3.6 → 4).
    assert_eq!(range_value_of(3.4 / 7.0, 0.0, true, 7), 3.0);
    assert_eq!(range_value_of(3.6 / 7.0, 0.0, true, 7), 4.0);
    assert_eq!(range_value_of(3.6 / 7.0, 2.0 / 7.0, true, 7), 2.0, "less a 2-character Offset");
}
