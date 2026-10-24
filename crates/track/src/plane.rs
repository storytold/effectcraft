//! Single- or multi-channel float planes: the tracker's working images (a crop of the frame in
//! the matching channel, pre-processed), their pyramids and bilinear sampling.
//!
//! Continuous coordinates follow the raster crate: pixel `i` covers `[i, i + 1)` and its centre is
//! at `i + 0.5`. Downsampling by 2 keeps that convention (level-1 coordinate = level-0 / 2).

use effectcraft_raster::Image;
use serde::{Deserialize, Serialize};

/// What the tracker compares (Motion Tracker Options ▸ Channel).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    /// All three colour channels.
    Rgb,
    /// Rec. 709 luma.
    #[default]
    Luminance,
    /// HSV saturation.
    Saturation,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Rgb, Channel::Luminance, Channel::Saturation];
    pub fn label(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::Luminance => "Luminance",
            Channel::Saturation => "Saturation",
        }
    }
    pub fn from_name(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.label().eq_ignore_ascii_case(s) || format!("{c:?}").eq_ignore_ascii_case(s))
    }
    pub fn channels(self) -> usize {
        if self == Channel::Rgb { 3 } else { 1 }
    }
}

/// An interleaved float plane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub ch: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize, ch: usize) -> Plane {
        Plane { w, h, ch, data: vec![0.0; w * h * ch] }
    }

    #[inline]
    fn idx(&self, x: usize, y: usize) -> usize {
        (y * self.w + x) * self.ch
    }

    /// Value at integer pixel `(x, y)` with edges repeated.
    #[inline]
    pub fn at(&self, x: isize, y: isize, c: usize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[self.idx(x, y) + c]
    }

    /// Bilinear sample at continuous coordinates (edges repeated) into `out[..ch]`.
    #[inline]
    pub fn sample(&self, x: f64, y: f64, out: &mut [f32]) {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let (x0, y0) = (x0 as isize, y0 as isize);
        let inside = x0 >= 0 && y0 >= 0 && (x0 as usize) + 1 < self.w && (y0 as usize) + 1 < self.h;
        if inside {
            let i00 = self.idx(x0 as usize, y0 as usize);
            let i10 = i00 + self.ch;
            let i01 = i00 + self.w * self.ch;
            let i11 = i01 + self.ch;
            for c in 0..self.ch {
                let top = self.data[i00 + c] + (self.data[i10 + c] - self.data[i00 + c]) * tx;
                let bot = self.data[i01 + c] + (self.data[i11 + c] - self.data[i01 + c]) * tx;
                out[c] = top + (bot - top) * ty;
            }
        } else {
            for c in 0..self.ch {
                let a = self.at(x0, y0, c);
                let b = self.at(x0 + 1, y0, c);
                let cc = self.at(x0, y0 + 1, c);
                let d = self.at(x0 + 1, y0 + 1, c);
                let top = a + (b - a) * tx;
                let bot = cc + (d - cc) * tx;
                out[c] = top + (bot - top) * ty;
            }
        }
    }

    /// Crop `w × h` pixels at `(x0, y0)` of `img` in the given channel (edges repeated outside).
    pub fn from_image(img: &Image, channel: Channel, x0: i64, y0: i64, w: usize, h: usize) -> Plane {
        let ch = channel.channels();
        let mut p = Plane::new(w, h, ch);
        if img.is_empty() {
            return p;
        }
        for y in 0..h {
            for x in 0..w {
                let px = img.get_clamped(x0 + x as i64, y0 + y as i64);
                let i = (y * w + x) * ch;
                match channel {
                    Channel::Rgb => p.data[i..i + 3].copy_from_slice(&px[..3]),
                    Channel::Luminance => p.data[i] = 0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2],
                    Channel::Saturation => {
                        let mx = px[0].max(px[1]).max(px[2]);
                        let mn = px[0].min(px[1]).min(px[2]);
                        p.data[i] = if mx > 1e-6 { (mx - mn) / mx } else { 0.0 };
                    }
                }
            }
        }
        p
    }

    /// Separable Gaussian blur (edges repeated).
    pub fn gaussian(&self, sigma: f64) -> Plane {
        if sigma <= 0.05 {
            return self.clone();
        }
        let r = (sigma * 3.0).ceil() as isize;
        let mut k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
        let s: f32 = k.iter().sum();
        k.iter_mut().for_each(|v| *v /= s);
        let mut tmp = Plane::new(self.w, self.h, self.ch);
        for y in 0..self.h {
            for x in 0..self.w {
                for c in 0..self.ch {
                    let mut acc = 0.0;
                    for (j, kv) in k.iter().enumerate() {
                        acc += kv * self.at(x as isize + j as isize - r, y as isize, c);
                    }
                    let i = tmp.idx(x, y) + c;
                    tmp.data[i] = acc;
                }
            }
        }
        let mut out = Plane::new(self.w, self.h, self.ch);
        for y in 0..self.h {
            for x in 0..self.w {
                for c in 0..self.ch {
                    let mut acc = 0.0;
                    for (j, kv) in k.iter().enumerate() {
                        acc += kv * tmp.at(x as isize, y as isize + j as isize - r, c);
                    }
                    let i = out.idx(x, y) + c;
                    out.data[i] = acc;
                }
            }
        }
        out
    }

    /// Edge enhancement (unsharp mask): `x + 2 (x - blur(x))`.
    pub fn enhance(&self) -> Plane {
        let b = self.gaussian(1.5);
        let mut out = self.clone();
        for (o, (x, y)) in out.data.iter_mut().zip(self.data.iter().zip(&b.data)) {
            *o = x + 2.0 * (x - y);
        }
        out
    }

    /// Half resolution (2 × 2 box average; odd sizes round up with the edge repeated).
    pub fn downsample(&self) -> Plane {
        let w = self.w.div_ceil(2).max(1);
        let h = self.h.div_ceil(2).max(1);
        let mut out = Plane::new(w, h, self.ch);
        for y in 0..h {
            for x in 0..w {
                for c in 0..self.ch {
                    let (sx, sy) = (2 * x as isize, 2 * y as isize);
                    let v = self.at(sx, sy, c) + self.at(sx + 1, sy, c) + self.at(sx, sy + 1, c) + self.at(sx + 1, sy + 1, c);
                    let i = out.idx(x, y) + c;
                    out.data[i] = v * 0.25;
                }
            }
        }
        out
    }

    /// Central-difference gradients `(d/dx, d/dy)`.
    pub fn gradients(&self) -> (Plane, Plane) {
        self.gradients_in([0, 0, self.w.saturating_sub(1), self.h.saturating_sub(1)])
    }

    /// The gradients of the pixels in `[x0, y0, x1, y1]` (inclusive, within the plane); all other
    /// pixels of the returned planes stay zero.
    pub fn gradients_in(&self, [x0, y0, x1, y1]: [usize; 4]) -> (Plane, Plane) {
        let mut gx = Plane::new(self.w, self.h, self.ch);
        let mut gy = Plane::new(self.w, self.h, self.ch);
        if self.w == 0 || self.h == 0 {
            return (gx, gy);
        }
        let (ch, row) = (self.ch, self.w * self.ch);
        for y in y0..=y1.min(self.h - 1) {
            for x in x0..=x1.min(self.w - 1) {
                let i = self.idx(x, y);
                if x > 0 && y > 0 && x + 1 < self.w && y + 1 < self.h {
                    for c in i..i + ch {
                        gx.data[c] = 0.5 * (self.data[c + ch] - self.data[c - ch]);
                        gy.data[c] = 0.5 * (self.data[c + row] - self.data[c - row]);
                    }
                } else {
                    for c in 0..ch {
                        let (xi, yi) = (x as isize, y as isize);
                        gx.data[i + c] = 0.5 * (self.at(xi + 1, yi, c) - self.at(xi - 1, yi, c));
                        gy.data[i + c] = 0.5 * (self.at(xi, yi + 1, c) - self.at(xi, yi - 1, c));
                    }
                }
            }
        }
        (gx, gy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_and_pyramid_share_coordinates() {
        let mut p = Plane::new(8, 8, 1);
        for y in 0..8 {
            for x in 0..8 {
                p.data[y * 8 + x] = x as f32 + 10.0 * y as f32;
            }
        }
        let mut o = [0.0];
        p.sample(2.5, 3.5, &mut o);
        assert_eq!(o[0], 32.0);
        p.sample(3.0, 3.5, &mut o);
        assert!((o[0] - 32.5).abs() < 1e-6);
        // A linear ramp keeps its value at matching coordinates on the next level.
        let d = p.downsample();
        d.sample(1.5, 1.5, &mut o);
        let mut o0 = [0.0];
        p.sample(3.0, 3.0, &mut o0);
        assert!((o[0] - o0[0]).abs() < 1e-5, "{} vs {}", o[0], o0[0]);
    }

    #[test]
    fn channels() {
        let img = Image::filled(2, 2, [1.0, 0.5, 0.0, 1.0]);
        assert_eq!(Plane::from_image(&img, Channel::Rgb, 0, 0, 2, 2).data[..3], [1.0, 0.5, 0.0]);
        assert!((Plane::from_image(&img, Channel::Luminance, 0, 0, 2, 2).data[0] - (0.2126 + 0.3576)).abs() < 1e-6);
        assert_eq!(Plane::from_image(&img, Channel::Saturation, 0, 0, 2, 2).data[0], 1.0);
        assert_eq!(Channel::from_name("luminance"), Some(Channel::Luminance));
    }

    #[test]
    fn gradient_window_and_interior_path_match_the_clamped_definition() {
        let mut p = Plane::new(11, 9, 3);
        for (i, v) in p.data.iter_mut().enumerate() {
            *v = ((i * 7919) % 101) as f32 / 37.0;
        }
        let (gx, gy) = p.gradients();
        let rect = [2, 1, 8, 6];
        let (wx, wy) = p.gradients_in(rect);
        for y in 0..p.h {
            for x in 0..p.w {
                for c in 0..3 {
                    let (xi, yi, i) = (x as isize, y as isize, p.idx(x, y) + c);
                    assert_eq!(gx.data[i].to_bits(), (0.5 * (p.at(xi + 1, yi, c) - p.at(xi - 1, yi, c))).to_bits());
                    assert_eq!(gy.data[i].to_bits(), (0.5 * (p.at(xi, yi + 1, c) - p.at(xi, yi - 1, c))).to_bits());
                    let inside = (rect[0]..=rect[2]).contains(&x) && (rect[1]..=rect[3]).contains(&y);
                    assert_eq!(wx.data[i], if inside { gx.data[i] } else { 0.0 });
                    assert_eq!(wy.data[i], if inside { gy.data[i] } else { 0.0 });
                }
            }
        }
    }
}
