//! The command registry. Ids follow After Effects' menu structure; every menu item, panel button,
//! shortcut and viewer/timeline gesture maps to one of these.

mod align;
mod align_data;
mod anim;
mod anim_presets;
pub mod anim_tools;
mod animation;
pub(crate) mod app_more;
mod autotrace;
mod batch;
mod camera_cmds;
mod comp;
pub(crate) mod comp_more;
pub mod content_fill;
mod create;
mod edit;
mod effect;
pub mod essential;
pub mod expr_tools;
mod file;
pub(crate) mod file_more;
mod focus;
pub mod footage_panel;
pub(crate) mod frame_export;
mod frontend;
mod help;
mod import_project;
mod key_labels;
mod key_transform;
mod keys_more;
mod layer;
mod layer_menu;
mod layer_time;
pub(crate) mod link;
mod liquify;
mod lottie;
pub mod markers;
mod mask;
pub mod mask_interp;
pub(crate) mod model3d;
mod nav_more;
pub mod paint;
pub mod panels_cmds;
pub(crate) mod path_nulls;
mod paths;
mod project_items;
pub(crate) mod prop;
mod prop_groups;
mod proxy;
pub mod puppet;
mod query;
mod quick_offset;
mod relink;
pub(crate) mod relink_moved;
mod render_queue;
pub(crate) mod rig3d;
pub mod roto_cmds;
mod scene_detect;
pub mod scripts;
mod settings;
pub mod shape_stroke;
pub mod shape_tool;
mod stubs;
mod styles;
mod text_anim;
pub mod text_edit;
mod three_d;
pub mod time;
mod timeline;
mod track;
mod view;
pub mod viewer_cmds;
pub(crate) mod vpe;
pub(crate) mod vr_editor;
mod warp_cmds;
pub(crate) mod watch_folder;
#[cfg(test)]
pub(crate) use app_more::report_text as report_text_for_tests;
pub(crate) use app_more::view_command;
#[cfg(test)]
pub(crate) use file::open as open_for_tests;
#[cfg(test)]
pub(crate) use keys_more::amplitudes as amplitudes_for_tests;
#[cfg(test)]
pub(crate) use mask::split_segment as split_segment_for_tests;

use std::sync::OnceLock;

use effectcraft_project::{Comp, ItemId, Layer, LayerId};
use effectcraft_time::Tick;
use serde_json::Value;

use crate::{EngineError, Result, Session};

pub type Run = fn(&mut Session, &Value) -> Result<Value>;
pub type Enabled = fn(&Session) -> std::result::Result<(), String>;

/// Metadata + implementation for one command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu placement, e.g. `["Layer", "New"]`. Empty = not in menus.
    pub menu: &'static [&'static str],
    /// Default shortcut (egui-style names, `Cmd` = ⌘ on macOS / Ctrl elsewhere).
    pub shortcut: Option<&'static str>,
    /// Further default shortcuts (the menus and tooltips show `shortcut`).
    pub alt_shortcuts: &'static [&'static str],
    /// Parameter description for agents and the MCP schema.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    /// Record in the journal (false for pure queries).
    pub journal: bool,
}

#[macro_export]
macro_rules! cmd {
    // Several default shortcuts: `["PageDown", "Cmd+ArrowRight"]`.
    ($id:literal, $label:literal, [$($m:literal),*], [$sc:literal, $($alt:literal),+], $params:literal, $en:expr, $run:expr) => {
        $crate::commands::CommandSpec { alt_shortcuts: &[$($alt),+], ..$crate::cmd!($id, $label, [$($m),*], Some($sc), $params, $en, $run) }
    };
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::commands::CommandSpec {
            id: $id,
            label: $label,
            menu: &[$($m),*],
            shortcut: $sc,
            alt_shortcuts: &[],
            params: $params,
            enabled: $en,
            run: $run,
            journal: true,
        }
    };
}
#[macro_export]
macro_rules! query {
    ($id:literal, $label:literal, $params:literal, $run:expr) => {
        $crate::commands::CommandSpec {
            id: $id,
            label: $label,
            menu: &[],
            shortcut: None,
            alt_shortcuts: &[],
            params: $params,
            enabled: $crate::commands::always,
            run: $run,
            journal: false,
        }
    };
}

mod enabled;
mod params;
mod schema;

use super::*;
pub use enabled::always;
pub(crate) use enabled::{has_comp, has_keys, has_layers, has_project_selection, not_yet, not_yet_run};
pub use params::match_path_of;
pub(crate) use params::{
    b_p, bad, comp_id, f_p, frontend, layer_mut, layer_p, layers_p, merge_p, resolve_effect_path, resolve_layer, selectable, selected_leaf_props, str_p,
    time_p, unlocked, visible_p,
};
pub use schema::{accepted_params, check_params, params_schema};

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: OnceLock<Vec<CommandSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(crate::models::specs());
        v.extend(edit::specs());
        v.extend(comp::specs());
        v.extend(layer::specs());
        v.extend(styles::specs());
        v.extend(three_d::specs());
        v.extend(model3d::specs());
        v.extend(rig3d::specs());
        v.extend(text_anim::specs());
        v.extend(text_edit::specs());
        v.extend(layer_time::specs());
        v.extend(prop::specs());
        v.extend(prop_groups::specs());
        v.extend(project_items::specs());
        v.extend(anim::specs());
        v.extend(anim_tools::specs());
        v.extend(link::specs());
        v.extend(markers::specs());
        v.extend(mask::specs());
        v.extend(paths::specs());
        v.extend(effect::specs());
        v.extend(time::specs());
        v.extend(nav_more::specs());
        v.extend(render_queue::specs());
        v.extend(help::specs());
        v.extend(query::specs());
        v.extend(batch::specs());
        v.extend(layer_menu::specs());
        v.extend(create::specs());
        v.extend(animation::specs());
        v.extend(anim_presets::specs());
        v.extend(view::specs());
        v.extend(viewer_cmds::specs());
        v.extend(shape_stroke::specs());
        v.extend(shape_tool::specs());
        v.extend(focus::specs());
        v.extend(key_labels::specs());
        v.extend(key_transform::specs());
        v.extend(quick_offset::specs());
        v.extend(file_more::specs());
        v.extend(relink::specs());
        v.extend(relink_moved::specs());
        v.extend(lottie::specs());
        v.extend(import_project::specs());
        v.extend(timeline::specs());
        v.extend(comp_more::specs());
        v.extend(frame_export::specs());
        v.extend(watch_folder::specs());
        v.extend(vpe::specs());
        v.extend(path_nulls::specs());
        v.extend(vr_editor::specs());
        v.extend(frontend::specs());
        v.extend(track::specs());
        v.extend(mask_interp::specs());
        v.extend(warp_cmds::specs());
        v.extend(camera_cmds::specs());
        v.extend(roto_cmds::specs());
        v.extend(paint::specs());
        v.extend(puppet::specs());
        v.extend(liquify::specs());
        v.extend(settings::specs());
        v.extend(app_more::specs());
        v.extend(keys_more::specs());
        v.extend(essential::specs());
        v.extend(expr_tools::specs());
        v.extend(proxy::specs());
        v.extend(scripts::specs());
        v.extend(panels_cmds::specs());
        v.extend(footage_panel::specs());
        v.extend(autotrace::specs());
        v.extend(scene_detect::specs());
        v.extend(align_data::specs());
        v.extend(align::specs());
        v.extend(content_fill::specs());
        v.extend(stubs::specs());
        v.extend(crate::storage::specs());
        v.extend(crate::learn::specs());
        v.extend(crate::templates::specs());
        v.extend(crate::preview::specs());
        v
    })
}

/// Set `footage` as the proxy of `item` (File ▸ Create Proxy's post-render action).
pub(crate) fn proxy_set(s: &mut Session, item: ItemId, footage: effectcraft_project::Footage) -> Result<()> {
    proxy::set_proxy_footage(s, &[item], footage)
}

/// (id, name) of the text animation presets.
pub fn text_preset_list() -> Vec<(String, String)> {
    text_anim::preset_names()
}

/// (id, name, category) of the built-in animation presets (Transitions…).
pub fn builtin_preset_list() -> Vec<(String, String, String)> {
    anim_presets::preset_names()
}

pub fn find(id: &str) -> Option<&'static CommandSpec> {
    // Every command execution looks its spec up: index the ~2,000 ids once.
    static INDEX: OnceLock<std::collections::HashMap<&'static str, usize>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        for (i, c) in command_specs().iter().enumerate() {
            m.entry(c.id).or_insert(i);
        }
        m
    });
    index.get(id).map(|&i| &command_specs()[i])
}

/// Font families available to text layers (bundled + scanned system fonts).
/// Text animation presets: (id, name), for menus.
pub fn text_presets() -> Vec<(String, String)> {
    commands::text_preset_list()
}

/// Built-in animation presets (Effects & Presets ▸ * Animation Presets): (id, name, category).
pub fn animation_presets() -> Vec<(String, String, String)> {
    commands::builtin_preset_list()
}
