//! Developer tool: replaces every embedded ICC profile of an .aep / .aepx by a stub that keeps only the
//! profile's description. Fixture hygiene: a saved project carries the vendor's working-space profile
//! (a few KB of colour data); the importer reads only the profile's name, the `desc` tag, so the stub
//! (a 128-byte header, one `desc` tag, no colour data, made here) imports identically.
//!
//! `cargo run -p effectcraft-aep --example stubicc -- in.aep out.aep`
//!
//! It handles the JSON `"colorProfileData":"<base64>"` in `Utf8` chunks (or anywhere in the text of an
//! .aepx) and a raw ICC profile stored as a chunk of its own (the profile of an imported image).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
use effectcraft_aep::{Body, Chunk};

const KEY: &[u8] = b"\"colorProfileData\":\"";
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

/// Replaces every profile value in `text`; returns the new text and how many were replaced.
fn stub_text(text: &[u8]) -> (Vec<u8>, usize) {
    let (mut out, mut n, mut i) = (Vec::with_capacity(text.len()), 0, 0);
    while let Some(&b) = text.get(i) {
        let value = text.get(i..).filter(|t| t.starts_with(KEY)).and_then(|t| {
            let rest = t.get(KEY.len()..)?;
            let len = rest.iter().position(|b| *b == b'"')?;
            let stubbed = description(&b64_decode(rest.get(..len)?)?).and_then(|d| stub(&d))?;
            Some((len, b64_encode(&stubbed)))
        });
        match value {
            Some((len, enc)) => {
                out.extend_from_slice(KEY);
                out.extend_from_slice(&enc);
                i += KEY.len() + len;
                n += 1;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    (out, n)
}

fn walk(chunks: &mut [Chunk<'_>], n: &mut usize) {
    for c in chunks.iter_mut() {
        let data = c.bytes().map(<[u8]>::to_vec);
        match data {
            Some(d) if c.id == *b"Utf8" && d.windows(KEY.len()).any(|w| w == KEY) => {
                let (new, k) = stub_text(&d);
                *c = Chunk::data(*b"Utf8", new);
                *n += k;
            }
            Some(d) if d.len() > 132 && d.get(36..40) == Some(b"acsp") => {
                if let Some(s) = description(&d).and_then(|t| stub(&t)) {
                    *c = Chunk::data(c.id, s);
                    *n += 1;
                }
            }
            _ => {
                if let Body::List { children, .. } | Body::Chunks(children) = &mut c.body {
                    walk(children, n);
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(input), Some(output)) = (args.get(1), args.get(2)) else { exit("usage: stubicc <in.aep|in.aepx> <out>") };
    let bytes = std::fs::read(input).or_exit("read the input");
    let (out, n) = if input.ends_with(".aepx") {
        stub_text(&bytes)
    } else {
        let mut file = effectcraft_aep::parse(&bytes).or_exit("parse the input");
        let mut n = 0;
        walk(&mut file.chunks, &mut n);
        (effectcraft_aep::write(&file).or_exit("write the output"), n)
    };
    std::fs::write(output, out).or_exit("write the output");
    println!("stubbed {n} profile(s)");
}
