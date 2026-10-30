//! The commands that write to a path they are given leave a file the project reads alone (#1743,
//! beyond the Render Queue): footage, a proxy, a frame of an imported image sequence and the
//! project file are refused with nothing changed, and any other path is written. Save and Save As
//! write the project's own file and are not among them.

use std::sync::Arc;

use effectcraft_project::{Footage, FootageKind, ItemId, Proxy};
use serde_json::{Value, json};

use super::render_overwrite::{Fixture, SOURCE};

/// Footage that is a file named `path`.
fn video(path: String) -> Footage {
    Footage { path, kind: FootageKind::Video, has_video: true, ..Default::default() }
}

/// `item` gets the file `file` of the project's folder as its proxy.
fn give_proxy(f: &mut Fixture, item: ItemId, file: &str) {
    f.write(file, SOURCE);
    let footage = video(f.text(file));
    Arc::make_mut(&mut f.s.project).item_mut(item).unwrap().proxy = Some(Box::new(Proxy { footage, enabled: true }));
}

/// A project, saved as `Project.ecproj`, that reads `clip.<ext>` (footage) with `proxy.<ext>` as
/// its proxy and the frames `shot_00000.<ext>` to `shot_00004.<ext>` of an image sequence, all
/// holding `SOURCE`. Its composition `Main` holds a red solid.
fn reading(name: &str, ext: &str) -> Fixture {
    let mut f = Fixture::new(name);
    let clip = f.footage(&format!("clip.{ext}"));
    give_proxy(&mut f, clip, &format!("proxy.{ext}"));
    f.sequence_as("shot", ext, 0..5);
    f.comp("Main");
    f.s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000"})).unwrap();
    f
}

/// The names in `dir`, sorted.
fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

/// A command that writes to a path.
struct Case {
    command: &'static str,
    /// The extension of a path for it.
    ext: &'static str,
    /// What it needs beyond a composition holding a solid.
    setup: fn(&mut Fixture),
    /// Its parameters for `path`.
    params: fn(&str) -> Value,
}

fn nothing(_: &mut Fixture) {}

/// The first layer of the active composition.
fn first_layer(f: &Fixture) -> u64 {
    f.s.active_comp().unwrap().layers[0].id.0
}

/// The composition shows one control, for `essential.exportTemplate`.
fn expose_opacity(f: &mut Fixture) {
    let layer = first_layer(f);
    f.s.execute("essential.addProperty", json!({"layer": layer, "path": "transform/opacity"})).unwrap();
}

/// A property is selected, for `anim.savePreset`.
fn select_opacity(f: &mut Fixture) {
    let layer = first_layer(f);
    f.s.execute("prop.select", json!({"layer": layer, "path": "transform/opacity"})).unwrap();
}

/// The composition has a guide, for `view.exportGuides`.
fn add_guide(f: &mut Fixture) {
    f.s.execute("view.addGuide", json!({"orientation": "vertical"})).unwrap();
}

/// Run `case` on every file the project reads: each is refused, the project's folder is as it was
/// and nothing was rendered; then on a free path, where it writes.
fn never_replaces_what_the_project_reads(case: &Case) {
    let ext = case.ext;
    let mut f = reading(&format!("{}-{ext}", case.command), ext);
    (case.setup)(&mut f);
    let mut reads = vec![(format!("clip.{ext}"), "footage"), (format!("proxy.{ext}"), "proxy"), (format!("shot_00002.{ext}"), "footage")];
    // (A timeline export adds its extension to `Project.ecproj`: it has a test of its own.)
    if case.command != "file.exportTimeline" {
        reads.push(("Project.ecproj".to_string(), "project"));
    }
    let before: Vec<Vec<u8>> = reads.iter().map(|(file, _)| f.read(file)).collect();
    let listed = listing(&f.dir);
    for ((file, source), bytes) in reads.iter().zip(&before) {
        let path = f.text(file);
        let e = f.s.execute(case.command, (case.params)(&path)).expect_err(&format!("{} wrote over {file}", case.command)).to_string();
        assert!(e.contains(file.as_str()) && e.contains(source), "{}: {file}: {e}", case.command);
        assert_eq!(&f.read(file), bytes, "{}: {file} is as it was", case.command);
    }
    assert_eq!(listing(&f.dir), listed, "{}: refusing changed nothing in the folder", case.command);
    assert!(f.exported().is_empty(), "{}: nothing was rendered for it", case.command);

    let free = format!("out.{ext}");
    f.s.execute(case.command, (case.params)(&f.text(&free))).unwrap_or_else(|e| panic!("{}: {e}", case.command));
    assert_ne!(f.read(&free), SOURCE, "{}: wrote its file", case.command);
    for ((file, _), bytes) in reads.iter().zip(&before) {
        assert_eq!(&f.read(file), bytes, "{}: {file} is still as it was", case.command);
    }
}

macro_rules! never_replaces {
    ($name:ident, $command:literal, $ext:literal, $setup:expr) => {
        #[test]
        fn $name() {
            never_replaces_what_the_project_reads(&Case { command: $command, ext: $ext, setup: $setup, params: |path| json!({"path": path}) });
        }
    };
}

never_replaces!(save_frame_as_png, "comp.saveFrameAs", "png", nothing);
never_replaces!(save_frame_as_photoshop_layers, "comp.saveFrameAsPsd", "psd", nothing);
never_replaces!(save_frame_as_proexr, "comp.saveFrameAsExr", "exr", nothing);
never_replaces!(export_lottie_json, "file.exportLottie", "json", nothing);
never_replaces!(export_lottie_dotlottie, "file.exportLottie", "lottie", nothing);
never_replaces!(save_a_copy, "file.saveCopy", "ecproj", nothing);
never_replaces!(save_a_copy_as_xml, "file.saveCopyAsXml", "ecprojx", nothing);
never_replaces!(export_timeline, "file.exportTimeline", "xml", nothing);
never_replaces!(export_template, "essential.exportTemplate", "ectemplate", expose_opacity);
never_replaces!(save_animation_preset, "anim.savePreset", "ecpreset", select_opacity);
never_replaces!(export_guides, "view.exportGuides", "json", add_guide);
never_replaces!(export_shortcuts, "shortcuts.export", "json", nothing);

// ---------------------------------------------------------------- Collect Files

/// A project reading the footage `in/<file>`, with an empty `out/(Footage)` where Collect Files
/// would put its copy.
fn collecting(name: &str, file: &str) -> Fixture {
    let mut f = Fixture::new(name);
    std::fs::create_dir_all(f.path("in")).unwrap();
    std::fs::create_dir_all(f.path("out/(Footage)")).unwrap();
    f.footage(&format!("in/{file}"));
    f
}

/// Collect Files into the project folder's `out`.
fn collect(f: &mut Fixture) -> crate::Result<Value> {
    let folder = f.text("out");
    f.s.execute("file.collectFiles", json!({"folder": folder}))
}

/// `there`, the place the copy of the footage `in/<original>` would go, is something the project
/// reads (`source`): Collect Files is refused before it copies or writes anything.
fn collect_is_refused(mut f: Fixture, original: &str, there: &str, source: &str) {
    f.write(there, b"NOT TO BE REPLACED");
    let listed = listing(&f.path("out/(Footage)"));
    let e = collect(&mut f).unwrap_err().to_string();
    assert!(e.contains(original) && e.contains(source), "{e}");
    assert_eq!(f.read(there), b"NOT TO BE REPLACED", "the file the project reads is as it was");
    assert_eq!(f.read(&format!("in/{original}")), SOURCE, "the original is as it was");
    assert!(!f.path("out/Project.ecproj").exists(), "the collected project was not written");
    assert_eq!(listing(&f.path("out/(Footage)")), listed, "nothing else was copied");
}

#[test]
fn collect_files_does_not_copy_over_other_footage() {
    let mut f = collecting("collect-footage", "a.mov");
    f.footage("out/(Footage)/a.mov");
    collect_is_refused(f, "a.mov", "out/(Footage)/a.mov", "footage");
}

#[test]
fn collect_files_does_not_copy_over_a_proxy() {
    let mut f = collecting("collect-proxy", "a.mov");
    let a = f.s.project.items.keys().next().copied().unwrap();
    give_proxy(&mut f, a, "out/(Footage)/a.mov");
    collect_is_refused(f, "a.mov", "out/(Footage)/a.mov", "proxy");
}

#[test]
fn collect_files_does_not_copy_over_a_sequence_frame() {
    let mut f = collecting("collect-frame", "shot_00002.png");
    f.sequence_as("out/(Footage)/shot", "png", 0..5);
    collect_is_refused(f, "shot_00002.png", "out/(Footage)/shot_00002.png", "footage");
}

/// Collecting into the project's own folder would write the collected copy over the project.
#[test]
fn collect_files_does_not_write_over_the_project_file() {
    let mut f = collecting("collect-project", "a.mov");
    let project = f.read("Project.ecproj");
    let folder = f.dir.to_string_lossy().into_owned();
    let e = f.s.execute("file.collectFiles", json!({"folder": folder})).unwrap_err().to_string();
    assert!(e.contains("Project.ecproj") && e.contains("project"), "{e}");
    assert_eq!(f.read("Project.ecproj"), project, "the project file is as it was");
    assert!(!f.path("(Footage)").exists(), "no folder was made, nothing was copied");
}

#[test]
fn collect_files_copies_into_a_free_folder() {
    let mut f = collecting("collect-free", "a.mov");
    let r = collect(&mut f).unwrap();
    assert_eq!((r["copied"].as_u64(), &r["errors"]), (Some(1), &json!([])), "{r}");
    assert_eq!(f.read("out/(Footage)/a.mov"), SOURCE);
    assert_eq!(f.read("in/a.mov"), SOURCE, "the original stays");
    assert!(f.path("out/Project.ecproj").exists());
}

/// Footage that is where its copy would go is already collected: it is not copied onto itself.
#[test]
fn collect_files_leaves_footage_that_is_already_in_the_folder() {
    let mut f = collecting("collect-in-place", "a.mov");
    f.footage("out/(Footage)/c.mov");
    f.write("out/(Footage)/c.mov", b"ALREADY COLLECTED");
    let r = collect(&mut f).unwrap();
    assert_eq!((r["copied"].as_u64(), &r["errors"]), (Some(2), &json!([])), "{r}");
    assert_eq!(f.read("out/(Footage)/c.mov"), b"ALREADY COLLECTED");
    assert_eq!(f.read("out/(Footage)/a.mov"), SOURCE);
}

// ---------------------------------------------------------------- the rest

/// The movies a timeline export renders beside it are written like any other output.
#[test]
fn a_timeline_export_does_not_pre_render_over_footage() {
    for background in [false, true] {
        let mut f = Fixture::new(&format!("timeline-prerender-{background}"));
        f.comp("Main");
        f.s.execute("layer.newSolid", json!({"name": "Red", "color": "#ff0000"})).unwrap();
        // OpenTimelineIO has no solids: the solid is rendered to a movie beside the document.
        let params = json!({"path": f.text("Main.otio"), "prerender": "unsupported", "background": background});
        let run = |s: &mut crate::Session, params: Value| {
            let result = s.execute("file.exportTimeline", params).unwrap();
            if background {
                assert!(result["job"].is_string(), "{result}");
                s.wait_jobs();
                let done = s.job_log.last().unwrap();
                assert_eq!(done.status, "done", "{}", done.message);
                done.result.clone()
            } else {
                result
            }
        };
        let r = run(&mut f.s, params.clone());
        let movies = f.exported();
        assert_eq!(movies.len(), 1, "{r}");
        assert_eq!(r["prerendered"].as_array().map(Vec::len), Some(1), "{r}");
        // That movie is footage now.
        let movie = std::path::Path::new(&movies[0]).file_name().unwrap().to_string_lossy().into_owned();
        f.footage(&movie);
        let r = run(&mut f.s, params);
        assert_eq!(f.read(&movie), SOURCE, "the footage is as it was");
        assert_eq!(f.exported().len(), 1, "no second render was started");
        assert_eq!(r["prerendered"], json!([]), "{r}");
        let warnings = r["warnings"].as_array().unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
        assert!(warnings.contains(&movie) && warnings.contains("footage"), "the export says why: {r}");
    }
}

/// `file.checkOutput` answers before anything is written. Its reply describes current state;
/// callers still need a guarded write in the owning session, as CLI/MCP frame saves use.
#[test]
fn check_output_names_the_files_an_export_may_not_use() {
    let mut f = reading("check-output", "png");
    for (file, source) in [("clip.png", "footage"), ("proxy.png", "proxy"), ("shot_00003.png", "footage"), ("Project.ecproj", "project")] {
        let r = f.s.execute("file.checkOutput", json!({"path": f.text(file)})).unwrap();
        assert_eq!((r["blocked"].as_bool(), r["source"].as_str()), (Some(true), Some(source)), "{file}: {r}");
        assert!(r["reason"].as_str().unwrap().contains(file), "{file}: {r}");
        assert_eq!(r["path"], json!(f.text(file)), "{r}");
    }
    let r = f.s.execute("file.checkOutput", json!({"path": f.text("free.png")})).unwrap();
    assert_eq!((r["blocked"].as_bool(), r["source"].is_null()), (Some(false), true), "{r}");
    assert!(f.s.execute("file.checkOutput", json!({})).is_err(), "a path is needed");
    assert!(!f.path("free.png").exists(), "it only looks");
}

/// A project saved under a timeline's name is not overwritten by the timeline.
#[test]
fn a_timeline_export_does_not_write_over_the_project_file() {
    let mut f = Fixture::new("timeline-project");
    f.comp("Main");
    f.s.execute("file.saveAs", json!({"path": f.text("Edit.xml")})).unwrap();
    let saved = f.read("Edit.xml");
    let e = f.s.execute("file.exportTimeline", json!({"path": f.text("Edit.xml")})).unwrap_err().to_string();
    assert!(e.contains("Edit.xml") && e.contains("project"), "{e}");
    assert_eq!(f.read("Edit.xml"), saved);
}

#[test]
fn a_background_timeline_export_rechecks_its_document_before_completion() {
    let mut f = Fixture::new("timeline-current-project");
    f.comp("Main");
    let path = f.text("Pending.xml");
    let job = f.s.execute("file.exportTimeline", json!({"path": path, "background": true, "prerender": "none"})).unwrap();
    assert!(job["job"].is_string());
    // The app may save the project while a background export is waiting to finish.
    f.s.execute("file.saveAs", json!({"path": path})).unwrap();
    let saved = f.read("Pending.xml");
    f.s.wait_jobs();
    let done = f.s.job_log.last().unwrap();
    assert_eq!(done.status, "failed", "the destination became the project: {}", done.message);
    assert!(done.message.contains("project"));
    assert_eq!(f.read("Pending.xml"), saved);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn background_pre_render_movies_are_checked_again_before_publication() {
    for source in ["footage", "project"] {
        let mut f = Fixture::new(&format!("timeline-staged-{source}"));
        f.comp("Main");
        f.s.execute("layer.newSolid", json!({"name": "Red"})).unwrap();
        let params = json!({"path": f.text("Main.otio"), "prerender": "unsupported"});
        let first = f.s.execute("file.exportTimeline", params.clone()).unwrap();
        let movie = first["prerendered"][0].as_str().unwrap().to_string();
        let filename = std::path::Path::new(&movie).file_name().unwrap().to_string_lossy().into_owned();
        let document = f.read("Main.otio");
        let mut background = params;
        background["background"] = json!(true);
        f.s.execute("file.exportTimeline", background).unwrap();
        if source == "footage" {
            f.footage(&filename);
        } else {
            f.s.execute("file.saveAs", json!({"path": movie})).unwrap();
        }
        let protected = f.read(&filename);
        f.s.wait_jobs();
        let done = f.s.job_log.last().unwrap();
        assert_eq!(done.status, "failed", "{}", done.message);
        assert!(done.message.contains(source), "{}", done.message);
        assert_eq!(f.read(&filename), protected);
        assert_eq!(f.read("Main.otio"), document);
        assert!(listing(&f.dir).iter().all(|name| !name.starts_with(".effectcraft-prerender-")));
    }
}
