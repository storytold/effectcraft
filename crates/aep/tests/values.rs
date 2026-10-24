//! Static transform values with their units, the "absent = default" rule, keyframe records and
//! expressions, checked against the values the corpus was made with (its MANIFEST).

mod common;

use common::{first_comp, fixture, layer_with_id, open};
use effectcraft_aep::{Interpolation, KeyEase, KeyframeKind, Project, PropNode, Property, Transform, ValueSource};

const L: u32 = 0x1b;

/// The transform of the solid `L` (comp-size defaults) in a fixture.
fn transform(name: &str) -> (Transform, (u32, u32)) {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_with_id(&comp, L);
    let ch = comp.header().unwrap();
    let t = layer.transform(u32::from(ch.width), u32::from(ch.height)).unwrap();
    (t, project.layer_source_size(&layer).unwrap())
}

fn approx(a: [f64; 3], b: [f64; 3]) {
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1e-9, "{a:?} != {b:?}");
    }
}

#[test]
fn absent_properties_are_the_defaults() {
    let (t, size) = transform("000-base.aep");
    assert_eq!(size, (1920, 1080));
    // The base stores none of the five: Position is the comp centre, Anchor the layer centre.
    assert_eq!(t.position.value, [960.0, 540.0, 0.0]);
    assert_eq!(t.anchor_fraction.value, [0.5, 0.5, 0.0]);
    assert_eq!(t.anchor_px(1920.0, 1080.0), [960.0, 540.0, 0.0]);
    assert_eq!(t.scale_percent.value, [100.0, 100.0, 100.0]);
    assert_eq!(t.rotation_degrees.value, 0.0);
    assert_eq!(t.opacity_percent.value, 100.0);
    for source in [t.position.source, t.anchor_fraction.source, t.scale_percent.source, t.rotation_degrees.source, t.opacity_percent.source] {
        assert_eq!(source, ValueSource::Default);
    }
}

#[test]
fn one_static_value_per_case() {
    // 040 Position [960,540,0] -> [100,200,0].
    let (t, _) = transform("040-static-position-100-200.aep");
    assert_eq!(t.position.value, [100.0, 200.0, 0.0]);
    assert_eq!(t.position.source, ValueSource::Stored);
    assert_eq!(t.scale_percent.value, [100.0, 100.0, 100.0]);
    assert_eq!(t.scale_percent.source, ValueSource::Default);

    // 041 Scale [100,100,100] -> [50,50,100]: stored as fractions.
    let (t, _) = transform("041-static-scale-50.aep");
    assert_eq!(t.scale_percent.value, [50.0, 50.0, 100.0]);
    assert_eq!(t.scale_percent.source, ValueSource::Stored);
    assert_eq!(t.position.value, [960.0, 540.0, 0.0]);

    // 042 Rotation 0 -> 45 degrees.
    let (t, _) = transform("042-static-rotation-45.aep");
    assert_eq!(t.rotation_degrees.value, 45.0);
    assert_eq!(t.rotation_degrees.source, ValueSource::Stored);

    // 043 Opacity 100 -> 25 percent.
    let (t, _) = transform("043-static-opacity-25.aep");
    assert_eq!(t.opacity_percent.value, 25.0);
    assert_eq!(t.opacity_percent.source, ValueSource::Stored);

    // 044 Anchor Point [960,540,0] -> [10,20,0]: a fraction of the layer size, 10/1920 and 20/1080.
    let (t, (w, h)) = transform("044-static-anchor-10-20.aep");
    assert_eq!(t.anchor_fraction.source, ValueSource::Stored);
    approx(t.anchor_fraction.value, [10.0 / 1920.0, 20.0 / 1080.0, 0.0]);
    approx(t.anchor_px(f64::from(w), f64::from(h)), [10.0, 20.0, 0.0]);
}

#[test]
fn a_composition_resize_materialises_the_old_centre() {
    // 030: the comp becomes 1280x720 and L (1920x1080) is not moved: its Position, which was
    // "absent = centre" in the base, is now stored as [960,540,0] (the new centre would be
    // [640,360]); the layer size is unchanged.
    let (t, size) = transform("030-comp-size-1280x720.aep");
    assert_eq!(size, (1920, 1080));
    assert_eq!(t.position.value, [960.0, 540.0, 0.0]);
    assert_eq!(t.position.source, ValueSource::Stored);
    assert_eq!(t.anchor_fraction.source, ValueSource::Default);
}

// ------------------------------------------------------------------------------------ keyframes

/// The Position property of `L` in a fixture, handed to `f`.
fn with_position<R>(name: &str, f: impl FnOnce(Property<'_, '_>) -> R) -> R {
    with_transform_property(name, "ADBE Position", f)
}

fn with_transform_property<R>(name: &str, prop: &str, f: impl FnOnce(Property<'_, '_>) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let Some(PropNode::Property(p)) = layer.properties().unwrap().find(&["ADBE Transform Group", prop]) else { panic!("{name}: no {prop}") };
    f(p)
}

#[test]
fn linear_position_keys() {
    with_position("055-key-position-3key-inserted-t2.5.aep", |p| {
        assert_eq!(p.time_scale(), Some(25600));
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 3);
        // Keys added through the scripting interface are linear in and out, uncoloured, not roving.
        for k in &keys {
            assert_eq!(k.kind(), KeyframeKind::Spatial { components: 3 });
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
            assert_eq!((k.label(), k.is_roving()), (Some(0), Some(false)));
        }
        let secs: Vec<_> = keys.iter().map(|k| k.seconds(25600).unwrap()).collect();
        assert_eq!(secs, [0.0, 1.0, 2.5]);
        let values: Vec<_> = keys.iter().map(|k| k.spatial().unwrap().value).collect();
        assert_eq!(values, [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [200.0, 100.0, 0.0]]);
        assert_eq!(p.first_key_value(), Some(vec![0.0, 0.0, 0.0]));
        assert_eq!(p.is_static_flag_set(), Some(false));
    });
    // The transform of the layer reports the animated Position as keyframed (value at the first key).
    let (t, _) = transform("055-key-position-3key-inserted-t2.5.aep");
    assert_eq!(t.position.source, ValueSource::Keyframed);
    assert_eq!(t.position.value, [0.0, 0.0, 0.0]);
}

#[test]
fn ease_preset_sets_bezier_and_influence() {
    // 052: both keys use the ease preset: bezier in and out, speed 0, influence 0.33 (33 %).
    with_position("052-key-position-2key-ease.aep", |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        for k in &keys {
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
            let s = k.spatial().unwrap();
            // The speeds sit at slots 0 and 2, the influences at slots 1 and 3; which pair is "in"
            // is not decided, so the slots are exposed as stored.
            assert_eq!((s.ease[0], s.ease[2]), (0.0, 0.0));
            assert_eq!((s.ease[1], s.ease[3]), (0.33, 0.33));
        }
        let secs: Vec<_> = keys.iter().map(|k| k.seconds(25600).unwrap()).collect();
        assert_eq!(secs, [0.0, 1.0]);
    });
}

#[test]
fn hold_keyframe() {
    // 053: key 1 is a hold keyframe on both sides (bytes 3, 3), key 2 stays linear; the values are unchanged.
    with_position("053-key-position-2key-hold-key1.aep", |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!((keys[0].interpolation_in(), keys[0].interpolation_out()), (Some(Interpolation::Hold), Some(Interpolation::Hold)));
        assert_eq!((keys[1].interpolation_in(), keys[1].interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
        assert_eq!(keys[0].spatial().unwrap().value, [0.0, 0.0, 0.0]);
        assert_eq!(keys[1].spatial().unwrap().value, [100.0, 0.0, 0.0]);
    });
}

#[test]
fn spatial_tangents() {
    // 054: key 1 spatial out tangent [30,60,0], key 2 spatial in tangent [-30,-60,0]; the
    // auto-bezier / continuous switches were turned off together (the flag word becomes 0).
    with_position("054-key-position-2key-spatial-bezier.aep", |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().map(|k| k.spatial().unwrap()).collect();
        assert_eq!(keys[0].out_tangent, [30.0, 60.0, 0.0]);
        assert_eq!(keys[1].in_tangent, [-30.0, -60.0, 0.0]);
        assert_eq!(keys[0].in_tangent, [0.0, 0.0, 0.0]);
        assert_eq!(keys[1].out_tangent, [0.0, 0.0, 0.0]);
        assert_eq!((keys[0].auto_bezier_flag, keys[1].auto_bezier_flag), (0, 0));
    });
}

#[test]
fn roving_and_label_flags() {
    // 056: key 2 roving (AE moved it from 1 s to 26509/25600 s, the value is kept).
    with_position("056-key-position-3key-roving-key2.aep", |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        let roving: Vec<_> = keys.iter().map(|k| k.is_roving().unwrap()).collect();
        assert_eq!(roving, [false, true, false]);
        assert_eq!(keys[1].time_units(), Some(26509));
        assert_eq!(keys[1].seconds(25600), Some(1.0355078125));
        assert_eq!(keys[1].spatial_value(), Some([100.0, 0.0, 0.0]));
        assert_eq!(keys[1].flags(), Some(0x0027));
        assert_eq!(keys[1].label(), Some(0));
    });
    // 057: key 2 gets keyframe label 9; flags 0x0007 become 0x0247.
    with_position("057-key-position-3key-label9-key2.aep", |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        let labels: Vec<_> = keys.iter().map(|k| k.label().unwrap()).collect();
        assert_eq!(labels, [0, 9, 0]);
        assert_eq!(keys[1].flags(), Some(0x0247));
        assert_eq!(keys[0].flags(), Some(0x0007));
        assert_eq!(keys[1].is_roving(), Some(false));
        assert_eq!(keys[1].spatial_value(), Some([100.0, 0.0, 0.0]));
    });
}

#[test]
fn scalar_opacity_keys() {
    // 058: Opacity 0 % at 0 s, 100 % at 2 s, linear: 48-byte records, the value is a fraction.
    with_transform_property("058-key-opacity-2key.aep", "ADBE Opacity", |p| {
        assert!(p.is_animated());
        assert_eq!(p.dims(), Some(1));
        assert_eq!(p.time_scale(), Some(25600));
        assert_eq!(p.is_static_flag_set(), Some(false));
        let list = p.keys().unwrap();
        assert_eq!(list.header.record_size, 48);
        let keys: Vec<_> = list.keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Plain { components: 1 }));
        assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 2.0]);
        assert_eq!(keys.iter().map(|k| k.scalar_value().unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
        for k in &keys {
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
            assert_eq!(k.ease(), Some(KeyEase { in_speed: 0.0, in_influence: 0.0, out_speed: 0.0, out_influence: 0.0 }));
            assert_eq!(k.flags(), Some(0));
        }
        assert_eq!(keys[0].spatial(), None);
    });
    let (t, _) = transform("058-key-opacity-2key.aep");
    assert_eq!(t.opacity_percent.source, ValueSource::Keyframed);
    assert_eq!(t.opacity_percent.value, 0.0);
}

// ---------------------------------------------------------------------------------- expressions

#[test]
fn expression_text_and_flag_bytes() {
    // 060: Opacity expression `wiggle(1,10)`; the static value 1.0 is kept.
    with_transform_property("060-expr-opacity-wiggle.aep", "ADBE Opacity", |p| {
        assert_eq!(p.expression().as_deref(), Some("wiggle(1,10)"));
        let flags = p.expression_flags().unwrap();
        assert!(flags.all_set());
        assert_eq!((flags.byte_76, flags.byte_77, flags.byte_120), (1, 1, 1));
        assert_eq!(p.static_value(), Some(vec![1.0]));
        assert!(!p.is_animated());
    });
    // A property without an expression has none of the three bytes set.
    with_position("040-static-position-100-200.aep", |p| {
        assert_eq!(p.expression(), None);
        let flags = p.expression_flags().unwrap();
        assert_eq!((flags.byte_76, flags.byte_77, flags.byte_120), (0, 0, 0));
        assert!(!flags.all_set());
        // A written static property: tdsb 1, byte 5 of tdb4 has bit 0 set (15 for Position).
        assert_eq!(p.flags(), Some(1));
        assert_eq!(p.is_static_flag_set(), Some(true));
        assert_eq!(p.header().and_then(|h| h.get(5).copied()), Some(15));
    });
}
