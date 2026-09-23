// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_feedback as fidl_feedback;
use fidl_fuchsia_feedback::{
    Annotation, CrashReporterMarker, CrashReporterProxy, FileReportResults,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::stream::StreamExt;
use std::cell::RefCell;
use std::rc::Rc;

#[macro_export]
macro_rules! send_report {
    // Send a crash report with the provided RecoveryError
    // Option<Rc<CrashReporter>>, RecoveryError
    ($rpt:expr,$error:expr) => {
        match $rpt {
            Some(reporter) => {
                if let Err(error) = reporter.file_crash_report($error, None).await {
                    println!("Failed to send crash report: {}", error);
                }
            }
            None => {
                println!("Crash reporter not available.");
            }
        }
    };
    // Send a crash report with the provided RecoveryError and additional error context
    // Option<Rc<CrashReporter>>, RecoveryError, Error
    ($rpt:expr,$error:expr,$ctx:expr) => {
        match $rpt {
            Some(reporter) => {
                if let Err(error) = reporter.file_crash_report($error, Some($ctx.to_string())).await
                {
                    println!("Failed to send crash report: {}", error);
                }
            }
            None => {
                println!("Crash reporter not available.");
            }
        }
    };
}

#[derive(thiserror::Error, Debug)]
pub enum RecoveryError {
    #[error("fuchsia-recovery-generic-error")]
    GenericError(),

    #[error("fuchsia-recovery-ota-failure-error")]
    OtaFailureError(),

    #[error("fuchsia-recovery-factory-reset-policy-failure")]
    FdrPolicyError(),

    #[error("fuchsia-recovery-factory-reset-failure")]
    FdrResetError(),

    #[error("fuchsia-recovery-wifi-connection-error")]
    WifiConnectionError(),

    #[error("fuchsia-recovery-wifi-connection-success")]
    WifiConnectionSuccess(),

    #[error("fuchsia-recovery-reports-exceed-limit")]
    OutOfSpace(),

    #[cfg(test)]
    #[error("{}", .0)]
    TestingError(String),
}

const SIGNATURE_MAX_LENGTH: usize = 128;
const ANNOTATION_MAX_LENGTH: usize = 1024;
const MAX_PENDING_CRASH_REPORTS: usize = 5;

type ProxyFn = Box<dyn Fn() -> Result<CrashReporterProxy, Error>>;

/// A builder for setting up a crash reporter.
pub struct CrashReportBuilder {
    proxy_fn: ProxyFn,
    max_pending_crash_reports: usize,
}

impl CrashReportBuilder {
    pub fn new() -> Self {
        Self {
            proxy_fn: Box::new(default_proxy_fn),
            max_pending_crash_reports: MAX_PENDING_CRASH_REPORTS,
        }
    }

    #[cfg(test)]
    pub fn with_proxy_fn(mut self, proxy: ProxyFn) -> Self {
        self.proxy_fn = proxy;
        self
    }

    #[cfg(test)]
    pub fn with_max_pending_crash_reports(mut self, max: usize) -> Self {
        self.max_pending_crash_reports = max;
        self
    }

    /// Set up the crash report sender that runs asynchronously
    pub fn build(self) -> Result<Rc<CrashReporter>, Error> {
        let (channel, receiver) = mpsc::channel(self.max_pending_crash_reports);
        CrashReporter::begin_crash_report_sender(self.proxy_fn, receiver);
        Ok(Rc::new(CrashReporter { crash_report_sender: RefCell::new(channel) }))
    }
}

pub fn default_proxy_fn() -> Result<CrashReporterProxy, Error> {
    connect_to_protocol::<CrashReporterMarker>()
}

pub struct CrashReporter {
    /// The channel to send new crash report requests to the async crash report sender.
    crash_report_sender: RefCell<mpsc::Sender<ErrorReportMessage>>,
}

pub struct ErrorReportMessage {
    error: RecoveryError,
    context: Option<String>,
}

impl CrashReporter {
    const DEFAULT_PROGRAM_NAME: &'static str = "recovery";

    /// Attempt to send a crash report with the given error and an optional context.
    pub async fn file_crash_report(
        &self,
        error: RecoveryError,
        context: Option<String>,
    ) -> Result<(), RecoveryError> {
        let message = ErrorReportMessage { error, context };
        match self.crash_report_sender.borrow_mut().try_send(message) {
            Ok(()) => Ok(()),
            Err(e) if e.is_full() => Err(RecoveryError::OutOfSpace()),
            Err(_) => Err(RecoveryError::GenericError()),
        }
    }

    /// Spawn an infinite future that receives crash report signatures over the channel and uses the
    /// proxy to send a File FIDL request to the CrashReporter service with the specified
    /// signatures.
    fn begin_crash_report_sender(
        proxy_fn: ProxyFn,
        mut receive_channel: mpsc::Receiver<ErrorReportMessage>,
    ) {
        fasync::Task::local(async move {
            while let Some(msg) = receive_channel.next().await {
                let ctx_string = match msg.context {
                    Some(ctx) => ctx,
                    None => "".to_string(),
                };
                match Self::send_crash_report(&proxy_fn, msg.error, ctx_string).await {
                    Err(e) => eprintln!("Failed to send crash report: {:?}", e),
                    Ok(_) => (),
                }
            }
        })
        .detach();
    }

    /// Send a File request to the CrashReporter service with the specified crash report signature.
    async fn send_crash_report(
        proxy_fn: &ProxyFn,
        error: RecoveryError,
        context: String,
    ) -> Result<FileReportResults, Error> {
        let mut signature = error.to_string();
        let mut ctx = context.clone();
        ctx.truncate(ANNOTATION_MAX_LENGTH);
        signature.truncate(SIGNATURE_MAX_LENGTH);
        let report = fidl_feedback::CrashReport {
            program_name: Some(CrashReporter::DEFAULT_PROGRAM_NAME.to_string()),
            crash_signature: Some(signature),
            annotations: Some(vec![Annotation { key: "context".into(), value: ctx }]),
            is_fatal: Some(false),
            ..Default::default()
        };
        let result =
            proxy_fn()?.file_report(report).await.map_err(|e| format_err!("IPC error: {}", e))?;
        result.map_err(|e| format_err!("Service error: {:?}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::TryStreamExt;

    // Generate a string with a specific size for testing.
    fn gen_string(c: char, length: usize) -> String {
        let mut count: usize = 0;
        let mut res = String::new();
        loop {
            res.push(c);
            count += 1;
            if count == length {
                return res;
            }
        }
    }

    #[fuchsia::test]
    async fn test_crash_report_content() {
        let received_error = RecoveryError::FdrResetError();

        // Set up a proxy to receive error messages
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_feedback::CrashReporterMarker,
        >();

        let crash_reporter = CrashReportBuilder::new()
            .with_proxy_fn(Box::new(move || Ok(proxy.clone())))
            .build()
            .unwrap();

        crash_reporter
            .file_crash_report(received_error, Some("test context".into()))
            .await
            .unwrap();

        // Verify the fake service receives the crash report with expected data
        if let Ok(Some(fidl_feedback::CrashReporterRequest::FileReport { responder: _, report })) =
            stream.try_next().await
        {
            assert_eq!(
                report,
                fidl_feedback::CrashReport {
                    program_name: Some("recovery".to_string()),
                    crash_signature: Some("fuchsia-recovery-factory-reset-failure".to_string()),
                    is_fatal: Some(false),
                    annotations: Some(vec![Annotation {
                        key: "context".into(),
                        value: "test context".into()
                    },]),
                    ..Default::default()
                }
            );
        } else {
            panic!("Did not receive a crash report");
        }
    }

    #[fuchsia::test]
    async fn test_crash_report_string_limits() {
        let signature = gen_string('A', SIGNATURE_MAX_LENGTH + 10);
        let context = gen_string('Z', ANNOTATION_MAX_LENGTH + 10);
        let received_error = RecoveryError::TestingError(signature);

        // Set up a proxy to receive error messages
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_feedback::CrashReporterMarker,
        >();

        let crash_reporter = CrashReportBuilder::new()
            .with_proxy_fn(Box::new(move || Ok(proxy.clone())))
            .build()
            .unwrap();

        crash_reporter.file_crash_report(received_error, Some(context)).await.unwrap();

        // Verify the fake service receives the crash report with expected data
        if let Ok(Some(fidl_feedback::CrashReporterRequest::FileReport { responder: _, report })) =
            stream.try_next().await
        {
            assert_eq!(
                report,
                fidl_feedback::CrashReport {
                    program_name: Some("recovery".to_string()),
                    crash_signature: Some(gen_string('A', SIGNATURE_MAX_LENGTH)),
                    is_fatal: Some(false),
                    annotations: Some(vec![Annotation {
                        key: "context".into(),
                        value: gen_string('Z', ANNOTATION_MAX_LENGTH)
                    },]),
                    ..Default::default()
                }
            );
        } else {
            panic!("Did not receive a crash report");
        }
    }

    #[test]
    fn test_crash_pending_reports() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, _stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_feedback::CrashReporterMarker,
        >();

        let crash_reporter = CrashReportBuilder::new()
            .with_proxy_fn(Box::new(move || Ok(proxy.clone())))
            .with_max_pending_crash_reports(1)
            .build()
            .unwrap();

        // The request stream is never serviced here, so we can verify the number of pending requests.
        exec.run_singlethreaded(async {
            // Ensure the first report is successfully queued
            assert_matches!(
                crash_reporter.file_crash_report(RecoveryError::FdrResetError(), None).await,
                Ok(())
            );

            // Ensure the second report is queued as the first is being processed.
            assert_matches!(
                crash_reporter.file_crash_report(RecoveryError::FdrResetError(), None).await,
                Ok(())
            );

            // Ensure the third report is rejected due to the maximum pending limit.
            assert_matches!(
                crash_reporter.file_crash_report(RecoveryError::FdrResetError(), None).await,
                Err(RecoveryError::OutOfSpace())
            );
        });
    }
}
