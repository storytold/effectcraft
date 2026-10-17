//! Batch 3 (objects): Point / Layer / Dropdown controls, masks, shape items, text styles, camera,
//! light, marker data and footage items. Every expected value is the value the corpus case was
//! made with (its MANIFEST).

mod common;

use common::{assert_near, first_comp, fixture, layer_named, layer_of_type, layer_shown_as, layer_with_id, near, open};
use effectcraft_aep::{
    Caps, CueType, GradientType, ItemKind, Justification, LayerRef, LayerType, MaskMode, ParamKind, ParamValue, PolystarType, Project, PropNode, Rational,
    ShapeDetail, ShapeItem, TextDocument, ValueSource,
};

const L: u32 = 0x1b;

fn rat(num: i64, den: i64) -> Rational {
    Rational { num, den }
}

// ------------------------------------------------------------------ Point control (310, 218)

#[test]
fn the_point_default_is_a_fraction_of_the_layer() {
    // 310: the default Point of a Point Control on a 100x100 solid reads back [50, 50]: the default is
    // two 16.16 fractions 0.5 in pard, relative to the layer, not pixels.
    let bytes = fixture("310-solid-100x100-pointcontrol.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "S100");
    assert_eq!(project.layer_source_size(&layer), Some((100, 100)));
    let effects = layer.effects();
    assert_eq!(effects.len(), 1);
    let p = effects[0].params().into_iter().find(|p| p.kind == ParamKind::Point).unwrap();
    assert!(p.stored().is_none());
    assert_eq!(p.pard_default(), Some(ParamValue::Point([0.5, 0.5])));
    assert_eq!((p.value(), p.is_default()), (Some(ParamValue::Point([0.5, 0.5])), Some(true)));
    assert_eq!(p.point_px(100.0, 100.0), Some([50.0, 50.0]));
    assert_eq!(&p.pard()[56..64], &[0, 0, 0x80, 0, 0, 0, 0x80, 0], "two 16.16 fractions 0.5");
    // 218: an explicit [111, 222] on the 1920x1080 solid is [111/1920, 222/1080] of the layer.
    let bytes = fixture("218-fx-point-111-222.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let p = layer.effects()[0].params().into_iter().find(|p| p.kind == ParamKind::Point).unwrap();
    assert_eq!(p.is_default(), Some(false));
    let px = p.point_px(1920.0, 1080.0).unwrap();
    assert_near(&px, &[111.0, 222.0], 1e-9);
    assert!(p.point_px(f64::NAN, 1.0).is_some());
    let other = layer.effects()[0].params().into_iter().find(|p| p.kind == ParamKind::Layer).unwrap();
    assert_eq!(other.point_px(1.0, 1.0), None);
}

// ------------------------------------------------------------------ Layer Control (320, 321, 220)

fn layer_ref(name: &str) -> (Option<LayerRef>, Option<ParamValue>, Option<LayerRef>) {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let effects = layer.effects();
    assert_eq!(effects.len(), 1, "{name}");
    assert_eq!(effects[0].match_name(), "ADBE Layer Control");
    let params = effects[0].params();
    let p = params.iter().find(|p| p.kind == ParamKind::Layer && p.index == 1).unwrap();
    // The implicit input layer is not a layer reference.
    (p.layer_ref(L), params[0].stored_value(), params[0].layer_ref(L))
}

#[test]
fn layer_control_self_versus_none() {
    // 320: Layer set explicitly to 1 = the layer itself: tdpi = the owning layer's id (0x1b).
    assert_eq!(layer_ref("320-fx-layercontrol-self-explicit.aep"), (Some(LayerRef::This), Some(ParamValue::Layer(L)), None));
    // 321: None: tdpi = 0.
    assert_eq!(layer_ref("321-fx-layercontrol-none.aep"), (Some(LayerRef::Nothing), Some(ParamValue::Layer(L)), None));
    // 220: Layer 1 of a new Null layer (id 0x22).
    assert_eq!(layer_ref("220-fx-layercontrol-to-layer1.aep").0, Some(LayerRef::Layer(0x22)));
}

// ------------------------------------------------------------------ Dropdown (323, 324, 077)

#[test]
fn dropdown_items_selection_and_count() {
    for (name, selected, text, is_default) in
        [("323-fx-dropdown-5items.aep", 1, "Alpha", Some(true)), ("324-fx-dropdown-5items-select3.aep", 3, "Gamma", Some(false))]
    {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let layer = layer_with_id(&first_comp(&project), L);
        let effects = layer.effects();
        assert_eq!(effects.len(), 1);
        // set_dropdown_items re-creates the effect definition under a new match name.
        assert_eq!(effects[0].match_name(), "Pseudo/@@KbKT0rCPQi2A0ytuyGLWSw", "{name}");
        assert_eq!(effects[0].name().as_deref(), Some("Dropdown Menu Control"));
        let p = effects[0].params().into_iter().find(|p| p.kind == ParamKind::Popup).unwrap();
        assert_eq!(p.items.as_deref(), Some(&["Alpha", "Beta", "Gamma", "Delta", "Epsilon"].map(String::from)[..]), "{name}");
        assert_eq!(p.popup_item_count(), Some(5), "{name}");
        assert_eq!((p.popup_selected_index(), p.popup_selected_item()), (Some(selected), Some(text)), "{name}");
        assert_eq!(p.is_default(), is_default, "{name}");
        assert_eq!(p.stored().is_some(), selected != 1, "{name}");
        if selected == 3 {
            // The selection is a value record and also overwrites pard@56.
            assert_eq!(p.stored_value(), Some(ParamValue::Index(3)));
            assert_eq!(p.pard_default(), Some(ParamValue::Index(3)));
        } else {
            assert_eq!(p.stored_value(), None);
            assert_eq!(p.pard_default(), Some(ParamValue::Index(1)));
        }
    }
    // 077: the default dropdown with three items.
    let bytes = fixture("077-fx-dropdown-default.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let p = layer.effects()[0].params().into_iter().find(|p| p.kind == ParamKind::Popup).unwrap();
    assert_eq!((p.popup_item_count(), p.popup_selected_item()), (Some(3), Some("Item 1")));
    let no_data = layer.effects()[0].params().into_iter().find(|p| p.kind == ParamKind::NoData).unwrap();
    assert_eq!((no_data.popup_item_count(), no_data.popup_selected_index(), no_data.popup_selected_item()), (None, None, None));
}

// ------------------------------------------------------------------ masks (309, 328, 329)

fn ellipse_vertices() -> [[f64; 2]; 4] {
    [[960.0, 340.0], [1360.0, 540.0], [960.0, 740.0], [560.0, 540.0]]
}

#[test]
fn a_mask_box_is_normalised_to_the_layer_size() {
    // 309: a 60x40 rectangle at [50,50] on a 100x100 solid: box 0.2, 0.3, 0.8, 0.7 of the LAYER;
    // vertices [20,30] [80,30] [80,70] [20,70].
    let bytes = fixture("309-solid-100x100-mask-rect.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "S100");
    let masks = layer.masks();
    assert_eq!(masks.len(), 1);
    let m = &masks[0];
    assert_eq!((m.mode, m.inverted, m.counter), (Some(MaskMode::Add), Some(false), Some(1)));
    let want_box = [0.2, 0.3, 0.8, 0.7];
    for (got, want) in m.shapes[0].bbox.iter().zip(want_box) {
        assert!(near(f64::from(*got), want, 1e-6), "{:?}", m.shapes[0].bbox);
    }
    let shapes = project.mask_vertices_px(&layer, m).unwrap();
    assert_eq!(shapes.len(), 1);
    let got: Vec<[f64; 2]> = shapes[0].iter().map(|v| v.position).collect();
    for (g, w) in got.iter().zip([[20.0, 30.0], [80.0, 30.0], [80.0, 70.0], [20.0, 70.0]]) {
        assert_near(g, &w, 1e-3);
    }
    assert_eq!(got.len(), 4);
    // A layer without a sized source has no pixel vertices.
    let bytes = fixture("333-shape-group-two-rects.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let shape = layer_named(&first_comp(&project), "Shape");
    assert!(project.mask_vertices_px(&shape, m).is_none());
}

#[test]
fn one_mkif_per_mask_and_the_mode_none() {
    // 328: ellipse 800x400 at [960,540] (Mask 1) and rectangle 400x300 at [700,500] (Mask 2), both Add;
    // the per-layer counter gives 1 and 2.
    let bytes = fixture("328-mask-two-ellipse-rect.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let masks = layer.masks();
    assert_eq!(masks.len(), 2);
    assert_eq!(masks.iter().map(|m| m.name.as_deref()).collect::<Vec<_>>(), [Some("Mask 1"), Some("Mask 2")]);
    for (m, counter) in masks.iter().zip([1, 2]) {
        assert_eq!((m.mode, m.inverted, m.counter), (Some(MaskMode::Add), Some(false), Some(counter)));
        assert_eq!(m.shapes.len(), 1);
    }
    let v1 = project.mask_vertices_px(&layer, &masks[0]).unwrap();
    for (v, w) in v1[0].iter().zip(ellipse_vertices()) {
        assert_near(&v.position, &w, 1e-2);
    }
    // Mask 2 has its own box (500..900 x 350..650) normalised to the layer.
    let v2 = project.mask_vertices_px(&layer, &masks[1]).unwrap();
    for (v, w) in v2[0].iter().zip([[500.0, 350.0], [900.0, 350.0], [900.0, 650.0], [500.0, 650.0]]) {
        assert_near(&v.position, &w, 1e-2);
    }
    assert_eq!(v2[0].len(), 4);
    // 329: the same ellipse as 232 with the mode None (0); the layer counter goes on (3 after 328's two).
    let bytes = fixture("329-mask-ellipse-mode-none.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let none = layer_with_id(&first_comp(&project), L).masks();
    assert_eq!(none.len(), 1);
    assert_eq!((none[0].mode, none[0].counter), (Some(MaskMode::None), Some(3)));
    assert_eq!(MaskMode::from_id(0), MaskMode::None);
    let bytes = fixture("232-mask-ellipse.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let add = layer_with_id(&first_comp(&project), L).masks();
    assert_eq!(add[0].mode, Some(MaskMode::Add));
    assert_eq!(add[0].shapes, none[0].shapes, "329 is 232 with another mode");
}

// ------------------------------------------------------------------ shapes (330-333)

fn shape_group(name: &str) -> effectcraft_aep::ShapeGroup {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_named(&first_comp(&project), "Shape");
    let h = layer.header().unwrap();
    assert_eq!((h.layer_type, h.label), (LayerType::Shape, 8), "{name}");
    let mut groups = layer.shape_groups();
    assert_eq!(groups.len(), 1, "{name}");
    groups.remove(0)
}

fn other_name(item: &ShapeItem) -> &str {
    match item {
        ShapeItem::Other { match_name, .. } => match_name,
        _ => "",
    }
}

#[test]
fn polystar_defaults_and_polygon() {
    // 330: star, 5 points, outer 100, inner 50 = the defaults: no property record at all.
    let g = shape_group("330-shape-polystar-star-5-50-100.aep");
    assert_eq!(g.name.as_deref(), Some("Group 1"));
    assert_eq!(g.items.len(), 2);
    assert_eq!(other_name(&g.items[0]), "ADBE Vector Shape - Star");
    assert!(matches!(&g.items[1], ShapeItem::Fill { color: Some(c), .. } if c.value == [1.0, 0.0, 1.0, 0.0]));
    let [ShapeDetail::Polystar { item_index, star_type, outer_radius, inner_radius, inner_roundness }] = &g.details[..] else { panic!("{:?}", g.details) };
    assert_eq!(*item_index, 0);
    assert_eq!((star_type.value, star_type.source), (PolystarType::Star, ValueSource::Default));
    assert_eq!((outer_radius.value, outer_radius.source), (100.0, ValueSource::Default));
    assert_eq!((inner_radius.value, inner_radius.source), (50.0, ValueSource::Default));
    assert!(inner_roundness.is_none());
    // 331: polygon (type 2): the type record, plus Inner Radius 50 and Inner Roundess 0 in their default state.
    let g = shape_group("331-shape-polystar-polygon-5-100.aep");
    let [ShapeDetail::Polystar { star_type, outer_radius, inner_radius, inner_roundness, .. }] = &g.details[..] else { panic!("{:?}", g.details) };
    assert_eq!((star_type.value, star_type.source), (PolystarType::Polygon, ValueSource::Stored));
    assert_eq!((outer_radius.value, outer_radius.source), (100.0, ValueSource::Default));
    assert_eq!((inner_radius.value, inner_radius.source), (50.0, ValueSource::Stored));
    let r = inner_roundness.unwrap();
    assert_eq!((r.value, r.source), (0.0, ValueSource::Stored));
    // The tdsb states: the type is a written record (1), the other two are default-state records (3).
    let bytes = fixture("331-shape-polystar-polygon-5-100.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_named(&first_comp(&project), "Shape");
    let base = ["ADBE Root Vectors Group", "ADBE Vector Group", "ADBE Vectors Group", "ADBE Vector Shape - Star"];
    for (prop, flags) in [("ADBE Vector Star Type", 1), ("ADBE Vector Star Inner Radius", 3), ("ADBE Vector Star Inner Roundess", 3)] {
        let path: Vec<&str> = base.iter().copied().chain([prop]).collect();
        let Some(PropNode::Property(p)) = layer.properties().unwrap().find(&path) else { panic!("{prop}") };
        assert_eq!(p.flags(), Some(flags), "{prop}");
    }
}

#[test]
fn radial_gradient_extra_records() {
    // 332: Rectangle Path 400x300 + Gradient Fill, radial (type 2), Start [0,0] = default (absent),
    // End [200,0], Highlight Length / Angle 0 written as records.
    let g = shape_group("332-shape-gradient-fill-radial.aep");
    assert_eq!(g.items.len(), 2);
    let ShapeItem::GradientFill { start, end, .. } = &g.items[1] else { panic!("{:?}", g.items[1]) };
    assert!(start.is_none());
    assert_eq!(end.map(|e| (e.value, e.source)), Some(([200.0, 0.0], ValueSource::Stored)));
    let gradient = g.details.iter().find_map(|d| match d {
        ShapeDetail::Gradient { item_index, gradient_type, highlight_length, highlight_angle } => {
            Some((*item_index, *gradient_type, *highlight_length, *highlight_angle))
        }
        _ => None,
    });
    let (index, kind, length, angle) = gradient.unwrap();
    assert_eq!(index, 1);
    assert_eq!((kind.value, kind.source), (GradientType::Radial, ValueSource::Stored));
    assert_eq!(length.map(|t| (t.value, t.source)), Some((0.0, ValueSource::Stored)));
    assert_eq!(angle.map(|t| (t.value, t.source)), Some((0.0, ValueSource::Stored)));
    // 241 (linear): the type is the default and the highlight records are absent.
    let bytes = fixture("241-shape-gradient-fill-linear.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_named(&first_comp(&project), "S");
    let g = layer.shape_groups().remove(0);
    let linear = g.details.iter().find_map(|d| match d {
        ShapeDetail::Gradient { gradient_type, highlight_length, highlight_angle, .. } => Some((*gradient_type, *highlight_length, *highlight_angle)),
        _ => None,
    });
    let (kind, length, angle) = linear.unwrap();
    assert_eq!((kind.value, kind.source), (GradientType::Linear, ValueSource::Default));
    assert!(length.is_none() && angle.is_none());
}

#[test]
fn two_rectangles_in_one_group_are_two_items() {
    // 333: Rectangle Path 1 [400,300], Rectangle Path 2 [100,100] (= default, no Size record) at
    // position [300,0], Fill 1 (order: rect, rect, fill).
    let g = shape_group("333-shape-group-two-rects.aep");
    assert_eq!(g.items.len(), 3);
    let ShapeItem::Rect { name: n1, size: s1 } = &g.items[0] else { panic!() };
    let ShapeItem::Rect { name: n2, size: s2 } = &g.items[1] else { panic!() };
    assert_eq!((n1.as_deref(), n2.as_deref()), (Some("Rectangle Path 1"), Some("Rectangle Path 2")));
    assert_eq!(s1.map(|s| s.value), Some([400.0, 300.0]));
    assert!(s2.is_none(), "[100, 100] is the default size");
    assert!(matches!(&g.items[2], ShapeItem::Fill { color: Some(c), .. } if c.value == [1.0, 0.0, 1.0, 0.0]));
    let positions: Vec<_> = g
        .details
        .iter()
        .filter_map(|d| match d {
            ShapeDetail::RectPosition { item_index, position } => Some((*item_index, position.value, position.source)),
            _ => None,
        })
        .collect();
    assert_eq!(positions, [(0, [0.0, 0.0], ValueSource::Default), (1, [300.0, 0.0], ValueSource::Stored)]);
}

// ------------------------------------------------------------------ text (334-340)

fn text(name: &str, layer: &str) -> TextDocument {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    layer_named(&first_comp(&project), layer).text_document().unwrap_or_else(|| panic!("{name}: no text document"))
}

#[test]
fn right_justification_in_box_text() {
    // 334: box text "Hello" in a 400x200 box at [-200,-100], right aligned (stored 1; compare 245: 0, 246: 2).
    let t = text("334-text-box-right-align.aep", "T");
    assert_eq!(t.text, "Hello");
    assert_eq!((t.paragraphs.len(), t.paragraphs[0].length, t.paragraphs[0].justification), (1, 6, Justification::Right));
    let b = t.text_box.unwrap();
    assert_eq!((b.x, b.y, b.width, b.height), (-200.0, -100.0, 400.0, 200.0));
    assert_eq!(text("245-text-box-400x200-Hello.aep", "T").paragraphs[0].justification, Justification::Left);
    assert_eq!(text("246-text-box-center-align.aep", "T").paragraphs[0].justification, Justification::Center);
}

#[test]
fn faux_styles_caps_and_leading() {
    // The reference style: no faux styles, normal caps, auto leading (stored 98), the font list of four.
    let t = text("337-text-point-allcaps.aep", "T");
    assert_eq!(t.style_runs.len(), 1);
    let plain = t.style_runs[0];
    assert_eq!((plain.faux_bold, plain.faux_italic, plain.leading, plain.font_index), (false, false, Some(98.0), 0));
    assert_eq!((plain.caps, t.fonts.len()), (Caps::All, 4));
    // 335 / 336: faux bold / italic; the font list gains a first entry (4 -> 5) and the style uses font 1.
    for (name, bold, italic) in [("335-text-point-fauxbold.aep", true, false), ("336-text-point-fauxitalic.aep", false, true)] {
        let t = text(name, "T");
        let s = t.style_runs[0];
        assert_eq!((s.faux_bold, s.faux_italic, s.caps), (bold, italic, Caps::Normal), "{name}");
        assert_eq!((t.fonts.len(), s.font_index, t.font_index), (5, 1, 1), "{name}");
        assert_eq!((t.fonts[0].as_str(), t.fonts[1].as_str()), ("TimesNewRomanPSMT", "TimesNewRomanPSMT"), "{name}");
        assert_eq!(t.font.as_deref(), Some("TimesNewRomanPSMT"));
    }
    // 338: small caps; 337: all caps.
    assert_eq!(text("338-text-point-smallcaps.aep", "T").style_runs[0].caps, Caps::Small);
    // 339: leading 60.
    let s = text("339-text-point-leading60.aep", "T").style_runs[0];
    assert_eq!((s.leading, s.caps, s.faux_bold), (Some(60.0), Caps::Normal, false));
}

#[test]
fn the_last_style_run_absorbs_the_final_carriage_return() {
    // 340: "el" red, "lo" blue and size 60 on "Hello": runs of 1, 2 and 3 characters (the last one
    // includes the \r), fills white / red / blue, sizes 100 / 100 / 60.
    let t = text("340-text-range-two-styles.aep", "T");
    assert_eq!(t.raw_text, "Hello\r");
    assert_eq!(t.style_runs.iter().map(|r| r.length).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(t.style_runs.iter().map(|r| r.length).sum::<usize>(), t.raw_text.chars().count());
    assert_eq!(t.style_runs.iter().map(|r| r.fill_argb).collect::<Vec<_>>(), [[1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]]);
    assert_eq!(t.style_runs.iter().map(|r| r.font_size).collect::<Vec<_>>(), [100.0, 100.0, 60.0]);
    assert_eq!(t.paragraphs.iter().map(|p| p.length).collect::<Vec<_>>(), [6]);
}

// ------------------------------------------------------------------ camera and light (341-345)

#[test]
fn camera_zoom_and_depth_of_field() {
    let camera = |name: &str| {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let layer = layer_of_type(&first_comp(&project), LayerType::Camera);
        (
            layer.camera_zoom(),
            layer.camera_depth_of_field(),
            layer.camera_focus_distance(),
            layer.camera_aperture(),
            layer.camera_position().map(|p| (p.stored, p.auto_state)),
        )
    };
    // 341: Zoom 2000 is stored; the auto Position keeps [0, 0, -2666.67].
    let (zoom, dof, focus, aperture, pos) = camera("341-camera-zoom-2000.aep");
    assert_eq!(zoom.map(|z| (z.value, z.source)), Some((2000.0, ValueSource::Stored)));
    assert_eq!((dof.map(|d| (d.value, d.source)), focus, aperture), (Some((false, ValueSource::Default)), None, None));
    let (stored, auto) = pos.unwrap();
    assert!(auto && near(stored[2], -2666.67, 0.01) && stored[0] == 0.0, "{stored:?}");
    // 342: DOF on, Focus Distance 800, Aperture 30; Zoom stays the default.
    let (zoom, dof, focus, aperture, _) = camera("342-camera-dof-800-30.aep");
    assert_eq!(zoom.map(|z| (z.source, near(z.value, 2666.67, 0.01))), Some((ValueSource::Default, true)));
    assert_eq!(dof.map(|d| (d.value, d.source)), Some((true, ValueSource::Stored)));
    assert_eq!(focus.map(|f| (f.value, f.source)), Some((800.0, ValueSource::Stored)));
    assert_eq!(aperture.map(|f| (f.value, f.source)), Some((30.0, ValueSource::Stored)));
    // The default camera (256) has nothing but the default zoom.
    let (zoom, dof, ..) = camera("256-camera-two-node-default.aep");
    assert_eq!((zoom.unwrap().source, dof.unwrap().value), (ValueSource::Default, false));
    assert_eq!(zoom.unwrap().value, effectcraft_aep::DEFAULT_CAMERA_ZOOM);
}

#[test]
fn light_colour_cone_angle_and_feather() {
    let light = |name: &str| {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let layer = layer_of_type(&first_comp(&project), LayerType::Light);
        assert_eq!(layer.header().unwrap().light_type(), Some(effectcraft_aep::LightType::Spot), "{name}");
        let t = |x: Option<effectcraft_aep::Tracked<f64>>| x.map(|x| (x.value, x.source));
        (layer.light_color().map(|c| (c.value, c.source)), t(layer.light_cone_angle()), t(layer.light_cone_feather()))
    };
    // The default spot light: white, 90, 50, all absent.
    let (colour, angle, feather) = light("254-light-spot-intensity-50.aep");
    assert_eq!((colour, angle, feather), (Some(([1.0; 4], ValueSource::Default)), Some((90.0, ValueSource::Default)), Some((50.0, ValueSource::Default))));
    // 343: colour [1,0,0,1] = ARGB doubles [255,255,0,0].
    let (colour, angle, feather) = light("343-light-spot-color-red.aep");
    assert_eq!(colour, Some(([1.0, 1.0, 0.0, 0.0], ValueSource::Stored)));
    assert_eq!((angle, feather), (Some((90.0, ValueSource::Default)), Some((50.0, ValueSource::Default))));
    // 344: Cone Angle 60. 345: Cone Feather 30.
    let (colour, angle, feather) = light("344-light-spot-coneangle-60.aep");
    assert_eq!((colour.unwrap().1, angle, feather), (ValueSource::Default, Some((60.0, ValueSource::Stored)), Some((50.0, ValueSource::Default))));
    let (_, angle, feather) = light("345-light-spot-conefeather-30.aep");
    assert_eq!((angle, feather), (Some((90.0, ValueSource::Default)), Some((30.0, ValueSource::Stored))));
    // A point light (252) stores Cone Angle 90 / Feather 50 in their default state.
    let bytes = fixture("252-light-point.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_of_type(&first_comp(&project), LayerType::Light);
    assert_eq!(layer.light_cone_angle().map(|a| (a.value, a.source)), Some((90.0, ValueSource::Stored)));
    // Layers without light options have no light accessors.
    let bytes = fixture("000-base.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let solid = layer_with_id(&first_comp(&project), L);
    assert!(solid.light_color().is_none() && solid.camera_zoom().is_none() && solid.camera_depth_of_field().is_none());
}

// ------------------------------------------------------------------ marker data (347-349, 249)

fn layer_markers(name: &str) -> Vec<effectcraft_aep::Marker> {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    layer_with_id(&first_comp(&project), L).markers()
}

#[test]
fn marker_label_cue_type_and_parameters() {
    // 347: marker at 1 s, comment M, label 3 (event cue by default, no parameters).
    let m = layer_markers("347-marker-layer-label3.aep");
    assert_eq!(m.len(), 1);
    assert_eq!((m[0].seconds, m[0].comment.as_str(), m[0].label, m[0].cue_type), (Some(1.0), "M", Some(3), Some(CueType::Event)));
    assert_eq!((m[0].duration_seconds, m[0].cue_point_name.as_str(), m[0].cue_params.len()), (Some(0.0), "", 0));
    // 348: cue point name CueN, navigation cue (5).
    let m = layer_markers("348-marker-layer-navigation-cue.aep");
    assert_eq!((m[0].cue_type, m[0].cue_point_name.as_str(), m[0].label), (Some(CueType::Navigation), "CueN", Some(0)));
    // 349: CueE, event cue, parameters speaker = A and n = 2.
    let m = layer_markers("349-marker-layer-event-cue-params.aep");
    assert_eq!((m[0].cue_type, m[0].cue_point_name.as_str(), m[0].comment.as_str()), (Some(CueType::Event), "CueE", "M"));
    assert_eq!(m[0].cue_params, [("speaker".to_string(), "A".to_string()), ("n".to_string(), "2".to_string())]);
    assert_eq!((m[0].chapter.as_str(), m[0].url.as_str(), m[0].frame_target.as_str()), ("", "", ""));
    // 249 (batch 2): duration 1 s, the five strings, event cue, no parameters, label 0.
    let m = layer_markers("249-marker-layer-full.aep");
    assert_eq!((m[0].cue_type, m[0].label, m[0].duration_seconds, m[0].cue_params.len()), (Some(CueType::Event), Some(0), Some(1.0), 0));
}

// ------------------------------------------------------------------ footage items (126, 350, 352, 354)

#[test]
fn audio_footage_item() {
    let bytes = fixture("350-footage-wav-imported.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    // The solid L and the WAV (item id 32): the WAV is not used in a layer.
    let footage: Vec<_> = project.all_items().into_iter().filter(|i| i.kind() == Some(ItemKind::Footage)).collect();
    assert_eq!(footage.len(), 2);
    let item = footage.iter().find(|i| i.id() == Some(0x20)).unwrap();
    let f = item.footage().unwrap();
    assert_eq!(f.file_type, "WAVE");
    assert_eq!((f.width, f.height, f.is_solid), (0, 0, false));
    assert_eq!(f.duration, Some(rat(48000, 48000)));
    assert_eq!((f.audio_sample_rate, f.file_size, f.bits_per_pixel, f.native_frame_rate), (Some(48000.0), 96044, 0, 0));
    assert!(!f.is_sequence && f.sequence_frame_count().is_none());
    assert_eq!(item.display_name().as_deref(), Some("test-tone.wav"));
    assert_eq!(item.label(), Some(7));
    assert!(f.path.as_deref().is_some_and(|p| p.ends_with("test-tone.wav")));
    // The solid has no sample rate.
    let solid = footage.iter().find(|i| i.id() == Some(0x1a)).unwrap().footage().unwrap();
    assert_eq!((solid.audio_sample_rate, solid.duration.map(|d| d.num), solid.file_size), (None, Some(0), 0));
}

#[test]
fn image_sequence_is_one_item() {
    for name in ["352-footage-pngseq-imported.aep", "354-footage-pngseq-timeremap.aep"] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let footage: Vec<_> = project.all_items().into_iter().filter(|i| i.kind() == Some(ItemKind::Footage)).collect();
        assert_eq!(footage.len(), 2, "{name}: the solid and ONE sequence item");
        let item = footage.iter().find(|i| i.id() == Some(0x22)).unwrap();
        let f = item.footage().unwrap();
        assert_eq!((f.width, f.height, f.is_solid), (64, 64, false), "{name}");
        assert_eq!(f.duration, Some(rat(3, 30)), "{name}");
        assert_eq!((f.native_frame_rate, f.bits_per_pixel, f.file_size, f.audio_sample_rate), (30, 24, 504, None), "{name}");
        assert!(f.is_sequence, "{name}");
        assert_eq!(f.sequence_frame_count(), Some(3), "{name}");
        // The first file name sits in opti; the alias points at the sequence folder.
        assert_eq!(f.name.as_deref(), Some("seq_001.png"), "{name}");
        assert!(f.path.as_deref().is_some_and(|p| p.ends_with("test-seq")), "{name}");
        assert_eq!(item.label(), Some(3), "{name}");
    }
    // The single png still (126): 32 bits per pixel, 149 bytes, not a sequence, duration 0.
    let bytes = fixture("126-footage-png-layer.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let f = project.all_items().into_iter().filter_map(|i| i.footage()).find(|f| !f.is_solid).unwrap();
    assert_eq!((f.bits_per_pixel, f.file_size, f.native_frame_rate, f.is_sequence), (32, 149, 0, false));
    assert_eq!(f.duration.map(|d| d.num), Some(0));
}
