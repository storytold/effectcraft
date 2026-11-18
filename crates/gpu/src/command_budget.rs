//! Bounds the command buffers that are created but not yet submitted on the device (#2746).
//!
//! wgpu-hal's Metal backend refuses a new command buffer, and reports the device lost, once
//! 4096 of them exist on a queue without having been submitted. wgpu-core creates them when an
//! encoder is finished: two per compute pass (the pass and the barriers before it), and one more
//! where copies sit between passes. An [`Enc`](crate::context::Enc) batches up to 256 passes,
//! so one finish makes several hundred, and the preview's workers finish at the same time:
//! sixteen full batches are over the limit. Each `Enc` therefore takes its share here before it
//! finishes its encoder and gives it back once the work is submitted.

use std::sync::{Condvar, Mutex, PoisonError};

/// Command buffers that tracked encoders may hold between finish and submit. Below the 4096
/// wgpu-hal allows, with room for the encoders that record a few commands without taking a
/// share (uploads, readbacks, the viewer).
const BUDGET: usize = 3200;

/// The device-wide count of command buffers between finish and submit.
#[derive(Default)]
pub(crate) struct CommandBudget {
    held: Mutex<usize>,
    freed: Condvar,
}

/// A share of the budget, returned when dropped.
pub(crate) struct Share<'a> {
    budget: &'a CommandBudget,
    cost: usize,
}

impl CommandBudget {
    /// Waits until an encoder of `passes` compute passes fits, and takes its share.
    pub(crate) fn take(&self, passes: usize) -> Share<'_> {
        // Three per pass and the encoder's own first and last; never more than the whole budget,
        // so a lone encoder always fits.
        let cost = passes.saturating_mul(3).saturating_add(2).min(BUDGET);
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        // A browser has no such limit and its main thread may not block.
        #[cfg(not(target_arch = "wasm32"))]
        while held.saturating_add(cost) > BUDGET {
            held = self.freed.wait(held).unwrap_or_else(PoisonError::into_inner);
        }
        *held = held.saturating_add(cost);
        Share { budget: self, cost }
    }

    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        *self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Share<'_> {
    fn drop(&mut self) {
        let mut held = self.budget.held.lock().unwrap_or_else(PoisonError::into_inner);
        *held = held.saturating_sub(self.cost);
        drop(held);
        self.budget.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_are_returned_and_a_full_budget_waits() {
        let budget = CommandBudget::default();
        let a = budget.take(256);
        assert_eq!(budget.held(), 770);
        // Far more passes than fit still get a share (the whole budget) once it is free.
        drop(a);
        let all = budget.take(usize::MAX);
        assert_eq!(budget.held(), BUDGET);
        let waited = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                let _b = budget.take(1);
                assert!(waited.load(std::sync::atomic::Ordering::SeqCst), "took a share of a full budget");
            });
            std::thread::sleep(std::time::Duration::from_millis(50));
            waited.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(all);
        });
        assert_eq!(budget.held(), 0);
    }
}
