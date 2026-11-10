//! Errors of the container reader and writer.

use std::fmt;

/// Why a file could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AepError {
    /// The input is shorter than the 12-byte `RIFX` header, or does not start with `RIFX`.
    NotRifx,
    /// The form type after the declared size is not `Egg!`.
    BadFormType([u8; 4]),
    /// The declared container size runs past the end of the input.
    Truncated { declared: u64, available: u64 },
    /// A chunk declares more body bytes than remain in its parent (`offset` is the chunk header).
    ChunkTooLarge { offset: usize, size: u64, available: u64 },
    /// The chunk structure is not well formed at `offset` (what is wrong is in `what`).
    Malformed { offset: usize, what: &'static str },
    /// Nesting is deeper than the allowed maximum.
    TooDeep,
    /// More chunks than the allowed maximum.
    TooManyChunks,
    /// A chunk or the whole file would not fit the 32-bit size fields when writing.
    TooLarge,
    /// An `.aepx` is not well-formed XML of the subset After Effects writes, at byte `offset`
    /// (what is wrong is in `what`).
    Xml { offset: usize, what: &'static str },
}

impl AepError {
    /// Structural problems of the input that a lenient caller may repair by keeping a body opaque;
    /// limits (`TooDeep`, `TooManyChunks`) are never repaired.
    pub(crate) fn is_structural(&self) -> bool {
        matches!(self, Self::ChunkTooLarge { .. } | Self::Malformed { .. } | Self::Truncated { .. })
    }
}

impl fmt::Display for AepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRifx => write!(f, "not an After Effects project: no RIFX header"),
            Self::BadFormType(t) => write!(f, "RIFX form type {:?} is not \"Egg!\"", String::from_utf8_lossy(t)),
            Self::Truncated { declared, available } => {
                write!(f, "truncated file: the container declares {declared} bytes, {available} are available")
            }
            Self::ChunkTooLarge { offset, size, available } => {
                write!(f, "chunk at offset {offset} declares {size} bytes, only {available} remain")
            }
            Self::Malformed { offset, what } => write!(f, "malformed chunk structure at offset {offset}: {what}"),
            Self::TooDeep => write!(f, "chunks are nested too deeply"),
            Self::TooManyChunks => write!(f, "too many chunks"),
            Self::TooLarge => write!(f, "chunk too large for the 32-bit size field"),
            Self::Xml { offset, what } => write!(f, "not a readable After Effects XML project (.aepx): {what} at byte {offset}"),
        }
    }
}

impl std::error::Error for AepError {}
