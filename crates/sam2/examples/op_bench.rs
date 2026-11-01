//! Times the tensor operations a Hiera block is made of, at the sizes of its first (biggest) block.
//! `cargo run --release -p effectcraft-sam2 --example op_bench`

use std::time::Instant;

use candle_core::{DType, Device, Tensor};
use candle_nn::{Linear, Module};

fn time<T>(name: &str, mut f: impl FnMut() -> T) {
    let _ = f();
    let n = 5;
    let t = Instant::now();
    for _ in 0..n {
        let _ = f();
    }
    println!("{name:<44} {:>8.1} ms", t.elapsed().as_secs_f64() * 1e3 / n as f64);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let d = Device::Cpu;
    // block 0: 1 x 256 x 256 x 96 activations, windows of 8 x 8
    let x = Tensor::randn(0f32, 1.0, (1, 256, 256, 96), &d)?;
    let w = Tensor::randn(0f32, 0.1, (288, 96), &d)?;
    let b = Tensor::zeros(288, DType::F32, &d)?;
    let lin = Linear::new(w, Some(b));
    let flat = x.reshape((65536, 96))?;
    time("linear 65536x96 -> 288 (qkv)", || lin.forward(&flat).unwrap());
    let big = Tensor::randn(0f32, 1.0, (65536, 384), &d)?;
    time("gelu_erf on 65536 x 384", || big.gelu_erf().unwrap());
    time("relu on 65536 x 384", || big.relu().unwrap());
    time("add 65536 x 384", || (&big + &big).unwrap());
    let xs = x.clone();
    time("permute+contiguous (window partition)", || x.reshape((1, 32, 8, 32, 8, 96)).unwrap().permute((0, 1, 3, 2, 4, 5)).unwrap().contiguous().unwrap());
    let lnw = Tensor::ones(96, DType::F32, &d)?;
    let lnb = Tensor::zeros(96, DType::F32, &d)?;
    time("fused layer_norm 256x256x96", || candle_nn::ops::layer_norm(&xs, &lnw, &lnb, 1e-6).unwrap());
    // windowed attention: 1024 windows x 64 tokens x 96 dims, one head
    let q = Tensor::randn(0f32, 1.0, (1024, 1, 64, 96), &d)?;
    let kt = Tensor::randn(0f32, 1.0, (1024, 1, 96, 64), &d)?;
    time("batched matmul 1024 x (64x96)(96x64)", || q.matmul(&kt).unwrap());
    let sc = Tensor::randn(0f32, 1.0, (1024, 1, 64, 64), &d)?;
    time("softmax_last_dim 1024x64x64", || candle_nn::ops::softmax_last_dim(&sc).unwrap());
    let v = Tensor::randn(0f32, 1.0, (1024, 1, 64, 96), &d)?;
    time("batched matmul 1024 x (64x64)(64x96)", || sc.matmul(&v).unwrap());
    // the MLP matmuls
    let w1 = Linear::new(Tensor::randn(0f32, 0.1, (384, 96), &d)?, Some(Tensor::zeros(384, DType::F32, &d)?));
    time("linear 65536x96 -> 384 (mlp in)", || w1.forward(&flat).unwrap());
    let w2 = Linear::new(Tensor::randn(0f32, 0.1, (96, 384), &d)?, Some(Tensor::zeros(96, DType::F32, &d)?));
    time("linear 65536x384 -> 96 (mlp out)", || w2.forward(&big).unwrap());
    // block 1 (the first stage change): pooling and the wide projections
    let p1 = Tensor::randn(0f32, 1.0, (1, 192, 256, 256), &d)?;
    time("max_pool2d 2x2 on 1 x 192 x 256 x 256", || p1.max_pool2d_with_stride(2, 2).unwrap());
    let p2 = Tensor::randn(0f32, 1.0, (1024, 192, 8, 8), &d)?;
    time("max_pool2d 2x2 on 1024 x 192 x 8 x 8", || p2.max_pool2d_with_stride(2, 2).unwrap());
    let nhwc = Tensor::randn(0f32, 1.0, (1, 256, 256, 192), &d)?;
    time("permute NHWC -> NCHW + contiguous (192ch)", || nhwc.permute((0, 3, 1, 2)).unwrap().contiguous().unwrap());
    let w3 = Linear::new(Tensor::randn(0f32, 0.1, (192, 96), &d)?, Some(Tensor::zeros(192, DType::F32, &d)?));
    time("linear 65536x96 -> 192 (block 1 proj)", || w3.forward(&flat).unwrap());
    time("NHWC 2x2 max via reshape + max(2), max(3)", || nhwc.reshape((1, 128, 2, 128, 2, 192)).unwrap().max(2).unwrap().max(3).unwrap());
    time("NHWC 2x2 max via two strided maxes of narrowed", || {
        let a = nhwc.reshape((1, 128, 2, 256, 192)).unwrap();
        let m = a.narrow(2, 0, 1).unwrap().maximum(&a.narrow(2, 1, 1).unwrap()).unwrap().squeeze(2).unwrap();
        let b = m.reshape((1, 128, 128, 2, 192)).unwrap();
        b.narrow(3, 0, 1).unwrap().maximum(&b.narrow(3, 1, 1).unwrap()).unwrap().squeeze(3).unwrap()
    });
    println!("threads: {}", std::thread::available_parallelism().map_or(1, |n| n.get()));
    Ok(())
}
