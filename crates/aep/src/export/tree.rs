//! Small helpers to edit a chunk tree without panicking: every accessor returns a `Result` with
//! [`crate::export::AepExportError::Internal`] when a prototype does not have the expected shape.

use crate::{Body, Chunk};

use crate::export::AepExportError;

pub(crate) type R<T> = Result<T, AepExportError>;

pub(crate) fn tpl(msg: &'static str) -> AepExportError {
    AepExportError::Internal(msg)
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

/// The one byte writer of the exporter: `bytes` at `at`, or an error when they do not fit.
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
        put(dst, n, ch.encode_utf8(&mut buf).as_bytes())?;
        n += l;
    }
    Ok(())
}

thread_local! {
    /// How many fixed layouts on this thread did not fit their buffer (see [`Fixed`]); the tests
    /// keep it at zero.
    static FIXED_LAYOUT_ERRORS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A buffer whose layout is fixed in code (constant size and offsets, no input from a project).
/// Every write goes through [`put`]; a write that does not fit is skipped and its error kept, the
/// others are written. Such an error is a bug in the layout, never something a project can cause:
/// the tests build every one of these layouts and require no error. The builders keep their
/// infallible signatures because files outside this module call them without a `Result`; if a
/// layout ever broke, [`Fixed::finish`] logs it once and returns what fit.
pub(crate) struct Fixed {
    bytes: Vec<u8>,
    error: Option<AepExportError>,
}

impl Fixed {
    /// `len` zero bytes.
    pub(crate) fn zeroed(len: usize) -> Fixed {
        Fixed { bytes: vec![0u8; len], error: None }
    }

    /// Writes `bytes` at `at`, or keeps the error (the first one) when they do not fit.
    pub(crate) fn put(&mut self, at: usize, bytes: &[u8]) {
        self.apply(|d| put(d, at, bytes));
    }

    /// Runs a writer on the buffer, keeping its error (the first one).
    pub(crate) fn apply(&mut self, write: impl FnOnce(&mut [u8]) -> R<()>) {
        if let Err(e) = write(&mut self.bytes) {
            self.error.get_or_insert(e);
        }
    }

    /// The error of the first write that did not fit (for the tests).
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<&AepExportError> {
        self.error.as_ref()
    }

    /// The bytes; a write that did not fit is logged once, with what `layout` says.
    pub(crate) fn finish(self, layout: impl FnOnce() -> String) -> Vec<u8> {
        if let Some(e) = self.error {
            FIXED_LAYOUT_ERRORS.with(|n| n.set(n.get().saturating_add(1)));
            log::error!("aep export: the fixed layout of {} does not fit: {e}", layout());
        }
        self.bytes
    }
}

/// How many fixed layouts have failed on this thread (for the tests).
#[cfg(test)]
pub(crate) fn fixed_layout_errors() -> usize {
    FIXED_LAYOUT_ERRORS.with(std::cell::Cell::get)
}

/// The entries of a property group by match name (see [`split_group`]).
pub(crate) type Entries = Vec<(String, Vec<Chunk<'static>>)>;

/// Splits the children of a property group into the header (up to the first `tdmn`) and its
/// entries by match name: `[tdmn, node]`, or `[tdmn]` for the closing `ADBE Group End`. A match
/// name without a node is an error.
pub(crate) fn split_group(kids: &mut Vec<Chunk<'static>>) -> R<(Vec<Chunk<'static>>, Entries)> {
    let first = kids.iter().position(|c| c.is(b"tdmn")).unwrap_or(kids.len());
    let header: Vec<Chunk<'static>> = kids.drain(..first).collect();
    let mut entries = Entries::new();
    let mut it = std::mem::take(kids).into_iter();
    while let Some(m) = it.next() {
        let name = crate::export::synth::tdmn_name(&m).unwrap_or_default();
        if name == crate::export::synth::GROUP_END {
            entries.push((name, vec![m]));
            continue;
        }
        let node = it.next().ok_or_else(|| tpl("match name without a node"))?;
        entries.push((name, vec![m, node]));
    }
    Ok((header, entries))
}

/// Replaces the `Utf8` child of an item or layer with `text`.
pub(crate) fn set_utf8(c: &mut Chunk<'static>, text: &str) -> R<()> {
    let k = child_mut(c, b"Utf8")?;
    *k = Chunk::data(*b"Utf8", text.as_bytes().to_vec());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The writer errors instead of dropping.
    #[test]
    fn the_writer_errors_instead_of_dropping() {
        let mut d = [0u8; 4];
        assert!(put(&mut d, 2, &[1, 2]).is_ok());
        assert_eq!(d, [0, 0, 1, 2]);
        // Out of range, partly or wholly, and an offset that overflows: an error, nothing written.
        assert!(put(&mut d, 3, &[9, 9]).is_err());
        assert!(put(&mut d, 8, &[9]).is_err());
        assert!(put(&mut d, usize::MAX, &[9]).is_err());
        assert_eq!(d, [0, 0, 1, 2]);
    }

    /// A cut string keeps a nul and whole characters.
    #[test]
    fn a_cut_string_keeps_a_nul_and_whole_characters() {
        let mut d = [0xffu8; 6];
        put_cstr(&mut d, 0, 6, "ab\u{e9}\u{e9}").unwrap();
        assert_eq!(d, [b'a', b'b', 0xc3, 0xa9, 0, 0]);
        assert!(put_cstr(&mut d, 4, 6, "x").is_err());
    }

    /// Every layout fixed in code fits its buffer: the prototypes of every export (the project
    /// skeleton, layers, Transform, masks, effects of the whole table, footage of every kind,
    /// text, cameras, lights, shapes, styles) are built and no write went out of range.
    #[test]
    fn every_fixed_layout_fits() {
        // A delta: other tests on this thread may have counted a broken layout on purpose.
        let before = fixed_layout_errors();
        assert!(crate::export::template::Template::load().is_ok());
        assert_eq!(fixed_layout_errors(), before);
    }

    /// A broken fixed layout writes what fits and is counted.
    #[test]
    fn a_broken_fixed_layout_writes_what_fits_and_is_counted() {
        let before = fixed_layout_errors();
        let mut b = Fixed::zeroed(4);
        b.put(0, &[1]);
        b.put(3, &[2, 2]);
        b.put(2, &[3]);
        assert!(b.error().is_some());
        assert_eq!(b.finish(|| "a test layout".to_string()), [1, 0, 3, 0]);
        assert_eq!(fixed_layout_errors(), before + 1);
    }

    /// A group splits into header and entries.
    #[test]
    fn a_group_splits_into_header_and_entries() {
        use crate::export::synth::{tdmn, tdsn};
        let rec = || Chunk::list(*b"tdbs", Vec::new());
        let mut kids = vec![tdsn("h"), tdmn("A"), rec(), tdmn("ADBE Group End")];
        let (header, entries) = split_group(&mut kids).unwrap();
        assert_eq!(header.len(), 1);
        let names: Vec<(&str, usize)> = entries.iter().map(|(n, v)| (n.as_str(), v.len())).collect();
        assert_eq!(names, [("A", 2), ("ADBE Group End", 1)]);
        // A match name without its node is an error, not a quiet stop.
        let mut broken = vec![tdmn("A")];
        assert!(split_group(&mut broken).is_err());
    }
}
