//! Auxiliary 3D channels: the comp's depth / ID / Cryptomatte pass and the 3D Channel effects on
//! a precomp of a 3D comp.

use effectcraft_color::Label;
use effectcraft_keyframe::Value;
use effectcraft_project::build::{self, Ids};
use effectcraft_project::{Comp, ItemId, ItemKind, Layer, LayerSource, Project, Solid};
use effectcraft_raster::channels3d::{BACKGROUND_DEPTH, crypto_hash};
use effectcraft_time::{FrameRate, Tick};

use crate::{NoFootage, RenderOpts, Renderer, render_frame};

const W: u32 = 200;
const H: u32 = 100;

fn zoom() -> f64 {
    W as f64 * 50.0 / 36.0
}

fn solid3(p: &mut Project, comp: &Comp, name: &str, color: [f32; 3], pos: [f64; 3]) -> Layer {
    let sid = p.add_item(name, Label::Red, None, ItemKind::Solid(Solid { color, width: 60, height: 60, pixel_aspect: 1.0 }));
    let mut l = build::layer(p, comp, name, LayerSource::Solid { item: sid }, (60, 60), None);
    l.name = name.to_string();
    l.switches.three_d = true;
    l.props.prop_mut("transform/position").unwrap().value = Value::Vec3(pos);
    l
}

/// Inner 3D comp: "Near" (red, z 0, left) above "Far" (green, z 500, right). Outer comp: the
/// precomp layer, optionally with an effect.
fn scene(effect: Option<(&str, Vec<(&str, Value)>)>) -> (Project, ItemId, ItemId) {
    let mut p = Project::default();
    p.settings.bit_depth = effectcraft_project::BitDepth::Bpc32;
    let inner = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let iid = p.add_item("Inner", Label::Sandstone, None, ItemKind::Comp(inner.clone().into()));
    let near = solid3(&mut p, &inner, "Near", [1.0, 0.0, 0.0], [60.0, 50.0, 0.0]);
    let far = solid3(&mut p, &inner, "Far", [0.0, 1.0, 0.0], [140.0, 50.0, 500.0]);
    p.comp_mut(iid).unwrap().layers.push(near);
    p.comp_mut(iid).unwrap().layers.push(far);
    let outer = Comp::new(W, H, FrameRate::FPS_30, Tick::from_seconds_f64(2.0));
    let oid = p.add_item("Outer", Label::Sandstone, None, ItemKind::Comp(outer.clone().into()));
    let mut pre = build::layer(&mut p, &outer, "Inner", LayerSource::Comp { item: iid }, (W, H), None);
    if let Some((id, vals)) = effect {
        let spec = effectcraft_effects::find(id).unwrap();
        let mut next = p.next_id;
        let mut g = effectcraft_effects::instantiate(spec, &mut Ids(&mut next), spec.name, [W as f64, H as f64]);
        p.next_id = next;
        for (k, v) in vals {
            g.prop_mut(k).unwrap().value = v.clone();
        }
        pre.props.sub_mut("effects").unwrap().children.push(g.into());
    }
    p.comp_mut(oid).unwrap().layers.push(pre);
    (p, iid, oid)
}

#[test]
fn comp_aux_pass_has_depth_ids_and_cryptomatte() {
    let (p, iid, _) = scene(None);
    let r = Renderer::new(&p, &NoFootage, RenderOpts::default());
    let a = r.comp_aux(iid, Tick::ZERO).unwrap();
    assert_eq!((a.width, a.height), (W, H));
    let at = |name: &str, x: usize, y: usize| a.get(name).unwrap()[y * W as usize + x];
    // Near plane at the camera's zoom distance, the far one 500 px behind it.
    assert!((at("Z", 60, 50) as f64 - zoom()).abs() < 1e-2, "{}", at("Z", 60, 50));
    assert!((at("Z", 114, 50) as f64 - (zoom() + 500.0)).abs() < 1e-2, "{}", at("Z", 114, 50));
    assert_eq!(at("Z", 5, 5), BACKGROUND_DEPTH);
    assert_eq!((at("ObjectID", 60, 50), at("ObjectID", 114, 50), at("ObjectID", 5, 5)), (1.0, 2.0, 0.0));
    assert_eq!(at("Coverage", 60, 50), 1.0);
    // Planes face the camera.
    assert!((at("N.Z", 60, 50) + 1.0).abs() < 1e-5);
    // Cryptomatte rank 0 = the layer under the pixel, full coverage; manifest lists names.
    assert_eq!(at("CryptoObject00.R", 60, 50).to_bits(), crypto_hash("Near"));
    assert_eq!(at("CryptoObject00.G", 60, 50), 1.0);
    assert_eq!(a.manifest_name("CryptoObject", crypto_hash("Far")), Some("Far"));
}

#[test]
fn three_d_channel_effects_work_on_a_precomp_of_a_3d_comp() {
    let near = (60, 50);
    let far = (114, 50);
    let alpha = |p: &Project, oid: ItemId, (x, y): (i64, i64)| render_frame(p, oid, Tick::ZERO, 1.0).get(x, y)[3];
    let (p, _, oid) = scene(Some(("ec.3d.idmatte", vec![("idSelection", Value::Scalar(2.0))])));
    assert_eq!(alpha(&p, oid, near), 0.0);
    assert!(alpha(&p, oid, far) > 0.99);
    let (p, _, oid) = scene(Some(("ec.3d.depthmatte", vec![("depth", Value::Scalar(zoom() + 250.0))])));
    assert_eq!(alpha(&p, oid, near), 0.0);
    assert!(alpha(&p, oid, far) > 0.99);
    let (p, _, oid) = scene(Some(("ec.3d.cryptomatte", vec![("selection", Value::Str("Near".into())), ("display", Value::Enum(1))])));
    assert!(alpha(&p, oid, near) > 0.99);
    assert_eq!(alpha(&p, oid, far), 0.0);
    // Half resolution: the pass follows the render scale.
    let (p, iid, _) = scene(None);
    let r = Renderer::new(&p, &NoFootage, RenderOpts { scale: 0.5, ..Default::default() });
    let a = r.comp_aux(iid, Tick::ZERO).unwrap();
    assert_eq!((a.width, a.height, a.scale), (W / 2, H / 2, 0.5));
    assert_eq!(a.get("ObjectID").unwrap()[25 * 100 + 30], 1.0);
}
