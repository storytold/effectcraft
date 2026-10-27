//! Keyframe features of M14.5: Animation ▸ Keyframe Assistant ▸ Convert Audio to Keyframes and
//! RPF Camera Import, and Edit ▸ Label on selected keyframes.

use effectcraft_color::Label;
use effectcraft_keyframe::{Keyframe, Value as KV};
use effectcraft_project::LayerId;
use effectcraft_project::build::Ids;
use effectcraft_time::Tick;
use serde_json::{Value, json};

use super::app_more::grouped;
use super::{CommandSpec, bad, comp_id, has_comp, layer_mut, str_p};
use crate::{EngineError, KeyRef, Result, Session, cmd};

// ---------------------------------------------------------------- keyframe labels

/// The label index (0 = None, 1–16) of a label name (built-in or renamed in Settings ▸ Labels).
fn label_index(s: &Session, name: &str) -> Option<u8> {
    let l = if name.eq_ignore_ascii_case("none") { Label::None } else { s.prefs.label_from_name(name)? };
    Label::ALL.iter().position(|x| *x == l).map(|i| i as u8)
}

fn keys_p(s: &Session, p: &Value) -> Vec<KeyRef> {
    match p.get("keys") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect(),
        _ => s.state.selected_keys.clone(),
    }
}

/// Edit ▸ Label on keyframes: colour the selected (or given) keyframes (label names as renamed
/// in Settings ▸ Labels; `keys.setLabel` is the Timeline context menu's command).
pub(crate) fn label_keys(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "label").unwrap_or("Red");
    let idx = label_index(s, name).ok_or_else(|| bad("keys.label", format!("unknown label `{name}`")))?;
    let keys = keys_p(s, p);
    if keys.is_empty() {
        return Err(bad("keys.label", "select keyframes first"));
    }
    let cid = comp_id(s, p)?;
    let n = s.edit("Keyframe Label", None, |proj, _| {
        let mut n = 0;
        for k in &keys {
            let Ok(l) = layer_mut(proj, cid, k.layer) else { continue };
            let Some(pr) = l.props.find_mut(k.prop) else { continue };
            for key in pr.keys.iter_mut().filter(|x| x.time == k.time) {
                key.label = idx;
                n += 1;
            }
        }
        Ok(n)
    })?;
    Ok(json!({"keys": n, "label": idx}))
}

// ---------------------------------------------------------------- Convert Audio to Keyframes

/// Per-frame amplitude of interleaved stereo: (left, right, both), mean absolute sample × 100.
pub fn amplitudes(buf: &[f32]) -> (f64, f64, f64) {
    let n = (buf.len() / 2).max(1) as f64;
    let (mut l, mut r) = (0.0f64, 0.0f64);
    for c in buf.as_chunks::<2>().0 {
        l += c[0].abs() as f64;
        r += c[1].abs() as f64;
    }
    let (l, r) = (l / n * 100.0, r / n * 100.0);
    (l, r, (l + r) / 2.0)
}

/// Animation ▸ Keyframe Assistant ▸ Convert Audio to Keyframes: an "Audio Amplitude" null with
/// Left Channel, Right Channel and Both Channels sliders keyed on every frame of the work area
/// with the amplitude of the comp's audible layers. One undo step.
fn audio_to_keys(s: &mut Session, p: &Value) -> Result<Value> {
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let fr = comp.frame_rate;
    let (a, b) = comp.work_area;
    let (f0, f1) = (fr.frame_at(fr.snap_nearest(a)), fr.frame_at(fr.snap_nearest(b)).max(fr.frame_at(fr.snap_nearest(a)) + 1));
    const RATE: u32 = 48_000;
    let per = ((RATE as f64 / fr.as_f64()).round() as usize).max(1);
    let mut rows = vec![];
    for f in f0..f1 {
        let t = fr.tick_of(f);
        let buf = effectcraft_render::audio::mix_comp(&s.project, s.footage.as_ref(), s.expr.as_deref(), cid, t, per, RATE);
        rows.push((t, amplitudes(&buf)));
    }
    let spec = effectcraft_effects::lookup("ec.control.slider").ok_or_else(|| EngineError::Other("Slider Control is missing".into()))?;
    let peak = rows.iter().map(|(_, (_, _, both))| *both).fold(0.0, f64::max);
    let r = grouped(s, "Convert Audio to Keyframes", |s| {
        let nl = s.execute("layer.newNull", json!({"comp": cid.0, "name": "Audio Amplitude"}))?["layer"].as_u64().unwrap_or(0);
        let lid = LayerId(nl);
        s.edit("Convert Audio to Keyframes", None, |proj, st| {
            let mut next = proj.next_id;
            let l = layer_mut(proj, cid, lid)?;
            let start = l.start_time;
            let fx = l.props.sub_mut("effects").ok_or_else(|| bad("keys.audioToKeyframes", "the null has no effects group"))?;
            for (i, name) in ["Left Channel", "Right Channel", "Both Channels"].into_iter().enumerate() {
                let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), name, [comp.width as f64, comp.height as f64]);
                if let Some(pr) = g.get_mut("slider") {
                    pr.keys = rows
                        .iter()
                        .map(|(t, amp)| {
                            let v = [amp.0, amp.1, amp.2][i];
                            Keyframe::new(*t - start, KV::Scalar((v * 100.0).round() / 100.0))
                        })
                        .collect();
                }
                fx.children.push(g.into());
            }
            proj.next_id = next;
            st.selected_layers = vec![lid];
            Ok(())
        })?;
        Ok(json!({"layer": nl, "frames": rows.len(), "peak": peak}))
    })?;
    Ok(r)
}

// ---------------------------------------------------------------- RPF Camera Import

/// One camera sample: time, position, orientation (degrees, X/Y/Z) and zoom (px).
#[derive(Clone, Debug, PartialEq)]
pub struct CamSample {
    pub time: Tick,
    pub position: [f64; 3],
    pub orientation: [f64; 3],
    pub zoom: Option<f64>,
}

fn zoom_of(fov: Option<f64>, zoom: Option<f64>, comp_w: f64) -> Option<f64> {
    zoom.or_else(|| fov.map(|a| comp_w * 0.5 / (a.clamp(0.1, 179.0).to_radians() * 0.5).tan()))
}

/// Parse camera data: JSON `{frameRate?, frames: [{frame | time, position, orientation |
/// rotation, zoom? | fov?}]}` or CSV with a header naming `frame`/`time`, `px py pz`
/// (or `x y z`), `rx ry rz` and `zoom`/`fov` columns.
pub fn parse_camera(text: &str, rate: effectcraft_time::FrameRate, comp_w: f64) -> std::result::Result<Vec<CamSample>, String> {
    let t = text.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        let v: Value = serde_json::from_str(t).map_err(|e| format!("camera JSON: {e}"))?;
        let rate = v.get("frameRate").and_then(Value::as_f64).map(effectcraft_time::FrameRate::from_f64).unwrap_or(rate);
        let frames = v.get("frames").or(Some(&v)).and_then(Value::as_array).ok_or("camera JSON: expected `frames`")?;
        let v3 = |x: Option<&Value>| -> Option<[f64; 3]> {
            let a = x?.as_array()?;
            Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2).and_then(Value::as_f64).unwrap_or(0.0)])
        };
        return frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let time = match (f.get("time").and_then(Value::as_f64), f.get("frame").and_then(Value::as_i64)) {
                    (Some(s), _) => Tick::from_seconds_f64(s),
                    (None, Some(n)) => rate.tick_of(n),
                    _ => rate.tick_of(i as i64),
                };
                Ok(CamSample {
                    time,
                    position: v3(f.get("position")).ok_or(format!("camera JSON: frame {i} needs `position` [x,y,z]"))?,
                    orientation: v3(f.get("orientation").or(f.get("rotation"))).unwrap_or([0.0; 3]),
                    zoom: zoom_of(f.get("fov").and_then(Value::as_f64), f.get("zoom").and_then(Value::as_f64), comp_w),
                })
            })
            .collect();
    }
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    let head: Vec<String> = lines.next().ok_or("camera CSV: empty")?.split([',', ';', '\t']).map(|h| h.trim().to_ascii_lowercase()).collect();
    let col = |names: &[&str]| head.iter().position(|h| names.contains(&h.as_str()));
    let (cf, ct) = (col(&["frame", "f"]), col(&["time", "seconds", "t"]));
    let pos = [col(&["px", "x", "posx", "position x"]), col(&["py", "y", "posy", "position y"]), col(&["pz", "z", "posz", "position z"])];
    let rot = [col(&["rx", "rotx", "orientation x", "tilt"]), col(&["ry", "roty", "orientation y", "pan"]), col(&["rz", "rotz", "orientation z", "roll"])];
    let (cz, cfov) = (col(&["zoom"]), col(&["fov", "angle", "angleofview"]));
    if pos.iter().any(Option::is_none) {
        return Err("camera CSV: needs px, py, pz (or x, y, z) columns".into());
    }
    lines
        .enumerate()
        .map(|(i, l)| {
            let cells: Vec<f64> = l.split([',', ';', '\t']).map(|c| c.trim().parse().unwrap_or(f64::NAN)).collect();
            let get = |c: Option<usize>| c.and_then(|c| cells.get(c).copied()).filter(|v| v.is_finite());
            let time = match (get(ct), get(cf)) {
                (Some(s), _) => Tick::from_seconds_f64(s),
                (None, Some(n)) => rate.tick_of(n as i64),
                _ => rate.tick_of(i as i64),
            };
            Ok(CamSample {
                time,
                position: pos.map(|c| get(c).unwrap_or(0.0)),
                orientation: rot.map(|c| get(c).unwrap_or(0.0)),
                zoom: zoom_of(get(cfov), get(cz), comp_w),
            })
        })
        .collect()
}

/// Animation ▸ Keyframe Assistant ▸ RPF Camera Import: a camera layer keyed from camera data.
/// RPF/RLA files carry their camera in an undocumented block, so EffectCraft reads the same data
/// from JSON or CSV exports (see `docs/preferences.md` ▸ RPF Camera Import). One undo step.
fn rpf_camera_import(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("keys.rpfCameraImport", "missing `path` (.json or .csv camera data)"))?;
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".rpf") || lower.ends_with(".rla") {
        return Err(bad(
            "keys.rpfCameraImport",
            "the camera block of RPF/RLA files is not publicly documented: export the camera from your 3D application as JSON or CSV (frame, px, py, pz, rx, ry, rz, zoom|fov)",
        ));
    }
    let cid = comp_id(s, p)?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?.clone();
    let bytes = s.services.read_file(path).map_err(|e| EngineError::Other(format!("cannot read {path}: {e}")))?;
    let samples = parse_camera(&String::from_utf8_lossy(&bytes), comp.frame_rate, comp.width as f64).map_err(|e| bad("keys.rpfCameraImport", e))?;
    if samples.is_empty() {
        return Err(bad("keys.rpfCameraImport", "no camera samples"));
    }
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "RPF Camera".into());
    grouped(s, "RPF Camera Import", |s| {
        let cam = s.execute("layer.newCamera", json!({"comp": cid.0, "name": name, "type": "oneNode"}))?["layer"].as_u64().unwrap_or(0);
        let lid = LayerId(cam);
        s.edit("RPF Camera Import", None, |proj, _| {
            let l = layer_mut(proj, cid, lid)?;
            let start = l.start_time;
            let keyed = |vals: Vec<(Tick, KV)>| -> Vec<Keyframe> { vals.into_iter().map(|(t, v)| Keyframe::new(t - start, v)).collect() };
            if let Some(pr) = l.props.prop_mut("transform/position") {
                pr.keys = keyed(samples.iter().map(|c| (c.time, KV::Vec3(c.position))).collect());
            }
            if let Some(pr) = l.props.prop_mut("transform/orientation") {
                pr.keys = keyed(samples.iter().map(|c| (c.time, KV::Vec3(c.orientation))).collect());
            }
            if samples.iter().any(|c| c.zoom.is_some())
                && let Some(pr) = l.props.prop_mut("cameraOptions/zoom")
            {
                pr.keys = keyed(samples.iter().filter_map(|c| Some((c.time, KV::Scalar(c.zoom?)))).collect());
            }
            Ok(())
        })?;
        Ok(json!({"layer": cam, "keyframes": samples.len()}))
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!("keys.audioToKeyframes", "Convert Audio to Keyframes", ["Animation", "Keyframe Assistant"], None, "{comp?}", has_comp, audio_to_keys),
        cmd!(
            "keys.rpfCameraImport",
            "RPF Camera Import",
            ["Animation", "Keyframe Assistant"],
            None,
            "{path: .json|.csv camera data, comp?}",
            has_comp,
            rpf_camera_import
        ),
    ]
}
