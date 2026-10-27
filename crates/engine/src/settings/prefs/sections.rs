use super::choices::theme_family;
use super::*;

/// Unknown keys inside a page, kept so a newer version's settings survive a round trip.
pub(super) type Extra = BTreeMap<String, Value>;

macro_rules! page {
    ($(#[$m:meta])* $name:ident { $($(#[$fm:meta])* $f:ident : $t:ty = $d:expr),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        pub struct $name {
            $($(#[$fm])* pub $f: $t,)*
            #[serde(flatten)]
            pub extra: Extra,
        }
        impl Default for $name {
            fn default() -> Self {
                $name { $($f: $d,)* extra: Extra::new() }
            }
        }
    };
}

page!(General {
    /// Interface language: `system` (the operating system's, where EffectCraft has it, else
    /// English), or a language code from `LANGUAGES`.
    language: String = "system".into(),
    /// Levels of Undo (1–99).
    undo_levels: u32 = 32,
    /// Path Point and Handle Size (px).
    path_point_size: u32 = 5,
    show_tool_tips: bool = true,
    /// Create New Layers at Composition Start Time (off: at the current time).
    create_layers_at_comp_start: bool = true,
    switches_affect_nested_comps: bool = true,
    /// Default Spatial Interpolation to Linear.
    default_spatial_linear: bool = false,
    preserve_constant_vertex_count: bool = true,
    /// Synchronize Time of All Related Items.
    sync_time_related_items: bool = true,
    expression_pick_whip_compact: bool = true,
    /// Create Split Layers Above Original Layer (After Effects' default).
    create_split_layers_above: bool = true,
    use_system_color_picker: bool = false,
    /// Number of projects in File ▸ Open Recent.
    recent_items: u32 = 10,
});

page!(Startup {
    show_home_on_launch: bool = true,
    show_home_on_open_project: bool = false,
    /// After a crash, offer to open the most recent auto-save.
    offer_crash_recovery: bool = true,
    /// What the desktop window draws with, from the next launch: `auto` (the platform's best
    /// graphics API) or `gl` (OpenGL, for drivers that crash with the others). A launch that
    /// stops as its window opens switches it to `gl` (#243).
    window_graphics: String = "auto".into(),
});

page!(ProjectPrefs {
    /// Open a template project for File ▸ New ▸ New Project.
    use_template: bool = false,
    template_path: String = String::new(),
});

page!(AutoSave {
    enabled: bool = true,
    interval_minutes: u32 = 20,
    max_versions: u32 = 5,
    /// `nextToProject` (an "EffectCraft Auto-Save" folder beside the project) or `custom`.
    location: String = "nextToProject".into(),
    folder: String = String::new(),
    save_on_render_start: bool = false,
});

page!(CompositionPrefs {
    /// Motion path: `all`, `none`, `seconds`, `keyframes`.
    motion_path: String = "keyframes".into(),
    motion_path_seconds: f64 = 15.0,
    motion_path_keyframes: u32 = 15,
    show_rendering_progress: bool = true,
    hardware_accelerate_panels: bool = true,
});

page!(Previews {
    /// Largest downsampling while playing back: `1/2`, `1/4`, `1/8`.
    adaptive_resolution_limit: String = "1/8".into(),
    show_internal_wireframes: bool = false,
    cache_frames_when_idle: bool = false,
    fast_previews: bool = false,
    /// `faster` or `moreAccurate`.
    zoom_quality: String = "moreAccurate".into(),
    show_gpu_info: bool = false,
    /// The monitor's colour space for View ▸ Use Display Color Management: `srgb` or `p3`.
    display_profile: String = "srgb".into(),
});

page!(Appearance {
    /// The single theme before appearance modes (`dark`, `darker` or `light`), kept for older
    /// settings files and automation: setting it picks its family's mode and theme. While the mode
    /// is Dark or Light it mirrors the theme in use.
    theme: String = "dark".into(),
    /// `auto` (follow the operating system), `dark` or `light`. New installs stay Dark.
    appearance_mode: String = "dark".into(),
    /// The theme used in dark appearance: `dark` or `darker`.
    dark_theme: String = "dark".into(),
    /// The theme used in light appearance: `light`.
    light_theme: String = "light".into(),
    /// Theme shortcuts in the appearance context menu.
    favorite_themes: Vec<String> = Vec::new(),
    /// User interface brightness, -1 (darker) … 1 (lighter).
    brightness: f64 = 0.0,
    /// The size of the whole interface in percent (75–200), on top of the display's own scale.
    ui_scale: u32 = 100,
    use_label_color_for_handles: bool = true,
    use_label_color_for_tabs: bool = true,
    cycle_mask_colors: bool = true,
    use_gradients: bool = true,
    /// macOS: draw the menu bar inside the window instead of the system menu bar.
    in_window_menu_bar_mac: bool = false,
});

page!(Grids {
    grid_color: String = "#7a9cff".into(),
    grid_spacing: f64 = 100.0,
    grid_subdivisions: u32 = 4,
    /// `lines`, `dashed` or `dots`.
    grid_style: String = "lines".into(),
    proportional_horizontal: u32 = 4,
    proportional_vertical: u32 = 4,
    guide_color: String = "#3cc8f0".into(),
    guide_style: String = "lines".into(),
    action_safe: f64 = 10.0,
    title_safe: f64 = 20.0,
});

page!(TypePrefs {
    /// `latin` or `southAsian` (South Asian and Middle Eastern).
    text_engine: String = "latin".into(),
    font_preview: bool = true,
    recent_fonts: u32 = 10,
    font_names_in_english: bool = false,
});

page!(Import {
    /// Still footage duration: `compLength` or `seconds`.
    still_footage: String = "compLength".into(),
    still_seconds: f64 = 5.0,
    sequence_fps: f64 = 30.0,
    report_missing_frames: bool = true,
    /// Automatically Reload Footage whose file another app changed (size or modification time),
    /// checked every two seconds and when the window comes back to the front: `off`,
    /// `nonSequence` (the default, as in After Effects: image sequences are left alone) or `all`.
    auto_reload_footage: String = "nonSequence".into(),
    /// Interpret unlabeled alpha as: `ask`, `guess`, `ignore`, `straight`, `premultiplied`.
    unlabeled_alpha: String = "ask".into(),
    /// Default drag import as: `footage`, `comp`, `compLayerSizes`.
    drag_import_as: String = "footage".into(),
});

page!(Export {
    default_output_folder: String = String::new(),
    segment_sequences: bool = false,
    segment_sequence_files: u32 = 700,
    segment_movies: bool = false,
    segment_movie_mb: u32 = 1024,
    append_bits_to_name: bool = false,
});

page!(Audio {
    /// Output device name (empty = the system default).
    output_device: String = String::new(),
    /// Output mapping: device channel for the left / right output (1-based).
    output_left: u32 = 1,
    output_right: u32 = 2,
    preview_sample_rate: u32 = 48_000,
});

page!(Disk {
    disk_cache_enabled: bool = true,
    disk_cache_max_gb: u32 = 100,
    disk_cache_folder: String = String::new(),
    media_cache_folder: String = String::new(),
    conformed_media_folder: String = String::new(),
});

page!(Memory {
    ram_reserved_gb: u32 = 6,
    /// Processed-layer cache budget (MB).
    layer_cache_mb: u32 = 1024,
    /// Decoded footage frame cache budget (MB).
    media_cache_mb: u32 = 1024,
    /// RAM preview (viewer frame) cache budget (MB).
    preview_cache_mb: u32 = 3072,
    reduce_cache_when_low: bool = true,
});

page!(Video {
    enable_output: bool = false,
    device: String = String::new(),
    output_during_playback: bool = true,
    mirror_on_monitor: bool = true,
    disable_when_background: bool = false,
});

page!(ThreeD {
    /// Default 3D renderer for new compositions: `classic` or `advanced`.
    default_renderer: String = "classic".into(),
    show_reference_axes: bool = true,
    extended_viewer: bool = false,
    realtime_shadows: bool = true,
});

page!(
    /// Settings ▸ Roto Brush.
    RotoPrefs {
        /// The segmentation model Roto Brush 2.0 / 3.0 use: `classical` (built in) or a model id
        /// from `effectcraft_segment::MODELS` (once installed).
        model: String = "classical".into(),
    }
);

page!(
    /// Settings ▸ Face Tracking.
    FacePrefs {
        /// The model Track Mask ▸ Face Tracking uses: `classical` (built in) or a model id from
        /// `effectcraft_segment::MODELS` (once installed).
        model: String = "classical".into(),
    }
);

page!(Scripting {
    allow_scripts_write_files: bool = false,
    warn_executing_files: bool = true,
    enable_js_debugger: bool = false,
    editor_font_size: u32 = 13,
    syntax_highlighting: bool = true,
    line_numbers: bool = true,
    auto_complete: bool = true,
    bracket_matching: bool = true,
    word_wrap: bool = false,
    error_banner: bool = true,
});

impl Scripting {
    /// Whether `other` grants scripts the same as this: Application Scripting (file and network
    /// access, the warning before executing files, the debugger). Scripts can't change these
    /// ([`crate::Session::execute_for_script`]): a script could otherwise give itself the
    /// access the user withheld.
    pub fn same_permissions(&self, other: &Scripting) -> bool {
        self.allow_scripts_write_files == other.allow_scripts_write_files
            && self.warn_executing_files == other.warn_executing_files
            && self.enable_js_debugger == other.enable_js_debugger
    }

    /// Take `from`'s Application Scripting settings (see [`Scripting::same_permissions`]).
    pub fn keep_permissions(&mut self, from: &Scripting) {
        self.allow_scripts_write_files = from.allow_scripts_write_files;
        self.warn_executing_files = from.warn_executing_files;
        self.enable_js_debugger = from.enable_js_debugger;
    }
}

/// One editable label: name and `#rrggbb` colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelPref {
    pub name: String,
    pub color: String,
}

/// A Composition Settings preset the user saved (Composition Settings ▸ Preset ▸ Save): the
/// frame size, pixel aspect and frame rate, as in After Effects' own presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompPreset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: f64,
    pub frame_rate: f64,
}

impl CompPreset {
    /// Longest preset name kept.
    pub const MAX_NAME: usize = 128;

    /// Values a composition accepts (a hand-edited settings file can hold anything).
    pub fn is_valid(&self) -> bool {
        let size = 4..=30000;
        !self.name.trim().is_empty()
            && self.name.chars().count() <= Self::MAX_NAME
            && size.contains(&self.width)
            && size.contains(&self.height)
            && self.pixel_aspect.is_finite()
            && (0.01..=100.0).contains(&self.pixel_aspect)
            && self.frame_rate.is_finite()
            && (1.0..=999.0).contains(&self.frame_rate)
    }
}

/// Saved composition presets, skipping entries that don't parse instead of losing the list.
pub(super) fn lenient_comp_presets<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<CompPreset>, D::Error> {
    let v = Vec::<Value>::deserialize(d)?;
    Ok(v.into_iter().filter_map(|x| serde_json::from_value(x).ok()).collect())
}

pub(super) fn default_labels() -> Vec<LabelPref> {
    Label::ALL.iter().skip(1).map(|l| LabelPref { name: l.name().to_string(), color: hex(l.rgb()) }).collect()
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

pub(super) fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

impl Appearance {
    /// Show the legacy single `theme`: fix the mode to its family and remember it as that
    /// family's theme. An unknown theme changes nothing.
    pub fn select_theme(&mut self) {
        match theme_family(&self.theme) {
            Some((mode, "darkTheme")) => {
                self.appearance_mode = mode.into();
                self.dark_theme = self.theme.clone();
            }
            Some((mode, _)) => {
                self.appearance_mode = mode.into();
                self.light_theme = self.theme.clone();
            }
            None => {}
        }
    }

    /// The theme to show: the light or dark family's choice by the mode; Auto follows `system_light`
    /// (`None` = the system said nothing: dark).
    pub fn resolved_theme(&self, system_light: Option<bool>) -> &str {
        let light = match self.appearance_mode.as_str() {
            "light" => true,
            "auto" => system_light == Some(true),
            _ => false,
        };
        if light { &self.light_theme } else { &self.dark_theme }
    }
}
