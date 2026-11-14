//! #2021: whole animators travel through Edit Copy/Paste, not the layer clipboard.
use crate::Session;
use effectcraft_project::{LayerId, Project, PropGroup};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

fn setup() -> (Session, u64, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width":320,"height":180,"duration":10})).unwrap();
    let a = s.execute("layer.newText", json!({"name":"A","text":"Two words","position":[160,90],"size":32})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newText", json!({"name":"B","text":"Two words","position":[160,90],"size":32})).unwrap()["layer"].as_u64().unwrap();
    let animator = s.execute("layer.applyTextPreset", json!({"layer":a,"preset":"fadeUpCharacters"})).unwrap()["animators"][0].as_u64().unwrap();
    (s, a, b, animator)
}
fn animators(s: &Session, layer: u64) -> &PropGroup {
    s.active_comp().unwrap().layer(LayerId(layer)).unwrap().props.group("text/animators").unwrap()
}
fn copy(s: &mut Session, layer: u64, uid: u64) -> Value {
    s.execute("prop.select", json!({"layer":layer,"prop":uid})).unwrap();
    s.execute("edit.copy", json!({})).unwrap()
}
fn payload(g: &PropGroup) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(o) => {
                o.remove("uid");
                for value in o.values_mut() {
                    strip(value);
                }
            }
            Value::Array(a) => {
                for value in a {
                    strip(value);
                }
            }
            _ => {}
        }
    }
    let mut v = serde_json::to_value(g).unwrap();
    strip(&mut v);
    v
}

#[test]
fn animator_clipboard_preserves_groups_and_renders_without_copying_layers() {
    let (mut s, a, b, animator) = setup();
    s.execute("text.addSelector", json!({"layer":a,"animator":animator,"kind":"wiggly"})).unwrap();
    s.execute("prop.setExpression", json!({"layer":a,"path":"text/animators/#1/properties/opacity","expression":"value + 1"})).unwrap();
    let original = animators(&s, a).groups().next().unwrap().clone();
    let selector = original.group("selectors/#1").unwrap().uid;
    s.execute("prop.select", json!({"layer":a,"prop":animator})).unwrap();
    s.execute("prop.select", json!({"layer":a,"prop":selector,"add":true})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap()["animators"], 1, "copy the animator once, including its selected selector");
    assert!(s.state.clipboard.is_empty());
    assert!(s.is_enabled("edit.paste"));
    s.execute("layer.select", json!({"layers":[b]})).unwrap();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let cid = s.active_comp_id().unwrap();
    let time = Tick::from_seconds_f64(0.75);
    let plain = s.render(cid, time, Default::default());
    let pasted = s.execute("edit.paste", json!({})).unwrap()["animators"][0].as_u64().unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 2);
    assert_eq!(s.history.undo.len(), undo + 1);
    let target = animators(&s, b).groups().next().unwrap();
    assert_ne!(pasted, animator);
    assert_eq!(payload(target), payload(&original));
    assert_eq!(s.state.selected_props, [(LayerId(b), pasted)]);
    let mut ids = vec![original.uid];
    original.walk("", &mut |_, p| ids.push(p.uid));
    assert!(!ids.contains(&target.uid));
    target.walk("", &mut |_, p| assert!(!ids.contains(&p.uid)));
    assert_eq!(animators(&s, a).groups().next().unwrap(), &original);
    let applied = s.project.clone();
    let frame = s.render(cid, time, Default::default());
    assert_ne!(frame.data, plain.data);
    let loaded = Project::from_json(&s.project.to_json()).unwrap();
    assert_eq!(*s.project, loaded);
    let round_trip = effectcraft_render::Renderer::new(&loaded, &effectcraft_render::NoFootage, Default::default()).comp_frame(cid, time);
    assert_eq!(round_trip.data, frame.data);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project, before);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, applied);
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(animators(&s, b).groups().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Fade Up", "Fade Up 2"]);
    s.execute("prop.set", json!({"layer":b,"path":"text/animators/#1/properties/opacity","value":30})).unwrap();
    assert_eq!(animators(&s, a).groups().next().unwrap(), &original, "editing a pasted animator does not change its source");
}

#[test]
fn animator_clipboard_skips_locked_and_non_text_targets_atomically() {
    let (mut s, a, b, uid) = setup();
    let c = s.execute("layer.newText", json!({"name":"C"})).unwrap()["layer"].as_u64().unwrap();
    let d = s.execute("layer.newText", json!({"name":"D"})).unwrap()["layer"].as_u64().unwrap();
    let solid = s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap();
    assert_eq!(copy(&mut s, a, uid)["animators"], 1);
    s.execute("layer.setSwitch", json!({"layers":[c],"switch":"lock","value":true})).unwrap();
    s.execute("layer.select", json!({"layers":[b,c,d,solid]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 5);
    let first = animators(&s, b).groups().next().unwrap();
    let second = animators(&s, d).groups().next().unwrap();
    assert_ne!(first.uid, second.uid);
    assert_eq!(payload(first), payload(second));
    assert_eq!(animators(&s, b).groups().count(), 1);
    assert_eq!(animators(&s, c).groups().count(), 0);
    s.execute("layer.select", json!({"layers":[c,solid]})).unwrap();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    assert!(s.execute("edit.paste", json!({})).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn animator_clipboard_uses_comp_offsets_and_retimes_ease_for_stretched_layers() {
    for rate in [FrameRate::FPS_24, FrameRate::FPS_29_97, FrameRate::FPS_60] {
        for source_stretch in [200.0, -200.0] {
            for target_stretch in [50.0, -50.0] {
                let (mut s, a, b, uid) = setup();
                s.execute("comp.settings", json!({"frameRate":rate.as_f64()})).unwrap();
                s.execute("layer.timing", json!({"layers":[a],"start":1})).unwrap();
                s.execute("layer.timeStretch", json!({"layers":[a],"percent":source_stretch})).unwrap();
                s.execute("layer.timing", json!({"layers":[b],"start":-0.25})).unwrap();
                s.execute("layer.timeStretch", json!({"layers":[b],"percent":target_stretch})).unwrap();
                let cid = s.active_comp_id().unwrap();
                s.edit("Test ease", None, |p, _| {
                    let keys =
                        &mut p.comp_mut(cid).unwrap().layer_mut(LayerId(a)).unwrap().props.prop_mut("text/animators/#1/selectors/#1/offset").unwrap().keys;
                    for key in keys {
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed = 4.0;
                        }
                    }
                    Ok(())
                })
                .unwrap();
                assert_eq!(copy(&mut s, a, uid)["animators"], 1);
                let at = rate.tick_of(120);
                s.execute("time.set", json!({"time":at.seconds()})).unwrap();
                s.execute("layer.select", json!({"layers":[b]})).unwrap();
                s.execute("edit.paste", json!({})).unwrap();
                let l = s.active_comp().unwrap().layer(LayerId(b)).unwrap();
                let keys = &l.props.prop("text/animators/#1/selectors/#1/offset").unwrap().keys;
                let mut expected = [l.layer_time(at), l.layer_time(at + Tick::from_seconds_f64(3.0))];
                expected.sort();
                assert_eq!(keys.iter().map(|k| k.time).collect::<Vec<_>>(), expected);
                for key in keys {
                    for ease in key.in_ease.iter().chain(&key.out_ease) {
                        assert!((ease.speed - 4.0 * target_stretch / source_stretch).abs() < 1e-9);
                    }
                }
            }
        }
    }
}

#[test]
fn animator_clipboard_is_replaced_by_an_explicit_layer_copy() {
    let (mut s, a, b, uid) = setup();
    assert_eq!(copy(&mut s, a, uid)["animators"], 1);
    s.execute("edit.copy", json!({"layers":[a]})).unwrap();
    s.execute("layer.select", json!({"layers":[b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 3);
    assert_eq!(animators(&s, b).groups().count(), 0);
}

#[test]
fn animator_clipboard_copies_multiple_groups_as_an_independent_snapshot() {
    let (mut s, a, b, first) = setup();
    let second = s.execute("layer.addTextAnimator", json!({"layer":a,"name":"Second","property":"tracking"})).unwrap()["animator"].as_u64().unwrap();
    s.execute("prop.select", json!({"layer":a,"prop":first})).unwrap();
    s.execute("prop.select", json!({"layer":a,"prop":second,"add":true})).unwrap();
    assert_eq!(s.execute("edit.copy", json!({})).unwrap()["animators"], 2);
    s.execute("prop.set", json!({"layer":a,"path":"text/animators/#1/properties/opacity","value":42})).unwrap();
    s.execute("layer.select", json!({"layers":[b]})).unwrap();
    s.execute("edit.paste", json!({})).unwrap();
    assert_eq!(animators(&s, b).groups().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Fade Up", "Second"]);
    assert_eq!(animators(&s, b).prop("#1/properties/opacity").unwrap().value.as_f64(), 0.0);
    s.execute("prop.select", json!({"layer":a,"prop":first})).unwrap();
    s.execute("prop.select", json!({"layer":a,"prop":second,"add":true})).unwrap();
    s.execute("edit.cut", json!({})).unwrap();
    assert_eq!(s.active_comp().unwrap().layers.len(), 2);
    assert_eq!(animators(&s, a).groups().count(), 0);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(animators(&s, a).groups().count(), 2);
}

#[test]
fn animator_clipboard_is_replaced_by_an_effect_copy() {
    let (mut s, a, b, uid) = setup();
    let fx = s.execute("effect.apply", json!({"layer":a,"effect":"Gaussian Blur"})).unwrap()["effects"][0].as_u64().unwrap();
    assert_eq!(copy(&mut s, a, uid)["animators"], 1);
    s.execute("effect.copy", json!({"layer":a,"effect":fx})).unwrap();
    s.execute("layer.select", json!({"layers":[b]})).unwrap();
    assert_eq!(s.execute("edit.paste", json!({})).unwrap()["effects"].as_array().unwrap().len(), 1);
    assert_eq!(animators(&s, b).groups().count(), 0);
}

#[test]
fn animator_clipboard_refuses_exhausted_ids_without_changing_the_project() {
    let (mut s, a, b, uid) = setup();
    assert_eq!(copy(&mut s, a, uid)["animators"], 1);
    s.execute("layer.select", json!({"layers":[b]})).unwrap();
    std::sync::Arc::make_mut(&mut s.project).next_id = u64::MAX;
    let before = s.project.clone();
    let undo = s.history.undo.len();
    assert!(s.execute("edit.paste", json!({})).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn animator_clipboard_rejects_time_overflow_without_a_partial_paste() {
    let (mut s, a, b, uid) = setup();
    let c = s.execute("layer.newText", json!({"name":"C"})).unwrap()["layer"].as_u64().unwrap();
    copy(&mut s, a, uid);
    // The first destination is valid; the second cannot represent the final key.
    let cid = s.active_comp_id().unwrap();
    s.edit("Test extreme timing", None, |p, _| {
        p.comp_mut(cid).unwrap().layer_mut(LayerId(c)).unwrap().start_time = Tick::MIN;
        Ok(())
    })
    .unwrap();
    s.execute("layer.select", json!({"layers":[b,c]})).unwrap();
    let before = s.project.clone();
    let state = s.state.selected_props.clone();
    let undo = s.history.undo.len();
    assert!(s.execute("edit.paste", json!({})).is_err());
    assert_eq!(s.project, before);
    assert_eq!(s.state.selected_props, state);
    assert_eq!(s.history.undo.len(), undo);
    assert!(animators(&s, b).groups().next().is_none());
}
