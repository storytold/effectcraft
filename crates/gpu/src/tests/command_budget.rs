//! Metal's command-buffer budget (#2746): wgpu-hal refuses a new command buffer, and reports the
//! device lost, once 4096 are created but not yet submitted on a queue. wgpu-core spends two
//! per compute pass when an encoder is finished, so sixteen preview workers each finishing a
//! full batch at once went past it.
#![cfg(not(target_arch = "wasm32"))]

use crate::Gpu;
use crate::context::{Enc, Params};

/// Sixteen threads submit full batches of small kernels at the same time, as the preview's
/// workers do on a heavy comp. A device of its own: before the fix this lost the device.
#[test]
fn concurrent_full_batches_keep_the_device() {
    crate::tests::hold_gpu_lock();
    let Some(gpu) = Gpu::headless() else {
        eprintln!("command budget: no adapter, skipping");
        return;
    };
    let g = gpu.context();
    let start = std::sync::Barrier::new(16);
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| {
                let mut e = Enc::new(g);
                let src = e.image(16, 16);
                let out = e.image(16, 16);
                start.wait();
                for _ in 0..24 * 256 {
                    e.pixels("fill", &Params::default(), &src, None, &out, None);
                }
                e.submit();
            });
        }
    });
    let mut e = Enc::new(g);
    let img = e.image(16, 16);
    assert!(e.download(&img).is_some(), "the device no longer renders");
    assert!(g.can_readback(), "the device was lost");
}
