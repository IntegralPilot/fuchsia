// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::ComponentError;
use crate::eval::EvaluationContext;
use crate::spec::{Accessor, ProgramSpec};
use diagnostics_reader::{ArchiveReader, RetryConfig};
use fidl::endpoints::ServerEnd;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use futures::channel::oneshot;
use futures::future::{abortable, select, Either};
use futures::{pin_mut, select, stream, FutureExt, StreamExt, TryStreamExt};
use log::warn;
use std::sync::Arc;
use zx::{MonotonicDuration, MonotonicInstant, Status};
use {
    fidl_fuchsia_component_runner as fcrunner, fidl_fuchsia_diagnostics as fdiagnostics,
    fidl_fuchsia_io as fio, fidl_fuchsia_test as ftest, fuchsia_async as fasync,
};

const NANOS_IN_SECONDS: f64 = 1_000_000_000.0;
const DEFAULT_PARALLEL: u16 = 1;

/// Output a log for the test. Automatically prepends the current monotonic time.
macro_rules! test_stdout {
    ($logger:ident, $format:literal) => {
        let formatted_with_time = format!(
            "[{:05.3}] {}\n",
            (MonotonicInstant::get().into_nanos() as f64 / NANOS_IN_SECONDS),
            $format
        );
        $logger.write(formatted_with_time.as_bytes()).ok()
    };
    ($logger:ident, $format:literal, $($content:expr),*) => {
        let formatted = format!($format, $($content, )*);
        let formatted_with_time = format!(
            "[{:05.3}] {}\n",
            (MonotonicInstant::get().into_nanos() as f64 / NANOS_IN_SECONDS),
            formatted
        );
        $logger.write(formatted_with_time.as_bytes()).ok()
    };
}

/// Implements `fuchsia.test.Suite` and runs provided test.
pub struct TestServer {
    spec: ProgramSpec,
    controller: ServerEnd<fcrunner::ComponentControllerMarker>,
    out_channel: ServerEnd<fio::DirectoryMarker>,
}

impl TestServer {
    /// Creates new test server.
    pub fn new(
        start_info: fcrunner::ComponentStartInfo,
        controller: ServerEnd<fcrunner::ComponentControllerMarker>,
    ) -> Result<Self, ComponentError> {
        match ProgramSpec::try_from(
            start_info.program.ok_or(ComponentError::MissingRequiredKey("program"))?,
        ) {
            Ok(spec) => Ok(Self {
                spec,
                controller,
                out_channel: start_info
                    .outgoing_dir
                    .ok_or(ComponentError::MissingOutgoingChannel)?,
            }),
            Err(e) => {
                warn!("Error loading spec: {}", e);
                controller.close_with_epitaph(Status::INVALID_ARGS).unwrap_or_default();
                Err(e)
            }
        }
    }

    /// Run the individual named test case from the given ProgramSpec.
    ///
    /// Output logs are written to the given socket.
    ///
    /// Returns true on pass and false on failure.
    async fn run_case(spec: &ProgramSpec, case: &str, logs: zx::Socket) -> bool {
        let case = match spec.cases.get(case) {
            Some(case) => case,
            None => {
                test_stdout!(logs, "Failed to find test case");
                return false;
            }
        };

        let svc = match spec.accessor {
            Accessor::All => "/svc/fuchsia.diagnostics.ArchiveAccessor",
            Accessor::Feedback => "/svc/fuchsia.diagnostics.ArchiveAccessor.feedback",
            Accessor::Legacy => "/svc/fuchsia.diagnostics.ArchiveAccessor.legacy_metrics",
        };

        test_stdout!(logs, "Reading `{}` from `{}`", case.key, svc);

        let context = match EvaluationContext::try_from(case) {
            Ok(c) => c,
            Err(e) => {
                test_stdout!(logs, "Failed to set up evaluation: {:?}\n", e);
                return false;
            }
        };

        let end_time =
            MonotonicInstant::get() + MonotonicDuration::from_seconds(spec.timeout_seconds);

        while end_time > MonotonicInstant::get() {
            let start_time = MonotonicInstant::get();

            let proxy = match client::connect_to_protocol_at_path::<
                fdiagnostics::ArchiveAccessorMarker,
            >(&svc)
            {
                Ok(p) => p,
                Err(e) => {
                    test_stdout!(logs, "Failed to connect to accessor: {:?}", e);
                    return false;
                }
            };

            test_stdout!(logs, "Attempting read");

            match ArchiveReader::inspect()
                .retry(RetryConfig::never())
                .with_archive(proxy)
                .with_timeout(end_time - start_time)
                .add_selector(case.selector.as_str())
                .snapshot_raw::<serde_json::Value>()
                .await
            {
                Ok(json) => {
                    match context.run(&serde_json::to_string_pretty(&json).unwrap_or_default()) {
                        Ok(_) => {
                            test_stdout!(logs, "Test case passed");
                            return true;
                        }
                        Err(e) => {
                            test_stdout!(logs, "Test case attempt failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    test_stdout!(logs, "Failed to obtain data: {}", e);
                }
            }

            let sleep_time = MonotonicDuration::from_seconds(1);

            if end_time - MonotonicInstant::get() >= MonotonicDuration::from_seconds(0) {
                test_stdout!(
                    logs,
                    "Retrying after {}s, timeout after {}s",
                    sleep_time.into_seconds(),
                    (end_time - MonotonicInstant::get()).into_seconds()
                );
                fasync::Timer::new(MonotonicInstant::after(sleep_time)).await;
            }
        }

        false
    }

    pub async fn execute(self) {
        let spec = Arc::new(self.spec);
        let controller = self.controller;

        let mut fs = ServiceFs::new_local();
        let (done_sender, done_fut) = oneshot::channel::<()>();
        let done_fut = done_fut.shared();
        fs.dir("svc").add_fidl_service(move |mut stream: ftest::SuiteRequestStream| {
            let spec = spec.clone();
            let mut done_fut = done_fut.clone();
            fasync::Task::spawn(async move {
                // Listen either for the next value form the stream, or the done signal.
                while let Ok(Some(req)) = select! {
                next = stream.try_next() => next,
                _ = done_fut => Ok(None) }
                {
                    match req {
                        ftest::SuiteRequest::GetTests { iterator, .. } => {
                            let mut names = spec.test_names().into_iter().map(|n| ftest::Case {
                                name: Some(n),
                                enabled: Some(true),
                                ..Default::default()
                            });
                            let mut done_fut = done_fut.clone();
                            fasync::Task::spawn(async move {
                                let mut stream = iterator.into_stream();
                                while let Ok(Some(req)) = select! {
                                next = stream.try_next() => next,
                                _ = done_fut => Ok(None)}
                                {
                                    match req {
                                        ftest::CaseIteratorRequest::GetNext {
                                            responder, ..
                                        } => {
                                            // Continually drain the |names| iterator on each
                                            // call.
                                            responder
                                                .send(&names.by_ref().collect::<Vec<_>>())
                                                .unwrap_or_default();
                                        }
                                    }
                                }
                            })
                            .detach();
                        }
                        ftest::SuiteRequest::Run { tests, options, listener, .. } => {
                            let proxy = listener.into_proxy();

                            let mut tasks = vec![];
                            let parallel = options.parallel.unwrap_or(DEFAULT_PARALLEL);
                            for test in tests.into_iter() {
                                let spec = spec.clone();
                                let proxy = proxy.clone();
                                tasks.push(async move {
                                    let (stdout_end, stdout) = zx::Socket::create_stream();

                                    let name = test.name.clone().unwrap_or_default();

                                    let (case_listener_proxy, case_listener) =
                                        fidl::endpoints::create_proxy::<ftest::CaseListenerMarker>(
                                        );

                                    proxy
                                        .on_test_case_started(
                                            &test,
                                            ftest::StdHandles {
                                                out: Some(stdout_end),
                                                ..Default::default()
                                            },
                                            case_listener,
                                        )
                                        .expect("on_test_case_started failed");

                                    let status =
                                        match TestServer::run_case(&spec, &name, stdout).await {
                                            true => ftest::Status::Passed,
                                            false => ftest::Status::Failed,
                                        };

                                    let result = ftest::Result_ {
                                        status: Some(status),
                                        ..Default::default()
                                    };

                                    case_listener_proxy
                                        .finished(&result)
                                        .expect("on_test_case_finished failed");
                                });
                            }
                            let done_fut = done_fut.clone();
                            fasync::Task::spawn(async move {
                                let proxy = proxy.clone();

                                let chunked_tasks_fut = stream::iter(tasks.into_iter())
                                    .buffered(parallel.into())
                                    .collect::<()>();

                                // If all tasks finished before abort, report OnFinished to the
                                // listener.
                                match select(done_fut, chunked_tasks_fut).await {
                                    Either::Right(_) => {
                                        proxy.on_finished().ok();
                                    }
                                    _ => {}
                                }
                            })
                            .detach();
                        }
                    }
                }
            })
            .detach();
        });

        if let Err(e) = fs.serve_connection(self.out_channel) {
            warn!("Failed to serve connection for child component {:?}", e);
        }

        let (fut, abort_handle) = abortable(fs.collect::<()>());

        let controller_fut = async move {
            let mut stream = controller.into_stream();
            let mut done_sender = Some(done_sender);
            while let Ok(Some(request)) = stream.try_next().await {
                match request {
                    fcrunner::ComponentControllerRequest::Stop { .. }
                    | fcrunner::ComponentControllerRequest::Kill { .. } => {
                        if let Some(done_sender) = done_sender.take() {
                            done_sender.send(()).ok();
                        }
                        abort_handle.abort();
                    }
                    fcrunner::ComponentControllerRequest::_UnknownMethod { .. } => (),
                }
            }
        };

        pin_mut!(fut);
        pin_mut!(controller_fut);

        select(fut, controller_fut).await;
    }
}
