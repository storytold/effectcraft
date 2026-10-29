//! Time limits, listing view, status texts and the bounded runner of remote storage.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::remote_util::{
    LOADING_GRACE_S, ListingView, META_TIMEOUT, TRANSFER_TIMEOUT, file_name, listing_view, media_failed_status, media_progress_status, opened_status,
    opening_status, run_bounded, timeout_for, timeout_message,
};

/// Limits follow the method.
#[test]
fn limits_follow_the_method() {
    for m in ["PROPFIND", "HEAD", "MOVE", "DELETE"] {
        assert_eq!(timeout_for(m), META_TIMEOUT, "{m}");
    }
    for m in ["GET", "PUT"] {
        assert_eq!(timeout_for(m), TRANSFER_TIMEOUT, "{m}");
    }
    assert!(META_TIMEOUT < TRANSFER_TIMEOUT);
    let msg = timeout_message("PROPFIND", Duration::from_secs(20));
    assert!(msg.contains("PROPFIND") && msg.contains("20 s"), "{msg}");
}

/// A listing keeps its error until the retry runs a while.
#[test]
fn a_listing_keeps_its_error_until_the_retry_runs_a_while() {
    use ListingView::*;
    assert_eq!(listing_view(true, true, Some(30.0)), Entries);
    assert_eq!(listing_view(false, true, None), Error);
    assert_eq!(listing_view(false, true, Some(0.0)), Error);
    assert_eq!(listing_view(false, true, Some(LOADING_GRACE_S + 0.1)), Loading);
    assert_eq!(listing_view(false, false, None), Loading);
    assert_eq!(listing_view(false, false, Some(0.0)), Loading);
}

/// Status texts name the file.
#[test]
fn status_texts_name_the_file() {
    assert_eq!(file_name("/Work/a/b.ecproj"), "b.ecproj");
    assert_eq!(file_name("b.ecproj"), "b.ecproj");
    assert_eq!(opening_status("/Work/b.ecproj"), "Opening b.ecproj…");
    assert_eq!(opened_status("/Work/b.ecproj"), "Opened b.ecproj");
    assert_eq!(media_progress_status("/Work/b.ecproj", 3, 12), "Opening b.ecproj: media 3/12…");
    let failed: Vec<String> = (1..=5).map(|i| format!("e{i}")).collect();
    assert_eq!(media_failed_status(&failed), "5 media file(s) could not be loaded: e1; e2; e3");
}

/// Pending for `n` polls, then ready; counts how many are alive at once.
struct Slow<'a> {
    left: u32,
    id: usize,
    live: &'a Cell<usize>,
    peak: &'a Cell<usize>,
    started: bool,
}

impl Future for Slow<'_> {
    type Output = usize;
    /// Pending for a few polls, then ready.
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<usize> {
        if !self.started {
            self.started = true;
            self.live.set(self.live.get() + 1);
            self.peak.set(self.peak.get().max(self.live.get()));
        }
        if self.left == 0 {
            self.live.set(self.live.get() - 1);
            return Poll::Ready(self.id);
        }
        self.left -= 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Run a future to completion with a waker that does nothing.
fn block_on<T>(f: impl Future<Output = T>) -> T {
    let mut f = Box::pin(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

/// Bounded runner limits concurrency and keeps order.
#[test]
fn bounded_runner_limits_concurrency_and_keeps_order() {
    let (live, peak) = (Cell::new(0), Cell::new(0));
    // item i takes (7 - i % 5) polls, so they finish out of order
    let out = block_on(run_bounded(0..20usize, 4, |i| Slow { left: 7 - (i % 5) as u32, id: i, live: &live, peak: &peak, started: false }));
    assert_eq!(out, (0..20).collect::<Vec<_>>());
    assert_eq!(peak.get(), 4);
    assert_eq!(live.get(), 0);
    // a limit of 0 still makes progress, one at a time
    peak.set(0);
    let out = block_on(run_bounded(0..3usize, 0, |i| Slow { left: 2, id: i, live: &live, peak: &peak, started: false }));
    assert_eq!((out, peak.get()), (vec![0, 1, 2], 1));
    assert!(block_on(run_bounded(Vec::<u8>::new(), 4, |_| async { 1 })).is_empty());
}

/// A failed item does not stop the others.
#[test]
fn a_failed_item_does_not_stop_the_others() {
    let seen = RefCell::new(vec![]);
    let out: Vec<Result<u32, String>> = block_on(run_bounded(0..6u32, 3, |i| {
        let seen = &seen;
        async move {
            seen.borrow_mut().push(i);
            if i % 2 == 1 { Err(format!("offline {i}")) } else { Ok(i) }
        }
    }));
    assert_eq!(out.iter().filter(|r| r.is_err()).count(), 3);
    assert_eq!(out[4], Ok(4));
    assert_eq!(seen.borrow().len(), 6);
}
