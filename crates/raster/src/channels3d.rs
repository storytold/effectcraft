//! Auxiliary (3D) channels that travel with a layer's pixels: depth, object / material IDs,
//! surface normals, texture UVs, coverage and Cryptomatte ID/coverage pairs.
//!
//! They come from multi-layer OpenEXR footage (any channel by its full `layer.channel` name)
//! or from the compositor's own render passes for precomps of 3D comps. The 3D Channel effects
//! read them through `EffectHost::aux`.
//!
//! Conventions (shared by the EXR reader and the render pass):
//!
//! * channel planes are row-major `width × height` `f32`s covering the layer's source rectangle,
//!   with `scale` aux pixels per layer pixel;
//! * depth is camera-space distance in pixels (`Z`), background `BACKGROUND_DEPTH`;
//! * `ObjectID` / `MaterialID` are small integers (0 = background);
//! * Cryptomatte layers follow the published Cryptomatte specification (Psyop, 2015): a layer
//!   named `CryptoObject` stores rank pairs in `CryptoObject00.R/G/B/A`, `CryptoObject01.R/…`
//!   (ID float, coverage, ID float, coverage, …), IDs are MurmurHash3 (x86, 32-bit, seed 0) of
//!   the object name reinterpreted as a float with the exponent nudged off 0 and 255, and the
//!   manifest maps names to hashes.

/// Depth written where nothing was rendered.
pub const BACKGROUND_DEPTH: f32 = 1.0e10;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuxChannels {
    pub width: u32,
    pub height: u32,
    /// Aux pixels per layer pixel.
    pub scale: f64,
    /// Named planes (`Z`, `ObjectID`, `CryptoObject00.R`, …).
    pub channels: Vec<(String, Vec<f32>)>,
    /// Cryptomatte manifests: (layer name, [(object name, hash)]).
    pub manifests: Vec<(String, Vec<(String, u32)>)>,
}

impl AuxChannels {
    pub fn new(width: u32, height: u32, scale: f64) -> AuxChannels {
        AuxChannels { width, height, scale, channels: vec![], manifests: vec![] }
    }

    /// A channel by exact name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.channels.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_slice())
    }

    /// The first channel present among `names` (also matching `*.name` suffixes, so `Z` finds
    /// `depth.Z`).
    pub fn find(&self, names: &[&str]) -> Option<&[f32]> {
        for n in names {
            if let Some(v) = self.get(n) {
                return Some(v);
            }
        }
        for n in names {
            let suffix = format!(".{}", n.to_ascii_lowercase());
            if let Some((_, v)) = self.channels.iter().find(|(c, _)| c.to_ascii_lowercase().ends_with(&suffix)) {
                return Some(v);
            }
        }
        None
    }

    pub fn names(&self) -> Vec<&str> {
        self.channels.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// The layers of the channels (OpenEXR layers: the name before the last `.`; `""` for the
    /// unnamed layer), in channel order.
    pub fn layers(&self) -> Vec<&str> {
        let mut out: Vec<&str> = vec![];
        for (n, _) in &self.channels {
            let (layer, _) = split_channel(n);
            if !out.contains(&layer) {
                out.push(layer);
            }
        }
        out
    }

    /// A layer's channels as the names to show in red, green, blue and alpha (`""` = none):
    /// its R, G, B, A channels (or red / green / blue / alpha, X / Y / Z, U / V); a layer of one
    /// channel (depth, an ID) shows it in all three colours.
    pub fn layer_rgba(&self, layer: &str) -> [String; 4] {
        let chans: Vec<&str> = self.channels.iter().map(|(n, _)| n.as_str()).filter(|n| split_channel(n).0 == layer).collect();
        let pick = |names: &[&str]| -> String {
            chans.iter().find(|c| names.iter().any(|n| split_channel(c).1.eq_ignore_ascii_case(n))).map_or(String::new(), |c| c.to_string())
        };
        if let [one] = chans.as_slice() {
            return [one.to_string(), one.to_string(), one.to_string(), String::new()];
        }
        [pick(&["R", "red", "X", "U"]), pick(&["G", "green", "Y", "V"]), pick(&["B", "blue", "Z"]), pick(&["A", "alpha"])]
    }
}

/// A channel name's layer and channel: `diffuse.R` → (`diffuse`, `R`), `ViewLayer.Combined.R`
/// → (`ViewLayer.Combined`, `R`), `Z` → (`""`, `Z`).
pub fn split_channel(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or(("", name))
}

impl AuxChannels {
    pub fn depth(&self) -> Option<&[f32]> {
        self.find(&["Z", "depth", "Depth", "zDepth", "Z-Depth"])
    }
    pub fn object_id(&self) -> Option<&[f32]> {
        self.find(&["ObjectID", "object_id", "objectId", "id"])
    }
    pub fn material_id(&self) -> Option<&[f32]> {
        self.find(&["MaterialID", "material_id", "materialId"])
    }
    pub fn coverage(&self) -> Option<&[f32]> {
        self.find(&["Coverage", "coverage"])
    }

    /// Cryptomatte layer names present (prefixes with a `00.R` channel).
    pub fn crypto_layers(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.channels.iter().filter_map(|(n, _)| n.strip_suffix("00.R").or_else(|| n.strip_suffix("00.r")).map(str::to_string)).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The rank planes of Cryptomatte layer `layer`, in rank order: (ID plane, coverage plane)
    /// for `layer00.R/G`, `layer00.B/A`, `layer01.R/G`, ... Resolve them once per evaluation and
    /// index them per pixel; looking the channels up by name for every pixel is what made the
    /// effect slow.
    pub fn crypto_rank_planes(&self, layer: &str) -> Vec<(&[f32], &[f32])> {
        let mut out = vec![];
        for k in 0.. {
            let (r, g, b, a) = (
                self.get(&format!("{layer}{k:02}.R")),
                self.get(&format!("{layer}{k:02}.G")),
                self.get(&format!("{layer}{k:02}.B")),
                self.get(&format!("{layer}{k:02}.A")),
            );
            let (Some(r), Some(g)) = (r, g) else { break };
            out.push((r, g));
            if let (Some(b), Some(a)) = (b, a) {
                out.push((b, a));
            }
        }
        out
    }

    /// Rank pairs (id, coverage) of Cryptomatte layer `layer` at aux pixel index `i`. For a
    /// loop over pixels, use [`AuxChannels::crypto_rank_planes`] once instead.
    pub fn crypto_ranks(&self, layer: &str, i: usize) -> Vec<(f32, f32)> {
        self.crypto_rank_planes(layer).iter().map(|(id, c)| (id[i], c[i])).collect()
    }

    /// The Cryptomatte layer an effect asking for `want` (`CryptoObject`, …) reads: an exact
    /// match, else one ending in it (`ViewLayer.CryptoObject`), else the first layer.
    pub fn resolve_crypto_layer(&self, want: &str) -> Option<String> {
        let layers = self.crypto_layers();
        layers.iter().find(|l| l.as_str() == want || l.ends_with(want)).or(layers.first()).cloned()
    }

    /// The ID (its hash, `f32::to_bits`) with the most coverage at aux pixel `i` of Cryptomatte
    /// layer `layer`, summed over the ranks it appears in. Ties go to the lower rank; IDs with
    /// no positive coverage don't count. `None` when nothing covers the pixel.
    pub fn crypto_id_at(&self, layer: &str, i: usize) -> Option<u32> {
        let mut ids: Vec<(u32, f32)> = vec![];
        for (id, cov) in self.crypto_rank_planes(layer) {
            let (Some(id), Some(&c)) = (id.get(i), cov.get(i)) else { continue };
            if c > 0.0 {
                match ids.iter_mut().find(|(h, _)| *h == id.to_bits()) {
                    Some((_, sum)) => *sum += c,
                    None => ids.push((id.to_bits(), c)),
                }
            }
        }
        // `min_by` keeps the first of equal elements: a tie goes to the lower rank.
        ids.into_iter().min_by(|a, b| b.1.total_cmp(&a.1)).map(|(h, _)| h)
    }

    /// Aux pixel index of a layer-space point (nearest; `None` outside).
    #[inline]
    pub fn index_at(&self, lx: f64, ly: f64) -> Option<usize> {
        let x = (lx * self.scale).floor();
        let y = (ly * self.scale).floor();
        if x < 0.0 || y < 0.0 || x >= self.width as f64 || y >= self.height as f64 {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// Manifest entry name for a hash in Cryptomatte layer `layer`.
    pub fn manifest_name(&self, layer: &str, hash: u32) -> Option<&str> {
        self.manifests.iter().find(|(l, _)| l == layer)?.1.iter().find(|(_, h)| *h == hash).map(|(n, _)| n.as_str())
    }
}

/// MurmurHash3 x86 32-bit (public-domain algorithm by Austin Appleby).
pub fn murmur3_32(data: &[u8], seed: u32) -> u32 {
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;
    let mut h = seed;
    let (chunks, tail) = data.as_chunks::<4>();
    for c in chunks {
        let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h ^= k;
        h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xe654_6b64);
    }
    let mut k = 0u32;
    for (i, b) in tail.iter().enumerate() {
        k ^= (*b as u32) << (8 * i);
    }
    if !tail.is_empty() {
        k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h ^= k;
    }
    h ^= data.len() as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

/// Cryptomatte hash of a name (the manifest value).
pub fn crypto_hash(name: &str) -> u32 {
    let h = murmur3_32(name.as_bytes(), 0);
    // Keep the float finite and normal: flip the low exponent bit when it is all 0s or 1s.
    let e = (h >> 23) & 0xff;
    if e == 0 || e == 255 { h ^ (1 << 23) } else { h }
}

/// The ID float stored in Cryptomatte channels for a hash.
pub fn crypto_float(hash: u32) -> f32 {
    f32::from_bits(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picked ID: most coverage (summed over ranks), ties to the lower rank, nothing when
    /// no rank covers the pixel; layers resolve as the Cryptomatte effect does.
    #[test]
    fn crypto_id_at_takes_the_top_coverage_and_layers_resolve_by_suffix() {
        let mut a = AuxChannels::new(4, 1, 1.0);
        let f = |n: &str| crypto_float(crypto_hash(n));
        let (x, y, z) = (f("x"), f("y"), f("z"));
        for (c, v) in [("R", [x, x, x, x]), ("G", [0.3, 0.5, 0.0, 0.2]), ("B", [y, y, y, y]), ("A", [0.6, 0.5, 0.0, 0.3])] {
            a.channels.push((format!("ViewLayer.CryptoObject00.{c}"), v.to_vec()));
        }
        for (c, v) in [("R", [z, z, z, x]), ("G", [0.1, 0.0, -1.0, 0.2])] {
            a.channels.push((format!("ViewLayer.CryptoObject01.{c}"), v.to_vec()));
        }
        a.channels.push(("Other00.R".into(), vec![0.0; 4]));
        let l = a.resolve_crypto_layer("CryptoObject").unwrap();
        assert_eq!(l, "ViewLayer.CryptoObject");
        assert_eq!(a.resolve_crypto_layer("CryptoMaterial").as_deref(), Some("Other"), "falls back to the first");
        let at = |i| a.crypto_id_at(&l, i);
        assert_eq!([at(0), at(1), at(2), at(3), at(9)], [Some(crypto_hash("y")), Some(crypto_hash("x")), None, Some(crypto_hash("x")), None]);
        assert_eq!(AuxChannels::new(1, 1, 1.0).resolve_crypto_layer("CryptoObject"), None);
    }

    #[test]
    fn murmur3_reference_values() {
        // Published MurmurHash3_x86_32 test vectors.
        assert_eq!(murmur3_32(b"", 0), 0);
        assert_eq!(murmur3_32(b"", 1), 0x514e_28b7);
        assert_eq!(murmur3_32(b"", 0xffff_ffff), 0x81f1_6f39);
        assert_eq!(murmur3_32(&[0, 0, 0, 0], 0), 0x2362_f9de);
        assert_eq!(murmur3_32(b"a", 0x9747_b28c), 0x7fa0_9ea6);
        assert_eq!(murmur3_32(b"aaaa", 0x9747_b28c), 0x5a97_808a);
        assert_eq!(murmur3_32(b"Hello, world!", 0x9747_b28c), 0x2488_4cba);
        assert_eq!(murmur3_32(b"The quick brown fox jumps over the lazy dog", 0x9747_b28c), 0x2fa8_26cd);
    }

    #[test]
    fn crypto_floats_are_normal_and_round_trip() {
        for n in ["bunny", "set", "ground", "a", ""] {
            let h = crypto_hash(n);
            let f = crypto_float(h);
            assert!(f.is_finite() && (f == 0.0 || f.is_normal() || f.abs() > 0.0), "{n}");
            assert_eq!(f.to_bits(), h);
        }
    }

    #[test]
    fn lookup_helpers() {
        let mut a = AuxChannels::new(2, 1, 1.0);
        a.channels.push(("depth.Z".into(), vec![5.0, 6.0]));
        a.channels.push(("CryptoObject00.R".into(), vec![1.0, 2.0]));
        a.channels.push(("CryptoObject00.G".into(), vec![0.5, 1.0]));
        assert_eq!(a.depth(), Some(&[5.0, 6.0][..]));
        assert_eq!(a.crypto_layers(), vec!["CryptoObject".to_string()]);
        assert_eq!(a.crypto_ranks("CryptoObject", 0), vec![(1.0, 0.5)]);
        assert_eq!(a.index_at(1.5, 0.2), Some(1));
        assert_eq!(a.index_at(2.5, 0.2), None);
    }

    /// The rank planes come in rank order (00.R/G, 00.B/A, 01.R/G, ...), match lowercase channel
    /// names as Blender writes them, stop at the first missing rank and agree with
    /// `crypto_ranks` at every pixel.
    #[test]
    fn crypto_rank_planes_resolve_ranks_in_order() {
        let mut a = AuxChannels::new(2, 1, 1.0);
        for (n, v) in [
            ("00.r", [1.0, 2.0]),
            ("00.g", [0.5, 0.6]),
            ("00.b", [3.0, 4.0]),
            ("00.a", [0.25, 0.3]),
            ("01.R", [5.0, 6.0]),
            ("01.G", [0.1, 0.2]),
            ("03.R", [9.0, 9.0]),
            ("03.G", [1.0, 1.0]),
        ] {
            a.channels.push((format!("CryptoMaterial{n}"), v.to_vec()));
        }
        let planes = a.crypto_rank_planes("CryptoMaterial");
        let ids: Vec<f32> = planes.iter().map(|(id, _)| id[1]).collect();
        assert_eq!(ids, vec![2.0, 4.0, 6.0]);
        for i in 0..2 {
            let from_planes: Vec<(f32, f32)> = planes.iter().map(|(id, c)| (id[i], c[i])).collect();
            assert_eq!(from_planes, a.crypto_ranks("CryptoMaterial", i));
        }
        assert!(a.crypto_rank_planes("CryptoObject").is_empty());
    }
}
