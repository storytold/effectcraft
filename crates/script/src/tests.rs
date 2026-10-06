//! End-to-end scripts against a headless session: build with the object model, read back through
//! the engine.

use std::sync::Arc;

use effectcraft_engine::Session;
use effectcraft_engine::keyframe::Value as KV;
use effectcraft_engine::project::{Comp, ItemId, Layer};
use serde_json::{Value, json};

use crate::{Outcome, run_code};

fn session() -> Session {
    let mut s = Session { expr: Some(Arc::new(effectcraft_expr::Expressions)), expr_check: Some(effectcraft_expr::check_syntax), ..Default::default() };
    crate::install(&mut s);
    s
}

fn ok(s: &mut Session, code: &str) -> Outcome {
    let o = run_code(s, code, "test.jsx");
    assert!(o.error.is_none(), "script failed: {:?}\noutput: {:?}", o.error, o.output);
    o
}

fn comp_named<'a>(s: &'a Session, name: &str) -> (ItemId, &'a Comp) {
    let it = s.project.items.values().find(|i| i.name == name && i.as_comp().is_some()).unwrap_or_else(|| panic!("no comp {name}"));
    (it.id, it.as_comp().unwrap())
}

fn layer<'a>(c: &'a Comp, name: &str) -> &'a Layer {
    c.layers.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no layer {name}"))
}

#[test]
fn builds_a_comp_end_to_end() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        app.beginUndoGroup("Build");
        var comp = app.project.items.addComp("Main", 640, 360, 1, 4, 25);
        var bg = comp.layers.addSolid([0.2, 0.4, 0.6], "BG", 640, 360, 1);
        var txt = comp.layers.addText("Hello");
        txt.name = "Title";
        var pos = txt.property("ADBE Transform Group").property("ADBE Position");
        pos.setValueAtTime(0, [100, 180]);
        pos.setValueAtTime(2, [540, 180]);
        txt.transform.opacity.expression = "50 + 25";
        var blur = bg.Effects.addProperty("ADBE Gaussian Blur 2");
        blur.property("Blurriness").setValue(12);
        var nul = comp.layers.addNull();
        txt.setParentWithJump(nul);
        bg.blendingMode = BlendingMode.MULTIPLY;
        var rq = app.project.renderQueue.items.add(comp);
        rq.outputModule(1).file = new File("/tmp/effectcraft-script-test/out.mp4");
        app.endUndoGroup();
        writeLn("layers: " + comp.numLayers);
        [comp.numLayers, pos.numKeys, pos.keyValue(2)[0], txt.transform.opacity.value, comp.layer(1).name]
        "#,
    );
    assert_eq!(o.output, vec!["layers: 3"]);
    assert_eq!(o.result, json!([3, 2, 540, 75, "Null 1"]));
    let (cid, c) = comp_named(&s, "Main");
    assert_eq!((c.width, c.height), (640, 360));
    assert_eq!(c.duration.seconds(), 4.0);
    // AE adds new layers at the top.
    let names: Vec<&str> = c.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Null 1", "Title", "BG"]);
    let title = layer(c, "Title");
    let p = title.props.prop("transform/position").unwrap();
    assert_eq!(p.keys.len(), 2);
    assert_eq!(p.keys[1].value, KV::Vec3([540.0, 180.0, 0.0]));
    assert_eq!(title.props.prop("transform/opacity").unwrap().expr.as_ref().unwrap().text, "50 + 25");
    assert_eq!(title.parent, Some(layer(c, "Null 1").id));
    let bg = layer(c, "BG");
    assert_eq!(bg.blend_mode, effectcraft_engine::color::BlendMode::Multiply);
    let fx = bg.props.group("effects/#1").unwrap();
    assert_eq!(fx.match_id, "ec.blur.gaussian");
    assert_eq!(bg.props.prop("effects/#1/blurriness").unwrap().value, KV::Scalar(12.0));
    assert_eq!(s.project.render_queue.len(), 1);
    assert_eq!(s.project.render_queue[0].comp, cid);
    // One undo step for the whole group.
    assert_eq!(s.history.undo.len(), 1);
    assert_eq!(s.history.undo[0].0, "Build");
    assert!(s.undo());
    assert!(s.project.items.is_empty());
}

#[test]
fn add_solid_retains_source_pixel_aspect_and_optional_duration() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("PAR", 100, 100, 2, 3, 30);
        var square = c.layers.addSolid([1, 0, 0], "Square", 20, 10, 1);
        var wide = c.layers.addSolid([0, 1, 0], "Wide", 20, 10, 1.5, 0.5);
        var inherited = c.layers.addSolid([0, 0, 1], "Inherited", 20, 10);
        var minimum = c.layers.addSolid([1, 1, 1], "Minimum", 20, 10, 0.01);
        var maximum = c.layers.addSolid([1, 1, 1], "Maximum", 20, 10, 100);
        [square.source.pixelAspect, wide.source.pixelAspect, inherited.source.pixelAspect,
         square.outPoint, wide.outPoint, inherited.outPoint, c.pixelAspect,
         minimum.source.pixelAspect, maximum.source.pixelAspect]
        "#,
    );
    assert_eq!(o.result, json!([1, 1.5, 2, 3, 0.5, 3, 2, 0.01, 100]));
    let (_, c) = comp_named(&s, "PAR");
    for (name, expected) in [("Square", 1.0), ("Wide", 1.5), ("Inherited", 2.0), ("Minimum", 0.01), ("Maximum", 100.0)] {
        let effectcraft_engine::project::LayerSource::Solid { item } = layer(c, name).source else { panic!("expected solid") };
        let effectcraft_engine::project::ItemKind::Solid(solid) = &s.project.item(item).unwrap().kind else { panic!("expected solid source") };
        assert_eq!(solid.pixel_aspect, expected);
    }
    assert_eq!(layer(c, "Wide").out_point.seconds(), 0.5);
}

#[test]
fn add_solid_rejects_invalid_pixel_aspect_before_creating_items() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("PAR", 100, 100, 2, 3, 30);
        var invalid = [0, -1, NaN, Infinity, -Infinity, true, 0.009, 100.001, null];
        var rejected = 0;
        for (var i = 0; i < invalid.length; i++) {
            try { c.layers.addSolid([1, 0, 0], "Invalid", 20, 10, invalid[i]); }
            catch (e) { rejected++; }
        }
        [rejected, c.numLayers, app.project.numItems]
        "#,
    );
    assert_eq!(o.result, json!([9, 0, 1]));
    assert_eq!(s.project.items.len(), 1);
    assert!(s.history.undo.iter().all(|entry| entry.0 != "New Solid"));
}

#[test]
fn fill_parameters_use_documented_match_names() {
    // M13.15: `ADBE Fill-0002` is Fill's Color (All Masks is -0007), not the second row.
    let mut s = session();
    let o = ok(
        &mut s,
        r#"var c = app.project.items.addComp("F", 100, 100, 1, 1, 12); var l = c.layers.addSolid([1,1,1], "S", 100, 100, 1);
           var fx = l.property("ADBE Effect Parade").addProperty("ADBE Fill");
           [fx.property("ADBE Fill-0002").name, fx.property("ADBE Fill-0007").name, fx.property(2).matchName].join("|")"#,
    );
    assert_eq!(o.result, json!("Color|All Masks|ADBE Fill-0007"));
}

#[test]
fn undo_steps_without_a_group() {
    let mut s = session();
    ok(&mut s, r#"var c = app.project.items.addComp("A", 100, 100, 1, 1, 24); c.layers.addNull(); c.layers.addNull();"#);
    assert_eq!(s.history.undo.len(), 3);
    // An unbalanced group is closed when the script ends.
    ok(&mut s, r#"app.beginUndoGroup("Two"); var c = app.project.item(1); c.layers.addNull(); c.layers.addNull();"#);
    assert_eq!(s.history.undo.len(), 4);
    assert_eq!(s.history.undo[3].0, "Two");
    // Nested groups fold into the outermost one.
    ok(
        &mut s,
        r#"app.beginUndoGroup("Outer"); app.beginUndoGroup("Inner"); app.project.item(1).layers.addNull(); app.endUndoGroup(); app.project.item(1).layers.addNull(); app.endUndoGroup();"#,
    );
    assert_eq!(s.history.undo.len(), 5);
    assert_eq!(s.history.undo[4].0, "Outer");
    s.undo();
    assert_eq!(s.project.comp(ItemId(1)).unwrap().layers.len(), 4);
}

#[test]
fn errors_report_line_and_column() {
    let mut s = session();
    let o = run_code(&mut s, "var a = 1;\nvar b = 2;\nfoo.bar();\n", "broken.jsx");
    let e = o.error.expect("error");
    assert_eq!(e.line, Some(3), "{e:?}");
    assert!(e.message.contains("foo"), "{e:?}");
    assert_eq!(e.file, "broken.jsx");
    let o = run_code(&mut s, "var a = 1;\n\nvar = ;\n", "syntax.jsx");
    let e = o.error.expect("syntax error");
    assert_eq!(e.line, Some(3), "{e:?}");
    let o = run_code(&mut s, "writeLn('before');\nthrow new Error('custom failure');\n", "throw.jsx");
    let e = o.error.expect("thrown");
    assert_eq!(e.message, "custom failure");
    assert_eq!(e.line, Some(2), "{e:?}");
    assert_eq!(o.output, vec!["before"]);
    // Errors from the object model point at the user's line.
    let o = run_code(&mut s, "var c = app.project.items.addComp('C', 10, 10, 1, 1, 24);\n\nc.layer(5);\n", "range.jsx");
    let e = o.error.expect("range error");
    assert!(e.message.contains("out of range"), "{e:?}");
    assert_eq!(e.line, Some(3), "{e:?}");
}

#[test]
fn match_names_and_lookups() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("M", 200, 200, 1, 2, 30);
        var l = c.layers.addSolid([1, 1, 1], "S", 200, 200, 1);
        var tr = l.property("ADBE Transform Group");
        var fx = l.property("ADBE Effect Parade").addProperty("Gaussian Blur");
        var m = l.property("ADBE Mask Parade").addProperty("ADBE Mask Atom");
        var sh = c.layers.addShape();
        var grp = sh.property("ADBE Root Vectors Group").addProperty("ADBE Vector Group");
        var rect = grp.property("ADBE Vectors Group").addProperty("ADBE Vector Shape - Rect");
        var fill = grp.property("Contents").addProperty("ADBE Vector Graphic - Fill");
        fill.property("ADBE Vector Fill Color").setValue([1, 0, 0]);
        rect.property("ADBE Vector Rect Size").setValue([50, 60]);
        [
          l.matchName, tr.matchName, tr.property("ADBE Position").matchName, tr.property("Anchor Point").matchName,
          tr.property(1).name, tr.anchorPoint.matchName, l.transform.position.propertyValueType == PropertyValueType.ThreeD_SPATIAL,
          fx.matchName, fx.property(1).matchName, fx.property("ADBE Gaussian Blur 2-0001").name, m.matchName,
          m.property("ADBE Mask Shape").name, sh.matchName, grp.matchName, rect.matchName, fill.matchName,
          tr.propertyType == PropertyType.NAMED_GROUP, l.Effects.propertyType == PropertyType.INDEXED_GROUP,
          tr.position.parentProperty.matchName, tr.position.propertyDepth, l.property("Nope") === null,
          fx.isEffect, m.isMask, l.Effects.numProperties
        ]
        "#,
    );
    assert_eq!(
        o.result,
        json!([
            "ADBE AV Layer",
            "ADBE Transform Group",
            "ADBE Position",
            "ADBE Anchor Point",
            "Anchor Point",
            "ADBE Anchor Point",
            true,
            "ADBE Gaussian Blur 2",
            "ADBE Gaussian Blur 2-0001",
            "Blurriness",
            "ADBE Mask Atom",
            "Mask Path",
            "ADBE Vector Layer",
            "ADBE Vector Group",
            "ADBE Vector Shape - Rect",
            "ADBE Vector Graphic - Fill",
            true,
            true,
            "ADBE Transform Group",
            2,
            true,
            true,
            true,
            1
        ])
    );
    let (_, c) = comp_named(&s, "M");
    let sh = c.layers.iter().find(|l| l.props.sub("contents").is_some()).unwrap();
    assert_eq!(sh.props.prop("contents/#1/contents/rect/size").unwrap().value, KV::Vec2([50.0, 60.0]));
    assert_eq!(sh.props.prop("contents/#1/contents/fill/color").unwrap().value, KV::Color([1.0, 0.0, 0.0, 1.0]));
}

#[test]
fn keyframes_eases_and_interpolation() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("K", 100, 100, 1, 5, 25);
        var l = c.layers.addNull();
        var r = l.transform.rotation;
        r.setValuesAtTimes([0, 1, 2], [0, 90, 180]);
        r.setInterpolationTypeAtKey(2, KeyframeInterpolationType.HOLD, KeyframeInterpolationType.LINEAR);
        r.setTemporalEaseAtKey(3, [new KeyframeEase(0, 75)]);
        var idx = r.addKey(1.5);
        r.removeKey(1);
        var out = [r.numKeys, idx, r.keyTime(1), r.keyInInterpolationType(1) == KeyframeInterpolationType.HOLD,
          r.keyOutInterpolationType(1) == KeyframeInterpolationType.LINEAR, r.keyInTemporalEase(3)[0].influence,
          r.valueAtTime(1, false), r.isTimeVarying, r.nearestKeyIndex(1.9)];
        l.startTime = 1;
        out.push(r.keyTime(1));
        out
        "#,
    );
    let v = o.result.as_array().unwrap();
    assert_eq!(v[0], json!(3));
    assert_eq!(v[1], json!(3));
    assert_eq!(v[2], json!(1));
    assert_eq!(v[3], json!(true));
    assert_eq!(v[4], json!(true));
    assert!((v[5].as_f64().unwrap() - 75.0).abs() < 1e-6, "{v:?}");
    assert_eq!(v[6], json!(90));
    assert_eq!(v[7], json!(true));
    assert_eq!(v[8], json!(3));
    // Moving the layer moves its keys (layer time): key 1 now sits at 2 s comp time.
    assert_eq!(v[9], json!(2));
}

#[test]
fn spatial_tangents_round_trip() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("T", 100, 100, 1, 2, 25);
        var p = c.layers.addNull().transform.position;
        p.setValuesAtTimes([0, 1], [[0, 0], [100, 0]]);
        p.setSpatialTangentsAtKey(1, [0, 0], [30, 40]);
        [p.keyOutSpatialTangent(1)[0], p.keyOutSpatialTangent(1)[1], p.keyInSpatialTangent(1)[0]]
        "#,
    );
    assert_eq!(o.result, json!([30, 40, 0]));
}

#[test]
fn text_documents() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("T", 400, 200, 1, 2, 30);
        var t = c.layers.addText("abc");
        var st = t.property("ADBE Text Properties").property("ADBE Text Document");
        var doc = st.value;
        doc.text = "Changed";
        doc.fontSize = 40;
        doc.fillColor = [1, 0, 0];
        doc.justification = ParagraphJustification.CENTER_JUSTIFY;
        doc.applyStroke = true;
        doc.strokeWidth = 3;
        st.setValue(doc);
        var d2 = t.text.sourceText.value;
        [d2.text, d2.fontSize, d2.fillColor, d2.justification == ParagraphJustification.CENTER_JUSTIFY, d2.strokeWidth, t instanceof TextLayer, t.name]
        "#,
    );
    assert_eq!(o.result, json!(["Changed", 40, [1, 0, 0], true, 3, true, "abc"]));
    let (_, c) = comp_named(&s, "T");
    let KV::Text(d) = &c.layers[0].props.prop("text/sourceText").unwrap().value else { panic!() };
    assert_eq!(d.text, "Changed");
    assert_eq!(d.style_at(0).size, 40.0);
}

#[test]
fn layers_items_and_markers() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var f = app.project.items.addFolder("Comps");
        var c = f.items.addComp("Inner", 100, 100, 1, 3, 30);
        var main = app.project.items.addComp("Outer", 200, 200, 1, 6, 30);
        var nested = main.layers.add(c);
        var a = main.layers.addSolid([1, 0, 0], "A", 50, 50, 1, 2);
        var b = main.layers.addSolid([0, 1, 0], "B", 50, 50, 1);
        b.moveAfter(nested);
        a.enabled = false; a.solo = true; a.locked = true; a.shy = true;
        a.threeDLayer = true;
        var cam = main.layers.addCamera("Cam", [100, 100]);
        var lt = main.layers.addLight("Key", [100, 100]);
        lt.lightType = LightType.SPOT;
        var d = a.duplicate();
        d.name = "A copy";
        var mv = new MarkerValue("hello");
        mv.duration = 0.5;
        main.markerProperty.setValueAtTime(1, mv);
        b.marker.setValueAtTime(0.5, new MarkerValue("layer marker"));
        b.inPoint = 1; b.outPoint = 3;
        var pc = main.layers.precompose([b.index], "Pre", true);
        [c.parentFolder.name, nested.source.name, a.outPoint, a.enabled, a.solo, a.locked, a.shy, a.threeDLayer,
         cam instanceof CameraLayer, lt.lightType == LightType.SPOT, main.markerProperty.numKeys, main.markerProperty.keyValue(1).comment,
         main.markerProperty.keyValue(1).duration, pc.name, pc instanceof CompItem, main.layer("A copy").index, app.project.numItems,
         main.layers.byName("Pre") !== null, f.numItems]
        "#,
    );
    assert_eq!(o.result, json!(["Comps", "Inner", 2, false, true, true, true, true, true, true, 1, "hello", 0.5, "Pre", true, 3, 7, true, 1]));
    let (_, c) = comp_named(&s, "Outer");
    assert_eq!(c.markers.len(), 1);
    assert!(
        c.layers
            .iter()
            .any(|l| l.name == "Key"
                && matches!(l.source, effectcraft_engine::project::LayerSource::Light { kind: effectcraft_engine::project::LightKind::Spot }))
    );
}

#[test]
fn security_gate_for_files() {
    let dir = std::env::temp_dir().join(format!("ec-script-gate-{}", std::process::id()));
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("data.txt"), "inside").unwrap();
    std::fs::write(dir.join("secret.txt"), "outside").unwrap();
    let mut s = session();
    s.path = Some(proj.join("p.ecproj").to_string_lossy().to_string());
    let q = |p: &std::path::Path| serde_json::to_string(&p.to_string_lossy()).unwrap();
    // Reading inside the project folder works.
    let o = ok(&mut s, &format!("var f = new File({}); f.open('r'); var t = f.read(); f.close(); t", q(&proj.join("data.txt"))));
    assert_eq!(o.result, json!("inside"));
    // Outside it (also via `..`) is refused.
    for path in [dir.join("secret.txt"), proj.join("../secret.txt")] {
        let o = run_code(&mut s, &format!("var f = new File({}); f.open('r');", q(&path)), "gate.jsx");
        let e = o.error.expect("blocked");
        assert!(e.message.contains("Allow Scripts to Write Files"), "{e:?}");
    }
    // Writing is refused anywhere, the network too.
    let o = run_code(&mut s, &format!("var f = new File({}); f.open('w'); f.write('x'); f.close();", q(&proj.join("out.txt"))), "gate.jsx");
    assert!(o.error.is_some());
    assert!(!proj.join("out.txt").exists());
    assert!(run_code(&mut s, "new Socket()", "net.jsx").error.is_some());
    assert!(run_code(&mut s, "system.callSystem('ls')", "sys.jsx").error.is_some());
    // With the preference on, scripts can write and read anywhere.
    s.prefs.scripting.allow_scripts_write_files = true;
    ok(&mut s, &format!("var f = new File({}); f.open('w'); f.writeln('written'); f.close();", q(&dir.join("w.txt"))));
    assert_eq!(std::fs::read_to_string(dir.join("w.txt")).unwrap(), "written\n");
    let o = ok(&mut s, &format!("var f = new File({}); f.open('r'); f.readln()", q(&dir.join("secret.txt"))));
    assert_eq!(o.result, json!("outside"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn engine_entry_points() {
    let mut s = session();
    // script.run reports errors in the result, not as a command failure.
    let r = s.execute("script.run", json!({"code": "writeLn('hi'); 1 + 2"})).unwrap();
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["result"], json!(3));
    assert_eq!(r["output"], json!("hi"));
    let r = s.execute("script.run", json!({"code": "\nnope()"})).unwrap();
    assert_eq!(r["ok"], json!(false));
    assert_eq!(r["error"]["line"], json!(2));
    // Scripts can't run scripts.
    let r = s.execute("script.run", json!({"code": "app.run('script.run', {code: '1'})"})).unwrap();
    assert!(r["error"]["message"].as_str().unwrap().contains("scripts can't run scripts"), "{r}");
    // File ▸ Scripts ▸ Run Script File… runs .jsx as JavaScript.
    let dir = std::env::temp_dir().join(format!("ec-script-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("make.jsx");
    std::fs::write(&f, "app.project.items.addComp('From File', 320, 240, 1, 2, 24);").unwrap();
    s.execute("file.runScript", json!({"path": f.to_string_lossy()})).unwrap();
    assert!(s.project.items.values().any(|i| i.name == "From File"));
    std::fs::write(&f, "\n\nbroken(;").unwrap();
    let e = s.execute("file.runScript", json!({"path": f.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("make.jsx (line 3)"), "{e}");
    // JSON command scripts still work.
    let j = dir.join("steps.json");
    std::fs::write(&j, r#"[{"command": "comp.new", "params": {"name": "From Steps"}}]"#).unwrap();
    s.execute("file.runScript", json!({"path": j.to_string_lossy()})).unwrap();
    assert!(s.project.items.values().any(|i| i.name == "From Steps"));
    // Menu commands by name.
    let o = ok(&mut s, "app.findMenuCommandId('New Project') > 0 && app.findMenuCommandId('No Such Thing') === 0");
    assert_eq!(o.result, json!(true));
    let o = ok(&mut s, "app.executeCommand(app.findMenuCommandId('New Project')); app.project.numItems");
    assert_eq!(o.result, json!(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn console_keeps_variables() {
    let mut s = session();
    let req = |code: &'static str| effectcraft_engine::ScriptRequest { code, name: "console", console: true };
    let r = crate::run(&mut s, &req("var counter = 41;"));
    assert!(r.error.is_none(), "{r:?}");
    let r = crate::run(&mut s, &req("counter + 1"));
    assert_eq!(r.result, json!(42));
    let _ = Value::Null;
}

#[test]
fn schedule_task_runs_after_the_script() {
    let mut s = session();
    let o = ok(
        &mut s,
        "app.scheduleTask('writeLn(\"later\")', 100, false); var id = app.scheduleTask('writeLn(\"never\")', 10, false); app.cancelTask(id); writeLn('now');",
    );
    assert_eq!(o.output, vec!["now", "later"]);
}

#[test]
fn layer_switches_timing_and_comp_settings() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var c = app.project.items.addComp("S", 320, 240, 1, 5, 30);
        var matte = c.layers.addText("M");
        var fill = c.layers.addSolid([1, 1, 1], "Fill", 320, 240, 1);
        fill.moveToEnd();
        fill.setTrackMatte(matte, TrackMatteType.LUMA);
        var t1 = fill.trackMatteType == TrackMatteType.LUMA && fill.hasTrackMatte && fill.trackMatteLayer.name == "M";
        fill.stretch = 200;
        fill.label = 3;
        c.width = 400;
        c.duration = 8;
        c.bgColor = [0.1, 0.2, 0.3];
        c.time = 2;
        c.workAreaStart = 1;
        c.workAreaDuration = 3;
        var extra = c.layers.addNull();
        extra.remove();
        fill.selected = true;
        var sel = c.selectedLayers.length;
        fill.transform.opacity.expression = "thisLayer.nope(";
        var err = fill.transform.opacity.expressionError;
        fill.transform.opacity.expressionEnabled = false;
        var copy = c.duplicate();
        var f = app.project.items.addFolder("Bin");
        copy.parentFolder = f;
        [t1, fill.stretch, fill.label, c.width, c.duration, c.bgColor[2], c.time, c.workAreaStart, c.workAreaDuration,
         c.numLayers, sel, err.length > 0, fill.transform.opacity.expressionEnabled, copy.name, f.numItems, copy.parentFolder.name]
        "#,
    );
    let v = o.result;
    assert_eq!(v[0], json!(true), "{v}");
    assert_eq!(v[1], json!(200));
    assert_eq!(v[2], json!(3));
    assert_eq!(v[3], json!(400));
    assert_eq!(v[4], json!(8));
    assert!((v[5].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(v[6], json!(2));
    assert_eq!(v[7], json!(1));
    assert_eq!(v[8], json!(3));
    assert_eq!(v[9], json!(2));
    assert_eq!(v[10], json!(1));
    assert_eq!(v[11], json!(true));
    assert_eq!(v[12], json!(false));
    assert_eq!(v[13], json!("S 2"));
    assert_eq!(v[14], json!(1));
    assert_eq!(v[15], json!("Bin"));
    // Removing the copy through the object model.
    let o = ok(&mut s, "var n = app.project.numItems; app.project.item(1).remove(); n - app.project.numItems");
    assert_eq!(o.result, json!(1));
}

#[test]
fn motion_graphics_template_hooks_add_mirrors_and_controllers() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Card", 64, 64, 1, 2, 30);
        var bg = comp.layers.addSolid([1, 1, 1], "BG", 64, 64, 1);
        var op = bg.transform.opacity;
        var r = [op.canAddToMotionGraphicsTemplate(comp), op.addToMotionGraphicsTemplateAs(comp, "Fade")];
        // Again: a mirror of the same property (a controller of its own).
        r.push(op.addToMotionGraphicsTemplate(comp));
        r.push(comp.motionGraphicsTemplateControllerCount);
        r.push(comp.getMotionGraphicsTemplateControllerName(1), comp.getMotionGraphicsTemplateControllerName(2));
        comp.setMotionGraphicsControllerName(2, "Fade Again");
        r.push(comp.getMotionGraphicsTemplateControllerName(2));
        r
        "#,
    );
    assert_eq!(o.result, json!([true, true, true, 2, "Fade", "Fade", "Fade Again"]));
    let (_, c) = comp_named(&s, "Card");
    let eg = c.essential.as_ref().unwrap();
    assert!(matches!(eg.controls[1].kind, effectcraft_engine::project::essential::EgKind::Mirror { of } if of == eg.controls[0].id));
}

#[test]
fn comp_preserves_nested_frame_rate_and_resolution() {
    let mut s = session();
    let o = ok(
        &mut s,
        r#"
        var comp = app.project.items.addComp("Nested", 320, 180, 1, 2, 12);
        var before = [comp.preserveNestedFrameRate, comp.preserveNestedResolution];
        comp.preserveNestedFrameRate = true;
        comp.preserveNestedResolution = true;
        before.concat([comp.preserveNestedFrameRate, comp.preserveNestedResolution]).join(",");
        "#,
    );
    assert_eq!(o.result, json!("false,false,true,true"));
    let (_, c) = comp_named(&s, "Nested");
    assert!(c.preserve_frame_rate && c.preserve_resolution);
}
