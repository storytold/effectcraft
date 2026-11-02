//! Deferred readbacks: rendering on a device whose readbacks cannot be waited for (WebGPU in a
//! browser worker: a buffer maps only once the worker returns to its event loop).
//!
//! A frame renders in passes (see [`effectcraft_render::Accelerator::frame_begin`]). Every
//! readback the renderer needs is keyed by what determines its pixels: a GPU effect chain by
//! its input buffer's pixels and the chain's parameters, the top-level frame by comp, time and
//! render options. A key seen for the first time records the GPU work, starts an asynchronous
//! copy (`map_async`) and *misses*: the step returns a placeholder and the pass is marked missed
//! (raising the gate that keeps placeholder-derived buffers out of the layer cache). The caller
//! awaits [`Deferred::settled`] and renders again; keys whose bytes arrived are served from
//! here. A pass without a miss is the finished frame.
//!
//! Correctness: a result is only computed from genuine inputs. Pixels a chain reads are part
//! of its key, and a chain whose kernels read other data through the effect host (other layers,
//! other times) is not started when that data missed while it ran (the miss counter moved).

use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use effectcraft_effects::Buf;
use effectcraft_render::FxStep;

/// Bytes read back for one key, with the geometry they describe.
#[derive(Clone, Debug)]
pub(crate) struct Readback {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Layer buffer offset and scale (chains).
    pub offset: [f64; 2],
    pub scale: f64,
}

/// What [`Deferred::lookup`] knows about a key.
pub(crate) enum Lookup {
    /// The bytes arrived (`None`: the readback failed; render this step another way).
    Ready(Option<Readback>),
    /// Started in this frame, not arrived yet.
    InFlight,
    /// Never asked for in this frame.
    Absent,
}

#[derive(Default)]
struct State {
    ready: HashMap<u128, Option<Readback>>,
    /// Each request owns a unique token, even when a key is reused after cancellation.
    waiting: HashMap<u128, Arc<()>>,
    /// Misses so far in this frame (a counter, so a chain sees misses during its own work).
    misses: u64,
    wakers: Vec<Waker>,
}

/// Deferred readback state of a device (see the module docs).
#[derive(Default)]
pub struct Deferred {
    st: Mutex<State>,
    /// Raised from the pass's first miss to its end.
    gate: Arc<AtomicBool>,
    /// Passes and readbacks (diagnostics).
    pub(crate) passes: std::sync::atomic::AtomicU64,
    pub(crate) readbacks: std::sync::atomic::AtomicU64,
}

impl Deferred {
    pub fn frame_begin(&self) {
        let wakers = {
            let mut s = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            s.ready.clear();
            s.waiting.clear();
            s.misses = 0;
            std::mem::take(&mut s.wakers)
        };
        for w in wakers {
            w.wake();
        }
        self.gate.store(false, Ordering::Relaxed);
    }

    pub fn pass_begin(&self) {
        self.gate.store(false, Ordering::Relaxed);
        self.passes.fetch_add(1, Ordering::Relaxed);
    }

    pub fn missed(&self) -> bool {
        self.gate.load(Ordering::Relaxed)
    }

    pub fn gate(&self) -> Arc<AtomicBool> {
        self.gate.clone()
    }

    /// Misses so far in this frame.
    pub(crate) fn misses(&self) -> u64 {
        self.st.lock().map(|s| s.misses).unwrap_or(0)
    }

    /// Mark the pass as missed.
    pub(crate) fn miss(&self) {
        if let Ok(mut s) = self.st.lock() {
            s.misses += 1;
        }
        self.gate.store(true, Ordering::Relaxed);
    }

    pub(crate) fn lookup(&self, key: u128) -> Lookup {
        let Ok(s) = self.st.lock() else { return Lookup::Ready(None) };
        if let Some(r) = s.ready.get(&key) {
            return Lookup::Ready(r.clone());
        }
        if s.waiting.contains_key(&key) { Lookup::InFlight } else { Lookup::Absent }
    }

    /// A readback for `key` is being started; returns the callback that stores its bytes.
    pub(crate) fn start(self: &Arc<Self>, key: u128, width: u32, height: u32, offset: [f64; 2], scale: f64) -> impl FnOnce(Option<Vec<u8>>) + 'static {
        let token = Arc::new(());
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).waiting.insert(key, token.clone());
        self.readbacks.fetch_add(1, Ordering::Relaxed);
        let me = self.clone();
        move |bytes: Option<Vec<u8>>| {
            let wakers = {
                let mut s = me.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                // Reject stale completion before touching either results or accounting.
                if !s.waiting.get(&key).is_some_and(|current| Arc::ptr_eq(current, &token)) {
                    return;
                }
                s.waiting.remove(&key);
                s.ready.insert(key, bytes.map(|bytes| Readback { bytes, width, height, offset, scale }));
                if s.waiting.is_empty() { std::mem::take(&mut s.wakers) } else { vec![] }
            };
            for w in wakers {
                w.wake();
            }
        }
    }

    /// Cancel pending requests without exposing placeholder pixels as successful results.
    /// Existing waiters wake; late callbacks cannot change a newer request's accounting.
    pub(crate) fn cancel(&self) {
        let wakers = {
            let mut s = self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (key, _) in std::mem::take(&mut s.waiting) {
                s.ready.insert(key, None);
            }
            std::mem::take(&mut s.wakers)
        };
        for w in wakers {
            w.wake();
        }
    }

    /// Readbacks still in flight.
    pub fn in_flight(&self) -> usize {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner).waiting.len()
    }

    /// Resolves once no readback is in flight (on the web the browser's event loop delivers
    /// them; natively a device poll does).
    pub fn settled(self: &Arc<Self>) -> Settled {
        Settled(self.clone())
    }
}

/// See [`Deferred::settled`].
pub struct Settled(Arc<Deferred>);

impl Future for Settled {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut s = self.0.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if s.waiting.is_empty() {
            return Poll::Ready(());
        }
        if !s.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            s.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// 128-bit key from two differently seeded std hashers (deterministic within a process).
pub(crate) struct Key(std::collections::hash_map::DefaultHasher, std::collections::hash_map::DefaultHasher);

impl Key {
    pub fn new(tag: u64) -> Key {
        let mut a = std::collections::hash_map::DefaultHasher::new();
        let mut b = std::collections::hash_map::DefaultHasher::new();
        tag.hash(&mut a);
        (tag ^ 0x9e37_79b9_7f4a_7c15).hash(&mut b);
        0xa5u8.hash(&mut b);
        Key(a, b)
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.0.write(v);
        self.1.write(v);
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub fn debug(&mut self, v: &impl std::fmt::Debug) {
        self.bytes(format!("{v:?}").as_bytes());
        self.bytes(&[0xff]);
    }

    pub fn finish(&self) -> u128 {
        ((self.0.finish() as u128) << 64) | self.1.finish() as u128
    }
}

/// Key of a GPU effect chain on `buf`: the input pixels and geometry, the bit depth, and every
/// step's effect, parameters (sorted) and context.
pub(crate) fn chain_key(chain: &[FxStep], buf: &Buf, levels: Option<f32>) -> u128 {
    let mut k = Key::new(1);
    k.u64(buf.img.width as u64);
    k.u64(buf.img.height as u64);
    k.bytes(bytemuck::cast_slice(&buf.img.data));
    k.f64(buf.offset[0]);
    k.f64(buf.offset[1]);
    k.f64(buf.scale);
    k.debug(&levels);
    for s in chain {
        let c = &s.ctx;
        k.bytes(s.spec.id.as_bytes());
        let mut names: Vec<&String> = c.params.values.keys().collect();
        names.sort();
        for n in names {
            k.bytes(n.as_bytes());
            k.debug(&c.params.values[n]);
        }
        k.f64(c.time);
        k.f64(c.layer_size[0]);
        k.f64(c.layer_size[1]);
        k.u64(c.seed as u64);
        k.u64(c.adjustment as u64);
        let e = &c.env;
        k.debug(&e.masks);
        k.u64(e.host.is_some() as u64);
        k.f64(e.comp_time);
        k.f64(e.frame_rate);
        k.u64(e.effect_index as u64);
        k.debug(&e.working_space);
        k.u64(e.working_linear as u64);
        k.u64(e.float_depth as u64);
        k.debug(&e.shutter);
    }
    k.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WakeCount(std::sync::atomic::AtomicUsize);
    impl std::task::Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn cancellation_wakes_waiters_and_stale_same_key_does_not_drain_new_work() {
        let d = Arc::new(Deferred::default());
        let old = d.start(7, 1, 1, [0.0; 2], 1.0);
        let wake = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut settled = std::pin::pin!(d.settled());
        for _ in 0..4 {
            assert!(settled.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        }
        d.cancel();
        assert_eq!(wake.0.load(Ordering::SeqCst), 1);
        assert_eq!(d.in_flight(), 0);
        assert!(settled.as_mut().poll(&mut Context::from_waker(&waker)).is_ready());
        assert!(matches!(d.lookup(7), Lookup::Ready(None)));
        d.frame_begin();
        let new = d.start(7, 1, 1, [0.0; 2], 1.0);
        old(Some(vec![99; 4]));
        assert_eq!(d.in_flight(), 1);
        assert!(matches!(d.lookup(7), Lookup::InFlight));
        new(None);
        assert_eq!(d.in_flight(), 0);
        assert!(matches!(d.lookup(7), Lookup::Ready(None)));
    }

    #[test]
    fn device_failure_without_map_callback_drains_and_wakes_deferred() {
        let d = Arc::new(Deferred::default());
        let registry = crate::readback::Readbacks::new().unwrap();
        let done = d.start(11, 1, 1, [0.0; 2], 1.0);
        let ticket = registry.start(move |r| done(r.ok())).unwrap();
        let wake = Arc::new(WakeCount(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut settled = std::pin::pin!(d.settled());
        assert!(settled.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        registry.retire("synthetic device loss".into());
        assert_eq!(wake.0.load(Ordering::SeqCst), 1);
        assert!(settled.as_mut().poll(&mut Context::from_waker(&waker)).is_ready());
        assert_eq!(d.in_flight(), 0);
        ticket.complete(Ok(vec![99; 4]));
        assert!(matches!(d.lookup(11), Lookup::Ready(None)));
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        let mut a = Key::new(1);
        a.bytes(b"abc");
        let mut b = Key::new(1);
        b.bytes(b"abc");
        let mut c = Key::new(2);
        c.bytes(b"abc");
        assert_eq!(a.finish(), b.finish());
        assert_ne!(a.finish(), c.finish());
    }

    #[test]
    fn readbacks_resolve_and_old_frames_are_dropped() {
        let d = Arc::new(Deferred::default());
        d.frame_begin();
        d.pass_begin();
        assert!(matches!(d.lookup(7), Lookup::Absent));
        let done = d.start(7, 2, 1, [1.0, 2.0], 0.5);
        d.miss();
        assert!(d.missed() && d.gate().load(Ordering::Relaxed));
        assert!(matches!(d.lookup(7), Lookup::InFlight));
        assert_eq!(d.in_flight(), 1);
        // The settle future wakes when the last readback lands.
        let mut fut = std::pin::pin!(d.settled());
        let waker = Waker::noop();
        assert!(fut.as_mut().poll(&mut Context::from_waker(waker)).is_pending());
        done(Some(vec![1, 2, 3, 4, 5, 6, 7, 8]));
        assert!(fut.as_mut().poll(&mut Context::from_waker(waker)).is_ready());
        let Lookup::Ready(Some(r)) = d.lookup(7) else { panic!("not ready") };
        assert_eq!((r.width, r.height, r.offset, r.scale, r.bytes.len()), (2, 1, [1.0, 2.0], 0.5, 8));
        d.pass_begin();
        assert!(!d.missed());
        // A readback of the previous frame arriving late is dropped.
        let late = d.start(9, 1, 1, [0.0; 2], 1.0);
        d.frame_begin();
        late(Some(vec![0; 4]));
        assert!(matches!(d.lookup(9), Lookup::Absent));
        assert!(matches!(d.lookup(7), Lookup::Absent));
        assert_eq!(d.in_flight(), 0);
    }
}
