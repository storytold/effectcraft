use super::*;

fn st(size: f32) -> TextStyle {
    TextStyle { size, ..Default::default() }
}

fn ids(text: &str, s: &TextStyle) -> Vec<u32> {
    layout_uncached(text, s, &ParagraphStyle::default()).glyphs.iter().map(|g| g.id).collect()
}

fn ot(f: impl FnOnce(&mut OpenType)) -> TextStyle {
    let mut s = st(100.0);
    f(&mut s.opentype);
    s
}

fn serif() -> TextStyle {
    TextStyle { family: "Noto Serif".into(), ..st(100.0) }
}

#[test]
fn bundled_faces_list_their_features() {
    let inter = fonts::face(fonts::resolve("Inter", "Regular").face).features();
    for t in ["ss01", "dlig", "frac", "tnum", "sups", "ordn"] {
        assert!(inter.iter().any(|f| f == t), "{t} in {inter:?}");
    }
    let serif = fonts::face(fonts::resolve("Noto Serif", "Regular").face).features();
    for t in ["smcp", "c2sc", "onum", "lnum"] {
        assert!(serif.iter().any(|f| f == t), "{t} in {serif:?}");
    }
}

#[test]
fn opentype_features_substitute_glyphs() {
    let plain = st(100.0);
    // Stylistic sets, fractions, ordinals, figure widths and discretionary ligatures.
    assert_ne!(ids("1234567890", &plain), ids("1234567890", &ot(|o| o.set_stylistic_set(1, true))), "ss01 alternate digits");
    assert_eq!(ids("1234567890", &plain), ids("1234567890", &ot(|o| o.set_stylistic_set(20, true))), "Inter has no ss20");
    assert_ne!(ids("1/2", &plain), ids("1/2", &ot(|o| o.fractions = true)), "frac");
    assert_ne!(ids("1a 2o No", &plain), ids("1a 2o No", &ot(|o| o.ordinals = true)), "ordn");
    let tab = measure("1111", &ot(|o| o.figure_width = effectcraft_keyframe::FigureWidth::Tabular));
    let prop = measure("1111", &ot(|o| o.figure_width = effectcraft_keyframe::FigureWidth::Proportional));
    assert!((tab - prop).abs() > 1.0, "tabular {tab} vs proportional {prop}");
    // Old-style figures (Noto Serif).
    let old = TextStyle { opentype: OpenType { figure_style: effectcraft_keyframe::FigureStyle::OldStyle, ..Default::default() }, ..serif() };
    assert_ne!(ids("123", &serif()), ids("123", &old), "onum");
    // Contextual alternates are on by default; turning them off can only keep or change glyphs.
    let no_calt = ot(|o| o.contextual_alternates = false);
    assert_eq!(ids("Hello", &plain).len(), ids("Hello", &no_calt).len());
}

#[test]
fn true_small_caps_and_superscripts_with_faux_fallback() {
    // Noto Serif has smcp: lowercase letters take their own small-capital glyphs at full size.
    let sc = TextStyle { caps: Caps::Small, ..serif() };
    let l = layout_uncached("Ab", &sc, &ParagraphStyle::default());
    let upper_b = ids("B", &serif())[0];
    assert_ne!(l.glyphs[1].id, upper_b, "true small cap glyph, not a scaled capital");
    assert_eq!(l.glyphs[1].size, 100.0);
    // Inter has no smcp: a capital scaled down.
    let faux = layout_uncached("Ab", &TextStyle { caps: Caps::Small, ..st(100.0) }, &ParagraphStyle::default());
    assert_eq!(faux.glyphs[1].id, ids("B", &st(100.0))[0]);
    assert!((faux.glyphs[1].size - 78.0).abs() < 0.01);
    // All Small Caps: capitals become small capitals too (c2sc).
    let all = TextStyle { opentype: OpenType { all_small_caps: true, ..Default::default() }, ..serif() };
    let a = layout_uncached("AB", &all, &ParagraphStyle::default());
    assert_ne!(a.glyphs[0].id, ids("A", &serif())[0]);
    // Superscript: Inter's sups figures at full size on the baseline; letters it lacks are
    // synthesized (smaller and raised).
    let sup = TextStyle { script: Script::Super, ..st(100.0) };
    let s2 = layout_uncached("2", &sup, &ParagraphStyle::default());
    assert_ne!(s2.glyphs[0].id, ids("2", &st(100.0))[0]);
    assert_eq!(s2.glyphs[0].size, 100.0);
    assert_eq!(s2.glyphs[0].y, 0.0);
    let sq = layout_uncached("q", &sup, &ParagraphStyle::default());
    if !feature_substitutes(sq.glyphs[0].face, b"sups", 'q') {
        assert!(sq.glyphs[0].size < 60.0 && sq.glyphs[0].y < -20.0, "{:?}", sq.glyphs[0]);
    }
    let sub = layout_uncached("2", &TextStyle { script: Script::Sub, ..st(100.0) }, &ParagraphStyle::default());
    assert_ne!(sub.glyphs[0].id, s2.glyphs[0].id);
}

#[test]
fn kerning_and_ligatures_change_advances() {
    let kern = measure("AVAVAV", &st(100.0));
    let nokern = measure("AVAVAV", &TextStyle { kerning: false, ..st(100.0) });
    assert!(kern < nokern - 5.0, "kerning tightens AV: {kern} vs {nokern}");
    // tracking adds 1/1000 em per cluster
    let tracked = measure("AVAVAV", &TextStyle { tracking: 100.0, ..st(100.0) });
    assert!((tracked - kern - 6.0 * 10.0).abs() < 0.5, "{tracked} {kern}");
}

#[test]
fn point_text_alignment() {
    let l = layout("Hello", &st(50.0), &ParagraphStyle { align: Align::Center, ..Default::default() });
    let line = &l.lines[0];
    assert!((line.x + line.width / 2.0).abs() < 0.01);
    let r = layout("Hello", &st(50.0), &ParagraphStyle { align: Align::Right, ..Default::default() });
    assert!((r.lines[0].x + r.lines[0].width).abs() < 0.01);
    assert_eq!(l.glyphs.len(), 5);
    assert!(l.bounds[1] < -30.0 && l.bounds[3] > 5.0, "{:?}", l.bounds);
}

#[test]
fn wraps_area_text_and_justifies() {
    let p = ParagraphStyle { width: Some(300.0), align: Align::Justify, ..Default::default() };
    let l = layout("the quick brown fox jumps over the lazy dog again and again", &st(40.0), &p);
    assert!(l.lines.len() >= 3, "{}", l.lines.len());
    for line in &l.lines[..l.lines.len() - 1] {
        assert!((line.width - 300.0).abs() < 0.5, "justified to the box: {}", line.width);
    }
    // the lines tile the text
    assert_eq!(l.lines[0].range.start, 0);
    for w in l.lines.windows(2) {
        assert_eq!(w[0].range.end, w[1].range.start);
        assert!(w[1].baseline > w[0].baseline);
    }
    let left = layout("the quick brown fox jumps over the lazy dog", &st(40.0), &ParagraphStyle { width: Some(300.0), ..Default::default() });
    assert!(left.lines.iter().all(|l| l.width <= 300.0 + 0.01));
}

#[test]
fn newlines_make_paragraphs_and_leading_adds() {
    let l = layout("one\ntwo\n\nfour", &st(30.0), &ParagraphStyle::default());
    assert_eq!(l.lines.len(), 4);
    assert_eq!(l.lines[1].range, 4..7);
    assert_eq!(l.lines[2].range, 8..8);
    let gap = l.lines[1].baseline - l.lines[0].baseline;
    let l2 = layout("one\ntwo", &st(30.0), &ParagraphStyle { leading: 10.0, ..Default::default() });
    assert!((l2.lines[1].baseline - l2.lines[0].baseline - gap - 10.0).abs() < 1e-3);
}

#[test]
fn carets_and_hit_testing() {
    let l = layout("abc\nde", &st(40.0), &ParagraphStyle::default());
    let (x0, _, li0) = l.caret(0);
    let (x1, _, _) = l.caret(1);
    let (x3, _, _) = l.caret(3);
    assert!(x0.abs() < 1e-3 && x1 > x0 && x3 > x1);
    assert_eq!(li0, 0);
    let (_, b4, li4) = l.caret(4);
    assert_eq!(li4, 1);
    assert!(b4 > 0.0);
    assert_eq!(l.hit(x1 + 1.0, 0.0), 1);
    assert_eq!(l.hit(1000.0, b4), 6);
    let rects = l.selection_rects(1, 5);
    assert_eq!(rects.len(), 2);
}

#[test]
fn bidi_reorders_rtl_runs() {
    // Hebrew letters are laid out right-to-left: the first logical letter is rightmost.
    let l = layout("ab \u{5d0}\u{5d1}\u{5d2} cd", &st(40.0), &ParagraphStyle::default());
    let x_of = |byte: usize| l.glyphs.iter().find(|g| g.cluster == byte).map(|g| g.x).unwrap();
    let alef = "ab ".len();
    let gimel = alef + 4;
    assert!(x_of(alef) > x_of(gimel), "alef right of gimel");
    assert!(x_of(0) < x_of(gimel) && x_of(alef) < x_of("ab \u{5d0}\u{5d1}\u{5d2} ".len()));
}

#[test]
fn caps_and_small_caps() {
    let s = st(50.0);
    let up = measure("HELLO", &s);
    let all = measure("hello", &TextStyle { caps: Caps::All, ..s.clone() });
    assert!((up - all).abs() < 0.01);
    let small = measure("hello", &TextStyle { caps: Caps::Small, ..s.clone() });
    assert!(small < up * 0.9 && small > up * 0.6, "{small} {up}");
    let l = layout("Hi", &TextStyle { baseline_shift: 10.0, underline: true, ..s }, &ParagraphStyle::default());
    assert!((l.glyphs[0].y + 10.0).abs() < 1e-3);
    assert_eq!(l.underlines.len(), 1);
}

#[test]
fn ligature_caret_interpolates() {
    // Inter has an "fi"-like ligature only via calt in some versions; use "ffi" which most
    // fonts ligate. Whatever shaping does, every char boundary has a caret.
    let l = layout("office", &st(40.0), &ParagraphStyle::default());
    let stops: Vec<usize> = l.lines[0].carets.iter().map(|c| c.0).collect();
    assert_eq!(stops, vec![0, 1, 2, 3, 4, 5, 6]);
    let xs: Vec<f32> = l.lines[0].carets.iter().map(|c| c.1).collect();
    assert!(xs.windows(2).all(|w| w[1] >= w[0]), "{xs:?}");
}

#[test]
fn fallback_font_for_missing_glyphs() {
    // Inter lacks Devanagari/Hebrew; Noto Serif lacks Hebrew too → glyphs still produced (notdef)
    let l = layout("A\u{3b1}", &TextStyle { family: "Noto Serif".into(), ..st(40.0) }, &ParagraphStyle::default());
    assert_eq!(l.glyphs.len(), 2);
    assert!(l.glyphs.iter().all(|g| g.id != 0));
}

#[test]
fn empty_text_has_a_caret() {
    let l = layout("", &st(40.0), &ParagraphStyle::default());
    assert_eq!(l.lines.len(), 1);
    assert_eq!(l.caret(0).0, 0.0);
    assert!(l.bounds[3] > l.bounds[1]);
}

fn rich(text: &str, runs: &[(Range<usize>, TextStyle)], para: ParagraphStyle) -> Layout {
    layout_rich_uncached(text, runs, &[para])
}

#[test]
fn mixed_sizes_share_a_baseline_and_take_the_largest_leading() {
    let text = "small BIG\nnext";
    let runs = [(0..6, st(20.0)), (6..9, st(80.0)), (9..text.len(), st(20.0))];
    let l = rich(text, &runs, ParagraphStyle::default());
    assert_eq!(l.lines.len(), 2);
    let y0 = l.glyphs[0].y;
    assert!(l.glyphs[..9].iter().all(|g| (g.y - y0).abs() < 1e-3), "one baseline per line");
    assert!(l.glyphs[7].size > l.glyphs[0].size * 3.0);
    // Line 2's advance is its own leading (20 px text: 24 px); line 1 is as tall as the big run.
    assert!((l.lines[1].baseline - l.lines[0].baseline - 24.0).abs() < 1e-3, "{}", l.lines[1].baseline);
    assert!(l.lines[0].ascent > 60.0);
    // A big run on the second line pushes it down by its 120% leading.
    let text2 = "aa\nBB";
    let l2 = rich(text2, &[(0..3, st(20.0)), (3..5, st(80.0))], ParagraphStyle::default());
    assert!((l2.lines[1].baseline - 96.0).abs() < 1e-3);
    // Explicit leading wins over auto.
    let mut fixed = st(20.0);
    fixed.leading = Some(50.0);
    let l3 = rich("a\nb", &[(0..3, fixed)], ParagraphStyle::default());
    assert!((l3.lines[1].baseline - 50.0).abs() < 1e-3);
}

#[test]
fn super_subscript_tsume_and_kerning_metrics() {
    let base = st(100.0);
    let sup = TextStyle { script: Script::Super, ..base.clone() };
    let sub = TextStyle { script: Script::Sub, ..base.clone() };
    // A letter the font has no superior / inferior glyph for: synthesized.
    let inter = fonts::resolve("Inter", "Regular").face;
    let c = "qwkzvjxyQWK".chars().find(|&c| !feature_substitutes(inter, b"sups", c) && !feature_substitutes(inter, b"subs", c)).unwrap();
    let text: String = ['x', c, 'y', c].iter().collect();
    let l = rich(&text, &[(0..1, base.clone()), (1..2, sup), (2..3, base.clone()), (3..4, sub)], ParagraphStyle::default());
    let g = &l.glyphs;
    assert!((g[1].size - 58.3).abs() < 0.1);
    assert!((g[1].y - (g[0].y - 33.3)).abs() < 0.1, "superscript raised by a third of the size");
    assert!((g[3].y - (g[2].y + 33.3)).abs() < 0.1, "subscript lowered");
    // Tsume removes side bearings; full tsume is narrower than none.
    let w0 = measure("onion", &base);
    let w1 = measure("onion", &TextStyle { tsume: 1.0, ..base.clone() });
    assert!(w1 < w0 - 5.0, "{w1} vs {w0}");
    // Manual kerning: +100/1000 em before each of the 5 later characters of AVAVAV, metrics off.
    let nokern = measure("AVAVAV", &TextStyle { kerning: false, ..base.clone() });
    let manual = measure("AVAVAV", &TextStyle { manual_kern: Some(100.0), ..base.clone() });
    assert!((manual - nokern - 50.0).abs() < 0.5, "{manual} {nokern}");
    let optical = measure("AVAVAV", &TextStyle { optical: true, ..base.clone() });
    assert!(optical > 0.0 && (optical - nokern).abs() < 100.0);
    // Horizontal scale stretches advances.
    let wide = measure("onion", &TextStyle { h_scale: 2.0, ..base.clone() });
    assert!((wide - 2.0 * w0).abs() < 0.5);
    let l = layout("o", &TextStyle { h_scale: 2.0, v_scale: 0.5, ..base }, &ParagraphStyle::default());
    assert_eq!((l.glyphs[0].h_scale, l.glyphs[0].v_scale), (2.0, 0.5));
}

#[test]
fn indents_and_paragraph_spacing() {
    let s = st(20.0);
    let text = "first paragraph here\nsecond one";
    let p0 = ParagraphStyle { width: Some(400.0), indent_start: 30.0, indent_first: 15.0, space_after: 10.0, ..Default::default() };
    let p1 = ParagraphStyle { width: Some(400.0), indent_end: 50.0, align: Align::Right, space_before: 7.0, ..Default::default() };
    let l = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p0.clone(), p1]);
    assert!((l.lines[0].x - 45.0).abs() < 1e-3, "left + first-line indent: {}", l.lines[0].x);
    assert!((l.lines[1].x + l.lines[1].width - 350.0).abs() < 0.5, "right indent: {}", l.lines[1].x + l.lines[1].width);
    assert!((l.lines[1].baseline - l.lines[0].baseline - (24.0 + 10.0 + 7.0)).abs() < 1e-3);
    assert_eq!((l.lines[0].para, l.lines[1].para), (0, 1));
    assert!(l.lines[0].para_end && l.lines[1].para_end);
    // Wrapped lines after the first don't get the first-line indent.
    let long = "word ".repeat(30);
    let l = layout_rich_uncached(&long, &[(0..long.len(), s.clone())], &[p0]);
    assert!(l.lines.len() > 2);
    assert!((l.lines[1].x - 30.0).abs() < 1e-3);
    assert!(l.lines.iter().all(|ln| ln.x + ln.width <= 400.0 + 0.5));
    assert!(!l.lines[0].para_end);
    // Point text: a right-aligned paragraph's end indent moves it left of the origin.
    let l = layout_rich_uncached("abc", &[(0..3, s)], &[ParagraphStyle { align: Align::Right, indent_end: 12.0, ..Default::default() }]);
    assert!((l.lines[0].x + l.lines[0].width + 12.0).abs() < 1e-3);
}

#[test]
fn justify_last_line_modes_and_every_line_composer() {
    let text = "the quick brown fox jumps over the lazy dog and keeps running far";
    let s = st(30.0);
    let p = |last: Align| ParagraphStyle { width: Some(320.0), align: Align::Justify, justify_last: last, ..Default::default() };
    let left = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Left)]);
    let n = left.lines.len();
    assert!(n >= 3);
    assert!((left.lines[0].width - 320.0).abs() < 0.5);
    assert!(left.lines[n - 1].width < 320.0 && left.lines[n - 1].x.abs() < 1e-3);
    let center = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Center)]);
    let lc = &center.lines[n - 1];
    assert!((lc.x - (320.0 - lc.width) / 2.0).abs() < 0.5);
    let right = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Right)]);
    let lr = &right.lines[n - 1];
    assert!((lr.x + lr.width - 320.0).abs() < 0.5);
    let all = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p(Align::Justify)]);
    assert!((all.lines[n - 1].width - 320.0).abs() < 0.5, "justify all stretches the last line");
    // Every-line composer: never worse (summed squared slack) than greedy breaking.
    let slack = |l: &Layout| -> f32 { l.lines[..l.lines.len() - 1].iter().map(|ln| (320.0 - ln.width).powi(2)).sum() };
    let greedy = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[ParagraphStyle { width: Some(320.0), ..Default::default() }]);
    let every = layout_rich_uncached(text, &[(0..text.len(), s)], &[ParagraphStyle { width: Some(320.0), every_line: true, ..Default::default() }]);
    assert!(slack(&every) <= slack(&greedy) + 1e-3, "{} vs {}", slack(&every), slack(&greedy));
    assert!(every.lines.iter().all(|ln| ln.width <= 320.0 + 0.5));
}

#[test]
fn hanging_punctuation_overhangs_the_box() {
    let s = st(40.0);
    let text = "\"Hi.\"";
    let p = ParagraphStyle { width: Some(400.0), hanging: true, ..Default::default() };
    let l = layout_rich_uncached(text, &[(0..text.len(), s.clone())], &[p]);
    assert!(l.lines[0].x < -1.0, "opening quote hangs left: {}", l.lines[0].x);
    let r =
        layout_rich_uncached(text, &[(0..text.len(), s)], &[ParagraphStyle { width: Some(400.0), hanging: true, align: Align::Right, ..Default::default() }]);
    assert!(r.lines[0].x + r.lines[0].width > 401.0, "closing quote hangs right");
}

#[test]
fn oversized_layout_requests_are_bounded_without_caching() {
    let text = "a".repeat(MAX_LAYOUT_BYTES + 1);
    let layout = layout(&text, &TextStyle::default(), &ParagraphStyle::default());
    assert_eq!(layout.text_len, text.len());
    assert!(layout.lines.is_empty() && layout.glyphs.is_empty());
    assert!(layout_rich_uncached("ok", &[], &[]).lines.len() == 1);
}

#[test]
fn rtl_paragraph_carets_run_right_to_left() {
    let text = "\u{5d0}\u{5d1}\u{5d2}";
    let l = layout_rich_uncached(text, &[(0..text.len(), st(40.0))], &[ParagraphStyle { rtl: Some(true), ..Default::default() }]);
    let xs: Vec<f32> = l.lines[0].carets.iter().map(|c| c.1).collect();
    assert_eq!(xs.len(), 4);
    assert!(xs.windows(2).all(|w| w[1] < w[0]), "{xs:?}");
    // Hit testing returns the logical position nearest the point.
    assert_eq!(l.hit(xs[1] + 0.5, 0.0), 2);
    assert_eq!(l.hit(xs[0] + 50.0, 0.0), 0);
}

/// #1563: text set in a named instance (`Black`) is laid out at the instance's axis values,
/// as if they had been set as variations; axes set on the text itself keep their values.
#[test]
fn named_instance_sets_the_base_variations() {
    const FAMILY: &str = "EffectCraft Layout Instance Test";
    fonts::add_font_data(crate::sfnt::test_font::variable(FAMILY, &[("Light", 300.0, None), ("Black", 900.0, None)], true));
    let st =
        |style: &str, variations: Vec<(String, f32)>| TextStyle { family: FAMILY.into(), style: style.into(), size: 50.0, variations, ..Default::default() };
    let coords = |l: &Layout| l.glyphs.iter().map(|g| crate::variable::coords(g.variations).to_vec()).collect::<Vec<_>>();
    let black = layout_uncached("Hi", &st("Black", vec![]), &ParagraphStyle::default());
    assert!(!black.glyphs.is_empty());
    assert!(coords(&black).iter().all(|c| c == &[("wght".to_string(), 900.0)]), "{:?}", coords(&black));
    let set = layout_uncached("Hi", &st("Black", vec![("wght".into(), 500.0)]), &ParagraphStyle::default());
    assert!(coords(&set).iter().all(|c| c == &[("wght".to_string(), 500.0)]), "{:?}", coords(&set));
    // A static family keeps no variations.
    let inter = layout_uncached("Hi", &TextStyle::default(), &ParagraphStyle::default());
    assert!(inter.glyphs.iter().all(|g| g.variations == 0));
}
