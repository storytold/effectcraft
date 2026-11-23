//! Movie frame jobs wait outside rayon until the preceding frame of that file finishes.
//!
//! A movie decoder holds its state lock while decoding slices in parallel. If a waiting
//! codec worker steals another frame job for that decoder, a blocking lock would re-enter
//! the same decoder and deadlock. Queuing that job here leaves codec workers free for the
//! slices, while other movies can still decode concurrently.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
pub(super) struct MovieJobs {
    /// An entry has one runnable job, plus these waiting jobs. Idle entries are removed.
    pending: Mutex<HashMap<Arc<str>, VecDeque<Job>>>,
}

impl MovieJobs {
    pub(super) fn spawn(self: &Arc<Self>, threads: &'static rayon::ThreadPool, path: Arc<str>, job: impl FnOnce() + Send + 'static) {
        let job = Box::new(job);
        {
            let mut pending = super::lock(&self.pending);
            if let Some(waiting) = pending.get_mut(&path) {
                waiting.push_back(job);
                return;
            }
            pending.insert(path.clone(), VecDeque::new());
        }
        self.launch(threads, path, job);
    }

    fn launch(self: &Arc<Self>, threads: &'static rayon::ThreadPool, path: Arc<str>, job: Job) {
        let advance = Advance { jobs: self.clone(), threads, path };
        threads.spawn(move || {
            // Also advances when a decoder panics or the requester has gone away.
            let _advance = advance;
            job();
        });
    }

    #[cfg(test)]
    pub(super) fn queued(&self, path: &str) -> usize {
        super::lock(&self.pending).get(path).map_or(0, VecDeque::len)
    }
}

struct Advance {
    jobs: Arc<MovieJobs>,
    threads: &'static rayon::ThreadPool,
    path: Arc<str>,
}

impl Drop for Advance {
    fn drop(&mut self) {
        let next = {
            let mut pending = super::lock(&self.jobs.pending);
            let next = pending.get_mut(&self.path).and_then(VecDeque::pop_front);
            if next.is_none() {
                pending.remove(&self.path);
            }
            next
        };
        if let Some(next) = next {
            self.jobs.launch(self.threads, self.path.clone(), next);
        }
    }
}
