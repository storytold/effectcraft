//! Source replacement must add audio controls without replacing the layer or its animation.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value};
use effectcraft_project::{Comp, Footage, FootageKind, ItemId, ItemKind, Layer, LayerId, build};
use effectcraft_time::{FrameRate, Tick};
use serde_json::json;

use crate::Session;

fn fixture() -> (Session, ItemId, Vec<LayerId>, [ItemId; 4]) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width":200,"height":100,"duration":3})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let layers = (0..2).map(|_| LayerId(s.execute("layer.newSolid", json!({"width":200,"height":100})).unwrap()["layer"].as_u64().unwrap())).collect();
    let items = s
        .edit("Synthetic audio sources", None, |p, _| {
            let audio = p.add_item(
                "Audio",
                Label::SeaFoam,
                None,
                ItemKind::Footage(Footage {
                    kind: FootageKind::Audio,
                    has_audio: true,
                    has_video: false,
                    width: 0,
                    height: 0,
                    duration: Tick::from_seconds_f64(3.0),
                    ..Default::default()
                }),
            );
            let video = p.add_item(
                "Video with audio",
                Label::SeaFoam,
                None,
                ItemKind::Footage(Footage { has_audio: true, width: 200, height: 100, duration: Tick::from_seconds_f64(3.0), ..Default::default() }),
            );
            let silent =
                p.add_item("Silent", Label::SeaFoam, None, ItemKind::Footage(Footage { has_audio: false, width: 200, height: 100, ..Default::default() }));
            let comp = p.add_item("Precomp", Label::SeaFoam, None, ItemKind::Comp(Comp::new(200, 100, FrameRate::FPS_30, Tick::from_seconds_f64(3.0)).into()));
            Ok([audio, video, silent, comp])
        })
        .unwrap();
    (s, cid, layers, items)
}

fn layer(s: &Session, id: LayerId) -> &Layer {
    s.active_comp().unwrap().layer(id).unwrap()
}

#[test]
fn replace_source_adds_audio_controls_like_new_layers_and_restores_all_on_undo() {
    for source_index in [0, 1, 3] {
        let (mut s, cid, ids, items) = fixture();
        let item = items[source_index];
        let before = s.project.clone();
        assert!(ids.iter().all(|id| layer(&s, *id).props.sub("audio").is_none()));
        s.execute("layer.replaceSource", json!({"layers":ids,"item":item.0})).unwrap();
        let after = s.project.clone();
        let mut uids = std::collections::BTreeSet::new();
        for id in &ids {
            let l = layer(&s, *id);
            let audio = l.props.sub("audio").unwrap();
            assert_eq!(audio.get("levels").unwrap().value, Value::Vec2([0.0, 0.0]));
            assert!(uids.insert(audio.uid));
            assert!(uids.insert(audio.get("levels").unwrap().uid));
            assert!(audio.uid >= before.next_id);
            let old = before.comp(cid).unwrap().layer(*id).unwrap();
            let mut restored = l.clone();
            restored.source = old.source.clone();
            restored.props.children.retain(|n| n.match_id() != "audio");
            assert_eq!(&restored, old, "existing settings and properties must survive exactly");
            let mut p = (*before).clone();
            let fresh = build::layer(&mut p, before.comp(cid).unwrap(), "Fresh", l.source.clone(), (200, 100), None);
            assert_eq!(audio.get("levels").unwrap().ui, fresh.props.sub("audio").unwrap().get("levels").unwrap().ui);
        }
        assert_eq!(after.next_id, before.next_id + 4);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(*s.project, *before);
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(*s.project, *after);
    }
}

#[test]
fn replace_source_preserves_audio_ids_animation_expressions_and_mute_through_silent_sources() {
    let (mut s, cid, ids, items) = fixture();
    let id = ids[0];
    s.execute("layer.replaceSource", json!({"layers":[id],"item":items[0].0})).unwrap();
    s.edit("Existing sound settings", None, |p, _| {
        let l = p.comp_mut(cid).unwrap().layer_mut(id).unwrap();
        l.switches.audio = false;
        let levels = l.props.prop_mut("audio/levels").unwrap();
        levels.value = Value::Vec2([-12.0, -6.0]);
        levels.keys =
            vec![Keyframe::new(Tick::ZERO, Value::Vec2([-12.0, -6.0])).eased(), Keyframe::new(Tick::from_seconds_f64(2.0), Value::Vec2([-24.0, -18.0]))];
        levels.expr = Some(effectcraft_project::Expression { text: "value".into(), enabled: true });
        Ok(())
    })
    .unwrap();
    let expected = layer(&s, id).props.sub("audio").unwrap().clone();
    let solid = layer(&s, ids[1]).source.item().unwrap();
    let next = s.project.next_id;
    // Sound -> silent footage -> solid -> precomp -> sound, including repeated replacements.
    for item in [items[2], solid, items[3], items[1], items[0], items[0]] {
        let before = s.project.clone();
        s.execute("layer.replaceSource", json!({"layers":[id],"item":item.0})).unwrap();
        assert_eq!(layer(&s, id).props.sub("audio"), Some(&expected));
        assert!(!layer(&s, id).switches.audio);
        assert_eq!(s.project.next_id, next);
        let after = s.project.clone();
        if !after.same_content(&before) {
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(*s.project, *before);
            s.execute("edit.redo", json!({})).unwrap();
            assert_eq!(*s.project, *after);
        }
    }
}

#[test]
fn replace_source_audio_controls_respect_locked_layers_and_uid_exhaustion() {
    let (mut s, cid, ids, items) = fixture();
    s.execute("layer.setSwitch", json!({"layers":[ids[1]],"switch":"locked","value":true})).unwrap();
    let locked = layer(&s, ids[1]).clone();
    s.execute("layer.replaceSource", json!({"layers":ids,"item":items[0].0})).unwrap();
    assert_eq!(layer(&s, ids[1]), &locked);
    assert!(layer(&s, ids[0]).props.sub("audio").is_some());
    let (mut s, _, ids, items) = fixture();
    s.edit("Exhausted IDs", None, |p, _| {
        p.next_id = u64::MAX - 2;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    assert!(s.execute("layer.replaceSource", json!({"layers":ids,"item":items[0].0})).is_err());
    assert_eq!(*s.project, *before, "partial creation and source changes must roll back");
    assert!(s.project.comp(cid).is_some());
}

#[test]
fn replace_source_audio_levels_and_mute_control_the_rendered_mix() {
    struct Samples;
    impl effectcraft_render::FootageSource for Samples {
        fn frame(&self, _: ItemId, _: &Footage, _: Tick) -> Option<std::sync::Arc<effectcraft_raster::Image>> {
            None
        }
        fn audio(&self, _: ItemId, _: &Footage, _: Tick, frames: usize, _: u32) -> Option<Vec<f32>> {
            Some((0..frames).flat_map(|_| [0.25, -0.25]).collect())
        }
    }
    let (mut s, cid, ids, items) = fixture();
    let id = ids[0];
    s.execute("layer.replaceSource", json!({"layers":[id],"item":items[0].0})).unwrap();
    s.execute("prop.set", json!({"layer":id,"path":"audio/levels","value":[-6.020599913279624,-96.0]})).unwrap();
    let mix = |s: &Session| effectcraft_render::audio::mix_comp(&s.project, &Samples, None, cid, Tick::ZERO, 16, 1000);
    for pair in mix(&s).as_chunks::<2>().0 {
        assert!((pair[0] - 0.125).abs() < 1e-5);
        assert_eq!(pair[1], 0.0);
    }
    s.execute("layer.replaceSource", json!({"layers":[id],"item":items[2].0})).unwrap();
    assert!(mix(&s).iter().all(|v| *v == 0.0));
    s.execute("layer.replaceSource", json!({"layers":[id],"item":items[0].0})).unwrap();
    assert!((mix(&s)[0] - 0.125).abs() < 1e-5);
    s.execute("layer.setSwitch", json!({"layers":[id],"switch":"audio","value":false})).unwrap();
    assert!(mix(&s).iter().all(|v| *v == 0.0));
}

#[test]
fn replace_source_does_not_add_audio_to_silent_sources() {
    let (mut s, _, ids, items) = fixture();
    let next = s.project.next_id;
    s.execute("layer.replaceSource", json!({"layers":ids,"item":items[2].0})).unwrap();
    assert!(ids.iter().all(|id| layer(&s, *id).props.sub("audio").is_none()));
    assert_eq!(s.project.next_id, next);
}

#[test]
fn replace_source_resolves_names_and_rejects_bad_explicit_items_without_fallback() {
    let (mut s, _, ids, items) = fixture();
    s.state.selected_layers.clear();
    s.state.project_selection = vec![items[0]];
    s.execute("layer.replaceSource", json!({"layers":[ids[0]],"item":"Video with audio"})).unwrap();
    assert_eq!(layer(&s, ids[0]).source.item(), Some(items[1]));
    for item in [json!(-1), json!(1.5), json!(true), json!(null), json!("does not exist")] {
        let before = s.project.clone();
        assert!(s.execute("layer.replaceSource", json!({"layers":[ids[0]],"item":item})).is_err());
        assert_eq!(*s.project, *before);
    }
    let before = s.project.clone();
    assert!(s.execute("layer.replaceSource", json!({"layers":[ids[0]],"item":items[0].0,"fixExpressions":true})).is_err());
    assert_eq!(*s.project, *before);
}

#[test]
fn replace_source_preview_and_enablement_match_atomic_target_validation() {
    let (mut s, cid, ids, items) = fixture();
    s.state.selected_layers = ids.clone();
    s.state.project_selection.clear();
    assert!(!s.is_enabled("layer.replaceSource"));
    s.state.project_selection = vec![items[0]];
    assert!(s.is_enabled("layer.replaceSource"));
    assert!(crate::commands::can_replace_source(&s.project, cid, items[0], &ids));
    assert!(!crate::commands::can_replace_source(&s.project, cid, cid, &ids));
    let null = s.execute("layer.newNull", json!({})).unwrap()["layer"].as_u64().unwrap();
    let mixed = vec![ids[0], LayerId(null)];
    s.state.selected_layers = mixed.clone();
    assert!(!s.is_enabled("layer.replaceSource"));
    assert!(!crate::commands::can_replace_source(&s.project, cid, items[0], &mixed));
    let before = s.project.clone();
    assert!(s.execute("layer.replaceSource", json!({"layers":mixed,"item":items[0].0})).is_err());
    assert_eq!(*s.project, *before);
}
