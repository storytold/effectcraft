//! Compact text encodings of mattes for project storage: run-length coding (LEB128 varints)
//! wrapped in standard base64 (RFC 4648 §4).

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    base64_decode_bounded(s, usize::MAX)
}

fn base64_decode_bounded(s: &str, max_bytes: usize) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let significant = s.bytes().filter(|c| !c.is_ascii_whitespace()).count();
    let max_encoded = max_bytes.checked_add(2).and_then(|n| n.checked_div(3)).and_then(|n| n.checked_mul(4)).unwrap_or(usize::MAX);
    if !significant.is_multiple_of(4) || significant > max_encoded {
        return None;
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(significant).ok()?;
    bytes.extend(s.bytes().filter(|c| !c.is_ascii_whitespace()));
    let decoded_len = bytes.len().checked_div(4)?.checked_mul(3)?;
    if decoded_len > max_bytes.saturating_add(2) {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(decoded_len.min(max_bytes)).ok()?;
    for c in bytes.chunks(4) {
        let pad = c.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || c[..4 - pad].contains(&b'=') {
            return None;
        }
        let mut n = 0u32;
        for (i, b) in c.iter().enumerate() {
            n |= if *b == b'=' { 0 } else { val(*b)? } << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn put(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *data.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Encode a byte plane as (value, run length) pairs.
pub fn encode(values: &[u8]) -> String {
    let mut out = vec![];
    put(&mut out, values.len() as u64);
    let mut i = 0;
    while i < values.len() {
        let v = values[i];
        let mut j = i + 1;
        while j < values.len() && values[j] == v {
            j += 1;
        }
        out.push(v);
        put(&mut out, (j - i) as u64);
        i = j;
    }
    base64_encode(&out)
}

/// Decode [`encode`]'s output (None when malformed or larger than the caller's validated plane).
pub fn decode_bounded(s: &str, max_len: usize) -> Option<Vec<u8>> {
    let encoded_limit = max_len.saturating_mul(16).saturating_add(128);
    if s.len() > encoded_limit {
        return None;
    }
    // An alternating byte plane takes at most one value byte and a ten-byte varint per
    // pixel, plus the declared-length varint.
    let decoded_limit = max_len.saturating_mul(11).saturating_add(10);
    let data = base64_decode_bounded(s, decoded_limit)?;
    let mut pos = 0;
    let n = usize::try_from(get(&data, &mut pos)?).ok()?;
    if n > max_len {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(n).ok()?;
    while out.len() < n {
        let v = *data.get(pos)?;
        pos += 1;
        let run = usize::try_from(get(&data, &mut pos)?).ok()?;
        let next = out.len().checked_add(run)?;
        if run == 0 || next > n {
            return None;
        }
        out.resize(next, v);
    }
    Some(out)
}

/// Decode [`encode`]'s output, capped at the standard matte-plane limit.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    decode_bounded(s, MAX_PLANE_BYTES)
}

/// Maximum frozen matte plane size (256 MiB at one byte per pixel).
pub const MAX_PLANE_BYTES: usize = 256 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_oversized_and_zero_runs_before_expansion() {
        let mut declared = Vec::new();
        put(&mut declared, 1024);
        declared.extend_from_slice(&[255, 0x80, 0x08]); // 1024-byte run
        assert!(decode_bounded(&base64_encode(&declared), 16).is_none());
        let mut zero_run = Vec::new();
        put(&mut zero_run, 1);
        zero_run.extend_from_slice(&[0, 0]);
        assert!(decode_bounded(&base64_encode(&zero_run), 16).is_none());
        assert!(decode_bounded(&"A".repeat(100_000), 16).is_none());
    }

    #[test]
    fn round_trips() {
        for n in [0usize, 1, 2, 3, 4, 5, 100, 1000] {
            let v: Vec<u8> = (0..n).map(|i| ((i / 7) % 3) as u8 * 100).collect();
            assert_eq!(decode(&encode(&v)).unwrap(), v, "RLE n={n}");
            let raw: Vec<u8> = (0..n).map(|i| (i * 31) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&raw)).unwrap(), raw);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(decode("!!!!").is_none());
    }
}
