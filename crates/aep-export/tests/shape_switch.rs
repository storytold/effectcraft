//! A shape item's eye switch (986, 987): bit 0 of the item group's `tdsb`. After Effects does
//! not draw a Fill whose bit is clear (986), and draws it once set (987). The importer keeps it as
//! the item's `enabled`, the exporter writes it back.

mod common;

use common::export_all;
use effectcraft_aep::{PropGroup as AepGroup, PropNode, parse};
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::{Node, Project, PropGroup};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures").join(name)).expect("fixture")
}

fn import_bytes(bytes: &[u8]) -> Project {
    let mut p = Project::default();
    let none = |_: &str| false;
    let r = import(&mut p, bytes, &ImportOptions { allow_losses: true, ..ImportOptions::new(&none) }).expect("import");
    assert!(r.report.losses.is_empty(), "{:?}", r.report.losses);
    p
}

/// (match id, enabled) of every shape item, depth first.
fn items(g: &PropGroup, out: &mut Vec<(String, bool)>) {
    for c in &g.children {
        if let Node::Group(sub) = c {
            if sub.match_id != "transform" && sub.match_id != "contents" {
                out.push((sub.match_id.clone(), sub.enabled));
            }
            items(sub, out);
        }
    }
}

fn switches(p: &Project) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (_, c) in p.comps() {
        for l in &c.layers {
            if let Some(contents) = l.props.sub("contents") {
                items(contents, &mut out);
            }
        }
    }
    out
}

/// The stored flags of every `ADBE Vector Graphic - Fill` group in a file.
fn fill_flags(bytes: &[u8]) -> Vec<u32> {
    fn walk(g: &AepGroup<'_, '_>, out: &mut Vec<u32>, depth: usize) {
        if depth > 16 {
            return;
        }
        for e in g.entries() {
            if let PropNode::Group(sub) = e.node {
                if e.match_name == "ADBE Vector Graphic - Fill" {
                    out.push(sub.flags().unwrap_or(99));
                }
                walk(&sub, out, depth + 1);
            }
        }
    }
    let f = parse(bytes).expect("parse");
    let view = effectcraft_aep::Project::new(&f).expect("project");
    let mut out = Vec::new();
    for item in view.all_items() {
        let Some(c) = item.comp() else { continue };
        for l in c.layers() {
            if let Some(root) = l.properties() {
                walk(&root, &mut out, 0);
            }
        }
    }
    out
}

#[test]
fn a_switched_off_fill_stays_off() {
    let bytes = fixture("986-shape-fill-switched-off-green.aep");
    assert_eq!(fill_flags(&bytes), [0]);
    let p = import_bytes(&bytes);
    let s = switches(&p);
    assert!(s.contains(&("fill".to_string(), false)), "{s:?}");
    assert!(s.iter().filter(|(id, _)| id != "fill").all(|(_, on)| *on), "{s:?}");
    let r = export_all(&p);
    assert!(r.report.losses.is_empty(), "{:?}", r.report.losses);
    assert_eq!(fill_flags(&r.bytes), [0]);
    assert_eq!(switches(&import_bytes(&r.bytes)), s);
    // Switched on again, it is written with the bit set (987).
    let mut q = p.clone();
    let ids: Vec<_> = q.comps().map(|(i, _)| *i).collect();
    for id in ids {
        if let Some(mut c) = q.comp(id).cloned() {
            for l in &mut c.layers {
                fn on(g: &mut PropGroup) {
                    g.enabled = true;
                    for c in &mut g.children {
                        if let Node::Group(s) = c {
                            on(s);
                        }
                    }
                }
                if let Some(contents) = l.props.sub_mut("contents") {
                    on(contents);
                }
            }
            if let Some(it) = q.item_mut(id) {
                it.kind = effectcraft_project::ItemKind::Comp(std::sync::Arc::new(c));
            }
        }
    }
    assert_eq!(fill_flags(&export_all(&q).bytes), [1]);
}
