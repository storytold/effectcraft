use super::mesh::*;
use super::*;
use crate::{EffectEnv, flatten_params, instantiate};

fn disc_alpha(w: usize, h: usize, c: [f64; 2], r: f64) -> Vec<f32> {
    let mut a = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 + 0.5 - c[0]).powi(2) + (y as f64 + 0.5 - c[1]).powi(2)).sqrt();
            a[y * w + x] = if d <= r { 1.0 } else { 0.0 };
        }
    }
    a
}

fn check_valid(m: &Mesh) {
    assert!(!m.tris.is_empty());
    for t in &m.tris {
        let a = cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]);
        assert!(a > 0.0, "flipped or degenerate triangle {t:?} area {a}");
    }
    let tri_area: f64 = m.tris.iter().map(|t| cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]) * 0.5).sum();
    let poly_area = signed_area(&m.outline).abs();
    assert!((tri_area - poly_area).abs() < 1e-6 * poly_area.max(1.0), "triangles {tri_area} vs outline {poly_area}");
}

fn covered(m: &Mesh, p: [f64; 2]) -> bool {
    m.tris.iter().any(|t| {
        let (a, b, c) = (m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]);
        cross(a, b, p) >= -1e-9 && cross(b, c, p) >= -1e-9 && cross(c, a, p) >= -1e-9
    })
}

#[test]
fn distance_transform_matches_brute_force() {
    let (w, h) = (13, 9);
    let src: Vec<bool> = (0..w * h).map(|i| i % 17 == 3 || i == 50).collect();
    let d = distance_sq(w, h, &src);
    for y in 0..h {
        for x in 0..w {
            let mut best = f64::MAX;
            for yy in 0..h {
                for xx in 0..w {
                    if src[yy * w + xx] {
                        best = best.min(((x as f64 - xx as f64).powi(2)) + (y as f64 - yy as f64).powi(2));
                    }
                }
            }
            assert_eq!(d[y * w + x], best, "({x},{y})");
        }
    }
}

#[test]
fn triangulation_is_valid_and_covers_the_alpha() {
    let (w, h) = (120, 100);
    let alpha = disc_alpha(w, h, [60.0, 50.0], 38.0);
    let outline = outline(&alpha, w, h, 1.0, [0.0; 2], [60.0, 50.0], 3.0);
    let m = triangulate(&outline, &MeshOpts::default());
    check_valid(&m);
    assert!(m.tris.len() > 150 && m.tris.len() < 800, "{} triangles", m.tris.len());
    for y in 0..h {
        for x in 0..w {
            if alpha[y * w + x] > 0.0 {
                assert!(covered(&m, [x as f64 + 0.5, y as f64 + 0.5]), "alpha pixel ({x},{y}) not covered");
            }
        }
    }
    // Density controls the triangle count.
    let fine = triangulate(&outline, &MeshOpts { density: 100.0, ..MeshOpts::default() });
    check_valid(&fine);
    assert!(fine.tris.len() > m.tris.len() * 2);
}

#[test]
fn concave_outline_keeps_its_edges() {
    // A U shape (concave) and an L shape.
    let (w, h) = (100, 100);
    let mut alpha = vec![0.0f32; w * h];
    for y in 10..90 {
        for x in 10..90 {
            let notch = (35..65).contains(&x) && y < 70;
            alpha[y * w + x] = if notch { 0.0 } else { 1.0 };
        }
    }
    let o = outline(&alpha, w, h, 1.0, [0.0; 2], [20.0, 50.0], 0.0);
    let m = triangulate(&o, &MeshOpts { triangles: 120.0, ..Default::default() });
    check_valid(&m);
    // Nothing in the notch.
    assert!(!covered(&m, [50.0, 30.0]));
    for (x, y) in [(15.0, 15.0), (85.0, 15.0), (50.0, 80.0), (12.0, 88.0)] {
        assert!(covered(&m, [x, y]), "({x},{y})");
    }
}

#[test]
fn expansion_grows_the_outline() {
    let (w, h) = (80, 80);
    let alpha = disc_alpha(w, h, [40.0, 40.0], 20.0);
    let a0 = signed_area(&outline(&alpha, w, h, 1.0, [0.0; 2], [40.0, 40.0], 0.0)).abs();
    let a5 = signed_area(&outline(&alpha, w, h, 1.0, [0.0; 2], [40.0, 40.0], 5.0)).abs();
    let am = signed_area(&outline(&alpha, w, h, 1.0, [0.0; 2], [40.0, 40.0], -5.0)).abs();
    let pi = std::f64::consts::PI;
    assert!((a0 - pi * 400.0).abs() < 60.0, "{a0}");
    assert!((a5 - pi * 625.0).abs() < 90.0, "{a5}");
    assert!((am - pi * 225.0).abs() < 60.0, "{am}");
}

fn rect_mesh(w: f64, h: f64, tris: f64) -> Mesh {
    triangulate(&[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]], &MeshOpts { triangles: tris, density: 50.0, expansion: 0.0 })
}

fn pin(kind: PinKind, rest: [f64; 2], pos: [f64; 2]) -> Pin {
    Pin { kind, uid: 0, rest, position: pos, scale: 1.0, rotation: 0.0, amount: 50.0, extent: 15.0, in_front: 50.0 }
}

#[test]
fn arap_identity_with_pins_at_rest() {
    let m = rect_mesh(100.0, 60.0, 200.0);
    check_valid(&m);
    let pins = [pin(PinKind::Position, [0.0, 0.0], [0.0, 0.0]), pin(PinKind::Position, [100.0, 60.0], [100.0, 60.0])];
    let d = solve(&m, &pins, 2);
    for (a, b) in d.iter().zip(&m.verts) {
        assert!((a[0] - b[0]).abs() < 1e-7 && (a[1] - b[1]).abs() < 1e-7, "{a:?} vs {b:?}");
    }
}

#[test]
fn arap_single_pin_translates_rigidly() {
    let m = rect_mesh(100.0, 60.0, 200.0);
    let d = solve(&m, &[pin(PinKind::Position, [50.0, 30.0], [70.0, 15.0])], 2);
    let v = m.verts[nearest(&m.verts, [50.0, 30.0]).unwrap()];
    let t = [70.0 - 50.0 + (v[0] - 50.0) * 0.0, 15.0 - 30.0];
    for (a, b) in d.iter().zip(&m.verts) {
        assert!((a[0] - b[0] - t[0]).abs() < 1e-7 && (a[1] - b[1] - t[1]).abs() < 1e-7, "{a:?} vs {b:?}");
    }
}

#[test]
fn arap_two_pins_rotate_rigidly() {
    let m = rect_mesh(100.0, 40.0, 200.0);
    let (a, b) = (m.verts[nearest(&m.verts, [0.0, 20.0]).unwrap()], m.verts[nearest(&m.verts, [100.0, 20.0]).unwrap()]);
    let c = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let ang = 30f64.to_radians();
    let rot = |p: [f64; 2]| {
        let (s, co) = ang.sin_cos();
        let d = [p[0] - c[0], p[1] - c[1]];
        [c[0] + d[0] * co - d[1] * s, c[1] + d[0] * s + d[1] * co]
    };
    let pins = [pin(PinKind::Position, a, rot(a)), pin(PinKind::Position, b, rot(b))];
    let d = solve(&m, &pins, 0);
    for (p, r) in d.iter().zip(&m.verts) {
        let e = rot(*r);
        assert!((p[0] - e[0]).abs() < 1e-3 && (p[1] - e[1]).abs() < 1e-3, "{p:?} vs {e:?}");
    }
}

fn distortion(m: &Mesh, d: &[[f64; 2]], near: [f64; 2], radius: f64) -> f64 {
    let mut s = 0.0;
    for t in &m.tris {
        let c = [(m.verts[t[0]][0] + m.verts[t[1]][0] + m.verts[t[2]][0]) / 3.0, (m.verts[t[0]][1] + m.verts[t[1]][1] + m.verts[t[2]][1]) / 3.0];
        if (c[0] - near[0]).hypot(c[1] - near[1]) > radius {
            continue;
        }
        for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let l0 = (m.verts[i][0] - m.verts[j][0]).hypot(m.verts[i][1] - m.verts[j][1]);
            let l1 = (d[i][0] - d[j][0]).hypot(d[i][1] - d[j][1]);
            s += (l1 / l0 - 1.0).powi(2);
        }
    }
    s
}

#[test]
fn starch_reduces_distortion() {
    let m = rect_mesh(200.0, 40.0, 300.0);
    // Squash the bar: both ends pushed inwards and one end lifted.
    let base = vec![pin(PinKind::Position, [0.0, 20.0], [30.0, 20.0]), pin(PinKind::Position, [200.0, 20.0], [160.0, 60.0])];
    let plain = solve(&m, &base, 2);
    let mut st = base.clone();
    st.push(Pin { amount: 100.0, extent: 30.0, ..pin(PinKind::Starch, [100.0, 20.0], [100.0, 20.0]) });
    let starched = solve(&m, &st, 2);
    let (a, b) = (distortion(&m, &plain, [100.0, 20.0], 25.0), distortion(&m, &starched, [100.0, 20.0], 25.0));
    assert!(b < a * 0.5, "starch {b} vs plain {a}");
}

#[test]
fn bend_pin_rotates_around_it() {
    let m = rect_mesh(100.0, 40.0, 200.0);
    let mut bend = pin(PinKind::Bend, [100.0, 20.0], [100.0, 20.0]);
    bend.rotation = 45.0;
    let d = solve(&m, &[pin(PinKind::Position, [0.0, 20.0], [0.0, 20.0]), bend], 2);
    // The far end turned downwards (positive rotation in y-down layer space).
    let v = nearest(&m.verts, [100.0, 40.0]).unwrap();
    let u = nearest(&m.verts, [100.0, 0.0]).unwrap();
    assert!(d[v][0] < d[u][0] - 5.0, "bottom {:?} top {:?}", d[v], d[u]);
}

#[test]
fn overlap_pins_set_depth() {
    let m = rect_mesh(100.0, 40.0, 100.0);
    let mut o = pin(PinKind::Overlap, [20.0, 20.0], [20.0, 20.0]);
    o.in_front = 80.0;
    o.extent = 15.0;
    let d = depths(&m, &[o]);
    assert!(d.contains(&80.0));
    assert!(d.contains(&0.0));
}

fn puppet_fx(pins: &[(PinKind, [f64; 2], [f64; 2])]) -> PropGroup {
    let mut next = 5000;
    let mut ids = Ids(&mut next);
    let mut fx = instantiate(crate::find(ID).unwrap(), &mut ids, "Puppet", [64.0, 64.0]);
    let mut mesh = mesh_group(&mut ids, "Mesh 1", pins.first().map(|p| p.1).unwrap_or([32.0; 2]), &MeshOpts::default());
    for (k, (kind, rest, pos)) in pins.iter().enumerate() {
        let mut g = pin_group(&mut ids, &format!("Puppet Pin {}", k + 1), *kind, *rest);
        if let Some(p) = g.get_mut("position") {
            p.value = Value::Vec2(*pos);
        }
        mesh.sub_mut("deform").unwrap().children.push(g.into());
    }
    fx.children.push(mesh.into());
    fx
}

fn run(fx: &PropGroup, img: Image) -> Buf {
    let params = flatten_params(fx, &mut |p| p.value.clone());
    let ctx =
        EffectCtx { params: &params, time: 0.0, layer_size: [img.width as f64, img.height as f64], seed: 1, adjustment: false, env: EffectEnv::default() };
    crate::apply(crate::find(ID).unwrap(), &ctx, Buf { img, offset: [0.0; 2], scale: 1.0 })
}

fn test_image() -> Image {
    let mut img = Image::new(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            if (12..52).contains(&x) && (16..48).contains(&y) {
                img.set(x, y, [x as f32 / 64.0, y as f32 / 64.0, ((x + y) % 3) as f32 / 3.0, 1.0]);
            }
        }
    }
    img
}

/// Output pixel at layer pixel (x, y).
fn at(b: &Buf, x: i64, y: i64) -> [f32; 4] {
    b.img.get(x + b.offset[0] as i64, y + b.offset[1] as i64)
}

#[test]
fn render_identity_and_translation() {
    let img = test_image();
    let id = run(&puppet_fx(&[(PinKind::Position, [30.0, 30.0], [30.0, 30.0])]), img.clone());
    for y in 17..47 {
        for x in 13..51 {
            let (o, e) = (at(&id, x, y), img.get(x, y));
            assert!((0..4).all(|c| (o[c] - e[c]).abs() < 1e-5), "({x},{y}) {o:?} vs {e:?}");
        }
    }
    let moved = run(&puppet_fx(&[(PinKind::Position, [30.0, 30.0], [40.0, 35.0])]), img.clone());
    for y in 17..47 {
        for x in 13..51 {
            let o = at(&moved, x + 10, y + 5);
            let e = img.get(x, y);
            for c in 0..4 {
                assert!((o[c] - e[c]).abs() < 1e-4, "({x},{y}) {o:?} vs {e:?}");
            }
        }
    }
    // Outside the mesh: nothing.
    assert_eq!(at(&moved, 2, 2), [0.0; 4]);
}

#[test]
fn render_is_deterministic_and_antialiased() {
    let img = test_image();
    let fx = puppet_fx(&[(PinKind::Position, [14.0, 30.0], [14.0, 30.0]), (PinKind::Position, [50.0, 30.0], [45.0, 10.0])]);
    let a = run(&fx, img.clone());
    let b = run(&fx, img);
    assert_eq!(a.img, b.img);
    assert_eq!(a.offset, b.offset);
    assert!(a.img.data.iter().any(|p| p[3] > 0.05 && p[3] < 0.95), "expected soft edges");
    assert!(a.img.data.iter().all(|p| p[3] <= 1.0001 && p.iter().all(|c| c.is_finite())));
}

#[test]
fn unmap_inverts_the_deformation() {
    let m = rect_mesh(100.0, 40.0, 150.0);
    let pins = [pin(PinKind::Position, [0.0, 20.0], [10.0, 30.0]), pin(PinKind::Position, [100.0, 20.0], [90.0, 0.0])];
    let d = solve(&m, &pins, 1);
    let q = unmap(&m, &d, d[5]);
    assert!((q[0] - m.verts[5][0]).abs() < 1e-6 && (q[1] - m.verts[5][1]).abs() < 1e-6);
}

#[test]
fn groups_round_trip_serde() {
    let fx = puppet_fx(&[(PinKind::Position, [1.0, 2.0], [3.0, 4.0]), (PinKind::Starch, [5.0, 5.0], [5.0, 5.0])]);
    let j = serde_json::to_string(&fx).unwrap();
    let back: PropGroup = serde_json::from_str(&j).unwrap();
    assert_eq!(back, fx);
    assert_eq!(pin_count(&back), 2);
    let params = flatten_params(&back, &mut |p| p.value.clone());
    let m = parse(&params);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].pins.len(), 2);
    assert_eq!(m[0].pins[1].kind, PinKind::Starch);
    assert_eq!(m[0].pins[0].position, [3.0, 4.0]);
}

#[test]
fn single_pin_on_a_traced_solid_does_not_rotate_it() {
    // #313: an 800×600 solid traced from its alpha, as the Puppet Pin tool builds it. One
    // pin is not enough to rotate or scale the mesh, so it may only translate it.
    let (w, h) = (800, 600);
    let o = outline(&vec![1.0; w * h], w, h, 1.0, [0.0; 2], [400.0, 300.0], 3.0);
    let m = triangulate(&o, &MeshOpts::default());
    check_valid(&m);
    for at in [[40.0, 40.0], [760.0, 30.0], [20.0, 580.0], [790.0, 590.0], [400.0, 300.0], [123.0, 456.0]] {
        for t in [[0.0, 0.0], [35.0, -20.0]] {
            let d = solve(&m, &[pin(PinKind::Position, at, [at[0] + t[0], at[1] + t[1]])], 2);
            let err = d.iter().zip(&m.verts).map(|(a, b)| (a[0] - b[0] - t[0]).hypot(a[1] - b[1] - t[1])).fold(0.0, f64::max);
            assert!(err < 1e-6, "pin at {at:?} moved by {t:?}: vertices off by up to {err} px");
        }
    }
}
