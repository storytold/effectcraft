//! Pure helpers of the browser's remote storage: request time limits, what a directory listing
//! shows while it retries, status line texts, and a bounded-concurrency runner.
//!
//! No browser types, so it runs natively in `tests/it`.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

/// Time limit of a request that moves no file data (PROPFIND, HEAD, MOVE, DELETE).
pub const META_TIMEOUT: Duration = Duration::from_secs(20);
/// Time limit of a whole file transfer (GET, PUT), body included.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);
/// Time limit of the config file read at startup (a stuck server must not hold the app back).
pub const CONFIG_TIMEOUT: Duration = Duration::from_secs(8);
/// Media files of a project fetched at the same time.
pub const MEDIA_CONCURRENCY: usize = 4;
/// A retried listing keeps showing its last error for this long (seconds) before it says
/// "Loading…": a retry that fails at once would otherwise flash between the two.
pub const LOADING_GRACE_S: f64 = 1.0;

/// How long a request with `method` may take.
pub fn timeout_for(method: &str) -> Duration {
    match method {
        "GET" | "PUT" => TRANSFER_TIMEOUT,
        _ => META_TIMEOUT,
    }
}

/// The error of a request that ran out of time.
pub fn timeout_message(method: &str, limit: Duration) -> String {
    format!("the server did not finish the {method} request within {} s", limit.as_secs())
}

/// What the Media Browser shows for a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingView {
    Entries,
    Error,
    Loading,
}

/// `loading_for`: how long (seconds) the request in flight has been running, `None` when none is.
/// The last error stays up until a retry has been running for [`LOADING_GRACE_S`].
pub fn listing_view(has_entries: bool, has_error: bool, loading_for: Option<f64>) -> ListingView {
    if has_entries {
        ListingView::Entries
    } else if has_error && loading_for.is_none_or(|t| t < LOADING_GRACE_S) {
        ListingView::Error
    } else {
        ListingView::Loading
    }
}

/// The last path segment.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

/// Status line while a remote project opens.
pub fn opening_status(path: &str) -> String {
    format!("Opening {}…", file_name(path))
}

/// Status line once a remote project is open.
pub fn opened_status(path: &str) -> String {
    format!("Opened {}", file_name(path))
}

/// Status line while a project's media is fetched (`done` of `total`).
pub fn media_progress_status(path: &str, done: usize, total: usize) -> String {
    format!("Opening {}: media {done}/{total}…", file_name(path))
}

/// Status line naming (up to three of) the media files that could not be fetched.
pub fn media_failed_status(failed: &[String]) -> String {
    format!("{} media file(s) could not be loaded: {}", failed.len(), failed.iter().take(3).cloned().collect::<Vec<_>>().join("; "))
}

pub const RESTORE_OFFLINE_STATUS: &str = "Remote media offline; reopen the project";

/// Run `f` over `items` with at most `limit` futures in flight; the results come back in the
/// order of `items`. One task, no spawning: the futures are polled together.
pub async fn run_bounded<I, F, Fut, T>(items: I, limit: usize, mut f: F) -> Vec<T>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = T>,
{
    let limit = limit.max(1);
    let mut iter = items.into_iter().enumerate();
    let mut running: Vec<(usize, Pin<Box<Fut>>)> = Vec::new();
    let mut done: Vec<(usize, T)> = Vec::new();
    std::future::poll_fn(|cx| {
        loop {
            while running.len() < limit {
                let Some((i, item)) = iter.next() else { break };
                running.push((i, Box::pin(f(item))));
            }
            if running.is_empty() {
                return Poll::Ready(());
            }
            let mut progressed = false;
            let mut k = 0;
            while k < running.len() {
                match running[k].1.as_mut().poll(cx) {
                    Poll::Ready(v) => {
                        let (i, _) = running.swap_remove(k);
                        done.push((i, v));
                        progressed = true;
                    }
                    Poll::Pending => k += 1,
                }
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    })
    .await;
    done.sort_by_key(|d| d.0);
    done.into_iter().map(|d| d.1).collect()
}
