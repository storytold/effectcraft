use super::*;

/// Alt/Option-drag takes the same Project items `layer.replaceSource` does: compositions,
/// footage and solids, never folders, data files or 3D models.
#[test]
fn replace_source_takes_visual_items_only() {
    use effectcraft_engine::color::Label;
    use effectcraft_engine::project::{Comp, Footage, Project, Solid};
    use effectcraft_engine::time::{FrameRate, Tick};
    let mut p = Project::default();
    let l = Label::None;
    let comp = p.add_item("C", l, None, ItemKind::Comp(Comp::new(10, 10, FrameRate::FPS_30, Tick::from_seconds_f64(1.0)).into()));
    let folder = p.add_item("F", l, None, ItemKind::Folder);
    let still = p.add_item("S", l, None, ItemKind::Footage(Footage { kind: FootageKind::Still, ..Default::default() }));
    let data = p.add_item("D", l, None, ItemKind::Footage(Footage { kind: FootageKind::Data, ..Default::default() }));
    let model = p.add_item("M", l, None, ItemKind::Footage(Footage { kind: FootageKind::Model, ..Default::default() }));
    let solid = p.add_item("So", l, None, ItemKind::Solid(Solid { color: [0.0; 3], width: 10, height: 10, pixel_aspect: 1.0 }));
    for (id, ok) in [(comp, true), (still, true), (solid, true), (folder, false), (data, false), (model, false)] {
        assert_eq!(replaceable_item(p.item(id).unwrap()), ok, "{id:?}");
    }
}

/// Alt/Cmd-click twirls Audio's Waveform row (not in the property tree) with the rest.
#[test]
fn the_waveform_row_twirls_with_its_tree() {
    let mut root = PropGroup::new(1, "", "");
    let mut audio = PropGroup::new(2, "audio", "Audio");
    audio.children.push(PropGroup::new(3, "x", "X").into());
    root.children.push(audio.into());
    let mut uids = vec![];
    walk_groups(&root, &mut uids);
    assert_eq!(uids, vec![2, WAVE_BIT | 2, 3]);
    let mut uids = vec![];
    walk_groups(find_group(&root, 2).unwrap(), &mut uids);
    assert_eq!(uids, vec![WAVE_BIT | 2, 3]);
}

/// #710: with Time Display set to Frames the ruler labels are frame numbers (as under the
/// CTI, from Start Numbering Frames At); otherwise seconds:frames.
#[test]
fn ruler_labels_follow_the_time_display_style() {
    use effectcraft_engine::project::{ProjectSettings, TimeDisplayStyle};
    let mut st = ProjectSettings { time_display: TimeDisplayStyle::Timecode, frame_start: 0, ..ProjectSettings::default() };
    assert_eq!(ruler_label(15, 30, &st), "00:15f");
    assert_eq!(ruler_label(45, 30, &st), "01:15f");
    assert_eq!(ruler_label(30 * 3600 + 31, 30, &st), "60:01:01f");
    st.time_display = TimeDisplayStyle::Frames;
    assert_eq!(ruler_label(0, 30, &st), "00000");
    assert_eq!(ruler_label(45, 30, &st), "00045");
    st.frame_start = 1;
    assert_eq!(ruler_label(45, 30, &st), "00046");
    assert_eq!(ruler_label(i64::MAX, 30, &st), format!("{:05}", i64::MAX));
}

/// #468: playback pages the view to the CTI; a scrub past either end scrolls that way.
#[test]
fn the_view_pages_to_the_cti() {
    assert_eq!(page_to_cti(10.0, 5.0, 60.0, 12.0), 10.0, "in view: unchanged");
    assert_eq!(page_to_cti(10.0, 5.0, 60.0, 16.0), 16.0, "past the right end: the page from the CTI");
    assert_eq!(page_to_cti(10.0, 5.0, 60.0, 3.0), 3.0, "before the start");
    assert_eq!(page_to_cti(10.0, 5.0, 60.0, 58.0), 55.0, "not past the comp's end");
    assert_eq!(page_to_cti(10.0, 80.0, 60.0, 95.0), 0.0);
    assert_eq!(page_to_cti(10.0, f64::NAN, 60.0, 95.0), 10.0);
    assert_eq!(edge_scroll_speed_x(100.0, 500.0, 300.0), 0.0);
    assert!(edge_scroll_speed_x(100.0, 500.0, 501.0) > 0.0 && edge_scroll_speed_x(100.0, 500.0, 99.0) < 0.0);
    assert!(edge_scroll_speed_x(100.0, 500.0, 600.0) > edge_scroll_speed_x(100.0, 500.0, 520.0));
}

/// #864: picking only a mode in a layer's Track Matte menu sets a matte (the layer above);
/// it used to close the menu and change nothing.
#[test]
fn a_track_matte_mode_without_a_matte_takes_the_layer_above() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 64, "height": 64, "duration": 1})).unwrap();
    for name in ["Chain Bottom", "Matte", "Chain Top"] {
        s.execute("layer.newSolid", json!({"name": name, "color": "#ffffff", "width": 64, "height": 64})).unwrap();
    }
    let find = |s: &effectcraft_engine::Session, n: &str| s.active_comp().unwrap().layers.iter().find(|l| l.name == n).unwrap().clone();
    let (bottom, matte, top) = (find(&s, "Chain Bottom"), find(&s, "Matte"), find(&s, "Chain Top"));
    let candidates = [top.id, matte.id];
    let pick = |s: &mut effectcraft_engine::Session, name: &str, i: usize, choice: &str| {
        let (id, params) = track_matte_pick(&[find(s, name).id.0], &candidates, i, choice).expect("a menu item is an action");
        s.execute(&id, params)
    };
    pick(&mut s, "Chain Bottom", 5, "Alpha Inverted Matte").unwrap();
    let m = find(&s, "Chain Bottom").track_matte.expect("a matte");
    assert_eq!((m.layer, m.kind), (matte.id, MatteKind::AlphaInverted));
    assert!(!find(&s, "Matte").switches.video, "the matte layer is hidden");
    // A mode change keeps the matte layer; a matte change keeps the mode.
    pick(&mut s, "Chain Bottom", 6, "Luma Matte").unwrap();
    assert_eq!(find(&s, "Chain Bottom").track_matte.map(|m| (m.layer, m.kind)), Some((matte.id, MatteKind::Luma)));
    pick(&mut s, "Chain Bottom", 2, "1. Chain Top").unwrap();
    assert_eq!(find(&s, "Chain Bottom").track_matte.map(|m| (m.layer, m.kind)), Some((top.id, MatteKind::Luma)));
    assert!(s.undo());
    assert_eq!(find(&s, "Chain Bottom").track_matte.map(|m| m.layer), Some(matte.id), "one undo step per pick");
    // The top layer has nothing above it: an error for the status bar, not a silent no-op.
    assert!(pick(&mut s, "Chain Top", 5, "Alpha Matte").is_err());
    pick(&mut s, "Chain Bottom", 0, "No Track Matte").unwrap();
    assert!(find(&s, "Chain Bottom").track_matte.is_none());
    assert!(track_matte_pick(&[bottom.id.0], &candidates, 1, "-").is_none(), "a separator does nothing");
}

/// #957: vector footage gets the Continuously Rasterize switch; raster footage does not.
#[test]
fn collapse_switch_shows_for_vector_footage_only() {
    use effectcraft_engine::color::Label;
    use effectcraft_engine::project::{Footage, ItemKind, Project};
    let mut p = Project::default();
    let mut layer_for = |name: &str, codec: &str| {
        let footage = Footage { path: name.into(), codec: codec.into(), ..Default::default() };
        let item = p.add_item(name, Label::SeaFoam, None, ItemKind::Footage(footage));
        let comp = Comp::new(64, 36, effectcraft_engine::time::FrameRate::new(30, 1), Tick::from_seconds_f64(1.0));
        effectcraft_engine::project::build::layer(&mut p, &comp, name, LayerSource::Footage { item }, (0, 0), None)
    };
    let svg = layer_for("logo.svg", "SVG");
    let png = layer_for("photo.png", "PNG");
    assert!(collapse_applies(&svg, &p));
    assert!(!collapse_applies(&png, &p));
}

/// A comp with two solids; "Box" has an animated Position and a Gaussian Blur.
fn app() -> EffectcraftApp {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080", "width": 320, "height": 180})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Box", "color": "#e04020", "width": 40, "height": 40})).unwrap();
    let comp = s.active_comp().unwrap();
    let bx = comp.layers.iter().find(|l| l.name == "Box").unwrap().id.0;
    s.execute("layer.select", json!({"layers": [bx]})).unwrap();
    s.execute("effect.apply", json!({"effect": "ec.blur.gaussian"})).unwrap();
    let pos = s.active_comp().unwrap().layer(LayerId(bx)).unwrap().transform().unwrap().get("position").unwrap().uid;
    s.execute("prop.toggleAnimation", json!({"layer": bx, "prop": pos})).unwrap();
    EffectcraftApp::new(s)
}

fn labels(app: &EffectcraftApp) -> Vec<String> {
    let comp = app.session.active_comp().unwrap().clone();
    build_rows(app, &comp)
        .into_iter()
        .map(|r| {
            let l = comp.layer(r.layer).unwrap();
            match r.kind {
                RowKind::Layer => l.name.clone(),
                RowKind::Group { name, .. } => format!("{}>{name}", "  ".repeat(r.depth)),
                RowKind::Prop { uid } => format!("{}{}", "  ".repeat(r.depth), l.props.find(uid).unwrap().name),
                _ => "?".into(),
            }
        })
        .collect()
}

#[test]
fn search_filters_layers_and_reveals_matching_properties() {
    let mut app = app();
    app.ui.timeline.search = "opac".into();
    let rows = labels(&app);
    // Both layers have Transform > Opacity; nothing else shows.
    assert_eq!(rows.iter().filter(|r| r.trim() == "Opacity").count(), 2, "{rows:?}");
    assert!(rows.iter().any(|r| r.trim() == ">Transform"), "{rows:?}");
    assert!(!rows.iter().any(|r| r.trim() == "Position"), "{rows:?}");
    // A layer name match shows the layer alone; a group match brings its contents.
    app.ui.timeline.search = "plate".into();
    assert_eq!(labels(&app), vec!["Plate".to_string()]);
    app.ui.timeline.search = "GAUSSIAN".into();
    let rows = labels(&app);
    assert_eq!(rows[0], "Box");
    assert!(rows.iter().any(|r| r.trim() == ">Effects") && rows.iter().any(|r| r.trim() == "Blurriness"), "{rows:?}");
    assert!(!rows.contains(&"Plate".to_string()));
    // Nothing matches → no rows.
    app.ui.timeline.search = "zzz".into();
    assert!(labels(&app).is_empty());
    // Through the command.
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "timeline.search", json!({"query": "scale"})).unwrap();
    assert!(labels(&app).iter().any(|r| r.trim() == "Scale"));
}

/// A shape group's inner Contents (AE's elided `ADBE Vectors Group`) draws no row: the
/// search shows the full row hierarchy with the group's items — path, fill and Transform —
/// listing directly under the group, and the only "Contents" row the layer's own.
#[test]
fn shape_group_inner_contents_is_elided() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "T", "width": 320, "height": 180, "duration": 4})).unwrap();
    s.execute("layer.newShape", json!({"kind": "rect"})).unwrap();
    let mut app = EffectcraftApp::new(s);
    app.ui.timeline.search = "rect".into();
    assert_eq!(
        labels(&app),
        [
            "Shape Layer 1",
            "  >Contents",
            "    >Rectangle 1",
            "      >Rectangle Path 1",
            "        Size",
            "        Position",
            "        Roundness",
            "        Per-Corner Roundness",
            "        Top Left Roundness",
            "        Top Right Roundness",
            "        Bottom Right Roundness",
            "        Bottom Left Roundness",
            "      >Fill 1",
            "        Composite",
            "        Fill Rule",
            "        Color",
            "        Opacity",
            "      >Transform",
            "        Anchor Point",
            "        Position",
            "        Scale",
            "        Skew",
            "        Skew Axis",
            "        Rotation",
            "        Opacity",
        ]
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
    );
}

#[test]
fn columns_toggle_and_layout() {
    let mut app = app();
    let tl = &mut app.ui.timeline;
    assert!(column_visible(tl, "switches") && !column_visible(tl, "comment") && !column_visible(tl, "modes"));
    set_column(tl, "comment", true).unwrap();
    set_column(tl, "stretch", true).unwrap();
    set_column(tl, "keys", false).unwrap();
    set_column(tl, "label", false).unwrap();
    set_column(tl, "modes", true).unwrap();
    assert!(tl.show_modes && column_visible(tl, "comment") && !column_visible(tl, "label"));
    assert!(set_column(tl, "name", false).is_err());
    assert!(set_column(tl, "bogus", true).is_err());
    let vis = Vis::of(tl);
    let cw = cols(0.0, 1400.0, vis);
    // Hidden columns take no room; visible ones are in After Effects' order.
    assert_eq!(cw.keys, cw.label);
    assert_eq!(cw.label, cw.num);
    assert!(cw.name < cw.comment && cw.comment < cw.switches && cw.switches < cw.mode && cw.parent < cw.stretch);
    assert_eq!(column_at(&cw, cw.comment + 5.0), Some("comment"));
    assert_eq!(column_at(&cw, cw.name + 5.0), Some("name"));
    assert_eq!(column_at(&cw, cw.stretch + 5.0), Some("stretch"));
    // Through the command (as agents do).
    let ctx = egui::Context::default();
    let r = crate::menus::invoke(&mut app, &ctx, "timeline.column", json!({"column": "duration"})).unwrap();
    assert!(r["columns"].as_array().unwrap().iter().any(|c| c == "duration"));
    crate::menus::invoke(&mut app, &ctx, "timeline.column", json!({"column": "duration", "visible": false})).unwrap();
    assert!(!column_visible(&app.ui.timeline, "duration"));
    crate::menus::invoke(&mut app, &ctx, "timeline.sourceName", json!({})).unwrap();
    assert!(app.ui.timeline.source_name);
}

#[test]
fn reveal_keys_add_with_shift_and_uu_reveals_modified() {
    let mut app = app();
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "timeline.reveal.opacity", json!({})).unwrap();
    assert_eq!(app.ui.timeline.reveal, vec!["opacity"]);
    crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.scale", json!({})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.effects", json!({})).unwrap();
    assert_eq!(app.ui.timeline.reveal, vec!["opacity", "scale", "effects"]);
    let rows = labels(&app);
    assert!(rows.iter().any(|r| r.trim() == "Scale") && rows.iter().any(|r| r.trim() == "Opacity"), "{rows:?}");
    assert!(rows.iter().any(|r| r.trim() == ">Gaussian Blur"), "{rows:?}");
    // Tree order: Scale before Opacity.
    let si = rows.iter().position(|r| r.trim() == "Scale").unwrap();
    let oi = rows.iter().position(|r| r.trim() == "Opacity").unwrap();
    assert!(si < oi);
    // Shift+S again removes Scale.
    crate::menus::invoke(&mut app, &ctx, "timeline.revealAdd.scale", json!({})).unwrap();
    assert_eq!(app.ui.timeline.reveal, vec!["opacity", "effects"]);
    // U: keyframed properties (Position); UU (twice quickly): every modified property.
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    let n_keys = app.ui.timeline.reveal_props.len();
    assert_eq!(app.ui.timeline.reveal, vec!["props"]);
    assert!(n_keys >= 1);
    let bx = app.session.state.selected_layers[0].0;
    app.session.execute("prop.set", json!({"layer": bx, "path": "effects/#1/blurriness", "value": 12})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    assert!(app.ui.timeline.reveal_props.len() > n_keys, "UU reveals modified properties too");
}

#[test]
fn scroll_to_time_centres_the_current_time_inside_the_comp() {
    assert_eq!(centered_on(4.0, 10.0, 5.0), 3.0);
    // Near the ends the view stops at the comp's start and end.
    assert_eq!(centered_on(4.0, 10.0, 1.0), 0.0);
    assert_eq!(centered_on(4.0, 10.0, 9.5), 6.0);
    // A span longer than the comp, and bad numbers, start at zero.
    assert_eq!(centered_on(20.0, 10.0, 5.0), 0.0);
    assert_eq!(centered_on(f64::NAN, 10.0, 5.0), 0.0);
}

/// U and the other reveal shortcuts act on the selected layers only: other layers keep their
/// twirl state and their own reveal (#160).
#[test]
fn reveal_shortcuts_leave_unselected_layers_alone() {
    let mut app = app();
    let ctx = egui::Context::default();
    let comp = app.session.active_comp().unwrap().clone();
    let id = |n: &str| comp.layers.iter().find(|l| l.name == n).unwrap().id.0;
    let (plate, bx) = (id("Plate"), id("Box"));
    let select = |app: &mut EffectcraftApp, l: u64| app.session.execute("layer.select", json!({"layers": [l]})).unwrap();
    // Plate shows Scale (S); then U on Box.
    select(&mut app, plate);
    crate::menus::invoke(&mut app, &ctx, "timeline.reveal.scale", json!({})).unwrap();
    select(&mut app, bx);
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    let tl = &app.ui.timeline;
    assert!(tl.open_layers.contains(&plate) && tl.open_layers.contains(&bx), "{:?}", tl.open_layers);
    assert_eq!(tl.layer_reveal.get(&plate), Some(&vec!["scale".to_string()]), "Plate keeps its reveal");
    assert_eq!(tl.layer_reveal.get(&bx), Some(&vec!["props".to_string()]));
    let rows = labels(&app);
    let at = |n: &str| rows.iter().position(|r| r == n).unwrap();
    assert_eq!(rows[at("Plate") + 1].trim(), "Scale", "{rows:?}");
    assert_eq!(rows[at("Box") + 1].trim(), "Position", "{rows:?}");
    // UU (modified): the effect's changed parameter only, under the effect twirled open.
    // Then U later: Box shows its keyframed properties only again.
    app.session.execute("prop.set", json!({"layer": bx, "path": "effects/#1/blurriness", "value": 12})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    let rows = labels(&app);
    let fx = rows.iter().position(|r| r.trim().ends_with("Gaussian Blur")).unwrap_or_else(|| panic!("UU shows the effect: {rows:?}"));
    assert_eq!(rows[fx + 1].trim(), "Blurriness", "{rows:?}");
    assert!(!rows.iter().any(|r| r.trim() == "Blur Dimensions"), "unchanged parameters stay hidden: {rows:?}");
    // Collapsed and expanded by hand, the effect shows all its parameters; UU filters again.
    let fx_uid = app.session.active_comp().unwrap().layer(LayerId(bx)).unwrap().effects().unwrap().groups().next().unwrap().uid;
    app.ui.timeline.toggle_group(bx, fx_uid);
    assert!(!labels(&app).iter().any(|r| r.trim() == "Blurriness"), "collapsed");
    app.ui.timeline.toggle_group(bx, fx_uid);
    let rows = labels(&app);
    assert!(rows.iter().any(|r| r.trim() == "Blurriness") && rows.iter().any(|r| r.trim() == "Blur Dimensions"), "{rows:?}");
    app.last_reveal = Some(("keyframes".into(), f64::MAX));
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    assert!(!labels(&app).iter().any(|r| r.trim() == "Blur Dimensions"), "UU again shows the changed ones only");
    app.last_reveal = Some(("keyframes".into(), -10.0));
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    app.last_reveal = Some(("keyframes".into(), -10.0));
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    let rows = labels(&app);
    assert!(!rows.iter().any(|r| r.trim() == ">Gaussian Blur") && rows.iter().any(|r| r.trim() == "Position"), "{rows:?}");
    // U again on Box (later than a double press) hides only Box's.
    app.last_reveal = Some(("keyframes".into(), -10.0));
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    let tl = &app.ui.timeline;
    assert!(!tl.open_layers.contains(&bx) && tl.open_layers.contains(&plate), "{:?}", tl.open_layers);
    // A layer twirled open by hand shows its whole tree, whatever was revealed elsewhere.
    app.session.execute("layer.select", json!({"layers": []})).unwrap();
    app.ui.timeline.open_layers.insert(bx);
    let rows = labels(&app);
    assert!(rows.iter().any(|r| r.trim() == ">Transform"), "{rows:?}");
}

/// U and UU show Time Remap on a layer with time remapping enabled.
#[test]
fn u_and_uu_reveal_time_remap() {
    let mut s = effectcraft_engine::Session::default();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "duration": 4})).unwrap();
    let nested = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    let mut app = EffectcraftApp::new(s);
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({"layers": [nested]})).unwrap();
    app.session.execute("layer.select", json!({"layers": [nested]})).unwrap();
    for uu in [false, true] {
        app.ui.timeline.layer_reveal.clear();
        app.last_reveal = uu.then(|| ("keyframes".into(), f64::MAX));
        crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
        assert_eq!(app.ui.timeline.layer_reveal.get(&nested), Some(&vec!["props".to_string()]), "UU: {uu}");
        let rows = labels(&app);
        let at = rows.iter().position(|r| r == "Inner").unwrap();
        assert_eq!(rows[at + 1].trim(), "Time Remap", "UU: {uu}: {rows:?}");
    }
}

/// U shows Time Remap while remapping is enabled even after its keys are deleted (#990):
/// enabling remapping is itself the modification.
#[test]
fn u_reveals_keyless_time_remap() {
    let mut s = effectcraft_engine::Session::default();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "duration": 4})).unwrap();
    let nested = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    let mut app = EffectcraftApp::new(s);
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({"layers": [nested]})).unwrap();
    let uid = app.session.active_comp().unwrap().layer(LayerId(nested)).unwrap().props.prop("timeRemap").unwrap().uid;
    for _ in 0..2 {
        let t = app.session.active_comp().unwrap().layer(LayerId(nested)).unwrap().props.prop("timeRemap").unwrap().keys[0].time.seconds();
        app.session.execute("keys.select", json!({"keys": [{"layer": nested, "prop": uid, "time": t}]})).unwrap();
        app.session.execute("edit.clear", json!({})).unwrap();
    }
    assert!(app.session.active_comp().unwrap().layer(LayerId(nested)).unwrap().props.prop("timeRemap").unwrap().keys.is_empty());
    app.session.execute("layer.select", json!({"layers": [nested]})).unwrap();
    crate::menus::invoke(&mut app, &ctx, "anim.reveal", json!({"kind": "keyframes"})).unwrap();
    assert_eq!(app.ui.timeline.layer_reveal.get(&nested), Some(&vec!["props".to_string()]));
    let rows = labels(&app);
    let at = rows.iter().position(|r| r == "Inner").unwrap();
    assert_eq!(rows[at + 1].trim(), "Time Remap", "{rows:?}");
}

/// Enabling time remapping twirls the layer open on Time Remap (#157); disabling it reveals
/// nothing.
#[test]
fn enabling_time_remapping_reveals_time_remap() {
    let mut s = effectcraft_engine::Session::default();
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 32, "height": 32, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "duration": 4})).unwrap();
    s.execute("layer.newSolid", json!({"name": "Other"})).unwrap();
    let nested = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    let mut app = EffectcraftApp::new(s);
    let ctx = egui::Context::default();
    crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({"layers": [nested]})).unwrap();
    assert_eq!(app.ui.timeline.layer_reveal.get(&nested), Some(&vec!["timeRemap".to_string()]));
    let rows = labels(&app);
    let at = rows.iter().position(|r| r == "Inner").unwrap();
    assert_eq!(rows[at + 1].trim(), "Time Remap", "{rows:?}");
    // Through the selection (the Layer menu), turning it off again: nothing new is revealed.
    app.session.execute("layer.select", json!({"layers": [nested]})).unwrap();
    app.ui.timeline.layer_reveal.clear();
    crate::menus::invoke(&mut app, &ctx, "layer.enableTimeRemap", json!({})).unwrap();
    assert!(app.ui.timeline.layer_reveal.is_empty());
}

#[test]
fn cti_snaps_to_the_nearest_candidate() {
    assert_eq!(snap_time(&[0.0, 1.0, 2.0], 1.04, 0.1), 1.0);
    assert_eq!(snap_time(&[0.0, 1.0, 2.0], 1.5, 0.1), 1.5);
    assert_eq!(snap_time(&[1.0, 1.1], 1.06, 0.1), 1.1);
    let app = app();
    let comp = app.session.active_comp().unwrap().clone();
    let c = snap_candidates(&comp, &build_rows(&app, &comp));
    assert!(c.contains(&comp.duration.seconds()) && c.contains(&0.0));
}

/// A dragged layer's own In / Out points are no snap targets; moved, neither are its markers
/// (#1828).
#[test]
fn bar_drags_dont_snap_to_their_own_layer() {
    let mut app = app();
    let bx = app.session.active_comp().unwrap().layers.iter().find(|l| l.name == "Box").unwrap().id.0;
    app.session.execute("layer.addMarker", json!({"layers": [bx], "time": 1.0})).unwrap();
    let comp = app.session.active_comp().unwrap().clone();
    let rows = build_rows(&app, &comp);
    let all = snap_targets(&comp, &rows, &[], false);
    let trimmed = snap_targets(&comp, &rows, &[bx], false);
    let moved = snap_targets(&comp, &rows, &[bx], true);
    assert_eq!(trimmed.len(), all.len() - 2, "trimming drops only the layer's In and Out");
    assert!(trimmed.contains(&1.0) && !moved.contains(&1.0), "moving drops the layer's marker: {moved:?}");
}

/// Shift snaps a bar drag's nearest end to a target within 8 px of the pointer; farther away, or
/// without Shift, the bar moves by the whole frames the pointer covered (#1828).
#[test]
fn bar_drags_snap_their_ends_with_shift() {
    let (pps, fd, ends) = (100.0, 0.1, [2.0, 6.0]);
    assert_eq!(bar_drag_frames(95.0, pps, fd, &ends, &[3.0], true), 10, "5 px short of 2 → 3 s snaps");
    assert_eq!(bar_drag_frames(95.0, pps, fd, &ends, &[3.0], false), 9, "no snap without Shift");
    assert_eq!(bar_drag_frames(88.0, pps, fd, &ends, &[3.0], true), 8, "12 px away lets go");
    assert_eq!(bar_drag_frames(104.0, pps, fd, &ends, &[3.0, 7.06], true), 11, "the target nearest the pointer wins");
    assert_eq!(bar_drag_frames(5.0, 0.0, 0.0, &ends, &[0.0], true), i64::MAX, "zero zoom saturates, no panic");
}

/// Every string drawn by one frame of `draw`.
fn drawn_text(mut draw: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    ctx.run_ui(Default::default(), |ui| draw(ui)).textures_delta.clear();
    let mut frame = ctx.run_ui(Default::default(), |ui| draw(ui));
    frame.textures_delta.clear();
    let mut out = Vec::new();
    frame.shapes.iter().for_each(|c| collect(&c.shape, &mut out));
    out
}

/// #1308: the layer context menu draws its items in the interface language, from the menu
/// bar's rows for the same commands (whose ellipsis is `...`), and its Effects submenu draws
/// the categories the Effect menu does; the actions it records stay the same.
#[test]
fn the_layer_context_menu_draws_in_the_interface_language() {
    let caps = LayerMenuCaps { vector: true, text: false, source: true };
    let (mut auto, mut actions) = (crate::automation::Registry::default(), Vec::new());
    crate::i18n::set_current("zh-hans");
    let menu = drawn_text(|ui| layer_menu(ui, &[7], caps, (&mut auto, "t"), &mut actions));
    let effects = drawn_text(|ui| effects_submenu(ui, &[7], &mut |_: &mut egui::Ui, _: &str, _: &str, _: serde_json::Value| {}));
    crate::i18n::set_current("en");
    let english = drawn_text(|ui| layer_menu(ui, &[7], caps, (&mut auto, "t"), &mut actions));
    for label in ["新建", "图层设置...", "在项目中显示图层源", "从矢量图层创建形状", "预合成...", "重复", "拆分图层", "删除"]
    {
        assert!(menu.iter().any(|t| t == label), "{label} missing from {menu:?}");
    }
    assert!(!menu.iter().any(|t| t.is_ascii()), "English left in {menu:?}");
    assert!(effects.iter().any(|t| t == "模糊和锐化"), "{effects:?}");
    assert!(!effects.iter().any(|t| t == "Blur & Sharpen"), "{effects:?}");
    for label in ["Layer Settings…", "Reveal Layer Source in Project", "Duplicate", "Delete"] {
        assert!(english.iter().any(|t| t == label), "{label} missing from {english:?}");
    }
    assert!(actions.is_empty());
}

/// The property context menu draws its items in the interface language too: the commands the
/// menu bar has from its rows, Reset, Edit Value… and Remove Expression from the panel catalog.
/// The automation labels stay English.
#[test]
fn the_property_context_menu_draws_in_the_interface_language() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 64, "duration": 4})).unwrap();
    let l = s.execute("layer.newSolid", json!({"name": "Plate", "color": "#406080"})).unwrap()["layer"].as_u64().unwrap();
    let opacity = s.active_comp().unwrap().layer(LayerId(l)).unwrap().props.prop("transform/opacity").unwrap().uid;
    s.execute("prop.setExpression", json!({"layer": l, "prop": opacity})).unwrap();
    let comp = s.active_comp_id().unwrap();
    let layer = s.active_comp().unwrap().layer(LayerId(l)).unwrap().clone();
    let (position, opacity) = (layer.props.prop("transform/position").unwrap(), layer.props.prop("transform/opacity").unwrap());
    let (mut auto, mut actions) = (crate::automation::Registry::default(), Vec::new());
    let mut menu = |language| {
        crate::i18n::set_current(language);
        let mut drawn = drawn_text(|ui| prop_menu(ui, &mut auto, comp, &layer, position, &mut actions));
        drawn.extend(drawn_text(|ui| prop_menu(ui, &mut auto, comp, &layer, opacity, &mut actions)));
        drawn
    };
    let chinese = menu("zh-hans");
    let english = menu("en");
    for label in ["重置", "编辑值…", "单独尺寸", "添加表达式", "移除表达式", "向基本图形添加属性"] {
        assert!(chinese.iter().any(|t| t == label), "{label} missing from {chinese:?}");
    }
    assert!(!chinese.iter().any(|t| t.is_ascii()), "English left in {chinese:?}");
    for label in ["Reset", "Edit Value…", "Separate Dimensions", "Remove Expression", "Add Property to Essential Graphics"] {
        assert!(english.iter().any(|t| t == label), "{label} missing from {english:?}");
    }
    assert!(auto.elements.iter().any(|e| e.label == "Remove Expression"), "the automation label stays English");
    assert!(actions.is_empty());
}

/// UI fidelity (M13.5): compact After Effects-like rows and muted label-coloured bars.
#[test]
fn rows_and_bars_follow_after_effects_proportions() {
    let t = crate::theme::Tokens::for_kind(crate::theme::ThemeKind::Dark);
    assert_eq!(t.row_h, 19.0);
    let red = t.label(effectcraft_engine::color::Label::Red);
    let bar = bar_color(red, false);
    // Muted: darker and less saturated than the label, still reddish.
    assert!(bar.r() < red.r() && bar.r() > bar.g() + 30 && bar.g() >= red.g().min(0x3a) - 1);
    let sel = bar_color(red, true);
    assert!(sel.r() > bar.r());
}

#[test]
fn disk_bar_range_covers_partial_frames() {
    let clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(2.0, 1.0));
    let tm = TMap { x0: 0.0, start: 10.25, pps: 1.0 };
    assert_eq!(disk_bar_range(&tm, clip, 1.0, 100), 10..13);

    let exact = TMap { start: 10.0, ..tm };
    assert_eq!(disk_bar_range(&exact, clip, 1.0, 100), 10..12);
    let off_comp = TMap { start: 200.0, ..tm };
    assert_eq!(disk_bar_range(&off_comp, clip, 1.0, 100), 100..100);
    assert_eq!(disk_bar_range(&tm, clip, f64::NAN, 100), 0..0);
    assert_eq!(disk_bar_range(&TMap { pps: 0.0, ..tm }, clip, 1.0, 100), 0..0);
    assert_eq!(disk_bar_range(&TMap { start: f64::NAN, ..tm }, clip, 1.0, 100), 0..0);
    assert_eq!(disk_bar_range(&tm, Rect::from_min_max(pos2(2.0, 0.0), pos2(2.0, 1.0)), 1.0, 100), 0..0);
    let above_limit = TMap { start: 100_001.0, ..tm };
    assert_eq!(disk_bar_range(&above_limit, clip, 1.0, i64::MAX), 100_000..100_000);
}

/// The keyframe menu is one for the Timeline and the Graph Editor: its commands are drawn in the
/// interface language, and a locked layer's keys can be copied and selected but not edited.
#[test]
fn the_key_menu_is_translated_and_cannot_edit_a_locked_layers_keys() {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "duration": 2})).unwrap();
    let id = s.execute("layer.newSolid", json!({"name": "Box", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(0.0, 0), (1.0, 50)] {
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    let app = EffectcraftApp::new(s);
    let layer = app.session.active_comp().unwrap().layer(LayerId(id)).unwrap().clone();
    let prop = layer.props.prop("transform/opacity").unwrap().clone();
    let key = prop.keys[0].clone();
    let enabled = |locked: bool, selected: bool, label: &str| {
        let mut layer = layer.clone();
        layer.switches.locked = locked;
        let mut actions = vec![];
        let mut h = Harness::new_ui(|ui| key_menu(ui, &app, &layer, &prop, &key, selected, &mut actions));
        h.run();
        !h.get_by_label(label).accesskit_node().is_disabled()
    };
    for label in [
        "Copy",
        "Select Equal Keyframes",
        "Select All Keyframes",
        "Delete",
        "Edit Value…",
        "Easy Ease",
        "Time-Reverse Keyframes",
        "Keyframe Velocity…",
        "Mute Keyframes",
    ] {
        assert!(enabled(false, true, label), "{label} on an unlocked layer");
    }
    for label in ["Copy", "Select Equal Keyframes", "Select All Keyframes"] {
        assert!(enabled(true, true, label), "{label} is still available on a locked layer");
    }
    for label in ["Delete", "Edit Value…", "Easy Ease", "Time-Reverse Keyframes", "Keyframe Velocity…", "Mute Keyframes", "Linear"] {
        assert!(!enabled(true, true, label), "{label} is off on a locked layer");
        assert!(!enabled(true, false, label), "{label} is off on an unselected locked key");
    }
    for label in ["Copy", "Select Equal Keyframes", "Select Previous Keyframes", "Select Following Keyframes"] {
        assert!(!enabled(true, false, label), "{label} requires selecting the clicked key");
    }
    assert!(enabled(true, false, "Select All Keyframes"));
    // The labels follow the interface language.
    crate::i18n::set_current("es");
    let copy = crate::i18n::context_item("keys.copy", &json!({}), "Copy");
    assert_ne!(copy, "Copy", "the catalog translates Copy");
    assert!(enabled(false, true, &copy));
    crate::i18n::set_current("en");
}

#[test]
fn a_key_menu_edit_selects_the_clicked_key_before_applying_the_command() {
    use egui_kittest::{Harness, kittest::Queryable};
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 64, "duration": 2})).unwrap();
    let id = s.execute("layer.newSolid", json!({"color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    for (time, value) in [(0.0, 0), (1.0, 50)] {
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": time, "value": value})).unwrap();
    }
    let layer = s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone();
    let prop = layer.props.prop("transform/opacity").unwrap().clone();
    s.execute("keys.select", json!({"keys": [{"layer": id, "prop": prop.uid, "time": 1.0}]})).unwrap();
    let mut app = EffectcraftApp::new(s);
    let mut actions = vec![];
    {
        let mut h = Harness::new_ui(|ui| key_menu(ui, &app, &layer, &prop, &prop.keys[0], false, &mut actions));
        h.run();
        h.get_by_label("Easy Ease").click();
        h.run();
    }
    assert_eq!(actions.iter().map(|(command, _)| command.as_str()).collect::<Vec<_>>(), ["keys.select", "keys.easyEase"]);
    for (command, params) in actions {
        app.session.execute(&command, params).unwrap();
    }
    let keys = &app.session.active_comp().unwrap().layer(LayerId(id)).unwrap().props.find(prop.uid).unwrap().keys;
    assert_ne!(keys[0], prop.keys[0], "the clicked key was eased");
    assert_eq!(keys[1], prop.keys[1], "the previously selected key was not edited");
    assert_eq!(app.session.state.selected_keys.len(), 1);
    assert_eq!(app.session.state.selected_keys[0].time, prop.keys[0].time);
}

/// A key's marquee target is the marker it draws: a roving key's is its smaller dot.
#[test]
fn a_keys_glyph_rect_is_what_it_draws() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "duration": 2})).unwrap();
    let id = s.execute("layer.newSolid", json!({"name": "Box", "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": 0.5, "value": 0})).unwrap();
    let comp = s.active_comp().unwrap();
    let mut k = comp.layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().keys[0].clone();
    let c = pos2(100.0, 50.0);
    k.roving = true;
    assert_eq!(key_glyph_rect(&k, c), Rect::from_center_size(c, vec2(5.0, 5.0)));
}

/// Keys come back in row order, and only the rows and keys the box touches. Rows taller than one
/// line (an expression editor) push the keys below them down, as they are drawn.
#[test]
fn keys_in_box_walks_the_rows_and_tests_each_glyph() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "duration": 4, "frameRate": 30})).unwrap();
    let mut layers = vec![];
    for name in ["A", "B"] {
        let id = s.execute("layer.newSolid", json!({"name": name, "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
        for t in [1.0, 2.0] {
            s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": t, "value": 50})).unwrap();
        }
        layers.push(id);
    }
    let comp = s.active_comp().unwrap();
    let uid = |id: u64| comp.layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let row = |id: u64, kind: RowKind| Row { layer: LayerId(id), depth: 1, kind };
    let rows = vec![
        row(layers[1], RowKind::Prop { uid: uid(layers[1]) }),
        row(layers[1], RowKind::Expr { uid: uid(layers[1]), lines: 3 }),
        row(layers[0], RowKind::Prop { uid: uid(layers[0]) }),
    ];
    // 1 s = 100 pt. Rows are 20 pt: centers at y 10 and, past the 60 pt editor, 90.
    let tm = TMap { x0: 0.0, start: 0.0, pps: 100.0 };
    let hit = |br: Rect| -> Vec<(u64, i64)> { keys_in_box(comp, &rows, &tm, 0.0, 20.0, br).iter().map(|k| (k.layer.0, k.time.0)).collect() };
    let (one, two) = (Tick::from_seconds_f64(1.0).0, Tick::from_seconds_f64(2.0).0);
    let across = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, 0.0), pos2(x1, 200.0));
    assert_eq!(hit(across(0.0, 300.0)), [(layers[1], one), (layers[1], two), (layers[0], one), (layers[0], two)], "row order");
    assert_eq!(hit(Rect::from_min_max(pos2(0.0, 95.5), pos2(300.0, 200.0))), [(layers[0], one), (layers[0], two)], "the box reaches the glyph's bottom point");
    assert!(hit(Rect::from_min_max(pos2(0.0, 97.0), pos2(300.0, 200.0))).is_empty(), "the box is below the glyph");
}

#[test]
fn layer_drop_target_skips_moving_layers_beyond_the_viewport() {
    let mut rows: Vec<Row> = (1..=6).map(|id| Row { layer: LayerId(id), depth: 0, kind: RowKind::Layer }).collect();
    rows.insert(4, Row { layer: LayerId(4), depth: 1, kind: RowKind::Prop { uid: 0 } });
    let visible: Vec<_> = (2..=4).map(|id| (Rect::NOTHING, LayerId(id))).collect();
    for at in [Some(2), None] {
        assert_eq!(layer_drop_target(&rows, &visible, at, &[1, 4, 5]), Some(LayerId(6)), "skip moving headers and their properties across the viewport edge");
        assert_eq!(layer_drop_target(&rows, &visible, at, &[1, 4, 5, 6]), None, "the stack ends when no stationary layer follows");
    }
}

fn mask_reveal_app() -> (EffectcraftApp, u64) {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 36})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Plate"})).unwrap()["layer"].as_u64().unwrap();
    s.drain_events();
    (EffectcraftApp::new(s), layer)
}

fn mask_reveal_add(app: &mut EffectcraftApp) -> u64 {
    let mask = app.session.execute("layer.addMask", json!({})).unwrap()["mask"].as_u64().unwrap();
    app.handle_events(&egui::Context::default());
    mask
}

/// Whole-mask rows and shortcut-selected properties must coexist without rendering controls twice.
#[test]
fn mask_reveal_opens_controls_and_preserves_other_properties_without_duplicates() {
    for kinds in [vec![], vec!["position"], vec!["feather"], vec!["masks"], vec!["masks", "feather"]] {
        let (mut app, layer) = mask_reveal_app();
        app.ui.timeline.reveal = vec!["rotation".into()];
        if !kinds.is_empty() {
            app.ui.timeline.layer_reveal.insert(layer, kinds.iter().map(|s| s.to_string()).collect());
        }
        mask_reveal_add(&mut app);
        mask_reveal_add(&mut app);
        let names = labels(&app);
        for name in [">Mask 1", ">Mask 2"] {
            assert_eq!(names.iter().filter(|r| r.trim() == name).count(), 1, "{kinds:?}: {names:?}");
        }
        assert_eq!(names.iter().filter(|r| r.trim() == "Mask Feather").count(), 2, "{names:?}");
        assert_eq!(names.iter().filter(|r| r.trim() == "Mask Path").count(), 2, "{names:?}");
        if kinds.contains(&"position") {
            assert_eq!(names.iter().filter(|r| r.trim() == "Position").count(), 1, "{names:?}");
        }
        assert_eq!(app.ui.timeline.reveal, ["rotation"]);
    }
}

/// Revealing controls is a presentation change; explicit search and the active drawing selection retain authority.
#[test]
fn mask_reveal_preserves_search_selection_focus_document_and_revision() {
    let (mut app, layer) = mask_reveal_app();
    app.ui.timeline.search = "position".into();
    app.ui.timeline.open_layers.insert(999);
    app.ui.timeline.layer_reveal.insert(999, vec!["scale".into()]);
    app.ui.focused = crate::dock::PanelKind::Composition;
    let selected = app.session.state.selected_layers.clone();
    let mask = app.session.execute("mask.new", json!({"vertices": [[2, 3]]})).unwrap()["mask"].as_u64().unwrap();
    let vertices = app.session.state.selected_vertices.clone();
    let before = app.session.project.to_json();
    let revision = app.session.revision;
    app.handle_events(&egui::Context::default());
    assert_eq!(app.session.project.to_json(), before);
    assert_eq!(app.session.revision, revision);
    assert_eq!(app.session.state.selected_layers, selected);
    assert_eq!(app.session.state.selected_vertices, vertices);
    assert_eq!(app.ui.focused, crate::dock::PanelKind::Composition);
    assert_eq!(app.ui.timeline.search, "position");
    assert_eq!(app.ui.timeline.layer_reveal.get(&999).unwrap(), &["scale"]);
    assert!(app.ui.timeline.open_layers.contains(&999));
    assert!(!labels(&app).iter().any(|r| r.trim() == ">Mask 1"));
    app.ui.timeline.search.clear();
    assert!(labels(&app).iter().any(|r| r.trim() == ">Mask 1"));
    assert!(app.ui.timeline.open_groups.contains(&mask));
    assert!(app.ui.timeline.open_layers.contains(&layer));
}

/// Queued events can outlive their target or project, so stale delivery must leave Timeline presentation untouched.
#[test]
fn mask_reveal_ignores_undone_deleted_inactive_and_replaced_project_events() {
    #[derive(Clone, Copy, Debug)]
    enum Stale {
        Undo,
        Delete,
        Inactive,
        Replace,
    }
    for stale in [Stale::Undo, Stale::Delete, Stale::Inactive, Stale::Replace] {
        let (mut app, layer) = mask_reveal_app();
        let mask = app.session.execute("layer.addMask", json!({})).unwrap()["mask"].as_u64().unwrap();
        match stale {
            Stale::Undo => {
                assert!(app.session.undo());
            }
            Stale::Delete => {
                app.session.execute("mask.remove", json!({"mask": mask})).unwrap();
            }
            Stale::Inactive => {
                app.session.execute("comp.new", json!({"name": "Other"})).unwrap();
            }
            Stale::Replace => {
                let project = (*app.session.project).clone();
                let comp = app.session.active_comp_id().unwrap();
                app.session.replace_project(project, None);
                app.session.execute("comp.open", json!({"comp": comp.0})).unwrap();
            }
        }
        let before = serde_json::to_value(&app.ui.timeline).unwrap();
        app.handle_events(&egui::Context::default());
        // Opening a composition can update its zoom, but must not reveal the stale mask.
        assert!(app.ui.timeline.open_layers.is_empty(), "{stale:?}");
        assert!(app.ui.timeline.open_groups.is_empty(), "{stale:?}");
        assert!(app.ui.timeline.layer_reveal.is_empty(), "{stale:?}");
        if !matches!(stale, Stale::Inactive | Stale::Replace) {
            assert_eq!(serde_json::to_value(&app.ui.timeline).unwrap(), before);
        }
        assert!(!app.ui.timeline.open_layers.contains(&layer));
    }
}

/// Adding an unkeyed mask must retain the visible Position key and restore its controls on redo.
#[test]
fn mask_reveal_undo_redo_restores_visible_controls_and_keeps_keyframe_scope() {
    let (mut app, layer) = mask_reveal_app();
    app.session.execute("prop.toggleAnimation", json!({"path": "transform/position"})).unwrap();
    app.ui.timeline.layer_reveal.insert(layer, vec!["position".into()]);
    mask_reveal_add(&mut app);
    let visible = visible_props(&app, app.session.active_comp().unwrap());
    assert_eq!(app.session.execute("keys.selectAll", json!({"visible": visible})).unwrap(), json!(1));
    assert!(app.session.undo());
    app.handle_events(&egui::Context::default());
    assert!(!labels(&app).iter().any(|r| r.trim() == ">Mask 1"));
    assert!(app.session.redo());
    app.handle_events(&egui::Context::default());
    assert!(labels(&app).iter().any(|r| r.trim() == ">Mask 1"));
    assert!(labels(&app).iter().any(|r| r.trim() == "Position"));
}

/// Repeated delivery must converge to the same presentation, while malformed external arguments fail without mutation.
#[test]
fn mask_reveal_validates_arguments_and_repeated_delivery_is_idempotent() {
    let (mut app, layer) = mask_reveal_app();
    let mask = mask_reveal_add(&mut app);
    let comp = app.session.active_comp_id().unwrap().0;
    let epoch = app.session.script_ui.project_epoch;
    let params = json!({"comp": comp, "layer": layer, "mask": mask, "projectEpoch": epoch});
    let ctx = egui::Context::default();
    let before = serde_json::to_value(&app.ui.timeline).unwrap();
    for _ in 0..2 {
        assert_eq!(crate::menus::frontend(&mut app, &ctx, "timeline.revealMask", params.clone()).unwrap(), json!({"revealed": true}));
        assert_eq!(serde_json::to_value(&app.ui.timeline).unwrap(), before);
    }
    for bad in [json!({}), json!({"comp": -1, "layer": layer, "mask": mask, "projectEpoch": epoch})] {
        assert!(crate::menus::frontend(&mut app, &ctx, "timeline.revealMask", bad).is_err());
        assert_eq!(serde_json::to_value(&app.ui.timeline).unwrap(), before);
    }
}

/// An ID reused by paste or another history branch must not inherit the removed mask's pending reveal.
#[test]
fn mask_reveal_does_not_follow_reused_ids_after_undo_or_history_jump() {
    for route in ["undo", "ancestor", "branch"] {
        let (mut app, layer) = mask_reveal_app();
        let source = mask_reveal_add(&mut app);
        app.ui.timeline = Default::default();
        app.session.state.selected_props = vec![(LayerId(layer), source)];
        app.session.execute("edit.copy", json!({})).unwrap();
        assert_eq!(app.session.state.mask_clipboard.len(), 1);
        let ancestor = app.session.history_tree().into_iter().find(|n| n.current).unwrap().id;
        let branch = if route == "branch" {
            app.session.execute("edit.paste", json!({"layers": [layer]})).unwrap();
            let id = app.session.history_tree().into_iter().find(|n| n.current).unwrap().id;
            assert!(app.session.undo());
            Some(id)
        } else {
            None
        };
        // Folder allocation aligns the creation UID with paste's increment-first allocator.
        app.session.execute("project.newFolder", json!({})).unwrap();
        let removed = app.session.execute("layer.addMask", json!({"layer": layer})).unwrap()["mask"].as_u64().unwrap();
        app.session.events.push(effectcraft_engine::Event::Toast { message: "keep unrelated events".into(), error: false });
        if route == "undo" {
            assert!(app.session.undo());
            assert!(app.session.undo());
        } else {
            assert!(app.session.goto_history(branch.as_deref().unwrap_or(&ancestor)));
        }
        let replacement = if route == "branch" {
            app.session.active_comp().unwrap().layer(LayerId(layer)).unwrap().masks().unwrap().groups().last().unwrap().uid
        } else {
            app.session.execute("edit.paste", json!({"layers": [layer]})).unwrap()["masks"][0].as_u64().unwrap()
        };
        assert_eq!(removed, replacement, "this regression must actually reuse the UID");
        app.handle_events(&egui::Context::default());
        assert!(app.ui.timeline.open_layers.is_empty(), "{route}");
        assert!(app.ui.timeline.open_groups.is_empty(), "{route}");
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("keep unrelated events"));
    }
}

/// History sanitation must retain notifications for masks that survive restoration, even after later edits.
#[test]
fn mask_reveal_keeps_valid_queued_creations_after_an_unrelated_undo() {
    let (mut app, layer) = mask_reveal_app();
    for _ in 0..2 {
        app.session.execute("layer.addMask", json!({})).unwrap();
    }
    app.session.execute("prop.set", json!({"path": "transform/opacity", "value": 50})).unwrap();
    assert!(app.session.undo());
    app.handle_events(&egui::Context::default());
    let names = labels(&app);
    assert!(app.ui.timeline.open_layers.contains(&layer));
    for name in [">Mask 1", ">Mask 2"] {
        assert_eq!(names.iter().filter(|r| r.trim() == name).count(), 1, "{names:?}");
    }
}

/// Collapsing a whole-mask row must not leave duplicate flat properties visible through another reveal filter.
#[test]
fn mask_reveal_collapsing_a_mask_hides_its_children_under_mixed_filters() {
    let (mut app, layer) = mask_reveal_app();
    app.ui.timeline.layer_reveal.insert(layer, vec!["masks".into(), "feather".into()]);
    let mask = mask_reveal_add(&mut app);
    app.ui.timeline.open_groups.remove(&mask);
    let names = labels(&app);
    assert_eq!(names.iter().filter(|r| r.trim() == ">Mask 1").count(), 1);
    assert!(!names.iter().any(|r| r.trim() == "Mask Feather"));
    app.ui.timeline.open_groups.insert(mask);
    assert_eq!(labels(&app).iter().filter(|r| r.trim() == "Mask Feather").count(), 1);
}

/// Every icon-only column header says what it is on hover, over the icon it explains, and every
/// layer switch under one shows the same text.
#[test]
fn every_icon_only_header_has_a_hover_text() {
    let all = crate::state::TimelineState { columns: COLUMNS.iter().map(|c| c.0.to_string()).collect(), show_modes: true, ..Default::default() };
    let cw = cols(0.0, 2000.0, Vis::of(&all));
    let ch = Rect::from_min_max(pos2(0.0, 0.0), pos2(2000.0, 20.0));
    let rects = header_tip_rects(&cw, ch);
    assert_eq!(rects.len(), HEADER_TIPS.len(), "every header icon is placed with every column shown");
    for (col, key, r) in &rects {
        assert_eq!(column_at(&cw, r.center().x), Some(*col), "{key} sits in its column");
        let (_, _, name, tip) = HEADER_TIPS.iter().find(|t| t.1 == *key).unwrap();
        assert!(tip.starts_with(&format!("{name}:")) || *key == "num", "{key}: {tip}");
    }
    for (i, (_, a, ra)) in rects.iter().enumerate() {
        for (_, b, rb) in &rects[i + 1..] {
            assert!(!ra.intersects(*rb) || ra.intersect(*rb).area() < 1.0, "{a} and {b} overlap");
        }
    }
    // The icons the header draws over the A/V and switch columns, left to right.
    let av: Vec<&str> = rects.iter().filter(|r| r.0 == "av").map(|r| r.1).collect();
    assert_eq!(av, ["video", "audio", "solo", "lock"]);
    let sw: Vec<&str> = rects.iter().filter(|r| r.0 == "switches").map(|r| r.1).collect();
    assert_eq!(sw, ["shy", "collapse", "quality", "fx", "frameBlend", "motionBlur", "adjustment", "threeD"]);
    for name in av.iter().chain(&sw).chain(&["preserveTransparency"]) {
        assert!(switch_tip(name).is_some(), "the {name} switch has a hover text");
    }
    // A hidden column has no hover targets.
    let none = crate::state::TimelineState { columns: Default::default(), show_modes: false, ..Default::default() };
    assert!(header_tip_rects(&cols(0.0, 800.0, Vis::of(&none)), ch).is_empty());
}

/// One box spans the outline and the time graph (#3056): rows it crosses are selected on both
/// sides of the boundary, and only keys in the time graph count (the part over the outline is cut).
#[test]
fn a_box_over_both_columns_selects_the_rows_it_crosses_and_the_keys_inside() {
    let mut s = effectcraft_engine::Session::default();
    s.execute("comp.new", json!({"name": "C", "width": 64, "height": 64, "duration": 4, "frameRate": 30})).unwrap();
    let mut layers = vec![];
    for name in ["A", "B", "C"] {
        let id = s.execute("layer.newSolid", json!({"name": name, "color": "#fff"})).unwrap()["layer"].as_u64().unwrap();
        s.execute("prop.addKey", json!({"layer": id, "path": "transform/opacity", "time": 0.0, "value": 50})).unwrap();
        layers.push(id);
    }
    s.execute("layer.setSwitch", json!({"layers": [layers[2]], "switch": "lock", "value": true})).unwrap();
    let comp = s.active_comp().unwrap();
    let uid = |id: u64| comp.layer(LayerId(id)).unwrap().props.prop("transform/opacity").unwrap().uid;
    let mut rows = vec![];
    for id in &layers {
        rows.push(Row { layer: LayerId(*id), depth: 0, kind: RowKind::Layer });
        rows.push(Row { layer: LayerId(*id), depth: 1, kind: RowKind::Prop { uid: uid(*id) } });
    }
    // Rows are 20 pt and 400 pt wide: the outline is left of x 200, a key at 0 s sits at x 205.
    let tm = TMap { x0: 205.0, start: 0.0, pps: 100.0 };
    let view = marquee::View { tm, top: 0.0, graph_x0: 200.0, graph_on: false };
    // Press at `from` with the rows scrolled by `s0`, release at `to` scrolled by `s1`.
    let drag = |from: Pos2, s0: f32, to: Pos2, s1: f32| {
        let (paint, sel) = marquee::Anchor::press(&view, s0, from).selection(&view, s1, to);
        marquee::targets(comp, &rows, &tm, -s1, 20.0, paint, &sel)
    };
    let go = |from: Pos2, to: Pos2| drag(from, 0.0, to, 0.0);
    let keyed = |ks: &[effectcraft_engine::KeyRef]| ks.iter().map(|k| k.layer.0).collect::<Vec<_>>();
    // From the outline across the boundary: layer A's row (y 0..20) and B's row, and their keys.
    let (ls, ks) = go(pos2(10.0, 5.0), pos2(260.0, 65.0));
    assert_eq!(ls, [layers[0], layers[1]]);
    assert_eq!(keyed(&ks), [layers[0], layers[1]]);
    // The same box dragged the other way: the same rows, the same keys.
    let (ls, ks) = go(pos2(260.0, 65.0), pos2(10.0, 5.0));
    assert_eq!((ls, keyed(&ks)), (vec![layers[0], layers[1]], vec![layers[0], layers[1]]));
    // Only over the outline: rows, no keys; only over the graph: keys, and the rows crossed.
    let (ls, ks) = go(pos2(10.0, 5.0), pos2(150.0, 65.0));
    assert_eq!((ls.len(), ks.len()), (2, 0), "a key under the outline isn't touched");
    // Kept inside the time graph: keys only, no rows (a marquee over empty track area).
    let (ls, ks) = go(pos2(204.0, 5.0), pos2(260.0, 65.0));
    assert_eq!((ls.len(), keyed(&ks)), (0, vec![layers[0], layers[1]]));
    // A locked layer is not selected as a row (the engine drops its keys in keys.select).
    let (ls, _) = go(pos2(10.0, 5.0), pos2(260.0, 119.0));
    assert_eq!(ls, [layers[0], layers[1]]);
    // Scrolled 20 pt mid-drag: the pressed corner stays on layer A's row (#2859), so A is still
    // selected; a screen-fixed anchor would start the box below A and select only B.
    let (ls, _) = drag(pos2(10.0, 5.0), 0.0, pos2(260.0, 45.0), 20.0);
    assert_eq!(ls, [layers[0], layers[1]]);
}
