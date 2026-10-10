//! The menu bar, laid out like After Effects' (same menus, order, submenus, separators and
//! labels). Every entry runs a registered command (`commands::find`), optionally with bound
//! parameters (e.g. `layer.setBlendMode {"mode":"Multiply"}`). Frontends render this tree; agents
//! read it through `ui.menu.list` / the command registry.
//!
//! The tree is written in a small indented text format (two spaces per level):
//!
//! ```text
//! Layer                                       top-level menu
//!   Blending Mode                             submenu (has deeper lines below it)
//!     Multiply | layer.setBlendMode {"mode":"Multiply"}
//!   Next Blending Mode | layer.setBlendMode {"step":1} | Shift+=
//!   ---                                       separator
//!   @effects                                  the effect categories (Effect menu)
//!   @dynamic:history                          entries computed from the session when the menu
//!                                             opens ([`dynamic`]: recent files, undo history…)
//! [mac] Quit EffectCraft | app.quit           only on macOS ([!mac] = everywhere else)
//! ```
//!
//! An entry's shortcut is the command's default shortcut unless the line gives one (needed for
//! entries with bound parameters). Adobe-service entries (Team Projects, Libraries, Bridge, Media
//! Encoder, Behance, Creative Cloud…) are intentionally absent; the Essential Graphics workspace
//! uses the Properties panel and the Essential Graphics panel, whose templates use EffectCraft's
//! own open `.ectemplate` format.

use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

use crate::Session;
use crate::commands::{self, CommandSpec};

/// One node of the menu tree.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MenuNode {
    Item(MenuEntry),
    Separator,
    Submenu {
        label: String,
        children: Vec<MenuNode>,
    },
    /// Entries computed when the menu opens (see [`dynamic`]).
    Dynamic {
        name: String,
    },
}

/// Frontend state dynamic menus depend on.
#[derive(Clone, Copy, Debug, Default)]
pub struct DynCtx<'a> {
    /// The current workspace (Window ▸ Assign Shortcut to "…" Workspace).
    pub workspace: Option<&'a str>,
    /// Workspaces the user saved (Save as New Workspace), listed in Window ▸ Workspace.
    pub saved_workspaces: &'a [String],
}

fn file_name(p: &str) -> String {
    std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string())
}

fn dyn_entry(label: String, command: &str, params: Value) -> MenuEntry {
    MenuEntry { label, command: command.into(), params, shortcut: None }
}

/// The bindable holding shortcut `keys` in the active preset (its label).
fn holder_of(s: &Session, keys: &str) -> Option<String> {
    let t = s.shortcuts();
    t.bindables.iter().find(|b| t.keys.get(&b.key).is_some_and(|k| k.iter().any(|k| k == keys))).map(|b| b.label.clone())
}

/// The entries of a `@dynamic:<name>` node, and the disabled placeholder shown when there are
/// none: `recentProjects`, `recentFootage`, `recentPresets`, `history` (undo steps, newest
/// first), `view3dShortcuts`, `workspaceShortcuts`, `savedWorkspaces` (Window ▸ Workspace) and
/// `openViewers` (open compositions other than the active one), `scripts` (File ▸ Scripts) and
/// `scriptPanels` (ScriptUI panels).
pub fn dynamic(s: &Session, name: &str, cx: &DynCtx) -> (Vec<MenuEntry>, Option<&'static str>) {
    use serde_json::json;
    let indexed = |list: &[String], cmd: &str, stem: bool| -> Vec<MenuEntry> {
        list.iter()
            .enumerate()
            .map(|(i, p)| {
                let n =
                    if stem { std::path::Path::new(p).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone()) } else { file_name(p) };
                dyn_entry(n, cmd, json!({"index": i}))
            })
            .collect()
    };
    match name {
        "recentProjects" => (indexed(&s.prefs.recent_projects, "file.openRecent", false), Some("No Recent Projects")),
        "recentFootage" => (indexed(&s.prefs.recent_footage, "file.importRecent", false), Some("No Recent Footage")),
        "recentPresets" => (indexed(&s.prefs.recent_presets, "anim.applyRecentPreset", true), Some("No Recent Presets")),
        "history" => (
            s.history.undo.iter().rev().enumerate().map(|(i, (l, _))| dyn_entry(format!("Undo {l}"), "edit.history", json!({"steps": i + 1}))).collect(),
            Some("No History"),
        ),
        "view3dShortcuts" => (
            ["F10", "F11", "F12"]
                .iter()
                .map(|k| {
                    let label = match holder_of(s, k) {
                        Some(h) => format!("{k} (Replace “{h}”)"),
                        None => k.to_string(),
                    };
                    dyn_entry(label, "view.assign3dShortcut", json!({"slot": k}))
                })
                .collect(),
            None,
        ),
        "workspaceShortcuts" => (
            ["Shift+F10", "Shift+F11", "Shift+F12"]
                .iter()
                .map(|k| {
                    let label = match holder_of(s, k) {
                        Some(h) => format!("{k} (Replace “{h}”)"),
                        None => k.to_string(),
                    };
                    let mut p = json!({"slot": k});
                    if let Some(w) = cx.workspace {
                        p["workspace"] = json!(w);
                    }
                    dyn_entry(label, "window.assignWorkspaceShortcut", p)
                })
                .collect(),
            None,
        ),
        "savedWorkspaces" => (cx.saved_workspaces.iter().map(|w| dyn_entry(w.clone(), "window.workspace", json!({"name": w}))).collect(), None),
        "openViewers" => (
            s.state
                .open_comps
                .iter()
                .filter(|c| Some(**c) != s.state.active_comp)
                .filter_map(|c| s.project.item(*c).map(|i| dyn_entry(format!("Composition: {}", i.name), "comp.open", json!({"comp": c.0}))))
                .collect(),
            None,
        ),
        // File ▸ Scripts: installed and sample scripts; the Window menu's ScriptUI panels.
        "scripts" | "scriptPanels" => {
            let panels = name == "scriptPanels";
            let cmd = if panels { "window.scriptPanel" } else { "file.runScript" };
            (
                crate::commands::scripts::scripts(s)
                    .into_iter()
                    .filter(|e| e.panel == panels)
                    .map(|e| dyn_entry(e.name.clone(), cmd, json!({"name": e.name})))
                    .collect(),
                None,
            )
        }
        _ => (vec![], None),
    }
}

/// Display label of a submenu (the "Assign Shortcut to …" submenus name the current view or
/// workspace, like After Effects).
pub fn submenu_label(s: &Session, label: &str, cx: &DynCtx) -> String {
    match label {
        "Assign Shortcut to 3D View" => {
            let v = s.active_comp_id().and_then(|c| s.state.views3d.get(&c)).map(|v| v.current).unwrap_or_default();
            let name = crate::commands::find(crate::commands::view_command(v)).map(|c| c.label).unwrap_or("Active Camera");
            format!("Assign Shortcut to “{name}”")
        }
        "Assign Shortcut to Workspace" => format!("Assign Shortcut to “{}” Workspace", cx.workspace.unwrap_or("Default")),
        _ => label.to_string(),
    }
}

/// A clickable menu entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MenuEntry {
    pub label: String,
    pub command: String,
    /// Parameters bound to the entry (`null` = none).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
    pub shortcut: Option<String>,
}

impl MenuEntry {
    pub fn spec(&self) -> Option<&'static CommandSpec> {
        commands::find(&self.command)
    }
    pub fn params_or_empty(&self) -> Value {
        if self.params.is_null() { Value::Object(Default::default()) } else { self.params.clone() }
    }
}

/// Top-level menus in order (macOS has the application menu first).
pub fn top_level() -> Vec<&'static str> {
    menu_bar()
        .iter()
        .filter_map(|n| match n {
            MenuNode::Submenu { label, .. } => Some(label.as_str()),
            _ => None,
        })
        .collect()
}

/// The menu bar for this platform.
pub fn menu_bar() -> &'static [MenuNode] {
    static BAR: OnceLock<Vec<MenuNode>> = OnceLock::new();
    BAR.get_or_init(|| {
        // The tree is a constant (a test parses it on every platform); never panic over it.
        parse(TREE, cfg!(target_os = "macos")).unwrap_or_else(|e| {
            log::error!("menu tree: {e}");
            Vec::new()
        })
    })
}

/// Every entry with its submenu path (`["Layer", "Blending Mode"]`), depth first.
pub fn entries() -> Vec<(Vec<String>, &'static MenuEntry)> {
    fn rec(nodes: &'static [MenuNode], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'static MenuEntry)>) {
        for n in nodes {
            match n {
                MenuNode::Item(e) => out.push((path.clone(), e)),
                MenuNode::Separator | MenuNode::Dynamic { .. } => {}
                MenuNode::Submenu { label, children } => {
                    path.push(label.clone());
                    rec(children, path, out);
                    path.pop();
                }
            }
        }
    }
    let mut out = vec![];
    rec(menu_bar(), &mut vec![], &mut out);
    out
}

/// Display label of an entry: label names come from Settings ▸ Labels.
pub fn entry_label(s: &Session, e: &MenuEntry) -> String {
    let comp_name = || s.active_comp_id().and_then(|c| s.project.item(c)).map(|i| i.name.clone()).unwrap_or_else(|| "(none)".into());
    let layer_name =
        || s.active_comp().and_then(|c| s.state.selected_layers.first().and_then(|l| c.layer(*l))).map(|l| l.name.clone()).unwrap_or_else(|| "(none)".into());
    match e.command.as_str() {
        // Window menu: the panels' current viewer instances.
        "window.panel" => match e.params.get("panel").and_then(Value::as_str) {
            Some("composition") => format!("Composition: {}", comp_name()),
            Some("timeline") => format!("Timeline: {}", comp_name()),
            Some("flowchart") => format!("Flowchart: {}", comp_name()),
            Some("layer") => format!("Layer: {}", layer_name()),
            Some("effectControls") if e.label == "Effect Controls" => format!("Effect Controls: {}", layer_name()),
            // The footage shown in the Footage panel (else the selected footage).
            Some("footage") => {
                let shown = s.state.footage_panel.as_ref().and_then(|f| s.project.item(f.item));
                let f = shown.or_else(|| {
                    s.state.project_selection.iter().filter_map(|i| s.project.item(*i)).find(|i| matches!(i.kind, effectcraft_project::ItemKind::Footage(_)))
                });
                format!("Footage: {}", f.map(|i| i.name.clone()).unwrap_or_else(|| "(none)".into()))
            }
            _ => e.label.clone(),
        },
        "edit.label" => match e.params.get("label").and_then(Value::as_str).and_then(effectcraft_color::Label::from_name) {
            Some(l) if l != effectcraft_color::Label::None => s.prefs.label_name(l),
            _ => e.label.clone(),
        },
        _ => e.label.clone(),
    }
}

/// Check-mark state of an entry (`None` = not a toggle / radio entry). Frontend-only toggles
/// (rulers, grid…) are answered by the frontend.
pub fn checked(s: &Session, command: &str, params: &Value) -> Option<bool> {
    if command == "help.enableLogging" {
        return Some(crate::logging::is_enabled());
    }
    let comp = s.active_comp()?;
    let layer = s.state.selected_layers.first().and_then(|l| comp.layer(*l));
    let pstr = |k: &str| params.get(k).and_then(Value::as_str);
    match command {
        "layer.setBlendMode" => {
            let want = effectcraft_color::BlendMode::from_name(pstr("mode")?)?;
            Some(layer?.blend_mode == want)
        }
        "layer.setSwitch" => {
            let l = layer?;
            let sw = &l.switches;
            Some(match pstr("switch")? {
                "shy" => sw.shy,
                "lock" => sw.locked,
                "audio" => sw.audio,
                "video" => sw.video,
                "solo" => sw.solo,
                "fx" => sw.effects,
                "collapse" => sw.collapse,
                "motionBlur" => sw.motion_blur,
                "adjustment" => sw.adjustment,
                "threeD" => sw.three_d,
                "guide" => sw.guide,
                "preserveTransparency" => l.preserve_transparency,
                _ => return None,
            })
        }
        "layer.quality" => {
            use effectcraft_project::Quality::*;
            let q = layer?.switches.quality;
            Some(matches!((pstr("quality")?, q), ("best", Best) | ("draft", Draft) | ("wireframe", Wireframe)))
        }
        "layer.sampling" => {
            use effectcraft_project::Sampling::*;
            let q = layer?.switches.sampling;
            Some(matches!((pstr("sampling")?, q), ("bilinear", Bilinear) | ("bicubic", Bicubic)))
        }
        "layer.frameBlending" => {
            use effectcraft_project::FrameBlend::*;
            let q = layer?.switches.frame_blend;
            Some(matches!((pstr("mode")?, q), ("off", Off) | ("frameMix", FrameMix) | ("pixelMotion", PixelMotion)))
        }
        "layer.trackMatte" => {
            use effectcraft_project::MatteKind::*;
            let m = layer?.track_matte;
            Some(match (pstr("op")?, m) {
                ("none", None) => true,
                ("alpha", Some(m)) => m.kind == Alpha,
                ("alphaInverted", Some(m)) => m.kind == AlphaInverted,
                ("luma", Some(m)) => m.kind == Luma,
                ("lumaInverted", Some(m)) => m.kind == LumaInverted,
                _ => false,
            })
        }
        "layer.markersLock" => Some(layer?.markers_locked),
        "layer.mask.hideLocked" => Some(s.state.hide_locked_masks),
        "view.layout" => Some(params.get("views").and_then(Value::as_u64) == Some(s.state.view_layout.max(1) as u64)),
        "view.shareViewOptions" => Some(s.state.share_view_options),
        "view.extendedViewer" => Some(s.prefs.three_d.extended_viewer),
        "view.snapping" => Some(s.state.snapping),
        "view.displayColorManagement" => Some(s.state.viewer.display_color_management),
        "view.customRgb" => Some(s.state.viewer.simulation.profile == crate::viewer::SimProfile::MyCustom),
        "view.simulateOutput" => {
            let want = pstr("profile")?;
            let sim = s.state.viewer.simulation;
            Some(if want == "custom" {
                sim.profile != crate::viewer::SimProfile::None && sim == s.state.viewer.custom_simulation
            } else {
                crate::viewer::SimProfile::parse(want) == Some(sim.profile)
                    && (sim != s.state.viewer.custom_simulation || sim.profile == crate::viewer::SimProfile::None)
            })
        }
        "layer.mask.motionBlur" | "layer.mask.featherFalloff" | "path.rotoBezier" => {
            use effectcraft_project::{FeatherFalloff, GroupKind, MaskMotionBlur};
            let GroupKind::Mask { motion_blur, feather_falloff, roto_bezier, .. } = layer?.masks()?.groups().next()?.kind else { return None };
            Some(match command {
                "path.rotoBezier" => roto_bezier,
                "layer.mask.motionBlur" => matches!(
                    (pstr("mode")?, motion_blur),
                    ("sameAsLayer", MaskMotionBlur::SameAsLayer) | ("on", MaskMotionBlur::On) | ("off", MaskMotionBlur::Off)
                ),
                _ => matches!((pstr("mode")?, feather_falloff), ("smooth", FeatherFalloff::Smooth) | ("linear", FeatherFalloff::Linear)),
            })
        }
        _ => None,
    }
}

/// Labels that name the platform's own tools: "Reveal in Finder" is "Reveal in Explorer" on
/// Windows (as in After Effects) and "Reveal in File Manager" elsewhere.
fn platform_label(label: &str, mac: bool) -> String {
    match label.strip_prefix("Reveal in Finder") {
        Some(rest) if !mac => format!("Reveal in {}{rest}", if cfg!(target_os = "windows") { "Explorer" } else { "File Manager" }),
        _ => label.to_string(),
    }
}

/// Parse the tree text. `mac` selects `[mac]` / `[!mac]` lines.
pub fn parse(text: &str, mac: bool) -> Result<Vec<MenuNode>, String> {
    struct Line<'a> {
        depth: usize,
        body: &'a str,
    }
    let mut lines = vec![];
    // Depth of a skipped (other-platform) line: its whole subtree is skipped too.
    let mut skip_below: Option<usize> = None;
    for (n, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() || raw.trim_start().starts_with("//") {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent % 2 != 0 {
            return Err(format!("line {}: odd indentation", n + 1));
        }
        let depth = indent / 2;
        match skip_below {
            Some(d) if depth > d => continue,
            _ => skip_below = None,
        }
        let mut body = raw.trim();
        let tagged = |tag: &str, keep: bool, body: &mut &str| -> Option<bool> {
            let rest = body.strip_prefix(tag)?;
            *body = rest;
            Some(keep)
        };
        if let Some(false) = tagged("[mac] ", mac, &mut body).or_else(|| tagged("[!mac] ", !mac, &mut body)) {
            skip_below = Some(depth);
            continue;
        }
        lines.push(Line { depth, body });
    }
    fn build(lines: &[Line], i: &mut usize, depth: usize, mac: bool) -> Result<Vec<MenuNode>, String> {
        let mut out = vec![];
        while *i < lines.len() {
            let l = &lines[*i];
            if l.depth < depth {
                break;
            }
            if l.depth > depth {
                return Err(format!("unexpected indentation at `{}`", l.body));
            }
            *i += 1;
            if l.body == "---" {
                out.push(MenuNode::Separator);
                continue;
            }
            if l.body == "@effects" {
                out.extend(effect_categories());
                continue;
            }
            if let Some(name) = l.body.strip_prefix("@dynamic:") {
                out.push(MenuNode::Dynamic { name: name.trim().to_string() });
                continue;
            }
            let mut parts = l.body.split(" | ");
            let label = platform_label(parts.next().unwrap_or_default().trim(), mac);
            match parts.next() {
                Some(cmd) => {
                    let cmd = cmd.trim();
                    let (command, params) = match cmd.split_once(' ') {
                        Some((c, json)) => (c.to_string(), serde_json::from_str(json.trim()).map_err(|e| format!("`{}`: {e}", l.body))?),
                        None => (cmd.to_string(), Value::Null),
                    };
                    // Entries with bound params state their own shortcut; plain entries use the command's.
                    let inherited = if params.is_null() { commands::find(&command).and_then(|c| c.shortcut).map(str::to_string) } else { None };
                    let shortcut = parts.next().map(|s| s.trim().to_string()).or(inherited);
                    out.push(MenuNode::Item(MenuEntry { label, command, params, shortcut }));
                }
                None => {
                    let children = build(lines, i, depth + 1, mac)?;
                    if children.is_empty() {
                        return Err(format!("`{label}` has neither a command nor children"));
                    }
                    out.push(MenuNode::Submenu { label, children });
                }
            }
        }
        Ok(out)
    }
    let mut i = 0;
    build(&lines, &mut i, 0, mac)
}

/// Effect ▸ category submenus from the effect registry.
fn effect_categories() -> Vec<MenuNode> {
    let mut cats: Vec<(&str, Vec<MenuNode>)> = vec![];
    for e in crate::effects::all() {
        let entry =
            MenuNode::Item(MenuEntry { label: e.name.into(), command: "effect.apply".into(), params: serde_json::json!({"effect": e.id}), shortcut: None });
        match cats.iter_mut().find(|(c, _)| *c == e.category) {
            Some((_, v)) => v.push(entry),
            None => cats.push((e.category, vec![entry])),
        }
    }
    cats.sort_by_key(|c| c.0.to_ascii_lowercase());
    for (_, v) in &mut cats {
        v.sort_by_key(|n| match n {
            MenuNode::Item(e) => e.label.to_ascii_lowercase(),
            _ => String::new(),
        });
    }
    cats.into_iter().map(|(c, children)| MenuNode::Submenu { label: c.to_string(), children }).collect()
}

/// The After Effects menu layout. Labels use "..." like AE's macOS menus.
pub const TREE: &str = r#"
[mac] EffectCraft
  About EffectCraft... | app.about
  Appearance
    Dark | prefs.set {"key":"appearance.theme","value":"dark"}
    Darker | prefs.set {"key":"appearance.theme","value":"darker"}
    Light | prefs.set {"key":"appearance.theme","value":"light"}
    Studio Dark | prefs.set {"key":"appearance.theme","value":"studioDark"}
    Studio Light | prefs.set {"key":"appearance.theme","value":"studioLight"}
    Classic | prefs.set {"key":"appearance.theme","value":"classic"}
    ---
    Appearance Settings... | app.settings {"page":"appearance"}
  ---
  Settings...
    General... | app.settings {"page":"general"} | Cmd+Alt+;
    Startup & Repair... | app.settings {"page":"startup"}
    Project... | app.settings {"page":"project"}
    Composition... | app.settings {"page":"composition"}
    Previews... | app.settings {"page":"previews"}
    Appearance... | app.settings {"page":"appearance"}
    Grids & Guides... | app.settings {"page":"grids"}
    Labels... | app.settings {"page":"labels"}
    Type... | app.settings {"page":"type"}
    Import... | app.settings {"page":"import"}
    Export... | app.settings {"page":"export"}
    Audio... | app.settings {"page":"audio"}
    Disk... | app.settings {"page":"disk"}
    Memory & CPU... | app.settings {"page":"memory"}
    Video... | app.settings {"page":"video"}
    3D... | app.settings {"page":"3d"}
    Scripting & Expressions... | app.settings {"page":"scripting"}
  ---
  Hide EffectCraft | app.hide
  Hide Others | app.hideOthers
  Show All | app.showAll
  ---
  Quit EffectCraft | app.quit
File
  New
    New Project | file.newProject
    New Project from Template... | file.newFromTemplate
    New Folder | project.newFolder
  Open Project... | file.open
  Open Recent
    @dynamic:recentProjects
    ---
    Clear Recent Projects | file.clearRecent
  ---
  Close | file.close
  Close Project | file.closeProject
  Save | file.save
  Save As
    Save As... | file.saveAs
    Save a Copy... | file.saveCopy
    Save a Copy As XML... | file.saveCopyAsXml
  Save as Template... | templates.saveAs
  Increment and Save | file.incrementAndSave
  Revert | file.revert
  ---
  Import
    File... | file.import
    Multiple Files... | file.importMultiple
    Adobe Premiere Pro Project... | file.importTimeline
    Placeholder... | file.importPlaceholder
    Solid... | file.importSolid
    Lottie... | file.importLottie
    Vanishing Point (.vpe)... | file.importVanishingPoint
    Essential Graphics Template... | essential.importTemplate
  Import Recent Footage
    @dynamic:recentFootage
    ---
    Clear Recent Footage | file.clearRecentFootage
  Export
    Add to Render Queue | renderQueue.add
    Adobe Premiere Pro Project... | file.exportTimeline
    Lottie JSON... | file.exportLottie
    Essential Graphics Template... | essential.exportTemplate
  ---
  Find | app.find
  ---
  Add Footage to Comp | layer.addItem
  New Comp from Selection... | file.newCompFromSelection
  ---
  Dependencies
    Collect Files... | file.collectFiles
    Consolidate All Footage | file.consolidateFootage
    Remove Unused Footage | file.removeUnusedFootage
    Reduce Project | file.reduceProject
    Find Missing Effects | file.findMissing {"what":"effects"}
    Find Missing Fonts | file.findMissing {"what":"fonts"}
    Find Missing Footage | file.findMissing {"what":"footage"}
  Watch Folder... | file.watchFolder
  ---
  Scripts
    @dynamic:scripts
    ---
    Install Script File... | file.installScript
    Install ScriptUI Panel... | file.installScriptUIPanel
    Run Script File... | file.runScript
  ---
  Create Proxy
    Still... | file.createProxy {"kind":"still"}
    Movie... | file.createProxy {"kind":"movie"}
  Set Proxy
    File... | file.setProxy
    None | file.setProxyNone
  Interpret Footage
    Main... | file.interpretFootage
    Proxy... | file.interpretProxy
    Remember Interpretation | file.rememberInterpretation
    Apply Interpretation | file.applyInterpretation
  Replace Footage
    File... | file.replaceFootage
    Placeholder... | file.replaceWithPlaceholder
    Solid... | file.replaceWithSolid
    With Layered Comp | file.replaceWithLayeredComp
  Reload Footage | file.reloadFootage
  Reveal in Finder | file.revealInFinder
  ---
  Project Settings... | file.projectSettings
  [!mac] ---
  [!mac] Exit | app.quit
Edit
  Undo | edit.undo
  Redo | edit.redo
  History
    @dynamic:history
  ---
  Quick Apply... | app.commandPalette
  ---
  Cut | edit.cut
  Copy | edit.copy
  Copy with Property Links | edit.copyWithPropertyLinks
  Copy with Relative Property Links | edit.copyWithRelativePropertyLinks
  Copy Expression Only | edit.copyExpressionOnly
  Paste | edit.paste
  Paste Reversed Keyframes | edit.pasteReversedKeyframes
  Paste Text and Match Formatting | edit.pasteTextMatchFormatting
  Paste Text Formatting Only | edit.pasteTextFormattingOnly
  Clear | edit.clear
  ---
  Duplicate | edit.duplicate
  Split Layer | edit.splitLayer
  Lift Work Area | edit.liftWorkArea
  Extract Work Area | edit.extractWorkArea
  Select All | edit.selectAll
  Deselect All | edit.deselectAll
  ---
  Label
    Select Label Group | edit.selectLabelGroup
    ---
    None | edit.label {"label":"None"}
    Red | edit.label {"label":"Red"}
    Yellow | edit.label {"label":"Yellow"}
    Aqua | edit.label {"label":"Aqua"}
    Pink | edit.label {"label":"Pink"}
    Lavender | edit.label {"label":"Lavender"}
    Peach | edit.label {"label":"Peach"}
    Sea Foam | edit.label {"label":"Sea Foam"}
    Blue | edit.label {"label":"Blue"}
    Green | edit.label {"label":"Green"}
    Purple | edit.label {"label":"Purple"}
    Orange | edit.label {"label":"Orange"}
    Brown | edit.label {"label":"Brown"}
    Fuchsia | edit.label {"label":"Fuchsia"}
    Cyan | edit.label {"label":"Cyan"}
    Sandstone | edit.label {"label":"Sandstone"}
    Dark Green | edit.label {"label":"Dark Green"}
    ---
    Edit Label Colors... | app.settings {"page":"labels"}
  Select Keyframe Label Group
    On Selected Layers | keys.selectLabelGroup {"scope":"selected"}
    On All Layers | keys.selectLabelGroup {"scope":"all"}
    Visible Keyframes on Selected Layers | keys.selectLabelGroup {"scope":"visibleSelected"}
    Visible Keyframes on All Layers | keys.selectLabelGroup {"scope":"visibleAll"}
  ---
  Purge
    All Cache... | edit.purge {"what":"all"}
    All Memory & Disk Cache... | edit.purge {"what":"memoryAndDisk"}
    All Memory | edit.purge {"what":"memory"}
    All Disk Cache... | edit.purge {"what":"disk"}
    All 3D Cache... | edit.purge {"what":"3d"}
    Undo | edit.purgeUndo
    Image Cache Memory | edit.purge {"what":"image"}
    Snapshot | edit.purge {"what":"snapshot"}
  ---
  Edit Original... | edit.editOriginal
  ---
  Templates
    Render Settings... | app.templates {"kind":"renderSettings"}
    Output Module... | app.templates {"kind":"outputModule"}
  Keyboard Shortcuts | app.keyboardShortcuts
  [!mac] Preferences
    [!mac] General... | app.settings {"page":"general"} | Cmd+Alt+;
    [!mac] Startup & Repair... | app.settings {"page":"startup"}
    [!mac] Project... | app.settings {"page":"project"}
    [!mac] Composition... | app.settings {"page":"composition"}
    [!mac] Previews... | app.settings {"page":"previews"}
    [!mac] Appearance... | app.settings {"page":"appearance"}
    [!mac] Grids & Guides... | app.settings {"page":"grids"}
    [!mac] Labels... | app.settings {"page":"labels"}
    [!mac] Type... | app.settings {"page":"type"}
    [!mac] Import... | app.settings {"page":"import"}
    [!mac] Export... | app.settings {"page":"export"}
    [!mac] Audio... | app.settings {"page":"audio"}
    [!mac] Disk... | app.settings {"page":"disk"}
    [!mac] Memory & CPU... | app.settings {"page":"memory"}
    [!mac] Video... | app.settings {"page":"video"}
    [!mac] 3D... | app.settings {"page":"3d"}
    [!mac] Scripting & Expressions... | app.settings {"page":"scripting"}
Composition
  New Composition... | comp.new
  ---
  Composition Settings... | comp.settings
  Set Poster Time | comp.setPosterTime
  Trim Comp to Work Area | comp.trimToWorkArea
  Crop Comp to Region of Interest | comp.cropToRegionOfInterest
  Crop Comp to Selected Layer(s) Bounds | comp.cropToLayerBounds
  ---
  Add to Render Queue | renderQueue.add
  Add Output Module | render.addOutputModule
  ---
  Preview
    Play Current Preview | playback.toggle
    Cache Frames When Idle | playback.cacheWhenIdle
    Audio | playback.audio
  Save Frame As
    File... | comp.saveFrameAs
    Photoshop Layers... | comp.saveFrameAsPsd
    ProEXR... | comp.saveFrameAsExr
  Pre-render... | render.preRender
  Save Current Preview... | render.saveCurrentPreview
  ---
  Open in Essential Graphics | comp.openInEssentialGraphics
  Responsive Design — Time
    Create Intro | comp.responsiveTime {"op":"intro"}
    Create Outro | comp.responsiveTime {"op":"outro"}
    Create Protected Region from Work Area | comp.responsiveTime {"op":"workArea"}
  ---
  Composition Flowchart | comp.flowchart
  Composition Mini-Flowchart | comp.miniFlowchart
  ---
  VR
    Create VR Environment... | comp.vr.createEnvironment
    Extract Cubemap... | comp.vr.extractCubemap
Layer
  New
    Text | layer.newText
    Solid... | layer.newSolid
    Light... | layer.newLight
    Camera... | layer.newCamera
    Null Object | layer.newNull
    Shape Layer | layer.newShape
    Adjustment Layer | layer.newAdjustment
    Content-Aware Fill Layer... | layer.newContentAwareFill
    Cube | layer.new3dPrimitive {"kind":"cube"}
    Sphere | layer.new3dPrimitive {"kind":"sphere"}
    Plane | layer.new3dPrimitive {"kind":"plane"}
    Torus | layer.new3dPrimitive {"kind":"torus"}
    Cone | layer.new3dPrimitive {"kind":"cone"}
    Cylinder | layer.new3dPrimitive {"kind":"cylinder"}
  Layer Settings... | layer.settings
  ---
  Open Layer | layer.openLayer
  Open Layer Source | layer.openSource
  Reveal in Finder | layer.revealInFinder
  ---
  Mask
    New Mask | layer.addMask
    Mask Shape... | layer.mask.shape
    Mask Feather... | layer.mask.set {"field":"feather"} | Cmd+Shift+F
    Mask Opacity... | layer.mask.set {"field":"opacity"}
    Mask Expansion... | layer.mask.set {"field":"expansion"}
    Reset Mask | layer.mask.reset
    Remove Mask | layer.mask.remove
    Remove All Masks | layer.mask.removeAll
    Track Mask | track.mask
    Mode
      None | layer.mask.mode {"mode":"None"}
      Add | layer.mask.mode {"mode":"Add"}
      Subtract | layer.mask.mode {"mode":"Subtract"}
      Intersect | layer.mask.mode {"mode":"Intersect"}
      Lighten | layer.mask.mode {"mode":"Lighten"}
      Darken | layer.mask.mode {"mode":"Darken"}
      Difference | layer.mask.mode {"mode":"Difference"}
    Inverted | layer.mask.invert
    Locked | layer.mask.lock
    Motion Blur
      Same As Layer | layer.mask.motionBlur {"mode":"sameAsLayer"}
      On | layer.mask.motionBlur {"mode":"on"}
      Off | layer.mask.motionBlur {"mode":"off"}
    Feather Falloff
      Smooth | layer.mask.featherFalloff {"mode":"smooth"}
      Linear | layer.mask.featherFalloff {"mode":"linear"}
    Unlock All Masks | layer.mask.unlockAll
    Lock Other Masks | layer.mask.lockOthers
    Hide Locked Masks | layer.mask.hideLocked
  Mask and Shape Path
    RotoBezier | path.rotoBezier
    Closed | mask.setClosed
    Convert To Bezier Path | path.convertToBezier
    Set First Vertex | path.setFirstVertex
    Free Transform Points | path.freeTransform
  Quality
    Best | layer.quality {"quality":"best"} | Cmd+U
    Draft | layer.quality {"quality":"draft"} | Cmd+Shift+U
    Wireframe | layer.quality {"quality":"wireframe"} | Cmd+Alt+Shift+U
    ---
    Bilinear | layer.sampling {"sampling":"bilinear"}
    Bicubic | layer.sampling {"sampling":"bicubic"}
  Switches
    Hide Other Video | layer.hideOtherVideo
    Show All Video | layer.showAllVideo
    Unlock All Layers | layer.unlockAll
    Enable Expressions | layer.expressions {"enabled":true}
    Disable Expressions | layer.expressions {"enabled":false}
    ---
    Shy | layer.setSwitch {"switch":"shy"}
    Lock | layer.setSwitch {"switch":"lock"} | Cmd+L
    Audio | layer.setSwitch {"switch":"audio"}
    Video | layer.setSwitch {"switch":"video"}
    Solo | layer.setSwitch {"switch":"solo"}
    Effect | layer.setSwitch {"switch":"fx"}
    Collapse | layer.setSwitch {"switch":"collapse"}
    Motion Blur | layer.setSwitch {"switch":"motionBlur"}
    Adjustment Layer | layer.setSwitch {"switch":"adjustment"}
  Transform
    Reset | layer.transform {"op":"reset"}
    Anchor Point... | layer.setTransform {"prop":"anchor"}
    Position... | layer.setTransform {"prop":"position"} | Cmd+Shift+P
    Scale... | layer.setTransform {"prop":"scale"}
    Orientation... | layer.setTransform {"prop":"orientation"} | Cmd+Alt+Shift+R
    Rotation... | layer.setTransform {"prop":"rotation"} | Cmd+Shift+R
    Opacity... | layer.setTransform {"prop":"opacity"} | Cmd+Shift+O
    ---
    Flip Horizontal | layer.transform {"op":"flipH"}
    Flip Vertical | layer.transform {"op":"flipV"}
    Center In View | layer.transform {"op":"center"} | Cmd+Home
    Center Anchor Point in Layer Content | layer.centerAnchor
    Fit to Comp | layer.transform {"op":"fit"} | Cmd+Alt+F
    Fit to Comp Width | layer.transform {"op":"fitWidth"} | Cmd+Alt+Shift+H
    Fit to Comp Height | layer.transform {"op":"fitHeight"} | Cmd+Alt+Shift+G
    ---
    Auto-Orient... | layer.autoOrient
  Time
    Enable Time Remapping | layer.enableTimeRemap
    Time-Reverse Layer | layer.timeReverse
    Time Stretch... | layer.timeStretch
    Freeze Frame | layer.freezeFrame
    Freeze On Last Frame | layer.freezeOnLastFrame
    Align Video to Data | layer.alignVideoToData
  Frame Blending
    Off | layer.frameBlending {"mode":"off"}
    Frame Mix | layer.frameBlending {"mode":"frameMix"}
    Pixel Motion | layer.frameBlending {"mode":"pixelMotion"}
  3D Layer | layer.setSwitch {"switch":"threeD"}
  Guide Layer | layer.setSwitch {"switch":"guide"}
  Environment Layer | layer.environment
  Markers
    Add Marker | layer.addMarker
    Update Markers From Source | layer.updateMarkersFromSource
    Lock Markers | layer.markersLock
    Delete All Markers | layer.deleteAllMarkers
  ---
  Preserve Transparency | layer.setSwitch {"switch":"preserveTransparency"}
  Blending Mode
    Normal | layer.setBlendMode {"mode":"Normal"}
    Dissolve | layer.setBlendMode {"mode":"Dissolve"}
    Dancing Dissolve | layer.setBlendMode {"mode":"Dancing Dissolve"}
    ---
    Darken | layer.setBlendMode {"mode":"Darken"}
    Multiply | layer.setBlendMode {"mode":"Multiply"}
    Color Burn | layer.setBlendMode {"mode":"Color Burn"}
    Classic Color Burn | layer.setBlendMode {"mode":"Classic Color Burn"}
    Linear Burn | layer.setBlendMode {"mode":"Linear Burn"}
    Darker Color | layer.setBlendMode {"mode":"Darker Color"}
    ---
    Add | layer.setBlendMode {"mode":"Add"}
    Lighten | layer.setBlendMode {"mode":"Lighten"}
    Screen | layer.setBlendMode {"mode":"Screen"}
    Color Dodge | layer.setBlendMode {"mode":"Color Dodge"}
    Classic Color Dodge | layer.setBlendMode {"mode":"Classic Color Dodge"}
    Linear Dodge | layer.setBlendMode {"mode":"Linear Dodge"}
    Lighter Color | layer.setBlendMode {"mode":"Lighter Color"}
    ---
    Overlay | layer.setBlendMode {"mode":"Overlay"}
    Soft Light | layer.setBlendMode {"mode":"Soft Light"}
    Hard Light | layer.setBlendMode {"mode":"Hard Light"}
    Linear Light | layer.setBlendMode {"mode":"Linear Light"}
    Vivid Light | layer.setBlendMode {"mode":"Vivid Light"}
    Pin Light | layer.setBlendMode {"mode":"Pin Light"}
    Hard Mix | layer.setBlendMode {"mode":"Hard Mix"}
    ---
    Difference | layer.setBlendMode {"mode":"Difference"}
    Classic Difference | layer.setBlendMode {"mode":"Classic Difference"}
    Exclusion | layer.setBlendMode {"mode":"Exclusion"}
    Subtract | layer.setBlendMode {"mode":"Subtract"}
    Divide | layer.setBlendMode {"mode":"Divide"}
    ---
    Hue | layer.setBlendMode {"mode":"Hue"}
    Saturation | layer.setBlendMode {"mode":"Saturation"}
    Color | layer.setBlendMode {"mode":"Color"}
    Luminosity | layer.setBlendMode {"mode":"Luminosity"}
    ---
    Stencil Alpha | layer.setBlendMode {"mode":"Stencil Alpha"}
    Stencil Luma | layer.setBlendMode {"mode":"Stencil Luma"}
    Silhouette Alpha | layer.setBlendMode {"mode":"Silhouette Alpha"}
    Silhouette Luma | layer.setBlendMode {"mode":"Silhouette Luma"}
    ---
    Alpha Add | layer.setBlendMode {"mode":"Alpha Add"}
    Luminescent Premul | layer.setBlendMode {"mode":"Luminescent Premul"}
  Next Blending Mode | layer.setBlendMode {"step":1} | Shift+=
  Previous Blending Mode | layer.setBlendMode {"step":-1} | Shift+-
  Track Matte
    No Track Matte | layer.trackMatte {"op":"none"}
    Alpha Matte | layer.trackMatte {"op":"alpha"}
    Alpha Inverted Matte | layer.trackMatte {"op":"alphaInverted"}
    Luma Matte | layer.trackMatte {"op":"luma"}
    Luma Inverted Matte | layer.trackMatte {"op":"lumaInverted"}
    ---
    Matte with Layer Above | layer.trackMatte {"op":"above"}
    Matte with Layer Below | layer.trackMatte {"op":"below"}
  Layer Styles
    Layer Style Options... | layer.style.options
    ---
    Convert to Editable Styles | layer.style.convertToEditable
    Show All | layer.style.showAll
    Remove All | layer.style.removeAll
    ---
    Drop Shadow | layer.style.dropShadow
    Inner Shadow | layer.style.innerShadow
    Outer Glow | layer.style.outerGlow
    Inner Glow | layer.style.innerGlow
    Bevel and Emboss | layer.style.bevelEmboss
    Satin | layer.style.satin
    Color Overlay | layer.style.colorOverlay
    Gradient Overlay | layer.style.gradientOverlay
    Stroke | layer.style.stroke
  Group Shapes | path.groupShapes
  Ungroup Shapes | path.ungroupShapes
  ---
  Arrange
    Bring Layer to Front | layer.arrange {"to":"front"} | Cmd+Shift+]
    Bring Layer Forward | layer.arrange {"to":"forward"} | Cmd+]
    Send Layer Backward | layer.arrange {"to":"backward"} | Cmd+[
    Send Layer to Back | layer.arrange {"to":"back"} | Cmd+Shift+[
  ---
  Reveal
    Reveal in Finder | layer.revealInFinder
    Reveal Layer Source in Project | layer.revealSource
    Reveal Layer in Project Flowchart | comp.flowchart
    Reveal Composition in Project | comp.revealInProject
    Reveal Expression Errors | layer.revealExpressionErrors
  Create
    Convert to Editable Text | layer.create {"op":"editableText"}
    Create Shapes from Text | layer.create {"op":"shapesFromText"}
    Create Masks from Text | layer.create {"op":"masksFromText"}
    Create Shapes from Vector Layer | layer.create {"op":"shapesFromVector"}
    Create Keyframes from Data | layer.create {"op":"keyframesFromData"}
    Null Controllers for Positional Points | layer.create {"op":"nullsPositional"}
    Null Controllers for Path Points | layer.create {"op":"nullsPathPoints"}
    Nulls Following Path Points | layer.create {"op":"nullsFollowing"}
    Null Tracing Along Path | layer.create {"op":"nullTracing"}
    Create 3D Layer Instance | layer.create {"op":"instance3d"}
  ---
  Camera
    Create Camera from 3D View | camera.fromView
    Create Stereo 3D Rig | camera.stereoRig
    Create Orbit Null | camera.orbitNull
    Create Cameras from 3D Model | camera.fromModel
    Link Focus Distance to Point of Interest | camera.linkFocusToPoi
    Link Focus Distance to Layer | camera.linkFocusToLayer
    Set Focus Distance to Layer | camera.setFocusToLayer
    Camera Settings... | layer.cameraSettings
    Reset 3D View | view.reset3DView
  Light
    Create Lights from 3D Model | light.fromModel
    Control Light with Camera | light.controlWithCamera
    Create Environment Light Background Layer | light.environmentBackground
  Material
    Reveal Material Source in Project | material.revealSource
    Reset Material | material.reset
    Duplicate and Assign Material | material.duplicateAssign
  Auto-trace... | layer.autoTrace
  Pre-compose... | layer.precompose
  Scene Edit Detection... | layer.sceneEditDetection
Effect
  Effect Controls | window.panel {"panel":"effectControls"} | F3
  Last Effect | effect.applyLast
  Remove All | effect.removeAll
  ---
  Manage Effects... | effect.manage
  Load Effect Plug-in... | effect.plugins.load
  ---
  @effects
Animation
  Save Animation Preset... | anim.savePreset
  Apply Animation Preset... | anim.applyPreset
  Recent Animation Presets
    @dynamic:recentPresets
    ---
    Clear Recent Presets | anim.clearRecentPresets
  Browse Presets... | anim.browsePresets
  Text Animation Presets
    Typewriter | layer.applyTextPreset {"preset":"typewriter"}
    Fade Up Characters | layer.applyTextPreset {"preset":"fadeUpCharacters"}
    Bounce In Words | layer.applyTextPreset {"preset":"bounceInWords"}
    Tracking In | layer.applyTextPreset {"preset":"trackingIn"}
    Scramble | layer.applyTextPreset {"preset":"scramble"}
    Blur In | layer.applyTextPreset {"preset":"blurIn"}
    Jitter | layer.applyTextPreset {"preset":"jitter"}
    Drop In Lines | layer.applyTextPreset {"preset":"dropInLines"}
  ---
  Add Keyframe | anim.addKeyframe
  Toggle Hold Keyframe | keys.toggleHold
  Keyframe Interpolation... | keys.interpolation
  Keyframe Velocity... | keys.velocity
  Keyframe Assistant
    Convert Audio to Keyframes | keys.audioToKeyframes
    Convert Expression to Keyframes | prop.convertExpressionToKeyframes
    Create Keyframes from Data | layer.create {"op":"keyframesFromData"}
    Easy Ease | keys.easyEase
    Easy Ease In | keys.easyEaseIn
    Easy Ease Out | keys.easyEaseOut
    Exponential Scale | keys.exponentialScale
    Null Controllers for Path Points | layer.create {"op":"nullsPathPoints"}
    Null Controllers for Positional Points | layer.create {"op":"nullsPositional"}
    Null Tracing Along Path | layer.create {"op":"nullTracing"}
    Nulls Following Path Points | layer.create {"op":"nullsFollowing"}
    RPF Camera Import | keys.rpfCameraImport
    Sequence Layers... | layer.sequence
    Time-Reverse Keyframes | keys.timeReverse
  ---
  Animate Text
    Enable Per-character 3D | layer.enablePerChar3D {"toggle":true}
    ---
    Anchor Point | layer.addTextAnimator {"properties":["anchor"]}
    Position | layer.addTextAnimator {"properties":["position"]}
    Scale | layer.addTextAnimator {"properties":["scale"]}
    Skew | layer.addTextAnimator {"properties":["skew"]}
    Rotation | layer.addTextAnimator {"properties":["rotation"]}
    Opacity | layer.addTextAnimator {"properties":["opacity"]}
    ---
    All Transform Properties | layer.addTextAnimator {"properties":["transformAll"]}
    ---
    Fill Color
      RGB | layer.addTextAnimator {"properties":["fillColor"]}
      Hue | layer.addTextAnimator {"properties":["fillHue"]}
      Saturation | layer.addTextAnimator {"properties":["fillSaturation"]}
      Brightness | layer.addTextAnimator {"properties":["fillBrightness"]}
      Opacity | layer.addTextAnimator {"properties":["fillOpacity"]}
    Stroke Color
      RGB | layer.addTextAnimator {"properties":["strokeColor"]}
      Hue | layer.addTextAnimator {"properties":["strokeHue"]}
      Saturation | layer.addTextAnimator {"properties":["strokeSaturation"]}
      Brightness | layer.addTextAnimator {"properties":["strokeBrightness"]}
      Opacity | layer.addTextAnimator {"properties":["strokeOpacity"]}
    Stroke Width | layer.addTextAnimator {"properties":["strokeWidth"]}
    ---
    Tracking | layer.addTextAnimator {"properties":["tracking"]}
    Line Anchor | layer.addTextAnimator {"properties":["lineAnchor"]}
    Line Spacing | layer.addTextAnimator {"properties":["lineSpacing"]}
    ---
    Character Offset | layer.addTextAnimator {"properties":["characterOffset"]}
    Character Value | layer.addTextAnimator {"properties":["characterValue"]}
    ---
    Blur | layer.addTextAnimator {"properties":["blur"]}
    Variable Font Axes | text.animatorFontAxes
  Add Text Selector
    Range | text.addSelector {"kind":"range"}
    Wiggly | text.addSelector {"kind":"wiggly"}
    Expression | text.addSelector {"kind":"expression"}
  Remove All Text Animators | text.removeAllAnimators
  ---
  Add Expression | prop.setExpression
  Add Property to Essential Graphics | essential.addProperty
  Separate Dimensions | prop.separateDimensions
  Track Camera | track.camera
  Warp Stabilizer VFX | track.warpStabilizer
  Track Motion | track.motion
  Stabilize Motion | track.stabilize
  Track Mask | track.mask
  Track this Property | track.property
  ---
  Reveal Properties with Keyframes | anim.reveal {"kind":"keyframes"} | U
  Reveal Properties with Animation | anim.reveal {"kind":"animation"}
  Reveal All Modified Properties | anim.reveal {"kind":"modified"}
View
  [!mac] Appearance
    Dark | prefs.set {"key":"appearance.theme","value":"dark"}
    Darker | prefs.set {"key":"appearance.theme","value":"darker"}
    Light | prefs.set {"key":"appearance.theme","value":"light"}
    Studio Dark | prefs.set {"key":"appearance.theme","value":"studioDark"}
    Studio Light | prefs.set {"key":"appearance.theme","value":"studioLight"}
    Classic | prefs.set {"key":"appearance.theme","value":"classic"}
    ---
    Appearance Settings... | app.settings {"page":"appearance"}
  New Viewer | view.newViewer
  Split with New Locked Viewer | view.splitLockedViewer
  ---
  Zoom In | view.zoomIn
  Zoom Out | view.zoomOut
  ---
  Resolution
    Full | view.res.full
    Half | view.res.half
    Third | view.res.third
    Quarter | view.res.quarter
    Custom... | view.res.custom
  ---
  Use Display Color Management | view.displayColorManagement
  Simulate Output
    No Output Simulation | view.simulateOutput {"profile":"none"}
    HDTV (Rec. 709) | view.simulateOutput {"profile":"rec709"}
    SDTV NTSC | view.simulateOutput {"profile":"ntsc"}
    SDTV PAL | view.simulateOutput {"profile":"pal"}
    Legacy Macintosh RGB (Gamma 1.8) | view.simulateOutput {"profile":"mac18"}
    Internet Standard RGB (sRGB) | view.simulateOutput {"profile":"srgb"}
    UHDTV (Rec. 2020) | view.simulateOutput {"profile":"rec2020"}
    Display P3 | view.simulateOutput {"profile":"p3"}
    Linear (1.0 Gamma) | view.simulateOutput {"profile":"linear"}
    ---
    My Custom RGB... | view.customRgb
    Custom... | view.simulateOutput {"profile":"custom"}
  ---
  Show Rulers | view.rulers
  Panel Background Color
    Black | view.panelBackground {"color":"black"}
    Dark Gray | view.panelBackground {"color":"darkGray"}
    Medium Gray (Default) | view.panelBackground {"color":"mediumGray"}
    Light Gray | view.panelBackground {"color":"lightGray"}
    White | view.panelBackground {"color":"white"}
    ---
    Custom | view.panelBackground {"color":"custom"}
    Select Custom Background Color... | view.panelBackground {"pick":true}
  ---
  Show Guides | view.guides
  Snap to Guides | view.snapToGuides
  Lock Guides | view.lockGuides
  Add Guide... | view.addGuide
  Clear Guides | view.clearGuides
  Import Guides... | view.importGuides
  Export Guides... | view.exportGuides
  ---
  Show Grid | view.grid
  Snap to Grid | view.snapToGrid
  Snapping | view.snapping
  ---
  View Options... | view.options
  Show Layer Controls | view.layerControls
  ---
  Camera Settings... | layer.cameraSettings
  Reset 3D View | view.reset3DView
  Create Camera from 3D View | camera.fromView
  Switch View Layout
    1 View | view.layout {"views":1}
    2 Views | view.layout {"views":2}
    4 Views | view.layout {"views":4}
    ---
    Share View Options | view.shareViewOptions
  Switch 3D View
    Active Camera | view.3d.activeCamera
    Default | view.3d.default
    ---
    Front | view.3d.front
    Left | view.3d.left
    Top | view.3d.top
    Back | view.3d.back
    Right | view.3d.right
    Bottom | view.3d.bottom
    ---
    Custom View 1 | view.3d.custom1
    Custom View 2 | view.3d.custom2
    Custom View 3 | view.3d.custom3
    ---
    Reset 3D View | view.reset3DView
  Assign Shortcut to 3D View
    @dynamic:view3dShortcuts
  Switch to Last 3D View | view.3d.last | Escape
  Look at Selected Layers | view.lookAtSelected
  Look at All Layers | view.lookAtAll
  ---
  Go to Time... | time.set
  Enter Full Screen | view.fullScreen
Window
  Workspace
    Default | window.workspace {"name":"Default"} | Shift+F10
    Review | window.workspace {"name":"Review"}
    Learn | window.workspace {"name":"Learn"}
    Small Screen | window.workspace {"name":"Small Screen"} | Shift+F12
    Standard | window.workspace {"name":"Standard"} | Shift+F11
    ---
    All Panels | window.workspace {"name":"All Panels"}
    Animation | window.workspace {"name":"Animation"}
    Color | window.workspace {"name":"Color"}
    Effects | window.workspace {"name":"Effects"}
    Essential Graphics | window.workspace {"name":"Essential Graphics"}
    Minimal | window.workspace {"name":"Minimal"}
    Motion Tracking | window.workspace {"name":"Motion Tracking"}
    Paint | window.workspace {"name":"Paint"}
    Text | window.workspace {"name":"Text"}
    Undocked Panels | window.workspace {"name":"Undocked Panels"}
    @dynamic:savedWorkspaces
    ---
    Reset to Saved Layout | window.resetWorkspace
    Save Changes to this Workspace | window.saveWorkspace
    Save as New Workspace... | window.saveWorkspaceAs
    Edit Workspaces... | window.editWorkspaces
  Assign Shortcut to Workspace
    @dynamic:workspaceShortcuts
  ---
  Align | window.panel {"panel":"align"}
  Audio | window.panel {"panel":"audio"} | Cmd+4
  Brushes | window.panel {"panel":"brushes"} | Cmd+9
  Character | window.panel {"panel":"character"} | Cmd+6
  Content-Aware Fill | window.panel {"panel":"contentAwareFill"}
  Effects & Presets | window.panel {"panel":"effectsPresets"} | Cmd+5
  Essential Graphics | window.panel {"panel":"essentialGraphics"}
  Info | window.panel {"panel":"info"} | Cmd+2
  Learn | help.inAppTutorials
  Lumetri Scopes | window.panel {"panel":"lumetriScopes"}
  Mask Interpolation | window.panel {"panel":"maskInterpolation"}
  Media Browser | window.panel {"panel":"mediaBrowser"}
  Metadata | window.panel {"panel":"metadata"}
  Motion Sketch | window.panel {"panel":"motionSketch"}
  Paint | window.panel {"panel":"paint"} | Cmd+8
  Paragraph | window.panel {"panel":"paragraph"} | Cmd+7
  Preview | window.panel {"panel":"preview"} | Cmd+3
  Progress | window.panel {"panel":"progress"}
  Properties | window.panel {"panel":"properties"}
  Script Console | window.panel {"panel":"scriptConsole"}
  Smoother | window.panel {"panel":"smoother"}
  Tools | window.panel {"panel":"tools"} | Cmd+1
  Tracker | window.panel {"panel":"tracker"}
  Wiggler | window.panel {"panel":"wiggler"}
  ---
  Composition | window.panel {"panel":"composition"}
  Effect Controls | window.panel {"panel":"effectControls"} | F3
  Flowchart | window.panel {"panel":"flowchart"} | Cmd+F11
  Footage | window.panel {"panel":"footage"}
  Layer | window.panel {"panel":"layer"}
  Project | window.panel {"panel":"project"} | Cmd+0
  Render Queue | window.panel {"panel":"renderQueue"} | Cmd+Alt+0
  Timeline | window.panel {"panel":"timeline"}
  @dynamic:openViewers
  ---
  Create Nulls From Paths | window.panel {"panel":"createNullsFromPaths"}
  VR Comp Editor | window.panel {"panel":"vrCompEditor"}
  @dynamic:scriptPanels
Help
  EffectCraft Help... | help.docs {"page":"help"} | F1
  Scripting Help... | help.docs {"page":"scripting"}
  Expression Reference... | help.docs {"page":"expressions"}
  Effect Reference... | help.docs {"page":"effects"}
  Animation Presets... | anim.browsePresets
  Keyboard Shortcuts... | app.keyboardShortcuts
  ---
  In-App Tutorials... | help.inAppTutorials
  Online Tutorials... | help.onlineTutorials
  ---
  System Compatibility Report... | help.systemReport
  Enable Logging | help.enableLogging
  Reveal Logging File | help.revealLogFile
  ---
  Join the ArtCraft Discord... | help.discord
  Provide Feedback... | help.reportIssue
  ---
  ArtCraft Website | help.website
  EffectCraft Home Page | help.appPage
  EffectCraft on GitHub | help.github
  Open Demo Project | file.openDemoProject
  [!mac] ---
  [!mac] About EffectCraft... | app.about
"#;

#[cfg(test)]
mod tests;
