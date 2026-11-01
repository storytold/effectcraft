#![allow(dead_code)]

use effectcraft_av1enc::{Encoder, EncoderConfig, Frame, Packet};

/// A synthetic 4:2:0 picture.
pub struct Pic {
    pub w: usize,
    pub h: usize,
    pub y: Vec<u16>,
    pub u: Vec<u16>,
    pub v: Vec<u16>,
}

impl Pic {
    pub fn frame(&self) -> Frame<'_> {
        Frame { y: &self.y, u: &self.u, v: &self.v, y_stride: self.w, uv_stride: self.w.div_ceil(2) }
    }
}

/// Smooth gradients with a soft blob, moved by (dx, dy) samples per frame index `t`, plus a
/// textured rectangle that moves differently.
pub fn synth(w: usize, h: usize, bd: u32, t: usize) -> Pic {
    let max = ((1u32 << bd) - 1) as f64;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let sample = |x: f64, y: f64| -> f64 {
        let fx = x - 2.0 * t as f64;
        let fy = y - 1.0 * t as f64;
        let base = 0.25 + 0.5 * (fx / 97.0).sin() * (fy / 61.0).cos() * 0.5 + 0.2 * ((fx + fy) / (w + h) as f64);
        let d = ((fx - w as f64 / 2.0).powi(2) + (fy - h as f64 / 2.0).powi(2)).sqrt();
        let blob = 0.3 * (-d * d / (2.0 * (w.min(h) as f64 / 5.0).powi(2))).exp();
        // a checkered patch moving right by 4 per frame
        let px = x - 4.0 * t as f64 - w as f64 / 4.0;
        let py = y - h as f64 / 3.0;
        let patch =
            if (0.0..24.0).contains(&px) && (0.0..16.0).contains(&py) { if ((px as i32 / 4) + (py as i32 / 4)) % 2 == 0 { 0.25 } else { -0.25 } } else { 0.0 };
        (base + blob + patch).clamp(0.0, 1.0)
    };
    let mut y = vec![0u16; w * h];
    for j in 0..h {
        for i in 0..w {
            y[j * w + i] = (sample(i as f64, j as f64) * max).round() as u16;
        }
    }
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    for j in 0..ch {
        for i in 0..cw {
            let s = sample(2.0 * i as f64, 2.0 * j as f64);
            u[j * cw + i] = ((0.5 + 0.3 * (s - 0.5) + 0.1 * (i as f64 / 13.0).sin()) * max).round() as u16;
            v[j * cw + i] = ((0.5 - 0.25 * (s - 0.5) + 0.1 * (j as f64 / 7.0).cos()) * max).round() as u16;
        }
    }
    Pic { w, h, y, u, v }
}

/// Deterministic noise picture (hard case for prediction).
pub fn noise(w: usize, h: usize, bd: u32, seed: u32) -> Pic {
    let mut s = seed.wrapping_mul(2_654_435_761).max(1);
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    let mask = (1u32 << bd) - 1;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    Pic {
        w,
        h,
        y: (0..w * h).map(|_| (rnd() & mask) as u16).collect(),
        u: (0..cw * ch).map(|_| (rnd() & mask) as u16).collect(),
        v: (0..cw * ch).map(|_| (rnd() & mask) as u16).collect(),
    }
}

pub fn psnr(a: &[u16], b: &[u16], bd: u32) -> f64 {
    let max = ((1u32 << bd) - 1) as f64;
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (max * max / mse).log10() }
}

pub struct Run {
    pub packets: Vec<Packet>,
    pub recons: Vec<[Vec<u16>; 3]>,
    pub psnr_y: Vec<f64>,
    pub lf_levels: Vec<u32>,
}

/// Encodes `pics`, checks every packet decodes (filmcraft-av1) to exactly the encoder's
/// reconstruction and returns packets, reconstructions and luma PSNRs.
pub fn encode_and_check(cfg: EncoderConfig, pics: &[Pic]) -> Run {
    let bd = cfg.bit_depth as u32;
    let mut enc = Encoder::new(cfg).expect("config");
    // single-threaded: each packet's picture comes out of its own decode call
    let mut dec = filmcraft_av1::Decoder::with_threads(1);
    let mut run = Run { packets: Vec::new(), recons: Vec::new(), psnr_y: Vec::new(), lf_levels: Vec::new() };
    for (i, p) in pics.iter().enumerate() {
        let pkt = enc.encode(&p.frame());
        let rec = enc.last_reconstruction().expect("reconstruction");
        let out = dec.decode(&pkt.data).unwrap_or_else(|e| panic!("frame {i}: decode error {e:?}"));
        assert_eq!(out.len(), 1, "frame {i}: one picture per packet");
        let d = &out[0];
        assert_eq!((d.width as usize, d.height as usize), (p.w, p.h));
        for pl in 0..3 {
            if d.planes[pl] != rec[pl] {
                let first = d.planes[pl].iter().zip(&rec[pl]).position(|(a, b)| a != b).unwrap_or(usize::MAX);
                let pw = if pl == 0 { p.w } else { p.w.div_ceil(2) };
                panic!("frame {i} plane {pl}: decoder output differs from the reconstruction (first at x {} y {})", first % pw, first / pw);
            }
        }
        run.psnr_y.push(psnr(&p.y, &rec[0], bd));
        run.lf_levels.push(enc.last_loop_filter_level().expect("level"));
        run.recons.push(rec);
        run.packets.push(pkt);
    }
    run
}
