//! EffectCraft's half of Open and Save against cloud storage.
//!
//! The pages themselves are `craft_ui::cloud`. Every Craft app offers the same
//! ones. What is here is what only EffectCraft can answer: a document is a
//! project file (`.ecproj` JSON). Bytes become a project through `file.open`
//! with a `text` parameter, and go back through `file.toJson`.
//!
//! EffectCraft holds no provider credential and speaks no provider protocol.
//! Every call goes to the CollabCraft service, which is the single place a
//! provider is configured and permissions are decided. Replacing Nextcloud
//! with something else changes nothing here.
//!
//! Nothing in this file opens a socket. The transport half is a [`CloudBackend`]
//! the host application supplies (see `apps/effectcraft/src/cloud` for desktop
//! and `apps/effectcraft-web/src/cloud` for the browser).

use craft_cloud::{Entry, EntryKind};
use egui::{Color32, FontFamily, Painter, Rect, Ui};
use serde_json::json;

// Re-exported so a host application, and a test, can name these types without
// depending on the crates they come from.
pub use craft_cloud::{Capability, EntryKind as CloudEntryKind};
pub use craft_ui::cloud::{
    // state
    Account,
    Busy,
    // backend
    CloudApp,
    CloudBackend,
    CloudFiles,
    Conflict,
    CopyFormat,
    Destination,
    DocumentKind,
    Event,
    Fault,
    Folder,
    Job,
    Link,
    Origin,
    Post,
    Source,
    UNAVAILABLE,
    // functions
    available,
    browse,
    can_converse,
    conflict_modal,
    destination_name,
    has_target,
    link,
    load_shared,
    open_entry,
    open_page,
    origin,
    parent_of,
    prefers_cloud,
    progress,
    pump,
    reload_active,
    report,
    save,
    save_as,
    save_page,
    share_button,
    shares,
    sign_in,
    sign_out,
    start,
    who,
    work_together,
};

use crate::EffectcraftApp;
use crate::icons::{self, Icon};

/// Extensions EffectCraft opens from the cloud.
const OPENS: [&str; 2] = ["ecproj", "ecprojx"];

/// Content types EffectCraft opens.
const MIMES: [&str; 1] = ["application/json"];

/// What an EffectCraft document is, as a cloud file list needs to know.
pub const PROJECT: DocumentKind = DocumentKind { extensions: &OPENS, mimes: &MIMES, noun: "project", default_extension: "ecproj" };

/// The Save a Copy formats the Share menu offers (none for now).
const COPIES: [(&str, &str); 0] = [];

/// Whether EffectCraft can open this entry.
pub fn opens(entry: &Entry) -> bool {
    craft_ui::cloud::opens(entry, PROJECT)
}

/// A typed name as a project file name. Any folder separators are dropped and
/// a missing extension becomes `.ecproj`.
pub fn project_name(typed: &str) -> String {
    craft_ui::cloud::document_name(typed, PROJECT)
}

/// The whole of the cloud state, for the control channel and for tests.
pub fn status(app: &EffectcraftApp) -> serde_json::Value {
    craft_ui::cloud::report(app)
}

/// The Share menu's export half (empty — no single-file export yet).
pub fn copies() -> Vec<CopyFormat> {
    COPIES.iter().map(|(id, label)| CopyFormat::new(*id, *label)).collect()
}

/// The host part of the service address for the account menu's first line.
fn service_host() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        // web_sys is not a direct dep of ui-egui; return empty string on wasm32.
        return String::new();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ["EFFECTCRAFT_CLOUD_URL", "CRAFT_CLOUD_URL", "COLLABCRAFT_URL"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .find(|value| !value.is_empty())
            .map(|value| value.trim_start_matches("https://").trim_start_matches("http://").to_owned())
            .unwrap_or_default()
    }
}

// ---- CloudApp for EffectcraftApp ----------------------------------------

impl CloudApp for EffectcraftApp {
    fn cloud(&self) -> &CloudFiles {
        &self.cloud
    }

    fn cloud_mut(&mut self) -> &mut CloudFiles {
        &mut self.cloud
    }

    fn kind(&self) -> DocumentKind {
        PROJECT
    }

    fn active(&self) -> Option<u64> {
        // EffectCraft holds one project per window; uid 0 represents it.
        Some(0)
    }

    fn title(&self, _uid: u64) -> String {
        self.session
            .path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled Project".to_owned())
    }

    fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Option<u64>, String> {
        // Projects are JSON text; feed them via the `text` param to `file.open`
        // so no temporary file is needed on any platform, including wasm32.
        let text = String::from_utf8(bytes).map_err(|e| format!("project bytes are not UTF-8: {e}"))?;
        match self.session.execute("file.open", json!({"path": name, "text": text})) {
            Ok(_) => Ok(Some(0)),
            Err(e) => Err(e.to_string()),
        }
    }

    fn encode(&mut self, _uid: u64, into: Destination<'_>) -> Result<Vec<u8>, String> {
        let ext = into.extension();
        if !ext.is_empty() && ext != "ecproj" {
            return Err(format!("EffectCraft can only save projects as .ecproj (got .{ext})"));
        }
        let result = self.session.execute("file.toJson", json!({})).map_err(|e| e.to_string())?;
        let text = result["text"].as_str().ok_or("file.toJson returned no text")?;
        Ok(text.as_bytes().to_vec())
    }

    fn close(&mut self, _uid: u64) {
        // Replace the cloud-opened project with an empty one. Ignore errors.
        let _ = self.session.execute("file.newProject", json!({}));
    }

    fn mark_saved(&mut self, _uid: u64) {
        self.session.mark_saved();
    }

    fn drop_pending(&mut self, _uid: u64) {
        // Nothing to snapshot at encode time in EffectCraft.
    }

    fn ready_to_save(&mut self) -> bool {
        true
    }

    fn sharing(&mut self) -> &mut craft_ui::share::ShareDialog {
        &mut self.share
    }

    fn presence(&mut self) -> &mut craft_ui::presence::PresenceBar {
        &mut self.presence
    }

    fn paint_icon(&self, painter: &Painter, rect: Rect, kind: EntryKind, colour: Color32) {
        let icon = if kind == EntryKind::Folder {
            Icon::Folder
        } else {
            // A Comp icon stands in for a project file in the cloud file list.
            Icon::Comp
        };
        icons::paint(painter, rect, icon, colour);
    }

    fn open_url(&self, url: &str) {
        if let Some(open) = &self.hooks.open_url {
            open(url);
        }
        // The fallback (no hook set) is handled in the cloud pump: device-flow
        // URLs are returned as events and the desktop app opens them via the
        // native shell.
    }

    fn say(&mut self, text: String) {
        self.ui.status = text;
    }

    fn toast(&mut self, text: String) {
        let now = self.frame_time();
        self.toast = Some((text, now));
    }

    fn app_name(&self) -> &str {
        "EffectCraft"
    }

    fn service(&self) -> String {
        service_host()
    }

    fn save_note(&self) -> Option<String> {
        Some("The project is saved as an EffectCraft project file (.ecproj) in the folder above.".to_owned())
    }

    fn style(&self, _ui: &Ui) -> craft_ui::cloud::CloudStyle {
        let t = &self.tokens;
        // EffectCraft has no `accent_soft` token; use a low-opacity accent fill.
        let soft = Color32::from_rgba_unmultiplied(t.accent.r(), t.accent.g(), t.accent.b(), 40);
        craft_ui::cloud::CloudStyle {
            surface: t.panel_bg,
            border: t.field_border,
            text: t.text,
            dim: t.text_dim,
            hover: t.hover,
            soft,
            family: FontFamily::Proportional,
            bold: FontFamily::Name("semibold".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_document_kind_accepts_ecproj() {
        let mut e = craft_cloud::Entry {
            id: "1".into(),
            room: "r".into(),
            name: "scene.ecproj".into(),
            path: "/".into(),
            kind: EntryKind::File,
            size: None,
            modified: None,
            mime: Some("application/json".into()),
            etag: None,
            writable: true,
            shared_by: None,
        };
        assert!(opens(&e));
        e.name = "scene.ecprojx".into();
        assert!(opens(&e));
        e.name = "render.aep".into();
        e.mime = None;
        assert!(!opens(&e));
    }

    #[test]
    fn project_name_appends_extension() {
        assert_eq!(project_name("My Scene"), "My Scene.ecproj");
        assert_eq!(project_name("My Scene.ecproj"), "My Scene.ecproj");
        assert_eq!(project_name("a/b/c.ecproj"), "c.ecproj");
    }
}
