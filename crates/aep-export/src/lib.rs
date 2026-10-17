//! After Effects project (`.aep`) export: an EffectCraft [`Project`] written as an `.aep` that
//! After Effects 2026 opens.
//!
//! The format has many chunks that are not understood, and After Effects only opens a file whose
//! chunks are consistent. The exporter therefore does not write a file from scratch: it **edits a
//! generated template** (a project After Effects saved, embedded in the crate) with
//! `effectcraft-aep`, cloning the item, layer and property records of the template and changing
//! only the fields whose layout is verified (`docs/aep-format.md` section 3, `docs/aep-export.md`):
//! item and layer ids, the composition header, the layer header, solid colour and size, the five
//! Transform properties (static values, keyframes with interpolation and ease, expressions).
//!
//! What cannot be written is **reported**, never dropped silently ([`ExportReport`]); by default an
//! export that would lose anything is refused ([`ExportOptions::allow_losses`]).
//!
//! The crate sits at L3 (above `effectcraft-project`, next to the importer) and builds for wasm32:
//! it does no file access, the caller gets the bytes.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod emit;
mod plan;
mod report;
mod template;
mod timebase;
mod tree;

use effectcraft_project::Project;

pub use report::{ExportReport, Exported, LIMITATIONS, Loss, LossCategory, MAX_DETAILS};

/// Why an `.aep` was not written.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AepExportError {
    /// Parts of the project cannot be exported and losses were not allowed. Nothing was written;
    /// the report says what would be lost.
    #[error("exporting would lose {}; allow losses to export anyway", .0.summary())]
    Refused(Box<ExportReport>),
    /// The embedded template does not have the shape the exporter expects (a bug in the crate).
    #[error("internal error: {0}")]
    Template(&'static str),
    /// The container could not be written (a chunk or the file exceeds the format's 4 GB limit).
    #[error("{0}")]
    Container(#[from] effectcraft_aep::AepError),
}

/// Options of [`export`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ExportOptions {
    /// Export even when [`ExportReport::losses`] is not empty.
    pub allow_losses: bool,
}

/// An exported project.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportResult {
    /// The `.aep` file.
    pub bytes: Vec<u8>,
    pub report: ExportReport,
}

/// A neutral XMP packet for the end of the file (After Effects writes one; it only holds metadata).
const XMP: &[u8] = b"<?xpacket begin=\"\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n   <dc:format>application/vnd.adobe.aftereffects.project</dc:format>\n  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>";

/// Exports `project` as an `.aep`.
///
/// # Errors
/// [`AepExportError::Refused`] when something would be lost and `allow_losses` is not set (nothing
/// is written), [`AepExportError::Container`] for a file beyond the container's size limits.
pub fn export(project: &Project, options: &ExportOptions) -> Result<ExportResult, AepExportError> {
    let mut col = report::Collector::default();
    let plan = plan::plan(project, &mut col);
    let report = col.finish();
    if !options.allow_losses && !report.is_lossless() {
        return Err(AepExportError::Refused(Box::new(report)));
    }
    let bytes = emit::emit(&plan, XMP)?;
    Ok(ExportResult { bytes, report })
}

/// The report an export would produce, without writing anything (a dry run).
pub fn analyze(project: &Project) -> ExportReport {
    let mut col = report::Collector::default();
    let _ = plan::plan(project, &mut col);
    col.finish()
}
