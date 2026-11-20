//! Queue scheduling on the shared `queue_jobs` table.
//!
//! Persistence is delegated to [`paradl_store::Queue`] (schema v4, WAL,
//! priority ordering `priority DESC, id ASC`). This module adds the scheduling
//! policy — `--at HH:MM` deferral, `--when-fast` speed probes, crash recovery —
//! and turns every state transition into a typed `paradl-events` value.
//!
//! ```
//! # use paradl_store::{QueueJob, SqliteStore, Store};
//! # use paradl_resilience::queue::QueueScheduler;
//! # let dir = tempfile::tempdir()?;
//! let store = SqliteStore::open_at(dir.path().join("history.db"))?;
//! let scheduler = QueueScheduler::new(store.open_queue()?);
//! let emitted = scheduler.add_job(&QueueJob::new("https://e.test/f", "/tmp"))?;
//! assert_eq!(emitted.name(), "queue_job_added");
//! # Ok::<(), paradl_core::Error>(())
//! ```

use chrono::{DateTime, Local, Timelike};
use paradl_events::{
    Event, QueueCrashRecoveredEvent, QueueJobAddedEvent, QueueJobDeferredEvent, QueueJobRow,
    QueueJobStartedEvent, QueueListEvent, QueuePausedEvent, QueueRemovedEvent, QueueResumedEvent,
    WhenFastProbeEvent, py_round,
};
use paradl_store::{Queue, QueueJob};
use serde_json::Value;

use crate::{Emitted, Result, now};

/// Outcome of a `--when-fast` gate evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct WhenFastGate {
    /// Whether the threshold was met.
    pub passed: bool,
    /// Last measured speed in bytes/s.
    pub measured_bps: f64,
    /// One `when_fast_probe` event per attempt.
    pub events: Vec<Emitted>,
}

/// Scheduler wrapping the shared queue with typed event emission.
pub struct QueueScheduler {
    queue: Queue,
}

impl QueueScheduler {
    /// Build a scheduler over an open `queue_jobs` handle.
    pub fn new(queue: Queue) -> Self {
        Self { queue }
    }

    /// Add a job and emit `queue_job_added` (E48).
    pub fn add_job(&self, job: &QueueJob) -> Result<Emitted> {
        let stored = self.queue.add_job(job)?;
        let event = QueueJobAddedEvent::new(
            now(),
            stored.id,
            stored.target.clone(),
            Some(false),
            Some(stored.priority),
            Some(stored.require_ac),
            Some(stored.when_fast_bps),
            stored.start_at.clone(),
        );
        Ok(Emitted::new(Event::QueueJobAdded(event)))
    }

    /// Convert a stored job into the oracle's 13-field `queue_list` row.
    ///
    /// The field order here is byte-exact against
    /// `src/paradl/resilience/queue.py:464-478`; the custom serializer in
    /// `paradl-events` preserves struct declaration order.
    fn job_row(job: &QueueJob) -> QueueJobRow {
        QueueJobRow::new(
            job.id,
            job.target.clone(),
            job.status.clone(),
            job.priority,
            job.connections,
            job.require_ac,
            job.when_fast_bps,
            job.start_at.clone(),
            job.bytes_downloaded,
            job.expected_length,
            job.out_dir.clone(),
            job.filename.clone(),
            job.error.clone(),
        )
    }

    /// List jobs in `priority DESC, id ASC` order and emit `queue_list` (E51).
    pub fn list_jobs(&self, status: Option<&str>) -> Result<(Vec<QueueJob>, Emitted)> {
        self.list_jobs_with_timestamp(status, now())
    }

    /// Same as [`Self::list_jobs`] with an injected timestamp for byte-exact goldens.
    pub fn list_jobs_with_timestamp(
        &self,
        status: Option<&str>,
        timestamp: f64,
    ) -> Result<(Vec<QueueJob>, Emitted)> {
        let jobs = self.queue.list_jobs(status)?;
        let rows = jobs.iter().map(Self::job_row).collect();
        let emitted = Emitted::new(Event::QueueList(QueueListEvent::new(timestamp, rows)));
        Ok((jobs, emitted))
    }

    /// Transition a job to `running` and emit `queue_job_started` (E50).
    pub fn start_job(&self, job: &QueueJob, connections: i32) -> Result<Emitted> {
        self.queue.update_job_status(job.id, "running", None, None, None)?;
        Ok(Emitted::new(Event::QueueJobStarted(QueueJobStartedEvent::new(
            now(),
            job.id,
            job.target.clone(),
            job.priority,
            connections,
        ))))
    }

    /// Keep a job queued and emit `queue_job_deferred` (E49).
    pub fn defer(
        &self,
        job: &QueueJob,
        reason: &str,
        start_at: Option<String>,
        measured_bps: Option<f64>,
        threshold_bps: Option<f64>,
    ) -> Result<Emitted> {
        Ok(Emitted::new(Event::QueueJobDeferred(QueueJobDeferredEvent::new(
            now(),
            job.id,
            reason.to_string(),
            start_at,
            measured_bps,
            threshold_bps,
        ))))
    }

    /// Evaluate `--when-fast`, emitting one `when_fast_probe` per attempt (E61).
    pub fn when_fast_gate<F: Fn(&str) -> f64>(
        &self,
        url: &str,
        threshold_bps: f64,
        polls: usize,
        prober: &F,
    ) -> Result<WhenFastGate> {
        let polls = polls.max(1);
        let mut events = Vec::with_capacity(polls);
        let mut measured = 0.0;
        for attempt in 1..=polls {
            measured = prober(url);
            let rounded_measured = py_round(measured, 2);
            let rounded_threshold = py_round(threshold_bps, 2);
            let passed = measured >= threshold_bps;
            events.push(Emitted::new(Event::WhenFastProbe(WhenFastProbeEvent::new(
                now(),
                url.to_string(),
                attempt as i32,
                rounded_measured,
                rounded_threshold,
                passed,
            ))));
            if passed {
                return Ok(WhenFastGate {
                    passed: true,
                    measured_bps: measured,
                    events,
                });
            }
        }
        Ok(WhenFastGate {
            passed: false,
            measured_bps: measured,
            events,
        })
    }

    /// Recover jobs left `running` after a crash and emit `queue_crash_recovered` (E47).
    pub fn recover_crashed(&self) -> Result<Option<Emitted>> {
        let recovered = self.queue.recover_crashed_jobs()?;
        if recovered == 0 {
            return Ok(None);
        }
        Ok(Some(Emitted::new(Event::QueueCrashRecovered(
            QueueCrashRecoveredEvent::new(now(), recovered),
        ))))
    }

    /// Pause jobs and emit `queue_paused` (E52).
    pub fn pause(&self, ids: &[i64], all: bool) -> Result<Emitted> {
        let changed = self.queue.pause_jobs(ids, all)?;
        Ok(Emitted::new(Event::QueuePaused(QueuePausedEvent::new(
            now(),
            Value::Number(changed.into()),
        ))))
    }

    /// Resume jobs and emit `queue_resumed` (E54).
    pub fn resume(&self, ids: &[i64], all: bool) -> Result<Emitted> {
        let changed = self.queue.resume_jobs(ids, all)?;
        Ok(Emitted::new(Event::QueueResumed(QueueResumedEvent::new(
            now(),
            Value::Number(changed.into()),
            Some(all),
        ))))
    }

    /// Remove jobs and emit `queue_removed` (E53).
    pub fn remove(&self, ids: &[i64], all: bool) -> Result<Emitted> {
        let changed = self.queue.remove_jobs(ids, all)?;
        Ok(Emitted::new(Event::QueueRemoved(QueueRemovedEvent::new(
            now(), changed,
        ))))
    }

    /// Start every due job, deferring those held by `--at` or `--when-fast`.
    ///
    /// `now` is injected so tests can assert schedule gates without sleeping.
    pub fn start_due_jobs<F: Fn(&str) -> f64>(
        &self,
        now_local: DateTime<Local>,
        connections: i32,
        prober: &F,
    ) -> Result<(Vec<QueueJob>, Vec<Emitted>)> {
        let (jobs, _listing) = self.list_jobs(Some("queued"))?;
        let mut started = Vec::new();
        let mut events = Vec::new();
        for job in jobs {
            if let Some(start_at) = job.start_at.as_deref() {
                let due = crate::plist::is_schedule_due(start_at, &now_local);
                if !due {
                    events.push(self.defer(&job, "start_at", Some(start_at.to_string()), None, None)?);
                    continue;
                }
            }
            if job.when_fast_bps > 0.0 {
                let gate = self.when_fast_gate(&job.target, job.when_fast_bps, 3, prober)?;
                events.extend(gate.events);
                if !gate.passed {
                    events.push(self.defer(
                        &job,
                        "when_fast",
                        None,
                        Some(py_round(gate.measured_bps, 2)),
                        Some(py_round(job.when_fast_bps, 2)),
                    )?);
                    continue;
                }
            }
            events.push(self.start_job(&job, connections)?);
            started.push(job);
        }
        Ok((started, events))
    }
}

/// `HH:MM` hour/minute extractor used by the `--at` gate.
pub fn schedule_hour_minute(now: &DateTime<Local>) -> (u32, u32) {
    (now.hour(), now.minute())
}
