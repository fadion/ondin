//! The font pool's work queue: three lanes, one of which is a stack.
//!
//! **A face the user asked for and a face a preview row speculated about are not
//! the same job.** Picking a family is a promise the bytes will be used, so those
//! go in a FIFO lane and are served first. A preview row's face is a guess: by
//! the time a worker frees up, the row may have scrolled off screen, so that lane
//! is LIFO and bounded — the newest request is the one still likely to be visible,
//! and the backlog is dropped from its *oldest* end.
//!
//! Without the bound, dragging the picker's scrollbar across 1900 families queued
//! a download per family and the twenty rows actually on screen arrived last.
//!
//! **And a face nothing has asked for yet is a third thing** (§15 D352): the
//! popular set, warmed at startup so the picker is worth opening offline. It is
//! speculation about a session that has not begun, so it waits for both of the
//! others — FIFO, because the order of that list is an editorial judgement, and
//! uncapped, because it is a fixed fifty pushed once.
//!
//! ⚠️ **The bound above discards work, and discarding work is an event the
//! caller has to hear about** (§15 D354). [`Queue::push_preview`] hands back what
//! it pushed out for exactly that reason: a dropped job never reports, so without
//! it the service holds that face's claim for the rest of the session and can
//! never ask for it again.

use super::FaceSpec;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// Preview faces allowed to wait for a worker. Roughly two screenfuls: enough
/// that a steady scroll never starves, few enough that a flung scrollbar cannot
/// bank thousands of stale downloads.
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
    /// Faces nothing has asked for yet — the popular set, warmed at startup so
    /// the picker is useful on a machine with no network (§15 D352). Served last,
    /// oldest first.
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
    /// ⚠️ **The caller must release what comes back** (§15 D354). A dropped job is
    /// a face that was claimed and dispatched and will now never report — so
    /// without this it leaves `FontService::claimed` holding its key and the
    /// family's `outstanding` count above zero **for the rest of the session**.
    /// The family then reads as permanently loading, and because the key is still
    /// claimed, `ensure` filters its spec out and it can never be requested again:
    /// one flung scrollbar and those rows are stuck for good. The bound has always
    /// dropped jobs deliberately; what was missing is that dropping one is an
    /// event the bookkeeping has to hear about.
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

    /// A face nothing has asked for: the popular set, warmed at startup
    /// (§15 D352).
    ///
    /// **A third lane rather than either of the two above**, because a prefetch is
    /// speculation about a session that has not happened and neither existing
    /// lane's rule fits it. `urgent` would put it ahead of a face the user is
    /// waiting on, which is the one thing that must never happen. `preview` is
    /// worse than it looks: it is LIFO, so the editorial order of the list would
    /// come out backwards, and it is capped at [`PREVIEW_BACKLOG`], so **the first
    /// two of fifty would be dropped before a worker ever woke up** — and the rest
    /// would be evicted by the first scroll, silently turning the feature off for
    /// exactly the user who is browsing fonts.
    ///
    /// FIFO because the order *is* the editorial judgement, and uncapped because
    /// the list is a fixed fifty pushed once. Nothing else may enqueue here.
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
            // Strictly in this order: asked for, then speculated about on screen,
            // then speculated about for a session that has not started.
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

    /// The reported symptom: flinging the scrollbar filled the queue, and the
    /// rows on screen were served after everything scrolled past.
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

    /// **A prefetch waits for both other lanes, and keeps its order** (§15 D352).
    ///
    /// Both halves matter and they fail differently. Served last, or the popular
    /// set delays the face the user is actually waiting on — the one thing this
    /// lane must never do. FIFO, because the order of the list *is* the editorial
    /// judgement: reversing it means the fifty-first-most-wanted family arrives
    /// first, which is the wrong fifty if the run is cut short by a quit.
    ///
    /// ⚠️ Flipped by routing `push_prefetch` to `push_preview`, which is the
    /// tempting way to avoid a third lane. Both assertions fail: the preview lane
    /// is LIFO *and* is drained before nothing, so `pop` hands back `p-2` first
    /// and hands it back before `picked`.
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

    /// **A scroll cannot throw the prefetch away**, which routing it through the
    /// preview lane would: that lane is capped at `PREVIEW_BACKLOG` and drops
    /// from its oldest end, so fifty prefetch jobs would lose two immediately and
    /// the rest to the first flung scrollbar — turning the feature off for the one
    /// user who is actively browsing fonts.
    ///
    /// ⚠️ Flipped the same way as above: the survivors drop to `PREVIEW_BACKLOG`
    /// and this fails on the count.
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
