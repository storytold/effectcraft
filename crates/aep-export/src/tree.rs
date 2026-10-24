//! Small helpers to edit a chunk tree without panicking: every accessor returns a `Result` with
//! [`crate::AepExportError::Template`] when the template does not have the expected shape.

use effectcraft_aep::{Body, Chunk};

use crate::AepExportError;

pub(crate) type R<T> = Result<T, AepExportError>;

pub(crate) fn tpl(msg: &'static str) -> AepExportError {
    AepExportError::Template(msg)
}

/// The bytes of a data chunk, owned and mutable.
pub(crate) fn data_mut<'a>(c: &'a mut Chunk<'static>) -> R<&'a mut Vec<u8>> {
    match &mut c.body {
        Body::Data(d) => Ok(d.to_mut()),
        _ => Err(tpl("expected a data chunk")),
    }
}

/// The child vector of a list or nested chunk.
pub(crate) fn kids_mut<'a>(c: &'a mut Chunk<'static>) -> R<&'a mut Vec<Chunk<'static>>> {
    match &mut c.body {
        Body::List { children, .. } | Body::Chunks(children) => Ok(children),
        _ => Err(tpl("expected a list chunk")),
    }
}

pub(crate) fn child_mut<'a>(c: &'a mut Chunk<'static>, id: &[u8; 4]) -> R<&'a mut Chunk<'static>> {
    kids_mut(c)?.iter_mut().find(|k| k.is(id)).ok_or_else(|| tpl("missing child chunk"))
}

pub(crate) fn list_mut<'a>(c: &'a mut Chunk<'static>, list_type: &[u8; 4]) -> R<&'a mut Chunk<'static>> {
    kids_mut(c)?.iter_mut().find(|k| k.is_list(list_type)).ok_or_else(|| tpl("missing child list"))
}

/// Mutable bytes of the data child `id`.
pub(crate) fn child_data<'a>(c: &'a mut Chunk<'static>, id: &[u8; 4]) -> R<&'a mut Vec<u8>> {
    data_mut(child_mut(c, id)?)
}

pub(crate) fn put(d: &mut [u8], at: usize, bytes: &[u8]) -> R<()> {
    let end = at.checked_add(bytes.len()).ok_or_else(|| tpl("offset overflow"))?;
    let dst = d.get_mut(at..end).ok_or_else(|| tpl("chunk shorter than its layout"))?;
    dst.copy_from_slice(bytes);
    Ok(())
}

pub(crate) fn put_u8(d: &mut [u8], at: usize, v: u8) -> R<()> {
    put(d, at, &[v])
}
pub(crate) fn put_u16(d: &mut [u8], at: usize, v: u16) -> R<()> {
    put(d, at, &v.to_be_bytes())
}
pub(crate) fn put_u32(d: &mut [u8], at: usize, v: u32) -> R<()> {
    put(d, at, &v.to_be_bytes())
}
pub(crate) fn put_i32(d: &mut [u8], at: usize, v: i32) -> R<()> {
    put(d, at, &v.to_be_bytes())
}
pub(crate) fn put_f32(d: &mut [u8], at: usize, v: f32) -> R<()> {
    put(d, at, &v.to_be_bytes())
}
pub(crate) fn put_f64(d: &mut [u8], at: usize, v: f64) -> R<()> {
    put(d, at, &v.to_be_bytes())
}

/// Sets or clears the bits `mask` of byte `at`.
pub(crate) fn put_bits(d: &mut [u8], at: usize, mask: u8, on: bool) -> R<()> {
    let b = d.get_mut(at).ok_or_else(|| tpl("chunk shorter than its layout"))?;
    if on {
        *b |= mask;
    } else {
        *b &= !mask;
    }
    Ok(())
}

/// Writes `text` NUL padded into `d[at..at + len]` (cut at a character boundary so that at least
/// one NUL remains).
pub(crate) fn put_cstr(d: &mut [u8], at: usize, len: usize, text: &str) -> R<()> {
    let end = at.checked_add(len).ok_or_else(|| tpl("offset overflow"))?;
    let dst = d.get_mut(at..end).ok_or_else(|| tpl("chunk shorter than its layout"))?;
    dst.fill(0);
    let mut n = 0;
    for ch in text.chars() {
        let l = ch.len_utf8();
        if n + l >= len {
            break;
        }
        let mut buf = [0u8; 4];
        let enc = ch.encode_utf8(&mut buf);
        for (i, b) in enc.as_bytes().iter().enumerate() {
            if let Some(slot) = dst.get_mut(n + i) {
                *slot = *b;
            }
        }
        n += l;
    }
    Ok(())
}

/// Replaces the `Utf8` child of an item or layer with `text`.
pub(crate) fn set_utf8(c: &mut Chunk<'static>, text: &str) -> R<()> {
    let k = child_mut(c, b"Utf8")?;
    *k = Chunk::data(*b"Utf8", text.as_bytes().to_vec());
    Ok(())
}
