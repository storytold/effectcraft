//! fixture -> import -> export -> import: the two imported projects are equal for the exported
//! subset, and the exporter never fails on what the importer produces.

mod common;

use common::export_all;
use effectcraft_aep_import::{ImportOptions, import};
use effectcraft_project::{Comp, Item, ItemKind, Layer, LayerSource, Project};

fn fixtures() -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../aep/tests/fixtures");
    let mut v: Vec<_> =
        std::fs::read_dir(&dir).expect("fixtures").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "aep")).collect();
    v.sort();
    v
}

fn import_bytes(bytes: &[u8]) -> Option<Project> {
    let mut p = Project::default();
    let none = |_: &str| false;
    let opts = ImportOptions { allow_losses: true, ..ImportOptions::new(&none) };
    import(&mut p, bytes, &opts).ok()?;
    Some(p)
}

/// Items in tree order: (depth, name) with the item.
fn tree<'a>(p: &'a Project, parent: Option<effectcraft_project::ItemId>, depth: usize, out: &mut Vec<(usize, &'a Item)>) {
    let mut kids: Vec<&Item> = p.items.values().filter(|i| i.parent == parent).collect();
    kids.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.0.cmp(&b.id.0)));
    for k in kids {
        out.push((depth, k));
        if k.is_folder() && depth < 50 {
            tree(p, Some(k.id), depth + 1, out);
        }
    }
}

fn same_comp(a: &Comp, b: &Comp, name: &str, pa: &Project, pb: &Project) {
    assert_eq!((a.width, a.height, a.frame_rate, a.duration), (b.width, b.height, b.frame_rate, b.duration), "{name}");
    assert_eq!(a.work_area, b.work_area, "{name} work area");
    assert!((0..3).all(|i| (a.background[i] - b.background[i]).abs() < 1e-6), "{name} background");
    assert!((a.pixel_aspect - b.pixel_aspect).abs() < 1e-3, "{name} pixel aspect");
    assert_eq!(
        (a.shutter_angle, a.shutter_phase, a.enable_motion_blur, a.display_start),
        (b.shutter_angle, b.shutter_phase, b.enable_motion_blur, b.display_start),
        "{name}"
    );
    // Layers that the exporter keeps (solid, null) in order.
    let keep = |c: &Comp| -> Vec<Layer> { c.layers.iter().filter(|l| matches!(l.source, LayerSource::Solid { .. } | LayerSource::Null)).cloned().collect() };
    let (la, lb) = (keep(a), keep(b));
    assert_eq!(la.len(), lb.len(), "{name} layer count");
    for (x, y) in la.iter().zip(&lb) {
        same_layer(x, y, a, b, name, pa, pb);
    }
}

fn same_layer(x: &Layer, y: &Layer, ca: &Comp, cb: &Comp, comp: &str, pa: &Project, pb: &Project) {
    let w = format!("{comp}/{}", x.name);
    assert_eq!(x.name, y.name, "{w}");
    assert_eq!((x.start_time, x.in_point, x.out_point), (y.start_time, y.in_point, y.out_point), "{w} timing");
    assert_eq!(x.blend_mode, y.blend_mode, "{w}");
    assert_eq!(x.label, y.label, "{w}");
    // The 3D switch is not exported (it is reported as a loss).
    let mut sx = x.switches.clone();
    sx.three_d = false;
    assert_eq!(sx, y.switches, "{w} switches");
    let idx = |c: &Comp, p: Option<effectcraft_project::LayerId>| p.and_then(|id| c.index_of(id));
    assert_eq!(idx(ca, x.parent), idx(cb, y.parent), "{w} parent");
    let src = |p: &Project, l: &Layer| match &l.source {
        LayerSource::Solid { item } => p.item(*item).map(|i| (i.name.clone(), i.kind.clone())),
        _ => None,
    };
    assert_eq!(src(pa, x), src(pb, y), "{w} source");
    let (tx, ty) = (x.transform().expect("t"), y.transform().expect("t"));
    for id in ["anchor", "position", "scale", "rotation", "opacity"] {
        let (px, py) = (tx.get(id).expect("p"), ty.get(id).expect("p"));
        let close =
            |a: &effectcraft_keyframe::Value, b: &effectcraft_keyframe::Value| a.components().iter().zip(b.components()).all(|(p, q)| (p - q).abs() < 1e-6);
        assert!(close(&px.value, &py.value), "{w} {id} value {:?} {:?}", px.value, py.value);
        assert_eq!(px.expr, py.expr, "{w} {id} expression");
        assert_eq!(px.keys.len(), py.keys.len(), "{w} {id} key count");
        for (kx, ky) in px.keys.iter().zip(&py.keys) {
            assert_eq!(kx.time, ky.time, "{w} {id} key time");
            assert!(close(&kx.value, &ky.value), "{w} {id} key value");
            assert_eq!(kx.roving, ky.roving, "{w} {id}");
            assert_eq!(kx.label, ky.label, "{w} {id}");
            assert_eq!(kx.spatial_auto, ky.spatial_auto, "{w} {id}");
            if px.spatial && !kx.spatial_auto {
                assert!(kx.spatial_in.iter().zip(ky.spatial_in).all(|(p, q)| (p - q).abs() < 1e-6), "{w} {id} tangent");
            }
            // A side that is not bezier keeps its kind. (Anchor point ease is not exported.)
            for (sx, sy, ex, ey) in [(kx.in_interp, ky.in_interp, &kx.in_ease, &ky.in_ease), (kx.out_interp, ky.out_interp, &kx.out_ease, &ky.out_ease)] {
                if id == "anchor" && sx == effectcraft_keyframe::Interp::Bezier {
                    assert_eq!(sy, effectcraft_keyframe::Interp::Linear);
                    continue;
                }
                assert_eq!(sx, sy, "{w} {id} interpolation");
                if sx == effectcraft_keyframe::Interp::Bezier {
                    assert_eq!(ex.len().max(1), ey.len().max(1), "{w} {id} ease dims");
                    for (a, b) in ex.iter().zip(ey) {
                        assert!((a.speed - b.speed).abs() < 1e-6 && (a.influence - b.influence).abs() < 1e-6, "{w} {id} ease {a:?} {b:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn every_fixture_survives_import_export_import() {
    let mut compared = 0;
    for path in fixtures() {
        let name = path.file_name().expect("name").to_string_lossy().to_string();
        let bytes = std::fs::read(&path).expect("read");
        let Some(p1) = import_bytes(&bytes) else { continue };
        let r = export_all(&p1);
        let p2 = import_bytes(&r.bytes).unwrap_or_else(|| panic!("{name}: the exported file does not import"));
        let (mut t1, mut t2) = (Vec::new(), Vec::new());
        tree(&p1, None, 0, &mut t1);
        tree(&p2, None, 0, &mut t2);
        // The exporter drops footage items and puts root solids in the Solids folder; compare the
        // compositions by name.
        let comps = |t: &[(usize, &Item)]| -> Vec<(String, Comp)> {
            let mut v: Vec<(String, Comp)> =
                t.iter().filter_map(|(_, i)| if let ItemKind::Comp(c) = &i.kind { Some((i.name.clone(), (**c).clone())) } else { None }).collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };
        let (c1, c2) = (comps(&t1), comps(&t2));
        assert_eq!(c1.len(), c2.len(), "{name}: composition count");
        for ((n1, a), (n2, b)) in c1.iter().zip(&c2) {
            assert_eq!(n1, n2, "{name}");
            same_comp(a, b, &format!("{name}/{n1}"), &p1, &p2);
            compared += 1;
        }
    }
    assert!(compared > 100, "only {compared} compositions compared");
}
