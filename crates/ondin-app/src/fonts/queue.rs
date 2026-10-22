//! The font pool's work queue: three lanes, one of which is a stack.
//!
//! A face the user picked is a promise, so it goes in a FIFO lane served first. A
//! preview row's face is a guess that may have scrolled off by the time a worker
//! frees up, so that lane is LIFO and bounded, dropping its oldest end. The third
//! lane is the popular set warmed at startup (§15 D352): it waits for both others,
//! FIFO because the list order is editorial, uncapped because it is pushed once.
//!
//! The bound discards work, so [`Queue::push_preview`] hands back what it pushed out
//! (§15 D354): a dropped job never reports, and its claim would be held for ever.

use super::FaceSpec;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// Preview faces allowed to wait for a worker: about two screenfuls, so a flung
/// scrollbar cannot bank thousands of stale downloads.
pub(super) const PREVIEW_BACKLOG: usize = 48;

/// One face to fetch, for one family.
pub(super) struct Job {
    pub family: String,
    pub face: FaceSpec,
}

#[derive(Default)]
pub(super) struct Queue {
    lanes: Mutex<Lanes>,
    ready: Condvar,
}

#[derive(Default)]
struct Lanes {
    /// Faces something is waiting for. Served first, oldest first.
    urgent: VecDeque<Job>,
    /// Faces wanted only to draw a preview row. Served newest first.
    preview: VecDeque<Job>,
    /// Faces nothing has asked for yet (the popular set). Served last, oldest first.
    prefetch: VecDeque<Job>,
    stop: bool,
}

impl Queue {
    pub fn push_urgent(&self, job: Job) {
        if let Ok(mut lanes) = self.lanes.lock() {
            lanes.urgent.push_back(job);
            self.ready.notify_one();
        }
    }

    /// Queue a preview face, returning whatever the backlog bound pushed out.
    ///
    /// The caller must release what comes back (§15 D354): a dropped job leaves
    /// `FontService::claimed` holding its key and the family's `outstanding` count
    /// above zero, so the family reads as loading for ever and `ensure` can never
    /// request it again.
    pub fn push_preview(&self, job: Job) -> Option<Job> {
        let mut dropped = None;
        if let Ok(mut lanes) = self.lanes.lock() {
            lanes.preview.push_back(job);
            while lanes.preview.len() > PREVIEW_BACKLOG {
                dropped = lanes.preview.pop_front();
            }
            self.ready.notify_one();
        }
        dropped
    }

    /// A face nothing has asked for: the popular set, warmed at startup (§15 D352).
    ///
    /// A lane of its own: `urgent` would jump a face the user waits on, and `preview`
    /// is LIFO (reversing the editorial order) and capped at [`PREVIEW_BACKLOG`], so
    /// the first scroll would evict the set. Nothing else may enqueue here.
    pub fn push_prefetch(&self, job: Job) {
        if let Ok(mut lanes) = self.lanes.lock() {
            lanes.prefetch.push_back(job);
            self.ready.notify_one();
        }
    }

    /// Block until there is a job, or `None` once the service is dropped.
    pub fn pop(&self) -> Option<Job> {
        let mut lanes = self.lanes.lock().ok()?;
        loop {
            if lanes.stop {
                return None;
            }
            // Strictly: asked for, then on-screen speculation, then the prefetch.
            if let Some(job) = lanes
                .urgent
                .pop_front()
                .or_else(|| lanes.preview.pop_back())
                .or_else(|| lanes.prefetch.pop_front())
            {
                return Some(job);
            }
            lanes = self.ready.wait(lanes).ok()?;
        }
    }

    pub fn stop(&self) {
        if let Ok(mut lanes) = self.lanes.lock() {
            lanes.stop = true;
            lanes.urgent.clear();
            lanes.preview.clear();
            lanes.prefetch.clear();
        }
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::FaceSource;
    use std::path::PathBuf;

    fn job(name: &str) -> Job {
        Job {
            family: name.to_string(),
            face: FaceSpec {
                key: name.to_string(),
                source: FaceSource::File(PathBuf::from(name)),
                variable: false,
            },
        }
    }

    fn drain(queue: &Queue) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(job) = {
            let empty = queue
                .lanes
                .lock()
                .map(|l| l.urgent.is_empty() && l.preview.is_empty() && l.prefetch.is_empty())
                .unwrap_or(true);
            if empty { None } else { queue.pop() }
        } {
            out.push(job.family);
        }
        out
    }

    /// A picked family must not wait behind a scroll's worth of speculation.
    #[test]
    fn urgent_faces_come_before_previews_however_late_they_arrive() {
        let queue = Queue::default();
        queue.push_preview(job("preview-1"));
        queue.push_preview(job("preview-2"));
        queue.push_urgent(job("picked"));
        assert_eq!(drain(&queue)[0], "picked");
    }

    /// A flung scrollbar must not serve the on-screen rows after everything scrolled past.
    #[test]
    fn the_newest_preview_is_served_first() {
        let queue = Queue::default();
        for i in 0..4 {
            queue.push_preview(job(&format!("row-{i}")));
        }
        assert_eq!(drain(&queue), ["row-3", "row-2", "row-1", "row-0"]);
    }

    #[test]
    fn the_preview_backlog_is_bounded_and_drops_its_oldest() {
        let queue = Queue::default();
        for i in 0..PREVIEW_BACKLOG + 10 {
            queue.push_preview(job(&format!("row-{i}")));
        }
        let served = drain(&queue);
        assert_eq!(served.len(), PREVIEW_BACKLOG);
        assert_eq!(served[0], format!("row-{}", PREVIEW_BACKLOG + 9));
        assert!(
            !served.contains(&"row-0".to_string()),
            "the oldest requests are the ones dropped"
        );
    }

    /// A prefetch waits for both other lanes and keeps its listed order (§15 D352).
    ///
    /// flip: route `push_prefetch` to `push_preview` → fails at the order assertion
    /// (the preview lane is LIFO, so `p-2` comes first).
    #[test]
    fn a_prefetch_is_served_last_and_in_the_order_it_was_listed() {
        let queue = Queue::default();
        for i in 0..3 {
            queue.push_prefetch(job(&format!("p-{i}")));
        }
        queue.push_preview(job("on-screen"));
        queue.push_urgent(job("picked"));
        assert_eq!(drain(&queue), ["picked", "on-screen", "p-0", "p-1", "p-2"]);
    }

    /// A scroll cannot throw the prefetch away, as the capped preview lane would.
    ///
    /// flip: route `push_prefetch` to `push_preview` → fails at the count (survivors
    /// drop to `PREVIEW_BACKLOG`).
    #[test]
    fn a_burst_of_previews_does_not_evict_the_prefetch() {
        let queue = Queue::default();
        for i in 0..50 {
            queue.push_prefetch(job(&format!("p-{i}")));
        }
        for i in 0..PREVIEW_BACKLOG * 2 {
            queue.push_preview(job(&format!("scroll-{i}")));
        }
        let served = drain(&queue);
        let prefetched: Vec<_> = served.iter().filter(|n| n.starts_with("p-")).collect();
        assert_eq!(
            prefetched.len(),
            50,
            "the prefetch lane lost jobs to a scroll; it is not the bounded lane"
        );
        assert_eq!(*prefetched[0], "p-0", "and it kept its editorial order");
    }

    #[test]
    fn a_stopped_queue_releases_its_workers() {
        let queue = Queue::default();
        queue.push_urgent(job("pending"));
        queue.stop();
        assert!(queue.pop().is_none());
    }
}
