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

/// Drop the cloud save target after a local command replaces the open project.
pub fn clear_origin_after_replace(app: &mut EffectcraftApp, command: &str) {
    const REPLACES: &[&str] = &["file.newProject", "file.open", "file.openRecent", "file.openDemoProject", "file.revert"];
    if !REPLACES.contains(&command) {
        return;
    }
    if let Some(uid) = CloudApp::active(app) {
        app.cloud_mut().origins.remove(&uid);
    }
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
        let is_json = into.mime.is_some_and(|mime| {
            mime.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json")
        });
        if !ext.is_empty() && ext != "ecproj" && ext != "ecprojx" && !is_json {
            return Err(format!("EffectCraft can only save projects as .ecproj or .ecprojx (got .{ext})"));
        }
        let result = self
            .session
            .execute("file.toJson", json!({"xml": ext == "ecprojx"}))
            .map_err(|e| e.to_string())?;
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
        } else if let Some(ctx) = self.frames.context() {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
    }

    fn say(&mut self, text: String) {
        self.ui.status = text;
    }

    fn toast(&mut self, text: String) {
        let now = self.frame_time();
        self.toast = Some((text, now));
    }

    fn go_to_save_as(&mut self) {
        self.cloud_page = Some(CloudPage::SaveAs);
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

/// Which cloud page the File menu opened, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudPage {
    Open,
    SaveAs,
}

/// Open / Save As pages as a modal from File ▸ Open from cloud… / Save as to cloud….
pub fn modal(app: &mut EffectcraftApp, ctx: &egui::Context) {
    let Some(page) = app.cloud_page else { return };
    let title = match page {
        CloudPage::Open => "Open from cloud",
        CloudPage::SaveAs => "Save as to cloud",
    };
    let mut close = false;
    egui::Modal::new(egui::Id::new("effectcraft_cloud_page")).show(ctx, |ui| {
        let margin = egui::Frame::popup(ui.style()).total_margin().sum();
        let room = (ctx.content_rect().size() - margin - egui::vec2(16.0, 16.0)).max(egui::Vec2::splat(1.0));
        let width = 680.0_f32.min(room.x);
        ui.set_min_width(360.0_f32.min(width));
        ui.set_max_width(width);
        ui.label(egui::RichText::new(title).strong().size(16.0));
        ui.add_space(10.0);
        let body = (room.y - ui.min_size().y - 48.0).max(1.0);
        egui::ScrollArea::vertical().id_salt(("effectcraft_cloud_page", page == CloudPage::SaveAs)).max_height(body).show(ui, |ui| match page {
            CloudPage::Open => open_page(app, ui),
            CloudPage::SaveAs => {
                close = save_page(app, ui);
            }
        });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        });
    });
    if close {
        app.cloud_page = None;
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

    /// `start` must submit `Job::Account` the first time it is called once a
    /// backend is connected, and must not submit a second one when called again
    /// in the same run (idempotent bootstrap).
    ///
    /// This is a regression for the web build where `start` was not called at
    /// all after wiring the backend, leaving the account controls blank.
    #[test]
    fn start_submits_account_bootstrap_exactly_once() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        struct Cap(Arc<AtomicU32>);
        impl CloudBackend for Cap {
            fn submit(&self, job: Job, _post: Post) {
                if matches!(job, Job::Account) {
                    self.0.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let count = Arc::new(AtomicU32::new(0));
        let mut app = crate::EffectcraftApp::new(effectcraft_engine::Session::default());
        app.cloud.connect(Cap(count.clone()));
        start(&mut app);
        assert_eq!(count.load(Ordering::Relaxed), 1, "start must submit Job::Account once");
        // Second call in the same run: no duplicate bootstrap.
        start(&mut app);
        assert_eq!(count.load(Ordering::Relaxed), 1, "start is idempotent: no second Account job");
    }

    /// Writes File-menu, Open, and Save As PNGs when `EC_CLOUD_SHOTS` is a directory.
    /// The account and folder are a fixture, so the pages are the real window and not a sign-in prompt.
    #[test]
    fn collabcraft_pages_screenshot() {
        let Some(dir) = std::env::var_os("EC_CLOUD_SHOTS") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();

        struct Quiet;
        impl CloudBackend for Quiet {
            fn submit(&self, _job: Job, _post: Post) {}
        }
        fn entry(name: &str, kind: EntryKind, shared_by: Option<&str>) -> Entry {
            Entry {
                id: name.into(),
                room: format!("room-{name}"),
                name: name.into(),
                path: "/Projects".into(),
                kind,
                size: if kind == EntryKind::Folder { None } else { Some(48_112) },
                modified: Some("2026-10-16T14:00:00Z".into()),
                mime: if kind == EntryKind::File { Some("application/json".into()) } else { None },
                etag: Some("1".into()),
                writable: shared_by.is_none(),
                shared_by: shared_by.map(str::to_owned),
            }
        }

        let mut session = effectcraft_engine::Session::default();
        let _ = session.execute("file.openDemoProject", serde_json::json!({}));
        let mut app = crate::EffectcraftApp::new(session);
        app.cloud.connect(Quiet);
        app.cloud.account = Some(Account {
            display: "Ada".into(),
            capabilities: vec![Capability::Files, Capability::SharedWithMe, Capability::Sharing, Capability::Etags],
            provider_linked: true,
        });
        app.cloud.mine.path = "/Projects".into();
        app.cloud.mine.loaded = true;
        app.cloud.mine.entries =
            vec![entry("Titles", EntryKind::Folder, None), entry("Cold Open.ecproj", EntryKind::File, None), entry("End Card.ecproj", EntryKind::File, None)];
        app.cloud.shared.loaded = true;
        app.cloud.shared.entries = vec![entry("Review Cut.ecproj", EntryKind::File, Some("Sam"))];
        app.cloud.save_name = "Cold Open.ecproj".into();
        app.cloud.status = Some("CollabCraft".into());

        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|_| app);
        for _ in 0..12 {
            h.step();
        }
        let file = h.state().auto.find("menu.File").expect("File menu");
        let at = egui::pos2(file.rect[0] + file.rect[2] / 2.0, file.rect[1] + file.rect[3] / 2.0);
        h.input_mut().events.push(egui::Event::PointerMoved(at));
        h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        h.step();
        h.input_mut().events.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
        h.run_steps(3);
        let menu = h.render().expect("file menu");
        menu.save(dir.join("file-menu.png")).unwrap();

        h.state_mut().cloud_page = Some(CloudPage::Open);
        h.run_steps(4);
        h.render().expect("open").save(dir.join("open.png")).unwrap();

        h.state_mut().cloud_page = Some(CloudPage::SaveAs);
        h.run_steps(4);
        h.render().expect("save").save(dir.join("save.png")).unwrap();
    }
}
