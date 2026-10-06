//! Explicit bundled-compiler acceptance, separate from Auto's dynamic-DLL/FXC fallback.
//! No frames, submissions, or large allocations: constructor resources are a 1x1 dummy
//! texture, a 16-byte dummy buffer, and the actual compositor's core pipelines.
#![cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]

use crate::Gpu;
use crate::context::GpuContext;

#[test]
#[ignore = "requires a real Windows x64 MSVC DX12 device and bundled Static DXC; never skips"]
fn bundled_static_dxc_compiles_compositor_core_pipelines_on_dx12() {
    crate::tests::hold_gpu_lock();
    // Deliberately do not apply environment overrides: neither Auto nor a DXC DLL on
    // PATH may satisfy this test. Without static-dxc support, instance/adapter setup fails.
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::StaticDxc;
    let instance = wgpu::Instance::new(desc);
    let adapter = pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
    )
    .expect("explicit Static DXC acceptance requires a usable DX12 adapter; absence is not a skip");
    let info = adapter.get_info();
    assert_eq!(info.backend, wgpu::Backend::Dx12, "this fixture must exercise DX12");
    eprintln!("Static DXC acceptance adapter: {} ({:?}); compiler explicitly StaticDxc", info.name, info.backend);

    let limits = adapter.limits();
    let required_limits = wgpu::Limits {
        max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384),
        max_buffer_size: limits.max_buffer_size,
        ..wgpu::Limits::default()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("explicit bundled Static DXC acceptance"),
        required_limits,
        ..Default::default()
    }))
    .expect("explicit Static DXC acceptance requires the compositor's device limits");

    // Also reject invalid placeholder handles if constructor error handling ever changes
    // to logging. Drain every scope before asserting; this is native wgpu-core only.
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let context = GpuContext::new(&adapter, device.clone(), queue);
    let validation_error = pollster::block_on(validation.pop());
    let internal_error = pollster::block_on(internal.pop());
    let oom_error = pollster::block_on(oom.pop());
    if let Some(error) = validation_error.or(internal_error).or(oom_error) {
        let diagnostic: String = error.to_string().chars().take(1024).collect();
        panic!("Static DXC compositor initialization reported a GPU error: {diagnostic}");
    }
    let gpu = Gpu::from_context(context.expect("all actual compositor core pipelines must compile with bundled Static DXC"));
    assert_eq!(effectcraft_render::Accelerator::name(&gpu), format!("{} ({:?})", info.name, info.backend));
    eprintln!("Static DXC acceptance: actual compositor core pipelines compiled successfully");
}
