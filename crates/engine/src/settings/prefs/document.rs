use super::choices::{APPEARANCE_MODES, DARK_THEMES, LANGUAGES, LIGHT_THEMES, THEMES, WINDOW_GRAPHICS, choices, is_choice, theme_family};
use super::recent::MAX_EFFECTS_FAVORITES;
use super::sections::{CompPreset, default_labels, parse_hex};
use super::*;

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
    // Settings saved before appearance modes had one theme: keep showing it, as a fixed Dark or
    // Light mode, instead of switching established users to Auto.
    if let Some(a) = obj.get_mut("appearance").and_then(Value::as_object_mut)
        && !a.contains_key("appearanceMode")
        && let Some(theme) = a.get("theme").and_then(Value::as_str).map(str::to_string)
        && let Some((mode, slot)) = theme_family(&theme)
    {
        a.insert("appearanceMode".into(), json!(mode));
        a.insert(slot.into(), json!(theme));
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
        self.comp_presets.retain(CompPreset::is_valid);
        let mut seen = std::collections::HashSet::new();
        self.effects_favorites.retain(|id| !id.is_empty() && seen.insert(id.clone()));
        self.effects_favorites.truncate(MAX_EFFECTS_FAVORITES);
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
        let ap = &mut self.appearance;
        let mut seen = std::collections::BTreeSet::new();
        ap.favorite_themes.retain(|id| is_choice(THEMES, id) && seen.insert(id.clone()));
        if !is_choice(APPEARANCE_MODES, &ap.appearance_mode) {
            ap.appearance_mode = Appearance::default().appearance_mode;
        }
        if !is_choice(DARK_THEMES, &ap.dark_theme) {
            ap.dark_theme = Appearance::default().dark_theme;
        }
        if !is_choice(LIGHT_THEMES, &ap.light_theme) {
            ap.light_theme = Appearance::default().light_theme;
        }
        // The legacy single theme mirrors the theme a fixed mode shows (Auto depends on the
        // system, so it keeps the last one).
        match ap.appearance_mode.as_str() {
            "dark" => ap.theme = ap.dark_theme.clone(),
            "light" => ap.theme = ap.light_theme.clone(),
            _ if !is_choice(THEMES, &ap.theme) => ap.theme = ap.dark_theme.clone(),
            _ => {}
        }
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
        let favorites = if key == "appearance.favoriteThemes" {
            Some(&value)
        } else if key == "appearance" {
            value.get("favoriteThemes")
        } else {
            None
        };
        if let Some(favorites) = favorites {
            let values = favorites.as_array().ok_or("favorite themes must be a list")?;
            let mut seen = std::collections::BTreeSet::new();
            for value in values {
                let id = value.as_str().ok_or("favorite theme must be a string")?;
                if !is_choice(THEMES, id) || !seen.insert(id) {
                    return Err("unknown or duplicate favorite theme".into());
                }
            }
        }
        // Older clients set the single theme (by key, or inside the page object): it still picks
        // a visible theme, fixing the mode to its family.
        let legacy_theme = key == "appearance.theme" || (key == "appearance" && value.get("theme").is_some() && value.get("appearanceMode").is_none());
        let mut v = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let ptr = format!("/{}", key.replace('.', "/"));
        let slot = v.pointer_mut(&ptr).ok_or_else(|| format!("unknown setting `{key}`"))?;
        let value = coerce(slot, value).ok_or_else(|| format!("`{key}` expects a {}", kind_name(slot)))?;
        *slot = value;
        let mut next: Prefs = serde_json::from_value(v).map_err(|e| format!("`{key}`: {e}"))?;
        let (a, b) = (&self.appearance, &next.appearance);
        let only_theme_changed = a.theme != b.theme && a.appearance_mode == b.appearance_mode && a.dark_theme == b.dark_theme && a.light_theme == b.light_theme;
        if legacy_theme || only_theme_changed {
            next.appearance.select_theme();
        }
        next.normalize();
        *self = next;
        Ok(())
    }

    /// Reset one page (`general`, `labels`, `autoSave`…) or everything (`None`). Open Recent and
    /// the saved composition presets are kept.
    pub fn reset(&mut self, page: Option<&str>) -> Result<(), String> {
        let d = Prefs::default();
        let Some(page) = page else {
            *self = Prefs {
                recent_projects: std::mem::take(&mut self.recent_projects),
                recent_footage: std::mem::take(&mut self.recent_footage),
                recent_presets: std::mem::take(&mut self.recent_presets),
                recent_fonts: std::mem::take(&mut self.recent_fonts),
                effects_favorites: std::mem::take(&mut self.effects_favorites),
                preview: std::mem::take(&mut self.preview),
                comp_presets: std::mem::take(&mut self.comp_presets),
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
