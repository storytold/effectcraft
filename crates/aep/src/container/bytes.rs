//! Bounds-checked big-endian readers shared by the typed views. Every read returns `None` when the
//! bytes are not there; nothing here indexes a slice directly.

pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The bytes up to the first NUL (all of them when there is none).
pub(crate) fn until_nul(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    b.get(..end).unwrap_or(&[])
}

/// A NUL-terminated ASCII (or Latin-1) string at `at`, at most `max` bytes long.
pub(crate) fn cstr(b: &[u8], at: usize, max: usize) -> Option<String> {
    let end = at.checked_add(max)?.min(b.len());
    let field = b.get(at..end)?;
    Some(until_nul(field).iter().map(|c| char::from(*c)).collect())
}

pub(crate) fn array<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    let s = b.get(at..at.checked_add(N)?)?;
    let mut a = [0u8; N];
    a.copy_from_slice(s);
    Some(a)
}

pub(crate) fn be_u16(b: &[u8], at: usize) -> Option<u16> {
    array::<2>(b, at).map(u16::from_be_bytes)
}

pub(crate) fn be_u32(b: &[u8], at: usize) -> Option<u32> {
    array::<4>(b, at).map(u32::from_be_bytes)
}

pub(crate) fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    array::<4>(b, at).map(i32::from_be_bytes)
}

pub(crate) fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    array::<4>(b, at).map(u32::from_le_bytes)
}

pub(crate) fn be_f32(b: &[u8], at: usize) -> Option<f32> {
    array::<4>(b, at).map(f32::from_be_bytes)
}

pub(crate) fn be_f64(b: &[u8], at: usize) -> Option<f64> {
    array::<8>(b, at).map(f64::from_be_bytes)
}
