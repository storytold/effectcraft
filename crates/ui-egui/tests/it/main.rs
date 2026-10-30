//! The crate's integration tests, one binary (each module is a former `tests/<name>.rs`).

mod native_menu_text_keys;
mod render_queue_snapshot;
mod shared_docking_ui;
mod ui_3d;
mod ui_agent_audit;
mod ui_angles;
mod ui_audio_scrub;
mod ui_azerty_shortcuts;
mod ui_batch_links;
mod ui_batch_switches;
mod ui_blank_context_menus;
mod ui_camera_tracker;
mod ui_character_eyedropper;
mod ui_color_hex;
mod ui_comp_presets;
mod ui_control_modifiers;
mod ui_dialog_preview;
mod ui_dock_resize;
mod ui_drag_drop;
mod ui_effect_controls;
mod ui_effect_drop_highlight;
mod ui_effect_names;
mod ui_env_light_gpu;
mod ui_expression_values;
mod ui_font_search;
mod ui_form_fit;
mod ui_frame_step;
mod ui_fx_editors;
mod ui_gpu;
mod ui_gradient_editor;
mod ui_gradient_handles;
mod ui_gradient_value;
mod ui_graph_fit;
mod ui_group_rename;
mod ui_guide_layers;
mod ui_inline_edit;
mod ui_keyframes;
mod ui_large_project;
mod ui_layer_panel_zoom;
mod ui_layer_source_picker;
mod ui_light_settings;
mod ui_live_preview;
mod ui_locked_color;
mod ui_marker_names;
mod ui_media_browser_cache;
mod ui_media_browser_copy_path;
mod ui_menus;
mod ui_new_primitive;
mod ui_object_matte;
mod ui_panel_focus;
mod ui_panel_windows;
mod ui_panels;
mod ui_parameter_spacing;
mod ui_presets_folder;
mod ui_preview_align;
mod ui_project_delete;
mod ui_project_duplicate;
mod ui_project_keyboard;
mod ui_project_new_folder;
mod ui_project_rows;
mod ui_quick_offset;
mod ui_ram_preview;
mod ui_render_queue;
mod ui_script_console;
mod ui_scriptui;
mod ui_scrolling;
mod ui_settings_info;
mod ui_tabs;
mod ui_tail;
mod ui_text_edit;
mod ui_text_range_handles;
mod ui_theme_favorites;
mod ui_time_zoom_controls;
mod ui_timeline_effect_label;
mod ui_timeline_pan;
mod ui_timeline_shapes;
mod ui_timeline_switches;
mod ui_timeline_trim;
mod ui_timeline_zoom;
mod ui_toolbar_gradient;
mod ui_tracker;
mod ui_unreadable_footage;
mod ui_video_preview_off;
mod ui_viewer;
mod ui_viewer_trackpad;
mod ui_viewers;
mod ui_visual_review;
mod ui_web_gpu_storage;
mod ui_web_job_gpu;
mod ui_web_offload;
mod ui_workflow_selection;
mod ui_workspace_focus;
mod viewer_geometry;
mod viewer_passive_display;

/// Register the managed build's canonical craft-fonts checkout (`CRAFT_FONTS_DIR/fonts`) through
/// the same directory source production scans for `EFFECTCRAFT_FONT_DIRS`. The UI tests then render
/// real Japanese glyphs from the canonical external input instead of relying on host fonts.
pub(crate) fn add_canonical_test_fonts() {
    use effectcraft_text::fonts::{self, DirectorySource};
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let Some(root) = std::env::var_os("CRAFT_FONTS_DIR") else { return };
        let dir = std::path::PathBuf::from(root).join("fonts");
        if dir.is_dir() {
            fonts::add_source(&DirectorySource { dirs: vec![dir] });
        }
    });
}
