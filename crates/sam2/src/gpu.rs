// Original work for EffectCraft (wgpu compute kernels); the maths follows the Hugging Face
// `transformers` SAM 2 implementation (Apache-2.0, see NOTICE).
//! An optional GPU path for the heavy parts of SAM 2 (the Hiera backbone, the neck and the memory
//! attention), as wgpu compute kernels: it runs on AMD, NVIDIA and Intel alike, where candle has
//! no backend for the first. The CPU path stays the reference: every kernel is tested against it,
//! and any GPU failure (no adapter, an out-of-memory error, a lost device) is an `Err` the caller
//! answers by running the CPU path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use wgpu::util::DeviceExt;

use crate::{Error, Result};

const SHADER: &str = concat!(include_str!("gpu/kernels.wgsl"), include_str!("gpu/kernels_ops.wgsl"));
/// Words in a kernel's parameter block.
const PARAM_WORDS: usize = 24;

/// A buffer of f32 on the GPU (cheap to clone: it is a handle).
#[derive(Clone)]
pub struct Buf {
    pub(crate) buffer: wgpu::Buffer,
    /// Elements (f32).
    pub len: usize,
    /// Two half-precision values per 32-bit word (a weight matrix only; `len` still counts values).
    pub(crate) f16: bool,
}

/// One strided matrix operand: `(offset, batch stride, row stride, column stride)` in elements.
#[derive(Clone, Copy, Debug)]
pub struct Strides {
    pub offset: usize,
    pub batch: usize,
    pub row: usize,
    pub col: usize,
}

impl Strides {
    /// A row-major `[rows, cols]` matrix (one batch).
    pub fn row_major(cols: usize) -> Strides {
        Strides { offset: 0, batch: 0, row: cols, col: 1 }
    }
}

/// What to do to a matrix product after the multiplication.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Act {
    #[default]
    None,
    Gelu,
    Relu,
}

/// A batched matrix product `C[b] = act(alpha * A[b] . B[b] + bias) (+ residual)`.
pub struct Matmul<'a> {
    pub m: usize,
    pub n: usize,
    pub k: usize,
    pub batch: usize,
    pub a: (&'a Buf, Strides),
    pub b: (&'a Buf, Strides),
    pub bias: Option<&'a Buf>,
    pub c: (&'a Buf, Strides),
    pub residual: Option<&'a Buf>,
    pub alpha: f32,
    pub act: Act,
}

/// The GPU device and its compiled kernels.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    module: wgpu::ShaderModule,
    pipelines: Mutex<HashMap<&'static str, Arc<wgpu::ComputePipeline>>>,
    encoder: Mutex<Option<wgpu::CommandEncoder>>,
    turn: Mutex<()>,
    error: Arc<Mutex<Option<String>>>,
    adapter: String,
}

fn words(v: &[u32]) -> [u32; PARAM_WORDS] {
    let mut out = [0u32; PARAM_WORDS];
    for (o, x) in out.iter_mut().zip(v) {
        *o = *x;
    }
    out
}

fn u32_of(v: usize, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| Error::Compute(format!("{what} {v} does not fit a GPU index")))
}

/// Whether weight matrices are kept on the GPU in half precision (`EFFECTCRAFT_SAM2_F16=1`).
pub fn half_weights() -> bool {
    std::env::var("EFFECTCRAFT_SAM2_F16").is_ok_and(|v| matches!(v.trim(), "1" | "on" | "true"))
}

/// The process's one GPU device, once something has asked for it.
static DEVICE: std::sync::OnceLock<std::result::Result<Arc<Gpu>, String>> = std::sync::OnceLock::new();

impl Gpu {
    /// The process's one GPU device, created on first use. Several devices at once upset some Vulkan loaders
    /// (a crash when two threads create or drop them together), and one is all SAM 2 needs: models come and go,
    /// the device stays. An error (no GPU) is remembered too.
    pub fn shared() -> Result<Arc<Gpu>> {
        DEVICE.get_or_init(|| Gpu::new().map(Arc::new).map_err(|e| e.to_string())).clone().map_err(Error::Compute)
    }

    /// Whether the shared device has been looked for yet, and found: `None` before anything has asked
    /// for it (this never creates it, so a caller that only wants to describe the GPU does not stall
    /// on the adapter and shader compilation), then `Some(true)` or `Some(false)` (no GPU).
    pub fn probe() -> Option<bool> {
        DEVICE.get().map(|d| d.is_ok())
    }

    /// A device on the best real GPU (a software adapter is refused: the CPU path is faster).
    fn new() -> Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
        )
        .map_err(|e| Error::Compute(format!("no GPU adapter: {e}")))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err(Error::Compute(format!("only a software adapter ({}) is available", info.name)));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("effectcraft sam2"),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| Error::Compute(format!("no GPU device: {e}")))?;
        let error: Arc<Mutex<Option<String>>> = Arc::default();
        let sink = error.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            log::error!("sam2 gpu: {e}");
            sink.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| e.to_string());
        }));
        let sink = error.clone();
        device.set_device_lost_callback(move |reason, message| {
            sink.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| format!("GPU device lost ({reason:?}): {message}"));
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sam2 kernels"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        Ok(Gpu { device, queue, module, pipelines: Mutex::default(), encoder: Mutex::new(None), turn: Mutex::new(()), error, adapter: info.name })
    }

    /// One GPU pass at a time: a caller holds this while it queues, flushes and reads back a whole job, so
    /// two threads never interleave their dispatches in one command stream.
    pub fn exclusive(&self) -> std::sync::MutexGuard<'_, ()> {
        self.turn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    fn take_error(&self) -> Result<()> {
        match self.error.lock().unwrap_or_else(PoisonError::into_inner).take() {
            Some(e) => Err(Error::Compute(format!("GPU: {e}"))),
            None => Ok(()),
        }
    }

    pub fn upload(&self, data: &[f32]) -> Buf {
        // `write_buffer` instead of a mapped-at-creation buffer: on a lost or out-of-memory device the latter panics inside wgpu,
        // while this records the error for `take_error`.
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (data.len().max(1) as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !data.is_empty() {
            self.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(data));
        }
        Buf { buffer, len: data.len(), f16: false }
    }

    /// A weight matrix: stored as half precision (half the memory) when `EFFECTCRAFT_SAM2_F16` asks for it and every
    /// value fits, as f32 otherwise.
    pub fn upload_weights(&self, data: &[f32]) -> Buf {
        if !half_weights() || data.is_empty() || data.iter().any(|v| v.abs() > 60_000.0) {
            return self.upload(data);
        }
        self.upload_f16(data)
    }

    /// `data` as half precision, two values per word.
    pub(crate) fn upload_f16(&self, data: &[f32]) -> Buf {
        let words: Vec<u32> = data
            .chunks(2)
            .map(|p| {
                let lo = half::f16::from_f32(p.first().copied().unwrap_or(0.0)).to_bits();
                let hi = half::f16::from_f32(p.get(1).copied().unwrap_or(0.0)).to_bits();
                u32::from(lo) | (u32::from(hi) << 16)
            })
            .collect();
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (words.len() as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&words));
        Buf { buffer, len: data.len(), f16: true }
    }

    /// A zeroed buffer of `len` elements.
    pub fn zeros(&self, len: usize) -> Result<Buf> {
        let bytes = (len.max(1) as u64).checked_mul(4).ok_or_else(|| Error::Budget("GPU buffer size overflows".into()))?;
        if bytes > self.device.limits().max_buffer_size || bytes > self.device.limits().max_storage_buffer_binding_size {
            return Err(Error::Budget(format!("a {} MB GPU buffer is over this GPU's limit", bytes >> 20)));
        }
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Buf { buffer, len, f16: false })
    }

    fn pipeline(&self, entry: &'static str) -> Arc<wgpu::ComputePipeline> {
        let mut map = self.pipelines.lock().unwrap_or_else(PoisonError::into_inner);
        map.entry(entry)
            .or_insert_with(|| {
                Arc::new(self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: None,
                    module: &self.module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                }))
            })
            .clone()
    }

    /// Queue one dispatch of `entry`. `bindings` are the kernel's buffers by binding index.
    pub(crate) fn run(&self, entry: &'static str, bindings: &[(u32, &Buf)], params: &[u32], groups: (u32, u32, u32)) {
        let pipeline = self.pipeline(entry);
        let uniform = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&words(params)),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut entries: Vec<wgpu::BindGroupEntry<'_>> =
            bindings.iter().map(|(i, b)| wgpu::BindGroupEntry { binding: *i, resource: b.buffer.as_entire_binding() }).collect();
        entries.push(wgpu::BindGroupEntry { binding: 5, resource: uniform.as_entire_binding() });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &entries });
        let mut slot = self.encoder.lock().unwrap_or_else(PoisonError::into_inner);
        let enc = slot.get_or_insert_with(|| self.device.create_command_encoder(&Default::default()));
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(groups.0, groups.1, groups.2);
    }

    /// Submit what is queued and wait for it to finish.
    pub fn flush(&self) -> Result<()> {
        let enc = self.encoder.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(enc) = enc {
            self.queue.submit([enc.finish()]);
        }
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        self.take_error()
    }

    /// Copy a buffer back to the CPU (after submitting everything queued).
    pub fn download(&self, buf: &Buf) -> Result<Vec<f32>> {
        let bytes = buf.len as u64 * 4;
        if bytes == 0 {
            return Ok(vec![]);
        }
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc =
            self.encoder.lock().unwrap_or_else(PoisonError::into_inner).take().unwrap_or_else(|| self.device.create_command_encoder(&Default::default()));
        enc.copy_buffer_to_buffer(&buf.buffer, 0, &staging, 0, bytes);
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        self.take_error()?;
        rx.recv().map_err(|_| Error::Compute("GPU readback never completed".into()))?.map_err(|e| Error::Compute(format!("GPU readback: {e}")))?;
        let out = {
            let view = staging.get_mapped_range(..).map_err(|e| Error::Compute(format!("GPU readback: {e}")))?;
            bytemuck::cast_slice::<u8, f32>(&view).to_vec()
        };
        staging.unmap();
        Ok(out)
    }

    /// Queue `C[b] = act(alpha * A[b] . B[b] + bias) (+ residual)`.
    pub fn matmul(&self, op: &Matmul<'_>) -> Result<()> {
        let (a, sa) = op.a;
        let (b, sb) = op.b;
        let (c, sc) = op.c;
        if op.m == 0 || op.n == 0 || op.batch == 0 {
            return Ok(());
        }
        if sc.col != 1 {
            return Err(Error::Input(format!("matmul output needs a column stride of 1, got {}", sc.col)));
        }
        // bounds: the last element each operand touches must lie inside its buffer
        let last = |s: Strides, rows: usize, cols: usize| {
            s.offset.checked_add((op.batch - 1).checked_mul(s.batch)?)?.checked_add((rows - 1).checked_mul(s.row)?)?.checked_add((cols - 1).checked_mul(s.col)?)
        };
        for (name, buf, s, rows, cols) in [("A", a, sa, op.m, op.k), ("B", b, sb, op.k, op.n), ("C", c, sc, op.m, op.n)] {
            if name != "C" && op.k == 0 {
                continue;
            }
            let last = last(s, rows, cols).ok_or_else(|| Error::Input(format!("matmul operand {name} layout overflows its element index")))?;
            if last >= buf.len {
                return Err(Error::Input(format!("matmul operand {name} reaches element {last} of a {}-element buffer", buf.len)));
            }
        }
        if op.bias.is_some_and(|bias| bias.len < op.n) || op.residual.is_some_and(|r| r.len < c.len) {
            return Err(Error::Input("matmul bias or residual is too small".into()));
        }
        let flags = u32::from(op.bias.is_some())
            | (match op.act {
                Act::None => 0,
                Act::Gelu => 1,
                Act::Relu => 2,
            } << 1)
            | (u32::from(op.residual.is_some()) << 3);
        let params = [
            u32_of(op.m, "m")?,
            u32_of(op.n, "n")?,
            u32_of(op.k, "k")?,
            u32_of(op.batch, "batch")?,
            u32_of(sa.offset, "a offset")?,
            u32_of(sa.batch, "a batch")?,
            u32_of(sa.row, "a row")?,
            u32_of(sa.col, "a col")?,
            u32_of(sb.offset, "b offset")?,
            u32_of(sb.batch, "b batch")?,
            u32_of(sb.row, "b row")?,
            u32_of(sb.col, "b col")?,
            u32_of(sc.offset, "c offset")?,
            u32_of(sc.batch, "c batch")?,
            u32_of(sc.row, "c row")?,
            flags,
            op.alpha.to_bits(),
        ];
        let dummy;
        let bias = match op.bias {
            Some(b) => b,
            None => {
                dummy = self.upload(&[0.0]);
                &dummy
            }
        };
        // a buffer is never bound twice in one dispatch (read and read-write), so no residual means a dummy
        let dummy_res;
        let res = match op.residual {
            Some(r) => r,
            None => {
                dummy_res = self.upload(&[0.0]);
                &dummy_res
            }
        };
        self.run(
            if b.f16 { "matmul_w16" } else { "matmul" },
            &[(0, a), (1, b), (2, bias), (3, c), (4, res)],
            &params,
            (u32_of(op.n.div_ceil(64), "groups")?, u32_of(op.m.div_ceil(64), "groups")?, u32_of(op.batch, "batch")?),
        );
        Ok(())
    }
}

pub mod encoder;
pub mod memattn;
pub mod memenc;
mod ops;

/// A linear layer's weight `[out, in]` and bias `[out]` on the GPU.
pub struct GLinear {
    pub(crate) w: Buf,
    pub(crate) b: Buf,
    pub(crate) inp: usize,
    pub(crate) out: usize,
}

/// A layer norm's scale and shift on the GPU.
pub struct GNorm {
    pub(crate) w: Buf,
    pub(crate) b: Buf,
}

fn flat(t: candle_core::Tensor) -> Result<Vec<f32>> {
    Ok(t.flatten_all()?.to_vec1::<f32>()?)
}

impl GLinear {
    /// From `{name}.weight` (`[out, in]`) and `{name}.bias`.
    pub fn load(g: &Gpu, w: &crate::weights::Weights, name: &str, inp: usize, out: usize) -> Result<GLinear> {
        Ok(GLinear {
            w: g.upload_weights(&flat(w.get(&format!("{name}.weight"), &[out, inp])?)?),
            b: g.upload(&flat(w.get(&format!("{name}.bias"), &[out])?)?),
            inp,
            out,
        })
    }

    /// As [`GLinear::load`], with output channel `j` scaled by `scale[j]` (the memory fuser's layer scale).
    pub fn load_scaled(g: &Gpu, w: &crate::weights::Weights, name: &str, inp: usize, out: usize, scale: &[f32]) -> Result<GLinear> {
        if scale.len() != out {
            return Err(Error::Weights(format!("`{name}`: {} scales for {out} outputs", scale.len())));
        }
        let mut wt = flat(w.get(&format!("{name}.weight"), &[out, inp])?)?;
        let mut bias = flat(w.get(&format!("{name}.bias"), &[out])?)?;
        for (j, s) in scale.iter().enumerate() {
            if let Some(row) = wt.get_mut(j * inp..(j + 1) * inp) {
                row.iter_mut().for_each(|v| *v *= s);
            }
            if let Some(b) = bias.get_mut(j) {
                *b *= s;
            }
        }
        Ok(GLinear { w: g.upload_weights(&wt), b: g.upload(&bias), inp, out })
    }

    /// A 1 x 1 convolution (`[out, in, 1, 1]`) as a linear layer over channels-last tokens.
    pub fn load_conv1x1(g: &Gpu, w: &crate::weights::Weights, name: &str, inp: usize, out: usize) -> Result<GLinear> {
        Ok(GLinear {
            w: g.upload_weights(&flat(w.get(&format!("{name}.weight"), &[out, inp, 1, 1])?)?),
            b: g.upload(&flat(w.get(&format!("{name}.bias"), &[out])?)?),
            inp,
            out,
        })
    }
}

impl GNorm {
    pub fn load(g: &Gpu, w: &crate::weights::Weights, name: &str, dim: usize) -> Result<GNorm> {
        Ok(GNorm { w: g.upload(&flat(w.get(&format!("{name}.weight"), &[dim])?)?), b: g.upload(&flat(w.get(&format!("{name}.bias"), &[dim])?)?) })
    }
}

impl Gpu {
    /// `out = act(x . W^T + bias) (+ residual)` for `rows` rows of `l.inp` values.
    pub fn linear(&self, x: &Buf, rows: usize, l: &GLinear, out: &Buf, act: Act, residual: Option<&Buf>) -> Result<()> {
        // the weight is stored [out, in]: B[k][j] = w[j * in + k]
        self.matmul(&Matmul {
            m: rows,
            n: l.out,
            k: l.inp,
            batch: 1,
            a: (x, Strides::row_major(l.inp)),
            b: (&l.w, Strides { offset: 0, batch: 0, row: 1, col: l.inp }),
            bias: Some(&l.b),
            c: (out, Strides::row_major(l.out)),
            residual,
            alpha: 1.0,
            act,
        })
    }
}

#[cfg(test)]
mod tests;
