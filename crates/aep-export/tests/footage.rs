//! Footage items and layers: projects with every footage kind the exporter writes are exported
//! and parsed back with `effectcraft-aep` (type tag, size, duration, rate, numbering, alias path,
//! the name), the footage fixtures go through import, export and import, and what cannot be
//! written is reported.

mod common;

use common::*;
use effectcraft_aep::{PropNode, parse};
use effectcraft_aep_export::LossCategory;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_color::Label;
use effectcraft_project::{Footage, FootageKind, ItemId, ItemKind, LayerSource, Project};
use effectcraft_time::FrameRate;

fn still(path: &str, w: u32, h: u32) -> Footage {
    Footage { path: path.to_string(), kind: FootageKind::Still, width: w, height: h, has_video: true, ..Footage::default() }
}

fn seq(dir: &str, prefix: &str, start: u32, n: u32, digits: usize, suffix: &str, rate: FrameRate) -> Footage {
    let files: Vec<String> = (start..start + n).map(|k| format!("{dir}\\{prefix}{k:0digits$}{suffix}")).collect();
    let mut f = Footage { kind: FootageKind::Sequence, frame_rate: rate, sequence: files.clone(), ..still(&files[0], 64, 48) };
    f.sync_sequence_duration();
    f
}

/// Adds the footage and a layer showing it, as long as the footage (stills: the composition).
fn add_footage(p: &mut Project, comp: ItemId, name: &str, f: Footage) -> ItemId {
    let size = (f.width.max(1), f.height.max(1));
    let timed = f.kind != FootageKind::Still && f.duration > effectcraft_time::Tick::ZERO;
    let duration = f.duration;
    let id = p.add_item(name, Label::Lavender, None, ItemKind::Footage(f));
    add_layer(p, comp, name, LayerSource::Footage { item: id }, size);
    if timed {
        with_layer(p, comp, 0, |l| l.out_point = l.start_time + duration);
    }
    id
}

fn reimport(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).expect("import");
    p
}

/// One of every kind in a composition.
fn all_kinds() -> Project {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 1920, 1080, FrameRate::new(25, 1), 5.0, None);
    add_footage(&mut p, c, "still.png", still(r"C:\media\still.png", 64, 48));
    add_footage(&mut p, c, "still.jpg", still(r"C:\media\still.jpg", 64, 48));
    add_footage(&mut p, c, "Renamed EXR", still(r"C:\media\still.exr", 64, 48));
    add_footage(&mut p, c, "shot_[0101-0103].exr", seq(r"C:\media\exrseq", "shot_", 101, 3, 4, ".exr", FrameRate::new(30, 1)));
    add_footage(&mut p, c, "seq_[001-003].png", seq(r"C:\media\test-seq", "seq_", 1, 3, 3, ".png", FrameRate::new(24, 1)));
    let clip = Footage { kind: FootageKind::Video, frame_rate: FrameRate::new(25, 1), duration: secs(2.0), ..still(r"C:\media\clip.mp4", 64, 48) };
    add_footage(&mut p, c, "clip.mp4", clip);
    let wav = Footage { kind: FootageKind::Audio, duration: secs(1.5), has_video: false, has_audio: true, ..still(r"C:\media\tone.wav", 0, 0) };
    add_footage(&mut p, c, "tone.wav", wav);
    add_footage(&mut p, c, "still.tif", still(r"C:\media\still.tif", 64, 48));
    add_footage(&mut p, c, "j_[000-002].jpg", seq(r"C:\media\jpgseq", "j_", 0, 3, 3, ".jpg", FrameRate::new(25, 1)));
    let mov = Footage { kind: FootageKind::Video, frame_rate: FrameRate::new(25, 1), duration: secs(1.0), ..still(r"C:\media\clip.mov", 64, 48) };
    add_footage(&mut p, c, "clip.mov", mov);
    for e in ["tga", "psd", "ai", "eps"] {
        add_footage(&mut p, c, &format!("still.{e}"), still(&format!(r"C:\media\still.{e}"), 64, 48));
    }
    // 645: 25 native frames conformed to 30 fps.
    let conformed = Footage {
        kind: FootageKind::Video,
        frame_rate: FrameRate::new(30, 1),
        native_rate: Some(FrameRate::new(25, 1)),
        duration: FrameRate::new(30, 1).tick_of(25),
        ..still(r"C:\media\conformed.mp4", 64, 48)
    };
    add_footage(&mut p, c, "conformed.mp4", conformed);
    p
}

#[test]
fn every_kind_is_written_with_its_fields() {
    let p = all_kinds();
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    assert_eq!((r.report.exported.footage, r.report.exported.footage_layers), (15, 15));
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let items = view.all_items();
    let by_path = |end: &str| items.iter().find(|i| i.footage().and_then(|f| f.path).is_some_and(|p| p.ends_with(end))).copied().expect(end);
    // (path end, tag, w, h, duration num / den, fps, sequence)
    let want: [(&str, &str, u32, u32, i64, i64, u32, bool); 15] = [
        (r"\still.png", "png!", 64, 48, 0, 1, 0, false),
        (r"\still.jpg", "ZPEG", 64, 48, 0, 600, 0, false),
        (r"\still.exr", "oEXR", 64, 48, 0, 100, 0, false),
        (r"\exrseq", "oEXR", 64, 48, 3, 30, 30, true),
        (r"\test-seq", "png!", 64, 48, 3, 24, 24, true),
        (r"\clip.mp4", "XCEX", 64, 48, 50, 25, 25, false),
        (r"\tone.wav", "WAVE", 0, 0, 72_000, 48_000, 0, false),
        (r"\still.tif", "TIF ", 64, 48, 0, 600, 0, false),
        (r"\jpgseq", "ZPEG", 64, 48, 3, 25, 25, true),
        (r"\clip.mov", "MOoV", 64, 48, 25, 25, 25, false),
        (r"\still.tga", "TPIC", 64, 48, 0, 600, 0, false),
        (r"\still.psd", "8BPS", 64, 48, 0, 600, 0, false),
        (r"\still.ai", "TEXT", 64, 48, 0, 600, 0, false),
        (r"\still.eps", "TEXT", 64, 48, 0, 600, 0, false),
        (r"\conformed.mp4", "XCEX", 64, 48, 25, 25, 25, false),
    ];
    for (end, tag, w, h, num, den, fps, is_seq) in want {
        let item = by_path(end);
        let f = item.footage().expect("footage");
        assert_eq!((f.file_type.as_str(), f.width, f.height, f.native_frame_rate, f.is_sequence), (tag, w, h, fps, is_seq), "{end}");
        let d = f.duration.expect("duration");
        assert_eq!((d.num, d.den), (num, den), "{end}");
    }
    // The conformed clip keeps its native rate and frames and stores the conformed rate (645).
    assert_eq!(by_path(r"\conformed.mp4").conform_frame_rate(), Some(30));
    assert_eq!(by_path(r"\clip.mp4").conform_frame_rate(), None);
    let s = by_path(r"\exrseq").sequence_info().expect("numbering");
    assert_eq!((s.prefix.as_str(), s.suffix.as_str(), s.start, s.end, s.digits), ("shot_", ".exr", 101, 103, 4));
    let s = by_path(r"\test-seq").sequence_info().expect("numbering");
    assert_eq!((s.prefix.as_str(), s.suffix.as_str(), s.start, s.end, s.digits), ("seq_", ".png", 1, 3, 3));
    // A default name is not stored (After Effects names the item after its file), another one is.
    assert_eq!(by_path(r"\still.png").name(), None);
    assert_eq!(by_path(r"\still.png").display_name().as_deref(), Some("still.png"));
    assert_eq!(by_path(r"\exrseq").name(), None);
    assert_eq!(by_path(r"\still.exr").name().as_deref(), Some("Renamed EXR"));
    // png options: size and the first file's name.
    assert_eq!(by_path(r"\test-seq").footage().and_then(|f| f.name).as_deref(), Some("seq_001.png"));
    // Layers: the source, its size, the Time Remapping maximum (the source's duration).
    let comp = items.iter().find_map(|i| i.comp()).expect("comp");
    for layer in comp.layers() {
        let src = view.layer_source(&layer).expect("source");
        let f = src.footage().expect("footage source");
        let Some(PropNode::Property(tr)) = layer.properties().and_then(|g| g.get("ADBE Time Remapping")) else { panic!("time remap") };
        let max = tr.chunk().child(b"tduM").and_then(|c| c.bytes()).map(|b| f64::from_be_bytes(b[..8].try_into().expect("8"))).expect("tduM");
        let d = f.duration.expect("duration");
        // A conformed clip lasts its native frames at the conformed rate (645: 25 / 30 s).
        let den = src.conform_frame_rate().map_or(d.den, i64::from);
        assert!((max - d.num as f64 / den as f64).abs() < 1e-9, "{:?}: {max}", f.path);
    }
}

#[test]
fn the_alias_holds_the_path_escaped() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 1.0, None);
    add_footage(&mut p, c, "odd", still("C:\\a \"quoted\" dir\\Ünïcödé\\x.png", 8, 8));
    add_footage(&mut p, c, "mac", still("/Volumes/Media/a.jpg", 8, 8));
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let paths: Vec<String> = view.all_items().iter().filter_map(|i| i.footage()).filter_map(|f| f.path).collect();
    assert!(paths.contains(&"C:\\a \"quoted\" dir\\Ünïcödé\\x.png".to_string()), "{paths:?}");
    assert!(paths.contains(&"/Volumes/Media/a.jpg".to_string()), "{paths:?}");
}

#[test]
fn footage_survives_import_export_import() {
    for name in [
        "126-footage-png-layer.aep",
        "603-footage-exrseq-0101-as-layer.aep",
        "645-footage-mp4-conform-30.aep",
        "650-footage-tga-still-as-layer.aep",
        "651-footage-psd-flat-as-layer.aep",
        "652-footage-ai-as-layer.aep",
        "653-footage-eps-as-layer.aep",
    ] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
        let p1 = reimport(&std::fs::read(dir.join(name)).expect("fixture"));
        let r = export_all(&p1);
        assert_eq!(r.report.count(LossCategory::FootageItems) + r.report.count(LossCategory::UnsupportedLayers), 0, "{name}: {:?}", r.report.losses);
        let p2 = reimport(&r.bytes);
        let footage = |p: &Project| -> Vec<(String, Footage)> {
            let mut v: Vec<(String, Footage)> =
                p.items.values().filter_map(|i| if let ItemKind::Footage(f) = &i.kind { Some((i.name.clone(), f.clone())) } else { None }).collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let (a, b) = (footage(&p1), footage(&p2));
        assert_eq!(a.len(), 1, "{name}");
        assert_eq!(a.len(), b.len(), "{name}");
        for ((na, fa), (nb, fb)) in a.iter().zip(&b) {
            assert_eq!(na, nb, "{name}");
            assert_eq!((&fa.path, fa.kind, fa.width, fa.height, &fa.sequence), (&fb.path, fb.kind, fb.width, fb.height, &fb.sequence), "{name}");
            assert_eq!((fa.frame_rate, fa.native_rate, fa.duration), (fb.frame_rate, fb.native_rate, fb.duration), "{name}");
        }
        let layers = |p: &Project| -> usize {
            p.items
                .values()
                .filter_map(|i| {
                    if let ItemKind::Comp(c) = &i.kind {
                        Some(c.layers.iter().filter(|l| matches!(l.source, LayerSource::Footage { .. })).count())
                    } else {
                        None
                    }
                })
                .sum()
        };
        assert_eq!(layers(&p1), 1, "{name}");
        assert_eq!(layers(&p2), 1, "{name}");
    }
}

/// The stored folder time (`sspc@118`) of the sequence item, big-endian.
fn sequence_folder_time(bytes: &[u8]) -> u32 {
    let file = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let item = view.all_items().into_iter().find(|i| i.footage().is_some_and(|f| f.is_sequence)).expect("sequence");
    let sspc = item.chunk().child_list(b"Pin ").and_then(|pin| pin.child(b"sspc")).and_then(|c| c.bytes()).expect("sspc");
    u32::from_be_bytes(sspc[118..122].try_into().expect("4 bytes"))
}

/// 860: After Effects stores the sequence folder's modification time (seconds since 1904, UTC) at
/// `sspc@118` and rescans the folder on open when it differs from the folder's, warning about the
/// missing number of a sparse sequence in a modal dialog (export-tests x1-x3: the folder's time
/// opens without the dialog, the first or the last file's time does not). The export writes the
/// time the caller gives (the folder of 860 at 2026-10-19 03:51:22 UTC is stored as 0xe6fb443a),
/// only for sequences, and keeps the template's without one.
#[test]
fn a_sequence_stores_its_folder_time() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 64, 48, FrameRate::new(30, 1), 1.0, None);
    add_footage(&mut p, c, "sp_[0001-0005].png", seq(r"C:\media\sparse", "sp_", 1, 5, 4, ".png", FrameRate::new(30, 1)));
    add_footage(&mut p, c, "still.png", still(r"C:\media\still.png", 64, 48));
    let asked = std::cell::RefCell::new(Vec::<String>::new());
    let time = |dir: &str| {
        asked.borrow_mut().push(dir.to_string());
        Some(1_792_381_882)
    };
    let opts = effectcraft_aep_export::ExportOptions { allow_losses: true };
    let r = effectcraft_aep_export::export_with(&p, &opts, &time).expect("export");
    assert_eq!(sequence_folder_time(&r.bytes), 0xe6fb_443a);
    assert_eq!(*asked.borrow(), [r"C:\media\sparse"], "only the sequence's folder is asked for");
    // Without a time (plain `export`, or an unknown folder) the template's value stays.
    let plain = export_all(&p);
    let unknown = effectcraft_aep_export::export_with(&p, &opts, &|_| None).expect("export");
    assert_eq!(sequence_folder_time(&unknown.bytes), sequence_folder_time(&plain.bytes));
    assert_ne!(sequence_folder_time(&plain.bytes), 0xe6fb_443a);
    // A time beyond the field (after 2040) is not written either.
    let late = effectcraft_aep_export::export_with(&p, &opts, &|_| Some(u64::MAX)).expect("export");
    assert_eq!(sequence_folder_time(&late.bytes), sequence_folder_time(&plain.bytes));
    // The stills stay byte for byte as without times.
    let still_bytes = |bytes: &[u8]| {
        let file = parse(bytes).expect("parse");
        let view = effectcraft_aep::Project::new(&file).expect("project");
        let item = view.all_items().into_iter().find(|i| i.footage().is_some_and(|f| !f.is_sequence)).expect("still");
        item.chunk().child_list(b"Pin ").and_then(|pin| pin.child(b"sspc")).and_then(|c| c.bytes()).expect("sspc").to_vec()
    };
    assert_eq!(still_bytes(&r.bytes), still_bytes(&plain.bytes));
}

/// A sparse sequence (860: After Effects imports sp_0001, 0002, 0004, 0005 as `sp_[0001-0005].png`
/// and stores only the folder and the numbering 1-5): the import lists every number in the range,
/// files on disk or not, and the export writes the same range, so After Effects finds the folder
/// and treats the missing number as it does in the original (it holds the frame before). Opened
/// without a dialog only with the folder's time (`a_sequence_stores_its_folder_time`).
#[test]
fn a_sparse_sequence_keeps_its_range() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 64, 48, FrameRate::new(30, 1), 1.0, None);
    add_footage(&mut p, c, "sp_[0001-0005].png", seq(r"C:\media\sparse", "sp_", 1, 5, 4, ".png", FrameRate::new(30, 1)));
    let r = export_all(&p);
    assert!(r.report.is_lossless(), "{:?}", r.report.losses);
    let file = parse(&r.bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&file).expect("project");
    let item = view.all_items().into_iter().find(|i| i.footage().is_some()).expect("footage");
    assert_eq!(item.footage().and_then(|f| f.path).as_deref(), Some(r"C:\media\sparse"), "the alias is the folder");
    let s = item.sequence_info().expect("numbering");
    assert_eq!((s.start, s.end, s.digits), (1, 5, 4));
    // Re-import with number 3 absent on disk: still five frames, 1 to 5.
    let mut q = Project::default();
    let exists = |path: &str| !path.ends_with("sp_0003.png");
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&exists) };
    import(&mut q, &r.bytes, &opts).expect("import");
    let f = q.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).expect("footage");
    let names: Vec<&str> = f.sequence.iter().map(|s| s.rsplit('\\').next().unwrap_or(s)).collect();
    assert_eq!(names, ["sp_0001.png", "sp_0002.png", "sp_0003.png", "sp_0004.png", "sp_0005.png"]);
}

#[test]
fn what_cannot_be_written_is_reported() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 1.0, None);
    add_footage(&mut p, c, "avi", Footage { kind: FootageKind::Video, frame_rate: FrameRate::new(25, 1), ..still(r"C:\m\a.avi", 8, 8) });
    add_footage(&mut p, c, "bmp", still(r"C:\m\a.bmp", 8, 8));
    add_footage(&mut p, c, "ntsc", Footage { kind: FootageKind::Video, frame_rate: FrameRate::FPS_29_97, ..still(r"C:\m\a.mp4", 8, 8) });
    let mut gaps = seq(r"C:\m", "g_", 1, 3, 2, ".png", FrameRate::new(25, 1));
    gaps.sequence.remove(1);
    add_footage(&mut p, c, "gaps", gaps);
    add_footage(&mut p, c, "nopath", still("", 8, 8));
    add_footage(&mut p, c, "mp3", Footage { kind: FootageKind::Audio, ..still(r"C:\m\a.mp3", 0, 0) });
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::FootageItems), 6, "{:?}", r.report.losses);
    assert_eq!(r.report.count(LossCategory::UnsupportedLayers), 6, "{:?}", r.report.losses);
    assert_eq!(r.report.exported.footage, 0);
    parse(&r.bytes).expect("still a valid file");
}

#[test]
fn a_layer_past_its_source_s_end_is_reported() {
    // After Effects ends such a layer at the source's end (f1 read-back).
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 5.0, None);
    add_footage(
        &mut p,
        c,
        "clip.mp4",
        Footage { kind: FootageKind::Video, frame_rate: FrameRate::new(25, 1), duration: secs(2.0), ..still(r"C:\m\clip.mp4", 8, 8) },
    );
    with_layer(&mut p, c, 0, |l| l.out_point = secs(4.0));
    add_footage(&mut p, c, "still.png", still(r"C:\m\still.png", 8, 8));
    let r = export_all(&p);
    assert_eq!(r.report.count(LossCategory::LayerTiming), 1, "{:?}", r.report.losses);
    assert_eq!(r.report.exported.footage_layers, 2);
}

#[test]
fn hostile_footage_never_panics() {
    let mut p = Project::default();
    let c = add_comp(&mut p, "C", 640, 360, FrameRate::new(25, 1), 1.0, None);
    let big = seq(r"C:\m", "f", 1, 100_000, 6, ".exr", FrameRate::new(25, 1));
    add_footage(&mut p, c, "big", big);
    add_footage(&mut p, c, "huge", still(r"C:\m\h.png", u32::MAX, u32::MAX));
    add_footage(&mut p, c, "ctl", still("C:\\m\\a\u{1}b.png", 8, 8));
    add_footage(&mut p, c, "nodigits", seq(r"C:\m", "x", 1, 1, 1, ".png", FrameRate::new(25, 1)));
    let mut nod = still(r"C:\m\plain.png", 8, 8);
    nod.kind = FootageKind::Sequence;
    add_footage(&mut p, c, "plain-seq", nod);
    add_footage(&mut p, c, "zero-rate", Footage { kind: FootageKind::Video, frame_rate: FrameRate::new(0, 1), ..still(r"C:\m\z.mp4", 8, 8) });
    add_footage(&mut p, c, "long", Footage { kind: FootageKind::Audio, duration: effectcraft_time::Tick(i64::MAX), ..still(r"C:\m\l.wav", 0, 0) });
    let r = export_all(&p);
    let file = parse(&r.bytes).expect("parse");
    assert_eq!(effectcraft_aep::write(&file).expect("write"), r.bytes);
    let _ = reimport(&r.bytes);
}
