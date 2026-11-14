//! Essential Properties overrides of a Source Text follow restyles of their master text layer
//! (#3186): an instance keeps its own characters and the attributes it changed itself, every
//! other attribute follows the precomp's text layer, which is what After Effects users expect.

use effectcraft_keyframe::Value as KV;
use effectcraft_keyframe::{Justify, TextDoc};
use effectcraft_project::essential;
use effectcraft_project::{ItemId, LayerId, Project};
use serde_json::json;

use crate::Session;

/// An instance's Essential Properties value for `control`, and whether it is overridden.
fn instance_value(s: &Session, comp: ItemId, inst: u64, control: u64) -> (TextDoc, bool) {
    let layer = s.project.comp(comp).unwrap().layer(LayerId(inst)).unwrap();
    let over = essential::overridden(layer);
    let mut out = None;
    essential::group(layer).unwrap().walk("", &mut |_, p| {
        if essential::control_of(&p.match_id) == Some(control) {
            out = Some((p.value.clone(), over.contains(&p.uid)));
        }
    });
    match out {
        Some((KV::Text(d), overridden)) => (*d, overridden),
        o => panic!("no Source Text control {control}: {o:?}"),
    }
}

/// The document the nested text layer `text` of `src` renders in instance `inst` of `comp`
/// (`essential::with_overrides`, as the renderer does).
fn rendered_text(s: &Session, comp: ItemId, inst: u64, src: ItemId, text: u64) -> TextDoc {
    let layer = s.project.comp(comp).unwrap().layer(LayerId(inst)).unwrap();
    let over_uids = essential::overridden(layer);
    let mut over = vec![];
    essential::group(layer).unwrap().walk("", &mut |_, p| {
        if let Some(c) = essential::control_of(&p.match_id)
            && over_uids.contains(&p.uid)
        {
            over.push(essential::Override { control: c, value: p.value.clone() });
        }
    });
    // (No override: the nested comp renders as it is.)
    let p = essential::with_overrides(&s.project, src, &over).unwrap_or_else(|| (*s.project).clone());
    match p.comp(src).unwrap().layer(LayerId(text)).unwrap().props.prop("text/sourceText").map(|x| x.value.clone()) {
        Some(KV::Text(d)) => *d,
        o => panic!("{o:?}"),
    }
}

fn master_text(s: &Session, comp: ItemId, text: u64) -> TextDoc {
    match s.project.comp(comp).unwrap().layer(LayerId(text)).unwrap().props.prop("text/sourceText").map(|x| x.value.clone()) {
        Some(KV::Text(d)) => *d,
        o => panic!("{o:?}"),
    }
}

fn assert_fill(d: &TextDoc, rgb: [u8; 3]) {
    for (i, c) in rgb.iter().enumerate() {
        assert!((d.fill[i] - f32::from(*c) / 255.0).abs() < 1e-4, "fill {:?} is not {rgb:?}", d.fill);
    }
}

/// A precomp `Bubble` with a text layer whose Source Text is in Essential Graphics, and a comp
/// `Main` (active) nesting it once: (session, bubble, text layer, control, main, instance).
fn bubble(text_params: serde_json::Value) -> (Session, ItemId, u64, u64, ItemId, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Bubble", "width": 1000, "height": 300, "frameRate": 30, "duration": 1})).unwrap();
    let bubble = s.active_comp_id().unwrap();
    let text = s.execute("layer.newText", text_params).unwrap()["layer"].as_u64().unwrap();
    let body = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.new", json!({"name": "Main", "width": 1000, "height": 300, "frameRate": 30, "duration": 1})).unwrap();
    let main = s.active_comp_id().unwrap();
    let inst = s.execute("layer.addItem", json!({"item": bubble.0})).unwrap()["layer"].as_u64().unwrap();
    (s, bubble, text, body, main, inst)
}

/// Set an instance's Source Text override to a whole document (as the panel sends it).
fn set_doc(s: &mut Session, inst: u64, control: u64, doc: &TextDoc) {
    s.execute("essential.set", json!({"layer": inst, "control": control, "value": serde_json::to_value(doc).unwrap()})).unwrap();
}

/// The issue's steps: the instance overrides the text, then the master text layer gets a new
/// fill, size and justification. The instance shows its own sentence in the new style. It used
/// to keep the whole document from the moment of the override.
#[test]
fn overridden_source_text_follows_a_restyle_of_the_master() {
    let (mut s, bubble, text, body, main, inst) = bubble(
        json!({"text": "Placeholder", "font": "Inter", "style": "SemiBold", "size": 54, "fill": "#2471a3", "justify": "left", "box": [90, 75, 820, 150]}),
    );
    let sentence = "It\u{2019}s tough to get a\ngood rally going.";
    s.execute("essential.set", json!({"layer": inst, "control": body, "value": sentence})).unwrap();
    let (own, overridden) = instance_value(&s, main, inst, body);
    assert!(overridden);
    assert_eq!((own.text.as_str(), own.size), (sentence, 54.0));

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#011f26", "size": 64, "justify": "right"})).unwrap();

    let (own, overridden) = instance_value(&s, main, inst, body);
    assert!(overridden, "the instance keeps its override");
    for d in [&own, &rendered_text(&s, main, inst, bubble, text)] {
        assert_eq!(d.text, sentence, "the instance keeps its own text");
        assert_fill(d, [0x01, 0x1f, 0x26]);
        assert_eq!(d.size, 64.0);
        assert_eq!(d.paras().iter().map(|p| p.justify).collect::<Vec<_>>(), vec![Justify::Right; 2]);
        assert_eq!((d.font.as_str(), d.style.as_str(), d.box_size), ("Inter", "SemiBold", Some([820.0, 150.0])));
    }
    assert_eq!(master_text(&s, bubble, text).text, "Placeholder", "the master keeps its own text");
}

/// An attribute the instance changed itself stays the instance's when the master is restyled;
/// the attributes it left alone follow the master.
#[test]
fn overridden_source_text_keeps_the_attributes_the_instance_changed() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    let (mut doc, _) = instance_value(&s, main, inst, body);
    doc.set_text("Mine");
    doc.fill = [1.0, 0.0, 0.0, 1.0];
    set_doc(&mut s, inst, body, &doc);

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#011f26", "size": 64})).unwrap();

    let (own, overridden) = instance_value(&s, main, inst, body);
    assert!(overridden);
    for d in [&own, &rendered_text(&s, main, inst, bubble, text)] {
        assert_eq!(d.text, "Mine");
        assert_eq!(d.fill, [1.0, 0.0, 0.0, 1.0], "the instance's own fill stays");
        assert_eq!(d.size, 64.0, "the size it left alone follows the master");
    }
}

/// The master's fill passes through the instance's own value (blue → red → green): the
/// instance keeps its red, and keeps following the master's size.
#[test]
fn own_attribute_survives_the_master_passing_through_its_value() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#0000ff"}));
    let (mut doc, _) = instance_value(&s, main, inst, body);
    doc.set_text("Mine");
    doc.fill = [1.0, 0.0, 0.0, 1.0];
    set_doc(&mut s, inst, body, &doc);

    for (fill, size) in [("#ff0000", 60), ("#00ff00", 70)] {
        s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": fill, "size": size})).unwrap();
        let (own, _) = instance_value(&s, main, inst, body);
        assert_eq!(own.fill, [1.0, 0.0, 0.0, 1.0], "after the master's {fill}");
        assert_eq!(own.size, f64::from(size));
        assert_eq!(own.text, "Mine");
    }
}

/// The instance owns a red fill, the master moves to green, then the instance is set back to
/// the master's old blue: blue is what the user set, so it stays (it used to be taken for
/// following the master and turned green in the same edit).
#[test]
fn setting_the_masters_old_value_keeps_it_as_the_instances_own() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#0000ff"}));
    let (mut doc, _) = instance_value(&s, main, inst, body);
    doc.set_text("Mine");
    doc.fill = [1.0, 0.0, 0.0, 1.0];
    set_doc(&mut s, inst, body, &doc);
    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#00ff00", "size": 60})).unwrap();
    let (mut doc, _) = instance_value(&s, main, inst, body);
    assert_eq!((doc.fill, doc.size), ([1.0, 0.0, 0.0, 1.0], 60.0));

    doc.fill = [0.0, 0.0, 1.0, 1.0];
    set_doc(&mut s, inst, body, &doc);
    let (own, _) = instance_value(&s, main, inst, body);
    assert_eq!(own.fill, [0.0, 0.0, 1.0, 1.0], "the blue just set stays");

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#ff0000", "size": 70})).unwrap();
    let (own, _) = instance_value(&s, main, inst, body);
    assert_eq!((own.text.as_str(), own.fill, own.size), ("Mine", [0.0, 0.0, 1.0, 1.0], 70.0));
}

/// A keyframed override: each key merges with the master on its own. One key owns its fill, the
/// other follows; both follow the size.
#[test]
fn keyframed_override_merges_every_key() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    let (doc, _) = instance_value(&s, main, inst, body);
    let uid = child_uid(essential::group(s.project.comp(main).unwrap().layer(LayerId(inst)).unwrap()).unwrap(), body);
    let first = TextDoc { text: "First".into(), fill: [1.0, 0.0, 0.0, 1.0], ..doc.clone() };
    let second = TextDoc { text: "Second".into(), ..doc.clone() };
    s.edit("Keys", None, |p, _| {
        let pr = p.comp_mut(main).unwrap().layer_mut(LayerId(inst)).unwrap().props.find_mut(uid).unwrap();
        pr.keys = vec![
            effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::ZERO, KV::Text(Box::new(first.clone()))),
            effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::from_seconds_f64(0.5), KV::Text(Box::new(second.clone()))),
        ];
        Ok(())
    })
    .unwrap();
    assert!(instance_value(&s, main, inst, body).1, "keying the property overrides it");

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#011f26", "size": 64})).unwrap();

    let layer = s.project.comp(main).unwrap().layer(LayerId(inst)).unwrap();
    let keys: Vec<TextDoc> = layer
        .props
        .find(uid)
        .unwrap()
        .keys
        .iter()
        .map(|k| match &k.value {
            KV::Text(d) => (**d).clone(),
            o => panic!("{o:?}"),
        })
        .collect();
    assert_eq!(keys.len(), 2);
    assert_eq!((keys[0].text.as_str(), keys[0].fill, keys[0].size), ("First", [1.0, 0.0, 0.0, 1.0], 64.0));
    assert_eq!((keys[1].text.as_str(), keys[1].size), ("Second", 64.0));
    assert_fill(&keys[1], [0x01, 0x1f, 0x26]);
}

/// A keyframed override where one key owns its fill and the other follows: the master's fill
/// passes through the owned value (red, then green). Each key has its own base, so the owned red
/// stays (with one base shared by the keys, the follower moved it to red and the owner then
/// counted as following).
#[test]
fn keyframed_override_keeps_a_keys_own_fill_when_the_master_passes_through_it() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    let (doc, _) = instance_value(&s, main, inst, body);
    let uid = child_uid(essential::group(s.project.comp(main).unwrap().layer(LayerId(inst)).unwrap()).unwrap(), body);
    let red = [1.0, 0.0, 0.0, 1.0];
    let first = TextDoc { text: "First".into(), fill: red, ..doc.clone() };
    let second = TextDoc { text: "Second".into(), ..doc.clone() };
    s.edit("Keys", None, |p, _| {
        let pr = p.comp_mut(main).unwrap().layer_mut(LayerId(inst)).unwrap().props.find_mut(uid).unwrap();
        pr.keys = vec![
            effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::ZERO, KV::Text(Box::new(first.clone()))),
            effectcraft_keyframe::Keyframe::new(effectcraft_time::Tick::from_seconds_f64(0.5), KV::Text(Box::new(second.clone()))),
        ];
        Ok(())
    })
    .unwrap();

    let keys = |s: &Session| -> Vec<TextDoc> {
        let layer = s.project.comp(main).unwrap().layer(LayerId(inst)).unwrap();
        layer
            .props
            .find(uid)
            .unwrap()
            .keys
            .iter()
            .map(|k| match &k.value {
                KV::Text(d) => (**d).clone(),
                o => panic!("{o:?}"),
            })
            .collect()
    };
    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#ff0000", "size": 60})).unwrap();
    let k = keys(&s);
    assert_eq!((k[0].fill, k[1].fill, k[1].size), (red, red, 60.0));

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#00ff00", "size": 70})).unwrap();
    let k = keys(&s);
    assert_eq!((k[0].text.as_str(), k[0].fill, k[0].size), ("First", red, 70.0), "the first key's own red stays");
    assert_eq!((k[1].text.as_str(), k[1].fill, k[1].size), ("Second", [0.0, 1.0, 0.0, 1.0], 70.0));
}

/// Precomps three deep, the outer comp created first (so visited first): the outer instance's
/// override of the middle comp's control (which shows the inner text) follows a restyle of the
/// inner text layer in the same edit.
#[test]
fn restyle_reaches_an_override_two_precomps_up() {
    let mut s = Session::default();
    let new_comp = |s: &mut Session, name: &str| {
        s.execute("comp.new", json!({"name": name, "width": 400, "height": 200, "frameRate": 30, "duration": 1})).unwrap();
        s.active_comp_id().unwrap()
    };
    let outer = new_comp(&mut s, "Outer");
    let middle = new_comp(&mut s, "Middle");
    let inner = new_comp(&mut s, "Inner");
    assert!(outer < middle && middle < inner);
    let text = s.execute("layer.newText", json!({"text": "Inner", "size": 54, "fill": "#2471a3"})).unwrap()["layer"].as_u64().unwrap();
    let inner_ctl = s.execute("essential.addProperty", json!({"layer": text, "path": "text/sourceText"})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.open", json!({"comp": middle.0})).unwrap();
    let mid_inst = s.execute("layer.addItem", json!({"item": inner.0})).unwrap()["layer"].as_u64().unwrap();
    let uid = s.execute("essential.instance", json!({"layer": mid_inst})).unwrap()["controls"][0]["uid"].as_u64().unwrap();
    let mid_ctl = s.execute("essential.addProperty", json!({"comp": middle.0, "layer": mid_inst, "prop": uid})).unwrap()["controls"][0].as_u64().unwrap();
    s.execute("comp.open", json!({"comp": outer.0})).unwrap();
    let out_inst = s.execute("layer.addItem", json!({"item": middle.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.set", json!({"layer": out_inst, "control": mid_ctl, "value": "Outer words"})).unwrap();
    assert!(instance_value(&s, outer, out_inst, mid_ctl).1);

    s.execute("layer.setText", json!({"comp": inner.0, "layer": text, "fill": "#011f26", "size": 64})).unwrap();

    let (mid, overridden) = instance_value(&s, middle, mid_inst, inner_ctl);
    assert!(!overridden);
    assert_eq!((mid.text.as_str(), mid.size), ("Inner", 64.0));
    let (own, overridden) = instance_value(&s, outer, out_inst, mid_ctl);
    assert!(overridden);
    assert_eq!(own.text, "Outer words");
    assert_fill(&own, [0x01, 0x1f, 0x26]);
    assert_eq!(own.size, 64.0);
}

/// Deleting the master's bold first word changes the style of its first character but is no
/// restyle: the instance's bold stays. A later restyle still reaches it.
#[test]
fn deleting_the_masters_bold_first_word_keeps_the_instance_bold() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Bold rest", "size": 54}));
    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "range": [0, 4], "fauxBold": true})).unwrap();
    // The instance's text takes the style of the master's first character: all bold.
    s.execute("essential.set", json!({"layer": inst, "control": body, "value": "Wow"})).unwrap();
    let (own, _) = instance_value(&s, main, inst, body);
    assert!(own.style_at(0).faux_bold && own.style_at(2).faux_bold);

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "range": [0, 5], "text": ""})).unwrap();
    let master = master_text(&s, bubble, text);
    assert_eq!(master.text, "rest");
    assert!(!master.style_at(0).faux_bold);
    let (own, _) = instance_value(&s, main, inst, body);
    assert!(own.style_at(0).faux_bold && own.style_at(2).faux_bold, "the instance stays bold");

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "size": 64})).unwrap();
    let (own, _) = instance_value(&s, main, inst, body);
    assert_eq!((own.text.as_str(), own.size, own.style_at(0).faux_bold), ("Wow", 64.0, true));
}

/// Two instances, one overriding the text and one not: both show the restyle, each with its
/// text.
#[test]
fn restyle_reaches_overriding_and_plain_instances() {
    let (mut s, bubble, text, body, main, a) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    let b = s.execute("layer.addItem", json!({"item": bubble.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.set", json!({"layer": a, "control": body, "value": "Mine"})).unwrap();

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#011f26", "size": 64})).unwrap();

    for (inst, words, overridden) in [(a, "Mine", true), (b, "Placeholder", false)] {
        let (own, o) = instance_value(&s, main, inst, body);
        assert_eq!(o, overridden);
        for d in [&own, &rendered_text(&s, main, inst, bubble, text)] {
            assert_eq!(d.text, words);
            assert_fill(d, [0x01, 0x1f, 0x26]);
            assert_eq!(d.size, 64.0);
        }
    }
}

/// Push to Comp from one instance restyles the master; another instance that overrides only
/// its text takes that style and keeps its text.
#[test]
fn push_to_comp_from_one_instance_restyles_another() {
    let (mut s, bubble, text, body, main, one) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    let two = s.execute("layer.addItem", json!({"item": bubble.0})).unwrap()["layer"].as_u64().unwrap();
    s.execute("essential.set", json!({"layer": two, "control": body, "value": "Two"})).unwrap();
    let (mut doc, _) = instance_value(&s, main, one, body);
    doc.set_text("One");
    doc.fill = [1.0, 0.0, 0.0, 1.0];
    doc.size = 40.0;
    set_doc(&mut s, one, body, &doc);

    s.execute("essential.pushToComp", json!({"layer": one, "control": body})).unwrap();

    let master = master_text(&s, bubble, text);
    assert_eq!((master.text.as_str(), master.fill, master.size), ("One", [1.0, 0.0, 0.0, 1.0], 40.0));
    let (d1, o1) = instance_value(&s, main, one, body);
    assert_eq!((o1, d1.text.as_str()), (false, "One"));
    let (d2, o2) = instance_value(&s, main, two, body);
    assert!(o2);
    assert_eq!((d2.text.as_str(), d2.fill, d2.size), ("Two", [1.0, 0.0, 0.0, 1.0], 40.0));
}

/// A project saved before overrides kept a base: it loads unchanged (even a style left stale by
/// the old behaviour stays, as the instance's own), and from then on the attributes the
/// instance shares with the master follow the master's restyles.
#[test]
fn old_override_without_a_base_loads_unchanged_and_follows_from_then_on() {
    let (mut s, bubble, text, body, main, inst) = bubble(json!({"text": "Placeholder", "size": 54, "fill": "#2471a3"}));
    s.execute("essential.set", json!({"layer": inst, "control": body, "value": "Mine"})).unwrap();
    // As an older version saved it: a fill left from an earlier master style, and no base.
    let mut p: Project = (*s.project).clone();
    let g = p.comp_mut(main).unwrap().layer_mut(LayerId(inst)).unwrap().props.sub_mut(essential::GROUP).unwrap();
    let uid = child_uid(g, body);
    match &mut g.find_mut(uid).unwrap().value {
        KV::Text(d) => d.fill = [0.5, 0.5, 0.5, 1.0],
        o => panic!("{o:?}"),
    }
    let mut json: serde_json::Value = serde_json::from_str(&p.to_json()).unwrap();
    strip_bases(&mut json);
    let loaded = Project::from_json(&json.to_string()).unwrap();
    s.replace_project(loaded, None);

    let (own, overridden) = instance_value(&s, main, inst, body);
    assert!(overridden);
    assert_eq!((own.text.as_str(), own.fill, own.size), ("Mine", [0.5, 0.5, 0.5, 1.0], 54.0), "loads unchanged");

    s.execute("layer.setText", json!({"comp": bubble.0, "layer": text, "fill": "#011f26", "size": 64})).unwrap();
    let (own, _) = instance_value(&s, main, inst, body);
    assert_eq!((own.text.as_str(), own.fill, own.size), ("Mine", [0.5, 0.5, 0.5, 1.0], 64.0));
}

/// Remove the bases of every Essential Properties group from a saved project.
fn strip_bases(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(o) => {
            if o.get("kind").and_then(serde_json::Value::as_str) == Some("Essential") {
                o.remove("bases");
            }
            for x in o.values_mut() {
                strip_bases(x);
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                strip_bases(x);
            }
        }
        _ => {}
    }
}

/// Uid of the Essential Properties child of `control` in group `g`.
fn child_uid(g: &effectcraft_project::PropGroup, control: u64) -> u64 {
    let mut uid = None;
    g.walk("", &mut |_, p| {
        if essential::control_of(&p.match_id) == Some(control) {
            uid = Some(p.uid);
        }
    });
    uid.unwrap()
}
