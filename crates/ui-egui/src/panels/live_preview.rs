//! Preview for the settings dialogs that edit something that already exists (Solid Settings on a
//! solid layer, Camera Settings, Light Settings, Composition Settings): After Effects' Preview
//! checkbox. Each change of the draft is applied to the project at once, so the canvas shows it
//! while it is made; OK keeps it and Cancel and Escape take it back.
//!
//! Every step goes back to the checkpoint taken when the dialog opened (the project and its undo
//! history) and then runs the dialog's command once with the draft's parameters. So the project is
//! always "as it opened, plus one command": OK leaves exactly one undo step, whatever was tried
//! meanwhile, and Cancel only has to put the checkpoint back. With Preview off nothing is applied
//! until OK, and switching it off takes back what was applied.
//!
//! While a preview is active the dialog opens at the lower right of the window, without the dark
//! backdrop, and the window behind it is not greyed out, so the canvas stays in view.

use egui::vec2;
use serde_json::Value;

use crate::panels::dialogs::Placement;
use crate::panels::layer_styles_dialog::{Checkpoint, checkpoint, restore};
use crate::{Dialog, EffectcraftApp};

/// Where a previewing dialog opens: the lower right, undimmed.
pub const PLACE: Placement = Placement { start: vec2(0.75, 0.8), dim: false };

/// The state of the one dialog being previewed (only one dialog is open at a time).
#[derive(Clone, Debug, Default)]
pub struct LivePreview {
    /// The dialog this belongs to; a preview left over from another one is ignored.
    dialog: Option<Dialog>,
    /// The Preview checkbox.
    pub on: bool,
    checkpoint: Option<Checkpoint>,
    /// The draft's parameters when the dialog opened: nothing is applied while it equals them.
    baseline: Option<Value>,
    /// The command and parameters the preview last applied.
    applied: Option<(&'static str, Value)>,
}

impl LivePreview {
    fn active_for(&self, dialog: Option<Dialog>) -> bool {
        self.checkpoint.is_some() && self.dialog.is_some() && self.dialog == dialog
    }
}

/// Start previewing for `dialog`, which has just opened on an existing item (Preview on).
pub fn begin(app: &mut EffectcraftApp, dialog: Dialog) {
    app.dialog_state.live = LivePreview { dialog: Some(dialog), on: true, checkpoint: Some(checkpoint(app)), ..Default::default() };
}

/// The dialog that is open is previewing.
pub fn active(app: &EffectcraftApp) -> bool {
    app.dialog_state.live.active_for(app.dialog)
}

/// Where the open dialog goes: the lower right, undimmed, while it previews; else the middle.
pub fn place(app: &EffectcraftApp) -> Placement {
    if active(app) { PLACE } else { Placement::default() }
}

/// Keep the project in step with the draft: `command` with `params` is what OK would run. The
/// first call (the draft as it opened) only notes it.
pub fn sync(app: &mut EffectcraftApp, command: &'static str, params: Value) {
    if !active(app) {
        return;
    }
    let live = &app.dialog_state.live;
    if live.baseline.is_none() {
        app.dialog_state.live.baseline = Some(params);
        return;
    }
    let want = live.on && live.baseline.as_ref() != Some(&params);
    let is_applied = live.applied.is_some();
    if want {
        if live.applied.as_ref().is_some_and(|(c, p)| *c == command && *p == params) {
            return;
        }
        if is_applied {
            put_back(app);
        }
        match app.session.execute(command, params.clone()) {
            Ok(_) => app.dialog_state.live.applied = Some((command, params)),
            Err(e) => app.ui.status = e.to_string(),
        }
    } else if is_applied {
        put_back(app);
    }
}

/// The project as the dialog found it.
fn put_back(app: &mut EffectcraftApp) {
    if let Some(cp) = app.dialog_state.live.checkpoint.clone() {
        restore(app, cp);
    }
    app.dialog_state.live.applied = None;
}

/// OK: the preview's result is the edit. Returns whether it already is (the caller then must not
/// run the command again); otherwise the caller runs it, as without a preview.
pub fn keep(app: &mut EffectcraftApp) -> bool {
    let kept = active(app) && app.dialog_state.live.applied.is_some();
    app.dialog_state.live = LivePreview::default();
    kept
}

/// Cancel and Escape: take back whatever the preview applied.
pub fn cancel(app: &mut EffectcraftApp) {
    if active(app) {
        put_back(app);
    }
    app.dialog_state.live = LivePreview::default();
}

/// The Preview checkbox, when the open dialog previews; `id` is its automation id.
pub fn checkbox(ui: &mut egui::Ui, auto: &mut crate::automation::Registry, on: &mut bool, id: &str) {
    let r = ui.checkbox(on, crate::i18n::tr("Preview"));
    auto.add(id, r.rect, "Preview");
}
