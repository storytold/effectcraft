//! Encodes a PNG as lossy WebP stills at several qualities, for comparing sizes and quality:
//! `cargo run -p effectcraft-vp8enc --release --example encode_png -- IN.png OUT_PREFIX Q [Q…]`
//! writes OUT_PREFIX_qQ.webp for each Q (alpha is ignored: the picture is encoded as if opaque).
//! `EFFECTCRAFT_VP8_LF=N` forces the loop filter level (0 = off).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(input), Some(prefix)) = (args.first(), args.get(1)) else {
        eprintln!("usage: encode_png IN.png OUT_PREFIX Q [Q…]");
        std::process::exit(2);
    };
    let img = match image::open(input) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("{input}: {e}");
            std::process::exit(1);
        }
    };
    let (w, h) = img.dimensions();
    let loop_filter = std::env::var("EFFECTCRAFT_VP8_LF").ok().and_then(|v| v.parse::<u8>().ok());
    for q in args.iter().skip(2).filter_map(|q| q.parse::<u8>().ok()) {
        let t = std::time::Instant::now();
        match effectcraft_vp8enc::encode_rgba(img.as_raw(), w, h, &effectcraft_vp8enc::Vp8Params { quality: q, loop_filter }) {
            Ok(f) => {
                let mut chunk = b"VP8 ".to_vec();
                chunk.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
                chunk.extend_from_slice(&f.data);
                if f.data.len() % 2 == 1 {
                    chunk.push(0);
                }
                let mut file = b"RIFF".to_vec();
                file.extend_from_slice(&(4 + chunk.len() as u32).to_le_bytes());
                file.extend_from_slice(b"WEBP");
                file.extend_from_slice(&chunk);
                let out = format!("{prefix}_q{q}.webp");
                if let Err(e) = std::fs::write(&out, &file) {
                    eprintln!("{out}: {e}");
                }
                println!("q{q}: {} bytes (q_index {}, lf {}) in {:.0} ms", file.len(), f.q_index, f.loop_filter, t.elapsed().as_secs_f64() * 1000.0);
            }
            Err(e) => eprintln!("q{q}: {e}"),
        }
    }
}
