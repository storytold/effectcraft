//! Reading PyTorch checkpoints (`torch.save` of a state dict) without PyTorch: the file is a zip
//! archive (stored or deflated entries, zip64 sizes) holding `data.pkl`, a pickle (protocol 2–4)
//! whose tensors are `torch._utils._rebuild_tensor_v2(storage, offset, size, stride, …)` calls
//! with storages as persistent ids pointing at `data/<key>` entries. Only what state dicts use is
//! understood; anything else is an error, never a panic.

use std::collections::HashMap;

const MAX_ARCHIVE_BYTES: usize = 1 << 30;
const MAX_PICKLE_OPS: usize = 4_000_000;
const MAX_TENSORS: usize = 1_000_000;
const MAX_TENSOR_ELEMENTS: usize = 1 << 28;
const MAX_ENTRY_BYTES: usize = 1 << 30;
const MAX_PICKLE_NODES: usize = 4_000_000;
const MAX_TOTAL_ELEMENTS: usize = 1 << 29;
const MAX_TENSOR_DIMS: usize = 64;

/// A tensor read from a checkpoint: its shape and its values as f32 (row-major, contiguous).
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

/// A checkpoint's tensors by name (non-float tensors such as batch-norm counters are skipped).
pub type Weights = HashMap<String, Tensor>;

type Result<T> = std::result::Result<T, String>;

fn u16_at(b: &[u8], o: usize) -> Result<usize> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize).ok_or_else(|| "truncated zip".to_string())
}
fn u32_at(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64).ok_or_else(|| "truncated zip".to_string())
}
fn u64_at(b: &[u8], o: usize) -> Result<u64> {
    b.get(o..o + 8).and_then(|s| s.try_into().ok()).map(u64::from_le_bytes).ok_or_else(|| "truncated zip".to_string())
}

/// One archive entry: where its data starts, its sizes and compression.
struct Entry {
    data: usize,
    size: usize,
    stored: usize,
    method: usize,
}

/// The entries of a zip archive by name.
fn zip_entries(b: &[u8]) -> Result<HashMap<String, Entry>> {
    if b.len() > MAX_ARCHIVE_BYTES {
        return Err("checkpoint exceeds the 1024 MiB archive limit".into());
    }
    // End of central directory: the last `PK\x05\x06` (a comment may follow it).
    let eocd =
        (0..b.len().saturating_sub(21)).rev().find(|&i| b.get(i..i + 4) == Some(b"PK\x05\x06")).ok_or("not a zip archive (no end of central directory)")?;
    let mut count = u16_at(b, eocd + 10)? as u64;
    let mut cd = u32_at(b, eocd + 16)?;
    // Zip64: the locator precedes the record.
    if (count == 0xffff || cd == 0xffff_ffff) && eocd >= 20 && b.get(eocd - 20..eocd - 16) == Some(b"PK\x06\x07") {
        let rec = u64_at(b, eocd - 12)? as usize;
        if b.get(rec..rec + 4) == Some(b"PK\x06\x06") {
            count = u64_at(b, rec + 32)?;
            cd = u64_at(b, rec + 48)?;
        }
    }
    let mut out = HashMap::new();
    let mut p = cd as usize;
    for _ in 0..count.min(1 << 20) {
        if b.get(p..p + 4) != Some(b"PK\x01\x02") {
            return Err("damaged zip central directory".into());
        }
        let method = u16_at(b, p + 10)?;
        let mut stored = u32_at(b, p + 20)?;
        let mut size = u32_at(b, p + 24)?;
        let (nl, xl, cl) = (u16_at(b, p + 28)?, u16_at(b, p + 30)?, u16_at(b, p + 32)?);
        let mut local = u32_at(b, p + 42)?;
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nl).ok_or("truncated zip")?).to_string();
        // Zip64 extended information: the 0xffffffff fields, in order.
        let mut x = p + 46 + nl;
        let xend = x + xl;
        while x + 4 <= xend {
            let (id, len) = (u16_at(b, x)?, u16_at(b, x + 2)?);
            if id == 1 {
                let mut f = x + 4;
                for v in [&mut size, &mut stored, &mut local] {
                    if *v == 0xffff_ffff {
                        *v = u64_at(b, f)?;
                        f += 8;
                    }
                }
            }
            x += 4 + len;
        }
        let l = local as usize;
        if b.get(l..l + 4) != Some(b"PK\x03\x04") {
            return Err(format!("damaged zip entry {name}"));
        }
        let data = l + 30 + u16_at(b, l + 26)? + u16_at(b, l + 28)?;
        out.insert(name, Entry { data, size: size as usize, stored: stored as usize, method });
        p += 46 + nl + xl + cl;
    }
    Ok(out)
}

/// An entry's bytes (stored, or raw-deflated).
fn read_entry<'a>(b: &'a [u8], e: &Entry) -> Result<std::borrow::Cow<'a, [u8]>> {
    let raw = b.get(e.data..e.data + e.stored).ok_or("truncated zip entry")?;
    match e.method {
        0 => Ok(std::borrow::Cow::Borrowed(raw)),
        8 => {
            if e.size > MAX_ENTRY_BYTES {
                return Err("zip entry exceeds the size limit".into());
            }
            miniz_oxide::inflate::decompress_to_vec_with_limit(raw, e.size.max(1)).map(std::borrow::Cow::Owned).map_err(|e| format!("bad deflate data: {e:?}"))
        }
        m => Err(format!("unsupported zip compression {m}")),
    }
}

/// The bytes of the archive entry `name` (model bundles are zip archives too).
pub(crate) fn zip_file<'a>(b: &'a [u8], name: &str) -> Result<std::borrow::Cow<'a, [u8]>> {
    let entries = zip_entries(b)?;
    read_entry(b, entries.get(name).ok_or_else(|| format!("{name} is missing from the archive"))?)
}

/// Pickle values (what state dicts use).
#[derive(Clone, Debug)]
enum Obj {
    None,
    Bool,
    Int(i64),
    Float,
    Str(String),
    Tuple(Vec<Obj>),
    List(Vec<Obj>),
    Dict(Vec<(Obj, Obj)>),
    Global(String, String),
    /// A storage: (dtype class name, entry key).
    Storage(String, String),
    /// `_rebuild_tensor_v2`: storage, element offset, size, stride.
    Tensor {
        dtype: String,
        key: String,
        offset: usize,
        size: Vec<usize>,
        stride: Vec<usize>,
    },
    /// A call we don't interpret.
    Other,
    Mark,
}

fn int_list(o: &Obj) -> Option<Vec<usize>> {
    match o {
        Obj::Tuple(v) | Obj::List(v) => v.iter().map(|x| if let Obj::Int(i) = x { usize::try_from(*i).ok() } else { None }).collect(),
        _ => None,
    }
}

/// Nodes in `o`, stopping once the count passes `budget`.
fn nodes(o: &Obj, budget: usize) -> usize {
    let kids: Vec<&Obj> = match o {
        Obj::Tuple(v) | Obj::List(v) => v.iter().collect(),
        Obj::Dict(v) => v.iter().flat_map(|(k, x)| [k, x]).collect(),
        _ => return 1,
    };
    let mut n = 1;
    for k in kids {
        if n > budget {
            break;
        }
        n += nodes(k, budget - n);
    }
    n
}

fn tensor_element_count(name: &str, offset: usize, size: &[usize], stride: &[usize], values_len: usize) -> Result<usize> {
    if size.len() != stride.len() || size.len() > MAX_TENSOR_DIMS {
        return Err(format!("{name}: invalid tensor dimensionality"));
    }
    let n = size
        .iter()
        .try_fold(1usize, |n, &dim| n.checked_mul(dim))
        .filter(|n| *n <= MAX_TENSOR_ELEMENTS)
        .ok_or_else(|| format!("{name}: tensor exceeds the element limit"))?;
    if n == 0 {
        return Ok(0);
    }
    let mut max_at = offset;
    for (&dim, &step) in size.iter().zip(stride) {
        max_at = max_at
            .checked_add((dim - 1).checked_mul(step).ok_or_else(|| format!("{name}: tensor stride overflow"))?)
            .ok_or_else(|| format!("{name}: tensor offset overflow"))?;
    }
    if max_at >= values_len {
        return Err(format!("{name}: data out of range"));
    }
    Ok(n)
}

/// Run the pickle program and return its result.
/// A memoized object, charged against the clone budget (memo reuse can otherwise grow trees exponentially).
fn memo_get(memo: &HashMap<u64, Obj>, k: u64, cloned: &mut usize) -> Result<Obj> {
    let o = memo.get(&k).ok_or("pickle: bad memo")?;
    *cloned = cloned.saturating_add(nodes(o, MAX_PICKLE_NODES));
    if *cloned > MAX_PICKLE_NODES {
        return Err("pickle exceeds the object limit".into());
    }
    Ok(o.clone())
}

fn unpickle(p: &[u8]) -> Result<Obj> {
    let mut stack: Vec<Obj> = Vec::new();
    let mut memo: HashMap<u64, Obj> = HashMap::new();
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Result<&[u8]> {
        let s = p.get(*i..*i + n).ok_or("truncated pickle")?;
        *i += n;
        Ok(s)
    };
    let pop = |stack: &mut Vec<Obj>| stack.pop().ok_or_else(|| "pickle stack underflow".to_string());
    let pop_mark = |stack: &mut Vec<Obj>| -> Result<Vec<Obj>> {
        let m = stack.iter().rposition(|o| matches!(o, Obj::Mark)).ok_or("pickle: no mark")?;
        let items = stack.split_off(m + 1);
        stack.pop();
        Ok(items)
    };
    let line = |i: &mut usize| -> Result<String> {
        let rest = p.get(*i..).ok_or("truncated pickle")?;
        let n = rest.iter().position(|c| *c == b'\n').ok_or("truncated pickle")?;
        let s = String::from_utf8_lossy(&rest[..n]).to_string();
        *i += n + 1;
        Ok(s)
    };
    let mut op_count = 0usize;
    let mut cloned = 0usize;
    loop {
        op_count = op_count.checked_add(1).filter(|n| *n <= MAX_PICKLE_OPS).ok_or("pickle exceeds the opcode limit")?;
        let op = *p.get(i).ok_or("truncated pickle")?;
        i += 1;
        match op {
            0x80 => {
                take(&mut i, 1)?;
            }
            0x95 => {
                take(&mut i, 8)?;
            }
            b'}' => stack.push(Obj::Dict(vec![])),
            b']' => stack.push(Obj::List(vec![])),
            b')' => stack.push(Obj::Tuple(vec![])),
            b'(' => stack.push(Obj::Mark),
            b'N' => stack.push(Obj::None),
            0x88 | 0x89 => stack.push(Obj::Bool),
            b'K' => stack.push(Obj::Int(take(&mut i, 1)?[0] as i64)),
            b'M' => {
                let s = take(&mut i, 2)?;
                stack.push(Obj::Int(u16::from_le_bytes([s[0], s[1]]) as i64));
            }
            b'J' => {
                let s = take(&mut i, 4)?;
                stack.push(Obj::Int(i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as i64));
            }
            0x8a => {
                let n = take(&mut i, 1)?[0] as usize;
                let s = take(&mut i, n)?;
                let mut v: i64 = 0;
                for (k, byte) in s.iter().take(8).enumerate() {
                    v |= (*byte as i64) << (8 * k);
                }
                if n > 0 && n < 8 && s[n - 1] & 0x80 != 0 {
                    v -= 1i64 << (8 * n);
                }
                stack.push(Obj::Int(v));
            }
            b'G' => {
                take(&mut i, 8)?;
                stack.push(Obj::Float);
            }
            b'X' => {
                let s = take(&mut i, 4)?;
                let n = u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize;
                stack.push(Obj::Str(String::from_utf8_lossy(take(&mut i, n)?).to_string()));
            }
            0x8c => {
                let n = take(&mut i, 1)?[0] as usize;
                stack.push(Obj::Str(String::from_utf8_lossy(take(&mut i, n)?).to_string()));
            }
            b'C' => {
                let n = take(&mut i, 1)?[0] as usize;
                take(&mut i, n)?;
                stack.push(Obj::Other);
            }
            b'B' => {
                let s = take(&mut i, 4)?;
                let n = u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize;
                take(&mut i, n)?;
                stack.push(Obj::Other);
            }
            b'c' => {
                let (m, n) = (line(&mut i)?, line(&mut i)?);
                stack.push(Obj::Global(m, n));
            }
            0x93 => {
                let n = pop(&mut stack)?;
                let m = pop(&mut stack)?;
                match (m, n) {
                    (Obj::Str(m), Obj::Str(n)) => stack.push(Obj::Global(m, n)),
                    _ => return Err("pickle: bad STACK_GLOBAL".into()),
                }
            }
            b'q' => {
                let k = take(&mut i, 1)?[0] as u64;
                memo.insert(k, stack.last().cloned().ok_or("pickle stack underflow")?);
            }
            b'r' => {
                let s = take(&mut i, 4)?;
                memo.insert(u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64, stack.last().cloned().ok_or("pickle stack underflow")?);
            }
            0x94 => {
                let k = memo.len() as u64;
                memo.insert(k, stack.last().cloned().ok_or("pickle stack underflow")?);
            }
            b'h' => {
                let k = take(&mut i, 1)?[0] as u64;
                stack.push(memo_get(&memo, k, &mut cloned)?);
            }
            b'j' => {
                let s = take(&mut i, 4)?;
                stack.push(memo_get(&memo, u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as u64, &mut cloned)?);
            }
            b't' => {
                let items = pop_mark(&mut stack)?;
                stack.push(Obj::Tuple(items));
            }
            0x85..=0x87 => {
                let n = (op - 0x84) as usize;
                if stack.len() < n {
                    return Err("pickle stack underflow".into());
                }
                let items = stack.split_off(stack.len() - n);
                stack.push(Obj::Tuple(items));
            }
            b'Q' => {
                // Persistent id: ('storage', dtype class, key, location, numel).
                let pid = pop(&mut stack)?;
                let Obj::Tuple(t) = pid else { return Err("pickle: bad persistent id".into()) };
                match (t.get(1), t.get(2)) {
                    (Some(Obj::Global(_, dtype)), Some(Obj::Str(key))) => stack.push(Obj::Storage(dtype.clone(), key.clone())),
                    _ => stack.push(Obj::Other),
                }
            }
            b'R' | 0x81 => {
                let args = pop(&mut stack)?;
                let f = pop(&mut stack)?;
                let args = match args {
                    Obj::Tuple(a) => a,
                    _ => vec![],
                };
                stack.push(match f {
                    Obj::Global(m, n) if m == "torch._utils" && n == "_rebuild_tensor_v2" => {
                        match (args.first(), args.get(1), args.get(2).and_then(int_list), args.get(3).and_then(int_list)) {
                            (Some(Obj::Storage(dtype, key)), Some(Obj::Int(off)), Some(size), Some(stride)) => match usize::try_from(*off) {
                                Ok(offset) => Obj::Tensor { dtype: dtype.clone(), key: key.clone(), offset, size, stride },
                                Err(_) => Obj::Other,
                            },
                            _ => Obj::Other,
                        }
                    }
                    Obj::Global(m, n) if m == "collections" && n == "OrderedDict" => Obj::Dict(vec![]),
                    _ => Obj::Other,
                });
            }
            b'b' => {
                pop(&mut stack)?;
            }
            b's' => {
                let v = pop(&mut stack)?;
                let k = pop(&mut stack)?;
                if let Some(Obj::Dict(d)) = stack.last_mut() {
                    d.push((k, v));
                }
            }
            b'u' => {
                let items = pop_mark(&mut stack)?;
                if let Some(Obj::Dict(d)) = stack.last_mut() {
                    let mut it = items.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        d.push((k, v));
                    }
                }
            }
            b'a' => {
                let v = pop(&mut stack)?;
                if let Some(Obj::List(l)) = stack.last_mut() {
                    l.push(v);
                }
            }
            b'e' => {
                let items = pop_mark(&mut stack)?;
                if let Some(Obj::List(l)) = stack.last_mut() {
                    l.extend(items);
                }
            }
            b'.' => return pop(&mut stack),
            other => return Err(format!("pickle: unsupported opcode 0x{other:02x}")),
        }
    }
}

/// Bytes per element of a float storage dtype.
fn elem_size(dtype: &str) -> Option<usize> {
    match dtype {
        "FloatStorage" => Some(4),
        "HalfStorage" | "BFloat16Storage" => Some(2),
        _ => None,
    }
}

/// Element `i` of a storage (`i` must be below `bytes.len() / elem_size`).
fn elem_f32(dtype: &str, bytes: &[u8], i: usize) -> f32 {
    match dtype {
        "FloatStorage" => f32::from_le_bytes([bytes[4 * i], bytes[4 * i + 1], bytes[4 * i + 2], bytes[4 * i + 3]]),
        "HalfStorage" => half_to_f32(u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]])),
        _ => f32::from_bits((u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as u32) << 16),
    }
}

pub(crate) fn half_to_f32(h: u16) -> f32 {
    let (s, e, m) = ((h >> 15) as u32, ((h >> 10) & 0x1f) as u32, (h & 0x3ff) as u32);
    let bits = match (e, m) {
        (0, 0) => s << 31,
        (0, m) => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            (s << 31) | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, m) => (s << 31) | (0xff << 23) | (m << 13),
        (e, m) => (s << 31) | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// Read a PyTorch checkpoint's float tensors. A dict holding the state dict under `model` or
/// `state_dict` is unwrapped.
pub fn read(bytes: &[u8]) -> Result<Weights> {
    let entries = zip_entries(bytes)?;
    let (pkl_name, pkl) = entries.iter().find(|(n, _)| n.ends_with("data.pkl")).ok_or("not a PyTorch checkpoint (no data.pkl)")?;
    let prefix = pkl_name.trim_end_matches("data.pkl").to_string();
    let root = unpickle(&read_entry(bytes, pkl)?)?;
    let dict = match root {
        Obj::Dict(d) => d,
        _ => return Err("the checkpoint is not a state dict".into()),
    };
    let dict = match dict.iter().find(|(k, _)| matches!(k, Obj::Str(s) if s == "model" || s == "state_dict")) {
        Some((_, Obj::Dict(inner))) => inner.clone(),
        _ => dict,
    };
    if dict.len() > MAX_TENSORS {
        return Err("checkpoint exceeds the tensor count limit".into());
    }
    let mut storages: HashMap<String, std::borrow::Cow<[u8]>> = HashMap::new();
    let mut out = Weights::new();
    let mut total = 0usize;
    let mut inflated = 0usize;
    for (k, v) in dict {
        let (Obj::Str(name), Obj::Tensor { dtype, key, offset, size, stride }) = (k, v) else { continue };
        if !storages.contains_key(&key) {
            let e = entries.get(&format!("{prefix}data/{key}")).ok_or_else(|| format!("missing storage {key}"))?;
            if e.method == 8 {
                inflated = inflated.saturating_add(e.size);
                if inflated > MAX_ENTRY_BYTES {
                    return Err("checkpoint exceeds the decompressed storage limit".into());
                }
            }
            storages.insert(key.clone(), read_entry(bytes, e)?);
        }
        let (Some(esz), Some(values)) = (elem_size(&dtype), storages.get(&key)) else { continue };
        // Check hostile stride math before allocating or indexing the backing storage.
        let n = tensor_element_count(&name, offset, &size, &stride, values.len() / esz)?;
        total = total.saturating_add(n);
        if total > MAX_TOTAL_ELEMENTS {
            return Err("checkpoint exceeds the total element limit".into());
        }
        let mut data = Vec::new();
        data.try_reserve_exact(n).map_err(|_| format!("{name}: tensor allocation failed"))?;
        let mut idx = vec![0usize; size.len()];
        for _ in 0..n {
            let at = idx
                .iter()
                .zip(&stride)
                .try_fold(offset, |at, (i, s)| i.checked_mul(*s).and_then(|v| at.checked_add(v)))
                .ok_or_else(|| format!("{name}: tensor offset overflow"))?;
            data.push(elem_f32(&dtype, values, at));
            for d in (0..idx.len()).rev() {
                idx[d] += 1;
                if idx[d] < size[d] {
                    break;
                }
                idx[d] = 0;
            }
        }
        out.insert(name, Tensor { shape: size, data });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip archive with stored entries (what `torch.save` writes).
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = vec![];
        let mut central = vec![];
        for (name, data) in files {
            let off = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&[0; 4]); // crc (unchecked)
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&[0; 4]);
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    /// The pickle `torch.save({'w': t.t()})` writes for a 2×3 tensor stored transposed (strides
    /// (1, 2)), plus an int64 counter that is skipped.
    fn pickle() -> Vec<u8> {
        let mut p = vec![0x80, 2, b'}', b'q', 0, b'('];
        let s = |p: &mut Vec<u8>, t: &str| {
            p.push(b'X');
            p.extend_from_slice(&(t.len() as u32).to_le_bytes());
            p.extend_from_slice(t.as_bytes());
        };
        for (name, dtype, key, size, stride) in [("w", "FloatStorage", "0", [2u8, 3], [1u8, 2]), ("n", "LongStorage", "1", [1, 1], [1, 1])] {
            s(&mut p, name);
            // The call's argument mark, then the persistent id tuple's.
            p.extend_from_slice(b"ctorch._utils\n_rebuild_tensor_v2\n((");
            s(&mut p, "storage");
            p.extend_from_slice(format!("ctorch\n{dtype}\n").as_bytes());
            s(&mut p, key);
            s(&mut p, "cpu");
            p.extend_from_slice(&[b'K', 6, b't', b'Q', b'K', 0]);
            p.extend_from_slice(&[b'K', size[0], b'K', size[1], 0x86, b'K', stride[0], b'K', stride[1], 0x86]);
            p.extend_from_slice(&[0x89, b'c']);
            p.extend_from_slice(b"collections\nOrderedDict\n");
            p.extend_from_slice(b")RtR");
        }
        p.extend_from_slice(b"u.");
        p
    }

    #[test]
    fn reads_a_state_dict_with_strides() {
        let storage: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let pkl = pickle();
        let file = zip(&[("ckpt/data.pkl", &pkl), ("ckpt/data/0", &storage), ("ckpt/data/1", &7i64.to_le_bytes())]);
        let w = read(&file).unwrap();
        // Strides (1, 2) over [1..6]: rows are [1, 3, 5] and [2, 4, 6].
        assert_eq!(w.get("w"), Some(&Tensor { shape: vec![2, 3], data: vec![1.0, 3.0, 5.0, 2.0, 4.0, 6.0] }));
        assert!(!w.contains_key("n"), "integer tensors are skipped");
    }

    #[test]
    fn rejects_tensor_shape_overflow_and_invalid_strides() {
        assert!(tensor_element_count("bad", 0, &[usize::MAX, 2], &[1, 1], 16).is_err());
        assert!(tensor_element_count("bad", 0, &[2, 2], &[usize::MAX, 1], 16).is_err());
        assert!(tensor_element_count("bad", 0, &[2, 2], &[1], 16).is_err());
        assert!(tensor_element_count("bad", usize::MAX, &[2], &[1], 16).is_err());
        assert_eq!(tensor_element_count("empty", 0, &[0, usize::MAX], &[0, 0], 0), Ok(0));
    }

    #[test]
    fn damaged_files_are_errors() {
        assert!(read(b"not a zip").is_err());
        let pkl = pickle();
        let file = zip(&[("ckpt/data.pkl", &pkl[..pkl.len() - 5])]);
        assert!(read(&file).is_err());
        // Truncate the storage: the tensor reads out of range.
        let file = zip(&[("ckpt/data.pkl", &pkl), ("ckpt/data/0", &[0u8; 8]), ("ckpt/data/1", &[0u8; 8])]);
        assert!(read(&file).is_err());
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
    }
}
