use std::sync::{Arc, Mutex};

use effectcraft_project::LayerId;
use effectcraft_render::{RenderOpts, Renderer};
use serde_json::json;

use crate::{Session, profiler::ProfileReport};

fn fixture() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Profile", "width": 16, "height": 16, "frameRate": 25, "duration": 2})).unwrap();
    let layer = s.execute("layer.newSolid", json!({"name": "Same name", "color": "#4090c0"})).unwrap()["layer"].as_u64().unwrap();
    for _ in 0..3 {
        s.execute("effect.apply", json!({"layers": [layer], "effect": "ec.blur.gaussian"})).unwrap();
    }
    let cid = s.active_comp_id().unwrap();
    s.edit("Disable middle effect", None, |project, _| {
        let fx = project.comp_mut(cid).unwrap().layer_mut(LayerId(layer)).unwrap().props.sub_mut("effects").unwrap();
        let effectcraft_project::Node::Group(group) = fx.children.get_mut(1).unwrap() else { panic!("effect group") };
        group.enabled = false;
        Ok(())
    })
    .unwrap();
    let adjustment = s.execute("layer.newAdjustment", json!({"name": "Same name"})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [adjustment], "effect": "ec.channel.invert"})).unwrap();
    s.edit("Duplicate layer names", None, |project, _| {
        project.comp_mut(cid).unwrap().layer_mut(LayerId(adjustment)).unwrap().name = "Same name".into();
        Ok(())
    })
    .unwrap();
    (s, layer, adjustment)
}

#[test]
fn capture_identifies_effect_instances_and_adjustments_without_editing() {
    let (mut s, layer, adjustment) = fixture();
    let project = s.project.clone();
    let state = serde_json::to_value(&s.state).unwrap();
    let history = (s.history.undo.len(), s.history.redo.len());
    let revision = s.revision;
    let report: ProfileReport = serde_json::from_value(s.execute_checked("comp.profile", json!({})).unwrap()).unwrap();
    assert_eq!(report.backend, "cpu");
    assert!(!report.layer_cache);
    assert_eq!((report.width, report.height), (16, 16));
    assert!(report.total_ms.is_finite() && report.total_ms >= 0.0);
    let ordinary = report.layers.iter().find(|row| row.layer_id == layer).unwrap();
    assert_eq!(ordinary.effect_instances.iter().map(|e| e.index).collect::<Vec<_>>(), vec![0, 2]);
    assert_ne!(ordinary.effect_instances[0].uid, ordinary.effect_instances[1].uid);
    let adjusted = report.layers.iter().find(|row| row.layer_id == adjustment).unwrap();
    assert_eq!(ordinary.layer, adjusted.layer, "duplicate names do not identify layers");
    assert_eq!(adjusted.effect_instances.len(), 1);
    for row in &report.layers {
        assert!(row.process_ms.is_finite() && row.process_ms >= 0.0);
        for effect in &row.effect_instances {
            assert!(effect.ms.is_finite() && effect.ms >= 0.0);
        }
    }
    assert!(Arc::ptr_eq(&project, &s.project));
    assert_eq!(state, serde_json::to_value(&s.state).unwrap());
    assert_eq!(history, (s.history.undo.len(), s.history.redo.len()));
    assert_eq!(revision, s.revision);
}

#[test]
fn profiled_composition_pixels_equal_unprofiled_pixels_and_master_switch_is_honored() {
    let (mut s, layer, _) = fixture();
    let cid = s.active_comp_id().unwrap();
    let opts = s.render_opts(RenderOpts::default());
    let mut plain = Renderer::new(&s.project, s.footage.as_ref(), opts);
    plain.expr = s.expr.as_deref();
    let expected = plain.comp_frame(cid, s.time());
    let sink = Mutex::new(Vec::new());
    plain.profile = Some(&sink);
    let actual = plain.comp_frame(cid, s.time());
    assert_eq!(expected.data, actual.data);
    assert_eq!((expected.width, expected.height), (actual.width, actual.height));
    assert!(!sink.lock().unwrap().is_empty());
    s.edit("Disable layer effects", None, |project, _| {
        project.comp_mut(cid).unwrap().layer_mut(LayerId(layer)).unwrap().switches.effects = false;
        Ok(())
    })
    .unwrap();
    let report = s.profile_job(&json!({})).unwrap()().unwrap();
    assert!(report.layers.iter().find(|row| row.layer_id == layer).unwrap().effect_instances.is_empty());
}

#[test]
fn invalid_capture_parameters_are_errors_and_other_comps_use_their_own_time() {
    let (mut s, _, _) = fixture();
    for params in [
        json!({"scale": 0}),
        json!({"scale": 2}),
        json!({"scale": null}),
        json!({"time": "NaN"}),
        json!({"time": 1e100}),
        json!({"frame": 0.5}),
        json!({"frame": i64::MAX}),
        json!({"comp": []}),
        json!({"comp": "missing"}),
        json!({"time": 0, "frame": 0}),
    ] {
        assert!(s.profile_job(&params).is_err(), "accepted {params}");
    }
    let original = s.active_comp_id().unwrap();
    s.set_time(crate::time::Tick::from_seconds_f64(0.4));
    s.execute("comp.new", json!({"name": "Other", "width": 16, "height": 16, "duration": 2})).unwrap();
    let report = s.profile_job(&json!({"comp": original.0})).unwrap()().unwrap();
    assert_eq!(report.time, s.time_of(original));
    assert_eq!(report.time, crate::time::Tick::from_seconds_f64(0.4));
    assert!(Session::default().profile_job(&json!({})).is_err());
}
