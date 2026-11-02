use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    NotPsd,
    Truncated,
    Unsupported(String),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotPsd => write!(f, "not a Photoshop document"),
            Error::Truncated => write!(f, "truncated Photoshop document"),
            Error::Unsupported(s) => write!(f, "unsupported Photoshop feature: {s}"),
            Error::Invalid(s) => write!(f, "invalid Photoshop document: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The most pixels one decoded image (the composite, a layer or a mask) may have: 2^28, e.g.
/// 16 384 × 16 384, 4 GiB as float RGBA. Bounds read from a file are untrusted, so a few bytes
/// can't make the reader allocate more than that.
const MAX_DECODE_PIXELS: u64 = 1 << 28;

/// `w × h` as a pixel count, or an error when it exceeds [`MAX_DECODE_PIXELS`].
pub(crate) fn decode_pixels(w: usize, h: usize) -> Result<usize> {
    match (w as u64).checked_mul(h as u64) {
        Some(n) if n <= MAX_DECODE_PIXELS => Ok(n as usize),
        _ => Err(Error::Unsupported(format!("{w}×{h} pixels is too large to decode"))),
    }
}

pub(crate) fn too_large(w: usize, h: usize) -> Error {
    Error::Unsupported(format!("{w}×{h} pixels is too large to decode"))
}

/// What one decode (the composite, or one layer) may hold in memory at once: 4 GiB, 1 GiB where
/// pointers are 32 bits wide (WebAssembly). [`MAX_DECODE_PIXELS`] bounds one image; this bounds
/// the sum of the buffers a decode keeps alive together.
#[cfg(target_pointer_width = "32")]
pub(crate) const DECODE_BUDGET: u64 = 1 << 30;

#[cfg(not(target_pointer_width = "32"))]
pub(crate) const DECODE_BUDGET: u64 = 4 << 30;

/// The bytes the buffers of one decode hold right now, against a limit. A decode charges a buffer
/// before it allocates it (and releases it when it drops it), so a file that would need more
/// than the limit fails with an error instead of an out-of-memory abort.
pub(crate) struct Budget {
    limit: u64,
    live: u64,
}

impl Budget {
    pub(crate) fn new(limit: u64) -> Budget {
        Budget { limit, live: 0 }
    }

    pub(crate) fn charge(&mut self, bytes: u64) -> Result<()> {
        self.check(bytes)?;
        self.live += bytes;
        Ok(())
    }

    /// Whether `bytes` more would still fit, without taking them. A decode checks its whole
    /// expected peak first, so one that cannot fit fails before it has allocated anything.
    pub(crate) fn check(&self, bytes: u64) -> Result<()> {
        match self.live.checked_add(bytes) {
            Some(total) if total <= self.limit => Ok(()),
            _ => {
                const MIB: u64 = 1 << 20;
                Err(Error::Unsupported(format!(
                    "decoding needs {} MiB more with {} MiB in use, over the {} MiB memory budget",
                    bytes / MIB,
                    self.live / MIB,
                    self.limit / MIB
                )))
            }
        }
    }

    pub(crate) fn release(&mut self, bytes: u64) {
        self.live = self.live.saturating_sub(bytes);
    }

    /// An empty vector with room for `n` items, charged.
    pub(crate) fn vec<T>(&mut self, n: usize) -> Result<Vec<T>> {
        self.charge((n as u64).saturating_mul(std::mem::size_of::<T>() as u64))?;
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| Error::Unsupported(format!("out of memory for {n} items")))?;
        Ok(v)
    }

    /// `n` copies of `value`, charged.
    pub(crate) fn filled<T: Clone>(&mut self, n: usize, value: T) -> Result<Vec<T>> {
        let mut v = self.vec::<T>(n)?;
        v.resize(n, value);
        Ok(v)
    }
}

/// Inflate a zlib stream to at most `limit` bytes.
pub(crate) fn inflate(src: &[u8], limit: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(src, limit).map_err(|e| Error::Invalid(format!("zip: {e:?}")))
}

/// Give back what a channel buffer from `Psd::channel_bytes` / `Psd::decode_plane` was charged:
/// a borrowed one (uncompressed data, or a plane of an inflated stream charged as a whole) was not.
pub(crate) fn release_bytes(budget: &mut Budget, raw: Cow<'_, [u8]>) {
    if let Cow::Owned(v) = raw {
        budget.release(v.len() as u64);
    }
}

/// A 32-bit float sample made safe to use: NaN reads as 0, +inf as white (1.0, like the integer
/// depths' maximum), -inf as 0.
pub(crate) fn finite_sample(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else if v == f32::INFINITY {
        1.0
    } else if v == f32::NEG_INFINITY {
        0.0
    } else {
        v
    }
}
