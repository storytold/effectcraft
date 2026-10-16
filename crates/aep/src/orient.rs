//! After Effects Orientation order.
//!
//! After Effects composes a layer's Orientation Z first (Rx·Ry·Rz), in Classic 3D as in Advanced
//! 3D; the EffectCraft layer transform uses Rz·Ry·Rx. The tilted solids of SceneYard "Space
//! Vision"'s floor only meet as steps the Z-first way (Advanced 3D), and in Classic 3D SceneYard
//! Chess Strategy's king (Orientation 180°, 0°, 287° under a parent turned Y 180°, Z 107°) only
//! stands upright that way. Import rewrites each Orientation into the angles that give the same
//! rotation in EffectCraft's order, so native projects keep their meaning. Models carry their
//! own AE axes flag ([`super::models3d`]); Advanced 3D cameras and lights are left as they are,
//! Classic 3D ones are rewritten too (the lights of SceneYard Drone Collage and the camera of
//! Calendar Illustration then light and frame as in AE).

use effectcraft_keyframe::Value;
use effectcraft_project::{Comp, LayerSource};

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn rx(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn ry(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rz(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// The angle closest to `near` among `a + k·360°`.
fn nearest(a: f64, near: f64) -> f64 {
    a + ((near - a) / 360.0).round() * 360.0
}

/// Angles (degrees) whose Rz·Ry·Rx equals Rx(o.x)·Ry(o.y)·Rz(o.z), each taken nearest `near`.
pub(crate) fn z_first_to_x_first(o: [f64; 3], near: [f64; 3]) -> Option<[f64; 3]> {
    if !o.iter().all(|v| v.is_finite()) {
        return None;
    }
    let m = mul(&mul(&rx(o[0]), &ry(o[1])), &rz(o[2]));
    // Rz(z)·Ry(y)·Rx(x): m[2][0] = −sin y, m[2][1] = cos y·sin x, m[2][2] = cos y·cos x,
    // m[1][0] = sin z·cos y, m[0][0] = cos z·cos y.
    let y = (-m[2][0]).clamp(-1.0, 1.0).asin();
    let (x, z) = if y.cos() > 1e-9 {
        (m[2][1].atan2(m[2][2]), m[1][0].atan2(m[0][0]))
    } else {
        // Gimbal lock: only x ∓ z is defined; keep z at 0.
        ((-m[1][2]).atan2(m[1][1]), 0.0)
    };
    let a = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
    Some(std::array::from_fn(|i| nearest(a[i], near[i])))
}

/// Rewrite the Orientation of every 3D layer of a comp (static value and each keyframe;
/// keyframes stay continuous) into EffectCraft's order.
pub(crate) fn ae_orientation(comp: &mut Comp, losses: &mut Vec<String>, context: &str) {
    let advanced = comp.renderer == effectcraft_project::Renderer::Advanced3D;
    for layer in &mut comp.layers {
        let camera_or_light = matches!(layer.source, LayerSource::Camera | LayerSource::Light { .. });
        if matches!(layer.source, LayerSource::Model { .. }) || (advanced && camera_or_light) || !(layer.switches.three_d || camera_or_light) {
            continue;
        }
        let name = layer.name.clone();
        let Some(prop) = layer.props.prop_mut("transform/orientation") else { continue };
        if prop.expr.is_some() {
            losses.push(format!("{context} layer {name:?}: Orientation expression kept in the Classic 3D axis order"));
        }
        if let Value::Vec3(o) = prop.value
            && let Some(v) = z_first_to_x_first(o, o)
        {
            prop.value = Value::Vec3(v);
        }
        let mut near = None;
        for key in &mut prop.keys {
            if let Value::Vec3(o) = key.value
                && let Some(v) = z_first_to_x_first(o, near.unwrap_or(o))
            {
                key.value = Value::Vec3(v);
                near = Some(v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &M3, b: &M3) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-9))
    }

    #[test]
    fn converted_angles_give_the_same_rotation() {
        for o in [
            [281.7119113818079, 1.6416268536408438, 302.6967172325551],
            [9.2, 302.7, 357.0],
            [30.0, 0.0, 0.0],
            [10.0, 90.0, 25.0],
            [0.0; 3],
            [123.0, -270.0, 45.0],
        ] {
            let v = z_first_to_x_first(o, o).unwrap();
            let want = mul(&mul(&rx(o[0]), &ry(o[1])), &rz(o[2]));
            let got = mul(&mul(&rz(v[2]), &ry(v[1])), &rx(v[0]));
            assert!(close(&got, &want), "{o:?} -> {v:?}");
        }
        // Single-axis orientations are unchanged.
        let v = z_first_to_x_first([30.0, 0.0, 0.0], [30.0, 0.0, 0.0]).unwrap();
        assert!((v[0] - 30.0).abs() < 1e-9 && v[1].abs() < 1e-9 && v[2].abs() < 1e-9, "{v:?}");
        assert!(z_first_to_x_first([f64::NAN, 0.0, 0.0], [0.0; 3]).is_none());
    }
}
