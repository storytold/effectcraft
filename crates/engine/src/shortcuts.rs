//! Keyboard shortcut presets (Edit ▸ Keyboard Shortcuts).
//!
//! Every menu entry, registry command and frontend command is *bindable*: it has a binding key
//! (the command id, plus its bound parameters for entries such as `app.settings
//! {"page":"general"}`), a scope (application-wide or a panel) and default keys. The built-in
//! preset "EffectCraft Default" is After Effects' default keyboard layout and is read-only; custom
//! presets store only their differences from it. Editing the default preset first duplicates it
//! (as After Effects asks you to "Save As" a custom set). The shortcut dispatcher and the menus
//! read the active preset through [`ShortcutTable`].
//!
//! Shortcut text is `Ctrl+Cmd+Alt+Shift+Key` (`Cmd` is ⌘ on macOS and Ctrl elsewhere).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Preset file in the config store.
pub const SHORTCUTS_FILE: &str = "shortcuts.json";
/// The built-in (read-only) preset.
pub const DEFAULT_PRESET: &str = "EffectCraft Default";
/// Scope of application-wide shortcuts.
pub const APP_SCOPE: &str = "Application";

/// A frontend-only command offered for binding (tools, timeline navigation…).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiCommand {
    pub id: String,
    pub label: String,
    pub shortcut: Option<String>,
}

/// One thing a shortcut can run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bindable {
    /// Binding key: `command` or `command {params}`.
    pub key: String,
    pub command: String,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
    pub label: String,
    /// Menu path (`["Layer", "New"]`) or a category for commands outside the menus.
    pub path: Vec<String>,
    /// `Application` or the panel the shortcut works in (`Timeline`, `Composition`…).
    pub scope: String,
    /// After Effects default keys.
    pub defaults: Vec<String>,
}

/// The binding key of a command with bound parameters.
pub fn binding_key(command: &str, params: &Value) -> String {
    match params {
        Value::Null => command.to_string(),
        Value::Object(m) if m.is_empty() => command.to_string(),
        p => format!("{command} {p}"),
    }
}

/// Scope of a frontend command from its id.
fn ui_scope(id: &str) -> &'static str {
    if id.starts_with("timeline.") {
        "Timeline"
    } else if id.starts_with("view.") {
        "Composition"
    } else {
        APP_SCOPE
    }
}

/// Every bindable command: menu entries, registry commands outside the menus, then the
/// frontend's own commands.
pub fn bindables(ui: &[UiCommand]) -> Vec<Bindable> {
    let mut out: Vec<Bindable> = vec![];
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut push = |b: Bindable, out: &mut Vec<Bindable>| {
        if let Some(&i) = seen.get(&b.key) {
            // Same command in two menus: merge defaults.
            for d in b.defaults {
                if !out[i].defaults.contains(&d) {
                    out[i].defaults.push(d);
                }
            }
            return;
        }
        seen.insert(b.key.clone(), out.len());
        out.push(b);
    };
    let entries = crate::menus::entries();
    for (path, e) in &entries {
        let key = binding_key(&e.command, &e.params);
        let mut defaults: Vec<String> = e.shortcut.iter().filter_map(|s| normalize(s)).collect();
        // Keep the AE . / , shortcuts while also supporting the standard composition
        // zoom keys. Shift+= produces '+' on many keyboard layouts.
        let zoom_keys: &[&str] = match e.command.as_str() {
            "view.zoomIn" => &["Cmd+=", "Cmd+Shift+="],
            "view.zoomOut" => &["Cmd+-"],
            _ => &[],
        };
        defaults.extend(zoom_keys.iter().filter_map(|key| normalize(key)));
        push(
            Bindable {
                key,
                command: e.command.clone(),
                params: e.params.clone(),
                label: e.label.clone(),
                path: path.clone(),
                scope: APP_SCOPE.into(),
                defaults,
            },
            &mut out,
        );
    }
    for c in crate::command_specs() {
        if !c.journal || entries.iter().any(|(_, e)| e.command == c.id) {
            continue;
        }
        let defaults = c.shortcut.and_then(normalize).into_iter().collect();
        push(
            Bindable {
                key: c.id.to_string(),
                command: c.id.to_string(),
                params: Value::Null,
                label: c.label.to_string(),
                path: vec!["Commands".into()],
                scope: APP_SCOPE.into(),
                defaults,
            },
            &mut out,
        );
    }
    for c in ui {
        let scope = ui_scope(&c.id);
        let cat = if scope == APP_SCOPE { "Tools & Panels" } else { scope };
        push(
            Bindable {
                key: c.id.clone(),
                command: c.id.clone(),
                params: Value::Null,
                label: c.label.clone(),
                path: vec![cat.into()],
                scope: scope.into(),
                defaults: c.shortcut.as_deref().and_then(normalize).into_iter().collect(),
            },
            &mut out,
        );
    }
    out
}

/// Canonical shortcut text (`cmd+shift+k`, `Shift+Cmd+K`, `⌘⇧K` → `Cmd+Shift+K`); `None` if it
/// has no key.
pub fn normalize(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (mut ctrl, mut cmd, mut alt, mut shift) = (false, false, false, false);
    let mut key: Option<String> = None;
    // `Cmd++` / `+` name the plus key.
    let (head, plus) = match s.strip_suffix("++") {
        Some(h) => (h, true),
        None if s == "+" => ("", true),
        None => (s, false),
    };
    let mut parts: Vec<String> = head.split('+').filter(|p| !p.is_empty()).map(str::to_string).collect();
    // macOS glyph notation.
    if parts.len() == 1 && parts[0].chars().any(|c| "⌃⌥⇧⌘".contains(c)) {
        let p = parts.remove(0);
        let mut rest = String::new();
        for c in p.chars() {
            match c {
                '⌃' => ctrl = true,
                '⌥' => alt = true,
                '⇧' => shift = true,
                '⌘' => cmd = true,
                c => rest.push(c),
            }
        }
        if !rest.is_empty() {
            parts.push(rest);
        }
    }
    for p in parts {
        match p.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" | "win" => cmd = true,
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" => alt = true,
            "shift" => shift = true,
            _ => key = Some(key_name(&p)),
        }
    }
    if plus {
        key = Some("+".into());
    }
    let key = key?;
    let mut out = String::new();
    for (on, m) in [(ctrl, "Ctrl+"), (cmd, "Cmd+"), (alt, "Alt+"), (shift, "Shift+")] {
        if on {
            out.push_str(m);
        }
    }
    out.push_str(&key);
    Some(out)
}

fn key_name(k: &str) -> String {
    let l = k.to_ascii_lowercase();
    match l.as_str() {
        "space" | "spacebar" => "Space".into(),
        "esc" | "escape" => "Escape".into(),
        "enter" | "return" => "Enter".into(),
        "tab" => "Tab".into(),
        "backspace" => "Backspace".into(),
        "delete" | "del" => "Delete".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "pgup" => "PageUp".into(),
        "pagedown" | "pgdn" => "PageDown".into(),
        "up" | "arrowup" => "ArrowUp".into(),
        "down" | "arrowdown" => "ArrowDown".into(),
        "left" | "arrowleft" => "ArrowLeft".into(),
        "right" | "arrowright" => "ArrowRight".into(),
        _ if l.starts_with("num") => format!("Num{}", &k[3..]),
        _ if l.len() > 1 && l.starts_with('f') && l[1..].chars().all(|c| c.is_ascii_digit()) => l.to_ascii_uppercase(),
        _ if k.chars().count() == 1 => k.to_uppercase(),
        _ => k.to_string(),
    }
}

/// A custom preset: differences from the default (`[]` removes a default shortcut).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub overrides: BTreeMap<String, Vec<String>>,
}

/// The saved presets and which one is active.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymaps {
    pub active: String,
    pub presets: Vec<Preset>,
}

impl Default for Keymaps {
    fn default() -> Self {
        Keymaps { active: DEFAULT_PRESET.into(), presets: vec![] }
    }
}

/// A conflict: another command already using the keys in an overlapping scope.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub keys: String,
    pub key: String,
    pub label: String,
    pub scope: String,
}

fn scopes_overlap(a: &str, b: &str) -> bool {
    a == b || a == APP_SCOPE || b == APP_SCOPE
}

impl Keymaps {
    pub fn from_json(text: &str) -> Keymaps {
        let mut k: Keymaps = serde_json::from_str(text).unwrap_or_default();
        if !k.presets.iter().any(|p| p.name == k.active) {
            k.active = DEFAULT_PRESET.into();
        }
        k
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Preset names, the default first.
    pub fn names(&self) -> Vec<String> {
        std::iter::once(DEFAULT_PRESET.to_string()).chain(self.presets.iter().map(|p| p.name.clone())).collect()
    }

    pub fn active_preset(&self) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == self.active)
    }

    /// Effective keys of a bindable in the active preset.
    pub fn keys_of(&self, b: &Bindable) -> Vec<String> {
        match self.active_preset().and_then(|p| p.overrides.get(&b.key)) {
            Some(k) => k.clone(),
            None => b.defaults.clone(),
        }
    }

    fn unique_name(&self, base: &str) -> String {
        if !self.names().iter().any(|n| n == base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !self.names().contains(n)).unwrap_or_else(|| base.to_string())
    }

    /// Make sure a custom preset is active (editing the default duplicates it as "Custom").
    /// Returns the name of a preset created for the edit.
    fn ensure_custom(&mut self) -> Option<String> {
        if self.active_preset().is_some() {
            return None;
        }
        let name = self.unique_name("Custom");
        self.presets.push(Preset { name: name.clone(), overrides: BTreeMap::new() });
        self.active = name.clone();
        Some(name)
    }

    /// Set the keys of a bindable (normalized; empty = no shortcut). Returns a newly created
    /// preset name when the default was active.
    pub fn set(&mut self, b: &Bindable, keys: &[String]) -> Result<Option<String>, String> {
        let mut norm = vec![];
        for k in keys {
            let n = normalize(k).ok_or_else(|| format!("`{k}` is not a shortcut"))?;
            if !norm.contains(&n) {
                norm.push(n);
            }
        }
        let created = self.ensure_custom();
        let p = self.presets.iter_mut().find(|p| p.name == self.active).ok_or("no active preset")?;
        if norm == b.defaults {
            p.overrides.remove(&b.key);
        } else {
            p.overrides.insert(b.key.clone(), norm);
        }
        Ok(created)
    }

    /// Clear the active preset's changes (its shortcuts become the defaults).
    pub fn reset_active(&mut self) {
        let active = self.active.clone();
        if let Some(p) = self.presets.iter_mut().find(|p| p.name == active) {
            p.overrides.clear();
        }
    }

    pub fn select(&mut self, name: &str) -> Result<(), String> {
        if !self.names().iter().any(|n| n == name) {
            return Err(format!("no shortcut preset `{name}`"));
        }
        self.active = name.to_string();
        Ok(())
    }

    /// Duplicate a preset (the default or a custom one) under a new name and activate it.
    pub fn duplicate(&mut self, from: &str, name: Option<&str>) -> Result<String, String> {
        let overrides = if from == DEFAULT_PRESET {
            BTreeMap::new()
        } else {
            self.presets.iter().find(|p| p.name == from).ok_or_else(|| format!("no shortcut preset `{from}`"))?.overrides.clone()
        };
        let name = self.unique_name(name.unwrap_or(&format!("{from} copy")));
        self.presets.push(Preset { name: name.clone(), overrides });
        self.active = name.clone();
        Ok(name)
    }

    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        if name == DEFAULT_PRESET {
            return Err("the default preset can't be deleted".into());
        }
        let n = self.presets.len();
        self.presets.retain(|p| p.name != name);
        if self.presets.len() == n {
            return Err(format!("no shortcut preset `{name}`"));
        }
        if self.active == name {
            self.active = DEFAULT_PRESET.into();
        }
        Ok(())
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if from == DEFAULT_PRESET || to == DEFAULT_PRESET {
            return Err("the default preset can't be renamed".into());
        }
        if self.names().iter().any(|n| n == to) {
            return Err(format!("a preset named `{to}` exists"));
        }
        let p = self.presets.iter_mut().find(|p| p.name == from).ok_or_else(|| format!("no shortcut preset `{from}`"))?;
        p.name = to.to_string();
        if self.active == from {
            self.active = to.to_string();
        }
        Ok(())
    }

    /// A preset as a portable JSON document.
    pub fn export(&self, name: Option<&str>) -> Result<Value, String> {
        let name = name.unwrap_or(&self.active);
        let overrides = if name == DEFAULT_PRESET {
            BTreeMap::new()
        } else {
            self.presets.iter().find(|p| p.name == name).ok_or_else(|| format!("no shortcut preset `{name}`"))?.overrides.clone()
        };
        Ok(json!({"format": "effectcraft-shortcuts", "version": 1, "name": name, "base": DEFAULT_PRESET, "overrides": overrides}))
    }

    /// Import an exported preset (renamed if the name is taken) and activate it.
    pub fn import(&mut self, doc: &Value, name: Option<&str>) -> Result<String, String> {
        let overrides: BTreeMap<String, Vec<String>> =
            serde_json::from_value(doc.get("overrides").cloned().unwrap_or_else(|| json!({}))).map_err(|e| format!("bad shortcut preset: {e}"))?;
        let overrides = overrides.into_iter().map(|(k, v)| (k, v.iter().filter_map(|s| normalize(s)).collect())).collect();
        let base = name.or_else(|| doc.get("name").and_then(Value::as_str)).unwrap_or("Imported");
        let base = if base == DEFAULT_PRESET { "Imported" } else { base };
        let name = self.unique_name(base);
        self.presets.push(Preset { name: name.clone(), overrides });
        self.active = name.clone();
        Ok(name)
    }
}

/// The active preset resolved against every bindable command.
#[derive(Clone, Debug, Default)]
pub struct ShortcutTable {
    pub bindables: Vec<Bindable>,
    /// Effective keys per binding key.
    pub keys: HashMap<String, Vec<String>>,
}

impl ShortcutTable {
    pub fn build(maps: &Keymaps, ui: &[UiCommand]) -> ShortcutTable {
        let bindables = bindables(ui);
        let keys = bindables.iter().map(|b| (b.key.clone(), maps.keys_of(b))).collect();
        ShortcutTable { bindables, keys }
    }

    pub fn find(&self, key: &str) -> Option<&Bindable> {
        self.bindables.iter().find(|b| b.key == key)
    }

    /// First shortcut of a command / menu entry (for menus).
    pub fn shortcut_of(&self, command: &str, params: &Value) -> Option<&str> {
        self.keys.get(&binding_key(command, params)).and_then(|k| k.first()).map(String::as_str)
    }

    /// Every active binding: (keys, bindable).
    pub fn bindings(&self) -> Vec<(&str, &Bindable)> {
        self.bindables.iter().flat_map(|b| self.keys.get(&b.key).into_iter().flatten().map(move |k| (k.as_str(), b))).collect()
    }

    /// Other bindables using `keys` in an overlapping scope.
    pub fn conflicts(&self, b: &Bindable, keys: &str) -> Vec<Conflict> {
        let Some(keys) = normalize(keys) else { return vec![] };
        self.bindings()
            .into_iter()
            .filter(|(k, o)| *k == keys && o.key != b.key && scopes_overlap(&o.scope, &b.scope))
            .map(|(k, o)| Conflict { keys: k.to_string(), key: o.key.clone(), label: o.label.clone(), scope: o.scope.clone() })
            .collect()
    }

    /// Every pair of commands sharing keys in an overlapping scope.
    pub fn all_conflicts(&self) -> Vec<(String, Conflict)> {
        let mut by_keys: BTreeMap<&str, Vec<&Bindable>> = BTreeMap::new();
        for (k, b) in self.bindings() {
            if k != "Num*" {
                by_keys.entry(k).or_default().push(b);
            }
        }
        let mut out = vec![];
        for (k, bs) in by_keys {
            for (i, a) in bs.iter().enumerate() {
                for b in &bs[i + 1..] {
                    if scopes_overlap(&a.scope, &b.scope) {
                        out.push((a.key.clone(), Conflict { keys: k.to_string(), key: b.key.clone(), label: b.label.clone(), scope: b.scope.clone() }));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod zoom_shortcut_tests {
    use super::*;

    #[test]
    fn composition_zoom_supports_primary_plus_minus_without_losing_existing_keys() {
        let table = ShortcutTable::build(&Keymaps::default(), &[]);
        let zoom_in = table.find("view.zoomIn").unwrap();
        let zoom_out = table.find("view.zoomOut").unwrap();
        assert_eq!(zoom_in.defaults, ["." , "Cmd+=", "Cmd+Shift+="]);
        assert_eq!(zoom_out.defaults, [",", "Cmd+-"]);
        for (key, command) in [
            (".", "view.zoomIn"),
            ("Cmd+=", "view.zoomIn"),
            ("Cmd+Shift+=", "view.zoomIn"),
            (",", "view.zoomOut"),
            ("Cmd+-", "view.zoomOut"),
        ] {
            assert!(table.bindings().iter().any(|(bound, b)| *bound == key && b.command == command), "{key} must invoke {command}");
        }
    }
}
