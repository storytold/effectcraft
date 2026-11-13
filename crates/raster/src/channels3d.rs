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

/// The names [`AuxChannels::depth`] looks for, in order (also as `*.name` suffixes).
pub const DEPTH_NAMES: [&str; 5] = ["Z", "depth", "Depth", "zDepth", "Z-Depth"];
/// The names [`AuxChannels::object_id`] looks for.
pub const OBJECT_ID_NAMES: [&str; 4] = ["ObjectID", "object_id", "objectId", "id"];
/// The names [`AuxChannels::material_id`] looks for.
pub const MATERIAL_ID_NAMES: [&str; 3] = ["MaterialID", "material_id", "materialId"];
/// The names [`AuxChannels::coverage`] looks for.
pub const COVERAGE_NAMES: [&str; 2] = ["Coverage", "coverage"];

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
    /// Channels of a multi-part OpenEXR file whose stored name is itself a `layer.channel` name
    /// but doesn't start with its part's name (`Crypto_Material00.R` in part `Crypto_Material`):
    /// (the name as stored, the plane's full name `Crypto_Material.Crypto_Material00.R`).
    /// Lookups by the stored name find the plane; the full name is what is shown (#3271).
    pub aliases: Vec<(String, String)>,
    /// Every channel name of the source in order, when `channels` holds only some of them (the
    /// channels a layer's effects read, #3271); empty when `channels` holds them all. Lists
    /// that depend on every channel (the Cryptomatte layers) read it.
    pub listed: Vec<String>,
}

/// What an effect reads of its layer's auxiliary channels, declared before the render
/// ([`AuxNeed`]): each lookup is resolved against the source's channel names exactly as the
/// effect resolves it on the planes, so the planes read are the ones it looks up (#3271).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AuxLookup {
    /// [`AuxChannels::get`] of this name.
    Name(String),
    /// [`AuxChannels::find`] of these names.
    Find(Vec<String>),
    /// The rank planes of the Cryptomatte layer the effect reads: Layer Name `name`, else the
    /// layer of the Layer popup's stored value `index` ([`crypto_layer_named`]).
    Crypto { name: String, index: usize },
}

impl AuxLookup {
    /// [`AuxLookup::Find`] of `names`.
    pub fn find(names: &[&str]) -> AuxLookup {
        AuxLookup::Find(names.iter().map(|n| n.to_string()).collect())
    }
}

/// The auxiliary channels a layer's effects read (#3271): all of them (an effect that doesn't
/// declare, or parameters that change over time), or what its lookups find.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum AuxNeed {
    #[default]
    All,
    Lookups(Vec<AuxLookup>),
}

impl AuxNeed {
    /// What both need.
    pub fn union(self, other: AuxNeed) -> AuxNeed {
        match (self, other) {
            (AuxNeed::Lookups(mut a), AuxNeed::Lookups(b)) => {
                for l in b {
                    if !a.contains(&l) {
                        a.push(l);
                    }
                }
                AuxNeed::Lookups(a)
            }
            _ => AuxNeed::All,
        }
    }

    /// The channel names of `names` (a source's channels; planes not needed) these lookups
    /// find, in channel order, once each. `None`: all of them.
    pub fn resolve(&self, names: &AuxChannels) -> Option<Vec<String>> {
        let AuxNeed::Lookups(lookups) = self else { return None };
        let mut picked = vec![false; names.channels.len()];
        let mut mark = |i: Option<usize>| {
            if let Some(slot) = i.and_then(|i| picked.get_mut(i)) {
                *slot = true;
            }
        };
        for l in lookups {
            match l {
                AuxLookup::Name(n) => mark(names.get_index(n.trim())),
                AuxLookup::Find(list) => {
                    let list: Vec<&str> = list.iter().map(String::as_str).collect();
                    mark(names.find_index(&list));
                }
                AuxLookup::Crypto { name, index } => {
                    if let Some(layer) = crypto_layer_named(names, name, *index) {
                        for (id, cov) in names.crypto_rank_indices(&layer) {
                            mark(Some(id));
                            mark(Some(cov));
                        }
                    }
                }
            }
        }
        Some(names.channels.iter().zip(picked).filter(|(_, p)| *p).map(|((n, _), _)| n.clone()).collect())
    }
}

impl AuxChannels {
    pub fn new(width: u32, height: u32, scale: f64) -> AuxChannels {
        AuxChannels { width, height, scale, channels: vec![], manifests: vec![], aliases: vec![], listed: vec![] }
    }

    /// The position in [`AuxChannels::channels`] of a channel by exact name (case-insensitive),
    /// or by the name a multi-part file stores it under ([`AuxChannels::aliases`]).
    pub fn get_index(&self, name: &str) -> Option<usize> {
        if let Some(i) = self.channels.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        let full = self.aliases.iter().find(|(stored, _)| stored.eq_ignore_ascii_case(name)).map(|(_, full)| full)?;
        self.channels.iter().position(|(n, _)| n == full)
    }

    /// A channel by exact name (case-insensitive), or by the name a multi-part file stores it
    /// under (`Crypto_Material00.R` for `Crypto_Material.Crypto_Material00.R`).
    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.get_index(name).and_then(|i| self.channels.get(i)).map(|(_, v)| v.as_slice())
    }

    /// The position of the first channel present among `names` ([`AuxChannels::find`]).
    pub fn find_index(&self, names: &[&str]) -> Option<usize> {
        for n in names {
            if let Some(i) = self.get_index(n) {
                return Some(i);
            }
        }
        for n in names {
            let suffix = format!(".{}", n.to_ascii_lowercase());
            if let Some(i) = self.channels.iter().position(|(c, _)| c.to_ascii_lowercase().ends_with(&suffix)) {
                return Some(i);
            }
        }
        None
    }

    /// The first channel present among `names` (also matching `*.name` suffixes, so `Z` finds
    /// `depth.Z`).
    pub fn find(&self, names: &[&str]) -> Option<&[f32]> {
        self.find_index(names).and_then(|i| self.channels.get(i)).map(|(_, v)| v.as_slice())
    }

    pub fn names(&self) -> Vec<&str> {
        self.channels.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// Every channel name of the source: [`AuxChannels::listed`] when `channels` holds only
    /// some, else the channels'.
    fn all_names(&self) -> Vec<&str> {
        if self.listed.is_empty() { self.names() } else { self.listed.iter().map(String::as_str).collect() }
    }

    /// The layers of the channels (OpenEXR layers: the name before the last `.`; `""` for the
    /// unnamed layer), in channel order.
    pub fn layers(&self) -> Vec<&str> {
        craft_exr::channels::layers(self.channels.iter().map(|(n, _)| n.as_str()))
    }

    /// A layer's channels as the names to show in red, green, blue and alpha (`""` = none):
    /// its R, G, B, A channels (or red / green / blue / alpha, X / Y / Z, U / V); a layer of one
    /// channel (depth, an ID) shows it in all three colours.
    pub fn layer_rgba(&self, layer: &str) -> [String; 4] {
        craft_exr::channels::layer_rgba(self.channels.iter().map(|(n, _)| n.as_str()), layer)
            .map(|i| i.and_then(|i| self.channels.get(i)).map_or(String::new(), |(n, _)| n.clone()))
    }
}

pub use craft_exr::channels::split_channel;

/// The Cryptomatte layer a channel name's first rank `R` belongs to (`CryptoObject00.R` →
/// `CryptoObject`), any case of the component letter.
fn crypto_layer_of(name: &str) -> Option<&str> {
    name.strip_suffix("00.R").or_else(|| name.strip_suffix("00.r"))
}

impl AuxChannels {
    pub fn depth(&self) -> Option<&[f32]> {
        self.find(&DEPTH_NAMES)
    }
    pub fn object_id(&self) -> Option<&[f32]> {
        self.find(&OBJECT_ID_NAMES)
    }
    pub fn material_id(&self) -> Option<&[f32]> {
        self.find(&MATERIAL_ID_NAMES)
    }
    pub fn coverage(&self) -> Option<&[f32]> {
        self.find(&COVERAGE_NAMES)
    }

    /// Cryptomatte layer names present (prefixes with a `00.R` channel), of every channel of the
    /// source ([`AuxChannels::listed`]).
    pub fn crypto_layers(&self) -> Vec<String> {
        let mut v: Vec<String> = self.all_names().into_iter().filter_map(crypto_layer_of).map(str::to_string).collect();
        v.sort();
        v.dedup();
        v
    }

    /// The positions in [`AuxChannels::channels`] of the rank planes of Cryptomatte layer
    /// `layer`, paired as [`AuxChannels::crypto_rank_planes`] pairs them.
    pub fn crypto_rank_indices(&self, layer: &str) -> Vec<(usize, usize)> {
        let mut out = vec![];
        for k in 0.. {
            let (r, g, b, a) = (self.rank(layer, k, 'R'), self.rank(layer, k, 'G'), self.rank(layer, k, 'B'), self.rank(layer, k, 'A'));
            let (Some(r), Some(g)) = (r, g) else { break };
            out.push((r, g));
            if let (Some(b), Some(a)) = (b, a) {
                out.push((b, a));
            }
        }
        out
    }

    /// The rank planes of Cryptomatte layer `layer`, in rank order: (ID plane, coverage plane)
    /// for `layer00.R/G`, `layer00.B/A`, `layer01.R/G`, ... Resolve them once per evaluation and
    /// index them per pixel; looking the channels up by name for every pixel is what made the
    /// effect slow.
    pub fn crypto_rank_planes(&self, layer: &str) -> Vec<(&[f32], &[f32])> {
        let plane = |i: usize| self.channels.get(i).map(|(_, v)| v.as_slice());
        self.crypto_rank_indices(layer).into_iter().filter_map(|(id, cov)| Some((plane(id)?, plane(cov)?))).collect()
    }

    /// The position of plane `{layer}{k:02}.{comp}`: the layer name matches case-sensitively
    /// (layers `Crypto_A` and `crypto_a` are different layers), the component letter either
    /// case; also by the name a multi-part file stores it under ([`AuxChannels::aliases`]).
    fn rank(&self, layer: &str, k: usize, comp: char) -> Option<usize> {
        let tail = format!("{k:02}.{comp}");
        let is_rank = |n: &str| n.strip_prefix(layer).is_some_and(|rest| rest.eq_ignore_ascii_case(&tail));
        if let Some(i) = self.channels.iter().position(|(n, _)| is_rank(n)) {
            return Some(i);
        }
        let full = self.aliases.iter().find(|(stored, _)| is_rank(stored)).map(|(_, full)| full)?;
        self.channels.iter().position(|(n, _)| n == full)
    }

    /// Rank pairs (id, coverage) of Cryptomatte layer `layer` at aux pixel index `i`. For a
    /// loop over pixels, use [`AuxChannels::crypto_rank_planes`] once instead.
    pub fn crypto_ranks(&self, layer: &str, i: usize) -> Vec<(f32, f32)> {
        self.crypto_rank_planes(layer).iter().filter_map(|(id, c)| Some((*id.get(i)?, *c.get(i)?))).collect()
    }

    /// The Cryptomatte layer an effect asking for `want` (`CryptoObject`, …) reads: an exact
    /// match, else one ending in it (`ViewLayer.CryptoObject`), else the first layer.
    pub fn resolve_crypto_layer(&self, want: &str) -> Option<String> {
        let layers = self.crypto_layers();
        layers.iter().find(|l| l.as_str() == want || l.ends_with(want)).or(layers.first()).cloned()
    }

    /// [`AuxChannels::resolve_crypto_layer`] with a layer named in full first: the layer called
    /// exactly `name` when it is not empty and the footage has it (`Crypto_Material`, a name an
    /// imported After Effects project carries), else the layer for `want`.
    /// The name as given is tried first (EXR channel names may carry spaces), then trimmed.
    /// A multi-part file's layer is also found by the name its part stores it under
    /// (`Crypto_Material` for `Crypto_Material.Crypto_Material`, [`AuxChannels::aliases`]).
    pub fn resolve_crypto_layer_named(&self, name: &str, want: &str) -> Option<String> {
        let layers = self.crypto_layers();
        for name in [name, name.trim()] {
            if !name.is_empty() && layers.iter().any(|l| l == name) {
                return Some(name.to_string());
            }
        }
        for name in [name, name.trim()] {
            if name.is_empty() {
                continue;
            }
            let full = self.aliases.iter().filter(|(stored, _)| crypto_layer_of(stored) == Some(name)).find_map(|(_, full)| crypto_layer_of(full));
            if let Some(full) = full.filter(|f| layers.iter().any(|l| l == f)) {
                return Some(full.to_string());
            }
        }
        self.resolve_crypto_layer(want)
    }

    /// The manifest of Cryptomatte layer `layer`: the one named as the layer, else the one named
    /// as a multi-part file's part stores the layer (the manifest `Crypto_Material` for the layer
    /// `Crypto_Material.Crypto_Material`).
    pub fn manifest(&self, layer: &str) -> Option<&[(String, u32)]> {
        if let Some((_, m)) = self.manifests.iter().find(|(l, _)| l == layer) {
            return Some(m.as_slice());
        }
        let stored = self.aliases.iter().filter(|(_, full)| crypto_layer_of(full) == Some(layer)).find_map(|(stored, _)| crypto_layer_of(stored))?;
        self.manifests.iter().find(|(l, _)| l == stored).map(|(_, m)| m.as_slice())
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
        self.manifest(layer)?.iter().find(|(_, h)| *h == hash).map(|(n, _)| n.as_str())
    }
}

/// The Cryptomatte layers every renderer names, and the Layer popup's options when the footage
/// has no Cryptomatte metadata. The stored Layer value is an index: 0-2 are these (an exact
/// match or a suffix such as `ViewLayer.CryptoObject`), 3 and up are the file's other Cryptomatte
/// layers in name order, so a project saved before the popup followed the footage loads unchanged.
pub const CRYPTO_LAYERS: [&str; 3] = ["CryptoObject", "CryptoMaterial", "CryptoAsset"];

/// The Layer popup's entries for Cryptomatte `layers` (sorted, as [`AuxChannels::crypto_layers`]
/// returns them): the stored value and the layer's name, in value order. Of several layers
/// that fit one standard slot (`A.CryptoObject`, `B.CryptoObject`) the first one keeps the standard
/// slot, so existing projects load unchanged, and the others get the next custom slots.
fn crypto_layer_slots(layers: &[String]) -> Vec<(usize, &str)> {
    let mut out: Vec<(usize, &str)> = vec![];
    let mut custom = CRYPTO_LAYERS.len();
    for l in layers {
        match CRYPTO_LAYERS.iter().position(|w| l == w || l.ends_with(w)) {
            Some(i) if !out.iter().any(|(j, _)| *j == i) => out.push((i, l)),
            _ => {
                out.push((custom, l));
                custom += 1;
            }
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out
}

/// What the Layer popup lists for the footage `aux`: the Cryptomatte layers the file has, as
/// (stored value, name), or the three standard ones when there is none (no footage, not an
/// OpenEXR, no Cryptomatte channels).
pub fn crypto_layer_choices(aux: Option<&AuxChannels>) -> Vec<(usize, String)> {
    crypto_layer_menu(aux, 0).0
}

/// The Layer popup for the stored value `index`: [`crypto_layer_choices`], and the name of the
/// entry the effect reads (the first when `index` names none, as the effect does).
pub fn crypto_layer_menu(aux: Option<&AuxChannels>, index: usize) -> (Vec<(usize, String)>, Option<String>) {
    let layers = aux.map(AuxChannels::crypto_layers).unwrap_or_default();
    if layers.is_empty() {
        let all: Vec<(usize, String)> = CRYPTO_LAYERS.iter().enumerate().map(|(i, n)| (i, n.to_string())).collect();
        let shown = all.iter().find(|c| c.0 == index).or(all.first()).map(|c| c.1.clone());
        return (all, shown);
    }
    let slots = crypto_layer_slots(&layers);
    let shown = slots.iter().find(|(i, _)| *i == index).map(|(_, n)| n.to_string()).or_else(|| layers.first().cloned());
    (slots.into_iter().map(|(i, n)| (i, n.to_string())).collect(), shown)
}

/// The Cryptomatte layer the effect reads: the one called `name` when the footage has it (Layer
/// Name, the layer an imported After Effects project names), else [`crypto_layer_name`]'s.
pub fn crypto_layer_named(aux: &AuxChannels, name: &str, index: usize) -> Option<String> {
    aux.resolve_crypto_layer_named(name, &crypto_layer_name(aux, index)?)
}

/// The Cryptomatte layer of `aux` that the stored Layer value `index` reads: its slot's layer, else
/// the first layer (what a file without that layer has always shown). `None` without any.
pub fn crypto_layer_name(aux: &AuxChannels, index: usize) -> Option<String> {
    if aux.crypto_layers().is_empty() {
        return None;
    }
    crypto_layer_menu(Some(aux), index).1
}

pub use craft_exr::cryptomatte::{hash as crypto_hash, murmur3_32};

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
        // A layer named in full wins; an empty or missing name falls back to `want`.
        assert_eq!(a.resolve_crypto_layer_named("Other", "CryptoObject").as_deref(), Some("Other"));
        assert_eq!(a.resolve_crypto_layer_named(" Other ", "CryptoObject").as_deref(), Some("Other"));
        assert_eq!(a.resolve_crypto_layer_named("", "CryptoObject").as_deref(), Some("ViewLayer.CryptoObject"));
        assert_eq!(a.resolve_crypto_layer_named("Crypto_Name", "CryptoObject").as_deref(), Some("ViewLayer.CryptoObject"));
        assert_eq!(AuxChannels::new(1, 1, 1.0).resolve_crypto_layer_named("Other", "CryptoObject"), None);
        // A layer whose name ends in a space is matched as named, before the trimmed name.
        let mut b = AuxChannels::new(1, 1, 1.0);
        for l in ["Mat", "Mat "] {
            b.channels.push((format!("{l}00.R"), vec![0.0]));
        }
        assert_eq!(b.resolve_crypto_layer_named("Mat ", "CryptoObject").as_deref(), Some("Mat "));
        assert_eq!(b.resolve_crypto_layer_named(" Mat", "CryptoObject").as_deref(), Some("Mat"));
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

    /// Layers whose names differ only in case stay separate: `Crypto_A` and `crypto_a` each read
    /// their own ranks (lowercase component letters still match), and picking agrees.
    #[test]
    fn crypto_layers_differing_in_case_read_their_own_planes() {
        let mut a = AuxChannels::new(1, 1, 1.0);
        let f = |n: &str| crypto_float(crypto_hash(n));
        for (n, v) in [("Crypto_A00.R", f("upper")), ("Crypto_A00.G", 1.0), ("crypto_a00.r", f("lower")), ("crypto_a00.g", 1.0)] {
            a.channels.push((n.into(), vec![v]));
        }
        assert_eq!(a.crypto_layers(), vec!["Crypto_A".to_string(), "crypto_a".to_string()]);
        assert_eq!(a.resolve_crypto_layer_named("crypto_a", "CryptoObject").as_deref(), Some("crypto_a"));
        assert_eq!(a.crypto_rank_planes("crypto_a")[0].0, &[f("lower")][..]);
        assert_eq!(a.crypto_rank_planes("Crypto_A")[0].0, &[f("upper")][..]);
        assert_eq!(a.crypto_id_at("crypto_a", 0), Some(crypto_hash("lower")));
        assert_eq!(a.crypto_id_at("Crypto_A", 0), Some(crypto_hash("upper")));
        assert!(a.crypto_rank_planes("CRYPTO_A").is_empty());
    }

    /// A names-only set of channels (planes of one sample) named `names`.
    fn named(names: &[&str]) -> AuxChannels {
        let mut a = AuxChannels::new(1, 1, 1.0);
        a.channels = names.iter().map(|n| (n.to_string(), vec![0.0])).collect();
        a
    }

    /// #3271: a multi-part file stores `Crypto_Material00.R` in part `Crypto_Material`; the plane
    /// is shown as `Crypto_Material.Crypto_Material00.R` (as today) and found by its stored name,
    /// its layer by the part's name, and the manifest named after the part. Bare stored names
    /// (`R` of part `Image`) don't stand for the part's channel.
    #[test]
    fn multipart_channels_are_found_by_their_stored_names() {
        let mut a = named(&["Crypto_Material.Crypto_Material00.R", "Crypto_Material.Crypto_Material00.G", "Image.R"]);
        for c in ["R", "G"] {
            a.aliases.push((format!("Crypto_Material00.{c}"), format!("Crypto_Material.Crypto_Material00.{c}")));
        }
        a.manifests = vec![("Crypto_Material".into(), vec![("bunny".into(), 7)])];
        assert_eq!(a.get_index("Crypto_Material00.R"), Some(0));
        assert_eq!(a.get_index("crypto_material00.g"), Some(1), "any case, as full names");
        assert!(a.get("R").is_none(), "a bare name is not an alias");
        assert_eq!(a.crypto_layers(), ["Crypto_Material.Crypto_Material"], "the shown name is unchanged");
        let layer = a.resolve_crypto_layer_named("Crypto_Material", "CryptoObject");
        assert_eq!(layer.as_deref(), Some("Crypto_Material.Crypto_Material"));
        assert_eq!(a.crypto_rank_indices("Crypto_Material.Crypto_Material"), [(0, 1)]);
        assert_eq!(a.crypto_rank_indices("Crypto_Material"), [(0, 1)], "by the stored layer name too");
        assert_eq!(a.manifest_name("Crypto_Material.Crypto_Material", 7), Some("bunny"));
        assert_eq!(crypto_layer_named(&a, "Crypto_Material", 0).as_deref(), Some("Crypto_Material.Crypto_Material"));
    }

    /// #3271: declared lookups resolve against the channel names as the effects resolve them on
    /// the planes; a set holding only some channels keeps listing every Cryptomatte layer.
    #[test]
    fn needs_resolve_as_the_effects_look_up() {
        let all = named(&[
            "A.CryptoAsset00.R",
            "A.CryptoAsset00.G",
            "B.CryptoObject00.R",
            "B.CryptoObject00.G",
            "Image.R",
            "depth.Z",
            "zz.Custom00.R",
            "zz.Custom00.G",
        ]);
        let need = AuxNeed::Lookups(vec![
            AuxLookup::Name(" image.r ".into()),
            AuxLookup::find(&DEPTH_NAMES),
            AuxLookup::Crypto { name: String::new(), index: 3 },
            AuxLookup::Name("missing".into()),
        ]);
        assert_eq!(need.resolve(&all).unwrap(), ["Image.R", "depth.Z", "zz.Custom00.R", "zz.Custom00.G"]);
        assert_eq!(AuxNeed::All.resolve(&all), None);
        assert_eq!(AuxNeed::All.union(need.clone()), AuxNeed::All);
        assert_eq!(need.clone().union(need.clone()), need, "once each");
        // The planes read, with every name listed: the custom slot still names the same layer.
        let mut some = named(&["zz.Custom00.R", "zz.Custom00.G"]);
        some.listed = all.names().iter().map(|n| n.to_string()).collect();
        assert_eq!(some.crypto_layers(), all.crypto_layers());
        assert_eq!(crypto_layer_name(&some, 3), crypto_layer_name(&all, 3));
    }
}
