//! Test files are generated here (no third-party samples).

use super::*;

use crate::write::pdf;

const CONTENT: &str = "/OC /MC0 BDC\n0 1 0 rg 0 0 200 100 re f\nEMC\n\
/OC /MC1 BDC\nq 10 10 80 80 re W n\n1 0 0 rg 0 0 50 100 re f\nQ\n\
/Pattern cs /P0 scn 100 0 100 50 re f\n\
0 0 1 RG 4 w 120 80 m 180 80 l S\n\
BT /F1 12 Tf (ignored) Tj ET\nEMC\n";

pub(crate) fn sample_pdf(object_stream: bool) -> Vec<u8> {
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Properties << /MC0 5 0 R /MC1 6 0 R >> /Pattern << /P0 7 0 R >> >> /Contents 4 0 R >>";
    let pattern = "<< /PatternType 2 /Shading << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [100 0 200 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> /Extend [true true] >> >>";
    let mut objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string(), None),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(), None),
        (3, page.to_string(), None),
        (4, "<< >>".to_string(), Some(CONTENT.as_bytes().to_vec())),
        (7, pattern.to_string(), None),
    ];
    let ocg5 = "<< /Type /OCG /Name (Background) >>";
    let ocg6 = "<< /Type /OCG /Name <FEFF00410072007400> >>"; // "Art" in UTF-16BE
    if object_stream {
        let body = format!("{ocg5} {ocg6}");
        let head = format!("5 0 6 {} ", ocg5.len() + 1);
        let data = format!("{head}{body}");
        objs.push((8, format!("<< /Type /ObjStm /N 2 /First {} >>", head.len()), Some(data.into_bytes())));
    } else {
        objs.push((5, ocg5.to_string(), None));
        objs.push((6, ocg6.to_string(), None));
    }
    pdf(&objs, 1)
}

fn px(doc: &Doc, x: i64, y: i64) -> [f32; 4] {
    let (w, h) = doc.pixel_size();
    let img = effectcraft_svg::rasterize(doc, w, h, 1.0);
    img.get(x, y)
}

#[test]
fn pdf_paths_clip_gradient_and_layers() {
    for objstm in [false, true] {
        let bytes = sample_pdf(objstm);
        assert_eq!(sniff(&bytes), Some(Format::Pdf));
        assert_eq!(page_count(&bytes), 1);
        let doc = parse(&bytes).unwrap();
        assert_eq!((doc.width, doc.height), (200.0, 100.0));
        assert_eq!(layer_names(&doc), vec!["Background", "Art"], "object stream {objstm}, skipped: {:?}", doc.skipped);
        if objstm {
            assert!(doc.skipped.iter().any(|warning| warning.contains("recovered PDF object")), "the malformed xref repair is reported");
        }
        assert!(doc.skipped.contains(&"text (no font)".to_string()), "{:?}", doc.skipped);
        let (w, h) = doc.pixel_size();
        let img = effectcraft_svg::rasterize(&doc, w, h, 1.0);
        let at = |x: i64, y: i64| img.get(x, y);
        // Red rectangle clipped to x ≥ 10, over the green background.
        assert!(at(30, 50)[0] > 0.99 && at(30, 50)[1] < 0.01, "{:?}", at(30, 50));
        assert!(at(5, 50)[1] > 0.99 && at(5, 50)[0] < 0.01, "clipped out: {:?}", at(5, 50));
        assert!(at(70, 50)[1] > 0.99, "{:?}", at(70, 50));
        // Axial shading, red → blue left to right (in the lower half: PDF y is up).
        let (l, r) = (at(110, 75), at(190, 75));
        assert!(l[0] > 0.8 && l[2] < 0.2 && r[2] > 0.8 && r[0] < 0.2, "{l:?} {r:?}");
        assert!(at(150, 25)[1] > 0.99, "the gradient stays in its rectangle: {:?}", at(150, 25));
        // Stroked line (PDF y = 80 → 20 px from the top).
        assert!(at(150, 20)[2] > 0.99 && at(150, 20)[1] < 0.01, "{:?}", at(150, 20));
        // One layer at a time.
        let bg = layer_doc(&doc, 0);
        assert!(px(&bg, 30, 50)[1] > 0.99);
        let art = layer_doc(&doc, 1);
        assert_eq!(px(&art, 5, 50)[3], 0.0);
        // Continuous rasterisation: twice the size stays sharp at the clip edge.
        let big = effectcraft_svg::rasterize(&doc, 400, 200, 2.0);
        assert!(big.get(21, 100)[0] > 0.99 && big.get(18, 100)[1] > 0.99);
    }
}

#[test]
fn pdf_forms_rotation_cmyk_and_alpha() {
    let content = "q 2 0 0 2 0 0 cm /Fm0 Do Q\n/GS0 gs 0 0 0 1 k 0 0 10 10 re f\n";
    let form = "0 1 1 0 k 0 0 20 20 re f";
    let objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".to_string(), None),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 /Rotate 90 >>".to_string(), None),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Resources << /XObject << /Fm0 5 0 R >> /ExtGState << /GS0 << /ca 0.5 >> >> >> /Contents 4 0 R >>"
                .to_string(),
            None,
        ),
        (4, "<< >>".to_string(), Some(content.as_bytes().to_vec())),
        (5, "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] >>".to_string(), Some(form.as_bytes().to_vec())),
    ];
    let doc = parse(&pdf(&objs, 1)).unwrap();
    // Rotated a quarter turn: 50 × 100.
    assert_eq!((doc.width, doc.height), (50.0, 100.0));
    let (w, h) = doc.pixel_size();
    let img = effectcraft_svg::rasterize(&doc, w, h, 1.0);
    // The form (CMYK red, clipped to its 10×10 box, scaled ×2) covers PDF (0..20, 0..20); the
    // half-transparent black square covers PDF (0..10, 0..10). /Rotate 90: PDF (x, y) → (y, x).
    let red = img.get(15, 15);
    assert!(red[0] > 0.99 && red[1] < 0.01 && red[3] > 0.99, "{red:?}");
    let dark = img.get(5, 5);
    assert!((dark[0] - 0.5).abs() < 0.02 && dark[3] > 0.99, "{dark:?}");
    assert_eq!(img.get(30, 30)[3], 0.0, "form clipped to its bounding box");
}

const EPS: &str = "%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 100 100\n%%EndComments\n\
/m {moveto} bind def /l {lineto} bind def\n\
/cm { 6 array astore concat } bind def\n\
/box { 4 dict begin /h exch def /w exch def /y exch def /x exch def x y m w 0 rlineto 0 h rlineto w neg 0 rlineto closepath end } bind def\n\
1 0 0 setrgbcolor\n10 10 30 30 box fill\n\
gsave 0 0 1 setrgbcolor 70 50 20 0 360 arc fill grestore\n\
gsave 1 0 0 1 50 0 cm 0 1 0 setrgbcolor 0 0 10 10 box fill grestore\n\
0 setgray 2 setlinewidth 0 90 m 100 90 l stroke\n\
/Helvetica findfont 12 scalefont setfont 5 5 moveto (text) show\n\
showpage\n%%EOF\n";

#[test]
fn eps_postscript_subset() {
    let doc = parse(EPS.as_bytes()).unwrap();
    assert_eq!((doc.width, doc.height), (100.0, 100.0));
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    assert!(shape_names(&doc).contains(&"Text: text".to_string()), "EPS text is drawn: {:?}", shape_names(&doc));
    let at = |x, y| px(&doc, x, y);
    assert!(at(25, 75)[0] > 0.99 && at(25, 75)[3] > 0.99, "red box {:?}", at(25, 75));
    assert!(at(70, 50)[2] > 0.99, "blue disc {:?}", at(70, 50));
    assert!(at(55, 95)[1] > 0.99, "translated green box {:?}", at(55, 95));
    let line = at(50, 10);
    assert!(line[3] > 0.99 && line[0] < 0.01, "black line {line:?}");
    assert_eq!(at(95, 70)[3], 0.0);
    // DOS EPS binary header (with a fake TIFF preview after the PostScript).
    let ps = EPS.as_bytes();
    let mut dos = vec![0xC5, 0xD0, 0xD3, 0xC6];
    dos.extend_from_slice(&30u32.to_le_bytes());
    dos.extend_from_slice(&(ps.len() as u32).to_le_bytes());
    dos.extend_from_slice(&[0; 18]);
    dos.extend_from_slice(ps);
    dos.extend_from_slice(b"II*\0 not a real preview");
    let d2 = parse(&dos).unwrap();
    assert_eq!(d2.root.children.len(), doc.root.children.len());
    assert_eq!(codec("art.eps", &dos), Some("EPS"));
    assert_eq!(codec("art.ai", &sample_pdf(false)), Some("AI"));
    assert_eq!(parse(b"hello"), Err(Error::NotVector));
}

#[test]
fn filters_decode() {
    assert_eq!(object::ascii85(b"<~87cURD]i,\"Ebo80~>"), b"Hello World!".to_vec());
    let z = miniz_oxide::deflate::compress_to_vec_zlib(b"abc", 6);
    assert_eq!(object::inflate(&z, 1 << 20).unwrap(), b"abc");
}

#[test]
fn flate_bomb_is_bounded() {
    // A decompression bomb in miniature: 16 MiB of zeros deflate to about 16 KB.
    let zeros = vec![0u8; 16 << 20];
    for bomb in [miniz_oxide::deflate::compress_to_vec_zlib(&zeros, 6), miniz_oxide::deflate::compress_to_vec(&zeros, 6)] {
        assert!(bomb.len() < 64 << 10);
        // Past the limit, as zlib and as raw deflate, it is undecodable: not inflated in full
        // and not salvaged as a truncated stream.
        assert_eq!(object::inflate(&bomb, 1 << 20).map(|v| v.len()), None);
        assert_eq!(object::inflate(&bomb, 32 << 20).map(|v| v.len()), Some(16 << 20));
    }
}

#[test]
fn truncated_flate_keeps_what_inflates() {
    let mut x = 1u32;
    let data: Vec<u8> = (0..20_000)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (x >> 16) as u8
        })
        .collect();
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
    let part = object::inflate(&z[..z.len() / 2], 1 << 20).unwrap();
    assert!(!part.is_empty() && part.len() < data.len());
    assert_eq!(part, data[..part.len()]);
}

#[test]
fn salvage_buffer_is_bounded_by_the_limit() {
    // Small inputs keep eight times their size and 1 KiB.
    assert_eq!(object::salvage_len(0, 1 << 20), 1024);
    assert_eq!(object::salvage_len(10_000, 1 << 20), 81_024);
    // Large ones stop at the limit (`1 << 28` fits a 32-bit `usize` too), without overflowing.
    assert_eq!(object::salvage_len(1 << 28, 1 << 20), 1 << 20);
    assert_eq!(object::salvage_len(usize::MAX, 1 << 20), 1 << 20);
}

#[test]
fn nested_bomb_in_the_second_flate_filter_is_refused() {
    // `/Filter [/FlateDecode /FlateDecode]`: the first filter inflates, within the limit, to a zlib
    // bomb (padded to just under the limit) that the second would inflate to 16 MiB. The second
    // stops at the limit, so the stream is undecodable.
    const MAX: usize = 1 << 20;
    let flate = object::Obj::Name("FlateDecode".into());
    let chain: object::Dict = [("Filter".to_string(), object::Obj::Array(vec![flate.clone(), flate]))].into_iter().collect();
    let file = object_file(vec![]);
    let mut nested = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; 16 << 20], 6);
    nested.resize(MAX - 1, 0);
    let raw = miniz_oxide::deflate::compress_to_vec_zlib(&nested, 6);
    assert_eq!(object::inflate(&raw, MAX).map(|v| v.len()), Some(MAX - 1));
    assert_eq!(object::decode_stream_within(&file, &chain, &raw, MAX).map(|v| v.len()), None);
}

fn bounded_filter(dict: &str, raw: &[u8], max: usize) -> Option<Vec<u8>> {
    let object::Obj::Dict(d) = object::Lexer::new(dict.as_bytes(), 0).next().unwrap() else { panic!("not a dictionary") };
    object::decode_stream_within(&object_file(vec![]), &d, raw, max)
}

#[test]
fn predictor_huge_columns_decode_to_nothing() {
    // A row of a petabyte used to be allocated before looking at the data (an allocation
    // failure aborts the process, so this test took the whole test binary down).
    let raw = miniz_oxide::deflate::compress_to_vec_zlib(&[0, 42], 6);
    let dict = "<< /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 1000000000000000 >> >>";
    assert_eq!(bounded_filter(dict, &raw, 16), Some(vec![]));
}

#[test]
fn predictor_overflow_and_empty_rows_decode_to_nothing() {
    let raw = miniz_oxide::deflate::compress_to_vec_zlib(&[0, 42], 6);
    for parms in [
        format!("/Columns {} /Colors 2 /BitsPerComponent 8", usize::MAX),
        format!("/Columns 1 /Colors {} /BitsPerComponent 8", usize::MAX),
        "/Columns 0".into(),
        // A row is two bytes plus its tag byte: longer than the two bytes of data.
        "/Columns 2".into(),
    ] {
        assert_eq!(bounded_filter(&format!("<< /Filter /FlateDecode /DecodeParms << /Predictor 12 {parms} >> >>"), &raw, 16), Some(vec![]));
    }
}

#[test]
fn run_length_chain_is_bounded() {
    // Each stage expands repetitions of 0x81, which is itself an RL repeat code.
    let raw = [0x81, 0x81, 128];
    let chain = "<< /Filter [/RunLengthDecode /RL /RunLengthDecode] >>";
    assert_eq!(bounded_filter(chain, &raw, 128).map(|v| v.len()), None);
    assert_eq!(bounded_filter("<< /Filter /RL >>", &raw, 128), Some(vec![0x81; 128]));
    // An over-limit intermediate stage must fail even if the next stage shrinks it.
    assert_eq!(bounded_filter("<< /Filter [/RL /AHx] >>", &raw, 127), None);
}

fn lzw_codes(codes: impl IntoIterator<Item = u16>, early: bool) -> Vec<u8> {
    let mut w = crate::ccitt::tests::W::default();
    let (mut bits, mut entries, mut prev) = (9, 258, false);
    for code in codes {
        w.put(code, bits);
        match code {
            256 => (bits, entries, prev) = (9, 258, false),
            257 => break,
            _ => {
                if prev && entries < 4096 {
                    entries += 1;
                }
                prev = true;
                if entries + usize::from(early) >= (1 << bits) && bits < 12 {
                    bits += 1;
                }
            }
        }
    }
    w.bytes
}

#[test]
fn lzw_long_run_is_bounded_and_full_table_decodes() {
    // Grow strings of A until the 12-bit table is full, then send literal A codes.
    // Both old and new implementations use well under 100 MB for this fixture.
    for early in [false, true] {
        let raw = lzw_codes([256, 65].into_iter().chain(258..=4095).chain(std::iter::repeat_n(65, 62_500)).chain([257]), early);
        assert!((95_000..105_000).contains(&raw.len()));
        let dict = format!("<< /Filter /LZWDecode /DecodeParms << /EarlyChange {} >> >>", usize::from(early));
        assert_eq!(bounded_filter(&dict, &raw, 1 << 20).map(|v| v.len()), None);
        let expected = 3839 * 3840 / 2 + 62_500;
        assert_eq!(bounded_filter(&dict, &raw, expected), Some(vec![b'A'; expected]));
        // Clear restores the initial dictionary and code width after saturation.
        let cleared = lzw_codes([256, 65].into_iter().chain(258..=4095).chain([256, 66, 257]), early);
        let mut expected = vec![b'A'; 3839 * 3840 / 2];
        expected.push(b'B');
        assert_eq!(bounded_filter(&dict, &cleared, expected.len()), Some(expected));
    }
}

#[test]
fn ccitt_declared_and_unknown_rows_are_bounded() {
    // Eight white Group 4 rows. The old decoder pads declared missing rows to white.
    assert_eq!(bounded_filter("<< /Filter /CCF /DecodeParms << /K -1 /Columns 64 /Rows 1024 >> >>", &[0xFF], 16), None);
    assert_eq!(bounded_filter("<< /Filter /CCF /DecodeParms << /K -1 /Columns 64 >> >>", &[0xFF], 16), None);
    assert_eq!(bounded_filter("<< /Filter /CCF /DecodeParms << /K -1 /Columns 8 /Rows 8 >> >>", &[0xFF], 8), Some(vec![0xFF; 8]));
    // Padding below the limit remains unchanged.
    assert_eq!(bounded_filter("<< /Filter /CCF /DecodeParms << /K -1 /Columns 8 /Rows 16 >> >>", &[0xFF], 16), Some(vec![0xFF; 16]));
    // Without `/Rows` the size is never computed up front: 2^20 rows of 4096 bytes overflow a
    // 32-bit `usize`, but eight rows of this width decode on every target.
    assert_eq!(bounded_filter("<< /Filter /CCF /DecodeParms << /K -1 /Columns 32768 >> >>", &[0xFF], 32768), Some(vec![0xFF; 32768]));
}

#[test]
fn ascii85_and_hex_outputs_are_bounded() {
    assert_eq!(bounded_filter("<< /Filter /A85 >>", b"zz", 7), None);
    assert_eq!(bounded_filter("<< /Filter /A85 >>", b"zz", 8), Some(vec![0; 8]));
    assert_eq!(bounded_filter("<< /Filter /A85 >>", b"87cURD]i,\"Ebo80", 12), Some(b"Hello World!".to_vec()));
    assert_eq!(bounded_filter("<< /Filter /A85 >>", b"87cURD]i,\"Ebo80", 11), None);
    assert_eq!(bounded_filter("<< /Filter /AHx >>", b"4142434>", 3), None);
    assert_eq!(bounded_filter("<< /Filter /AHx >>", b"4142434>", 4), Some(b"ABC@".to_vec()));
}

#[test]
fn predictors_and_normal_lzw_decode_within_limit() {
    for predictor in [1, 2, 12] {
        let (data, expected) = if predictor == 12 { (vec![0, 10, 20, 2, 1, 2], vec![10, 20, 11, 22]) } else { (vec![10, 20], vec![10, 20]) };
        let raw = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
        let dict = format!("<< /Filter /FlateDecode /DecodeParms << /Predictor {predictor} /Columns 2 >> >>");
        assert_eq!(bounded_filter(&dict, &raw, data.len()), Some(expected));
    }
    for early in [false, true] {
        let raw = lzw_codes([256, 65, 66, 258, 257], early);
        assert_eq!(bounded_filter("<< /Filter /LZW >>", &raw, 4), Some(b"ABAB".to_vec()));
        assert_eq!(bounded_filter("<< /Filter /LZW >>", &raw, 3), None);
    }
    assert_eq!(bounded_filter("<< /Filter /RL >>", &[2, b'a', b'b', b'c', 254, b'd', 128], 6), Some(b"abcddd".to_vec()));
    assert_eq!(bounded_filter("<< /Filter /RL >>", &[2, b'a', b'b', b'c', 128], 2), None);
    assert_eq!(bounded_filter("<< /Filter /RL >>", &[128], 0), Some(vec![]));
}

// ------------------------------------------------------------------ M13.6: text, images,
// patterns, soft masks, blend modes, pages

pub(crate) type Objs = Vec<(u32, String, Option<Vec<u8>>)>;

/// A one-page 200×100 PDF: `resources` is the page's resource dictionary body, `extra` more
/// objects (numbered from 10).
pub(crate) fn page_pdf(content: &str, resources: &str, extra: Objs) -> Vec<u8> {
    let mut objs: Objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(), None),
        (3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << {resources} >> /Contents 4 0 R >>"), None),
        (4, "<< >>".into(), Some(content.as_bytes().to_vec())),
    ];
    objs.extend(extra);
    pdf(&objs, 1)
}

pub(crate) fn render(doc: &Doc) -> effectcraft_raster::Image {
    let (w, h) = doc.pixel_size();
    effectcraft_svg::rasterize(doc, w, h, 1.0)
}

pub(crate) fn coverage(img: &effectcraft_raster::Image, x0: i64, y0: i64, x1: i64, y1: i64) -> f32 {
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            s += img.get(x, y)[3];
        }
    }
    s
}

pub(crate) fn shape_names(doc: &Doc) -> Vec<String> {
    fn walk(g: &effectcraft_svg::Group, out: &mut Vec<String>) {
        for c in &g.children {
            match c {
                Node::Group(s) => walk(s, out),
                Node::Shape(s) => out.push(s.name.clone()),
                Node::Image(i) => out.push(i.name.clone()),
            }
        }
    }
    let mut out = vec![];
    walk(&doc.root, &mut out);
    out
}

/// Type 2 charstring operands.
fn t2(v: &[i32]) -> Vec<u8> {
    let mut o = vec![];
    for &x in v {
        if (-107..=107).contains(&x) {
            o.push((x + 139) as u8);
        } else {
            o.push(28);
            o.extend_from_slice(&(x as i16).to_be_bytes());
        }
    }
    o
}

/// A CFF font with a 500-unit square glyph `square` (x 50..550, y 0..500, width 600).
fn square_cff() -> Vec<u8> {
    let mut sq = t2(&[600, 50, 0]);
    sq.push(21);
    sq.extend(t2(&[500, 500, -500]));
    sq.push(6);
    sq.push(14);
    crate::cff::write_test_cff(&[(".notdef", vec![14]), ("square", sq)])
}

const SQUARE_FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Square /FirstChar 65 /LastChar 65 /Widths [600] /Encoding << /Differences [65 /square] >> /FontDescriptor 11 0 R >>";

pub(crate) fn square_font_objs() -> Objs {
    vec![
        (10, SQUARE_FONT.into(), None),
        (11, "<< /Type /FontDescriptor /FontName /Square /Flags 32 /FontFile3 12 0 R >>".into(), None),
        (12, "<< /Subtype /Type1C >>".into(), Some(square_cff())),
    ]
}

#[test]
fn text_with_an_embedded_cff_font_spacing_and_tj() {
    // Squares 10 pt per 100 units: A at 0..60 (square 5..55), Tc 10 and a TJ kern of −400
    // (+40) put the second at 110 (square 115..165).
    let bytes = page_pdf("BT /F1 100 Tf 0 0 Td 10 Tc [(A) -400 (A)] TJ ET", "/Font << /F1 10 0 R >>", square_font_objs());
    let doc = parse(&bytes).unwrap();
    assert!(!doc.skipped.iter().any(|s| s.contains("text")), "{:?}", doc.skipped);
    assert!(shape_names(&doc).iter().any(|n| n.starts_with("Text")), "{:?}", shape_names(&doc));
    let img = render(&doc);
    for (x, on) in [(30, true), (60, false), (105, false), (140, true), (170, false)] {
        assert_eq!(img.get(x, 75)[3] > 0.99, on, "x = {x}: {:?}", img.get(x, 75));
    }
    // Squares are 50 pt tall: rows 50..100.
    assert_eq!(img.get(30, 45)[3], 0.0);
}

#[test]
fn text_render_mode_clip() {
    let bytes = page_pdf("BT /F1 100 Tf 7 Tr 0 0 Td (A) Tj ET 1 0 0 rg 0 0 200 100 re f", "/Font << /F1 10 0 R >>", square_font_objs());
    let img = render(&parse(&bytes).unwrap());
    assert!(img.get(30, 75)[0] > 0.99 && img.get(30, 75)[3] > 0.99);
    assert_eq!(img.get(100, 75)[3], 0.0, "clipped to the glyph");
    assert_eq!(img.get(30, 25)[3], 0.0);
}

#[test]
fn text_with_an_embedded_type1_font() {
    // hsbw 50 600; 0 0 rmoveto; 500 hlineto 500 vlineto -500 hlineto closepath endchar
    let cs = vec![50 + 139, 248, 236, 13, 139, 139, 21, 248, 136, 6, 248, 136, 7, 252, 136, 6, 9, 14];
    let font = crate::type1::write_test_type1(&[(".notdef", vec![139, 139, 13, 14]), ("box", cs)], &[(66, "box")]);
    let extra: Objs = vec![
        (10, "<< /Type /Font /Subtype /Type1 /BaseFont /Box /FirstChar 66 /LastChar 66 /Widths [600] /FontDescriptor 11 0 R >>".into(), None),
        (11, "<< /Type /FontDescriptor /FontName /Box /Flags 4 /FontFile 12 0 R >>".into(), None),
        (12, "<< /Length1 0 /Length2 0 /Length3 0 >>".into(), Some(font)),
    ];
    // The font's built-in encoding maps B (66) to `box`.
    let img = render(&parse(&page_pdf("BT /F1 100 Tf 0 0 Td (BB) Tj ET", "/Font << /F1 10 0 R >>", extra)).unwrap());
    for (x, on) in [(30, true), (60, false), (90, true), (120, false)] {
        assert_eq!(img.get(x, 75)[3] > 0.99, on, "x = {x}");
    }
}

#[test]
fn text_with_a_type3_font() {
    let extra: Objs = vec![
        (
            10,
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 100 100] /FontMatrix [0.01 0 0 0.01 0 0] /CharProcs << /sq 11 0 R >> /Encoding << /Type /Encoding /Differences [65 /sq] >> /FirstChar 65 /LastChar 65 /Widths [100] >>".into(),
            None,
        ),
        (11, "<< >>".into(), Some(b"100 0 d0 0 0 100 100 re f".to_vec())),
    ];
    let img = render(&parse(&page_pdf("BT 0 0 1 rg /F1 20 Tf 10 10 Td (AA) Tj ET", "/Font << /F1 10 0 R >>", extra)).unwrap());
    // 20 pt squares from x = 10, advancing 20: user 10..30 and 30..50, y 10..30 (rows 70..90).
    assert!(img.get(20, 80)[2] > 0.99 && img.get(20, 80)[3] > 0.99, "{:?}", img.get(20, 80));
    assert!(img.get(40, 80)[3] > 0.99);
    assert_eq!(img.get(55, 80)[3], 0.0);
    assert_eq!(img.get(20, 60)[3], 0.0);
}

#[test]
fn text_with_standard_fonts_embedded_truetype_and_cid_fonts() {
    use skrifa::MetadataProvider;
    let inter = effectcraft_text::fonts::INTER_REGULAR.to_vec();
    // Not embedded: Helvetica drawn with the bundled sans serif.
    let std14 = page_pdf("BT /F1 60 Tf 10 30 Td (Hi) Tj ET", "/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >>", vec![]);
    let doc = parse(&std14).unwrap();
    assert!(shape_names(&doc).contains(&"Text: Hi".to_string()), "{:?}", shape_names(&doc));
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let a = render(&doc);
    let ink = coverage(&a, 0, 0, 200, 100);
    assert!(ink > 300.0, "{ink}");
    // Ink only on the text line: baseline at row 70, cap height ≈ 44 px above it.
    assert!(coverage(&a, 0, 72, 200, 100) < 1.0 && coverage(&a, 0, 0, 200, 20) < 1.0);
    // The same face embedded (a TrueType font program, WinAnsiEncoding): the same outlines.
    let tt = page_pdf(
        "BT /F1 60 Tf 10 30 Td (Hi) Tj ET",
        "/Font << /F1 10 0 R >>",
        vec![
            (10, "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Inter /Encoding /WinAnsiEncoding /FontDescriptor 11 0 R >>".into(), None),
            (11, "<< /Type /FontDescriptor /Flags 32 /FontFile2 12 0 R >>".into(), None),
            (12, "<< >>".into(), Some(inter.clone())),
        ],
    );
    let b = render(&parse(&tt).unwrap());
    assert!((coverage(&b, 0, 0, 200, 100) - ink).abs() < 1.0, "{} vs {ink}", coverage(&b, 0, 0, 200, 100));
    // A composite font: Identity-H codes are glyph ids of a CIDFontType2.
    let font = skrifa::FontRef::new(&inter).unwrap();
    let gid = |c: char| font.charmap().map(c).unwrap().to_u32() as u16;
    let (h, i) = (gid('H'), gid('i'));
    let loc = skrifa::instance::LocationRef::default();
    let upem = font.metrics(skrifa::instance::Size::unscaled(), loc).units_per_em as f32;
    let w = |g: u16| font.glyph_metrics(skrifa::instance::Size::unscaled(), loc).advance_width(skrifa::GlyphId::new(g as u32)).unwrap() / upem * 1000.0;
    let cid = page_pdf(
        &format!("BT /F1 60 Tf 10 30 Td <{h:04X}{i:04X}> Tj ET"),
        "/Font << /F1 10 0 R >>",
        vec![
            (10, "<< /Type /Font /Subtype /Type0 /BaseFont /Inter /Encoding /Identity-H /DescendantFonts [13 0 R] >>".into(), None),
            (
                13,
                format!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Inter /CIDToGIDMap /Identity /W [{h} [{}] {i} [{}]] /FontDescriptor 11 0 R >>",
                    w(h),
                    w(i)
                ),
                None,
            ),
            (11, "<< /Type /FontDescriptor /Flags 32 /FontFile2 12 0 R >>".into(), None),
            (12, "<< >>".into(), Some(inter)),
        ],
    );
    let c = render(&parse(&cid).unwrap());
    assert!((coverage(&c, 0, 0, 200, 100) - ink).abs() < 1.0, "{} vs {ink}", coverage(&c, 0, 0, 200, 100));
}

#[test]
fn images_flate_smask_indexed_stencil_inline_and_dct() {
    let objs = |im: &str, data: Vec<u8>, extra: Objs| -> Vec<u8> {
        let mut e: Objs = vec![(10, im.to_string(), Some(data))];
        e.extend(extra);
        page_pdf("q 100 0 0 50 0 0 cm /Im1 Do Q", "/XObject << /Im1 10 0 R >>", e)
    };
    // RGB with a soft mask: red opaque, green transparent.
    let doc = parse(&objs(
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 11 0 R >>",
        vec![255, 0, 0, 0, 255, 0],
        vec![(11, "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 >>".into(), Some(vec![255, 0]))],
    ))
    .unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    assert!(img.get(25, 75)[0] > 0.95 && img.get(25, 75)[3] > 0.95, "{:?}", img.get(25, 75));
    assert!(img.get(75, 75)[3] < 0.05, "{:?}", img.get(75, 75));
    assert_eq!(img.get(25, 25)[3], 0.0, "the image is the unit square of user space");
    // Indexed (palette in a string), 8 bits.
    let img = render(
        &parse(&objs(
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/Indexed /DeviceRGB 1 <0000FFFFFF00>] /BitsPerComponent 8 >>",
            vec![1, 0],
            vec![],
        ))
        .unwrap(),
    );
    let (l, r) = (img.get(25, 75), img.get(75, 75));
    assert!(l[0] > 0.95 && l[1] > 0.95 && l[2] < 0.05 && r[2] > 0.95 && r[0] < 0.05, "{l:?} {r:?}");
    // ICCBased with an /Alternate space, 1-bit samples.
    let img = render(
        &parse(&objs(
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/ICCBased 11 0 R] /BitsPerComponent 1 >>",
            vec![0b0100_0000],
            vec![(11, "<< /N 1 /Alternate /DeviceGray >>".into(), Some(vec![0; 16]))],
        ))
        .unwrap(),
    );
    assert!(img.get(25, 75)[0] < 0.05 && img.get(75, 75)[0] > 0.95);
    // Stencil mask painted in the fill colour (sample 0 paints).
    let stencil = page_pdf(
        "0 1 0 rg q 100 0 0 50 0 0 cm /Im1 Do Q",
        "/XObject << /Im1 10 0 R >>",
        vec![(10, "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ImageMask true >>".into(), Some(vec![0b0100_0000]))],
    );
    let img = render(&parse(&stencil).unwrap());
    assert!(img.get(25, 75)[1] > 0.95 && img.get(25, 75)[3] > 0.95);
    assert!(img.get(75, 75)[3] < 0.05);
    // Inline image (abbreviated keys, ASCIIHex) in the top half.
    let inline = page_pdf("q 100 0 0 50 0 50 cm BI /W 2 /H 1 /CS /RGB /BPC 8 /F /AHx ID 00FF00FF0000> EI Q 0 0 1 rg 150 0 50 50 re f", "", vec![]);
    let doc = parse(&inline).unwrap();
    let img = render(&doc);
    assert!(img.get(25, 25)[1] > 0.95 && img.get(75, 25)[0] > 0.95, "{:?} {:?}", img.get(25, 25), img.get(75, 25));
    assert!(img.get(175, 75)[2] > 0.95, "content after EI still runs");
    // DCT (JPEG): left half red, right half blue.
    let mut jpg = vec![];
    let mut rgb = ::image::RgbImage::new(16, 16);
    for (x, _, p) in rgb.enumerate_pixels_mut() {
        *p = if x < 8 { ::image::Rgb([255, 0, 0]) } else { ::image::Rgb([0, 0, 255]) };
    }
    ::image::DynamicImage::ImageRgb8(rgb).write_to(&mut std::io::Cursor::new(&mut jpg), ::image::ImageFormat::Jpeg).unwrap();
    let doc =
        parse(&objs("<< /Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode >>", jpg, vec![]))
            .unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    let (l, r) = (img.get(20, 75), img.get(80, 75));
    assert!(l[0] > 0.85 && l[2] < 0.15 && r[2] > 0.85 && r[0] < 0.15, "{l:?} {r:?}");
}

#[test]
fn soft_masks_and_blend_modes() {
    // Luminosity: a form that is white on the left half, transparent (→ black backdrop) elsewhere.
    let lum = page_pdf(
        "q /GS1 gs 1 0 0 rg 0 0 200 100 re f Q 0 0 1 rg 0 0 20 20 re f",
        "/ExtGState << /GS1 << /SMask << /S /Luminosity /G 10 0 R >> >> >>",
        vec![(
            10,
            "<< /Type /XObject /Subtype /Form /BBox [0 0 200 100] /Group << /S /Transparency /CS /DeviceGray >> >>".into(),
            Some(b"1 g 0 0 100 100 re f".to_vec()),
        )],
    );
    let doc = parse(&lum).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    assert!(img.get(50, 50)[0] > 0.99 && img.get(50, 50)[3] > 0.99, "{:?}", img.get(50, 50));
    assert!(img.get(150, 50)[3] < 0.01, "{:?}", img.get(150, 50));
    assert!(img.get(10, 90)[2] > 0.99, "the mask ends with Q");
    // Alpha: the mask form's coverage.
    let alpha = page_pdf(
        "/GS1 gs 1 0 0 rg 0 0 200 100 re f",
        "/ExtGState << /GS1 << /SMask << /S /Alpha /G 10 0 R >> >> >>",
        vec![(10, "<< /Type /XObject /Subtype /Form /BBox [0 0 200 100] >>".into(), Some(b"0 0 0 rg 100 0 100 100 re f".to_vec()))],
    );
    let img = render(&parse(&alpha).unwrap());
    assert!(img.get(50, 50)[3] < 0.01 && img.get(150, 50)[0] > 0.99);
    // Multiply over a light blue backdrop; Screen in the right half.
    let blend = page_pdf(
        "0.5 0.5 1 rg 0 0 200 100 re f q /M gs 1 1 0 rg 0 0 100 100 re f Q q /S gs 0.5 0 0 rg 100 0 100 100 re f Q",
        "/ExtGState << /M << /BM /Multiply >> /S << /BM [/Screen] >> >>",
        vec![],
    );
    let img = render(&parse(&blend).unwrap());
    let m = img.get(50, 50);
    assert!((m[0] - 0.5).abs() < 0.01 && (m[1] - 0.5).abs() < 0.01 && m[2] < 0.01, "multiply {m:?}");
    let s = img.get(150, 50);
    assert!((s[0] - 0.75).abs() < 0.01 && (s[1] - 0.5).abs() < 0.01 && (s[2] - 1.0).abs() < 0.01, "screen {s:?}");
}

#[test]
fn tiling_patterns_coloured_and_uncoloured() {
    let cell = b"1 0 0 rg 0 0 10 10 re f 0 0 1 rg 10 10 10 10 re f".to_vec();
    let bytes = page_pdf(
        "/Pattern cs /P1 scn 0 0 100 100 re f /Cs1 cs 0 1 0 /P2 scn 100 0 100 100 re f",
        "/Pattern << /P1 10 0 R /P2 11 0 R >> /ColorSpace << /Cs1 [/Pattern /DeviceRGB] >>",
        vec![
            (10, "<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 /Resources << >> >>".into(), Some(cell)),
            (
                11,
                "<< /Type /Pattern /PatternType 1 /PaintType 2 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 >>".into(),
                Some(b"1 0 0 rg 0 0 10 10 re f".to_vec()),
            ),
        ],
    );
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    let img = render(&doc);
    // PDF (x, y) → pixel (x, 100 − y).
    let at = |x: i64, y: i64| img.get(x, 100 - y);
    assert!(at(5, 5)[0] > 0.99 && at(25, 5)[0] > 0.99 && at(85, 45)[0] > 0.99, "{:?}", at(5, 5));
    assert!(at(15, 15)[2] > 0.99 && at(55, 35)[2] > 0.99);
    assert_eq!(at(15, 5)[3], 0.0);
    // Uncoloured: the cell in the colour given with the pattern (green), its own colour ignored.
    assert!(at(105, 5)[1] > 0.99 && at(105, 5)[0] < 0.01, "{:?}", at(105, 5));
    assert_eq!(at(115, 15)[3], 0.0);
}

/// Build isolated interpreter objects without presenting a function or colour space as a
/// document catalog. Complete-document parsing is covered by the page and reader fixtures.
fn object_file(objs: Objs) -> object::File {
    let objects = objs
        .into_iter()
        .map(|(number, body, stream)| {
            let mut lexer = object::Lexer::new(body.as_bytes(), 0);
            let value = lexer.next().unwrap();
            let value = lexer.maybe_ref(value);
            let value = match stream {
                Some(bytes) => {
                    let object::Obj::Dict(dict) = value else { panic!("stream fixture must have a dictionary") };
                    object::Obj::Stream(dict, bytes)
                }
                None => value,
            };
            ((number, 0), value)
        })
        .collect();
    object::File { objects, trailers: vec![], repairs: vec![] }
}

/// Object 1 of a file holding just `objs`, read as a function.
fn read_function(objs: Objs) -> color::Func {
    color::Func::read(&object_file(objs), &object::Obj::Ref(1, 0))
}

/// A sampled (type 0) function, read from a file holding just its stream.
fn read_sampled_function(dict: &str, data: Vec<u8>) -> color::Func {
    read_function(vec![(1, dict.into(), Some(data))])
}

/// Object 1 of a file holding just `objs`, read as a colour space.
fn read_color_space(objs: Objs) -> color::Cs {
    color::color_space(&object_file(objs), &object::Obj::Ref(1, 0), None)
}

#[test]
fn sampled_function_over_the_sample_cap_is_unsupported() {
    // 4 Mi entries, as many as `/Size` allows; every `/Range` pair adds an output, 4 Mi more samples.
    let dict = |outputs: usize| format!("<< /FunctionType 0 /Domain [0 1] /Size [4194304] /Range [{}] /BitsPerSample 1 >>", "0 1 ".repeat(outputs));
    // Five outputs ask for 20 Mi samples, past the per-table cap (16 Mi); refused by the sample
    // count before any decode, so this is cheap and no lost cap reaches the large table below.
    assert!(matches!(read_sampled_function(&dict(5), vec![]), color::Func::Unsupported));
    // 32 outputs of an empty stream ask for 128 Mi zeros; also past the cap. Through parse() the
    // refused tint transform takes the Separation fallback: tint 0.25 paints grey 0.75, not black.
    let bytes =
        page_pdf("/Tint cs 0.25 scn 0 0 200 100 re f", "/ColorSpace << /Tint [/Separation /Ink /DeviceRGB 10 0 R] >>", vec![(10, dict(32), Some(vec![]))]);
    let doc = parse(&bytes).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    assert_eq!(px(&doc, 50, 50), [0.75, 0.75, 0.75, 1.0]);
}

#[test]
fn sampled_function_one_input_two_outputs() {
    let func =
        read_sampled_function("<< /FunctionType 0 /Domain [0 1] /Size [4] /Range [0 1 -1 1] /BitsPerSample 8 >>", vec![0, 255, 85, 170, 170, 85, 255, 0]);
    assert!(matches!(func, color::Func::Sampled { outputs: 2, .. }));
    // 0.5 → coordinate 1.5, halfway between samples 1 and 2; the second output decodes to [-1, 1].
    assert_eq!(func.eval(&[0.5]), vec![0.5, 0.0]);
    // Inputs outside the domain take the end samples.
    assert_eq!(func.eval(&[-1.0]), vec![0.0, 1.0]);
    assert_eq!(func.eval(&[2.0]), vec![1.0, -1.0]);
}

#[test]
fn sampled_function_two_inputs() {
    let func = read_sampled_function(
        "<< /FunctionType 0 /Domain [0 1 0 1] /Size [3 3] /Range [0 1 0 1] /BitsPerSample 8 >>",
        vec![0, 0, 85, 0, 170, 0, 0, 85, 85, 85, 170, 85, 0, 170, 85, 170, 170, 170],
    );
    assert!(matches!(func, color::Func::SampledN { outputs: 2, .. }));
    // Entry (i, j), the first input varying fastest, holds (i / 3, j / 3); (0.25, 0.75) → (0.5, 1.5).
    assert_eq!(func.eval(&[0.25, 0.75]), vec![1.0 / 6.0, 0.5]);
    assert_eq!(func.eval(&[0.0, 0.0]), vec![0.0, 0.0]);
    assert_eq!(func.eval(&[1.0, 1.0]), vec![2.0 / 3.0, 2.0 / 3.0]);
}

#[test]
fn sampled_functions_have_at_most_32_inputs_and_outputs() {
    // Each evaluation allocates and loops over every output, and shadings evaluate millions of
    // times; no colour space has more than 32 components.
    let outputs = |n: usize| format!("<< /FunctionType 0 /Domain [0 1] /Size [2] /Range [{}] /BitsPerSample 8 >>", "0 1 ".repeat(n));
    assert!(matches!(read_sampled_function(&outputs(32), vec![255; 64]), color::Func::Sampled { outputs: 32, .. }));
    assert_eq!(read_sampled_function(&outputs(33), vec![255; 66]), color::Func::Unsupported);
    let inputs = |m: usize| format!("<< /FunctionType 0 /Domain [{}] /Size [{}] /Range [0 1] /BitsPerSample 8 >>", "0 1 ".repeat(m), "1 ".repeat(m));
    let f = read_sampled_function(&inputs(32), vec![255]);
    assert!(matches!(f, color::Func::SampledN { .. }), "{f:?}");
    assert_eq!(f.eval(&[0.5; 32]), vec![1.0]);
    assert_eq!(read_sampled_function(&inputs(33), vec![255]), color::Func::Unsupported);
}

/// Object 10: a sampled function with one input, one output and samples 0 and 1.
fn ramp() -> (u32, String, Option<Vec<u8>>) {
    (10, "<< /FunctionType 0 /Domain [0 1] /Size [2] /Range [0 1] /BitsPerSample 8 >>".into(), Some(vec![0, 255]))
}

/// A stitching function of `n` equal pieces, each function object 10.
fn stitching_of_10(n: usize) -> String {
    let bounds: Vec<String> = (1..n).map(|i| (i as f64 / n as f64).to_string()).collect();
    format!("<< /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{}] >>", "10 0 R ".repeat(n), bounds.join(" "), "0 1 ".repeat(n))
}

#[test]
fn function_arrays_hold_at_most_32_functions() {
    // An array of one-output functions gives an output per function: 32 at most, as for a single
    // function. (A stitching function's pieces are NOT capped this way; see the 100-piece test.)
    let array = |n: usize| format!("[{}]", "10 0 R ".repeat(n));
    assert!(matches!(read_function(vec![(1, array(32), None), ramp()]), color::Func::Array(fs) if fs.len() == 32));
    assert_eq!(read_function(vec![(1, array(33), None), ramp()]), color::Func::Unsupported);
}

#[test]
fn stitching_function_with_100_pieces_builds_and_evaluates() {
    // A repeating gradient (Cairo, Inkscape) lists one sub-function once per repeat, often far
    // more than 32 times; such a stitching function is valid and must build. Here object 10 (a
    // 0 → 1 ramp) is listed 100 times, read once and shared.
    let f = read_function(vec![(1, stitching_of_10(100), None), ramp()]);
    let color::Func::Stitch { funcs, .. } = &f else { panic!("{f:?}") };
    assert_eq!(funcs.len(), 100);
    let table = |g: &color::Func| match g {
        color::Func::Sampled { samples, .. } => Some(samples.as_ptr()),
        _ => None,
    };
    assert!(table(&funcs[0]).is_some());
    assert!(funcs.iter().all(|g| g == &funcs[0] && table(g) == table(&funcs[0])));
    // The sawtooth's ends: piece 0 at t = 0 and piece 99 at t = 1 both map through the ramp.
    assert_eq!(f.eval1(0.0), vec![0.0]);
    assert_eq!(f.eval1(1.0), vec![1.0]);
}

#[test]
fn a_function_repeated_in_an_array_is_read_once() {
    // Read once per reference, 32 references to one 32 MiB table made 1 GiB. Now the object is
    // read once: the copies are equal and share its table.
    let table = |f: &color::Func| match f {
        color::Func::Sampled { samples, .. } => samples.as_ptr(),
        _ => std::ptr::null(),
    };
    let array = read_function(vec![(1, format!("[{}]", "10 0 R ".repeat(32)), None), ramp()]);
    let stitch = read_function(vec![(1, stitching_of_10(32), None), ramp()]);
    for f in [&array, &stitch] {
        let (color::Func::Array(fs) | color::Func::Stitch { funcs: fs, .. }) = f else { panic!("{f:?}") };
        assert_eq!(fs.len(), 32);
        assert!(!table(&fs[0]).is_null());
        assert!(fs.iter().all(|g| g == &fs[0] && table(g) == table(&fs[0])));
    }
    assert_eq!(array.eval1(0.5), vec![0.5; 32]);
}

#[test]
fn saved_graphics_states_share_a_sampled_tint() {
    // `q` saves a copy of the graphics state, colour spaces included: the copies share the tint
    // transform's table.
    let tint = "<< /FunctionType 0 /Domain [0 1] /Size [4096] /Range [0 1 0 1 0 1] /BitsPerSample 8 >>";
    let data: Vec<u8> = (0..4096u32).flat_map(|i| [(i >> 4) as u8, 0, 255 - (i >> 4) as u8]).collect();
    let cs = read_color_space(vec![(1, "[/Separation /Ink /DeviceRGB 10 0 R]".into(), None), (10, tint.into(), Some(data.clone()))]);
    let table = |cs: &color::Cs| match cs {
        color::Cs::Tint { func: color::Func::Sampled { samples, .. }, .. } => samples.as_ptr(),
        _ => std::ptr::null(),
    };
    assert!(!table(&cs).is_null());
    assert_eq!(table(&cs.clone()), table(&cs));
    // A thousand nested `q` around a fill in that space: tint 1 is the last entry, red.
    let content = format!("/Tint cs 1 scn {}0 0 200 100 re f {}", "q ".repeat(1000), "Q ".repeat(1000));
    let doc = parse(&page_pdf(&content, "/ColorSpace << /Tint [/Separation /Ink /DeviceRGB 10 0 R] >>", vec![(10, tint.into(), Some(data))])).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    assert_eq!(px(&doc, 100, 50), [1.0, 0.0, 0.0, 1.0]);
}

/// A one-page PDF drawing image object 10 over the whole 200 × 100 page; `extra` holds more objects.
fn image_page(dict: &str, data: Vec<u8>, extra: Objs) -> Vec<u8> {
    let mut objs: Objs = vec![(10, dict.into(), Some(data))];
    objs.extend(extra);
    page_pdf("q 200 0 0 100 0 0 cm /Im1 Do Q", "/XObject << /Im1 10 0 R >>", objs)
}

/// Whether two colours match within rendering error.
fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
}

#[test]
fn indexed_space_reads_at_most_256_entries() {
    // hival 10^12 asked the image palette for 10^12 colours (24 TB); an indexed palette holds at
    // most 256 entries, so hival is 0 to 255.
    let im = "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace [/Indexed /DeviceRGB 1000000000000 <00>] /BitsPerComponent 8 >>";
    let doc = parse(&image_page(im, vec![200], vec![])).unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    // Entry 200 is past the one-byte lookup string: black.
    assert!(near(px(&doc, 100, 50), [0.0, 0.0, 0.0, 1.0]), "{:?}", px(&doc, 100, 50));
    let cs = read_color_space(vec![(1, "[/Indexed /DeviceRGB 1000000000000 <00>]".into(), None)]);
    assert!(matches!(cs, color::Cs::Indexed { hival: 255, .. }), "{cs:?}");
}

#[test]
fn indexed_images_with_4_and_256_entries() {
    let im = |cs: &str, w: usize| format!("<< /Type /XObject /Subtype /Image /Width {w} /Height 1 /ColorSpace {cs} /BitsPerComponent 8 >>");
    let doc = parse(&image_page(&im("[/Indexed /DeviceRGB 3 <FF000000FF000000FFFFFFFF>]", 4), vec![0, 1, 2, 3], vec![])).unwrap();
    for (x, c) in [(25, [1.0, 0.0, 0.0, 1.0]), (75, [0.0, 1.0, 0.0, 1.0]), (125, [0.0, 0.0, 1.0, 1.0]), (175, [1.0; 4])] {
        assert!(near(px(&doc, x, 50), c), "x = {x}: {:?}", px(&doc, x, 50));
    }
    // 256 entries, the lookup table in a stream: entry i is (i, 0, 255 − i).
    let lookup: Vec<u8> = (0..=255u8).flat_map(|i| [i, 0, 255 - i]).collect();
    let doc = parse(&image_page(&im("[/Indexed /DeviceRGB 255 11 0 R]", 2), vec![255, 0], vec![(11, "<< >>".into(), Some(lookup))])).unwrap();
    assert!(near(px(&doc, 50, 50), [1.0, 0.0, 0.0, 1.0]), "{:?}", px(&doc, 50, 50));
    assert!(near(px(&doc, 150, 50), [0.0, 0.0, 1.0, 1.0]), "{:?}", px(&doc, 150, 50));
}

/// Functions nested down the first element of arrays and stitching functions, outermost first.
fn nested_functions(f: &color::Func) -> Vec<&color::Func> {
    let mut out = vec![f];
    let mut f = f;
    while let color::Func::Array(fs) | color::Func::Stitch { funcs: fs, .. } = f
        && let Some(first) = fs.first()
    {
        out.push(first);
        f = first;
    }
    out
}

/// Colour spaces nested down Indexed bases and Separation or DeviceN alternates, outermost first.
fn nested_spaces(cs: &color::Cs) -> Vec<&color::Cs> {
    let mut out = vec![cs];
    let mut cs = cs;
    while let color::Cs::Indexed { base: next, .. } | color::Cs::Tint { alt: next, .. } = cs {
        cs = next;
        out.push(cs);
    }
    out
}

#[test]
fn function_array_containing_itself_stops_at_depth_8() {
    // `[1 0 R]` as object 1: the reader recursed until the stack overflowed (an abort).
    let f = read_function(vec![(1, "[1 0 R]".into(), None)]);
    let chain = nested_functions(&f);
    assert_eq!(chain.len(), 9);
    assert!(chain[..8].iter().all(|f| matches!(f, color::Func::Array(_))));
    assert_eq!(chain[8], &color::Func::Unsupported);
    // As a tint transform: the innermost function's 0.5, in DeviceGray.
    let tint = "/ColorSpace << /Tint [/Separation /Ink /DeviceGray 10 0 R] >>";
    let doc = parse(&page_pdf("/Tint cs 1 scn 0 0 200 100 re f", tint, vec![(10, "[10 0 R]".into(), None)])).unwrap();
    assert_eq!(px(&doc, 100, 50), [0.5, 0.5, 0.5, 1.0]);
}

#[test]
fn stitching_function_containing_itself_stops_at_depth_8() {
    let stitch = |me: u32| format!("<< /FunctionType 3 /Domain [0 1] /Functions [{me} 0 R] /Bounds [] /Encode [0 1] >>");
    let f = read_function(vec![(1, stitch(1), None)]);
    let chain = nested_functions(&f);
    assert_eq!(chain.len(), 9);
    assert!(chain[..8].iter().all(|f| matches!(f, color::Func::Stitch { .. })));
    assert_eq!(chain[8], &color::Func::Unsupported);
    let tint = "/ColorSpace << /Tint [/Separation /Ink /DeviceGray 10 0 R] >>";
    let doc = parse(&page_pdf("/Tint cs 1 scn 0 0 200 100 re f", tint, vec![(10, stitch(10), None)])).unwrap();
    assert_eq!(px(&doc, 100, 50), [0.5, 0.5, 0.5, 1.0]);
}

#[test]
fn colour_spaces_based_on_themselves_stop_at_depth_8() {
    // Object 1 as its own Indexed base, Separation alternate or ICCBased alternate: the reader
    // recursed until the stack overflowed (an abort).
    let indexed = read_color_space(vec![(1, "[/Indexed 1 0 R 1 <00>]".into(), None)]);
    let separation = read_color_space(vec![(1, "[/Separation /Ink 1 0 R << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]".into(), None)]);
    for cs in [&indexed, &separation] {
        let chain = nested_spaces(cs);
        assert_eq!(chain.len(), 9);
        assert!(chain[..8].iter().all(|c| matches!(c, color::Cs::Indexed { .. } | color::Cs::Tint { .. })));
        assert_eq!(chain[8], &color::Cs::Gray);
    }
    // An ICCBased space reads as its alternate: DeviceGray once the depth runs out.
    let icc = read_color_space(vec![(1, "[/ICCBased 2 0 R]".into(), None), (2, "<< /N 3 /Alternate 1 0 R >>".into(), Some(vec![0; 16]))]);
    assert_eq!(icc, color::Cs::Gray);
    // A fill in the Indexed one paints black.
    let doc = parse(&page_pdf("/CS0 cs 0 scn 0 0 200 100 re f", "/ColorSpace << /CS0 10 0 R >>", vec![(10, "[/Indexed 10 0 R 1 <00>]".into(), None)])).unwrap();
    assert_eq!(px(&doc, 100, 50), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn nesting_deeper_than_8_is_refused() {
    // Chains of 100 distinct objects, each nesting the next (real files nest two or three
    // levels): read down to depth 7, refused at depth 8.
    let functions = |n: u32| -> Objs {
        let stitch = |next: u32| format!("<< /FunctionType 3 /Domain [0 1] /Functions [{next} 0 R] /Bounds [] /Encode [0 1] >>");
        (1..n).map(|i| (i, stitch(i + 1), None)).chain([(n, "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".into(), None)]).collect()
    };
    let f = read_function(functions(100));
    let chain = nested_functions(&f);
    assert_eq!(chain.len(), 9);
    assert!(chain[..8].iter().all(|f| matches!(f, color::Func::Stitch { .. })));
    assert_eq!(chain[8], &color::Func::Unsupported);
    // Three levels are read to the end.
    let f = read_function(functions(3));
    assert_eq!(nested_functions(&f).len(), 3);
    assert_eq!(f.eval1(0.25), vec![0.25]);
    // Indexed bases.
    let spaces: Objs =
        (1..100).map(|i| (i, format!("[/Indexed {} 0 R 0 <00>]", i + 1), None)).chain([(100, "[/Indexed /DeviceRGB 0 <FF0000>]".into(), None)]).collect();
    let cs = read_color_space(spaces);
    let chain = nested_spaces(&cs);
    assert_eq!(chain.len(), 9);
    assert!(chain[..8].iter().all(|c| matches!(c, color::Cs::Indexed { .. })));
    assert_eq!(chain[8], &color::Cs::Gray);
}

#[test]
fn a_fanning_out_reference_graph_is_refused_within_the_budget() {
    // Eight objects, each an array of 32 references to the next, with a function at the bottom:
    // depth 8 alone would still expand 32^7 copies. Each object is read once per array but its
    // nodes are charged per copy, so the per-read node budget is spent after a few levels and the
    // whole function is refused — fast and with bounded memory, not 32^7 nodes.
    let objs: Objs = (1..=7u32)
        .map(|i| (i, format!("[{}]", format!("{} 0 R ", i + 1).repeat(32)), None))
        .chain([(8, "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>".into(), None)])
        .collect();
    assert_eq!(read_function(objs), color::Func::Unsupported);
}

#[test]
fn colour_space_chain_of_depth_3() {
    // Indexed → Separation → ICCBased → DeviceRGB: entry 0 is tint 1 (blue), entry 1 tint 0 (white).
    let doc = parse(&image_page(
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace [/Indexed 11 0 R 1 <FF00>] /BitsPerComponent 8 >>",
        vec![0, 1],
        vec![
            (11, "[/Separation /Ink 12 0 R 13 0 R]".into(), None),
            (12, "[/ICCBased 14 0 R]".into(), None),
            (13, "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 0 1] /N 1 >>".into(), None),
            (14, "<< /N 3 /Alternate /DeviceRGB >>".into(), Some(vec![0; 16])),
        ],
    ))
    .unwrap();
    assert!(doc.skipped.is_empty(), "{:?}", doc.skipped);
    assert!(near(px(&doc, 50, 50), [0.0, 0.0, 1.0, 1.0]), "{:?}", px(&doc, 50, 50));
    assert!(near(px(&doc, 150, 50), [1.0; 4]), "{:?}", px(&doc, 150, 50));
}

#[test]
fn stitching_function_with_a_reversed_domain() {
    // `f64::clamp` panics on bounds in the wrong order, as `/Domain [1 0]` gives them.
    let exp = "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>";
    let f = read_function(vec![(1, format!("<< /FunctionType 3 /Domain [1 0] /Functions [{exp}] /Bounds [] /Encode [0 1] >>"), None)]);
    assert_eq!(f.eval(&[0.25]), vec![0.0]);
}

#[test]
fn sampled_function_with_a_reversed_domain() {
    let f = read_sampled_function("<< /FunctionType 0 /Domain [1 0] /Size [2] /Range [0 1] /BitsPerSample 8 >>", vec![0, 255]);
    assert_eq!(f.eval(&[0.25]), vec![0.0]);
}

#[test]
fn calculator_function_with_a_reversed_range() {
    let f = read_function(vec![(1, "<< /FunctionType 4 /Domain [0 1] /Range [1 0] >>".into(), Some(b"{ 0.5 add }".to_vec()))]);
    assert_eq!(f.eval(&[0.25]), vec![0.75]);
}

#[test]
fn multiple_pages_and_calculator_shadings() {
    let objs: Objs = vec![
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into(), None),
        (2, "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(), None),
        (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>".into(), None),
        (4, "<< >>".into(), Some(b"1 0 0 rg 0 0 200 100 re f".to_vec())),
        (5, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Resources << /Shading << /Sh 7 0 R >> >> /Contents 6 0 R >>".into(), None),
        (6, "<< >>".into(), Some(b"/Sh sh".to_vec())),
        (7, "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 100 0] /Function 8 0 R >>".into(), None),
        (8, "<< /FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] >>".into(), Some(b"{ 0 0 }".to_vec())),
    ];
    let bytes = pdf(&objs, 1);
    assert_eq!(page_count(&bytes), 2);
    let p1 = parse_page(&bytes, 0).unwrap();
    assert_eq!((p1.width, p1.height), (200.0, 100.0));
    let p2 = parse_page(&bytes, 1).unwrap();
    assert_eq!((p2.width, p2.height), (100.0, 50.0));
    assert!(p2.skipped.is_empty(), "{:?}", p2.skipped);
    let img = render(&p2);
    assert!(img.get(5, 25)[0] < 0.1 && img.get(95, 25)[0] > 0.9, "{:?} {:?}", img.get(5, 25), img.get(95, 25));
    assert_eq!(parse_page(&bytes, 2), Err(Error::PageOutOfRange(2)));
}

#[test]
fn retained_layers_keep_strokes_clips_and_empty_layers() {
    let doc = parse(&sample_pdf(false)).unwrap();
    for i in 0..layer_names(&doc).len() {
        let full = layer_doc(&doc, i);
        let (cropped, offset) = layer_doc_with_sizes(&doc, i, true);
        let (w, h) = cropped.pixel_size();
        let image = effectcraft_svg::rasterize(&cropped, w, h, 1.0);
        let original = effectcraft_svg::rasterize(&full, 200, 100, 1.0);
        for y in 0..100 {
            for x in 0..200 {
                let a = image.get(x - offset[0] as i64, y - offset[1] as i64);
                let b = original.get(x, y);
                assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5), "layer {i} at {x},{y}: {a:?} vs {b:?}");
            }
        }
    }
    let (empty, _) = layer_doc_with_sizes(&doc, usize::MAX, true);
    assert_eq!(empty.pixel_size(), (1, 1));
}

#[test]
fn retained_layers_measure_transformed_artwork_without_rasterizing() {
    let doc = effectcraft_svg::parse(br#"<svg width="120" height="80"><g transform="translate(60,0)"><rect x="10" y="20" width="30" height="10" transform="rotate(90)"/></g><path d="M 10 20 L 30 20" fill="none" stroke="red" stroke-width="4" stroke-linecap="square"/></svg>"#).unwrap();
    let (rotated, offset) = layer_doc_with_sizes(&doc, 0, true);
    assert_eq!(offset, [30.0, 10.0]);
    assert_eq!(rotated.pixel_size(), (10, 30));
    let (stroked, offset) = layer_doc_with_sizes(&doc, 1, true);
    assert_eq!(offset, [8.0, 18.0]);
    assert_eq!(stroked.pixel_size(), (24, 4));
}

#[test]
fn retained_layers_preserve_artwork_beyond_the_bounds_depth_limit() {
    let mut doc = effectcraft_svg::parse(br#"<svg width="120" height="80"><rect x="20" y="15" width="30" height="20"/></svg>"#).unwrap();
    for _ in 0..300 {
        let mut group = Group::new("nested");
        group.children = std::mem::take(&mut doc.root.children);
        doc.root.children.push(Node::Group(group));
    }
    let (cropped, offset) = layer_doc_with_sizes(&doc, 0, true);
    assert_eq!(cropped.pixel_size(), (120, 80), "unknown bounds keep the full page");
    assert_eq!(offset, [0.0, 0.0]);
}

#[test]
fn cid_width_entries_at_the_top_of_the_cid_range_do_not_overflow() {
    // A first CID of u32::MAX (larger values saturate to it) used to overflow `first + k` and `first + 65535`:
    // a panic in debug builds; in release builds the list wrapped round to CID 0 and the range was dropped.
    let load = |entry: &str| {
        let type0 = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [13 0 R] >>";
        let desc = format!("<< /Type /Font /Subtype /CIDFontType2 {entry} >>");
        let file = object::File::parse(&page_pdf("", "/Font << /F1 10 0 R >>", vec![(10, type0.into(), None), (13, desc, None)])).unwrap();
        let d = file.objects.get(&(10, 0)).and_then(object::Obj::dict).unwrap();
        font::Font::load(&file, d, &mut vec![])
    };
    let max = u32::MAX;
    // c [w1 w2 ...]
    let f = load("/W [4294967295 [500 600 700]]");
    assert_eq!((f.width(max), f.width(0)), (0.5, 1.0));
    // cfirst clast w
    assert_eq!(load("/W [4294967295 4294967295 400]").width(max), 0.4);
    // c [w1y vx vy ...]
    let f = load("/W2 [4294967295 [1 2 3 4 5 6]]");
    assert_eq!((f.vmetrics(max), f.vmetrics(0)), ((0.001, 0.002, 0.003), (-1.0, 0.5, 0.88)));
    // cfirst clast w1y vx vy
    assert_eq!(load("/W2 [4294967295 4294967295 1 2 3]").vmetrics(max), (0.001, 0.002, 0.003));
}

#[test]
fn cmap_entries_at_the_top_of_the_code_range_do_not_overflow() {
    // A bfrange array starting at code 0xFFFFFFFF and a cidrange mapping onto CID u32::MAX used to overflow
    // `lo + k` and `c + (code - lo)`: a panic in debug builds, a wrap round to code / CID 0 in release builds.
    let to_unicode = "beginbfrange <FFFFFFFF> <FFFFFFFF> [<0041> <0042>] endbfrange";
    let encoding = "begincodespacerange <0000> <FFFF> endcodespacerange begincidrange <0000> <FFFF> 4294967295 endcidrange";
    let objs: Objs = vec![
        (10, "<< /Type /Font /Subtype /Type0 /Encoding 15 0 R /ToUnicode 14 0 R /DescendantFonts [13 0 R] >>".into(), None),
        (13, "<< /Type /Font /Subtype /CIDFontType2 /W [0 [700 800] 4294967295 [500]] >>".into(), None),
        (14, "<< >>".into(), Some(to_unicode.as_bytes().to_vec())),
        (15, "<< /Type /CMap >>".into(), Some(encoding.as_bytes().to_vec())),
    ];
    let file = object::File::parse(&page_pdf("", "/Font << /F1 10 0 R >>", objs)).unwrap();
    let d = file.objects.get(&(10, 0)).and_then(object::Obj::dict).unwrap();
    let f = font::Font::load(&file, d, &mut vec![]);
    assert_eq!((f.unicode(0xFFFF_FFFF), f.unicode(0)), ("A".to_string(), String::new()));
    // Code 0 is CID u32::MAX; code 2 would be past it, so it falls back to CID 0 like an unmapped code.
    assert_eq!((f.width(0), f.width(2)), (0.5, 0.7));
}
