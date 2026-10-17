//! After Effects project (`.aep`) import: the typed view of [`effectcraft_aep`] mapped into an
//! EffectCraft [`Project`].
//!
//! What arrives: the project item tree (folders), compositions (size, exact frame rate, duration,
//! work area, background, pixel aspect, motion blur, shutter angle, renderer), solid, null,
//! adjustment, footage and precomposition layers (name, timing, stretch, switches, label, blend
//! mode for the six sampled modes, parent), the Transform properties (static values and keyframes
//! with interpolation, ease and spatial tangents) and their expressions as source text, solids,
//! and footage files as links (missing files become missing-footage placeholders).
//!
//! What does not arrive is **reported**, never dropped silently: [`ImportReport`] counts effects,
//! masks, markers, shape/text/camera/light layers, unsupported blend modes and transform
//! properties, simplified keyframes and unreadable data per [`LossCategory`]. By default the import
//! is refused when anything would be lost ([`ImportOptions::allow_losses`] lifts that); a refusal
//! changes nothing. A damaged file yields an error or a report, never a panic.
//!
//! The crate sits at L3 (above `effectcraft-project`, as `effectcraft-lottie` does) so it builds
//! for wasm32; it reads no files itself: the caller passes the bytes and a way to ask whether a
//! footage path exists.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod import;
mod props;
mod report;
mod time;

use effectcraft_project::{ItemId, Project};

pub use report::{ImportReport, Imported, LIMITATIONS, Loss, LossCategory, MAX_DETAILS};

/// Why an `.aep` was not imported.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AepImportError {
    /// The file is not a readable After Effects project container.
    #[error("{0}")]
    Container(#[from] effectcraft_aep::AepError),
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
    /// The folder every imported item goes into (After Effects names it after the file).
    pub folder_name: &'a str,
}

impl<'a> ImportOptions<'a> {
    /// Losses refused, folder `After Effects Project`.
    pub fn new(exists: &'a dyn Fn(&str) -> bool) -> Self {
        Self { allow_losses: false, exists, folder_name: "After Effects Project" }
    }
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
}

/// Imports the `.aep` in `bytes` into `project` (as new items; nothing existing changes).
///
/// # Errors
/// [`AepImportError::Container`] / [`AepImportError::NoProject`] for a file that is not a project,
/// [`AepImportError::Refused`] when something would be lost and `allow_losses` is not set. On any
/// error `project` is unchanged.
pub fn import(project: &mut Project, bytes: &[u8], options: &ImportOptions<'_>) -> Result<ImportResult, AepImportError> {
    let file = effectcraft_aep::parse(bytes)?;
    let view = effectcraft_aep::Project::new(&file).ok_or(AepImportError::NoProject)?;
    // Work on a copy so a refusal leaves the project untouched (compositions are shared, cheap).
    let mut work = project.clone();
    let (folder, comps, items, report) = import::run(&mut work, &view, options);
    if !options.allow_losses && !report.is_lossless() {
        return Err(AepImportError::Refused(Box::new(report)));
    }
    work.fix_next_id();
    *project = work;
    Ok(ImportResult { folder, comps, items, report })
}

/// The report an import would produce, without touching any project (a dry run).
///
/// # Errors
/// As [`import`], except that losses are never refused.
pub fn analyze(bytes: &[u8], exists: &dyn Fn(&str) -> bool) -> Result<ImportReport, AepImportError> {
    let mut scratch = Project::default();
    let options = ImportOptions { allow_losses: true, exists, folder_name: "After Effects Project" };
    Ok(import(&mut scratch, bytes, &options)?.report)
}
