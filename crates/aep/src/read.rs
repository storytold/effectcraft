//! Parsing the `RIFX` container into an [`AepFile`].

use std::borrow::Cow;

use crate::chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS};
use crate::error::AepError;

/// Bounds for hostile input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Deepest list nesting that is parsed (the top level is depth 1). A list or nested chunk
    /// whose children would be deeper is kept opaque instead.
    pub max_depth: usize,
    /// Most chunks a file may contain (counted across all parse attempts) before parsing fails.
    pub max_chunks: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_depth: 64, max_chunks: 10_000_000 }
    }
}

/// Parses an `.aep` with the default [`Limits`].
///
/// # Errors
/// Returns an error when the input is not a well-formed `RIFX`/`Egg!` container at the top level.
/// Odd bodies *inside* the container (a `LIST` whose content is not a chunk sequence) do not
/// fail the parse: they are kept opaque.
pub fn parse(data: &[u8]) -> Result<AepFile<'_>, AepError> {
    parse_with_limits(data, Limits::default())
}

/// Parses an `.aep` with explicit [`Limits`].
///
/// # Errors
/// See [`parse`]; also fails with [`AepError::TooManyChunks`].
pub fn parse_with_limits(data: &[u8], limits: Limits) -> Result<AepFile<'_>, AepError> {
    if id4(data, 0) != Some(*b"RIFX") {
        return Err(AepError::NotRifx);
    }
    let declared = u64::from(be32(data, 4).ok_or(AepError::NotRifx)?);
    let form = id4(data, 8).ok_or(AepError::NotRifx)?;
    if &form != b"Egg!" {
        return Err(AepError::BadFormType(form));
    }
    let available = (data.len() as u64).saturating_sub(8);
    if declared < 4 || declared > available {
        return Err(AepError::Truncated { declared, available });
    }
    // `declared <= available < data.len()`, so the end is inside the input.
    let end = usize::try_from(declared).map_err(|_| AepError::TooLarge)?.checked_add(8).ok_or(AepError::TooLarge)?;
    let mut parser = Parser { data, limits, count: 0 };
    let chunks = parser.sequence(12, end, 1)?;
    let trailer = data.get(end..).unwrap_or(&[]);
    Ok(AepFile { chunks, trailer: Cow::Borrowed(trailer) })
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    id4(data, at).map(u32::from_be_bytes)
}

fn id4(data: &[u8], at: usize) -> Option<[u8; 4]> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some([*b.first()?, *b.get(1)?, *b.get(2)?, *b.get(3)?])
}

struct Parser<'a> {
    data: &'a [u8],
    limits: Limits,
    count: usize,
}

impl<'a> Parser<'a> {
    /// Parses the sibling chunks that exactly fill `start..end`.
    fn sequence(&mut self, start: usize, end: usize, depth: usize) -> Result<Vec<Chunk<'a>>, AepError> {
        let mut out = Vec::new();
        let mut pos = start;
        while pos < end {
            let short = AepError::Malformed { offset: pos, what: "bytes left over that are too short for a chunk header" };
            let body_start = pos.checked_add(8).filter(|b| *b <= end).ok_or(short)?;
            let id = id4(self.data, pos).ok_or(AepError::Malformed { offset: pos, what: "short read" })?;
            let size = be32(self.data, pos + 4).ok_or(AepError::Malformed { offset: pos, what: "short read" })? as usize;
            let avail = end - body_start;
            if size > avail {
                return Err(AepError::ChunkTooLarge { offset: pos, size: size as u64, available: avail as u64 });
            }
            self.count = self.count.saturating_add(1);
            if self.count > self.limits.max_chunks {
                return Err(AepError::TooManyChunks);
            }
            let body_end = body_start + size;
            let body = self.body(id, body_start, body_end, depth)?;
            let mut next = body_end;
            let mut pad = None;
            if size % 2 == 1 && next < end {
                pad = Some(self.data.get(next).copied().ok_or(AepError::Malformed { offset: next, what: "missing pad byte" })?);
                next += 1;
            }
            out.push(Chunk { id, body, pad });
            pos = next;
        }
        Ok(out)
    }

    /// Like [`Self::sequence`] but a structural problem yields `None` (the caller keeps bytes).
    fn tentative(&mut self, start: usize, end: usize, depth: usize) -> Result<Option<Vec<Chunk<'a>>>, AepError> {
        match self.sequence(start, end, depth) {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.is_structural() => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn slice(&self, start: usize, end: usize) -> &'a [u8] {
        self.data.get(start..end).unwrap_or(&[])
    }

    fn body(&mut self, id: [u8; 4], start: usize, end: usize, depth: usize) -> Result<Body<'a>, AepError> {
        let len = end - start;
        if id == LIST && len >= 4 {
            let list_type = id4(self.data, start).ok_or(AepError::Malformed { offset: start, what: "short read" })?;
            let opaque = |p: &Self| Body::OpaqueList { list_type, data: Cow::Borrowed(p.slice(start + 4, end)) };
            if depth >= self.limits.max_depth {
                return Ok(opaque(self));
            }
            return Ok(match self.tentative(start + 4, end, depth + 1)? {
                Some(children) => Body::List { list_type, children },
                None => opaque(self),
            });
        }
        if NESTED_IDS.contains(&id)
            && len >= 8
            && depth < self.limits.max_depth
            && let Some(children) = self.tentative(start, end, depth + 1)?
            && !children.is_empty()
        {
            return Ok(Body::Chunks(children));
        }
        Ok(Body::Data(Cow::Borrowed(self.slice(start, end))))
    }
}
