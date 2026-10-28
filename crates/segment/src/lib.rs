//! Trained models for Roto Brush and face tracking, each kind behind one swappable interface.
//!
//! - [`MaskModel`]: what Roto Brush asks of a model, the foreground probability of each pixel of a
//!   frame given prompts (foreground / background points from the strokes, a box, a prior mask).
//!   Anything implementing it can be plugged in; the classical graph-cut segmenter in
//!   `effectcraft-track` stays the built-in fallback when no model is chosen.
//! - [`face::FaceModel`]: what face tracking asks of a model: find a face in a region, follow it
//!   from frame to frame. The classical face tracker stays the built-in fallback.
//! - [`MODELS`]: the registry. Every entry is open source under a licence compatible with
//!   EffectCraft's (MIT OR Apache-2.0), with its authors, source, size and SHA-256. Weights are
//!   never bundled: they are fetched on demand (or installed from a file), verified ([`sha256`])
//!   and loaded by [`load`].
//! - [`pt`]: a PyTorch checkpoint reader; [`tflite`]: a TensorFlow Lite reader and interpreter;
//!   so official weights load as published.
//! - [`mobilesam`]: MobileSAM (Apache-2.0) and [`mediapipe`]: MediaPipe Face Landmarker
//!   (Apache-2.0), in plain Rust ([`nn`] kernels on rayon).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod face;
pub mod mediapipe;
pub mod mobilesam;
pub mod nn;
pub mod pt;
pub mod tflite;

use std::sync::Arc;

pub type Result<T> = std::result::Result<T, String>;

/// What a model is asked: coordinates in frame pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    /// Points and whether each is foreground (`true`) or background.
    pub points: Vec<([f32; 2], bool)>,
    /// A box around the object: `[x0, y0, x1, y1]`.
    pub bbox: Option<[f32; 4]>,
    /// A prior: the foreground probability of every pixel (`w×h`), e.g. the previous frame's
    /// matte carried forward.
    pub mask: Option<Vec<f32>>,
}

/// A trained segmentation model (swappable: Roto Brush only uses this interface).
pub trait MaskModel: Send + Sync {
    fn info(&self) -> &'static ModelInfo;
    /// Foreground probability (0–1) of each pixel of a frame (straight RGB 0–1, row-major `w×h`).
    fn segment(&self, rgb: &[[f32; 3]], w: usize, h: usize, prompt: &Prompt) -> Result<Vec<f32>>;
}

/// What a model is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    /// Roto Brush ([`MaskModel`]).
    Mask,
    /// Face tracking ([`face::FaceModel`]).
    Face,
}

/// A model in the registry: what it is, who made it, its licence and where its weights come from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub task: Task,
    pub name: &'static str,
    pub description: &'static str,
    /// Who made it (shown with the model, and in ATTRIBUTION.md).
    pub authors: &'static str,
    /// SPDX licence of the weights and the architecture.
    pub licence: &'static str,
    pub licence_url: &'static str,
    /// The project's page.
    pub homepage: &'static str,
    /// The official weights file.
    pub url: &'static str,
    pub file_name: &'static str,
    pub size: u64,
    /// SHA-256 of the weights file (lower-case hex).
    pub sha256: &'static str,
}

/// Licences a model may have to be listed (open source, compatible with MIT OR Apache-2.0).
pub const ALLOWED_LICENCES: &[&str] = &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "CC0-1.0"];

pub const MOBILE_SAM: ModelInfo = ModelInfo {
    id: "mobilesam",
    task: Task::Mask,
    name: "MobileSAM",
    description: "Segment Anything distilled to a 5M-parameter TinyViT encoder (C. Zhang et al., 2023). Turns your strokes into a clean matte and tidies every propagated frame.",
    authors: "Chaoning Zhang et al. (MobileSAM), building on Meta AI's Segment Anything and Microsoft's TinyViT",
    licence: "Apache-2.0",
    licence_url: "https://github.com/ChaoningZhang/MobileSAM/blob/master/LICENSE",
    homepage: "https://github.com/ChaoningZhang/MobileSAM",
    url: "https://raw.githubusercontent.com/ChaoningZhang/MobileSAM/master/weights/mobile_sam.pt",
    file_name: "mobile_sam.pt",
    size: 40_728_226,
    sha256: "6dbb90523a35330fedd7f1d3dfc66f995213d81b29a5ca8108dbcdd4e37d6c2f",
};

pub const FACE_LANDMARKER: ModelInfo = ModelInfo {
    id: "mediapipe-face",
    task: Task::Face,
    name: "MediaPipe Face Landmarker",
    description: "Google's BlazeFace detector and 478-point Face Mesh V2. Follows faces turned to the side, partly covered or in difficult light, where the classic tracker loses them.",
    authors: "Google LLC (MediaPipe)",
    licence: "Apache-2.0",
    licence_url: "https://storage.googleapis.com/mediapipe-assets/Model%20Card%20MediaPipe%20Face%20Mesh%20V2.pdf",
    homepage: "https://developers.google.com/edge/mediapipe/solutions/vision/face_landmarker",
    url: "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task",
    file_name: "face_landmarker.task",
    size: 3_758_596,
    sha256: "64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
};

/// The trained models, besides the built-in classical engines.
pub const MODELS: &[ModelInfo] = &[MOBILE_SAM, FACE_LANDMARKER];

/// The id of the built-in classical engines (graph cut segmenter, shape-model face tracker): no
/// weights, always available.
pub const CLASSICAL: &str = "classical";

pub fn info(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

/// The models for `task`.
pub fn models(task: Task) -> impl Iterator<Item = &'static ModelInfo> {
    MODELS.iter().filter(move |m| m.task == task)
}

/// A loaded model.
#[derive(Clone)]
pub enum Loaded {
    Mask(Arc<dyn MaskModel>),
    Face(Arc<dyn face::FaceModel>),
}

/// Check a weights file against the registry and build the model.
pub fn load(id: &str, bytes: &[u8]) -> Result<Loaded> {
    let info = info(id).ok_or_else(|| format!("unknown model `{id}`"))?;
    if !ALLOWED_LICENCES.contains(&info.licence) {
        return Err(format!("{}: licence {} is not allowed", info.name, info.licence));
    }
    let got = sha256::hex(bytes);
    if got != info.sha256 {
        return Err(format!("{}: the file does not match (SHA-256 {got}, expected {})", info.name, info.sha256));
    }
    match id {
        "mobilesam" => {
            let w = pt::read(bytes)?;
            Ok(Loaded::Mask(Arc::new(Sam { net: mobilesam::MobileSam::from_weights(&w)? })))
        }
        "mediapipe-face" => Ok(Loaded::Face(Arc::new(mediapipe::FaceLandmarker::from_task(bytes)?))),
        _ => Err(format!("no loader for `{id}`")),
    }
}

/// The notice kept next to installed weights: what they are, who made them, their licence.
pub fn notice(m: &ModelInfo) -> String {
    format!(
        "{}\n\nAuthors: {}\nLicence: {} (full text: https://spdx.org/licenses/{}.html; as stated by the authors: {})\nSource: {}\nProject: {}\n\nDownloaded unmodified from the source above. EffectCraft runs it with its own implementation\nand is not affiliated with or endorsed by its authors.\n",
        m.name, m.authors, m.licence, m.licence, m.licence_url, m.url, m.homepage
    )
}

struct Sam {
    net: mobilesam::MobileSam,
}

impl MaskModel for Sam {
    fn info(&self) -> &'static ModelInfo {
        &MOBILE_SAM
    }
    fn segment(&self, rgb: &[[f32; 3]], w: usize, h: usize, prompt: &Prompt) -> Result<Vec<f32>> {
        let e = self.net.embed(rgb, w, h)?;
        Ok(self.net.predict(&e, prompt).0)
    }
}

/// SHA-256 (FIPS 180-4), to verify downloaded weights without another dependency.
pub mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    pub fn digest(data: &[u8]) -> [u8; 32] {
        let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let bits = (data.len() as u64).wrapping_mul(8);
        let mut tail = data[data.len() - data.len() % 64..].to_vec();
        tail.push(0x80);
        while tail.len() % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for block in data[..data.len() - data.len() % 64].as_chunks::<64>().0.iter().chain(tail.as_chunks::<64>().0.iter()) {
            let mut w = [0u32; 64];
            for (i, c) in block.as_chunks::<4>().0.iter().enumerate() {
                w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *x = x.wrapping_add(y);
            }
        }
        let mut out = [0u8; 32];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    pub fn hex(data: &[u8]) -> String {
        digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn known_vectors() {
            assert_eq!(super::hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
            assert_eq!(super::hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
            let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
            assert_eq!(super::hex(long), "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
            assert_eq!(super::hex(&vec![b'a'; 1_000_000]), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_open_source_and_verifies_files() {
        for m in MODELS {
            assert!(ALLOWED_LICENCES.contains(&m.licence), "{}", m.id);
            assert_eq!(m.sha256.len(), 64);
            assert!(m.url.starts_with("https://") && m.licence_url.starts_with("https://") && m.homepage.starts_with("https://"));
            assert!(!m.authors.is_empty() && notice(m).contains(m.authors));
        }
        assert_eq!(models(Task::Face).map(|m| m.id).collect::<Vec<_>>(), ["mediapipe-face"]);
        assert!(load("mediapipe-face", b"nope").err().is_some_and(|e| e.contains("does not match")));
        // A file that isn't the published one is refused before anything is parsed.
        assert!(load("mobilesam", b"not the weights").err().is_some_and(|e| e.contains("does not match")));
        assert!(load("nope", b"").is_err());
    }
}
