use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use crate::domain::{StatusReport, StatusState, StatusUpdate};

const STATUS_CLASSIFIER_CAPACITY: usize = 256;
const STATUS_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Status records received from one foreground process observation. This is
/// producer metadata only; it never enters the public status model or wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeStatusBatch {
    pub updates: Vec<StatusUpdate>,
    pub foreground_pid: Option<u32>,
}

impl NativeStatusBatch {
    pub(crate) fn owner_pid(&self, terminal_pid: u32) -> Option<u32> {
        // This PID was observed on the PTY when the batch was received. Its
        // ancestry may already be gone by delivery; cleanup, not attribution,
        // decides whether that recorded owner is still alive.
        self.foreground_pid
            .filter(|pid| *pid != 0 && *pid != terminal_pid)
    }
}

/// Runtime-owned pacing state. Ghostty only records callbacks; this layer
/// associates those records with the foreground process observed immediately
/// after their VT feed and keeps that association through deferred delivery.
#[derive(Default)]
pub(super) struct StatusCoalescer {
    pending: PendingStatusUpdates,
    foreground_pid: Option<u32>,
    has_producer: bool,
}

impl StatusCoalescer {
    pub(super) fn receive(&mut self, batch: NativeStatusBatch) -> Vec<NativeStatusBatch> {
        self.receive_at(batch, Instant::now())
    }

    pub(super) fn flush(&mut self, force: bool) -> Option<NativeStatusBatch> {
        self.flush_at(force, Instant::now())
    }

    pub(super) fn wait(&self) -> Option<Duration> {
        self.pending.wait_at(Instant::now())
    }

    pub(super) fn finish(&mut self) -> NativeStatusBatch {
        self.pending.process_exit();
        self.take()
            .expect("process exit is an urgent status update")
    }

    fn receive_at(&mut self, batch: NativeStatusBatch, now: Instant) -> Vec<NativeStatusBatch> {
        if batch.updates.is_empty() {
            return Vec::new();
        }

        let mut ready = Vec::with_capacity(2);
        if self.has_producer && self.foreground_pid != batch.foreground_pid {
            if let Some(previous) = self.take() {
                ready.push(previous);
            }
            // Report history belongs to the producer that established it. A
            // new producer's first record is semantic evidence, not progress
            // that may be folded into the previous producer's record.
            self.pending = PendingStatusUpdates::default();
        }
        self.foreground_pid = batch.foreground_pid;
        self.has_producer = true;

        for update in batch.updates {
            self.pending.push(update, now);
        }
        if self.pending.ready_at(now)
            && let Some(current) = self.take()
        {
            ready.push(current);
        }
        ready
    }

    fn flush_at(&mut self, force: bool, now: Instant) -> Option<NativeStatusBatch> {
        if !force && !self.pending.ready_at(now) {
            return None;
        }
        self.take()
    }

    fn take(&mut self) -> Option<NativeStatusBatch> {
        let updates = self.pending.take();
        (!updates.is_empty()).then_some(NativeStatusBatch {
            updates,
            foreground_pid: self.foreground_pid,
        })
    }
}

#[derive(Default)]
struct PendingStatusUpdates {
    updates: VecDeque<StatusUpdate>,
    // Pacing metadata only: reports are always enqueued after a delivered
    // batch, even when they equal this runtime-local history. Authoritative
    // state lives in resources and may have changed through another producer.
    classifiers: HashMap<String, StatusReport>,
    progress_started: Option<Instant>,
    urgent: bool,
}

impl PendingStatusUpdates {
    fn push(&mut self, update: StatusUpdate, now: Instant) {
        match update {
            StatusUpdate::Report(report) => self.report(report, now),
            StatusUpdate::Clear { id } => self.clear(id),
            StatusUpdate::PromptStart => self.prompt_start(),
            StatusUpdate::ProcessExit => self.process_exit(),
        }
    }

    fn report(&mut self, report: StatusReport, now: Instant) {
        let progress_only = self
            .classifiers
            .get(&report.id)
            .is_some_and(|previous| reports_differ_only_in_progress(previous, &report));
        if !self.classifiers.contains_key(&report.id)
            && self.classifiers.len() == STATUS_CLASSIFIER_CAPACITY
            && let Some(evicted) = self.classifiers.keys().next().cloned()
        {
            self.classifiers.remove(&evicted);
        }
        self.classifiers.insert(report.id.clone(), report.clone());

        // With no urgent operation pending, every queued item is replaceable
        // progress. Keep each ID's latest receipt at its actual parser position:
        // A10/B10/A20 becomes B10/A20, never A20/B10. Do not cross a lifecycle
        // or meaningful-report barrier, where Working evidence must survive.
        if !self.urgent
            && let Some(index) = self.updates.iter().position(|update| {
                matches!(update, StatusUpdate::Report(queued)
                    if queued.id == report.id
                        && (queued == &report || reports_differ_only_in_progress(queued, &report)))
            })
        {
            self.updates.remove(index);
        }
        if let Some(StatusUpdate::Report(queued)) = self.updates.back_mut()
            && queued.id == report.id
            && (queued == &report
                || (progress_only && reports_differ_only_in_progress(queued, &report)))
        {
            *queued = report;
            return;
        }

        self.updates.push_back(StatusUpdate::Report(report));
        if progress_only {
            self.progress_started.get_or_insert(now);
        } else {
            self.urgent = true;
        }
    }

    fn clear(&mut self, id: Option<String>) {
        let target = id.as_deref().unwrap_or_default();
        if target.is_empty() {
            self.classifiers.clear();
        } else {
            self.classifiers.retain(|record_id, _| {
                record_id != target
                    && !record_id
                        .strip_prefix(target)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            });
        }
        self.updates.push_back(StatusUpdate::Clear { id });
        self.urgent = true;
    }

    fn prompt_start(&mut self) {
        self.classifiers
            .retain(|_, report| matches!(report.state, StatusState::Done | StatusState::Error));
        self.updates.push_back(StatusUpdate::PromptStart);
        self.urgent = true;
    }

    fn process_exit(&mut self) {
        self.classifiers
            .retain(|_, report| matches!(report.state, StatusState::Done | StatusState::Error));
        self.updates.push_back(StatusUpdate::ProcessExit);
        self.urgent = true;
    }

    fn ready_at(&self, now: Instant) -> bool {
        self.urgent
            || self
                .progress_started
                .is_some_and(|started| now.duration_since(started) >= STATUS_PROGRESS_INTERVAL)
    }

    fn wait_at(&self, now: Instant) -> Option<Duration> {
        if self.updates.is_empty() || self.urgent {
            return self.urgent.then_some(Duration::ZERO);
        }
        self.progress_started
            .map(|started| STATUS_PROGRESS_INTERVAL.saturating_sub(now.duration_since(started)))
    }

    fn take(&mut self) -> Vec<StatusUpdate> {
        self.urgent = false;
        self.progress_started = None;
        self.updates.drain(..).collect()
    }
}

fn reports_differ_only_in_progress(previous: &StatusReport, report: &StatusReport) -> bool {
    previous.id == report.id
        && previous.state == report.state
        && previous.kind == report.kind
        && previous.app == report.app
        && previous.title == report.title
        && previous.message == report.message
        && previous.progress != report.progress
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_owner_uses_receipt_metadata_without_a_liveness_query() {
        let mut batch = NativeStatusBatch {
            updates: Vec::new(),
            foreground_pid: Some(12345),
        };
        assert_eq!(batch.owner_pid(12344), Some(12345));
        assert_eq!(batch.owner_pid(12345), None);
        batch.foreground_pid = Some(0);
        assert_eq!(batch.owner_pid(12344), None);
        batch.foreground_pid = None;
        assert_eq!(batch.owner_pid(12344), None);
    }

    fn report(id: impl Into<String>, progress: u8) -> StatusReport {
        StatusReport {
            id: id.into(),
            state: StatusState::Working,
            kind: None,
            progress: Some(progress),
            app: None,
            title: None,
            message: None,
        }
    }

    fn receipt(pid: u32, updates: Vec<StatusUpdate>) -> NativeStatusBatch {
        NativeStatusBatch {
            updates,
            foreground_pid: Some(pid),
        }
    }

    #[test]
    fn delayed_progress_flush_keeps_its_receipt_time_pid() {
        let now = Instant::now();
        let mut statuses = StatusCoalescer::default();
        assert_eq!(
            statuses.receive_at(
                receipt(41, vec![StatusUpdate::Report(report("task", 0))]),
                now,
            ),
            vec![receipt(41, vec![StatusUpdate::Report(report("task", 0))])]
        );
        assert!(
            statuses
                .receive_at(
                    receipt(41, vec![StatusUpdate::Report(report("task", 100))]),
                    now,
                )
                .is_empty()
        );

        // The tty foreground may change while no status is received. A due
        // flush does not resample it or relabel the already-received record.
        assert_eq!(
            statuses.flush_at(false, now + STATUS_PROGRESS_INTERVAL),
            Some(receipt(41, vec![StatusUpdate::Report(report("task", 100))]))
        );
    }

    #[test]
    fn pid_boundary_flushes_each_record_with_its_own_cohort() {
        let now = Instant::now();
        let mut statuses = StatusCoalescer::default();
        let _ = statuses.receive_at(
            receipt(41, vec![StatusUpdate::Report(report("task", 0))]),
            now,
        );
        assert!(
            statuses
                .receive_at(
                    receipt(41, vec![StatusUpdate::Report(report("task", 50))]),
                    now,
                )
                .is_empty()
        );

        assert_eq!(
            statuses.receive_at(
                receipt(73, vec![StatusUpdate::Report(report("task", 75))]),
                now,
            ),
            vec![
                receipt(41, vec![StatusUpdate::Report(report("task", 50))]),
                receipt(73, vec![StatusUpdate::Report(report("task", 75))]),
            ]
        );
    }

    #[test]
    fn pure_progress_is_bounded_and_keeps_last_parser_order() {
        let now = Instant::now();
        let mut statuses = StatusCoalescer::default();
        let initial = (0..STATUS_CLASSIFIER_CAPACITY)
            .map(|id| StatusUpdate::Report(report(id.to_string(), 0)))
            .collect();
        let delivered = statuses.receive_at(receipt(1, initial), now);
        assert_eq!(delivered[0].updates.len(), STATUS_CLASSIFIER_CAPACITY);

        for progress in 1..=20 {
            for id in 0..STATUS_CLASSIFIER_CAPACITY {
                assert!(
                    statuses
                        .receive_at(
                            receipt(
                                1,
                                vec![StatusUpdate::Report(report(id.to_string(), progress))]
                            ),
                            now,
                        )
                        .is_empty()
                );
            }
        }
        let updates = statuses.flush_at(true, now).unwrap().updates;
        assert_eq!(updates.len(), STATUS_CLASSIFIER_CAPACITY);
        assert_eq!(
            updates,
            (0..STATUS_CLASSIFIER_CAPACITY)
                .map(|id| StatusUpdate::Report(report(id.to_string(), 20)))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn interleaved_progress_keeps_latest_receipt_order() {
        let now = Instant::now();
        let mut statuses = StatusCoalescer::default();
        let _ = statuses.receive_at(
            receipt(
                1,
                vec![
                    StatusUpdate::Report(report("a", 0)),
                    StatusUpdate::Report(report("b", 0)),
                ],
            ),
            now,
        );
        let _ = statuses.receive_at(
            receipt(
                1,
                vec![
                    StatusUpdate::Report(report("a", 10)),
                    StatusUpdate::Report(report("b", 10)),
                    StatusUpdate::Report(report("a", 20)),
                ],
            ),
            now,
        );

        assert_eq!(
            statuses.flush_at(true, now).unwrap().updates,
            vec![
                StatusUpdate::Report(report("b", 10)),
                StatusUpdate::Report(report("a", 20)),
            ]
        );
    }

    #[test]
    fn classifier_history_never_suppresses_exact_redelivery() {
        let now = Instant::now();
        let update = StatusUpdate::Report(report("task", 40));
        let mut statuses = StatusCoalescer::default();
        assert_eq!(
            statuses.receive_at(receipt(1, vec![update.clone()]), now)[0].updates,
            vec![update.clone()]
        );
        assert_eq!(
            statuses.receive_at(receipt(1, vec![update.clone()]), now)[0].updates,
            vec![update]
        );
    }
}
