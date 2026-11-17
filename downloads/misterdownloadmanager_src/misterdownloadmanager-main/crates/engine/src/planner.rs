//! Dynamic segmentation planner — pure logic, no I/O.
//!
//! Implements IDM's documented rule (notes/ENGINE.md §1). When a connection is free it:
//! 1. takes an unassigned, unfinished segment (e.g. after a resume or a failed worker);
//! 2. else takes over the segment right after the one it just finished, if that segment's
//!    connection has not started receiving yet and has waited at least `preempt_after` (ours:
//!    a request still on its way would be sent twice) — that connection is preempted;
//! 3. else splits the largest remaining segment in half and takes the upper half,
//!    unless each half would be smaller than `min_split`.
//!
//! Workers must [`claim`](Planner::claim) bytes before writing them and
//! [`commit`](Planner::commit) after. Splits are measured from the claimed position, so a
//! split can never cut into bytes that are being written.

use serde::{Deserialize, Serialize};

pub type WorkerId = u32;

/// Split points are rounded down to this boundary when possible.
const ALIGN: u64 = 4096;

/// One contiguous part of the file. Positions are byte offsets; ranges are half-open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Where the segment began (a start marker on the progress bar).
    pub start: u64,
    /// `[start, cursor)` is written to disk.
    pub cursor: u64,
    /// `[cursor, reserved)` is claimed by the worker and being written.
    pub reserved: u64,
    /// Exclusive end; moves down when the segment is split.
    pub end: u64,
    pub worker: Option<WorkerId>,
    /// The worker has received data for this assignment (it can no longer be preempted).
    pub receiving: bool,
    /// When the current worker got it.
    pub given_at: Option<std::time::Instant>,
}

impl Segment {
    fn unclaimed(&self) -> u64 {
        self.end - self.reserved
    }
}

/// The persisted form of a segment (resume state).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSegment {
    pub start: u64,
    pub cursor: u64,
    pub end: u64,
}

/// Bytes `[from, to)` for a worker to fetch. `to` may later shrink (see [`Planner::claim`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub from: u64,
    pub to: u64,
    /// Bytes of this segment directly before `from` that are already on disk (resumed or
    /// taken-over segments; 0 for a fresh split). The worker re-fetches a few of them to check
    /// the file did not change (IDM's resume overlap check).
    pub written: u64,
    /// A worker whose not-yet-receiving segment was taken over; its next claim is `Lost`.
    pub preempted: Option<WorkerId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// Write this many bytes (possibly fewer than asked: the segment was split).
    Granted(u64),
    /// The worker no longer owns a segment at that position; it should ask for a new one.
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidState;

#[derive(Clone, Debug)]
pub struct Planner {
    size: u64,
    min_split: u64,
    /// How long a connection may wait for its first byte before a free one takes its segment.
    preempt_after: std::time::Duration,
    /// Sorted by `start`, contiguous, covering `[0, size)`.
    segs: Vec<Segment>,
}

impl Planner {
    /// A fresh download of `size` bytes. `min_split` (≥ 1) is the smallest half a split may leave.
    pub fn new(size: u64, min_split: u64) -> Self {
        let segs = if size == 0 {
            Vec::new()
        } else {
            vec![fresh(0, size, None)]
        };
        Planner {
            size,
            min_split: min_split.max(1),
            preempt_after: std::time::Duration::ZERO,
            segs,
        }
    }

    /// A pending connection is only taken over after waiting this long (about two of the
    /// server's first-byte times): sooner, its request is usually already on its way.
    pub fn set_preempt_after(&mut self, d: std::time::Duration) {
        self.preempt_after = d;
    }

    /// Restore saved progress. Fails if the saved segments don't tile `[0, size)` exactly.
    pub fn resume(size: u64, min_split: u64, saved: &[SavedSegment]) -> Result<Self, InvalidState> {
        let mut expect = 0;
        for s in saved {
            if s.start != expect || s.cursor < s.start || s.cursor > s.end || s.end <= s.start {
                return Err(InvalidState);
            }
            expect = s.end;
        }
        if expect != size {
            return Err(InvalidState);
        }
        let segs = saved
            .iter()
            .map(|s| Segment {
                start: s.start,
                cursor: s.cursor,
                reserved: s.cursor,
                end: s.end,
                worker: None,
                receiving: false,
                given_at: None,
            })
            .collect();
        Ok(Planner {
            size,
            min_split: min_split.max(1),
            preempt_after: std::time::Duration::ZERO,
            segs,
        })
    }

    pub fn save(&self) -> Vec<SavedSegment> {
        self.segs
            .iter()
            .map(|s| SavedSegment {
                start: s.start,
                cursor: s.cursor,
                end: s.end,
            })
            .collect()
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segs
    }

    /// Bytes written to disk.
    pub fn downloaded(&self) -> u64 {
        self.segs.iter().map(|s| s.cursor - s.start).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.segs.iter().all(|s| s.cursor == s.end)
    }

    pub fn active_workers(&self) -> usize {
        self.segs.iter().filter(|s| s.worker.is_some()).count()
    }

    /// Workers that have received data for their current segment (not just connecting).
    pub fn receiving_workers(&self) -> usize {
        self.segs
            .iter()
            .filter(|s| s.worker.is_some() && s.receiving)
            .count()
    }

    /// Next work for free worker `w`. `after` is the end of the segment it just finished, if any.
    /// `None` means there is nothing worth splitting: the worker should stop.
    pub fn assign(&mut self, w: WorkerId, after: Option<u64>) -> Option<Assignment> {
        self.release(w);

        // 1. An unassigned, unfinished segment — prefer the one continuing where `w` stopped.
        let open = |s: &Segment| s.worker.is_none() && s.unclaimed() > 0;
        let pick = self
            .segs
            .iter()
            .position(|s| open(s) && Some(s.start) == after)
            .or_else(|| self.segs.iter().position(open));
        if let Some(i) = pick {
            return Some(self.give(i, w, None));
        }

        // 2. IDM's reassign rule: the next segment's connection hasn't started receiving.
        if let Some(a) = after
            && let Some(i) = self.segs.iter().position(|s| {
                s.start == a
                    && s.worker.is_some()
                    && !s.receiving
                    && s.unclaimed() > 0
                    && s.given_at.is_none_or(|t| t.elapsed() >= self.preempt_after)
            })
        {
            let pre = self.segs[i].worker;
            return Some(self.give(i, w, pre));
        }

        // 3. Split the largest remaining segment in half.
        let (i, seg) = self
            .segs
            .iter()
            .enumerate()
            .max_by_key(|(i, s)| (s.unclaimed(), std::cmp::Reverse(*i)))?;
        let half = seg.unclaimed() / 2;
        if half < self.min_split {
            return None;
        }
        let mut mid = seg.reserved + half;
        let aligned = mid - mid % ALIGN;
        if aligned >= seg.reserved + self.min_split {
            mid = aligned;
        }
        let end = seg.end;
        self.segs[i].end = mid;
        self.segs.insert(i + 1, fresh(mid, end, Some(w)));
        Some(Assignment {
            from: mid,
            to: end,
            written: 0,
            preempted: None,
        })
    }

    /// How many bytes worker `w` may still fetch from `at` before its segment ends (the end
    /// moves down when the segment is split). `None`: `w` no longer owns a segment.
    pub fn room(&self, w: WorkerId, at: u64) -> Option<u64> {
        let s = self.segs.iter().find(|s| s.worker == Some(w))?;
        Some(s.end.saturating_sub(at))
    }

    /// Worker `w` wants to write `len` bytes at `pos` (its next unwritten position).
    pub fn claim(&mut self, w: WorkerId, pos: u64, len: u64) -> Claim {
        let Some(s) = self.segs.iter_mut().find(|s| s.worker == Some(w)) else {
            return Claim::Lost;
        };
        if pos != s.reserved {
            return Claim::Lost;
        }
        let n = len.min(s.unclaimed());
        s.reserved += n;
        if n > 0 {
            s.receiving = true;
        }
        Claim::Granted(n)
    }

    /// Worker `w` has written everything up to `upto`. Returns `Some(end)` when its segment is
    /// now complete (the worker is released and should call [`assign`](Self::assign) with
    /// `after = Some(end)`).
    pub fn commit(&mut self, w: WorkerId, upto: u64) -> Option<u64> {
        let s = self.segs.iter_mut().find(|s| s.worker == Some(w))?;
        s.cursor = upto.clamp(s.cursor, s.reserved);
        if s.cursor == s.end {
            s.worker = None;
            s.receiving = false;
            return Some(s.end);
        }
        None
    }

    /// Worker `w` failed or stopped: its segment becomes unassigned and uncommitted claims are
    /// dropped (those bytes will be fetched again).
    pub fn release(&mut self, w: WorkerId) {
        if let Some(s) = self.segs.iter_mut().find(|s| s.worker == Some(w)) {
            s.worker = None;
            s.reserved = s.cursor;
            s.receiving = false;
        }
    }

    /// Whether worker `w` holds a segment (it is fetching one).
    pub fn holds(&self, w: WorkerId) -> bool {
        self.segs.iter().any(|s| s.worker == Some(w))
    }

    /// Worker `w`, at the end of its segment, takes over the next one if nothing of it has
    /// arrived yet: a connection whose answer runs on to the end of the file (the probe's plain
    /// GET) keeps reading instead of leaving those bytes to a range request the server may
    /// ignore. The next segment's worker, if any, loses it (its claims come back `Lost`).
    pub fn absorb_next(&mut self, w: WorkerId) -> bool {
        let Some(i) = self.segs.iter().position(|s| s.worker == Some(w)) else {
            return false;
        };
        let Some(next) = self.segs.get(i + 1).copied() else {
            return false;
        };
        if next.start != self.segs[i].end || next.reserved > next.start {
            return false;
        }
        self.segs[i].end = next.end;
        self.segs.remove(i + 1);
        true
    }

    /// The server turned out not to support ranges: give the whole file back to `w` (the
    /// connection streaming from byte 0) and never split again. Fails if `w` doesn't own the
    /// segment at 0 or another segment already has data — then the download must restart.
    pub fn make_single(&mut self, w: WorkerId) -> bool {
        let Some(first) = self.segs.first().copied() else {
            return false;
        };
        if first.worker != Some(w) || self.segs[1..].iter().any(|s| s.cursor > s.start) {
            return false;
        }
        self.segs = vec![Segment {
            end: self.size,
            ..first
        }];
        self.min_split = u64::MAX;
        true
    }

    fn give(&mut self, i: usize, w: WorkerId, preempted: Option<WorkerId>) -> Assignment {
        let s = &mut self.segs[i];
        s.worker = Some(w);
        s.receiving = false;
        s.given_at = Some(std::time::Instant::now());
        s.reserved = s.cursor;
        Assignment {
            from: s.cursor,
            to: s.end,
            written: s.cursor - s.start,
            preempted,
        }
    }
}

fn fresh(start: u64, end: u64, worker: Option<WorkerId>) -> Segment {
    Segment {
        start,
        cursor: start,
        reserved: start,
        end,
        worker,
        receiving: false,
        given_at: worker.map(|_| std::time::Instant::now()),
    }
}

#[cfg(test)]
mod tests;
