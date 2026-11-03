//! Developer tool: replaces every embedded ICC profile of an .aep / .aepx by a stub that keeps only the
//! profile's description. Fixture hygiene: a saved project carries the vendor's working-space profile
//! (a few KB of colour data); the importer reads only the profile's name, the `desc` tag, so the stub
//! (a 128-byte header, one `desc` tag, no colour data, made here) imports identically.
//!
//! `cargo run -p effectcraft-aep --example stubicc -- in.aep out.aep`
//!
//! It handles:
//! - the JSON value `"colorProfileData": "<base64>"` in any chunk of an .aep and anywhere in the text
//!   of an .aepx (white space around the colon, `\/` escapes and quotes written as `&quot;` allowed);
//! - a raw ICC profile stored as a chunk of its own (the profile of an imported image), in an .aep
//!   as the chunk's bytes and in an .aepx as its `bdata="<hex>"` attribute.
//!
//! A profile it cannot replace (no readable description, data that does not decode) stops the tool
//! before anything is written. Check the result independently (the corpus has
//! `tools/aep/inspect-embedded.py`), not by this tool's own count.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use effectcraft_aep::{Body, Chunk};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Ends this developer tool with a message instead of a panic.
trait OrExit<T> {
    fn or_exit(self, what: &str) -> T;
}

impl<T> OrExit<T> for Option<T> {
    fn or_exit(self, what: &str) -> T {
        match self {
            Some(v) => v,
            None => exit(what),
        }
    }
}

impl<T, E: std::fmt::Debug> OrExit<T> for Result<T, E> {
    fn or_exit(self, what: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => exit(&format!("{what}: {e:?}")),
        }
    }
}

fn exit(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

fn b64_decode(text: &[u8]) -> Option<Vec<u8>> {
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0u32);
    for &c in text.iter().filter(|c| **c != b'=') {
        let v = u32::try_from(B64.iter().position(|b| *b == c)?).ok()?;
        acc = (acc << 6 | v) & 0x00ff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn b64_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for ch in data.chunks(3) {
        let at = |i: usize| u32::from(ch.get(i).copied().unwrap_or(0));
        let n = (at(0) << 16) | (at(1) << 8) | at(2);
        for i in 0..4usize {
            if i <= ch.len() {
                out.push(B64.get(((n >> (18 - 6 * i)) & 63) as usize).copied().unwrap_or(b'A'));
            } else {
                out.push(b'=');
            }
        }
    }
    out
}

fn be32(d: &[u8], at: usize) -> Option<usize> {
    let b: [u8; 4] = d.get(at..at.checked_add(4)?)?.try_into().ok()?;
    usize::try_from(u32::from_be_bytes(b)).ok()
}

/// The profile description (ICC.1 `desc`: v2 ASCII or v4 `mluc`, first record).
fn description(icc: &[u8]) -> Option<String> {
    let count = be32(icc, 128)?;
    let tag = (0..count.min(1024)).find_map(|i| {
        let at = 132usize.checked_add(i.checked_mul(12)?)?;
        (icc.get(at..at.checked_add(4)?)? == b"desc").then_some(at)
    })?;
    let off = be32(icc, tag.checked_add(4)?)?;
    let body = icc.get(off..off.checked_add(be32(icc, tag.checked_add(8)?)?)?)?;
    let text = match body.get(..4)? {
        b"desc" => {
            let n = be32(body, 8)?;
            String::from_utf8_lossy(body.get(12..12usize.checked_add(n)?)?.split(|b| *b == 0).next()?).into_owned()
        }
        b"mluc" => {
            let (n, at) = (be32(body, 20)?, be32(body, 24)?);
            let units: Vec<u16> = body.get(at..at.checked_add(n)?)?.chunks(2).filter_map(|p| Some(u16::from_be_bytes([*p.first()?, *p.get(1)?]))).collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    Some(text.trim_end_matches('\0').trim().to_owned())
}

/// A profile with a header, one `desc` tag and no colour data.
fn stub(desc: &str) -> Option<Vec<u8>> {
    let mut body = b"desc\0\0\0\0".to_vec();
    body.extend_from_slice(&u32::try_from(desc.len().checked_add(1)?).ok()?.to_be_bytes());
    body.extend_from_slice(desc.as_bytes());
    body.push(0);
    body.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]);
    let mut p = vec![0u8; 128];
    for (at, v) in [(8, &0x0240_0000u32.to_be_bytes()[..]), (12, b"mntr"), (16, b"RGB "), (20, b"XYZ "), (36, b"acsp")] {
        p.get_mut(at..at + v.len())?.copy_from_slice(v);
    }
    p.extend_from_slice(&1u32.to_be_bytes());
    p.extend_from_slice(b"desc");
    p.extend_from_slice(&144u32.to_be_bytes());
    p.extend_from_slice(&u32::try_from(body.len()).ok()?.to_be_bytes());
    p.extend_from_slice(&body);
    let size = u32::try_from(p.len()).ok()?.to_be_bytes();
    p.get_mut(..4)?.copy_from_slice(&size);
    Some(p)
}

/// The JSON key, without its quotes.
const KEY: &[u8] = b"colorProfileData";
/// How a JSON quote is written: as is (a `Utf8` chunk, the `<string>` text of an .aepx) and as an
/// XML entity (JSON inside an attribute of an .aepx).
const QUOTES: [&[u8]; 2] = [b"\"", b"&quot;"];

/// What the tool did, counted.
#[derive(Default)]
struct Tally {
    stubbed: usize,
    /// Values too short to be a profile (no ICC header), kept as they are.
    kept_short: usize,
    /// Profiles that could not be replaced (no readable description, undecodable data): the tool
    /// fails, so no profile is ever left behind silently.
    failed: Vec<String>,
}

fn is_profile(d: &[u8]) -> bool {
    d.len() > 132 && d.get(36..40) == Some(b"acsp")
}

/// The stub for the bytes `d` of a value, or `None` (counted) when it holds no profile or the
/// profile has no readable description.
fn stub_of(d: &[u8], at: usize, tally: &mut Tally) -> Option<Vec<u8>> {
    if !is_profile(d) {
        tally.kept_short += 1;
        return None;
    }
    let s = description(d).and_then(|t| stub(&t));
    if s.is_none() {
        tally.failed.push(format!("profile at byte {at}: no readable description"));
    }
    s
}

/// At the start of `t`: the key in quotes, optional white space, a colon, optional white space and
/// the opening quote of the value. Returns the length up to and including that quote, and the quote.
fn value_start(t: &[u8]) -> Option<(usize, &'static [u8])> {
    let ws = |mut i: usize| {
        while t.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    QUOTES.into_iter().find_map(|q| {
        let key_end = q.len().checked_mul(2)?.checked_add(KEY.len())?;
        let quoted = t.get(..key_end)?;
        if !(quoted.starts_with(q) && quoted.get(q.len()..q.len() + KEY.len())? == KEY && quoted.ends_with(q)) {
            return None;
        }
        let colon = ws(key_end);
        if t.get(colon) != Some(&b':') {
            return None;
        }
        let open = ws(colon + 1);
        t.get(open..)?.starts_with(q).then_some((open + q.len(), q))
    })
}

fn hex_decode(h: &[u8]) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) {
        return None;
    }
    h.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
}

fn hex_encode(d: &[u8], upper: bool) -> Vec<u8> {
    d.iter().flat_map(|b| if upper { format!("{b:02X}") } else { format!("{b:02x}") }.into_bytes()).collect()
}

/// Replaces, in `text`, every `colorProfileData` JSON value (base64; `\/` escapes, white space
/// around the colon and quotes written as `&quot;` allowed) and every .aepx `bdata="<hex>"`
/// attribute that holds a raw profile.
fn stub_text(text: &[u8], tally: &mut Tally) -> Vec<u8> {
    const BDATA: &[u8] = b"bdata=\"";
    let (mut out, mut i) = (Vec::with_capacity(text.len()), 0);
    while let Some(&b) = text.get(i) {
        let rest = text.get(i..).unwrap_or_default();
        if let Some((open, q)) = value_start(rest)
            && let Some(len) = rest.get(open..).and_then(|v| v.windows(q.len()).position(|w| w == q))
        {
            let raw = rest.get(open..open + len).unwrap_or_default();
            let unescaped: Vec<u8> = raw.iter().copied().filter(|c| *c != b'\\').collect();
            let replaced = match b64_decode(&unescaped) {
                Some(d) => stub_of(&d, i, tally).map(|s| b64_encode(&s)),
                None => {
                    tally.failed.push(format!("colorProfileData at byte {i}: not base64"));
                    None
                }
            };
            out.extend_from_slice(rest.get(..open).unwrap_or_default());
            match replaced {
                Some(enc) => {
                    out.extend_from_slice(&enc);
                    tally.stubbed += 1;
                }
                None => out.extend_from_slice(raw),
            }
            i += open + len;
            continue;
        }
        if rest.starts_with(BDATA)
            && let Some(len) = rest.get(BDATA.len()..).and_then(|v| v.iter().position(|c| *c == b'"'))
        {
            let hex = rest.get(BDATA.len()..BDATA.len() + len).unwrap_or_default();
            let replaced = hex_decode(hex).filter(|d| is_profile(d)).and_then(|d| stub_of(&d, i, tally));
            out.extend_from_slice(BDATA);
            match replaced {
                Some(s) => {
                    out.extend_from_slice(&hex_encode(&s, hex.iter().any(u8::is_ascii_uppercase)));
                    tally.stubbed += 1;
                }
                None => out.extend_from_slice(hex),
            }
            i += BDATA.len() + len;
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

fn walk(chunks: &mut [Chunk<'_>], tally: &mut Tally) {
    for c in chunks.iter_mut() {
        // Only a data chunk is rewritten as one: an opaque `LIST` also has bytes, but replacing it by
        // a data chunk would drop its list type.
        let data = if let Body::Data(d) = &c.body { Some(d.to_vec()) } else { None };
        match data {
            Some(d) if d.windows(KEY.len()).any(|w| w == KEY) => {
                let new = stub_text(&d, tally);
                *c = Chunk::data(c.id, new);
            }
            Some(d) if is_profile(&d) => {
                if let Some(s) = stub_of(&d, 0, tally) {
                    *c = Chunk::data(c.id, s);
                    tally.stubbed += 1;
                }
            }
            _ => {
                if let Body::List { children, .. } | Body::Chunks(children) = &mut c.body {
                    walk(children, tally);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(input), Some(output)) = (args.get(1), args.get(2)) else { exit("usage: stubicc <in.aep|in.aepx> <out>") };
    let bytes = std::fs::read(input).or_exit("read the input");
    let mut tally = Tally::default();
    let out = if input.ends_with(".aepx") {
        stub_text(&bytes, &mut tally)
    } else {
        let mut file = effectcraft_aep::parse(&bytes).or_exit("parse the input");
        walk(&mut file.chunks, &mut tally);
        effectcraft_aep::write(&file).or_exit("write the output")
    };
    if !tally.failed.is_empty() {
        exit(&format!("nothing written: {} profile(s) could not be replaced:\n{}", tally.failed.len(), tally.failed.join("\n")));
    }
    std::fs::write(output, out).or_exit("write the output");
    println!("stubbed {} profile(s); kept {} short value(s) that hold no profile", tally.stubbed, tally.kept_short);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn profile(desc: &str) -> Vec<u8> {
        // A profile with colour data after the description (like a real one): the stub of `desc`
        // plus 3000 bytes, its size field updated.
        let mut p = stub(desc).unwrap();
        p.extend(std::iter::repeat_n(0x5a, 3000));
        let size = u32::try_from(p.len()).unwrap().to_be_bytes();
        p[..4].copy_from_slice(&size);
        p
    }

    fn stubbed(text: &[u8]) -> (Vec<u8>, Tally) {
        let mut tally = Tally::default();
        (stub_text(text, &mut tally), tally)
    }

    #[test]
    fn json_values_in_every_spelling_are_replaced() {
        let b64 = String::from_utf8(b64_encode(&profile("sRGB IEC61966-2.1"))).unwrap();
        let want = String::from_utf8(b64_encode(&stub("sRGB IEC61966-2.1").unwrap())).unwrap();
        for (before, after) in [
            (format!("{{\"colorProfileData\":\"{b64}\"}}"), format!("{{\"colorProfileData\":\"{want}\"}}")),
            (format!("{{\"colorProfileData\" : \n\t\"{b64}\"}}"), format!("{{\"colorProfileData\" : \n\t\"{want}\"}}")),
            (format!("{{\"colorProfileData\":\"{}\"}}", b64.replace('/', "\\/")), format!("{{\"colorProfileData\":\"{want}\"}}")),
            (format!("{{&quot;colorProfileData&quot;: &quot;{b64}&quot;}}"), format!("{{&quot;colorProfileData&quot;: &quot;{want}&quot;}}")),
        ] {
            let (out, tally) = stubbed(before.as_bytes());
            assert_eq!(String::from_utf8(out).unwrap(), after);
            assert_eq!((tally.stubbed, tally.failed.len()), (1, 0));
        }
    }

    #[test]
    fn a_raw_profile_in_aepx_hex_is_replaced_and_other_hex_kept() {
        let hex = String::from_utf8(hex_encode(&profile("Rec.709 Gamma 2.4"), false)).unwrap();
        let want = String::from_utf8(hex_encode(&stub("Rec.709 Gamma 2.4").unwrap(), false)).unwrap();
        let text = format!("<icc bdata=\"{hex}\"/><PwCs bdata=\"01\"/><cdta bdata=\"{}\"/>", "ab".repeat(200));
        let (out, tally) = stubbed(text.as_bytes());
        assert_eq!(String::from_utf8(out).unwrap(), format!("<icc bdata=\"{want}\"/><PwCs bdata=\"01\"/><cdta bdata=\"{}\"/>", "ab".repeat(200)));
        assert_eq!((tally.stubbed, tally.failed.len()), (1, 0));
        // Upper-case hex stays upper case.
        let (out, _) = stubbed(format!("bdata=\"{}\"", hex.to_uppercase()).as_bytes());
        assert_eq!(String::from_utf8(out).unwrap(), format!("bdata=\"{}\"", want.to_uppercase()));
    }

    #[test]
    fn a_profile_that_cannot_be_replaced_is_a_failure_not_a_silent_copy() {
        // An ICC header whose tag table has no `desc`.
        let mut p = profile("x");
        p[132..136].copy_from_slice(b"wtpt");
        let (_, tally) = stubbed(format!("\"colorProfileData\":\"{}\"", String::from_utf8(b64_encode(&p)).unwrap()).as_bytes());
        assert_eq!((tally.stubbed, tally.failed.len()), (0, 1));
        let (_, tally) = stubbed(b"\"colorProfileData\":\"not*base64\"");
        assert_eq!(tally.failed.len(), 1);
        // A short value holds no profile: kept and counted, not a failure.
        let (out, tally) = stubbed(b"\"colorProfileData\":\"AAAA\"");
        assert_eq!((out.as_slice(), tally.kept_short, tally.failed.len()), (&b"\"colorProfileData\":\"AAAA\""[..], 1, 0));
    }
}
