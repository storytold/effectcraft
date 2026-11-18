//! Per-compositor, fixed-category observations from error scopes. No driver strings escape.

#[cfg(any(test, not(target_arch = "wasm32")))]
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopedError {
    OutOfMemory,
    Internal,
    Validation,
}

pub type ScopedErrorObserver = Arc<dyn Fn(ScopedError) + Send + Sync>;

#[derive(Default)]
pub(crate) struct ScopedObservations {
    observer: OnceLock<ScopedErrorObserver>,
    #[cfg(any(test, not(target_arch = "wasm32")))]
    seen: AtomicU8,
}

impl ScopedObservations {
    /// Register before rendering. Shared by all clones of the compositor, never global.
    pub(crate) fn install(&self, observer: ScopedErrorObserver) -> bool {
        self.observer.set(observer).is_ok()
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    pub(crate) fn report_scopes(&self, oom: bool, internal: bool, validation: bool) {
        for (present, category) in [(oom, ScopedError::OutOfMemory), (internal, ScopedError::Internal), (validation, ScopedError::Validation)] {
            if present {
                self.report(category);
            }
        }
    }

    #[cfg(any(test, not(target_arch = "wasm32")))]
    fn report(&self, error: ScopedError) {
        let Some(observer) = self.observer.get() else { return };
        let bit = match error {
            ScopedError::OutOfMemory => 1,
            ScopedError::Internal => 2,
            ScopedError::Validation => 4,
        };
        if self.seen.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
            return;
        }
        // Diagnostics must not turn a recoverable GPU frame error into an application panic.
        #[cfg(not(target_arch = "wasm32"))]
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(error))).is_err() {
            log::warn!("GPU scoped-error observer panicked");
        }
        #[cfg(target_arch = "wasm32")]
        observer(error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_are_independent_bounded_and_not_global() {
        let observations = ScopedObservations::default();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let copy = seen.clone();
        assert!(observations.install(Arc::new(move |error| copy.lock().unwrap().push(error))));
        assert!(!observations.install(Arc::new(|_| panic!("must not replace observer"))));
        for _ in 0..100 {
            observations.report(ScopedError::Internal);
            observations.report(ScopedError::Validation);
            observations.report(ScopedError::OutOfMemory);
        }
        ScopedObservations::default().report(ScopedError::OutOfMemory);
        assert_eq!(*seen.lock().unwrap(), [ScopedError::Internal, ScopedError::Validation, ScopedError::OutOfMemory]);
    }

    #[test]
    fn internal_and_validation_scopes_do_not_claim_out_of_memory() {
        let observations = ScopedObservations::default();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let copy = seen.clone();
        observations.install(Arc::new(move |error| copy.lock().unwrap().push(error)));
        observations.report_scopes(false, false, false);
        assert!(seen.lock().unwrap().is_empty());
        observations.report_scopes(false, true, true);
        assert_eq!(*seen.lock().unwrap(), [ScopedError::Internal, ScopedError::Validation]);
        observations.report_scopes(true, true, true);
        assert_eq!(*seen.lock().unwrap(), [ScopedError::Internal, ScopedError::Validation, ScopedError::OutOfMemory]);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn observer_panic_does_not_escape_frame_recovery() {
        let observations = ScopedObservations::default();
        observations.install(Arc::new(|_| panic!("synthetic observer panic")));
        observations.report(ScopedError::OutOfMemory);
        observations.report(ScopedError::Internal);
        assert_eq!(observations.seen.load(Ordering::Relaxed), 3);
    }
}
