//! Hostile values in a small committed project (`tests/fixtures/synthetic.aep`), so these cases
//! run without the external corpus.
//!
//! The project was made in After Effects 2026 with the working space set to None: one
//! composition `C` (64x48, 25 fps) with an image sequence layer (`shot_[0001-0003].png`, three
//! 8x8 PNGs) holding a Levels and a Glow effect, a solid layer and a folder. Levels' Clip To
//! Output Black has the expression `2`, Glow's A & B Midpoint the expression `0.25`. The only
//! change made to what After Effects wrote is that the one `Utf8` chunk holding an embedded
//! colour profile was replaced by `{}`.

use effectcraft_aep::import::{ImportOptions, ImportResult, LossCategory, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_project::{FootageKind, ItemKind, Project};

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

fn visit(chunks: &mut [Chunk<'_>], f: &mut dyn FnMut(&mut Chunk<'_>)) {
    for c in chunks {
        f(c);
        visit(c.children_mut(), f);
    }
}

/// The project with `edit` applied to every chunk.
fn mutate(edit: &mut dyn FnMut(&mut Chunk<'_>)) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    visit(&mut file.chunks, edit);
    write(&file).unwrap()
}

fn set_data(c: &mut Chunk<'_>, at: usize, bytes: &[u8]) {
    if let Body::Data(d) = &mut c.body {
        let d = d.to_mut();
        for (i, b) in bytes.iter().enumerate() {
            if let Some(slot) = d.get_mut(at + i) {
                *slot = *b;
            }
        }
    }
}

fn run(bytes: &[u8], allow_losses: bool) -> (Project, Result<ImportResult, effectcraft_aep::import::AepImportError>) {
    let mut project = Project::default();
    let none = |_: &str| false;
    let r = import(&mut project, bytes, &ImportOptions { allow_losses, ..ImportOptions::new(&none) });
    (project, r)
}

fn losses(r: &ImportResult, category: LossCategory) -> Vec<String> {
    r.report.losses.iter().filter(|l| l.category == category).flat_map(|l| l.details.clone()).collect()
}

#[test]
fn the_unchanged_project_imports_its_sequence() {
    let (p, r) = run(SYNTHETIC, true);
    r.unwrap();
    let seq = p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).expect("the sequence");
    assert_eq!((seq.kind, seq.sequence.len()), (FootageKind::Sequence, 3));
}

#[test]
fn a_sequence_with_long_names_is_not_expanded_past_the_byte_budget() {
    // 100 000 frames (within the frame limits) of 400-character names are 40 MB of strings.
    let bytes = mutate(&mut |c| {
        if c.id == *b"sspc" {
            set_data(c, 176, &100_000u32.to_be_bytes());
        }
        if c.id == *b"Utf8" && c.bytes() == Some(b"shot_") {
            *c = Chunk::data(*b"Utf8", vec![b'a'; 400]);
        }
    });
    let (p, r) = run(&bytes, true);
    let r = r.unwrap();
    let found = losses(&r, LossCategory::DamagedData);
    assert!(found.iter().any(|d| d.contains("longer than the import expands")), "{found:?}");
    let longest = p.items.values().filter_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.sequence.len()) } else { None }).max().unwrap_or(0);
    assert_eq!(longest, 0, "the sequence was not expanded");
}

/// The project with the sequence's naming (the chunks between `Als2` and `opti` in its `Pin `)
/// replaced by `naming`, and the `sspc` numbering (start, end, digits at 172, 176, 180) set to
/// `numbering` when given.
fn with_sequence_naming(naming: &[Chunk<'static>], numbering: Option<[u32; 3]>) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"Pin ") {
            return;
        }
        let Body::List { children, .. } = &mut c.body else { return };
        let Some(als) = children.iter().position(|k| k.is_list(b"Als2")) else { return };
        let Some(opti) = children.iter().position(|k| k.is(b"opti")) else { return };
        assert!(als < opti);
        let tail = children.split_off(opti);
        children.truncate(als + 1);
        children.extend(naming.iter().map(shorten));
        children.extend(tail);
        if let Some(numbering) = numbering {
            let sspc = children.iter_mut().find(|k| k.is(b"sspc")).unwrap();
            for (at, v) in [172, 176, 180].into_iter().zip(numbering) {
                set_data(sspc, at, &v.to_be_bytes());
            }
        }
        patched += 1;
    });
    assert_eq!(patched, 1, "the synthetic project has one sequence");
    write(&file).unwrap()
}

/// `bytes` with the first file name the sequence stores in its png `opti` (byte 58, NUL-terminated)
/// set to `name`.
fn with_first_file(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(bytes).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if c.is(b"opti") && c.bytes().is_some_and(|d| d.get(..4) == Some(&b"png!"[..])) {
            let mut z = name.as_bytes().to_vec();
            z.push(0);
            set_data(c, 58, &z);
            patched += 1;
        }
    });
    assert_eq!(patched, 1, "the synthetic project has one png item");
    write(&file).unwrap()
}

/// The imported sequence item's footage.
fn the_footage(p: &Project) -> effectcraft_project::Footage {
    p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some(f.clone()) } else { None }).expect("the sequence")
}

/// The file names (without the folder) of a footage's sequence.
fn file_names(f: &effectcraft_project::Footage) -> Vec<String> {
    f.sequence.iter().map(|s| s.rsplit(['\\', '/']).next().unwrap_or_default().to_string()).collect()
}

/// A copy of `c` that borrows for any lifetime (a chunk is covariant in its lifetime).
fn shorten<'a>(c: &Chunk<'static>) -> Chunk<'a> {
    c.clone()
}

/// A `Utf8` string chunk holding `s`.
fn utf8(s: &str) -> Chunk<'static> {
    Chunk::data(*b"Utf8", s.as_bytes().to_vec())
}

/// A sequence whose naming has only a suffix (no filename prefix) imports as a sequence of its files.
#[test]
fn a_sequence_without_a_filename_prefix_imports_its_files() {
    // `0001.png` .. `0003.png`: After Effects omits the empty prefix, so only `.png` follows the
    // alias. Before, `.png` was read as the prefix, the suffix was missing and the item became a
    // video linked to its folder.
    let (p, r) = run(&with_sequence_naming(&[utf8(".png")], None), true);
    r.unwrap();
    let f = the_footage(&p);
    assert_eq!(f.kind, FootageKind::Sequence);
    assert_eq!(file_names(&f), ["0001.png", "0002.png", "0003.png"]);
    assert!(f.path.ends_with("0001.png"), "{}", f.path);
}

/// A single `Utf8` naming part is read as the suffix or the prefix depending on how the stored first file name matches it.
#[test]
fn a_single_naming_part_is_placed_by_the_stored_first_file_name() {
    // `0001_v2.png` ..: the one `Utf8` is the suffix, as the first file name (`opti`) ends with it.
    let bytes = with_first_file(&with_sequence_naming(&[utf8("_v2.png")], None), "0001_v2.png");
    let (p, r) = run(&bytes, true);
    r.unwrap();
    let f = the_footage(&p);
    assert_eq!(f.kind, FootageKind::Sequence);
    assert_eq!(file_names(&f), ["0001_v2.png", "0002_v2.png", "0003_v2.png"]);
    // `shot_0001` ..: a name without an extension; the one `Utf8` is the prefix.
    let bytes = with_first_file(&with_sequence_naming(&[utf8("shot_")], None), "shot_0001");
    let (p, r) = run(&bytes, true);
    r.unwrap();
    assert_eq!(file_names(&the_footage(&p)), ["shot_0001", "shot_0002", "shot_0003"]);
    // Without that evidence (the stored first file is `shot_0001.png`), a part that is no extension
    // is not guessed: the item is reported.
    let bytes = with_sequence_naming(&[utf8("_v2.png")], None);
    reported_and_refused(&bytes, LossCategory::DamagedData, "image sequence naming not read, linked to its folder");
}

/// A file list containing a name with a separator, drive colon, dot directory or control character is refused and reported.
#[test]
fn a_file_list_with_a_name_that_is_not_a_plain_file_name_is_reported() {
    // The listed names are joined to the folder: a separator, a drive colon or a dot directory
    // would point outside it, and a NUL or another control character is no file name, so such a
    // list is not read.
    for hostile in ["..\\..\\x.png", "../x.png", "C:x.png", "..", ".", "x\0.png", "x\n.png"] {
        let list = Chunk::list(*b"StVc", vec![Chunk::data(*b"StVS", 2u32.to_le_bytes().to_vec()), utf8("a.png"), utf8(hostile)]);
        let bytes = with_sequence_naming(&[list], Some([u32::MAX, u32::MAX, 0]));
        reported_and_refused(&bytes, LossCategory::DamagedData, "image sequence naming not read, linked to its folder");
    }
}

/// A file list with a missing or too short count chunk is refused and reported.
#[test]
fn a_file_list_without_a_readable_count_is_reported() {
    // No `StVS`, or one too short for its `u32`: the list is not read rather than taken whole.
    for count in [None, Some(vec![2u8, 0])] {
        let mut kids: Vec<Chunk<'static>> = count.map(|c| Chunk::data(*b"StVS", c)).into_iter().collect();
        kids.extend([utf8("a.png"), utf8("b.png")]);
        let bytes = with_sequence_naming(&[Chunk::list(*b"StVc", kids)], Some([u32::MAX, u32::MAX, 0]));
        reported_and_refused(&bytes, LossCategory::DamagedData, "image sequence naming not read, linked to its folder");
    }
}

/// A force-alphabetical sequence imports its stored file list in the stored order.
#[test]
fn a_sequence_imported_with_force_alphabetical_order_imports_its_file_list() {
    // Force alphabetical order: `LIST StVc` with `StVS` (the count, little-endian) and one `Utf8`
    // per file instead of the prefix and suffix; the numbering in `sspc` is not set. Before, the
    // item became a video linked to its folder.
    let list = Chunk::list(*b"StVc", vec![Chunk::data(*b"StVS", 3u32.to_le_bytes().to_vec()), utf8("b_take.png"), utf8("a_take.png"), utf8("c_take.png")]);
    let (p, r) = run(&with_sequence_naming(&[list], Some([u32::MAX, u32::MAX, 0])), true);
    let r = r.unwrap();
    let f = the_footage(&p);
    assert_eq!(f.kind, FootageKind::Sequence);
    // The stored order is kept.
    assert_eq!(file_names(&f), ["b_take.png", "a_take.png", "c_take.png"]);
    assert!(f.path.ends_with("b_take.png"), "{}", f.path);
    assert!(losses(&r, LossCategory::DamagedData).is_empty(), "{:?}", r.report.losses);
    // The count bounds the list.
    let list = Chunk::list(*b"StVc", vec![Chunk::data(*b"StVS", 2u32.to_le_bytes().to_vec()), utf8("x.png"), utf8("y.png"), utf8("z.png")]);
    let (p, r) = run(&with_sequence_naming(&[list], Some([u32::MAX, u32::MAX, 0])), true);
    r.unwrap();
    assert_eq!(file_names(&the_footage(&p)), ["x.png", "y.png"]);
}

/// A sequence with no naming or an empty file list is reported and linked to its folder.
#[test]
fn a_sequence_whose_naming_cannot_be_read_is_reported() {
    // No naming at all after the alias. Before, the item silently linked its folder.
    let bytes = with_sequence_naming(&[], None);
    reported_and_refused(&bytes, LossCategory::DamagedData, "image sequence naming not read, linked to its folder");
    // An empty file list is no naming either.
    let empty = Chunk::list(*b"StVc", vec![Chunk::data(*b"StVS", 0u32.to_le_bytes().to_vec())]);
    let bytes = with_sequence_naming(&[empty], Some([u32::MAX, u32::MAX, 0]));
    reported_and_refused(&bytes, LossCategory::DamagedData, "image sequence naming not read, linked to its folder");
}

#[test]
fn a_solid_with_a_nan_channel_is_repaired_and_reported() {
    let nan = f32::NAN.to_be_bytes();
    let bytes = mutate(&mut |c| {
        // The solid's colour: ARGB as four f32 from byte 10 of `opti`.
        if c.id == *b"opti" && c.bytes().is_some_and(|d| d.len() > 22 && d.get(..4) == Some(b"Soli")) {
            set_data(c, 14, &nan);
        }
    });
    let (p, r) = run(&bytes, true);
    let r = r.unwrap();
    for item in p.items.values() {
        if let ItemKind::Solid(s) = &item.kind {
            assert!(s.color.iter().all(|c| c.is_finite()), "{:?}", s.color);
        }
    }
    assert!(losses(&r, LossCategory::DamagedData).iter().any(|d| d.contains("not a number")), "{:?}", r.report.losses);
    // Strict import refuses and leaves the project alone.
    let (p, r) = run(&bytes, false);
    assert!(r.is_err());
    assert_eq!(p.items.values().count(), 0);
}

#[test]
fn expressions_on_converted_effect_parameters_are_reported_not_imported() {
    let (p, r) = run(SYNTHETIC, true);
    let r = r.unwrap();
    let found = losses(&r, LossCategory::Effects);
    assert!(found.iter().any(|d| d.contains("expression") && d.contains("Clip To Output Black")), "{found:?}");
    assert!(found.iter().any(|d| d.contains("expression") && d.contains("Midpoint")), "{found:?}");
    for item in p.items.values() {
        if let ItemKind::Comp(c) = &item.kind {
            for layer in &c.layers {
                if let Some(fx) = layer.effects() {
                    fx.walk("", &mut |path, prop| assert!(prop.expr.is_none(), "an expression on the converted parameter {path}"));
                }
            }
        }
    }
}

/// Imports `bytes` with and without allowing losses: the lenient import reports `needle` under
/// `category`, the strict one refuses and leaves the project empty.
fn reported_and_refused(bytes: &[u8], category: LossCategory, needle: &str) {
    let (_, r) = run(bytes, true);
    let r = r.unwrap();
    let found = losses(&r, category);
    assert!(found.iter().any(|d| d.contains(needle)), "{needle:?} not in {:?}", r.report.losses);
    let (p, r) = run(bytes, false);
    assert!(r.is_err(), "strict import must refuse");
    assert!(p.items.is_empty(), "a refused import leaves the project alone");
}

#[test]
fn the_unchanged_project_has_no_damage_reports() {
    let (_, r) = run(SYNTHETIC, true);
    let r = r.unwrap();
    for c in [LossCategory::DamagedData, LossCategory::TextLayers, LossCategory::ProjectSettings] {
        assert!(losses(&r, c).is_empty(), "{c:?}: {:?}", r.report.losses);
    }
}

#[test]
fn a_folder_without_its_contents_list_is_reported() {
    // After Effects writes an empty `Sfdr` for an empty folder; removing it damages the folder.
    let mut removed = 0;
    let bytes = {
        let mut file = parse(SYNTHETIC).unwrap();
        fn drop_empty_sfdr(chunks: &mut Vec<Chunk<'_>>, n: &mut usize) {
            chunks.retain(|c| {
                let empty = c.is_list(b"Sfdr") && c.children().is_empty();
                *n += usize::from(empty);
                !empty
            });
            for c in chunks.iter_mut() {
                if let Body::List { children, .. } | Body::Chunks(children) = &mut c.body {
                    drop_empty_sfdr(children, n);
                }
            }
        }
        drop_empty_sfdr(&mut file.chunks, &mut removed);
        write(&file).unwrap()
    };
    assert_eq!(removed, 1, "the synthetic project has one empty folder");
    reported_and_refused(&bytes, LossCategory::DamagedData, "contents list is missing");
}

#[test]
fn invalid_timing_is_reported() {
    // The work area ends before it starts (cdta bytes 28 and 36).
    let bytes = mutate(&mut |c| {
        if c.id == *b"cdta" {
            set_data(c, 28, &10u32.to_be_bytes());
            set_data(c, 36, &5u32.to_be_bytes());
        }
    });
    reported_and_refused(&bytes, LossCategory::DamagedData, "work area");
    // A pixel aspect ratio with a zero denominator.
    let bytes = mutate(&mut |c| {
        if c.id == *b"cdta" {
            set_data(c, 148, &0u32.to_be_bytes());
        }
    });
    reported_and_refused(&bytes, LossCategory::DamagedData, "pixel aspect");
    // A layer with a zero time stretch (ldta bytes 8 and 108).
    let bytes = mutate(&mut |c| {
        if c.id == *b"ldta" {
            set_data(c, 8, &0u32.to_be_bytes());
        }
    });
    reported_and_refused(&bytes, LossCategory::DamagedData, "time stretch");
}

#[test]
fn an_unknown_colour_depth_code_is_reported() {
    let bytes = mutate(&mut |c| {
        if c.id == *b"nhed" {
            set_data(c, 15, &[7]);
        }
    });
    let (_, r) = run(&bytes, true);
    let r = r.unwrap();
    assert!(losses(&r, LossCategory::ProjectSettings).iter().any(|d| d.contains("colour depth code 7")), "{:?}", r.report.losses);
}

/// The project with every `LIST` of type `list_type` made unreadable: its content replaced by three
/// bytes that are no chunk sequence, so the reader keeps it as an opaque list. Returns the bytes and
/// how many lists were damaged.
fn damage_lists(list_type: &[u8; 4], only_empty: bool) -> (Vec<u8>, usize) {
    fn walk(chunks: &mut [Chunk<'_>], list_type: &[u8; 4], only_empty: bool, n: &mut usize) {
        for c in chunks.iter_mut() {
            if c.is_list(list_type) && (!only_empty || c.children().is_empty()) {
                c.body = Body::OpaqueList { list_type: *list_type, data: vec![0xff; 3].into() };
                *n += 1;
            } else {
                walk(c.children_mut(), list_type, only_empty, n);
            }
        }
    }
    let mut file = parse(SYNTHETIC).unwrap();
    let mut n = 0;
    walk(&mut file.chunks, list_type, only_empty, &mut n);
    let bytes = write(&file).unwrap();
    // The damage survives a round trip through the reader.
    let reread = parse(&bytes).unwrap();
    let mut opaque = 0;
    fn count(chunks: &[Chunk<'_>], list_type: &[u8; 4], n: &mut usize) {
        for c in chunks {
            *n += usize::from(c.is_list(list_type) && c.is_opaque_list());
            count(c.children(), list_type, n);
        }
    }
    count(&reread.chunks, list_type, &mut opaque);
    assert_eq!(opaque, n, "the damaged lists read back as opaque");
    (bytes, n)
}

#[test]
fn a_damaged_project_item_list_is_reported() {
    // Before, the import gave an empty project folder and no loss.
    let (bytes, n) = damage_lists(b"Fold", false);
    assert_eq!(n, 1);
    reported_and_refused(&bytes, LossCategory::DamagedData, "item list is damaged");
}

#[test]
fn a_damaged_folder_contents_list_is_reported() {
    let (bytes, n) = damage_lists(b"Sfdr", true);
    assert_eq!(n, 1, "the synthetic project has one empty folder");
    reported_and_refused(&bytes, LossCategory::DamagedData, "contents list is damaged");
}

/// The project with the `ldta` time at byte `at` (numerator, then denominator) set to `num/den`
/// in every layer.
fn layer_time(at: usize, num: i32, den: u32) -> Vec<u8> {
    mutate(&mut |c| {
        if c.id == *b"ldta" {
            set_data(c, at, &num.to_be_bytes());
            set_data(c, at + 4, &den.to_be_bytes());
        }
    })
}

#[test]
fn layer_times_with_a_zero_denominator_are_reported() {
    // ldta bytes 12 (start), 20 (in point) and 28 (out point), each a numerator and a
    // denominator. Before, a zero denominator silently became 0 s.
    for (at, what) in [(12, "start"), (20, "in point"), (28, "out point")] {
        let bytes = layer_time(at, 2, 0);
        reported_and_refused(&bytes, LossCategory::DamagedData, &format!("stored {what} time is unreadable"));
    }
    // A valid time with another denominator is no damage.
    let (_, r) = run(&layer_time(12, 4, 2), true);
    assert!(losses(&r.unwrap(), LossCategory::DamagedData).is_empty());
}

/// The time stretch of every imported layer.
fn stretches(p: &Project) -> Vec<f64> {
    p.items.values().filter_map(|i| if let ItemKind::Comp(c) = &i.kind { Some(c) } else { None }).flat_map(|c| c.layers.iter().map(|l| l.stretch)).collect()
}

#[test]
fn a_time_stretch_outside_one_to_9900_percent_is_clamped_and_reported() {
    // ldta byte 8 is the stretch numerator (i32), byte 108 the denominator (u32). 1/(2^31 - 1) is
    // about 5e-8 %: imported as stored, the layer and audio time divided by ~0 (#3247).
    for (num, den, want) in [(1i32, i32::MAX as u32, 1.0), (-1, i32::MAX as u32, -1.0), (i32::MAX, 1, 9900.0), (i32::MIN, 1, -9900.0)] {
        let bytes = mutate(&mut |c| {
            if c.id == *b"ldta" {
                set_data(c, 8, &num.to_be_bytes());
                set_data(c, 108, &den.to_be_bytes());
            }
        });
        let (p, r) = run(&bytes, true);
        let r = r.unwrap();
        let found = losses(&r, LossCategory::DamagedData);
        assert!(found.iter().any(|d| d.contains("time stretch") && d.contains("outside")), "{num}/{den}: {found:?}");
        let s = stretches(&p);
        assert!(!s.is_empty() && s.iter().all(|v| *v == want), "{num}/{den}: {s:?}");
        let (p, r) = run(&bytes, false);
        assert!(r.is_err(), "{num}/{den}: strict import must refuse");
        assert!(p.items.is_empty());
    }
    // A small stretch within the range (1/64: 1.5625 %) and -9900 % itself are no damage.
    for (num, den, want) in [(1i32, 64u32, 1.5625), (-99, 1, -9900.0)] {
        let bytes = mutate(&mut |c| {
            if c.id == *b"ldta" {
                set_data(c, 8, &num.to_be_bytes());
                set_data(c, 108, &den.to_be_bytes());
            }
        });
        let (p, r) = run(&bytes, true);
        let r = r.unwrap();
        assert!(!losses(&r, LossCategory::DamagedData).iter().any(|d| d.contains("time stretch")), "{num}/{den}: {:?}", r.report.losses);
        assert!(stretches(&p).iter().all(|v| *v == want), "{num}/{den}: {:?}", stretches(&p));
    }
}

/// The project with `nhed` bytes from `at` replaced.
fn header(at: usize, bytes: &[u8]) -> Vec<u8> {
    mutate(&mut |c| {
        if c.id == *b"nhed" {
            set_data(c, at, bytes);
        }
    })
}

#[test]
fn unknown_time_display_and_frame_start_codes_are_reported() {
    // nhed byte 8: 0 timecode, 1 frames or feet + frames; byte 11 = 1 feet + frames, byte 13 the
    // frames per foot (16 or 40); byte 14 the frame count start (2 from 0, 1 from 1).
    reported_and_refused(&header(8, &[5]), LossCategory::ProjectSettings, "time display code 5/");
    reported_and_refused(&header(8, &[1, 0, 0, 1, 0, 24]), LossCategory::ProjectSettings, "24 frames per foot");
    reported_and_refused(&header(14, &[9]), LossCategory::ProjectSettings, "frame count start code 9");
    // The known codes are no loss.
    for (at, bytes) in [(8, &[1u8, 0, 0, 0][..]), (8, &[1, 0, 0, 1, 0, 16]), (8, &[1, 0, 0, 1, 0, 40]), (14, &[1])] {
        let (_, r) = run(&header(at, bytes), true);
        assert!(losses(&r.unwrap(), LossCategory::ProjectSettings).is_empty(), "nhed {at}: {bytes:?}");
    }
}

/// The project with the value records named `param` (`<effect>-NNNN`: the instance's and the one in
/// the project's copy of the effect, `EfdG`) set to state `tdsb` and holding `value` (the `cdat`
/// starts with the big-endian `f64`).
fn with_record(param: &str, tdsb: u32, value: f64) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    let mut patched = 0;
    visit(&mut file.chunks, &mut |c| {
        if !c.is_list(b"tdgp") {
            return;
        }
        let kids = c.children_mut();
        let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(param.as_bytes()))) else {
            return;
        };
        let Some(record) = kids.get_mut(at + 1).filter(|k| k.is_list(b"tdbs")) else { return };
        for k in record.children_mut() {
            if k.is(b"tdsb") {
                set_data(k, 0, &tdsb.to_be_bytes());
            } else if k.is(b"cdat") {
                set_data(k, 0, &value.to_be_bytes());
            }
        }
        patched += 1;
    });
    assert_eq!(patched, 2, "{param}: the instance and the EfdG copy");
    write(&file).unwrap()
}

/// The values of the imported effect parameters, by property id, in file order.
fn effect_values(p: &Project) -> Vec<(String, effectcraft_keyframe::Value)> {
    let mut out = Vec::new();
    for item in p.items.values() {
        if let ItemKind::Comp(c) = &item.kind {
            for layer in &c.layers {
                if let Some(fx) = layer.effects() {
                    fx.walk("", &mut |_, prop| out.push((prop.match_id.clone(), prop.value.clone())));
                }
            }
        }
    }
    out
}

/// A default-state record (`tdsb` 3) reads like a written one, except through `stored_value`.
#[test]
fn a_default_state_effect_record_holds_the_value() {
    // After Effects writes the values of some parameters as records in the default state (`tdsb`
    // 3): Lumetri Color's hidden controls (1200) and Cryptomatte's Selection Mode (1400), each
    // read back as recorded. The reader reads them like a written record (`tdsb` 1).
    let bytes = with_record("ADBE Glo2-0011", 3, 0.75);
    let file = parse(&bytes).unwrap();
    let project = effectcraft_aep::Project::new(&file).unwrap();
    let defs = project.effect_definition_index();
    let mut seen = 0;
    for item in project.all_items() {
        let Some(comp) = item.comp() else { continue };
        for layer in comp.layers() {
            for fx in layer.effects().iter().filter(|fx| fx.match_name() == "ADBE Glo2") {
                let params = defs.params(fx);
                let p = params.iter().find(|p| p.match_name == "ADBE Glo2-0011").unwrap();
                assert_eq!(p.stored().and_then(|s| s.flags()), Some(3));
                // `stored_value` keeps reading written records only.
                assert_eq!(p.stored_value(), None);
                assert_eq!(p.recorded_value(), Some(effectcraft_aep::ParamValue::Number(0.75)));
                assert_eq!(p.value(), Some(effectcraft_aep::ParamValue::Number(0.75)));
                assert_eq!(p.is_default(), Some(false));
                seen += 1;
            }
        }
    }
    assert_eq!(seen, 1);
}

/// Default-state records import with their values and no loss, like written records.
#[test]
fn default_state_effect_records_import_like_written_ones() {
    // Before, such a record was reported as an unreadable value and the default used.
    for (param, value) in [("ADBE Glo2-0011", 0.75), ("ADBE Easy Levels2-0008", 1.0)] {
        let (written, r1) = run(&with_record(param, 1, value), true);
        let (default_state, r3) = run(&with_record(param, 3, value), true);
        let (r1, r3) = (r1.unwrap(), r3.unwrap());
        assert!(losses(&r3, LossCategory::DamagedData).is_empty(), "{param}: {:?}", r3.report.losses);
        assert_eq!(r3.report.losses, r1.report.losses, "{param}");
        assert_eq!(effect_values(&default_state), effect_values(&written), "{param}");
    }
    // Glow's A & B Midpoint 0.75 is 75 % (the default is 50).
    let (p, _) = run(&with_record("ADBE Glo2-0011", 3, 0.75), true);
    let mid: Vec<_> = effect_values(&p).into_iter().filter(|(id, _)| id == "abMidpoint").collect();
    assert_eq!(mid, vec![("abMidpoint".to_string(), effectcraft_keyframe::Value::Scalar(75.0))]);
    // Levels' Clip To Output Black (`-0008`) recorded as After Effects' item 1, On: EffectCraft's
    // On is item 1 of Off, On, Off for 32 bpc Color (the default is item 2).
    let (p, _) = run(&with_record("ADBE Easy Levels2-0008", 3, 1.0), true);
    let clip: Vec<_> = effect_values(&p).into_iter().filter(|(id, _)| id == "clipToOutputBlack").collect();
    assert_eq!(clip, vec![("clipToOutputBlack".to_string(), effectcraft_keyframe::Value::Enum(1))]);
}

/// Sets the `cdat` of the value record `param` in the value group `tdgp` (the first `f64`), and
/// its state to `tdsb` when given. True when the group has that record.
fn patch_record(tdgp: &mut Chunk<'_>, param: &str, tdsb: Option<u32>, value: f64) -> bool {
    let kids = tdgp.children_mut();
    let Some(at) = kids.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(param.as_bytes()))) else {
        return false;
    };
    let Some(record) = kids.get_mut(at + 1).filter(|k| k.is_list(b"tdbs")) else { return false };
    for k in record.children_mut() {
        if k.is(b"tdsb")
            && let Some(state) = tdsb
        {
            set_data(k, 0, &state.to_be_bytes());
        } else if k.is(b"cdat") {
            set_data(k, 0, &value.to_be_bytes());
        }
    }
    true
}

/// The project without its project-level effect definitions (`EfdG`; some files have none, seen
/// in the projects inside After Effects-made Motion Graphics templates) and with a second Glow
/// just above the first on the layer: a copy of it whose `parT` is empty, as such files write
/// every instance of a type but one. `edit_copy` is applied to every chunk of the copy; with
/// `empty_original` the original's `parT` is emptied too.
fn second_glow_without_definitions(edit_copy: &mut dyn FnMut(&mut Chunk<'_>), empty_original: bool) -> Vec<u8> {
    fn empty_part(fx: &mut Chunk<'_>) {
        for k in fx.children_mut() {
            if let Body::List { list_type, children, .. } = &mut k.body
                && list_type == b"parT"
            {
                children.clear();
            }
        }
    }
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    let mut added = 0;
    visit(&mut file.chunks, &mut |c| {
        if let Body::List { list_type, children, .. } = &mut c.body {
            if list_type == b"EfdG" {
                *list_type = *b"XfdG";
                return;
            }
            if list_type != b"tdgp" {
                return;
            }
            let Some(at) = children.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(b"ADBE Glo2"))) else {
                return;
            };
            let (Some(name), Some(mut fx)) = (children.get(at).cloned(), children.get(at + 1).filter(|k| k.is_list(b"sspc")).cloned()) else { return };
            empty_part(&mut fx);
            edit_copy(&mut fx);
            visit(fx.children_mut(), edit_copy);
            if empty_original && let Some(original) = children.get_mut(at + 1) {
                empty_part(original);
            }
            children.insert(at, fx);
            children.insert(at, name);
            added += 1;
        }
    });
    assert_eq!(added, 1, "one Glow on the layer");
    write(&file).unwrap()
}

/// The imported Glows in stack order, each as its property values by property id.
fn glow_values(p: &Project) -> Vec<Vec<(String, effectcraft_keyframe::Value)>> {
    let glow = effectcraft_project::GroupKind::Effect { effect: "ec.stylize.glow".to_string() };
    let mut glows = Vec::new();
    for item in p.items.values() {
        if let ItemKind::Comp(c) = &item.kind {
            for layer in &c.layers {
                for node in layer.effects().map(|fx| fx.children.as_slice()).unwrap_or_default() {
                    if let effectcraft_project::Node::Group(g) = node
                        && g.kind == glow
                    {
                        let mut values = Vec::new();
                        g.walk("", &mut |_, prop| values.push((prop.match_id.clone(), prop.value.clone())));
                        glows.push(values);
                    }
                }
            }
        }
    }
    glows
}

/// The value of property `id` in `values`.
fn value_of<'v>(values: Option<&'v Vec<(String, effectcraft_keyframe::Value)>>, id: &str) -> Option<&'v effectcraft_keyframe::Value> {
    values?.iter().find(|(i, _)| i == id).map(|(_, v)| v)
}

/// Every instance of an effect type but one has an empty `parT`; without a project-level copy the
/// others borrow the definitions of that one (#3170: 129 Color Controls of a template were
/// reported "without readable parameter definitions" and dropped).
#[test]
fn an_instance_without_definitions_imports_with_those_of_another_instance() {
    // Both Glows import; before, the copy was reported and not imported.
    let (p, r) = run(&second_glow_without_definitions(&mut |_| {}, false), true);
    let r = r.unwrap();
    let found = losses(&r, LossCategory::Effects);
    assert!(!found.iter().any(|d| d.contains("without readable parameter definitions") || d.contains("no parameter definition")), "{found:?}");
    assert_eq!(r.report.imported.effects, 3, "Levels and both Glows: {:?}", r.report.losses);
    let glows = glow_values(&p);
    assert_eq!(glows.len(), 2);
    assert_eq!(glows.first(), glows.get(1), "the copy has the same records, so the same values");

    // The values come from each instance's own records: the copy's A & B Midpoint (`-0011`) set
    // to 0.75 arrives as 75 % in the copy only; the Glow it borrows from keeps its own.
    let mut patched = 0;
    let bytes = second_glow_without_definitions(
        &mut |c| {
            if c.is_list(b"tdgp") && patch_record(c, "ADBE Glo2-0011", None, 0.75) {
                patched += 1;
            }
        },
        false,
    );
    assert_eq!(patched, 1, "the copy's record");
    let (p, r) = run(&bytes, true);
    r.unwrap();
    let glows = glow_values(&p);
    let seventy_five = effectcraft_keyframe::Value::Scalar(75.0);
    assert_eq!(value_of(glows.first(), "abMidpoint"), Some(&seventy_five));
    let own = value_of(glows.get(1), "abMidpoint");
    assert!(own.is_some() && own != Some(&seventy_five), "{own:?}");
}

/// A value record that the borrowed definitions do not name is reported, not dropped silently.
#[test]
fn a_value_record_the_borrowed_definitions_do_not_name_is_reported() {
    let bytes = second_glow_without_definitions(
        &mut |c| {
            if c.is(b"tdmn") && c.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(b"ADBE Glo2-0011")) {
                set_data(c, 0, b"ADBE Glo2-0099\0");
            }
        },
        false,
    );
    let (_, r) = run(&bytes, true);
    let r = r.unwrap();
    let found = losses(&r, LossCategory::Effects);
    assert!(found.iter().any(|d| d.contains("ADBE Glo2-0099") && d.contains("no parameter definition")), "{found:?}");
    assert_eq!(r.report.imported.effects, 3, "{:?}", r.report.losses);
}

/// When no instance of the type keeps its definitions, every instance is reported.
#[test]
fn instances_without_any_definitions_are_reported() {
    let (p, r) = run(&second_glow_without_definitions(&mut |_| {}, true), true);
    let r = r.unwrap();
    let found = losses(&r, LossCategory::Effects);
    assert_eq!(found.iter().filter(|d| d.contains("ADBE Glo2") && d.contains("without readable parameter definitions, not imported")).count(), 2, "{found:?}");
    assert_eq!(r.report.imported.effects, 1, "Levels only: {:?}", r.report.losses);
    assert!(glow_values(&p).is_empty());
}

/// The reader's side: the copy's definitions come from the other instance.
#[test]
fn an_instance_without_definitions_reads_those_of_another_instance() {
    let bytes = second_glow_without_definitions(&mut |_| {}, false);
    let file = parse(&bytes).unwrap();
    let project = effectcraft_aep::Project::new(&file).unwrap();
    let defs = project.effect_definition_index();
    let mut sources = Vec::new();
    for item in project.all_items() {
        let Some(comp) = item.comp() else { continue };
        for layer in comp.layers() {
            for fx in layer.effects().iter().filter(|fx| fx.match_name() == "ADBE Glo2") {
                sources.push(defs.source(fx));
                let params = defs.params(fx);
                assert_eq!(params.len(), 16, "Glow's parameters");
                assert_eq!(params.iter().all(|p| p.borrowed_definition), defs.source(fx) == effectcraft_aep::DefinitionSource::OtherInstance);
            }
        }
    }
    // The copy is above the instance it borrows from (earlier in the stack).
    assert_eq!(sources, [effectcraft_aep::DefinitionSource::OtherInstance, effectcraft_aep::DefinitionSource::Instance]);
}

/// A record in a state other than 1 or 3 is not read and is reported.
#[test]
fn an_effect_record_in_another_state_is_reported_as_damaged() {
    // Only the written (1) and the default (3) state hold a value; any other state is not read.
    for tdsb in [0, 2, 5] {
        let (_, r) = run(&with_record("ADBE Glo2-0011", tdsb, 0.75), true);
        let r = r.unwrap();
        let damaged = losses(&r, LossCategory::DamagedData);
        assert!(damaged.iter().any(|l| l.contains("unreadable value")), "tdsb {tdsb}: {:?}", r.report.losses);
    }
}

/// The project with a value record for Glow's Color A (`ADBE Glo2-0012`, which the project does
/// not store) in state `tdsb` holding the ARGB doubles `argb` (0..255): a copy of the A & B
/// Midpoint record (`-0011`) made a 4-component colour without its expression.
fn with_color_a(tdsb: u32, argb: [f64; 4]) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    let mut added = 0;
    visit(&mut file.chunks, &mut |c| {
        let Body::List { list_type, children, .. } = &mut c.body else { return };
        if list_type != b"tdgp" {
            return;
        }
        let Some(at) = children.iter().position(|k| k.is(b"tdmn") && k.bytes().is_some_and(|b| b.split(|x| *x == 0).next() == Some(b"ADBE Glo2-0011"))) else {
            return;
        };
        let (Some(mut name), Some(mut record)) = (children.get(at).cloned(), children.get(at + 1).filter(|k| k.is_list(b"tdbs")).cloned()) else { return };
        set_data(&mut name, 0, b"ADBE Glo2-0012\0");
        if let Body::List { children: kids, .. } = &mut record.body {
            kids.retain(|k| !k.is(b"Utf8"));
            for k in kids.iter_mut() {
                if k.is(b"tdsb") {
                    set_data(k, 0, &tdsb.to_be_bytes());
                } else if k.is(b"tdb4") {
                    // 4 components; no expression (bytes 76, 77 and 120).
                    set_data(k, 3, &[4]);
                    for byte in [76, 77, 120] {
                        set_data(k, byte, &[0]);
                    }
                } else if k.is(b"cdat") {
                    *k = Chunk::data(*b"cdat", argb.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<u8>>());
                }
            }
        }
        children.insert(at + 2, record);
        children.insert(at + 2, name);
        added += 1;
    });
    assert_eq!(added, 2, "the instance and the EfdG copy");
    write(&file).unwrap()
}

/// A colour recorded in the default state (`tdsb` 3) reads like a written one (#3170's "parameter
/// Color ... holds an unreadable value" on 0.21.1, which read only written records).
#[test]
fn a_default_state_colour_record_holds_the_value() {
    let argb = [255.0, 51.0, 153.0, 255.0];
    let (written, r1) = run(&with_color_a(1, argb), true);
    let (default_state, r3) = run(&with_color_a(3, argb), true);
    let (r1, r3) = (r1.unwrap(), r3.unwrap());
    assert!(losses(&r3, LossCategory::DamagedData).is_empty(), "{:?}", r3.report.losses);
    assert_eq!(r3.report.losses, r1.report.losses);
    let colour = |p: &Project| value_of(glow_values(p).first(), "colorA").cloned();
    let (unchanged, _) = run(SYNTHETIC, true);
    assert!(colour(&written).is_some());
    assert_eq!(colour(&default_state), colour(&written));
    assert_ne!(colour(&default_state), colour(&unchanged), "the recorded colour, not the default");
}

/// The project with its png item made one layer of a Photoshop file `art.psd`: the `opti` of
/// docs/aep-format.md 3.3 (tag `8BPS`, the layer index at 14, a 1000 x 1000 document at 32 and 36,
/// its bounds at 78-94, its name at 344) and the item `size` big.
fn as_psd_layer(index: i32, bounds: [i32; 4], name: &str, size: (u32, u32)) -> Vec<u8> {
    let mut opti = vec![0u8; 602];
    opti[..4].copy_from_slice(b"8BPS");
    opti[14..18].copy_from_slice(&index.to_be_bytes());
    opti[32..36].copy_from_slice(&1000u32.to_le_bytes());
    opti[36..40].copy_from_slice(&1000u32.to_le_bytes());
    for (i, v) in bounds.iter().enumerate() {
        opti[78 + 4 * i..82 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    opti[344..344 + name.len()].copy_from_slice(name.as_bytes());
    let alias = br#"{"fullpath":"C:\\x\\art.psd","platform":1,"target_is_folder":false}"#.to_vec();
    mutate(&mut |c| {
        if !c.is_list(b"Pin ") {
            return;
        }
        let Body::List { children, .. } = &mut c.body else { return };
        if !children.iter().any(|k| k.is(b"opti") && k.bytes().is_some_and(|d| d.get(..4) == Some(&b"png!"[..]))) {
            return;
        }
        // No sequence naming between the alias and the `opti`.
        children.retain(|k| k.is(b"sspc") || k.is(b"opti") || k.is_list(b"Als2"));
        for k in children.iter_mut() {
            if k.is(b"opti") {
                k.body = Body::Data(opti.clone().into());
            } else if k.is(b"sspc") {
                set_data(k, 30, &size.0.to_be_bytes());
                set_data(k, 34, &size.1.to_be_bytes());
            }
        }
        visit(children, &mut |k| {
            if k.is(b"alas") {
                k.body = Body::Data(alias.clone().into());
            }
        });
    })
}

/// #3087: an item made from one layer of a Photoshop file shows that layer, named
/// `<layer>/<file>`; it showed the whole document, named after the file. Item as big as the layer:
/// Retain Layer Sizes; as big as the document: the layer on the document.
#[test]
fn a_photoshop_layer_item_shows_its_layer() {
    for (size, layer_size) in [((48, 15), true), ((1000, 1000), false)] {
        let (p, r) = run(&as_psd_layer(7, [749, 800, 764, 848], "Tëst", size), true);
        r.unwrap();
        let (name, f) = p.items.values().find_map(|i| if let ItemKind::Footage(f) = &i.kind { Some((i.name.clone(), f.clone())) } else { None }).unwrap();
        assert_eq!(name, "Tëst/art.psd");
        let l = f.layer.expect("the layer");
        assert_eq!((l.index, l.name.as_str(), l.layer_size, (f.width, f.height)), (7, "Tëst", layer_size, size));
    }
}

/// A layer as big as the document but off its corner fits both framings: shown at its own bounds
/// and reported. One at the corner, or bigger than the document, reads one way and is not. An item
/// as big as neither is shown on the document and reported.
#[test]
fn a_photoshop_layer_as_big_as_the_document_off_its_corner_is_reported() {
    let framing = |bounds, size| {
        let (p, r) = run(&as_psd_layer(3, bounds, "Moved", size), true);
        let found = losses(&r.unwrap(), LossCategory::UnsupportedItems);
        let said = |what: &str| found.iter().any(|d| d.contains("'Moved/art.psd'") && d.contains(what));
        (the_footage(&p).layer.map(|l| l.layer_size), said("as big as the document"), said("neither the Photoshop layer's size"))
    };
    assert_eq!(framing([5, 10, 1005, 1010], (1000, 1000)), (Some(true), true, false));
    assert_eq!(framing([0, 0, 1000, 1000], (1000, 1000)), (Some(true), false, false));
    assert_eq!(framing([9, -346, 1083, 1086], (1432, 1074)), (Some(true), false, false));
    assert_eq!(framing([749, 800, 764, 848], (1000, 1000)), (Some(false), false, false));
    // An item as big as neither the layer nor the document: shown on the document, reported.
    assert_eq!(framing([5, 10, 1005, 1010], (800, 800)), (Some(false), false, true));
}

/// #3087: a Photoshop layer without pixels (an adjustment layer) keeps the merged image and is
/// reported; the merged image itself (index -1) is no layer.
#[test]
fn a_photoshop_layer_without_pixels_keeps_the_merged_image() {
    let (p, r) = run(&as_psd_layer(4, [0; 4], "Levels 1", (1000, 1000)), true);
    let found = losses(&r.unwrap(), LossCategory::UnsupportedItems);
    assert!(found.iter().any(|d| d.contains("'Levels 1/art.psd'") && d.contains("without pixels")), "{found:?}");
    assert_eq!(the_footage(&p).layer, None);
    let (p, _) = run(&as_psd_layer(-1, [0; 4], "", (1000, 1000)), true);
    assert_eq!(the_footage(&p).layer, None);
}

/// #3087: an expression on Orientation arrives with the stored value, as on any property; it was
/// dropped together with the value and reported as "not decoded".
#[test]
fn an_orientation_expression_is_imported_with_its_value() {
    let bytes = mutate(&mut |c| {
        if !c.is_list(b"otst") {
            return;
        }
        for child in c.children_mut() {
            if child.is_list(b"tdbs") {
                let Body::List { children, .. } = &mut child.body else { continue };
                for k in children.iter_mut() {
                    if k.is(b"tdsb") {
                        set_data(k, 0, &1u32.to_be_bytes());
                    } else if k.is(b"tdb4") {
                        set_data(k, 120, &[1]);
                    }
                }
                children.push(Chunk::data(*b"Utf8", b"[0, time * 90, 0]".to_vec()));
            } else if child.is_list(b"otky") {
                for k in child.children_mut() {
                    let degrees: Vec<u8> = [10.0f64, 20.0, 30.0].iter().flat_map(|d| d.to_be_bytes()).collect();
                    set_data(k, 0, &degrees);
                }
            }
        }
    });
    let (p, r) = run(&bytes, true);
    assert_eq!(losses(&r.unwrap(), LossCategory::TransformProperties), Vec::<String>::new());
    let mut seen = 0;
    for item in p.items.values() {
        let ItemKind::Comp(c) = &item.kind else { continue };
        for layer in &c.layers {
            let Some(o) = layer.transform().and_then(|t| t.prop("orientation")).filter(|o| o.expr.is_some()) else { continue };
            assert_eq!(o.value, effectcraft_keyframe::Value::Vec3([10.0, 20.0, 30.0]), "{}", layer.name);
            assert_eq!(o.expr.as_ref().map(|e| (e.text.as_str(), e.enabled)), Some(("[0, time * 90, 0]", true)), "{}", layer.name);
            seen += 1;
        }
    }
    assert!(seen > 0);
}

/// `SYNTHETIC` with Levels' Channel and Clip to Output Black / White popups' item names (`pdnm`)
/// rewritten to the French labels a French-language After Effects stores there, keeping the same
/// item order so the index each popup is set to is unaffected.
fn with_french_levels_popups() -> Vec<u8> {
    let (mut channel, mut clip) = (0, 0);
    let bytes = mutate(&mut |c| {
        if !c.is(b"Utf8") {
            return;
        }
        if c.bytes() == Some(b"RGB|Red|Green|Blue|Alpha".as_slice()) {
            *c = Chunk::data(*b"Utf8", b"RVB|Rouge|Vert|Bleu|Alpha".to_vec());
            channel += 1;
        } else if c.bytes() == Some(b"On|Off|Off for 32 bpc Color".as_slice()) {
            *c = Chunk::data(*b"Utf8", "Activé|Désactivé|Désactivé pour la couleur 32 bpc".as_bytes().to_vec());
            clip += 1;
        }
    });
    assert_eq!(channel, 2, "Levels' Channel pdnm");
    assert_eq!(clip, 4, "Levels' two Clip pdnm (Black and White), each stored twice");
    bytes
}

/// A popup's item names in the file are matched to EffectCraft's own (English) ones by name
/// (`popup_key`): a project saved by a French After Effects stores "RVB", "Activé", "Désactivé
/// pour la couleur 32 bpc" for Levels' Channel and Clip to Output Black / White (observed in a
/// real "Niveaux" effect, After Effects 2026 26.0.0x67). Before `popup_alias`, none of these
/// matched EffectCraft's English-only options, so each came back "has no counterpart, default
/// used" and the real selection was silently lost.
#[test]
fn french_levels_popup_items_are_recognized() {
    let (unchanged, r1) = run(SYNTHETIC, true);
    let (french, r2) = run(&with_french_levels_popups(), true);
    let (r1, r2) = (r1.unwrap(), r2.unwrap());
    let found = losses(&r2, LossCategory::Effects);
    assert!(!found.iter().any(|d| d.contains("has no counterpart")), "{found:?}");
    for id in ["channel", "clipToOutputBlack", "clipToOutputWhite"] {
        let want: Vec<_> = effect_values(&unchanged).into_iter().filter(|(p, _)| p == id).collect();
        let got: Vec<_> = effect_values(&french).into_iter().filter(|(p, _)| p == id).collect();
        assert_eq!(got, want, "{id}");
    }
    assert_eq!(r1.report.imported.effects, r2.report.imported.effects);
}

/// `popup_alias` must cover every item of a popup it claims to know, not only the one a first
/// report happened to show: Channel's French "Rouge", "Vert", "Bleu" ("Alpha" is the same word in
/// both languages) alongside the already-covered "RVB" (`effect_map.rs`, reviewed on #3432).
#[test]
fn french_levels_channel_aliases_cover_every_item() {
    use effectcraft_aep::effect_map::popup_alias;
    for (french, english) in [("rvb", "rgb"), ("rouge", "red"), ("vert", "green"), ("bleu", "blue")] {
        assert_eq!(popup_alias(french), Some(english), "{french}");
    }
    // "Alpha" needs no alias: the same word in both languages.
    assert_eq!(popup_alias("alpha"), None);
}
