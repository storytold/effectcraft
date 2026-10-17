//! Keyframe interpolation sides and ease, the record layouts per template, the anchor point
//! normalisation and the disabled-expression flag, checked against the values the corpus was made
//! with (its MANIFEST, batch 2). The solid `L` is 1920x1080 in a 1920x1080 composition.

mod common;

use common::{assert_near, first_comp, fixture, layer_shown_as, layer_with_id, open};
use effectcraft_aep::{Interpolation, KeyEase, KeyframeKind, Project, PropNode, Property, ValueSource};

const L: u32 = 0x1b;

fn prop_of<R>(name: &str, path: &[&str], f: impl FnOnce(Property<'_, '_>) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let Some(PropNode::Property(p)) = layer.properties().unwrap().find(path) else { panic!("{name}: no {path:?}") };
    f(p)
}

const POSITION: &[&str] = &["ADBE Transform Group", "ADBE Position"];
const OPACITY: &[&str] = &["ADBE Transform Group", "ADBE Opacity"];

fn ease(in_speed: f64, in_influence: f64, out_speed: f64, out_influence: f64) -> KeyEase {
    KeyEase { in_speed, in_influence, out_speed, out_influence }
}

#[test]
fn in_and_out_interpolation_are_separate_bytes() {
    // 202: Position keys at 0 s [0,0,0] and 1 s [100,0,0]; key 1 inType LINEAR, outType HOLD (record bytes 01 03).
    prop_of("202-key-out-hold-in-linear.aep", POSITION, |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(&keys[0].raw()[4..6], [1, 3]);
        assert_eq!((keys[0].interpolation_in(), keys[0].interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Hold)));
        assert_eq!((keys[1].interpolation_in(), keys[1].interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
        assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
        assert_eq!(keys[1].spatial_value(), Some([100.0, 0.0, 0.0]));
    });
    // 203: key 1 inType HOLD, outType LINEAR (bytes 03 01): byte 4 is the incoming side.
    prop_of("203-key-in-hold-out-linear.aep", POSITION, |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(&keys[0].raw()[4..6], [3, 1]);
        assert_eq!((keys[0].interpolation_in(), keys[0].interpolation_out()), (Some(Interpolation::Hold), Some(Interpolation::Linear)));
        assert_eq!((keys[1].interpolation_in(), keys[1].interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
    });
}

#[test]
fn position_ease_slots_in_speed_influence_out_speed_influence() {
    // 204: key 1 inSpeed 5 / inInfluence 20, outSpeed 50 / outInfluence 80; key 2 in 7 / 60, out 9 / 40.
    // After Effects reads key 1's in speed and key 2's out speed back as 0; the file keeps 5 and 9.
    prop_of("204-key-ease-asymmetric.aep", POSITION, |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys[0].ease(), Some(ease(5.0, 0.2, 50.0, 0.8)));
        assert_eq!(keys[1].ease(), Some(ease(7.0, 0.6, 9.0, 0.4)));
        for k in &keys {
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
            assert_eq!(k.kind(), KeyframeKind::Spatial { components: 3 });
        }
        // The spatial record gives the same four slots.
        assert_eq!(keys[0].spatial().unwrap().ease, [5.0, 0.2, 50.0, 0.8]);
        // The arc length of the segment to the next key (100 px) is on key 1, none on the last key.
        assert_eq!(keys.iter().map(|k| k.spatial().unwrap().segment_length).collect::<Vec<_>>(), [100.0, 0.0]);
    });
}

#[test]
fn opacity_ease_speed_is_in_stored_units_per_second() {
    // 205: Opacity 0 at 0 s, 100 at 2 s; key 1 in 5 / 20 %, out 20 / 80 %; key 2 in 30 / 60 %, out 9 / 40 %.
    // The speed is in stored value units (fractions), 100 times smaller than the UI value; the influence is a fraction.
    prop_of("205-key-opacity-ease-asymmetric.aep", OPACITY, |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert!(keys.iter().all(|k| k.kind() == KeyframeKind::Plain { components: 1 } && k.raw().len() == 48));
        assert_eq!(keys.iter().map(|k| k.scalar_value().unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
        assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 2.0]);
        let e0 = keys[0].ease().unwrap();
        assert_near(&[e0.in_speed, e0.in_influence, e0.out_speed, e0.out_influence], &[0.05, 0.2, 0.2, 0.8], 1e-12);
        let e1 = keys[1].ease().unwrap();
        assert_near(&[e1.in_speed, e1.in_influence, e1.out_speed, e1.out_influence], &[0.3, 0.6, 0.09, 0.4], 1e-12);
        assert_eq!(keys[0].plain_ease_slots().unwrap().len(), 4);
        for k in &keys {
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
        }
    });
}

#[test]
fn scale_keys_use_the_plain_template_with_three_components() {
    // 206: Scale keys at 0 s [100,100,100] and 1 s [200,150,100], linear: 128-byte plain records.
    prop_of("206-key-scale-2key-linear.aep", &["ADBE Transform Group", "ADBE Scale"], |p| {
        assert_eq!(p.dims(), Some(3));
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        for k in &keys {
            assert_eq!(k.raw().len(), 128);
            // A 128-byte record is not always spatial: the header's template flag decides.
            assert_eq!(k.kind(), KeyframeKind::Plain { components: 3 });
            assert_eq!(k.spatial(), None);
            assert_eq!(k.plain_ease_slots(), Some(vec![0.0; 12]));
            assert_eq!(k.ease(), None);
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Linear), Some(Interpolation::Linear)));
        }
        assert_eq!(keys.iter().map(|k| k.value().unwrap()).collect::<Vec<_>>(), [vec![1.0, 1.0, 1.0], vec![2.0, 1.5, 1.0]]);
        assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
        assert_eq!(p.first_key_value(), Some(vec![1.0, 1.0, 1.0]));
    });
    // The transform reports the first key as percent (before the template flag was used it read the wrong bytes).
    let bytes = fixture("206-key-scale-2key-linear.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let t = layer.transform(1920, 1080).unwrap();
    assert_eq!((t.scale_percent.source, t.scale_percent.value), (ValueSource::Keyframed, [100.0, 100.0, 100.0]));
}

#[test]
fn two_dimensional_mask_feather_keys() {
    // 208: Mask Feather keys at 0 s [0,0] and 1 s [40,20], linear: 88-byte plain records with two components.
    let bytes = fixture("208-key-mask-feather-2key-linear.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let Some(PropNode::Group(parade)) = layer.properties().unwrap().get("ADBE Mask Parade") else { panic!("no mask parade") };
    let Some(PropNode::Group(atom)) = parade.get("ADBE Mask Atom") else { panic!("no mask atom") };
    let Some(PropNode::Property(feather)) = atom.get("ADBE Mask Feather") else { panic!("no feather") };
    let keys: Vec<_> = feather.keys().unwrap().keyframes().collect();
    assert!(keys.iter().all(|k| k.raw().len() == 88 && k.kind() == KeyframeKind::Plain { components: 2 }));
    assert_eq!(keys.iter().map(|k| k.value().unwrap()).collect::<Vec<_>>(), [vec![0.0, 0.0], vec![40.0, 20.0]]);
    assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
    assert_eq!(keys[0].plain_ease_slots(), Some(vec![0.0; 8]));
    let mask = &layer.masks()[0];
    assert!(mask.feather.source == ValueSource::Keyframed && mask.feather.value == [0.0, 0.0]);
}

#[test]
fn colour_keys_use_the_spatial_template_with_four_components() {
    // 210: Fill colour keys at 0 s [1,0,0] and 1 s [0,0.5,1], linear: 152-byte records, ARGB doubles 0..255 at byte 56.
    let bytes = fixture("210-key-fill-color-2key-linear.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let effects = layer.effects();
    assert_eq!(effects.len(), 1);
    let params = effects[0].params();
    let color = params.iter().find(|p| p.name == "Color").unwrap();
    let keys: Vec<_> = color.stored().unwrap().keys().unwrap().keyframes().collect();
    assert_eq!(keys.len(), 2);
    for k in &keys {
        assert_eq!(k.raw().len(), 152);
        assert_eq!(k.kind(), KeyframeKind::Spatial { components: 4 });
        assert_eq!(k.flags(), Some(0x0001));
        let s = k.spatial().unwrap();
        assert_eq!(s.in_tangent, [0.0; 4]);
        assert_eq!(s.out_tangent, [0.0; 4]);
    }
    assert_eq!(keys[0].value(), Some(vec![255.0, 255.0, 0.0, 0.0]));
    assert_eq!(keys[1].value(), Some(vec![255.0, 0.0, 127.5, 255.0]));
    assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 1.0]);
    // A keyframed parameter has no single stored value.
    assert_eq!(color.stored_value(), None);
}

#[test]
fn keyed_slider_uses_48_byte_records() {
    // 221: Slider Control, Slider keyed 10 at 0 s and 90 at 2 s (linear).
    let bytes = fixture("221-fx-slider-2key.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let layer = layer_with_id(&first_comp(&project), L);
    let params = layer.effects()[0].params();
    let slider = params.iter().find(|p| p.name == "Slider").unwrap();
    let stored = slider.stored().unwrap();
    assert!(stored.is_animated());
    assert_eq!(stored.static_value(), None);
    let keys: Vec<_> = stored.keys().unwrap().keyframes().collect();
    assert!(keys.iter().all(|k| k.raw().len() == 48));
    assert_eq!(keys.iter().filter_map(|k| k.scalar_value()).collect::<Vec<_>>(), [10.0, 90.0]);
    assert_eq!(keys.iter().map(|k| k.seconds(25600).unwrap()).collect::<Vec<_>>(), [0.0, 2.0]);
}

#[test]
fn anchor_is_a_fraction_of_the_layer_source_size() {
    // 211: a new 100x100 solid in the 1920x1080 composition, Anchor Point [50,50,0] -> [10,20,0], stored [0.1, 0.2, 0].
    // 212: a 400x300 solid, Anchor Point [100,50,0], stored [0.25, 1/6, 0]. The composition size is not the divisor.
    for (name, solid, size, fraction, px) in [
        ("211-anchor-100x100-solid-10-20.aep", "S100", (100, 100), [0.1, 0.2, 0.0], [10.0, 20.0, 0.0]),
        ("212-anchor-400x300-solid-100-50.aep", "S400", (400, 300), [0.25, 1.0 / 6.0, 0.0], [100.0, 50.0, 0.0]),
    ] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let comp = first_comp(&project);
        let layer = layer_shown_as(&project, &comp, solid);
        assert_eq!(project.layer_source_size(&layer), Some(size), "{name}");
        let t = layer.transform(1920, 1080).unwrap();
        assert_eq!(t.anchor_fraction.source, ValueSource::Stored);
        assert_near(&t.anchor_fraction.value, &fraction, 1e-12);
        let anchor = project.layer_anchor_px(&layer).unwrap();
        assert_eq!(anchor.source, ValueSource::Stored);
        assert_near(&anchor.value, &px, 1e-9);
        // Position stays at the default, the composition centre.
        assert_eq!((t.position.source, t.position.value), (ValueSource::Default, [960.0, 540.0, 0.0]));
        // L is unchanged: the anchor is its centre.
        let l = layer_with_id(&comp, L);
        let a = project.layer_anchor_px(&l).unwrap();
        assert_eq!((a.source, a.value), (ValueSource::Default, [960.0, 540.0, 0.0]));
    }
}

fn expression_state(name: &str) -> (Option<String>, Option<bool>, bool, bool) {
    prop_of(name, OPACITY, |p| {
        let f = p.expression_flags().unwrap();
        (p.expression(), p.expression_enabled(), f.all_set(), f.disabled())
    })
}

#[test]
fn disabled_expression_keeps_its_text() {
    // 060: Opacity wiggle(1,10), enabled; 213: the same expression after expression.disable_all.
    assert_eq!(expression_state("060-expr-opacity-wiggle.aep"), (Some("wiggle(1,10)".to_string()), Some(true), true, false));
    assert_eq!(expression_state("213-expr-opacity-wiggle-disabled.aep"), (Some("wiggle(1,10)".to_string()), Some(false), true, true));
    // Only byte 119 differs between the two property headers.
    let header = |name: &str| prop_of(name, OPACITY, |p| p.header().unwrap().to_vec());
    let (enabled, disabled) = (header("060-expr-opacity-wiggle.aep"), header("213-expr-opacity-wiggle-disabled.aep"));
    let differing: Vec<_> = (0..enabled.len()).filter(|i| enabled[*i] != disabled[*i]).collect();
    assert_eq!(differing, [119]);
    // A property without an expression has no enabled state.
    assert_eq!(prop_of("058-key-opacity-2key.aep", OPACITY, |p| p.expression_enabled()), None);
}
