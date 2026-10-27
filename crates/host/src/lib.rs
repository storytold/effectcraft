//! A fully wired [`Session`]: footage decoding through `effectcraft-media` (FilmCraft's codecs),
//! the media importer, the expression engine, JavaScript scripting (`effectcraft-script`) and
//! Render Queue export through `effectcraft-export` (FilmCraft's encoders). Frontends (desktop,
//! CLI, MCP, web) start here.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use effectcraft_engine::project::render_queue::OutputFormat;
use effectcraft_engine::{ExportJob, ExportResult, Exporter, Importer, Session};
use effectcraft_project::Footage;
pub use effectcraft_script as script;

pub mod app_dirs;

struct MediaImporter;

impl Importer for MediaImporter {
    fn probe(&self, path: &str) -> Result<Footage, String> {
        effectcraft_media::probe(path).map_err(|e| e.to_string())
    }
}

/// Render Queue export via `effectcraft-export`: to the file system, or to `sink` (the web app
/// turns written files into downloads).
#[derive(Default)]
pub struct FileExporter {
    pub sink: Option<Arc<effectcraft_export::Sink>>,
}

impl Exporter for FileExporter {
    fn formats(&self) -> Vec<OutputFormat> {
        effectcraft_export::available_formats()
    }
    fn export(&self, job: &ExportJob, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ExportResult, String> {
        effectcraft_engine::render::passes::block_on(self.export_async(job, progress))
    }

    fn export_async<'a>(
        &'a self,
        job: &'a ExportJob<'a>,
        progress: &'a mut dyn FnMut(u64, u64) -> bool,
    ) -> effectcraft_engine::render_queue::LocalFuture<'a, Result<ExportResult, String>> {
        Box::pin(async move {
            let j = effectcraft_export::Job {
                project: job.project,
                footage: job.footage,
                expr: job.expr,
                accel: job.accel,
                comp: job.item.comp,
                settings: &job.item.settings,
                output: &job.item.output,
                path: job.path,
                sink: self.sink.as_deref(),
                options: effectcraft_export::JobOptions {
                    log: job.item.log,
                    label: job.label.clone(),
                    storage: job.storage,
                    overflow: job.project.render_prefs.overflow_folders.clone(),
                },
                nested_switches: job.nested_switches,
            };
            match effectcraft_export::export_async(&j, &mut |p| progress(p.done, p.total)).await {
                Ok(r) => Ok(ExportResult {
                    path: r.path,
                    frames: r.frames,
                    width: r.width,
                    height: r.height,
                    bytes: r.bytes,
                    seconds: r.seconds,
                    audio: r.audio,
                    log: r.log,
                    overflow: r.overflow,
                }),
                Err(effectcraft_export::ExportError::Cancelled) => Err(effectcraft_engine::render_queue::CANCELLED.into()),
                Err(e) => Err(e.to_string()),
            }
        })
    }
}

/// A new session with media, import, expressions, scripting and export enabled.
pub fn session() -> Session {
    let mut s = Session {
        // The desktop app, the CLI and the MCP server share installed Roto Brush models.
        models_dir: config_dir().map(|d| d.join("models")),
        exporter: Some(Arc::new(FileExporter::default())),
        footage: Arc::new(effectcraft_media::MediaPool::new()),
        importer: Some(Arc::new(MediaImporter)),
        expr: Some(Arc::new(effectcraft_expr::Expressions)),
        expr_check: Some(effectcraft_expr::check_syntax),
        plugin_loader: effectcraft_plugin::wasm_available().then_some(effectcraft_plugin::loader as effectcraft_engine::PluginLoader),
        ..Default::default()
    };
    effectcraft_script::install(&mut s);
    s
}

/// The platform config directory for EffectCraft (`EFFECTCRAFT_CONFIG_DIR` overrides):
/// `~/Library/Application Support/EffectCraft` (macOS), `%APPDATA%\EffectCraft` (Windows),
/// `$XDG_CONFIG_HOME/effectcraft` or `~/.config/effectcraft` (Linux and others). A `portable.txt`
/// beside the executable puts it in `EffectCraftData` there instead ([`app_dirs`]).
pub fn config_dir() -> Option<std::path::PathBuf> {
    app_dirs::current().dir.clone()
}

#[cfg(test)]
mod tests;
