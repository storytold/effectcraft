//! Native linked-media workflow. Fixtures are our own generated RGBA pixels and stereo tones;
//! the shipping CLI encodes the movies, and the host's MediaPool decodes them. No external
//! encoder, downloaded fixture, optional tool or skipped codec check is involved.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use effectcraft_engine::Session;
use effectcraft_engine::project::{Footage, ItemId, ItemKind};
use effectcraft_render::{Image, audio};
use effectcraft_time::Tick;
use serde_json::{Value, json};

fn directory() -> PathBuf {
    let parent = std::path::absolute(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-out/linked-native-media")).unwrap();
    std::fs::create_dir_all(&parent).unwrap();
    for attempt in 0..1000 {
        let path = parent.join(format!("run-{}-{attempt}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!("cannot create linked-media test directory: {error}"),
        }
    }
    panic!("cannot allocate linked-media test directory");
}

fn comp(session: &mut Session, name: &str, width: u32, height: u32, fps: u32, duration: f64) -> ItemId {
    let result = session.execute("comp.new", json!({"name":name,"width":width,"height":height,"frameRate":fps,"duration":duration})).unwrap();
    ItemId(result["comp"].as_u64().unwrap())
}

fn save(session: &mut Session, path: &Path) {
    session.execute("file.saveAs", json!({"path":path.to_string_lossy()})).unwrap();
}

fn import(session: &mut Session, path: &Path, sequence: bool, add: bool) -> ItemId {
    let result = session.execute("file.import", json!({"paths":[path.to_string_lossy()],"sequence":sequence,"frameRate":24,"addToComp":add})).unwrap();
    ItemId(result["items"][0].as_u64().unwrap())
}

fn footage(session: &Session, item: ItemId) -> &Footage {
    let ItemKind::Footage(footage) = &session.project.item(item).unwrap().kind else { panic!("expected imported footage") };
    footage
}

fn write_sequence(directory: &Path, revised: bool) {
    for frame in 1..=24 {
        let color = if revised {
            if frame <= 12 { [16, 220, 24, 128] } else { [220, 220, 16, 128] }
        } else if frame <= 12 {
            [220, 16, 24, 128]
        } else {
            [16, 24, 220, 128]
        };
        let image = image::RgbaImage::from_fn(32, 24, |x, y| image::Rgba(if (4..28).contains(&x) && (4..20).contains(&y) { color } else { [0, 0, 0, 0] }));
        image.save(directory.join(format!("plate_{frame:04}.png"))).unwrap();
    }
}

fn write_tone(path: &Path) {
    const RATE: u32 = 48_000;
    let bytes = RATE * 4;
    let mut wav = Vec::with_capacity((44 + bytes) as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for sample in 0..RATE {
        for frequency in [440.0, 660.0] {
            let phase = std::f64::consts::TAU * frequency * f64::from(sample) / f64::from(RATE);
            wav.extend_from_slice(&((phase.sin() * 0.2 * f64::from(i16::MAX)) as i16).to_le_bytes());
        }
    }
    std::fs::write(path, wav).unwrap();
}

fn export_movie(directory: &Path, project: &Path, comp: &str, out: &Path) {
    let mut child = Command::new(crate::binary())
        .current_dir(directory)
        .arg("render")
        .arg("--project")
        .arg(project)
        .args(["--comp", comp, "--format", "prores", "--prores", "4444", "--channels", "rgba", "--audio", "on", "--out"])
        .arg(out)
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            timed_out = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!timed_out, "native movie export stalled: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "native export failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!result["rendered"].as_array().unwrap().is_empty(), "{result}");
    assert!(std::fs::metadata(out).unwrap().len() > 0);
}

fn frame(session: &Session, comp: ItemId, seconds: f64) -> Image {
    session.render(comp, Tick::from_seconds_f64(seconds), Default::default())
}

fn save_frame(path: &Path, frame: &Image) {
    image::save_buffer(path, &frame.to_rgba8(), frame.width, frame.height, image::ColorType::Rgba8).unwrap();
}

fn mix(session: &Session, comp: ItemId, seconds: f64) -> Vec<f32> {
    audio::mix_comp(&session.project, session.footage.as_ref(), session.expr.as_deref(), comp, Tick::from_seconds_f64(seconds), 480, 48_000)
}

fn rms(samples: &[f32]) -> f32 {
    assert!(!samples.is_empty() && samples.iter().all(|sample| sample.is_finite()));
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn check_offline_recovery(destination: &mut Session, main: ItemId, linked: ItemId, source_path: &Path, movie: &Path, expected: &Image) {
    let local = destination.project.comp(main).unwrap().layers.clone();
    let unavailable_source = source_path.with_file_name("Source-away.ecproj");
    std::fs::rename(source_path, &unavailable_source).unwrap();
    assert!(destination.execute("file.reloadFootage", json!({"items":[linked.0]})).is_err(), "a missing project is explicitly offline");
    assert!(frame(destination, main, 0.3).get(36, 24)[3] < 0.01, "a failed refresh does not render a stale generation");
    assert_eq!(destination.project.comp(main).unwrap().layers, local);
    destination.execute("file.save", json!({})).unwrap();
    std::fs::rename(&unavailable_source, source_path).unwrap();
    destination.execute("file.reloadFootage", json!({"items":[linked.0]})).unwrap();
    assert_eq!(frame(destination, main, 0.3).data, expected.data, "restoring the source recovers the same saved generation");

    let unavailable_movie = movie.with_file_name("plate-away.mov");
    std::fs::rename(movie, &unavailable_movie).unwrap();
    assert!(destination.execute("file.reloadFootage", json!({"items":[linked.0]})).is_err(), "a missing hidden movie is explicitly offline");
    assert!(frame(destination, main, 0.3).get(36, 24)[3] < 0.01);
    std::fs::rename(&unavailable_movie, movie).unwrap();
    destination.execute("file.reloadFootage", json!({"items":[linked.0]})).unwrap();
    assert_eq!(frame(destination, main, 0.3).data, expected.data);
}

fn check_exported_movie(path: &Path) {
    let mut decoded = effectcraft_host::session();
    let item = import(&mut decoded, path, false, false);
    let exported = footage(&decoded, item);
    assert!(exported.has_video && exported.has_audio, "the native linked render exports its sound track");
    assert_eq!(exported.frame_rate.as_f64(), 30.0);
    let image = decoded.footage.frame(item, exported, Tick::from_seconds_f64(0.3)).unwrap();
    assert!(image.get(36, 24)[1] > image.get(36, 24)[0]);
    assert!(image.get(36, 24)[3] > 0.02 && image.get(36, 24)[3] < 0.4, "partial alpha survives ProRes 4444 export and import");
    assert!(image.get(1, 1)[3] < 0.01, "transparent canvas survives export");
    let samples = |seconds| decoded.footage.audio(item, exported, Tick::from_seconds_f64(seconds), 480, 48_000).unwrap();
    assert!(rms(&samples(0.3)) > 0.02);
    assert!(rms(&samples(0.1)) < 0.001, "the exported audio retains the local in point");
    assert!(rms(&samples(0.9)) < 0.001, "the exported audio retains the shortened source duration");
}

#[test]
fn linked_movie_refresh_and_cli_export_preserve_alpha_audio_timing_and_local_edits() {
    let directory = directory();
    let seed_path = directory.join("Fixture.ecproj");
    let movie = directory.join("plate.mov");
    let replacement = directory.join("replacement.mov");
    let source_path = directory.join("Source.ecproj");
    let main_path = directory.join("Main.ecproj");
    let final_movie = directory.join("linked-output.mov");
    write_sequence(&directory, false);
    write_tone(&directory.join("tone.wav"));
    let mut seed = effectcraft_host::session();
    comp(&mut seed, "Fixture", 32, 24, 24, 1.0);
    import(&mut seed, &directory.join("plate_0001.png"), true, true);
    import(&mut seed, &directory.join("tone.wav"), false, true);
    save(&mut seed, &seed_path);
    export_movie(&directory, &seed_path, "Fixture", &movie);

    let mut source = effectcraft_host::session();
    let source_comp = comp(&mut source, "Opening", 32, 24, 24, 1.0);
    let media = import(&mut source, &movie, false, true);
    assert!(footage(&source, media).has_audio && footage(&source, media).has_video);
    assert_eq!(footage(&source, media).frame_rate.as_f64(), 24.0);
    let source_layer = source.project.comp(source_comp).unwrap().layers[0].id;
    save(&mut source, &source_path);

    let mut destination = effectcraft_host::session();
    let main = comp(&mut destination, "Main", 64, 48, 30, 1.2);
    let local_items = destination.project.items.len();
    let result = destination.execute("file.linkComposition", json!({"path":source_path.to_string_lossy(),"comp":"Opening"})).unwrap();
    let linked = ItemId(result["item"].as_u64().unwrap());
    assert_eq!(destination.project.items.len(), local_items + 1, "the imported movie stays inside the linked source");
    let result = destination.execute("layer.addItem", json!({"item":linked.0,"time":0.1,"in":0.2,"out":1.0,"position":[36,24]})).unwrap();
    let layer = result["layer"].as_u64().unwrap();
    destination.execute("prop.set", json!({"layer":layer,"path":"transform/scale","value":[75,75]})).unwrap();
    destination.execute("prop.set", json!({"layer":layer,"path":"transform/opacity","value":60})).unwrap();
    save(&mut destination, &main_path);
    let local = destination.project.comp(main).unwrap().layers.clone();
    let before = frame(&destination, main, 0.3);
    save_frame(&directory.join("before.png"), &before);
    let late = frame(&destination, main, 0.7);
    assert!(before.get(36, 24)[0] > before.get(36, 24)[2]);
    assert!(late.get(36, 24)[2] > late.get(36, 24)[0], "the 24fps movie advances inside the 30fps destination");
    assert!(before.get(36, 24)[3] > 0.1 && before.get(36, 24)[3] < 0.5, "the linked movie preserves partial alpha and local opacity");
    assert!(before.get(1, 1)[3] < 0.01);
    assert!(rms(&mix(&destination, main, 0.3)) > 0.02);
    assert!(rms(&mix(&destination, main, 0.1)) < 0.001, "the local in point trims audio too");

    source.execute("prop.set", json!({"layer":source_layer.0,"path":"transform/opacity","value":40})).unwrap();
    source.execute("comp.settings", json!({"comp":source_comp.0,"width":40,"height":28,"anchor":0,"duration":0.75})).unwrap();
    assert_eq!(frame(&destination, main, 0.3).data, before.data, "unsaved source edits remain invisible");
    source.execute("file.save", json!({})).unwrap();
    assert_eq!(frame(&destination, main, 0.3).data, before.data, "saved edits wait for Reload Footage in the manual-refresh prototype");
    destination.execute("file.reloadFootage", json!({"items":[linked.0]})).unwrap();
    let refreshed = frame(&destination, main, 0.3);
    save_frame(&directory.join("source-refresh.png"), &refreshed);
    assert!(refreshed.get(36, 24)[3] < before.get(36, 24)[3] * 0.6);
    assert_eq!((footage(&destination, linked).width, footage(&destination, linked).height), (40, 28));
    assert_eq!(destination.project.comp(main).unwrap().layers, local, "source size and duration changes retain the local transform and trims");
    assert!(rms(&mix(&destination, main, 0.9)) < 0.001, "linked audio stops at the shortened source duration");

    // Change only hidden source media, retaining its path. A real decoder and its cached
    // frames have already been used; Reload Footage must discard them as well as the comp.
    let source_bytes = std::fs::read(&source_path).unwrap();
    write_sequence(&directory, true);
    export_movie(&directory, &seed_path, "Fixture", &replacement);
    std::fs::copy(&replacement, &movie).unwrap();
    destination.execute("file.reloadFootage", json!({"items":[linked.0]})).unwrap();
    let changed_media = frame(&destination, main, 0.3);
    save_frame(&directory.join("media-refresh.png"), &changed_media);
    assert_ne!(changed_media.data, refreshed.data);
    assert!(changed_media.get(36, 24)[1] > changed_media.get(36, 24)[0], "the decoder shows the replaced source movie");
    assert_eq!(std::fs::read(&source_path).unwrap(), source_bytes, "media invalidation does not require resaving the source project");
    assert_eq!(destination.project.comp(main).unwrap().layers, local);

    check_offline_recovery(&mut destination, main, linked, &source_path, &movie, &changed_media);
    destination.execute("file.save", json!({})).unwrap();
    export_movie(&directory, &main_path, "Main", &final_movie);
    check_exported_movie(&final_movie);
}
