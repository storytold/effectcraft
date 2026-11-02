//! Menus, keyboard shortcuts and UI-level commands. The menu bar is the engine's After Effects
//! menu tree (`effectcraft_engine::menus`): every entry is an engine command, and frontend-only
//! commands (viewer zoom, panels, dialogs…) come back as `Event::Frontend` and are performed by
//! [`frontend`]. [`UI_COMMANDS`] holds the remaining UI-only shortcuts (tools, timeline
//! navigation, reveal keys). `invoke` is the single entry point used by menus, shortcuts and the
//! control channel.

use serde_json::{Value, json};

use crate::EffectcraftApp;
use crate::dock::{PanelKind, WorkspaceExt as _};
use crate::state::{Resolution, Tool};
use effectcraft_engine::menus::{MenuEntry, MenuNode};

mod bar;
mod commands;
mod entries;
mod frontend;
mod invoke;
mod shortcuts;

pub use bar::menu_bar;
pub(crate) use bar::{dyn_ctx, entry_for, menu_contents, submenu_at};
pub use commands::UI_COMMANDS;
pub use entries::{MenuItem, menu_items, menus};
pub(crate) use entries::{entry_checked, entry_enabled, entry_label, entry_shortcut, menu_choice, panel_shown};
pub use frontend::frontend;
#[cfg(test)]
use frontend::zoom_step;
pub(crate) use frontend::{dialog_answered, run_engine};
pub use invoke::{invoke, panel_command_id, reveal};
#[cfg(test)]
use shortcuts::modifiers_at_start;
pub use shortcuts::{Binding, bindings, chord_matches, forwardable_during_text_edit, handle_shortcuts, handle_tab_shortcut, parse_shortcut, shortcut_text};

pub struct UiCommand {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
}

#[cfg(test)]
mod tests;
