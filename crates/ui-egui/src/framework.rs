//! The craft-ui runtime, hosted from EffectCraft's own eframe `App` (craft-libs docs/craft-framework.md, section 2.2).
//! The desktop app calls [`install`] once, [`EffectcraftApp::before_input`] in `raw_input_hook`,
//! [`EffectcraftApp::close_path`] last in `logic`, and [`before_ui`] first in `ui`; the browser build and tests
//! install with [`install_headless`]. The framework dialogs run on [`EffectcraftApp::framework_dialogs`] after
//! EffectCraft's own UI.
//!
//! The runtime takes Escape, Tab and the clipboard events for the whole window unless one of EffectCraft's own egui
//! widgets has the keyboard. [`return_window_keys`] puts the window's Escape back into egui's input for EffectCraft's
//! own handlers (the window's plain Enter stays in egui's input); the Composition viewer is declared to the runtime with [`host_canvas`], which keeps
//! the edit chords and the clipboard events in egui's input for EffectCraft's shortcuts.

use std::sync::{Arc, OnceLock};

use craft_host::target::ShellInfo;
use craft_host::{AppInfo, Host};
use craft_keymap::Overrides;
use craft_ui::fonts::{I18nScripts, SystemFonts, UiFonts};
use craft_ui::runtime::commands::{CommandInput, CommandSet as _, NavCommand, QuitCommand};
use craft_ui::runtime::env::{RootEnv, Scale};
use craft_ui::runtime::focus::{EditSet, KeyUse};
use craft_ui::runtime::frame::{FrameInput, FrameOutput};
use craft_ui::runtime::keys::{Intent, with_window_intents};
use craft_ui::shell::embed::{self, CloseDecision};
use craft_window::system::WindowSystem;
use egui::ViewportId;

use crate::EffectcraftApp;

/// Each pass of the root window, first in `App::ui`: hands the runtime the native window it answers the hit test of
/// (Windows) and fits the window to its monitor once.
#[cfg(not(target_arch = "wasm32"))]
pub use craft_ui::shell::embed::before_ui;

/// The id every OS window of the app carries, as the desktop entry and the update app name it.
const APP_ID: &str = "ai.storyteller.effectcraft";

/// The scripts of the interface languages, for the font stack.
static SCRIPTS: I18nScripts = I18nScripts;

/// The runtime's first font stack: egui's bundled faces and no installed system fonts. EffectCraft's own definitions
/// ([`crate::theme::install`]) replace it before anything is drawn.
fn ui_fonts() -> UiFonts<'static> {
    UiFonts::new(&SCRIPTS).system_fonts(SystemFonts::Off)
}

fn install_with(ctx: &egui::Context, shell: ShellInfo, windows: Arc<dyn WindowSystem>) -> Result<(), String> {
    let spec = craft_ui::InstallSpec::new(shell, windows).app_id(APP_ID);
    embed::install(ctx, spec, &ui_fonts()).map(drop).map_err(|error| format!("the UI runtime could not be installed: {error}"))
}

/// Installs the runtime on the desktop window's context, once, before the first pass. EffectCraft keeps the system
/// title bar, so the window gets no frame decision.
#[cfg(not(target_arch = "wasm32"))]
pub fn install(ctx: &egui::Context) -> Result<(), String> {
    let shell = ShellInfo::default().backend(craft_host::target::Backend::from_env());
    install_with(ctx, shell, Arc::new(craft_window::system::SystemWindows))
}

/// Installs the runtime on a context with no native window of its own: the browser build's canvas and test windows.
pub fn install_headless(ctx: &egui::Context) {
    if let Err(error) = install_with(ctx, ShellInfo::default(), Arc::new(craft_window::system::NoWindows)) {
        log::warn!("{error}");
    }
}

/// Who EffectCraft is to the framework's dialogs (the update dialogs show its name and icon).
pub(crate) fn host() -> Arc<Host> {
    Arc::new(
        Host::builder(AppInfo { id: "effectcraft", name: "EffectCraft", version: env!("CARGO_PKG_VERSION") })
            .icon_png(include_bytes!("../../../assets/app-icon/hicolor/128x128/apps/ai.storyteller.effectcraft.png"))
            .build(),
    )
}

/// EffectCraft's own shortcut table keeps the keys the framework binds by default: Cmd+Q is `app.quit`, and F6 is
/// free for the user to bind there, so the framework's quit and pane moves are unbound. Overrides are keyed by command
/// id, which the command sets give.
fn command_overrides() -> Arc<Overrides> {
    static OVERRIDES: OnceLock<Arc<Overrides>> = OnceLock::new();
    OVERRIDES
        .get_or_init(|| {
            let mut overrides = Overrides::new();
            for id in [QuitCommand::Quit.id(), NavCommand::NextPane.id(), NavCommand::PreviousPane.id()] {
                overrides.insert(id, Vec::new());
            }
            Arc::new(overrides)
        })
        .clone()
}

impl EffectcraftApp {
    /// The framework's part of `App::raw_input_hook`, once per frame before the first pass: takes what the runtime
    /// gathered since the last frame and gives the coming passes the root values (theme mode and interface scale).
    pub fn before_input(&mut self, ctx: &egui::Context) {
        embed::before_input(ctx, |output| self.frame_input(ctx, output));
    }

    /// The root values of this frame. EffectCraft registers no commands with the runtime (its own shortcut table runs
    /// every key, [`command_overrides`]) and lets it toggle no chrome. The one output it acts on is the clipboard
    /// chords of a window with no canvas ([`clipboard_events`]). The
    /// interface scale is Settings > Appearance > UI Scale; the runtime writes it to the context. While one of
    /// EffectCraft's own dialogs or a command's file dialog is up, the runtime blocks the panels floated into windows
    /// of their own.
    pub(crate) fn frame_input(&mut self, ctx: &egui::Context, output: FrameOutput) -> FrameInput {
        self.forwarded.clips.extend(clipboard_events(&output));
        let mode = if self.tokens.kind.is_light() { craft_theme::Mode::Light } else { craft_theme::Mode::Dark };
        let scale = self.session.prefs.appearance.ui_scale as f32 / 100.0;
        let root = RootEnv::new().mode(mode).scale(Scale::new(scale));
        let app_modal = self.floats_blocked(ctx).then_some(ViewportId::ROOT);
        FrameInput::new(root).commands(CommandInput::new().overrides(command_overrides())).host(Arc::clone(&self.framework_host)).app_modal(app_modal)
    }

    /// The framework's part of `App::logic`, after EffectCraft's own: the root window's close path
    /// ([`crate::panels::unsaved::close_decision`]).
    pub fn close_path(&mut self, ctx: &egui::Context) {
        embed::logic(ctx, || crate::panels::unsaved::close_decision(self));
    }
}

/// The Copy, Cut and Paste the runtime took in a window with no canvas, as the clipboard events that window's chord
/// would have been. Only a command a key press fired, and a Paste only with the platform's content, which a key press
/// never carries (so a pasted key press cannot run twice). A Copy or Cut from a synthetic key press that egui kept
/// as well runs twice. Native only: the web sends the key press with the clipboard event.
#[cfg(not(target_arch = "wasm32"))]
fn clipboard_events(output: &FrameOutput) -> Vec<egui::Event> {
    use craft_ui::runtime::commands::{EditCommands, FirePhase, FireSource};
    output
        .fired
        .iter()
        .filter(|fired| fired.source == FireSource::Key && fired.phase == FirePhase::Press && fired.target.is_none())
        .filter_map(|fired| match fired.command.id.as_str() {
            id if id == EditCommands::Copy.id() => Some(egui::Event::Copy),
            id if id == EditCommands::Cut.id() => Some(egui::Event::Cut),
            id if id == EditCommands::Paste.id() => fired.pasted.as_ref()?.text.as_ref().map(|text| egui::Event::Paste(text.to_string())),
            _ => None,
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn clipboard_events(_: &FrameOutput) -> Vec<egui::Event> {
    Vec::new()
}

/// The Composition viewer's widget, its window's document canvas.
pub(crate) fn viewer_id() -> egui::Id {
    egui::Id::unique("viewer-interact")
}

/// Declares the Composition viewer, drawn in this pass, as the window's document canvas. With nothing focused it
/// is the window's key target: the edit chords and the clipboard events stay in
/// egui's input for EffectCraft's shortcuts. The focused viewer keeps its focus at Escape, which EffectCraft's own
/// handlers read from egui.
pub(crate) fn host_canvas(ctx: &egui::Context) {
    const KEYS: KeyUse = KeyUse::CANVAS.with_edits(EditSet::ALL);
    craft_ui::runtime::focus::host_canvas(ctx, viewer_id(), KEYS);
}

/// The widget other than the Composition viewer that has the keyboard. The viewer as the focused Tab stop leaves the
/// keys to EffectCraft, as with nothing focused.
pub(crate) fn focused_widget(ctx: &egui::Context) -> Option<egui::Id> {
    ctx.memory(|m| m.focused()).filter(|id| *id != viewer_id())
}

/// Whether a widget other than the Composition viewer has the keyboard ([`focused_widget`]).
pub(crate) fn widget_has_keyboard(ctx: &egui::Context) -> bool {
    focused_widget(ctx).is_some()
}

/// Puts the window's Escape (the key the runtime routes to no component while nothing has framework focus) back into
/// egui's input as a key press, first in the pass, so EffectCraft's dialogs, popups, tools and shortcuts read it from
/// egui as they read every other key. The window's plain Enter is its Submit and also stays in egui's input, where
/// EffectCraft reads it.
pub(crate) fn return_window_keys(ctx: &egui::Context) {
    if take_window_intent(ctx, Intent::Cancel) {
        ctx.input_mut(|i| {
            let modifiers = i.modifiers;
            i.events.insert(0, egui::Event::Key { key: egui::Key::Escape, physical_key: Some(egui::Key::Escape), pressed: true, repeat: false, modifiers });
        });
    }
}

/// Takes the window's `intent` when it is there: no later reader sees it.
fn take_window_intent(ctx: &egui::Context, intent: Intent) -> bool {
    let mut taken = false;
    with_window_intents(ctx, false, |intents| {
        taken = intents.take(intent.clone());
        taken
    });
    taken
}

/// Whether the close decision is `Close`.
pub(crate) fn decide(close: bool) -> CloseDecision {
    if close { CloseDecision::Close } else { CloseDecision::Hold }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ui::runtime::commands::{ClipContent, CommandRef, FirePhase, FireSource, Fired};

    fn fired(id: &str, pasted: Option<&str>) -> Fired {
        let mut fired = Fired::new(CommandRef::new(id), FireSource::Key, FirePhase::Press, 0.0);
        fired.pasted = pasted.map(|text| {
            let mut content = ClipContent::default();
            content.text = Some(text.into());
            content
        });
        fired
    }

    /// Copy pressed in a window with no canvas reaches the shortcuts once; a Paste with no content of its own is a key
    /// press the app already reads, and so is any other edit command.
    #[test]
    fn clipboard_chords_of_a_window_with_no_canvas_queue_once() {
        let mut output = FrameOutput::default();
        output.fired = vec![fired("edit.copy", None), fired("edit.paste", None), fired("edit.undo", None), fired("edit.paste", Some("x"))];
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        app.frame_input(&ctx, output);
        assert_eq!(app.forwarded.clips, [egui::Event::Copy, egui::Event::Paste("x".into())]);
    }
}
