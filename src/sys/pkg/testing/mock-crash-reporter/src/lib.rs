// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_feedback::{
    CrashReporterMarker, CrashReporterProxy, CrashReporterRequestStream, FileReportResults,
    FilingError,
};
use fuchsia_async::Task;
use futures::TryStreamExt;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::prelude::*;
use std::sync::Arc;

pub use fidl_fuchsia_feedback::CrashReport;

/// A call hook that can be used to inject responses into the CrashReporter service.
pub trait Hook: Send + Sync {
    /// Describes what the file_report call will return.
    fn file_report(
        &self,
        report: CrashReport,
    ) -> BoxFuture<'static, Result<FileReportResults, FilingError>>;
}

impl<F> Hook for F
where
    F: Fn(CrashReport) -> Result<FileReportResults, FilingError> + Send + Sync,
{
    fn file_report(
        &self,
        report: CrashReport,
    ) -> BoxFuture<'static, Result<FileReportResults, FilingError>> {
        future::ready(self(report)).boxed()
    }
}

pub struct MockCrashReporterService {
    call_hook: Box<dyn Hook>,
}

impl MockCrashReporterService {
    /// Creates a new MockCrashReporterService with a given callback to run per call to the service.
    pub fn new(hook: impl Hook + 'static) -> Self {
        Self { call_hook: Box::new(hook) }
    }

    /// Spawns an `fasync::Task` which serves fuchsia.feedback/CrashReporter.
    pub fn spawn_crash_reporter_service(self: Arc<Self>) -> (CrashReporterProxy, Task<()>) {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<CrashReporterMarker>();

        let task = Task::spawn(self.run_crash_reporter_service(stream));

        (proxy, task)
    }

    /// Serves fuchsia.feedback/CrashReporter.FileReport requests on the given request stream.
    pub async fn run_crash_reporter_service(
        self: Arc<Self>,
        mut stream: CrashReporterRequestStream,
    ) {
        while let Some(event) = stream.try_next().await.expect("received CrashReporter request") {
            match event {
                fidl_fuchsia_feedback::CrashReporterRequest::FileReport { report, responder } => {
                    let res = self.call_hook.file_report(report).await;
                    match res {
                        Err(_) => responder.send(Err(FilingError::InvalidArgsError)).unwrap(),
                        Ok(_) => responder.send(Ok(&FileReportResults::default())).unwrap(),
                    }
                }
            }
        }
    }
}

/// Hook that can be used to yield control of the `file_report` call to the caller. The caller can
/// control when `file_report` calls complete by pulling from the mpsc::Receiver.
pub struct ThrottleHook {
    file_response: Result<FileReportResults, FilingError>,
    sender: Arc<Mutex<mpsc::Sender<CrashReport>>>,
}

impl ThrottleHook {
    pub fn new(
        file_response: Result<FileReportResults, FilingError>,
    ) -> (Self, mpsc::Receiver<CrashReport>) {
        // We deliberately give a capacity of 1 so that the caller must pull from the
        // receiver in order to unblock the `file_report` call.
        let (sender, recv) = mpsc::channel(0);
        (Self { file_response, sender: Arc::new(Mutex::new(sender)) }, recv)
    }
}
impl Hook for ThrottleHook {
    fn file_report(
        &self,
        report: CrashReport,
    ) -> BoxFuture<'static, Result<FileReportResults, FilingError>> {
        let sender = Arc::clone(&self.sender);
        let file_response = self.file_response.clone();

        async move {
            sender.lock().await.send(report).await.unwrap();
            file_response
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[fuchsia::test]
    async fn test_mock_crash_reporter() {
        let mock = Arc::new(MockCrashReporterService::new(|_| Ok(FileReportResults::default())));
        let (proxy, _server) = mock.spawn_crash_reporter_service();

        let file_result = proxy.file_report(CrashReport::default()).await.expect("made fidl call");

        assert_eq!(file_result, Ok(FileReportResults::default()));
    }

    #[fuchsia::test]
    async fn test_mock_crash_reporter_fails() {
        let mock = Arc::new(MockCrashReporterService::new(|_| Err(FilingError::InvalidArgsError)));
        let (proxy, _server) = mock.spawn_crash_reporter_service();

        let file_result = proxy.file_report(CrashReport::default()).await.expect("made fidl call");

        assert_eq!(file_result, Err(FilingError::InvalidArgsError));
    }

    #[fuchsia::test]
    async fn test_mock_crash_reporter_with_external_state() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);
        let mock = Arc::new(MockCrashReporterService::new(move |_| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            Ok(FileReportResults::default())
        }));
        let (proxy, _server) = mock.spawn_crash_reporter_service();

        let file_result = proxy.file_report(CrashReport::default()).await.expect("made fidl call");

        assert_eq!(file_result, Ok(FileReportResults::default()));
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }
}
