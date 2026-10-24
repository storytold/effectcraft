//! Batch 3 (core): explicit defaults, multi-component bezier ease, colour key distance, exact time
//! stretch and in / out points, Time Remapping, composition aspect / shutter phase / display start.
//! Every expected value is the value the corpus case was made with (its MANIFEST).

mod common;

use common::{assert_near, first_comp, fixture, layer_shown_as, layer_with_id, near, open};
use effectcraft_aep::{Interpolation, KeyEase, KeyframeKind, LayerHeader, ParamValue, Project, PropNode, Property, Rational, ValueSource};

const L: u32 = 0x1b;

fn with_layer<R>(name: &str, id: u32, f: impl FnOnce(&Project<'_, '_>, &effectcraft_aep::Layer<'_, '_>) -> R) -> R {
    let bytes = fixture(name);
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_with_id(&comp, id);
    f(&project, &layer)
}

fn header(name: &str) -> LayerHeader {
    with_layer(name, L, |_, l| l.header().unwrap())
}

fn rat(num: i64, den: i64) -> Rational {
    Rational { num, den }
}

fn ease(in_speed: f64, in_influence: f64, out_speed: f64, out_influence: f64) -> KeyEase {
    KeyEase { in_speed, in_influence, out_speed, out_influence }
}

fn same_ease(a: &KeyEase, b: &KeyEase) -> bool {
    near(a.in_speed, b.in_speed, 1e-9)
        && near(a.in_influence, b.in_influence, 1e-9)
        && near(a.out_speed, b.out_speed, 1e-9)
        && near(a.out_influence, b.out_influence, 1e-9)
}

/// The property at a path of match names below a layer.
fn with_property<R>(name: &str, id: u32, path: &[&str], f: impl FnOnce(&Property<'_, '_>) -> R) -> R {
    with_layer(name, id, |_, layer| {
        let Some(PropNode::Property(p)) = layer.properties().unwrap().find(path) else { panic!("{name}: no property {path:?}") };
        f(&p)
    })
}

// ------------------------------------------------------------------ explicit defaults (300-304, 311)

fn blur_param(name: &str) -> (Option<ParamValue>, Option<ParamValue>, Option<bool>, ParamValue, bool) {
    with_layer(name, L, |_, layer| {
        let effects = layer.effects();
        assert_eq!(effects.len(), 1, "{name}");
        let p = effects[0].params().into_iter().find(|p| p.name == "Blurriness").unwrap();
        (p.stored_value(), p.pard_default(), p.is_default(), p.value().unwrap(), p.stored().is_some())
    })
}

#[test]
fn an_explicit_zero_blurriness_is_a_value_record_and_25_is_the_default() {
    // 300: a fresh Gaussian Blur reads back Blurriness 25 (the MANIFEST row 214 "0" was wrong);
    // Blurriness set explicitly to 0 writes a record, the default in pard stays 25.
    let (stored, default, is_default, value, has_record) = blur_param("300-fx-gaussianblur-0-explicit.aep");
    assert_eq!((stored, default, value, has_record), (Some(ParamValue::Number(0.0)), Some(ParamValue::Number(25.0)), ParamValue::Number(0.0), true));
    assert_eq!(is_default, Some(false));
    // 301: 25 -> 40 -> 25: the record is removed again, the effective value is the default 25.
    let (stored, default, is_default, value, has_record) = blur_param("301-fx-gaussianblur-40-then-25.aep");
    assert_eq!((stored, default, value, has_record), (None, Some(ParamValue::Number(25.0)), ParamValue::Number(25.0), false));
    assert_eq!(is_default, Some(true));
    // 216 (Blurriness 40): a record, default still 25.
    let (stored, default, is_default, value, _) = blur_param("216-fx-gaussianblur-40.aep");
    assert_eq!((stored, default, value), (Some(ParamValue::Number(40.0)), Some(ParamValue::Number(25.0)), ParamValue::Number(40.0)));
    assert_eq!(is_default, Some(false));
}

#[test]
fn setting_a_parameter_back_to_its_default_leaves_no_record() {
    // 303: Fill Opacity 1 -> 0.5 -> 1 and 302: Opacity set to 1 explicitly: no record, value 1.0 (100 %).
    for name in ["302-fx-fill-opacity-100-explicit.aep", "303-fx-fill-opacity-50-then-100.aep"] {
        with_layer(name, L, |_, layer| {
            let p = layer.effects()[0].params().into_iter().find(|p| p.name == "Opacity").unwrap();
            assert!(p.stored().is_none(), "{name}");
            assert_eq!((p.value(), p.is_default()), (Some(ParamValue::Number(1.0)), Some(true)), "{name}");
        });
    }
    // 304: Slider set to 0 explicitly: no record, value 0.
    with_layer("304-fx-slider-0-explicit.aep", L, |_, layer| {
        let p = layer.effects()[0].params().into_iter().find(|p| p.name == "Slider").unwrap();
        assert!(p.stored().is_none());
        assert_eq!((p.value(), p.is_default()), (Some(ParamValue::Number(0.0)), Some(true)));
    });
    // The implicit input layer and the built-in entry are not "at default" questions.
    with_layer("304-fx-slider-0-explicit.aep", L, |_, layer| {
        let params = layer.effects()[0].params();
        assert_eq!(params.len(), 3);
        assert_eq!(params[0].is_default(), None);
    });
    // 071 (Slider 77) is a record that differs from the default.
    with_layer("071-fx-slider-value77.aep", L, |_, layer| {
        let p = layer.effects()[0].params().into_iter().find(|p| p.name == "Slider").unwrap();
        assert_eq!((p.value(), p.is_default()), (Some(ParamValue::Number(77.0)), Some(false)));
    });
}

#[test]
fn a_transform_set_to_its_default_adds_no_record() {
    // 311: Position set to the value it already had ([960,540,0]): the layer tree is the tree of the
    // unedited 299 (which equals the base layer except for the session), no ADBE Position record.
    with_layer("311-position-default-explicit.aep", L, |_, layer| {
        let t = layer.transform(1920, 1080).unwrap();
        assert_eq!((t.position.value, t.position.source), ([960.0, 540.0, 0.0], ValueSource::Default));
        assert_eq!(t.opacity_percent.source, ValueSource::Default);
        assert!(layer.properties().unwrap().find(&["ADBE Transform Group", "ADBE Position"]).is_none());
        let a = layer.chunk().clone();
        with_layer("299-save-noise.aep", L, |_, other| assert_eq!(&a, other.chunk()));
    });
}

// ------------------------------------------------------------------ bezier ease layouts (305-307)

#[test]
fn scale_keys_keep_their_ease_type_major() {
    // 305: Scale keys t=0 [100,100,100] and t=2 [200,150,100]; key1 in speed 0 / influence 33, out
    // speed 20 / influence 66; key2 in speed 10 / influence 66, out speed 0 / influence 33. The
    // stored speed is a fraction per second (percent / 100), the influence a fraction.
    with_property("305-key-scale-bezier-ease.aep", L, &["ADBE Transform Group", "ADBE Scale"], |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        let scale = p.time_scale().unwrap();
        assert_eq!(keys.iter().map(|k| k.seconds(scale).unwrap()).collect::<Vec<_>>(), [0.0, 2.0]);
        for k in &keys {
            assert_eq!(k.kind(), KeyframeKind::Plain { components: 3 });
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
        }
        assert_near(&keys[0].value().unwrap(), &[1.0, 1.0, 1.0], 1e-9);
        assert_near(&keys[1].value().unwrap(), &[2.0, 1.5, 1.0], 1e-9);
        // The raw block: [in speed x3][in influence x3][out speed x3][out influence x3].
        assert_near(&keys[0].plain_ease_slots().unwrap(), &[0.0, 0.0, 0.0, 0.33, 0.33, 0.33, 0.2, 0.2, 0.2, 0.66, 0.66, 0.66], 1e-9);
        assert_near(&keys[1].plain_ease_slots().unwrap(), &[0.1, 0.1, 0.1, 0.66, 0.66, 0.66, 0.0, 0.0, 0.0, 0.33, 0.33, 0.33], 1e-9);
        for i in 0..3 {
            assert!(same_ease(&keys[0].ease_component(i).unwrap(), &ease(0.0, 0.33, 0.2, 0.66)), "key 1 component {i}");
            assert!(same_ease(&keys[1].ease_component(i).unwrap(), &ease(0.1, 0.66, 0.0, 0.33)), "key 2 component {i}");
        }
        assert!(keys[0].ease_component(3).is_none());
        assert_eq!(keys[0].ease_components().unwrap().len(), 3);
        assert!(keys[0].color_distance().is_none());
    });
}

#[test]
fn mask_feather_keys_keep_their_ease_type_major() {
    // 306: Mask Feather keys t=0 [0,0], t=2 [50,30]; key1 in 0/33, out 15/66; key2 in 8/66, out 0/33
    // (speed in pixels per second).
    with_layer("306-key-maskfeather-bezier-ease.aep", L, |_, layer| {
        let masks = layer.masks();
        assert_eq!(masks.len(), 1);
        assert_eq!(masks[0].feather.source, ValueSource::Keyframed);
        assert_eq!(masks[0].feather.value, [0.0, 0.0]);
    });
    with_property("306-key-maskfeather-bezier-ease.aep", L, &["ADBE Mask Parade", "ADBE Mask Atom", "ADBE Mask Feather"], |p| {
        let keys: Vec<_> = p.keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        for k in &keys {
            assert_eq!(k.kind(), KeyframeKind::Plain { components: 2 });
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
        }
        assert_near(&keys[0].value().unwrap(), &[0.0, 0.0], 1e-9);
        assert_near(&keys[1].value().unwrap(), &[50.0, 30.0], 1e-9);
        assert_near(&keys[0].plain_ease_slots().unwrap(), &[0.0, 0.0, 0.33, 0.33, 15.0, 15.0, 0.66, 0.66], 1e-9);
        for i in 0..2 {
            assert!(same_ease(&keys[0].ease_component(i).unwrap(), &ease(0.0, 0.33, 15.0, 0.66)), "key 1 component {i}");
            assert!(same_ease(&keys[1].ease_component(i).unwrap(), &ease(8.0, 0.66, 0.0, 0.33)), "key 2 component {i}");
        }
        assert!(keys[1].ease_component(2).is_none());
    });
}

#[test]
fn one_component_and_position_ease_through_the_component_accessor() {
    // 205 (Opacity, n = 1): in 0.05 / 0.2, out 0.2 / 0.8; 204 (Position): the single entry.
    with_property("205-key-opacity-ease-asymmetric.aep", L, &["ADBE Transform Group", "ADBE Opacity"], |p| {
        let k = p.keys().unwrap().keyframes().next().unwrap();
        assert_eq!(k.ease_component(0), k.ease());
        assert!(same_ease(&k.ease_component(0).unwrap(), &ease(0.05, 0.2, 0.2, 0.8)));
        assert!(k.ease_component(1).is_none());
    });
    with_property("204-key-ease-asymmetric.aep", L, &["ADBE Transform Group", "ADBE Position"], |p| {
        let k = p.keys().unwrap().keyframes().next().unwrap();
        assert_eq!(k.ease_component(0), k.ease());
        assert!(k.ease_component(1).is_none());
        assert_eq!(k.ease_components().unwrap().len(), 1);
    });
}

#[test]
fn bezier_colour_keys_carry_the_distance_to_the_next_key() {
    // 307: Fill Color keys t=0 [1,0,0,1] and t=2 [0,0,1,1], both bezier; ease 0/0; f64@16 = the
    // ARGB distance 255 sqrt(2) = 360.6245 on key 1 and 0 on the last key. The linear keys of 210 have 0.
    with_layer("307-key-fillcolor-bezier.aep", L, |_, layer| {
        let p = layer.effects()[0].params().into_iter().find(|p| p.name == "Color").unwrap();
        let keys: Vec<_> = p.stored().unwrap().keys().unwrap().keyframes().collect();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].kind(), KeyframeKind::Spatial { components: 4 });
        assert_near(&keys[0].value().unwrap(), &[255.0, 255.0, 0.0, 0.0], 1e-9);
        assert_near(&keys[1].value().unwrap(), &[255.0, 0.0, 0.0, 255.0], 1e-9);
        for k in &keys {
            assert_eq!((k.interpolation_in(), k.interpolation_out()), (Some(Interpolation::Bezier), Some(Interpolation::Bezier)));
            assert!(same_ease(&k.ease_component(0).unwrap(), &ease(0.0, 0.0, 0.0, 0.0)));
        }
        assert!(near(keys[0].color_distance().unwrap(), 255.0 * 2f64.sqrt(), 1e-3), "{:?}", keys[0].color_distance());
        assert!(near(keys[0].color_distance().unwrap(), 360.6245, 1e-3));
        assert_eq!(keys[1].color_distance(), Some(0.0));
    });
    with_layer("210-key-fill-color-2key-linear.aep", L, |_, layer| {
        let p = layer.effects()[0].params().into_iter().find(|p| p.name == "Color").unwrap();
        let k = p.stored().unwrap().keys().unwrap().keyframes().next().unwrap();
        assert_eq!(k.color_distance(), Some(0.0));
    });
}

// ------------------------------------------------------------------ time stretch and in / out points (316-318, 351, 353)

fn stretch_facts(name: &str) -> (LayerHeader, Option<Rational>, Option<Rational>, u32) {
    with_layer(name, L, |_, layer| {
        let h = layer.header().unwrap();
        let Some(PropNode::Property(p)) = layer.properties().unwrap().get("ADBE Time Remapping") else { panic!("no time remap property") };
        (h.clone(), h.in_point(), h.out_point(), p.time_scale().unwrap())
    })
}

#[test]
fn time_stretch_is_an_exact_ratio() {
    // 316: stretch 100 -> 50 %: 1/2; the stored out point stays 5 s, the layer ends at 2.5 s; the
    // property time scale stays 25600.
    let (h, in_p, out_p, scale) = stretch_facts("316-layer-stretch50.aep");
    assert_eq!((h.stretch, h.stretch_den), (1, 2));
    assert_eq!(h.stretch_ratio(), Some(rat(1, 2)));
    assert_eq!(h.stretch_percent(), 50.0);
    assert_eq!(h.out_relative, rat(128000, 25600));
    assert_eq!((in_p, out_p, scale), (Some(rat(0, 1)), Some(rat(5, 2)), 25600));
    // 317: 150 %: 3/2, ends at 7.5 s, the property time scale is 25600 x 3/2.
    let (h, in_p, out_p, scale) = stretch_facts("317-layer-stretch150.aep");
    assert_eq!((h.stretch, h.stretch_den), (3, 2));
    assert_eq!(h.stretch_ratio(), Some(rat(3, 2)));
    assert_eq!(h.stretch_percent(), 150.0);
    assert_eq!((in_p, out_p, scale), (Some(rat(0, 1)), Some(rat(15, 2)), 38400));
    // 018: 200 %: 2/1, ends at 10 s, time scale 51200. The base is 1/1.
    let (h, _, out_p, scale) = stretch_facts("018-layer-stretch200.aep");
    assert_eq!((h.stretch, h.stretch_den, h.stretch_ratio()), (2, 1, Some(rat(2, 1))));
    assert_eq!((out_p, scale), (Some(rat(10, 1)), 51200));
    let (h, _, out_p, scale) = stretch_facts("000-base.aep");
    assert_eq!((h.stretch, h.stretch_den, h.stretch_percent()), (1, 1, 100.0));
    assert_eq!((out_p, scale), (Some(rat(5, 1)), 25600));
}

#[test]
fn fractional_times_are_exact_multiples_of_one_over_25600() {
    // 318: inPoint 0.3333 s became 8532/25600 = 0.33328125 s; the out point moved with it (duration
    // kept): 136532/25600 = 5.33328125 s; start stays 0.
    let h = header("318-layer-inpoint-0.3333.aep");
    assert_eq!(h.in_relative, rat(8532, 25600));
    assert_eq!(h.out_relative, rat(136532, 25600));
    assert_eq!(h.start, rat(0, 25600));
    assert_eq!(h.in_point(), Some(rat(2133, 6400)));
    assert_eq!(h.in_point_seconds(), Some(0.33328125));
    assert_eq!(h.out_point_seconds(), Some(5.33328125));
    assert_eq!(h.stretch_ratio(), Some(rat(1, 1)));
}

#[test]
fn footage_layers_use_the_footage_time_base_as_out_point_denominator() {
    // 351: the audio layer's out point is 48000/48000 (the sample rate as denominator), start and
    // in point keep 25600; the layer copies the item's label 7 and shows the item's name.
    let bytes = fixture("351-footage-wav-as-layer.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "test-tone.wav");
    let h = layer.header().unwrap();
    assert_eq!(h.out_relative, rat(48000, 48000));
    assert_eq!((h.start.den, h.in_relative.den), (25600, 25600));
    assert_eq!(h.out_point(), Some(rat(1, 1)));
    assert_eq!((h.label, h.source_id), (7, Some(0x20)));
    assert!(layer.name().is_none());
    assert!(h.is_video_enabled(), "the video switch stays set on an audio-only layer (flag byte 39 = 07)");
    // 354: the image sequence layer: out point 3/30 = 0.1 s (the frame rate as denominator), label 3,
    // the layer has no name of its own and shows the first file name.
    let bytes = fixture("354-footage-pngseq-timeremap.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "seq_001.png");
    let h = layer.header().unwrap();
    assert_eq!(h.out_relative, rat(3, 30));
    assert_eq!(h.out_point(), Some(rat(1, 10)));
    assert_eq!((h.label, h.source_id, h.start.den), (3, Some(0x22), 25600));
}

// ------------------------------------------------------------------ Time Remapping (319, 354)

#[test]
fn time_remapping_is_an_inert_property_until_enabled() {
    // Every layer has an inert ADBE Time Remapping property (tdsb 3, static 0, no keys).
    with_layer("000-base.aep", L, |_, layer| {
        let t = layer.time_remap().unwrap();
        assert!(!t.enabled && t.keys.is_empty());
        let Some(PropNode::Property(p)) = layer.properties().unwrap().get("ADBE Time Remapping") else { panic!() };
        assert_eq!((p.flags(), p.is_animated(), p.static_value()), (Some(3), false, Some(vec![0.0])));
    });
    // 319: the precomp layer `Pre` with the remap on: keyed (tdsb 1), two keys 0 s -> 0 and 5 s -> 5;
    // the layer inside Pre and the other layers stay inert.
    let bytes = fixture("319-precomp-timeremap.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "Pre");
    let t = layer.time_remap().unwrap();
    assert!(t.enabled);
    assert_eq!(t.keys.len(), 2);
    assert_eq!((t.keys[0].time_seconds, t.keys[0].value_seconds), (Some(0.0), 0.0));
    assert_eq!((t.keys[1].time_seconds, t.keys[1].value_seconds), (Some(5.0), 5.0));
    assert_eq!(t.keys[1].time_units, 128000);
    let Some(PropNode::Property(p)) = layer.properties().unwrap().get("ADBE Time Remapping") else { panic!() };
    assert_eq!((p.flags(), p.is_animated(), p.static_value()), (Some(1), true, None));
    let pre = project.all_items().into_iter().find(|i| i.name().as_deref() == Some("Pre")).unwrap().comp().unwrap();
    assert!(pre.layers().all(|l| !l.time_remap().unwrap().enabled));
    // 354: on a footage layer: keys 0 s -> 0 and 0.1 s -> 0.1.
    let bytes = fixture("354-footage-pngseq-timeremap.aep");
    let file = open(&bytes);
    let project = Project::new(&file).unwrap();
    let comp = first_comp(&project);
    let layer = layer_shown_as(&project, &comp, "seq_001.png");
    let t = layer.time_remap().unwrap();
    assert!(t.enabled);
    assert_eq!(t.keys.len(), 2);
    assert_eq!((t.keys[0].time_units, t.keys[0].value_seconds), (0, 0.0));
    assert!(near(t.keys[1].time_seconds.unwrap(), 0.1, 1e-6) && near(t.keys[1].value_seconds, 0.1, 1e-6), "{t:?}");
    assert!(!layer_with_id(&comp, L).time_remap().unwrap().enabled);
}

// ------------------------------------------------------------------ composition (325-327)

fn comp_header(name: &str) -> effectcraft_aep::CompHeader {
    let bytes = fixture(name);
    let file = open(&bytes);
    first_comp(&Project::new(&file).unwrap()).header().unwrap()
}

#[test]
fn pixel_aspect_shutter_phase_and_display_start() {
    // 325: pixelAspect 0.9091 reads back 0.90909090909091 = the exact ratio 10/11.
    let h = comp_header("325-comp-pixelaspect-0.9091.aep");
    assert_eq!(h.pixel_aspect.reduced(), Some(rat(10, 11)));
    assert!(near(h.pixel_aspect_ratio().unwrap(), 0.90909090909091, 1e-12));
    // 326: shutterPhase -90, the shutter angle stays 180.
    let h = comp_header("326-comp-shutterphase-m90.aep");
    assert_eq!((h.shutter_phase, h.shutter_angle), (Some(-90), 180));
    // 327: displayStartFrame 100 at 25 fps = 4 s = 102400/25600; the shutter phase is back to 0.
    let h = comp_header("327-comp-displaystartframe-100.aep");
    assert_eq!(h.display_start, Some(rat(102400, 25600)));
    assert_eq!((h.display_start_seconds(), h.display_start_frame(), h.shutter_phase), (Some(4.0), Some(100), Some(0)));
    // The base: no phase, start 0.
    let h = comp_header("000-base.aep");
    assert_eq!((h.shutter_phase, h.display_start_seconds(), h.display_start_frame()), (Some(0), Some(0.0), Some(0)));
    assert_eq!(h.pixel_aspect_ratio(), Some(1.0));
    // A short chunk still decodes the old fields.
    let bytes = [0u8; 176];
    let h = effectcraft_aep::CompHeader::parse(&bytes).unwrap();
    assert_eq!((h.shutter_phase, h.display_start.map(|r| r.den)), (None, Some(0)));
}
