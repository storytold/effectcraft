//! The After Effects version of a file (`head`, `svap`) in the import report, and files that After
//! Effects 2026 saved as copies for 25.x and 24.x (issue #3115).
//!
//! The fixture-free tests edit the committed `synthetic.aep` (saved by 26.2.1x2). The fixture tests
//! read, per family of the corpus, the same-session 2026 save `<family>-26.aep` and its copies
//! `<family>-25x.aep` (File ▸ Save a Copy As 25.x) and `<family>-24x.aep` (24.x) from
//! `EFFECTCRAFT_AEP_FIXTURES`; they are ignored until the corpus holds those files.

use effectcraft_aep::import::{AeVersionInfo, ImportOptions, ImportReport, ImportResult, TESTED_MAJORS, UNKNOWN, analyze, import};
use effectcraft_aep::{AepFile, Body, Chunk, parse, write};
use effectcraft_project::Project;

const SYNTHETIC: &[u8] = include_bytes!("../../fixtures/synthetic.aep");

/// The project with `edit` applied to every top-level chunk.
fn mutate(edit: &mut dyn FnMut(&mut Chunk<'_>)) -> Vec<u8> {
    let mut file: AepFile<'_> = parse(SYNTHETIC).unwrap();
    for c in &mut file.chunks {
        edit(c);
    }
    write(&file).unwrap()
}

/// `synthetic.aep` with `head` bytes 4 to 7 replaced by `word`.
fn with_head_word(word: u32) -> Vec<u8> {
    mutate(&mut |c| {
        if c.is(b"head")
            && let Body::Data(d) = &mut c.body
        {
            d.to_mut().get_mut(4..8).unwrap().copy_from_slice(&word.to_be_bytes());
        }
    })
}

/// The report of an analysis of `bytes` (no footage on disk).
fn report(bytes: &[u8]) -> ImportReport {
    analyze(bytes, &|_| false).unwrap()
}

/// The limitation lines about an untested version.
fn version_limitations(r: &ImportReport) -> Vec<&String> {
    r.limitations.iter().filter(|l| l.contains("is not tested")).collect()
}

#[test]
fn the_report_names_the_version_the_file_is_written_for_and_saved_by() {
    let r = report(SYNTHETIC);
    assert_eq!(r.ae_version, AeVersionInfo { file_version: "26.2.1x2".into(), saved_by: "26.2.1x2".into(), tested: true });
    assert!(version_limitations(&r).is_empty());
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["aeVersion"]["fileVersion"], "26.2.1x2");
    assert_eq!(json["aeVersion"]["savedBy"], "26.2.1x2");
    assert_eq!(json["aeVersion"]["tested"], true);
}

/// A copy saved for 25.x or 24.x reports that version and the 2026 writer, and counts as tested.
#[test]
fn a_copy_saved_for_an_older_version_names_both() {
    // The words of the copies After Effects 2026 saved as 25.x and 24.x; `svap` stays 2026's.
    for (word, text) in [(0x0f0b_0661, "25.6.0x97"), (0x0f03_0641, "24.6.0x65")] {
        let r = report(&with_head_word(word));
        assert_eq!(r.ae_version.file_version, text);
        assert_eq!(r.ae_version.saved_by, "26.2.1x2");
        assert!(r.ae_version.tested);
        assert!(version_limitations(&r).is_empty());
    }
}

#[test]
fn an_untested_version_is_imported_and_said_to_be_untested() {
    // 23.6.0x97: major 23 (bits 25-24 = 10, bits 21-19 = 111).
    let bytes = with_head_word(0x0e3b_0661);
    let mut project = Project::default();
    let none = |_: &str| false;
    // Not a loss: the losses are those of the unedited file.
    let r: ImportResult = import(&mut project, &bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).unwrap();
    assert_eq!(r.report.losses, report(SYNTHETIC).losses);
    assert_eq!(r.report.ae_version.file_version, "23.6.0x97");
    assert!(!r.report.ae_version.tested);
    assert!(!TESTED_MAJORS.contains(&23));
    let lines = version_limitations(&r.report);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("After Effects 23.6.0x97 (saved by 26.2.1x2)"), "{}", lines[0]);
    assert!(!r.comps.is_empty());
}

/// A missing, short or empty version word reads as `unknown` and the file still imports.
#[test]
fn a_file_without_a_version_reads_as_unknown_and_imports() {
    // A zero word (After Effects itself refuses it, "[0.0x0]") and all ones.
    for word in [0, u32::MAX] {
        let r = report(&with_head_word(word));
        assert_eq!(r.ae_version.file_version, UNKNOWN);
        assert!(!r.ae_version.tested);
        assert_eq!(version_limitations(&r).len(), 1);
    }
    // No `svap` (After Effects opens such a file): the id is renamed, the size kept.
    let bytes = mutate(&mut |c| {
        if c.is(b"svap") {
            c.id = *b"xsvp";
        }
    });
    let r = report(&bytes);
    assert_eq!(r.ae_version.saved_by, UNKNOWN);
    assert_eq!(r.ae_version.file_version, "26.2.1x2");
    // A `head` too short for the word, and none at all.
    for keep in [Some(5usize), None] {
        let bytes = mutate(&mut |c| {
            if c.is(b"head") {
                match keep {
                    Some(n) => {
                        if let Body::Data(d) = &mut c.body {
                            d.to_mut().truncate(n);
                        }
                    }
                    None => c.id = *b"xhed",
                }
            }
        });
        let r = report(&bytes);
        assert_eq!(r.ae_version, AeVersionInfo { file_version: UNKNOWN.into(), saved_by: "26.2.1x2".into(), tested: false });
    }
}

#[test]
fn a_report_without_the_field_still_reads() {
    let mut json = serde_json::to_value(report(SYNTHETIC)).unwrap();
    json.as_object_mut().unwrap().remove("aeVersion");
    let r: ImportReport = serde_json::from_value(json).unwrap();
    assert_eq!(r.ae_version, AeVersionInfo::default());
    assert_eq!(r.ae_version.file_version, UNKNOWN);
}

/// The corpus families saved three ways.
const FAMILIES: &[&str] =
    &["1200-lumetri", "3d", "effects", "footage-precomp", "keyframes", "layers-comps", "markers", "masks", "settings-a", "shapes", "styles", "text"];

/// The import of a file: the project as JSON, the report and the parsed versions.
fn import_file(name: &str) -> Option<(serde_json::Value, ImportReport, effectcraft_project::ProjectSettings)> {
    let bytes = crate::common::try_file(name)?;
    let mut project = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    let r = import(&mut project, &bytes, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
    Some((serde_json::to_value(&project).unwrap(), r.report, r.settings))
}

/// Per corpus family, the 25.x and 24.x copies import to the same project and report as the 2026 save.
#[test]
#[ignore = "needs the version fixtures of the corpus (<family>-26/-25x/-24x.aep)"]
fn copies_saved_as_25x_and_24x_import_like_the_2026_file() {
    for family in FAMILIES {
        let Some((project_26, report_26, settings_26)) = import_file(&format!("{family}-26.aep")) else { continue };
        assert_eq!(report_26.ae_version, AeVersionInfo { file_version: "26.2.1x2".into(), saved_by: "26.2.1x2".into(), tested: true }, "{family}");
        for (suffix, version) in [("25x", "25.6.0x97"), ("24x", "24.6.0x65")] {
            let name = format!("{family}-{suffix}.aep");
            let Some((project, report, settings)) = import_file(&name) else { continue };
            assert_eq!(report.ae_version, AeVersionInfo { file_version: version.into(), saved_by: "26.2.1x2".into(), tested: true }, "{name}");
            assert_eq!(project, project_26, "{name}: the imported project differs from the 2026 file's");
            assert_eq!(settings, settings_26, "{name}: the imported settings differ from the 2026 file's");
            let same_but_version = ImportReport { ae_version: report_26.ae_version.clone(), ..report };
            assert_eq!(same_but_version, report_26, "{name}: the report differs from the 2026 file's");
        }
    }
}

/// Per corpus family, `head` holds the version each copy is written for and `svap` the writer.
#[test]
#[ignore = "needs the version fixtures of the corpus (<family>-26/-25x/-24x.aep)"]
fn the_container_of_a_copy_states_its_version_in_head_and_the_writer_in_svap() {
    for family in FAMILIES {
        for (suffix, version) in [("26", "26.2.1x2"), ("25x", "25.6.0x97"), ("24x", "24.6.0x65")] {
            let name = format!("{family}-{suffix}.aep");
            let Some(bytes) = crate::common::try_file(&name) else { continue };
            let file = parse(&bytes).unwrap();
            assert_eq!(file.file_version().map(|v| v.text()).as_deref(), Some(version), "{name}");
            assert_eq!(file.saved_by_version().map(|v| v.text()).as_deref(), Some("26.2.1x2"), "{name}");
        }
    }
}
