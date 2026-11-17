//! A movie's Dolby Vision configuration is read into its footage (`Footage::dolby_vision`) and
//! shown with it, a stream without a base layer EffectCraft can show is refused at import, and
//! Dolby Vision video FilmCraft reads no codec in is reported. The movies are made here with
//! craft-media's writers; probing reads only their headers, so the frames are not a real stream.

use std::io::Cursor;
use std::path::PathBuf;

use craft_media::isobmff::{
    Brand, ColorInfo, DolbyVisionConfig, Edit, FourCc, FragmentedWriter, HevcConfig, Mp4Writer, SampleEntry, TrackConfig, WriteSample, WriterOptions,
};
use effectcraft_project::{DolbyVisionInfo, Footage, ItemId, ItemKind};
use effectcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

/// A 64×36 HEVC Main 10 entry of type `format` (BT.2020 HLG), with Dolby Vision configuration `dv`.
fn entry(format: &[u8; 4], dv: Option<DolbyVisionConfig>) -> SampleEntry {
    let hevc = HevcConfig {
        general_profile_idc: 2,
        general_level_idc: 93,
        chroma_format_idc: 1,
        bit_depth_luma: 10,
        bit_depth_chroma: 10,
        length_size: 4,
        ..Default::default()
    };
    let mut e = SampleEntry::hevc(hevc, 64, 36);
    e.format = FourCc(*format);
    if let Some(v) = e.video.as_mut() {
        v.color = Some(ColorInfo::Nclx { primaries: 9, transfer: 18, matrix: 9, full_range: false });
        v.dolby_vision = dv;
    }
    e
}

const FRAME: WriteSample<'static> = WriteSample { data: &[0, 0, 0, 2, 0x26, 0x01], duration: 1, composition_offset: 0, is_sync: true };

/// A two-frame 30 fps movie of [`entry`] ([`movie_of`]).
fn movie(format: &[u8; 4], dv: Option<DolbyVisionConfig>, fragmented: bool) -> Vec<u8> {
    movie_of(vec![track(format, dv, 30, &[1, 1])], fragmented)
}

/// A video track of [`entry`] at `timescale`, and its frames, one per duration in `durations`.
fn track(format: &[u8; 4], dv: Option<DolbyVisionConfig>, timescale: u32, durations: &[u32]) -> (TrackConfig, Vec<WriteSample<'static>>) {
    (TrackConfig::new(entry(format, dv), timescale), durations.iter().map(|&duration| WriteSample { duration, ..FRAME }).collect())
}

/// A movie of `tracks` (each a track and its frames), in that order: a MOV, or with `fragmented`
/// an fMP4, whose header lists no samples.
fn movie_of(tracks: Vec<(TrackConfig, Vec<WriteSample<'static>>)>, fragmented: bool) -> Vec<u8> {
    // Every track goes in before any frame (the fragmented muxer refuses a track after its first
    // fragment). The two muxers have the same calls but share no trait, hence a macro.
    macro_rules! mux {
        ($writer:expr) => {{
            let mut w = $writer;
            let (configs, frames): (Vec<_>, Vec<_>) = tracks.into_iter().unzip();
            let ids: Vec<_> = configs.into_iter().map(|c| w.add_track(c).expect("track")).collect();
            for (t, samples) in ids.into_iter().zip(frames) {
                for s in samples {
                    w.write_sample(t, s).expect("frame");
                }
            }
            w.finish().expect("finish").into_inner()
        }};
    }
    if fragmented {
        mux!(FragmentedWriter::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mp4)))
    } else {
        mux!(Mp4Writer::new(Cursor::new(Vec::new()), WriterOptions::new(Brand::Mov)).expect("writer"))
    }
}

fn dv(profile: u8, bl_present: bool, compatibility: u8) -> Option<DolbyVisionConfig> {
    Some(DolbyVisionConfig::new(profile, 4, true, false, bl_present, compatibility).expect("fits"))
}

/// A file in this run's folder. Tests run at once, so each uses names of its own.
fn path(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("effectcraft-host-dolby-vision-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir.join(name)
}

/// `bytes` probed both ways the app probes a movie: read in place from a file (with what the
/// import tells the user), and from memory (the browser).
fn probe_both(name: &str, bytes: &[u8]) -> [Result<(Footage, Option<String>), String>; 2] {
    let p = path(name);
    std::fs::write(&p, bytes).expect("write");
    let in_place = effectcraft_media::probe_noting(&p).map_err(|e| e.to_string());
    let in_memory = effectcraft_media::probe_bytes(&p.to_string_lossy(), bytes.into()).map(|f| (f, None)).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&p);
    [in_place, in_memory]
}

const IPHONE: DolbyVisionInfo = DolbyVisionInfo { profile: 8, level: 4, compatibility: 4 };

/// An iPhone recording's `hvc1` entry with Dolby Vision 8.4: FilmCraft decodes it as HEVC, the
/// footage records the configuration, and the import has nothing to warn about.
#[test]
fn hevc_with_dolby_vision_8_4_records_it() {
    for fragmented in [false, true] {
        for (i, r) in probe_both("hvc1.mov", &movie(b"hvc1", dv(8, true, 4), fragmented)).into_iter().enumerate() {
            let (f, warning) = r.unwrap_or_else(|e| panic!("probe {i} (fragmented {fragmented}): {e}"));
            assert_eq!(f.codec, "HEVC", "probe {i} (fragmented {fragmented})");
            assert_eq!(f.dolby_vision, Some(IPHONE), "probe {i} (fragmented {fragmented})");
            assert_eq!(f.dolby_vision.map(|d| d.label()).as_deref(), Some("Dolby Vision 8.4 (HLG base layer)"));
            assert_eq!(warning, None, "probe {i} (fragmented {fragmented})");
        }
    }
}

/// What is read of a movie's video (its Dolby Vision configuration, and the frame times and edit
/// list that correct its rate and length) comes from the track FilmCraft shows, the first it can
/// play. A video track before it without frames, a timescale or a sample description is passed
/// over. Here that track holds profile 5, which would be refused, and an edit list that would show
/// half of the 30 fps track shown; its frames, where it has any, average to 60 fps.
#[test]
fn what_is_read_comes_from_the_shown_track() {
    let passed_over = |durations: &[u32]| {
        let mut t = track(b"hvc1", dv(5, true, 0), 1000, durations);
        t.0.edits = vec![Edit { segment_duration: 100, media_time: 0, media_rate: 0x0001_0000 }];
        t
    };
    let shown = || track(b"hvc1", dv(8, true, 4), 30, &[1; 6]);
    let movie = |durations: &[u32]| movie_of(vec![passed_over(durations), shown()], false);
    // A 60 fps clip's frame times rounded to milliseconds.
    let sixty = [17, 16, 17, 17, 16, 17];
    // The first track's `kind` box with its field `offset` bytes from the start of the box type,
    // holding `was`, set to 0.
    let zeroed = |mut bytes: Vec<u8>, kind: &[u8; 4], offset: usize, was: u32| {
        let at = bytes.windows(4).position(|w| w == kind).expect("the box") + offset;
        assert_eq!(&bytes[at..at + 4], &was.to_be_bytes()[..], "{kind:?}");
        bytes[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
        bytes
    };
    let cases = [
        ("no-frames.mov", movie(&[])),
        // `mdhd` version 0: the timescale follows the version, flags and two times.
        ("no-timescale.mov", zeroed(movie(&sixty), b"mdhd", 16, 1000)),
        // `stsd`: the entry count follows the version and flags.
        ("no-description.mov", zeroed(movie(&sixty), b"stsd", 8, 1)),
    ];
    for (name, bytes) in cases {
        for (i, r) in probe_both(name, &bytes).into_iter().enumerate() {
            let (f, warning) = r.unwrap_or_else(|e| panic!("{name} probe {i}: {e}"));
            assert_eq!((f.codec.as_str(), f.dolby_vision, warning), ("HEVC", Some(IPHONE), None), "{name} probe {i}");
            assert_eq!((f.frame_rate, f.duration), (FrameRate::FPS_30, Tick::from_units(1, 5)), "{name} probe {i}");
        }
    }
}

/// `dvh1` / `dvhe` entries with Dolby Vision 8.4: the footage records the configuration, and while
/// FilmCraft reads no codec in them the import says the video can't be decoded yet.
#[test]
fn dolby_vision_entries_record_it_and_warn_while_filmcraft_reads_no_codec() {
    for format in [b"dvh1", b"dvhe"] {
        let name = String::from_utf8_lossy(format).to_string();
        let [in_place, in_memory] = probe_both(&format!("{name}.mov"), &movie(format, dv(8, true, 4), false));
        let (f, warning) = in_place.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(f.dolby_vision, Some(IPHONE), "{name}");
        let (g, _) = in_memory.unwrap_or_else(|e| panic!("{name} from memory: {e}"));
        assert_eq!(g.dolby_vision, Some(IPHONE), "{name} from memory");
        if f.codec_unread() {
            let warning = warning.unwrap_or_else(|| panic!("{name}: no warning"));
            assert!(warning.contains(&format!("Dolby Vision video in a {name} track can't be decoded by this build yet")), "{warning}");
        } else {
            // FilmCraft reads the entry as HEVC: the warning has gone with the reason for it.
            // Another name would mean a stream FilmCraft still can't decode, now without a warning.
            assert_eq!((f.codec.as_str(), warning), ("HEVC", None), "{name}");
        }
    }
}

/// Dolby Vision without a base layer EffectCraft can show (profile 5, compatibility ID 0; or no
/// base layer at all) fails to import with a message naming the profile, whatever entry type
/// holds it and however the movie is laid out.
#[test]
fn dolby_vision_profile_5_is_refused() {
    for (format, config) in [(b"dvh1", dv(5, true, 0)), (b"dvhe", dv(5, true, 0)), (b"hvc1", dv(5, true, 0)), (b"dvh1", dv(8, false, 4))] {
        for fragmented in [false, true] {
            let profile = config.as_ref().map_or(0, DolbyVisionConfig::profile);
            for (i, r) in probe_both("refused.mov", &movie(format, config.clone(), fragmented)).into_iter().enumerate() {
                let e = r.err().unwrap_or_else(|| panic!("{format:?} probe {i} (fragmented {fragmented}) imported"));
                assert!(e.contains(&format!("Dolby Vision profile {profile} has no HDR10, SDR or HLG base layer")), "{e}");
            }
        }
    }
}

/// A movie without a configuration, or with one craft-media can't read (a record cut short, so
/// the box ends early), imports as before: no Dolby Vision information and no warning.
#[test]
fn a_movie_without_a_readable_configuration_imports_as_before() {
    for (i, r) in probe_both("plain.mov", &movie(b"hvc1", None, false)).into_iter().enumerate() {
        let (f, warning) = r.unwrap_or_else(|e| panic!("probe {i}: {e}"));
        assert_eq!((f.codec.as_str(), f.dolby_vision, warning), ("HEVC", None, None), "probe {i}");
    }
    // Profile 5's `dvcC` box cut to a 4-byte record (it needs 5): the rest of the entry no longer
    // parses as boxes. Were the cut record read, the movie would be refused.
    let mut bytes = movie(b"hvc1", dv(5, true, 0), false);
    let at = bytes.windows(4).position(|w| w == b"dvcC").expect("a dvcC box") - 4;
    bytes[at..at + 4].copy_from_slice(&12u32.to_be_bytes());
    for (i, r) in probe_both("truncated.mov", &bytes).into_iter().enumerate() {
        let (f, warning) = r.unwrap_or_else(|e| panic!("probe {i}: {e}"));
        assert_eq!((f.dolby_vision, warning), (None, None), "probe {i}");
    }
}

/// `bytes` written to `name` and imported with File ▸ Import, into a new composition.
fn import(name: &str, bytes: &[u8]) -> (effectcraft_engine::Session, Value) {
    let p = path(name);
    std::fs::write(&p, bytes).expect("write");
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [p.to_string_lossy()], "newComp": true})).expect("import");
    let _ = std::fs::remove_file(&p);
    (s, r)
}

fn footage(s: &effectcraft_engine::Session, r: &Value) -> Footage {
    let id = ItemId(r["items"][0].as_u64().expect("an item"));
    match &s.project.item(id).expect("the item").kind {
        ItemKind::Footage(f) => f.clone(),
        k => panic!("not footage: {k:?}"),
    }
}

/// File ▸ Import of Dolby Vision profile 5 makes nothing and says why; of an iPhone recording's
/// Dolby Vision 8.4, the footage carries it, its metadata shows it, and the saved project keeps it.
#[test]
fn import_refuses_profile_5_and_keeps_8_4() {
    let (s, r) = import("import-p5.mov", &movie(b"dvh1", dv(5, true, 0), false));
    assert_eq!(r["items"], json!([]), "{r}");
    assert_eq!(r["comps"], json!([]), "{r}");
    let errors = r["errors"].to_string();
    assert!(errors.contains("import-p5.mov") && errors.contains("Dolby Vision profile 5 has no HDR10, SDR or HLG base layer"), "{r}");
    assert!(!s.project.items.values().any(|i| matches!(i.kind, ItemKind::Footage(_))));

    let (mut s, r) = import("import-iphone.mov", &movie(b"hvc1", dv(8, true, 4), false));
    assert_eq!((r["errors"].clone(), r["warnings"].clone()), (json!([]), json!([])), "{r}");
    assert_eq!(footage(&s, &r).dolby_vision, Some(IPHONE));
    let m = s.execute("item.metadata", json!({"item": r["items"][0]})).expect("metadata");
    assert_eq!(m["item"]["dolbyVision"], "Dolby Vision 8.4 (HLG base layer)", "{m}");
    let saved = path("iphone.ecproj");
    s.execute("file.saveAs", json!({"path": saved.to_string_lossy()})).expect("save");
    let mut back = effectcraft_host::session();
    back.execute("file.open", json!({"path": saved.to_string_lossy()})).expect("open");
    let _ = std::fs::remove_file(&saved);
    let f = back.project.items.values().find_map(|i| match &i.kind {
        ItemKind::Footage(f) => Some(f.clone()),
        _ => None,
    });
    assert_eq!(f.and_then(|f| f.dolby_vision), Some(IPHONE));
}

/// A `dvh1` movie imports with its Dolby Vision information, and while FilmCraft reads no codec in
/// it the import result warns that this build can't decode it yet.
#[test]
fn import_warns_while_dolby_vision_video_cannot_be_decoded() {
    let (s, r) = import("import-dvh1.mov", &movie(b"dvh1", dv(8, true, 4), false));
    assert_eq!(r["errors"], json!([]), "{r}");
    let f = footage(&s, &r);
    assert_eq!(f.dolby_vision, Some(IPHONE));
    let warnings = r["warnings"].to_string();
    if f.codec_unread() {
        assert!(warnings.contains("import-dvh1.mov: Dolby Vision video in a dvh1 track can't be decoded by this build yet"), "{r}");
    } else {
        assert_eq!((f.codec.as_str(), &r["warnings"]), ("HEVC", &json!([])), "{r}");
    }
}

/// File ▸ Reload Footage reads the configuration again: a file that gained one (re-exported with
/// Dolby Vision) shows it, and one that lost it no longer does.
#[test]
fn reload_footage_reads_the_configuration_again() {
    let p = path("reloaded.mov");
    std::fs::write(&p, movie(b"hvc1", None, false)).expect("write");
    let mut s = effectcraft_host::session();
    let r = s.execute_checked("file.import", json!({"paths": [p.to_string_lossy()]})).expect("import");
    assert_eq!(footage(&s, &r).dolby_vision, None);
    for (dv, want) in [(dv(8, true, 4), Some(IPHONE)), (None, None)] {
        std::fs::write(&p, movie(b"hvc1", dv, false)).expect("rewrite");
        s.execute_checked("file.reloadFootage", json!({"items": [r["items"][0]]})).expect("reload");
        assert_eq!(footage(&s, &r).dolby_vision, want);
    }
    let _ = std::fs::remove_file(&p);
}

/// The Media Browser's file information names the configuration of a movie not yet imported.
#[test]
fn the_media_browser_shows_the_configuration() {
    let p = path("browsed.mov");
    std::fs::write(&p, movie(b"hvc1", dv(8, true, 4), false)).expect("write");
    let mut s = effectcraft_host::session();
    let v = s.execute("mediaBrowser.fileInfo", json!({"path": p.to_string_lossy()})).expect("file info");
    let _ = std::fs::remove_file(&p);
    assert_eq!(v["footage"]["dolbyVision"], "Dolby Vision 8.4 (HLG base layer)", "{v}");
}
