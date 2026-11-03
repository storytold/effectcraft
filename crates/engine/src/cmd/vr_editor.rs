//! Window ▸ VR Comp Editor: our own panel over the VR environments Composition ▸ VR ▸ Create VR
//! Environment builds (an equirectangular output comp ← a 3:2 cube map ← six face comps, each
//! with a 90° camera). It lists the environments and edits the 360 view's camera orientation:
//! the six face cameras turn together, so the whole equirectangular output rotates (pan / tilt /
//! roll the viewer's starting direction) without touching the scene.
//!
//! The view orientation is not stored separately: it is read back from the Front camera
//! (`camera = view × face base`), so editing a camera by hand shows up here too.

use effectcraft_geom::{Mat4, vec3};
use effectcraft_project::{GroupKind, ItemId, LayerId, LayerSource, Project};
use serde_json::{Value, json};

use super::app_more::{CELLS_3X2, FACES, ae, grouped, orientation_for_axes};
use super::{CommandSpec, always, bad};
use crate::{EngineError, Result, Session, cmd, query};

/// A VR environment: its output comp, cube map and the face cameras in [`FACES`] order.
pub struct VrEnv {
    pub output: ItemId,
    pub cube: ItemId,
    pub faces: [(ItemId, LayerId); 6],
}

/// The VR environments of a project.
pub fn environments(p: &Project) -> Vec<VrEnv> {
    let mut out = vec![];
    for (oid, oc) in p.comps() {
        for l in &oc.layers {
            let LayerSource::Comp { item: cube } = l.source else { continue };
            let vr = l.effects().is_some_and(|fx| fx.groups().any(|g| matches!(&g.kind, GroupKind::Effect { effect } if effect == "ec.vr.converter")));
            if !vr {
                continue;
            }
            let Some(cc) = p.comp(cube) else { continue };
            let size = (cc.width / 3).max(1) as f64;
            let mut faces: [Option<(ItemId, LayerId)>; 6] = [None; 6];
            for fl in &cc.layers {
                let LayerSource::Comp { item: face } = fl.source else { continue };
                let pos = fl.props.prop("transform/position").map(|x| x.value.components()).unwrap_or_default();
                let (Some(x), Some(y)) = (pos.first(), pos.get(1)) else { continue };
                let cell = ((x / size).floor().max(0.0) as u32, (y / size).floor().max(0.0) as u32);
                let Some(i) = CELLS_3X2.iter().position(|c| *c == cell) else { continue };
                let cam = p.comp(face).and_then(|fc| fc.layers.iter().find(|x| x.is_camera())).map(|x| x.id);
                if let Some(cam) = cam {
                    faces[i] = Some((face, cam));
                }
            }
            if faces.iter().all(Option::is_some) {
                out.push(VrEnv { output: *oid, cube, faces: faces.map(|f| f.unwrap_or((ItemId(0), LayerId(0)))) });
            }
        }
    }
    out
}

fn base(i: usize) -> [f64; 3] {
    let (_, fw, r, u) = FACES[i];
    orientation_for_axes(ae(r), ae([-u[0], -u[1], -u[2]]), ae(fw))
}

fn orient(o: [f64; 3]) -> Mat4 {
    Mat4::orientation(vec3(o[0], o[1], o[2]))
}

fn transpose(m: Mat4) -> Mat4 {
    let mut t = Mat4::IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            t.0[r][c] = m.0[c][r];
        }
    }
    t
}

fn norm(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    let r = if (r - 360.0).abs() < 1e-7 { 0.0 } else { r };
    (r * 1e6).round() / 1e6
}

/// Orientation angles (X, Y, Z degrees, applied X then Y then Z) of a rotation matrix.
pub fn euler(m: Mat4) -> [f64; 3] {
    let m = m.0;
    let y = (-m[2][0]).clamp(-1.0, 1.0).asin();
    let (x, z) = if y.cos().abs() > 1e-6 { (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0])) } else { ((-m[1][2]).atan2(m[1][1]), 0.0) };
    [norm(x.to_degrees()), norm(y.to_degrees()), norm(z.to_degrees())]
}

/// The current view orientation of an environment (from its Front camera).
fn view_of(p: &Project, env: &VrEnv) -> [f64; 3] {
    let (fc, cam) = env.faces[4];
    let o = p.comp(fc).and_then(|c| c.layer(cam)).and_then(|l| l.props.prop("transform/orientation")).map(|x| x.value.components()).unwrap_or_default();
    let o = [o.first().copied().unwrap_or(0.0), o.get(1).copied().unwrap_or(0.0), o.get(2).copied().unwrap_or(0.0)];
    euler(orient(o) * transpose(orient(base(4))))
}

fn env_json(p: &Project, e: &VrEnv) -> Value {
    let name = |i: ItemId| p.item(i).map(|x| x.name.clone()).unwrap_or_default();
    json!({
        "output": e.output.0, "name": name(e.output), "cubeMap": e.cube.0,
        "faces": e.faces.iter().enumerate().map(|(i, (c, cam))| json!({"face": FACES[i].0, "comp": c.0, "camera": cam.0})).collect::<Vec<_>>(),
        "view": view_of(p, e),
    })
}

fn list(s: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(environments(&s.project).iter().map(|e| env_json(&s.project, e)).collect::<Vec<_>>()))
}

/// The environment `comp` (its output, cube map or a face comp), else the active comp's, else
/// the only one.
fn env_p(s: &Session, p: &Value, cmd: &str) -> Result<VrEnv> {
    let envs = environments(&s.project);
    let want = p.get("comp").and_then(Value::as_u64).map(ItemId).or(s.state.active_comp);
    let hit = |e: &VrEnv, c: ItemId| e.output == c || e.cube == c || e.faces.iter().any(|f| f.0 == c);
    let i = want.and_then(|c| envs.iter().position(|e| hit(e, c))).or((envs.len() == 1).then_some(0));
    let i = i.ok_or_else(|| {
        bad(cmd, if envs.is_empty() { "no VR environment (Composition ▸ VR ▸ Create VR Environment…)" } else { "give `comp`: a VR environment's output comp" })
    })?;
    envs.into_iter().nth(i).ok_or(EngineError::NoComp)
}

fn set_view(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "comp.vr.setView";
    let env = env_p(s, p, C)?;
    let mut o = view_of(&s.project, &env);
    match p.get("orientation") {
        Some(Value::Array(a)) => {
            for (k, v) in a.iter().take(3).enumerate() {
                o[k] = v.as_f64().ok_or_else(|| bad(C, "orientation: [x, y, z] degrees"))?;
            }
        }
        Some(_) => return Err(bad(C, "orientation: [x, y, z] degrees")),
        None => {}
    }
    // Pan (Y), tilt (X) and roll (Z) by name.
    for (k, i) in [("tilt", 0), ("pan", 1), ("roll", 2)] {
        if let Some(v) = p.get(k).and_then(Value::as_f64) {
            o[i] = v;
        }
    }
    let v = orient(o);
    grouped(s, "VR View Orientation", |s| {
        for (i, (fc, cam)) in env.faces.iter().enumerate() {
            let e = euler(v * orient(base(i)));
            s.execute("prop.set", json!({"comp": fc.0, "layer": cam.0, "path": "transform/orientation", "value": e}))?;
        }
        Ok(())
    })?;
    Ok(json!({"output": env.output.0, "view": view_of(&s.project, &env)}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        query!("comp.vr.environments", "VR Environments", "{} → [{output, name, cubeMap, faces: [{face, comp, camera}], view: [x, y, z]}]", list),
        cmd!(
            "comp.vr.setView",
            "VR View Orientation",
            [],
            None,
            "{comp? (an environment's output / cube map / face comp), orientation?: [x, y, z] degrees, pan?, tilt?, roll?} → turns the six face cameras together",
            always,
            set_view
        ),
    ]
}
