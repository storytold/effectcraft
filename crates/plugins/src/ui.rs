//! The plug-in windows: the permission prompt, the manager and plug-in panels.
//!
//! The windows live in `craft-plugin-ui` (`manager`), shared with EffectCraft. This module feeds
//! them EffectCraft's plug-in host and carries out the acts they return.
//!
//! Two entries. [`serve`] runs on every logic tick (`Hooks::per_tick`), also while the window is
//! hidden, and serves the host calls that other threads queued. [`frame`] runs once per painted
//! frame (`Hooks::per_frame`) and draws the windows.
//!
//! Known gap: these windows are not in the automation registry (`ui.elements`), because that
//! registry lives in `effectcraft-ui-egui` and the per-frame hook does not reach it. The same
//! actions are commands (`plugin.app.effectcraft.*`, see [`crate::commands`]) and the control
//! channel drives those.

use std::sync::Arc;

use craft_plugin_abi::manifest::Kind;
use craft_plugin_host::lifecycle::PluginState;
use craft_plugin_ui::manager::{Act, BrokenRow, ManagerView, PluginRow, draw, state_text};
use effectcraft_engine::Session;
use serde_json::json;

pub use craft_plugin_ui::manager::{FilePicker, UiState};

use crate::host::PluginHost;

impl PluginHost {
    /// Let the manager window browse for files.
    pub fn set_file_picker(&self, picker: FilePicker) {
        self.ui_state().set_picker(picker);
    }

    /// Ids of the plug-ins that wait for an answer to their permission prompt, oldest first.
    pub fn waiting_prompts(&self) -> Vec<String> {
        self.ui_state().waiting()
    }

    /// Name of the plug-in dialog that is open, for `ui.inspect`.
    pub fn dialog_label(&self) -> Option<String> {
        self.ui_state().dialog_label()
    }

    /// Open a panel window. Sends the `open` event and shows the tree the plug-in answers with.
    pub fn open_panel(&self, session: &mut Session, plugin_id: &str, contribution_id: &str) -> Result<(), String> {
        let surface = self
            .surfaces
            .find(plugin_id, contribution_id)
            .filter(|s| s.contribution.kind == Kind::Panel)
            .ok_or_else(|| format!("{plugin_id} has no panel {contribution_id}"))?;
        let title = surface.contribution.title.clone().or_else(|| surface.contribution.label.clone()).unwrap_or_else(|| contribution_id.to_owned());
        let answer = self.bridge.scoped(session, || surface.panel_event(&json!({ "event": "open" }))).map_err(|e| e.to_string());
        self.ui_state().open_panel_window(plugin_id, contribution_id, &title, answer);
        Ok(())
    }
}

/// Serve the host calls that other threads queued, and show the notices. Call on every logic
/// tick on the session thread, with the window hidden or not. A served call can change the
/// session, so it asks for a repaint.
pub fn serve(host: &PluginHost, ctx: &egui::Context, session: &mut Session) {
    if host.pump(session) > 0 {
        ctx.request_repaint();
    }
}

/// Draw the plug-in windows and carry out what the user did in them. Call once per painted frame
/// on the session thread.
pub fn frame(host: &Arc<PluginHost>, ctx: &egui::Context, session: &mut Session) {
    let acts = draw(ctx, &mut host.ui_state(), || manager_view(host));
    for act in acts {
        carry_out(host, session, act);
        ctx.request_repaint();
    }
}

/// What the manager window shows now. Built only while the window is open.
fn manager_view(host: &PluginHost) -> ManagerView {
    let ids = host.manager.list();
    let rows = ids
        .iter()
        .filter_map(|id| host.manager.info(id))
        .map(|info| PluginRow {
            progress: host.services.progress_of(&info.id).map(|p| format!("{:.0}% {}", p.fraction * 100.0, p.message.unwrap_or_default())),
            panels: info
                .surfaces
                .registered
                .iter()
                .filter(|s| host.surfaces.find(&info.id, s).is_some_and(|x| x.contribution.kind == Kind::Panel))
                .cloned()
                .collect(),
            info,
        })
        .collect();
    let broken = host
        .restored
        .iter()
        .filter(|(id, s)| matches!(s, PluginState::Faulted { .. }) && !ids.contains(id))
        .map(|(id, s)| BrokenRow { id: id.clone(), state: state_text(s) })
        .collect();
    ManagerView { app: "EffectCraft".into(), rows, broken }
}

fn carry_out(host: &Arc<PluginHost>, session: &mut Session, act: Act) {
    let say = |text: String| host.ui_state().message = Some(text);
    match act {
        Act::Answer { id, accept, optional } => {
            // `answer` records its own message.
            let _ = host.answer(&id, accept, optional);
        }
        Act::Install(path) => {
            if let Err(e) = host.install_path(std::path::Path::new(&path)) {
                say(format!("install failed: {e}"));
            }
        }
        Act::Enable(id) => match host.manager.enable(&id) {
            Ok(o) => say(format!("{id}: {o:?}")),
            Err(e) => say(format!("{id}: {e}")),
        },
        Act::Disable(id) => match host.manager.disable(&id) {
            Ok(()) => say(format!("{id}: disabled")),
            Err(e) => say(format!("{id}: {e}")),
        },
        Act::Remove(id) => match host.manager.remove(&id) {
            Ok(()) => say(format!("{id}: removed")),
            Err(e) => say(format!("{id}: {e}")),
        },
        Act::Revoke(id, p) => match host.manager.revoke(&id, &p) {
            Ok(()) => say(format!("{id}: revoked {p}")),
            Err(e) => say(format!("{id}: {e}")),
        },
        Act::OpenPanel(plugin, contribution) => {
            if let Err(e) = host.open_panel(session, &plugin, &contribution) {
                say(e);
            }
        }
        Act::Panel { plugin, contribution, event } => {
            let Some(surface) = host.surfaces.find(&plugin, &contribution) else { return };
            let answer = host.bridge.scoped(session, || surface.panel_event(&event)).map_err(|e| e.to_string());
            host.ui_state().apply_panel_answer(&plugin, &contribution, answer);
        }
        Act::ClosePanel { plugin, contribution } => {
            if let Some(surface) = host.surfaces.find(&plugin, &contribution) {
                // The plug-in is told it closed; its answer is not used.
                let _ = host.bridge.scoped(session, || surface.panel_event(&json!({ "event": "close" })));
            }
            host.ui_state().close_panel_window(&plugin, &contribution);
        }
    }
}
