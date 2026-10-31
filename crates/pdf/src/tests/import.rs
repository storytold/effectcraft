//! M13.12: CCITT images in PDF pages, EPS text and parser robustness over truncated and
//! corrupt inputs (generated fixtures).

use crate::tests::{coverage, page_pdf, render, shape_names};
use crate::*;

fn eps(body: &[u8]) -> Doc {
    let mut b = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 200 100\n".to_vec();
    b.extend_from_slice(body);
    let doc = parse(&b).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    doc
}

/// The ink columns `(x0, x1)` of a render (alpha > 0.5).
fn ink_x(img: &effectcraft_raster::Image) -> (i64, i64) {
    let (mut x0, mut x1) = (i64::MAX, i64::MIN);
    for y in 0..img.height as i64 {
        for x in 0..img.width as i64 {
            if img.get(x, y)[3] > 0.5 {
                (x0, x1) = (x0.min(x), x1.max(x));
            }
        }
    }
    (x0, x1)
}

/// #955: EPS files that name their operators (`/mo /moveto load def`, as Illustrator's do),
/// query the device and embed metadata read through a SubFileDecode filter draw their artwork.
/// They imported as an empty layer.
#[test]
fn eps_with_aliased_operators_device_queries_and_embedded_data_draws() {
    let square = "1 0 0 setrgbcolor 50 25 mo 150 25 li 150 75 li 50 75 li cp f\n";
    let aliases = "/mo /moveto load def /li /lineto load def /cp /closepath load def /f /fill load def\n";
    let doc = eps(format!("{aliases}{square}").as_bytes());
    assert!(coverage(&render(&doc), 50, 25, 150, 75) > 4900.0, "aliased operators draw the square");
    // Every operator that returns a value pushes it, so the stack stays aligned.
    let queries = "mark currentflat currentscreen currentpagedevice currenthalftone currentblackgeneration \
                   currentundercolorremoval currentcolortransfer currentsystemparams counttomark 13 eq /ok exch def cleartomark\n";
    let doc = eps(format!("{aliases}{queries}ok {{ {square} }} if\n").as_bytes());
    assert!(coverage(&render(&doc), 50, 25, 150, 75) > 4900.0, "device queries leave 13 values");
    // Data up to the end-of-data string isn't run: its unbalanced brace would swallow the rest.
    let data = "currentfile 0 (%END-OF-DATA) /SubFileDecode filter pop\n<x> { ( not PostScript\n%END-OF-DATA\n";
    let doc = eps(format!("{aliases}{data}{square}").as_bytes());
    assert!(coverage(&render(&doc), 50, 25, 150, 75) > 4900.0, "drawing after the embedded data");
}

#[test]
fn eps_filter_on_a_string_source_does_not_skip_the_program() {
    let square = "1 0 0 setrgbcolor 50 25 moveto 150 25 lineto 150 75 lineto 50 75 lineto closepath fill\n";
    let img = "gsave 1 1 8 [1 0 0 1 0 0] (FF) /ASCIIHexDecode filter image grestore\n";
    // Images aren't drawn yet, so this parses without `eps`'s nothing-skipped check.
    let doc = parse(format!("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 200 100\n{img}{square}").as_bytes()).unwrap();
    assert_eq!(doc.skipped, ["image"]);
    assert!(coverage(&render(&doc), 50, 25, 150, 75) > 4900.0, "drawing after an image read from a string");
}

/// A loaded operator runs that operator even after its name is redefined (the prolog idiom
/// `/_m /moveto load def /moveto {… _m} def`), and an executable name bound with `cvx` runs what
/// its name is bound to.
#[test]
fn eps_loaded_operators_and_executable_name_aliases() {
    let saved = "/_m /moveto load def /moveto {_m} def 1 0 0 setrgbcolor 50 25 moveto 150 25 lineto 150 75 lineto 50 75 lineto closepath fill\n";
    assert!(coverage(&render(&eps(saved.as_bytes())), 50, 25, 150, 75) > 4900.0, "a redefined moveto that calls the saved one");
    let alias = "/sq {50 25 moveto 150 25 lineto 150 75 lineto 50 75 lineto closepath fill} def /y /sq cvx def 1 0 0 setrgbcolor y\n";
    assert!(coverage(&render(&eps(alias.as_bytes())), 50, 25, 150, 75) > 4900.0, "an executable name bound to a procedure's name");
}

/// Each `exec` of the executable name `exec` runs the next one: the chain is cut off at the call
/// depth instead of overflowing the stack.
#[test]
fn eps_exec_of_executable_names_stops_at_the_call_depth() {
    let doc = parse(b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 200 100\n/exec cvx 99999 {dup} repeat exec\n").unwrap();
    assert!(doc.skipped.iter().any(|s| s.contains("recursion limit")), "{:?}", doc.skipped);
}

#[test]
fn eps_text_with_bundled_fonts_and_show_variants() {
    // Not embedded: drawn with the bundled stand-in, like the same text in a PDF.
    let doc = eps(b"/Helvetica findfont 40 scalefont setfont 10 30 moveto (Hi) show\n");
    assert_eq!(shape_names(&doc), vec!["Text: Hi"]);
    let pdf = parse(&page_pdf("BT /F1 40 Tf 10 30 Td (Hi) Tj ET", "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>", vec![])).unwrap();
    let (a, b) = (render(&doc), render(&pdf));
    let (ia, ib) = (coverage(&a, 0, 0, 200, 100), coverage(&b, 0, 0, 200, 100));
    assert!(ia > 100.0 && (ia - ib).abs() < 1.0, "eps {ia} vs pdf {ib}");
    assert_eq!(ink_x(&a), ink_x(&b));
    // ashow spreads the letters; widthshow widens spaces only; selectfont; stringwidth centres.
    let plain = ink_x(&render(&eps(b"/Helvetica 30 selectfont 10 30 moveto (HHH) show\n")));
    let spread = ink_x(&render(&eps(b"/Helvetica 30 selectfont 10 30 moveto 10 0 (HHH) ashow\n")));
    assert_eq!(spread.0, plain.0);
    assert!(((spread.1 - plain.1) - 20).abs() <= 1, "{plain:?} {spread:?}");
    let ws = ink_x(&render(&eps(b"/Helvetica 30 selectfont 10 30 moveto 15 0 72 (HHH) widthshow\n")));
    assert!(((ws.1 - plain.1) - 30).abs() <= 1, "{plain:?} {ws:?}");
    let centred = ink_x(&render(&eps(b"/Helvetica 30 selectfont 100 (HHH) stringwidth pop 2 div sub 30 moveto (HHH) show\n")));
    let mid = (centred.0 + centred.1) as f64 / 2.0;
    assert!((mid - 100.0).abs() < 3.0, "{centred:?}");
    // xshow: explicit advances.
    let xs = ink_x(&render(&eps(b"/Helvetica 30 selectfont 10 30 moveto (HH) [50 0] xshow\n")));
    let one = ink_x(&render(&eps(b"/Helvetica 30 selectfont 10 30 moveto (H) show\n")));
    assert!(((xs.1 - one.1) - 50).abs() <= 1, "{one:?} {xs:?}");
    // makefont mirrors; charpath clips; colour applies.
    let doc = eps(b"1 0 0 setrgbcolor /Helvetica findfont [40 0 0 40 0 0] makefont setfont 10 30 moveto (I) false charpath clip 0 0 200 100 rectfill\n");
    let img = render(&doc);
    assert!(coverage(&img, 0, 0, 200, 100) > 50.0);
    assert!(coverage(&img, 60, 0, 200, 100) < 1.0, "only the glyph is painted");
}

#[test]
fn eps_text_with_an_embedded_type1_font_and_reencoding() {
    // hsbw 50 600; 0 0 rmoveto; 500 hlineto 500 vlineto -500 hlineto closepath endchar
    let cs = vec![50 + 139, 248, 236, 13, 139, 139, 21, 248, 136, 6, 248, 136, 7, 252, 136, 6, 9, 14];
    let raw = crate::type1::write_test_type1(&[(".notdef", vec![139, 139, 13, 14]), ("box", cs)], &[(66, "box")]);
    // Give the program a name.
    let mut font = vec![];
    let nl = raw.iter().position(|c| *c == b'\n').unwrap() + 1;
    font.extend_from_slice(&raw[..nl]);
    font.extend_from_slice(b"12 dict begin /FontName /BoxFont def /FontType 1 def\n");
    font.extend_from_slice(&raw[nl..]);
    let mut body = font.clone();
    body.extend_from_slice(b"\n/BoxFont findfont 100 scalefont setfont 0 0 moveto (BB) show\n");
    let doc = eps(&body);
    let img = render(&doc);
    // Boxes 50..550 / 650..1150 font units at size 100: x 5..55 and 65..115, y 0..50.
    for (x, on) in [(30, true), (60, false), (90, true), (120, false)] {
        assert_eq!(img.get(x, 75)[3] > 0.99, on, "x = {x}");
    }
    // Re-encoded copy (`forall` / `definefont`): code 65 → box.
    let mut body = font;
    body.extend_from_slice(
        b"\n/BoxFont findfont dup length dict begin { 1 index /FID ne { def } { pop pop } ifelse } forall \
/Encoding 256 array def 0 1 255 { Encoding exch /.notdef put } for Encoding 65 /box put currentdict end \
/BoxA exch definefont pop /BoxA findfont 100 scalefont setfont 0 0 moveto (AB) show\n",
    );
    let img = render(&eps(&body));
    assert!(img.get(30, 75)[3] > 0.99, "A is the box now");
    assert!(img.get(90, 75)[3] < 0.01, "B is unencoded");
}

/// An image XObject of `img` (black = true) CCITT-encoded with `k`, drawn over the page.
fn ccitt_page(img: &[Vec<bool>], k: i64, mask: bool) -> Doc {
    let (w, h) = (img[0].len(), img.len());
    let data = crate::ccitt::tests::encode(img, k);
    let kind = if mask { "/ImageMask true" } else { "/ColorSpace /DeviceGray /BitsPerComponent 1" };
    let bytes = page_pdf(
        "0 0 1 rg q 200 0 0 100 0 0 cm /Im Do Q",
        "/XObject << /Im 10 0 R >>",
        vec![(
            10,
            format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} {kind} /Filter /CCITTFaxDecode /DecodeParms << /K {k} /Columns {w} /Rows {h} /EndOfLine {} >> >>",
                k >= 0
            ),
            Some(data),
        )],
    );
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    doc
}

#[test]
fn ccitt_images_and_stencil_masks() {
    // Left half black, a white bar on rows 4..6 (20×10 pixels → 10 PDF units per pixel).
    let img: Vec<Vec<bool>> = (0..10).map(|y| (0..20).map(|x| x < 10 && !(4..6).contains(&y)).collect()).collect();
    for k in [-1, 0, 2] {
        let doc = ccitt_page(&img, k, false);
        let r = render(&doc);
        let px = |x: usize, y: usize| r.get(x as i64 * 10 + 5, y as i64 * 10 + 5);
        assert!(px(2, 2)[0] < 0.01 && px(2, 2)[3] > 0.99, "K {k}: black {:?}", px(2, 2));
        assert!(px(15, 2)[0] > 0.99, "K {k}: white {:?}", px(15, 2));
        assert!(px(2, 4)[0] > 0.99, "K {k}: the white bar {:?}", px(2, 4));
        // Stencil mask: black (sample 0) pixels paint the fill colour, the rest is clear.
        let doc = ccitt_page(&img, k, true);
        let r = render(&doc);
        let px = |x: usize, y: usize| r.get(x as i64 * 10 + 5, y as i64 * 10 + 5);
        assert!(px(2, 2)[2] > 0.99 && px(2, 2)[3] > 0.99, "K {k}: mask {:?}", px(2, 2));
        assert_eq!(px(15, 2)[3], 0.0, "K {k}");
    }
}

/// Byte-level mutations of `bytes`: truncations, flipped bytes, deleted and duplicated
/// spans (deterministic).
fn mutations(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let n = bytes.len();
    for k in 1..24 {
        out.push(bytes[..n * k / 24].to_vec());
    }
    let mut s = 0x9E37_79B9u32;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for _ in 0..60 {
        let mut b = bytes.to_vec();
        for _ in 0..1 + rnd() % 6 {
            let i = rnd() as usize % n;
            b[i] = match rnd() % 4 {
                0 => b[i] ^ 0xFF,
                1 => b'0' + (rnd() % 10) as u8,
                2 => b"[]<>()/{}% \n"[(rnd() % 12) as usize],
                _ => rnd() as u8,
            };
        }
        out.push(b);
    }
    for _ in 0..30 {
        let mut b = bytes.to_vec();
        let i = rnd() as usize % n;
        let len = (rnd() as usize % 64).min(n - i);
        if rnd() % 2 == 0 {
            b.drain(i..i + len);
        } else {
            let span = b[i..i + len].to_vec();
            b.splice(i..i, span);
        }
        out.push(b);
    }
    out
}

/// Parse and render every page of every mutation: errors are fine, panics are not.
fn survive(name: &str, bytes: &[u8]) {
    for (i, m) in mutations(bytes).into_iter().enumerate() {
        let r = std::panic::catch_unwind(|| {
            let pages = page_count(&m);
            for p in 0..pages.clamp(1, 3) {
                if let Ok(doc) = parse_page(&m, p) {
                    let (w, h) = doc.pixel_size();
                    let s = 64.0 / (w.max(h).max(1) as f64);
                    let _ = effectcraft_svg::rasterize(&doc, 64, 64, s);
                    let _ = layer_names(&doc);
                }
            }
        });
        assert!(r.is_ok(), "{name}: mutation {i} panicked");
    }
}

#[test]
fn truncated_and_corrupt_pdfs_never_panic() {
    survive("sample", &crate::tests::sample_pdf(false));
    survive("object streams", &crate::tests::sample_pdf(true));
    let mut fonts = crate::tests::square_font_objs();
    fonts.push((13, "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 >>".into(), Some(vec![255; 12])));
    survive(
        "text and images",
        &page_pdf("BT /F1 40 Tf 10 10 Td (AAA) Tj ET q 50 0 0 50 0 0 cm /Im Do Q", "/Font << /F1 10 0 R >> /XObject << /Im 13 0 R >>", fonts),
    );
    let img: Vec<Vec<bool>> = (0..8).map(|y| (0..16).map(|x| (x + y) % 3 == 0).collect()).collect();
    let data = crate::ccitt::tests::encode(&img, -1);
    survive(
        "ccitt",
        &page_pdf(
            "q 100 0 0 50 0 0 cm /Im Do Q",
            "/XObject << /Im 10 0 R >>",
            vec![(
                10,
                "<< /Type /XObject /Subtype /Image /Width 16 /Height 8 /ImageMask true /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 16 >> >>".into(),
                Some(data),
            )],
        ),
    );
}

#[test]
fn hostile_structures_return_errors_or_draw_nothing() {
    // Not a PDF / EPS at all, empty, and headers alone.
    for b in [&b""[..], b"%PDF-", b"%PDF-1.7\n%%EOF", b"%!PS-Adobe-3.0 EPSF-3.0\n", b"\xC5\xD0\xD3\xC6", b"hello"] {
        let _ = parse(b);
        let _ = page_count(b);
    }
    assert_eq!(parse(b"hello"), Err(Error::NotVector));
    assert_eq!(parse(b"%PDF-1.7\n%%EOF"), Err(Error::NoPages));
    // A page tree that refers to itself, a content stream that is a reference loop, deep
    // form recursion and absurd numbers.
    let objs: crate::tests::Objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
        (2, "<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 1 >>".into(), None),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1e300 -1e300] /Contents 5 0 R /Resources << /XObject << /F 4 0 R >> >> >>".into(), None),
        (
            4,
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /F 4 0 R >> >> >>".into(),
            Some(b"/F Do 1e308 1e308 m 1e308 0 l f".to_vec()),
        ),
        (5, "<< >>".into(), Some(b"/F Do q q q q Q Q Q Q Q Q 99999999999 w [1 0 0 1 0 0] cm".to_vec())),
    ];
    let bytes = crate::write::pdf(&objs, 1);
    if let Ok(doc) = parse(&bytes) {
        let _ = effectcraft_svg::rasterize(&doc, 32, 32, 1e-300);
    }
    // Images with absurd sizes and filters.
    let bytes = page_pdf(
        "/A Do /B Do /C Do /D Do",
        "/XObject << /A 10 0 R /B 11 0 R /C 12 0 R /D 13 0 R >>",
        vec![
            (10, "<< /Subtype /Image /Width 100000 /Height 100000 /BitsPerComponent 8 /ColorSpace /DeviceRGB >>".into(), Some(vec![0; 4])),
            (11, "<< /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 7 /ColorSpace /DeviceRGB >>".into(), Some(vec![0; 4])),
            (
                12,
                "<< /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 1 /ImageMask true /Filter /CCITTFaxDecode /DecodeParms << /K -1 /Columns 0 >> >>"
                    .into(),
                Some(vec![0xFF; 4]),
            ),
            (13, "<< /Subtype /Image /Width 4 /Height 4 /BitsPerComponent 8 /ColorSpace [/Indexed /DeviceRGB 300 <00>] >>".into(), Some(vec![9; 16])),
        ],
    );
    let doc = parse(&bytes).unwrap();
    let _ = render(&doc);
    assert!(!doc.skipped.is_empty());
}

#[test]
fn inline_image_with_absurd_dimensions_falls_back_to_ei_scan() {
    // `/W 5000000000000000000` overflows the row math (`* bpc` alone exceeds usize::MAX):
    // `inline_len` must return `None` (scan for `EI`) instead of overflowing; the oversized
    // image itself is then refused by size.
    let bytes = page_pdf("q 10 0 0 10 0 0 cm BI /W 5000000000000000000 /H 100 /CS /G /BPC 8 ID \0 EI Q", "", vec![]);
    let doc = parse(&bytes).unwrap();
    let _ = render(&doc);
    assert!(!doc.skipped.is_empty());
    // A length just under `usize::MAX` passes the row math; past byte 4096 of the content the
    // caller's `start + len` would then overflow.
    let content = format!("{}q 10 0 0 10 0 0 cm BI /W 2305843009213693440 /H 8 /CS /G /BPC 8 ID \0 EI Q", " ".repeat(5000));
    let doc = parse(&page_pdf(&content, "", vec![])).unwrap();
    let _ = render(&doc);
}

#[test]
fn smask_with_absurd_bpc_is_ignored_not_fatal() {
    // `/BitsPerComponent 32` in a soft mask: `1u32 << bpc` must neither panic (debug)
    // nor mistranslate samples into alpha (release); the mask is treated as absent.
    let bytes = page_pdf(
        "q 2 0 0 2 0 0 cm /Im Do Q",
        "/XObject << /Im 10 0 R >>",
        vec![
            (
                10,
                "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 /SMask 11 0 R >>".into(),
                Some(vec![0x80; 4]),
            ),
            (11, "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 32 >>".into(), Some(vec![0xFF; 16])),
        ],
    );
    let doc = parse(&bytes).unwrap();
    let img = render(&doc);
    assert!(coverage(&img, 0, 0, 200, 100) > 0.0, "image still draws, opaque");
}

#[test]
fn truncated_and_corrupt_eps_never_panic() {
    let eps = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\n\
/sq { newpath 0 0 moveto 10 0 lineto 10 10 lineto closepath } bind def\n\
1 0 0 setrgbcolor 0 1 9 { gsave dup 10 mul 0 translate sq fill grestore } for\n\
/Helvetica findfont 12 scalefont setfont 10 50 moveto (Hello) show\n\
[1 2] 0 setdash 0 0 moveto 100 100 lineto stroke\n\
1 1 1 { pop } repeat { exit } loop 0 0 50 0 360 arc closepath eofill\n%%EOF\n";
    survive("eps", eps);
    // Stack underflow, unbalanced procedures, runaway loops and huge repeats.
    for prog in [
        &b"pop pop pop add mul def"[..],
        b"{ { { { {",
        b"} } ] >> )",
        b"{ } loop",
        b"0 1 1e9 { pop } for",
        b"1e9 { 1 } repeat",
        b"/a { a } def a",
        b"(unterminated",
        b"<abc",
        b"100 100 scale 0 0 moveto 1e308 1e308 lineto stroke",
    ] {
        let mut b = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 10 10\n".to_vec();
        b.extend_from_slice(prog);
        let r = std::panic::catch_unwind(|| {
            if let Ok(doc) = parse(&b) {
                let _ = effectcraft_svg::rasterize(&doc, 10, 10, 1.0);
            }
        });
        assert!(r.is_ok(), "{}", String::from_utf8_lossy(prog));
    }
}

/// Parses `body` after the header [`eps`] adds, on a thread of its own, and fails the test when
/// parsing takes more than 20 s.
fn eps_soon(body: &str) -> Doc {
    let mut b = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 200 100\n".to_vec();
    b.extend_from_slice(body.as_bytes());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(parse(&b)).ok());
    rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap_or_else(|_| panic!("parsing {body} did not finish")).unwrap()
}

#[test]
fn arcs_with_huge_angles_end_at_their_second_angle() {
    use kurbo::{Point, Rect, Shape as _};
    // `arc` raises its second angle by multiples of 360 until it is at least the first, and
    // `arcn` lowers it until it is at most the first (PostScript Language Reference §8.2),
    // however large the angles: the arc starts at the first angle's remainder after whole turns
    // and ends at the second angle, the new current point. 10^18 and 10^308 are written in
    // digits: the tokenizer reads no exponents.
    let (e18, e308) = (format!("1{}", "0".repeat(18)), format!("1{}", "0".repeat(308)));
    // `at(deg)` is the point at `deg` degrees on the circle of radius 10 around (50, 50), in
    // document pixels.
    let at = |deg: f64| Point::new(50.0 + 10.0 * deg.to_radians().cos(), 50.0 - 10.0 * deg.to_radians().sin());
    let near = |a: Rect, b: Rect| [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)].iter().all(|(x, y)| (x - y).abs() < 0.05);
    // Each program is listed with the remainders of its first and second angles after whole
    // turns, and whether its sweep is more than 1,024 turns (a skip note is recorded, and the arc
    // still goes around the whole circle).
    for (body, first, second, whole) in [
        (format!("50 50 10 {e18} 0 arc"), 280.0, 0.0, false),
        (format!("50 50 10 {e308} 0 arc"), 296.0, 0.0, false),
        (format!("50 50 10 0 {e18} arcn"), 0.0, 280.0, false),
        (format!("50 50 10 0 {e308} arcn"), 0.0, 296.0, false),
        (format!("50 50 10 0 {e18} arc"), 0.0, 280.0, true),
        (format!("50 50 10 0 {e308} arc"), 0.0, 296.0, true),
        (format!("50 50 10 {e18} 0 arcn"), 280.0, 0.0, true),
        (format!("50 50 10 {e308} 0 arcn"), 296.0, 0.0, true),
    ] {
        let doc = eps_soon(&format!("newpath {body} currentpoint stroke 1 1 rectfill"));
        let skipped: &[&str] = if whole { &["arc (too many turns)"] } else { &[] };
        assert_eq!(doc.skipped, skipped, "{body}");
        let shapes = doc.flatten();
        assert_eq!(shapes.len(), 2, "{body}");
        let bounds = |i: usize| (shapes[i].0 * shapes[i].2.geom.to_path()).bounding_box();
        // An arc within one quarter of the circle is spanned by its endpoints.
        let (a, b) = (at(first), at(second));
        let arc = if whole { Rect::new(40.0, 40.0, 60.0, 60.0) } else { Rect::from_points(a, b) };
        assert!(near(bounds(0), arc), "{body}: {:?}", bounds(0));
        // The second shape is the square drawn at the current point.
        assert!(near(bounds(1), Rect::new(b.x, b.y - 1.0, b.x + 1.0, b.y)), "{body}: {:?}", bounds(1));
    }
    // Sweeps of up to 1,024 turns are drawn whole: five turns counterclockwise and three
    // clockwise wind around the disk twice, and a nonzero fill covers it.
    let doc = eps_soon("50 50 10 0 1800 arc 50 50 10 0 -1080 arcn fill");
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    assert!(img.get(50, 50)[3] > 0.99, "{:?}", img.get(50, 50));
    // A longer sweep keeps the parity of its turns: 5 × 10^18 degrees is an even number of whole
    // turns and 320 degrees, so an even-odd fill covers the center of the disk.
    let doc = eps_soon("50 50 10 0 5000000000000000000 arc eofill");
    assert_eq!(doc.skipped, ["arc (too many turns)"]);
    let img = render(&doc);
    assert!(img.get(50, 50)[3] > 0.99, "{:?}", img.get(50, 50));
}

/// Parses `body` after a square through [`eps_soon`]: the square stays, and the program stops at
/// the operation limit.
fn runs_out_of_budget(body: &str) {
    let doc = eps_soon(&format!("0 0 10 10 rectfill {body}"));
    assert_eq!(doc.flatten().len(), 1, "{body}: {:?}", doc.skipped);
    assert_eq!(doc.skipped, ["PostScript (operation limit)"], "{body}");
}

#[test]
fn paths_made_in_a_loop_run_out_of_the_budget() {
    // Each curve, rectangle element and glyph outline element an operator makes counts against the
    // operation budget: `arc` at a radius of 2.2 × 10^18 makes about 6,800 curves a call,
    // `rectclip` with an array of 2,000 rectangles makes 10,000 elements, and `charpath` makes the
    // outlines of 1,000 H's.
    for body in [
        "{ newpath 0 0 2200000000000000000 0 1440 arc } loop",
        "/a 8000 array def 0 1 7999 { a exch 0 put } for { gsave a rectclip grestore } loop",
        "/Helvetica findfont 10 scalefont setfont /s 1000 string def 0 1 999 { s exch 72 put } for \
         { newpath 0 0 moveto s false charpath } loop",
    ] {
        runs_out_of_budget(body);
    }
}

#[test]
fn paths_read_or_copied_in_a_loop_run_out_of_the_budget() {
    // `pathbbox` reads each element of the current path, and `gsave`, `save` and `clip` copy it:
    // each element counts against the operation budget. The path has 100,001 elements.
    let path = "newpath 0 0 moveto 0 1 99999 { pop 1 0 rlineto } for";
    for body in ["{ pathbbox pop pop pop pop } loop", "{ gsave grestore } loop", "{ save restore } loop", "{ gsave clip grestore } loop"] {
        runs_out_of_budget(&format!("{path} {body}"));
    }
}

#[test]
fn nested_loops_run_out_of_the_budget() {
    // Each pass of `loop`, `repeat`, `for` and `forall` counts against the operation budget, and
    // so does each item of a procedure that is not a name. `clear` empties the operand stack that
    // the passes fill.
    let literals = format!("{{ {} clear }} loop", "0 ".repeat(100_000));
    for body in [
        "{ {} loop } loop",
        "{ 1000000 {} repeat } loop",
        "{ 0 1 999999 {} for clear } loop",
        "/s 1000000 string def { s {} forall clear } loop",
        "/a 1000000 array def { a {} forall clear } loop",
        "/d 1 dict def 0 1 99999 { d exch 0 put } for { d {} forall clear } loop",
        literals.as_str(),
    ] {
        runs_out_of_budget(body);
    }
}

#[test]
fn paths_stop_at_the_path_limit() {
    // A path holds at most 1,048,576 elements. An operator that would add more stops the program:
    // the square drawn before it stays, and the square after it is left out.
    for body in [
        "newpath 0 0 moveto 1100 { 1000 { 1 0 rlineto } repeat } repeat",
        "newpath 0 0 moveto 0 0 0 0 0 0 1100 { 1000 { 6 copy rcurveto } repeat } repeat",
        "newpath 0 0 moveto 1100 { 1000 { closepath } repeat } repeat",
        // 1,024 turns at a radius of 2 × 10^17: about 1,171,000 curves.
        "newpath 0 0 200000000000000000 0 368640 arc",
        // At a radius of 10^300, kurbo divides the arc into more curves than a usize can count.
        "newpath 0 0 10 300 exp 0 90 arc",
        // The outlines of 100,000 H's, 13 elements each.
        "/Helvetica findfont 10 scalefont setfont /s 100000 string def 0 1 99999 { s exch 72 put } for \
         0 0 moveto s false charpath",
        // 210,000 rectangles of five elements each.
        "/a 840000 array def 0 1 839999 { a exch 0 put } for a rectfill",
    ] {
        let doc = eps_soon(&format!("0 0 10 10 rectfill {body} 20 20 10 10 rectfill"));
        assert_eq!(doc.flatten().len(), 1, "{body}: {:?}", doc.skipped);
        assert_eq!(doc.skipped, ["PostScript (path limit)"], "{body}");
    }
    // A `moveto` replaces a `moveto` just before it (PostScript Language Reference §8.2, `moveto`),
    // and so does the current point that `show` sets, so 550,000 pieces of text placed with
    // `moveto` and `show` leave a path of one element.
    let doc = eps_soon("/Helvetica findfont 10 scalefont setfont 550 { 1000 { 0 0 moveto ( ) show } repeat } repeat 0 0 10 10 rectfill");
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    assert_eq!(doc.flatten().len(), 1);
}

#[test]
fn pdf_paths_stop_at_the_path_limit() {
    // A content stream's current path holds at most 1,048,576 elements: `re` (five elements each)
    // and `h` stop the stream at the limit, and the square painted before it stays.
    for (op, n) in [("0 0 1 1 re ", 210_000), ("h ", 1_100_000)] {
        let doc = parse(&page_pdf(&format!("0 0 10 10 re f 0 0 m {} f 20 20 10 10 re f", op.repeat(n)), "", vec![])).unwrap();
        assert_eq!(doc.flatten().len(), 1, "{op}: {:?}", doc.skipped);
        assert_eq!(doc.skipped, ["content (path limit)"], "{op}");
    }
}

#[test]
fn closing_an_empty_path_is_a_no_op() {
    // "s", "b" and "b*" closed the path without a current point (kurbo asserts on that).
    for ops in ["s", "b", "b*", "0 0 m 50 0 l 50 50 l b s b*", "f s 10 10 m 20 20 l s"] {
        let doc = parse(&page_pdf(ops, "", vec![])).unwrap();
        let _ = render(&doc);
    }
}

#[test]
fn postscript_radix_numbers_outside_2_to_36_are_not_numbers() {
    // `0#7` asked `i64::from_str_radix` for radix 0, which panics (found by fuzzing).
    for n in ["0#7", "1#1", "37#1", "99999999999#1", "16#FF"] {
        let src = format!("%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\n{n} pop 0 0 moveto 10 10 lineto stroke\n");
        let _ = parse(src.as_bytes());
    }
}

#[test]
fn extreme_page_indices_have_a_printable_error() {
    let text = Error::PageOutOfRange(usize::MAX).to_string();
    assert!(text.starts_with("the PDF has no page "));
}

#[test]
fn postscript_idiv_and_mod_of_i64_min_by_minus_one_give_zero() {
    // `i64::MIN / -1` and `i64::MIN % -1` overflow, and `/` and `%` panic on them in debug and
    // release builds. `as i64` turns any dividend at or below -2^63 into `i64::MIN` and turns
    // a divisor of -1.5 into -1.
    for (prog, want) in [
        ("-9223372036854775808 -1 idiv", 0),
        ("-9223372036854775808 -1 mod", 0),
        ("-99999999999999999999 -1.5 idiv", 0),
        ("2 63 exp neg -1 mod", 0),
        // The result is 0 for a divisor that truncates to 0. Other quotients are truncated
        // toward zero, and a remainder has the sign of the dividend.
        ("7 0 idiv", 0),
        ("7 0.5 mod", 0),
        ("-7 2 idiv", -3),
        ("-7 2 mod", -1),
    ] {
        let doc = eps(format!("{prog} {want} eq {{ 0 0 10 10 rectfill }} if\n").as_bytes());
        assert_eq!(shape_names(&doc).len(), 1, "{prog}");
    }
}
