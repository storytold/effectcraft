//! Copied keyframes as After Effects keyframe text for other applications (#1908).

use effectcraft_project::{LayerId, Uid};
use effectcraft_time::Tick;
use serde_json::json;

use crate::{KeyRef, Session};

/// A 1920×1080 23.976 fps comp with a 200×100 solid.
fn session() -> (Session, LayerId) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 1920, "height": 1080, "frameRate": 23.976, "duration": 10})).unwrap();
    let id = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    (s, LayerId(id))
}

fn frame(s: &Session, n: i64) -> Tick {
    Tick(s.active_comp().unwrap().frame_rate.frame_duration().0 * n)
}

fn uid(s: &Session, lid: LayerId, path: &str) -> Uid {
    s.active_comp().unwrap().layer(lid).unwrap().props.prop(path).unwrap().uid
}

fn edit_layer(s: &mut Session, lid: LayerId, f: impl FnOnce(&mut effectcraft_project::Layer)) {
    let cid = s.active_comp_id().unwrap();
    f(std::sync::Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(lid).unwrap());
}

fn add_key(s: &mut Session, lid: LayerId, path: &str, time: Tick, value: serde_json::Value) {
    s.execute("prop.addKey", json!({"layer": lid.0, "path": path, "time": time.seconds(), "value": value})).unwrap();
}

#[test]
fn copied_keys_are_after_effects_text_in_comp_frames() {
    let (mut s, l) = session();
    // The layer starts 10 frames in: its keys at layer frames 2, 5 and 9 are comp frames 12, 15 and 19.
    let start = frame(&s, 10);
    edit_layer(&mut s, l, |l| l.start_time = start);
    for (n, v) in [(2, [960.0, 540.0]), (5, [1000.5, 520.25]), (9, [1100.0, 500.0])] {
        let t = frame(&s, n);
        add_key(&mut s, l, "transform/position", t, json!(v));
    }
    let t = frame(&s, 5);
    add_key(&mut s, l, "transform/rotation", t, json!(-45));
    let (position, rotation) = (uid(&s, l, "transform/position"), uid(&s, l, "transform/rotation"));
    // A subset: two of the three Position keys and the Rotation key.
    s.state.selected_keys = [(position, 2), (position, 9), (rotation, 5)].map(|(prop, n)| KeyRef { layer: l, prop, time: frame(&s, n) }).to_vec();
    s.execute("keys.copy", json!({})).unwrap();
    assert_eq!(
        s.state.keyframe_text.as_deref(),
        Some(
            "Adobe After Effects 9.0 Keyframe Data\n\n\tUnits Per Second\t23.976\n\tSource Width\t200\n\tSource Height\t100\n\tSource Pixel Aspect Ratio\t1\n\tComp Pixel Aspect Ratio\t1\n\n\
             Layer\n\
             Transform\tPosition\n\tFrame\tX pixels\tY pixels\tZ pixels\t\n\t12\t960\t540\t0\t\n\t19\t1100\t500\t0\t\n\n\
             Transform\tRotation\n\tFrame\tdegrees\t\n\t15\t-45\t\n\n\
             \n\
             End of Keyframe Data\n"
        )
    );
    // The internal clipboard still has the keys themselves, relative to the earliest.
    let times: Vec<Vec<Tick>> = s.state.key_clipboard.iter().map(|c| c.keys.iter().map(|k| k.time).collect()).collect();
    assert_eq!(times, [vec![Tick::ZERO, frame(&s, 7)], vec![frame(&s, 3)]]);
}

#[test]
fn a_stretched_layer_writes_the_frames_its_keys_show_at() {
    let (mut s, l) = session();
    edit_layer(&mut s, l, |l| l.stretch = 200.0);
    let t = frame(&s, 6);
    add_key(&mut s, l, "transform/opacity", Tick::ZERO, json!(0));
    add_key(&mut s, l, "transform/opacity", t, json!(100));
    s.execute("prop.select", json!({"layer": l.0, "path": "transform/opacity"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    assert!(text.contains("Transform\tOpacity\n\tFrame\tpercent\t\n\t0\t0\t\n\t12\t100\t\n\n"), "{text}");
}

#[test]
fn copied_static_values_have_no_frame() {
    let (mut s, l) = session();
    s.execute("prop.set", json!({"layer": l.0, "path": "transform/position", "value": [100, 200]})).unwrap();
    s.execute("prop.select", json!({"layer": l.0, "path": "transform/position"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    assert!(text.contains("Transform\tPosition\n\tFrame\tX pixels\tY pixels\tZ pixels\t\n\t\t100\t200\t0\t\n\n"), "{text}");
    assert_eq!(s.state.value_clipboard.len(), 1, "the internal clipboard keeps the value");
}

#[test]
fn a_copied_transform_group_lists_what_the_layer_shows() {
    let (mut s, l) = session();
    let t = frame(&s, 24);
    add_key(&mut s, l, "transform/scale", Tick::ZERO, json!([50, 50, 100]));
    add_key(&mut s, l, "transform/scale", t, json!([150, 150, 100]));
    let transform = s.active_comp().unwrap().layer(l).unwrap().props.sub("transform").unwrap().uid;
    s.execute("prop.select", json!({"layer": l.0, "prop": transform, "selectKeys": false})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    let names: Vec<&str> = text.lines().filter_map(|line| line.strip_prefix("Transform\t")).collect();
    assert_eq!(names, ["Anchor Point", "Position", "Scale", "Rotation", "Opacity"], "a 2D layer has no Orientation or X / Y Rotation");
    assert!(text.contains("\tFrame\tX percent\tY percent\tZ percent\t\n\t0\t50\t50\t100\t\n\t24\t150\t150\t100\t\n"), "{text}");
    // 3D: Orientation and X / Y Rotation. Z Rotation stays Rotation, and Orientation's three
    // values sit under one label, as After Effects writes them.
    edit_layer(&mut s, l, |l| l.switches.three_d = true);
    s.execute("prop.select", json!({"layer": l.0, "prop": transform, "selectKeys": false})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    let names: Vec<&str> = text.lines().filter_map(|line| line.strip_prefix("Transform\t")).collect();
    assert_eq!(names, ["Anchor Point", "Position", "Scale", "Orientation", "X Rotation", "Y Rotation", "Rotation", "Opacity"]);
    assert!(text.contains("Transform\tOrientation\n\tFrame\tX degrees\t\n\t\t0\t0\t0\t\n"), "{text}");
}

fn copy_transform(s: &mut Session, lid: LayerId) -> String {
    let transform = s.active_comp().unwrap().layer(lid).unwrap().props.sub("transform").unwrap().uid;
    s.execute("prop.select", json!({"layer": lid.0, "prop": transform, "selectKeys": false})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.state.keyframe_text.clone().unwrap()
}

fn names(text: &str) -> Vec<&str> {
    text.lines().filter_map(|line| line.strip_prefix("Transform\t")).collect()
}

#[test]
fn properties_the_layer_does_not_use_are_left_out() {
    // Separate Dimensions: the old Position no longer moves the layer.
    let (mut s, l) = session();
    s.execute("prop.set", json!({"layer": l.0, "path": "transform/position", "value": [100, 200]})).unwrap();
    s.execute("prop.separateDimensions", json!({"layer": l.0, "value": true})).unwrap();
    s.execute("prop.set", json!({"layer": l.0, "path": "transform/positionX", "value": 321})).unwrap();
    let text = copy_transform(&mut s, l);
    assert!(!names(&text).contains(&"Position"), "{text}");
    // A one-node camera has no Point of Interest; a two-node one does.
    let cam = LayerId(s.execute("layer.newCamera", json!({"type": "oneNode"})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_transform(&mut s, cam);
    assert!(!names(&text).contains(&"Point of Interest"), "{text}");
    s.execute("layer.cameraSettings", json!({"layer": cam.0, "type": "twoNode"})).unwrap();
    let text = copy_transform(&mut s, cam);
    assert!(names(&text).contains(&"Point of Interest"), "{text}");
}

#[test]
fn separated_position_writes_x_y_and_z_position() {
    let (mut s, l) = session();
    s.execute("prop.separateDimensions", json!({"layer": l.0, "value": true})).unwrap();
    let t = frame(&s, 12);
    add_key(&mut s, l, "transform/positionX", Tick::ZERO, json!(100));
    add_key(&mut s, l, "transform/positionX", t, json!(321));
    s.execute("prop.set", json!({"layer": l.0, "path": "transform/positionY", "value": 200})).unwrap();
    let text = copy_transform(&mut s, l);
    // After Effects labels their single column with no unit.
    assert!(text.contains("Transform\tX Position\n\tFrame\t\t\n\t0\t100\t\n\t12\t321\t\n\nTransform\tY Position\n\tFrame\t\t\n\t\t200\t\n\n"), "{text}");
    assert!(!names(&text).contains(&"Z Position"), "a 2D layer has no Z Position: {text}");
    edit_layer(&mut s, l, |l| l.switches.three_d = true);
    let text = copy_transform(&mut s, l);
    assert!(text.contains("Transform\tZ Position\n\tFrame\t\t\n\t\t0\t\n"), "{text}");
}

#[test]
fn time_remap_writes_source_seconds_at_comp_frames() {
    let (mut s, l) = session();
    let r = s.execute("layer.precompose", json!({"layers": [l.0], "name": "Inner"})).unwrap();
    let l = LayerId(r["layer"].as_u64().unwrap());
    s.execute("layer.select", json!({"layers": [l.0]})).unwrap();
    s.execute("layer.enableTimeRemap", json!({})).unwrap();
    let start = frame(&s, 10);
    edit_layer(&mut s, l, |l| l.start_time = start);
    let t = frame(&s, 6);
    add_key(&mut s, l, "timeRemap", t, json!(2.5));
    s.execute("prop.select", json!({"layer": l.0, "path": "timeRemap"})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    // As After Effects writes it: the key at layer frame 6 of a layer starting at frame 10 is at 16.
    assert!(text.contains("Layer\nTime Remap\n\tFrame\tseconds\t\n\t10\t0\t\n\t16\t2.5\t\n"), "{text}");
}

/// Selects the layer's properties at `paths` and copies them.
fn copy_props(s: &mut Session, lid: LayerId, paths: &[&str]) -> String {
    for (i, path) in paths.iter().enumerate() {
        s.execute("prop.select", json!({"layer": lid.0, "path": path, "add": i > 0, "selectKeys": false})).unwrap();
    }
    s.execute("edit.copy", json!({})).unwrap();
    s.state.keyframe_text.clone().unwrap()
}

#[test]
fn effect_properties_are_written_as_after_effects_does() {
    // The blocks After Effects 2019 wrote for the same effects and values (2019 has no Dropdown
    // Menu Control: that block follows its other popups, written 1-based with no unit).
    let (mut s, l) = session();
    for fx in ["Fill", "Checkbox Control", "Dropdown Menu Control", "Slider Control", "Point Control"] {
        s.execute("effect.apply", json!({"layer": l.0, "effect": fx})).unwrap();
    }
    let t = frame(&s, 12);
    add_key(&mut s, l, "effects/#1/color", Tick::ZERO, json!([1, 0.5, 0.25, 1]));
    add_key(&mut s, l, "effects/#1/color", t, json!([0, 0, 1, 1]));
    add_key(&mut s, l, "effects/#2/checkbox", Tick::ZERO, json!(true));
    add_key(&mut s, l, "effects/#2/checkbox", t, json!(false));
    s.execute("prop.set", json!({"layer": l.0, "path": "effects/#3/menu", "value": 1})).unwrap();
    s.execute("prop.set", json!({"layer": l.0, "path": "effects/#4/slider", "value": 20.5})).unwrap();
    s.execute("prop.set", json!({"layer": l.0, "path": "effects/#5/point", "value": [10, 20]})).unwrap();
    // After Effects writes a renamed effect under the effect's own name.
    let slider = s.active_comp().unwrap().layer(l).unwrap().props.group("effects/#4").unwrap().uid;
    s.execute("prop.renameGroup", json!({"layer": l.0, "prop": slider, "name": "My Slider"})).unwrap();
    let text = copy_props(&mut s, l, &["effects/#1/color", "effects/#2/checkbox", "effects/#3/menu", "effects/#4/slider", "effects/#5/point"]);
    assert!(
        text.contains(
            "Layer\n\
             Effects\tFill #1\tColor #4\n\tFrame\talpha \tred \tgreen \tblue \t\n\t0\t255\t255\t127.5\t63.75\t\n\t12\t255\t0\t0\t255\t\n\n\
             Effects\tCheckbox Control #2\tCheckbox #2\n\tFrame\t\t\n\t0\t1\t\n\t12\t0\t\n\n\
             Effects\tDropdown Menu Control #3\tMenu #2\n\tFrame\t\t\n\t\t2\t\n\n\
             Effects\tSlider Control #4\tSlider #2\n\tFrame\t\t\n\t\t20.5\t\n\n\
             Effects\tPoint Control #5\tPoint #2\n\tFrame\tX pixels\tY pixels\t\n\t\t10\t20\t\n\n"
        ),
        "{text}"
    );
}

#[test]
fn effect_properties_have_after_effects_indices() {
    // After Effects 2019 numbered Fractal Noise's Rotation (in the Transform group) #9,
    // Complexity (after it) #17, Evolution #25, and Opacity (after Evolution Options) #31: a
    // group takes an index where it opens and closes, and its properties are written without
    // it. Set Matte's Use For Matte is #3: the layer parameter before it has no extra index for
    // its source choice.
    let (mut s, l) = session();
    for fx in ["Fractal Noise", "Set Matte"] {
        s.execute("effect.apply", json!({"layer": l.0, "effect": fx})).unwrap();
    }
    let text = copy_props(
        &mut s,
        l,
        &[
            "effects/#1/transform/rotation",
            "effects/#1/complexity",
            "effects/#1/evolution",
            "effects/#1/opacity",
            "effects/#1/blendingMode",
            "effects/#2/takeMatteFrom",
        ],
    );
    for header in [
        "Effects\tFractal Noise #1\tRotation #9\n",
        "Effects\tFractal Noise #1\tComplexity #17\n",
        "Effects\tFractal Noise #1\tEvolution #25\n",
        "Effects\tFractal Noise #1\tOpacity #31\n",
        "Effects\tFractal Noise #1\tBlending Mode #32\n",
        "Effects\tSet Matte #2\tUse For Matte #3\n",
    ] {
        assert!(text.contains(header), "{header}in\n{text}");
    }
}

#[test]
fn effect_parameters_after_effects_numbers_or_labels_otherwise_follow_it() {
    // After Effects 2019: Radial Blur counts a hidden parameter before Amount, which has no
    // unit; 3D Channel Extract lists Anti-alias before Clamp Output; Selective Color's per-color
    // sliders start at #10 and are percent.
    let (mut s, l) = session();
    for fx in ["Radial Blur", "3D Channel Extract", "Selective Color"] {
        s.execute("effect.apply", json!({"layer": l.0, "effect": fx})).unwrap();
    }
    let text = copy_props(
        &mut s,
        l,
        &["effects/#1/amount", "effects/#1/center", "effects/#2/antialias", "effects/#2/invertDepthMap", "effects/#3/details/reds/redsCyan"],
    );
    for block in [
        "Effects\tRadial Blur #1\tAmount #3\n\tFrame\t\t\n",
        "Effects\tRadial Blur #1\tCenter #4\n\tFrame\tX pixels\tY pixels\t\n",
        "Effects\t3D Channel Extract #2\tAnti-alias #5\n\tFrame\t\t\n",
        "Effects\t3D Channel Extract #2\tInvert Depth Map #7\n\tFrame\t\t\n",
        "Effects\tSelective Color #3\tReds Cyan #10\n\tFrame\tpercent\t\n",
    ] {
        assert!(text.contains(block), "{block}in\n{text}");
    }
}

#[test]
fn mask_properties_are_written_as_after_effects_does() {
    // The blocks After Effects 2019 wrote for the same masks and values: masks go by place, not
    // name, and Mask Path is left out.
    let (mut s, l) = session();
    for _ in 0..2 {
        s.execute("mask.new", json!({"layer": l.0, "vertices": [[0, 0], [100, 0], [100, 100]], "closed": true})).unwrap();
    }
    let t = frame(&s, 12);
    for (prop, a, b) in [("feather", json!([5, 5]), json!([10, 20])), ("opacity", json!(50), json!(75)), ("expansion", json!(3), json!(-4))] {
        add_key(&mut s, l, &format!("masks/#1/{prop}"), Tick::ZERO, a);
        add_key(&mut s, l, &format!("masks/#1/{prop}"), t, b);
    }
    s.execute("prop.set", json!({"layer": l.0, "path": "masks/#2/feather", "value": [7, 8]})).unwrap();
    let second = s.active_comp().unwrap().layer(l).unwrap().props.group("masks/#2").unwrap().uid;
    s.execute("prop.renameGroup", json!({"layer": l.0, "prop": second, "name": "Renamed Mask"})).unwrap();
    let text = copy_props(&mut s, l, &["masks/#1/path", "masks/#1/feather", "masks/#1/opacity", "masks/#1/expansion", "masks/#2/feather"]);
    assert!(
        text.contains(
            "Layer\n\
             Masks\tMask #1\tMask Feather\n\tFrame\tX pixels\tY pixels\t\n\t0\t5\t5\t\n\t12\t10\t20\t\n\n\
             Masks\tMask #1\tMask Opacity\n\tFrame\tpercent\t\n\t0\t50\t\n\t12\t75\t\n\n\
             Masks\tMask #1\tMask Expansion\n\tFrame\tpixels\t\n\t0\t3\t\n\t12\t-4\t\n\n\
             Masks\tMask #2\tMask Feather\n\tFrame\tX pixels\tY pixels\t\n\t\t7\t8\t\n\n\
             \nEnd of Keyframe Data\n"
        ),
        "{text}"
    );
}

#[test]
fn text_properties_are_written_as_after_effects_does() {
    // The blocks After Effects 2019 wrote for the same text properties and values.
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 1920, "height": 1080, "frameRate": 23.976})).unwrap();
    let l = LayerId(s.execute("layer.newText", json!({"text": "Hello"})).unwrap()["layer"].as_u64().unwrap());
    s.execute("layer.addTextAnimator", json!({"properties": ["position", "fillColor"]})).unwrap();
    s.execute("layer.addTextAnimator", json!({"properties": ["opacity"]})).unwrap();
    let second = s.active_comp().unwrap().layer(l).unwrap().props.group("text/animators/#2").unwrap().uid;
    s.execute("prop.renameGroup", json!({"layer": l.0, "prop": second, "name": "Fade"})).unwrap();
    let t = frame(&s, 12);
    add_key(&mut s, l, "text/sourceText", Tick::ZERO, json!("Hello"));
    add_key(&mut s, l, "text/pathOptions/firstMargin", Tick::ZERO, json!(0));
    add_key(&mut s, l, "text/pathOptions/firstMargin", t, json!(50));
    add_key(&mut s, l, "text/animators/#1/selectors/#1/start", Tick::ZERO, json!(0));
    add_key(&mut s, l, "text/animators/#1/selectors/#1/start", t, json!(100));
    add_key(&mut s, l, "text/animators/#1/properties/position", Tick::ZERO, json!([0, 0, 0]));
    add_key(&mut s, l, "text/animators/#1/properties/position", t, json!([10, -20, 0]));
    for (path, v) in [
        ("text/animators/#1/selectors/#1/end", json!(80)),
        ("text/animators/#1/selectors/#1/offset", json!(-10)),
        ("text/animators/#1/properties/fillColor", json!([1, 0, 0, 1])),
        ("text/moreOptions/groupingAlignment", json!([10, -20])),
    ] {
        s.execute("prop.set", json!({"layer": l.0, "path": path, "value": v})).unwrap();
    }
    let text = copy_props(
        &mut s,
        l,
        &[
            "text/sourceText",
            "text/pathOptions/firstMargin",
            "text/animators/#1/selectors/#1/start",
            "text/animators/#1/selectors/#1/end",
            "text/animators/#1/selectors/#1/offset",
            "text/animators/#1/selectors/#1/advanced/amount",
            "text/animators/#1/properties/position",
            "text/animators/#1/properties/fillColor",
            "text/animators/#2/properties/opacity",
            "text/moreOptions/groupingAlignment",
        ],
    );
    for block in [
        "Text\tPath Options\tFirst Margin\n\tFrame\t\t\n\t0\t0\t\n\t12\t50\t\n\n",
        "Text\tAnimators\tAnimator #1\tSelectors\tRange Selector #1\tStart #1\n\tFrame\tpercent\t\n\t0\t0\t\n\t12\t100\t\n\n",
        "Text\tAnimators\tAnimator #1\tSelectors\tRange Selector #1\tEnd #2\n\tFrame\tpercent\t\n\t\t80\t\n\n",
        "Text\tAnimators\tAnimator #1\tSelectors\tRange Selector #1\tOffset #3\n\tFrame\tpercent\t\n\t\t-10\t\n\n",
        "Text\tAnimators\tAnimator #1\tProperties\tPosition\n\tFrame\tX pixels\tY pixels\tZ pixels\t\n\t0\t0\t0\t0\t\n\t12\t10\t-20\t0\t\n\n",
        "Text\tAnimators\tAnimator #1\tProperties\tFill Color\n\tFrame\talpha \tred \tgreen \tblue \t\n\t\t255\t255\t0\t0\t\n\n",
        "Text\tAnimators\tAnimator #2\tProperties\tOpacity\n\tFrame\tpercent\t\n\t\t100\t\n\n",
        "Text\tAnimators\tAnimator #1\tSelectors\tRange Selector #1\tAdvanced\tAmount\n\tFrame\tpercent\t\n\t\t100\t\n\n",
        "Text\tMore Options\tGrouping Alignment\n\tFrame\tX percent\tY percent\t\n\t\t10\t-20\t\n\n",
    ] {
        assert!(text.contains(block), "{block}\nin\n{text}");
    }
    assert!(!text.contains("Source Text"), "{text}");
}

#[test]
fn a_2d_layer_writes_the_z_it_shows() {
    // Keys pasted from a 3D layer keep their Z on a 2D one, which ignores it.
    let (mut s, l) = session();
    let src = LayerId(s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap());
    edit_layer(&mut s, src, |l| l.switches.three_d = true);
    add_key(&mut s, src, "transform/position", Tick::ZERO, json!([100, 200, 300]));
    add_key(&mut s, src, "transform/scale", Tick::ZERO, json!([50, 60, 70]));
    s.state.selected_keys = ["transform/position", "transform/scale"].map(|p| KeyRef { layer: src, prop: uid(&s, src, p), time: Tick::ZERO }).to_vec();
    s.execute("keys.copy", json!({})).unwrap();
    s.execute("keys.paste", json!({"layers": [l.0], "time": 0})).unwrap();
    let pasted = s.active_comp().unwrap().layer(l).unwrap().props.prop("transform/position").unwrap().keys[0].value.components();
    assert_eq!(pasted, [100.0, 200.0, 300.0], "the 2D layer keeps the pasted Z");
    s.state.selected_keys = ["transform/position", "transform/scale"].map(|p| KeyRef { layer: l, prop: uid(&s, l, p), time: Tick::ZERO }).to_vec();
    s.execute("keys.copy", json!({})).unwrap();
    let text = s.state.keyframe_text.clone().unwrap();
    assert!(text.contains("Transform\tPosition\n\tFrame\tX pixels\tY pixels\tZ pixels\t\n\t0\t100\t200\t0\t\n"), "{text}");
    assert!(text.contains("Transform\tScale\n\tFrame\tX percent\tY percent\tZ percent\t\n\t0\t50\t60\t100\t\n"), "{text}");
}

#[test]
fn the_header_has_the_source_pixel_aspect() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"width": 1920, "height": 1080, "frameRate": 24, "pixelAspect": 1.2})).unwrap();
    let solid = LayerId(s.execute("layer.newSolid", json!({"width": 200, "height": 100, "pixelAspect": 1.5})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_transform(&mut s, solid);
    assert!(text.contains("\tSource Pixel Aspect Ratio\t1.5\n\tComp Pixel Aspect Ratio\t1.2\n"), "{text}");
    // A text layer: 1 × 1, as After Effects 2019 wrote it, with the comp's pixel aspect.
    let t = LayerId(s.execute("layer.newText", json!({"text": "Hi"})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_transform(&mut s, t);
    assert!(text.contains("\tSource Width\t1\n\tSource Height\t1\n\tSource Pixel Aspect Ratio\t1.2\n"), "{text}");
    // A shape layer has no source of its own: the comp's size.
    let shape = LayerId(s.execute("layer.newShape", json!({})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_transform(&mut s, shape);
    assert!(text.contains("\tSource Width\t1920\n\tSource Height\t1080\n"), "{text}");
}

#[test]
fn keys_from_two_layers_have_no_text() {
    let (mut s, a) = session();
    let b = LayerId(s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap());
    for l in [a, b] {
        add_key(&mut s, l, "transform/opacity", Tick::ZERO, json!(50));
    }
    s.state.selected_keys = [a, b].map(|l| KeyRef { layer: l, prop: uid(&s, l, "transform/opacity"), time: Tick::ZERO }).to_vec();
    s.execute("keys.copy", json!({})).unwrap();
    assert_eq!(s.state.keyframe_text, None, "After Effects copies one layer's keys as text");
    assert_eq!(s.state.key_clipboard.len(), 2, "EffectCraft's own clipboard has both");
    // A copied layer leaves no text behind.
    s.state.selected_keys = vec![KeyRef { layer: a, prop: uid(&s, a, "transform/opacity"), time: Tick::ZERO }];
    s.execute("keys.copy", json!({})).unwrap();
    assert!(s.state.keyframe_text.is_some());
    s.state.selected_keys.clear();
    s.execute("edit.copy", json!({"layers": [a.0]})).unwrap();
    assert_eq!(s.state.keyframe_text, None);
}

#[test]
fn shape_contents_are_written_as_after_effects_does() {
    // Headers and labels After Effects 2019 wrote for the same shape items: items go by kind and
    // place in their group, and a Size has no unit (`X `).
    let (mut s, _) = session();
    let sh = LayerId(s.execute("layer.newShape", json!({"kind": "none"})).unwrap()["layer"].as_u64().unwrap());
    for (kind, group) in [("group", None), ("rectangle", Some("contents/#1")), ("fill", Some("contents/#1")), ("star", None), ("repeater", None)] {
        let mut p = json!({"layer": sh.0, "kind": kind});
        if let Some(g) = group {
            p["group"] = json!(g);
        }
        s.execute("layer.addShapeItem", p).unwrap();
    }
    let text = copy_props(
        &mut s,
        sh,
        &[
            "contents/#1/contents/#1/size",
            "contents/#1/contents/#2/color",
            "contents/#1/transform/position",
            "contents/#2/points",
            "contents/#3/transform/startOpacity",
        ],
    );
    for block in [
        "Contents\tGroup #1\tContents\tRectangle Path #1\tSize\n\tFrame\tX \tY \t\n",
        "Contents\tGroup #1\tContents\tFill #2\tColor\n\tFrame\talpha \tred \tgreen \tblue \t\n",
        "Contents\tGroup #1\tTransform\tPosition\n\tFrame\tX pixels\tY pixels\t\n",
        "Contents\tPolystar Path #2\tPoints\n\tFrame\t\t\n",
        "Contents\tRepeater #3\tTransform\tStart Opacity\n\tFrame\tpercent\t\n",
    ] {
        assert!(text.contains(block), "{block}in\n{text}");
    }
}

#[test]
fn camera_light_and_material_options_are_written_as_after_effects_does() {
    // Blocks After Effects 2019 wrote for these options, defaults included: Zoom and Shadow
    // Diffusion in pixels, Highlight Threshold on 0–1, and the material's shadow choices from 0.
    let (mut s, l) = session();
    let cam = LayerId(s.execute("layer.newCamera", json!({})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_props(&mut s, cam, &["cameraOptions/zoom", "cameraOptions/highlightThreshold"]);
    assert!(text.contains("Camera Options\tZoom\n\tFrame\tpixels\t\n"), "{text}");
    assert!(text.contains("Camera Options\tHighlight Threshold\n\tFrame\t\t\n\t\t1\t\n"), "{text}");
    let light = LayerId(s.execute("layer.newLight", json!({"type": "spot"})).unwrap()["layer"].as_u64().unwrap());
    let text = copy_props(&mut s, light, &["lightOptions/intensity", "lightOptions/shadowDiffusion"]);
    assert!(text.contains("Light Options\tIntensity\n\tFrame\tpercent\t\n\t\t100\t\n"), "{text}");
    assert!(text.contains("Light Options\tShadow Diffusion\n\tFrame\tpixels\t\n\t\t0\t\n"), "{text}");
    edit_layer(&mut s, l, |l| l.switches.three_d = true);
    let text = copy_props(&mut s, l, &["materialOptions/castsShadows", "materialOptions/acceptsShadows", "materialOptions/diffuse"]);
    for block in [
        "Material Options\tCasts Shadows\n\tFrame\t\t\n\t\t0\t\n",
        "Material Options\tAccepts Shadows\n\tFrame\t\t\n\t\t1\t\n",
        "Material Options\tDiffuse\n\tFrame\tpercent\t\n\t\t50\t\n",
    ] {
        assert!(text.contains(block), "{block}in\n{text}");
    }
}
