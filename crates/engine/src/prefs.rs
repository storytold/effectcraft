//! Settings (After Effects' Preferences): a versioned serde model, addressed by dotted keys
//! (`general.undoLevels`, `autoSave.intervalMinutes`, `labels.3.name`), plus the page schema the
//! Settings dialog and agents (`prefs.pages`) are built from.
//!
//! Settings are stored as JSON (`prefs.json`) through the session's [`crate::config::ConfigStore`].
//! Loading is forgiving: missing keys take their defaults, unknown keys (written by a newer
//! version) are kept and written back, and older layouts are migrated ([`migrate`]).
//!
//! Every setting in [`pages`] says whether it already changes behaviour (`live`). Settings that
//! don't yet are still shown (After Effects parity of the dialog) and are listed in
//! `docs/preferences.md`; a test keeps that list in step with the schema.

use std::cell::Cell;
use std::collections::BTreeMap;

use effectcraft_color::Label;
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Current settings layout version.
pub const PREFS_VERSION: u32 = 2;
/// Settings file name in the config store.
pub const PREFS_FILE: &str = "prefs.json";

thread_local! {
    /// True while an engine command runs on behalf of a script (which stays on this thread,
    /// through nested commands such as `engine.batch`).
    static SCRIPT_ORIGIN: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` as a script's engine command: `prefs.set` refuses to turn on file and network
/// access for it, so a script can't grant itself that permission. The previous state comes
/// back on return, error or unwind.
pub fn with_script_origin<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCRIPT_ORIGIN.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(SCRIPT_ORIGIN.with(|c| c.replace(true)));
    f()
}

/// Whether the running command was started by a script.
pub(crate) fn script_origin() -> bool {
    SCRIPT_ORIGIN.with(Cell::get)
}

/// Settings ▸ General ▸ Language: (label, `general.language` value).
pub const LANGUAGES: &[(&str, &str)] =
    &[("Match System", "system"), ("English", "en"), ("日本語", "ja"), ("简体中文", "zh-hans"), ("繁體中文", "zh-hant"), ("Українська", "uk")];

/// Settings ▸ Appearance ▸ UI Scale: (label, `appearance.uiScale` percent).
pub const UI_SCALES: &[(&str, &str)] = &[("75%", "75"), ("100%", "100"), ("125%", "125"), ("150%", "150"), ("175%", "175"), ("200%", "200")];

/// Settings ▸ Startup & Repair ▸ Window Graphics: (label, `startup.windowGraphics` value).
pub const WINDOW_GRAPHICS: &[(&str, &str)] = &[("Automatic", "auto"), ("OpenGL (compatibility)", "gl")];

/// The settings whose value must be one of their choices.
fn choices(key: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match key {
        "general.language" => Some(LANGUAGES),
        "startup.windowGraphics" => Some(WINDOW_GRAPHICS),
        _ => None,
    }
}

fn is_choice(choices: &[(&str, &str)], v: &str) -> bool {
    choices.iter().any(|(_, c)| *c == v)
}

/// Unknown keys inside a page, kept so a newer version's settings survive a round trip.
type Extra = BTreeMap<String, Value>;

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
    create_split_layers_above: bool = false,
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
    /// graphics API) or `gl` (OpenGL, for drivers that crash with the others). A launch whose
    /// window never drew switches it to `gl` (#243).
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
    /// `dark`, `darker` or `light`.
    theme: String = "dark".into(),
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

/// One editable label: name and `#rrggbb` colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabelPref {
    pub name: String,
    pub color: String,
}

fn default_labels() -> Vec<LabelPref> {
    Label::ALL.iter().skip(1).map(|l| LabelPref { name: l.name().to_string(), color: hex(l.rgb()) }).collect()
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([p(0)?, p(2)?, p(4)?])
}

/// All settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    pub version: u32,
    pub general: General,
    pub startup: Startup,
    pub project: ProjectPrefs,
    pub auto_save: AutoSave,
    pub composition: CompositionPrefs,
    pub previews: Previews,
    pub appearance: Appearance,
    pub grids: Grids,
    /// The 16 labels (Red … Dark Green), in order.
    pub labels: Vec<LabelPref>,
    #[serde(rename = "type")]
    pub type_: TypePrefs,
    pub import: Import,
    pub export: Export,
    pub audio: Audio,
    pub disk: Disk,
    pub memory: Memory,
    pub video: Video,
    #[serde(rename = "threeD")]
    pub three_d: ThreeD,
    pub scripting: Scripting,
    pub roto: RotoPrefs,
    pub face: FacePrefs,
    /// File ▸ Open Recent, newest first.
    pub recent_projects: Vec<String>,
    /// File ▸ Import Recent Footage, newest first.
    pub recent_footage: Vec<String>,
    /// Animation ▸ Recent Animation Presets (`.ecpreset` paths or built-in preset ids), newest
    /// first.
    pub recent_presets: Vec<String>,
    /// Character panel: recently used font families, newest first.
    pub recent_fonts: Vec<String>,
    /// Preview panel: every shortcut's options.
    pub preview: crate::preview::PreviewSettings,
    /// View ▸ Simulate Output ▸ My Custom RGB.
    pub custom_rgb: crate::viewer::CustomRgb,
    /// Top-level keys this version doesn't know (kept for newer versions).
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            version: PREFS_VERSION,
            general: General::default(),
            startup: Startup::default(),
            project: ProjectPrefs::default(),
            auto_save: AutoSave::default(),
            composition: CompositionPrefs::default(),
            previews: Previews::default(),
            appearance: Appearance::default(),
            grids: Grids::default(),
            labels: default_labels(),
            type_: TypePrefs::default(),
            import: Import::default(),
            export: Export::default(),
            audio: Audio::default(),
            disk: Disk::default(),
            memory: Memory::default(),
            video: Video::default(),
            three_d: ThreeD::default(),
            scripting: Scripting::default(),
            roto: RotoPrefs::default(),
            face: FacePrefs::default(),
            recent_projects: vec![],
            recent_footage: vec![],
            recent_presets: vec![],
            recent_fonts: vec![],
            preview: crate::preview::PreviewSettings::default(),
            custom_rgb: crate::viewer::CustomRgb::default(),
            extra: Extra::new(),
        }
    }
}

/// Bring an older settings document up to [`PREFS_VERSION`].
///
/// - version 0/1 (early builds) kept a flat object: `undoLevels`, `autoSaveEnabled`,
///   `autoSaveMinutes`, `autoSaveVersions`, `labelNames` (array) and `theme` at the top level.
pub fn migrate(mut v: Value) -> Value {
    let Some(obj) = v.as_object_mut() else { return json!({}) };
    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version < 2 {
        let moved = |from: &str, page: &str, key: &str, obj: &mut Map<String, Value>| {
            if let Some(x) = obj.remove(from) {
                let p = obj.entry(page.to_string()).or_insert_with(|| json!({}));
                if let Some(p) = p.as_object_mut() {
                    p.entry(key.to_string()).or_insert(x);
                }
            }
        };
        moved("undoLevels", "general", "undoLevels", obj);
        moved("autoSaveEnabled", "autoSave", "enabled", obj);
        moved("autoSaveMinutes", "autoSave", "intervalMinutes", obj);
        moved("autoSaveVersions", "autoSave", "maxVersions", obj);
        moved("theme", "appearance", "theme", obj);
        if let Some(Value::Array(names)) = obj.remove("labelNames") {
            let mut labels = default_labels();
            for (l, n) in labels.iter_mut().zip(names) {
                if let Some(n) = n.as_str() {
                    l.name = n.to_string();
                }
            }
            obj.insert("labels".into(), serde_json::to_value(labels).unwrap_or_default());
        }
    }
    obj.insert("version".into(), json!(PREFS_VERSION));
    v
}

impl Prefs {
    /// Parse a settings document (migrating older layouts; unknown keys are kept, bad values
    /// fall back to defaults).
    pub fn from_json(text: &str) -> Prefs {
        let v: Value = serde_json::from_str(text).unwrap_or_else(|_| json!({}));
        let v = migrate(v);
        let mut p: Prefs = match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            // One bad value shouldn't lose everything: take each page that parses.
            Err(_) => {
                let mut base = serde_json::to_value(Prefs::default()).unwrap_or_default();
                if let (Some(b), Some(o)) = (base.as_object_mut(), v.as_object()) {
                    for (k, x) in o {
                        let mut trial = b.clone();
                        trial.insert(k.clone(), x.clone());
                        if serde_json::from_value::<Prefs>(Value::Object(trial)).is_ok() {
                            b.insert(k.clone(), x.clone());
                        }
                    }
                }
                serde_json::from_value(base).unwrap_or_default()
            }
        };
        p.normalize();
        p
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Clamp values into their valid ranges and fill missing labels.
    pub fn normalize(&mut self) {
        self.version = PREFS_VERSION;
        if !is_choice(WINDOW_GRAPHICS, &self.startup.window_graphics) {
            self.startup.window_graphics = Startup::default().window_graphics;
        }
        let g = &mut self.general;
        if !is_choice(LANGUAGES, &g.language) {
            g.language = General::default().language;
        }
        g.undo_levels = g.undo_levels.clamp(1, 99);
        g.path_point_size = g.path_point_size.clamp(3, 20);
        g.recent_items = g.recent_items.clamp(1, 30);
        let a = &mut self.auto_save;
        a.interval_minutes = a.interval_minutes.clamp(1, 240);
        a.max_versions = a.max_versions.clamp(1, 99);
        self.appearance.brightness = self.appearance.brightness.clamp(-1.0, 1.0);
        self.appearance.ui_scale = self.appearance.ui_scale.clamp(75, 200);
        let defaults = default_labels();
        self.labels.truncate(defaults.len());
        for d in defaults.iter().skip(self.labels.len()) {
            self.labels.push(d.clone());
        }
        for (l, d) in self.labels.iter_mut().zip(&defaults) {
            if parse_hex(&l.color).is_none() {
                l.color = d.color.clone();
            }
        }
        let m = &mut self.memory;
        m.layer_cache_mb = m.layer_cache_mb.clamp(64, 1 << 20);
        m.media_cache_mb = m.media_cache_mb.clamp(64, 1 << 20);
        m.preview_cache_mb = m.preview_cache_mb.clamp(64, 1 << 20);
        self.grids.grid_spacing = self.grids.grid_spacing.clamp(1.0, 10_000.0);
        let n = self.general.recent_items as usize;
        self.recent_projects.truncate(n);
    }

    /// The value at a dotted key (`general.undoLevels`, `labels.3.color`); `""` = everything.
    pub fn get(&self, key: &str) -> Option<Value> {
        let v = serde_json::to_value(self).ok()?;
        if key.is_empty() {
            return Some(v);
        }
        let ptr = format!("/{}", key.replace('.', "/"));
        v.pointer(&ptr).cloned()
    }

    /// Set the value at a dotted key. The key must exist and the value must have its type.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        if let Some(c) = choices(key)
            && !value.as_str().is_some_and(|v| is_choice(c, v))
        {
            let all: Vec<&str> = c.iter().map(|(_, v)| *v).collect();
            return Err(format!("`{key}` expects one of {}", all.join(", ")));
        }
        let mut v = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let ptr = format!("/{}", key.replace('.', "/"));
        let slot = v.pointer_mut(&ptr).ok_or_else(|| format!("unknown setting `{key}`"))?;
        let value = coerce(slot, value).ok_or_else(|| format!("`{key}` expects a {}", kind_name(slot)))?;
        *slot = value;
        let mut next: Prefs = serde_json::from_value(v).map_err(|e| format!("`{key}`: {e}"))?;
        next.normalize();
        *self = next;
        Ok(())
    }

    /// Reset one page (`general`, `labels`, `autoSave`…) or everything (`None`). Open Recent is
    /// kept.
    pub fn reset(&mut self, page: Option<&str>) -> Result<(), String> {
        let d = Prefs::default();
        let Some(page) = page else {
            *self = Prefs {
                recent_projects: std::mem::take(&mut self.recent_projects),
                recent_footage: std::mem::take(&mut self.recent_footage),
                recent_presets: std::mem::take(&mut self.recent_presets),
                recent_fonts: std::mem::take(&mut self.recent_fonts),
                preview: std::mem::take(&mut self.preview),
                ..d
            };
            return Ok(());
        };
        let keys: Vec<&str> = match page {
            "project" => vec!["project", "autoSave"],
            p => vec![section_key(p).ok_or_else(|| format!("unknown settings page `{p}`"))?],
        };
        let dv = serde_json::to_value(&d).map_err(|e| e.to_string())?;
        for k in keys {
            if let Some(x) = dv.get(k) {
                self.set(k, x.clone())?;
            }
        }
        Ok(())
    }

    /// Display name of a label (Labels settings).
    pub fn label_name(&self, l: Label) -> String {
        match label_index(l) {
            Some(i) => self.labels.get(i).map(|p| p.name.clone()).unwrap_or_else(|| l.name().to_string()),
            None => l.name().to_string(),
        }
    }

    /// sRGB colour of a label (Labels settings).
    pub fn label_rgb(&self, l: Label) -> [u8; 3] {
        label_index(l).and_then(|i| self.labels.get(i)).and_then(|p| parse_hex(&p.color)).unwrap_or(l.rgb())
    }

    /// A label by its built-in name or its (renamed) display name.
    pub fn label_from_name(&self, s: &str) -> Option<Label> {
        Label::from_name(s).or_else(|| Label::ALL.iter().skip(1).copied().find(|l| self.label_name(*l).eq_ignore_ascii_case(s)))
    }

    /// Remember a project in File ▸ Open Recent.
    pub fn push_recent(&mut self, path: &str) {
        self.recent_projects.retain(|p| p != path);
        self.recent_projects.insert(0, path.to_string());
        self.recent_projects.truncate(self.general.recent_items as usize);
    }

    /// Layer / media / preview cache budgets in bytes.
    pub fn layer_cache_bytes(&self) -> usize {
        (self.memory.layer_cache_mb as usize) << 20
    }
    pub fn media_cache_bytes(&self) -> usize {
        (self.memory.media_cache_mb as usize) << 20
    }
    pub fn preview_cache_bytes(&self) -> usize {
        (self.memory.preview_cache_mb as usize) << 20
    }

    /// The adaptive resolution limit as a render-scale floor (1/2 → 0.5).
    pub fn adaptive_limit(&self) -> f64 {
        match self.previews.adaptive_resolution_limit.as_str() {
            "1/2" => 0.5,
            "1/4" => 0.25,
            _ => 0.125,
        }
    }

    /// The Memory & CPU cache budgets in effect: the configured layer / footage / preview
    /// budgets, scaled down so together they leave **RAM Reserved for Other Applications** free,
    /// and halved while the system is low on memory when **Reduce Cache Size When System Is Low
    /// on Memory** is on. `mem` = the system's memory (`None` = unknown: configured budgets).
    pub fn cache_budgets(&self, mem: Option<crate::sysinfo::SysMemory>) -> CacheBudgets {
        let (mut l, mut m, mut p) = (self.layer_cache_bytes() as u64, self.media_cache_bytes() as u64, self.preview_cache_bytes() as u64);
        let floor = 64u64 << 20;
        let mut capped = false;
        let mut reduced = false;
        if let Some(mem) = mem.filter(|m| m.total > 0) {
            let reserved = (self.memory.ram_reserved_gb as u64) << 30;
            let cap = mem.total.saturating_sub(reserved).max(3 * floor);
            let sum = l + m + p;
            if sum > cap {
                let k = cap as f64 / sum as f64;
                (l, m, p) = ((l as f64 * k) as u64, (m as f64 * k) as u64, (p as f64 * k) as u64);
                capped = true;
            }
            if self.memory.reduce_cache_when_low && mem.is_low() {
                (l, m, p) = (l / 2, m / 2, p / 2);
                reduced = true;
            }
        }
        CacheBudgets { layer: l.max(floor) as usize, media: m.max(floor) as usize, preview: p.max(floor) as usize, capped, reduced }
    }

    /// Composition ▸ Motion Path: the layer-time span of a motion path to draw around `now`
    /// (layer time), for keyframes at `keys` (sorted). `None` = No Motion Path.
    pub fn motion_path_span(&self, keys: &[Tick], now: Tick) -> Option<(Tick, Tick)> {
        let (first, last) = (*keys.first()?, *keys.last()?);
        let c = &self.composition;
        match c.motion_path.as_str() {
            "none" => None,
            "seconds" => {
                let half = Tick::from_seconds_f64(c.motion_path_seconds.max(0.0) / 2.0);
                let (a, b) = ((now - half).max(first), (now + half).min(last));
                (a < b).then_some((a, b))
            }
            "keyframes" => {
                let n = (c.motion_path_keyframes.max(1) as usize).min(keys.len());
                if n < 2 {
                    return None;
                }
                // The n keyframes centred on the current time.
                let at = keys.partition_point(|k| *k < now);
                let start = at.saturating_sub(n / 2).min(keys.len() - n);
                Some((keys[start], keys[start + n - 1]))
            }
            _ => Some((first, last)),
        }
    }

    /// Previews ▸ Viewer Zoom Quality: smooth (bilinear) scaling of the viewer image, or nearest
    /// neighbour ("Faster").
    pub fn viewer_zoom_smooth(&self) -> bool {
        self.previews.zoom_quality != "faster"
    }

    /// Import ▸ Interpret Unlabeled Alpha As, for footage whose file has an alpha channel that
    /// doesn't state how it is stored. `None` = Ask User (keep straight and open Interpret
    /// Footage); `guess` takes premultiplied for movies (codecs with alpha are usually written
    /// premultiplied) and straight for stills.
    pub fn unlabeled_alpha(&self, movie: bool) -> Option<effectcraft_project::AlphaMode> {
        use effectcraft_project::AlphaMode::*;
        Some(match self.import.unlabeled_alpha.as_str() {
            "ignore" => Ignore,
            "straight" => Straight,
            "premultiplied" => Premultiplied,
            "guess" if movie => Premultiplied,
            "guess" => Straight,
            _ => return None,
        })
    }

    /// Import ▸ Default Drag Import As, as `file.import`'s `importAs`.
    pub fn drag_import_as(&self) -> &'static str {
        match self.import.drag_import_as.as_str() {
            "comp" => "composition",
            "compLayerSizes" => "compositionLayerSizes",
            _ => "footage",
        }
    }

    /// Export ▸ Append Bit Depth to File Name: `Comp 1.png` → `Comp 1_16bpc.png` (sequence frame
    /// runs and folders are kept).
    pub fn output_name(&self, path: &str, bits: u32) -> String {
        if !self.export.append_bits_to_name {
            return path.to_string();
        }
        let p = std::path::Path::new(path);
        let (Some(stem), dir) = (p.file_stem().map(|s| s.to_string_lossy().to_string()), p.parent()) else { return path.to_string() };
        let tag = format!("_{bits}bpc");
        if stem.ends_with(&tag) {
            return path.to_string();
        }
        let name = match p.extension() {
            Some(e) => format!("{stem}{tag}.{}", e.to_string_lossy()),
            None => format!("{stem}{tag}"),
        };
        match dir.filter(|d| !d.as_os_str().is_empty()) {
            Some(d) => d.join(name).to_string_lossy().to_string(),
            None => name,
        }
    }

    /// Export ▸ Segment Sequences / Segment Movie Files: how many frames go in each segment of an
    /// output of `frames` frames (`None` = one file / folder). Sequences split every Files per
    /// Segment frames; movies every Segment Size MB at the output's data rate
    /// (`bytes_per_frame`, estimated from the format's bitrate).
    pub fn segment_frames(&self, frames: u64, sequence: bool, bytes_per_frame: f64) -> Option<u64> {
        let e = &self.export;
        let per = if sequence {
            e.segment_sequences.then_some(e.segment_sequence_files.max(1) as u64)?
        } else {
            if !e.segment_movies || bytes_per_frame <= 0.0 {
                return None;
            }
            (((e.segment_movie_mb.max(1) as f64) * 1_048_576.0 / bytes_per_frame).floor() as u64).max(1)
        };
        (per < frames).then_some(per)
    }

    /// Remember a font in the Character panel's recent fonts (Type ▸ Number of Recent Fonts).
    pub fn push_recent_font(&mut self, family: &str) {
        push_mru(&mut self.recent_fonts, family, 30);
    }

    /// The recent fonts the Character panel shows (at most Number of Recent Fonts to Display).
    pub fn recent_fonts_shown(&self) -> &[String] {
        let n = (self.type_.recent_fonts as usize).min(self.recent_fonts.len());
        &self.recent_fonts[..n]
    }

    /// Remember imported footage for File ▸ Import Recent Footage.
    pub fn push_recent_footage(&mut self, path: &str) {
        push_mru(&mut self.recent_footage, path, self.general.recent_items as usize);
    }

    /// Remember an applied animation preset for Animation ▸ Recent Animation Presets.
    pub fn push_recent_preset(&mut self, path: &str) {
        push_mru(&mut self.recent_presets, path, self.general.recent_items as usize);
    }
}

fn push_mru(list: &mut Vec<String>, v: &str, max: usize) {
    list.retain(|x| x != v);
    list.insert(0, v.to_string());
    list.truncate(max.max(1));
}

/// Effective cache budgets in bytes (see [`Prefs::cache_budgets`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct CacheBudgets {
    pub layer: usize,
    pub media: usize,
    pub preview: usize,
    /// Scaled down to leave the reserved RAM free.
    pub capped: bool,
    /// Halved because the system is low on memory.
    pub reduced: bool,
}

/// Colour of the `k`-th mask of a layer: Appearance ▸ Cycle Mask Colors cycles through the mask
/// colours, otherwise every new mask takes the first one.
pub fn mask_color(cycle: bool, k: usize) -> [u8; 3] {
    use effectcraft_project::build::MASK_COLORS;
    if cycle { MASK_COLORS[k % MASK_COLORS.len()] } else { MASK_COLORS[0] }
}

fn label_index(l: Label) -> Option<usize> {
    Label::ALL.iter().position(|x| *x == l).and_then(|i| i.checked_sub(1))
}

/// Settings key of a page id (`3d` → `threeD`).
pub fn section_key(page: &str) -> Option<&'static str> {
    Some(match page {
        "general" => "general",
        "startup" => "startup",
        "project" => "project",
        "autoSave" | "autosave" => "autoSave",
        "composition" | "display" => "composition",
        "previews" => "previews",
        "appearance" => "appearance",
        "grids" => "grids",
        "labels" => "labels",
        "type" => "type",
        "import" => "import",
        "export" => "export",
        "audio" => "audio",
        "disk" => "disk",
        "memory" => "memory",
        "video" => "video",
        "3d" | "threeD" => "threeD",
        "scripting" => "scripting",
        "roto" => "roto",
        "face" => "face",
        _ => return None,
    })
}

/// Settings page id for a page name, including older After Effects page names
/// (Auto-Save → Project, Media & Disk Cache → Disk, Memory & Performance → Memory & CPU…).
pub fn page_id(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase().replace([' ', '&', '-', '_'], "");
    let id = match n.as_str() {
        "general" => "general",
        "startup" | "startuprepair" => "startup",
        "project" | "autosave" | "newproject" => "project",
        "composition" | "display" => "composition",
        "previews" | "preview" => "previews",
        "appearance" => "appearance",
        "grids" | "gridsguides" => "grids",
        "labels" => "labels",
        "type" => "type",
        "import" => "import",
        "export" | "output" => "export",
        "audio" | "audiohardware" | "audiooutputmapping" => "audio",
        "disk" | "mediadiskcache" | "mediacache" => "disk",
        "memory" | "memorycpu" | "memoryperformance" => "memory",
        "video" | "videopreview" => "video",
        "3d" | "threed" => "3d",
        "scripting" | "scriptingexpressions" => "scripting",
        "roto" | "rotobrush" | "models" => "roto",
        "face" | "facetracking" => "face",
        _ => return None,
    };
    Some(id)
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
        Value::Null => "value",
    }
}

/// `value` converted to the type of `slot` (numbers from strings, booleans from 0/1…).
fn coerce(slot: &Value, value: Value) -> Option<Value> {
    Some(match (slot, value) {
        (Value::Bool(_), Value::Bool(b)) => json!(b),
        (Value::Bool(_), Value::Number(n)) => json!(n.as_f64()? != 0.0),
        (Value::Bool(_), Value::String(s)) => json!(matches!(s.as_str(), "true" | "on" | "1" | "yes")),
        (Value::Number(cur), v) => {
            let f = match &v {
                Value::Number(n) => n.as_f64()?,
                Value::String(s) => s.trim().parse().ok()?,
                _ => return None,
            };
            if cur.is_u64() || cur.is_i64() { json!(f.round().max(0.0) as u64) } else { json!(f) }
        }
        (Value::String(_), Value::String(s)) => json!(s),
        (Value::String(_), Value::Number(n)) => json!(n.to_string()),
        (Value::Array(_), v @ Value::Array(_)) => v,
        (Value::Object(_), v @ Value::Object(_)) => v,
        _ => return None,
    })
}

// ---------------------------------------------------------------- schema (Settings dialog)

/// What a setting row edits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bool,
    /// Integer with range and unit.
    Int(i64, i64, &'static str),
    /// Number with range and unit.
    Float(f64, f64, &'static str),
    /// One of (label, value).
    Choice(&'static [(&'static str, &'static str)]),
    Text,
    /// A folder or file path (text + Choose…).
    Path,
    /// `#rrggbb`.
    Color,
    /// UI brightness slider.
    Slider(f64, f64),
}

/// One row of a settings page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item {
    Section(&'static str),
    Setting {
        key: &'static str,
        label: &'static str,
        kind: Kind,
        /// Changes behaviour today (false = shown for parity, listed in docs/preferences.md).
        live: bool,
    },
    /// A button running a command (`params` is JSON).
    Button {
        label: &'static str,
        command: &'static str,
        params: &'static str,
    },
    /// The 16 label names and colours.
    Labels,
    /// Output device picker (device list from the host).
    AudioDevices {
        key: &'static str,
    },
    /// Read-only information line.
    Note(&'static str),
    /// The browser's storage manager (web app only, `storage.*`): usage and quota, persistent
    /// storage, Clear buttons. Not shown where there is no browser storage.
    BrowserStorage,
    /// A task's trained models (`roto.models`, `face.models`): authors, licence, size, install,
    /// choose.
    Models(effectcraft_segment::Task),
}

/// One page of the Settings dialog.
#[derive(Clone, Debug)]
pub struct Page {
    pub id: &'static str,
    pub title: &'static str,
    pub items: Vec<Item>,
}

const fn s(key: &'static str, label: &'static str, kind: Kind, live: bool) -> Item {
    Item::Setting { key, label, kind, live }
}

const B: Kind = Kind::Bool;
const ON_OFF_RES: &[(&str, &str)] = &[("1/2", "1/2"), ("1/4", "1/4"), ("1/8", "1/8")];
const STYLES: &[(&str, &str)] = &[("Lines", "lines"), ("Dashed Lines", "dashed"), ("Dots", "dots")];

/// The Settings pages in After Effects 2026 order.
pub fn pages() -> Vec<Page> {
    use Item::*;
    vec![
        Page {
            id: "general",
            title: "General",
            items: vec![
                s("general.language", "Language", Kind::Choice(LANGUAGES), true),
                s("general.undoLevels", "Levels of Undo", Kind::Int(1, 99, ""), true),
                s("general.pathPointSize", "Path Point and Handle Size", Kind::Int(3, 20, "px"), true),
                s("general.recentItems", "Recent Projects Shown", Kind::Int(1, 30, ""), true),
                Section("Options"),
                s("general.showToolTips", "Show Tool Tips", B, true),
                s("general.createLayersAtCompStart", "Create Layers at Composition Start Time", B, true),
                s("general.switchesAffectNestedComps", "Switches Affect Nested Comps", B, true),
                s("general.defaultSpatialLinear", "Default Spatial Interpolation to Linear", B, true),
                s("general.preserveConstantVertexCount", "Preserve Constant Vertex and Feather Point Count when Editing Masks", B, true),
                s("general.syncTimeRelatedItems", "Synchronize Time of All Related Items", B, true),
                s("general.expressionPickWhipCompact", "Expression Pick Whip Writes Compact English", B, true),
                s("general.createSplitLayersAbove", "Create Split Layers Above Original Layer", B, true),
                s("general.useSystemColorPicker", "Use System Color Picker", B, false),
                Note("EffectCraft always uses its own colour picker (the same on every platform and the web)."),
            ],
        },
        Page {
            id: "startup",
            title: "Startup & Repair",
            items: vec![
                s("startup.showHomeOnLaunch", "Show Home Screen When Launching", B, true),
                s("startup.showHomeOnOpenProject", "Show Home Screen When Opening a Project", B, true),
                s("startup.offerCrashRecovery", "Offer to Open the Latest Auto-Save After a Crash", B, true),
                s("startup.windowGraphics", "Window Graphics", Kind::Choice(WINDOW_GRAPHICS), true),
                Note(
                    "Window Graphics applies from the next launch. When the graphics driver stops EffectCraft before its window draws, the next launch switches to OpenGL.",
                ),
                Section("Repair"),
                Button { label: "Reset Settings", command: "prefs.reset", params: "{}" },
                Button { label: "Reset Keyboard Shortcuts", command: "shortcuts.reset", params: "{}" },
                Button { label: "Empty Disk Cache", command: "edit.purge", params: r#"{"what":"disk"}"# },
            ],
        },
        Page {
            id: "project",
            title: "Project",
            items: vec![
                Section("New Project"),
                s("project.useTemplate", "New Project Loads Template", B, true),
                s("project.templatePath", "Template Project", Kind::Path, true),
                Section("Auto-Save"),
                s("autoSave.enabled", "Automatically Save Projects", B, true),
                s("autoSave.intervalMinutes", "Save Every", Kind::Int(1, 240, "Minutes"), true),
                s("autoSave.maxVersions", "Maximum Project Versions", Kind::Int(1, 99, ""), true),
                s("autoSave.location", "Auto-Save Location", Kind::Choice(&[("Next to Project", "nextToProject"), ("Custom Location", "custom")]), true),
                s("autoSave.folder", "Custom Location", Kind::Path, true),
                s("autoSave.saveOnRenderStart", "Save When Starting Render Queue", B, true),
                Button { label: "Save Now", command: "file.autoSave", params: "{}" },
            ],
        },
        Page {
            id: "composition",
            title: "Composition",
            items: vec![
                Section("Motion Path"),
                s(
                    "composition.motionPath",
                    "Motion Path",
                    Kind::Choice(&[
                        ("All Keyframes", "all"),
                        ("No Motion Path", "none"),
                        ("No More Than (Seconds)", "seconds"),
                        ("No More Than (Keyframes)", "keyframes"),
                    ]),
                    true,
                ),
                s("composition.motionPathSeconds", "Seconds", Kind::Float(0.0, 3600.0, "s"), true),
                s("composition.motionPathKeyframes", "Keyframes", Kind::Int(1, 10_000, ""), true),
                Section("Display"),
                s("composition.showRenderingProgress", "Show Rendering Progress in Info Panel and Flowchart", B, true),
                s("composition.hardwareAcceleratePanels", "Hardware Accelerate Composition, Layer and Footage Panels", B, false),
                Note("Panels are always drawn by the GPU (wgpu)."),
            ],
        },
        Page {
            id: "previews",
            title: "Previews",
            items: vec![
                s("previews.adaptiveResolutionLimit", "Adaptive Resolution Limit", Kind::Choice(ON_OFF_RES), true),
                s("previews.showInternalWireframes", "Show Internal Wireframes", B, true),
                s("previews.cacheFramesWhenIdle", "Cache Frames When Idle", B, true),
                s("previews.fastPreviews", "Fast Previews (Draft 3D, Faster Effects)", B, true),
                s("previews.zoomQuality", "Viewer Zoom Quality", Kind::Choice(&[("Faster", "faster"), ("More Accurate", "moreAccurate")]), true),
                s("previews.displayProfile", "Display Color Space", Kind::Choice(&[("sRGB", "srgb"), ("Display P3", "p3")]), true),
                Button { label: "GPU Information...", command: "app.gpuInfo", params: "{}" },
            ],
        },
        Page {
            id: "appearance",
            title: "Appearance",
            items: vec![
                s("appearance.theme", "Theme", Kind::Choice(&[("Dark", "dark"), ("Darker", "darker"), ("Light", "light")]), true),
                s("appearance.brightness", "Brightness", Kind::Slider(-1.0, 1.0), true),
                s("appearance.uiScale", "UI Scale", Kind::Choice(UI_SCALES), true),
                Section("Labels and Colors"),
                s("appearance.useLabelColorForHandles", "Use Label Color for Layer Handles and Paths", B, true),
                s("appearance.useLabelColorForTabs", "Use Label Color for Related Tabs", B, true),
                s("appearance.cycleMaskColors", "Cycle Mask Colors", B, true),
                s("appearance.useGradients", "Use Gradients", B, true),
                Section("Menu Bar"),
                s("appearance.inWindowMenuBarMac", "Use In-Window Menu Bar on macOS", B, true),
            ],
        },
        Page {
            id: "grids",
            title: "Grids & Guides",
            items: vec![
                Section("Grid"),
                s("grids.gridColor", "Color", Kind::Color, true),
                s("grids.gridSpacing", "Gridline Every", Kind::Float(1.0, 10_000.0, "px"), true),
                s("grids.gridSubdivisions", "Subdivisions", Kind::Int(1, 100, ""), true),
                s("grids.gridStyle", "Style", Kind::Choice(STYLES), true),
                Section("Proportional Grid"),
                s("grids.proportionalHorizontal", "Horizontal", Kind::Int(1, 100, ""), true),
                s("grids.proportionalVertical", "Vertical", Kind::Int(1, 100, ""), true),
                Section("Guides"),
                s("grids.guideColor", "Color", Kind::Color, true),
                s("grids.guideStyle", "Style", Kind::Choice(STYLES), true),
                Section("Safe Margins"),
                s("grids.actionSafe", "Action-safe", Kind::Float(0.0, 50.0, "%"), true),
                s("grids.titleSafe", "Title-safe", Kind::Float(0.0, 50.0, "%"), true),
            ],
        },
        Page { id: "labels", title: "Labels", items: vec![Labels] },
        Page {
            id: "type",
            title: "Type",
            items: vec![
                s("type.textEngine", "Text Engine", Kind::Choice(&[("Latin", "latin"), ("South Asian and Middle Eastern", "southAsian")]), true),
                s("type.fontPreview", "Show Font Preview", B, true),
                s("type.recentFonts", "Number of Recent Fonts to Display", Kind::Int(0, 30, ""), true),
                s("type.fontNamesInEnglish", "Show Font Names in English", B, true),
            ],
        },
        Page {
            id: "import",
            title: "Import",
            items: vec![
                s("import.stillFootage", "Still Footage", Kind::Choice(&[("Length of Composition", "compLength"), ("Duration (Seconds)", "seconds")]), true),
                s("import.stillSeconds", "Still Duration", Kind::Float(0.04, 86_400.0, "s"), true),
                s("import.sequenceFps", "Sequence Footage", Kind::Float(1.0, 999.0, "frames per second"), true),
                s("import.reportMissingFrames", "Report Missing Frames", B, true),
                s(
                    "import.unlabeledAlpha",
                    "Interpret Unlabeled Alpha As",
                    Kind::Choice(&[
                        ("Ask User", "ask"),
                        ("Guess", "guess"),
                        ("Ignore", "ignore"),
                        ("Straight", "straight"),
                        ("Premultiplied", "premultiplied"),
                    ]),
                    true,
                ),
                s(
                    "import.dragImportAs",
                    "Default Drag Import As",
                    Kind::Choice(&[("Footage", "footage"), ("Composition", "comp"), ("Composition - Retain Layer Sizes", "compLayerSizes")]),
                    true,
                ),
            ],
        },
        Page {
            id: "export",
            title: "Export",
            items: vec![
                s("export.defaultOutputFolder", "Default Output Folder", Kind::Path, true),
                s("export.segmentSequences", "Segment Sequences", B, true),
                s("export.segmentSequenceFiles", "Files per Segment", Kind::Int(1, 100_000, "files"), true),
                s("export.segmentMovies", "Segment Movie Files", B, true),
                s("export.segmentMovieMb", "Segment Size", Kind::Int(1, 1_000_000, "MB"), true),
                s("export.appendBitsToName", "Append Bit Depth to File Name", B, true),
            ],
        },
        Page {
            id: "audio",
            title: "Audio",
            items: vec![
                Section("Audio Hardware"),
                AudioDevices { key: "audio.outputDevice" },
                s("audio.previewSampleRate", "Preview Sample Rate", Kind::Int(8_000, 192_000, "Hz"), true),
                Section("Audio Output Mapping"),
                s("audio.outputLeft", "Left", Kind::Int(1, 64, "channel"), true),
                s("audio.outputRight", "Right", Kind::Int(1, 64, "channel"), true),
            ],
        },
        Page {
            id: "disk",
            title: "Disk",
            items: vec![
                Section("Disk Cache"),
                s("disk.diskCacheEnabled", "Enable Disk Cache", B, true),
                s("disk.diskCacheMaxGb", "Maximum Disk Cache Size", Kind::Int(1, 100_000, "GB"), true),
                s("disk.diskCacheFolder", "Disk Cache Folder", Kind::Path, true),
                Button { label: "Empty Disk Cache", command: "edit.purge", params: r#"{"what":"disk"}"# },
                BrowserStorage,
                Section("Media Cache"),
                s("disk.mediaCacheFolder", "Database and Cache Folder", Kind::Path, true),
                s("disk.conformedMediaFolder", "Conformed Audio Folder", Kind::Path, true),
            ],
        },
        Page {
            id: "memory",
            title: "Memory & CPU",
            items: vec![
                s("memory.ramReservedGb", "RAM Reserved for Other Applications", Kind::Int(0, 1024, "GB"), true),
                Section("Cache Budgets"),
                s("memory.layerCacheMb", "Layer Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.mediaCacheMb", "Footage Frame Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.previewCacheMb", "Preview (RAM) Cache", Kind::Int(64, 1 << 20, "MB"), true),
                s("memory.reduceCacheWhenLow", "Reduce Cache Size When System Is Low on Memory", B, true),
            ],
        },
        Page {
            id: "video",
            title: "Video",
            items: vec![
                s("video.enableOutput", "Enable Video Preview Output", B, true),
                s("video.device", "Video Device", Kind::Choice(&[("Floating Window", ""), ("Full Screen", "fullscreen")]), true),
                s("video.outputDuringPlayback", "Video Output During Playback", B, true),
                s("video.mirrorOnMonitor", "Mirror on Computer Monitor", B, true),
                s("video.disableWhenBackground", "Disable Video Output When in Background", B, true),
            ],
        },
        Page {
            id: "3d",
            title: "3D",
            items: vec![
                s("threeD.defaultRenderer", "Default 3D Renderer", Kind::Choice(&[("Classic 3D", "classic"), ("Advanced 3D", "advanced")]), true),
                s("threeD.showReferenceAxes", "Show 3D Reference Axes", B, true),
                s("threeD.extendedViewer", "Extended Viewer", B, true),
                s("threeD.realtimeShadows", "Realtime Shadows in Draft", B, true),
            ],
        },
        Page {
            id: "scripting",
            title: "Scripting & Expressions",
            items: vec![
                Section("Application Scripting"),
                s("scripting.allowScriptsWriteFiles", "Allow Scripts to Write Files and Access Network", B, true),
                s("scripting.warnExecutingFiles", "Warn User When Executing Files", B, true),
                s("scripting.enableJsDebugger", "Enable JavaScript Debugger", B, false),
                Section("Expressions Editor"),
                s("scripting.editorFontSize", "Font Size", Kind::Int(8, 40, "pt"), true),
                s("scripting.syntaxHighlighting", "Syntax Highlighting", B, true),
                s("scripting.lineNumbers", "Line Numbers", B, true),
                s("scripting.autoComplete", "Auto-complete", B, true),
                s("scripting.bracketMatching", "Bracket Matching", B, true),
                s("scripting.wordWrap", "Word Wrap", B, true),
                s("scripting.errorBanner", "Show Expression Error Banner", B, true),
                Note("Expressions use the JavaScript engine. Script errors report their file and line in the Script Console; there is no step debugger."),
            ],
        },
        Page {
            id: "roto",
            title: "Roto Brush",
            items: vec![
                Section("Segmentation Model"),
                Note(
                    "Roto Brush 2.0 and 3.0 use the model chosen here (Version 1.0 always uses the classic engine). Models are open source, downloaded only when you ask, and checked against their published SHA-256.",
                ),
                Models(effectcraft_segment::Task::Mask),
            ],
        },
        Page {
            id: "face",
            title: "Face Tracking",
            items: vec![
                Section("Face Model"),
                Note(
                    "Face tracking in the Tracker panel (Outline Only and Detailed Features) uses the model chosen here. Models are open source, downloaded only when you ask, and checked against their published SHA-256.",
                ),
                Models(effectcraft_segment::Task::Face),
            ],
        },
    ]
}

/// Keys of every setting that doesn't change behaviour yet (the TODO list in
/// `docs/preferences.md`).
pub fn todo_keys() -> Vec<&'static str> {
    pages()
        .into_iter()
        .flat_map(|p| p.items)
        .filter_map(|i| match i {
            Item::Setting { key, live: false, .. } => Some(key),
            _ => None,
        })
        .collect()
}

/// The schema as JSON (for `prefs.pages`).
pub fn pages_json() -> Value {
    let kind = |k: &Kind| -> Value {
        match k {
            Kind::Bool => json!({"type": "bool"}),
            Kind::Int(a, b, u) => json!({"type": "int", "min": a, "max": b, "unit": u}),
            Kind::Float(a, b, u) => json!({"type": "number", "min": a, "max": b, "unit": u}),
            Kind::Choice(o) => json!({"type": "choice", "options": o.iter().map(|(l, v)| json!({"label": l, "value": v})).collect::<Vec<_>>()}),
            Kind::Text => json!({"type": "text"}),
            Kind::Path => json!({"type": "path"}),
            Kind::Color => json!({"type": "color"}),
            Kind::Slider(a, b) => json!({"type": "slider", "min": a, "max": b}),
        }
    };
    let pages: Vec<Value> = pages()
        .iter()
        .map(|p| {
            let items: Vec<Value> = p
                .items
                .iter()
                .map(|i| match i {
                    Item::Section(t) => json!({"section": t}),
                    Item::Setting { key, label, kind: k, live } => {
                        let mut o = kind(k);
                        o["key"] = json!(key);
                        o["label"] = json!(label);
                        o["live"] = json!(live);
                        o
                    }
                    Item::Button { label, command, params } => {
                        json!({"button": label, "command": command, "params": serde_json::from_str::<Value>(params).unwrap_or_default()})
                    }
                    Item::Labels => json!({"labels": "labels.N.name / labels.N.color (N = 0..15)"}),
                    Item::AudioDevices { key } => json!({"type": "device", "key": key, "label": "Default Output"}),
                    Item::Note(t) => json!({"note": t}),
                    Item::BrowserStorage => json!({"browserStorage": "storage.info / storage.persist / storage.clear (web app)"}),
                    Item::Models(task) => {
                        let p = if *task == effectcraft_segment::Task::Face { "face" } else { "roto" };
                        json!({"models": format!("{p}.models / {p}.model.select / {p}.model.download / {p}.model.install / {p}.model.remove"), "key": format!("{p}.model")})
                    }
                })
                .collect();
            json!({"id": p.id, "title": p.title, "items": items})
        })
        .collect();
    json!(pages)
}
