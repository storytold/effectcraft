//! Rendering never replaces a file the project reads (#1743). Ordinary-file replacement
//! behavior stays with the exporter and its confirmation API. Two items of one job never write
//! the same file either.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use effectcraft_color::Label;
use effectcraft_project::render_queue::{OutputFormat, RenderStatus, sequence_path};
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind};
use effectcraft_time::Tick;
use serde_json::{Value, json};

use crate::{ExportJob, ExportResult, Exporter, Session};

pub(crate) const SOURCE: &[u8] = b"SOURCE FOOTAGE";
const EARLIER: &[u8] = b"AN EARLIER RENDER";
const RENDERED: &[u8] = b"RENDERED";

/// An exporter that writes like the real one: `RENDERED` at the job's path, or at the path of
/// every frame for a sequence (leaving the frames that exist alone when the item skips them).
#[derive(Default)]
struct Writer {
    jobs: Mutex<Vec<String>>,
}

impl Exporter for Writer {
    fn formats(&self) -> Vec<OutputFormat> {
        OutputFormat::ALL.to_vec()
    }
    fn supports_skip_existing(&self, _: &effectcraft_project::render_queue::RenderSettings) -> bool {
        true
    }
    fn export(&self, job: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        self.jobs.lock().unwrap().push(job.path.to_string());
        let comp = job.project.comp(job.item.comp).ok_or("no comp")?;
        let paths: Vec<String> = if job.item.output.format.is_sequence() {
            let first = job.item.settings.first_frame(comp);
            (0..job.item.settings.frame_count(comp)).map(|k| sequence_path(job.path, first + k as i64)).collect()
        } else {
            vec![job.path.to_string()]
        };
        for path in &paths {
            if job.item.settings.skip_existing && Path::new(path).exists() {
                continue;
            }
            if let Some(dir) = Path::new(path).parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, RENDERED).map_err(|e| e.to_string())?;
        }
        Ok(ExportResult { path: paths.first().cloned().unwrap_or_default(), ..Default::default() })
    }
}

/// A session whose project is saved in a fresh folder (the footage goes there too), with an
/// exporter that writes real files.
pub(crate) struct Fixture {
    pub(crate) s: Session,
    writer: Arc<Writer>,
    pub(crate) dir: PathBuf,
}

impl Fixture {
    pub(crate) fn new(name: &str) -> Fixture {
        let dir = std::env::temp_dir().join("effectcraft-render-overwrite").join(format!("{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::default();
        let writer = Arc::new(Writer::default());
        s.exporter = Some(writer.clone());
        s.execute("file.saveAs", json!({"path": dir.join("Project.ecproj").to_string_lossy()})).unwrap();
        Fixture { s, writer, dir }
    }

    pub(crate) fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    pub(crate) fn text(&self, file: &str) -> String {
        self.path(file).to_string_lossy().into_owned()
    }

    pub(crate) fn read(&self, file: &str) -> Vec<u8> {
        std::fs::read(self.path(file)).unwrap()
    }

    pub(crate) fn write(&self, file: &str, bytes: &[u8]) {
        std::fs::write(self.path(file), bytes).unwrap();
    }

    /// A video file in the project's folder, imported.
    pub(crate) fn footage(&mut self, file: &str) -> ItemId {
        self.write(file, SOURCE);
        let f = Footage {
            path: self.text(file),
            kind: FootageKind::Video,
            width: 64,
            height: 36,
            has_video: true,
            duration: Tick::from_seconds_f64(1.0),
            ..Default::default()
        };
        Arc::make_mut(&mut self.s.project).add_item(file, Label::None, None, ItemKind::Footage(f))
    }

    /// Numbered stills in the project's folder, imported as one image sequence.
    fn sequence(&mut self, name: &str, frames: std::ops::Range<u32>) -> ItemId {
        self.sequence_as(name, "png", frames)
    }

    /// [`Fixture::sequence`] of files with extension `ext`.
    pub(crate) fn sequence_as(&mut self, name: &str, ext: &str, frames: std::ops::Range<u32>) -> ItemId {
        let files: Vec<String> = frames
            .map(|n| {
                let file = format!("{name}_{n:05}.{ext}");
                self.write(&file, SOURCE);
                self.text(&file)
            })
            .collect();
        let f = Footage {
            path: files[0].clone(),
            kind: FootageKind::Sequence,
            sequence: files,
            width: 16,
            height: 16,
            has_video: true,
            duration: Tick::from_seconds_f64(0.5),
            ..Default::default()
        };
        Arc::make_mut(&mut self.s.project).add_item(&format!("{name}_[00000-00004].{ext}"), Label::None, None, ItemKind::Footage(f))
    }

    /// A five-frame composition.
    pub(crate) fn comp(&mut self, name: &str) {
        self.s.execute("comp.new", json!({"name": name, "width": 16, "height": 16, "frameRate": 10, "duration": 0.5})).unwrap();
    }

    /// The output path of queue item `index` (0-based).
    fn output(&self, index: usize) -> PathBuf {
        PathBuf::from(self.s.resolve_output(&self.s.project.render_queue[index]).unwrap())
    }

    /// The paths the exporter was started for.
    pub(crate) fn exported(&self) -> Vec<String> {
        self.writer.jobs.lock().unwrap().clone()
    }

    fn render(&mut self, params: Value) -> crate::Result<Value> {
        self.s.execute("renderQueue.render", params)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// A folder that leads to `target`: a symbolic link, or on Windows, where that takes a privilege
/// most accounts lack, a junction.
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_dir(target, link).or_else(|_| {
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(link).arg(target).output()?;
        if made.status.success() { Ok(()) } else { Err(std::io::Error::other(String::from_utf8_lossy(&made.stdout).into_owned())) }
    });
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

/// The report: save a new project beside `footage.mp4`, import it, make a composition from it
/// (it is named after the file), add that to the Render Queue and click Render. The composition's
/// default output is `<project folder>/footage.mp4`, the footage itself.
#[test]
fn a_comp_made_from_footage_does_not_render_over_it() {
    let mut f = Fixture::new("comp-from-footage");
    let footage = f.footage("footage.mp4");
    f.s.state.project_selection = vec![footage];
    f.s.execute("file.newCompFromSelection", json!({"addToRenderQueue": true})).unwrap();
    assert_eq!(f.s.project.render_queue.len(), 1);
    f.render(json!({})).unwrap();
    assert_eq!(f.read("footage.mp4"), SOURCE, "the footage is still the footage");
    assert_eq!(f.s.project.render_queue[0].status, RenderStatus::Done);
    // The default name stepped aside: the render sits next to it.
    assert_eq!(f.output(0), f.path("footage_1.mp4"));
    assert_eq!(f.read("footage_1.mp4"), RENDERED);
    // Another item for the same composition takes the next free number.
    f.s.execute("renderQueue.add", json!({})).unwrap();
    assert_eq!(f.output(1), f.path("footage_2.mp4"));
}

#[test]
fn an_output_that_is_the_footage_is_refused_with_or_without_overwrite() {
    for (n, params) in [json!({}), json!({"overwrite": true})].into_iter().enumerate() {
        let mut f = Fixture::new(&format!("explicit-footage-{n}"));
        f.footage("clip.mov");
        f.comp("Main");
        let clip = f.text("clip.mov");
        f.s.execute("renderQueue.add", json!({"format": "prores", "output": clip})).unwrap();
        let e = f.render(params).unwrap_err().to_string();
        assert!(e.contains("clip.mov") && e.contains("footage"), "{e}");
        assert_eq!(f.read("clip.mov"), SOURCE);
        assert!(f.exported().is_empty(), "the exporter was not started");
        let RenderStatus::Failed(why) = &f.s.project.render_queue[0].status else { panic!("{:?}", f.s.project.render_queue[0].status) };
        assert!(why.contains("clip.mov"), "{why}");
    }
}

/// An item that would replace the footage fails on its own; the rest of the queue renders.
#[test]
fn the_rest_of_the_queue_renders_when_one_item_would_replace_footage() {
    let mut f = Fixture::new("one-refused");
    f.footage("clip.mov");
    f.comp("Main");
    let clip = f.text("clip.mov");
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": clip})).unwrap();
    let ok = f.text("main.mp4");
    f.s.execute("renderQueue.add", json!({"output": ok})).unwrap();
    let r = f.render(json!({})).unwrap();
    assert_eq!(f.read("clip.mov"), SOURCE);
    assert_eq!(f.read("main.mp4"), RENDERED);
    assert!(matches!(f.s.project.render_queue[0].status, RenderStatus::Failed(_)));
    assert_eq!(f.s.project.render_queue[1].status, RenderStatus::Done);
    assert!(r["items"].as_array().unwrap().iter().any(|i| i["status"].get("Failed").is_some()), "the reply includes the refused item: {r}");
    assert!(r["items"].as_array().unwrap().iter().any(|i| i["status"] == "Done"), "{r}");
}

#[test]
fn the_project_file_is_never_an_output() {
    let mut f = Fixture::new("project-file");
    // A project saved under a name a movie could have.
    let name = f.text("final.mp4");
    f.s.execute("file.saveAs", json!({"path": name})).unwrap();
    f.comp("final");
    f.s.execute("renderQueue.add", json!({})).unwrap();
    assert_eq!(f.output(0), f.path("final_1.mp4"), "the default name moves aside");
    f.s.execute("renderQueue.remove", json!({"index": 1})).unwrap();
    f.s.execute("renderQueue.add", json!({"output": name})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("final.mp4") && e.contains("project"), "{e}");
    assert!(std::fs::read_to_string(f.path("final.mp4")).unwrap().contains("\"schema\""), "the project file is still a project");
}

#[test]
fn a_proxy_is_not_an_output_either() {
    let mut f = Fixture::new("proxy");
    f.footage("clip.mov");
    f.write("proxy.mov", SOURCE);
    f.comp("Main");
    let proxy = Footage { path: f.text("proxy.mov"), kind: FootageKind::Video, has_video: true, ..Default::default() };
    let comp = f.s.active_comp_id().unwrap();
    Arc::make_mut(&mut f.s.project).item_mut(comp).unwrap().proxy = Some(Box::new(effectcraft_project::Proxy { footage: proxy, enabled: true }));
    let target = f.text("proxy.mov");
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": target})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("proxy"), "{e}");
    assert_eq!(f.read("proxy.mov"), SOURCE);
}

/// Recreating a default proxy writes a new file and attaches it, without replacing the proxy
/// the project is currently reading. Explicitly choosing that protected path is still refused.
#[test]
fn creating_a_proxy_again_chooses_a_new_default_and_attaches_it() {
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            if !Path::new(path).is_file() {
                return Err("proxy file missing".into());
            }
            Ok(Footage {
                path: path.into(),
                kind: if path.ends_with(".png") { FootageKind::Still } else { FootageKind::Video },
                width: 16,
                height: 16,
                has_video: true,
                ..Default::default()
            })
        }
    }
    for kind in ["movie", "still"] {
        let mut f = Fixture::new(&format!("recreate-proxy-{kind}"));
        f.s.importer = Some(Arc::new(Probe));
        f.comp("Main");
        let comp = f.s.active_comp_id().unwrap();
        f.s.execute("file.createProxy", json!({"kind": kind})).unwrap();
        f.render(json!({})).unwrap();
        let first = f.s.project.item(comp).unwrap().proxy.as_ref().unwrap().footage.path.clone();
        assert_eq!(std::fs::read(&first).unwrap(), RENDERED);

        f.s.execute("file.createProxy", json!({"kind": kind})).unwrap();
        f.render(json!({})).unwrap();
        let second = f.s.project.item(comp).unwrap().proxy.as_ref().unwrap().footage.path.clone();
        assert_ne!(second, first, "the replacement proxy gets its own file");
        assert_eq!(std::fs::read(&second).unwrap(), RENDERED);
        assert_eq!(std::fs::read(&first).unwrap(), RENDERED);
        assert_eq!(f.exported().len(), 2);

        // For a still proxy, an explicit output is the sequence pattern, not its first
        // rendered frame (a bare PNG name would have another frame number appended).
        let protected_output = f.output(1);
        f.s.execute("file.createProxy", json!({"kind": kind, "path": protected_output})).unwrap();
        let error = f.render(json!({})).unwrap_err().to_string();
        assert!(error.contains("never replaces a proxy"), "{error}");
        assert_eq!(f.exported().len(), 2);
    }
}

/// A pre-render becomes imported footage. Its old output stays protected even from the
/// same queue item; issuing Pre-render again supplies a fresh default output instead.
#[test]
fn refreshing_a_pre_render_uses_a_new_default_without_exempting_imported_footage() {
    struct Probe;
    impl crate::Importer for Probe {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            if !Path::new(path).is_file() {
                return Err("pre-render missing".into());
            }
            Ok(Footage { path: path.into(), kind: FootageKind::Video, width: 16, height: 16, has_video: true, ..Default::default() })
        }
    }
    let mut f = Fixture::new("refresh-pre-render");
    f.s.importer = Some(Arc::new(Probe));
    f.comp("Main");
    f.s.execute("render.preRender", json!({"format": "gif"})).unwrap();
    let first = f.output(0);
    f.render(json!({})).unwrap();
    f.s.execute("renderQueue.setRender", json!({"index": 1, "render": true})).unwrap();
    let error = f.render(json!({})).unwrap_err().to_string();
    assert!(error.contains("footage"), "{error}");
    assert_eq!(std::fs::read(&first).unwrap(), RENDERED);
    assert_eq!(f.exported().len(), 1);
    f.s.execute("renderQueue.setRender", json!({"index": 1, "render": false})).unwrap();
    f.s.execute("render.preRender", json!({"format": "gif"})).unwrap();
    let second = f.output(1);
    assert_ne!(first, second);
    f.render(json!({})).unwrap();
    assert_eq!(std::fs::read(first).unwrap(), RENDERED);
    assert_eq!(std::fs::read(second).unwrap(), RENDERED);
    assert_eq!(f.exported().len(), 2);
}

/// Exercise the engine's import and project serialization paths, rather than injecting a
/// Footage item directly. The importer reads dimensions from a real, generated PNG.
#[test]
fn imported_footage_stays_protected_after_the_project_is_reopened() {
    struct PngProbe;
    impl crate::Importer for PngProbe {
        fn probe(&self, path: &str) -> Result<Footage, String> {
            let (width, height) = image::image_dimensions(path).map_err(|e| e.to_string())?;
            Ok(Footage { path: path.into(), kind: FootageKind::Still, width, height, has_video: true, ..Default::default() })
        }
    }
    let mut f = Fixture::new("import-reopen");
    f.s.importer = Some(Arc::new(PngProbe));
    let path = f.path("plate_00002.png");
    image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255])).save(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    let result = f.s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    f.comp("Main");
    f.s.execute("renderQueue.add", json!({"format": "png", "output": f.text("plate_[#####].png")})).unwrap();
    f.s.execute("file.save", json!({})).unwrap();
    f.s.execute("file.open", json!({"path": f.text("Project.ecproj")})).unwrap();
    let error = f.render(json!({})).unwrap_err().to_string();
    assert!(error.contains("plate_00002.png") && error.contains("footage"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(f.exported().is_empty());
}

#[test]
fn a_protected_frame_past_the_probe_limit_is_not_rendered() {
    struct LastFrame(PathBuf);
    impl Exporter for LastFrame {
        fn formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::PngSequence]
        }
        fn export(&self, _: &ExportJob, _: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
            // A synthetic exporter writes only the vulnerable late frame, so a broken guard
            // can be reproduced without creating 100,001 output directories.
            std::fs::write(&self.0, RENDERED).map_err(|e| e.to_string())?;
            Ok(ExportResult { path: self.0.to_string_lossy().into_owned(), ..Default::default() })
        }
    }
    let mut f = Fixture::new("late-frame");
    std::fs::create_dir_all(f.path("f100000")).unwrap();
    f.footage("f100000/plate.png");
    f.comp("Main");
    f.s.execute("comp.settings", json!({"duration": 10000.1})).unwrap();
    f.s.exporter = Some(Arc::new(LastFrame(f.path("f100000/plate.png"))));
    f.s.execute("renderQueue.add", json!({"format": "png", "output": f.text("f[######]/plate.png")})).unwrap();
    let error = f.render(json!({})).unwrap_err().to_string();
    assert!(error.contains("cannot safely check"), "{error}");
    assert_eq!(f.read("f100000/plate.png"), SOURCE);
}

/// A numbered sequence is footage frame by frame: one colliding frame is enough, and nothing of
/// the item is rendered.
#[test]
fn a_sequence_never_replaces_one_of_its_footage_frames() {
    let mut f = Fixture::new("sequence");
    f.sequence("shot", 0..5);
    f.comp("shot");
    // The default name steps aside as a whole: shot_1_00000.png …
    f.s.execute("renderQueue.add", json!({"format": "png"})).unwrap();
    assert_eq!(f.output(0), f.path("shot_1_[#####].png"));
    f.render(json!({})).unwrap();
    for n in 0..5 {
        assert_eq!(f.read(&format!("shot_{n:05}.png")), SOURCE, "frame {n}");
        assert_eq!(f.read(&format!("shot_1_{n:05}.png")), RENDERED, "frame {n}");
    }

    // Named by hand, one frame of five is a footage file.
    let mut f = Fixture::new("sequence-one-frame");
    f.write("shot_00002.png", SOURCE);
    let still = Footage { path: f.text("shot_00002.png"), kind: FootageKind::Still, width: 16, height: 16, has_video: true, ..Default::default() };
    Arc::make_mut(&mut f.s.project).add_item("shot_00002.png", Label::None, None, ItemKind::Footage(still));
    f.comp("shot");
    let pattern = f.text("shot_[#####].png");
    f.s.execute("renderQueue.add", json!({"format": "png", "output": pattern})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("shot_00002.png"), "{e}");
    assert_eq!(f.read("shot_00002.png"), SOURCE);
    assert!(!f.path("shot_00000.png").exists(), "the frames before it were not rendered either");
    assert!(f.exported().is_empty());
}

#[test]
fn ordinary_existing_outputs_keep_the_exporters_replacement_policy() {
    let mut f = Fixture::new("earlier-render");
    f.comp("Main");
    f.s.execute("renderQueue.add", json!({"output": f.text("render.mp4")})).unwrap();
    f.write("render.mp4", EARLIER);
    f.render(json!({})).unwrap();
    assert_eq!(f.read("render.mp4"), RENDERED);
    assert_eq!(f.s.project.render_queue[0].status, RenderStatus::Done);
}

#[test]
fn a_render_with_nothing_in_the_way_needs_no_flag() {
    let mut f = Fixture::new("free");
    f.comp("Main");
    f.s.execute("renderQueue.add", json!({})).unwrap();
    assert_eq!(f.output(0), f.path("Main.mp4"), "no footage in the way: the comp's own name");
    f.render(json!({})).unwrap();
    assert_eq!(f.read("Main.mp4"), RENDERED);
}

/// Skip Existing Files leaves the frames that exist alone, so they need no answer.
#[test]
fn a_sequence_that_skips_existing_frames_needs_no_flag() {
    let mut f = Fixture::new("skip-existing");
    f.comp("Main");
    let pattern = f.text("render_[#####].png");
    f.s.execute("renderQueue.add", json!({"format": "png", "output": pattern, "skipExisting": true})).unwrap();
    for n in 0..3 {
        f.write(&format!("render_{n:05}.png"), EARLIER);
    }
    f.render(json!({})).unwrap();
    assert_eq!(f.read("render_00002.png"), EARLIER);
    assert_eq!(f.read("render_00003.png"), RENDERED);
    // Ordinary files still follow the exporter's replacement policy when skipping is off.
    f.s.execute("renderQueue.setRenderSettings", json!({"index": 1, "skipExisting": false})).unwrap();
    f.render(json!({})).unwrap();
    assert_eq!(f.read("render_00002.png"), RENDERED);
}

/// A link is another name for the file it points to.
#[test]
fn a_link_to_the_footage_is_the_footage() {
    let mut f = Fixture::new("link");
    f.footage("clip.mov");
    f.comp("Main");
    let alias = f.path("render.mov");
    if link(&f.path("clip.mov"), &alias).is_err() {
        eprintln!("this account cannot make symbolic links: skipped");
        return;
    }
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": f.text("render.mov")})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("render.mov") && e.contains("footage"), "{e}");
    assert_eq!(f.read("clip.mov"), SOURCE);
    assert!(std::fs::symlink_metadata(&alias).unwrap().file_type().is_symlink(), "the link was not replaced either");
}

/// A hard link to the footage under another name is the same file on disk: replacing it would truncate the footage.
#[test]
fn a_hard_link_to_the_footage_is_the_footage() {
    let mut f = Fixture::new("hardlink");
    f.footage("clip.mov");
    f.comp("Main");
    if std::fs::hard_link(f.path("clip.mov"), f.path("render.mov")).is_err() {
        eprintln!("this volume cannot make hard links: skipped");
        return;
    }
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": f.text("render.mov")})).unwrap();
    // not even with overwrite
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("render.mov") && e.contains("footage"), "{e}");
    assert_eq!(f.read("clip.mov"), SOURCE);
    assert_eq!(f.read("render.mov"), SOURCE, "the other name still leads to the footage");
    assert!(f.exported().is_empty(), "nothing was rendered");
}

/// A copy of the footage has its size and bytes, but it is another file: an earlier render may replace it.
#[test]
fn a_copy_of_the_footage_is_not_the_footage() {
    let mut f = Fixture::new("copy");
    f.footage("clip.mov");
    f.comp("Main");
    std::fs::copy(f.path("clip.mov"), f.path("copy.mov")).unwrap();
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": f.text("copy.mov")})).unwrap();
    f.render(json!({})).unwrap();
    assert_eq!(f.read("copy.mov"), RENDERED);
    assert_eq!(f.read("clip.mov"), SOURCE, "the footage is as it was");
}

/// Footage imported through a linked folder is the same file as the one in the real folder.
#[test]
fn footage_reached_through_a_linked_folder_is_still_footage() {
    let mut f = Fixture::new("linked-folder");
    std::fs::create_dir_all(f.path("real")).unwrap();
    std::fs::write(f.path("real").join("clip.mov"), SOURCE).unwrap();
    if link_dir(&f.path("real"), &f.path("linked")).is_err() {
        eprintln!("this account cannot link folders: skipped");
        return;
    }
    let via_link =
        Footage { path: f.path("linked").join("clip.mov").to_string_lossy().into_owned(), kind: FootageKind::Video, has_video: true, ..Default::default() };
    Arc::make_mut(&mut f.s.project).add_item("clip.mov", Label::None, None, ItemKind::Footage(via_link));
    f.comp("Main");
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": f.path("real").join("clip.mov").to_string_lossy()})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("footage"), "{e}");
    assert_eq!(std::fs::read(f.path("real").join("clip.mov")).unwrap(), SOURCE);
}

/// Where the volume ignores case, `CLIP.MOV` is `clip.mov`.
#[test]
fn another_spelling_of_the_footage_name_is_the_footage_where_the_volume_ignores_case() {
    let mut f = Fixture::new("case");
    f.footage("clip.mov");
    f.comp("Main");
    if !f.path("CLIP.MOV").exists() {
        eprintln!("this volume tells the two names apart: skipped");
        return;
    }
    let shouting = f.text("CLIP.MOV");
    f.s.execute("renderQueue.add", json!({"format": "prores", "output": shouting})).unwrap();
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("footage"), "{e}");
    assert_eq!(f.read("clip.mov"), SOURCE);
}

#[test]
fn save_current_preview_never_replaces_footage() {
    let mut f = Fixture::new("preview");
    f.footage("clip.mov");
    f.comp("Main");
    let clip = f.text("clip.mov");
    let e = f.s.execute("render.saveCurrentPreview", json!({"path": clip})).unwrap_err().to_string();
    assert!(e.contains("clip.mov") && e.contains("footage"), "{e}");
    assert_eq!(f.read("clip.mov"), SOURCE);
    assert!(f.exported().is_empty());
    // Anywhere else it renders.
    let elsewhere = f.text("preview.mp4");
    f.s.execute("render.saveCurrentPreview", json!({"path": elsewhere})).unwrap();
    assert_eq!(f.read("preview.mp4"), RENDERED);
}

#[test]
fn save_current_preview_checks_sequence_frames_as_well_as_the_pattern() {
    let mut f = Fixture::new("preview-sequence");
    f.sequence("plate", 0..5);
    f.comp("Main");
    let error = f.s.execute("render.saveCurrentPreview", json!({"path": f.text("plate_[#####].png")})).unwrap_err().to_string();
    assert!(error.contains("plate_000") && error.contains("footage"), "{error}");
    for frame in 0..5 {
        assert_eq!(f.read(&format!("plate_{frame:05}.png")), SOURCE);
    }
    assert!(f.exported().is_empty());
}

/// A watched folder renders unattended; delete the status file to render a project again.
#[test]
fn a_watched_folder_still_renders_a_project_again_over_its_outputs() {
    let mut f = Fixture::new("watch");
    f.comp("Main");
    let out = f.text("Main.mp4");
    f.s.execute("renderQueue.add", json!({"output": out})).unwrap();
    f.s.execute("file.save", json!({})).unwrap();
    f.write("Main.mp4", EARLIER);
    let folder = f.dir.to_string_lossy().into_owned();
    let done = crate::commands::watch_folder::poll(&mut f.s, &folder).unwrap();
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(done[0]["state"], "done", "{:?}", done[0]);
    assert_eq!(f.read("Main.mp4"), RENDERED);
}

// ------------------------------------------------------------ one file, two items of one job
//
// Nothing is on disk when a job starts, so the check for files that exist said nothing about two
// queue items that resolve to one output: the item rendered last replaced the file of the one
// before it, inside the same job.

impl Fixture {
    /// The output path of every queue item, in queue order.
    fn outputs(&self) -> Vec<PathBuf> {
        (0..self.s.project.render_queue.len()).map(|i| self.output(i)).collect()
    }

    fn add(&mut self, params: Value) {
        self.s.execute("renderQueue.add", params).unwrap();
    }

    /// What the queue would write twice, in the shape the tests read: `{"duplicates": [...]}`.
    fn check(&mut self) -> Value {
        let formats = self.s.exporter.as_ref().map(|e| e.formats()).unwrap_or_default();
        let (items, _) = self.s.queued_outputs(&formats);
        let refused = self.s.check_outputs(&items);
        let duplicates: Vec<Value> = self
            .s
            .duplicate_outputs(&items, &refused)
            .unwrap()
            .iter()
            .map(|d| {
                json!({
                    "item": d.item, "index": d.index, "comp": d.comp, "path": d.path, "files": d.files, "sequence": d.sequence,
                    "with": {"item": d.with, "index": d.with_index, "comp": d.with_comp}, "reason": d.reason(),
                })
            })
            .collect();
        json!({"duplicates": duplicates})
    }

    /// What `check` lists under `duplicates`: for each, the place in the queue of the item that
    /// would write over a file, the place of the item that wrote it first, and how many files.
    fn clashes(&mut self) -> Vec<(u64, u64, u64)> {
        let found = self.check();
        let list = found["duplicates"].as_array().unwrap_or_else(|| panic!("checkOutputs lists no duplicates: {found}"));
        list.iter().map(|d| (d["index"].as_u64().unwrap_or(0), d["with"]["index"].as_u64().unwrap_or(0), d["files"].as_u64().unwrap_or(0))).collect()
    }
}

/// A PNG sequence of `pattern` for the composition's seconds `start..end`.
fn span(pattern: &str, start: f64, end: f64) -> Value {
    json!({"format": "png", "output": pattern, "timeSpan": "custom", "start": start, "end": end})
}

fn with_overflow(f: &mut Fixture) {
    struct Room;
    impl effectcraft_project::render_queue::StorageQuota for Room {
        fn has_room(&self, _: &str, _: u64) -> bool {
            true
        }
    }
    f.s.storage_quota = Some(Arc::new(Room));
    std::fs::create_dir_all(f.path("overflow")).unwrap();
    Arc::make_mut(&mut f.s.project).render_prefs.overflow_folders = vec![f.text("overflow")];
}

#[test]
fn default_names_avoid_possible_storage_overflow_collisions() {
    let mut f = Fixture::new("overflow-default");
    f.comp("Main");
    with_overflow(&mut f);
    f.add(json!({"output": f.text("other/Main.mp4"), "storageOverflow": true}));
    f.add(json!({"storageOverflow": true}));
    assert_eq!(f.output(1), f.path("Main_1.mp4"));
    assert!(f.clashes().is_empty());
}

#[test]
fn possible_storage_overflow_destinations_cannot_replace_footage() {
    let mut f = Fixture::new("overflow-footage");
    f.comp("Main");
    with_overflow(&mut f);
    f.footage("overflow/Main.mp4");
    f.add(json!({"output": f.text("other/Main.mp4"), "storageOverflow": true}));
    let error = f.render(json!({})).unwrap_err().to_string();
    assert!(error.contains("footage") && error.contains("Main.mp4"), "{error}");
    assert!(f.exported().is_empty());
    assert_eq!(f.read("overflow/Main.mp4"), SOURCE);
}

#[test]
fn default_names_avoid_protected_storage_overflow_destinations() {
    let mut f = Fixture::new("overflow-protected-default");
    f.comp("Main");
    with_overflow(&mut f);
    f.footage("overflow/Main.mp4");
    f.add(json!({"storageOverflow": true}));
    assert_eq!(f.output(0), f.path("Main_1.mp4"));
    f.render(json!({})).unwrap();
    assert_eq!(f.exported().len(), 1);
    assert_eq!(f.read("overflow/Main.mp4"), SOURCE);
}

/// After Effects numbers the second item of one composition (`Comp 1_1.mov`). A default output
/// name that another item of the queue already has is numbered the same way.
#[test]
fn a_second_default_item_for_one_comp_gets_the_next_free_name() {
    let mut f = Fixture::new("second-default");
    f.comp("Main");
    for _ in 0..3 {
        f.add(json!({}));
    }
    assert_eq!(f.outputs(), [f.path("Main.mp4"), f.path("Main_1.mp4"), f.path("Main_2.mp4")]);
    // The name is stored in the item: taking the first one out renames nothing, and the name it
    // frees is the next default.
    f.s.execute("renderQueue.remove", json!({"index": 1})).unwrap();
    assert_eq!(f.outputs(), [f.path("Main_1.mp4"), f.path("Main_2.mp4")]);
    f.add(json!({}));
    assert_eq!(f.output(2), f.path("Main.mp4"));
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    for file in ["Main.mp4", "Main_1.mp4", "Main_2.mp4"] {
        assert_eq!(f.read(file), RENDERED, "{file}");
    }
    assert_eq!(f.exported().len(), 3);
}

#[test]
fn only_outputs_that_name_one_file_are_numbered() {
    let mut f = Fixture::new("same-file-only");
    f.comp("Main");
    f.add(json!({"format": "h264"}));
    f.add(json!({"format": "prores"}));
    f.add(json!({"format": "hevc"})); // H.264 and HEVC both write .mp4
    f.add(json!({"format": "png"}));
    f.add(json!({"format": "png"})); // the same five frames of the same sequence
    assert_eq!(f.outputs(), [f.path("Main.mp4"), f.path("Main.mov"), f.path("Main_1.mp4"), f.path("Main_[#####].png"), f.path("Main_1_[#####].png")]);
    assert!(f.clashes().is_empty());
}

#[test]
fn a_default_name_avoids_every_output_module_of_the_other_items() {
    let mut f = Fixture::new("modules");
    f.comp("Main");
    f.add(json!({}));
    // The item's second module is named Main_2.mp4, so that it does not clash with its first.
    let item = f.s.execute("render.addOutputModule", json!({})).unwrap();
    assert_eq!(Path::new(item["extraOutputs"][0]["outputPath"].as_str().unwrap()), f.path("Main_2.mp4"), "{item}");
    f.add(json!({}));
    f.add(json!({}));
    assert_eq!(f.output(1), f.path("Main_1.mp4"));
    assert_eq!(f.output(2), f.path("Main_3.mp4"), "Main_2.mp4 is the first item's second module");
    assert!(f.clashes().is_empty());
}

#[test]
fn a_name_the_caller_gave_is_never_numbered() {
    let mut f = Fixture::new("named");
    f.comp("Main");
    let out = f.text("final.mp4");
    f.add(json!({"output": out}));
    f.add(json!({"output": out}));
    f.add(json!({"path": out}));
    assert_eq!(f.outputs(), vec![f.path("final.mp4"); 3]);
    assert!(f.s.project.render_queue.iter().all(|i| i.output.output == out), "the names stored in the items are the ones given");
    // Rendering them is what is refused (below): items 2 and 3 each clash with item 1.
    assert_eq!(f.clashes(), [(2, 1, 1), (3, 1, 1)]);
}

#[test]
fn a_pre_render_numbers_its_default_name_too() {
    let mut f = Fixture::new("pre-render");
    f.comp("Main");
    f.s.execute("render.preRender", json!({})).unwrap();
    f.s.execute("render.preRender", json!({})).unwrap();
    f.s.execute("render.preRender", json!({"output": f.text("mine.mov")})).unwrap();
    f.s.execute("render.preRender", json!({"output": f.text("mine.mov")})).unwrap();
    assert_eq!(f.outputs(), [f.path("Main_prerender.mov"), f.path("Main_prerender_1.mov"), f.path("mine.mov"), f.path("mine.mov")]);
}

/// File ▸ Create Proxy queues a render with a default name of its own too.
#[test]
fn create_proxy_numbers_its_default_name_too() {
    let mut f = Fixture::new("proxy-names");
    f.comp("Main");
    f.s.execute("file.createProxy", json!({"kind": "movie"})).unwrap();
    f.s.execute("file.createProxy", json!({"kind": "movie"})).unwrap();
    f.s.execute("file.createProxy", json!({"kind": "movie", "output": f.text("mine.mov")})).unwrap();
    f.s.execute("file.createProxy", json!({"kind": "movie", "output": f.text("mine.mov")})).unwrap();
    assert_eq!(f.outputs(), [f.path("Main_proxy.mov"), f.path("Main_proxy_1.mov"), f.path("mine.mov"), f.path("mine.mov")]);
}

/// Save Frame As ▸ File… with `queue`: every file of a sequence has its frame number in its name,
/// so two frames of one composition share no file, and the same frame twice does.
#[test]
fn queued_frames_number_their_default_names_only_when_they_are_the_same_frame() {
    let mut f = Fixture::new("frames");
    f.comp("Main");
    for time in [0.0, 0.2, 0.0] {
        f.s.execute("comp.saveFrameAs", json!({"queue": true, "time": time})).unwrap();
    }
    assert_eq!(f.outputs(), [f.path("Main_[#####].png"), f.path("Main_[#####].png"), f.path("Main_1_[#####].png")]);
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    for file in ["Main_00000.png", "Main_00002.png", "Main_1_00000.png"] {
        assert_eq!(f.read(file), RENDERED, "{file}");
    }
}

/// The report: two queue items forced to one path (the same Output To) replaced each other's file
/// inside one job. The render is refused before anything is written, whatever `overwrite` says:
/// that flag is for files that exist, and this is the render replacing its own output.
#[test]
fn two_items_forced_to_one_file_are_reported_and_the_render_is_refused() {
    let mut f = Fixture::new("one-file");
    f.comp("Main");
    let out = f.text("render.mp4");
    f.add(json!({"output": out}));
    f.add(json!({"output": out}));
    let found = f.check();
    let list = found["duplicates"].as_array().unwrap_or_else(|| panic!("checkOutputs lists no duplicates: {found}"));
    assert_eq!(list.len(), 1, "{found}");
    let d = &list[0];
    let (first, second) = (f.s.project.render_queue[0].id, f.s.project.render_queue[1].id);
    assert_eq!((d["item"].as_u64(), d["index"].as_u64(), d["comp"].as_str()), (Some(second), Some(2), Some("Main")), "{found}");
    assert_eq!((d["with"]["item"].as_u64(), d["with"]["index"].as_u64(), d["with"]["comp"].as_str()), (Some(first), Some(1), Some("Main")), "{found}");
    assert_eq!((d["files"].as_u64(), d["sequence"].as_bool()), (Some(1), Some(false)), "{found}");
    assert_eq!(Path::new(d["path"].as_str().unwrap_or_default()), f.path("render.mp4"), "{found}");
    let why = d["reason"].as_str().unwrap_or_default();
    assert!(why.contains("#1") && why.contains("#2") && why.contains("render.mp4") && why.contains("Output To"), "{found}");

    let queue = f.s.project.render_queue.clone();
    let e = f.render(json!({})).unwrap_err().to_string();
    assert!(e.contains("#1") && e.contains("#2") && e.contains("render.mp4") && e.contains("Nothing was rendered"), "{e}");
    assert!(!f.path("render.mp4").exists());
    assert!(f.exported().is_empty(), "the exporter was not started");
    assert_eq!(f.s.project.render_queue, queue, "a refused render changes nothing in the queue");

    // A file that exists as well does not change that: replacing it would not help.
    f.write("render.mp4", EARLIER);
    let found = f.check();
    assert_eq!(found["duplicates"].as_array().map(Vec::len), Some(1), "{found}");
    let e = f.render(json!({})).unwrap_err().to_string();
    assert!(e.contains("#2") && e.contains("Nothing was rendered"), "{e}");
    assert_eq!(f.read("render.mp4"), EARLIER);

    // Another Output To for one of them, and both render.
    f.s.execute("renderQueue.setOutput", json!({"index": 2, "path": f.text("second.mp4")})).unwrap();
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    assert_eq!((f.read("render.mp4"), f.read("second.mp4")), (RENDERED.to_vec(), RENDERED.to_vec()));
}

/// `renderQueue.duplicate` copies the item as it is, output included: it is reported until one of
/// the two gets another Output To (or is turned off).
#[test]
fn a_duplicated_item_is_a_copy_and_clashes_until_one_of_them_changes() {
    let mut f = Fixture::new("duplicate");
    f.comp("Main");
    f.add(json!({}));
    f.s.execute("renderQueue.duplicate", json!({"index": 1})).unwrap();
    assert_eq!(f.outputs(), [f.path("Main.mp4"), f.path("Main.mp4")], "a duplicate is a copy, its output included");
    assert_eq!(f.clashes(), [(2, 1, 1)]);
    let e = f.render(json!({})).unwrap_err().to_string();
    assert!(e.contains("#1") && e.contains("#2") && e.contains("Main.mp4"), "{e}");
    assert!(f.exported().is_empty());
    // Turned off, the copy is not part of the job.
    f.s.execute("renderQueue.setRender", json!({"index": 2, "render": false})).unwrap();
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    assert_eq!(f.read("Main.mp4"), RENDERED);
    assert_eq!(f.exported().len(), 1);
}

/// Only the items of the job are compared: an earlier job's file is a file that exists, and
/// replacing it is up to `overwrite`.
#[test]
fn an_item_that_is_not_in_the_job_does_not_clash() {
    let mut f = Fixture::new("not-in-job");
    f.comp("Main");
    let out = f.text("render.mp4");
    f.add(json!({"output": out}));
    f.add(json!({"output": out}));
    f.s.execute("renderQueue.setRender", json!({"index": 2, "render": false})).unwrap();
    f.render(json!({})).unwrap(); // item 1 renders alone
    f.s.execute("renderQueue.setRender", json!({"index": 2, "render": true})).unwrap();
    assert!(f.clashes().is_empty(), "item 1 is done and is not in this job");
    // item 2 now replaces the file item 1 wrote in the earlier job: an ordinary file, left to the exporter
    f.render(json!({})).unwrap();
}

#[test]
fn another_spelling_of_one_file_is_one_file_where_the_volume_ignores_case() {
    let mut f = Fixture::new("spellings");
    f.comp("Main");
    f.write("probe.txt", b"x");
    if !f.path("PROBE.TXT").exists() {
        eprintln!("this volume tells the two names apart: skipped");
        return;
    }
    f.add(json!({"output": f.text("Render.mp4")}));
    f.add(json!({"output": f.text("render.MP4")}));
    f.add(json!({"output": f.dir.join("sub").join("..").join("RENDER.mp4").to_string_lossy()}));
    f.add(json!({"output": f.text("other.mp4")}));
    assert_eq!(f.clashes(), [(2, 1, 1), (3, 1, 1)]);
}

/// A sequence writes one file per frame: two items of one pattern clash on the frames they share,
/// and not at all when their frames differ.
#[test]
fn sequences_clash_only_on_the_frames_they_both_write() {
    let mut f = Fixture::new("sequence-halves");
    f.comp("Main");
    let pattern = f.text("seq_[#####].png");
    f.add(span(&pattern, 0.0, 0.2)); // frames 0-1
    f.add(span(&pattern, 0.2, 0.5)); // frames 2-4
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    for n in 0..5 {
        assert_eq!(f.read(&format!("seq_{n:05}.png")), RENDERED, "frame {n}");
    }

    let mut f = Fixture::new("sequence-overlap");
    f.comp("Main");
    let pattern = f.text("seq_[#####].png");
    f.add(span(&pattern, 0.0, 0.2));
    f.add(span(&pattern, 0.2, 0.5));
    f.add(span(&pattern, 0.3, 0.5)); // frames 3-4 again
    assert_eq!(f.clashes(), [(3, 2, 2)]);
    let found = f.check();
    assert_eq!(found["duplicates"][0]["sequence"], true, "{found}");
    let e = f.render(json!({"overwrite": true})).unwrap_err().to_string();
    assert!(e.contains("#2") && e.contains("#3") && e.contains("2 frames") && e.contains("seq_[#####].png"), "{e}");
    assert!(!f.path("seq_00000.png").exists(), "the frames before the clash were not rendered either");
    assert!(f.exported().is_empty());
}

#[test]
fn differently_padded_sequences_cannot_replace_the_same_frames() {
    let mut f = Fixture::new("padding-width-clash");
    f.s.execute("comp.new", json!({"name": "Main", "width": 16, "height": 16, "frameRate": 10, "duration": 2})).unwrap();
    f.add(span(&f.text("seq_[#].png"), 0.9, 1.2));
    f.add(span(&f.text("seq_[##].png"), 0.9, 1.2));
    assert_eq!(f.clashes(), [(2, 1, 2)]);
    assert!(f.render(json!({})).is_err());
    assert!(f.exported().is_empty());
}

/// Skip Existing Files leaves a frame alone when it is there: the item that skips and comes second
/// writes only what the first did not. The item that skips and comes first is written over.
#[test]
fn a_later_item_that_skips_existing_frames_leaves_the_earlier_ones_alone() {
    let mut f = Fixture::new("skip-later");
    f.comp("Main");
    let pattern = f.text("seq_[#####].png");
    f.add(json!({"format": "png", "output": pattern}));
    f.add(json!({"format": "png", "output": pattern, "skipExisting": true}));
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    assert_eq!(f.read("seq_00004.png"), RENDERED);

    let mut f = Fixture::new("skip-earlier");
    f.comp("Main");
    let pattern = f.text("seq_[#####].png");
    f.add(json!({"format": "png", "output": pattern, "skipExisting": true}));
    f.add(json!({"format": "png", "output": pattern}));
    assert_eq!(f.clashes(), [(2, 1, 5)]);
}

#[test]
fn two_output_modules_of_one_item_cannot_share_a_file() {
    let mut f = Fixture::new("two-modules");
    f.comp("Main");
    let out = f.text("render.mp4");
    f.add(json!({"output": out}));
    f.s.execute("render.addOutputModule", json!({"output": out})).unwrap();
    assert_eq!(f.clashes(), [(1, 1, 1)]);
    let e = f.render(json!({})).unwrap_err().to_string();
    assert!(e.contains("#1") && e.contains("twice"), "{e}");
    assert!(f.exported().is_empty());
}

/// Settings ▸ Export ▸ Segment Sequences puts each item's frames in numbered folders: default
/// names keep two items' folders apart, and items given one name clash segment by segment.
#[test]
fn segmented_sequences_clash_segment_by_segment() {
    let mut f = Fixture::new("segments");
    f.comp("Main");
    f.s.prefs.export.segment_sequences = true;
    f.s.prefs.export.segment_sequence_files = 2; // five frames: segments of 2, 2 and 1
    f.add(json!({"format": "png"}));
    f.add(json!({"format": "png"}));
    assert_eq!(f.output(1), f.path("Main_1_[#####].png"));
    assert!(f.clashes().is_empty());
    f.render(json!({})).unwrap();
    assert_eq!(f.read("Main_001/Main_00000.png"), RENDERED);
    assert_eq!(f.read("Main_1_003/Main_1_00004.png"), RENDERED);

    let mut f = Fixture::new("segments-named");
    f.comp("Main");
    f.s.prefs.export.segment_sequences = true;
    f.s.prefs.export.segment_sequence_files = 2;
    let pattern = f.text("seg_[#####].png");
    f.add(json!({"format": "png", "output": pattern}));
    f.add(json!({"format": "png", "output": pattern}));
    assert_eq!(f.clashes(), [(2, 1, 2), (2, 1, 2), (2, 1, 1)]);
    let e = f.render(json!({})).unwrap_err().to_string();
    assert!(e.contains("2 more"), "the first clash is named, the rest counted: {e}");
    assert!(f.exported().is_empty());
}

/// An item that is refused for the file it would replace does not write anything, so it cannot
/// clash with another item.
#[test]
fn items_refused_for_footage_are_not_compared_with_each_other() {
    let mut f = Fixture::new("refused-pair");
    f.footage("clip.mov");
    f.comp("Main");
    let clip = f.text("clip.mov");
    f.add(json!({"format": "prores", "output": clip}));
    f.add(json!({"format": "prores", "output": clip}));
    f.add(json!({"output": f.text("ok.mp4")}));
    let found = f.check();
    assert_eq!(found["duplicates"].as_array().map(Vec::len), Some(0), "{found}");
    let r = f.render(json!({})).unwrap();
    let failed = r["items"].as_array().unwrap().iter().filter(|i| i["status"].get("Failed").is_some()).count();
    assert_eq!(failed, 2, "both items on the footage are refused: {r}");
    assert_eq!(f.read("clip.mov"), SOURCE);
    assert_eq!(f.read("ok.mp4"), RENDERED);
}
