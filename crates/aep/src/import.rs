//! After Effects project (`.aep`) import: the typed view of [`effectcraft_aep`] mapped into an
//! EffectCraft [`Project`]. The XML form (`.aepx`) is accepted as well: both are read into the same
//! flat chunk index (an `.aep` in place, an `.aepx` decoded into one buffer first).
//!
//! What arrives: the project item tree (folders), project settings (depth, working space, time
//! display), compositions (size, exact frame rate, duration, work area, background, pixel
//! aspect, motion blur, shutter, renderer, markers), solid, null, adjustment, footage,
//! precomposition, text, shape, camera and light layers (name, timing, stretch, switches, label,
//! blend mode, parent, track matte, markers, time remapping), the Transform properties (static
//! values and keyframes with interpolation, ease and spatial tangents) and expressions as source
//! text, layer masks, the Source Text and text animators of text layers, shape groups and their
//! items, layer styles, the effects of [`supported_effects`] with their parameters and keyframes
//! in stack order, solids, and footage files and image sequences as links (missing files become
//! missing-footage placeholders). `docs/aep-import.md` has the full list.
//!
//! What does not arrive is **reported**, never dropped silently: [`ImportReport`] counts other
//! effects (by match name; they arrive as placeholders that render nothing, with their parameters
//! for expressions) and unimported effect parameters, unusable masks, layer style
//! features, text layer features, shape items, unsupported blend modes and transform
//! properties, simplified keyframes and unreadable data per [`LossCategory`]. By default the import
//! is refused when anything would be lost ([`ImportOptions::allow_losses`] lifts that); a refusal
//! changes nothing. A damaged file yields an error or a report, never a panic.
//!
//! The crate sits at L3 (above `effectcraft-project`, as `effectcraft-lottie` does) so it builds
//! for wasm32; it reads no files itself: the caller passes the bytes, a way to ask whether a
//! footage path exists and one to ask whether a font is installed.

mod build;
mod camera;
mod effects;
mod lumetri;
mod markers;
mod masks;
mod placeholder;
mod props;
mod puppet;
mod report;
mod settings;
mod shapes;
mod styles;
mod text;
mod text_anim;
mod time;

use effectcraft_project::{ItemId, Project, ProjectSettings};

use crate::container::index::{Root, TreeIndex};
use crate::container::{AepxDrafts, Limits, index_aepx};

pub use effects::supported_effects;
pub use report::{AeVersionInfo, ImportReport, Imported, LIMITATIONS, Loss, LossCategory, MAX_DETAILS, TESTED_MAJORS, UNKNOWN};

/// Why an `.aep` was not imported.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AepImportError {
    /// The file is not a readable After Effects project container.
    #[error("{0}")]
    Container(#[from] crate::AepError),
    /// The container has no project (no `Fold` list).
    #[error("the file holds no After Effects project")]
    NoProject,
    /// Parts of the project cannot be imported and losses were not allowed. The project is
    /// unchanged; the report says what would be lost.
    #[error("importing would lose {}; allow losses to import anyway", .0.summary())]
    Refused(Box<ImportReport>),
}

/// Options of [`import`].
pub struct ImportOptions<'a> {
    /// Import even when [`ImportReport::losses`] is not empty.
    pub allow_losses: bool,
    /// Whether a footage path exists (the caller's file access); a path that does not becomes a
    /// missing-footage placeholder.
    pub exists: &'a dyn Fn(&str) -> bool,
    /// Whether a font (a PostScript name such as `TimesNewRomanPSMT`) is installed; fonts it
    /// rejects are listed in [`ImportReport::missing_fonts`]. Text keeps the font name either way.
    pub has_font: &'a dyn Fn(&str) -> bool,
    /// The folder every imported item goes into (After Effects names it after the file).
    pub folder_name: &'a str,
    /// Whether the caller takes over the file's project settings ([`ImportResult::settings`]).
    /// When not, a setting EffectCraft has no counterpart for is no loss (nothing is changed), so it
    /// is not reported.
    pub settings: bool,
}

impl<'a> ImportOptions<'a> {
    /// Losses refused, every font taken as installed, folder `After Effects Project`, the file's
    /// settings taken over.
    pub fn new(exists: &'a dyn Fn(&str) -> bool) -> Self {
        Self { allow_losses: false, exists, has_font: &any_font, folder_name: "After Effects Project", settings: true }
    }
}

/// The font check of [`ImportOptions::new`]: every font counts as installed.
fn any_font(_: &str) -> bool {
    true
}

/// What an import created.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    /// The folder holding the imported items.
    pub folder: ItemId,
    /// The compositions, in file order.
    pub comps: Vec<ItemId>,
    /// Every item created, the folder first.
    pub items: Vec<ItemId>,
    pub report: ImportReport,
    /// The project's settings with those the file stores (colour depth, working space, working
    /// gamma, the linear-light switches, time display, frame count start) put in. [`import`]
    /// leaves the project's own settings alone; a caller opening the file as a new project
    /// applies these.
    pub settings: ProjectSettings,
}

/// Imports the `.aep` or `.aepx` in `bytes` into `project` (as new items; nothing existing changes).
///
/// # Errors
/// [`AepImportError::Container`] / [`AepImportError::NoProject`] for a file that is not a project,
/// [`AepImportError::Refused`] when something would be lost and `allow_losses` is not set. On any
/// error `project` is unchanged.
pub fn import(project: &mut Project, bytes: &[u8], options: &ImportOptions<'_>) -> Result<ImportResult, AepImportError> {
    // The importer only reads: an `.aep` is indexed in place (one flat array that borrows the
    // bytes); an `.aepx` is decoded into one buffer, which the index borrows.
    if crate::is_aepx(bytes) {
        let AepxDrafts { buffer, nodes, trailer } = index_aepx(bytes, Limits::default())?;
        let index = TreeIndex::from_drafts(&buffer, nodes, trailer);
        import_root(project, Root::from(&index), options)
    } else {
        let index = TreeIndex::parse(bytes)?;
        import_root(project, Root::from(&index), options)
    }
}

/// [`import`] of a file read into either tree.
fn import_root(project: &mut Project, file: Root<'_, '_>, options: &ImportOptions<'_>) -> Result<ImportResult, AepImportError> {
    let view = crate::Project::of(file).ok_or(AepImportError::NoProject)?;
    // Work on a copy so a refusal leaves the project untouched (compositions are shared, cheap).
    let mut work = project.clone();
    let (folder, comps, items, mut report) = build::run(&mut work, &view, options);
    report.ae_version = report::AeVersionInfo::of_root(file);
    if let Some(line) = report.ae_version.limitation() {
        report.limitations.push(line);
    }
    let mut col = report::Collector::default();
    let settings = settings::map(file, &work.settings, &mut col);
    for loss in col.finish().losses.into_iter().filter(|_| options.settings) {
        let at = report.losses.iter().position(|l| l.category > loss.category).unwrap_or(report.losses.len());
        report.losses.insert(at, loss);
    }
    if !options.allow_losses && !report.is_lossless() {
        return Err(AepImportError::Refused(Box::new(report)));
    }
    work.fix_next_id();
    *project = work;
    Ok(ImportResult { folder, comps, items, report, settings })
}

/// The report an import would produce, without touching any project (a dry run); every font
/// counts as installed (see [`analyze_with`]).
///
/// # Errors
/// As [`import`], except that losses are never refused.
pub fn analyze(bytes: &[u8], exists: &dyn Fn(&str) -> bool) -> Result<ImportReport, AepImportError> {
    analyze_with(bytes, &ImportOptions::new(exists))
}

/// The report an import with `options` would produce (losses are always allowed), without touching
/// any project.
///
/// # Errors
/// As [`import`], except that losses are never refused.
pub fn analyze_with(bytes: &[u8], options: &ImportOptions<'_>) -> Result<ImportReport, AepImportError> {
    let mut scratch = Project::default();
    let options = ImportOptions { allow_losses: true, ..*options };
    Ok(import(&mut scratch, bytes, &options)?.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{AepFile, Body, Chunk, parse, write};

    /// Asserts the two destination projects are structurally equal and serialize identically (layers,
    /// properties and values included, which `ImportResult` does not carry). Nothing is normalized:
    /// the import is deterministic, ids and timestamps included.
    fn same_project(a: &Project, b: &Project, name: &str) {
        assert!(a == b, "{name}: the imported projects differ through the index");
        let (ja, jb) = (serde_json::to_string(a).ok(), serde_json::to_string(b).ok());
        assert!(ja.is_some() && ja == jb, "{name}: the serialized projects differ through the index");
    }

    /// Imports `bytes` through the owned chunk tree and through the index: the same project, report
    /// and settings, or the same error.
    fn both(bytes: &[u8], name: &str) {
        let exists = |_: &str| true;
        let options = ImportOptions { allow_losses: true, ..ImportOptions::new(&exists) };
        match (parse(bytes), TreeIndex::parse(bytes)) {
            (Ok(file), Ok(index)) => {
                let (mut pa, mut pb) = (Project::default(), Project::default());
                let a = import_root(&mut pa, Root::from(&file), &options);
                let b = import_root(&mut pb, Root::from(&index), &options);
                assert!(a == b, "{name}: the import differs through the index");
                same_project(&pa, &pb, name);
            }
            (Err(a), Err(b)) => assert_eq!(a, b, "{name}"),
            (a, b) => panic!("{name}: {:?} vs {:?}", a.err(), b.err()),
        }
    }

    /// A 40-byte NUL-padded match name.
    fn tdmn(name: &str) -> Chunk<'static> {
        let mut b = name.as_bytes().to_vec();
        b.resize(40, 0);
        Chunk::data(*b"tdmn", b)
    }

    /// A synthetic project imports the same through the index.
    #[test]
    fn a_synthetic_project_imports_the_same_through_the_index() {
        // A composition with a layer whose effect stores arbitrary data and a named value group.
        let utf8 = |id: [u8; 4], t: &str| Chunk { id, body: Body::Chunks(vec![Chunk::data(*b"Utf8", t.as_bytes().to_vec())]), pad: None };
        let record = Chunk::list(*b"tdbs", vec![Chunk::data(*b"tdsb", vec![0, 0, 0, 1]), Chunk::data(*b"cdat", vec![0, 0, 0, 1])]);
        let values = Chunk::list(
            *b"tdgp",
            vec![
                utf8(*b"tdsn", "Mine"),
                tdmn("ADBE Easy Levels2-0001"),
                record,
                Chunk::list(*b"aRbs", vec![Chunk::data(*b"aRbp", vec![0u8; 108])]),
                tdmn("ADBE Group End"),
            ],
        );
        let part = Chunk::list(*b"parT", vec![Chunk::data(*b"parn", vec![0, 0, 0, 2]), tdmn("ADBE Easy Levels2-0001"), Chunk::data(*b"pard", vec![0u8; 148])]);
        let sspc = Chunk::list(*b"sspc", vec![utf8(*b"fnam", "Levels"), part, values]);
        let parade = Chunk::list(*b"tdgp", vec![tdmn("ADBE Easy Levels2"), sspc, tdmn("ADBE Group End")]);
        let root = Chunk::list(*b"tdgp", vec![tdmn("ADBE Effect Parade"), parade, tdmn("ADBE Group End")]);
        let layer = Chunk::list(*b"Layr", vec![Chunk::data(*b"ldta", vec![0u8; 160]), Chunk::data(*b"Utf8", b"Layer".to_vec()), root]);
        let mut idta = vec![0u8; 64];
        idta[..2].copy_from_slice(&4u16.to_be_bytes());
        let mut cdta = vec![0u8; 200];
        cdta[4..12].copy_from_slice(&[0, 0, 0, 1, 0, 0, 0, 30]);
        let item = Chunk::list(*b"Item", vec![Chunk::data(*b"idta", idta), Chunk::data(*b"Utf8", b"Comp".to_vec()), Chunk::data(*b"cdta", cdta), layer]);
        let file = AepFile { chunks: vec![Chunk::data(*b"head", vec![0u8; 20]), Chunk::list(*b"Fold", vec![item])], trailer: std::borrow::Cow::Borrowed(&[]) };
        both(&write(&file).expect("write"), "synthetic");
    }

    /// Every `.aep` of the fixture directory (skipped without `EFFECTCRAFT_AEP_FIXTURES`).
    #[test]
    fn every_fixture_imports_the_same_through_the_index() {
        let Some(dir) = std::env::var_os("EFFECTCRAFT_AEP_FIXTURES").filter(|d| !d.is_empty()) else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<std::path::PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aep")).collect();
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            both(&bytes, &path.display().to_string());
        }
    }

    /// Imports an `.aepx` through the owned tree ([`crate::read_aepx`]) and through the index
    /// ([`import`]): the same project, report and settings, or the same error.
    fn both_aepx(bytes: &[u8], name: &str) -> Result<ImportResult, AepImportError> {
        let exists = |_: &str| true;
        let options = ImportOptions { allow_losses: true, ..ImportOptions::new(&exists) };
        let (mut pa, mut pb) = (Project::default(), Project::default());
        let a = crate::read_aepx(bytes).map_err(AepImportError::from).and_then(|file| import_root(&mut pa, Root::from(&file), &options));
        let b = import(&mut pb, bytes, &options);
        assert!(a == b, "{name}: the .aepx import differs through the index");
        same_project(&pa, &pb, name);
        b
    }

    /// A synthetic `.aepx` imports the same through the index as through the owned tree.
    #[test]
    fn a_synthetic_aepx_imports_the_same_through_the_index() {
        // The comp of the owned-tree test above (`idta` type 4, `cdta` 1/30 s frame duration).
        let idta = format!("0004{}", "00".repeat(62));
        let cdta = format!("00000000000000010000001e{}", "00".repeat(188));
        let x = format!(
            "<?xml version=\"1.0\"?><AfterEffectsProject><head bdata=\"{}\"/><Fold><Item><idta bdata=\"{idta}\"/><string>Comp</string><cdta bdata=\"{cdta}\"/></Item></Fold><ProjectXMPMetadata>x</ProjectXMPMetadata></AfterEffectsProject>",
            "00".repeat(20)
        );
        let imported = both_aepx(x.as_bytes(), "synthetic").expect("the synthetic project imports");
        assert_eq!(imported.comps.len(), 1);
        assert_eq!(both_aepx(b"<AfterEffectsProject/>", "empty").err(), Some(AepImportError::NoProject));
        assert!(matches!(both_aepx(b"<AfterEffectsProject><a>", "broken"), Err(AepImportError::Container(crate::AepError::Xml { .. }))));
    }

    /// Every `.aepx` of the fixture directory (skipped without `EFFECTCRAFT_AEP_FIXTURES`).
    #[test]
    fn every_fixture_aepx_imports_the_same_through_the_index() {
        let Some(dir) = std::env::var_os("EFFECTCRAFT_AEP_FIXTURES").filter(|d| !d.is_empty()) else { return };
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<std::path::PathBuf> =
            entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aepx")).collect();
        paths.sort();
        for path in paths {
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let _ = both_aepx(&bytes, &path.display().to_string());
        }
    }
}
