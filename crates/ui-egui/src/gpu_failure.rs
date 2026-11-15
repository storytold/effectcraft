//! Host-owned shared-device failure forwarding. Device handlers belong to the executable or
//! embedding host, never to a library borrowing its device. This bridge does no device work,
//! document mutation, file I/O or renderer locking inside a notification.

use std::sync::{Arc, Mutex, OnceLock};

pub use effectcraft_gpu::{ScopedError, ScopedErrorObserver};

type Notifier = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuFailure {
    pub reason: String,
    /// Device loss requires restarting/recreating the presentation backend too. CPU
    /// compositing alone cannot make a lost eframe presentation device usable again.
    pub presentation_lost: bool,
}

#[derive(Default)]
struct State {
    failure: Option<GpuFailure>,
    notifier: Option<Notifier>,
}

/// One bridge per host device. Failure is latched; duplicate reports cannot grow a queue.
#[derive(Clone)]
pub struct GpuFailureBridge {
    state: Arc<Mutex<State>>,
    ctx: egui::Context,
    scoped_observer: Arc<OnceLock<ScopedErrorObserver>>,
}

impl GpuFailureBridge {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { state: Arc::default(), ctx: ctx.clone(), scoped_observer: Arc::default() }
    }

    /// Register the host observer before handing this bridge to the app. Scoped errors do
    /// not retire the device: they remain recoverable per-frame observations.
    pub fn set_scoped_error_observer(&self, observer: impl Fn(ScopedError) + Send + Sync + 'static) -> bool {
        self.scoped_observer.set(Arc::new(observer)).is_ok()
    }

    pub(crate) fn scoped_error_observer(&self) -> Option<ScopedErrorObserver> {
        self.scoped_observer.get().cloned()
    }

    pub fn failure(&self) -> Option<GpuFailure> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).failure.clone()
    }

    /// Forward to a compositor's weak notifier once construction succeeds. A fault that
    /// preceded registration is replayed; the compositor must not then be attached to the UI.
    pub fn forward_to(&self, notifier: impl Fn(String) + Send + Sync + 'static) {
        let notifier: Notifier = Arc::new(notifier);
        let failure = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.notifier = Some(notifier.clone());
            state.failure.clone()
        };
        if let Some(failure) = failure {
            notify(&notifier, failure.reason);
        }
    }

    /// Called by the device owner for an uncaptured error or device loss. A later device-loss
    /// notification escalates the recovery message without completing pending work twice.
    pub fn report(&self, reason: &str, presentation_lost: bool) -> bool {
        let (notification, changed) = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(failure) = &mut state.failure {
                let changed = presentation_lost && !failure.presentation_lost;
                failure.presentation_lost |= presentation_lost;
                (None, changed)
            } else {
                let reason: String = reason.chars().take(2048).collect();
                state.failure = Some(GpuFailure { reason: reason.clone(), presentation_lost });
                (state.notifier.clone().map(|notifier| (notifier, reason)), true)
            }
        };
        if let Some((notifier, reason)) = notification {
            notify(&notifier, reason);
        }
        if changed {
            self.ctx.request_repaint();
        }
        changed
    }
}

fn notify(notifier: &Notifier, reason: String) {
    // A host callback must never propagate a consumer panic into wgpu's error handling.
    #[cfg(not(target_arch = "wasm32"))]
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notifier(reason))).is_err() {
        log::error!("GPU failure consumer panicked");
    }
    #[cfg(target_arch = "wasm32")]
    notifier(reason);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn scoped_observer_is_shared_without_retiring_the_device() {
        let bridge = GpuFailureBridge::new(&egui::Context::default());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let copy = seen.clone();
        assert!(bridge.set_scoped_error_observer(move |error| copy.lock().unwrap().push(error)));
        assert!(!bridge.set_scoped_error_observer(|_| panic!("must not replace observer")));
        bridge.clone().scoped_error_observer().unwrap()(ScopedError::Internal);
        assert_eq!(*seen.lock().unwrap(), [ScopedError::Internal]);
        assert!(bridge.failure().is_none());
    }

    #[test]
    fn a_construction_failure_is_replayed_after_notifier_registration() {
        let bridge = GpuFailureBridge::new(&egui::Context::default());
        bridge.report("pipeline allocation failed", false);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let copy = observed.clone();
        let probe = Arc::downgrade(&bridge.state);
        bridge.forward_to(move |reason| {
            assert!(probe.upgrade().unwrap().try_lock().is_ok(), "consumer runs outside bridge lock");
            copy.lock().unwrap().push(reason);
        });
        assert_eq!(*observed.lock().unwrap(), ["pipeline allocation failed"]);
        bridge.report("duplicate", false);
        assert_eq!(observed.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_late_failure_notifies_once_and_later_device_loss_escalates() {
        let bridge = GpuFailureBridge::new(&egui::Context::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let copy = calls.clone();
        bridge.forward_to(move |_| {
            copy.fetch_add(1, Ordering::SeqCst);
        });
        assert!(bridge.failure().is_none());
        bridge.report("uncaptured allocation failure", false);
        bridge.report("device lost", true);
        bridge.report("another error", false);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let failure = bridge.failure().unwrap();
        assert!(failure.presentation_lost);
        assert_eq!(failure.reason, "uncaptured allocation failure");
    }

    #[test]
    fn diagnostics_are_bounded_and_native_consumer_panics_are_isolated() {
        let bridge = GpuFailureBridge::new(&egui::Context::default());
        #[cfg(not(target_arch = "wasm32"))]
        bridge.forward_to(|_| panic!("synthetic consumer failure"));
        bridge.report(&"é".repeat(3000), false);
        assert_eq!(bridge.failure().unwrap().reason.chars().count(), 2048);
        bridge.report("device lost after consumer failure", true);
        assert!(bridge.failure().unwrap().presentation_lost);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn concurrent_registration_and_failure_deliver_exactly_once() {
        let bridge = GpuFailureBridge::new(&egui::Context::default());
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let calls = Arc::new(AtomicUsize::new(0));
        let reporter = bridge.clone();
        let ready = barrier.clone();
        let thread = std::thread::spawn(move || {
            ready.wait();
            reporter.report("device failed during subscription", false);
        });
        let count = calls.clone();
        barrier.wait();
        bridge.forward_to(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        thread.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
