use crate::{Event, Session};
use effectcraft_project::{LayerId, Project};
use serde_json::json;

/// The frontend notification must remain separate from document history and saved mask values.
#[test]
fn mask_reveal_creation_notifies_once_and_keeps_one_undo_step() {
    for (command, params) in [("layer.addMask", json!({})), ("mask.new", json!({"vertices": [[1, 2], [10, 2], [10, 12]], "closed": true}))] {
        let mut s = Session::default();
        s.execute("comp.new", json!({"width": 64, "height": 36})).unwrap();
        let layer = s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap();
        let comp = s.active_comp_id().unwrap();
        s.drain_events();
        let before = s.project.to_json();
        let mask = s.execute(command, params).unwrap()["mask"].as_u64().unwrap();
        let notices: Vec<_> = s
            .drain_events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Frontend { command, params } if command == "timeline.revealMask" => Some(params),
                _ => None,
            })
            .collect();
        assert_eq!(notices, [json!({"comp": comp.0, "layer": layer, "mask": mask, "projectEpoch": s.script_ui.project_epoch})]);
        s.execute("prop.set", json!({"layer": layer, "path": "masks/#1/feather", "value": [4, 6]})).unwrap();
        let reopened = Project::from_json(&s.project.to_json()).unwrap();
        assert_eq!(reopened.comp(comp).unwrap().layer(LayerId(layer)).unwrap().props.prop("masks/#1/feather").unwrap().value.components(), [4.0, 6.0]);
        assert!(s.undo());
        assert!(s.undo());
        assert_eq!(s.project.to_json(), before);
        assert!(s.redo());
        assert_eq!(s.active_comp().unwrap().layer(LayerId(layer)).unwrap().masks().unwrap().groups().count(), 1);
        s.drain_events();
        s.execute("mask.addVertex", json!({"layer": layer, "mask": mask, "point": [5, 8]})).unwrap();
        assert!(!s.drain_events().iter().any(|e| matches!(e, Event::Frontend { command, .. } if command == "timeline.revealMask")));
    }
}

/// Invalid tool inputs must not leave either a partial mask or a queued reveal behind.
#[test]
fn mask_reveal_failed_creation_emits_no_notice_and_keeps_document() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 64, "height": 36})).unwrap();
    s.execute("layer.newSolid", json!({})).unwrap();
    s.drain_events();
    let before = s.project.to_json();
    for (command, params) in [("layer.addMask", json!({"shape": "invalid"})), ("mask.new", json!({"vertices": []}))] {
        assert!(s.execute(command, params).is_err());
        assert_eq!(s.project.to_json(), before);
        assert!(s.drain_events().is_empty());
    }
}

/// Mask creation must use the same notification contract for still and video footage.
#[test]
fn mask_reveal_image_and_video_layers_keep_their_source_and_mask() {
    use effectcraft_project::{Footage, FootageKind, ItemKind, LayerSource};
    for kind in [FootageKind::Still, FootageKind::Video] {
        for command in ["layer.addMask", "mask.new"] {
            let mut s = Session::default();
            s.execute("comp.new", json!({"width": 64, "height": 36})).unwrap();
            let layer = LayerId(s.execute("layer.newSolid", json!({})).unwrap()["layer"].as_u64().unwrap());
            let comp = s.active_comp_id().unwrap();
            let item = s
                .edit("Synthetic footage", None, |project, _| {
                    let item =
                        project.add_item("Clip", Default::default(), None, ItemKind::Footage(Footage { kind, width: 64, height: 36, ..Default::default() }));
                    project.comp_mut(comp).unwrap().layer_mut(layer).unwrap().source = LayerSource::Footage { item };
                    Ok(item)
                })
                .unwrap();
            s.drain_events();
            let mask = s.execute(command, json!({"vertices": [[2, 3], [20, 3], [20, 12]]})).unwrap()["mask"].as_u64().unwrap();
            let layer = s.active_comp().unwrap().layer(layer).unwrap();
            assert_eq!(layer.source, LayerSource::Footage { item });
            assert_eq!(layer.masks().unwrap().groups().map(|g| g.uid).collect::<Vec<_>>(), [mask]);
            assert_eq!(s.drain_events().iter().filter(|e| matches!(e, Event::Frontend { command, .. } if command == "timeline.revealMask")).count(), 1);
        }
    }
}
