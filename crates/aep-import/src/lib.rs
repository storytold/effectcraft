//! After Effects project (`.aep`) import: the typed view of [`effectcraft_aep`] mapped into an
//! EffectCraft [`Project`]. The XML form (`.aepx`) is accepted as well: it is read into the same
//! chunk tree ([`effectcraft_aep::parse_any`] picks the reader by content).
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
//! effects (by match name) and unimported effect parameters, unusable masks, layer style
//! features, text layer features, shape items, unsupported blend modes and transform
//! properties, simplified keyframes and unreadable data per [`LossCategory`]. By default the import
//! is refused when anything would be lost ([`ImportOptions::allow_losses`] lifts that); a refusal
//! changes nothing. A damaged file yields an error or a report, never a panic.
//!
//! The crate sits at L3 (above `effectcraft-project`, as `effectcraft-lottie` does) so it builds
//! for wasm32; it reads no files itself: the caller passes the bytes, a way to ask whether a
//! footage path exists and one to ask whether a font is installed.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod camera;
mod effects;
mod import;
mod markers;
mod masks;
mod props;
mod report;
mod settings;
mod shapes;
mod styles;
mod text;
mod text_anim;
mod time;

use effectcraft_project::{ItemId, Project, ProjectSettings};

pub use effects::supported_effects;
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
    /// Whether a font (a PostScript name such as `TimesNewRomanPSMT`) is installed; fonts it
    /// rejects are listed in [`ImportReport::missing_fonts`]. Text keeps the font name either way.
    pub has_font: &'a dyn Fn(&str) -> bool,
    /// The folder every imported item goes into (After Effects names it after the file).
    pub folder_name: &'a str,
}

impl<'a> ImportOptions<'a> {
    /// Losses refused, every font taken as installed, folder `After Effects Project`.
    pub fn new(exists: &'a dyn Fn(&str) -> bool) -> Self {
        Self { allow_losses: false, exists, has_font: &any_font, folder_name: "After Effects Project" }
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
    let file = effectcraft_aep::parse_any(bytes)?;
    let view = effectcraft_aep::Project::new(&file).ok_or(AepImportError::NoProject)?;
    // Work on a copy so a refusal leaves the project untouched (compositions are shared, cheap).
    let mut work = project.clone();
    let (folder, comps, items, mut report) = import::run(&mut work, &view, options);
    let mut col = report::Collector::default();
    let settings = settings::map(&file, &work.settings, &mut col);
    for loss in col.finish().losses {
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
