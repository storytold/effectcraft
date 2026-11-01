//! Hand-built containers for odd but well-formed shapes the reader must keep, and the limits against hostile
//! input. Every successfully parsed input must write back byte for byte.

use effectcraft_aep::{AepError, Body, Chunk, Limits, parse, parse_with_limits, write};

/// One chunk with its pad byte (`pad` is used when the body is odd).
fn chunk(id: &[u8; 4], body: &[u8], pad: u8) -> Vec<u8> {
    let mut v = id.to_vec();
    v.extend_from_slice(&(body.len() as u32).to_be_bytes());
    v.extend_from_slice(body);
    if body.len() % 2 == 1 {
        v.push(pad);
    }
    v
}

fn list(list_type: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
    let mut body = list_type.to_vec();
    for c in children {
        body.extend_from_slice(c);
    }
    chunk(b"LIST", &body, 0)
}

/// A whole file: `RIFX`, size, `Egg!`, chunks, trailer.
fn file(chunks: &[Vec<u8>], trailer: &[u8]) -> Vec<u8> {
    let mut body = b"Egg!".to_vec();
    for c in chunks {
        body.extend_from_slice(c);
    }
    let mut v = b"RIFX".to_vec();
    v.extend_from_slice(&(body.len() as u32).to_be_bytes());
    v.extend_from_slice(&body);
    v.extend_from_slice(trailer);
    v
}

fn assert_round_trip(bytes: &[u8]) -> effectcraft_aep::AepFile<'_> {
    let f = parse(bytes).unwrap();
    assert!(write(&f).unwrap() == bytes, "round trip differs");
    f
}

#[test]
fn minimal_file_and_empty_container() {
    let bytes = file(&[], b"");
    let f = assert_round_trip(&bytes);
    assert!(f.chunks.is_empty() && f.trailer.is_empty());
}

#[test]
fn empty_input_and_bad_headers_are_errors() {
    assert_eq!(parse(b""), Err(AepError::NotRifx));
    assert_eq!(parse(b"RIFX"), Err(AepError::NotRifx));
    assert_eq!(parse(b"RIFF\0\0\0\x04Egg!"), Err(AepError::NotRifx));
    assert_eq!(parse(b"RIFX\0\0\0\x04Evil"), Err(AepError::BadFormType(*b"Evil")));
    assert!(matches!(parse(b"RIFX\0\0\0\x03Egg!"), Err(AepError::Truncated { .. })));
    // Declared size past the end of the input.
    assert!(matches!(parse(b"RIFX\0\0\0\x20Egg!"), Err(AepError::Truncated { declared: 32, available: 4 })));
    assert!(matches!(parse(b"RIFX\xff\xff\xff\xffEgg!"), Err(AepError::Truncated { .. })));
}

#[test]
fn ids_ending_in_a_space_are_kept() {
    let bytes = file(&[chunk(b"seq ", &[1, 2], 0), chunk(b"Pin ", &[], 0), chunk(b"FEE ", &[9, 9, 9, 9], 0), chunk(b"LOm ", &[1], 0)], b"");
    let f = assert_round_trip(&bytes);
    let ids: Vec<&[u8; 4]> = f.chunks.iter().map(|c| &c.id).collect();
    assert_eq!(ids, [b"seq ", b"Pin ", b"FEE ", b"LOm "]);
    assert!(f.chunks[0].is(b"seq "));
}

#[test]
fn odd_sized_chunks_keep_their_pad_byte_even_when_not_zero() {
    let bytes = file(&[chunk(b"abcd", &[1, 2, 3], 0xAA), chunk(b"efgh", &[4], 0)], b"");
    let f = assert_round_trip(&bytes);
    assert_eq!(f.chunks[0].pad, Some(0xAA));
    assert_eq!(f.chunks[1].pad, Some(0));
    assert_eq!(f.chunks[0].bytes(), Some(&[1, 2, 3][..]));
}

#[test]
fn odd_last_chunk_without_pad_byte() {
    // The container ends right after an odd body: no pad byte exists in the file.
    let mut body = b"Egg!".to_vec();
    body.extend_from_slice(b"abcd\0\0\0\x03xyz");
    let mut bytes = b"RIFX".to_vec();
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    let f = assert_round_trip(&bytes);
    assert_eq!(f.chunks.len(), 1);
    assert_eq!(f.chunks[0].pad, None);
    // The same with a trailer following.
    bytes.extend_from_slice(b"<?xpacket begin?>");
    let f = assert_round_trip(&bytes);
    assert_eq!(f.chunks[0].pad, None);
    assert_eq!(&*f.trailer, b"<?xpacket begin?>");
}

#[test]
fn list_with_raw_text_body_is_opaque() {
    // Like LIST btdk: the body after the list type is text, not chunks.
    let text = b"{\"a\": 1, \"b\": [1, 2, 3]} not a chunk sequence at all";
    let bytes = file(&[chunk(b"LIST", &[b"btdk".as_slice(), text].concat(), 0), chunk(b"next", &[], 0)], b"");
    let f = assert_round_trip(&bytes);
    assert!(matches!(f.chunks[0].body, Body::OpaqueList { .. }));
    assert_eq!(f.chunks[0].list_type(), Some(*b"btdk"));
    assert_eq!(f.chunks[0].bytes(), Some(&text[..]));
    assert!(f.chunks[1].is(b"next"));
}

#[test]
fn list_with_a_child_larger_than_the_list_is_opaque_not_an_error() {
    let mut bad_child = b"kid1".to_vec();
    bad_child.extend_from_slice(&1000u32.to_be_bytes());
    bad_child.extend_from_slice(&[0; 4]);
    let bytes = file(&[chunk(b"LIST", &[b"lsta".as_slice(), &bad_child].concat(), 0), chunk(b"tail", &[], 0)], b"");
    let f = assert_round_trip(&bytes);
    assert!(matches!(f.chunks[0].body, Body::OpaqueList { .. }));
    assert!(f.chunks[1].is(b"tail"));
}

#[test]
fn list_with_trailing_bytes_shorter_than_a_header_is_opaque() {
    let bytes = file(&[chunk(b"LIST", &[b"lsta".as_slice(), &chunk(b"kid1", &[], 0), &[1, 2, 3]].concat(), 0)], b"");
    let f = assert_round_trip(&bytes);
    assert!(matches!(f.chunks[0].body, Body::OpaqueList { .. }));
}

#[test]
fn empty_list_and_list_shorter_than_its_type() {
    let bytes = file(&[list(b"Pefl", &[]), chunk(b"LIST", &[1, 2], 0), chunk(b"LIST", &[], 0)], b"");
    let f = assert_round_trip(&bytes);
    assert!(matches!(&f.chunks[0].body, Body::List { children, .. } if children.is_empty()));
    // A LIST chunk too short to hold a type stays plain data.
    assert!(matches!(f.chunks[1].body, Body::Data(_)));
    assert!(matches!(f.chunks[2].body, Body::Data(_)));
}

#[test]
fn nested_ids_hold_a_utf8_chunk() {
    let name = chunk(b"Utf8", b"Hello", 0x7f);
    let empty = chunk(b"Utf8", b"", 0);
    let bytes = file(&[chunk(b"tdsn", &name, 0), chunk(b"pdnm", &empty, 0), chunk(b"fnam", &name, 0), chunk(b"othr", &name, 0)], b"");
    let f = assert_round_trip(&bytes);
    for c in &f.chunks[..3] {
        assert!(matches!(c.body, Body::Chunks(_)), "{:?}", c.id);
        assert_eq!(c.children().len(), 1);
        let expect: &[u8] = if c.id == *b"pdnm" { b"" } else { b"Hello" };
        assert_eq!(c.children()[0].bytes(), Some(expect));
    }
    assert_eq!(f.chunks[0].children()[0].pad, Some(0x7f));
    // An id outside the nested set is plain data even though its body looks like a chunk.
    assert!(matches!(f.chunks[3].body, Body::Data(_)));
}

#[test]
fn nested_id_with_a_body_that_is_not_a_chunk_stays_data() {
    let bytes = file(&[chunk(b"tdsn", b"not a chunk sequence!", 0), chunk(b"tdsn", &[0; 3], 0), chunk(b"tdsn", &[], 0)], b"");
    let f = assert_round_trip(&bytes);
    assert!(f.chunks.iter().all(|c| matches!(c.body, Body::Data(_))));
}

#[test]
fn xmp_packet_follows_the_container() {
    let xmp = b"<?xpacket begin=\"x\" id=\"W5M0\"?>\n<x:xmpmeta>data</x:xmpmeta>\n   \n<?xpacket end=\"w\"?>";
    let mut trailer = b"junk before ".to_vec();
    trailer.extend_from_slice(xmp);
    trailer.extend_from_slice(b"\n\n");
    let bytes = file(&[chunk(b"abcd", &[1, 2], 0)], &trailer);
    let f = assert_round_trip(&bytes);
    assert_eq!(&*f.trailer, &trailer[..]);
    assert_eq!(f.xmp(), Some(&xmp[..]));
    // No packet, no XMP.
    let bytes = file(&[], b"nothing here");
    assert_eq!(assert_round_trip(&bytes).xmp(), None);
    // An unterminated packet is not returned.
    let bytes = file(&[], b"<?xpacket begin?> and no end");
    assert_eq!(assert_round_trip(&bytes).xmp(), None);
}

#[test]
fn chunk_larger_than_its_parent_is_an_error_at_the_top_level() {
    let mut bad = b"big!".to_vec();
    bad.extend_from_slice(&0xffff_fff0u32.to_be_bytes());
    bad.extend_from_slice(&[0; 4]);
    let bytes = file(&[bad], b"");
    assert!(matches!(parse(&bytes), Err(AepError::ChunkTooLarge { size: 0xffff_fff0, .. })));
    // A chunk header cut short by the container end.
    let bytes = file(&[vec![b'a', b'b', b'c', b'd', 0, 0]], b"");
    assert!(matches!(parse(&bytes), Err(AepError::Malformed { .. })));
}

#[test]
fn deep_nesting_is_kept_opaque_below_the_depth_limit() {
    // 50 000 nested lists, emitted outermost first so building stays linear.
    let levels: usize = 50_000;
    let leaf = chunk(b"leaf", &[1, 2], 0);
    let mut chunk_bytes = Vec::new();
    for _ in 0..levels {
        chunk_bytes.extend_from_slice(b"LIST");
        // placeholder for the size, filled below
        chunk_bytes.extend_from_slice(&[0; 4]);
        chunk_bytes.extend_from_slice(b"deep");
    }
    chunk_bytes.extend_from_slice(&leaf);
    let mut inner_total = leaf.len();
    for level in (0..levels).rev() {
        let size = 4 + inner_total;
        let at = level * 12 + 4;
        chunk_bytes[at..at + 4].copy_from_slice(&(size as u32).to_be_bytes());
        inner_total = 8 + size;
    }
    let bytes = file(&[chunk_bytes], b"");
    let f = assert_round_trip(&bytes);
    let depth = f.walk().map(|(d, _)| d).max().unwrap();
    assert!(depth <= Limits::default().max_depth, "depth {depth}");
    // A smaller limit parses less and still round-trips.
    let f = parse_with_limits(&bytes, Limits { max_depth: 3, max_chunks: 1000 }).unwrap();
    assert!(f.walk().map(|(d, _)| d).max().unwrap() <= 3);
    assert!(write(&f).unwrap() == bytes);
}

#[test]
fn chunk_count_limit() {
    let many: Vec<Vec<u8>> = (0..100).map(|_| chunk(b"abcd", &[], 0)).collect();
    let bytes = file(&many, b"");
    assert_eq!(parse_with_limits(&bytes, Limits { max_depth: 8, max_chunks: 99 }), Err(AepError::TooManyChunks));
    assert!(parse_with_limits(&bytes, Limits { max_depth: 8, max_chunks: 100 }).is_ok());
}

#[test]
fn huge_declared_sizes_do_not_allocate() {
    // A list that claims a gigantic child inside a small file: no allocation sized by the claim.
    let mut c = b"LIST".to_vec();
    c.extend_from_slice(&u32::MAX.to_be_bytes());
    c.extend_from_slice(b"lsta");
    let bytes = file(&[c], b"");
    assert!(matches!(parse(&bytes), Err(AepError::ChunkTooLarge { .. })));
}

#[test]
fn modified_trees_are_re_sized_on_write() {
    let bytes = file(&[list(b"Fold", &[chunk(b"name", b"abc", 0), chunk(b"keep", &[7, 7], 0)])], b"tail");
    let mut f = parse(&bytes).unwrap();
    if let Some(Chunk { body: Body::List { children, .. }, .. }) = f.chunks.first_mut() {
        children[0] = Chunk::data(*b"name", b"a longer name!".to_vec());
    }
    let out = write(&f).unwrap();
    let g = parse(&out).unwrap();
    let kids = g.chunks[0].children();
    assert_eq!(kids[0].bytes(), Some(&b"a longer name!"[..]));
    assert_eq!(kids[1].bytes(), Some(&[7u8, 7][..]));
    assert_eq!(&*g.trailer, b"tail");
    assert!(write(&g).unwrap() == out);
}

#[test]
fn writer_rejects_absurd_depth_instead_of_overflowing_the_stack() {
    let mut c = Chunk::data(*b"leaf", vec![1u8, 2]);
    for _ in 0..2000 {
        c = Chunk::list(*b"deep", vec![c]);
    }
    let f = effectcraft_aep::AepFile { chunks: vec![c], trailer: std::borrow::Cow::Borrowed(&[]) };
    assert_eq!(write(&f), Err(AepError::TooDeep));
    // Dropping the deep tree must not overflow either (2000 levels).
}
