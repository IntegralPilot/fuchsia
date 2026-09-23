// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants::TEST_ROOT_REALM_NAME;
use crate::error::DebugAgentError;
use crate::run_events::SuiteEvents;
use fidl_fuchsia_debugger as fdbg;
use fidl_fuchsia_test_manager as ftest_manager;
use ftest_manager::LaunchError;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::channel::mpsc;
use futures::future::RemoteHandle;
use futures::prelude::*;

pub(crate) struct ThreadBacktraceInfo {
    pub thread: u64,
    pub backtrace: String,
}

pub(crate) struct DebugAgent {
    proxy: fdbg::DebugAgentProxy,
    event_stream_processor_handle: Option<futures::future::RemoteHandle<()>>,
    takeable_fatal_exception_recevier: Option<mpsc::Receiver<ThreadBacktraceInfo>>,
}

impl DebugAgent {
    // Creates a new `DebugAgent` and starts processing events received via the connection.
    // The new `DebugAgent` maintains the connection until dropped.
    pub(crate) async fn new() -> Result<Self, DebugAgentError> {
        Self::from_launcher_proxy(
            connect_to_protocol::<fdbg::LauncherMarker>()
                .map_err(|e| DebugAgentError::ConnectToLauncher(e))?,
        )
        .await
    }

    // Creates a new `DebugAgent` from a launcher proxy and starts processing events received
    // via the connection. The new `DebugAgent` maintains the connection until dropped.
    async fn from_launcher_proxy(
        launcher_proxy: fdbg::LauncherProxy,
    ) -> Result<Self, DebugAgentError> {
        let (proxy, agent_server_end) = fidl::endpoints::create_proxy();

        launcher_proxy
            .launch(
                agent_server_end,
                &fdbg::LaunchOptions { monitor_root_job: Some(false), ..Default::default() },
            )
            .await
            .map_err(|e| DebugAgentError::LaunchLocal(e))?
            .map_err(|zx_status| DebugAgentError::LaunchResponse(zx_status))?;

        // Attach to all test realms using a recursive, job-only filter. This will instruct
        // debug_agent to monitor each test realm for unhandled exceptions, and will also mean that
        // `get_process_info` will iterate over all processes in all active test realms unless
        // further filtered. debug_agent will **not** create exception_channels on any entity other
        // than the root job of each test realm. See the DebugAgent protocol documentation for
        // further details.
        let _ = proxy
            .attach_to(
                TEST_ROOT_REALM_NAME,
                fdbg::FilterType::MonikerSuffix,
                &fdbg::FilterOptions {
                    job_only: Some(true),
                    recursive: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(|e| DebugAgentError::AttachToTestsLocal(e))?
            .map_err(|e| DebugAgentError::AttachToTestsResponse(e))?;

        let (fatal_exception_sender, fatal_exception_receiver) = mpsc::channel(1024);
        let event_stream_processor_handle =
            Self::begin_process_event_stream(proxy.take_event_stream(), fatal_exception_sender);

        Ok(Self {
            proxy,
            event_stream_processor_handle: Some(event_stream_processor_handle),
            takeable_fatal_exception_recevier: Some(fatal_exception_receiver),
        })
    }

    // Takes the fatal exception receiver. This function panics if the receiver has already
    // been taken.
    pub(crate) fn take_fatal_exception_receiver(&mut self) -> mpsc::Receiver<ThreadBacktraceInfo> {
        self.takeable_fatal_exception_recevier.take().unwrap()
    }

    // Stops sending fatal exceptions and closes the channel of fatal exceptions.
    pub(crate) fn stop_sending_fatal_exceptions(&mut self) {
        self.event_stream_processor_handle = None;
    }

    // Reports backtraces from all attached processes. This function executes in non-trivial
    // time, but won't block long-term if the debug agent server is working properly.
    pub(crate) async fn report_all_backtraces(
        &self,
        test_url: Option<&str>,
        mut event_sender: mpsc::Sender<Result<SuiteEvents, LaunchError>>,
    ) {
        let (client_end, mut server_end) = fidl::handle::fuchsia_handles::Socket::create_stream();
        event_sender.send(Ok(SuiteEvents::suite_stderr(client_end).into())).await.unwrap();

        let mut maybe_filter: Option<fdbg::Filter> = None;
        if let Some(test_url) = test_url {
            // In the case of timeouts, we get the test's root realm URL.
            //
            // TODO(https://fxbug.dev/454904881): This won't handle concurrent runs of the same
            // test suite very well, since they share the same package name, the filter will match
            // all of them even if only one timed out. We'll need a more fine grained filter
            // mechanism (job, moniker, etc) to filter to just the test realm we care about.
            maybe_filter = Some(fdbg::Filter {
                pattern: test_url.to_string(),
                type_: fdbg::FilterType::Url,
                options: fdbg::FilterOptions { recursive: Some(true), ..Default::default() },
            });
        }

        let (iterator_proxy, iterator_server_end) = fidl::endpoints::create_proxy();
        if let Err(_) = self
            .proxy
            .get_process_info(
                &fdbg::GetProcessInfoOptions {
                    filter: maybe_filter,
                    interest: Some(fdbg::ThreadDetailsInterest {
                        backtrace: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                iterator_server_end,
            )
            .await
        {
            // report error?
            return;
        }

        loop {
            match iterator_proxy.get_next().await {
                Ok(result) => match result {
                    Ok(infos) => {
                        if infos.len() == 0 {
                            break;
                        }

                        for info in infos {
                            let _ = server_end.write(
                                format!(
                                    "Component: {}\nProcess: {} Thread: {}\n",
                                    info.moniker, info.process, info.thread
                                )
                                .as_bytes(),
                            );
                            DebugAgent::write_backtrace_info(
                                &ThreadBacktraceInfo {
                                    thread: info.thread,
                                    backtrace: info.details.backtrace.unwrap(),
                                },
                                &mut server_end,
                            )
                            .await;

                            let _ = server_end.write("\n".as_bytes());
                        }
                    }
                    Err(_e) => {
                        break;
                    }
                },
                Err(_e) => {
                    break;
                }
            }
        }
    }

    // Begins processing events sent by the debug agent server. Event processing runs in a
    // spawned task and termines when the returned `RemoteHandle` is dropped.
    fn begin_process_event_stream(
        mut event_stream: fdbg::DebugAgentEventStream,
        mut sender: mpsc::Sender<ThreadBacktraceInfo>,
    ) -> RemoteHandle<()> {
        let (event_stream_receiver, event_stream_receiver_handle) = async move {
            loop {
                match event_stream.next().await {
                    Some(Ok(fdbg::DebugAgentEvent::OnFatalException {
                        payload:
                            fdbg::DebugAgentOnFatalExceptionRequest {
                                thread: Some(thread),
                                backtrace: Some(backtrace),
                                ..
                            },
                    })) => {
                        sender.send(ThreadBacktraceInfo { thread, backtrace }).await.unwrap();
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
        .remote_handle();

        fasync::Task::spawn(event_stream_receiver).detach();

        event_stream_receiver_handle
    }

    pub(crate) async fn write_backtrace_info(
        backtrace_info: &ThreadBacktraceInfo,
        socket_server_end: &mut fidl::Socket,
    ) {
        for line in backtrace_info.backtrace.split('\n') {
            if line.starts_with("{{{reset:begin}}}") {
                let _ = socket_server_end.write(line[..17].as_bytes());
                let _ = socket_server_end.write(b"\n");
                let _ = socket_server_end.write(line[17..].as_bytes());
            } else {
                let _ = socket_server_end.write(line.as_bytes());
            }

            let _ = socket_server_end.write(b"\n");
        }
    }
}

#[cfg(test)]
mod test {
    use crate::run_events::SuiteEventPayload;

    use super::*;

    const TEST_THREAD_1: u64 = 1234;
    const TEST_THREAD_2: u64 = 5678;
    const TEST_PROCESS_1: u64 = 4321;
    const TEST_PROCESS_2: u64 = 8765;
    const TEST_BACKTRACE_1: &str = "{{{reset:begin}}}test backtrace 1, with reset:begin prefix";
    const TEST_BACKTRACE_2: &str = "test backtrace 2, no special prefix";
    const TEST_MONIKER_1: &str = "test moniker 1";
    const TEST_MONIKER_2: &str = "test moniker 2";
    const TEST_STDERR_TEXT: &str = "Component: test moniker 1\nProcess: 4321 Thread: 1234\n\
        {{{reset:begin}}}\ntest backtrace 1, with reset:begin prefix\n\nComponent: test moniker 2\n\
        Process: 8765 Thread: 5678\ntest backtrace 2, no special prefix\n\n";
    const TEST_STDERR_SIZE: usize = 204;

    #[fuchsia::test]
    async fn fatal_exceptions() {
        let (mut under_test, mut debug_agent_service) = start_agent().await;

        // Pretend a fatal exception has occurred.
        debug_agent_service.send_on_fatal_exception_event(TEST_THREAD_1, TEST_BACKTRACE_1);

        // Expect to receive the fatal exception.
        let mut receiver = under_test.take_fatal_exception_receiver();
        let thread_backtrace_info = receiver.next().await.expect("receive fatal exception");
        assert_eq!(TEST_THREAD_1, thread_backtrace_info.thread);
        assert_eq!(TEST_BACKTRACE_1, thread_backtrace_info.backtrace);

        debug_agent_service.expect_nothing_more();
    }

    #[fuchsia::test]
    async fn all_backtraces() {
        let (under_test, mut debug_agent_service) = start_agent().await;

        let (event_sender, mut event_receiver) =
            mpsc::channel::<Result<SuiteEvents, LaunchError>>(16);

        // Call `report_all_backtraces` while expecting a `GetProcessInfo` request and serving
        // the process info iterator that is returned.
        futures::future::join(under_test.report_all_backtraces(None, event_sender), async {
            let iterator_server_end = debug_agent_service
                .expect_get_process_info(&fdbg::GetProcessInfoOptions {
                    filter: None,
                    interest: Some(fdbg::ThreadDetailsInterest {
                        backtrace: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .await;
            serve_process_info_iterator(iterator_server_end).await;
        })
        .await;

        // Expect to receive a stderr event.
        match event_receiver.try_next() {
            Ok(Some(Ok(SuiteEvents {
                timestamp: _,
                payload: SuiteEventPayload::SuiteStderr(socket),
            }))) => {
                let mut buffer: [u8; 256] = [0; 256];
                let size = socket.read(&mut buffer).expect("read from socket");
                assert_eq!(TEST_STDERR_SIZE, size);
                let stderr_string =
                    std::str::from_utf8(&buffer[0..TEST_STDERR_SIZE]).expect("convert [u8] to str");
                assert_eq!(TEST_STDERR_TEXT, stderr_string);
            }
            Ok(Some(Ok(_))) => {
                assert!(false, "Expected SuiteStderr event, got other event");
            }
            Ok(Some(Err(e))) => {
                assert!(false, "Expected event, got error {:?}", e);
            }
            Ok(None) => {
                assert!(false, "Expected event, got channel closed");
            }
            Err(e) => {
                assert!(false, "Expected event, got channel error {:?}", e);
            }
        }
    }

    // Fake fuchsia.debugger.Launcher service.
    struct FakeLauncherService {
        request_stream: fdbg::LauncherRequestStream,
    }

    impl FakeLauncherService {
        // Creates a new FakeLauncherService.
        fn new() -> (fdbg::LauncherProxy, Self) {
            let (proxy, request_stream) =
                fidl::endpoints::create_proxy_and_stream::<fdbg::LauncherMarker>();

            (proxy, Self { request_stream })
        }

        // Expects a launch request and returns the resulting FakeDebugAgentService.
        async fn expect_launch_request(&mut self) -> FakeDebugAgentService {
            match self.next_request().await {
                fdbg::LauncherRequest::Launch { agent, options, responder } => {
                    assert_eq!(options.monitor_root_job, Some(false));
                    responder.send(Ok(())).expect("send response to Launcher::Launch");
                    return FakeDebugAgentService::new(agent);
                }
                _ => panic!("Call to unexpected method."),
            }
        }

        async fn expect_connection_closed(&mut self) {
            if let Some(request) = self.request_stream.try_next().await.expect("get request") {
                assert!(false, "Expected connection closed, got request {:?}", request);
            }
        }

        // Expects that no more requests have arrived and the client connection has not closed.
        fn expect_nothing_more(&mut self) {
            match self.request_stream.try_next().now_or_never() {
                Some(Ok(None)) => {
                    assert!(false, "Expected nothing more, got connection closed");
                }
                Some(Ok(v)) => {
                    assert!(false, "Expected nothing more, got request {:?}", v);
                }
                Some(Err(e)) => {
                    assert!(false, "Expected nothing more, got stream error {:?}", e);
                }
                None => {}
            }
        }

        // Returns the next request.
        async fn next_request(&mut self) -> fdbg::LauncherRequest {
            self.request_stream
                .try_next()
                .await
                .expect("get request")
                .expect("connection not closed")
        }
    }

    // Fake fuchsia.debugger.DebugAgent service.
    struct FakeDebugAgentService {
        request_stream: fdbg::DebugAgentRequestStream,
        control_handle: fdbg::DebugAgentControlHandle,
    }

    impl FakeDebugAgentService {
        // Creates a new FakeDebugAgentService.
        fn new(server_end: fidl::endpoints::ServerEnd<fdbg::DebugAgentMarker>) -> Self {
            let (request_stream, control_handle) = server_end.into_stream_and_control_handle();
            FakeDebugAgentService { request_stream, control_handle }
        }

        // Expects an AttachTo request with the expected parameters and responds with `num_matches`.
        async fn expect_attach_to(
            &mut self,
            expected_pattern: &str,
            expected_type: fdbg::FilterType,
            expected_options: fdbg::FilterOptions,
            num_matches: u32,
        ) {
            match self.next_request().await {
                fdbg::DebugAgentRequest::AttachTo { pattern, type_, options, responder } => {
                    responder.send(Ok(num_matches)).expect("send response to DebugAgent::AttachTo");
                    assert_eq!(expected_pattern, pattern);
                    assert_eq!(expected_type, type_);
                    assert_eq!(expected_options, options);
                }
                _ => panic!("Call to unexpected method."),
            }
        }

        // Expects a GetProcessInfo request with the expected parameters and returns the server
        // end of the process info iterator.
        async fn expect_get_process_info(
            &mut self,
            expected_options: &fdbg::GetProcessInfoOptions,
        ) -> fidl::endpoints::ServerEnd<fdbg::ProcessInfoIteratorMarker> {
            match self.next_request().await {
                fdbg::DebugAgentRequest::GetProcessInfo { options, iterator, responder } => {
                    responder.send(Ok(())).expect("send response to DebugAgent::GetProcessInfo");
                    assert_eq!(expected_options, &options);
                    iterator
                }
                _ => panic!("Call to unexpected method."),
            }
        }

        // Sends an OnFatalException event to the client.
        fn send_on_fatal_exception_event(&mut self, thread: u64, backtrace: &str) {
            self.control_handle
                .send_on_fatal_exception(&fdbg::DebugAgentOnFatalExceptionRequest {
                    thread: Some(thread),
                    backtrace: Some(backtrace.to_string()),
                    ..Default::default()
                })
                .expect("send OnFatalException event");
        }

        // Expects that no more requests have arrived and the client connection has not closed.
        fn expect_nothing_more(&mut self) {
            match self.request_stream.try_next().now_or_never() {
                Some(Ok(None)) => {
                    assert!(false, "Expected nothing more, got connection closed");
                }
                Some(Ok(v)) => {
                    assert!(false, "Expected nothing more, got request {:?}", v);
                }
                Some(Err(e)) => {
                    assert!(false, "Expected nothing more, got stream error {:?}", e);
                }
                None => {}
            }
        }

        // Returns the next request.
        async fn next_request(&mut self) -> fdbg::DebugAgentRequest {
            self.request_stream
                .try_next()
                .await
                .expect("get request")
                .expect("connection not closed")
        }
    }

    async fn start_agent() -> (DebugAgent, FakeDebugAgentService) {
        // Create the fake launcher service.
        let (launcher_proxy, mut launcher_service) = FakeLauncherService::new();
        launcher_service.expect_nothing_more();

        // Create a DebugAgent and the fake debug agent service.
        let (under_test, mut debug_agent_service) =
            futures::future::join(DebugAgent::from_launcher_proxy(launcher_proxy), async {
                let mut debug_agent_service = launcher_service.expect_launch_request().await;
                debug_agent_service
                    .expect_attach_to(
                        TEST_ROOT_REALM_NAME,
                        fdbg::FilterType::MonikerSuffix,
                        fdbg::FilterOptions {
                            recursive: Some(true),
                            job_only: Some(true),
                            ..Default::default()
                        },
                        0,
                    )
                    .await;
                debug_agent_service
            })
            .await;

        let under_test = under_test.expect("create launcher from proxy");

        // from_launcher_proxy consumes the proxy, so the launcher connection should close.
        launcher_service.expect_connection_closed().await;
        debug_agent_service.expect_nothing_more();

        (under_test, debug_agent_service)
    }

    // Serves a `ProcessInfoIterator` producing two process infos.
    async fn serve_process_info_iterator(
        server_end: fidl::endpoints::ServerEnd<fdbg::ProcessInfoIteratorMarker>,
    ) {
        let mut request_stream = server_end.into_stream();

        // Expect a request and respond with two process infos.
        let request =
            request_stream.try_next().await.expect("get request").expect("connection not closed");
        match request {
            fdbg::ProcessInfoIteratorRequest::GetNext { responder } => {
                responder
                    .send(Ok(&vec![
                        fdbg::ProcessInfo {
                            process: TEST_PROCESS_1,
                            moniker: TEST_MONIKER_1.to_string(),
                            thread: TEST_THREAD_1,
                            details: fdbg::ThreadDetails {
                                backtrace: Some(TEST_BACKTRACE_1.to_string()),
                                ..Default::default()
                            },
                        },
                        fdbg::ProcessInfo {
                            process: TEST_PROCESS_2,
                            moniker: TEST_MONIKER_2.to_string(),
                            thread: TEST_THREAD_2,
                            details: fdbg::ThreadDetails {
                                backtrace: Some(TEST_BACKTRACE_2.to_string()),
                                ..Default::default()
                            },
                        },
                    ]))
                    .expect("send response");
            }
        }

        // Expect another request and respond with an empty vector.
        let request =
            request_stream.try_next().await.expect("get request").expect("connection not closed");
        match request {
            fdbg::ProcessInfoIteratorRequest::GetNext { responder } => {
                responder.send(Ok(&vec![])).expect("send response");
            }
        }

        // Expect the client to close the connection.
        if let Some(request) = request_stream.try_next().await.expect("get request") {
            assert!(false, "Expected connection closed, got request {:?}", request);
        }
    }
}
