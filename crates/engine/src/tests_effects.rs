//! Effect Controls operations: copy/paste, duplicate, reorder, remove (Delete), reset, and the
//! Edit menu routing to them when effects are selected, all with undo.

use effectcraft_keyframe::{Ease, Interp, Value as KV};
use effectcraft_project::{Layer, LayerId, PropGroup};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;

fn comp_with_two_solids() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 400, "height": 300})).unwrap()["layer"].as_u64().unwrap();
    (s, a, b)
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn fx_names(s: &Session, id: u64) -> Vec<String> {
    layer(s, id).effects().map(|f| f.groups().map(|g| g.name.clone()).collect()).unwrap_or_default()
}

fn apply(s: &mut Session, lid: u64, effect: &str) -> u64 {
    s.execute("effect.apply", json!({"layers": [lid], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap()
}

fn select_effect(s: &mut Session, lid: u64, uid: u64) {
    s.state.selected_layers = vec![LayerId(lid)];
    s.state.selected_props = vec![(LayerId(lid), uid)];
}

fn effect_group(s: &Session, lid: u64, uid: u64) -> PropGroup {
    layer(s, lid).effects().unwrap().groups().find(|g| g.uid == uid).unwrap().clone()
}

fn animate_effect(s: &mut Session, lid: u64, uid: u64, path: &str, keys: &[(f64, f64)]) {
    let prop = effect_group(s, lid, uid).prop(path).unwrap().uid;
    for &(time, value) in keys {
        s.execute("prop.addKey", json!({"layer": lid, "prop": prop, "time": time, "timeBase": "comp", "value": value})).unwrap();
    }
}

fn set_layer_clock(s: &mut Session, lid: u64, start: f64, stretch: f64) {
    let cid = s.active_comp_id().unwrap();
    let l = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(lid)).unwrap();
    l.start_time = Tick::from_seconds_f64(start);
    l.stretch = stretch;
}

#[test]
fn copy_paste_effects_between_layers_with_undo() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let gid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    s.execute("prop.set", json!({"layer": a, "prop": gid, "value": 12.5})).unwrap();
    select_effect(&mut s, a, blur);
    // Edit ▸ Copy with an effect selected copies the effect, not the layer.
    s.execute("edit.copy", json!({})).unwrap();
    assert_eq!(s.state.effect_clipboard.len(), 1);
    assert!(s.state.clipboard.is_empty());
    // Paste onto the other layer.
    s.state.selected_layers = vec![LayerId(b)];
    s.state.selected_props.clear();
    let n_layers = s.active_comp().unwrap().layers.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n_layers, "no layer pasted");
    assert_eq!(fx_names(&s, b), vec!["Gaussian Blur"]);
    let pasted = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_ne!(pasted.uid, blur, "fresh uids");
    assert_eq!(pasted.get("blurriness").unwrap().value, KV::Scalar(12.5));
    // Pasting again names the second instance uniquely.
    s.execute("effect.paste", json!({"layers": [b]})).unwrap();
    assert_eq!(fx_names(&s, b), vec!["Gaussian Blur", "Gaussian Blur 2"]);
    s.undo();
    s.undo();
    assert!(fx_names(&s, b).is_empty());
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur"]);
}

/// #989: copying an animated effect aligns its first key to the destination CTI.
#[test]
fn pasted_effect_keys_start_at_the_playhead() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    for (time, value) in [(1.0, 10.0), (3.0, 30.0)] {
        s.execute("prop.addKey", json!({"layer": a, "prop": uid, "time": time, "value": value})).unwrap();
    }
    let source = layer(&s, a);
    select_effect(&mut s, a, blur);
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    s.execute("layer.select", json!({"layers": [b]})).unwrap();
    let undo = s.history.undo.len();
    s.execute("edit.paste", json!({})).unwrap();
    let pasted = layer(&s, b);
    let keys = &pasted.effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().keys;
    assert_eq!(keys.iter().map(|k| pasted.comp_time(k.time).seconds()).collect::<Vec<_>>(), vec![5.0, 7.0]);
    assert_eq!(keys.iter().map(|k| k.value.as_f64()).collect::<Vec<_>>(), vec![10.0, 30.0]);
    assert_eq!(layer(&s, a), source, "copy and paste do not edit the source");
    assert_eq!(s.history.undo.len(), undo + 1, "the whole paste is one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(fx_names(&s, b).is_empty());
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(layer(&s, b), pasted);
}

#[test]
fn pasted_effects_share_one_anchor_including_nested_parameters() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let fractal = apply(&mut s, a, "Fractal");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    animate_effect(&mut s, a, fractal, "mandelbrot/mandelbrotX", &[(2.0, 1.25), (4.0, 2.5)]);
    animate_effect(&mut s, a, fractal, "mandelbrot/mandelbrotY", &[(3.0, 0.5), (5.0, 2.0)]);
    let static_value = effect_group(&s, a, fractal).get("setChoice").unwrap().value.clone();
    s.state.selected_props = vec![(LayerId(a), blur), (LayerId(a), fractal)];
    s.execute("edit.copy", json!({})).unwrap();
    s.execute("time.set", json!({"time": 5.0})).unwrap();
    let reply = s.execute("effect.paste", json!({"layers": [b]})).unwrap();
    for (index, path, times) in
        [(0, "blurriness", vec![5.0, 7.0]), (1, "mandelbrot/mandelbrotX", vec![6.0, 8.0]), (1, "mandelbrot/mandelbrotY", vec![7.0, 9.0])]
    {
        let group = effect_group(&s, b, reply["effects"][index].as_u64().unwrap());
        assert_eq!(group.prop(path).unwrap().keys.iter().map(|k| k.time.seconds()).collect::<Vec<_>>(), times, "{path}");
    }
    let group = effect_group(&s, b, reply["effects"][1].as_u64().unwrap());
    assert_eq!(group.get("setChoice").unwrap().value, static_value, "unanimated controls stay unchanged");
}

#[test]
fn pasted_effects_preserve_comp_timing_and_eased_values_on_stretched_layers() {
    for (source_start, source_stretch) in [(2.0, 100.0), (1.0, 200.0), (8.0, -200.0)] {
        for (target_start, target_stretch) in [(0.0, 100.0), (3.0, 200.0), (8.0, -100.0)] {
            let (mut s, a, b) = comp_with_two_solids();
            set_layer_clock(&mut s, a, source_start, source_stretch);
            set_layer_clock(&mut s, b, target_start, target_stretch);
            let blur = apply(&mut s, a, "Gaussian Blur");
            animate_effect(&mut s, a, blur, "blurriness", &[(3.0, 10.0), (5.0, 30.0)]);
            let uid = effect_group(&s, a, blur).get("blurriness").unwrap().uid;
            let cid = s.active_comp_id().unwrap();
            let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
            for key in &mut pr.keys {
                key.in_interp = Interp::Bezier;
                key.out_interp = Interp::Bezier;
                key.in_ease = vec![Ease { speed: 7.0, influence: 0.25 }];
                key.out_ease = vec![Ease { speed: 3.0, influence: 0.6 }];
                key.label = 4;
            }
            let source = layer(&s, a);
            let source_pr = source.props.find(uid).unwrap();
            s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
            s.execute("time.set", json!({"time": 5.0})).unwrap();
            let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
            let target = layer(&s, b);
            let target_group = target.effects().unwrap().groups().find(|g| g.uid == pasted).unwrap();
            let target_pr = target_group.get("blurriness").unwrap();
            let mut times: Vec<Tick> = target_pr.keys.iter().map(|k| target.comp_time(k.time)).collect();
            times.sort();
            assert_eq!(times, vec![Tick::from_seconds_f64(5.0), Tick::from_seconds_f64(7.0)]);
            for offset in [0.0, 0.25, 0.75, 1.5, 2.0] {
                let expected = source_pr.value_at(source.layer_time(Tick::from_seconds_f64(3.0 + offset))).as_f64();
                let actual = target_pr.value_at(target.layer_time(Tick::from_seconds_f64(5.0 + offset))).as_f64();
                assert!((expected - actual).abs() < 1e-8, "source {source_stretch} %, target {target_stretch} %, t={offset}: {expected} != {actual}");
            }
            assert!(target_pr.keys.iter().all(|k| k.label == 4));
            assert_eq!(layer(&s, a), source);
        }
    }
}

#[test]
fn pasted_spatial_effect_keys_preserve_the_eased_path_on_reversed_layers() {
    for (source_stretch, target_stretch) in [(200.0, 100.0), (-200.0, 100.0), (100.0, -200.0)] {
        let (mut s, a, b) = comp_with_two_solids();
        set_layer_clock(&mut s, a, if source_stretch < 0.0 { 8.0 } else { 1.0 }, source_stretch);
        set_layer_clock(&mut s, b, if target_stretch < 0.0 { 8.0 } else { 0.0 }, target_stretch);
        let ramp = apply(&mut s, a, "Gradient Ramp");
        let uid = effect_group(&s, a, ramp).get("start").unwrap().uid;
        for (time, value) in [(3.0, [50.0, 25.0]), (5.0, [180.0, 75.0])] {
            s.execute("prop.addKey", json!({"layer": a, "prop": uid, "time": time, "timeBase": "comp", "value": value})).unwrap();
        }
        let cid = s.active_comp_id().unwrap();
        let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
        pr.spatial = true;
        for key in &mut pr.keys {
            key.in_interp = Interp::Bezier;
            key.out_interp = Interp::Bezier;
            key.in_ease = vec![Ease { speed: 15.0, influence: 0.25 }];
            key.out_ease = vec![Ease { speed: 30.0, influence: 0.6 }];
            key.spatial_auto = false;
            key.spatial_in = [-10.0, -20.0, 0.0];
            key.spatial_out = [20.0, 5.0, 0.0];
        }
        let source = layer(&s, a);
        let source_pr = source.props.find(uid).unwrap();
        s.execute("effect.copy", json!({"layer": a, "effect": ramp})).unwrap();
        s.execute("time.set", json!({"time": 5.0})).unwrap();
        let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
        let target = layer(&s, b);
        let group = target.effects().unwrap().groups().find(|g| g.uid == pasted).unwrap();
        let target_pr = group.get("start").unwrap();
        for offset in [0.0, 0.25, 0.75, 1.5, 2.0] {
            let expected = source_pr.value_at(source.layer_time(Tick::from_seconds_f64(3.0 + offset))).components();
            let actual = target_pr.value_at(target.layer_time(Tick::from_seconds_f64(5.0 + offset))).components();
            assert_eq!(expected.len(), actual.len());
            assert!(expected.iter().zip(&actual).all(|(a, b)| (a - b).abs() < 0.05), "path at {offset}: {expected:?} != {actual:?}");
        }
    }
}

#[test]
fn effect_clipboard_is_a_snapshot_and_repeated_paste_uses_the_target_comps_cti() {
    let (mut s, a, _) = comp_with_two_solids();
    let original_comp = s.active_comp_id().unwrap();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let clipboard = s.state.effect_clipboard.clone();
    // Changing the source after Copy must not change what the clipboard will paste.
    animate_effect(&mut s, a, blur, "blurriness", &[(4.0, 99.0)]);
    s.execute("time.set", json!({"time": 9.0})).unwrap();
    let destination = s.execute("comp.new", json!({"name": "Destination", "frameRate": 24, "duration": 10})).unwrap()["comp"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#ffffff"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("time.set", json!({"time": 4.0})).unwrap();
    s.open_comp(original_comp);
    for (cti, expected) in [(4.0, vec![4.0, 6.0]), (2.0, vec![2.0, 4.0])] {
        s.state.times.insert(effectcraft_project::ItemId(destination), Tick::from_seconds_f64(cti));
        let uid = s.execute("effect.paste", json!({"comp": destination, "layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
        let l = s.project.comp(effectcraft_project::ItemId(destination)).unwrap().layer(LayerId(b)).unwrap();
        let pr = l.effects().unwrap().groups().find(|g| g.uid == uid).unwrap().get("blurriness").unwrap();
        assert_eq!(pr.keys.iter().map(|k| l.comp_time(k.time).seconds()).collect::<Vec<_>>(), expected);
        assert_eq!(pr.keys.iter().map(|k| k.value.as_f64()).collect::<Vec<_>>(), vec![10.0, 30.0]);
        assert_eq!(s.state.effect_clipboard, clipboard);
    }
}

#[test]
fn invalid_effect_paste_timing_leaves_every_target_and_history_unchanged() {
    for stretch in [0.0, f64::NAN, f64::INFINITY, 1e-300, 1e300] {
        let (mut s, a, b) = comp_with_two_solids();
        let blur = apply(&mut s, a, "Gaussian Blur");
        animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
        s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
        set_layer_clock(&mut s, b, 0.0, stretch);
        s.execute("time.set", json!({"time": 5.0})).unwrap();
        let project = s.project.clone();
        let clipboard = s.state.effect_clipboard.clone();
        let undo = s.history.undo.len();
        // A is valid and comes first; failure on B must roll back A's new effect too.
        assert!(s.execute("effect.paste", json!({"layers": [a, b]})).is_err(), "{stretch}");
        assert!(std::sync::Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.state.effect_clipboard, clipboard);
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn invalid_effect_copy_keeps_the_previous_clipboard() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let clipboard = s.state.effect_clipboard.clone();
    set_layer_clock(&mut s, a, 0.0, 0.0);
    assert!(s.execute("effect.copy", json!({"layer": a, "effect": blur})).is_err());
    assert_eq!(s.state.effect_clipboard, clipboard);
}

#[test]
fn effect_paste_keeps_exact_ntsc_frame_offsets() {
    let (mut s, a, b) = comp_with_two_solids();
    let cid = s.active_comp_id().unwrap();
    let frame = effectcraft_time::FrameRate::new(30000, 1001).frame_duration();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = effect_group(&s, a, blur).get("blurriness").unwrap().uid;
    let pr = std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_mut(uid).unwrap();
    pr.keys = vec![
        effectcraft_keyframe::Keyframe::new(frame.mul_ratio(11, 1), KV::Scalar(10.0)),
        effectcraft_keyframe::Keyframe::new(frame.mul_ratio(43, 1), KV::Scalar(30.0)),
    ];
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let cti = frame.mul_ratio(71, 1);
    s.state.times.insert(cid, cti);
    let pasted = s.execute("effect.paste", json!({"layers": [b]})).unwrap()["effects"][0].as_u64().unwrap();
    let group = effect_group(&s, b, pasted);
    assert_eq!(group.get("blurriness").unwrap().keys.iter().map(|k| k.time).collect::<Vec<_>>(), vec![cti, cti + frame.mul_ratio(32, 1)]);
}

#[test]
fn overflowing_effect_paste_times_return_an_error_without_an_edit() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    animate_effect(&mut s, a, blur, "blurriness", &[(1.0, 10.0), (3.0, 30.0)]);
    s.execute("effect.copy", json!({"layer": a, "effect": blur})).unwrap();
    let cid = s.active_comp_id().unwrap();
    for cti in [Tick::MAX, Tick(i64::MAX)] {
        s.state.times.insert(cid, cti);
        let project = s.project.clone();
        let undo = s.history.undo.len();
        assert!(s.execute("effect.paste", json!({"layers": [b]})).is_err());
        assert!(std::sync::Arc::ptr_eq(&s.project, &project));
        assert_eq!(s.history.undo.len(), undo);
    }
}

#[test]
fn duplicate_reorder_and_delete_selected_effect() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    apply(&mut s, a, "Invert");
    select_effect(&mut s, a, blur);
    // Edit ▸ Duplicate (Cmd+D) duplicates the selected effect right after it and selects the copy.
    s.execute("edit.duplicate", json!({})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Gaussian Blur 2", "Invert"]);
    let dup = s.state.selected_props[0].1;
    assert_ne!(dup, blur);
    // Reorder: move Invert to the top.
    s.execute("effect.reorder", json!({"layer": a, "effect": "Invert", "index": 1})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur", "Gaussian Blur 2"]);
    // Delete with the duplicate selected removes just that effect (not the layer).
    s.execute("edit.clear", json!({})).unwrap();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur"]);
    assert!(s.active_comp().unwrap().layer(LayerId(a)).is_some());
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Invert", "Gaussian Blur", "Gaussian Blur 2"]);
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Gaussian Blur 2", "Invert"]);
    s.undo();
    assert_eq!(fx_names(&s, a), vec!["Gaussian Blur", "Invert"]);
}

#[test]
fn reset_effect_restores_defaults_in_one_undo_step() {
    let (mut s, _, b) = comp_with_two_solids();
    let ramp = apply(&mut s, b, "Gradient Ramp");
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    let start = g.get("start").unwrap().uid;
    let shape = g.get("shape").unwrap().uid;
    // Point defaults are fractions of the layer (400 × 300).
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([200.0, 0.0]));
    s.execute("prop.set", json!({"layer": b, "prop": start, "value": [10.0, 20.0]})).unwrap();
    s.execute("prop.set", json!({"layer": b, "prop": shape, "value": 1})).unwrap();
    let undo_before = s.history.undo.len();
    s.execute("effect.reset", json!({"layer": b, "effect": ramp})).unwrap();
    assert_eq!(s.history.undo.len(), undo_before + 1);
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([200.0, 0.0]));
    assert_eq!(g.get("shape").unwrap().value, KV::Enum(0));
    s.undo();
    let g = layer(&s, b).effects().unwrap().groups().next().unwrap().clone();
    assert_eq!(g.get("start").unwrap().value, KV::Vec2([10.0, 20.0]));
}

#[test]
fn reset_restores_parameters_inside_twirl_down_groups() {
    let (mut s, a, _) = comp_with_two_solids();
    // Fractal and Scribble keep most parameters in groups (`mandelbrot/…`, `edgeOptions/…`).
    for (effect, changes) in [
        ("Fractal", [("setChoice", json!(1)), ("mandelbrot/mandelbrotX", json!(1.25)), ("fractalColor/hue", json!(90.0))]),
        ("Scribble", [("scribble", json!(2)), ("edgeOptions/edgeWidth", json!(33.0)), ("strokeOptions/spacing", json!(9.0))]),
    ] {
        let fx = apply(&mut s, a, effect);
        let group = |s: &Session| layer(s, a).effects().unwrap().groups().find(|g| g.uid == fx).unwrap().clone();
        let defaults: Vec<_> = changes.iter().map(|(id, _)| group(&s).prop(id).unwrap().value.clone()).collect();
        for (id, v) in &changes {
            let uid = group(&s).prop(id).unwrap().uid;
            s.execute("prop.set", json!({"layer": a, "prop": uid, "value": v})).unwrap();
            assert_ne!(group(&s).prop(id).unwrap().value, defaults[changes.iter().position(|(i, _)| i == id).unwrap()], "{effect} {id} changed");
        }
        s.execute("effect.reset", json!({"layer": a, "effect": fx})).unwrap();
        for ((id, _), d) in changes.iter().zip(&defaults) {
            assert_eq!(&group(&s).prop(id).unwrap().value, d, "{effect} {id} back to its default");
        }
    }
}

#[test]
fn reset_keys_animated_params_at_the_cti() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let uid = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().uid;
    s.execute("prop.toggleAnimation", json!({"layer": a, "prop": uid})).unwrap();
    s.execute("prop.set", json!({"layer": a, "prop": uid, "value": 30.0})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("effect.reset", json!({"layer": a, "effect": blur})).unwrap();
    let pr = layer(&s, a).effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().clone();
    assert_eq!(pr.keys.len(), 2, "a key at the CTI with the default value");
    assert_eq!(pr.keys[1].value, KV::Scalar(0.0));
}

#[test]
fn tint_swaps_only_its_colors_in_one_undo_step() {
    let (mut s, a, b) = comp_with_two_solids();
    let first = apply(&mut s, a, "Tint");
    let target = apply(&mut s, a, "Tint");
    let other = apply(&mut s, b, "Tint");
    for (path, value) in [("black", json!([1.0, 0.0, 0.0, 1.0])), ("white", json!([0.0, 0.25, 1.0, 1.0])), ("amount", json!(45.0))] {
        s.execute("prop.set", json!({"layer": a, "path": format!("effects/#2/{path}"), "value": value})).unwrap();
    }
    let before_a = layer(&s, a);
    let before_b = layer(&s, b);
    let n = s.history.undo.len();
    s.execute("effect.swapTintColors", json!({"layer": a, "effect": target})).unwrap();
    let swapped = layer(&s, a);
    let g = swapped.props.find_group(target).unwrap();
    assert_eq!(g.prop("black").unwrap().value, KV::Color([0.0, 0.25, 1.0, 1.0]));
    assert_eq!(g.prop("white").unwrap().value, KV::Color([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(g.prop("amount").unwrap().value, KV::Scalar(45.0));
    assert_eq!(swapped.props.find_group(first), before_a.props.find_group(first));
    assert_eq!(layer(&s, b).props.find_group(other), before_b.props.find_group(other));
    assert_eq!(s.history.undo.len(), n + 1);
    assert!(s.undo());
    assert_eq!(layer(&s, a).props, before_a.props);
    assert!(s.redo());
    assert_eq!(layer(&s, a).props, swapped.props);
}

#[test]
fn tint_swap_keys_animated_colors_at_layer_time() {
    let (mut s, a, _) = comp_with_two_solids();
    let target = apply(&mut s, a, "Tint");
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/black", "value": [1.0, 0.0, 0.0, 1.0]})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/white", "value": [0.0, 0.0, 1.0, 1.0]})).unwrap();
    s.execute("prop.toggleAnimation", json!({"layer": a, "path": "effects/#1/black"})).unwrap();
    s.execute("time.set", json!({"time": 2.0})).unwrap();
    s.execute("prop.set", json!({"layer": a, "path": "effects/#1/black", "value": [0.0, 1.0, 0.0, 1.0]})).unwrap();
    s.execute("layer.timeStretch", json!({"layers": [a], "percent": 200.0})).unwrap();
    // At comp time 2s the stretched layer is at 1s, between its unchanged 0s and 2s keys.
    let before = layer(&s, a);
    let g = before.props.find_group(target).unwrap();
    let lt = before.layer_time(s.time());
    let expected = g.prop("black").unwrap().value_at(lt);
    let keys = g.prop("black").unwrap().keys.clone();
    select_effect(&mut s, a, target);
    s.execute("effect.swapTintColors", json!({})).unwrap();
    let after = layer(&s, a);
    let g = after.props.find_group(target).unwrap();
    let black = g.prop("black").unwrap();
    assert_eq!(black.keys.len(), 3);
    assert_eq!(black.keys[0], keys[0]);
    assert_eq!(black.keys[2], keys[1]);
    assert_eq!(black.keys[1].time, lt);
    assert_eq!(black.keys[1].value, KV::Color([0.0, 0.0, 1.0, 1.0]));
    assert_eq!(g.prop("white").unwrap().value, expected);
    assert!(g.prop("white").unwrap().keys.is_empty());
    assert!(s.undo());
    assert_eq!(layer(&s, a).props, before.props);
}

#[test]
fn tint_swap_rejects_invalid_targets_without_editing() {
    let mut empty = Session::default();
    assert!(empty.execute("effect.swapTintColors", json!({})).is_err());
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let tint = apply(&mut s, a, "Tint");
    for params in [
        json!({"layer": a, "effect": blur}),
        json!({"layer": a, "effect": "missing"}),
        json!({"layer": a, "effect": []}),
        json!({"layer": a, "effect": 1.5}),
        json!({"layer": a, "effect": 0}),
        json!(false),
    ] {
        let before = layer(&s, a);
        let n = s.history.undo.len();
        assert!(s.execute("effect.swapTintColors", params).is_err());
        assert_eq!(layer(&s, a).props, before.props);
        assert_eq!(s.history.undo.len(), n);
    }
    // A damaged project's colour parameter must also fail atomically.
    let cid = s.active_comp_id().unwrap();
    s.edit("Break Tint", None, |p, _| {
        p.comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.find_group_mut(tint).unwrap().prop_mut("white").unwrap().value = KV::Scalar(1.0);
        Ok(())
    })
    .unwrap();
    let before = layer(&s, a);
    let n = s.history.undo.len();
    assert!(s.execute("effect.swapTintColors", json!({"layer": a, "effect": tint})).is_err());
    assert_eq!(layer(&s, a).props, before.props);
    assert_eq!(s.history.undo.len(), n);
}

#[test]
fn copy_layers_clears_effect_clipboard() {
    let (mut s, a, b) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    select_effect(&mut s, a, blur);
    s.execute("effect.copy", json!({})).unwrap();
    s.state.selected_props.clear();
    s.state.selected_layers = vec![LayerId(b)];
    s.execute("edit.copy", json!({})).unwrap();
    assert!(s.state.effect_clipboard.is_empty());
    let n = s.active_comp().unwrap().layers.len();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), n + 1, "layer pasted");
}

#[test]
fn layer_params_get_a_source_companion() {
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "ec.channel.blend");
    let g = layer(&s, a).effects().unwrap().groups().next().unwrap().clone();
    let layer_params: Vec<_> = g.props().filter(|p| matches!(p.ui, effectcraft_project::ParamUi::Layer)).map(|p| p.match_id.clone()).collect();
    assert!(!layer_params.is_empty());
    for id in layer_params {
        let src = g.get(&effectcraft_effects::layer_source_id(&id)).expect("companion");
        assert_eq!(src.value, KV::Enum(2), "Effects & Masks by default");
        assert!(matches!(src.ui, effectcraft_project::ParamUi::Hidden));
    }
}

/// Projects saved before parameters were renamed, regrouped, added or had their popups
/// reordered open with their instances brought up to date (`effectcraft_effects::migrate`).
#[test]
fn opening_an_old_project_upgrades_effect_instances() {
    use effectcraft_effects::migrate::PARAM_ID_ALIASES;
    use effectcraft_project::{Node, ParamUi, PropGroup};
    let (mut s, a, _) = comp_with_two_solids();
    apply(&mut s, a, "ec.blur.gaussian");
    // One instance of every effect with renamed parameter ids.
    let mut aliased: Vec<&str> = PARAM_ID_ALIASES.iter().map(|(e, _, _)| *e).collect();
    aliased.dedup();
    for e in &aliased {
        apply(&mut s, a, e);
    }
    // Rewrite the saved instances the way an older version wrote them.
    fn take(g: &mut PropGroup, path: &str) -> Option<effectcraft_project::Property> {
        match path.split_once('/') {
            None => {
                let i = g.children.iter().position(|c| matches!(c, Node::Prop(p) if p.match_id == path))?;
                match g.children.remove(i) {
                    Node::Prop(p) => Some(p),
                    Node::Group(_) => None,
                }
            }
            Some((first, rest)) => take(g.sub_mut(first)?, rest),
        }
    }
    let mut old = (*s.project).clone();
    let cid = s.active_comp_id().unwrap();
    {
        let comp = std::sync::Arc::make_mut(match &mut old.items.get_mut(&cid).unwrap().kind {
            effectcraft_project::ItemKind::Comp(c) => c,
            _ => panic!("not a composition"),
        });
        let l = comp.layers.iter_mut().find(|l| l.id == LayerId(a)).unwrap();
        for n in &mut l.props.sub_mut("effects").unwrap().children {
            let Node::Group(g) = n else { continue };
            if g.match_id == "ec.blur.gaussian" {
                // Repeat Edge Pixels did not exist yet, Blurriness had another name, the popup
                // listed its options in reverse.
                take(g, "repeatEdge").unwrap();
                g.get_mut("blurriness").unwrap().name = "Old Blurriness".into();
                let d = g.get_mut("dimensions").unwrap();
                let ParamUi::Popup { options } = &d.ui else { panic!("dimensions is a popup") };
                d.ui = ParamUi::Popup { options: options.iter().rev().cloned().collect() };
                d.value = KV::Enum(0);
            }
            for (e, old_id, new_id) in PARAM_ID_ALIASES {
                if g.match_id == *e {
                    let mut p = take(g, new_id).unwrap_or_else(|| panic!("{e}: no {new_id}"));
                    // Put it back at its old path (which may be inside a group).
                    let (path, leaf) = old_id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, old_id));
                    p.match_id = leaf.to_string();
                    let mut at: &mut PropGroup = g;
                    for seg in path.into_iter().flat_map(|p| p.split('/')) {
                        if at.sub(seg).is_none() {
                            at.children.push(PropGroup::new(1_000_000 + at.children.len() as u64, seg, seg).into());
                        }
                        at = at.sub_mut(seg).unwrap();
                    }
                    at.children.push(p.into());
                }
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("ec-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old.to_json()).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    let l = s2.project.comp(cid).unwrap().layer(LayerId(a)).unwrap().clone();
    let fx = l.effects().unwrap();
    let spec = effectcraft_effects::find("ec.blur.gaussian").unwrap();
    let sp = |id: &str| spec.params.iter().find(|p| p.id == id).unwrap();
    let blur = fx.groups().find(|g| g.match_id == "ec.blur.gaussian").unwrap();
    assert_eq!(blur.get("blurriness").unwrap().name, sp("blurriness").name);
    assert!(blur.get("repeatEdge").is_some(), "missing parameter added");
    let dims = blur.get("dimensions").unwrap();
    let ParamUi::Popup { options } = &sp("dimensions").ui else { panic!() };
    assert_eq!(dims.value, KV::Enum(options.len() as u32 - 1), "popup value remapped by label");
    assert_eq!(dims.ui, sp("dimensions").ui);
    for (e, old_id, new_id) in PARAM_ID_ALIASES {
        let g = fx.groups().find(|g| g.match_id == *e).unwrap();
        let at = |id: &str| {
            let (path, leaf) = id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, id));
            let mut node = Some(g);
            for seg in path.into_iter().flat_map(|p| p.split('/')) {
                node = node.and_then(|n| n.sub(seg));
            }
            node.and_then(|n| n.get(leaf)).is_some()
        };
        assert!(!at(old_id), "{e}: {old_id} still present");
        let (path, leaf) = new_id.rsplit_once('/').map(|(p, l)| (Some(p), l)).unwrap_or((None, new_id));
        let mut node = Some(g);
        for seg in path.into_iter().flat_map(|p| p.split('/')) {
            node = node.and_then(|n| n.sub(seg));
        }
        assert!(node.and_then(|n| n.get(leaf)).is_some(), "{e}: {old_id} not moved to {new_id}");
    }
    // The upgraded project renders.
    let _ = s2.render(cid, s2.time(), Default::default());
}

/// A project saved before schema 2 (#227): effect points on a shape layer were measured from
/// its origin. Opening converts them to effect space so points that were set keep their place
/// in the comp; points at their default become the layer centre; layers with a source
/// rectangle, adjustment layers (their effects see the comp below as it is) and expression
/// controls keep their values. Saving writes schema 2, and opening that again changes nothing.
#[test]
fn opening_a_schema_1_project_moves_shape_layer_effect_points_into_effect_space() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 200, "height": 100, "frameRate": 30, "duration": 2})).unwrap();
    let shape = s.execute("layer.newShape", json!({"kind": "rect", "size": [180, 80]})).unwrap()["layer"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({"color": "#0000ff", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let point = |s: &Session, lid: u64, fx: u64, id: &str| -> KV {
        let l = layer(s, lid);
        let g = l.effects().unwrap().groups().find(|g| g.uid == fx).unwrap();
        g.props().find(|p| p.match_id == id || (id.is_empty() && p.ui == effectcraft_project::ParamUi::Point)).unwrap().value.clone()
    };
    let set = |s: &mut Session, lid: u64, fx: u64, id: &str, v: [f64; 2]| {
        let l = layer(s, lid);
        let g = l.effects().unwrap().groups().find(|g| g.uid == fx).unwrap();
        let uid = g.props().find(|p| p.match_id == id || (id.is_empty() && p.ui == effectcraft_project::ParamUi::Point)).unwrap().uid;
        s.execute("prop.set", json!({"layer": lid, "prop": uid, "value": v})).unwrap();
    };
    let twirl = apply(&mut s, shape, "Twirl");
    set(&mut s, shape, twirl, "center", [-30.0, -10.0]);
    let bulge = apply(&mut s, shape, "Bulge");
    let control = apply(&mut s, shape, "Point Control");
    set(&mut s, shape, control, "", [5.0, 5.0]);
    let on_solid = apply(&mut s, solid, "Twirl");
    set(&mut s, solid, on_solid, "center", [30.0, 20.0]);
    let adjust = s.execute("layer.newShape", json!({"kind": "rect", "size": [50, 50]})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.setSwitch", json!({"layers": [adjust], "switch": "adjustment", "value": true})).unwrap();
    let on_adjust = apply(&mut s, adjust, "Twirl");
    set(&mut s, adjust, on_adjust, "center", [40.0, 30.0]);
    assert_eq!(point(&s, shape, bulge, "center"), KV::Vec2([100.0, 50.0]), "Bulge's default centre");

    let old = s.project.to_file_json().unwrap().replacen("\"schema\": 2", "\"schema\": 1", 1);
    assert!(old.contains("\"schema\": 1"));
    let dir = std::env::temp_dir().join(format!("ec-effect-space-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, old).unwrap();
    let mut s2 = Session::default();
    s2.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(s2.project.schema, effectcraft_project::SCHEMA_VERSION);
    let check = |s2: &Session| {
        assert_eq!(point(s2, shape, twirl, "center"), KV::Vec2([70.0, 40.0]), "a set point keeps its place in the comp");
        assert_eq!(point(s2, shape, bulge, "center"), KV::Vec2([100.0, 50.0]), "a default point becomes the layer centre");
        assert_eq!(point(s2, shape, control, ""), KV::Vec2([5.0, 5.0]), "expression controls keep their values");
        assert_eq!(point(s2, solid, on_solid, "center"), KV::Vec2([30.0, 20.0]), "a solid's effect space is its layer space");
        assert_eq!(point(s2, adjust, on_adjust, "center"), KV::Vec2([40.0, 30.0]), "an adjustment layer's effects see the comp");
    };
    check(&s2);
    // Saved as schema 2 and opened again: nothing moves twice.
    s2.execute("file.save", json!({"path": path})).unwrap();
    let mut s3 = Session::default();
    s3.execute("file.open", json!({"path": path})).unwrap();
    let _ = std::fs::remove_dir_all(dir);
    check(&s3);
}

/// #410: `effect.warning` reports what an OCIO effect can't render as asked (Effect Controls
/// shows it under the effect's name): a transform of its custom config that passes colours
/// through, a colour space the config lacks or a config it can't read; nothing for the
/// built-in config.
#[test]
fn ocio_effect_warns_about_unsupported_config_transforms() {
    let (mut s, a, _) = comp_with_two_solids();
    let fx = apply(&mut s, a, "OCIO Color Space Transform");
    let warning = |s: &mut Session| s.execute("effect.warning", json!({"layer": a, "effect": fx})).unwrap()["warning"].clone();
    assert!(warning(&mut s).is_null(), "built-in config");
    let set = |s: &mut Session, param: &str, value: serde_json::Value| {
        let uid = layer(s, a).effects().unwrap().groups().next().unwrap().get(param).unwrap().uid;
        s.execute("prop.set", json!({"layer": a, "prop": uid, "value": value})).unwrap();
    };
    let config = "ocio_profile_version: 2\ncolorspaces:\n  - !<ColorSpace>\n    name: lin\n  - !<ColorSpace>\n    name: graded\n    from_scene_reference: !<GradingToneTransform> {}\n";
    set(&mut s, "config", json!(1));
    set(&mut s, "configFile", json!(config));
    set(&mut s, "sourceName", json!("lin"));
    set(&mut s, "destinationName", json!("graded"));
    let w = warning(&mut s);
    assert!(w.as_str().is_some_and(|w| w.contains("GradingToneTransform")), "{w}");
    set(&mut s, "destinationName", json!("lin"));
    assert!(warning(&mut s).is_null());
    set(&mut s, "destinationName", json!("nowhere"));
    assert!(warning(&mut s).as_str().is_some_and(|w| w.contains("no color space `nowhere`")));
    set(&mut s, "configFile", json!("/no/such/config.ocio"));
    assert!(warning(&mut s).as_str().is_some_and(|w| w.starts_with("Can't read the OCIO config")));
}

#[test]
fn effect_colour_labels_set_clear_undo_and_round_trip() {
    let (mut s, a, _) = comp_with_two_solids();
    let blur = apply(&mut s, a, "Gaussian Blur");
    let label = |s: &Session| layer(s, a).effects().unwrap().groups().next().unwrap().label;
    assert_eq!(label(&s), None);
    s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": "Blue"})).unwrap();
    assert_eq!(label(&s), Some(effectcraft_color::Label::Blue));
    // Duplicates keep the label.
    s.execute("effect.duplicate", json!({"layer": a, "effect": blur})).unwrap();
    assert_eq!(layer(&s, a).effects().unwrap().groups().filter(|g| g.label.is_some()).count(), 2);
    s.undo();
    // It saves with the project and survives a reload.
    let json = s.project.to_file_json().unwrap();
    let back = effectcraft_project::Project::from_json(&json).unwrap();
    let l = back.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().clone();
    assert_eq!(l.effects().unwrap().groups().next().unwrap().label, Some(effectcraft_color::Label::Blue));
    // None (or index 0) clears it; the change undoes.
    s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": 0})).unwrap();
    assert_eq!(label(&s), None);
    s.undo();
    assert_eq!(label(&s), Some(effectcraft_color::Label::Blue));
    assert!(s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": "Chartreuse"})).is_err());
    assert!(s.execute("effect.setLabel", json!({"layer": a, "effect": blur, "label": 99})).is_err());
    // A project saved before labels existed has none.
    let old = json.replace("\"label\":\"Blue\"", "\"x\":0").replace("\"label\": \"Blue\"", "\"x\": 0");
    assert_ne!(old, json, "the replace removed the effect's label");
    let old = effectcraft_project::Project::from_json(&old).unwrap();
    let l = old.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(a)).unwrap().clone();
    assert_eq!(l.effects().unwrap().groups().next().unwrap().label, None);
}
