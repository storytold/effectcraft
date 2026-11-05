use super::{Comp, EngineError, ItemId, KV, Layer, LayerId, PropGroup, Result, Session, Tick, Value, b_p, bad, layers_p, resolve_layer, unlocked};

/// Re-express a layer in its new parent space without moving its anchor.
pub(crate) struct ParentFix {
    pub layer: LayerId,
    /// Maps Position from the old parent's space into the new one's.
    m: effectcraft_geom::Mat3,
    /// Added to Rotation.
    rotation: f64,
    /// Multiply Scale.
    scale: [f64; 2],
    /// 3D AV layers retain their world-space anchor, including Z. Cameras and lights also
    /// re-express their Point of Interest.
    position_3d: Option<effectcraft_geom::Mat4>,
    /// A uniform relative parent transform re-expresses static Orientation and Scale.
    pose_3d: Option<Pose3d>,
}

/// A uniform relative parent transform, as it applies to a 3D layer's Orientation and Scale.
#[derive(Clone, Copy)]
struct Pose3d {
    /// Rotation that Orientation is re-expressed under: `Orientation' = correction * Orientation`.
    correction: effectcraft_geom::Mat4,
    /// Uniform scale factor, multiplied into Scale (negative for a mirrored parent).
    factor: f64,
}

impl ParentFix {
    /// Re-express the layer's Position, Rotation and Scale (every key) in the new parent's space.
    pub fn apply(&self, l: &mut Layer) {
        let Some(tr) = l.props.sub_mut("transform") else { return };
        if let Some(m) = self.position_3d {
            // Spatial Position speed measures distance along its path. Its parent-space
            // unit changes even when the child's Orientation cannot be re-expressed.
            let position_speed = uniform_rotation(m).map(|(_, factor)| factor.abs());
            let map_position = |v: &KV| {
                let q = m.apply(effectcraft_geom::Vec3::from(v.as_vec3()));
                KV::Vec3([q.x, q.y, q.z])
            };
            // Read all separated axes before writing any of them.
            let names = ["positionX", "positionY", "positionZ"];
            if tr.get("positionX").is_some() {
                let axes = names.map(|name| tr.get(name).cloned());
                let current = axes.each_ref().map(|pr| pr.as_ref().map(|pr| pr.value.as_f64()).unwrap_or(0.0));
                let q = m.apply(effectcraft_geom::Vec3::from(current));
                for (i, name) in names.into_iter().enumerate() {
                    if let Some(pr) = tr.get_mut(name) {
                        pr.value = KV::Scalar([q.x, q.y, q.z].get(i).copied().unwrap_or(0.0));
                        for key in &mut pr.keys {
                            let p = axes.each_ref().map(|axis| axis.as_ref().map(|axis| axis.value_at(key.time).as_f64()).unwrap_or(0.0));
                            let q = m.apply(effectcraft_geom::Vec3::from(p));
                            key.value = KV::Scalar([q.x, q.y, q.z].get(i).copied().unwrap_or(0.0));
                        }
                    }
                }
            }
            // Position and a camera's or light's Point of Interest are both points in the parent's space.
            let map_point = |pr: &mut effectcraft_project::Property| {
                pr.value = map_position(&pr.value);
                for key in &mut pr.keys {
                    key.value = map_position(&key.value);
                    // Spatial tangents are vectors, so the parent's translation is excluded.
                    for tangent in [&mut key.spatial_in, &mut key.spatial_out] {
                        let v = m.apply_vec(effectcraft_geom::Vec3::from(*tangent));
                        *tangent = [v.x, v.y, v.z];
                    }
                }
                if let Some(factor) = position_speed
                    .filter(|_| pr.spatial)
                    .filter(|factor| pr.keys.iter().flat_map(|key| key.in_ease.iter().chain(&key.out_ease)).all(|ease| (ease.speed * *factor).is_finite()))
                {
                    for key in &mut pr.keys {
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed *= factor;
                        }
                    }
                }
            };
            if let Some(pr) = tr.get_mut("position") {
                map_point(pr);
            }
            if let Some(pr) = tr.get_mut("poi") {
                map_point(pr);
            }
            if let Some(pose) = self.pose_3d {
                if let Some(pr) = tr.get_mut("orientation") {
                    reexpress_orientation(pose.correction, pr);
                }
                if let Some(pr) = tr.get_mut("scale") {
                    let factor = pose.factor;
                    let scaled = |value: &KV| KV::Vec3(value.as_vec3().map(|v| v * factor));
                    pr.value = scaled(&pr.value);
                    for key in &mut pr.keys {
                        key.value = scaled(&key.value);
                        // Scale's temporal speeds use the same units as its values.
                        for ease in key.in_ease.iter_mut().chain(&mut key.out_ease) {
                            ease.speed *= factor;
                        }
                    }
                }
            }
            return;
        }
        let map = |pr: &mut effectcraft_project::Property, f: &dyn Fn(&KV) -> KV| {
            pr.value = f(&pr.value);
            for key in &mut pr.keys {
                key.value = f(&key.value);
            }
        };
        let m = self.m;
        if tr.get("positionX").is_some() {
            // Separated dimensions: X and Y move together, key by key at the same times.
            // Everything is read before anything is written.
            let [x, y] = [tr.get("positionX"), tr.get("positionY")].map(|p| p.map(|p| p.value.as_f64()).unwrap_or(0.0));
            let q = m.apply(effectcraft_geom::vec2(x, y));
            let keys_x: Vec<(Tick, f64)> = tr.get("positionX").map(|p| p.keys.iter().map(|k| (k.time, k.value.as_f64())).collect()).unwrap_or_default();
            let keys_y: Vec<(Tick, f64)> = tr.get("positionY").map(|p| p.keys.iter().map(|k| (k.time, k.value.as_f64())).collect()).unwrap_or_default();
            let at = |keys: &[(Tick, f64)], name: &str, t: Tick, tr: &PropGroup| {
                keys.iter().find(|k| k.0 == t).map(|k| k.1).unwrap_or_else(|| tr.get(name).map(|p| p.value_at(t).as_f64()).unwrap_or(0.0))
            };
            let (xs, ys): (Vec<f64>, Vec<f64>) = (
                keys_x.iter().map(|&(t, x)| m.apply(effectcraft_geom::vec2(x, at(&keys_y, "positionY", t, tr))).x).collect(),
                keys_y.iter().map(|&(t, y)| m.apply(effectcraft_geom::vec2(at(&keys_x, "positionX", t, tr), y)).y).collect(),
            );
            for (name, value, vals) in [("positionX", q.x, xs), ("positionY", q.y, ys)] {
                if let Some(pr) = tr.get_mut(name) {
                    pr.value = KV::Scalar(value);
                    for (k, v) in pr.keys.iter_mut().zip(vals) {
                        k.value = KV::Scalar(v);
                    }
                }
            }
        } else if let Some(pr) = tr.get_mut("position") {
            map(pr, &|v| {
                let c = v.as_vec3();
                let q = m.apply(effectcraft_geom::vec2(c[0], c[1]));
                KV::Vec3([q.x, q.y, c[2]])
            });
        }
        let (dr, k) = (self.rotation, self.scale);
        if let Some(pr) = tr.get_mut("rotation") {
            map(pr, &|v| KV::Scalar(v.as_f64() + dr));
        }
        if let Some(pr) = tr.get_mut("scale") {
            map(pr, &|v| {
                let c = v.as_vec3();
                KV::Vec3([c[0] * k[0], c[1] * k[1], c[2]])
            });
        }
    }
}

/// The rotation and signed scale factor of a uniform parent transform's linear part (negative for a
/// mirrored parent). `None` for a non-uniform or degenerate transform.
fn uniform_rotation(m: effectcraft_geom::Mat4) -> Option<(effectcraft_geom::Mat4, f64)> {
    use effectcraft_geom::{Mat4, vec3};
    let a = m.0;
    let columns = [vec3(a[0][0], a[1][0], a[2][0]), vec3(a[0][1], a[1][1], a[2][1]), vec3(a[0][2], a[1][2], a[2][2])];
    let magnitude = columns[0].x.hypot(columns[0].y).hypot(columns[0].z);
    if !magnitude.is_finite() || magnitude <= 0.0 {
        return None;
    }
    let c = columns.map(|c| c / magnitude);
    if c.iter().any(|c| (c.dot(*c) - 1.0).abs() > 1e-9) || c[0].dot(c[1]).abs() > 1e-9 || c[0].dot(c[2]).abs() > 1e-9 || c[1].dot(c[2]).abs() > 1e-9 {
        return None;
    }
    let cross = vec3(c[1].y * c[2].z - c[1].z * c[2].y, c[1].z * c[2].x - c[1].x * c[2].z, c[1].x * c[2].y - c[1].y * c[2].x);
    let factor = if c[0].dot(cross) < 0.0 { -magnitude } else { magnitude };
    let mut rotation = Mat4::IDENTITY;
    for row in 0..3 {
        for col in 0..3 {
            rotation.0[row][col] = a[row][col] / factor;
        }
    }
    Some((rotation, factor))
}

/// The turn (degrees) of a rotation about Z alone, or `None` for any other rotation.
fn z_only_turn(correction: effectcraft_geom::Mat4) -> Option<f64> {
    let m = correction.0;
    let about_z = [m[0][2], m[1][2], m[2][0], m[2][1]].iter().all(|v| v.abs() < 1e-9) && (m[2][2] - 1.0).abs() < 1e-9;
    about_z.then(|| m[1][0].atan2(m[0][0]).to_degrees())
}

/// ZYX Euler angles in degrees of a row-major rotation (`Rz * Ry * Rx`), safe at gimbal lock.
fn euler_zyx(r: [[f64; 4]; 4]) -> [f64; 3] {
    // atan2 rather than asin: near ±90 degrees asin loses precision, while cos(y) stays exact.
    let cy = r[0][0].hypot(r[1][0]);
    let y = (-r[2][0]).atan2(cy);
    let (x, z) = if cy > 1e-9 { (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0])) } else { ((-r[1][2]).atan2(r[1][1]), 0.0) };
    [x.to_degrees(), y.to_degrees(), z.to_degrees()]
}

/// The equivalent ZYX Euler triple nearest `reference`: each axis moved by whole turns, or the
/// `(x + 180, 180 - y, z + 180)` form when that is nearer.
fn nearest_euler(euler: [f64; 3], reference: [f64; 3]) -> [f64; 3] {
    let near = |v: [f64; 3]| -> [f64; 3] { std::array::from_fn(|i| v[i] + ((reference[i] - v[i]) / 360.0).round() * 360.0) };
    let natural = near(euler);
    let flipped = near([euler[0] + 180.0, 180.0 - euler[1], euler[2] + 180.0]);
    let distance = |v: [f64; 3]| v.iter().zip(reference.iter()).map(|(a, b)| (a - b).powi(2)).sum::<f64>();
    if distance(flipped) < distance(natural) { flipped } else { natural }
}

/// The Euler Orientation of `correction * Orientation(o)`, in the form nearest `reference`.
fn reexpress_euler(correction: effectcraft_geom::Mat4, o: [f64; 3], reference: [f64; 3]) -> [f64; 3] {
    let world = correction * effectcraft_geom::Mat4::orientation(effectcraft_geom::Vec3::from(o));
    nearest_euler(euler_zyx(world.0), reference)
}

/// Largest angle (degrees) a baked Orientation may drift from the exact rotation between its keys.
const BAKE_TOLERANCE_DEGREES: f64 = 0.05;
/// How many times a segment may be halved while baking, and the most keys a property may end up with.
const BAKE_MAX_DEPTH: u32 = 10;
const BAKE_MAX_KEYS: usize = 4096;

/// The angle (degrees) between two rotations.
fn rotation_angle(a: effectcraft_geom::Mat4, b: effectcraft_geom::Mat4) -> f64 {
    let trace: f64 = (0..3).flat_map(|i| (0..3).map(move |j| (i, j))).map(|(i, j)| a.0[i][j] * b.0[i][j]).sum();
    ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Adds keys between `(ta, ea)` and `(tb, eb)` (already converted) wherever the straight Euler blend
/// between them drifts from `correction * Orientation(authored)` by more than the tolerance. `out`
/// never grows past `budget` keys: what the whole property may still add, not what one segment may.
fn bake_between(
    correction: effectcraft_geom::Mat4,
    authored: &effectcraft_project::Property,
    (ta, ea): (Tick, [f64; 3]),
    (tb, eb): (Tick, [f64; 3]),
    depth: u32,
    budget: usize,
    out: &mut Vec<effectcraft_keyframe::Keyframe>,
) {
    use effectcraft_geom::{Mat4, Vec3};
    if depth == 0 || tb.0 - ta.0 < 2 || out.len() >= budget {
        return;
    }
    let tm = Tick(ta.0 + (tb.0 - ta.0) / 2);
    let blend = [(ea[0] + eb[0]) / 2.0, (ea[1] + eb[1]) / 2.0, (ea[2] + eb[2]) / 2.0];
    let exact = correction * Mat4::orientation(Vec3::from(authored.value_at(tm).as_vec3()));
    // Symmetric easing crosses the straight blend at its midpoint. Check both sides too before
    // accepting a segment whose original easing will be replaced with a linear blend.
    let quarter = (tb.0 - ta.0) / 4;
    if [Tick(ta.0 + quarter), tm, Tick(tb.0 - quarter)].into_iter().all(|t| {
        let u = (t.0 - ta.0) as f64 / (tb.0 - ta.0) as f64;
        let linear = std::array::from_fn(|i| ea[i] + (eb[i] - ea[i]) * u);
        let target = correction * Mat4::orientation(Vec3::from(authored.value_at(t).as_vec3()));
        rotation_angle(Mat4::orientation(Vec3::from(linear)), target) <= BAKE_TOLERANCE_DEGREES
    }) {
        return;
    }
    let em = nearest_euler(euler_zyx(exact.0), blend);
    bake_between(correction, authored, (ta, ea), (tm, em), depth - 1, budget, out);
    if out.len() >= budget {
        return;
    }
    out.push(effectcraft_keyframe::Keyframe::new(tm, KV::Vec3(em)));
    bake_between(correction, authored, (tm, em), (tb, eb), depth - 1, budget, out);
}

/// Re-expresses Orientation as `correction * Orientation`. A Z-only correction shifts Z exactly in the
/// value and every key. For any other correction a static value is converted directly; keys are
/// converted, and keys are added between them wherever the Euler blend between two keys would drift
/// from the exact rotation (the segments become linear, with the original easing baked in).
fn reexpress_orientation(correction: effectcraft_geom::Mat4, pr: &mut effectcraft_project::Property) {
    if let Some(turn) = z_only_turn(correction) {
        // No turn (an identity correction) leaves every authored value as it is.
        if turn.abs() <= 1e-12 {
            return;
        }
        let shift = |value: &KV| {
            let [x, y, z] = value.as_vec3();
            KV::Vec3([x, y, z + turn])
        };
        pr.value = shift(&pr.value);
        for key in &mut pr.keys {
            key.value = shift(&key.value);
        }
        return;
    }
    let value = pr.value.as_vec3();
    pr.value = KV::Vec3(reexpress_euler(correction, value, value));
    if pr.keys.is_empty() {
        return;
    }
    let authored = pr.clone();
    // Each key keeps the authored change from the previous key on top of its converted value.
    let mut converted: Vec<[f64; 3]> = vec![];
    let mut prev: Option<([f64; 3], [f64; 3])> = None;
    for key in &authored.keys {
        let o = key.value.as_vec3();
        let reference = match prev {
            Some((before, after)) => [after[0] + o[0] - before[0], after[1] + o[1] - before[1], after[2] + o[2] - before[2]],
            None => o,
        };
        let n = reexpress_euler(correction, o, reference);
        converted.push(n);
        prev = Some((o, n));
    }
    use effectcraft_keyframe::Interp;
    let mut keys: Vec<effectcraft_keyframe::Keyframe> = vec![];
    let mut after_baked_segment = false;
    for (i, key) in authored.keys.iter().enumerate() {
        let mut k = key.clone();
        k.value = KV::Vec3(converted[i]);
        if after_baked_segment {
            k.in_interp = Interp::Linear;
            k.in_ease.clear();
            k.auto_bezier = false;
            k.continuous = false;
        }
        after_baked_segment = false;
        let segment = authored.keys.get(i + 1).filter(|_| key.out_interp != Interp::Hold);
        let mut between = vec![];
        if let Some(next) = segment {
            // What the whole property may still add: the keys it will have already (the ones written
            // and the authored ones still to come) count against the cap.
            let budget = BAKE_MAX_KEYS.saturating_sub(keys.len() + authored.keys.len() - i);
            bake_between(correction, &authored, (key.time, converted[i]), (next.time, converted[i + 1]), BAKE_MAX_DEPTH, budget, &mut between);
            // With the cap reached the segment can't be checked or subdivided: it keeps its authored easing between
            // the converted keys, rather than becoming a straight blend that was never checked against the original.
            if between.len() >= budget {
                between.clear();
            // An eased segment, or one that gained keys, becomes a run of straight blends.
            } else if !between.is_empty() || key.out_interp != Interp::Linear || next.in_interp != Interp::Linear {
                k.out_interp = Interp::Linear;
                k.out_ease.clear();
                k.auto_bezier = false;
                k.continuous = false;
                after_baked_segment = true;
            }
        }
        keys.push(k);
        keys.extend(between);
    }
    pr.keys = keys;
}

/// Correction for a two-node rig's Orientation: after a uniform parent change its aim is
/// `rotation * aim`, so the roll is re-expressed against that aim. `None` for a degenerate aim.
fn aim_correction(rotation: effectcraft_geom::Mat4, aim: [f64; 3]) -> Option<effectcraft_geom::Mat4> {
    use effectcraft_geom::vec3;
    use effectcraft_render::three_d::camera::basis_rotation;
    let [x, y, z] = aim;
    let length = (x * x + y * y + z * z).sqrt();
    if !length.is_finite() || length <= 1e-9 {
        return None;
    }
    let dir = vec3(x / length, y / length, z / length);
    let turned = rotation.apply_vec(dir);
    // A view along the up axis has no roll to keep, and its basis is undefined.
    if 1.0 - dir.y * dir.y < 1e-12 || 1.0 - turned.y * turned.y < 1e-12 {
        return None;
    }
    let up = vec3(0.0, 1.0, 0.0);
    Some(basis_rotation(turned, up).transpose() * rotation * basis_rotation(dir, up))
}

/// A parent change retains 2D transforms and the world-space anchor of 3D layers (Position,
/// and Point of Interest for two-node cameras and lights). Position is
/// re-expressed in the new parent's space (`None`: the composition), evaluated at the current
/// time.
pub(crate) fn parent_fixes(s: &Session, cid: ItemId, ids: &[LayerId], parent: Option<LayerId>) -> Vec<ParentFix> {
    let Some(comp) = s.project.comp(cid) else { return vec![] };
    let ectx = effectcraft_render::EvalCtx {
        project: &s.project,
        comp_id: cid,
        comp,
        time: s.time(),
        expr: s.expr.as_deref(),
        footage: Some(s.footage.as_ref()),
        property_overrides: &[],
        media_overrides: &[],
    };
    let space = |id: Option<LayerId>| -> effectcraft_geom::Mat3 {
        id.and_then(|i| comp.layer(i)).filter(|l| !l.is_3d()).map(|l| ectx.layer_to_comp(l).0).unwrap_or(effectcraft_geom::Mat3::IDENTITY)
    };
    let decompose = |m: &effectcraft_geom::Mat3| {
        let a = m.0;
        let sx = (a[0][0] * a[0][0] + a[1][0] * a[1][0]).sqrt();
        let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
        (a[1][0].atan2(a[0][0]).to_degrees(), sx, if sx > 1e-12 { det / sx } else { 0.0 })
    };
    let new_m = space(parent);
    let Some(inv) = new_m.inverse() else { return vec![] };
    let (rn, sxn, syn) = decompose(&new_m);
    let mut fixes: Vec<_> = comp
        .layers
        .iter()
        .filter(|l| ids.contains(&l.id) && !l.is_3d() && l.parent != parent)
        .map(|l| {
            let old_m = space(l.parent);
            let (ro, sxo, syo) = decompose(&old_m);
            let scale = [if sxn.abs() > 1e-12 { sxo / sxn } else { 1.0 }, if syn.abs() > 1e-12 { syo / syn } else { 1.0 }];
            ParentFix { layer: l.id, m: inv * old_m, rotation: ro - rn, scale, position_3d: None, pose_3d: None }
        })
        .collect();
    let space_3d = |id: Option<LayerId>| {
        id.and_then(|id| comp.layer(id)).map_or(effectcraft_geom::Mat4::IDENTITY, |layer| {
            // Parent transforms do not inherit the parent's source pixel-aspect stretch.
            let world = ectx.world_matrix(layer);
            let par = ectx.par_ratio(layer);
            let a = layer.transform().map(|tr| ectx.v3(layer, tr, "anchor", [0.0; 3])).unwrap_or([0.0; 3]);
            let anchor = effectcraft_geom::vec3(a[0], a[1], if layer.is_3d() { a[2] } else { 0.0 });
            world
                * effectcraft_geom::Mat4::translate(anchor)
                * effectcraft_geom::Mat4::scale(effectcraft_geom::vec3(1.0 / par, 1.0, 1.0))
                * effectcraft_geom::Mat4::translate(-anchor)
        })
    };
    if let Some(inv) = space_3d(parent).inverse().filter(|m| m.0.iter().flatten().all(|v| v.is_finite())) {
        for layer in comp.layers.iter().filter(|l| ids.contains(&l.id) && l.is_3d() && l.parent != parent) {
            let m = inv * space_3d(layer.parent);
            // A one-node camera or light has no Scale to carry a mirror, and its frame (forward and
            // down) would come out reversed: compensate none of it, so Position and Orientation stay
            // as authored instead of half-following the parent.
            let rig = layer.is_camera() || layer.is_light();
            if rig && !effectcraft_render::three_d::camera::is_two_node(layer) && uniform_rotation(m).is_some_and(|(_, factor)| factor <= 0.0) {
                continue;
            }
            if m.0.iter().flatten().all(|v| v.is_finite()) {
                let pose_3d = layer.transform().and_then(|tr| {
                    let orientation = tr.get("orientation")?;
                    // Cameras and lights have no Scale; their Orientation is re-expressed alone.
                    let scale = tr.get("scale");
                    // Expression-driven values stay authored. Auto-orient replaces Orientation, except
                    // a two-node rig, whose roll is re-expressed against its new aim.
                    let two_node = (layer.is_camera() || layer.is_light()) && effectcraft_render::three_d::camera::is_two_node(layer);
                    if orientation.expr.is_some()
                        || scale.is_some_and(|scale| scale.expr.is_some())
                        || (layer.auto_orient != effectcraft_project::AutoOrient::Off && !two_node)
                    {
                        return None;
                    }
                    let (rotation, factor) = uniform_rotation(m)?;
                    let correction = if two_node {
                        // A mirrored parent flips the aim, which this correction does not model.
                        if factor <= 0.0 {
                            return None;
                        }
                        let (w, h) = (comp.width as f64, comp.height as f64);
                        let pos = ectx.v3(layer, tr, "position", [w / 2.0, h / 2.0, -1000.0]);
                        let poi = ectx.v3(layer, tr, "poi", [w / 2.0, h / 2.0, 0.0]);
                        aim_correction(rotation, [poi[0] - pos[0], poi[1] - pos[1], poi[2] - pos[2]])?
                    } else {
                        rotation
                    };
                    let orientation_finite = std::iter::once(&orientation.value)
                        .chain(orientation.keys.iter().map(|key| &key.value))
                        .all(|value| value.as_vec3().iter().all(|v| v.is_finite()));
                    if !correction.0.iter().flatten().all(|v| v.is_finite()) || !orientation_finite {
                        return None;
                    }
                    if let Some(scale) = scale {
                        let values_finite = std::iter::once(&scale.value)
                            .chain(scale.keys.iter().map(|key| &key.value))
                            .all(|value| value.as_vec3().iter().all(|v| (v * factor).is_finite()));
                        let speeds_finite = scale.keys.iter().all(|key| key.in_ease.iter().chain(&key.out_ease).all(|ease| (ease.speed * factor).is_finite()));
                        if !values_finite || !speeds_finite {
                            return None;
                        }
                    }
                    Some(Pose3d { correction, factor })
                });
                fixes.push(ParentFix { layer: layer.id, m: effectcraft_geom::Mat3::IDENTITY, rotation: 0.0, scale: [1.0; 2], position_3d: Some(m), pose_3d });
            }
        }
    }
    fixes
}

/// Validate the proposed ancestor chain before any authored edit, including when compensation
/// is disabled. Bound both unique ancestors and linear lookup/selection work on loaded graphs.
fn check_parent_chain(comp: &Comp, ids: &[LayerId], parent: Option<LayerId>, max_visits: usize, max_work: usize) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    let mut work = 0usize;
    let mut cur = parent;
    while let Some(id) = cur {
        if seen.len() >= max_visits {
            return Err(bad("layer.setParent", "parent chain exceeds the safety visit limit"));
        }
        if !seen.insert(id) {
            return Err(bad("layer.setParent", "parent chain contains an existing cycle"));
        }
        for selected in ids {
            if work >= max_work {
                return Err(bad("layer.setParent", "parent chain exceeds the safety work limit"));
            }
            work += 1;
            if *selected == id {
                return Err(bad("layer.setParent", "parenting would create a cycle"));
            }
        }
        let mut ancestor = None;
        for layer in &comp.layers {
            if work >= max_work {
                return Err(bad("layer.setParent", "parent chain exceeds the safety work limit"));
            }
            work += 1;
            if layer.id == id {
                ancestor = Some(layer);
                break;
            }
        }
        let layer = ancestor.ok_or_else(|| bad("layer.setParent", "parent chain refers to a missing layer"))?;
        cur = layer.parent;
    }
    Ok(())
}

pub(super) fn set_parent(s: &mut Session, p: &Value) -> Result<Value> {
    let (cid, ids) = layers_p(s, p)?;
    // Locked layers keep their parent (several at once: the others are parented).
    let ids = unlocked(s, cid, ids, "layer.setParent")?;
    let comp = s.project.comp(cid).ok_or(EngineError::NoComp)?;
    let parent = match p.get("parent") {
        None | Some(Value::Null) => None,
        Some(v) => Some(resolve_layer(comp, v).ok_or_else(|| bad("layer.setParent", "no such parent layer"))?),
    };
    // A loaded cycle need not include a selected child. Validate before compensation or edit;
    // do not impose the renderer's 64-ancestor truncation as a new ordinary hierarchy limit.
    check_parent_chain(comp, &ids, parent, 4096, 1_048_576)?;
    // Like AE's pick-whip, parenting keeps the layer where it is: its transform is re-expressed
    // in the new parent's space (Position, Rotation, Scale), unless `compensate: false`.
    let compensate = b_p(p, "compensate").unwrap_or(true);
    let fixes = if compensate { parent_fixes(s, cid, &ids, parent) } else { vec![] };
    s.edit("Parent", None, |proj, _| {
        let comp = proj.comp_mut(cid).ok_or(EngineError::NoComp)?;
        for l in comp.layers.iter_mut_where(|l| ids.contains(&l.id)) {
            l.parent = parent;
            if let Some(f) = fixes.iter().find(|f| f.layer == l.id) {
                f.apply(l);
            }
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

#[cfg(test)]
mod parent_chain_tests {
    use super::super::json;
    use super::*;
    use std::sync::Arc;

    fn fixture() -> (Session, u64, u64, u64) {
        let mut s = Session::default();
        s.execute("comp.new", json!({"name":"Parent safety","width":16,"height":16,"duration":1})).unwrap();
        let a = s.execute("layer.newSolid", json!({"name":"A"})).unwrap()["layer"].as_u64().unwrap();
        let b = s.execute("layer.newSolid", json!({"name":"B"})).unwrap()["layer"].as_u64().unwrap();
        let c = s.execute("layer.newSolid", json!({"name":"C"})).unwrap()["layer"].as_u64().unwrap();
        for id in [a, b, c] {
            s.execute("prop.set", json!({"layer":id,"path":"transform/anchor","value":[0,0,0]})).unwrap();
        }
        s.execute("prop.set", json!({"layer":a,"path":"transform/position","value":[100,200,0]})).unwrap();
        s.execute("prop.set", json!({"layer":c,"path":"transform/position","value":[10,20,0]})).unwrap();
        s.history.undo.clear();
        (s, a, b, c)
    }
    fn malformed_parent(s: &mut Session, child: u64, parent: u64) {
        let cid = s.active_comp_id().unwrap();
        Arc::make_mut(&mut s.project).comp_mut(cid).unwrap().layer_mut(LayerId(child)).unwrap().parent = Some(LayerId(parent));
    }
    fn assert_atomic_error(s: &mut Session, child: u64, parent: u64, compensate: bool, message: &str) {
        let before = s.project.clone();
        let revision = s.revision;
        let undo = s.history.undo.len();
        let err = s.execute("layer.setParent", json!({"layers":[child],"parent":parent,"compensate":compensate})).unwrap_err();
        assert!(err.to_string().contains(message), "{err}");
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(s.revision, revision);
        assert_eq!(s.history.undo.len(), undo);
    }
    #[test]
    fn loaded_cycle_excluding_selected_child_is_rejected_atomically() {
        for compensate in [true, false] {
            let (mut s, a, b, c) = fixture();
            // Simulate a malformed loaded project. Never execute this on the unguarded baseline.
            malformed_parent(&mut s, a, b);
            malformed_parent(&mut s, b, a);
            assert_atomic_error(&mut s, c, a, compensate, "existing cycle");
        }
    }
    #[test]
    fn dangling_loaded_parent_is_rejected_atomically() {
        for compensate in [true, false] {
            let (mut s, a, _, c) = fixture();
            malformed_parent(&mut s, a, u64::MAX);
            assert_atomic_error(&mut s, c, a, compensate, "missing layer");
        }
    }
    #[test]
    fn selected_descendant_and_direct_self_are_rejected() {
        let (mut s, a, _, c) = fixture();
        assert_atomic_error(&mut s, c, c, false, "create a cycle");
        s.execute("layer.setParent", json!({"layers":[a],"parent":c,"compensate":false})).unwrap();
        assert_atomic_error(&mut s, c, a, true, "create a cycle");
    }
    #[test]
    fn ordinary_parent_and_unparent_keep_explicit_false_and_undo_semantics() {
        for compensate in [true, false] {
            let (mut s, a, _, c) = fixture();
            let old = s.project.clone();
            let world = |s: &Session| {
                let cid = s.active_comp_id().unwrap();
                let comp = s.active_comp().unwrap();
                let ctx = effectcraft_render::EvalCtx::new(&s.project, cid, comp, s.time());
                ctx.world_matrix(comp.layer(LayerId(c)).unwrap()).apply(effectcraft_geom::Vec3::ZERO)
            };
            let before = world(&s);
            s.execute("layer.setParent", json!({"layers":[c],"parent":a,"compensate":compensate})).unwrap();
            let expected = if compensate { before } else { before + effectcraft_geom::vec3(100.0, 200.0, 0.0) };
            assert!((world(&s) - expected).length() < 1e-9);
            assert_eq!(s.active_comp().unwrap().layer(LayerId(c)).unwrap().parent, Some(LayerId(a)));
            assert_eq!(s.history.undo.len(), 1);
            let parented = s.project.clone();
            assert!(s.undo());
            assert!(Arc::ptr_eq(&s.project, &old));
            assert!(s.redo());
            assert!(Arc::ptr_eq(&s.project, &parented));
            s.execute("layer.setParent", json!({"layers":[c],"parent":null,"compensate":compensate})).unwrap();
            assert_eq!(s.active_comp().unwrap().layer(LayerId(c)).unwrap().parent, None);
            assert!(s.undo());
            assert!(Arc::ptr_eq(&s.project, &parented));
        }
    }
    #[test]
    fn ancestor_and_lookup_work_budgets_fail_closed() {
        let (mut s, a, b, c) = fixture();
        malformed_parent(&mut s, a, b);
        let comp = s.active_comp().unwrap();
        // New solids are inserted above the selection: C,B,A. Walking A->B scans
        // three then two layers, and compares the one selected child twice: seven work units.
        assert_eq!(comp.layers.iter().map(|l| l.id).collect::<Vec<_>>(), vec![LayerId(c), LayerId(b), LayerId(a)]);
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 7).is_ok());
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 6).unwrap_err().to_string().contains("work limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 256).is_ok());
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 1, 256).unwrap_err().to_string().contains("visit limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], Some(LayerId(a)), 2, 1).unwrap_err().to_string().contains("work limit"));
        assert!(check_parent_chain(comp, &[LayerId(c)], None, 0, 0).is_ok(), "unparenting does not traverse ancestors");
    }

    #[test]
    fn well_formed_chain_beyond_renderer_depth_is_not_unnecessarily_rejected() {
        let (s, _, _, c) = fixture();
        let mut comp = s.active_comp().unwrap().clone();
        let template = comp.layers.first().unwrap().clone();
        for n in 0..80 {
            let mut layer = template.clone();
            layer.id = LayerId(10_000 + n);
            layer.parent = (n < 79).then_some(LayerId(10_001 + n));
            comp.layers.push(layer);
        }
        assert!(check_parent_chain(&comp, &[LayerId(c)], Some(LayerId(10_000)), 4096, 1_048_576).is_ok());
    }
}
