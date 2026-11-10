//! Timing of the SAM 2 image path on this machine: load, encode one frame, decode a click.
//!
//! `cargo run -p effectcraft-sam2 --example sam2_bench -- [MODEL_DIR]` (default `sam2-tiny`).
//! Prints a message and exits when the weights are not there.

use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use effectcraft_sam2::model::ImageModel;
use effectcraft_sam2::prompt::Prompt;

fn main() {
    if let Err(e) = run() {
        eprintln!("sam2_bench: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).unwrap_or_else(|| "sam2-tiny".to_string());
    let file = std::path::Path::new(&dir).join("model.safetensors");
    if !file.exists() {
        println!("no weights at {} (download facebook/sam2.1-hiera-tiny first)", file.display());
        return Ok(());
    }
    let t = Instant::now();
    let model = ImageModel::open(&file)?;
    println!("load: {:.2} s", t.elapsed().as_secs_f64());
    let pixels = Tensor::randn(0f32, 1.0, (1, 3, 1024, 1024), &Device::Cpu)?.to_dtype(DType::F32)?;
    for run in 0..3 {
        let t = Instant::now();
        let enc = model.encode(&pixels)?;
        let encode = t.elapsed().as_secs_f64();
        let prompt = Prompt { points: vec![([512.0, 512.0], true)], bbox: None };
        let t = Instant::now();
        let out = model.segment(&enc, &prompt, true)?;
        println!("run {run}: encode {encode:.2} s, click {:.0} ms, masks {:?}", t.elapsed().as_secs_f64() * 1e3, out.masks.dims());
    }
    // Where the encode time goes: patch embedding, then each Hiera block.
    let hiera = &model.vision.hiera;
    let t = Instant::now();
    let mut x = hiera.patch_embed(&pixels)?;
    println!("patch embed: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    for (i, b) in hiera.blocks.iter().enumerate() {
        let t = Instant::now();
        x = b.forward(&x)?;
        println!("block {i}: {:.0} ms, out {:?}", t.elapsed().as_secs_f64() * 1e3, x.dims());
    }
    // The rest of the encode: the neck (1x1 convolutions, top-down sums) and its sine position encodings.
    let stages = hiera.forward(&pixels)?;
    let t = Instant::now();
    let _ = model.vision.neck.forward(&stages)?;
    println!("neck (convolutions + positions): {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    let t = Instant::now();
    for (h, w) in [(32, 32), (64, 64), (128, 128), (256, 256)] {
        let _ = effectcraft_sam2::neck::sine_position(128, h, w)?;
    }
    println!("sine positions for the four levels: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    // What a tracked frame adds to the encode: memory attention over the memory bank (7 memory frames and
    // 16 object pointers of 4 tokens), then the memory encoder.
    let weights = effectcraft_sam2::weights::Weights::open(&file)?;
    let attn = effectcraft_sam2::memattn::MemoryAttention::load(&weights)?;
    let cur = Tensor::randn(0f32, 1.0, (4096, 256), &Device::Cpu)?;
    let cur_pos = Tensor::randn(0f32, 1.0, (4096, 256), &Device::Cpu)?;
    let m = 7 * 4096 + 64;
    let mem = Tensor::randn(0f32, 1.0, (m, 64), &Device::Cpu)?;
    let mem_pos = Tensor::randn(0f32, 1.0, (m, 64), &Device::Cpu)?;
    for run in 0..2 {
        let t = Instant::now();
        let _ = attn.forward(&cur, &cur_pos, &mem, &mem_pos, 64)?;
        println!("memory attention (run {run}, {m} memory tokens): {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    // The same two costs on the GPU (wgpu kernels), when there is one.
    match effectcraft_sam2::gpu::Gpu::shared() {
        Ok(gpu) => {
            println!("GPU: {}", gpu.adapter_name());
            let enc = effectcraft_sam2::gpu::encoder::GpuEncoder::load(gpu.clone(), &weights, model.vision.hiera.clone())?;
            let gattn = effectcraft_sam2::gpu::memattn::GpuMemoryAttention::load(gpu, &weights)?;
            for run in 0..3 {
                let t = Instant::now();
                let _ = enc.encode(&pixels)?;
                let encode = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let _ = gattn.forward(&cur, &cur_pos, &mem, &mem_pos, 64)?;
                println!("GPU run {run}: encode {encode:.0} ms, memory attention (full bank) {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            }
        }
        Err(e) => println!("no GPU: {e}"),
    }
    // A whole tracked frame (encode, memory attention, decode, memory encoding) through the video session, on each backend.
    struct Same(Tensor);
    impl effectcraft_sam2::session::FrameSource for Same {
        fn num_frames(&self) -> usize {
            12
        }
        fn frame(&self, _i: usize) -> effectcraft_sam2::Result<Tensor> {
            Ok(self.0.clone())
        }
    }
    for backend in [effectcraft_sam2::session::Backend::Cpu, effectcraft_sam2::session::Backend::Gpu] {
        let Ok(vm) = effectcraft_sam2::session::VideoModel::load_with(&weights, backend) else { continue };
        let vm = std::sync::Arc::new(vm);
        let mut tracker = effectcraft_sam2::session::VideoTracker::new(vm.clone(), Box::new(Same(pixels.clone())))?;
        let obj = effectcraft_sam2::session::ObjectId(1);
        let t = Instant::now();
        tracker.prompt(0, obj, &Prompt { points: vec![([512.0, 512.0], true)], bbox: None })?;
        println!("{:?} ({}): prompt frame {:.0} ms", backend, vm.device_name(), t.elapsed().as_secs_f64() * 1e3);
        let mut last = Instant::now();
        let mut times = vec![];
        tracker.propagate(0, effectcraft_sam2::session::Direction::Forward, None, &effectcraft_sam2::session::Cancel::new(), &mut |m| {
            if m.frame > 0 {
                times.push(last.elapsed().as_secs_f64() * 1e3);
            }
            last = Instant::now();
        })?;
        println!(
            "{:?}: tracked frames {:.0} ms each (first {:.0}, last {:.0})",
            backend,
            times.iter().sum::<f64>() / times.len().max(1) as f64,
            times.first().copied().unwrap_or(0.0),
            times.last().copied().unwrap_or(0.0)
        );
    }
    println!("threads: {}", std::thread::available_parallelism().map_or(1, |n| n.get()));
    Ok(())
}
