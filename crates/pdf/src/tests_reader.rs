//! The shared object adapter preserves scalar references and bounds both tree and graph input.

use crate::{Error, parse};

fn document(extra: &str, tail: &[(u32, u16, String)]) -> Vec<u8> {
    let content = "/GS gs 1 0 0 rg 0 0 20 10 re f";
    let mut objects = vec![
        (1, 0, format!("<< /Type /Catalog /Pages 2 0 R {extra} >>")),
        (2, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (3, 0, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 10] /Resources << /ExtGState << /GS << /ca 5 7 R >> >> >> /Contents 4 0 R >>".into()),
        (4, 0, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())),
        (5, 7, "0.25".into()),
    ];
    objects.extend_from_slice(tail);
    let mut data = b"%PDF-1.7\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();
    for (number, generation, value) in objects {
        offsets.insert(number, (data.len(), generation));
        data.extend_from_slice(format!("{number} {generation} obj\n{value}\nendobj\n").as_bytes());
    }
    let xref = data.len();
    let count = offsets.keys().last().copied().unwrap() + 1;
    data.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
    for number in 1..count {
        let entry = offsets.get(&number).map_or_else(|| "0000000000 65535 f \n".into(), |(offset, generation)| format!("{offset:010} {generation:05} n \n"));
        data.extend_from_slice(entry.as_bytes());
    }
    data.extend_from_slice(format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    data
}

#[test]
fn a_nonzero_generation_scalar_keeps_the_drawing_opacity() {
    let doc = parse(&document("", &[])).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let pixel = effectcraft_svg::rasterize(&doc, 20, 10, 1.0).get(10, 5);
    assert!((pixel[0] - 0.25).abs() < 0.001 && (pixel[3] - 0.25).abs() < 0.001, "{pixel:?}");
}

#[test]
fn a_long_cyclic_reference_graph_does_not_expand_recursively() {
    let objects: Vec<_> = (10..1010).map(|id| (id, 0, format!("<< /Next {} 0 R >>", if id == 1009 { 10 } else { id + 1 }))).collect();
    let plain = parse(&document("", &[])).unwrap();
    let cyclic = parse(&document("/Extra 10 0 R", &objects)).unwrap();
    assert_eq!(plain, cyclic);
}

#[test]
fn excessive_inline_nesting_returns_an_error_instead_of_partial_art() {
    let extra = format!("/Extra {}0{}", "[".repeat(80), "]".repeat(80));
    let bytes = document(&extra, &[]);
    // The shared parser rejects the catalog before the adapter's depth limit runs.
    // Its Invalid result follows the existing NoPages mapping, with no partial document.
    assert_eq!(crate::page_count(&bytes), 0);
    assert_eq!(parse(&bytes), Err(Error::NoPages));
}

fn object_stream(entries: &[(u32, &str)]) -> String {
    let (mut header, mut body) = (String::new(), String::new());
    for (number, value) in entries {
        header.push_str(&format!("{number} {} ", body.len()));
        body.push_str(value);
        body.push(' ');
    }
    let first = header.len();
    let content = header + &body;
    format!("<< /Type /ObjStm /N {} /First {first} /Length {} >>\nstream\n{content}\nendstream", entries.len(), content.len())
}

#[test]
fn duplicate_unreferenced_compressed_objects_do_not_block_repair() {
    let objects = vec![(20, 0, object_stream(&[(10, "<< /Value 1 >>"), (30, "null")])), (21, 0, object_stream(&[(30, "42")]))];
    let doc = parse(&document("/Missing 10 0 R", &objects)).unwrap();
    assert!(doc.skipped.iter().any(|warning| warning.contains("recovered PDF object from an object stream: 10 0")));
    assert!(matches!(parse(&document("/Missing 30 0 R", &objects)), Err(Error::Read(message)) if message.contains("ambiguous recovered object 30")));
}
