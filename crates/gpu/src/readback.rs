//! Bounded, exactly-once readback completion, independent of GPU callbacks.
// Browser callbacks stay on their worker thread. The workspace's wgpu feature still
// requires Send callbacks on non-atomic wasm, and ParticleSim requires Send + Sync.
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Condvar;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) type Error = String;
type Done = Box<dyn FnOnce(Result<Vec<u8>, Error>) + Send>;
const TIMED_OUT: &str = "GPU readback timed out; context retired";

struct Entry {
    deadline: web_time::Instant,
    done: Done,
}
#[derive(Default)]
struct State {
    next: u64,
    pending: BTreeMap<u64, Entry>,
    retired: Option<Error>,
    #[cfg(target_arch = "wasm32")]
    timer_armed: bool,
    /// The deadline worker is to exit: the context was dropped.
    #[cfg(not(target_arch = "wasm32"))]
    closed: bool,
    /// How many times the deadline worker woke up (tests: it must not wake while idle).
    #[cfg(all(test, not(target_arch = "wasm32")))]
    worker_wakes: u64,
}
/// The state and, natively, the condition the deadline worker sleeps on. The worker owns an
/// `Arc` of this, not of the [`Readbacks`], so it can block on the condition without keeping
/// the context alive; dropping the context closes it.
#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    #[cfg(not(target_arch = "wasm32"))]
    wake: Condvar,
}
#[derive(Default)]
pub(crate) struct Readbacks(Arc<Inner>);

/// Retire the context when a deadline has passed: the entries to fail, or `None` when none
/// is due (or it is retired already). Decides and drains under the lock the caller holds.
fn expire_locked(s: &mut State, now: web_time::Instant) -> Option<Vec<Entry>> {
    // Decide and drain atomically: a cancelled old deadline must not retire
    // a newer request inserted between the deadline check and retirement.
    if s.retired.is_some() || !s.pending.values().any(|e| now >= e.deadline) {
        return None;
    }
    s.retired = Some(TIMED_OUT.to_string());
    Some(std::mem::take(&mut s.pending).into_values().collect())
}

/// The deadline worker. It sleeps while nothing is pending, and otherwise exactly until the
/// earliest deadline or until what is pending changes, so an idle context costs no wakeups
/// (a 100 ms polling loop here was ten wakeups a second for the life of the app). When a
/// deadline has passed it retires the context and fails every pending readback.
#[cfg(not(target_arch = "wasm32"))]
fn run_deadlines(inner: &Inner) {
    let mut s = inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if s.closed {
            return;
        }
        let now = web_time::Instant::now();
        match s.pending.values().map(|e| e.deadline).min() {
            None => s = inner.wake.wait(s).unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(deadline) if deadline > now => {
                s = inner.wake.wait_timeout(s, deadline - now).unwrap_or_else(std::sync::PoisonError::into_inner).0;
            }
            Some(_) => {
                let expired = expire_locked(&mut s, now);
                drop(s);
                for entry in expired.into_iter().flatten() {
                    notify(entry.done, Err(TIMED_OUT.into()));
                }
                s = inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            }
        }
        #[cfg(test)]
        {
            s.worker_wakes += 1;
        }
    }
}
pub(crate) struct Ticket {
    owner: Weak<Readbacks>,
    id: u64,
}

/// User callbacks run outside locks and are isolated one at a time. On panic=abort
/// (including the usual wasm build), Rust panics cannot be caught: the host must
/// avoid panicking callbacks. This branch does not claim abort-mode recovery.
fn notify(done: Done, result: Result<Vec<u8>, Error>) {
    #[cfg(panic = "unwind")]
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result))).is_err() {
        log::error!("GPU readback callback panicked; remaining callbacks will still settle");
    }
    #[cfg(not(panic = "unwind"))]
    done(result);
}

/// Reject an optional-pixel callback through the same isolation boundary as mapped results.
pub(crate) fn reject(done: impl FnOnce(Option<Vec<u8>>) + wgpu::WasmNotSend + 'static, error: Error) {
    notify(Box::new(move |result| done(result.ok())), Err(error));
}

impl Readbacks {
    pub(crate) fn new() -> Result<Arc<Self>, Error> {
        let me = Arc::new(Self::default());
        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::clone(&me.0);
            std::thread::Builder::new()
                .name("gpu-readback-deadlines".into())
                .spawn(move || run_deadlines(&inner))
                .map_err(|e| format!("starting GPU readback deadline worker: {e}"))?;
        }
        Ok(me)
    }

    /// Have the deadline worker look at what is pending again (natively; the browser's timer
    /// re-arms itself).
    fn poke(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.0.wake.notify_one();
    }

    pub(crate) fn check(&self) -> Result<(), Error> {
        let s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match &s.retired {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    pub(crate) fn start(self: &Arc<Self>, done: impl FnOnce(Result<Vec<u8>, Error>) + wgpu::WasmNotSend + 'static) -> Result<Ticket, Error> {
        let mut done: Option<Done> = Some(Box::new(done));
        let result = {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(e) = &s.retired {
                Err(e.clone())
            } else if s.pending.len() >= 256 {
                Err("too many pending GPU readbacks".into())
            } else if let Some(id) = s.next.checked_add(1) {
                s.next = id;
                if let Some(done) = done.take() {
                    s.pending.insert(id, Entry { deadline: web_time::Instant::now() + TIMEOUT, done });
                }
                Ok(Ticket { owner: Arc::downgrade(self), id })
            } else {
                Err("GPU readback request id exhausted".into())
            }
        };
        if let Err(e) = &result
            && let Some(done) = done
        {
            notify(done, Err(e.clone()));
        }
        if result.is_ok() {
            // The worker may be asleep with nothing pending: this is its new earliest deadline.
            self.poke();
            #[cfg(target_arch = "wasm32")]
            self.arm_timer();
        }
        result
    }

    #[cfg(target_arch = "wasm32")]
    fn arm_timer(self: &Arc<Self>) {
        let delay = {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if s.timer_armed || s.retired.is_some() {
                return;
            }
            let Some(deadline) = s.pending.values().map(|e| e.deadline).min() else {
                return;
            };
            s.timer_armed = true;
            deadline.saturating_duration_since(web_time::Instant::now())
        };
        if let Err(e) = schedule(Arc::downgrade(self), delay) {
            self.retire(e);
        }
    }

    pub(crate) fn retire(&self, error: Error) {
        let pending = {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if s.retired.is_some() {
                return;
            }
            s.retired = Some(error.clone());
            std::mem::take(&mut s.pending)
        };
        self.poke();
        for (_, entry) in pending {
            notify(entry.done, Err(error.clone()));
        }
    }

    pub(crate) fn cancel_pending(&self, error: Error) {
        let pending = {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut s.pending)
        };
        self.poke();
        for (_, entry) in pending {
            notify(entry.done, Err(error.clone()));
        }
    }

    /// Retire the context when a deadline has passed (the browser's timer and tests; natively
    /// the deadline worker does this itself).
    #[cfg(any(test, target_arch = "wasm32"))]
    fn expire(&self, now: web_time::Instant) {
        let expired = {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            expire_locked(&mut s, now)
        };
        for entry in expired.into_iter().flatten() {
            notify(entry.done, Err(TIMED_OUT.into()));
        }
    }

    /// Move every pending deadline to `deadline` and wake the worker, as a test's stand-in for
    /// waiting out the production timeout.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn move_deadlines_for_test(&self, deadline: web_time::Instant) {
        {
            let mut s = self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for entry in s.pending.values_mut() {
                entry.deadline = deadline;
            }
        }
        self.poke();
    }
}
impl Drop for Readbacks {
    fn drop(&mut self) {
        self.retire("GPU readback context dropped".into());
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
            self.0.wake.notify_all();
        }
    }
}

pub(crate) struct Layout {
    pub row: u32,
    pub stride: u32,
    pub size: u64,
    pub len: usize,
}
impl Layout {
    pub(crate) fn new(w: u32, h: u32, bpp: u32, limit: u64) -> Result<Self, Error> {
        if w == 0 || h == 0 || bpp == 0 {
            return Err("empty GPU texture readback".into());
        }
        let row = w.checked_mul(bpp).ok_or("GPU readback row size overflow")?;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let stride = row.checked_add(alignment - 1).map(|n| n / alignment * alignment).ok_or("GPU readback row alignment overflow")?;
        let size = u64::from(stride) * u64::from(h);
        // Bound host allocations even when the adapter advertises much larger buffers.
        if size > limit.min(1 << 30) {
            return Err("GPU readback exceeds buffer budget".into());
        }
        let len = usize::try_from(u64::from(row) * u64::from(h)).map_err(|e| format!("GPU readback host size: {e}"))?;
        Ok(Self { row, stride, size, len })
    }
}

impl Ticket {
    pub(crate) fn active(&self) -> bool {
        self.owner.upgrade().is_some_and(|me| me.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending.contains_key(&self.id))
    }
    pub(crate) fn complete(self, result: Result<Vec<u8>, Error>) {
        let Some(me) = self.owner.upgrade() else { return };
        let entry = me.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).pending.remove(&self.id);
        if let Some(entry) = entry {
            // Its deadline is gone: the worker sleeps until the next one, or until something new.
            me.poke();
            notify(entry.done, result);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn schedule(me: Weak<Readbacks>, delay: Duration) -> Result<(), Error> {
    use wasm_bindgen::{JsCast, closure::Closure};
    let global = js_sys::global();
    let function = js_sys::Reflect::get(&global, &"setTimeout".into())
        .map_err(|e| format!("GPU deadline timer lookup: {e:?}"))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "GPU deadline timer unavailable".to_string())?;
    let callback = Closure::once(move || {
        if let Some(me) = me.upgrade() {
            me.0.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).timer_armed = false;
            me.expire(web_time::Instant::now());
            // If the earliest request completed, arm for the next live deadline.
            me.arm_timer();
        }
    });
    function
        .call2(&global, callback.as_ref(), &wasm_bindgen::JsValue::from_f64(delay.as_secs_f64() * 1000.0 + 1.0))
        .map_err(|e| format!("scheduling GPU readback deadline: {e:?}"))?;
    // Transfer the wrapper to JS GC rather than leaking a wrapper per readback.
    let _ = callback.into_js_value();
    Ok(())
}

/// A weak notification bridge. Shared-device hosts keep ownership of their handlers.
pub(crate) fn failure_notifier(me: &Arc<Readbacks>) -> impl Fn(String) + Send + Sync + 'static {
    #[cfg(not(target_arch = "wasm32"))]
    let fail = {
        let me = Arc::downgrade(me);
        move |error: String| {
            if let Some(me) = me.upgrade() {
                me.retire(error);
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let fail = {
        // GPU callbacks require Send even on wasm. Capture only the index; the registry
        // (which can contain JS callbacks) stays on this worker's event-loop thread.
        let index = REGISTRIES.with(|r| {
            let mut r = r.borrow_mut();
            r.push(Arc::downgrade(me));
            r.len() - 1
        });
        move |error: String| {
            let me = REGISTRIES.with(|r| r.borrow().get(index).and_then(Weak::upgrade));
            if let Some(me) = me {
                me.retire(error);
            }
        }
    };
    fail
}

/// Replacement handlers are installed only by the successful owned-device request path.
pub(crate) fn install(device: &wgpu::Device, me: &Arc<Readbacks>) {
    let fail = Arc::new(failure_notifier(me));
    let lost = fail.clone();
    device.set_device_lost_callback(move |reason, message| lost(format!("GPU device lost ({reason:?}): {message}")));
    device.on_uncaptured_error(Arc::new(move |e| {
        let message = format!("uncaptured GPU error: {e}");
        fail(message.clone());
        #[cfg(test)]
        #[allow(clippy::panic)]
        if matches!(e, wgpu::Error::Validation { .. }) {
            panic!("{message}");
        }
        log::error!("{message}");
    }));
}
#[cfg(target_arch = "wasm32")]
thread_local! {
    static REGISTRIES: std::cell::RefCell<Vec<Weak<Readbacks>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Validate every row before publishing tightly packed bytes.
pub(crate) fn packed(view: &[u8], row: usize, stride: usize, height: usize) -> Result<Vec<u8>, Error> {
    if row == 0 || height == 0 || stride < row {
        return Err("invalid GPU readback layout".into());
    }
    let size = row.checked_mul(height).ok_or("GPU readback output size overflow")?;
    let end = (height - 1).checked_mul(stride).and_then(|s| s.checked_add(row)).ok_or("GPU readback mapped size overflow")?;
    if view.len() < end {
        return Err(format!("short GPU readback: {} bytes, needs {end}", view.len()));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(size).map_err(|e| format!("allocating GPU readback output: {e}"))?;
    for y in 0..height {
        let start = y.checked_mul(stride).ok_or("GPU readback row overflow")?;
        let bytes = view.get(start..start + row).ok_or("short GPU readback row")?;
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn early_rejection_reports_no_pixels_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        reject(
            move |pixels| {
                assert!(pixels.is_none());
                observed.fetch_add(1, Ordering::SeqCst);
            },
            "synthetic early rejection".into(),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), panic = "unwind"))]
    fn early_rejection_contains_callback_panic_and_allows_later_completion() {
        let calls = Arc::new(AtomicUsize::new(0));
        let first = calls.clone();
        reject(
            move |pixels| {
                assert!(pixels.is_none());
                first.fetch_add(1, Ordering::SeqCst);
                panic!("synthetic rejected callback panic");
            },
            "synthetic retired context".into(),
        );
        let second = calls.clone();
        reject(
            move |pixels| {
                assert!(pixels.is_none());
                second.fetch_add(1, Ordering::SeqCst);
            },
            "synthetic invalid layout".into(),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), panic = "unwind"))]
    fn panicking_callback_does_not_strand_waiters_or_kill_deadline_worker() {
        use std::future::Future;
        use std::task::{Context, Poll, Wake, Waker};
        struct WakeCount(AtomicUsize);
        impl Wake for WakeCount {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let me = Readbacks::new().unwrap();
        let deferred = Arc::new(crate::deferred::Deferred::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let _first = me.start(|_| panic!("synthetic public callback panic")).unwrap();
        for key in 1..=2 {
            let done = deferred.start(key, 1, 1, [0.0; 2], 1.0);
            let tx = tx.clone();
            let _ticket = me
                .start(move |r| {
                    assert!(r.is_err());
                    done(None);
                    tx.send(()).unwrap();
                })
                .unwrap();
        }
        let wakes = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut settled = std::pin::pin!(deferred.settled());
        assert!(settled.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        me.cancel_pending("synthetic cancellation".into());
        assert_eq!(deferred.in_flight(), 0);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        assert!(matches!(settled.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(())));
        assert!(me.0.state.lock().unwrap().pending.is_empty());
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();

        // Drive an actual later watchdog expiry without waiting for the production 10s.
        let _first = me.start(|_| panic!("synthetic watchdog callback panic")).unwrap();
        let tx2 = tx.clone();
        let _second = me
            .start(move |r| {
                assert!(r.is_err());
                tx2.send(()).unwrap();
            })
            .unwrap();
        me.move_deadlines_for_test(web_time::Instant::now());
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(me.0.state.lock().unwrap().pending.is_empty());
        assert!(me.check().is_err());
    }

    /// With nothing pending the deadline worker sleeps: no periodic wakeups (it used to poll
    /// every 100 ms for the life of the app, ten wakeups a second while idle).
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn an_idle_deadline_worker_does_not_wake_up() {
        let me = Readbacks::new().unwrap();
        std::thread::sleep(Duration::from_millis(350));
        let wakes = me.0.state.lock().unwrap().worker_wakes;
        assert!(wakes <= 1, "the worker woke {wakes} times while nothing was pending");
        // A readback that completes in time: the worker is told, and goes back to sleep.
        let ticket = me.start(|r| assert!(r.is_ok())).unwrap();
        ticket.complete(Ok(vec![1]));
        std::thread::sleep(Duration::from_millis(350));
        let wakes = me.0.state.lock().unwrap().worker_wakes;
        assert!(wakes <= 3, "the worker woke {wakes} times for one completed readback");
        assert!(me.check().is_ok());
    }

    /// A deadline that passes wakes the worker exactly then, without the old polling loop.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_pending_deadline_wakes_the_worker_when_it_is_due() {
        let me = Readbacks::new().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ticket = me
            .start(move |r| {
                assert!(r.is_err());
                tx.send(web_time::Instant::now()).unwrap();
            })
            .unwrap();
        let due = web_time::Instant::now() + Duration::from_millis(200);
        me.move_deadlines_for_test(due);
        let fired = rx.recv_timeout(Duration::from_secs(5)).expect("the deadline fires");
        assert!(fired >= due, "fired {:?} early", due - fired);
        assert!(fired < due + Duration::from_secs(2), "fired {:?} late", fired - due);
        assert!(me.check().is_err());
    }

    /// Dropping the last handle lets the worker exit: it holds the state, not the context.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn dropping_the_context_stops_the_deadline_worker() {
        let me = Readbacks::new().unwrap();
        let inner = Arc::downgrade(&me.0);
        drop(me);
        let t0 = web_time::Instant::now();
        while inner.upgrade().is_some() {
            assert!(t0.elapsed() < Duration::from_secs(5), "the worker is still holding the state");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn missing_callback_times_out_once_and_rejects_late_success() {
        let me = Arc::new(Readbacks::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ticket = me
            .start(move |r| {
                assert!(r.is_err());
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        me.expire(web_time::Instant::now() + TIMEOUT + Duration::from_secs(1));
        assert!(!ticket.active());
        ticket.complete(Ok(vec![42]));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(me.check().is_err());
    }
    #[test]
    fn cancellation_drains_all_and_new_context_is_independent() {
        let old = Arc::new(Readbacks::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut tickets = Vec::new();
        for _ in 0..3 {
            let c = calls.clone();
            tickets.push(
                old.start(move |r| {
                    assert!(r.is_err());
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap(),
            );
        }
        old.retire("synthetic device failure".into());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let new = Arc::new(Readbacks::default());
        let c = calls.clone();
        let fresh = new
            .start(move |r| {
                assert_eq!(r.unwrap(), vec![7]);
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        for t in tickets {
            t.complete(Ok(vec![99]));
        }
        assert!(fresh.active());
        fresh.complete(Ok(vec![7]));
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }
    #[test]
    fn cancelled_ticket_cannot_complete_a_new_request_on_same_context() {
        let me = Arc::new(Readbacks::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let old = me
            .start(move |r| {
                assert!(r.is_err());
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        me.cancel_pending("cancelled".into());
        let c = calls.clone();
        let new = me
            .start(move |r| {
                assert_eq!(r.unwrap(), vec![7]);
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        old.complete(Ok(vec![99]));
        assert!(new.active());
        new.complete(Ok(vec![7]));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn completion_and_failure_race_invokes_callback_once() {
        for _ in 0..32 {
            let me = Arc::new(Readbacks::default());
            let calls = Arc::new(AtomicUsize::new(0));
            let c = calls.clone();
            let ticket = me
                .start(move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let b = barrier.clone();
            let t = std::thread::spawn(move || {
                b.wait();
                ticket.complete(Ok(vec![1]));
            });
            barrier.wait();
            me.retire("concurrent device failure".into());
            t.join().unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn malformed_layouts_fail_without_allocating() {
        assert!(Layout::new(u32::MAX, 2, 16, u64::MAX).is_err());
        assert!(Layout::new(1, 0, 4, u64::MAX).is_err());
        assert!(Layout::new(8192, 8192, 16, 128).is_err());
    }
    #[test]
    fn completed_request_survives_later_failure_without_second_callback() {
        let me = Arc::new(Readbacks::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let ticket = me
            .start(move |r| {
                assert_eq!(r.unwrap(), vec![1]);
                c.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        ticket.complete(Ok(vec![1]));
        me.retire("later failure".into());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn no_partial_rows() {
        assert!(packed(&[0; 7], 4, 4, 2).is_err());
        assert_eq!(packed(&[1, 2, 0, 0, 3, 4], 2, 4, 2).unwrap(), vec![1, 2, 3, 4]);
    }
}
