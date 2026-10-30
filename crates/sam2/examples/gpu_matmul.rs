//! A spike for the GPU path (plan phase 6): how fast is a plain tiled WGSL matrix multiply on this
//! machine's GPU, against the candle CPU matmul the encoder uses today? Candle has no AMD/Vulkan
//! backend; this measures what hand-written wgpu kernels could reach on the shapes SAM 2 needs.
//!
//! `cargo run --release -p effectcraft-sam2 --example gpu_matmul`

use std::time::Instant;

use candle_core::{Device, Tensor};
use wgpu::util::DeviceExt;

const SHADER: &str = r#"
struct P { m: u32, n: u32, k: u32, pad: u32 }
@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read> b: array<f32>;
@group(0) @binding(2) var<storage, read_write> c: array<f32>;
@group(0) @binding(3) var<uniform> p: P;
var<workgroup> sa: array<f32, 1024>;
var<workgroup> sb: array<f32, 1024>;
@compute @workgroup_size(16, 16)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) li: vec3<u32>) {
    let row0 = wg.y * 64u;
    let col0 = wg.x * 64u;
    let tid = li.y * 16u + li.x;
    var acc: array<f32, 16>;
    for (var i = 0u; i < 16u; i++) { acc[i] = 0.0; }
    for (var kt = 0u; kt < p.k; kt += 16u) {
        for (var i = 0u; i < 4u; i++) {
            let idx = tid + i * 256u;
            let r = idx / 16u;
            let cc = idx % 16u;
            let gr = row0 + r;
            let gc = kt + cc;
            var va = 0.0;
            if (gr < p.m && gc < p.k) { va = a[gr * p.k + gc]; }
            sa[idx] = va;
            let r2 = idx / 64u;
            let c2 = idx % 64u;
            let gr2 = kt + r2;
            let gc2 = col0 + c2;
            var vb = 0.0;
            if (gr2 < p.k && gc2 < p.n) { vb = b[gr2 * p.n + gc2]; }
            sb[idx] = vb;
        }
        workgroupBarrier();
        for (var kk = 0u; kk < 16u; kk++) {
            var ar: array<f32, 4>;
            var br: array<f32, 4>;
            for (var i = 0u; i < 4u; i++) {
                ar[i] = sa[(li.y * 4u + i) * 16u + kk];
                br[i] = sb[kk * 64u + li.x * 4u + i];
            }
            for (var i = 0u; i < 4u; i++) {
                for (var j = 0u; j < 4u; j++) { acc[i * 4u + j] += ar[i] * br[j]; }
            }
        }
        workgroupBarrier();
    }
    for (var i = 0u; i < 4u; i++) {
        for (var j = 0u; j < 4u; j++) {
            let gr = row0 + li.y * 4u + i;
            let gc = col0 + li.x * 4u + j;
            if (gr < p.m && gc < p.n) { c[gr * p.n + gc] = acc[i * 4u + j]; }
        }
    }
}
"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
    )?;
    println!("adapter: {:?}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() }))?;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    // (name, m, k, n): the matmul shapes of SAM 2's encoder and memory attention
    let shapes = [
        ("hiera block 0 qkv", 65536usize, 96usize, 288usize),
        ("hiera block 0 mlp in", 65536, 96, 384),
        ("hiera global attention q.kT (one head)", 4096, 96, 4096),
        ("memory attention q.kT (full bank)", 4096, 256, 28736),
    ];
    for (name, m, k, n) in shapes {
        let a = Tensor::randn(0f32, 1.0, (m, k), &Device::Cpu)?;
        let b = Tensor::randn(0f32, 1.0, (k, n), &Device::Cpu)?;
        let av = a.flatten_all()?.to_vec1::<f32>()?;
        let bv = b.flatten_all()?.to_vec1::<f32>()?;
        let ba = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&av),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&bv),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bc = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (m * n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = [m as u32, n as u32, k as u32, 0];
        let bp = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ba.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: bb.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: bc.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: bp.as_entire_binding() },
            ],
        });
        let run = |reps: usize| {
            let mut enc = device.create_command_encoder(&Default::default());
            for _ in 0..reps {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.dispatch_workgroups(n.div_ceil(64) as u32, m.div_ceil(64) as u32, 1);
            }
            queue.submit([enc.finish()]);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        };
        run(1);
        let reps = 5;
        let t = Instant::now();
        run(reps);
        let gpu = t.elapsed().as_secs_f64() / reps as f64;
        let flops = 2.0 * m as f64 * k as f64 * n as f64;
        // the CPU: candle's matmul, as the encoder uses it
        let _ = a.matmul(&b)?;
        let t = Instant::now();
        let cpu_reps = 3;
        for _ in 0..cpu_reps {
            let _ = a.matmul(&b)?;
        }
        let cpu = t.elapsed().as_secs_f64() / cpu_reps as f64;
        println!(
            "{name:<42} gpu {:>7.1} ms ({:>6.0} GFLOPS)   cpu {:>7.1} ms ({:>5.0} GFLOPS)   x{:.1}",
            gpu * 1e3,
            flops / gpu / 1e9,
            cpu * 1e3,
            flops / cpu / 1e9,
            cpu / gpu
        );
    }
    Ok(())
}
