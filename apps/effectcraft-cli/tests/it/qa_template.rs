//! M13.15 scenario 5 — Essential Graphics through the CLI: a MOGRT-style template built by an
//! After Effects-style script (`effectcraft-cli script`), its controls changed with commands on
//! an instance, rendered, exported as a template and imported into a fresh project. Direct
//! template rendering uses the same original, procedural fixture under the project licence.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn cli(dir: &PathBuf, args: &[&str]) -> (i32, Value) {
    let out = Command::new(crate::binary()).current_dir(dir).args(args).arg("--json").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let v = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout} {}", String::from_utf8_lossy(&out.stderr)));
    (out.status.code().unwrap_or(-1), v)
}

fn ok(dir: &PathBuf, args: &[&str]) -> Value {
    let (code, v) = cli(dir, args);
    assert_eq!(code, 0, "{args:?}: {v}");
    v
}

const BUILD: &str = r#"
app.beginUndoGroup("Build Template");
var comp = app.project.items.addComp("Lower Third", 320, 90, 1, 2, 12);
var bar = comp.layers.addSolid([1, 1, 1], "Bar", 320, 40, 1, 2);
bar.property("ADBE Transform Group").property("ADBE Position").setValue([160, 65]);
var t = comp.layers.addText("Jane Doe");
t.name = "Name";
t.property("ADBE Transform Group").property("ADBE Position").setValue([160, 30]);
var fill = bar.property("ADBE Effect Parade").addProperty("ADBE Fill");
t.property("ADBE Text Properties").property("ADBE Text Document").addToMotionGraphicsTemplateAs(comp, "Title");
fill.property("ADBE Fill-0002").addToMotionGraphicsTemplateAs(comp, "Bar Color");
bar.property("ADBE Transform Group").property("ADBE Opacity").addToMotionGraphicsTemplateAs(comp, "Bar Opacity");
comp.motionGraphicsTemplateName = "Lower Third";
app.endUndoGroup();
writeLn("controls: " + comp.motionGraphicsTemplateControllerCount);
comp.motionGraphicsTemplateName;
"#;

#[test]
fn essential_graphics_template_via_script_and_commands() {
    let dir = std::env::temp_dir().join(format!("ec-qa-template-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();

    let r = ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["output"], "controls: 3");
    let list = ok(&dir, &["exec", "essential.list", r##"{"comp":"Lower Third"}"##, "t.ecproj"]);
    let controls: Vec<(String, String)> =
        list["controls"].as_array().unwrap().iter().map(|c| (c["name"].as_str().unwrap().to_string(), c["type"].as_str().unwrap().to_string())).collect();
    assert_eq!(
        controls,
        [("Title".into(), "text".into()), ("Bar Color".into(), "color".into()), ("Bar Opacity".into(), "slider".into())],
        "ADBE Fill-0002 is the Fill's Color: {list}"
    );

    // An edit comp with an instance; controls addressed by name.
    let r = ok(
        &dir,
        &[
            "run",
            "t.ecproj",
            "comp.new",
            r##"{"name":"Edit","width":320,"height":90,"frameRate":12,"duration":2}"##,
            "layer.addItem",
            r##"{"item":"Lower Third"}"##,
            "comp.new",
            r##"{"name":"Plain","width":320,"height":90,"frameRate":12,"duration":2}"##,
            "layer.addItem",
            r##"{"item":"Lower Third"}"##,
            "comp.open",
            r##"{"comp":"Edit"}"##,
            "essential.set",
            r##"{"layer":"#1","control":"Title","value":"John Smith"}"##,
            "essential.set",
            r##"{"layer":"#1","control":"Bar Color","value":"#00c080"}"##,
            "essential.set",
            r##"{"layer":"#1","control":"bar opacity","value":50}"##,
            "--save",
        ],
    );
    let inst = ok(&dir, &["exec", "essential.instance", r##"{"comp":"Edit","layer":"#1"}"##, "t.ecproj"]);
    assert!(inst["controls"].as_array().unwrap().iter().all(|c| c["overridden"] == true), "{inst}");
    let (code, e) = cli(&dir, &["exec", "essential.set", r##"{"comp":"Edit","layer":"#1","control":"Subtitle","value":"x"}"##, "t.ecproj"]);
    assert_eq!(code, 1);
    assert!(e["error"].as_str().unwrap().contains("controls: Title, Bar Color, Bar Opacity"), "{e} ({r})");

    // Render both instances and compare.
    ok(&dir, &["render-frame", "t.ecproj", "--comp", "Edit", "--time", "1", "--out", "edit.png"]);
    ok(&dir, &["render-frame", "t.ecproj", "--comp", "Plain", "--time", "1", "--out", "plain.png"]);
    let edit = image::open(dir.join("edit.png")).unwrap().to_rgba8();
    let plain = image::open(dir.join("plain.png")).unwrap().to_rgba8();
    let bar = edit.get_pixel(10, 75).0;
    assert!(bar[0] < 15 && (bar[1] as i32 - 96).abs() < 8 && (bar[2] as i32 - 64).abs() < 8, "50 % of #00c080 over black: {bar:?}");
    let pbar = plain.get_pixel(10, 75).0;
    assert!(pbar[0] > 240 && pbar[1] < 15, "the template's own red: {pbar:?}");
    let diff = |y0: u32, y1: u32| -> u32 {
        (y0..y1).flat_map(|y| (0..320).map(move |x| (x, y))).filter(|(x, y)| edit.get_pixel(*x, *y).0 != plain.get_pixel(*x, *y).0).count() as u32
    };
    assert!(diff(10, 45) > 100, "the title text differs");

    // Export the template and bring it into a fresh project.
    let x = ok(&dir, &["exec", "essential.exportTemplate", r##"{"comp":"Lower Third","path":"lt.ectemplate"}"##, "t.ecproj"]);
    assert_eq!(x["controls"], 3, "{x}");
    let info = ok(&dir, &["exec", "essential.templateInfo", r##"{"path":"lt.ectemplate"}"##, "--empty"]);
    assert_eq!(info["name"], "Lower Third", "{info}");
    let imp = ok(&dir, &["run", "--empty", "essential.importTemplate", r##"{"path":"lt.ectemplate"}"##, "--save-as", "fresh.ecproj"]);
    assert!(imp["saved"].is_string(), "{imp}");
    let list = ok(&dir, &["exec", "essential.list", r##"{"comp":"Lower Third"}"##, "fresh.ecproj"]);
    assert_eq!(list["controls"].as_array().unwrap().len(), 3, "{list}");

    // One-shot rendering applies the JSON values to an instance, preserving the template defaults.
    let original_template = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    std::fs::write(dir.join("values.json"), r##"{"Title":"John Smith","Bar Color":"#00c080","bar opacity":50}"##).unwrap();
    ok(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", "direct.png"]);
    assert_eq!(image::open(dir.join("direct.png")).unwrap().to_rgba8(), edit);
    ok(&dir, &["frame", "--template", "lt.ectemplate", "--frame", "12", "--out", "defaults.png"]);
    assert_eq!(image::open(dir.join("defaults.png")).unwrap().to_rgba8(), plain);

    let output = dir.join("sequence/direct_[#####].png");
    let rendered = ok(
        &dir,
        &[
            "render",
            "--template",
            "lt.ectemplate",
            "--values",
            "values.json",
            "--out",
            output.to_str().unwrap(),
            "--format",
            "png",
            "--channels",
            "rgb",
            "--start",
            "1",
            "--end",
            "1.0833333333333333",
        ],
    );
    assert_eq!(rendered["rendered"].as_array().unwrap().len(), 1, "{rendered}");
    let frames: Vec<_> = std::fs::read_dir(dir.join("sequence"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(image::open(&frames[0]).unwrap().to_rgba8(), edit);
    assert_eq!(std::fs::read(dir.join("lt.ectemplate")).unwrap(), original_template);

    // Invalid values fail before the queue creates an output directory, or frame rendering
    // overwrites an existing output. A JSON object and existing control names are required.
    for (name, values, expected) in [
        ("malformed", "{", "invalid JSON"),
        ("array", "[]", "JSON object"),
        ("unknown", r#"{"Subtitle":"x"}"#, "controls: Title, Bar Color, Bar Opacity"),
        ("invalid-value", r#"{"Bar Opacity":"bad"}"#, "can't use"),
    ] {
        let values_file = format!("{name}.json");
        std::fs::write(dir.join(&values_file), values).unwrap();
        let output_dir = dir.join(format!("failed-{name}"));
        let out = output_dir.join("frame_[#####].png");
        let (code, error) = cli(&dir, &["render", "--template", "lt.ectemplate", "--values", &values_file, "--out", out.to_str().unwrap()]);
        assert_eq!(code, 1, "{name}: {error}");
        assert!(error["error"].as_str().unwrap().contains(expected), "{name}: {error}");
        assert!(!output_dir.exists(), "{name}: invalid input created render output");
        std::fs::write(dir.join("untouched.png"), b"existing output").unwrap();
        let (code, error) = cli(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", &values_file, "--out", "untouched.png"]);
        assert_eq!(code, 1, "{name}: {error}");
        assert_eq!(std::fs::read(dir.join("untouched.png")).unwrap(), b"existing output");
    }
    let (code, error) = cli(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "missing-values.json", "--out", "missing.png"]);
    assert_eq!(code, 1, "{error}");
    assert!(error["error"].as_str().unwrap().contains("cannot read values"), "{error}");
    assert!(!dir.join("missing.png").exists());
    std::fs::write(dir.join("large-values.json"), vec![b' '; 1024 * 1024 + 1]).unwrap();
    let (code, error) = cli(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "large-values.json", "--out", "large.png"]);
    assert_eq!(code, 1, "{error}");
    assert!(error["error"].as_str().unwrap().contains("exceeds 1 MiB"), "{error}");
    assert!(!dir.join("large.png").exists());

    for args in [
        vec!["render-frame", "--values", "values.json"],
        vec!["info", "--template", "lt.ectemplate"],
        vec!["render-frame", "--template", "lt.ectemplate", "--comp", "Lower Third"],
        vec!["render-frame", "t.ecproj", "--template", "lt.ectemplate"],
        vec!["render", "--template", "lt.ectemplate", "--queue"],
        vec!["render-frame", "--template", "lt.ectemplate", "--bridge", "1"],
    ] {
        let output = Command::new(crate::binary()).current_dir(&dir).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }

    // File-loaded templates can use exact rational rates that comp.new's float parser rounds.
    // Preserve that rate, Tick duration, work area, background and locked source layers.
    let mut project = effectcraft_engine::project::Project::from_json(&std::fs::read_to_string(dir.join("t.ecproj")).unwrap()).unwrap();
    let source = project.find_by_name("Lower Third").unwrap().id;
    let rate = effectcraft_time::FrameRate::new(24001, 1001);
    let comp = project.comp_mut(source).unwrap();
    comp.frame_rate = rate;
    comp.duration = rate.tick_of(24);
    comp.work_area = (rate.tick_of(3), rate.tick_of(7));
    comp.background = [0.08, 0.16, 0.24];
    comp.layers.first_mut().unwrap().switches.locked = true;
    std::fs::write(dir.join("rational.ecproj"), project.to_json()).unwrap();
    ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Lower Third","path":"rational.ectemplate"}"#, "rational.ecproj"]);
    let rational_template = std::fs::read(dir.join("rational.ectemplate")).unwrap();
    let rational = ok(&dir, &["render-frame", "--template", "rational.ectemplate", "--frame", "5", "--out", "rational.png"]);
    assert!((rational["time"].as_f64().unwrap() - 5.0 / rate.as_f64()).abs() < 1e-9, "{rational}");
    assert_eq!((rational["width"].as_u64().unwrap(), rational["height"].as_u64().unwrap()), (320, 90));
    let output = dir.join("rational-work-area/frame_[#####].png");
    ok(&dir, &["render", "--template", "rational.ectemplate", "--work-area", "--out", output.to_str().unwrap(), "--format", "png", "--resolution", "0.05"]);
    let count = std::fs::read_dir(dir.join("rational-work-area"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "png"))
        .count();
    assert_eq!(count, 4, "the template's exact work area contains frames 3 through 6");
    assert_eq!(std::fs::read(dir.join("rational.ectemplate")).unwrap(), rational_template);
    assert_eq!(std::fs::read(dir.join("lt.ectemplate")).unwrap(), original_template);
    let _ = std::fs::remove_dir_all(&dir);
}
fn template_output_alias_fixture(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ec-qa-template-output-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();
    let built = ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    assert_eq!(built["output"], "controls: 3", "{built}");
    let exported = ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Lower Third","path":"lt.ectemplate"}"#, "t.ecproj"]);
    assert_eq!(exported["controls"], 3, "{exported}");
    std::fs::write(dir.join("values.json"), r##"{"Title":"John Smith","Bar Color":"#00c080","bar opacity":50}"##).unwrap();
    let template_before = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let values_before = std::fs::read(dir.join("values.json")).unwrap();

    // The same valid inputs must still render to a distinct output before testing aliases.
    ok(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", "ordinary.png"]);
    let image = image::open(dir.join("ordinary.png")).unwrap().to_rgba8();
    let bar = image.get_pixel(10, 75).0;
    assert!(bar[0] < 15 && (bar[1] as i32 - 96).abs() < 8 && (bar[2] as i32 - 64).abs() < 8, "50 % of #00c080 over black: {bar:?}");
    assert_eq!(std::fs::read(dir.join("lt.ectemplate")).unwrap(), template_before);
    assert_eq!(std::fs::read(dir.join("values.json")).unwrap(), values_before);
    dir
}

fn assert_template_output_preserves_inputs(dir: &PathBuf, out: &str) -> Value {
    let template_before = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let values_before = std::fs::read(dir.join("values.json")).unwrap();
    // Read the inputs before interpreting the subprocess status or error text.
    let result = Command::new(crate::binary())
        .current_dir(dir)
        .args(["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", out, "--json"])
        .output()
        .unwrap();
    let template_after = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let values_after = std::fs::read(dir.join("values.json")).unwrap();
    let template_preserved = template_after == template_before;
    let values_preserved = values_after == values_before;
    eprintln!(
        "output={out:?} status={:?} template_preserved={template_preserved} values_preserved={values_preserved} template_bytes={}->{} values_bytes={}->{} stdout={} stderr={}",
        result.status.code(),
        template_before.len(),
        template_after.len(),
        values_before.len(),
        values_after.len(),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    );
    assert!(template_preserved, "template rendering must preserve its template input when --out aliases that file");
    assert!(values_preserved, "template rendering must preserve its values input when --out aliases that file");
    assert_eq!(result.status.code(), Some(1), "an output alias must fail before rendering overwrites an input");
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn template_frame_output_cannot_replace_its_template_input() {
    let dir = template_output_alias_fixture("template");
    assert_template_output_preserves_inputs(&dir, "lt.ectemplate");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_frame_output_cannot_replace_its_values_input() {
    let dir = template_output_alias_fixture("values");
    assert_template_output_preserves_inputs(&dir, "values.json");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_frame_output_cannot_alias_its_template_through_parent_components() {
    let dir = template_output_alias_fixture("normalized-template");
    std::fs::create_dir(dir.join("alias")).unwrap();
    let out = "alias/../lt.ectemplate";
    assert_eq!(dir.join(out).canonicalize().unwrap(), dir.join("lt.ectemplate").canonicalize().unwrap());
    assert_template_output_preserves_inputs(&dir, out);
    let _ = std::fs::remove_dir_all(&dir);
}

fn timeline_tail_render_pair(dir: &PathBuf, name: &str, duration: effectcraft_time::Tick) {
    let mut project = effectcraft_engine::project::Project::from_json(&std::fs::read_to_string(dir.join("base.ecproj")).unwrap()).unwrap();
    let source = project.find_by_name("Lower Third").unwrap().id;
    let comp = project.comp_mut(source).unwrap();
    assert_eq!(comp.frame_rate, effectcraft_time::FrameRate::new(12, 1));
    comp.duration = duration;
    comp.work_area = (effectcraft_time::Tick::ZERO, duration);
    for layer in &mut comp.layers {
        layer.out_point = duration;
    }
    assert!(comp.layers.iter().all(|layer| layer.is_active_at(effectcraft_time::Tick::from_seconds_f64(1.0))));
    let project_name = format!("{name}.ecproj");
    let template_name = format!("{name}.ectemplate");
    let source_frame = format!("{name}-source.png");
    let template_frame = format!("{name}-template.png");
    std::fs::write(dir.join(&project_name), project.to_json()).unwrap();
    let params = serde_json::json!({"comp":"Lower Third", "path":template_name}).to_string();
    let exported = ok(dir, &["exec", "essential.exportTemplate", &params, &project_name]);
    assert_eq!(exported["controls"], 3, "{exported}");
    let project_before = std::fs::read(dir.join(&project_name)).unwrap();
    let template_before = std::fs::read(dir.join(&template_name)).unwrap();

    let direct = ok(dir, &["render-frame", &project_name, "--comp", "Lower Third", "--time", "1", "--out", &source_frame]);
    let instance = ok(dir, &["render-frame", "--template", &template_name, "--time", "1", "--out", &template_frame]);
    for result in [&direct, &instance] {
        assert_eq!(result["time"].as_f64(), Some(1.0), "{name}: {result}");
        assert_eq!((result["width"].as_u64(), result["height"].as_u64()), (Some(320), Some(90)), "{name}: {result}");
    }
    assert_eq!(std::fs::read(dir.join(&project_name)).unwrap(), project_before, "{name}: rendering preserves the source project bytes");
    assert_eq!(std::fs::read(dir.join(&template_name)).unwrap(), template_before, "{name}: rendering preserves the exported template bytes");
    let direct = image::open(dir.join(&source_frame)).unwrap().to_rgba8();
    let instance = image::open(dir.join(&template_frame)).unwrap().to_rgba8();
    assert_eq!(direct.dimensions(), (320, 90));
    assert_eq!(instance.dimensions(), direct.dimensions());
    let direct_bar = direct.get_pixel(10, 75).0;
    let instance_bar = instance.get_pixel(10, 75).0;
    assert!(
        direct_bar[0] > 240 && direct_bar[1] < 15 && direct_bar[2] < 15,
        "{name}: direct source has the authored red bar at the requested valid time: {direct_bar:?}"
    );
    let different = direct.pixels().zip(instance.pixels()).filter(|(a, b)| a != b).count();
    eprintln!("{name}: duration={duration:?}, direct_bar={direct_bar:?}, instance_bar={instance_bar:?}, different_pixels={different}");
    assert_eq!(different, 0, "{name}: the template instance must preserve every source pixel at the same valid time");
}

#[test]
fn template_instance_preserves_the_partial_final_source_frame() {
    let dir = std::env::temp_dir().join(format!("ec-qa-template-partial-tail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();
    let built = ok(&dir, &["script", "build.jsx", "--save-as", "base.ecproj"]);
    assert_eq!(built["output"], "controls: 3", "{built}");

    // Establish ordinary instance rendering before testing the final partial source frame.
    timeline_tail_render_pair(&dir, "aligned", effectcraft_time::Tick::from_seconds_f64(2.0));
    let partial = effectcraft_time::Tick(effectcraft_time::TICKS_PER_SECOND * 101 / 100);
    assert_eq!(partial.seconds(), 1.01);
    timeline_tail_render_pair(&dir, "partial", partial);
    let _ = std::fs::remove_dir_all(&dir);
}

// Original procedural motion: the renderer supplies the expected pixels for both states.
const GEOMETRY_BLUR_BUILD: &str = r#"
app.beginUndoGroup("Build Blur Template");
var comp = app.project.items.addComp("Blur Template", 160, 64, 1, 2, 24);
var mover = comp.layers.addSolid([1, 0.3, 0.1], "Mover", 12, 16, 1, 2);
var position = mover.property("ADBE Transform Group").property("ADBE Position");
position.setValueAtTime(0, [16, 32]);
position.setValueAtTime(1, [144, 32]);
position.setInterpolationTypeAtKey(1, KeyframeInterpolationType.LINEAR, KeyframeInterpolationType.LINEAR);
position.setInterpolationTypeAtKey(2, KeyframeInterpolationType.LINEAR, KeyframeInterpolationType.LINEAR);
mover.motionBlur = true;
comp.motionBlur = true;
comp.shutterAngle = 180;
comp.shutterPhase = -90;
mover.property("ADBE Transform Group").property("ADBE Opacity").addToMotionGraphicsTemplateAs(comp, "Opacity");
app.endUndoGroup();
"#;

fn geometry_blur_difference(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count()
}

fn geometry_blur_assert_inputs(dir: &std::path::Path, inputs: &[(&str, Vec<u8>)]) {
    for (name, before) in inputs {
        assert_eq!(&std::fs::read(dir.join(name)).unwrap(), before, "template rendering preserves input {name}");
    }
}

#[test]
fn template_wrapper_preserves_authored_motion_blur() {
    let dir = std::env::temp_dir().join(format!("ec-qa-template-authored-blur-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("motion.jsx"), GEOMETRY_BLUR_BUILD).unwrap();
    let built = ok(&dir, &["script", "motion.jsx", "--save-as", "moving.ecproj"]);
    assert_eq!(built["ok"], true, "{built}");

    // Check the authored switch/key setup, then use an unchanged top-level composition as
    // the production pixel oracle. No expected raster or threshold is synthesized here.
    let project = effectcraft_engine::project::Project::from_json(&std::fs::read_to_string(dir.join("moving.ecproj")).unwrap()).unwrap();
    let source = project.find_by_name("Blur Template").unwrap().id;
    let comp = project.comp(source).unwrap();
    assert!(comp.enable_motion_blur);
    assert_eq!(comp.frame_rate, effectcraft_time::FrameRate::new(24, 1));
    assert_eq!(comp.layers.len(), 1);
    assert!(comp.layers[0].switches.motion_blur);
    assert_eq!(comp.layers[0].props.prop("transform/position").unwrap().keys.len(), 2);
    let exported = ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Blur Template","path":"moving.ectemplate"}"#, "moving.ecproj"]);
    assert_eq!(exported["controls"], 1, "{exported}");

    // The positive control disables only the source layer's authored Motion Blur switch.
    // It retains the same geometry, timing, source comp settings and template-control values.
    ok(
        &dir,
        &[
            "run",
            "moving.ecproj",
            "layer.setSwitch",
            r#"{"comp":"Blur Template","layers":["Mover"],"switch":"motionBlur","value":false}"#,
            "--save-as",
            "unblurred.ecproj",
        ],
    );
    let exported = ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Blur Template","path":"unblurred.ectemplate"}"#, "unblurred.ecproj"]);
    assert_eq!(exported["controls"], 1, "{exported}");
    let inputs: Vec<_> = ["moving.ecproj", "moving.ectemplate", "unblurred.ecproj", "unblurred.ectemplate"]
        .into_iter()
        .map(|name| (name, std::fs::read(dir.join(name)).unwrap()))
        .collect();

    ok(&dir, &["render-frame", "moving.ecproj", "--comp", "Blur Template", "--time", "0.5", "--transparent", "--out", "direct-blur.png"]);
    geometry_blur_assert_inputs(&dir, &inputs);
    ok(&dir, &["render-frame", "unblurred.ecproj", "--comp", "Blur Template", "--time", "0.5", "--transparent", "--out", "direct-unblurred.png"]);
    geometry_blur_assert_inputs(&dir, &inputs);
    ok(&dir, &["render-frame", "--template", "unblurred.ectemplate", "--time", "0.5", "--transparent", "--out", "template-unblurred.png"]);
    geometry_blur_assert_inputs(&dir, &inputs);
    let blurred = image::open(dir.join("direct-blur.png")).unwrap().to_rgba8();
    let unblurred = image::open(dir.join("direct-unblurred.png")).unwrap().to_rgba8();
    let template_unblurred = image::open(dir.join("template-unblurred.png")).unwrap().to_rgba8();
    assert_eq!(blurred.dimensions(), (160, 64));
    assert_eq!(unblurred.dimensions(), (160, 64));
    assert!(blurred.pixels().any(|p| p.0[3] > 0), "the direct blurred source is visible");
    assert!(unblurred.pixels().any(|p| p.0[3] > 0), "the direct unblurred source is visible");
    let authored_blur_difference = geometry_blur_difference(&blurred, &unblurred);
    assert!(authored_blur_difference > 0, "the real source renderer must show the authored motion blur");
    assert_eq!(template_unblurred, unblurred, "an unblurred template preserves its source pixels");
    eprintln!("CLI_TEMPLATE_BLUR_CONTROLS_PASSED source_blur_vs_source_unblurred={authored_blur_difference}");

    ok(&dir, &["render-frame", "--template", "moving.ectemplate", "--time", "0.5", "--transparent", "--out", "template-blur.png"]);
    geometry_blur_assert_inputs(&dir, &inputs);
    let template_blurred = image::open(dir.join("template-blur.png")).unwrap().to_rgba8();
    let differing_pixels = geometry_blur_difference(&template_blurred, &blurred);
    let unblurred_difference = geometry_blur_difference(&template_blurred, &unblurred);
    eprintln!(
        "template authored-blur result: source_vs_template_differing_pixels={differing_pixels}, template_vs_unblurred_differing_pixels={unblurred_difference}"
    );
    assert_eq!(differing_pixels, 0, "template wrapper must preserve authored source motion blur");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_unknown_control_does_not_replace_extracted_media() {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let dir = temp_root.join(format!("ec-qa-template-embedded-media-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();
    let built = ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    assert_eq!(built["output"], "controls: 3", "{built}");

    // Both PNGs are generated here; the template uses an ordinary file-backed footage layer.
    image::RgbaImage::from_pixel(2, 2, image::Rgba([30, 60, 90, 255])).save(dir.join("source.png")).unwrap();
    let imported_reply = ok(&dir, &["exec", "file.import", r#"{"paths":["source.png"]}"#, "t.ecproj", "--save"]);
    let imported = &imported_reply["result"];
    assert!(imported["errors"].as_array().unwrap().is_empty(), "{imported}");
    let items = imported["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{imported}");
    let item = items[0].as_u64().unwrap();
    let params = serde_json::json!({"comp": "Lower Third", "item": item, "position": [8, 8], "time": 0}).to_string();
    ok(&dir, &["exec", "layer.addItem", &params, "t.ecproj", "--save"]);
    let exported = ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Lower Third","path":"lt.ectemplate"}"#, "t.ecproj"]);
    assert_eq!(exported["controls"], 3, "{exported}");
    let info = ok(&dir, &["exec", "essential.templateInfo", r#"{"path":"lt.ectemplate"}"#, "--empty"]);
    let media = info["media"].as_array().unwrap();
    assert_eq!(media.len(), 1, "{info}");
    let files = media[0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{info}");
    let entry = files[0].as_str().unwrap();
    let relative = std::path::Path::new(entry.strip_prefix("media/").unwrap());
    assert_eq!(relative.components().count(), 2, "{entry}");
    assert!(relative.components().all(|part| matches!(part, std::path::Component::Normal(_))), "{entry}");
    assert_eq!(relative.file_name().unwrap(), "source.png");
    let extracted = dir.join("lt Media").join(relative);
    assert!(extracted.starts_with(&dir));

    std::fs::write(dir.join("values.json"), r##"{"Title":"John Smith","Bar Color":"#00c080","bar opacity":50}"##).unwrap();
    let template_before = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let valid_values_before = std::fs::read(dir.join("values.json")).unwrap();
    let source_before = std::fs::read(dir.join("source.png")).unwrap();
    ok(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", "ordinary.png"]);
    let ordinary = image::open(dir.join("ordinary.png")).unwrap().to_rgba8();
    let bar = ordinary.get_pixel(10, 75).0;
    assert!(bar[0] < 15 && (bar[1] as i32 - 96).abs() < 8 && (bar[2] as i32 - 64).abs() < 8, "known control values rendered: {bar:?}");
    let footage = ordinary.get_pixel(8, 8).0;
    assert!(
        (footage[0] as i32 - 30).abs() <= 2 && (footage[1] as i32 - 60).abs() <= 2 && (footage[2] as i32 - 90).abs() <= 2 && footage[3] == 255,
        "embedded footage rendered: {footage:?}"
    );
    assert_eq!(std::fs::read(&extracted).unwrap(), source_before, "ordinary render extracted the declared embedded PNG");
    assert_eq!(std::fs::read(dir.join("lt.ectemplate")).unwrap(), template_before);
    assert_eq!(std::fs::read(dir.join("values.json")).unwrap(), valid_values_before);
    assert_eq!(std::fs::read(dir.join("source.png")).unwrap(), source_before);
    let ordinary_before = std::fs::read(dir.join("ordinary.png")).unwrap();

    // Keep a different valid PNG at the exact extraction path and an existing render output.
    image::RgbaImage::from_pixel(2, 2, image::Rgba([220, 10, 20, 255])).save(&extracted).unwrap();
    let sentinel_before = std::fs::read(&extracted).unwrap();
    assert_ne!(sentinel_before, source_before);
    std::fs::copy(dir.join("ordinary.png"), dir.join("failed.png")).unwrap();
    let output_before = std::fs::read(dir.join("failed.png")).unwrap();
    std::fs::write(dir.join("bad-values.json"), r#"{"Subtitle":"x"}"#).unwrap();
    let invalid_values_before = std::fs::read(dir.join("bad-values.json")).unwrap();

    // Capture every file before interpreting status/JSON, so a failed command cannot hide a write.
    let result = Command::new(crate::binary())
        .current_dir(&dir)
        .args(["render-frame", "--template", "lt.ectemplate", "--values", "bad-values.json", "--time", "1", "--out", "failed.png", "--json"])
        .output()
        .unwrap();
    let template_after = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let valid_values_after = std::fs::read(dir.join("values.json")).unwrap();
    let invalid_values_after = std::fs::read(dir.join("bad-values.json")).unwrap();
    let source_after = std::fs::read(dir.join("source.png")).unwrap();
    let ordinary_after = std::fs::read(dir.join("ordinary.png")).unwrap();
    let output_after = std::fs::read(dir.join("failed.png")).unwrap();
    let sentinel_after = std::fs::read(&extracted).unwrap();
    let media_preserved = sentinel_after == sentinel_before;
    eprintln!(
        "EMBEDDED_MEDIA_OBSERVATION status={:?} media_preserved={media_preserved} source_preserved={} output_preserved={} stdout={} stderr={}",
        result.status.code(),
        source_after == source_before,
        output_after == output_before,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.status.code(), Some(1));
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("no control named `Subtitle`") && error.contains("controls: Title, Bar Color, Bar Opacity"), "{reply}");
    assert_eq!(template_after, template_before);
    assert_eq!(valid_values_after, valid_values_before);
    assert_eq!(invalid_values_after, invalid_values_before);
    assert_eq!(source_after, source_before);
    assert_eq!(ordinary_after, ordinary_before);
    assert_eq!(output_after, output_before);
    eprintln!("EMBEDDED_MEDIA_UNKNOWN_CONTROL: valid ordinary render and actual unknown-control failure verified");
    assert!(media_preserved, "invalid template values must fail before overwriting previously extracted media");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn template_missing_output_does_not_replace_extracted_media() {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let dir = temp_root.join(format!("ec-qa-template-missing-output-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();
    let built = ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    assert_eq!(built["output"], "controls: 3", "{built}");

    // Both PNGs are generated here; the template uses an ordinary file-backed footage layer.
    image::RgbaImage::from_pixel(2, 2, image::Rgba([30, 60, 90, 255])).save(dir.join("source.png")).unwrap();
    let imported_reply = ok(&dir, &["exec", "file.import", r#"{"paths":["source.png"]}"#, "t.ecproj", "--save"]);
    let imported = &imported_reply["result"];
    assert!(imported["errors"].as_array().unwrap().is_empty(), "{imported}");
    let items = imported["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{imported}");
    let item = items[0].as_u64().unwrap();
    let params = serde_json::json!({"comp": "Lower Third", "item": item, "position": [8, 8], "time": 0}).to_string();
    ok(&dir, &["exec", "layer.addItem", &params, "t.ecproj", "--save"]);
    let exported = ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Lower Third","path":"lt.ectemplate"}"#, "t.ecproj"]);
    assert_eq!(exported["controls"], 3, "{exported}");
    let info = ok(&dir, &["exec", "essential.templateInfo", r#"{"path":"lt.ectemplate"}"#, "--empty"]);
    let media = info["media"].as_array().unwrap();
    assert_eq!(media.len(), 1, "{info}");
    let files = media[0]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{info}");
    let entry = files[0].as_str().unwrap();
    let relative = std::path::Path::new(entry.strip_prefix("media/").unwrap());
    assert_eq!(relative.components().count(), 2, "{entry}");
    assert!(relative.components().all(|part| matches!(part, std::path::Component::Normal(_))), "{entry}");
    assert_eq!(relative.file_name().unwrap(), "source.png");
    let extracted = dir.join("lt Media").join(relative);
    assert!(extracted.starts_with(&dir));

    std::fs::write(dir.join("values.json"), r##"{"Title":"John Smith","Bar Color":"#00c080","bar opacity":50}"##).unwrap();
    let template_before = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let valid_values_before = std::fs::read(dir.join("values.json")).unwrap();
    let source_before = std::fs::read(dir.join("source.png")).unwrap();
    ok(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", "ordinary.png"]);
    let ordinary = image::open(dir.join("ordinary.png")).unwrap().to_rgba8();
    let bar = ordinary.get_pixel(10, 75).0;
    assert!(bar[0] < 15 && (bar[1] as i32 - 96).abs() < 8 && (bar[2] as i32 - 64).abs() < 8, "known control values rendered: {bar:?}");
    let footage = ordinary.get_pixel(8, 8).0;
    assert!(
        (footage[0] as i32 - 30).abs() <= 2 && (footage[1] as i32 - 60).abs() <= 2 && (footage[2] as i32 - 90).abs() <= 2 && footage[3] == 255,
        "embedded footage rendered: {footage:?}"
    );
    assert_eq!(std::fs::read(&extracted).unwrap(), source_before, "ordinary render extracted the declared embedded PNG");
    assert_eq!(std::fs::read(dir.join("lt.ectemplate")).unwrap(), template_before);
    assert_eq!(std::fs::read(dir.join("values.json")).unwrap(), valid_values_before);
    assert_eq!(std::fs::read(dir.join("source.png")).unwrap(), source_before);
    let ordinary_before = std::fs::read(dir.join("ordinary.png")).unwrap();

    // Keep a different valid PNG at the normal extraction path before a usage error.
    image::RgbaImage::from_pixel(2, 2, image::Rgba([220, 10, 20, 255])).save(&extracted).unwrap();
    let sentinel_before = std::fs::read(&extracted).unwrap();
    assert_ne!(sentinel_before, source_before);

    // Observe writes before interpreting the actual CLI usage exit.
    let result =
        Command::new(crate::binary()).current_dir(&dir).args(["render", "--template", "lt.ectemplate", "--values", "values.json", "--json"]).output().unwrap();
    let template_after = std::fs::read(dir.join("lt.ectemplate")).unwrap();
    let values_after = std::fs::read(dir.join("values.json")).unwrap();
    let source_after = std::fs::read(dir.join("source.png")).unwrap();
    let output_after = std::fs::read(dir.join("ordinary.png")).unwrap();
    let sentinel_after = std::fs::read(&extracted).unwrap();
    let media_preserved = sentinel_after == sentinel_before;
    let stderr = String::from_utf8_lossy(&result.stderr);
    eprintln!(
        "MISSING_OUTPUT_OBSERVATION status={:?} media_preserved={media_preserved} source_preserved={} output_preserved={} stdout={} stderr={stderr}",
        result.status.code(),
        source_after == source_before,
        output_after == ordinary_before,
        String::from_utf8_lossy(&result.stdout),
    );
    assert_eq!(result.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("render: --out FILE is required (or --queue)"), "{stderr}");
    assert_eq!(template_after, template_before);
    assert_eq!(values_after, valid_values_before);
    assert_eq!(source_after, source_before);
    assert_eq!(output_after, ordinary_before);
    assert!(!dir.join("frame.png").exists(), "a missing render output must not create a default frame");
    eprintln!("MISSING_OUTPUT_REQUIRED_OUT: valid ordinary render/extraction and actual required-output usage failure verified");
    assert!(media_preserved, "missing --out must fail before overwriting previously extracted media");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn template_frame_output_cannot_alias_its_template_through_a_hard_link() {
    use std::os::unix::fs::MetadataExt;

    let dir = template_output_alias_fixture("hard-linked-template");
    let input = dir.join("lt.ectemplate");
    let output = dir.join("hard-linked-template.png");
    std::fs::hard_link(&input, &output).unwrap();
    let input_metadata = std::fs::metadata(&input).unwrap();
    let output_metadata = std::fs::metadata(&output).unwrap();
    assert_eq!((input_metadata.dev(), input_metadata.ino()), (output_metadata.dev(), output_metadata.ino()), "actual hard-link file identity");
    assert_ne!(input.canonicalize().unwrap(), output.canonicalize().unwrap(), "the hard link has a distinct canonical path");
    let reply = assert_template_output_preserves_inputs(&dir, "hard-linked-template.png");
    assert_eq!(reply["error"], "--out hard-linked-template.png would overwrite --template lt.ectemplate", "{reply}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn template_frame_output_cannot_alias_its_values_through_a_hard_link() {
    use std::os::unix::fs::MetadataExt;

    let dir = template_output_alias_fixture("hard-linked-values");
    let input = dir.join("values.json");
    let output = dir.join("hard-linked-values.png");
    std::fs::hard_link(&input, &output).unwrap();
    let input_metadata = std::fs::metadata(&input).unwrap();
    let output_metadata = std::fs::metadata(&output).unwrap();
    assert_eq!((input_metadata.dev(), input_metadata.ino()), (output_metadata.dev(), output_metadata.ino()), "actual hard-link file identity");
    assert_ne!(input.canonicalize().unwrap(), output.canonicalize().unwrap(), "the hard link has a distinct canonical path");
    let reply = assert_template_output_preserves_inputs(&dir, "hard-linked-values.png");
    assert_eq!(reply["error"], "--out hard-linked-values.png would overwrite --values values.json", "{reply}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A template whose `Clip` control is a Media Replacement showing `source.png` (blue), with
/// `other.png` (red) beside it to replace it with, exported as `lt.ectemplate` in a fresh `dir`.
fn media_template(case: &str) -> PathBuf {
    let temp_root = std::env::temp_dir().canonicalize().unwrap();
    let dir = temp_root.join(format!("ec-qa-template-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("build.jsx"), BUILD).unwrap();
    ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    image::RgbaImage::from_pixel(2, 2, image::Rgba([30, 60, 90, 255])).save(dir.join("source.png")).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 20, 40, 255])).save(dir.join("other.png")).unwrap();
    let imported = ok(&dir, &["exec", "file.import", r#"{"paths":["source.png"]}"#, "t.ecproj", "--save"]);
    let item = imported["result"]["items"][0].as_u64().unwrap();
    let params = serde_json::json!({"comp": "Lower Third", "item": item, "position": [8, 8], "time": 0}).to_string();
    let added = ok(&dir, &["exec", "layer.addItem", &params, "t.ecproj", "--save"]);
    let layer = added["result"]["layer"].as_u64().unwrap();
    let params = serde_json::json!({"comp": "Lower Third", "layer": layer, "name": "Clip"}).to_string();
    ok(&dir, &["exec", "essential.addMedia", &params, "t.ecproj", "--save"]);
    ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Lower Third","path":"lt.ectemplate"}"#, "t.ecproj"]);
    dir
}

/// Renders the template's frame with `values` and checks that the control shows `other.png`.
fn assert_renders_the_replacement(dir: &PathBuf, values: &str) {
    std::fs::write(dir.join("values.json"), values).unwrap();
    ok(dir, &["render-frame", "--template", "lt.ectemplate", "--values", "values.json", "--time", "1", "--out", "out.png"]);
    let px = image::open(dir.join("out.png")).unwrap().to_rgba8().get_pixel(8, 8).0;
    assert!(
        (px[0] as i32 - 200).abs() <= 2 && (px[1] as i32 - 20).abs() <= 2 && (px[2] as i32 - 40).abs() <= 2,
        "{values}: replacement media rendered: {px:?}"
    );
}

#[test]
fn template_values_fill_a_media_replacement_control_with_a_file_path() {
    let dir = media_template("media-control");
    assert_renders_the_replacement(&dir, r#"{"Clip":"other.png"}"#);

    // A control that is not given a path is rejected with a clear message.
    std::fs::write(dir.join("bad.json"), r#"{"Clip":5}"#).unwrap();
    let (code, reply) = cli(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "bad.json", "--time", "1", "--out", "bad.png"]);
    assert_eq!(code, 1, "{reply}");
    assert!(reply["error"].as_str().unwrap().contains("media control"), "{reply}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The object forms `{"path"|"file"|"media": …}` take a file path too, and the control is found
/// by name the way `essential.set` finds it: exactly, then ignoring case (#1787).
#[test]
fn template_media_control_takes_a_path_object_and_a_name_in_any_case() {
    let dir = media_template("media-forms");
    for values in [
        r#"{"Clip":{"path":"other.png"}}"#,
        r#"{"Clip":{"file":"other.png"}}"#,
        r#"{"Clip":{"media":"other.png"}}"#,
        r#"{"clip":"other.png"}"#,
        r#"{"CLIP":{"path":"other.png"}}"#,
    ] {
        let _ = std::fs::remove_file(dir.join("out.png"));
        assert_renders_the_replacement(&dir, values);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A media path that cannot be imported fails the render with an error naming the control and
/// the path, and writes no frame (#1787).
#[test]
fn template_media_control_reports_a_path_that_cannot_be_imported() {
    let dir = media_template("media-missing");
    std::fs::write(dir.join("missing.json"), r#"{"Clip":"missing.png"}"#).unwrap();
    let (code, reply) = cli(&dir, &["render-frame", "--template", "lt.ectemplate", "--values", "missing.json", "--time", "1", "--out", "missing-out.png"]);
    assert_eq!(code, 1, "{reply}");
    let error = reply["error"].as_str().unwrap();
    assert!(error.contains("control `Clip`: cannot import missing.png"), "{reply}");
    assert!(!dir.join("missing-out.png").exists(), "{reply}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Renders with `--template` name their outputs after the template's composition (#1937), not
/// the numbered name a duplicate would get. The name ends in a number and differs from the
/// archive's file name; an expression that looks the composition up by that name still finds it.
#[test]
fn template_render_outputs_keep_the_template_composition_name() {
    let dir = std::env::temp_dir().join(format!("ec-qa-template-comp-name-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let build = BUILD.replace(r#"addComp("Lower Third""#, r#"addComp("Title Card 2""#).replace(
        "var fill =",
        "bar.property(\"ADBE Transform Group\").property(\"ADBE Position\").expression = '[comp(\"Title Card 2\").numLayers * 80, 65]';\nvar fill =",
    );
    std::fs::write(dir.join("build.jsx"), build).unwrap();
    let built = ok(&dir, &["script", "build.jsx", "--save-as", "t.ecproj"]);
    assert_eq!(built["output"], "controls: 3", "{built}");
    ok(&dir, &["exec", "essential.exportTemplate", r#"{"comp":"Title Card 2","path":"card.ectemplate"}"#, "t.ecproj"]);
    let template_before = std::fs::read(dir.join("card.ectemplate")).unwrap();
    std::fs::write(dir.join("values.json"), r#"{"Title":"John Smith"}"#).unwrap();

    for (case, values) in [("defaults", None), ("values", Some("values.json"))] {
        let out = dir.join(case).join("[compName]_[#####].png");
        let mut args =
            vec!["render", "--template", "card.ectemplate", "--out", out.to_str().unwrap(), "--format", "png", "--start", "1", "--end", "1.0833333333333333"];
        if let Some(values) = values {
            args.extend(["--values", values]);
        }
        let rendered = ok(&dir, &args);
        let rendered = rendered["rendered"].as_array().unwrap();
        assert_eq!(rendered.len(), 1, "{case}: {rendered:?}");
        assert_eq!(rendered[0]["comp"], "Title Card 2", "{case}: {rendered:?}");
        let frames: Vec<_> = std::fs::read_dir(dir.join(case)).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(frames.len(), 1, "{case}: {frames:?}");
        assert!(frames[0].starts_with("Title Card 2_") && frames[0].ends_with(".png"), "{case}: {frames:?}");
        // The template's own two layers place the bar; the one-layer wrapper would move it left.
        let frame = image::open(dir.join(case).join(&frames[0])).unwrap().to_rgba8();
        let bar = frame.get_pixel(300, 75).0;
        assert!(bar[0] > 240 && bar[1] < 15, "{case}: comp(\"Title Card 2\") is the template's composition: {bar:?}");
    }
    assert_eq!(std::fs::read(dir.join("card.ectemplate")).unwrap(), template_before);
    let _ = std::fs::remove_dir_all(&dir);
}
