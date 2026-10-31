//! A minimal ZIP writer (stored entries, PKWARE APPNOTE 6.3 local headers + central directory)
//! for `.lottie` (dotLottie) archives.

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Build a ZIP archive with the given (name, bytes) entries, uncompressed.
pub fn store(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let n = name.as_bytes();
        // Local file header.
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, 20); // version needed
        put16(&mut out, 0x0800); // flags: UTF-8 names
        put16(&mut out, 0); // stored
        put16(&mut out, 0); // time
        put16(&mut out, 0x21); // date (1980-01-01)
        put32(&mut out, crc);
        put32(&mut out, data.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, n.len() as u16);
        put16(&mut out, 0);
        out.extend_from_slice(n);
        out.extend_from_slice(data);
        // Central directory record.
        put32(&mut central, 0x0201_4b50);
        put16(&mut central, 20); // made by
        put16(&mut central, 20);
        put16(&mut central, 0x0800);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0x21);
        put32(&mut central, crc);
        put32(&mut central, data.len() as u32);
        put32(&mut central, data.len() as u32);
        put16(&mut central, n.len() as u16);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // comment
        put16(&mut central, 0); // disk
        put16(&mut central, 0); // internal attrs
        put32(&mut central, 0); // external attrs
        put32(&mut central, offset);
        central.extend_from_slice(n);
    }
    let cd_offset = out.len() as u32;
    let cd_len = central.len() as u32;
    out.extend_from_slice(&central);
    put32(&mut out, 0x0605_4b50);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, cd_len);
    put32(&mut out, cd_offset);
    put16(&mut out, 0);
    out
}

/// Whether an entry name is a relative path inside the archive: not absolute (`/…`, `\…`), no
/// drive (`C:…`) and no `..` component.
fn is_relative_name(name: &str) -> bool {
    let drive = name.as_bytes().get(1) == Some(&b':');
    !name.starts_with(['/', '\\']) && !drive && !name.split(['/', '\\']).any(|c| c == "..")
}

/// Read the stored (uncompressed) entries of a ZIP archive written by [`store`] or any other
/// writer that did not compress. Compressed entries and entries whose names are not relative
/// paths inside the archive are skipped.
///
/// Sizes and offsets come from the central directory, so entries written by streaming writers
/// (flag bit 3: zero sizes in the local header, real sizes in a trailing data descriptor) keep
/// their data. Archives without an end-of-central-directory record fall back to walking the
/// local headers.
pub fn read_stored(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    read_central(data).unwrap_or_else(|| read_local(data))
}

fn u16_at(data: &[u8], p: usize) -> Option<usize> {
    let b = data.get(p..p.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]) as usize)
}

fn u32_at(data: &[u8], p: usize) -> Option<usize> {
    let b = data.get(p..p.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

/// Walk the central directory found through the end-of-central-directory record. `None` when
/// there is no such record.
fn read_central(data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    // The EOCD is 22 bytes plus a comment of at most 65535 bytes; search backwards for it.
    let last = data.len().checked_sub(22)?;
    let eocd = (last.saturating_sub(0xFFFF)..=last).rev().find(|&p| u32_at(data, p) == Some(0x0605_4b50))?;
    let count = u16_at(data, eocd + 10)?;
    let mut p = u32_at(data, eocd + 16)?;
    let mut out = vec![];
    for _ in 0..count {
        let Some(entry) = central_entry(data, p) else { break };
        let (next, item) = entry;
        out.extend(item);
        p = next;
    }
    Some(out)
}

/// Parse the central record at `p`: the offset of the next record and the entry, if it is a
/// stored entry with a relative name whose data lies inside the file.
fn central_entry(data: &[u8], p: usize) -> Option<(usize, Option<(String, Vec<u8>)>)> {
    if u32_at(data, p)? != 0x0201_4b50 {
        return None;
    }
    let method = u16_at(data, p + 10)?;
    let size = u32_at(data, p + 20)?;
    let nlen = u16_at(data, p + 28)?;
    let xlen = u16_at(data, p + 30)?;
    let clen = u16_at(data, p + 32)?;
    let local = u32_at(data, p + 42)?;
    let name_start = p + 46;
    let name = String::from_utf8_lossy(data.get(name_start..name_start + nlen)?).to_string();
    let next = name_start + nlen + xlen + clen;
    if method != 0 || !is_relative_name(&name) {
        return Some((next, None));
    }
    let entry = (|| {
        if u32_at(data, local)? != 0x0403_4b50 {
            return None;
        }
        let body = local.checked_add(30)?.checked_add(u16_at(data, local + 26)?)?.checked_add(u16_at(data, local + 28)?)?;
        Some(data.get(body..body.checked_add(size)?)?.to_vec())
    })();
    Some((next, entry.map(|bytes| (name, bytes))))
}

/// Walk the local file headers from the start (used when there is no central directory).
fn read_local(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = vec![];
    let mut i = 0usize;
    let u16_at = |p: usize| u16_at(data, p);
    let u32_at = |p: usize| u32_at(data, p);
    while u32_at(i) == Some(0x0403_4b50) {
        let (Some(method), Some(size), Some(nlen), Some(xlen)) = (u16_at(i + 8), u32_at(i + 18), u16_at(i + 26), u16_at(i + 28)) else { break };
        let name_start = i + 30;
        let body = name_start + nlen + xlen;
        let (Some(name), Some(bytes)) = (data.get(name_start..name_start + nlen), data.get(body..body + size)) else { break };
        let name = String::from_utf8_lossy(name).to_string();
        if method == 0 && is_relative_name(&name) {
            out.push((name, bytes.to_vec()));
        }
        i = body + size;
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc_and_roundtrip() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
        let z = super::store(&[("a.json".into(), b"{}".to_vec()), ("b/c.txt".into(), b"hello".to_vec())]);
        let back = super::read_stored(&z);
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].0, "b/c.txt");
        assert_eq!(back[1].1, b"hello");
    }

    #[test]
    fn entries_outside_the_archive_root_are_skipped() {
        let names = ["../up.txt", "a/../../up.txt", "/abs/x", "\\abs\\x", "C:\\x", "c:x", "a\\..\\x", "ok/x.txt", "ok/..x"];
        let z = super::store(&names.iter().map(|n| (n.to_string(), b"x".to_vec())).collect::<Vec<_>>());
        let back: Vec<String> = super::read_stored(&z).into_iter().map(|(n, _)| n).collect();
        assert_eq!(back, ["ok/x.txt", "ok/..x"]);
    }

    /// Build a streaming-style archive: flag bit 3, zero sizes in the local headers, the real
    /// sizes in a data descriptor (with the optional signature for even entries, without for odd
    /// ones) and in the central directory.
    fn streamed(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        let put16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let put32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        for (k, (name, data)) in entries.iter().enumerate() {
            let offset = out.len() as u32;
            let crc = super::crc32(data);
            let n = name.as_bytes();
            put32(&mut out, 0x0403_4b50);
            put16(&mut out, 20);
            put16(&mut out, 0x0808);
            put16(&mut out, 0);
            put16(&mut out, 0);
            put16(&mut out, 0x21);
            put32(&mut out, 0);
            put32(&mut out, 0);
            put32(&mut out, 0);
            put16(&mut out, n.len() as u16);
            put16(&mut out, 0);
            out.extend_from_slice(n);
            out.extend_from_slice(data);
            if k % 2 == 0 {
                put32(&mut out, 0x0807_4b50);
            }
            put32(&mut out, crc);
            put32(&mut out, data.len() as u32);
            put32(&mut out, data.len() as u32);
            put32(&mut central, 0x0201_4b50);
            put16(&mut central, 20);
            put16(&mut central, 20);
            put16(&mut central, 0x0808);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put16(&mut central, 0x21);
            put32(&mut central, crc);
            put32(&mut central, data.len() as u32);
            put32(&mut central, data.len() as u32);
            put16(&mut central, n.len() as u16);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put16(&mut central, 0);
            put32(&mut central, 0);
            put32(&mut central, offset);
            central.extend_from_slice(n);
        }
        let cd_offset = out.len() as u32;
        let cd_len = central.len() as u32;
        out.extend_from_slice(&central);
        put32(&mut out, 0x0605_4b50);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, entries.len() as u16);
        put16(&mut out, entries.len() as u16);
        put32(&mut out, cd_len);
        put32(&mut out, cd_offset);
        put16(&mut out, 0);
        out
    }

    #[test]
    fn stored_entries_with_data_descriptors_keep_their_data() {
        let z = streamed(&[("a.json", b"{}"), ("b.txt", b"hello"), ("c.txt", b"xyz")]);
        let back = super::read_stored(&z);
        assert_eq!(back.len(), 3);
        assert_eq!(back[0], ("a.json".to_string(), b"{}".to_vec()));
        assert_eq!(back[1], ("b.txt".to_string(), b"hello".to_vec()));
        assert_eq!(back[2], ("c.txt".to_string(), b"xyz".to_vec()));
    }

    #[test]
    fn bad_central_directory_offsets_and_truncation_do_not_panic() {
        let mut z = streamed(&[("a.json", b"{}"), ("b.txt", b"hello")]);
        // Point the second central record's local-header offset past the end of the file.
        let cd = z.len() - 22;
        let cd_offset = u32::from_le_bytes([z[cd + 16], z[cd + 17], z[cd + 18], z[cd + 19]]) as usize;
        let second = cd_offset + 46 + "a.json".len();
        z[second + 42..second + 46].copy_from_slice(&u32::MAX.to_le_bytes());
        let back = super::read_stored(&z);
        assert!(back.iter().all(|(n, _)| n != "b.txt"));
        for cut in 0..z.len() {
            for (_, bytes) in super::read_stored(&z[..cut]) {
                assert!(bytes.len() <= 5);
            }
        }
    }
}
