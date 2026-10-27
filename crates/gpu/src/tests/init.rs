//! Native constructor containment. Tiny original shaders; no frames or large allocations.
//! Set EFFECTCRAFT_REQUIRE_GPU=1 for acceptance: unavailable adapters/devices then fail
//! instead of taking the explicitly logged headless-CI skip path.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used)] // Test helpers require successful setup to exercise rejection.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "windows")]
use crate::context::GpuContext;
use crate::context::init_resource;

fn brief(message: impl std::fmt::Display) -> String {
    message.to_string().chars().take(1024).collect()
}

fn native_device(desc: wgpu::InstanceDescriptor, required: bool) -> Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    crate::tests::hold_gpu_lock();
    let required = required || std::env::var("EFFECTCRAFT_REQUIRE_GPU").is_ok_and(|value| value == "1");
    let instance = wgpu::Instance::new(desc);
    let adapter = match pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
    ) {
        Ok(adapter) => adapter,
        Err(error) => {
            assert!(!required, "required initialization-test adapter unavailable: {}", brief(error));
            eprintln!("initialization tests: no adapter, explicitly skipping");
            return None;
        }
    };
    let info = adapter.get_info();
    eprintln!("initialization tests: adapter {} ({:?})", brief(&info.name), info.backend);
    let limits = adapter.limits();
    let desc = wgpu::DeviceDescriptor {
        label: Some("initialization containment test"),
        required_limits: wgpu::Limits {
            max_texture_dimension_2d: limits.max_texture_dimension_2d.min(16384),
            max_buffer_size: limits.max_buffer_size,
            ..wgpu::Limits::default()
        },
        ..Default::default()
    };
    match pollster::block_on(adapter.request_device(&desc)) {
        Ok((device, queue)) => Some((adapter, device, queue)),
        Err(error) => {
            assert!(!required, "required initialization-test device unavailable: {}", brief(error));
            eprintln!("initialization tests: no device, explicitly skipping");
            None
        }
    }
}

fn count_host_errors(device: &wgpu::Device) -> Arc<AtomicUsize> {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    device.on_uncaptured_error(Arc::new(move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    calls
}

fn invalid_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("original deliberately invalid initialization-test module"),
        source: wgpu::ShaderSource::Wgsl("this is deliberately not WGSL".into()),
    })
}

fn valid_module(device: &wgpu::Device) -> wgpu::ShaderModule {
    init_resource(device, "valid tiny module", || {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("original empty initialization-test kernel"),
            source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
        })
    })
    .expect("a valid tiny module still compiles after rejection")
}

fn pipeline(device: &wgpu::Device, module: &wgpu::ShaderModule, entry: &str) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("initialization-test pipeline"),
        layout: None,
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn valid_pipeline(device: &wgpu::Device) {
    let module = valid_module(device);
    init_resource(device, "valid tiny pipeline", || pipeline(device, &module, "main")).expect("a valid pipeline still compiles after rejection");
}

#[test]
fn scoped_module_and_pipeline_errors_preserve_outer_scope_and_host_handler() {
    let Some((_, device, _queue)) = native_device(wgpu::InstanceDescriptor::new_without_display_handle().with_env(), false) else { return };
    let host = count_host_errors(&device);
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let error = init_resource(&device, "invalid tiny module", || invalid_module(&device)).expect_err("invalid module must return Err");
    assert!(error.contains("GPU initialization (invalid tiny module)"), "{}", brief(error));
    let module = valid_module(&device);
    let error = init_resource(&device, "missing tiny entry", || pipeline(&device, &module, "missing_entry")).expect_err("missing entry point must return Err");
    assert!(error.contains("GPU initialization (missing tiny entry)"), "{}", brief(error));
    assert_eq!(host.load(Ordering::SeqCst), 0, "contained errors must not reach the host");
    valid_pipeline(&device);

    // This belongs to our pre-existing caller scope. Its successful pop also proves that
    // none of the three nested helper scopes remain on the native thread-local stack.
    let _invalid = invalid_module(&device);
    assert!(pollster::block_on(outer.pop()).is_some(), "the caller's own scope survives");
    assert_eq!(host.load(Ordering::SeqCst), 0);
    let _invalid = invalid_module(&device);
    assert_eq!(host.load(Ordering::SeqCst), 1, "the original host handler still receives unscoped errors");
}

#[test]
fn initialization_unwind_drains_all_scopes_before_the_next_operation() {
    let Some((_, device, _queue)) = native_device(wgpu::InstanceDescriptor::new_without_display_handle().with_env(), false) else { return };
    let host = count_host_errors(&device);
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        init_resource::<()>(&device, "synthetic native unwind", || panic!("original synthetic initialization failure"))
    }));
    let error = result.expect("the initialization unwind is contained").expect_err("unwind becomes an error");
    assert!(error.contains("synthetic native unwind") && error.contains("original synthetic initialization failure"), "{}", brief(error));
    valid_pipeline(&device);
    let _invalid = invalid_module(&device);
    assert!(pollster::block_on(outer.pop()).is_some(), "all helper scopes must be drained after unwind");
    assert_eq!(host.load(Ordering::SeqCst), 0);
    let _invalid = invalid_module(&device);
    assert_eq!(host.load(Ordering::SeqCst), 1, "unwind must neither replace the host handler nor leak a scope");
}

/// Acceptance for the captured RTX 5090 / FXC failure. Future compiler/shader fixes can
/// legitimately make FXC succeed; this diagnostic is deliberately outside portable CI.
/// On OpenGL the compositor declines at once instead of translating its kernels for tens of
/// seconds and then failing (#243). Skips where there is no OpenGL adapter.
#[test]
fn opengl_adapters_get_cpu_compositing_at_once() {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::GL;
    let Some((adapter, device, queue)) = native_device(desc, false) else { return };
    let t = std::time::Instant::now();
    let error = crate::Gpu::new(&adapter, device, queue).err().expect("no GPU compositor on OpenGL");
    assert!(error.contains("(Gl)"), "{error}");
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires the reproduced Windows DX12/FXC pointwise compiler failure; no adapter skips"]
fn reproduced_fxc_pointwise_failure_returns_error_without_escaping_or_notifying_host() {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::DX12;
    desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let (adapter, device, queue) = native_device(desc, true).expect("required DX12 device");
    assert_eq!(adapter.get_info().backend, wgpu::Backend::Dx12);
    let host = count_host_errors(&device);
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| GpuContext::new(&adapter, device.clone(), queue)));
    let error = match result.expect("FXC compiler failure must not escape the constructor") {
        Err(error) => error,
        Ok(_) => panic!("the platform-specific FXC failure no longer reproduces; reassess this diagnostic"),
    };
    assert!(error.contains("GPU initialization (pointwise)") && error.contains("FXC D3DCompile"), "{}", brief(error));
    assert!(pollster::block_on(outer.pop()).is_none(), "constructor must drain its own scopes and leave the caller's scope clean");
    assert_eq!(host.load(Ordering::SeqCst), 0);
    valid_pipeline(&device);
    let _invalid = invalid_module(&device);
    assert_eq!(host.load(Ordering::SeqCst), 1);
}
