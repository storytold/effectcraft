//! VectorCraft documents read through the PDF they carry (Create PDF-Compatible File), written
//! with craft-vectorcraft-format's envelope types: plain and compressed, the PDF inline as base64
//! (version 3) and after the JSON (version 4).

use std::io::Write;

use craft_vectorcraft_format::{BLOB_MAGIC, File, INLINE_MAX};

use crate::write::pdf;
use crate::*;

/// Two artboards: a 200×100 page with the layers "Background" (green) and "Art" (a red left
/// half), and a 100×50 blue page. `pad` adds an unused object so the PDF is long enough to go
/// after the JSON.
fn artboards_pdf(pad: bool) -> Vec<u8> {
    let first = "/OC /L0 BDC\n0 1 0 rg 0 0 200 100 re f\nEMC\n/OC /L1 BDC\n1 0 0 rg 0 0 100 100 re f\nEMC\n";
    let mut objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string(), None),
        (2, "<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_string(), None),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Properties << /L0 5 0 R /L1 6 0 R >> >> /Contents 4 0 R >>".to_string(), None),
        (4, "<< >>".to_string(), Some(first.as_bytes().to_vec())),
        (5, "<< /Type /OCG /Name (Background) >>".to_string(), None),
        (6, "<< /Type /OCG /Name (Art) >>".to_string(), None),
        (7, "<< >>".to_string(), Some(b"0 0 1 rg 0 0 100 50 re f\n".to_vec())),
        (8, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 7 0 R >>".to_string(), None),
    ];
    if pad {
        // Written as given (the writer compresses streams without a `/Filter`); never read.
        objs.push((9, "<< /Filter /ASCIIHexDecode >>".to_string(), Some(vec![b'0'; INLINE_MAX + 1024])));
    }
    pdf(&objs, 1)
}

/// A `.vectorcraft` file of `version` holding an empty document and `carried` as its `pdf`
/// member, gzip-compressed when `gzip` is set (Use Compression).
fn vectorcraft(carried: Option<&[u8]>, version: u32, gzip: bool) -> Vec<u8> {
    let mut file = File::new(serde_json::json!({"artboards": [], "layers": []}));
    file.version = version;
    file.generator = Some("VectorCraft 0.1.0".into());
    let mut tail = file.tail();
    file.pdf = carried.map(|b| tail.image("application/pdf", b, false));
    let mut out = serde_json::to_vec(&file).unwrap();
    tail.write_to(&mut out).unwrap();
    if !gzip {
        return out;
    }
    let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::default());
    gz.write_all(&out).unwrap();
    gz.finish().unwrap()
}

fn pixels(doc: &Doc) -> Vec<[f32; 4]> {
    let (w, h) = doc.pixel_size();
    effectcraft_svg::rasterize(doc, w, h, 1.0).data
}

#[test]
fn a_vectorcraft_document_reads_as_the_pdf_it_carries() {
    for (version, gzip, pad) in [(3, false, false), (3, true, false), (4, false, true), (4, true, true)] {
        let carried = artboards_pdf(pad);
        let file = vectorcraft(Some(&carried), version, gzip);
        let case = format!("version {version}, gzip {gzip}");
        if !gzip {
            let after_json = file.windows(BLOB_MAGIC.len()).any(|w| w == BLOB_MAGIC);
            assert_eq!(after_json, version == 4, "{case}: the PDF goes after the JSON in version 4");
        }
        assert_eq!(sniff(&file), Some(Format::VectorCraft), "{case}");
        assert_eq!(codec("art.vectorcraft", &file), Some("VectorCraft"), "{case}");
        assert_eq!(page_count(&file), 2, "{case}");
        let doc = parse(&file).unwrap();
        assert_eq!((doc.width, doc.height), (200.0, 100.0), "{case}");
        assert_eq!(layer_names(&doc), vec!["Background", "Art"], "{case}");
        assert_eq!(pixels(&doc), pixels(&parse(&carried).unwrap()), "{case}: drawn as the PDF itself");
        let second = parse_page(&file, 1).unwrap();
        assert_eq!((second.width, second.height), (100.0, 50.0), "{case}");
        assert_eq!(parse_page(&file, 2).err(), Some(Error::PageOutOfRange(2)), "{case}");
    }
}

#[test]
fn a_vectorcraft_document_without_a_pdf_names_create_pdf_compatible_file() {
    for gzip in [false, true] {
        let file = vectorcraft(None, 4, gzip);
        assert_eq!(sniff(&file), Some(Format::VectorCraft));
        assert_eq!(page_count(&file), 0);
        let e = parse(&file).err();
        assert_eq!(e, Some(Error::NoCarriedPdf));
        assert!(e.map(|e| e.to_string()).unwrap_or_default().contains("Create PDF-Compatible File"));
    }
}

#[test]
fn a_carried_pdf_may_be_as_large_as_a_pdf_file() {
    let limits = carried_limits();
    assert_eq!(limits.max_entry, craft_pdf::MAX_INPUT_BYTES as u64);
    assert_eq!(limits.max_json, craft_doc::Limits::default().max_json);
    assert_eq!(limits.max_total, limits.max_json + 2 * limits.max_entry);
}

#[test]
fn a_vectorcraft_document_read_wrongly_is_an_error() {
    // A newer format version.
    let newer = vectorcraft(Some(&artboards_pdf(false)), craft_vectorcraft_format::VERSION + 1, false);
    let e = parse(&newer).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(e.contains("newer"), "{e}");
    // A `pdf` member that holds something else.
    let not_pdf = vectorcraft(Some(b"not a PDF"), 4, false);
    assert!(matches!(parse(&not_pdf), Err(Error::Read(_))));
    // A cut-off file.
    let file = vectorcraft(Some(&artboards_pdf(true)), 4, true);
    assert!(parse(&file[..file.len() / 2]).is_err());
    // Other JSON is not a VectorCraft document.
    assert_eq!(sniff(br#"{"format":"other","version":1}"#), None);
}
