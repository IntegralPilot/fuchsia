// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_input_report::{
    InputReport, InputReportsReaderV2Request, InputReportsReaderV2RequestStream,
};
use futures::{FutureExt as _, StreamExt};
use std::collections::VecDeque;
use std::convert::TryFrom as _;

/// Implements the server side of the `fuchsia.input.report.InputReportsReaderV2`
/// protocol. Used by `input_device::InputDevice`.
pub(super) struct InputReportsReaderV2 {
    pub(super) request_stream: InputReportsReaderV2RequestStream,
    /// FIFO queue of reports to be pushed via `OnInputReports` events.
    pub(super) report_receiver: futures::channel::mpsc::UnboundedReceiver<InputReport>,
    pub(super) max_unacknowledged_reports: u16,
}

impl InputReportsReaderV2 {
    pub(super) async fn into_future(self) -> Result<(), Error> {
        let chunk_size = usize::try_from(fidl_fuchsia_input_report::MAX_DEVICE_REPORT_COUNT)
            .context("converting MAX_DEVICE_REPORT_COUNT to usize")?;
        let mut reports_stream = self.report_receiver.ready_chunks(chunk_size).fuse();
        let control_handle = self.request_stream.control_handle();
        let mut request_stream = self.request_stream.fuse();

        let mut last_report_stamp: u64 = 0;
        let mut last_acknowledged_report_stamp: u64 = 0;
        let mut pending_reports: VecDeque<InputReport> = VecDeque::new();
        let mut reports_done = false;

        loop {
            while let Some(item) = reports_stream.next().now_or_never() {
                match item {
                    Some(batch) if !batch.is_empty() => {
                        pending_reports.extend(batch);
                    }
                    _ => {
                        reports_done = true;
                        break;
                    }
                }
            }

            let unacknowledged = last_report_stamp.saturating_sub(last_acknowledged_report_stamp);
            let max_allowed =
                (self.max_unacknowledged_reports as u64).saturating_sub(unacknowledged) as usize;
            let max_to_send = std::cmp::min(max_allowed, chunk_size);

            if max_to_send > 0 && !pending_reports.is_empty() {
                let take_count = std::cmp::min(pending_reports.len(), max_to_send);
                let reports_batch: Vec<InputReport> = pending_reports.drain(..take_count).collect();
                last_report_stamp += reports_batch.len() as u64;
                control_handle
                    .send_on_input_reports(reports_batch, last_report_stamp)
                    .context("failed to send OnInputReports event")?;
                continue;
            }

            if reports_done && pending_reports.is_empty() {
                break;
            }

            futures::select! {
                request = request_stream.next() => {
                    match request {
                        Some(Ok(InputReportsReaderV2Request::AcknowledgeReports {
                            last_acknowledged_report_stamp: stamp,
                            ..
                        })) => {
                            if stamp > last_acknowledged_report_stamp {
                                last_acknowledged_report_stamp = stamp;
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(anyhow::Error::from(e).context("error on V2 reader stream")),
                        None => {
                            while let Some(item) = reports_stream.next().now_or_never() {
                                match item {
                                    Some(batch) if !batch.is_empty() => {
                                        pending_reports.extend(batch);
                                    }
                                    _ => break,
                                }
                            }
                            if !pending_reports.is_empty() {
                                return Err(format_err!("request_stream terminated with reports still pending"));
                            }
                            break;
                        }
                    }
                }
                reports = reports_stream.next() => {
                    match reports {
                        Some(reports_batch) if !reports_batch.is_empty() => {
                            pending_reports.extend(reports_batch);
                        }
                        _ => {
                            reports_done = true;
                        }
                    }
                }
                complete => break,
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{InputReport, InputReportsReaderV2};
    use anyhow::Error;
    use assert_matches::assert_matches;
    use fidl::endpoints;
    use fidl_fuchsia_input_report::{InputReportsReaderV2Event, InputReportsReaderV2Marker};
    use futures::StreamExt;

    #[fuchsia::test(allow_stalls = false)]
    async fn immediately_resolves_to_ok_when_reports_is_initially_empty() -> Result<(), Error> {
        let (_proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 2 }
                .into_future();
        std::mem::drop(report_sender); // Drop `report_sender` to terminate `report_receiver`.
        assert_matches!(reader_fut.await, Ok(()));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn serves_single_report() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 2 }
                .into_future();

        report_sender
            .unbounded_send(InputReport { event_time: Some(1), ..Default::default() })
            .expect("sending first report");

        let mut event_stream = proxy.take_event_stream();
        let receive_event_fut = async move {
            let event = event_stream.next().await.expect("expected event").expect("fidl error");
            match event {
                InputReportsReaderV2Event::OnInputReports { reports, last_report_stamp } => {
                    assert_eq!(reports.len(), 1);
                    assert_eq!(last_report_stamp, 1);
                    let _ = proxy.acknowledge_reports(last_report_stamp);
                }
                _ => panic!("unexpected event: {:?}", event),
            }
        };

        std::mem::drop(report_sender);
        let (reader_res, _) = futures::join!(reader_fut, receive_event_fut);
        assert_matches!(reader_res, Ok(()));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn preserves_report_order() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut = InputReportsReaderV2 {
            request_stream,
            report_receiver,
            max_unacknowledged_reports: 10,
        }
        .into_future();

        report_sender
            .unbounded_send(InputReport { event_time: Some(100), ..Default::default() })
            .expect("sending first report");
        report_sender
            .unbounded_send(InputReport { event_time: Some(200), ..Default::default() })
            .expect("sending second report");

        let mut event_stream = proxy.take_event_stream();
        let receive_event_fut = async move {
            let mut received_times = Vec::new();
            while let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                for r in reports {
                    received_times.push(r.event_time);
                }
                let _ = proxy.acknowledge_reports(last_report_stamp);
                if received_times.len() >= 2 {
                    break;
                }
            }
            received_times
        };

        std::mem::drop(report_sender);
        let (reader_res, times) = futures::join!(reader_fut, receive_event_fut);
        assert_matches!(reader_res, Ok(()));
        assert_eq!(times, vec![Some(100), Some(200)]);
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn resolves_to_err_when_client_terminates_with_pending_reports() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        // Limit max unacknowledged to 1, but send 5 reports so reports remain pending.
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 1 }
                .into_future();

        for i in 0..5 {
            report_sender
                .unbounded_send(InputReport { event_time: Some(i), ..Default::default() })
                .expect("sending report");
        }
        std::mem::drop(report_sender);

        std::mem::drop(proxy); // Drop client proxy without acking.
        assert_matches!(reader_fut.await, Err(_));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn flow_control_enforces_max_unacknowledged_reports_limit() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        // Limit max unacknowledged to 3.
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 3 }
                .into_future();

        for i in 0..10 {
            report_sender
                .unbounded_send(InputReport { event_time: Some(i), ..Default::default() })
                .expect("sending report");
        }

        let mut event_stream = proxy.take_event_stream();
        let receive_fut = async move {
            let mut total_received = 0;
            // 1st batch: should get at most 3 reports
            if let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                total_received += reports.len();
                assert_eq!(total_received, 3);
                assert_eq!(last_report_stamp, 3);

                // Acknowledge stamp 2 (2 reports acknowledged, 1 still unacknowledged)
                proxy.acknowledge_reports(2).expect("ack");
            } else {
                panic!("expected first batch");
            }

            // 2nd batch: should get 2 reports (allowed: 3 - 1 = 2)
            if let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                total_received += reports.len();
                assert_eq!(total_received, 5);
                assert_eq!(last_report_stamp, 5);

                // Acknowledge stamp 5 (all 5 acknowledged)
                proxy.acknowledge_reports(5).expect("ack");
            } else {
                panic!("expected second batch");
            }

            // 3rd batch: should get remaining 5 in chunks up to limit 3
            if let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                total_received += reports.len();
                assert_eq!(total_received, 8);
                assert_eq!(last_report_stamp, 8);

                // Acknowledge stamp 8
                proxy.acknowledge_reports(8).expect("ack");
            } else {
                panic!("expected third batch");
            }

            // 4th batch: should get last 2 reports
            if let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                total_received += reports.len();
                assert_eq!(total_received, 10);
                assert_eq!(last_report_stamp, 10);

                // Acknowledge stamp 10
                proxy.acknowledge_reports(10).expect("ack");
            } else {
                panic!("expected fourth batch");
            }

            total_received
        };

        std::mem::drop(report_sender);
        let (reader_res, total) = futures::join!(reader_fut, receive_fut);
        assert_matches!(reader_res, Ok(()));
        assert_eq!(total, 10);
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handles_stale_and_out_of_order_acknowledgments() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 5 }
                .into_future();

        for i in 0..5 {
            report_sender
                .unbounded_send(InputReport { event_time: Some(i), ..Default::default() })
                .expect("sending report");
        }

        let mut event_stream = proxy.take_event_stream();
        let receive_fut = async move {
            let event = event_stream.next().await.expect("expected event").expect("fidl error");
            match event {
                InputReportsReaderV2Event::OnInputReports { reports, last_report_stamp } => {
                    assert_eq!(reports.len(), 5);
                    assert_eq!(last_report_stamp, 5);

                    // Send higher ack, then stale lower ack, then duplicate
                    proxy.acknowledge_reports(4).expect("ack");
                    proxy.acknowledge_reports(2).expect("stale ack"); // ignored
                    proxy.acknowledge_reports(4).expect("duplicate ack"); // ignored
                    proxy.acknowledge_reports(5).expect("final ack");
                }
                _ => panic!("unexpected event"),
            }
        };

        std::mem::drop(report_sender);
        let (reader_res, _) = futures::join!(reader_fut, receive_fut);
        assert_matches!(reader_res, Ok(()));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn preserves_order_under_large_burst_and_chunking() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut = InputReportsReaderV2 {
            request_stream,
            report_receiver,
            max_unacknowledged_reports: 100,
        }
        .into_future();

        const NUM_REPORTS: usize = 100;
        for i in 0..NUM_REPORTS {
            report_sender
                .unbounded_send(InputReport { event_time: Some(i as i64), ..Default::default() })
                .expect("sending report");
        }

        let mut event_stream = proxy.take_event_stream();
        let receive_fut = async move {
            let mut received_ids = Vec::new();
            while let Some(Ok(InputReportsReaderV2Event::OnInputReports {
                reports,
                last_report_stamp,
            })) = event_stream.next().await
            {
                // Verify batch size never exceeds MAX_DEVICE_REPORT_COUNT (50)
                assert!(
                    reports.len() <= fidl_fuchsia_input_report::MAX_DEVICE_REPORT_COUNT as usize
                );
                for r in reports {
                    received_ids.push(r.event_time.unwrap());
                }
                proxy.acknowledge_reports(last_report_stamp).expect("ack");
                if received_ids.len() == NUM_REPORTS {
                    break;
                }
            }
            received_ids
        };

        std::mem::drop(report_sender);
        let (reader_res, ids) = futures::join!(reader_fut, receive_fut);
        assert_matches!(reader_res, Ok(()));
        let expected: Vec<i64> = (0..NUM_REPORTS as i64).collect();
        assert_eq!(ids, expected);
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn client_disconnect_with_zero_pending_succeeds() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 5 }
                .into_future();

        report_sender
            .unbounded_send(InputReport { event_time: Some(42), ..Default::default() })
            .expect("send");
        std::mem::drop(report_sender);

        let receive_fut = async move {
            let mut event_stream = proxy.take_event_stream();
            let event = event_stream.next().await.expect("event").expect("fidl");
            if let InputReportsReaderV2Event::OnInputReports { last_report_stamp, .. } = event {
                proxy.acknowledge_reports(last_report_stamp).expect("ack");
            }
            std::mem::drop(proxy); // Drop client proxy cleanly after all consumed.
        };

        let (reader_res, _) = futures::join!(reader_fut, receive_fut);
        assert_matches!(reader_res, Ok(()));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn handles_future_acknowledgment_stamps_without_overflow() -> Result<(), Error> {
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputReportsReaderV2Marker>();
        let (report_sender, report_receiver) = futures::channel::mpsc::unbounded::<InputReport>();
        let reader_fut =
            InputReportsReaderV2 { request_stream, report_receiver, max_unacknowledged_reports: 2 }
                .into_future();

        report_sender
            .unbounded_send(InputReport { event_time: Some(1), ..Default::default() })
            .expect("send 1");

        let receive_fut = async move {
            let mut event_stream = proxy.take_event_stream();
            let _ = event_stream.next().await.expect("event").expect("fidl");
            // Spurious futuristic ack stamp
            proxy.acknowledge_reports(9999).expect("ack future");

            // Now send another report to check reader handles it without underflow
            report_sender
                .unbounded_send(InputReport { event_time: Some(2), ..Default::default() })
                .expect("send 2");
            std::mem::drop(report_sender);

            let _ = event_stream.next().await.expect("event 2").expect("fidl");
            proxy.acknowledge_reports(10000).expect("ack future 2");
        };

        let (reader_res, _) = futures::join!(reader_fut, receive_fut);
        assert_matches!(reader_res, Ok(()));
        Ok(())
    }
}
