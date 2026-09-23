// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow, format_err};
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use injectable_time::TimeSource;
use log::{error, warn};
use std::cell::RefCell;
use {fidl_fuchsia_feedback as fidl_feedback, fuchsia_async as fasync};

// Name of the crash-report product we're filing against.
const CRASH_PRODUCT_NAME: &str = "FuchsiaDetect";

// CRASH_PROGRAM_NAME serves two purposes:
// 1) It is sent with the crash report. It may show up on the server as
//   "process type".
// 2) The on-device crash reporting program associates this string with the
//   "product" CRASH_PRODUCT_NAME we're requesting to file against, so we
//   only have to send the program name and not the product name with each
//   crash report request.
//   This association is registered via a call to
//   CrashReportingProductRegister.upsert_with_ack().
const CRASH_PROGRAM_NAME: &str = "triage_detect";

#[derive(Debug)]
pub struct SnapshotRequest {
    signature: String,
}

impl SnapshotRequest {
    pub fn new(signature: String) -> SnapshotRequest {
        SnapshotRequest { signature }
    }
}

/// The maximum number of pending crash report requests. This is needed because the FIDL API to file
/// a crash report does not return until the crash report has been fully generated, which can take
/// many seconds. Supporting pending crash reports means Detect can file
/// a new crash report for any other reason within that window, but the CrashReportHandler will
/// handle rate limiting to the CrashReporter service.
const MAX_PENDING_CRASH_REPORTS: usize = 10;

/// A builder for constructing the CrashReportHandler node.
pub struct CrashReportHandlerBuilder<T: TimeSource> {
    proxy: Option<fidl_feedback::CrashReporterProxy>,
    max_pending_crash_reports: usize,
    time_source: T,
}

/// Logs an error message if the passed in `result` is an error.
#[macro_export]
macro_rules! log_if_err {
    ($result:expr, $log_prefix:expr) => {
        if let Err(e) = $result.as_ref() {
            log::error!("{}: {}", $log_prefix, e);
        }
    };
}

impl<T> CrashReportHandlerBuilder<T>
where
    T: TimeSource + 'static,
{
    pub fn new(time_source: T) -> Self {
        Self { time_source, max_pending_crash_reports: MAX_PENDING_CRASH_REPORTS, proxy: None }
    }

    pub async fn build(self) -> Result<CrashReportHandler, Error> {
        // Proxy is only pre-set for tests. If a proxy was not specified,
        // this is a good time to configure for our crash reporting product.
        if self.proxy.is_none() {
            let config_proxy =
                connect_to_protocol::<fidl_feedback::CrashReportingProductRegisterMarker>()?;
            let product_config = fidl_feedback::CrashReportingProduct {
                name: Some(CRASH_PRODUCT_NAME.to_string()),
                ..Default::default()
            };
            config_proxy.upsert_with_ack(CRASH_PROGRAM_NAME, &product_config).await?;
        }
        // Connect to the CrashReporter service if a proxy wasn't specified
        let proxy =
            self.proxy.unwrap_or(connect_to_protocol::<fidl_feedback::CrashReporterMarker>()?);
        Ok(CrashReportHandler::new(proxy, self.time_source, self.max_pending_crash_reports))
    }
}

#[cfg(test)]
impl<T> CrashReportHandlerBuilder<T>
where
    T: TimeSource,
{
    fn with_proxy(mut self, proxy: fidl_feedback::CrashReporterProxy) -> Self {
        self.proxy = Some(proxy);
        self
    }

    fn with_max_pending_crash_reports(mut self, max: usize) -> Self {
        self.max_pending_crash_reports = max;
        self
    }
}

/// CrashReportHandler
/// Triggers a snapshot via FIDL
///
/// Summary: Provides a mechanism for filing crash reports.
///
/// FIDL dependencies:
///     - fuchsia.feedback.CrashReporter: CrashReportHandler uses this protocol to communicate
///       with the CrashReporter service in order to file crash reports.
///     - fuchsia.feedback.CrashReportingProductRegister: CrashReportHandler uses this protocol
///       to communicate with the CrashReportingProductRegister service in order to configure
///       the crash reporting product it will be filing on.
pub struct CrashReportHandler {
    /// The channel to send new crash report requests to the asynchronous crash report sender
    /// future. The maximum pending crash reports are implicitly enforced by the channel length.
    crash_report_sender: RefCell<mpsc::Sender<SnapshotRequest>>,
    channel_size: usize,
    _server_task: fasync::Task<()>,
}

impl CrashReportHandler {
    fn new<T>(proxy: fidl_feedback::CrashReporterProxy, time_source: T, channel_size: usize) -> Self
    where
        T: TimeSource + 'static,
    {
        // Set up the crash report sender that runs asynchronously
        let (channel, receiver) = mpsc::channel(channel_size);
        let server_task = Self::begin_crash_report_sender(proxy, receiver, time_source);
        Self { channel_size, crash_report_sender: RefCell::new(channel), _server_task: server_task }
    }

    /// Handle a FileCrashReport message by sending the specified crash report signature over the
    /// channel to the crash report sender.
    pub fn request_snapshot(&self, request: SnapshotRequest) -> Result<(), Error> {
        // Try to send the crash report signature over the channel. If the channel is full, return
        // an error
        match self.crash_report_sender.borrow_mut().try_send(request) {
            Ok(()) => Ok(()),
            Err(e) if e.is_full() => {
                warn!("Too many crash reports pending: {e}");
                Err(anyhow!("Pending crash reports exceeds max ({})", self.channel_size))
            }
            Err(e) => {
                warn!("Error sending crash report: {e}");
                Err(anyhow!("{e}"))
            }
        }
    }

    /// Spawn a Task that receives crash report signatures over the channel and uses
    /// the proxy to send a File FIDL request to the CrashReporter service with the specified
    /// signatures.
    fn begin_crash_report_sender<T>(
        proxy: fidl_feedback::CrashReporterProxy,
        mut receive_channel: mpsc::Receiver<SnapshotRequest>,
        time_source: T,
    ) -> fasync::Task<()>
    where
        T: TimeSource + 'static,
    {
        fasync::Task::local(async move {
            while let Some(request) = receive_channel.next().await {
                log_if_err!(
                    Self::send_crash_report(&proxy, request, &time_source).await,
                    "Failed to file crash report"
                );
            }
            error!(
                "Crash reporter task ended. Crash reports will no longer be filed. This should not happen."
            )
        })
    }

    /// Send a File request to the CrashReporter service with the specified crash report signature.
    async fn send_crash_report<T: TimeSource>(
        proxy: &fidl_feedback::CrashReporterProxy,
        payload: SnapshotRequest,
        time_source: &T,
    ) -> Result<fidl_feedback::FileReportResults, Error> {
        warn!("Filing crash report, signature '{}'", payload.signature);
        let report = fidl_feedback::CrashReport {
            program_name: Some(CRASH_PROGRAM_NAME.to_string()),
            program_uptime: Some(time_source.now()),
            crash_signature: Some(payload.signature),
            is_fatal: Some(false),
            ..Default::default()
        };

        let result = proxy.file_report(report).await.map_err(|e| format_err!("IPC error: {e}"))?;
        result.map_err(|e| format_err!("Service error: {e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::TryStreamExt;
    use injectable_time::{FakeTime, IncrementingFakeTime};

    /// Tests that the node responds to the FileCrashReport message and that the expected crash
    /// report is received by the CrashReporter service.
    #[fuchsia::test]
    async fn test_crash_report_content() {
        // The crash report signature to use and verify against
        let crash_report_signature = "TestCrashReportSignature";

        // Set up the CrashReportHandler node
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_feedback::CrashReporterMarker>();
        let fake_time = FakeTime::new();
        fake_time.set_ticks(9876);
        let crash_report_handler =
            CrashReportHandlerBuilder::new(fake_time).with_proxy(proxy).build().await.unwrap();

        // File a crash report
        crash_report_handler
            .request_snapshot(SnapshotRequest::new(crash_report_signature.to_string()))
            .unwrap();

        // Verify the fake service receives the crash report with expected data
        if let Ok(Some(fidl_feedback::CrashReporterRequest::FileReport { responder: _, report })) =
            stream.try_next().await
        {
            assert_eq!(
                report,
                fidl_feedback::CrashReport {
                    program_name: Some(CRASH_PROGRAM_NAME.to_string()),
                    program_uptime: Some(9876),
                    crash_signature: Some(crash_report_signature.to_string()),
                    is_fatal: Some(false),
                    ..Default::default()
                }
            );
        } else {
            panic!("Did not receive a crash report");
        }
    }

    /// Tests that the number of pending crash reports is correctly bounded.
    #[fuchsia::test]
    async fn test_crash_report_pending_reports() {
        // Set up the proxy/stream and node outside of the large future used below. This way we can
        // still poll the stream after the future completes.
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fidl_feedback::CrashReporterMarker>();
        let fake_time = IncrementingFakeTime::new(1000, std::time::Duration::from_nanos(1000));
        let crash_report_handler = CrashReportHandlerBuilder::new(fake_time)
            .with_proxy(proxy)
            .with_max_pending_crash_reports(1)
            .build()
            .await
            .unwrap();

        // Set up the CrashReportHandler node. The request stream is never serviced, so when the
        // node makes the FIDL call to file the crash report, the call will block indefinitely.
        // This lets us test the pending crash report counts.

        // The first FileCrashReport should succeed
        assert_matches!(
            crash_report_handler.request_snapshot(SnapshotRequest::new("TestCrash1".to_string())),
            Ok(())
        );

        // The second FileCrashReport should also succeed because since the first is now in
        // progress, this is now the first "pending" report request
        assert_matches!(
            crash_report_handler.request_snapshot(SnapshotRequest::new("TestCrash2".to_string())),
            Ok(())
        );

        // Since the first request has not completed, and there is already one pending request,
        // this request should fail
        assert_matches!(
            crash_report_handler.request_snapshot(SnapshotRequest::new("TestCrash3".to_string())),
            Err(_)
        );

        // Verify the signature of the first crash report
        if let Ok(Some(fidl_feedback::CrashReporterRequest::FileReport { responder, report })) =
            stream.try_next().await
        {
            // Send a reply to allow the node to process the next crash report
            let _ = responder.send(Ok(&fidl_feedback::FileReportResults::default()));
            assert_eq!(
                report,
                fidl_feedback::CrashReport {
                    program_name: Some(CRASH_PROGRAM_NAME.to_string()),
                    program_uptime: Some(1000),
                    crash_signature: Some("TestCrash1".to_string()),
                    is_fatal: Some(false),
                    ..Default::default()
                }
            );
        } else {
            panic!("Did not receive a crash report");
        }

        // Verify the signature of the second crash report
        if let Ok(Some(fidl_feedback::CrashReporterRequest::FileReport { responder, report })) =
            stream.try_next().await
        {
            // Send a reply to allow the node to process the next crash report
            let _ = responder.send(Ok(&fidl_feedback::FileReportResults::default()));
            assert_eq!(
                report,
                fidl_feedback::CrashReport {
                    program_name: Some(CRASH_PROGRAM_NAME.to_string()),
                    program_uptime: Some(2000),
                    crash_signature: Some("TestCrash2".to_string()),
                    is_fatal: Some(false),
                    ..Default::default()
                }
            );
        } else {
            panic!("Did not receive a crash report");
        }
    }
}
