//! M13.12: function-based and mesh shadings (generated fixtures).

use crate::tests::{Objs, page_pdf, render};
use crate::*;

/// Mesh vertex data: 8-bit flags, 16-bit coordinates (Decode `[0 65535 0 65535 …]`, so raw
/// values are page units) and 8-bit components.
#[derive(Default)]
struct Mesh(Vec<u8>);

impl Mesh {
    fn flag(&mut self, f: u8) -> &mut Self {
        self.0.push(f);
        self
    }
    fn pt(&mut self, x: u16, y: u16) -> &mut Self {
        self.0.extend_from_slice(&x.to_be_bytes());
        self.0.extend_from_slice(&y.to_be_bytes());
        self
    }
    fn c(&mut self, c: &[u8]) -> &mut Self {
        self.0.extend_from_slice(c);
        self
    }
}

const RGB_DECODE: &str = "/Decode [0 65535 0 65535 0 1 0 1 0 1]";

fn close(a: [f32; 4], b: [f32; 3], tol: f32) -> bool {
    a[3] > 0.99 && (0..3).all(|i| (a[i] - b[i]).abs() < tol)
}

fn sh_page(shading: &str, data: Vec<u8>) -> Doc {
    let bytes = page_pdf("/Sh sh", "/Shading << /Sh 10 0 R >>", vec![(10, shading.into(), Some(data))]);
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    doc
}

#[test]
fn free_form_triangle_mesh_with_edge_flags() {
    let mut m = Mesh::default();
    // Triangle (20,20) red, (80,20) green, (20,80) blue; then flag 1: (80,20) (20,80) (80,80) white.
    m.flag(0).pt(20, 20).c(&[255, 0, 0]);
    m.flag(0).pt(80, 20).c(&[0, 255, 0]);
    m.flag(0).pt(20, 80).c(&[0, 0, 255]);
    m.flag(1).pt(80, 80).c(&[255, 255, 255]);
    // Flag 2 off the shared edge (a, c): (80,20) (80,80) → (140,20) black.
    m.flag(2).pt(140, 20).c(&[0, 0, 0]);
    let doc = sh_page(&format!("<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /BitsPerFlag 8 {RGB_DECODE} >>"), m.0);
    let img = render(&doc);
    let at = |x: i64, y: i64| img.get(x, 99 - y);
    assert!(close(at(21, 21), [1.0, 0.0, 0.0], 0.06), "{:?}", at(21, 21));
    // Centroid of the first triangle: the average of its colours.
    assert!(close(at(40, 40), [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0], 0.04), "{:?}", at(40, 40));
    assert!(close(at(79, 79), [1.0, 1.0, 1.0], 0.06), "{:?}", at(79, 79));
    // Third triangle (a = (80,20) green, c = (80,80) white, (140,20) black).
    assert!(close(at(135, 22), [0.0, 0.0, 0.0], 0.1), "{:?}", at(135, 22));
    // Outside the mesh: nothing.
    assert_eq!(at(10, 50)[3], 0.0);
    assert_eq!(at(150, 90)[3], 0.0);
    // The shading is an image node (no Gouraud primitive in the render tree).
    assert!(crate::tests::shape_names(&doc).contains(&"Shading".to_string()));
}

#[test]
fn lattice_mesh_with_a_function() {
    // Two rows of three vertices; t = 0 at the left, 1 at the right; grey ramp function.
    let mut m = Mesh::default();
    for y in [10u16, 90] {
        for (x, t) in [(10u16, 0u8), (100, 128), (190, 255)] {
            m.pt(x, y).c(&[t]);
        }
    }
    let doc = sh_page(
        "<< /ShadingType 5 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /VerticesPerRow 3 /Decode [0 65535 0 65535 0 1] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >>",
        m.0,
    );
    let img = render(&doc);
    let at = |x: i64, y: i64| img.get(x, 99 - y);
    for x in [20i64, 60, 100, 140, 180] {
        let want = (x as f32 + 0.5 - 10.0) / 180.0;
        let p = at(x, 50);
        assert!(p[3] > 0.99 && (p[0] - want).abs() < 0.03 && (p[1] - p[0]).abs() < 0.01, "x {x}: {p:?} want {want}");
    }
    assert_eq!(at(5, 50)[3], 0.0);
}

/// A straight-sided rectangle as the 12 boundary points from `(x0, y0)` up the left side,
/// along the top, down the right side and back along the bottom.
fn rect_loop(x0: u16, y0: u16, x1: u16, y1: u16) -> Vec<(u16, u16)> {
    let lerp = |a: u16, b: u16, k: u16| ((a as u32 * (3 - k as u32) + b as u32 * k as u32) / 3) as u16;
    let mut v = vec![];
    for k in 0..3 {
        v.push((x0, lerp(y0, y1, k)));
    }
    for k in 0..3 {
        v.push((lerp(x0, x1, k), y1));
    }
    for k in 0..3 {
        v.push((x1, lerp(y1, y0, k)));
    }
    for k in 0..3 {
        v.push((lerp(x1, x0, k), y0));
    }
    v
}

fn patch_doc(tensor: bool) -> Doc {
    let mut m = Mesh::default();
    // Patch 1 over x 100..140, y 20..80: corners (100,20) red, (100,80) green, (140,80) blue,
    // (140,20) black.
    m.flag(0);
    let l1 = rect_loop(100, 20, 140, 80);
    for (x, y) in &l1 {
        m.pt(*x, *y);
    }
    if tensor {
        // Interior points p11 p12 p22 p21 at the thirds.
        for (x, y) in [(113, 40), (113, 60), (127, 60), (127, 40)] {
            m.pt(x, y);
        }
    }
    m.c(&[255, 0, 0]).c(&[0, 255, 0]).c(&[0, 0, 255]).c(&[0, 0, 0]);
    // Patch 2 (flag 2): shares patch 1's third side (140,80) → (140,20); then (180,20) white,
    // (180,80) red.
    m.flag(2);
    // The 8 new points: from (140,20) along the bottom to (180,20), up to (180,80), back
    // along the top towards (140,80).
    let new_pts = [(153, 20), (167, 20), (180, 20), (180, 40), (180, 60), (180, 80), (167, 80), (153, 80)];
    for (x, y) in new_pts {
        m.pt(x, y);
    }
    if tensor {
        for (x, y) in [(153, 60), (153, 40), (167, 40), (167, 60)] {
            m.pt(x, y);
        }
    }
    m.c(&[255, 255, 255]).c(&[255, 0, 0]);
    let ty = if tensor { 7 } else { 6 };
    sh_page(&format!("<< /ShadingType {ty} /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /BitsPerFlag 8 {RGB_DECODE} >>"), m.0)
}

#[test]
fn coons_and_tensor_patch_meshes() {
    for tensor in [false, true] {
        let doc = patch_doc(tensor);
        let img = render(&doc);
        let at = |x: i64, y: i64| img.get(x, 99 - y);
        let k = if tensor { "tensor" } else { "coons" };
        assert!(close(at(100, 20), [1.0, 0.0, 0.0], 0.08), "{k} {:?}", at(100, 20));
        assert!(close(at(100, 79), [0.0, 1.0, 0.0], 0.08), "{k} {:?}", at(100, 79));
        assert!(close(at(139, 79), [0.0, 0.0, 1.0], 0.08), "{k} {:?}", at(139, 79));
        // Patch centre: the average of the corners.
        assert!(close(at(120, 50), [0.25, 0.25, 0.25], 0.04), "{k} {:?}", at(120, 50));
        // Patch 2 inherits blue / black on its shared side.
        assert!(close(at(179, 20), [1.0, 1.0, 1.0], 0.08), "{k} {:?}", at(179, 20));
        assert!(close(at(179, 79), [1.0, 0.0, 0.0], 0.08), "{k} {:?}", at(179, 79));
        assert!(close(at(141, 22), [0.0, 0.0, 0.0], 0.1), "{k} {:?}", at(141, 22));
        assert_eq!(at(90, 50)[3], 0.0);
        assert_eq!(at(190, 50)[3], 0.0);
    }
}

#[test]
fn curved_coons_patch_follows_its_boundary() {
    // A patch whose top side bulges up to y = 95 between x 40 and 60.
    let mut m = Mesh::default();
    m.flag(0);
    let pts = [(20, 20), (20, 40), (20, 60), (20, 80), (40, 107), (60, 107), (80, 80), (80, 60), (80, 40), (80, 20), (60, 20), (40, 20)];
    for (x, y) in pts {
        m.pt(x, y);
    }
    m.c(&[255, 0, 0]).c(&[255, 0, 0]).c(&[255, 0, 0]).c(&[255, 0, 0]);
    let doc = sh_page(&format!("<< /ShadingType 6 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /BitsPerFlag 8 {RGB_DECODE} >>"), m.0);
    let img = render(&doc);
    let at = |x: i64, y: i64| img.get(x, 99 - y);
    // The top curve peaks at 80 + 0.75·27 ≈ 100 at x = 50.
    assert!(at(50, 95)[3] > 0.99, "{:?}", at(50, 95));
    assert_eq!(at(22, 95)[3], 0.0);
}

#[test]
fn function_based_shadings_calculator_and_sampled() {
    let bytes = page_pdf(
        "/A sh /B sh",
        "/Shading << /A 10 0 R /B 11 0 R >>",
        vec![
            (10, "<< /ShadingType 1 /ColorSpace /DeviceRGB /Matrix [100 0 0 50 0 0] /Function 12 0 R >>".into(), None),
            (11, "<< /ShadingType 1 /ColorSpace /DeviceRGB /Domain [0 1 0 1] /Matrix [100 0 0 50 100 50] /Function 13 0 R >>".into(), None),
            (12, "<< /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] >>".into(), Some(b"{ 0 }".to_vec())),
            (
                13,
                "<< /FunctionType 0 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Size [2 2] /BitsPerSample 8 >>".into(),
                Some(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]),
            ),
        ],
    );
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    let at = |x: i64, y: i64| img.get(x, 99 - y);
    // Calculator: (x, y) → (x, y, 0).
    let p = at(90, 10);
    assert!(close(p, [0.905, 0.21, 0.0], 0.03), "{p:?}");
    // Only the domain is painted.
    assert_eq!(at(50, 75)[3], 0.0);
    // Sampled 2-in function, bilinear: red (0,0), green (1,0), blue (0,1), white (1,1).
    assert!(close(at(150, 75), [0.5, 0.5, 0.5], 0.03), "{:?}", at(150, 75));
    assert!(close(at(100, 50), [1.0, 0.0, 0.0], 0.04), "{:?}", at(100, 50));
    assert!(close(at(199, 99), [1.0, 1.0, 1.0], 0.04), "{:?}", at(199, 99));
}

#[test]
fn mesh_shading_pattern_fills_a_path() {
    let mut m = Mesh::default();
    m.flag(0).pt(0, 0).c(&[255, 0, 0]);
    m.flag(0).pt(200, 0).c(&[255, 0, 0]);
    m.flag(0).pt(0, 200).c(&[255, 0, 0]);
    let objs: Objs = vec![
        (10, "<< /PatternType 2 /Shading 11 0 R >>".into(), None),
        (11, format!("<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /BitsPerFlag 8 {RGB_DECODE} >>"), Some(m.0)),
    ];
    let bytes = page_pdf("/Pattern cs /P scn 10 10 30 30 re f", "/Pattern << /P 10 0 R >>", objs);
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    let at = |x: i64, y: i64| img.get(x, 99 - y);
    assert!(close(at(20, 20), [1.0, 0.0, 0.0], 0.02), "{:?}", at(20, 20));
    // Clipped to the path.
    assert_eq!(at(50, 20)[3], 0.0);
    assert_eq!(at(5, 5)[3], 0.0);
}

#[test]
fn corrupt_mesh_data_draws_what_parses() {
    // Truncated and garbage vertex data never panic.
    for ty in [4, 5, 6, 7] {
        for data in [vec![], vec![0u8; 3], vec![0xFF; 7], (0..=255u8).collect::<Vec<u8>>(), vec![3u8; 200]] {
            let bytes = page_pdf(
                "/Sh sh /Pattern cs /P scn 0 0 50 50 re f",
                "/Shading << /Sh 10 0 R >> /Pattern << /P << /PatternType 2 /Shading 10 0 R >> >>",
                vec![(
                    10,
                    format!(
                        "<< /ShadingType {ty} /ColorSpace /DeviceRGB /BitsPerCoordinate 16 /BitsPerComponent 8 /BitsPerFlag 8 /VerticesPerRow 2 {RGB_DECODE} >>"
                    ),
                    Some(data),
                )],
            );
            let doc = parse(&bytes).unwrap();
            let _ = render(&doc);
        }
    }
    // Bad layouts are reported.
    let bytes = page_pdf(
        "/Sh sh",
        "/Shading << /Sh 10 0 R >>",
        vec![(10, "<< /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 7 >>".into(), Some(vec![0; 16]))],
    );
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.iter().any(|s| s.contains("layout")), "{:?}", doc.skipped);
}
