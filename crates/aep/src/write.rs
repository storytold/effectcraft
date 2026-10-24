//! Writing an [`AepFile`] back to bytes.

use std::io::Write;

use crate::chunk::{AepFile, Body, Chunk};
use crate::error::AepError;

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
