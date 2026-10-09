//! `OfxMessageSuiteV1` / `V2`: plug-in messages go to the `log` crate (and, for errors, to the
//! effect so a failed render can report the plug-in's own explanation).
//!
//! The suite's functions are printf-style variadics, which stable Rust cannot receive. They are
//! declared as fixed-slot functions instead (see the module docs of [`crate::ffi`] and
//! [`VarMessageFn`]): eight extra 8-byte slots on Windows x64; six extra integer-class and eight
//! float arguments on System V. [`expand`] then formats the message with the common printf
//! conversions (`%% %s %d %i %u %x %X %o %c %p %f %e %g` with flags, width, precision and the
//! `hh h l ll z j t` length modifiers), consuming at most [`MAX_ARGS`] arguments; a conversion it
//! does not know, or that has no argument left, is kept verbatim.
//!
//! Limitations:
//!
//! * System V: the first two integer arguments travel in registers and the rest on the stack, read
//!   in declaration order. That matches the caller as long as no float argument also overflowed to
//!   the stack (more than eight doubles), which a message never does.
//! * The slots beyond what the caller passed read whatever is on its stack frame; they are never
//!   consumed, because only as many arguments as the format string asks for are taken.
//! * Apple Silicon passes variadic arguments differently and is not expanded: the format string is
//!   logged as written (only `%%` is folded).

use std::ffi::c_char;

use crate::ffi::*;
use crate::instance::EffectObj;

/// The most variadic arguments a message consumes (the number of slots declared for the caller).
const MAX_ARGS: usize = 8;
/// The longest format string or `%s` argument read from the plug-in, in bytes.
const MAX_STR: usize = 4096;
/// The widest field width / precision honoured, so a hostile format cannot allocate gigabytes.
const MAX_WIDTH: usize = 512;

/// Where the variadic argument values come from; the formatting logic is independent of the ABI.
trait ArgSource {
    /// The next integer-class argument (`int`, `long`, a pointer) as raw 8-byte slot contents.
    fn next_int(&mut self) -> Option<u64>;
    /// The next `double` argument.
    fn next_double(&mut self) -> Option<f64>;
}

/// Windows x64: one slot per argument, integers and doubles (as bit patterns) alike.
#[cfg(any(windows, test))]
struct SlotArgs {
    slots: [u64; MAX_ARGS],
    next: usize,
}

#[cfg(any(windows, test))]
impl SlotArgs {
    fn new(slots: [u64; MAX_ARGS]) -> SlotArgs {
        SlotArgs { slots, next: 0 }
    }
    fn take(&mut self) -> Option<u64> {
        let v = self.slots.get(self.next).copied()?;
        self.next += 1;
        Some(v)
    }
}

#[cfg(any(windows, test))]
impl ArgSource for SlotArgs {
    fn next_int(&mut self) -> Option<u64> {
        self.take()
    }
    fn next_double(&mut self) -> Option<f64> {
        self.take().map(f64::from_bits)
    }
}

/// System V: integer-class arguments and doubles come from separate register files.
#[cfg(any(all(not(windows), not(all(target_vendor = "apple", target_arch = "aarch64"))), test))]
struct RegArgs {
    ints: [u64; 6],
    floats: [f64; 8],
    ni: usize,
    nf: usize,
}

#[cfg(any(all(not(windows), not(all(target_vendor = "apple", target_arch = "aarch64"))), test))]
impl RegArgs {
    fn new(ints: [u64; 6], floats: [f64; 8]) -> RegArgs {
        RegArgs { ints, floats, ni: 0, nf: 0 }
    }
}

#[cfg(any(all(not(windows), not(all(target_vendor = "apple", target_arch = "aarch64"))), test))]
impl ArgSource for RegArgs {
    fn next_int(&mut self) -> Option<u64> {
        let v = self.ints.get(self.ni).copied()?;
        self.ni += 1;
        Some(v)
    }
    fn next_double(&mut self) -> Option<f64> {
        let v = self.floats.get(self.nf).copied()?;
        self.nf += 1;
        Some(v)
    }
}

/// Apple Silicon: no variadic arguments are readable.
#[cfg(all(not(windows), target_vendor = "apple", target_arch = "aarch64"))]
struct NoArgs;

#[cfg(all(not(windows), target_vendor = "apple", target_arch = "aarch64"))]
impl ArgSource for NoArgs {
    fn next_int(&mut self) -> Option<u64> {
        None
    }
    fn next_double(&mut self) -> Option<f64> {
        None
    }
}

/// An [`ArgSource`] that stops after [`MAX_ARGS`] arguments.
struct Taker<'a> {
    src: &'a mut dyn ArgSource,
    used: usize,
}

impl Taker<'_> {
    fn int(&mut self) -> Option<u64> {
        if self.used >= MAX_ARGS {
            return None;
        }
        let v = self.src.next_int()?;
        self.used += 1;
        Some(v)
    }
    fn double(&mut self) -> Option<f64> {
        if self.used >= MAX_ARGS {
            return None;
        }
        let v = self.src.next_double()?;
        self.used += 1;
        Some(v)
    }
}

/// Parsed flags, width and precision of one conversion.
#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    zero: bool,
    alt: bool,
    width: usize,
    precision: Option<usize>,
}

/// The size an integer conversion reads from its 8-byte slot.
#[derive(Clone, Copy, PartialEq)]
enum Len {
    Char,
    Short,
    Int,
    Long,
    Wide,
}

impl Len {
    fn bits(self) -> u32 {
        match self {
            Len::Char => 8,
            Len::Short => 16,
            Len::Int => 32,
            // C `long` is 32 bits on Windows and 64 elsewhere.
            Len::Long if cfg!(windows) => 32,
            Len::Long | Len::Wide => 64,
        }
    }
    /// The slot truncated to this size and sign-extended.
    fn signed(self, v: u64) -> i64 {
        match self.bits() {
            8 => i64::from(v as u8 as i8),
            16 => i64::from(v as u16 as i16),
            32 => i64::from(v as u32 as i32),
            _ => v as i64,
        }
    }
    /// The slot truncated to this size.
    fn unsigned(self, v: u64) -> u64 {
        match self.bits() {
            8 => u64::from(v as u8),
            16 => u64::from(v as u16),
            32 => u64::from(v as u32),
            _ => v,
        }
    }
}

/// Format `format` with printf semantics, taking its arguments from `args` and the text behind a
/// `%s` pointer from `read_str`. Never consumes more than [`MAX_ARGS`] arguments.
fn expand(format: &str, args: &mut dyn ArgSource, read_str: &dyn Fn(u64) -> String) -> String {
    let chars: Vec<char> = format.chars().collect();
    let mut taker = Taker { src: args, used: 0 };
    let mut out = String::with_capacity(format.len());
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c != '%' {
            out.push(c);
            continue;
        }
        let start = i - 1;
        match conversion(&chars, &mut i, &mut taker, read_str) {
            Some(text) => out.push_str(&text),
            // Unknown conversion, truncated specifier or no argument left: keep it as written.
            None => out.extend(chars.get(start..i).unwrap_or_default()),
        }
    }
    out
}

/// Digits at `chars[*i..]` as a number (saturating); advances `i` past them.
fn digits(chars: &[char], i: &mut usize) -> usize {
    let mut n = 0usize;
    while let Some(d) = chars.get(*i).and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(d as usize);
        *i += 1;
    }
    n
}

/// Parse and format the conversion that follows a `%` at `chars[*i..]`. `None`: keep the
/// specifier verbatim (`*i` is left after everything that was parsed).
fn conversion(chars: &[char], i: &mut usize, args: &mut Taker, read_str: &dyn Fn(u64) -> String) -> Option<String> {
    let mut spec = Spec::default();
    while let Some(f) = chars.get(*i).copied() {
        match f {
            '-' => spec.left = true,
            '+' => spec.plus = true,
            ' ' => spec.space = true,
            '0' => spec.zero = true,
            '#' => spec.alt = true,
            _ => break,
        }
        *i += 1;
    }
    if chars.get(*i) == Some(&'*') {
        *i += 1;
        let w = args.int()? as u32 as i32;
        spec.left |= w < 0;
        spec.width = (w.unsigned_abs() as usize).min(MAX_WIDTH);
    } else {
        spec.width = digits(chars, i).min(MAX_WIDTH);
    }
    if chars.get(*i) == Some(&'.') {
        *i += 1;
        if chars.get(*i) == Some(&'*') {
            *i += 1;
            let p = args.int()? as u32 as i32;
            spec.precision = usize::try_from(p).ok().map(|p| p.min(MAX_WIDTH));
        } else {
            spec.precision = Some(digits(chars, i).min(MAX_WIDTH));
        }
    }
    let mut len = Len::Int;
    while let Some(m) = chars.get(*i).copied() {
        match m {
            'h' => len = if len == Len::Short { Len::Char } else { Len::Short },
            'l' => len = if len == Len::Long { Len::Wide } else { Len::Long },
            'z' | 'j' | 't' | 'q' | 'L' => len = Len::Wide,
            _ => break,
        }
        *i += 1;
    }
    let conv = chars.get(*i).copied()?;
    *i += 1;
    match conv {
        '%' => Some("%".to_string()),
        'd' | 'i' => {
            let v = len.signed(args.int()?);
            let sign = sign_prefix(v < 0, &spec);
            Some(pad(&sign, &int_digits(v.unsigned_abs().to_string(), &spec), &spec, spec.precision.is_none()))
        }
        'u' | 'x' | 'X' | 'o' => {
            let v = len.unsigned(args.int()?);
            let (text, prefix) = match conv {
                'u' => (v.to_string(), ""),
                'x' => (format!("{v:x}"), if spec.alt && v != 0 { "0x" } else { "" }),
                'X' => (format!("{v:X}"), if spec.alt && v != 0 { "0X" } else { "" }),
                _ => (format!("{v:o}"), if spec.alt && v != 0 { "0" } else { "" }),
            };
            Some(pad(prefix, &int_digits(text, &spec), &spec, spec.precision.is_none()))
        }
        'c' => {
            let ch = char::from(args.int()? as u8);
            Some(pad("", &ch.to_string(), &spec, false))
        }
        's' => {
            let p = args.int()?;
            let mut s = if p == 0 { "(null)".to_string() } else { read_str(p) };
            if let Some(max) = spec.precision {
                s = s.chars().take(max).collect();
            }
            Some(pad("", &s, &spec, false))
        }
        'p' => {
            let p = args.int()?;
            let s = if p == 0 { "(nil)".to_string() } else { format!("0x{p:x}") };
            Some(pad("", &s, &spec, false))
        }
        'f' | 'F' | 'e' | 'E' | 'g' | 'G' => {
            let v = args.double()?;
            let (neg, text) = float_text(v, conv, &spec);
            Some(pad(&sign_prefix(neg, &spec), &text, &spec, v.is_finite()))
        }
        // Includes `%n`, which must never write through a plug-in's pointer.
        _ => None,
    }
}

/// The sign (or `+` / space flag) that goes in front of a number.
fn sign_prefix(negative: bool, spec: &Spec) -> String {
    let sign = if negative {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    sign.to_string()
}

/// Left-pad integer digits with zeros up to the precision (the minimum digit count).
fn int_digits(digits: String, spec: &Spec) -> String {
    match spec.precision {
        Some(p) if digits.len() < p => format!("{}{digits}", "0".repeat(p - digits.len())),
        _ => digits,
    }
}

/// Apply the field width: `sign` + `body`, padded with spaces (or zeros between the sign and the
/// body when the `0` flag applies and `zero_ok`).
fn pad(sign: &str, body: &str, spec: &Spec, zero_ok: bool) -> String {
    let fill = spec.width.saturating_sub(sign.chars().count().saturating_add(body.chars().count()));
    if spec.left {
        format!("{sign}{body}{}", " ".repeat(fill))
    } else if spec.zero && zero_ok {
        format!("{sign}{}{body}", "0".repeat(fill))
    } else {
        format!("{}{sign}{body}", " ".repeat(fill))
    }
}

/// The text of a `%f` / `%e` / `%g` conversion without sign (the sign is returned separately).
fn float_text(v: f64, conv: char, spec: &Spec) -> (bool, String) {
    let upper = conv.is_ascii_uppercase();
    let negative = v.is_sign_negative() && !v.is_nan();
    let a = v.abs();
    if !a.is_finite() {
        let s = if a.is_nan() { "nan" } else { "inf" };
        return (negative, if upper { s.to_uppercase() } else { s.to_string() });
    }
    let precision = spec.precision.unwrap_or(6);
    let text = match conv.to_ascii_lowercase() {
        'f' => format!("{a:.precision$}"),
        'e' => exp_text(a, precision),
        _ => general_text(a, precision, spec.alt),
    };
    (negative, if upper { text.to_uppercase() } else { text })
}

/// C-style `%e` of a non-negative finite number: `d.ddde+XX`.
fn exp_text(a: f64, precision: usize) -> String {
    let rust = format!("{a:.precision$e}");
    let (mantissa, exp) = rust.split_once('e').unwrap_or((rust.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    format!("{mantissa}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.unsigned_abs())
}

/// The decimal exponent of a non-negative finite number as `%e` with `precision` digits shows it.
fn exponent(a: f64, precision: usize) -> i64 {
    let rust = format!("{a:.precision$e}");
    rust.split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0)
}

/// Remove trailing zeros (and a bare `.`) from the fractional part of a number.
fn trim_zeros(s: &str) -> &str {
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s }
}

/// C-style `%g` of a non-negative finite number.
fn general_text(a: f64, precision: usize, alt: bool) -> String {
    let p = precision.max(1);
    let x = exponent(a, p - 1);
    let p_i = i64::try_from(p).unwrap_or(i64::MAX);
    if (-4..p_i).contains(&x) {
        let decimals = usize::try_from(p_i.saturating_sub(1).saturating_sub(x)).unwrap_or(0);
        let fixed = format!("{a:.decimals$}");
        if alt { fixed } else { trim_zeros(&fixed).to_string() }
    } else {
        let sci = exp_text(a, p - 1);
        match sci.split_once('e') {
            Some((mantissa, exp)) if !alt => format!("{}e{exp}", trim_zeros(mantissa)),
            _ => sci.clone(),
        }
    }
}

/// Read the NUL-terminated string at `p`, at most [`MAX_STR`] bytes; invalid UTF-8 is replaced.
///
/// # Safety
/// `p` must be non-null and point to a NUL-terminated string (or to at least [`MAX_STR`] readable
/// bytes).
unsafe fn read_c_str(p: *const u8) -> String {
    let mut bytes = Vec::new();
    for off in 0..MAX_STR {
        // SAFETY: the caller guarantees the string is readable up to its NUL (or `MAX_STR` bytes).
        let b = unsafe { *p.add(off) };
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn post(handle: Handle, kind: *const c_char, format: *const c_char, persistent: bool, args: &mut dyn ArgSource) -> OfxStatus {
    guard(|| {
        // SAFETY: strings come from the plug-in; a `%s` argument is a C string by contract.
        let kind = unsafe { str_arg(kind) }.unwrap_or(MSG_MESSAGE);
        let format = if format.is_null() { String::new() } else { unsafe { read_c_str(format.cast::<u8>()) } };
        let text = expand(&format, args, &|p: u64| unsafe { read_c_str(p as usize as *const u8) });
        let text = text.as_str();
        match kind {
            MSG_FATAL | MSG_ERROR => log::error!("openfx plug-in: {text}"),
            MSG_WARNING => log::warn!("openfx plug-in: {text}"),
            MSG_LOG => log::debug!("openfx plug-in: {text}"),
            _ => log::info!("openfx plug-in: {text}"),
        }
        if (persistent || kind == MSG_ERROR || kind == MSG_FATAL)
            // SAFETY: effect handle or null.
            && let Some(e) = unsafe { EffectObj::from_handle(handle) }
        {
            e.set_message(text);
        }
        if kind == MSG_QUESTION { K_STAT_REPLY_YES } else { K_STAT_OK }
    })
}

/// Declare a printf-style suite function as the fixed-slot function the C caller's arguments land
/// in (see the module docs).
macro_rules! message_fn {
    ($name:ident, $persistent:expr) => {
        #[cfg(windows)]
        #[allow(clippy::too_many_arguments)]
        extern "C" fn $name(
            handle: Handle,
            kind: *const c_char,
            _id: *const c_char,
            format: *const c_char,
            a1: u64,
            a2: u64,
            a3: u64,
            a4: u64,
            a5: u64,
            a6: u64,
            a7: u64,
            a8: u64,
        ) -> OfxStatus {
            post(handle, kind, format, $persistent, &mut SlotArgs::new([a1, a2, a3, a4, a5, a6, a7, a8]))
        }
        #[cfg(all(not(windows), not(all(target_vendor = "apple", target_arch = "aarch64"))))]
        #[allow(clippy::too_many_arguments)]
        extern "C" fn $name(
            handle: Handle,
            kind: *const c_char,
            _id: *const c_char,
            format: *const c_char,
            i1: u64,
            i2: u64,
            i3: u64,
            i4: u64,
            i5: u64,
            i6: u64,
            f1: f64,
            f2: f64,
            f3: f64,
            f4: f64,
            f5: f64,
            f6: f64,
            f7: f64,
            f8: f64,
        ) -> OfxStatus {
            post(handle, kind, format, $persistent, &mut RegArgs::new([i1, i2, i3, i4, i5, i6], [f1, f2, f3, f4, f5, f6, f7, f8]))
        }
        #[cfg(all(not(windows), target_vendor = "apple", target_arch = "aarch64"))]
        extern "C" fn $name(handle: Handle, kind: *const c_char, _id: *const c_char, format: *const c_char) -> OfxStatus {
            post(handle, kind, format, $persistent, &mut NoArgs)
        }
    };
}

message_fn!(message, false);
message_fn!(set_persistent_message, true);

extern "C" fn clear_persistent_message(handle: Handle) -> OfxStatus {
    guard(|| {
        // SAFETY: effect handle or null.
        if let Some(e) = unsafe { EffectObj::from_handle(handle) } {
            e.set_message("");
        }
        K_STAT_OK
    })
}

/// Message suite V1.
pub static SUITE_V1: OfxMessageSuiteV1 = OfxMessageSuiteV1 { message };

/// Message suite V2.
pub static SUITE_V2: OfxMessageSuiteV2 = OfxMessageSuiteV2 { message, set_persistent_message, clear_persistent_message };

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake argument values, independent of any ABI.
    struct Fake {
        ints: Vec<u64>,
        doubles: Vec<f64>,
    }

    impl ArgSource for Fake {
        fn next_int(&mut self) -> Option<u64> {
            if self.ints.is_empty() { None } else { Some(self.ints.remove(0)) }
        }
        fn next_double(&mut self) -> Option<f64> {
            if self.doubles.is_empty() { None } else { Some(self.doubles.remove(0)) }
        }
    }

    /// Pointer values the fake `%s` reader knows.
    fn fake_str(p: u64) -> String {
        match p {
            1 => "world".to_string(),
            2 => "Sapphire".to_string(),
            _ => "?".to_string(),
        }
    }

    fn run(format: &str, ints: &[u64], doubles: &[f64]) -> String {
        expand(format, &mut Fake { ints: ints.to_vec(), doubles: doubles.to_vec() }, &fake_str)
    }

    #[test]
    fn literals_and_percent() {
        assert_eq!(run("plain text", &[], &[]), "plain text");
        assert_eq!(run("100%% sure", &[], &[]), "100% sure");
        assert_eq!(run("", &[], &[]), "");
    }

    #[test]
    fn strings() {
        assert_eq!(run("hello %s", &[1], &[]), "hello world");
        assert_eq!(run("hello %s", &[0], &[]), "hello (null)");
        assert_eq!(run("%.2s", &[1], &[]), "wo");
        assert_eq!(run("[%7s]", &[1], &[]), "[  world]");
        assert_eq!(run("[%-7s]", &[1], &[]), "[world  ]");
    }

    #[test]
    fn integers_are_truncated_per_length() {
        assert_eq!(run("%d days", &[(-5i64) as u64], &[]), "-5 days");
        // A 32-bit `int` ignores the garbage in the upper half of the slot.
        assert_eq!(run("%d", &[0xdead_beef_ffff_fffb], &[]), "-5");
        assert_eq!(run("%i", &[30], &[]), "30");
        assert_eq!(run("%u", &[0x1_0000_0001], &[]), "1");
        assert_eq!(run("%lld", &[1 << 33], &[]), "8589934592");
        assert_eq!(run("%zu", &[1 << 33], &[]), "8589934592");
        assert_eq!(run("%ld", &[1 << 33], &[]), if cfg!(windows) { "0" } else { "8589934592" });
        assert_eq!(run("%hd", &[0x1_0001], &[]), "1");
        assert_eq!(run("%hhd", &[0x1ff], &[]), "-1");
        assert_eq!(run("%x %X %o", &[255, 255, 8], &[]), "ff FF 10");
        assert_eq!(run("%#x", &[255], &[]), "0xff");
        assert_eq!(run("%c%c", &[72, 105], &[]), "Hi");
    }

    #[test]
    fn flags_width_precision() {
        assert_eq!(run("%5d|", &[42], &[]), "   42|");
        assert_eq!(run("%-5d|", &[42], &[]), "42   |");
        assert_eq!(run("%05d", &[42], &[]), "00042");
        assert_eq!(run("%05d", &[(-42i64) as u64], &[]), "-0042");
        assert_eq!(run("%+d", &[7], &[]), "+7");
        assert_eq!(run("% d", &[7], &[]), " 7");
        assert_eq!(run("%.3d", &[7], &[]), "007");
        assert_eq!(run("%*d", &[4, 7], &[]), "   7");
        assert_eq!(run("%-*d|", &[(-4i64) as u64, 7], &[]), "7   |");
        assert_eq!(run("%.*f", &[1], &[2.56]), "2.6");
        // A hostile width is capped, not allocated.
        assert!(run("%999999999999d", &[1], &[]).len() <= MAX_WIDTH);
    }

    #[test]
    fn floats() {
        assert_eq!(run("%f", &[], &[1.5]), "1.500000");
        assert_eq!(run("%.2f", &[], &[1.23456]), "1.23");
        assert_eq!(run("%.0f", &[], &[2.0]), "2");
        assert_eq!(run("%8.3f|", &[], &[1.23456]), "   1.235|");
        assert_eq!(run("%08.3f", &[], &[-1.23456]), "-001.235");
        assert_eq!(run("%e", &[], &[1234.5]), "1.234500e+03");
        assert_eq!(run("%.1E", &[], &[0.00012]), "1.2E-04");
        assert_eq!(run("%g", &[], &[0.0001]), "0.0001");
        assert_eq!(run("%g", &[], &[100000.0]), "100000");
        assert_eq!(run("%g", &[], &[1e6]), "1e+06");
        assert_eq!(run("%g", &[], &[1.5]), "1.5");
        assert_eq!(run("%g", &[], &[0.0]), "0");
        assert_eq!(run("%g", &[], &[0.00001]), "1e-05");
        assert_eq!(run("%.3g", &[], &[1234.5]), "1.23e+03");
        assert_eq!(run("%lf", &[], &[0.25]), "0.250000");
        assert_eq!(run("%f %f %F", &[], &[f64::INFINITY, f64::NAN, f64::NEG_INFINITY]), "inf nan -INF");
    }

    #[test]
    fn pointers() {
        assert_eq!(run("%p", &[0], &[]), "(nil)");
        assert_eq!(run("%p", &[0x1f], &[]), "0x1f");
    }

    #[test]
    fn unknown_and_truncated_conversions_are_verbatim() {
        assert_eq!(run("%y %d", &[3], &[]), "%y 3");
        // `%n` would write through a plug-in pointer: never interpreted, never consumes an argument.
        assert_eq!(run("a%nb %d", &[3], &[]), "a%nb 3");
        assert_eq!(run("50%", &[], &[]), "50%");
        assert_eq!(run("%5", &[], &[]), "%5");
        assert_eq!(run("%-08.3l", &[], &[]), "%-08.3l");
    }

    #[test]
    fn missing_arguments_keep_the_conversion() {
        assert_eq!(run("%d %d", &[1], &[]), "1 %d");
        assert_eq!(run("%s %f", &[], &[]), "%s %f");
    }

    #[test]
    fn never_reads_more_than_eight_arguments() {
        let ints: Vec<u64> = (1..=9).collect();
        assert_eq!(run("%d%d%d%d%d%d%d%d%d", &ints, &[]), "12345678%d");
        // Ints and doubles share the cap.
        assert_eq!(run("%d%d%d%d%d%d%d%d%f", &ints, &[1.0]), "12345678%f");
    }

    #[test]
    fn mixed_arguments_are_taken_from_their_own_files() {
        assert_eq!(run("Licence for %s expires in %d days (%.1f%%)", &[2, 30], &[99.5]), "Licence for Sapphire expires in 30 days (99.5%)");
    }

    #[test]
    fn windows_slots_are_shared_in_argument_order() {
        let mut args = SlotArgs::new([2, 30, 99.5f64.to_bits(), 0, 0, 0, 0, 0]);
        assert_eq!(expand("%s: %d days, %.1f%%", &mut args, &fake_str), "Sapphire: 30 days, 99.5%");
        // Past the last slot nothing is available.
        let mut args = SlotArgs::new([1; MAX_ARGS]);
        assert_eq!(expand("%d%d%d%d%d%d%d%d%d", &mut args, &fake_str), "11111111%d");
    }

    #[test]
    fn system_v_registers_are_separate_files() {
        let mut args = RegArgs::new([2, 30, 0, 0, 0, 0], [99.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(expand("%s: %d days, %.1f%%", &mut args, &fake_str), "Sapphire: 30 days, 99.5%");
        // Six integer slots and eight float slots exist; the 7th integer is not available.
        let mut args = RegArgs::new([1; 6], [0.5; 8]);
        assert_eq!(expand("%d%d%d%d%d%d%d", &mut args, &fake_str), "111111%d");
        let mut args = RegArgs::new([0; 6], [0.5; 8]);
        assert_eq!(expand("%.1f%.1f%.1f%.1f%.1f%.1f%.1f%.1f%.1f", &mut args, &fake_str), "0.50.50.50.50.50.50.50.5%.1f");
    }
}
