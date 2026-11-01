//! The non-matmul kernels, as methods of [`Gpu`] (each queues one dispatch).

use super::{Buf, Gpu};
use crate::{Error, Result};

fn n32(v: usize, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| Error::Compute(format!("{what} {v} does not fit a GPU index")))
}

/// A 2-D grid of groups covering `groups` (a dimension holds 65535).
fn grid(groups: usize) -> Result<(u32, u32, u32)> {
    let gx = groups.clamp(1, 65535);
    Ok((n32(gx, "groups")?, n32(groups.div_ceil(gx).max(1), "groups")?, 1))
}

/// One 256-thread group per 256 items.
fn items(n: usize) -> Result<(u32, u32, u32)> {
    grid(n.div_ceil(256))
}

fn need(buf: &Buf, len: usize, what: &str) -> Result<()> {
    if buf.len < len {
        return Err(Error::Input(format!("{what}: a buffer of {} elements is smaller than the {len} needed", buf.len)));
    }
    Ok(())
}

impl Gpu {
    /// `y = layer_norm(x) * weight + bias` over the last dimension of a `[rows, cols]` buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn layernorm(&self, x: &Buf, weight: &Buf, bias: &Buf, y: &Buf, rows: usize, cols: usize, eps: f32, gelu: bool) -> Result<()> {
        need(x, rows * cols, "layernorm x")?;
        need(y, rows * cols, "layernorm y")?;
        need(weight, cols, "layernorm weight")?;
        need(bias, cols, "layernorm bias")?;
        self.run(
            "layernorm",
            &[(0, x), (1, weight), (2, bias), (3, y)],
            &[n32(rows, "rows")?, n32(cols, "cols")?, eps.to_bits(), u32::from(gelu)],
            grid(rows)?,
        );
        Ok(())
    }

    /// Softmax over each of `rows` rows of `cols`, in place.
    pub fn softmax(&self, x: &Buf, rows: usize, cols: usize) -> Result<()> {
        need(x, rows * cols, "softmax")?;
        self.run("softmax", &[(3, x)], &[n32(rows, "rows")?, n32(cols, "cols")?], grid(rows)?);
        Ok(())
    }

    /// A convolution over channels-last `[h, w, ci]` with weight `[co, ci / groups, k, k]` and bias `[co]`,
    /// output `[ho, wo, co]` (`groups == ci == co` is depthwise).
    #[allow(clippy::too_many_arguments)]
    pub fn conv2d(
        &self,
        x: &Buf,
        weight: &Buf,
        bias: &Buf,
        out: &Buf,
        dims: (usize, usize, usize, usize),
        k: usize,
        stride: usize,
        pad: usize,
        groups: usize,
    ) -> Result<()> {
        let (h, w, ci, co) = dims;
        if k == 0 || stride == 0 || groups == 0 || ci % groups != 0 || co % groups != 0 || h + 2 * pad < k || w + 2 * pad < k {
            return Err(Error::Input(format!("a {k}x{k} convolution (stride {stride}, groups {groups}) does not fit {w}x{h}x{ci} -> {co}")));
        }
        let (ho, wo) = ((h + 2 * pad - k) / stride + 1, (w + 2 * pad - k) / stride + 1);
        need(x, h * w * ci, "conv2d input")?;
        need(weight, co * (ci / groups) * k * k, "conv2d weight")?;
        need(bias, co, "conv2d bias")?;
        need(out, ho * wo * co, "conv2d output")?;
        let n = ho * wo * co;
        self.run(
            "conv2d",
            &[(0, x), (1, weight), (2, bias), (3, out)],
            &[
                n32(n, "n")?,
                n32(h, "h")?,
                n32(w, "w")?,
                n32(ci, "ci")?,
                n32(co, "co")?,
                n32(ho, "ho")?,
                n32(wo, "wo")?,
                n32(k, "k")?,
                n32(stride, "stride")?,
                n32(pad, "pad")?,
                n32(groups, "groups")?,
            ],
            items(n)?,
        );
        Ok(())
    }

    /// `out = a + b` over `n` elements.
    pub fn add(&self, a: &Buf, b: &Buf, out: &Buf, n: usize) -> Result<()> {
        need(a, n, "add a")?;
        need(b, n, "add b")?;
        need(out, n, "add out")?;
        self.run("add", &[(0, a), (1, b), (3, out)], &[n32(n, "n")?], items(n)?);
        Ok(())
    }

    /// `[h, w, c]` to `[windows, ws, ws, c]`, zero padded at the bottom and right.
    pub fn window_partition(&self, x: &Buf, out: &Buf, h: usize, w: usize, c: usize, ws: usize) -> Result<()> {
        if ws == 0 {
            return Err(Error::Input("window size 0".into()));
        }
        let (nwx, nwy) = (w.div_ceil(ws), h.div_ceil(ws));
        let n = nwx * nwy * ws * ws * c;
        need(x, h * w * c, "window_partition input")?;
        need(out, n, "window_partition output")?;
        self.run(
            "window_partition",
            &[(0, x), (3, out)],
            &[n32(n, "n")?, n32(h, "h")?, n32(w, "w")?, n32(c, "c")?, n32(ws, "ws")?, n32(nwx, "windows")?],
            items(n)?,
        );
        Ok(())
    }

    /// The inverse of [`Gpu::window_partition`] (the padding is dropped).
    pub fn window_unpartition(&self, x: &Buf, out: &Buf, h: usize, w: usize, c: usize, ws: usize) -> Result<()> {
        if ws == 0 {
            return Err(Error::Input("window size 0".into()));
        }
        let (nwx, nwy) = (w.div_ceil(ws), h.div_ceil(ws));
        need(x, nwx * nwy * ws * ws * c, "window_unpartition input")?;
        need(out, h * w * c, "window_unpartition output")?;
        self.run(
            "window_unpartition",
            &[(0, x), (3, out)],
            &[n32(h * w * c, "n")?, n32(h, "h")?, n32(w, "w")?, n32(c, "c")?, n32(ws, "ws")?, n32(nwx, "windows")?],
            items(h * w * c)?,
        );
        Ok(())
    }

    /// 2 x 2 max pool of `nb` images of `[h, w, c]` tokens (`h` and `w` even) read with the given strides
    /// (image, row, token) so it can pool one third of a qkv buffer in place; output contiguous.
    #[allow(clippy::too_many_arguments)]
    pub fn maxpool2(&self, x: &Buf, out: &Buf, nb: usize, h: usize, w: usize, c: usize, strides: (usize, usize, usize)) -> Result<()> {
        if !h.is_multiple_of(2) || !w.is_multiple_of(2) {
            return Err(Error::Input(format!("maxpool2 needs even sizes, got {w}x{h}")));
        }
        let n = nb * (h / 2) * (w / 2) * c;
        let last = (nb.saturating_sub(1)) * strides.0 + (h - 1) * strides.1 + (w - 1) * strides.2 + c;
        need(x, last, "maxpool2 input")?;
        need(out, n, "maxpool2 output")?;
        self.run(
            "maxpool2",
            &[(0, x), (3, out)],
            &[
                n32(n, "n")?,
                n32(nb, "nb")?,
                n32(h, "h")?,
                n32(w, "w")?,
                n32(c, "c")?,
                n32(strides.0, "stride")?,
                n32(strides.1, "stride")?,
                n32(strides.2, "stride")?,
            ],
            items(n)?,
        );
        Ok(())
    }

    /// The patch embedding: `[3, h, w]` pixels, weight `[co, 3, 7, 7]`, bias `[co]`; output `[h/4, w/4, co]`.
    pub fn patch_embed(&self, pixels: &Buf, weight: &Buf, bias: &Buf, out: &Buf, (h, w, co): (usize, usize, usize)) -> Result<()> {
        let (ho, wo) = ((h + 6 - 7) / 4 + 1, (w + 6 - 7) / 4 + 1);
        need(pixels, 3 * h * w, "patch_embed pixels")?;
        need(weight, co * 3 * 49, "patch_embed weight")?;
        need(bias, co, "patch_embed bias")?;
        need(out, ho * wo * co, "patch_embed output")?;
        let n = ho * wo * co;
        self.run(
            "patch_embed",
            &[(0, pixels), (1, weight), (2, bias), (3, out)],
            &[n32(n, "n")?, n32(h, "h")?, n32(w, "w")?, n32(co, "co")?, n32(ho, "ho")?, n32(wo, "wo")?],
            items(n)?,
        );
        Ok(())
    }

    /// Rotary encoding of `[rows, cols]`: rows below `rotated` are rotated with the `[table_rows, cols]`
    /// tables (row `r` uses table row `r % table_rows`), the rest copied.
    #[allow(clippy::too_many_arguments)]
    pub fn rope(&self, x: &Buf, cos: &Buf, sin: &Buf, out: &Buf, rows: usize, cols: usize, table_rows: usize, rotated: usize) -> Result<()> {
        if !cols.is_multiple_of(2) || table_rows == 0 {
            return Err(Error::Input("rope needs an even width and a table".into()));
        }
        need(x, rows * cols, "rope input")?;
        need(out, rows * cols, "rope output")?;
        need(cos, table_rows * cols, "rope cos")?;
        need(sin, table_rows * cols, "rope sin")?;
        self.run(
            "rope",
            &[(0, x), (1, cos), (2, sin), (3, out)],
            &[n32(rows * cols, "n")?, n32(cols, "cols")?, n32(table_rows, "table")?, n32(rotated.min(rows), "rotated")?],
            items(rows * cols)?,
        );
        Ok(())
    }

    /// `out = lateral + nearest_upsample_2x(prev)` for `[h, w, c]` (`h` and `w` even).
    pub fn upsample_add(&self, lateral: &Buf, prev: &Buf, out: &Buf, h: usize, w: usize, c: usize) -> Result<()> {
        if !h.is_multiple_of(2) || !w.is_multiple_of(2) {
            return Err(Error::Input(format!("upsample_add needs even sizes, got {w}x{h}")));
        }
        need(lateral, h * w * c, "upsample_add lateral")?;
        need(prev, h / 2 * (w / 2) * c, "upsample_add prev")?;
        need(out, h * w * c, "upsample_add out")?;
        self.run("upsample_add", &[(0, lateral), (1, prev), (3, out)], &[n32(h * w * c, "n")?, n32(h, "h")?, n32(w, "w")?, n32(c, "c")?], items(h * w * c)?);
        Ok(())
    }
}
