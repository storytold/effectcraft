//! Composition viewer parity: snapping maths, channel / exposure display transforms, snapshots,
//! guides, the shape Pen, vertex add / delete / convert, motion-path key and tangent edits and
//! the Graph Editor transform box. Every edit undoes.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::LayerId;
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::viewer::{Channel, PRI_GRID, PRI_GUIDE, PRI_LAYER, PRI_VERTEX, SnapKind, SnapSource, SnapTarget, display_transform, snap};

fn comp() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

// ---------------------------------------------------------------- snapping

#[test]
fn snap_picks_nearest_target_within_tolerance() {
    let t = [SnapTarget::point([100.0, 100.0], PRI_LAYER, SnapSource::Comp), SnapTarget::point([108.0, 100.0], PRI_LAYER, SnapSource::Comp)];
    let s = snap(&[[103.0, 101.0]], &t, 10.0).unwrap();
    assert_eq!(s.delta, [-3.0, -1.0]);
    assert_eq!(s.at, [100.0, 100.0]);
    // Out of tolerance: nothing.
    assert!(snap(&[[130.0, 100.0]], &t, 10.0).is_none());
    // Of several sources, the one nearest a target snaps.
    let s = snap(&[[0.0, 0.0], [107.0, 100.0]], &t, 10.0).unwrap();
    assert_eq!(s.delta, [1.0, 0.0]);
}

#[test]
fn snap_lines_snap_axes_independently_and_priority_breaks_ties() {
    let t = [
        SnapTarget::vline(50.0, PRI_GRID, SnapSource::Grid),
        SnapTarget::vline(54.0, PRI_GUIDE, SnapSource::Guide),
        SnapTarget::hline(20.0, PRI_GRID, SnapSource::Grid),
    ];
    // x snaps to the nearest vertical line (equal distance: the guide wins over the grid), y to
    // the horizontal one.
    let s = snap(&[[52.0, 23.0]], &t, 5.0).unwrap();
    assert_eq!(s.delta, [2.0, -3.0]);
    assert_eq!(s.hits.len(), 2);
    assert_eq!(s.hits[0].source, SnapSource::Guide);
    assert_eq!(s.at, [54.0, 20.0]);
    // A point at the same distance as a line beats it (it snaps both axes).
    let t2 = [SnapTarget::vline(10.0, PRI_GUIDE, SnapSource::Guide), SnapTarget::point([10.0, 12.0], PRI_VERTEX, SnapSource::Vertex(LayerId(1)))];
    let s = snap(&[[12.0, 12.0]], &t2, 5.0).unwrap();
    assert_eq!(s.hits[0].kind, SnapKind::Point);
    assert_eq!(s.delta, [-2.0, 0.0]);
    // Equal point distances: lower priority value (vertex) wins over a layer feature.
    let t3 =
        [SnapTarget::point([0.0, 3.0], PRI_LAYER, SnapSource::Layer(LayerId(2))), SnapTarget::point([3.0, 0.0], PRI_VERTEX, SnapSource::Vertex(LayerId(3)))];
    assert_eq!(snap(&[[0.0, 0.0]], &t3, 5.0).unwrap().hits[0].source, SnapSource::Vertex(LayerId(3)));
}

#[test]
fn snap_targets_cover_layers_comp_guides_and_grid() {
    let mut s = comp();
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 100, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("view.addGuide", json!({"orientation": "vertical", "position": 77})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap().clone();
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &comp, Tick::ZERO);
    let opts = crate::viewer::SnapOptions { layers: true, features: Default::default(), guides: true, grid: true, grid_spacing: 100.0 };
    let t = crate::viewer::targets(&ctx, &[LayerId(b)], opts);
    // Layer a's corner (270, 155) is a target; layer b (dragged) is not.
    assert!(t.iter().any(|x| x.source == SnapSource::Layer(LayerId(a)) && x.kind == SnapKind::Point && x.pos == [270.0, 155.0]));
    assert!(!t.iter().any(|x| x.source == SnapSource::Layer(LayerId(b))));
    assert!(t.iter().any(|x| x.source == SnapSource::Guide && x.pos[0] == 77.0));
    assert!(t.iter().any(|x| x.source == SnapSource::Grid && x.kind == SnapKind::HLine && x.pos[1] == 300.0));
    assert!(t.iter().any(|x| x.source == SnapSource::Comp && x.kind == SnapKind::Point && x.pos == [320.0, 180.0]));
    // Dragging a corner near layer a's corner lands on it (edges snap x and y).
    let hit = snap(&[[272.0, 157.0]], &t, 6.0).unwrap();
    assert_eq!(hit.at, [270.0, 155.0]);
    assert_eq!(hit.delta, [-2.0, -2.0]);
}

/// Tools bar ▸ Snapping options (#253): Snap Edges Extended off keeps a layer's edges to its
/// extent, every feature of a layer snaps, hidden layers are no targets, and with Snapping off
/// only guides and the grid are left.
#[test]
fn snapping_options_pick_the_targets() {
    use crate::viewer::{SnapFeatures, SnapOptions, layer_features, targets};
    let mut s = comp();
    // A 100×50 solid at the comp centre: x 270–370, y 155–205.
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 100, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    let hidden = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 40, "height": 40})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": hidden, "path": "transform/position", "value": [50, 50, 0]})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [hidden], "switch": "video", "value": false})).unwrap();
    s.execute("view.addGuide", json!({"orientation": "vertical", "position": 77})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let comp = s.project.comp(cid).unwrap().clone();
    let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &comp, Tick::ZERO);
    let opts = |features: SnapFeatures| SnapOptions { layers: true, features, guides: false, grid: false, grid_spacing: 0.0 };
    let all = targets(&ctx, &[], opts(SnapFeatures::default()));
    assert!(!all.iter().any(|t| t.source == SnapSource::Layer(LayerId(hidden))), "hidden layers are no targets");
    // Extended: a point far below layer a snaps to its left edge's line.
    assert_eq!(snap(&[[268.0, 340.0]], &all, 4.0).unwrap().at, [270.0, 340.0]);
    // Not extended: only along the edge.
    let own = targets(&ctx, &[], opts(SnapFeatures { edges_extended: false, ..Default::default() }));
    assert!(snap(&[[268.0, 340.0]], &own, 4.0).is_none_or(|h| h.hits.iter().all(|t| t.source != SnapSource::Layer(LayerId(a)))));
    assert_eq!(snap(&[[268.0, 190.0]], &own, 4.0).unwrap().at, [270.0, 190.0]);
    // Corners, edge midpoints and the centre of a are targets.
    for p in [[270.0, 155.0], [320.0, 155.0], [320.0, 180.0]] {
        assert!(all.iter().any(|t| t.source == SnapSource::Layer(LayerId(a)) && t.pos == p), "{p:?}");
    }
    // The dragged layer's features: corners, edge midpoints, centre and anchor point.
    let l = comp.layer(LayerId(a)).unwrap();
    let f = layer_features(&ctx, l);
    assert_eq!(f[..4], [[270.0, 155.0], [370.0, 155.0], [370.0, 205.0], [270.0, 205.0]]);
    assert_eq!(f.len(), 10, "{f:?}");
    // Snapping off: guides only.
    let guides = targets(&ctx, &[], SnapOptions { layers: false, guides: true, ..opts(SnapFeatures::default()) });
    assert!(!guides.is_empty() && guides.iter().all(|t| t.source == SnapSource::Guide));
}

/// Snap to Features in Collapsed Compositions and Text Layers: the layers inside a collapsed
/// precomp layer are targets (highlighting the precomp layer), only while the option is on and
/// the precomp layer collapses transformations.
#[test]
fn collapsed_precomp_features_snap() {
    use crate::viewer::{SnapFeatures, SnapOptions, targets};
    let mut s = comp();
    // A 40×20 solid at (100, 50) in a 640×360 precomp.
    let inner = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 40, "height": 20})).unwrap()["layer"].as_u64().unwrap();
    s.execute("prop.set", json!({"layer": inner, "path": "transform/position", "value": [100, 50, 0]})).unwrap();
    s.execute("layer.precompose", json!({"layers": [inner], "name": "Inner"})).unwrap();
    let pre = s.active_comp().unwrap().layers[0].id;
    // Moved by (10, 20) in the outer comp: the inner solid's corner (80, 40) lands at (90, 60).
    s.execute("prop.set", json!({"layer": pre.0, "path": "transform/position", "value": [330, 200, 0]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let at = |s: &Session, f: SnapFeatures| {
        let comp = s.project.comp(cid).unwrap().clone();
        let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, &comp, Tick::ZERO);
        targets(&ctx, &[], SnapOptions { layers: true, features: f, guides: false, grid: false, grid_spacing: 0.0 })
    };
    let corner = |t: &[crate::viewer::SnapTarget]| t.iter().any(|t| t.kind == SnapKind::Point && t.pos == [90.0, 60.0] && t.source == SnapSource::Layer(pre));
    assert!(!corner(&at(&s, SnapFeatures::default())), "not collapsed");
    s.execute("layer.setSwitch", json!({"layers": [pre.0], "switch": "collapse", "value": true})).unwrap();
    let on = at(&s, SnapFeatures::default());
    assert!(corner(&on), "collapsed");
    assert_eq!(snap(&[[93.0, 63.0]], &on, 5.0).unwrap().at, [90.0, 60.0]);
    assert!(!corner(&at(&s, SnapFeatures { collapsed_features: false, ..Default::default() })), "option off");
}

#[test]
fn snapping_options_command_sets_and_toggles() {
    let mut s = comp();
    assert_eq!(s.state.snap_features, crate::viewer::SnapFeatures { edges_extended: true, collapsed_features: true });
    let r = s.execute("view.snappingOptions", json!({"edgesExtended": false})).unwrap();
    assert_eq!(r, json!({"edgesExtended": false, "collapsedFeatures": true}));
    s.execute("view.snappingOptions", json!({"toggle": "collapsedFeatures"})).unwrap();
    assert!(!s.state.snap_features.collapsed_features);
    s.execute("view.snappingOptions", json!({"toggle": "collapsedFeatures"})).unwrap();
    assert!(s.state.snap_features.collapsed_features);
    // The per-feature toggles are gone, as in After Effects' menu.
    for k in ["nope", "corners", "anchorPoints", "paths"] {
        assert!(s.execute("view.snappingOptions", json!({"toggle": k})).is_err(), "{k}");
    }
}

// ---------------------------------------------------------------- channels, exposure, snapshot

#[test]
fn channel_and_exposure_pixel_transforms() {
    // Premultiplied: straight (200, 100, 50) at 50% alpha.
    let px = [100u8, 50, 25, 128];
    let run = |c: Channel, col: bool, stops: f32| {
        let mut v = [px];
        display_transform(&mut v, c, col, stops);
        v[0]
    };
    assert_eq!(run(Channel::Rgb, false, 0.0), px);
    assert_eq!(run(Channel::Red, false, 0.0), [100, 100, 100, 255]);
    assert_eq!(run(Channel::Green, true, 0.0), [0, 50, 0, 255]);
    assert_eq!(run(Channel::Blue, false, 0.0), [25, 25, 25, 255]);
    assert_eq!(run(Channel::Alpha, false, 0.0), [128, 128, 128, 255]);
    let st = run(Channel::RgbStraight, false, 0.0);
    assert!((st[0] as i32 - 199).abs() <= 1 && (st[1] as i32 - 100).abs() <= 1 && st[3] == 255, "{st:?}");
    // +1 stop doubles linear light: sRGB 50% gray → about 69%; -1 stop darkens; 0 is identity.
    let mut g = [[128u8, 128, 128, 255]];
    display_transform(&mut g, Channel::Rgb, false, 1.0);
    assert!((174..=178).contains(&g[0][0]), "{:?}", g[0]);
    let mut g = [[128u8, 128, 128, 255]];
    display_transform(&mut g, Channel::Rgb, false, -1.0);
    assert!((92..=96).contains(&g[0][0]), "{:?}", g[0]);
    assert_eq!(crate::viewer::exposure_lut(0.0)[200], 200);
    // Exposure applies to a single channel too, and keeps alpha untouched in RGB.
    assert!(run(Channel::Red, false, 1.0)[0] > 100);
    assert_eq!(run(Channel::Rgb, false, 2.0)[3], 128);
}

#[test]
fn channel_exposure_and_fast_preview_commands() {
    let mut s = comp();
    s.execute("view.channel", json!({"channel": "alpha"})).unwrap();
    assert_eq!(s.state.viewer.channel, Channel::Alpha);
    s.execute("view.channel", json!({"channel": "alpha", "toggle": true})).unwrap();
    assert_eq!(s.state.viewer.channel, Channel::Rgb);
    s.execute("view.channel", json!({"channel": "RGB Straight", "colorized": true})).unwrap();
    assert_eq!(s.state.viewer.channel, Channel::RgbStraight);
    assert!(s.state.viewer.colorized);
    s.execute("view.exposure", json!({"stops": 1.5})).unwrap();
    s.execute("view.exposure", json!({"delta": -0.5})).unwrap();
    assert_eq!(s.state.viewer.exposure, 1.0);
    s.execute("view.resetExposure", json!({})).unwrap();
    assert_eq!(s.state.viewer.exposure, 0.0);
    s.execute("view.fastPreviewMode", json!({"mode": "fastDraft"})).unwrap();
    assert_eq!(s.state.viewer.fast_previews, crate::commands::viewer_cmds::FastPreviews::FastDraft);
    assert!(s.execute("view.fastPreviewMode", json!({"mode": "nope"})).is_err());
    let was = s.state.snapping;
    s.execute("view.snapping", json!({})).unwrap();
    assert_eq!(s.state.snapping, !was);
}

#[test]
fn snapshot_store_and_restore() {
    let mut s = comp();
    assert!(!s.is_enabled("view.showSnapshot"));
    s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 640, "height": 360})).unwrap();
    let r = s.execute("view.takeSnapshot", json!({"scale": 0.25})).unwrap();
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(160), Some(90)));
    // The project changes; the snapshot keeps the old frame.
    s.execute("layer.newSolid", json!({"color": "#0000ff", "width": 640, "height": 360})).unwrap();
    let snap = s.snapshot.clone().unwrap();
    assert_eq!(snap.image.get(10, 10), [1.0, 0.0, 0.0, 1.0]);
    let now = s.render(s.active_comp_id().unwrap(), Tick::ZERO, effectcraft_render::RenderOpts { scale: 0.25, ..Default::default() });
    assert_eq!(now.get(10, 10), [0.0, 0.0, 1.0, 1.0]);
    s.execute("view.showSnapshot", json!({"value": true})).unwrap();
    assert!(s.state.viewer.show_snapshot);
    let (w, h, rgba) = crate::commands::viewer_cmds::snapshot_rgba8(&s).unwrap();
    assert_eq!((w, h), (160, 90));
    assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
    s.execute("edit.purge", json!({"what": "snapshot"})).unwrap();
    assert!(s.snapshot.is_none() && !s.state.viewer.show_snapshot);
}

#[test]
fn guides_move_and_remove_with_undo() {
    let mut s = comp();
    s.execute("view.addGuide", json!({"orientation": "horizontal", "position": 40})).unwrap();
    s.execute("view.moveGuide", json!({"index": 0, "position": 90, "merge": "g"})).unwrap();
    s.execute("view.moveGuide", json!({"index": 0, "position": 95, "merge": "g"})).unwrap();
    assert_eq!(s.active_comp().unwrap().guides[0].position, 95.0);
    s.undo();
    assert_eq!(s.active_comp().unwrap().guides[0].position, 40.0);
    s.execute("view.removeGuide", json!({"index": 0})).unwrap();
    assert!(s.active_comp().unwrap().guides.is_empty());
    assert!(s.execute("view.removeGuide", json!({"index": 0})).is_err());
    s.undo();
    assert_eq!(s.active_comp().unwrap().guides.len(), 1);
}

#[test]
fn region_of_interest_renders_and_crops() {
    let mut s = comp();
    s.execute("layer.newSolid", json!({"color": "#ff8000", "width": 200, "height": 100})).unwrap();
    s.execute("view.setRegionOfInterest", json!({"rect": [200, 100, 240, 160]})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let full = s.render(cid, Tick::ZERO, Default::default());
    let roi = s.render(cid, Tick::ZERO, effectcraft_render::RenderOpts { roi: s.state.region_of_interest, ..Default::default() });
    assert_eq!((roi.width, roi.height), (240, 160));
    for (x, y) in [(0, 0), (30, 50), (239, 159), (119, 79)] {
        assert_eq!(roi.get(x, y), full.get(x + 200, y + 100));
    }
    s.execute("comp.cropToRegionOfInterest", json!({})).unwrap();
    let c = s.active_comp().unwrap();
    assert_eq!((c.width, c.height), (240, 160));
    let cropped = s.render(cid, Tick::ZERO, Default::default());
    assert_eq!(cropped.get(30, 50), roi.get(30, 50));
    assert_eq!(cropped.get(30, 50)[3], 1.0);
}

// ---------------------------------------------------------------- shape pen and vertices

fn shape_path(s: &Session, lid: u64, uid: u64) -> effectcraft_keyframe::ShapePath {
    let l = layer(s, lid);
    l.props.find_group(uid).unwrap().get("path").unwrap().value.as_path().unwrap().clone()
}

#[test]
fn shape_pen_builds_shape_group_and_renders() {
    let mut s = comp();
    // Nothing selected: a new shape layer; vertices in comp space.
    let r = s
        .execute("shape.newPath", json!({"vertices": [[100, 100], [300, 100], [300, 250], [100, 250]], "closed": true, "fill": [0, 1, 0], "strokeWidth": 0}))
        .unwrap();
    let (lid, group, path) = (r["layer"].as_u64().unwrap(), r["group"].as_u64().unwrap(), r["path"].as_u64().unwrap());
    let l = layer(&s, lid);
    assert!(matches!(l.source, effectcraft_project::LayerSource::Shape));
    let g = l.props.find_group(group).unwrap();
    assert_eq!(g.name, "Shape 1");
    let items: Vec<&str> = g.sub("contents").unwrap().groups().map(|x| x.match_id.as_str()).collect();
    assert_eq!(items, vec!["path", "fill"]);
    assert_eq!(g.sub("contents").unwrap().groups().next().unwrap().name, "Path 1");
    // Layer space: the layer sits at the comp centre.
    assert_eq!(shape_path(&s, lid, path).vertices[0], [100.0 - 320.0, 100.0 - 180.0]);
    let img = s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default());
    assert_eq!(img.get(200, 170), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(img.get(50, 50)[3], 0.0);
    // With the shape layer selected the Pen adds a second group (with a stroke) to it.
    let r2 = s.execute("shape.newPath", json!({"vertices": [[0, 0], [10, 10]], "space": "layer", "strokeWidth": 2})).unwrap();
    assert_eq!(r2["layer"].as_u64(), Some(lid));
    let l = layer(&s, lid);
    let names: Vec<String> = l.props.sub("contents").unwrap().groups().map(|g| g.name.clone()).collect();
    assert_eq!(names, vec!["Shape 2", "Shape 1"]);
    let g2 = l.props.find_group(r2["group"].as_u64().unwrap()).unwrap();
    let items: Vec<&str> = g2.sub("contents").unwrap().groups().map(|x| x.match_id.as_str()).collect();
    assert_eq!(items, vec!["path", "stroke", "fill"]);
    s.undo();
    assert_eq!(layer(&s, lid).props.sub("contents").unwrap().groups().count(), 1);
    // Not a shape layer → error.
    let sol = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("shape.newPath", json!({"layer": sol, "vertices": [[0, 0]]})).is_err());
}

/// The shape tools draw into the selected shape layer's Contents as a new group, as After
/// Effects does, and a new shape layer only with none selected (#227).
#[test]
fn shape_tools_draw_into_the_selected_shape_layer() {
    let mut s = comp();
    // Nothing selected: a new shape layer centred on the shape.
    let r = s.execute("shape.newShape", json!({"kind": "rect", "size": [100, 60], "position": [200, 100], "fill": [1, 0, 0]})).unwrap();
    let lid = r["layer"].as_u64().unwrap();
    let l = layer(&s, lid);
    assert_eq!(l.name, "Shape Layer 1");
    assert_eq!(l.props.prop("transform/position").unwrap().value.components()[..2], [200.0, 100.0]);
    // Selected (and scaled 200%): the next shape goes on top of its Contents, in layer space.
    s.execute("prop.set", json!({"layer": lid, "path": "transform/scale", "value": [200, 200, 100]})).unwrap();
    let n = s.active_comp().unwrap().layers.len();
    let r = s.execute("shape.newShape", json!({"kind": "ellipse", "size": [40, 20], "position": [300, 160]})).unwrap();
    assert_eq!(r["layer"].as_u64(), Some(lid));
    assert_eq!(s.active_comp().unwrap().layers.len(), n, "no new layer");
    let l = layer(&s, lid);
    let contents = l.props.sub("contents").unwrap();
    let names: Vec<&str> = contents.groups().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["Ellipse 1", "Rectangle 1"]);
    let g = l.props.find_group(r["group"].as_u64().unwrap()).unwrap();
    assert_eq!(g.sub("transform").unwrap().get("position").unwrap().value.components(), [50.0, 30.0]);
    assert_eq!(g.sub("contents").unwrap().groups().next().unwrap().get("size").unwrap().value.components(), [20.0, 10.0]);
    // A second rectangle is "Rectangle 2"; it renders where it was drawn.
    s.execute("shape.newShape", json!({"kind": "rect", "size": [40, 40], "position": [500, 300], "fill": [0, 1, 0]})).unwrap();
    let names: Vec<String> = layer(&s, lid).props.sub("contents").unwrap().groups().map(|g| g.name.clone()).collect();
    assert_eq!(names, ["Rectangle 2", "Ellipse 1", "Rectangle 1"]);
    let img = s.render(s.active_comp_id().unwrap(), Tick::ZERO, Default::default());
    assert_eq!(img.get(500, 300), [0.0, 1.0, 0.0, 1.0]);
    // Each shape is one undo step.
    s.undo();
    s.undo();
    assert_eq!(layer(&s, lid).props.sub("contents").unwrap().groups().count(), 1);
    // A locked shape layer isn't drawn into; a solid is refused when named.
    s.execute("layer.setSwitch", json!({"layers": [lid], "switch": "locked", "value": true})).unwrap();
    s.state.selected_layers = vec![LayerId(lid)];
    let r = s.execute("shape.newShape", json!({"kind": "star"})).unwrap();
    assert_ne!(r["layer"].as_u64(), Some(lid));
    let sol = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    assert!(s.execute("shape.newShape", json!({"layer": sol})).is_err());
    assert!(s.execute("shape.newShape", json!({"kind": "blob"})).is_err());
}

/// The Tools bar's Fill / Stroke Options (None, Solid Color, Linear and Radial Gradient, blend
/// mode, opacity) and Stroke Width paint new shapes and Pen paths; a command's own paint
/// parameters win (#227).
#[test]
fn shape_tool_options_paint_new_shapes() {
    use crate::commands::shape_tool::ShapeTool;
    let mut s = comp();
    let o = s.execute("shape.toolOptions", json!({})).unwrap();
    assert_eq!((o["createsMask"].clone(), o["fill"]["kind"].clone(), o["strokeWidth"].clone()), (json!(false), json!("solid"), json!(0.0)));
    // A radial gradient fill at 50% in Multiply and a red 4 px stroke.
    let o = s
        .execute("shape.toolOptions", json!({"fillType": "Radial Gradient", "fillBlend": "multiply", "fillOpacity": 50, "stroke": [1, 0, 0], "strokeWidth": 4}))
        .unwrap();
    assert_eq!(o["fill"]["kind"], json!("radial"));
    let r = s.execute("shape.newShape", json!({"kind": "ellipse", "size": [100, 60]})).unwrap();
    let lid = r["layer"].as_u64().unwrap();
    let items = |s: &Session, g: u64| layer(s, lid).props.find_group(g).unwrap().sub("contents").unwrap().clone();
    let g = items(&s, r["group"].as_u64().unwrap());
    assert_eq!(g.groups().map(|x| x.match_id.as_str()).collect::<Vec<_>>(), ["ellipse", "stroke", "gfill"]);
    let at = |g: &effectcraft_project::PropGroup, item: &str, prop: &str| g.groups().find(|x| x.match_id == item).unwrap().get(prop).unwrap().value.clone();
    let multiply = effectcraft_color::BlendMode::ALL.iter().position(|m| *m == effectcraft_color::BlendMode::Multiply).unwrap() as u32;
    assert_eq!(at(&g, "gfill", "type"), KV::Enum(1));
    assert_eq!(at(&g, "gfill", "blend"), KV::Enum(multiply));
    assert_eq!(at(&g, "gfill", "opacity").as_f64(), 50.0);
    assert_eq!(at(&g, "gfill", "end").components(), [50.0, 0.0], "out from the centre");
    assert_eq!(at(&g, "stroke", "color").components(), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(at(&g, "stroke", "width").as_f64(), 4.0);
    // No fill and a linear gradient stroke, across a Pen path's points.
    s.execute("shape.toolOptions", json!({"fill": false, "strokeType": "linear"})).unwrap();
    let r = s.execute("shape.newPath", json!({"layer": lid, "vertices": [[10, 20], [110, 40]], "space": "layer"})).unwrap();
    let g = items(&s, r["group"].as_u64().unwrap());
    assert_eq!(g.groups().map(|x| x.match_id.as_str()).collect::<Vec<_>>(), ["path", "gstroke"]);
    assert_eq!((at(&g, "gstroke", "start").components(), at(&g, "gstroke", "end").components()), (vec![10.0, 30.0], vec![110.0, 30.0]));
    // A command's own paint wins over the Tools bar's.
    let r = s.execute("shape.newShape", json!({"kind": "rect", "fill": "#00ff00", "strokeWidth": 0})).unwrap();
    let g = items(&s, r["group"].as_u64().unwrap());
    assert_eq!(g.groups().map(|x| x.match_id.as_str()).collect::<Vec<_>>(), ["rect", "fill"]);
    assert_eq!(at(&g, "fill", "color").components(), [0.0, 1.0, 0.0, 1.0]);
    // Bad values are errors that leave the options as they were; reset restores the defaults.
    let before = s.state.shape_tool.clone();
    for bad in [json!({"fillType": "plaid"}), json!({"strokeBlend": "nope"}), json!({"fill": true}), json!({"strokeWidth": "wide"})] {
        assert!(s.execute("shape.toolOptions", bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(s.state.shape_tool, before);
    s.execute("shape.toolOptions", json!({"reset": true, "createsMask": true})).unwrap();
    assert_eq!(s.state.shape_tool, ShapeTool { creates_mask: true, ..ShapeTool::default() });
}

/// Tool Creates Mask: every shape tool draws its mask (#227).
#[test]
fn masks_of_every_shape_tool_kind() {
    let mut s = comp();
    let sol = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    for (kind, n) in [("rect", 4), ("ellipse", 4), ("rounded", 8), ("polygon", 6), ("star", 10)] {
        let m = s.execute("layer.addMask", json!({"layer": sol, "shape": kind, "rect": [100, 100, 80, 80]})).unwrap()["mask"].as_u64().unwrap();
        let path = layer(&s, sol).props.find_group(m).unwrap().get("path").unwrap().value.as_path().unwrap().clone();
        assert!(path.closed, "{kind}");
        assert!(if kind == "rounded" { path.vertices.len() >= n } else { path.vertices.len() == n }, "{kind}: {}", path.vertices.len());
        assert!(path.vertices.iter().all(|v| (100.0..=180.0).contains(&v[0]) && (100.0..=180.0).contains(&v[1])), "{kind}: inside the box");
    }
    assert!(s.execute("layer.addMask", json!({"layer": sol, "shape": "blob"})).is_err());
}

#[test]
fn vertex_add_delete_convert_on_shape_paths_and_masks_undo() {
    let mut s = comp();
    let r = s.execute("shape.newPath", json!({"vertices": [[0, 0], [100, 0], [100, 100]], "space": "layer"})).unwrap();
    let (lid, path) = (r["layer"].as_u64().unwrap(), r["path"].as_u64().unwrap());
    // Pen continues the path: add a vertex, move it, close.
    s.execute("mask.addVertex", json!({"layer": lid, "mask": path, "point": [0, 100]})).unwrap();
    s.execute("mask.setClosed", json!({"layer": lid, "mask": path, "closed": true})).unwrap();
    let sp = shape_path(&s, lid, path);
    assert_eq!(sp.vertices.len(), 4);
    assert!(sp.closed);
    // Add Vertex on segment 0 at t=0.5 keeps the line and inserts its midpoint.
    let i = s.execute("mask.insertVertex", json!({"layer": lid, "mask": path, "segment": 0})).unwrap();
    assert_eq!(i.as_u64(), Some(1));
    assert_eq!(shape_path(&s, lid, path).vertices[1], [50.0, 0.0]);
    // Convert Vertex: corner → smooth → corner.
    let c = s.execute("mask.convertVertex", json!({"layer": lid, "mask": path, "index": 2})).unwrap();
    assert_eq!(c["smooth"], json!(true));
    let sp = shape_path(&s, lid, path);
    // Neighbours (50,0) and (100,100): tangent along them, a sixth of the way.
    assert_eq!(sp.out_tangents[2], [50.0 / 6.0, 100.0 / 6.0]);
    assert_eq!(sp.in_tangents[2], [-50.0 / 6.0, -100.0 / 6.0]);
    s.execute("mask.convertVertex", json!({"layer": lid, "mask": path, "index": 2})).unwrap();
    assert_eq!(shape_path(&s, lid, path).out_tangents[2], [0.0, 0.0]);
    s.undo();
    assert_ne!(shape_path(&s, lid, path).out_tangents[2], [0.0, 0.0]);
    // Delete Vertex (selection-based).
    s.execute("mask.selectVertices", json!({"vertices": [{"layer": lid, "mask": path, "index": 1}]})).unwrap();
    s.execute("mask.deleteVertices", json!({})).unwrap();
    assert_eq!(shape_path(&s, lid, path).vertices.len(), 4);
    s.undo();
    assert_eq!(shape_path(&s, lid, path).vertices.len(), 5);
    // Free transform of the whole shape path (scale 200% about the bbox centre).
    s.execute("mask.selectVertices", json!({"vertices": []})).unwrap();
    s.execute("path.freeTransform", json!({"layer": lid, "mask": path, "scale": 200})).unwrap();
    assert_eq!(shape_path(&s, lid, path).vertices[0], [-50.0, -50.0]);
    // Masks work through the same commands.
    let sol = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    let m = s.execute("mask.new", json!({"layer": sol, "vertices": [[0, 0], [100, 0], [100, 100]], "closed": true})).unwrap()["mask"].as_u64().unwrap();
    s.execute("mask.insertVertex", json!({"layer": sol, "mask": m, "segment": 2, "t": 0.5})).unwrap();
    let l = layer(&s, sol);
    let mp = l.masks().unwrap().find_group(m).unwrap().get("path").unwrap().value.as_path().unwrap().clone();
    assert_eq!(mp.vertices.len(), 4);
    assert_eq!(mp.vertices[3], [50.0, 50.0]);
}

#[test]
fn split_segment_keeps_curve_shape() {
    use effectcraft_keyframe::ShapePath;
    let mut sp = ShapePath {
        vertices: vec![[0.0, 0.0], [100.0, 0.0]],
        in_tangents: vec![[0.0; 2], [0.0, -50.0]],
        out_tangents: vec![[30.0, 60.0], [0.0; 2]],
        closed: false,
        feather: Vec::new(),
    };
    let seg = |sp: &ShapePath, i: usize, t: f64| {
        let (a, d) = (sp.vertices[i], sp.vertices[i + 1]);
        let b = [a[0] + sp.out_tangents[i][0], a[1] + sp.out_tangents[i][1]];
        let c = [d[0] + sp.in_tangents[i + 1][0], d[1] + sp.in_tangents[i + 1][1]];
        let u = 1.0 - t;
        let f = |k: usize| u * u * u * a[k] + 3.0 * u * u * t * b[k] + 3.0 * u * t * t * c[k] + t * t * t * d[k];
        [f(0), f(1)]
    };
    let before: Vec<[f64; 2]> = (0..=2000).map(|i| seg(&sp, 0, i as f64 / 2000.0)).collect();
    crate::commands::split_segment_for_tests(&mut sp, 0, 0.3);
    assert_eq!(sp.vertices.len(), 3);
    for i in 0..2 {
        for j in 0..=50 {
            let p = seg(&sp, i, j as f64 / 50.0);
            let d = before.iter().map(|q| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()).fold(f64::MAX, f64::min);
            assert!(d < 0.2, "{p:?} {d}");
        }
    }
}

// ---------------------------------------------------------------- motion paths and graph box

fn animated_position(s: &mut Session) -> u64 {
    let lid = s.execute("layer.newSolid", json!({"color": "#ffffff", "width": 50, "height": 50})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(0.0, [100.0, 100.0]), (1.0, [300.0, 100.0]), (2.0, [300.0, 300.0])] {
        s.execute("prop.addKey", json!({"layer": lid, "path": "transform/position", "time": t, "value": [v[0], v[1], 0.0]})).unwrap();
    }
    lid
}

#[test]
fn motion_path_key_drag_edits_value_and_tangents() {
    let mut s = comp();
    let lid = animated_position(&mut s);
    // Dragging the middle key's dot sets that key's value (merged drag = one undo step).
    s.execute("keys.set", json!({"layer": lid, "path": "transform/position", "time": 1.0, "value": [310, 120, 0], "merge": "mp"})).unwrap();
    s.execute("keys.set", json!({"layer": lid, "path": "transform/position", "time": 1.0, "value": [320, 140, 0], "merge": "mp"})).unwrap();
    let pos = |s: &Session| layer(s, lid).props.prop("transform/position").unwrap().clone();
    assert_eq!(pos(&s).keys[1].value, KV::Vec3([320.0, 140.0, 0.0]));
    assert_eq!(pos(&s).keys[0].value, KV::Vec3([100.0, 100.0, 0.0]));
    // Tangent drag: out handle set, in mirrored, no longer auto.
    s.execute("keys.setSpatialTangents", json!({"layer": lid, "path": "transform/position", "time": 1.0, "out": [40, 0], "merge": "tg"})).unwrap();
    let k = pos(&s).keys[1].clone();
    assert_eq!(k.spatial_out, [40.0, 0.0, 0.0]);
    assert_eq!(k.spatial_in, [-40.0, 0.0, 0.0]);
    assert!(!k.spatial_auto);
    // The motion path passes through the key and bulges along the tangent.
    let mid = pos(&s).value_at(Tick::from_seconds_f64(1.0)).as_vec3();
    assert_eq!([mid[0], mid[1]], [320.0, 140.0]);
    // Alt-drag (break): only the in handle changes.
    s.execute("keys.setSpatialTangents", json!({"layer": lid, "path": "transform/position", "time": 1.0, "in": [0, -30], "break": true})).unwrap();
    let k = pos(&s).keys[1].clone();
    assert_eq!((k.spatial_in, k.spatial_out), ([0.0, -30.0, 0.0], [40.0, 0.0, 0.0]));
    s.undo();
    s.undo();
    assert!(pos(&s).keys[1].spatial_auto);
    s.undo();
    assert_eq!(pos(&s).keys[1].value, KV::Vec3([300.0, 100.0, 0.0]));
}

#[test]
fn graph_transform_box_scales_key_times_and_values() {
    let mut s = comp();
    let lid = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    for (t, v) in [(0.0, 0.0), (1.0, 50.0), (2.0, 100.0)] {
        s.execute("prop.addKey", json!({"layer": lid, "path": "transform/opacity", "time": t, "value": v})).unwrap();
    }
    let uid = layer(&s, lid).props.prop("transform/opacity").unwrap().uid;
    let keys: Vec<_> = [0.0, 1.0, 2.0].iter().map(|t| json!({"layer": lid, "prop": uid, "time": t})).collect();
    s.execute("keys.select", json!({"keys": keys})).unwrap();
    // Time: scale 2× about 0 s; value: scale 0.5 about 100.
    s.execute("keys.transform", json!({"timeScale": 2.0, "timeAnchor": 0.0, "valueScale": 0.5, "valueAnchor": 100.0})).unwrap();
    let p = layer(&s, lid).props.prop("transform/opacity").unwrap().clone();
    let got: Vec<(f64, f64)> = p.keys.iter().map(|k| (k.time.seconds(), k.value.as_f64())).collect();
    assert_eq!(got, vec![(0.0, 50.0), (2.0, 75.0), (4.0, 100.0)]);
    assert_eq!(s.state.selected_keys.len(), 3);
    // Alt-drag of the last key in the timeline: scale the group about the first key; times
    // land on frames.
    s.execute("keys.transform", json!({"timeScale": 0.501, "timeAnchor": 0.0})).unwrap();
    let p = layer(&s, lid).props.prop("transform/opacity").unwrap().clone();
    let got: Vec<f64> = p.keys.iter().map(|k| k.time.seconds()).collect();
    assert_eq!(got, vec![0.0, 1.0, 2.0]);
    s.undo();
    s.undo();
    let p = layer(&s, lid).props.prop("transform/opacity").unwrap().clone();
    assert_eq!(p.keys.iter().map(|k| k.time.seconds()).collect::<Vec<_>>(), vec![0.0, 1.0, 2.0]);
    assert_eq!(p.keys[1].value.as_f64(), 50.0);
    assert!(s.execute("keys.transform", json!({"timeScale": 0})).is_err());
}

// ---------------------------------------------------------------- resolution

#[test]
fn each_comp_keeps_its_resolution_outside_the_undo_history_and_in_the_file() {
    use effectcraft_project::Resolution;
    let mut s = comp();
    let a = s.active_comp_id().unwrap();
    s.execute("layer.newSolid", json!({"name": "S", "color": "#ff0000", "width": 10, "height": 10})).unwrap();
    let path = std::env::temp_dir().join(format!("ec-resolution-{}.ecproj", std::process::id()));
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(s.active_comp().unwrap().resolution, Resolution::Auto);
    let (steps, pixels) = (s.history.undo.len(), s.active_comp_arc().unwrap());
    let r = s.execute_checked("view.res.half", json!({})).unwrap();
    assert_eq!((r["resolution"].clone(), r["factor"].clone()), (json!("Half"), json!(2)));
    // No undo step, but the project is modified (it's saved with it).
    assert_eq!(s.history.undo.len(), steps);
    assert!(s.is_dirty());
    assert!(s.active_comp().unwrap().same_pixels(&pixels));
    // Another comp has its own; switching back shows comp A's again.
    s.execute("comp.new", json!({"name": "B", "width": 320, "height": 180, "frameRate": 30, "duration": 5})).unwrap();
    let b = s.active_comp_id().unwrap();
    assert_ne!(a, b);
    assert_eq!(s.active_comp().unwrap().resolution, Resolution::Auto);
    s.execute_checked("view.res.custom", json!({"factor": 6})).unwrap();
    s.open_comp(a);
    assert_eq!(s.active_comp().unwrap().resolution, Resolution::Half);
    assert_eq!(s.execute("comp.info", json!({})).unwrap()["resolution"], json!("Half"));
    assert_eq!(crate::menus::checked(&s, "view.res.half", &json!({})), Some(true));
    assert_eq!(crate::menus::checked(&s, "view.res.full", &json!({})), Some(false));
    // Undo and redo restore the comps, not their resolutions.
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.comp(b).is_none(), "undid New Composition");
    assert_eq!(s.project.comp(a).unwrap().resolution, Resolution::Half);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project.comp(b).unwrap().resolution, Resolution::Custom(6));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_comp().unwrap().layers.is_empty(), "undid the solid");
    assert_eq!(s.project.comp(a).unwrap().resolution, Resolution::Half);
    s.execute("edit.redo", json!({})).unwrap();
    s.execute("edit.redo", json!({})).unwrap();
    // The comp's explicit `comp` parameter; the same value again changes nothing.
    s.execute_checked("view.res.quarter", json!({"comp": b.0})).unwrap();
    let rev = s.revision;
    s.execute_checked("view.res.quarter", json!({"comp": b.0})).unwrap();
    assert_eq!(s.revision, rev);
    // Saved with the project and read back.
    s.execute("file.save", json!({})).unwrap();
    assert!(!s.is_dirty());
    let mut o = Session::default();
    o.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(o.project.comp(a).unwrap().resolution, Resolution::Half);
    assert_eq!(o.project.comp(b).unwrap().resolution, Resolution::Quarter);
    let _ = std::fs::remove_file(path);
    // Custom without a factor asks the frontend (the Custom Resolution dialog).
    s.drain_events();
    s.execute_checked("view.res.custom", json!({})).unwrap();
    assert!(s.drain_events().iter().any(|e| matches!(e, crate::Event::Frontend { command, .. } if command == "view.res.custom")));
    assert!(s.execute_checked("view.res.custom", json!({"factor": "big"})).is_err());
    s.execute_checked("view.res.auto", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().resolution, Resolution::Auto);
}
