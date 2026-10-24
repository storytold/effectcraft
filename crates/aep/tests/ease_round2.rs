//! Ease of Anchor Point, effect point and colour, and mask path keys (597-599, 700, 730); light
//! type codes (701, 702).

mod common;

use common::{first_comp, fixture, layer_shown_as, open};
use effectcraft_aep::{KeyframeKind, Project};

#[test]
fn mask_path_keys_keep_their_ease_at_byte_24() {
    for (name, first_out, last_in) in [("599-key-mask-path-ease-infl75.aep", 0.75, 0.75), ("700-key-mask-path-ease-out20-in50.aep", 0.2, 0.5)] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let comp = first_comp(&project);
        let layer = layer_shown_as(&project, &comp, "L");
        let atoms = layer.mask_atoms();
        let list = atoms[0].path.as_ref().and_then(|p| p.keys()).expect("path keys");
        let keys: Vec<_> = list.keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].kind(), KeyframeKind::MaskPath);
        let (a, b) = (keys[0].ease().unwrap(), keys[1].ease().unwrap());
        assert_eq!((a.out_speed, a.out_influence), (0.0, first_out), "{name}");
        assert_eq!((b.in_speed, b.in_influence), (0.0, last_in), "{name}");
    }
}

#[test]
fn a_spatial_speed_is_divided_by_the_layer_height() {
    // 597: 1920x1080 L, out speed 50 px/s stored as 50 / 1080; 730: 400x800 P, 50 / 800.
    for (name, layer, h) in [("597-key-anchor-ease-out50-40-in30-60.aep", "L", 1080.0), ("730-key-anchor-ease-on-400x800-solid.aep", "P", 800.0)] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let comp = first_comp(&project);
        let props = layer_shown_as(&project, &comp, layer).properties().unwrap();
        let Some(effectcraft_aep::PropNode::Group(t)) = props.get("ADBE Transform Group") else { panic!() };
        let Some(effectcraft_aep::PropNode::Property(a)) = t.get("ADBE Anchor Point") else { panic!() };
        let keys: Vec<_> = a.keys().unwrap().keyframes().collect();
        let (e0, e1) = (keys[0].ease().unwrap(), keys[1].ease().unwrap());
        assert!((e0.out_speed * h - 50.0).abs() < 1e-9 && (e1.in_speed * h - 30.0).abs() < 1e-9, "{name}: {e0:?} {e1:?}");
        assert_eq!((e0.out_influence, e1.in_influence), (0.4, 0.6));
    }
}

#[test]
fn parallel_and_ambient_lights_have_codes_0_and_3() {
    use effectcraft_aep::{LayerType, LightType};
    for (name, want) in [("701-light-parallel-int80-color.aep", LightType::Parallel), ("702-light-ambient-int50-color.aep", LightType::Ambient)] {
        let bytes = fixture(name);
        let file = open(&bytes);
        let project = Project::new(&file).unwrap();
        let comp = first_comp(&project);
        let light = comp.layers().find(|l| l.header().is_some_and(|h| h.layer_type == LayerType::Light)).expect("light");
        assert_eq!(light.header().unwrap().light_type(), Some(want), "{name}");
    }
}

#[test]
fn keyed_orientation_pairs_records_and_values() {
    use effectcraft_aep::Interpolation;
    let bytes = fixture("703-key-orientation-0-to-10-20-30-linear.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "L");
    assert!(layer.orientation().unwrap().animated);
    let keys = layer.orientation_keys();
    assert_eq!(keys.iter().map(|k| (k.time_units, k.degrees)).collect::<Vec<_>>(), [(0, [0.0; 3]), (51200, [10.0, 20.0, 30.0])]);
    assert!(keys.iter().all(|k| k.interpolation_in == Interpolation::Linear && k.interpolation_out == Interpolation::Linear));
}
