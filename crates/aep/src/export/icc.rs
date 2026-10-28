//! A colour profile for a working space After Effects names without shipping its data to us:
//! "Rec.709 Gamma 2.4", authored here from the standards (ITU-R BT.709 primaries and D65 white,
//! a pure 2.4 power curve, ICC.1:2010 v4 with a D50 PCS and a Bradford adaptation), so that no
//! profile of another party is copied. After Effects keeps a working space as the profile's data
//! (base64) and its profile ID (`cpid`, 624: the MD5 profile ID of the v4 header), R49.

/// The profile's description and the name After Effects lists the working space under (624).
pub(crate) const REC709_NAME: &str = "Rec.709 Gamma 2.4";

fn s15(v: f64) -> [u8; 4] {
    let x = (v * 65536.0).round();
    let x = if x.is_finite() { x.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) } else { 0.0 };
    (x as i32).to_be_bytes()
}

type M3 = [[f64; 3]; 3];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn mulv(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| (0..3).map(|k| a[i][k] * v[k]).sum())
}

fn inv(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let det = if det.abs() < 1e-12 { 1.0 } else { det };
    [
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ]
}

fn xyz(x: f64, y: f64) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

fn mluc(text: &str) -> Vec<u8> {
    let t: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut v = b"mluc\0\0\0\0".to_vec();
    v.extend(1u32.to_be_bytes());
    v.extend(12u32.to_be_bytes());
    v.extend(b"enUS");
    v.extend(u32::try_from(t.len()).unwrap_or(0).to_be_bytes());
    v.extend(28u32.to_be_bytes());
    v.extend(t);
    v
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut o = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        o.extend(s15(c));
    }
    o
}

/// The Rec.709 Gamma 2.4 display profile (ICC v4.3) with its profile ID filled in.
pub(crate) fn rec709_gamma24() -> Vec<u8> {
    let prim = [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)];
    let w65 = xyz(0.3127, 0.3290);
    let w50 = [0.9642, 1.0, 0.8249];
    let cols = prim.map(|(x, y)| xyz(x, y));
    let p: M3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| cols[c][r]));
    let s = mulv(&inv(&p), w65);
    let m: M3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| p[r][c] * s[c]));
    let brad: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let (cs, cd) = (mulv(&brad, w65), mulv(&brad, w50));
    let d: M3 = [[cd[0] / cs[0], 0.0, 0.0], [0.0, cd[1] / cs[1], 0.0], [0.0, 0.0, cd[2] / cs[2]]];
    let chad = mul(&inv(&brad), &mul(&d, &brad));
    let m50 = mul(&chad, &m);
    let mut sf32 = b"sf32\0\0\0\0".to_vec();
    for row in chad {
        for v in row {
            sf32.extend(s15(v));
        }
    }
    let mut trc = b"para\0\0\0\0".to_vec();
    trc.extend([0, 0, 0, 0]);
    trc.extend(s15(2.4));
    let tags: [(&[u8; 4], Vec<u8>); 10] = [
        (b"desc", mluc("Rec.709 Gamma 2.4 (EffectCraft)")),
        (b"cprt", mluc("No copyright, use freely (EffectCraft)")),
        (b"wtpt", xyz_tag(w50)),
        (b"chad", sf32),
        (b"rXYZ", xyz_tag([m50[0][0], m50[1][0], m50[2][0]])),
        (b"gXYZ", xyz_tag([m50[0][1], m50[1][1], m50[2][1]])),
        (b"bXYZ", xyz_tag([m50[0][2], m50[1][2], m50[2][2]])),
        (b"rTRC", trc.clone()),
        (b"gTRC", trc.clone()),
        (b"bTRC", trc),
    ];
    let start = 128 + 4 + 12 * tags.len();
    let mut table = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    let mut placed: Vec<(Vec<u8>, usize)> = Vec::new();
    for (sig, data) in &tags {
        let off = match placed.iter().find(|(d, _)| d == data) {
            Some((_, off)) => *off,
            None => {
                let off = start + body.len();
                body.extend(data);
                body.resize(body.len().div_ceil(4) * 4, 0);
                placed.push((data.clone(), off));
                off
            }
        };
        table.extend(*sig);
        table.extend(u32::try_from(off).unwrap_or(0).to_be_bytes());
        table.extend(u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
    }
    let size = start + body.len();
    let mut icc = vec![0u8; 128];
    icc[0..4].copy_from_slice(&u32::try_from(size).unwrap_or(0).to_be_bytes());
    icc[8..12].copy_from_slice(&[4, 0x30, 0, 0]);
    icc[12..16].copy_from_slice(b"mntr");
    icc[16..20].copy_from_slice(b"RGB ");
    icc[20..24].copy_from_slice(b"XYZ ");
    for (i, v) in [2026u16, 10, 19, 0, 0, 0].iter().enumerate() {
        icc[24 + 2 * i..26 + 2 * i].copy_from_slice(&v.to_be_bytes());
    }
    icc[36..40].copy_from_slice(b"acsp");
    for (i, c) in w50.iter().enumerate() {
        icc[68 + 4 * i..72 + 4 * i].copy_from_slice(&s15(*c));
    }
    icc.extend(u32::try_from(tags.len()).unwrap_or(0).to_be_bytes());
    icc.extend(table);
    icc.extend(body);
    // Profile ID (ICC.1:2010 7.2.18): the MD5 with flags, rendering intent and the ID zeroed (all
    // zero here already).
    let id = md5(&icc);
    icc[84..100].copy_from_slice(&id);
    icc
}

/// The profile ID of a v4 profile (header bytes 84..100).
pub(crate) fn profile_id(icc: &[u8]) -> [u8; 16] {
    let mut id = [0u8; 16];
    if let Some(src) = icc.get(84..100) {
        id.copy_from_slice(src);
    }
    id
}

/// MD5 (RFC 1321).
pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4,
        11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] = std::array::from_fn(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32);
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend(bits.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    let (blocks, _) = msg.as_chunks::<64>();
    for chunk in blocks {
        let m: [u32; 16] = std::array::from_fn(|i| u32::from_le_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]));
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, w) in [a0, b0, c0, d0].iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Standard base64 with padding.
pub(crate) fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(c.get(1).copied().unwrap_or(0)) << 8) | u32::from(c.get(2).copied().unwrap_or(0));
        for (i, shift) in [18u32, 12, 6, 0].iter().enumerate() {
            if i <= c.len() {
                out.push(char::from(T[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_and_base64_match_their_specifications() {
        // RFC 1321 test suite and RFC 4648.
        assert_eq!(md5(b"").iter().map(|b| format!("{b:02x}")).collect::<String>(), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"abc").iter().map(|b| format!("{b:02x}")).collect::<String>(), "900150983cd24fb0d6963f7d28e17f72");
        let long = b"12345678901234567890123456789012345678901234567890123456789012345678901234567890";
        assert_eq!(md5(long).iter().map(|b| format!("{b:02x}")).collect::<String>(), "57edf4a22be3c955ac49da2e2107b67a");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
    }

    #[test]
    fn the_rec709_profile_is_a_valid_v4_display_profile() {
        let icc = rec709_gamma24();
        assert_eq!(u32::from_be_bytes(icc[0..4].try_into().unwrap()) as usize, icc.len());
        assert_eq!(&icc[36..40], b"acsp");
        // The ID is the MD5 of the profile with the ID zeroed.
        let mut z = icc.clone();
        z[84..100].fill(0);
        assert_eq!(profile_id(&icc), md5(&z));
        // D50 red primary of BT.709 (0.4361, 0.2225, 0.0139).
        let tags = u32::from_be_bytes(icc[128..132].try_into().unwrap()) as usize;
        let r = (0..tags).find(|t| &icc[132 + 12 * t..136 + 12 * t] == b"rXYZ").unwrap();
        let off = u32::from_be_bytes(icc[136 + 12 * r..140 + 12 * r].try_into().unwrap()) as usize;
        let x = f64::from(i32::from_be_bytes(icc[off + 8..off + 12].try_into().unwrap())) / 65536.0;
        assert!((x - 0.4361).abs() < 2e-4, "{x}");
    }
}
