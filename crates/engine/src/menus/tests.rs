use std::collections::BTreeMap;

use super::*;

fn all_entries(nodes: &[MenuNode], path: &mut Vec<String>, out: &mut Vec<(Vec<String>, MenuEntry)>) {
    for n in nodes {
        match n {
            MenuNode::Item(e) => out.push((path.clone(), e.clone())),
            MenuNode::Separator | MenuNode::Dynamic { .. } => {}
            MenuNode::Submenu { label, children } => {
                path.push(label.clone());
                all_entries(children, path, out);
                path.pop();
            }
        }
    }
}

fn platform_entries(mac: bool) -> Vec<(Vec<String>, MenuEntry)> {
    let tree = parse(TREE, mac).unwrap();
    let mut out = vec![];
    all_entries(&tree, &mut vec![], &mut out);
    out
}

#[test]
fn top_level_order_matches_after_effects() {
    let mac: Vec<String> =
        parse(TREE, true).unwrap().iter().filter_map(|n| if let MenuNode::Submenu { label, .. } = n { Some(label.clone()) } else { None }).collect();
    assert_eq!(mac, ["Visualab Effects", "File", "Edit", "Composition", "Layer", "Effect", "Animation", "View", "Window", "Help"]);
    let other: Vec<String> =
        parse(TREE, false).unwrap().iter().filter_map(|n| if let MenuNode::Submenu { label, .. } = n { Some(label.clone()) } else { None }).collect();
    assert_eq!(other, ["File", "Edit", "Composition", "Layer", "Effect", "Animation", "View", "Window", "Help"]);
    // The live bar is one of the two.
    assert!(top_level().len() == mac.len() || top_level().len() == other.len());
}

#[test]
fn every_menu_entry_resolves_to_a_registered_command() {
    for mac in [true, false] {
        for (path, e) in platform_entries(mac) {
            assert!(crate::commands::find(&e.command).is_some(), "{} ▸ {}: unknown command `{}`", path.join(" ▸ "), e.label, e.command);
            assert!(e.params.is_null() || e.params.is_object(), "{}: params must be an object", e.label);
        }
    }
}

#[test]
fn command_menu_paths_and_labels_agree_with_the_tree() {
    let entries = platform_entries(true);
    let mut by_cmd: BTreeMap<&str, Vec<&(Vec<String>, MenuEntry)>> = BTreeMap::new();
    for e in &entries {
        by_cmd.entry(e.1.command.as_str()).or_default().push(e);
    }
    for spec in crate::command_specs() {
        if spec.menu.is_empty() {
            continue;
        }
        let places = by_cmd.get(spec.id).unwrap_or_else(|| panic!("`{}` declares menu {:?} but is not in the menu tree", spec.id, spec.menu));
        assert!(
            places.iter().any(|(p, _)| p.iter().map(String::as_str).eq(spec.menu.iter().copied())),
            "`{}` declares menu {:?}; the tree has it at {:?}",
            spec.id,
            spec.menu,
            places.iter().map(|(p, _)| p.join(" ▸ ")).collect::<Vec<_>>()
        );
        for (p, e) in places {
            if e.params.is_null() && p.iter().map(String::as_str).eq(spec.menu.iter().copied()) {
                assert_eq!(e.label, spec.label, "label of `{}`", spec.id);
            }
        }
    }
}

#[test]
fn dialog_labels_use_ascii_ellipsis() {
    for (_, e) in platform_entries(true) {
        assert!(!e.label.contains('…'), "`{}` uses a Unicode ellipsis; AE's menus use \"...\"", e.label);
    }
}

#[test]
fn shortcuts_are_unambiguous() {
    // shortcut → (command, params) from command defaults and menu entries.
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut add = |sc: &str, id: &str, params: String| {
        if sc == "Num*" {
            return;
        }
        if let Some(prev) = seen.get(sc) {
            assert_eq!(prev, &(id.to_string(), params.clone()), "shortcut {sc} is bound twice");
        } else {
            seen.insert(sc.to_string(), (id.to_string(), params));
        }
    };
    for (_, e) in platform_entries(true) {
        if let Some(sc) = &e.shortcut {
            add(sc, &e.command, e.params.to_string());
        }
    }
    for c in crate::command_specs() {
        // Defaults of commands whose menu entries bind params are carried by the entries.
        if let Some(sc) = c.shortcut
            && !platform_entries(true).iter().any(|(_, e)| e.command == c.id && !e.params.is_null())
        {
            add(sc, c.id, "null".into());
        }
    }
}

#[test]
fn menus_cover_after_effects_structure() {
    let entries = platform_entries(true);
    let count = |m: &str| entries.iter().filter(|(p, _)| p[0] == m).count();
    // Floors (AE 2026 minus Adobe-service, history and user-specific entries).
    assert!(count("File") >= 40, "File {}", count("File"));
    assert!(count("Edit") >= 54, "Edit {}", count("Edit"));
    assert!(count("Composition") >= 19, "Composition {}", count("Composition"));
    assert!(count("Layer") >= 197, "Layer {}", count("Layer"));
    assert!(count("Animation") >= 53, "Animation {}", count("Animation"));
    assert!(count("View") >= 59, "View {}", count("View"));
    assert!(count("Window") >= 43, "Window {}", count("Window"));
    // 38 blending modes + next/previous.
    let modes = entries.iter().filter(|(p, e)| p.last().map(String::as_str) == Some("Blending Mode") && e.command == "layer.setBlendMode").count();
    assert_eq!(modes, 38);
    // 16 label colours + None.
    let labels = entries.iter().filter(|(_, e)| e.command == "edit.label").count();
    assert_eq!(labels, 17);
}

#[test]
fn checked_reflects_layer_state() {
    let mut s = crate::Session::default();
    s.execute("comp.new", serde_json::json!({"width": 320, "height": 180})).unwrap();
    s.execute("layer.newSolid", serde_json::json!({})).unwrap();
    s.execute("layer.setBlendMode", serde_json::json!({"mode": "Screen"})).unwrap();
    assert_eq!(checked(&s, "layer.setBlendMode", &serde_json::json!({"mode": "Screen"})), Some(true));
    assert_eq!(checked(&s, "layer.setBlendMode", &serde_json::json!({"mode": "Normal"})), Some(false));
    assert_eq!(checked(&s, "layer.quality", &serde_json::json!({"quality": "best"})), Some(true));
    assert_eq!(checked(&s, "file.save", &serde_json::json!({})), None);
}

/// `cargo test -p effectcraft-engine dump_specs -- --ignored --nocapture`: every command as TSV.
#[test]
#[ignore]
fn dump_specs() {
    for c in crate::command_specs() {
        eprintln!("SPEC\t{}\t{}\t{}\t{}\t{}", c.id, c.label, c.menu.join(" > "), c.shortcut.unwrap_or(""), c.params);
    }
}

#[test]
fn parse_rejects_bad_trees() {
    assert!(parse("File\n  Empty\n", true).is_err());
    assert!(parse("File\n   Odd | file.save\n", true).is_err());
    assert!(parse("File\n  Bad | file.save {nope}\n", true).is_err());
}

#[test]
fn m3_13_entries_follow_after_effects() {
    let find = |mac: bool, path: &[&str], label: &str| platform_entries(mac).into_iter().find(|(p, e)| p == path && e.label == label).map(|(_, e)| e);
    // Group / Ungroup Shapes sit in the Layer menu itself.
    for mac in [true, false] {
        assert_eq!(find(mac, &["Layer"], "Group Shapes").unwrap().shortcut.as_deref(), Some("Cmd+G"));
        assert_eq!(find(mac, &["Layer"], "Ungroup Shapes").unwrap().shortcut.as_deref(), Some("Cmd+Shift+G"));
        assert!(find(mac, &["Layer", "Mask and Shape Path"], "Group Shapes").is_none());
        // Composition Flowchart Ctrl+Shift+F11, Window ▸ Flowchart Ctrl+F11; Window ▸ Learn.
        assert_eq!(find(mac, &["Composition"], "Composition Flowchart").unwrap().shortcut.as_deref(), Some("Cmd+Shift+F11"));
        assert_eq!(find(mac, &["Window"], "Flowchart").unwrap().shortcut.as_deref(), Some("Cmd+F11"));
        assert_eq!(find(mac, &["Window"], "Learn").unwrap().command, "help.inAppTutorials");
        assert_eq!(find(mac, &["Layer", "Transform"], "Center In View").unwrap().shortcut.as_deref(), Some("Cmd+Home"));
    }
    // "Reveal in Finder" names the platform's file browser.
    assert!(find(true, &["File"], "Reveal in Finder").is_some());
    let other = if cfg!(target_os = "windows") { "Reveal in Explorer" } else { "Reveal in File Manager" };
    for path in [&["File"][..], &["Layer"], &["Layer", "Reveal"]] {
        assert!(find(false, path, other).is_some(), "{path:?}");
        assert!(find(false, path, "Reveal in Finder").is_none(), "{path:?}");
    }
    assert_eq!(platform_label("Reveal Layer Source in Project", false), "Reveal Layer Source in Project");
}
