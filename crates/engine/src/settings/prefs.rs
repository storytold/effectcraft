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

use std::collections::BTreeMap;

use effectcraft_color::Label;
use effectcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

mod choices;
mod dialog;
mod document;
mod queries;
mod recent;
mod sections;

pub use choices::{APPEARANCE_MODES, DARK_THEMES, LANGUAGES, LIGHT_THEMES, THEMES, WINDOW_GRAPHICS, theme_family};
pub use dialog::{Item, Kind, Page, pages, pages_json, todo_keys};
pub use document::{migrate, page_id, section_key};
pub use queries::{CacheBudgets, WEB_LAYER_CACHE, WEB_MEDIA_CACHE, mask_color};
pub use recent::{MAX_RECENT_SCRIPTS, RecentScript};
pub use sections::{
    Appearance, Audio, AutoSave, CompPreset, CompositionPrefs, Disk, Export, FacePrefs, General, Grids, Import, LabelPref, Memory, Previews, ProjectPrefs,
    RotoPrefs, Scripting, Startup, ThreeD, TypePrefs, Video,
};
use sections::{Extra, default_labels, lenient_comp_presets};

/// Current settings layout version.
pub const PREFS_VERSION: u32 = 3;
/// Settings file name in the config store.
pub const PREFS_FILE: &str = "prefs.json";

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
    /// File ▸ Scripts ▸ Recent Script Files, newest first, at most [`MAX_RECENT_SCRIPTS`].
    pub recent_scripts: Vec<RecentScript>,
    /// Character panel: recently used font families, newest first.
    pub recent_fonts: Vec<String>,
    /// Effects & Presets: the starred effects' ids, in the order starred.
    pub effects_favorites: Vec<String>,
    /// Preview panel: every shortcut's options.
    pub preview: crate::preview::PreviewSettings,
    /// View ▸ Simulate Output ▸ My Custom RGB.
    pub custom_rgb: crate::viewer::CustomRgb,
    /// Composition Settings presets the user saved, in the order saved.
    #[serde(deserialize_with = "lenient_comp_presets")]
    pub comp_presets: Vec<CompPreset>,
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
            recent_scripts: vec![],
            recent_fonts: vec![],
            effects_favorites: vec![],
            preview: crate::preview::PreviewSettings::default(),
            custom_rgb: crate::viewer::CustomRgb::default(),
            comp_presets: vec![],
            extra: Extra::new(),
        }
    }
}
