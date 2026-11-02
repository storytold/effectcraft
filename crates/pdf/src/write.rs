//! A minimal PDF writer for generating test files (Visualab Effects does not export PDF).

/// Numbered objects `(number, dictionary, stream data)` (streams Flate-compressed unless the
/// dictionary names its own `/Filter`, in which case the data is written as given), a
/// cross-reference table and a trailer naming object `root` as the catalog.
pub fn pdf(objects: &[(u32, String, Option<Vec<u8>>)], root: u32) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![];
    for (n, dict, stream) in objects {
        offsets.push((*n, out.len()));
        match stream {
            Some(data) if dict.contains("/Filter") => {
                let d = format!("{} /Length {} >>", dict.trim_end().trim_end_matches(">>"), data.len());
                out.extend_from_slice(format!("{n} 0 obj\n{d}\nstream\n").as_bytes());
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream\nendobj\n");
            }
            Some(data) => {
                let z = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
                let d = format!("{} /Filter /FlateDecode /Length {} >>", dict.trim_end().trim_end_matches(">>"), z.len());
                out.extend_from_slice(format!("{n} 0 obj\n{d}\nstream\n").as_bytes());
                out.extend_from_slice(&z);
                out.extend_from_slice(b"\nendstream\nendobj\n");
            }
            None => out.extend_from_slice(format!("{n} 0 obj\n{dict}\nendobj\n").as_bytes()),
        }
    }
    let xref = out.len();
    let max = objects.iter().map(|o| o.0).max().unwrap_or(0);
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", max + 1).as_bytes());
    for i in 1..=max {
        match offsets.iter().find(|o| o.0 == i) {
            Some((_, off)) => out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()),
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n", max + 1).as_bytes());
    out
}
