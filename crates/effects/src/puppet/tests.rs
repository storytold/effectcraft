use std::collections::HashMap;

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

// ------------------------------------------------------------------------------------------
// #2866: deformation quality, pinned to measurements on far drags. The "was" numbers are the
// renderer before that work; these meshes are what the Puppet Pin tool builds for a 1280x720
// solid.

fn traced_solid() -> Mesh {
    let (w, h) = (1280, 720);
    let o = outline(&vec![1.0; w * h], w, h, 1.0, [0.0; 2], [640.0, 360.0], 3.0);
    triangulate(&o, &MeshOpts::default())
}

fn three_pins(dragged_to: [f64; 2]) -> Vec<Pin> {
    let (a, b, c) = ([200.0, 360.0], [640.0, 360.0], [1080.0, 360.0]);
    vec![pin(PinKind::Position, a, a), pin(PinKind::Position, b, b), pin(PinKind::Position, c, dragged_to)]
}

fn default_rounds() -> usize {
    refinement_rounds(&flatten_params(&puppet_fx(&[]), &mut |p| p.value.clone()))
}

/// The deformation gradient of triangle `t`, and its singular values (the second negative when
/// the triangle is flipped).
fn gradient(rest: &[[f64; 2]], def: &[[f64; 2]], t: &[usize; 3]) -> ([f64; 4], f64, f64) {
    let e = |p: &[[f64; 2]], a: usize, b: usize| [p[t[b]][0] - p[t[a]][0], p[t[b]][1] - p[t[a]][1]];
    let (e1, e2, f1, f2) = (e(rest, 0, 1), e(rest, 0, 2), e(def, 0, 1), e(def, 0, 2));
    let det = e1[0] * e2[1] - e2[0] * e1[1];
    let g = [
        (f1[0] * e2[1] - f2[0] * e1[1]) / det,
        (f2[0] * e1[0] - f1[0] * e2[0]) / det,
        (f1[1] * e2[1] - f2[1] * e1[1]) / det,
        (f2[1] * e1[0] - f1[1] * e2[0]) / det,
    ];
    let (q, r) = (((g[0] + g[3]).hypot(g[2] - g[1])) / 2.0, ((g[0] - g[3]).hypot(g[2] + g[1])) / 2.0);
    let flipped = g[0] * g[3] - g[1] * g[2] < 0.0;
    (g, q + r, if flipped { -(q - r).abs() } else { (q - r).abs() })
}

/// Smallest singular value, and the largest and mean jump of the gradient across an edge (the
/// crease a straight line in the image gets there).
fn creases(rest: &[[f64; 2]], def: &[[f64; 2]], tris: &[[usize; 3]]) -> (f64, f64, f64) {
    let grads: Vec<_> = tris.iter().map(|t| gradient(rest, def, t)).collect();
    let mut across: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (k, t) in tris.iter().enumerate() {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            across.entry((a.min(b), a.max(b))).or_default().push(k);
        }
    }
    let jumps: Vec<f64> =
        across.values().filter(|k| k.len() == 2).map(|k| (0..4).map(|i| (grads[k[0]].0[i] - grads[k[1]].0[i]).powi(2)).sum::<f64>().sqrt()).collect();
    (grads.iter().map(|g| g.2).fold(f64::MAX, f64::min), jumps.iter().copied().fold(0.0, f64::max), jumps.iter().sum::<f64>() / jumps.len().max(1) as f64)
}

/// The rigidity energy per unit area: how far the triangles' gradients are from rotations.
fn rigidity(m: &Mesh, def: &[[f64; 2]]) -> f64 {
    let (mut e, mut a) = (0.0, 0.0);
    for t in &m.tris {
        let (_, s1, s2) = gradient(&m.verts, def, t);
        let area = cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]) * 0.5;
        e += area * ((s1 - 1.0).powi(2) + (s2 - 1.0).powi(2));
        a += area;
    }
    e / a
}

#[test]
fn the_default_refinement_converges_on_a_far_drag() {
    // Was: 1.13 mean edges from the converged shape, with twice its rigidity energy (2 rounds).
    let m = traced_solid();
    let pins = three_pins([1000.0, -60.0]);
    let d = solve(&m, &pins, default_rounds());
    let end = solve(&m, &pins, 400);
    let edge = m.tris.iter().map(|t| (m.verts[t[0]][0] - m.verts[t[1]][0]).hypot(m.verts[t[0]][1] - m.verts[t[1]][1])).sum::<f64>() / m.tris.len() as f64;
    let rms = (d.iter().zip(&end).map(|(a, b)| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sum::<f64>() / d.len() as f64).sqrt() / edge;
    assert!(rms < 0.06, "{rms} mean edges from the converged shape");
    let (e, conv) = (rigidity(&m, &d), rigidity(&m, &end));
    assert!(e < conv * 1.02, "rigidity energy {e} vs converged {conv}");
}

#[test]
fn pins_do_not_crease_or_fold_the_mesh() {
    let m = traced_solid();
    let rounds = default_rounds();
    // Dragged far: was smallest singular value -0.07 (a fold), largest crease 2.13.
    let (s, crease, _) = creases(&m.verts, &solve(&m, &three_pins([1000.0, -60.0]), rounds), &m.tris);
    assert!(s > 0.6 && crease < 0.4, "smallest singular value {s}, crease {crease}");
    // Pulled straight out: was -0.10 and 2.45, three folded triangles and a chevron through the pins.
    let (s, crease, _) = creases(&m.verts, &solve(&m, &three_pins([1500.0, 360.0]), rounds), &m.tris);
    assert!(s > 0.6 && crease < 0.65, "smallest singular value {s}, crease {crease}");
}

/// How a drag's strain spreads: (share of the strain energy within one mean edge of a pin, share
/// of the mesh's area there, largest ring's mean strain over the mesh's, 99th percentile over mean).
fn spread(m: &Mesh, pins: &[Pin], def: &[[f64; 2]]) -> (f64, f64, f64, f64) {
    let edge = m.tris.iter().map(|t| (m.verts[t[0]][0] - m.verts[t[1]][0]).hypot(m.verts[t[0]][1] - m.verts[t[1]][1])).sum::<f64>() / m.tris.len() as f64;
    let held: Vec<[f64; 2]> = pins.iter().map(|p| m.verts[nearest(&m.verts, p.rest).unwrap()]).collect();
    let (mut near, mut near_area, mut total, mut area, mut mean) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let mut rings = vec![(0.0, 0.0); 32];
    let mut all = vec![];
    for t in &m.tris {
        let (_, s1, s2) = gradient(&m.verts, def, t);
        let strain = ((s1 - 1.0).powi(2) + (s2 - 1.0).powi(2)).sqrt();
        let a = cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]) * 0.5;
        let c = centroid(m, t);
        let r = held.iter().map(|h| (h[0] - c[0]).hypot(h[1] - c[1])).fold(f64::MAX, f64::min) / edge;
        if r <= 1.0 {
            near += a * strain * strain;
            near_area += a;
        }
        let ring = &mut rings[(r as usize).min(31)];
        ring.0 += a * strain;
        ring.1 += a;
        total += a * strain * strain;
        area += a;
        mean += a * strain;
        all.push(strain);
    }
    mean /= area;
    all.sort_by(f64::total_cmp);
    let peak = rings.iter().filter(|r| r.1 > 0.0).map(|r| r.0 / r.1).fold(0.0, f64::max);
    (near / total, near_area / area, peak / mean, all[(all.len() - 1) * 99 / 100] / mean)
}

#[test]
fn a_pin_pull_spreads_through_the_mesh() {
    let m = traced_solid();
    let rounds = default_rounds();
    // Holding one vertex: 38 % of the strain energy on the 5 % of the mesh by the pins, the ring
    // there at 3.3 times the mean strain. A stiff patch only moved that to a ring around the patch.
    for to in [[1000.0, -60.0], [1080.0, 240.0]] {
        let pins = three_pins(to);
        let (near, near_area, ring, p99) = spread(&m, &pins, &solve(&m, &pins, rounds));
        assert!(near < near_area * 1.3, "{near} of the strain on {near_area} of the area");
        assert!(ring < 1.25 && p99 < 2.0, "largest ring {ring}, 99th percentile {p99} times the mean");
    }
}

#[test]
fn a_pin_added_where_the_mesh_already_is_barely_moves_it() {
    // #318 on a far drag. It isn't nothing: the mesh settles towards the pins' rigid fit, and a new
    // pin changes that fit and frees the mesh around it. It was 77 px on this kind of mesh when the
    // solve stopped short of converging.
    let m = traced_solid();
    let pins = three_pins([1000.0, -60.0]);
    let d = solve(&m, &pins, default_rounds());
    for at in [[420.0, 200.0], [860.0, 500.0], [1100.0, 150.0]] {
        let v = nearest(&m.verts, at).unwrap();
        let mut more = pins.clone();
        more.push(pin(PinKind::Position, m.verts[v], d[v]));
        let after = solve(&m, &more, default_rounds());
        let moved = after.iter().zip(&d).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).fold(0.0, f64::max);
        assert!(moved < 35.0, "a pin at {at:?} moved the mesh by {moved} px");
    }
}

#[test]
fn a_pin_holds_the_point_under_it() {
    // Was: the nearest vertex at a fixed offset, up to half an edge from the pin once the mesh
    // turned, and a second pin on the same vertex was dropped.
    let m = traced_solid();
    let (a, b) = (m.verts[nearest(&m.verts, [640.0, 360.0]).unwrap()], [650.0, 372.0]);
    let pins = [
        pin(PinKind::Position, [203.0, 351.0], [203.0, 351.0]),
        pin(PinKind::Position, a, [a[0], a[1] - 60.0]),
        pin(PinKind::Position, b, [b[0] + 30.0, b[1] + 70.0]),
        pin(PinKind::Position, [1077.0, 368.0], [1000.0, -60.0]),
    ];
    let d = solve(&m, &pins, default_rounds());
    for p in &pins {
        let (t, l) = locate(&m, p.rest).unwrap();
        let at = [l[0] * d[t[0]][0] + l[1] * d[t[1]][0] + l[2] * d[t[2]][0], l[0] * d[t[0]][1] + l[1] * d[t[1]][1] + l[2] * d[t[2]][1]];
        assert!((at[0] - p.position[0]).hypot(at[1] - p.position[1]) < 0.05, "{at:?} vs {:?}", p.position);
    }
}

#[test]
fn a_bend_pin_alone_stays_finite_and_turns_the_mesh() {
    // No held vertex at all: only the regulariser pins down the translation.
    let m = traced_solid();
    let mut bend = pin(PinKind::Bend, [640.0, 360.0], [640.0, 360.0]);
    bend.rotation = 20.0;
    let d = solve(&m, &[bend], default_rounds());
    assert!(d.iter().flatten().all(|c| c.is_finite()));
    let moved = d.iter().zip(&m.verts).map(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1])).fold(0.0, f64::max);
    assert!(moved > 1.0 && moved < 1000.0, "moved {moved}");
}

#[test]
fn drawing_follows_a_smooth_curve_through_the_solve() {
    let m = traced_solid();
    let d = solve(&m, &three_pins([1000.0, -60.0]), default_rounds());
    let sm = smooth_mesh(&m, &d, 1.0);
    // Mesh vertices (and so the pins) stay exactly where the solve put them.
    assert_eq!(&sm.deformed[..d.len()], &d[..]);
    assert_eq!(&sm.rest[..m.verts.len()], &m.verts[..]);
    // Was: creases of 0.127 on average and 2.13 at worst at the triangles' edges.
    let (s, worst, mean) = creases(&sm.rest, &sm.deformed, &sm.tris);
    assert!(s > 0.4 && worst < 0.3 && mean < 0.04, "smallest singular value {s}, creases {worst} worst, {mean} mean");
    // No cracks: the small triangles cover the rest mesh, and an edge only ever has two sides.
    let area: f64 = sm.tris.iter().map(|t| cross(sm.rest[t[0]], sm.rest[t[1]], sm.rest[t[2]]) * 0.5).sum();
    let want: f64 = m.tris.iter().map(|t| cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]) * 0.5).sum();
    assert!((area - want).abs() < 1e-6 * want, "{area} vs {want}");
    let mut sides: HashMap<(usize, usize), u32> = HashMap::new();
    for t in &sm.tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *sides.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    assert!(sides.values().all(|n| *n <= 2));
    let open_small = sides.values().filter(|n| **n == 1).count();
    let open_mesh = {
        let mut e: HashMap<(usize, usize), u32> = HashMap::new();
        for t in &m.tris {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *e.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        e.values().filter(|n| **n == 1).count()
    };
    let split = (sm.tris.len() as f64 / m.tris.len() as f64).sqrt().round() as usize;
    assert_eq!(open_small, open_mesh * split);
}

#[test]
fn smoothing_leaves_a_rigid_mesh_alone() {
    let m = rect_mesh(100.0, 40.0, 200.0);
    // Turned 30 degrees and moved: one affine map over the whole mesh.
    let (s, c) = 30f64.to_radians().sin_cos();
    let d: Vec<[f64; 2]> = m.verts.iter().map(|p| [p[0] * c - p[1] * s + 7.0, p[0] * s + p[1] * c - 3.0]).collect();
    let sm = smooth_mesh(&m, &d, 1.0);
    assert_eq!(sm.tris, m.tris);
    assert_eq!(sm.deformed, d);
}

/// `rasterize` over one quad (two triangles) from `rest` to `to`, both [x0, y0, x1, y1].
fn draw_quad(img: Image, rest: [f64; 4], to: [f64; 4]) -> Buf {
    let c = |q: [f64; 4]| [[q[0], q[1]], [q[2], q[1]], [q[2], q[3]], [q[0], q[3]]];
    let (r, d) = (c(rest), c(to));
    let tris = [([r[0], r[1], r[2]], [d[0], d[1], d[2]]), ([r[0], r[2], r[3]], [d[0], d[2], d[3]])];
    rasterize(&Buf { img, offset: [0.0; 2], scale: 1.0 }, &tris)
}

#[test]
fn a_squeezed_image_averages_instead_of_skipping_texels() {
    // One-pixel stripes squeezed to a third: one read per pixel lands on every third stripe,
    // which was solid black and white bars (0.5 off the true grey).
    let mut img = Image::new(150, 40);
    for y in 0..40 {
        for x in 0..150 {
            let v = (x % 2) as f32;
            img.set(x, y, [v, v, v, 1.0]);
        }
    }
    let out = draw_quad(img, [0.0, 0.0, 150.0, 40.0], [0.0, 0.0, 50.0, 40.0]);
    let worst = (5..45).map(|x| (at(&out, x, 20)[0] - 0.5).abs()).fold(0.0, f32::max);
    assert!(worst < 0.2, "{worst} off grey");
}

#[test]
fn the_outline_has_a_smooth_ramp_and_shared_edges_no_seam() {
    let mut img = Image::new(100, 100);
    for y in 0..100 {
        for x in 0..100 {
            img.set(x, y, [1.0, 1.0, 1.0, 1.0]);
        }
    }
    // A mesh inside the opaque image, so its own edge is the outline. Sheared so the edge
    // crosses pixels at every offset: was 3 partial alpha levels (2x2 coverage).
    let (r, d) = ([[20.0, 20.0], [80.0, 20.0], [80.0, 80.0], [20.0, 80.0]], [[20.0, 20.0], [80.0, 27.0], [73.0, 87.0], [13.0, 80.0]]);
    let tris = [([r[0], r[1], r[2]], [d[0], d[1], d[2]]), ([r[0], r[2], r[3]], [d[0], d[2], d[3]])];
    let out = rasterize(&Buf { img, offset: [0.0; 2], scale: 1.0 }, &tris);
    let mut levels: Vec<i32> = out.img.data.iter().filter(|p| p[3] > 0.0 && p[3] < 1.0).map(|p| (p[3] * 16.0).round() as i32).collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(levels.len() >= 12, "alpha levels {levels:?}");
    // The diagonal both triangles share leaves every pixel along it opaque.
    for k in 8..52 {
        let (x, y) = (20 + k - k / 9, 20 + k + k / 9);
        assert_eq!(at(&out, x, y)[3], 1.0, "({x},{y})");
    }
}

#[test]
fn a_sheet_dragged_over_the_mesh_stays_whole() {
    // The right half is green, the left half red; dragging the right end back over the left
    // lays one half across the other. It was a shatter of both, by triangle number, about half of each.
    let (w, h) = (640u32, 360u32);
    let mut img = Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.set(x, y, if x < w / 2 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0, 1.0] });
        }
    }
    let fx = puppet_fx(&[
        (PinKind::Position, [100.0, 180.0], [100.0, 180.0]),
        (PinKind::Position, [320.0, 100.0], [320.0, 100.0]),
        (PinKind::Position, [540.0, 180.0], [100.0, 180.0]),
    ]);
    let out = run(&fx, img);
    // Where the two halves overlap (around the dragged pin), the dragged one is on top.
    let (mut red, mut green) = (0, 0);
    for y in 172..188 {
        for x in 92..108 {
            let p = at(&out, x, y);
            if p[3] > 0.99 {
                if p[0] > 0.9 {
                    red += 1;
                } else if p[1] > 0.9 {
                    green += 1;
                }
            }
        }
    }
    assert!(green > 200 && red < 45, "green {green}, red {red}");
}

#[test]
fn a_pin_with_no_number_for_a_position_draws_nothing_and_does_not_crash() {
    let fx = puppet_fx(&[(PinKind::Position, [14.0, 30.0], [14.0, 30.0]), (PinKind::Position, [50.0, 30.0], [f64::NAN, 10.0])]);
    let out = run(&fx, test_image());
    assert!(out.img.data.iter().all(|p| p.iter().all(|c| c.is_finite())));
}

/// The stress test for how the mesh shares out a pull: a bar of four squares with a pin at the
/// centre of each, the second up by a square and the third down by one.
fn s_curve(triangles: f64) -> (Mesh, Vec<Pin>, Vec<[f64; 2]>) {
    let x = 200.0;
    let (w, h) = (800, 200);
    let o = outline(&vec![1.0; w * h], w, h, 1.0, [0.0; 2], [100.0, 100.0], 3.0);
    let m = triangulate(&o, &MeshOpts { triangles, ..MeshOpts::default() });
    let at = |k: f64, up: f64| pin(PinKind::Position, [(k + 0.5) * x, x / 2.0], [(k + 0.5) * x, x / 2.0 + up]);
    let pins = vec![at(0.0, 0.0), at(1.0, -x), at(2.0, x), at(3.0, 0.0)];
    let d = solve(&m, &pins, default_rounds());
    (m, pins, d)
}

#[test]
fn the_s_curve_shares_its_strain_out() {
    // At 350 and 2000 triangles, for origin/main and then for stiff patches at the pins:
    // smallest singular value -0.73, -0.78 and -0.21, -0.88 (6, 15, 3 and 22 flipped triangles);
    // largest crease 2.82, 4.67 and 1.98, 1.78; largest ring 2.55, 5.62 and 1.15, 1.56 times the
    // mean strain. origin/main had 35 % and 26 % of the strain energy within an edge of the pins.
    for (triangles, min_s, max_crease) in [(350.0, 0.2, 0.9), (2000.0, 0.25, 0.6)] {
        let (m, pins, d) = s_curve(triangles);
        let (s, crease, _) = creases(&m.verts, &d, &m.tris);
        assert!(s > min_s, "{triangles} triangles: smallest singular value {s}");
        assert!(crease < max_crease, "{triangles} triangles: crease {crease}");
        let (near, near_area, ring, _) = spread(&m, &pins, &d);
        assert!(near < near_area * 1.1, "{triangles} triangles: {near} of the strain on {near_area} of the area");
        assert!(ring < 1.2, "{triangles} triangles: largest ring {ring} times the mean");
    }
}

#[test]
fn the_s_curve_is_as_even_at_the_second_pin_as_at_the_third() {
    // The bar and the pull are symmetric under a half turn about the centre. A pin on its nearest
    // vertex broke that (the band beyond the third pin was crushed, not the second's).
    let (m, _, d) = s_curve(350.0);
    let band = |cx: f64, up: bool| {
        // Width of the mesh along the line through the pin, on its outer side.
        let (mut low, mut high) = (f64::MAX, f64::MIN);
        for (r, p) in m.verts.iter().zip(&d) {
            if (r[0] - cx).abs() < 40.0 && (r[1] < 100.0) == up {
                low = low.min(p[1]);
                high = high.max(p[1]);
            }
        }
        high - low
    };
    let (top, bottom) = (band(300.0, true), band(500.0, false));
    assert!((top - bottom).abs() < 0.2 * top.max(bottom), "outer bands {top} and {bottom}");
}

/// The stress test for keeping volume and staying local: a square with four pins in a cross, each
/// a fifth of the side from the centre, the top one pulled up to the top edge and the bottom one
/// down to the bottom edge.
fn cross_case(triangles: f64) -> (Mesh, Vec<[f64; 2]>) {
    let size = 800.0;
    let n = size as usize;
    let c = size / 2.0;
    let off = 0.2 * size;
    let o = outline(&vec![1.0; n * n], n, n, 1.0, [0.0; 2], [c, c], 3.0);
    let m = triangulate(&o, &MeshOpts { triangles, ..MeshOpts::default() });
    let hold = |p: [f64; 2]| pin(PinKind::Position, p, p);
    let pins = [pin(PinKind::Position, [c, c - off], [c, 0.0]), pin(PinKind::Position, [c, c + off], [c, size]), hold([c - off, c]), hold([c + off, c])];
    let d = solve(&m, &pins, default_rounds());
    (m, d)
}

#[test]
fn the_cross_keeps_its_volume_and_stays_local() {
    // Rest area of the mesh's outline against the deformed, through the triangles' edges.
    for (triangles, min_s) in [(350.0, 0.15), (2000.0, 0.2)] {
        let (m, d) = cross_case(triangles);
        let area = |p: &[[f64; 2]]| m.tris.iter().map(|t| cross(p[t[0]], p[t[1]], p[t[2]]) * 0.5).sum::<f64>();
        let grown = area(&d) / area(&m.verts) - 1.0;
        let moved = |at: [f64; 2]| {
            let v = nearest(&m.verts, at).unwrap();
            (d[v][0] - m.verts[v][0]).hypot(d[v][1] - m.verts[v][1])
        };
        let corner = [[0.0, 0.0], [800.0, 0.0], [0.0, 800.0], [800.0, 800.0]].map(moved).into_iter().fold(0.0, f64::max);
        let side = [[0.0, 400.0], [800.0, 400.0]].map(moved).into_iter().fold(0.0, f64::max);
        let (s, _, _) = creases(&m.verts, &d, &m.tris);
        // Last time: the outline grew 97 % and the corners moved 256 px.
        assert!(grown.abs() < 0.2, "{triangles} triangles: area changed by {grown}");
        assert!(corner < 40.0 && side < 70.0, "{triangles} triangles: corners moved {corner} px, sides {side} px");
        assert!(s > min_s, "{triangles} triangles: smallest singular value {s}");
    }
}

/// The stress test for a hard push: a square of three by three cells, pins at the centres of the left
/// and right columns' cells, the middle left one dragged a fraction of the way to the centre.
fn push_case(triangles: f64, fraction: f64) -> (Mesh, Vec<[f64; 2]>) {
    let n = 900;
    let o = outline(&vec![1.0; n * n], n, n, 1.0, [0.0; 2], [450.0, 450.0], 3.0);
    let m = triangulate(&o, &MeshOpts { triangles, ..MeshOpts::default() });
    let mut pins = vec![];
    for x in [150.0, 750.0] {
        for y in [150.0, 450.0, 750.0] {
            pins.push(pin(PinKind::Position, [x, y], [x, y]));
        }
    }
    pins[1].position = [150.0 + 300.0 * fraction, 450.0];
    let d = solve(&m, &pins, default_rounds());
    (m, d)
}

/// Where a rest point lands, through the triangle over it.
fn landed(m: &Mesh, d: &[[f64; 2]], p: [f64; 2]) -> [f64; 2] {
    let (t, l) = locate(m, p).unwrap();
    [l[0] * d[t[0]][0] + l[1] * d[t[1]][0] + l[2] * d[t[2]][0], l[0] * d[t[0]][1] + l[1] * d[t[1]][1] + l[2] * d[t[2]][1]]
}

#[test]
fn a_hard_push_compresses_what_is_ahead_instead_of_folding() {
    // Before the barrier, at the full drag and however many rounds: a flipped triangle at 350
    // triangles, area ratios down to -0.02, and cell lengths 0.15 0.16 0.47 0.66 0.74 0.81.
    for triangles in [350.0, 2000.0] {
        for fraction in [0.25, 0.5, 0.75, 1.0] {
            let (m, d) = push_case(triangles, fraction);
            let low = m.tris.iter().map(|t| cross(d[t[0]], d[t[1]], d[t[2]]) / cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]])).fold(f64::MAX, f64::min);
            assert!(low > 0.2, "{triangles} triangles at {fraction}: area ratio {low}");
            // No two triangles over one point, on a coarse grid.
            let mut twice = 0;
            for y in (-100..1000).step_by(6) {
                for x in (-100..1000).step_by(6) {
                    let p = [x as f64 + 0.5, y as f64 + 0.5];
                    let over = m
                        .tris
                        .iter()
                        .filter(|t| cross(d[t[0]], d[t[1]], p) > 1e-6 && cross(d[t[1]], d[t[2]], p) > 1e-6 && cross(d[t[2]], d[t[0]], p) > 1e-6)
                        .count();
                    twice += (over > 1) as usize;
                }
            }
            assert_eq!(twice, 0, "{triangles} triangles at {fraction}");
        }
        let (m, d) = push_case(triangles, 1.0);
        let along: Vec<[f64; 2]> = (0..7).map(|k| landed(&m, &d, [150.0 + 100.0 * k as f64, 450.0])).collect();
        let cells: Vec<f64> = along.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]) / 100.0).collect();
        // Spread along the way, least right ahead of the pin, and reaching the far cell.
        assert!(cells[0] > 0.25 && cells[5] < 0.78, "{triangles} triangles: cells {cells:?}");
        assert!(cells.windows(2).all(|w| w[0] < w[1] + 0.02), "{triangles} triangles: cells {cells:?}");
    }
}

#[test]
fn pins_dragged_through_each_other_land_without_hanging_or_blowing_up() {
    let m = traced_solid();
    let (a, b, c) = ([200.0, 360.0], [640.0, 360.0], [1080.0, 360.0]);
    for pins in [
        vec![pin(PinKind::Position, a, c), pin(PinKind::Position, b, b), pin(PinKind::Position, c, a)],
        vec![pin(PinKind::Position, a, b), pin(PinKind::Position, b, b), pin(PinKind::Position, c, b)],
    ] {
        let d = solve(&m, &pins, default_rounds());
        assert!(d.iter().flatten().all(|v| v.is_finite()));
        for p in &pins {
            let at = landed(&m, &d, p.rest);
            assert!((at[0] - p.position[0]).hypot(at[1] - p.position[1]) < 0.5, "{at:?} vs {:?}", p.position);
        }
    }
}

/// Area-weighted mean and 95th percentile of `|log(area ratio)|`.
fn area_change(m: &Mesh, d: &[[f64; 2]]) -> (f64, f64) {
    let mut logs: Vec<(f64, f64)> = m
        .tris
        .iter()
        .map(|t| {
            let rest = cross(m.verts[t[0]], m.verts[t[1]], m.verts[t[2]]);
            ((cross(d[t[0]], d[t[1]], d[t[2]]) / rest).max(0.01).ln().abs(), rest)
        })
        .collect();
    let total: f64 = logs.iter().map(|l| l.1).sum();
    let mean = logs.iter().map(|l| l.0 * l.1).sum::<f64>() / total;
    logs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut acc = 0.0;
    for l in &logs {
        acc += l.1;
        if acc >= 0.95 * total {
            return (mean, l.0);
        }
    }
    (mean, 0.0)
}

#[test]
fn triangles_keep_their_area_in_all_three_stress_tests() {
    // Mean and 95th percentile of |log(area ratio)| at 350 triangles without the area terms:
    // push 0.205, 0.641; cross 0.277, 0.932; s_curve 0.182, 0.414.
    for triangles in [350.0, 2000.0] {
        let (m, d) = push_case(triangles, 1.0);
        let (mean, p95) = area_change(&m, &d);
        assert!(mean < 0.18 && p95 < 0.42, "push at {triangles}: {mean} {p95}");
        let (m, d) = cross_case(triangles);
        let (mean, p95) = area_change(&m, &d);
        assert!(mean < 0.25 && p95 < 0.82, "cross at {triangles}: {mean} {p95}");
        let (m, _, d) = s_curve(triangles);
        let (mean, p95) = area_change(&m, &d);
        assert!(mean < 0.14 && p95 < 0.38, "s_curve at {triangles}: {mean} {p95}");
    }
}

#[test]
fn material_ahead_of_a_push_bulges_sideways() {
    // Without the area terms the column ahead of the pin kept its height (1.07 at most).
    for triangles in [350.0, 2000.0] {
        let (m, d) = push_case(triangles, 1.0);
        let (a, b) = (landed(&m, &d, [350.0, 350.0]), landed(&m, &d, [350.0, 550.0]));
        let tall = (a[0] - b[0]).hypot(a[1] - b[1]) / 200.0;
        assert!(tall > 1.35, "{triangles} triangles: height {tall}");
    }
}
