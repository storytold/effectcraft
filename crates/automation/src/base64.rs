//! Standard base64 (RFC 4648 §4, padded): MCP image content and the control channel's inline PNGs.

pub fn encode(data: &[u8]) -> String {
    craft_mime::base64::STANDARD.encode(data)
}

/// Decode, ignoring ASCII whitespace; `None` on any other invalid input.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (ci, c) in bytes.chunks(4).enumerate() {
        let last = ci == bytes.len() / 4 - 1;
        let pad = c.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &b in &c[..4 - pad] {
            n = n << 6 | val(b)?;
        }
        n <<= 6 * pad as u32;
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_encoder_round_trips_every_byte_and_remainder() {
        let all: Vec<u8> = (0..=255).cycle().take(513).collect();
        for length in 0..=all.len() {
            let input = &all[..length];
            let encoded = super::encode(input);
            assert_eq!(encoded.len(), length.div_ceil(3) * 4);
            assert_eq!(super::decode(&encoded).as_deref(), Some(input));
            assert_eq!(encoded.bytes().rev().take_while(|b| *b == b'=').count(), (3 - length % 3) % 3);
        }
    }

    #[test]
    fn rfc4648_vectors() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::encode(i.as_bytes()), o);
            assert_eq!(super::decode(o).unwrap(), i.as_bytes());
        }
        assert!(super::decode("Zm9v!").is_none());
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(super::decode(&super::encode(&all)).unwrap(), all);
    }
}
