//! Parsing the `RIFX` container into an [`AepFile`].

use std::borrow::Cow;

use crate::container::bytes::{array, be_u32};
use crate::container::chunk::{AepFile, Body, Chunk, LIST, NESTED_IDS};
use crate::container::error::AepError;

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
    let declared = u64::from(be_u32(data, 4).ok_or(AepError::NotRifx)?);
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

fn id4(data: &[u8], at: usize) -> Option<[u8; 4]> {
    array::<4>(data, at)
}

struct Parser<'a> {
    data: &'a [u8],
    limits: Limits,
    count: usize,
}

impl<'a> Parser<'a> {
    /// Parses the sibling chunks that exactly fill `start..end`.
    fn sequence(&mut self, start: usize, end: usize, depth: usize) -> Result<Vec<Chunk<'a>>, AepError> {
        // Sized once: a well-formed sequence fills exactly this many slots. The count is bounded by
        // the input (one per 8 bytes at most) and by the chunks the limit still allows.
        let capacity = self.count_siblings(start, end).min(self.limits.max_chunks.saturating_sub(self.count));
        let mut out = Vec::with_capacity(capacity);
        let mut pos = start;
        while pos < end {
            let short = AepError::Malformed { offset: pos, what: "bytes left over that are too short for a chunk header" };
            let body_start = pos.checked_add(8).filter(|b| *b <= end).ok_or(short)?;
            let id = id4(self.data, pos).ok_or(AepError::Malformed { offset: pos, what: "short read" })?;
            let size = be_u32(self.data, pos + 4).ok_or(AepError::Malformed { offset: pos, what: "short read" })? as usize;
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

    /// The number of chunk headers that tile `start..end` the way [`Self::sequence`] walks them,
    /// stopping at the first that does not fit. Never more than `(end - start) / 8`.
    fn count_siblings(&self, start: usize, end: usize) -> usize {
        let mut n = 0usize;
        let mut pos = start;
        while let Some(body_start) = pos.checked_add(8).filter(|b| *b <= end) {
            let Some(size) = be_u32(self.data, pos.saturating_add(4)) else { break };
            let Some(next) = usize::try_from(size).ok().and_then(|s| body_start.checked_add(s)).filter(|e| *e <= end) else { break };
            n = n.saturating_add(1);
            pos = if size % 2 == 1 && next < end { next.saturating_add(1) } else { next };
        }
        n
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every children `Vec` in the tree, depth first.
    fn child_vecs<'f, 'a>(chunks: &'f [Chunk<'a>], out: &mut Vec<&'f Vec<Chunk<'a>>>) {
        for c in chunks {
            match &c.body {
                Body::List { children, .. } | Body::Chunks(children) => {
                    out.push(children);
                    child_vecs(children, out);
                }
                Body::Data(_) | Body::OpaqueList { .. } => {}
            }
        }
    }

    #[test]
    fn every_sibling_list_is_allocated_once_at_its_exact_length() {
        let inner: Vec<Chunk<'static>> = (0..37u8).map(|i| Chunk::data(*b"cdat", vec![i; usize::from(i % 5)])).collect();
        let middle: Vec<Chunk<'static>> = (0..9).map(|_| Chunk::list(*b"tdbs", inner.clone())).collect();
        let file = AepFile { chunks: vec![Chunk::data(*b"head", vec![0; 3]), Chunk::list(*b"Fold", middle)], trailer: Cow::Borrowed(&[]) };
        let bytes = crate::container::write(&file).expect("write");
        let parsed = parse(&bytes).expect("parse");
        assert_eq!(crate::container::write(&parsed).expect("rewrite"), bytes);
        let mut vecs = vec![&parsed.chunks];
        child_vecs(&parsed.chunks, &mut vecs);
        assert_eq!(vecs.len(), 11);
        for v in vecs {
            assert_eq!(v.capacity(), v.len(), "a sibling Vec grew past its length");
        }
    }

    #[test]
    fn the_preallocation_is_bounded_by_the_input_and_the_chunk_limit() {
        // 1000 empty headers: the most chunks the bytes can hold.
        let mut data = b"RIFX\0\0\0\0Egg!".to_vec();
        data.extend(std::iter::repeat_n(*b"abcd\0\0\0\0", 1000).flatten());
        let declared = u32::try_from(data.len() - 8).expect("small");
        data[4..8].copy_from_slice(&declared.to_be_bytes());
        let parser = Parser { data: &data, limits: Limits::default(), count: 0 };
        assert_eq!(parser.count_siblings(12, data.len()), 1000);
        assert!(parser.count_siblings(12, data.len()) <= (data.len() - 12) / 8);
        // A header whose size runs past the end stops the count; a stray tail is not counted.
        assert_eq!(parser.count_siblings(12, data.len() - 3), 999);
        let mut big = data.clone();
        big[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let parser = Parser { data: &big, limits: Limits::default(), count: 0 };
        assert_eq!(parser.count_siblings(12, big.len()), 0);
        let f = parse(&data).expect("parse");
        assert_eq!((f.chunks.len(), f.chunks.capacity()), (1000, 1000));
        // Under a tighter limit the parse fails as before, without sizing for more than allowed.
        let tight = Limits { max_depth: 64, max_chunks: 10 };
        assert_eq!(parse_with_limits(&data, tight), Err(AepError::TooManyChunks));
    }
}
