//! The menu bar with remote storage: with [`crate::Hooks::remote`] the File menu's local entries
//! say "Local" and the remote ones follow them. Without it the engine's menu bar is used as is.

use std::borrow::Cow;

use effectcraft_engine::menus::{MenuEntry, MenuNode, menu_bar};
use serde_json::Value;

use crate::EffectcraftApp;

/// A menu entry running `command`.
fn item(label: &str, command: &str) -> MenuNode {
    MenuNode::Item(MenuEntry { label: label.into(), command: command.into(), params: Value::Null, shortcut: None, alt_shortcuts: Vec::new() })
}

/// Relabel the entry that runs `command` and add `after` (label, command) behind it.
fn change(nodes: &[MenuNode], command: &str, label: &str, after: Option<(&str, &str)>) -> Vec<MenuNode> {
    let mut out = Vec::with_capacity(nodes.len() + 1);
    for n in nodes {
        match n {
            MenuNode::Item(e) if e.command == command => {
                let mut e = e.clone();
                if !label.is_empty() {
                    e.label = label.into();
                }
                out.push(MenuNode::Item(e));
                if let Some((l, c)) = after {
                    out.push(item(l, c));
                }
            }
            n => out.push(n.clone()),
        }
    }
    out
}

/// The File menu with the remote entries added.
fn file_menu(children: &[MenuNode]) -> Vec<MenuNode> {
    let mut v = change(children, "file.open", "Open Local Project...", Some(("Open Remote Project...", "file.openRemote")));
    for n in &mut v {
        let MenuNode::Submenu { label, children } = n else { continue };
        match label.as_str() {
            "Save As" => *children = change(children, "file.saveAs", "", Some(("Save As Remote...", "file.saveAsRemote"))),
            "Import" => {
                let c = change(children, "file.import", "Local File...", None);
                *children = change(&c, "file.importMultiple", "Multiple Local Files...", Some(("Import from Remote...", "file.importRemote")));
            }
            _ => {}
        }
    }
    v
}

/// The menu bar to show.
pub(crate) fn bar(app: &EffectcraftApp) -> Cow<'static, [MenuNode]> {
    if app.hooks.remote.is_none() {
        return Cow::Borrowed(menu_bar());
    }
    Cow::Owned(
        menu_bar()
            .iter()
            .map(|n| match n {
                MenuNode::Submenu { label, children } if label == "File" => MenuNode::Submenu { label: label.clone(), children: file_menu(children) },
                n => n.clone(),
            })
            .collect(),
    )
}

/// Every entry of `nodes` with its submenu path, depth first (as `effectcraft_engine::menus::entries`).
pub(crate) fn entries(nodes: &[MenuNode]) -> Vec<(Vec<String>, &MenuEntry)> {
    /// Collect the entries of `nodes` with their menu paths.
    fn rec<'a>(nodes: &'a [MenuNode], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, &'a MenuEntry)>) {
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
    let mut out = Vec::new();
    rec(nodes, &mut Vec::new(), &mut out);
    out
}
