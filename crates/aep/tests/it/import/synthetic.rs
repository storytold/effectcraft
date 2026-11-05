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
