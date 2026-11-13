//! Writing an [`AepFile`] back to bytes.

use std::io::Write;

use crate::container::chunk::{AepFile, Body, Chunk};
use crate::container::error::AepError;

/// Deepest tree [`write`] accepts (a hand-built tree may be deeper than any parsed one).
const MAX_WRITE_DEPTH: usize = 256;

/// Serialises `file`: `RIFX`, the recomputed container size, `Egg!`, the chunks and the trailer.
/// Sizes of the container and of every chunk are recomputed from the tree; an unmodified parsed
/// file comes back byte for byte.
///
/// # Errors
/// [`AepError::TooLarge`] when a chunk or the container exceeds the 32-bit size field,
/// [`AepError::TooDeep`] for an absurdly deep hand-built tree.
pub fn write(file: &AepFile<'_>) -> Result<Vec<u8>, AepError> {
    let mut out = Vec::new();
    // Sized once to the exact output. A tree past the 32-bit container size fails at once (the
    // same error the write below would give after serialising it). `try_reserve` so a hand-built
    // tree that repeats one large borrowed body many times cannot abort on the allocation; the
    // writer then grows as it goes, as before.
    if let Some(len) = sequence_len(&file.chunks, 1) {
        if u32::try_from(len.saturating_add(4)).is_err() {
            return Err(AepError::TooLarge);
        }
        if let Some(total) = len.checked_add(12).and_then(|n| n.checked_add(file.trailer.len())) {
            let _ = out.try_reserve_exact(total);
        }
    }
    out.extend_from_slice(b"RIFX");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"Egg!");
    write_sequence(&mut out, &file.chunks, 1)?;
    let size = u32::try_from(out.len().saturating_sub(8)).map_err(|_| AepError::TooLarge)?;
    patch_u32(&mut out, 4, size)?;
    out.extend_from_slice(&file.trailer);
    Ok(out)
}

/// Like [`write`], into any writer.
///
/// # Errors
/// A size or depth error of [`write`] becomes `ErrorKind::InvalidData`; otherwise I/O errors.
pub fn write_to(file: &AepFile<'_>, w: &mut impl Write) -> std::io::Result<()> {
    let bytes = write(file).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    w.write_all(&bytes)
}

fn patch_u32(out: &mut [u8], at: usize, v: u32) -> Result<(), AepError> {
    let end = at.checked_add(4).ok_or(AepError::TooLarge)?;
    let dst = out.get_mut(at..end).ok_or(AepError::TooLarge)?;
    for (d, s) in dst.iter_mut().zip(v.to_be_bytes()) {
        *d = s;
    }
    Ok(())
}

fn write_sequence(out: &mut Vec<u8>, chunks: &[Chunk<'_>], depth: usize) -> Result<(), AepError> {
    if depth > MAX_WRITE_DEPTH {
        return Err(AepError::TooDeep);
    }
    let count = chunks.len();
    for (i, chunk) in chunks.iter().enumerate() {
        write_chunk(out, chunk, i + 1 == count, depth)?;
    }
    Ok(())
}

/// The number of bytes [`write_sequence`] produces for `chunks`; `None` past the depth limit or
/// on overflow (both make the write fail).
fn sequence_len(chunks: &[Chunk<'_>], depth: usize) -> Option<usize> {
    if depth > MAX_WRITE_DEPTH {
        return None;
    }
    let count = chunks.len();
    let mut total = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        let body = match &chunk.body {
            Body::Data(d) => d.len(),
            Body::Chunks(c) => sequence_len(c, depth + 1)?,
            Body::List { children, .. } => sequence_len(children, depth + 1)?.checked_add(4)?,
            Body::OpaqueList { data, .. } => data.len().checked_add(4)?,
        };
        // The same pad rule as `write_chunk`.
        let pad = usize::from(body % 2 == 1 && (chunk.pad.is_some() || i + 1 != count));
        total = total.checked_add(8)?.checked_add(body)?.checked_add(pad)?;
    }
    Some(total)
}

fn write_chunk(out: &mut Vec<u8>, chunk: &Chunk<'_>, is_last: bool, depth: usize) -> Result<(), AepError> {
    out.extend_from_slice(&chunk.id);
    let size_at = out.len();
    out.extend_from_slice(&[0; 4]);
    let body_start = out.len();
    match &chunk.body {
        Body::Data(d) => out.extend_from_slice(d),
        Body::Chunks(c) => write_sequence(out, c, depth + 1)?,
        Body::List { list_type, children } => {
            out.extend_from_slice(list_type);
            write_sequence(out, children, depth + 1)?;
        }
        Body::OpaqueList { list_type, data } => {
            out.extend_from_slice(list_type);
            out.extend_from_slice(data);
        }
    }
    let len = out.len().saturating_sub(body_start);
    patch_u32(out, size_at, u32::try_from(len).map_err(|_| AepError::TooLarge)?)?;
    if len % 2 == 1 {
        match chunk.pad {
            Some(b) => out.push(b),
            None if !is_last => out.push(0),
            None => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    /// The output is sized once to its exact length.
    #[test]
    fn the_output_is_sized_once_to_its_exact_length() {
        let odd = |n: u8| Chunk::data(*b"cdat", vec![n; usize::from(n)]);
        let mut padded = odd(3);
        padded.pad = Some(0x7f);
        let trees = [
            AepFile { chunks: Vec::new(), trailer: Cow::Borrowed(&[]) },
            AepFile {
                chunks: vec![
                    odd(1),
                    Chunk::list(*b"Fold", vec![odd(2), odd(3), Chunk::list(*b"tdgp", vec![odd(5)])]),
                    Chunk { id: *b"LIST", body: Body::OpaqueList { list_type: *b"btdk", data: Cow::Owned(vec![1, 2, 3]) }, pad: None },
                    Chunk { id: *b"tdmn", body: Body::Chunks(vec![odd(7), padded]), pad: None },
                    odd(9),
                ],
                trailer: Cow::Borrowed(b"<x:xmpmeta/>"),
            },
        ];
        for file in &trees {
            let out = write(file).expect("write");
            assert_eq!(sequence_len(&file.chunks, 1).map(|n| n + 12 + file.trailer.len()), Some(out.len()));
            // Exact with std's allocator today; grown push by push it would be rounded up.
            assert_eq!(out.capacity(), out.len(), "the output grew past its size");
        }
    }

    /// A tree past the container size fails before serialising.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_tree_past_the_container_size_fails_before_serialising() {
        // 4097 chunks borrowing one 1 MiB body: over 4 GiB of output, none of it written.
        let body = vec![0u8; 1 << 20];
        let chunks: Vec<Chunk<'_>> = (0..4097).map(|_| Chunk::data(*b"cdat", &body[..])).collect();
        let file = AepFile { chunks, trailer: Cow::Borrowed(&[]) };
        assert_eq!(write(&file), Err(AepError::TooLarge));
        // Just under the limit still sizes, and fails nothing up front.
        assert!(sequence_len(&file.chunks[..4000], 1).is_some_and(|n| u32::try_from(n + 4).is_ok()));
    }

    /// A tree past the depth limit has no size.
    #[test]
    fn a_tree_past_the_depth_limit_has_no_size() {
        let mut deep = Chunk::data(*b"cdat", vec![0]);
        for _ in 0..=MAX_WRITE_DEPTH {
            deep = Chunk::list(*b"tdgp", vec![deep]);
        }
        let file = AepFile { chunks: vec![deep], trailer: Cow::Borrowed(&[]) };
        assert_eq!(sequence_len(&file.chunks, 1), None);
        assert_eq!(write(&file), Err(AepError::TooDeep));
    }
}
