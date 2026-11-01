//! Running TensorFlow Lite models without TensorFlow: a reader for the `.tflite` FlatBuffer
//! (fields as numbered in TensorFlow Lite's published `schema.fbs`, Apache-2.0) and an
//! interpreter for the float operators small vision models use: `CONV_2D`, `DEPTHWISE_CONV_2D`,
//! `MAX_POOL_2D`, `ADD`, `PAD`, `CONCATENATION`, `RESHAPE`, `RELU`, `RELU6`, `PRELU`, `LOGISTIC`
//! and `DEQUANTIZE` of float16 weights (folded at load). Weights are re-laid out once at load for
//! the [`nn`](crate::nn) kernels. Anything else (other operators, quantised tensors, dilation) is a
//! load error, never a panic: models are pinned by SHA-256, so what loads once always loads.
//!
//! A model is also refused at load when running it would take more than a fixed budget: memory
//! (each tensor, all tensors alive at once, one operator's working space, the weights), work
//! (multiply-adds, values written) or kernel size. Every one is worked out from the static
//! shapes with checked arithmetic, so a small hostile file can neither exhaust memory nor keep
//! [`Model::run`] busy for minutes. The budgets leave wide headroom over the official
//! MediaPipe face models.

use crate::nn::{Conv, Depthwise, Linear};
use crate::pt::half_to_f32;

type Result<T> = std::result::Result<T, String>;

// ---------------------------------------------------------------- FlatBuffers

/// A FlatBuffer: bounds-checked little-endian reads.
#[derive(Clone, Copy)]
struct Fb<'a>(&'a [u8]);

/// A table: its position in the buffer.
#[derive(Clone, Copy)]
struct Table(usize);

impl<'a> Fb<'a> {
    fn bytes<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0.get(at..at.checked_add(N).ok_or("bad offset")?).and_then(|s| s.try_into().ok()).ok_or_else(|| "truncated model".to_string())
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.bytes::<1>(at)?[0])
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(at)?))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(at)?))
    }
    fn i32(&self, at: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(at)?))
    }
    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(at)?))
    }
    /// Follow the offset stored at `at`.
    fn deref(&self, at: usize) -> Result<usize> {
        at.checked_add(self.u32(at)? as usize).ok_or_else(|| "bad offset".to_string())
    }
    fn root(&self) -> Result<Table> {
        Ok(Table(self.deref(0)?))
    }
    /// Where field `id` of `t` is stored (absent fields have their default).
    fn field(&self, t: Table, id: usize) -> Result<Option<usize>> {
        let vt = (t.0 as i64 - self.i32(t.0)? as i64).try_into().map_err(|_| "bad vtable")?;
        let vt_len = self.u16(vt)? as usize;
        let slot = 4 + 2 * id;
        if slot + 2 > vt_len {
            return Ok(None);
        }
        let off = self.u16(vt + slot)? as usize;
        Ok((off != 0).then_some(t.0 + off))
    }
    fn int(&self, t: Table, id: usize, default: i32) -> Result<i32> {
        self.field(t, id)?.map_or(Ok(default), |p| self.i32(p))
    }
    fn byte(&self, t: Table, id: usize, default: u8) -> Result<u8> {
        self.field(t, id)?.map_or(Ok(default), |p| self.u8(p))
    }
    fn table(&self, t: Table, id: usize) -> Result<Option<Table>> {
        self.field(t, id)?.map(|p| self.deref(p).map(Table)).transpose()
    }
    /// A vector field: (start of its elements, length).
    fn vector(&self, t: Table, id: usize) -> Result<(usize, usize)> {
        let Some(p) = self.field(t, id)? else { return Ok((0, 0)) };
        let v = self.deref(p)?;
        let n = self.u32(v)? as usize;
        if v.saturating_add(4).saturating_add(n) > self.0.len() {
            return Err("truncated model".into());
        }
        Ok((v + 4, n))
    }
    fn tables(&self, t: Table, id: usize) -> Result<Vec<Table>> {
        let (at, n) = self.vector(t, id)?;
        (0..n).map(|i| self.deref(at + 4 * i).map(Table)).collect()
    }
    fn ints(&self, t: Table, id: usize) -> Result<Vec<i32>> {
        let (at, n) = self.vector(t, id)?;
        (0..n).map(|i| self.i32(at + 4 * i)).collect()
    }
    fn string(&self, t: Table, id: usize) -> Result<String> {
        let (at, n) = self.vector(t, id)?;
        Ok(String::from_utf8_lossy(self.0.get(at..at + n).ok_or("truncated model")?).into_owned())
    }
}

// ---------------------------------------------------------------- the model

/// Operator codes (`BuiltinOperator`).
mod op {
    pub const ADD: i32 = 0;
    pub const CONCATENATION: i32 = 2;
    pub const CONV_2D: i32 = 3;
    pub const DEPTHWISE_CONV_2D: i32 = 4;
    pub const DEQUANTIZE: i32 = 6;
    pub const LOGISTIC: i32 = 14;
    pub const MAX_POOL_2D: i32 = 17;
    pub const RELU: i32 = 19;
    pub const RELU6: i32 = 21;
    pub const RESHAPE: i32 = 22;
    pub const PAD: i32 = 34;
    pub const PRELU: i32 = 54;
}

/// The most values a tensor may hold.
const MAX_TENSOR: usize = 1 << 28;
/// The widest convolution or pooling window side.
const MAX_KERNEL: usize = 32;
/// The most floats one operator may use as working space (a convolution's patch matrix for one
/// output row: `ow·kh·kw·cin`).
const MAX_SCRATCH: u64 = 1 << 22;
/// The most multiply-adds (convolutions) and window comparisons (pooling) in one run.
const MAX_MACS: u64 = 1 << 33;
/// The most values all operators together may write in one run.
const MAX_VALUES: u64 = 1 << 30;
/// The most floats alive at once while the model runs.
const MAX_LIVE: u64 = 1 << 29;
/// The most constant values decoded and laid out at load (a weight shared by several tensors or
/// operators counts once per use).
const MAX_CONSTS: usize = 1 << 27;

/// Tensor types (`TensorType`).
const FLOAT32: u8 = 0;
const FLOAT16: u8 = 1;
const INT32: u8 = 2;

/// A fused activation (`ActivationFunctionType`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    None,
    Relu,
    ReluN1To1,
    Relu6,
    Tanh,
}

impl Act {
    fn from(code: u8) -> Result<Act> {
        Ok(match code {
            0 => Act::None,
            1 => Act::Relu,
            2 => Act::ReluN1To1,
            3 => Act::Relu6,
            4 => Act::Tanh,
            c => return Err(format!("unsupported fused activation {c}")),
        })
    }
    fn apply(self, x: &mut [f32]) {
        let f: fn(f32) -> f32 = match self {
            Act::None => return,
            Act::Relu => |v| v.max(0.0),
            Act::ReluN1To1 => |v| v.clamp(-1.0, 1.0),
            Act::Relu6 => |v| v.clamp(0.0, 6.0),
            Act::Tanh => f32::tanh,
        };
        for v in x {
            *v = f(*v);
        }
    }
}

/// One operator, its weights laid out for the kernels.
#[derive(Clone, Debug)]
enum Op {
    /// A 1×1, stride-1 convolution: a dense layer over the pixels.
    Pointwise(Linear, Act),
    Conv(Conv, [usize; 2], Act),
    Depthwise(Depthwise, [usize; 2], Act),
    /// Kernel, stride, padding before (top, left).
    MaxPool {
        k: [usize; 2],
        stride: [usize; 2],
        pad: [usize; 2],
        act: Act,
    },
    Add(Act),
    /// Constant padding (zeros), per dimension (before, after).
    Pad(Vec<[usize; 2]>),
    Concat {
        axis: usize,
        act: Act,
    },
    Reshape,
    Act(Act),
    Prelu(Vec<f32>),
    Logistic,
}

#[derive(Clone, Debug)]
struct Step {
    op: Op,
    /// Runtime inputs (tensor ids).
    inputs: Vec<usize>,
    output: usize,
}

/// A loaded model, ready to run (batch of one, static shapes).
#[derive(Clone, Debug)]
pub struct Model {
    shapes: Vec<Vec<usize>>,
    names: Vec<String>,
    /// Constant float tensors (weights folded into the steps are not kept).
    consts: Vec<Option<Vec<f32>>>,
    steps: Vec<Step>,
    inputs: Vec<usize>,
    outputs: Vec<usize>,
    /// After which step each tensor is last needed (to free memory as the model runs).
    last_use: Vec<usize>,
}

fn index(i: i32, n: usize) -> Result<usize> {
    usize::try_from(i).ok().filter(|&i| i < n).ok_or_else(|| format!("bad tensor index {i}"))
}

/// "Same" padding: output size and padding before.
fn same(input: usize, k: usize, stride: usize) -> (usize, usize) {
    let out = input.div_ceil(stride);
    let total = ((out.saturating_sub(1)) * stride + k).saturating_sub(input);
    (out, total / 2)
}

/// Output size and padding before, for `padding` (0 same, 1 valid).
fn window(padding: u8, input: usize, k: usize, stride: usize) -> Result<(usize, usize)> {
    match padding {
        0 => Ok(same(input, k, stride)),
        1 if input >= k => Ok(((input - k) / stride + 1, 0)),
        _ => Err("bad window".into()),
    }
}

impl Model {
    /// Read a `.tflite` file.
    pub fn read(bytes: &[u8]) -> Result<Model> {
        let fb = Fb(bytes);
        let root = fb.root()?;
        let codes: Vec<i32> = fb.tables(root, 1)?.into_iter().map(|c| Ok(fb.int(c, 3, 0)?.max(fb.byte(c, 0, 0)? as i8 as i32))).collect::<Result<_>>()?;
        let buffers = fb.tables(root, 4)?;
        let graphs = fb.tables(root, 2)?;
        let g = *graphs.first().ok_or("the model has no graph")?;
        let tensors = fb.tables(g, 0)?;
        let n = tensors.len();
        let mut shapes = Vec::with_capacity(n);
        let mut names = Vec::with_capacity(n);
        // Constant data: float tensors as f32, int tensors (pads, shapes) as i32, decoded once
        // their total is known to fit the budget.
        let mut floats: Vec<Option<Vec<f32>>> = vec![None; n];
        let mut ints: Vec<Option<Vec<i32>>> = vec![None; n];
        let mut data: Vec<(usize, u8, usize, &[u8])> = vec![];
        let mut consts = 0usize;
        for (i, t) in tensors.iter().enumerate() {
            let shape: Vec<usize> = fb
                .ints(*t, 0)?
                .into_iter()
                .map(|d| usize::try_from(d).map_err(|_| "dynamic shapes are not supported"))
                .collect::<std::result::Result<_, _>>()?;
            let ty = fb.byte(*t, 1, FLOAT32)?;
            let buf = fb.int(*t, 2, 0)? as u32 as usize;
            names.push(fb.string(*t, 3)?);
            // Every size computed from shapes later stays in range, even a product of some of the
            // dimensions (a zero dimension does not let the others grow without bound).
            let count = shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d)).ok_or("a tensor is too large")?;
            if shape.iter().try_fold(1usize, |a, &d| a.checked_mul(d.max(1))).is_none_or(|c| c > MAX_TENSOR) {
                return Err("a tensor is too large".into());
            }
            if fb.field(*t, 4)?.is_some() && fb.table(*t, 4)?.is_some_and(|q| fb.vector(q, 2).is_ok_and(|v| v.1 > 0)) {
                return Err(format!("{}: quantised tensors are not supported", names[i]));
            }
            if buf > 0 {
                let b = *buffers.get(buf).ok_or("bad buffer index")?;
                let (mut at, mut len) = fb.vector(b, 0)?;
                if len == 0 {
                    // Large models keep data after the FlatBuffer: offset and size from its start.
                    let (o, s) = (fb.field(b, 1)?.map(|p| fb.u64(p)).transpose()?.unwrap_or(0), fb.field(b, 2)?.map(|p| fb.u64(p)).transpose()?.unwrap_or(0));
                    (at, len) = (o as usize, s as usize);
                }
                if len > 0 {
                    data.push((i, ty, count, bytes.get(at..at.checked_add(len).ok_or("bad buffer")?).ok_or("truncated model")?));
                    // Several tensors may share one buffer: each is decoded on its own.
                    consts = consts.checked_add(count).filter(|&c| c <= MAX_CONSTS).ok_or("the model's weights are too large")?;
                }
            }
            shapes.push(shape);
        }
        for (i, ty, count, data) in data {
            match ty {
                FLOAT32 if data.len() == 4 * count => {
                    floats[i] = Some(data.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
                }
                FLOAT16 if data.len() == 2 * count => {
                    floats[i] = Some(data.as_chunks::<2>().0.iter().map(|c| half_to_f32(u16::from_le_bytes([c[0], c[1]]))).collect())
                }
                INT32 if data.len() == 4 * count => {
                    ints[i] = Some(data.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
                }
                _ => return Err(format!("{}: unsupported tensor data", names[i])),
            }
        }
        // Copies of constants made below (folded DEQUANTIZEs, weights laid out for the kernels).
        let consts = std::cell::Cell::new(consts);
        let spend = |values: usize| -> Result<()> {
            consts.set(consts.get().checked_add(values).filter(|&c| c <= MAX_CONSTS).ok_or("the model's weights are too large")?);
            Ok(())
        };
        let inputs = fb.ints(g, 1)?.into_iter().map(|i| index(i, n)).collect::<Result<Vec<_>>>()?;
        let outputs = fb.ints(g, 2)?.into_iter().map(|i| index(i, n)).collect::<Result<Vec<_>>>()?;
        let mut steps = vec![];
        for o in fb.tables(g, 3)? {
            let code = *codes.get(fb.int(o, 0, 0)? as u32 as usize).ok_or("bad operator code")?;
            // Optional inputs are -1.
            let ins: Vec<Option<usize>> = fb.ints(o, 1)?.into_iter().map(|i| if i < 0 { Ok(None) } else { index(i, n).map(Some) }).collect::<Result<_>>()?;
            let outs = fb.ints(o, 2)?;
            let output = index(*outs.first().ok_or("an operator has no output")?, n)?;
            // Float16 weights: fold their DEQUANTIZE.
            if code == op::DEQUANTIZE
                && let Some(i) = ins.first().copied().flatten()
                && let Some(len) = floats[i].as_ref().map(Vec::len)
            {
                spend(len)?;
                floats[output] = floats[i].clone();
                continue;
            }
            let opts = fb.table(o, 4)?;
            let opt_int = |id: usize, default: i32| opts.map_or(Ok(default), |t| fb.int(t, id, default));
            let opt_byte = |id: usize, default: u8| opts.map_or(Ok(default), |t| fb.byte(t, id, default));
            let input = |k: usize| ins.get(k).copied().flatten().ok_or_else(|| format!("{}: missing input {k}", names[output]));
            let weights = |k: usize| -> Result<Vec<f32>> {
                let id = input(k)?;
                let w = floats[id].as_ref().ok_or_else(|| format!("{}: input {k} is not constant", names[output]))?;
                spend(w.len())?;
                Ok(w.clone())
            };
            let bias = |count: usize| -> Result<Vec<f32>> {
                spend(count)?;
                match ins.get(2).copied().flatten() {
                    Some(id) => floats[id].clone().filter(|b| b.len() == count).ok_or_else(|| format!("{}: bad bias", names[output])),
                    None => Ok(vec![0.0; count]),
                }
            };
            let in_shape = |k: usize| -> Result<Vec<usize>> { Ok(shapes[input(k)?].clone()) };
            let out_shape = shapes[output].clone();
            let stride = |w: usize, h: usize| -> Result<[usize; 2]> {
                let s = [opt_int(h, 1)?, opt_int(w, 1)?];
                if s.iter().any(|v| *v < 1) {
                    return Err("bad stride".into());
                }
                Ok(s.map(|v| v as usize))
            };
            let mut runtime = vec![input(0)?];
            let op = match code {
                op::DEQUANTIZE => Op::Reshape,
                op::CONV_2D | op::DEPTHWISE_CONV_2D => {
                    let depthwise = code == op::DEPTHWISE_CONV_2D;
                    let (padding, s) = (opt_byte(0, 0)?, stride(1, 2)?);
                    let act_id = if depthwise { 4 } else { 3 };
                    let dil = if depthwise { [opt_int(6, 1)?, opt_int(5, 1)?] } else { [opt_int(5, 1)?, opt_int(4, 1)?] };
                    if dil != [1, 1] {
                        return Err(format!("{}: dilated convolutions are not supported", names[output]));
                    }
                    let act = Act::from(opt_byte(act_id, 0)?)?;
                    let (x, f) = (in_shape(0)?, shapes[input(1)?].clone());
                    let (&[1, h, w, cin], &[fo, kh, kw, fi], &[1, oh, ow, cout]) = (&x[..], &f[..], &out_shape[..]) else {
                        return Err(format!("{}: unexpected convolution shapes", names[output]));
                    };
                    if kh != kw || s[0] != s[1] {
                        return Err(format!("{}: only square kernels and strides are supported", names[output]));
                    }
                    if kh > MAX_KERNEL {
                        return Err(format!("{}: the convolution kernel is too large", names[output]));
                    }
                    let (ey, py) = window(padding, h, kh, s[0])?;
                    let (ex, px) = window(padding, w, kw, s[1])?;
                    if (ey, ex) != (oh, ow) {
                        return Err(format!("{}: output size disagrees with the padding", names[output]));
                    }
                    let wt = weights(1)?;
                    let b = bias(cout)?;
                    if depthwise {
                        if fo != 1 || fi != cout || cin != cout {
                            return Err(format!("{}: depth multipliers are not supported", names[output]));
                        }
                        Op::Depthwise(Depthwise { w: wt, b, c: cout, ks: kh, stride: s[0] }, [py, px], act)
                    } else {
                        if fo != cout || fi != cin {
                            return Err(format!("{}: unexpected filter shape", names[output]));
                        }
                        // OHWI → [(ky·ks + kx)·inp + ci] × out.
                        let kk = kh * kw * cin;
                        spend(kk * cout)?;
                        let mut t = vec![0.0f32; kk * cout];
                        for o in 0..cout {
                            for p in 0..kk {
                                t[p * cout + o] = *wt.get(o * kk + p).ok_or("bad filter")?;
                            }
                        }
                        if kh == 1 && s[0] == 1 {
                            Op::Pointwise(Linear { wt: t, b, inp: cin, out: cout }, act)
                        } else {
                            Op::Conv(Conv { wt: t, b, inp: cin, out: cout, ks: kh, stride: s[0], pad: 0 }, [py, px], act)
                        }
                    }
                }
                op::MAX_POOL_2D => {
                    let (padding, s) = (opt_byte(0, 0)?, stride(1, 2)?);
                    let k = [opt_int(4, 1)?, opt_int(3, 1)?];
                    if k.iter().any(|v| *v < 1) {
                        return Err("bad pool size".into());
                    }
                    let k = k.map(|v| v as usize);
                    if k.iter().any(|&v| v > MAX_KERNEL) {
                        return Err(format!("{}: the pooling window is too large", names[output]));
                    }
                    let x = in_shape(0)?;
                    let (&[1, h, w, c], &[1, oh, ow, oc]) = (&x[..], &out_shape[..]) else { return Err("unexpected pool shapes".into()) };
                    if c != oc {
                        return Err(format!("{}: pooling changes the channels", names[output]));
                    }
                    let (ey, py) = window(padding, h, k[0], s[0])?;
                    let (ex, px) = window(padding, w, k[1], s[1])?;
                    if (ey, ex) != (oh, ow) {
                        return Err(format!("{}: output size disagrees with the padding", names[output]));
                    }
                    Op::MaxPool { k, stride: s, pad: [py, px], act: Act::from(opt_byte(5, 0)?)? }
                }
                op::ADD => {
                    runtime.push(input(1)?);
                    Op::Add(Act::from(opt_byte(0, 0)?)?)
                }
                op::PAD => {
                    let p = ints[input(1)?].clone().ok_or("PAD: paddings must be constant")?;
                    let pads = p
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| Ok([usize::try_from(c[0]).map_err(|_| "bad pad")?, usize::try_from(c[1]).map_err(|_| "bad pad")?]))
                        .collect::<Result<Vec<_>>>()?;
                    // The padded shape is the declared one (so it is as bounded as any tensor).
                    let x = in_shape(0)?;
                    let padded = x.iter().zip(&pads).map(|(d, p)| d.checked_add(p[0])?.checked_add(p[1])).collect::<Option<Vec<_>>>();
                    if pads.len() != x.len() || padded.as_ref() != Some(&out_shape) {
                        return Err(format!("{}: the padding disagrees with the output shape", names[output]));
                    }
                    Op::Pad(pads)
                }
                op::CONCATENATION => {
                    runtime = (0..ins.len()).map(input).collect::<Result<_>>()?;
                    let rank = out_shape.len() as i32;
                    let a = opt_int(0, 0)?;
                    let axis = index(if a < 0 { a.saturating_add(rank) } else { a }, out_shape.len())?;
                    // The parts fit the output: same rank, same sizes but along `axis`, where they add up.
                    let mut along = 0usize;
                    for &id in &runtime {
                        let p = &shapes[id];
                        let fits = p.len() == out_shape.len() && p.iter().zip(&out_shape).enumerate().all(|(d, (a, b))| d == axis || a == b);
                        along = p
                            .get(axis)
                            .filter(|_| fits)
                            .and_then(|&d| along.checked_add(d))
                            .ok_or_else(|| format!("{}: the parts disagree with the output shape", names[output]))?;
                    }
                    if out_shape.get(axis) != Some(&along) {
                        return Err(format!("{}: the parts disagree with the output shape", names[output]));
                    }
                    Op::Concat { axis, act: Act::from(opt_byte(1, 0)?)? }
                }
                op::RESHAPE => Op::Reshape,
                op::RELU => Op::Act(Act::Relu),
                op::RELU6 => Op::Act(Act::Relu6),
                op::LOGISTIC => Op::Logistic,
                op::PRELU => {
                    let alpha = weights(1)?;
                    let c = *out_shape.last().ok_or("PRELU: bad shape")?;
                    if alpha.len() != c {
                        return Err(format!("{}: only per-channel PReLU is supported", names[output]));
                    }
                    Op::Prelu(alpha)
                }
                c => return Err(format!("operator {c} is not supported")),
            };
            steps.push(Step { op, inputs: runtime, output });
        }
        let mut last_use = vec![usize::MAX; n];
        for (k, s) in steps.iter().enumerate() {
            for &i in &s.inputs {
                last_use[i] = k;
            }
        }
        for &o in &outputs {
            last_use[o] = usize::MAX;
        }
        let model = Model { shapes, names, consts: floats, steps, inputs, outputs, last_use };
        let c = model.cost().ok_or("the model is too expensive to run")?;
        let over = [
            (c.kernel > MAX_KERNEL as u64, "a kernel is too large"),
            (c.scratch > MAX_SCRATCH, "an operator's working space is too large"),
            (c.macs > MAX_MACS, "the model needs too many multiply-adds"),
            (c.values > MAX_VALUES, "the model does too much work"),
            (c.live > MAX_LIVE, "the model needs too much memory"),
        ];
        if let Some((_, why)) = over.iter().find(|o| o.0) {
            return Err((*why).to_string());
        }
        Ok(model)
    }

    /// What one run takes, from the static shapes (`None` if a count overflows).
    fn cost(&self) -> Option<Cost> {
        let size = |i: usize| -> Option<u64> { self.shapes.get(i)?.iter().try_fold(1u64, |a, &d| a.checked_mul(d as u64)) };
        let mut c = Cost::default();
        // Floats alive per tensor, as `run` keeps them: inputs are copied in, and every step's
        // output stays until its last use (for good when nothing uses it).
        let mut alive = vec![0u64; self.shapes.len()];
        let mut total = 0u64;
        for &i in &self.inputs {
            total = total.checked_sub(*alive.get(i)?)?.checked_add(size(i)?)?;
            *alive.get_mut(i)? = size(i)?;
        }
        c.live = total;
        for (k, s) in self.steps.iter().enumerate() {
            let &x = s.inputs.first()?;
            let (xs, ys) = (self.shapes.get(x)?, self.shapes.get(s.output)?);
            let dim = |sh: &[usize], d: usize| sh.get(d).copied().unwrap_or(1) as u64;
            let (h, w, oh, ow) = (dim(xs, 1), dim(xs, 2), dim(ys, 1), dim(ys, 2));
            let out = size(s.output)?;
            let mut scratch = 0u64;
            let macs = match &s.op {
                Op::Pointwise(l, _) => h.checked_mul(w)?.checked_mul(l.inp as u64)?.checked_mul(l.out as u64)?,
                Op::Conv(cv, ..) => {
                    let ks = cv.ks as u64;
                    c.kernel = c.kernel.max(ks);
                    let kk = ks.checked_mul(ks)?.checked_mul(cv.inp as u64)?;
                    scratch = ow.checked_mul(kk)?;
                    oh.checked_mul(ow)?.checked_mul(cv.out as u64)?.checked_mul(kk)?
                }
                Op::Depthwise(d, ..) => {
                    let ks = d.ks as u64;
                    c.kernel = c.kernel.max(ks);
                    oh.checked_mul(ow)?.checked_mul(d.c as u64)?.checked_mul(ks)?.checked_mul(ks)?
                }
                Op::MaxPool { k, .. } => {
                    c.kernel = c.kernel.max(k[0].max(k[1]) as u64);
                    out.checked_mul(k[0] as u64)?.checked_mul(k[1] as u64)?
                }
                _ => 0,
            };
            c.macs = c.macs.checked_add(macs)?;
            c.values = c.values.checked_add(out)?;
            c.scratch = c.scratch.max(scratch);
            // The output and the scratch are allocated while the inputs (and any earlier value of
            // the output) are still alive.
            c.live = c.live.max(total.checked_add(out)?.checked_add(scratch)?);
            total = total.checked_sub(*alive.get(s.output)?)?.checked_add(out)?;
            *alive.get_mut(s.output)? = out;
            for &i in &s.inputs {
                if self.last_use.get(i) == Some(&k) {
                    total = total.checked_sub(*alive.get(i)?)?;
                    *alive.get_mut(i)? = 0;
                }
            }
        }
        // Outputs that are constants are copied out.
        for &o in &self.outputs {
            if *alive.get(o)? == 0 {
                total = total.checked_add(size(o)?)?;
            }
        }
        c.live = c.live.max(total);
        Some(c)
    }

    /// The shape of input `k`.
    pub fn input_shape(&self, k: usize) -> Option<&[usize]> {
        self.inputs.get(k).and_then(|&i| self.shapes.get(i)).map(Vec::as_slice)
    }

    /// The shape and name of output `k`.
    pub fn output(&self, k: usize) -> Option<(&[usize], &str)> {
        let &i = self.outputs.get(k)?;
        Some((self.shapes.get(i)?.as_slice(), self.names.get(i)?.as_str()))
    }

    /// Run the model on its inputs (row-major, NHWC); returns its outputs.
    pub fn run(&self, inputs: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
        let mut vals: Vec<Option<Vec<f32>>> = vec![None; self.shapes.len()];
        for (k, &id) in self.inputs.iter().enumerate() {
            let x = inputs.get(k).ok_or("missing model input")?;
            if x.len() != self.shapes[id].iter().product::<usize>() {
                return Err(format!("input {k}: expected {:?}", self.shapes[id]));
            }
            vals[id] = Some(x.to_vec());
        }
        for (k, s) in self.steps.iter().enumerate() {
            let y = {
                let get = |i: usize| -> Result<&[f32]> {
                    vals.get(i)
                        .and_then(Option::as_deref)
                        .or_else(|| self.consts.get(i).and_then(Option::as_deref))
                        .ok_or_else(|| format!("{}: input not computed", self.names[s.output]))
                };
                let x = get(s.inputs[0])?;
                let xs = &self.shapes[s.inputs[0]];
                let ys = &self.shapes[s.output];
                let (h, w) = (xs.get(1).copied().unwrap_or(1), xs.get(2).copied().unwrap_or(1));
                let (oh, ow) = (ys.get(1).copied().unwrap_or(1), ys.get(2).copied().unwrap_or(1));
                let y = match &s.op {
                    Op::Pointwise(l, a) => {
                        let mut y = l.forward(x, h * w);
                        a.apply(&mut y);
                        y
                    }
                    Op::Conv(c, pad, a) => {
                        let mut y = c.forward_sized(x, h, w, oh, ow, *pad);
                        a.apply(&mut y);
                        y
                    }
                    Op::Depthwise(d, pad, a) => {
                        let mut y = d.forward_sized(x, h, w, oh, ow, *pad);
                        a.apply(&mut y);
                        y
                    }
                    Op::MaxPool { k, stride, pad, act } => {
                        let mut y = max_pool(x, [h, w], *xs.last().unwrap_or(&1), [oh, ow], *k, *stride, *pad);
                        act.apply(&mut y);
                        y
                    }
                    Op::Add(a) => {
                        let b = get(s.inputs[1])?;
                        if b.len() != x.len() {
                            return Err(format!("{}: broadcasting ADD is not supported", self.names[s.output]));
                        }
                        let mut y: Vec<f32> = x.iter().zip(b).map(|(p, q)| p + q).collect();
                        a.apply(&mut y);
                        y
                    }
                    Op::Pad(p) => pad(x, xs, p)?,
                    Op::Concat { axis, act } => {
                        let parts = s.inputs.iter().map(|&i| Ok((get(i)?, &self.shapes[i]))).collect::<Result<Vec<_>>>()?;
                        let mut y = concat(&parts, *axis)?;
                        act.apply(&mut y);
                        y
                    }
                    Op::Reshape => x.to_vec(),
                    Op::Act(a) => {
                        let mut y = x.to_vec();
                        a.apply(&mut y);
                        y
                    }
                    Op::Prelu(alpha) => {
                        let c = alpha.len().max(1);
                        x.iter().enumerate().map(|(i, &v)| if v >= 0.0 { v } else { v * alpha[i % c] }).collect()
                    }
                    Op::Logistic => x.iter().map(|v| 1.0 / (1.0 + (-v).exp())).collect(),
                };
                if y.len() != ys.iter().product::<usize>() {
                    return Err(format!("{}: computed {} values for shape {ys:?}", self.names[s.output], y.len()));
                }
                y
            };
            vals[s.output] = Some(y);
            for &i in &s.inputs {
                if self.last_use[i] == k {
                    vals[i] = None;
                }
            }
        }
        self.outputs.iter().map(|&o| vals[o].take().or_else(|| self.consts[o].clone()).ok_or_else(|| "an output was not computed".to_string())).collect()
    }
}

/// What running a model takes, worked out at load from its static shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Cost {
    /// Multiply-adds (convolutions) and window comparisons (pooling).
    macs: u64,
    /// Values all operators write.
    values: u64,
    /// The most floats alive at once.
    live: u64,
    /// The largest working space one operator allocates, in floats.
    scratch: u64,
    /// The widest window side.
    kernel: u64,
}

/// Max pooling over `k` windows; window cells outside the input are ignored.
fn max_pool(x: &[f32], [h, w]: [usize; 2], c: usize, [oh, ow]: [usize; 2], k: [usize; 2], s: [usize; 2], pad: [usize; 2]) -> Vec<f32> {
    let mut y = vec![f32::NEG_INFINITY; oh * ow * c];
    for oy in 0..oh {
        for ox in 0..ow {
            let out = &mut y[(oy * ow + ox) * c..][..c];
            for ky in 0..k[0] {
                let Some(iy) = (oy * s[0] + ky).checked_sub(pad[0]).filter(|&v| v < h) else { continue };
                for kx in 0..k[1] {
                    let Some(ix) = (ox * s[1] + kx).checked_sub(pad[1]).filter(|&v| v < w) else { continue };
                    let Some(src) = x.get((iy * w + ix) * c..(iy * w + ix + 1) * c) else { continue };
                    for (o, v) in out.iter_mut().zip(src) {
                        *o = o.max(*v);
                    }
                }
            }
        }
    }
    y
}

/// Zero padding of a row-major tensor of shape `shape`.
fn pad(x: &[f32], shape: &[usize], pads: &[[usize; 2]]) -> Result<Vec<f32>> {
    if pads.len() != shape.len() {
        return Err("PAD: rank mismatch".into());
    }
    let out: Vec<usize> = shape.iter().zip(pads).map(|(d, p)| d + p[0] + p[1]).collect();
    let mut y = vec![0.0f32; out.iter().product()];
    let rank = shape.len();
    let inner = *shape.last().unwrap_or(&1);
    let rows = x.len() / inner.max(1);
    // Copy each innermost row to its padded place.
    let mut idx = vec![0usize; rank.saturating_sub(1)];
    for r in 0..rows {
        let mut at = 0usize;
        for d in 0..rank {
            let i = if d + 1 == rank { pads[d][0] } else { idx[d] + pads[d][0] };
            at = at * out[d] + i;
        }
        if let (Some(dst), Some(src)) = (y.get_mut(at..at + inner), x.get(r * inner..(r + 1) * inner)) {
            dst.copy_from_slice(src);
        }
        for d in (0..idx.len()).rev() {
            idx[d] += 1;
            if idx[d] < shape[d] {
                break;
            }
            idx[d] = 0;
        }
    }
    Ok(y)
}

/// Concatenate along `axis`.
fn concat(parts: &[(&[f32], &Vec<usize>)], axis: usize) -> Result<Vec<f32>> {
    let Some((_, s0)) = parts.first() else { return Ok(vec![]) };
    let outer: usize = s0.get(..axis).ok_or("CONCATENATION: bad axis")?.iter().product();
    let mut y = Vec::with_capacity(parts.iter().map(|p| p.0.len()).sum());
    for o in 0..outer {
        for (x, s) in parts {
            let chunk: usize = s.get(axis..).ok_or("CONCATENATION: bad axis")?.iter().product();
            y.extend_from_slice(x.get(o * chunk..(o + 1) * chunk).ok_or("CONCATENATION: bad input")?);
        }
    }
    Ok(y)
}

#[cfg(test)]
mod tests;
