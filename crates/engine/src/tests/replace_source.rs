//! Layer ▸ Replace Source (#991, and the `replaceSource` part of #813): Alt-drag and Ctrl+Alt+/
//! replace an AV layer's source while every other layer property stays.

use std::sync::Arc;

use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Solid};
use serde_json::json;

use crate::Session;

fn comp() -> Session {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    s
}

fn layer(s: &Session, id: u64) -> effectcraft_project::Layer {
    s.active_comp().unwrap().layer(effectcraft_project::LayerId(id)).unwrap().clone()
}

fn solid(s: &mut Session, name: &str) -> u64 {
    s.execute("layer.newSolid", json!({"name": name, "color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap()
}

fn add_footage(s: &mut Session, name: &str, kind: FootageKind, width: u32, height: u32, has_audio: bool) -> ItemId {
    let f = Footage { path: format!("/tmp/{name}"), kind, width, height, has_video: kind != FootageKind::Data, has_audio, ..Default::default() };
    Arc::make_mut(&mut s.project).add_item(name, Label::None, None, ItemKind::Footage(f))
}

/// The replacement changes `source` and nothing else: name, label, comment, timing, stretch,
/// switches, blending, parenting, track matte and the whole property tree (keys, expression,
/// effect, mask) stay.
#[test]
fn replace_source_keeps_every_layer_property() {
    let mut s = comp();
    let a = solid(&mut s, "Hero");
    let parent = solid(&mut s, "Parent");
    let matte = solid(&mut s, "Matte");
    s.execute("layer.timing", json!({"layers": [a], "in": 1.0, "out": 5.0})).unwrap();
    s.execute("layer.timeStretch", json!({"layers": [a], "percent": 50})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "preserveTransparency", "value": true})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [a], "switch": "motionBlur", "value": true})).unwrap();
    s.execute("layer.setBlendMode", json!({"layers": [a], "mode": "Multiply"})).unwrap();
    s.execute("layer.setParent", json!({"layers": [a], "parent": parent, "compensate": false})).unwrap();
    s.execute("layer.setTrackMatte", json!({"layer": a, "matte": matte, "kind": "alpha"})).unwrap();
    s.execute("layer.setComment", json!({"layers": [a], "comment": "keep me"})).unwrap();
    s.execute("effect.apply", json!({"layer": a, "effect": "Gaussian Blur"})).unwrap();
    s.execute("mask.new", json!({"layer": a, "vertices": [[10, 10], [90, 10], [90, 90]], "closed": true})).unwrap();
    s.execute("prop.addKey", json!({"layer": a, "path": "transform/opacity", "time": 1.0, "value": 25.0})).unwrap();
    s.execute("prop.addKey", json!({"layer": a, "path": "transform/opacity", "time": 2.0, "value": 75.0})).unwrap();
    s.execute("prop.setExpression", json!({"layer": a, "path": "transform/rotation", "expression": "time * 90"})).unwrap();
    let before = layer(&s, a);
    let item = add_footage(&mut s, "new.png", FootageKind::Still, 120, 80, false);
    s.execute("layer.replaceSource", json!({"layers": [a], "item": item.0})).unwrap();
    let after = layer(&s, a);
    assert_eq!(after.source, LayerSource::Footage { item });
    assert_eq!(after.name, before.name);
    assert_eq!(after.label, before.label);
    assert_eq!(after.comment, before.comment);
    assert_eq!((after.start_time, after.in_point, after.out_point, after.stretch), (before.start_time, before.in_point, before.out_point, before.stretch));
    assert_eq!((after.switches, after.blend_mode, after.preserve_transparency), (before.switches, before.blend_mode, before.preserve_transparency));
    assert_eq!((after.parent, after.track_matte), (before.parent, before.track_matte));
    assert_eq!(after.props, before.props);
    assert_eq!(s.active_comp().unwrap().layers.len(), 3, "no layer is added or removed");
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, a).source, before.source, "one undo step restores the source");
}

/// Composition, footage and solid sources are interchangeable; a precomposition adds its Audio
/// group when the layer has none, and an existing Audio group is kept when the next source has no
/// audio.
#[test]
fn replace_source_crosses_types_and_adds_audio() {
    let mut s = comp();
    let a = solid(&mut s, "Layer");
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 200, "height": 100, "frameRate": 25, "duration": 3})).unwrap()["comp"].as_u64().unwrap();
    let main = s.project.items.values().find(|i| i.name == "Main").unwrap().id;
    s.execute("comp.open", json!({"comp": main})).unwrap();
    let before = layer(&s, a);
    s.execute("layer.replaceSource", json!({"layers": [a], "item": inner})).unwrap();
    let after = layer(&s, a);
    assert_eq!(after.source, LayerSource::Comp { item: ItemId(inner) });
    assert_eq!(after.name, before.name);
    assert!(after.props.sub("audio").is_some(), "a precomposition brings Audio");
    let footage = add_footage(&mut s, "still.png", FootageKind::Still, 64, 32, false);
    s.execute("layer.replaceSource", json!({"layers": [a], "item": footage.0})).unwrap();
    let after = layer(&s, a);
    assert_eq!(after.source, LayerSource::Footage { item: footage });
    assert!(after.props.sub("audio").is_some(), "the Audio group is preserved for the next source");
    let new_solid = Arc::make_mut(&mut s.project).add_item(
        "New Solid",
        Label::None,
        None,
        ItemKind::Solid(Solid { color: [0.0, 1.0, 0.0], width: 10, height: 10, pixel_aspect: 1.0 }),
    );
    s.execute("layer.replaceSource", json!({"layers": [a], "item": new_solid.0})).unwrap();
    assert_eq!(layer(&s, a).source, LayerSource::Solid { item: new_solid });
    assert_eq!(s.active_comp().unwrap().layers.len(), 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer(&s, a).source, LayerSource::Footage { item: footage });
}

/// Folders, data files, 3D models, non-source layers and composition cycles are refused before
/// anything is edited.
#[test]
fn replace_source_refuses_invalid_sources_targets_and_cycles() {
    let mut s = comp();
    let a = solid(&mut s, "Layer");
    let folder = Arc::make_mut(&mut s.project).add_item("Folder", Label::None, None, ItemKind::Folder);
    let data = add_footage(&mut s, "data.txt", FootageKind::Data, 0, 0, false);
    let model = add_footage(&mut s, "model.glb", FootageKind::Model, 10, 10, false);
    for (item, needle) in [(folder, "folder"), (data, "data"), (model, "3D model")] {
        let err = s.execute("layer.replaceSource", json!({"layers": [a], "item": item.0})).unwrap_err().to_string();
        assert!(err.contains(needle), "{err}");
    }
    let footage = add_footage(&mut s, "still.png", FootageKind::Still, 10, 10, false);
    let text = s.execute("layer.newText", json!({"text": "Hello"})).unwrap()["layer"].as_u64().unwrap();
    let err = s.execute("layer.replaceSource", json!({"layers": [text], "item": footage.0})).unwrap_err().to_string();
    assert!(err.contains("no source item"), "{err}");
    let inner = s.execute("comp.new", json!({"name": "Inner", "width": 100, "height": 100, "frameRate": 30, "duration": 2})).unwrap()["comp"].as_u64().unwrap();
    let main = s.project.items.values().find(|i| i.name == "Main").unwrap().id;
    s.execute("comp.open", json!({"comp": main})).unwrap();
    let inner_layer = s.execute("layer.addItem", json!({"item": inner})).unwrap()["layer"].as_u64().unwrap();
    let err = s.execute("layer.replaceSource", json!({"layers": [inner_layer], "item": main.0})).unwrap_err().to_string();
    assert!(err.contains("contain"), "{err}");
}

/// Ctrl+Alt+/ takes the source from the Project panel selection and replaces every selected
/// layer; locked layers are left alone (and an all-locked selection is an error).
#[test]
fn replace_source_uses_the_project_selection_and_skips_locked_layers() {
    let mut s = comp();
    let a = solid(&mut s, "A");
    let b = solid(&mut s, "B");
    let footage = add_footage(&mut s, "still.png", FootageKind::Still, 10, 10, false);
    s.state.project_selection = vec![footage];
    let r = s.execute("layer.replaceSource", json!({"layers": [a, b]})).unwrap();
    assert_eq!(r["item"], json!(footage.0));
    assert!(matches!(layer(&s, a).source, LayerSource::Footage { item } if item == footage));
    assert!(matches!(layer(&s, b).source, LayerSource::Footage { item } if item == footage));
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("layer.setSwitch", json!({"layers": [b], "switch": "lock", "value": true})).unwrap();
    let old_b = layer(&s, b).source;
    let r = s.execute("layer.replaceSource", json!({"layers": [a, b], "item": footage.0})).unwrap();
    assert_eq!(r["layers"], json!([a]));
    assert!(matches!(layer(&s, a).source, LayerSource::Footage { item } if item == footage));
    assert_eq!(layer(&s, b).source, old_b);
    let err = s.execute("layer.replaceSource", json!({"layers": [b], "item": footage.0})).unwrap_err().to_string();
    assert!(err.contains("locked"), "{err}");
}

/// After Effects' Ctrl+Alt+/ default (Cmd+Alt+/ in EffectCraft's shortcut syntax).
#[test]
fn replace_source_is_bound_to_ctrl_alt_slash() {
    let b = crate::shortcuts::bindables(&[]).into_iter().find(|b| b.command == "layer.replaceSource").expect("layer.replaceSource is bindable");
    assert!(b.defaults.iter().any(|d| d == "Cmd+Alt+/"), "{:?}", b.defaults);
}
