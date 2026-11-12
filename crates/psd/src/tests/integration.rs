//! The hardening of the Photoshop reader working together: one hostile file (groups nested past
//! the limit, a NaN text transform), a damaged layer section, and a tiny file claiming a huge
//! image. Each ends in a clean result or a clean error, on a 2 MiB stack, never a panic.

use crate::memory_tests::with_merged;
use crate::write::*;
use crate::*;

/// Run `f` on a 2 MiB thread; a panic or stack overflow in it fails the test.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().stack_size(2 << 20).spawn(f).expect("spawn").join().expect("no panic")
}

fn depth_of(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| if let Node::Group { children, .. } = n { 1 + depth_of(children) } else { 0 }).max().unwrap_or(0)
}

#[test]
fn a_hostile_file_with_deep_groups_and_a_nan_text_value_loads_and_decodes() {
    let n = 2 * MAX_GROUP_DEPTH + 36;
    let mut d = WDoc::new(8, 8);
    // File order is bottom first: the section ends open the nest, the group headers close it.
    d.layers = (0..n).map(|_| WLayer::group_end()).collect();
    for i in 0..n {
        d.layers.push(WLayer::solid(&format!("L{i}"), Rect::new(0, 0, 2, 2), [1.0, 0.0, 0.0, 1.0]));
        d.layers.push(WLayer::group(&format!("G{i}"), true, *b"norm"));
    }
    d.layers.push(WLayer::empty("Title", vec![text_block("x", [f64::NAN, f64::INFINITY], "Inter", 12.0, [0.0; 3], 0)]));
    let bytes = write(&d);
    let (depth, flattened, finite, ok) = on_small_stack(move || {
        let p = Psd::parse(bytes).expect("parses");
        let (tree, flattened) = p.tree_report();
        let title = p.layers.iter().find(|l| l.name == "Title").and_then(|l| l.text.as_ref());
        let finite = title.is_some_and(|t| t.transform.iter().all(|v| v.is_finite()));
        // Every layer and the merged image decode to a result or an error, and results are finite.
        let mut ok = p.composite().map_or(true, |px| px.data.iter().flatten().all(|v| v.is_finite()));
        for i in 0..p.layers.len() {
            ok &= p.layer_pixels(i, false).map_or(true, |px| px.data.iter().flatten().all(|v| v.is_finite()));
        }
        (depth_of(&tree), flattened, finite, ok)
    });
    assert!(depth <= MAX_GROUP_DEPTH, "{depth}");
    assert!(flattened > 0);
    assert!(finite, "the NaN transform fell back to the identity");
    assert!(ok);
}

#[test]
fn a_damaged_layer_section_falls_back_to_the_merged_image() {
    let mut d = WDoc::new(30, 20);
    d.layers = vec![
        WLayer::solid("Back", Rect::new(0, 0, 30, 20), [0.0, 0.0, 1.0, 1.0]),
        WLayer::solid("Box", Rect::new(2, 2, 8, 8), [1.0, 0.0, 0.0, 1.0]).with_blend(b"mul "),
    ];
    let mut bytes = write(&d);
    let at = bytes.windows(8).position(|w| w == b"8BIMmul ").expect("blend signature");
    bytes[at] = b'X';
    let (layers, warnings, size, composite_ok) = on_small_stack(move || {
        let p = Psd::parse(bytes).expect("lenient");
        (p.layers.len(), p.warnings.clone(), (p.width, p.height), p.composite().is_ok())
    });
    assert_eq!(layers, 0);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("Layers could not be read"));
    assert_eq!(size, (30, 20));
    assert!(composite_ok);
}

#[test]
fn a_tiny_file_claiming_a_huge_image_is_refused_within_the_budget() {
    let mut small = WDoc::new(2, 2);
    small.rle = false;
    small.layers = vec![WLayer::solid("L", Rect::new(0, 0, 2, 2), [1.0, 0.0, 0.0, 1.0])];
    let base = write(&small);
    let stream = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; 1 << 20], 9);
    for (w, h, comp) in [(16384u32, 16384u32, 2u16), (16384, 16384, 3), (4096, 4096, 2), (4096, 4096, 3), (16384, 16384, 1), (16384, 16384, 0)] {
        let bytes = with_merged(&base, Some((w, h)), None, comp, &stream);
        assert!(bytes.len() < 8192);
        let outcome = on_small_stack(move || {
            let p = Psd::parse(bytes).expect("header and layers parse");
            // A small test budget: one MiB, far under what the claimed image needs.
            let merged = p.composite_with_budget(1 << 20).map(|_| ()).map_err(|e| e.to_string());
            let layer = p.layer_pixels_with_budget(0, true, 1 << 20).map(|_| ()).map_err(|e| e.to_string());
            let default_budget = p.composite().map(|_| ()).map_err(|e| e.to_string());
            (merged, layer, default_budget)
        });
        let (merged, layer, default_budget) = outcome;
        assert!(merged.as_ref().is_err_and(|e| e.contains("memory budget")), "{w}x{h} comp {comp}: {merged:?}");
        assert!(layer.as_ref().is_err_and(|e| e.contains("memory budget")), "{w}x{h} comp {comp}: {layer:?}");
        if w == 16384 {
            assert!(default_budget.as_ref().is_err_and(|e| e.contains("memory budget")), "comp {comp}: {default_budget:?}");
        }
    }
}
