//! `CCITTFaxDecode` (ISO 32000-1 §7.4.6): ITU-T T.4 Group 3 one-dimensional (Modified
//! Huffman) and two-dimensional (Modified READ) coding and ITU-T T.6 Group 4 (Modified Modified
//! READ), implemented from the ITU-T recommendations. The result is 1-bit rows, `0` black
//! unless `BlackIs1`.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The decoding parameters (`/DecodeParms`).
#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    /// `< 0` Group 4, `0` Group 3 1-D, `> 0` Group 3 mixed 1-D / 2-D.
    pub k: i64,
    pub end_of_line: bool,
    pub byte_align: bool,
    pub columns: usize,
    /// `0`: as many rows as the data holds.
    pub rows: usize,
    pub end_of_block: bool,
    pub black_is_1: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params { k: 0, end_of_line: false, byte_align: false, columns: 1728, rows: 0, end_of_block: true, black_is_1: false }
    }
}

/// White terminating codes (T.4 Table 2), run lengths 0–63, as (code, length).
const WHITE_TERM: [(u16, u8); 64] = [
    (0b00110101, 8),
    (0b000111, 6),
    (0b0111, 4),
    (0b1000, 4),
    (0b1011, 4),
    (0b1100, 4),
    (0b1110, 4),
    (0b1111, 4),
    (0b10011, 5),
    (0b10100, 5),
    (0b00111, 5),
    (0b01000, 5),
    (0b001000, 6),
    (0b000011, 6),
    (0b110100, 6),
    (0b110101, 6),
    (0b101010, 6),
    (0b101011, 6),
    (0b0100111, 7),
    (0b0001100, 7),
    (0b0001000, 7),
    (0b0010111, 7),
    (0b0000011, 7),
    (0b0000100, 7),
    (0b0101000, 7),
    (0b0101011, 7),
    (0b0010011, 7),
    (0b0100100, 7),
    (0b0011000, 7),
    (0b00000010, 8),
    (0b00000011, 8),
    (0b00011010, 8),
    (0b00011011, 8),
    (0b00010010, 8),
    (0b00010011, 8),
    (0b00010100, 8),
    (0b00010101, 8),
    (0b00010110, 8),
    (0b00010111, 8),
    (0b00101000, 8),
    (0b00101001, 8),
    (0b00101010, 8),
    (0b00101011, 8),
    (0b00101100, 8),
    (0b00101101, 8),
    (0b00000100, 8),
    (0b00000101, 8),
    (0b00001010, 8),
    (0b00001011, 8),
    (0b01010010, 8),
    (0b01010011, 8),
    (0b01010100, 8),
    (0b01010101, 8),
    (0b00100100, 8),
    (0b00100101, 8),
    (0b01011000, 8),
    (0b01011001, 8),
    (0b01011010, 8),
    (0b01011011, 8),
    (0b01001010, 8),
    (0b01001011, 8),
    (0b00110010, 8),
    (0b00110011, 8),
    (0b00110100, 8),
];

/// White make-up codes (T.4 Table 3), run lengths 64, 128, … 1728.
const WHITE_MAKEUP: [(u16, u8); 27] = [
    (0b11011, 5),
    (0b10010, 5),
    (0b010111, 6),
    (0b0110111, 7),
    (0b00110110, 8),
    (0b00110111, 8),
    (0b01100100, 8),
    (0b01100101, 8),
    (0b01101000, 8),
    (0b01100111, 8),
    (0b011001100, 9),
    (0b011001101, 9),
    (0b011010010, 9),
    (0b011010011, 9),
    (0b011010100, 9),
    (0b011010101, 9),
    (0b011010110, 9),
    (0b011010111, 9),
    (0b011011000, 9),
    (0b011011001, 9),
    (0b011011010, 9),
    (0b011011011, 9),
    (0b010011000, 9),
    (0b010011001, 9),
    (0b010011010, 9),
    (0b011000, 6),
    (0b010011011, 9),
];

/// Black terminating codes (T.4 Table 2), run lengths 0–63.
const BLACK_TERM: [(u16, u8); 64] = [
    (0b0000110111, 10),
    (0b010, 3),
    (0b11, 2),
    (0b10, 2),
    (0b011, 3),
    (0b0011, 4),
    (0b0010, 4),
    (0b00011, 5),
    (0b000101, 6),
    (0b000100, 6),
    (0b0000100, 7),
    (0b0000101, 7),
    (0b0000111, 7),
    (0b00000100, 8),
    (0b00000111, 8),
    (0b000011000, 9),
    (0b0000010111, 10),
    (0b0000011000, 10),
    (0b0000001000, 10),
    (0b00001100111, 11),
    (0b00001101000, 11),
    (0b00001101100, 11),
    (0b00000110111, 11),
    (0b00000101000, 11),
    (0b00000010111, 11),
    (0b00000011000, 11),
    (0b000011001010, 12),
    (0b000011001011, 12),
    (0b000011001100, 12),
    (0b000011001101, 12),
    (0b000001101000, 12),
    (0b000001101001, 12),
    (0b000001101010, 12),
    (0b000001101011, 12),
    (0b000011010010, 12),
    (0b000011010011, 12),
    (0b000011010100, 12),
    (0b000011010101, 12),
    (0b000011010110, 12),
    (0b000011010111, 12),
    (0b000001101100, 12),
    (0b000001101101, 12),
    (0b000011011010, 12),
    (0b000011011011, 12),
    (0b000001010100, 12),
    (0b000001010101, 12),
    (0b000001010110, 12),
    (0b000001010111, 12),
    (0b000001100100, 12),
    (0b000001100101, 12),
    (0b000001010010, 12),
    (0b000001010011, 12),
    (0b000000100100, 12),
    (0b000000110111, 12),
    (0b000000111000, 12),
    (0b000000100111, 12),
    (0b000000101000, 12),
    (0b000001011000, 12),
    (0b000001011001, 12),
    (0b000000101011, 12),
    (0b000000101100, 12),
    (0b000001011010, 12),
    (0b000001100110, 12),
    (0b000001100111, 12),
];

/// Black make-up codes (T.4 Table 3), run lengths 64, 128, … 1728.
const BLACK_MAKEUP: [(u16, u8); 27] = [
    (0b0000001111, 10),
    (0b000011001000, 12),
    (0b000011001001, 12),
    (0b000001011011, 12),
    (0b000000110011, 12),
    (0b000000110100, 12),
    (0b000000110101, 12),
    (0b0000001101100, 13),
    (0b0000001101101, 13),
    (0b0000001001010, 13),
    (0b0000001001011, 13),
    (0b0000001001100, 13),
    (0b0000001001101, 13),
    (0b0000001110010, 13),
    (0b0000001110011, 13),
    (0b0000001110100, 13),
    (0b0000001110101, 13),
    (0b0000001110110, 13),
    (0b0000001110111, 13),
    (0b0000001010010, 13),
    (0b0000001010011, 13),
    (0b0000001010100, 13),
    (0b0000001010101, 13),
    (0b0000001011010, 13),
    (0b0000001011011, 13),
    (0b0000001100100, 13),
    (0b0000001100101, 13),
];

/// Extended make-up codes for both colours (T.4 Table 3a), run lengths 1792, 1856, … 2560.
const EXT_MAKEUP: [(u16, u8); 13] = [
    (0b00000001000, 11),
    (0b00000001100, 11),
    (0b00000001101, 11),
    (0b000000010010, 12),
    (0b000000010011, 12),
    (0b000000010100, 12),
    (0b000000010101, 12),
    (0b000000010110, 12),
    (0b000000010111, 12),
    (0b000000011100, 12),
    (0b000000011101, 12),
    (0b000000011110, 12),
    (0b000000011111, 12),
];

/// A colour's code table: (length, code) → run length.
type Table = HashMap<(u8, u16), u32>;

fn table(term: &[(u16, u8); 64], makeup: &[(u16, u8); 27]) -> Table {
    let mut t = HashMap::new();
    for (run, (code, len)) in term.iter().enumerate() {
        t.insert((*len, *code), run as u32);
    }
    for (i, (code, len)) in makeup.iter().enumerate() {
        t.insert((*len, *code), 64 * (i as u32 + 1));
    }
    for (i, (code, len)) in EXT_MAKEUP.iter().enumerate() {
        t.insert((*len, *code), 1792 + 64 * i as u32);
    }
    t
}

fn tables() -> &'static (Table, Table) {
    static T: OnceLock<(Table, Table)> = OnceLock::new();
    T.get_or_init(|| (table(&WHITE_TERM, &WHITE_MAKEUP), table(&BLACK_TERM, &BLACK_MAKEUP)))
}

/// The code (code, length) for a run of `run` (≥ 0, any length) as make-up codes then a
/// terminating code (used by the test encoder).
#[cfg(test)]
pub(crate) fn run_codes(white: bool, mut run: usize) -> Vec<(u16, u8)> {
    let (term, makeup) = if white { (&WHITE_TERM, &WHITE_MAKEUP) } else { (&BLACK_TERM, &BLACK_MAKEUP) };
    let mut out = vec![];
    while run >= 2560 {
        out.push(EXT_MAKEUP[12]);
        run -= 2560;
    }
    if run >= 1792 {
        out.push(EXT_MAKEUP[(run - 1792) / 64]);
        run -= 1792 + (run - 1792) / 64 * 64;
    } else if run >= 64 {
        out.push(makeup[run / 64 - 1]);
        run %= 64;
    }
    out.push(term[run]);
    out
}

struct Reader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl Reader<'_> {
    fn bit_at(&self, i: usize) -> u16 {
        self.data.get(i / 8).map(|b| ((b >> (7 - i % 8)) & 1) as u16).unwrap_or(0)
    }

    fn peek(&self, n: usize) -> u16 {
        (0..n).fold(0, |v, k| (v << 1) | self.bit_at(self.bit + k))
    }

    fn eof(&self) -> bool {
        self.bit >= self.data.len() * 8
    }

    fn take(&mut self, n: usize) -> u16 {
        let v = self.peek(n);
        self.bit += n;
        v
    }

    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }

    /// An EOL (eleven or more 0 bits then 1) at the read position: consume it.
    fn eol(&mut self) -> bool {
        let mut zeros = 0;
        while zeros < 64 && self.bit + zeros < self.data.len() * 8 && self.bit_at(self.bit + zeros) == 0 {
            zeros += 1;
        }
        if zeros >= 11 && self.bit + zeros < self.data.len() * 8 {
            self.bit += zeros + 1;
            return true;
        }
        false
    }

    /// One run (make-up codes then a terminating code), `None` on an invalid code.
    fn run(&mut self, t: &Table) -> Option<u32> {
        let mut total = 0u32;
        loop {
            if self.eof() {
                return None;
            }
            let mut found = None;
            for len in 2..=13u8 {
                if let Some(r) = t.get(&(len, self.peek(len as usize))) {
                    found = Some((len, *r));
                    break;
                }
            }
            let (len, r) = found?;
            self.bit += len as usize;
            total = total.saturating_add(r);
            if r < 64 {
                return Some(total);
            }
        }
    }
}

/// 2-D mode codes (T.4 Table 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Pass,
    Horizontal,
    Vertical(i64),
    Eol,
}

fn mode(r: &mut Reader) -> Option<Mode> {
    if r.eof() {
        return None;
    }
    let m = if r.peek(1) == 1 {
        r.bit += 1;
        Mode::Vertical(0)
    } else if r.peek(3) == 0b011 {
        r.bit += 3;
        Mode::Vertical(1)
    } else if r.peek(3) == 0b010 {
        r.bit += 3;
        Mode::Vertical(-1)
    } else if r.peek(3) == 0b001 {
        r.bit += 3;
        Mode::Horizontal
    } else if r.peek(4) == 0b0001 {
        r.bit += 4;
        Mode::Pass
    } else if r.peek(6) == 0b000011 {
        r.bit += 6;
        Mode::Vertical(2)
    } else if r.peek(6) == 0b000010 {
        r.bit += 6;
        Mode::Vertical(-2)
    } else if r.peek(7) == 0b0000011 {
        r.bit += 7;
        Mode::Vertical(3)
    } else if r.peek(7) == 0b0000010 {
        r.bit += 7;
        Mode::Vertical(-3)
    } else if r.eol() {
        Mode::Eol
    } else {
        return None;
    };
    Some(m)
}

/// The first changing element of `refl` after `a0` whose colour is the opposite of `white`
/// (index parity: even indices start black runs), and the one after it.
fn b1b2(refl: &[usize], a0: i64, white: bool, columns: usize) -> (usize, usize) {
    let want = if white { 0 } else { 1 };
    // The changing elements are in order, so the first past `a0` is found by bisection (a scan
    // from the start made every row quadratic in its transitions).
    let mut i = refl.partition_point(|&c| c as i64 <= a0);
    if i % 2 != want {
        i += 1;
    }
    let b1 = refl.get(i).copied().unwrap_or(columns);
    let b2 = refl.get(i + 1).copied().unwrap_or(columns);
    (b1, b2)
}

/// Decode a 1-D (Modified Huffman) row into changing elements.
fn row_1d(r: &mut Reader, columns: usize) -> Option<Vec<usize>> {
    let (wt, bt) = tables();
    let mut changes = vec![];
    let mut a0 = 0usize;
    let mut white = true;
    while a0 < columns {
        let run = r.run(if white { wt } else { bt })? as usize;
        a0 = (a0 + run).min(columns);
        changes.push(a0);
        white = !white;
    }
    Some(changes)
}

/// Decode a 2-D row against the reference row's changing elements. `Err(true)` at an EOL.
fn row_2d(r: &mut Reader, refl: &[usize], columns: usize) -> Result<Vec<usize>, bool> {
    let (wt, bt) = tables();
    let mut changes: Vec<usize> = vec![];
    let mut a0: i64 = -1;
    let mut white = true;
    let mut guard = 0;
    while a0 < columns as i64 {
        guard += 1;
        if guard > 4 * columns + 16 {
            return Err(false);
        }
        let (b1, b2) = b1b2(refl, a0, white, columns);
        match mode(r).ok_or(false)? {
            Mode::Pass => {
                // The run continues under b2 (no change in the coding line).
                a0 = b2 as i64;
            }
            Mode::Horizontal => {
                let start = a0.max(0) as usize;
                let r1 = r.run(if white { wt } else { bt }).ok_or(false)? as usize;
                let r2 = r.run(if white { bt } else { wt }).ok_or(false)? as usize;
                let a1 = (start + r1).min(columns);
                let a2 = (a1 + r2).min(columns);
                changes.push(a1);
                changes.push(a2);
                a0 = a2 as i64;
            }
            Mode::Vertical(d) => {
                let a1 = (b1 as i64 + d).clamp(0, columns as i64);
                if a1 < a0 {
                    return Err(false);
                }
                changes.push(a1 as usize);
                a0 = a1;
                white = !white;
            }
            Mode::Eol => return Err(true),
        }
    }
    Ok(changes)
}

/// Changing elements → packed bits (`black` bit value for black pixels).
fn pack(changes: &[usize], columns: usize, black_is_1: bool, out: &mut Vec<u8>) {
    let row_bytes = columns.div_ceil(8);
    let base = out.len();
    out.resize(base + row_bytes, if black_is_1 { 0 } else { 0xFF });
    let mut x = 0usize;
    let mut white = true;
    for &c in changes.iter().chain(std::iter::once(&columns)) {
        let c = c.min(columns);
        if !white {
            for p in x..c {
                let byte = &mut out[base + p / 8];
                let m = 0x80u8 >> (p % 8);
                if black_is_1 { *byte |= m } else { *byte &= !m }
            }
        }
        x = x.max(c);
        white = !white;
        if x >= columns {
            break;
        }
    }
}

/// Decode CCITT fax data. Damaged data ends the image at the last good row (rows already
/// decoded are kept); `None` when no row decodes or the output would exceed `max`.
pub fn decode(data: &[u8], p: &Params, max: usize) -> Option<Vec<u8>> {
    let columns = p.columns.clamp(1, 1 << 16);
    let max_rows = if p.rows > 0 { p.rows.min(1 << 20) } else { 1 << 20 };
    let row_bytes = columns.checked_add(7)? / 8;
    // With `/Rows` the image is padded to that many rows (below), so its size is known up front.
    let padded_len = if p.rows > 0 { max_rows.checked_mul(row_bytes)? } else { 0 };
    if row_bytes > max || padded_len > max {
        return None;
    }
    let mut r = Reader { data, bit: 0 };
    let mut out = vec![];
    let mut refl: Vec<usize> = vec![];
    let mut rows = 0;
    while rows < max_rows && !r.eof() {
        let line: Option<Vec<usize>> = if p.k < 0 {
            if p.byte_align {
                r.align();
            }
            // An error is EOFB (two EOLs) or damage.
            row_2d(&mut r, &refl, columns).ok()
        } else {
            // Group 3: optional fill and EOL before each row, then a tag bit when K > 0.
            let had_eol = r.eol();
            if !had_eol && p.byte_align {
                r.align();
                r.eol();
            }
            // RTC: six consecutive EOLs (the first already consumed).
            if had_eol && p.end_of_block {
                let save = r.bit;
                if r.eol() {
                    break;
                }
                r.bit = save;
            }
            if r.eof() {
                break;
            }
            let two_d = p.k > 0 && r.take(1) == 0;
            if two_d { row_2d(&mut r, &refl, columns).ok() } else { row_1d(&mut r, columns) }
        };
        let Some(line) = line else { break };
        // Unknown row counts are bounded as rows arrive, without returning a prefix.
        if out.len().checked_add(row_bytes)? > max {
            return None;
        }
        pack(&line, columns, p.black_is_1, &mut out);
        refl = line;
        rows += 1;
    }
    // Rows missing from damaged or short data stay white.
    if p.rows > 0 && rows > 0 && rows < p.rows {
        out.resize(padded_len, if p.black_is_1 { 0 } else { 0xFF });
    }
    (rows > 0).then_some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A bit writer for the test encoder.
    #[derive(Default)]
    pub(crate) struct W {
        pub bytes: Vec<u8>,
        n: usize,
    }

    impl W {
        pub fn put(&mut self, code: u16, len: u8) {
            for k in (0..len).rev() {
                if self.n.is_multiple_of(8) {
                    self.bytes.push(0);
                }
                if (code >> k) & 1 == 1 {
                    *self.bytes.last_mut().unwrap() |= 0x80 >> (self.n % 8);
                }
                self.n += 1;
            }
        }
        fn run(&mut self, white: bool, run: usize) {
            for (c, l) in run_codes(white, run) {
                self.put(c, l);
            }
        }
    }

    /// Changing elements of a row of pixels (`true` = black).
    fn changes(row: &[bool]) -> Vec<usize> {
        let mut out = vec![];
        let mut cur = false;
        for (i, &b) in row.iter().enumerate() {
            if b != cur {
                out.push(i);
                cur = b;
            }
        }
        out
    }

    fn encode_1d(w: &mut W, row: &[bool]) {
        let cols = row.len();
        let mut ch = changes(row);
        ch.push(cols);
        let mut a0 = 0;
        let mut white = true;
        for c in ch {
            w.run(white, c - a0);
            a0 = c;
            white = !white;
            if a0 >= cols {
                break;
            }
        }
    }

    fn encode_2d(w: &mut W, row: &[bool], refl: &[usize]) {
        let cols = row.len();
        let cur = changes(row);
        let mut a0: i64 = -1;
        let mut white = true;
        let next = |a0: i64| cur.iter().copied().find(|&c| c as i64 > a0).unwrap_or(cols);
        while a0 < cols as i64 {
            let a1 = next(a0);
            let (b1, b2) = b1b2(refl, a0, white, cols);
            if b2 < a1 {
                w.put(0b0001, 4);
                a0 = b2 as i64;
            } else if (a1 as i64 - b1 as i64).abs() <= 3 {
                let d = a1 as i64 - b1 as i64;
                let (c, l) = match d {
                    0 => (1, 1),
                    1 => (0b011, 3),
                    -1 => (0b010, 3),
                    2 => (0b000011, 6),
                    -2 => (0b000010, 6),
                    3 => (0b0000011, 7),
                    _ => (0b0000010, 7),
                };
                w.put(c, l);
                a0 = a1 as i64;
                white = !white;
            } else {
                let a2 = next(a1 as i64);
                w.put(0b001, 3);
                let start = a0.max(0) as usize;
                w.run(white, a1 - start);
                w.run(!white, a2 - a1);
                a0 = a2 as i64;
            }
        }
    }

    pub(crate) fn encode(img: &[Vec<bool>], k: i64) -> Vec<u8> {
        let mut w = W::default();
        let mut refl: Vec<usize> = vec![];
        for (i, row) in img.iter().enumerate() {
            match k {
                k if k < 0 => encode_2d(&mut w, row, &refl),
                0 => {
                    w.put(1, 12);
                    encode_1d(&mut w, row);
                }
                _ => {
                    w.put(1, 12);
                    if i as i64 % k == 0 {
                        w.put(1, 1);
                        encode_1d(&mut w, row);
                    } else {
                        w.put(0, 1);
                        encode_2d(&mut w, row, &refl);
                    }
                }
            }
            refl = changes(row);
        }
        if k < 0 {
            w.put(1, 12);
            w.put(1, 12);
        }
        w.bytes
    }

    pub(crate) fn pattern(cols: usize, rows: usize, seed: u32) -> Vec<Vec<bool>> {
        let mut s = seed;
        let mut rnd = || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (s >> 16) & 0x7FFF
        };
        let mut img = vec![];
        let mut prev = vec![false; cols];
        for y in 0..rows {
            let mut row = prev.clone();
            // Mostly like the row above (2-D coding's case), with random edits and runs.
            for _ in 0..(rnd() % 5) {
                let a = rnd() as usize % cols;
                let len = 1 + rnd() as usize % (cols / 3).max(1);
                let v = rnd() % 2 == 0;
                for p in row.iter_mut().skip(a).take(len) {
                    *p = v;
                }
            }
            if y % 7 == 3 {
                row.iter_mut().for_each(|p| *p = rnd() % 3 == 0);
            }
            img.push(row.clone());
            prev = row;
        }
        img
    }

    fn unpack(data: &[u8], cols: usize, rows: usize) -> Vec<Vec<bool>> {
        let rb = cols.div_ceil(8);
        (0..rows).map(|y| (0..cols).map(|x| data[y * rb + x / 8] & (0x80 >> (x % 8)) == 0).collect()).collect()
    }

    #[test]
    fn code_tables_are_prefix_free() {
        for (white, term, makeup) in [(true, &WHITE_TERM, &WHITE_MAKEUP), (false, &BLACK_TERM, &BLACK_MAKEUP)] {
            let mut all: Vec<(u16, u8)> = term.iter().chain(makeup.iter()).chain(EXT_MAKEUP.iter()).copied().collect();
            all.push((1, 12)); // EOL
            let mut kraft = 0.0;
            for (i, a) in all.iter().enumerate() {
                kraft += 0.5f64.powi(a.1 as i32);
                for (j, b) in all.iter().enumerate() {
                    if i != j && a.1 <= b.1 {
                        assert_ne!(b.0 >> (b.1 - a.1), a.0, "white {white}: {a:?} is a prefix of {b:?}");
                    }
                }
            }
            assert!(kraft <= 1.0, "white {white}: Kraft sum {kraft}");
        }
    }

    #[test]
    fn round_trips_group_3_and_group_4() {
        for (cols, rows, seed) in [(1, 3, 1), (8, 8, 2), (37, 29, 3), (300, 40, 4), (2600, 6, 5)] {
            let img = pattern(cols, rows, seed);
            for k in [-1, 0, 1, 3] {
                let data = encode(&img, k);
                let p = Params { k, columns: cols, rows, ..Params::default() };
                let out = decode(&data, &p, 1 << 20).unwrap_or_else(|| panic!("{cols}×{rows} K {k}: no rows"));
                assert_eq!(unpack(&out, cols, rows), img, "{cols}×{rows} K {k}");
                // Rows unknown: stop at the end of the data / EOFB.
                let out = decode(&data, &Params { rows: 0, ..p.clone() }, 1 << 20).unwrap();
                assert_eq!(unpack(&out, cols, rows), img, "{cols}×{rows} K {k}, no /Rows");
                // BlackIs1 inverts.
                let inv = decode(&data, &Params { black_is_1: true, ..p }, 1 << 20).unwrap();
                assert!(inv.iter().zip(&decode(&data, &Params { k, columns: cols, rows, ..Params::default() }, 1 << 20).unwrap()).all(|(a, b)| a ^ b == 0xFF));
            }
        }
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Data from an independent encoder (libtiff's `tiffcp -c g4 / g3:1d / g3:2d / g3:1d:fill`,
    /// run once as an external fixture generator) for a 61×23 generated image (1 = black).
    #[test]
    fn decodes_an_independent_encoders_output() {
        let raw = hex(
            "0000000000000008380000000000000042d05803696223a87fc00003697fffe87f00007ffd7fffe87f00007ffd7fffe87f00007fffffffe87f00007ffffffff85cc00128608c10505cc001f860007ff0fffffffffffffff8fffffffffffdfff8fffffffffffffff8f8040003fffffff84140e4180f5002d04140e4180f5002d05ffdfc180f5000005ffdfc180f5000005ffdfc180f5000005ffdf000000ff800270ca82855a00d78ffecf82855a00c00ffecf82855a00c00",
        );
        let cases = [
            (
                -1,
                "52f3408da23e4747111f34c8f91d17447cc2308c4082588c447fe2223e11ac13ffff88fe3943845f08a72872acf8541f0ac0412ff11e22108e844445c82b0f8e4e6555608e223a12f17408fe58e50e51ca1c104bffffffc8e150ffc447ffffffffc21116449b29cd76088e88e8e223a3688e88e88f8455191d11d8420f88ffffc4e06ffffffc004004",
            ),
            (
                0,
                "0014b4002f2c8008ead0f8eb10fd31f1d3a1f850a21d0e8004710463e3a7438103a0011c75820e870207400238eb041d0e040e800471d61a43a0011c7586a0008e879f0d3a1d5f8a3c5887438008e879f0cafd028e0026a0b40026a0ac3838009a82d0009a9cd540d20023ac43ae9d67ec743a3a1f1d0e0023ac43ae9d67ec743a3a1f1d0e0023a1c28e3cfd8e87558008e870a38f3f63a1d560023a1c28e3cfd8e87558008e870a39a8126002e9ebdd0e875887568743a1f1d531d0ec004d428fb9e21d5a1d0e87c754fc004d428fb9e21d5a1d0e87c754fc",
            ),
            (
                4,
                "001a5a001179800c75687c75887e98f8e9d0fc28510e87400288c447fe2223c00638eb041d0e040e8005ff800c71d61a43a00171800c743cf869d0eafc51e2c43a1c005fc47888423800cd416800520ac3c0066a0b400293995540063ac43ae9d67ec743a3a1f1d0e002ffffff800c743851c79fb1d0eab0017fff0018e870a38f3f63a1d56002f84222c893001ba7af743a1d621d5a1d0e87c754c743b0010420f88ffffc4e06e003350a3ee7887568743a1f1d53f0",
            ),
            (
                0,
                "00014b4001796400011d5a1f1d621fa63e3a743f0a1443a1d000011c4118f8e9d0e040e800011c75820e87020740011c75820e87020740011c758690e800011c7586a000011d0f3e1a743abf1478b10e8700011d0f3e195fa051c0013505a001350561c1c0013505a0013539aa81a400011d621d74eb3f63a1d1d0f8e870011d621d74eb3f63a1d1d0f8e870011d0e1471e7ec743aac00011d0e1471e7ec743aac00011d0e1471e7ec743aac00011d0e14735024c00174f5ee8743ac43ab43a1d0f8ea98e8760001350a3ee7887568743a1f1d53f001350a3ee7887568743a1f1d53f0",
            ),
        ];
        for (i, (k, data)) in cases.iter().enumerate() {
            let p = Params { k: *k, columns: 61, rows: 23, black_is_1: true, end_of_line: *k >= 0, ..Params::default() };
            let out = decode(&hex(data), &p, 1 << 20).unwrap();
            assert_eq!(out, raw, "case {i} (K {k})");
            let out = decode(&hex(data), &Params { rows: 0, ..p }, 1 << 20).unwrap();
            assert_eq!(out, raw, "case {i} (K {k}), no /Rows");
        }
    }

    #[test]
    fn damaged_data_keeps_the_good_rows() {
        let img = pattern(64, 20, 9);
        let data = encode(&img, -1);
        for cut in [0, 1, data.len() / 3, data.len() - 3] {
            let out = decode(&data[..cut], &Params { k: -1, columns: 64, rows: 20, ..Params::default() }, 1 << 20);
            if let Some(out) = out {
                assert_eq!(out.len(), 20 * 8);
            }
        }
        let mut garbage = data.clone();
        for (i, b) in garbage.iter_mut().enumerate() {
            *b ^= (i * 37) as u8;
        }
        for k in [-1, 0, 2] {
            let _ = decode(&garbage, &Params { k, columns: 64, rows: 20, ..Params::default() }, 1 << 20);
            let _ = decode(&garbage, &Params { k, columns: 1, rows: 0, ..Params::default() }, 1 << 20);
        }
    }

    /// `b1b2` by bisection agrees with the scan it replaced, for every position and colour
    /// over a dense reference row.
    #[test]
    fn b1b2_by_bisection_matches_the_scan() {
        let scan = |refl: &[usize], a0: i64, white: bool, columns: usize| {
            let want = if white { 0 } else { 1 };
            let mut i = 0;
            while i < refl.len() && (refl[i] as i64 <= a0 || i % 2 != want) {
                i += 1;
            }
            (refl.get(i).copied().unwrap_or(columns), refl.get(i + 1).copied().unwrap_or(columns))
        };
        let refl: Vec<usize> = (0..64).map(|i| i * 3 / 2).collect();
        for a0 in -1..100 {
            for white in [true, false] {
                assert_eq!(b1b2(&refl, a0, white, 100), scan(&refl, a0, white, 100), "a0 {a0} white {white}");
            }
        }
    }
}
