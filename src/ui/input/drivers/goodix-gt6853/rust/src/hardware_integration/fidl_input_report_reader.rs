// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The code in this module is useful, but does not meet the team's quality
//! bar. See go/fuchsia-display-rough

// TODO(https://fxbug.dev/559080325): Replace with shared input-report-reader crate once available.

use crate::data_types::report_stamp::ReportStamp;
use fidl_next;
use fidl_next_fuchsia_input_report as fidl_input_report;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::channel::mpsc;
use std::collections::VecDeque;
use std::sync::Arc;

/// Max reports stored per reader before dropping oldest.
const MAX_DEVICE_REPORT_COUNT: usize = fidl_input_report::MAX_DEVICE_REPORT_COUNT as usize;

/// Reason why new reports may be ready to send to an `InputReportsReaderV2` client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportsReadyReason {
    /// A new report has been queued in the reader state.
    NewReportQueued,
    /// A client acknowledged previous reports, freeing up in-flight capacity.
    ReportsAcknowledged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct V2BatchRecord {
    report_count: usize,
    last_report_stamp: ReportStamp,
}

struct V2ReaderState {
    pending_reports: VecDeque<(fidl_input_report::InputReport, ReportStamp)>,
    unacknowledged_batches: VecDeque<V2BatchRecord>,
    next_report_stamp: ReportStamp,
    last_acknowledged_report_stamp: ReportStamp,
    max_unacknowledged_reports: u16,
}

impl V2ReaderState {
    fn new(max_unacknowledged_reports: u16) -> Self {
        Self {
            pending_reports: VecDeque::with_capacity(MAX_DEVICE_REPORT_COUNT),
            unacknowledged_batches: VecDeque::new(),
            next_report_stamp: ReportStamp(1),
            last_acknowledged_report_stamp: ReportStamp::INVALID,
            max_unacknowledged_reports,
        }
    }

    /// Pushes an input report to the reader FIFO queue with a monotonically increasing stamp.
    ///
    /// If the queue is full (exceeds [`MAX_DEVICE_REPORT_COUNT`]), the oldest report is dropped.
    fn push_report(&mut self, report: fidl_input_report::InputReport) {
        if self.pending_reports.len() >= MAX_DEVICE_REPORT_COUNT {
            log::debug!("V2ReaderState pending report queue full; dropping oldest pending report");
            self.pending_reports.pop_front();
        }
        let stamp = self.next_report_stamp;
        self.next_report_stamp = self.next_report_stamp.next();
        self.pending_reports.push_back((report, stamp));
    }

    /// Acknowledge report batches with stamps <= `stamp`.
    fn acknowledge_reports(&mut self, stamp: ReportStamp) {
        while self.unacknowledged_batches.front().is_some_and(|f| f.last_report_stamp <= stamp) {
            self.unacknowledged_batches.pop_front();
        }
        if stamp > self.last_acknowledged_report_stamp {
            self.last_acknowledged_report_stamp = stamp;
        }
    }

    /// Returns the total number of unacknowledged reports currently in flight across all batches.
    fn unacknowledged_reports_count(&self) -> usize {
        self.unacknowledged_batches.iter().map(|b| b.report_count).sum()
    }

    #[cfg(test)]
    fn last_sent_report_stamp(&self) -> ReportStamp {
        self.unacknowledged_batches
            .back()
            .map(|b| b.last_report_stamp)
            .unwrap_or(self.last_acknowledged_report_stamp)
    }

    /// Returns the reports to send and the [`ReportStamp`] of the last report in the batch.
    fn take_next_reports_to_send(
        &mut self,
    ) -> Option<(Vec<fidl_input_report::InputReport>, ReportStamp)> {
        if self.pending_reports.is_empty() {
            return None;
        }

        let unacknowledged = self.unacknowledged_reports_count();
        let max_unacknowledged = self.max_unacknowledged_reports as usize;
        if unacknowledged >= max_unacknowledged {
            return None;
        }

        let allowance = max_unacknowledged - unacknowledged;
        let to_take = allowance.min(self.pending_reports.len()).min(MAX_DEVICE_REPORT_COUNT);
        if to_take == 0 {
            return None;
        }

        let mut reports = Vec::with_capacity(to_take);
        let mut last_stamp = ReportStamp::INVALID;

        for (report, stamp) in self.pending_reports.drain(..to_take) {
            reports.push(report);
            last_stamp = stamp;
        }

        debug_assert_ne!(last_stamp, ReportStamp::INVALID);

        self.unacknowledged_batches.push_back(V2BatchRecord {
            report_count: reports.len(),
            last_report_stamp: last_stamp,
        });
        Some((reports, last_stamp))
    }
}

/// Handles [`fuchsia.input.report.InputReportsReaderV2`] protocol requests.
struct InputReportsReaderV2FidlHandler {
    reader_state: Arc<Mutex<V2ReaderState>>,
    /// Sender to notify the background sender worker when in-flight capacity is freed.
    reports_ready_notification_sender: mpsc::Sender<ReportsReadyReason>,
}

impl InputReportsReaderV2FidlHandler {
    fn new(
        reader_state: Arc<Mutex<V2ReaderState>>,
        reports_ready_notification_sender: mpsc::Sender<ReportsReadyReason>,
    ) -> Self {
        Self { reader_state, reports_ready_notification_sender }
    }
}

impl fidl_input_report::InputReportsReaderV2ServerHandler for InputReportsReaderV2FidlHandler {
    async fn acknowledge_reports(
        &mut self,
        request: fidl_next::Request<fidl_input_report::input_reports_reader_v2::AcknowledgeReports>,
    ) {
        let stamp = ReportStamp(request.payload().last_acknowledged_report_stamp);
        {
            let mut state = self.reader_state.lock();
            state.acknowledge_reports(stamp);
        }
        if let Err(e) =
            self.reports_ready_notification_sender.try_send(ReportsReadyReason::ReportsAcknowledged)
        {
            if e.is_disconnected() {
                log::debug!(
                    "Reports ready notification receiver disconnected on acknowledge: {:?}",
                    e
                );
            }
        }
    }
}

/// Binds together an [`fuchsia.input.report.InputReportsReaderV2`] server with a
/// client and tracks client-specific report stamp acknowledgement state.
pub struct V2ReaderEntry {
    id: usize,
    reader_state: Arc<Mutex<V2ReaderState>>,
    /// Sender to notify the background sender worker when new reports or acknowledgements arrive.
    reports_ready_notification_sender: mpsc::Sender<ReportsReadyReason>,
    #[expect(unused)]
    server_worker_and_cleanup_task: fuchsia_async::Task<()>,
}

impl V2ReaderEntry {
    /// Creates a new `V2ReaderEntry` and spawns the FIDL server loop and sender worker.
    ///
    /// When the FIDL channel closes, `on_disconnect` is invoked to perform cleanup.
    pub fn new(
        id: usize,
        server_end: fidl_next::ServerEnd<fidl_input_report::InputReportsReaderV2>,
        max_unacknowledged_reports: u16,
        on_disconnect: impl FnOnce() + Send + 'static,
    ) -> Self {
        let reader_state = Arc::new(Mutex::new(V2ReaderState::new(max_unacknowledged_reports)));
        let (reports_ready_notification_sender, mut reports_ready_notification_receiver) =
            mpsc::channel::<ReportsReadyReason>(1);

        let handler = InputReportsReaderV2FidlHandler::new(
            reader_state.clone(),
            reports_ready_notification_sender.clone(),
        );
        let dispatcher = fidl_next::ServerDispatcher::new(server_end);
        let server = dispatcher.server();

        let state_for_worker = reader_state.clone();
        let server_worker_and_cleanup_task = fuchsia_async::Task::spawn(async move {
            let server_loop = dispatcher.run(handler);
            let worker_loop = async {
                while let Some(_reason) = reports_ready_notification_receiver.next().await {
                    loop {
                        let to_send = {
                            let mut state = state_for_worker.lock();
                            state.take_next_reports_to_send()
                        };

                        match to_send {
                            Some((reports, stamp)) => {
                                if server.on_input_reports(reports, stamp.get()).await.is_err() {
                                    return;
                                }
                            }
                            None => break,
                        }
                    }
                }
            };

            futures::pin_mut!(server_loop);
            futures::pin_mut!(worker_loop);
            futures::future::select(server_loop, worker_loop).await;

            on_disconnect();
        });

        Self { id, reader_state, reports_ready_notification_sender, server_worker_and_cleanup_task }
    }

    /// Returns the unique ID of this reader.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Pushes an input report to the reader FIFO and triggers asynchronous delivery.
    ///
    /// If the FIFO queue is full, the oldest unread report is dropped.
    pub fn push_report(&self, report: fidl_input_report::InputReport) {
        let mut state = self.reader_state.lock();
        state.push_report(report);
        if let Err(e) = self
            .reports_ready_notification_sender
            .clone()
            .try_send(ReportsReadyReason::NewReportQueued)
        {
            if e.is_disconnected() {
                log::debug!("Reports ready notification receiver disconnected on push: {:?}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;

    fn make_test_input_report(x: i64, y: i64) -> fidl_input_report::InputReport {
        let contact = fidl_input_report::ContactInputReport {
            contact_id: Some(1),
            position_x: Some(x),
            position_y: Some(y),
            ..Default::default()
        };
        let touch_report = fidl_input_report::TouchInputReport {
            contacts: Some(vec![contact]),
            ..Default::default()
        };
        fidl_input_report::InputReport {
            event_time: Some(12345),
            touch: Some(touch_report),
            ..Default::default()
        }
    }

    #[test]
    fn test_v2_reader_state_acknowledge_reports() {
        let mut state = V2ReaderState::new(10);
        state.push_report(make_test_input_report(1, 1));
        state.push_report(make_test_input_report(2, 2));
        let (batch1, stamp1) = state.take_next_reports_to_send().unwrap();
        assert_eq!(batch1.len(), 2);
        assert_eq!(stamp1.get(), 2);
        assert_eq!(state.unacknowledged_reports_count(), 2);

        state.push_report(make_test_input_report(3, 3));
        let (batch2, stamp2) = state.take_next_reports_to_send().unwrap();
        assert_eq!(batch2.len(), 1);
        assert_eq!(stamp2.get(), 3);
        assert_eq!(state.unacknowledged_reports_count(), 3);

        // Acknowledge batch 1
        state.acknowledge_reports(ReportStamp(2));
        assert_eq!(state.unacknowledged_reports_count(), 1);
        assert_eq!(state.last_acknowledged_report_stamp.get(), 2);

        // Acknowledge batch 2
        state.acknowledge_reports(ReportStamp(3));
        assert_eq!(state.unacknowledged_reports_count(), 0);
        assert_eq!(state.last_acknowledged_report_stamp.get(), 3);
    }

    #[fuchsia::test]
    async fn test_v2_reader_entry_delivers_reports_without_acknowledgement_within_limit() {
        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let entry = V2ReaderEntry::new(1, reader_server_end, 5, || {});

        entry.push_report(make_test_input_report(10, 20));
        while entry.reader_state.lock().unacknowledged_reports_count() < 1 {
            fasync::yield_now().await;
        }

        entry.push_report(make_test_input_report(30, 40));
        while entry.reader_state.lock().unacknowledged_reports_count() < 2 {
            fasync::yield_now().await;
        }

        let state = entry.reader_state.lock();
        assert_eq!(state.last_sent_report_stamp().get(), 2);
        assert_eq!(state.last_acknowledged_report_stamp, ReportStamp::INVALID);
        assert_eq!(state.pending_reports.len(), 0);
        assert_eq!(state.unacknowledged_reports_count(), 2);
    }

    #[fuchsia::test]
    async fn test_v2_reader_entry_pauses_delivery_at_unacknowledged_limit() {
        let (reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let reader_client = reader_client_end.spawn();
        let entry = V2ReaderEntry::new(1, reader_server_end, 1, || {});

        // 1. Send first report (fills unacknowledged limit of 1)
        entry.push_report(make_test_input_report(10, 20));
        while entry.reader_state.lock().unacknowledged_reports_count() < 1 {
            fasync::yield_now().await;
        }

        // 2. Push second report, which should be queued in pending_reports because limit is reached
        entry.push_report(make_test_input_report(30, 40));
        fasync::yield_now().await;

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.last_sent_report_stamp().get(), 1);
            assert_eq!(state.pending_reports.len(), 1);
            assert_eq!(state.unacknowledged_reports_count(), 1);
        }

        // 3. Client acknowledges report 1, freeing capacity for report 2 to be delivered
        reader_client.acknowledge_reports(1).await.unwrap();

        while entry.reader_state.lock().last_sent_report_stamp().get() < 2 {
            fasync::yield_now().await;
        }

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.last_sent_report_stamp().get(), 2);
            assert_eq!(state.last_acknowledged_report_stamp.get(), 1);
            assert_eq!(state.pending_reports.len(), 0);
            assert_eq!(state.unacknowledged_reports_count(), 1);
        }
    }

    #[fuchsia::test]
    async fn test_v2_reader_entry_batches_pending_reports_when_limit_freed() {
        let (reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let reader_client = reader_client_end.spawn();
        // Limit of 3 reports
        let entry = V2ReaderEntry::new(1, reader_server_end, 3, || {});

        // 1. Fill limit of 3
        for i in 0..3 {
            entry.push_report(make_test_input_report(i, i));
        }
        while entry.reader_state.lock().unacknowledged_reports_count() < 3 {
            fasync::yield_now().await;
        }

        // 2. Queue 2 more pending reports while limit is full
        entry.push_report(make_test_input_report(100, 100));
        entry.push_report(make_test_input_report(200, 200));
        fasync::yield_now().await;

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.pending_reports.len(), 2);
            assert_eq!(state.unacknowledged_reports_count(), 3);
        }

        // 3. Acknowledge all 3 in-flight reports (freeing 3 slots)
        reader_client.acknowledge_reports(3).await.unwrap();

        // The 2 pending reports should be dispatched together as a batch
        while entry.reader_state.lock().last_sent_report_stamp().get() < 5 {
            fasync::yield_now().await;
        }

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.last_sent_report_stamp().get(), 5);
            assert_eq!(state.last_acknowledged_report_stamp.get(), 3);
            assert_eq!(state.pending_reports.len(), 0);
            assert_eq!(state.unacknowledged_reports_count(), 2);
        }
    }

    #[fuchsia::test]
    async fn test_v2_reader_entry_queue_overflow_drops_oldest() {
        let (_reader_client_end, reader_server_end) =
            fidl_next::fuchsia::create_channel::<fidl_input_report::InputReportsReaderV2>();
        let entry = V2ReaderEntry::new(1, reader_server_end, 1, || {});

        // Send 1st report to exhaust the in-flight limit (limit = 1)
        entry.push_report(make_test_input_report(1, 1));
        while entry.reader_state.lock().unacknowledged_reports_count() < 1 {
            fasync::yield_now().await;
        }

        // Queue MAX_DEVICE_REPORT_COUNT reports
        for i in 0..MAX_DEVICE_REPORT_COUNT {
            entry.push_report(make_test_input_report(i as i64, i as i64));
        }

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.pending_reports.len(), MAX_DEVICE_REPORT_COUNT);
        }

        // Push one more report (touch up at 999, 999), oldest pending report (0, 0)
        // should be dropped.
        entry.push_report(make_test_input_report(999, 999));

        {
            let state = entry.reader_state.lock();
            assert_eq!(state.pending_reports.len(), MAX_DEVICE_REPORT_COUNT);
            let (first_pending, first_stamp) = state.pending_reports.front().unwrap();
            let touch = first_pending.touch.as_ref().unwrap();
            let contacts = touch.contacts.as_ref().unwrap();
            assert_eq!(contacts[0].position_x, Some(1));
            // First report was stamp 1 (sent). Reports 0..MAX_DEVICE_REPORT_COUNT got stamps 2..51.
            // Report 0 (stamp 2) was dropped when report 999 was pushed.
            // So first_stamp should be 3!
            assert_eq!(first_stamp.get(), 3);

            let (last_pending, last_stamp) = state.pending_reports.back().unwrap();
            let last_touch = last_pending.touch.as_ref().unwrap();
            let last_contacts = last_touch.contacts.as_ref().unwrap();
            assert_eq!(last_contacts[0].position_x, Some(999));
            assert_eq!(last_stamp.get(), (MAX_DEVICE_REPORT_COUNT + 2) as u64);
        }
    }
}
