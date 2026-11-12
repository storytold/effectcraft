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

#[test]
fn many_trailer_markers_with_unterminated_objects_do_not_rescan_the_file() {
    // The encryption pre-scan lexes after every `trailer` marker. Unbounded, each unterminated
    // string or dictionary here ran to the end of the file: about 10^11 bytes lexed (minutes),
    // against 2 × 10^8 within the window. The markers sit in an unreferenced stream, so the
    // file still opens as the plain document.
    let junk = "trailer(trailer<<".repeat(25_000) + &" ".repeat(2 << 20);
    let stream = (6, 0, format!("<< /Length {} >>\nstream\n{junk}\nendstream", junk.len()));
    let plain = parse(&document("", &[])).unwrap();
    assert_eq!(parse(&document("", &[stream])), Ok(plain));
}

#[test]
fn a_classic_trailer_with_an_encrypt_entry_is_refused() {
    let plain = String::from_utf8(document("", &[])).unwrap();
    let encrypted = plain.replace("/Root 1 0 R >>", "/Root 1 0 R /Encrypt 9 0 R >>");
    assert_ne!(encrypted, plain);
    // Object 9 does not exist: the trailer pre-scan refuses the file before the shared parser runs.
    assert_eq!(parse(encrypted.as_bytes()), Err(Error::Encrypted));
}

#[test]
fn a_trailer_with_a_long_id_fits_the_window() {
    // Two 128-byte file identifiers leave the trailer dictionary under 600 bytes, well inside the
    // 4 KiB pre-scan window, so /Encrypt after them is read there.
    let plain = String::from_utf8(document("", &[])).unwrap();
    let id = format!("<{}>", "0123456789ABCDEF".repeat(16));
    let with_id = plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R /ID [{id} {id}] >>"));
    let encrypted = plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R /ID [{id} {id}] /Encrypt 9 0 R >>"));
    let doc = parse(plain.as_bytes()).unwrap();
    assert_eq!(parse(with_id.as_bytes()), Ok(doc));
    assert_eq!(parse(encrypted.as_bytes()), Err(Error::Encrypted));
}

#[test]
fn an_encrypt_entry_past_the_trailer_window_is_refused() {
    // The filler pushes /Encrypt past the pre-scan's 4 KiB window. Object 9 does not exist, and the
    // shared parser reads /Encrypt only as a dictionary, so it alone would open the file.
    let plain = String::from_utf8(document("", &[])).unwrap();
    let filler = "x".repeat(8 * 1024);
    let long = plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R /Filler ({filler}) >>"));
    let encrypted = plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R /Filler ({filler}) /Encrypt 9 0 R >>"));
    let doc = parse(plain.as_bytes()).unwrap();
    assert_eq!(parse(long.as_bytes()), Ok(doc));
    assert_eq!(parse(encrypted.as_bytes()), Err(Error::Encrypted));
}

#[test]
fn an_escaped_encrypt_entry_past_the_trailer_window_is_refused() {
    // A name may write any byte as `#` and two hex digits, so these keys all read as /Encrypt and
    // the past-the-window fallback refuses them. The decoys decode to other names and still open.
    let plain = String::from_utf8(document("", &[])).unwrap();
    let doc = parse(plain.as_bytes()).unwrap();
    let filler = "x".repeat(8 * 1024);
    let trailer = |key: &str| plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R /Filler ({filler}) {key} 9 0 R >>"));
    for key in ["/Encr#79pt", "/#45#6E#63rypt", "/E#6e#63rypt"] {
        assert_eq!(parse(trailer(key).as_bytes()), Err(Error::Encrypted), "{key}");
    }
    for decoy in ["/Encrypted", "/Encr#7Apt"] {
        assert_eq!(parse(trailer(decoy).as_bytes()).as_ref(), Ok(&doc), "{decoy}");
    }
}

#[test]
fn a_hash_without_two_hex_digits_stays_in_a_name() {
    // `#` escapes a byte only before two hex digits. Otherwise it and what follows stay as written,
    // so none of these reads as `Encrypt`.
    let name = |token: &str| String::from_utf8(crate::object::name_bytes(token.as_bytes()).collect()).unwrap();
    assert_eq!(name("Encry#pt"), "Encry#pt");
    assert_eq!(name("Encrypt#"), "Encrypt#");
    assert_eq!(name("Encrypt#7"), "Encrypt#7");
}

#[test]
fn an_encrypt_entry_after_a_window_cut_before_the_trailer_dictionary_is_refused() {
    // The 4 KiB pre-scan window ends in the whitespace before the trailer dictionary (the lexer
    // returns nothing) or between the two `<` of its `<<` (the lexer reads a hex string).
    let plain = String::from_utf8(document("", &[])).unwrap();
    let doc = parse(plain.as_bytes()).unwrap();
    for pad in [4 * 1024, 4 * 1024 - 2] {
        let spaced = plain.replace("trailer\n<<", &format!("trailer\n{}<<", " ".repeat(pad)));
        let encrypted = spaced.replace("/Root 1 0 R >>", "/Root 1 0 R /Encrypt 9 0 R >>");
        assert_eq!(parse(spaced.as_bytes()).as_ref(), Ok(&doc));
        assert_eq!(parse(encrypted.as_bytes()), Err(Error::Encrypted));
    }
}

#[test]
fn a_key_cut_by_the_trailer_window_is_read_whole() {
    // Spaces put the 4 KiB pre-scan window's end inside a trailer key, `inside` bytes into it. The
    // `/Encrypt` of the decoy `/Encrypted` is not taken for the key, a real `/Encrypt` cut after
    // `/Encryp` is still refused, and one ending exactly at the window's end is read there.
    let plain = String::from_utf8(document("", &[])).unwrap();
    let doc = parse(plain.as_bytes()).unwrap();
    let window_end = plain.find("trailer").unwrap() + "trailer".len() + 4 * 1024;
    let key_at = plain.find("/Root 1 0 R >>").unwrap() + "/Root 1 0 R ".len();
    let trailer = |key: &str, inside: usize| {
        let file = plain.replace("/Root 1 0 R >>", &format!("/Root 1 0 R {}{key} 9 0 R >>", " ".repeat(window_end - inside - key_at)));
        assert!(file[window_end - inside..].starts_with(key), "{key}");
        file
    };
    assert_eq!(parse(trailer("/Encrypted", 8).as_bytes()).as_ref(), Ok(&doc));
    assert_eq!(parse(trailer("/Encrypt", 7).as_bytes()), Err(Error::Encrypted));
    assert_eq!(parse(trailer("/Encrypt", 8).as_bytes()), Err(Error::Encrypted));
}

#[test]
fn a_trailer_string_in_a_stream_does_not_refuse_a_later_encrypt_name() {
    // A `trailer` marker in an unreferenced stream opens a string or array that runs on past the
    // 4 KiB pre-scan window to an `Encrypt` name. The unbounded lexer read that object, not a
    // trailer dictionary, so the past-the-window fallback does not refuse the file either.
    let plain = parse(&document("", &[])).unwrap();
    for (open, key) in [("(", "/Encrypt"), ("[", "/Encr#79pt")] {
        let content = format!("trailer{open}{}{key} 9 0 R", "text ".repeat(1024));
        let stream = (6, 0, format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        assert_eq!(parse(&document("", &[stream])).as_ref(), Ok(&plain), "{open}");
    }
}
