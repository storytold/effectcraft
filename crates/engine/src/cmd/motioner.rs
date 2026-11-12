//! Motioner's original, sampled motion presets. One transaction, editable keys, no scripts.
use super::{CommandSpec, bad, f_p, has_layers, layer_mut, layers_p, str_p};
use crate::{EngineError, Result, Session, cmd, query};
use effectcraft_keyframe::{Keyframe, Value as Kv};
use effectcraft_time::Tick;
use serde_json::{Value, json};

pub const PRESETS: &[(&str, &str)] = &[
    ("fade", "Fade"),
    ("subida-suave", "Deslize suave"),
    ("slide-cima", "Slide"),
    ("lateral", "Deslize lateral"),
    ("quique-lateral", "Quique lateral"),
    ("quique-vertical", "Quique vertical"),
    ("escala-elastica", "Escala elástica"),
    ("overshoot", "Overshoot"),
    ("snap", "Snap"),
    ("zoom-profundo", "Zoom profundo"),
    ("chegada", "Chegada"),
    ("queda-giro", "Queda com giro"),
    ("giro-baixo", "Giro para baixo"),
    ("virar-x", "Virar X"),
    ("virar-y", "Virar Y"),
    ("cartao", "Cartão"),
    ("rolo", "Rolo"),
    ("balanco", "Balanço"),
    ("cortina-h", "Cortina horizontal"),
    ("cortina-v", "Cortina vertical"),
    ("desfoque", "Desfoque"),
];

fn progress(id: &str, t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    match id {
        "escala-elastica" | "balanco" => 1.0 - (-7.0 * t).exp() * (t * 18.0).cos(),
        "overshoot" | "cartao" | "chegada" => {
            let x = t - 1.0;
            1.0 + 2.6 * x * x * x + 1.6 * x * x
        }
        "quique-lateral" | "quique-vertical" => 1.0 - (1.0 - t).powi(2) * (t * 11.0).cos().abs(),
        _ => 1.0 - (1.0 - t).powi(3),
    }
}
fn tracks(id: &str, width: f64, height: f64) -> Vec<(&'static str, [f64; 3], bool)> {
    let mut out = vec![("transform/opacity", [0.0, 0.0, 0.0], true)];
    match id {
        "subida-suave" => out.push(("transform/position", [0.0, height * 0.06, 0.0], false)),
        "slide-cima" => out.push(("transform/position", [0.0, height * 0.35, 0.0], false)),
        "lateral" | "quique-lateral" => out.push(("transform/position", [-width * 0.4, 0.0, 0.0], false)),
        "quique-vertical" => out.push(("transform/position", [0.0, -height * 0.4, 0.0], false)),
        "escala-elastica" | "overshoot" => out.push(("transform/scale", [0.0, 0.0, 100.0], true)),
        "snap" => out.push(("transform/scale", [55.0, 55.0, 100.0], true)),
        "zoom-profundo" => out.push(("transform/scale", [350.0, 350.0, 100.0], true)),
        "chegada" => out.push(("transform/scale", [150.0, 150.0, 100.0], true)),
        "queda-giro" => {
            out.push(("transform/position", [0.0, -height * 0.5, 0.0], false));
            out.push(("transform/rotation", [-160.0, 0.0, 0.0], false));
        }
        "giro-baixo" => out.push(("transform/rotation", [-90.0, 0.0, 0.0], false)),
        "virar-x" => out.push(("transform/rotationX", [90.0, 0.0, 0.0], false)),
        "virar-y" | "cartao" => out.push(("transform/rotationY", [75.0, 0.0, 0.0], false)),
        "rolo" => {
            out.push(("transform/position", [-width * 0.5, 0.0, 0.0], false));
            out.push(("transform/rotation", [-360.0, 0.0, 0.0], false));
        }
        "balanco" => out.push(("transform/rotation", [-28.0, 0.0, 0.0], false)),
        "cortina-h" => out.push(("transform/scale", [0.0, 100.0, 100.0], true)),
        "cortina-v" => out.push(("transform/scale", [100.0, 0.0, 100.0], true)),
        _ => {}
    }
    out
}
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "motioner.animate";
    let id = str_p(p, "preset").unwrap_or("fade");
    if !PRESETS.iter().any(|(key, _)| *key == id) {
        return Err(bad(C, "unknown preset"));
    }
    let duration = f_p(p, "duration").unwrap_or(0.7);
    let stagger = f_p(p, "stagger").unwrap_or(0.0);
    let intensity = f_p(p, "intensity").unwrap_or(100.0) / 100.0;
    if !duration.is_finite()
        || !(0.05..=60.0).contains(&duration)
        || !stagger.is_finite()
        || !(0.0..=120.0).contains(&stagger)
        || !intensity.is_finite()
        || !(0.0..=3.0).contains(&intensity)
    {
        return Err(bad(C, "duration 0.05..60, stagger 0..120 frames, intensity 0..300"));
    }
    let mode = str_p(p, "mode").unwrap_or("entrada");
    if !["entrada", "saida", "ambos"].contains(&mode) {
        return Err(bad(C, "mode: entrada|saida|ambos"));
    }
    let (cid, ids) = layers_p(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    if ids.is_empty() || ids.len() > 1000 {
        return Err(bad(C, "select 1..1000 layers"));
    }
    if ids.iter().any(|id| comp.layer(*id).is_some_and(|l| l.switches.locked)) {
        return Err(bad(C, "unlock selected layers first"));
    }
    let (w, h, fps) = (f64::from(comp.width), f64::from(comp.height), comp.frame_rate.as_f64());
    let now = s.time();
    let count = ids.len();
    s.edit("Motioner · Animate", None, |project, _| {
        for (order, lid) in ids.iter().enumerate() {
            let mut next = project.next_id;
            let l = layer_mut(project, cid, *lid)?;
            let mut paths: Vec<(String, [f64; 3], bool)> = tracks(id, w, h).into_iter().map(|(path, v, a)| (path.into(), v, a)).collect();
            if ["desfoque", "zoom-profundo"].contains(&id) {
                let spec = effectcraft_effects::lookup("ec.blur.gaussian").ok_or_else(|| bad(C, "Gaussian Blur unavailable"))?;
                let group = effectcraft_effects::instantiate(spec, &mut effectcraft_project::build::Ids(&mut next), "Motioner Blur", [w, h]);
                let path = format!("effects/@{}/blurriness", group.uid);
                l.props.sub_mut("effects").ok_or_else(|| bad(C, "layer does not support blur"))?.children.push(group.into());
                paths.push((path, [60.0, 0.0, 0.0], false));
            }
            if ["virar-x", "virar-y", "cartao"].contains(&id) {
                l.switches.three_d = true;
            }
            let offset = Tick::from_seconds_f64(order as f64 * stagger / fps.max(1.0));
            let entry = now + offset;
            let exit = l.out_point - Tick::from_seconds_f64(duration) - offset;
            if mode == "ambos" && exit < entry + Tick::from_seconds_f64(duration) {
                return Err(bad(C, "layer too short for entrance and exit"));
            }
            for (path, delta, absolute) in paths {
                let Some(prop) = l.props.prop(&path) else { continue };
                let base_time = if mode == "saida" { exit } else { entry + Tick::from_seconds_f64(duration) };
                let base = prop.value_at(l.layer_time(base_time));
                let mut samples = vec![];
                let mut windows = vec![];
                for (out, at) in [(false, entry), (true, exit)] {
                    if (out && mode == "entrada") || (!out && mode == "saida") {
                        continue;
                    }
                    let start = l.layer_time(at);
                    let end = l.layer_time(at + Tick::from_seconds_f64(duration));
                    windows.push((start.min(end), start.max(end)));
                    for step in 0..=24 {
                        let t = f64::from(step) / 24.0;
                        let amount = if out { progress(id, t) } else { 1.0 - progress(id, t) } * intensity;
                        let amount = if path == "transform/opacity" { amount.clamp(0.0, 1.0) } else { amount };
                        let value = match &base {
                            Kv::Scalar(b) => Kv::Scalar(b + if absolute { (delta[0] - b) * amount } else { delta[0] * amount }),
                            Kv::Vec2(b) => {
                                Kv::Vec2(std::array::from_fn(|i| b[i] + if absolute { (b[i] * delta[i] / 100.0 - b[i]) * amount } else { delta[i] * amount }))
                            }
                            Kv::Vec3(b) => {
                                Kv::Vec3(std::array::from_fn(|i| b[i] + if absolute { (b[i] * delta[i] / 100.0 - b[i]) * amount } else { delta[i] * amount }))
                            }
                            _ => continue,
                        };
                        samples.push(Keyframe::new(l.layer_time(at + Tick::from_seconds_f64(t * duration)), value));
                    }
                }
                if let Some(prop) = l.props.prop_mut(&path) {
                    prop.keys.retain(|k| !windows.iter().any(|(lo, hi)| k.time >= *lo && k.time <= *hi));
                    prop.keys.extend(samples);
                    prop.keys.sort_by_key(|k| k.time);
                    prop.keys.dedup_by_key(|k| k.time);
                }
            }
            project.next_id = next;
        }
        Ok(())
    })?;
    Ok(json!({"preset":id,"layers":count}))
}
pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "motioner.animate",
            "Motioner Animate",
            [],
            None,
            "{preset, duration?:seconds, stagger?:frames, intensity?:percent, mode?:entrada|saida|ambos, layers?}",
            has_layers,
            apply
        ),
        query!("motioner.presets", "Motioner Presets", "{}", |_, _| Ok(json!(
            PRESETS.iter().map(|(id, name)| json!({"id":id,"name":name})).collect::<Vec<_>>()
        ))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> Session {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name":"Motioner test","width":120,"height":80,"duration":5.0})).unwrap();
        s.execute("layer.newSolid", json!({"name":"Test"})).unwrap();
        s
    }
    #[test]
    fn all_motioner_presets_are_editable_and_undoable() {
        for (id, _) in PRESETS {
            let mut s = session();
            let before = s.project.clone();
            s.execute("motioner.animate", json!({"preset":id,"duration":0.5})).unwrap();
            let l = &s.active_comp().unwrap().layers[0];
            assert_eq!(l.props.prop("transform/opacity").unwrap().keys.len(), 25, "{id}");
            assert_eq!(l.props.prop("transform/opacity").unwrap().keys.first().unwrap().value.as_f64(), 0.0);
            assert_eq!(l.props.prop("transform/opacity").unwrap().keys.last().unwrap().value.as_f64(), 100.0);
            if *id == "virar-x" {
                assert!(l.switches.three_d);
                assert_eq!(l.props.prop("transform/rotationX").unwrap().keys.len(), 25);
            }
            if *id == "desfoque" {
                assert_eq!(l.effects().unwrap().groups().next().unwrap().get("blurriness").unwrap().keys.len(), 25);
            }
            s.execute("edit.undo", json!({})).unwrap();
            assert_eq!(s.project, before, "{id}");
        }
    }
    #[test]
    fn invalid_motion_parameters_leave_project_untouched() {
        let mut s = session();
        let before = s.project.clone();
        assert!(s.execute("motioner.animate", json!({"duration":-2})).is_err());
        assert_eq!(s.project, before);
    }
}
