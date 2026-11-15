//! #1898: CC Particle Systems II and CC Particle World replace their layer since v0.10.0, which
//! is what After Effects does. A project written before v0.10.0 composited the particles over the
//! layer instead, and opening it must not hide its artwork or an earlier effect in the stack.

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{ItemId, ItemKind, LayerId, Node, Project, PropGroup};
use effectcraft_time::Tick;
use serde_json::json;

use crate::Session;
use crate::render::RenderOpts;

/// The two effects v0.10.0 made replace the layer.
const PARTICLES: [&str; 2] = ["ec.sim.ccparticlesystems2", "ec.sim.ccparticleworld"];

/// The control that gives an older project the old compositing back.
const COMPOSITE: &str = "compositeWithOriginal";

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ec-1898-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn effect_group(s: &Session, lid: u64, uid: u64) -> PropGroup {
    s.active_comp().unwrap().layer(LayerId(lid)).unwrap().effects().unwrap().groups().find(|g| g.uid == uid).unwrap().clone()
}

/// A 64×48 comp: a red solid with a Fill (green) below both particle effects, with no particles
/// born, so only the artwork, the effect earlier in the stack and the compositing show.
fn particle_comp() -> (Session, ItemId, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 64, "height": 48, "frameRate": 30, "duration": 2})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let lid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 64, "height": 48})).unwrap()["layer"].as_u64().unwrap();
    s.execute("effect.apply", json!({"layers": [lid], "effect": "ec.generate.fill"})).unwrap();
    s.execute("prop.set", json!({"layer": lid, "path": "effects/#1/color", "value": [0.0, 1.0, 0.0, 1.0]})).unwrap();
    for effect in PARTICLES {
        let uid = s.execute("effect.apply", json!({"layers": [lid], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap();
        let rate = effect_group(&s, lid, uid).prop("birthRate").unwrap().uid;
        s.execute("prop.set", json!({"layer": lid, "prop": rate, "value": 0.0})).unwrap();
    }
    (s, cid, lid)
}

/// `s` as the text of a project file written by `version`. `control` keeps the `Composite With
/// Original` control: a project from before v0.10.0 has none.
fn file_text(s: &Session, version: Option<&str>, control: bool) -> String {
    let mut p = (*s.project).clone();
    if !control {
        for it in p.items.values_mut() {
            let ItemKind::Comp(c) = &mut it.kind else { continue };
            for l in &mut std::sync::Arc::make_mut(c).layers {
                let Some(fx) = l.props.sub_mut("effects") else { continue };
                for n in &mut fx.children {
                    let Node::Group(g) = n else { continue };
                    g.children.retain(|c| c.match_id() != COMPOSITE);
                }
            }
        }
    }
    let mut file: serde_json::Value = serde_json::from_str(&p.to_file_json().unwrap()).unwrap();
    match version {
        Some(v) => file["savedBy"] = json!(v),
        None => {
            file.as_object_mut().unwrap().remove("savedBy");
        }
    }
    file.to_string()
}

/// The control's value on every effect of the layer that has one, in the open project.
fn controls(s: &Session, cid: ItemId, lid: u64) -> Vec<KV> {
    let fx = s.project.comp(cid).unwrap().layer(LayerId(lid)).unwrap().effects().unwrap();
    fx.groups().filter_map(|g| g.get(COMPOSITE).map(|p| p.value.clone())).collect()
}

#[test]
fn a_project_from_before_v0_10_0_keeps_the_artwork_under_its_particles() {
    let (s, cid, lid) = particle_comp();
    let path = tmp("legacy.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, file_text(&s, Some("0.6.0"), false)).unwrap();
    let mut opened = Session::default();
    opened.execute("file.open", json!({"path": path})).unwrap();
    let img = opened.render(cid, Tick::from_seconds_f64(0.5), RenderOpts::default());
    assert_eq!(img.get(32, 24), [0.0, 1.0, 0.0, 1.0], "the artwork and the Fill below the particles show through");
    assert_eq!(img.get(1, 1), [0.0, 1.0, 0.0, 1.0], "all of it, not only under the producer");
    assert_eq!(controls(&opened, cid, lid), vec![KV::Bool(true); PARTICLES.len()], "both effects keep the old compositing");
    // File ▸ Import ▸ EffectCraft Project runs the same upgrade before it remaps the ids.
    let mut imported = Session::default();
    imported.execute("file.importProject", json!({"path": path})).unwrap();
    let (icid, ilid) = imported
        .project
        .items
        .values()
        .find(|i| i.name == "Main")
        .and_then(|i| match &i.kind {
            ItemKind::Comp(c) => Some((i.id, c.layers.iter().next()?.id)),
            _ => None,
        })
        .expect("the imported comp");
    assert_eq!(controls(&imported, icid, ilid.0).len(), PARTICLES.len(), "both effects survive the import");
    assert!(controls(&imported, icid, ilid.0).iter().all(|v| *v == KV::Bool(true)), "{:?}", controls(&imported, icid, ilid.0));
    let imported_img = imported.render(icid, Tick::from_seconds_f64(0.5), RenderOpts::default());
    assert_eq!(imported_img.get(1, 1), [0.0, 1.0, 0.0, 1.0], "imported artwork shows too");
}

#[test]
fn a_current_project_still_replaces_the_layer_with_its_particles() {
    let (s, cid, lid) = particle_comp();
    // A brand-new instance replaces the layer, as after v0.10.0 and as After Effects does: this is
    // what keeps the After Effects frames the effects are matched against green.
    assert_eq!(controls(&s, cid, lid), vec![KV::Bool(false); PARTICLES.len()], "a new instance composites nothing");
    for (name, version) in [("current", Some(effectcraft_project::APP_VERSION)), ("anonymous", None)] {
        let path = tmp(&format!("{name}.ecproj")).to_string_lossy().to_string();
        std::fs::write(&path, file_text(&s, version, true)).unwrap();
        let mut opened = Session::default();
        opened.execute("file.open", json!({"path": path})).unwrap();
        let img = opened.render(cid, Tick::from_seconds_f64(0.5), RenderOpts::default());
        assert_eq!(img.get(32, 24)[3], 0.0, "{name}: no particles born, nothing left of the layer");
        assert_eq!(controls(&opened, cid, lid), vec![KV::Bool(false); PARTICLES.len()], "{name}: the control stays off");
    }
}

/// Settings ▸ Project ▸ New Project Loads Template opens the template as an untitled project: a
/// template written before v0.10.0 has to be upgraded like any other project file.
#[test]
fn a_new_project_template_from_before_v0_10_0_is_migrated() {
    let (s, _, _) = particle_comp();
    let path = tmp("template.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, file_text(&s, Some("0.6.0"), false)).unwrap();
    let mut fresh = Session::default();
    fresh.prefs.project.use_template = true;
    fresh.prefs.project.template_path = path;
    fresh.execute("file.newProject", json!({})).unwrap();
    let cid = fresh.project.items.values().find(|i| i.name == "Main").map(|i| i.id).expect("the template's comp");
    let lid = fresh.project.comp(cid).unwrap().layers.iter().next().expect("its layer").id;
    assert_eq!(controls(&fresh, cid, lid.0), vec![KV::Bool(true); PARTICLES.len()], "the template's particles keep the old compositing");
    let img = fresh.render(cid, Tick::from_seconds_f64(0.5), RenderOpts::default());
    assert_eq!(img.get(1, 1), [0.0, 1.0, 0.0, 1.0], "and its artwork shows");
}

#[test]
fn only_a_writer_before_v0_10_0_gets_the_old_compositing() {
    let (s, cid, lid) = particle_comp();
    for (version, legacy) in [
        (Some("0.6.0"), true),
        (Some("0.9.2"), true),
        (Some("0.9.2+build"), true),
        (Some("0.10.0"), false),
        (Some("0.24.0"), false),
        (Some("1.0.0"), false),
        (None, false),
        (Some("unknown"), false),
        (Some("0.9"), false),
        (Some("0.9.2.1"), false),
    ] {
        let mut p = Project::from_json(&file_text(&s, version, false)).unwrap();
        crate::upgrade_effects(&mut p, version);
        let fx = p.comp(cid).unwrap().layer(LayerId(lid)).unwrap().effects().unwrap();
        let got: Vec<_> = fx.groups().filter_map(|g| g.get(COMPOSITE).map(|p| p.value.clone())).collect();
        assert_eq!(got, vec![KV::Bool(legacy); PARTICLES.len()], "{version:?}");
    }
    // A control the file already holds is the artist's own choice, whatever wrote it.
    let mut explicit = Project::from_json(&file_text(&s, Some("0.6.0"), true)).unwrap();
    crate::upgrade_effects(&mut explicit, Some("0.6.0"));
    let fx = explicit.comp(cid).unwrap().layer(LayerId(lid)).unwrap().effects().unwrap();
    assert!(fx.groups().all(|g| g.get(COMPOSITE).is_none_or(|p| p.value == KV::Bool(false))), "an explicit choice wins");
}
