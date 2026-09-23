// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::manager::{FidlEndpoint, Manager};
use anyhow::{Context as _, Error, Result};
use fidl::endpoints::{
    ClientEnd, ControlHandle, ServerEnd, create_proxy, create_proxy_and_stream,
    create_request_stream,
};
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_fuzzer as fuzz;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_test_manager as test_manager;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::future::join_all;
use futures::{
    AsyncReadExt, AsyncWriteExt, FutureExt, SinkExt, StreamExt, TryStreamExt, join, pin_mut,
    select, try_join,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use test_manager::*;

////////////////////////////////////////////////////////////////////////////////
// Test fixtures.

const BUF_SIZE: u64 = 4096;

// The |TestRealm| simulates running multiple components concurrently: the |Registry|, the
// test_manager's |RunController| and |SuiteController|, and the |BatchIterator|.

pub struct TestRealm {
    fuzzers: Rc<FakeFuzzerMap>,
    manager_streams: Vec<fuzz::ManagerRequestStream>,
    pub registry_status: Result<(), zx::Status>,
    pub launch_error: Option<LaunchError>,
}

impl TestRealm {
    pub fn new() -> Self {
        Self {
            fuzzers: Rc::new(FakeFuzzerMap::new()),
            manager_streams: Vec::new(),
            registry_status: Ok(()),
            launch_error: None,
        }
    }

    pub async fn write_stdout(&self, url: &str, msg: &str) -> Result<()> {
        // Don't hold mutable refs across awaits.
        if let Some(mut socket) = self.fuzzers.take_stdout(url) {
            socket
                .write_all(msg.as_bytes())
                .await
                .context(format!("failed to write to stdout of {}", url))?;
            self.fuzzers.put_stdout(url, socket);
        }
        Ok(())
    }

    pub async fn write_stderr(&self, url: &str, msg: &str) -> Result<()> {
        // Don't hold mutable refs across awaits.
        if let Some(mut socket) = self.fuzzers.take_stderr(url) {
            socket
                .write_all(msg.as_bytes())
                .await
                .context(format!("failed to write to stderr of {}", url))?;
            self.fuzzers.put_stderr(url, socket);
        }
        Ok(())
    }

    pub async fn write_syslog(&self, url: &str, msg: &str) -> Result<()> {
        // Don't hold mutable refs across awaits.
        if let Some(mut sender) = self.fuzzers.take_syslog(url) {
            sender
                .send(msg.to_string())
                .await
                .context(format!("failed to write to syslog of {}", url))?;
            self.fuzzers.put_syslog(url, sender);
        }
        Ok(())
    }

    pub fn send_suite_stopped(&self, url: &str) -> Result<()> {
        self.fuzzers
            .send_suite_stopped(url)
            .context(format!("failed to send 'suite stopped' to {}", url))
    }
}

pub fn connect_to_manager(test_realm: Rc<RefCell<TestRealm>>) -> Result<fuzz::ManagerProxy> {
    let (manager_proxy, manager_stream) = create_proxy_and_stream::<fuzz::ManagerMarker>();
    test_realm.borrow_mut().manager_streams.push(manager_stream);
    Ok(manager_proxy)
}

pub async fn read_async(socket: &zx::Socket) -> Result<String> {
    let socket =
        socket.duplicate_handle(zx::Rights::SAME_RIGHTS).context("failed to duplicate handle")?;
    let mut socket = fasync::Socket::from_socket(socket);
    let mut buf: [u8; BUF_SIZE as usize] = [0; BUF_SIZE as usize];
    let bytes_read = socket.read(&mut buf).await.context("failed to read from socket")?;
    Ok(std::str::from_utf8(&buf[..bytes_read]).unwrap_or_default().to_string())
}

pub async fn serve_test_realm(test_realm: Rc<RefCell<TestRealm>>) -> Result<()> {
    // Create a fake registry that serves requests from the fuzz-manager.
    let (registry_proxy, registry_stream) = create_proxy_and_stream::<fuzz::RegistryMarker>();
    let registry_fut = serve_registry(registry_stream, Rc::clone(&test_realm)).fuse();

    // Create a fake test_manager that serves requests from the fuzz-manager.
    let test_manager = Rc::new(FakeTestManager::new());
    let run_builder = test_manager.create_run_builder();
    let test_manager_fut = test_manager.serve(Rc::clone(&test_realm)).fuse();

    // Multiplex concurrent streams for the manager.
    let (sender, receiver) = mpsc::unbounded::<fuzz::ManagerRequest>();
    let manager_streams = {
        let mut test_realm = test_realm.borrow_mut();
        test_realm.manager_streams.drain(..).collect::<Vec<_>>()
    };
    let mut stream_futs = Vec::new();
    for stream in manager_streams.into_iter() {
        let sender = sender.clone();
        let stream_fut = async move {
            stream.map_err(Error::msg).forward(sender.sink_map_err(Error::msg)).await
        };
        stream_futs.push(stream_fut);
    }
    drop(sender);
    // Create a fake fuzz-manager that serves requests from connected unit tests.
    let fuzz_manager_fut = || async move {
        let manager = Manager::new(registry_proxy, run_builder);
        let results = join!(join_all(stream_futs), manager.serve(receiver));
        for result in results.0.into_iter() {
            result.context("failed to mulitplex stream")?;
        }
        results.1.context("failed to serve manager")?;
        Ok::<(), Error>(())
    };
    let fuzz_manager_fut = fuzz_manager_fut().fuse();

    // The registry and test_manager futures will run indefinitely, so simply drop them once all
    // connections from unit tests to the fuzz-manager have closed.
    pin_mut!(registry_fut, test_manager_fut, fuzz_manager_fut);
    loop {
        select! {
            result = fuzz_manager_fut => break result.context("failed to serve manager"),
            result = registry_fut => result.context("failed to serve fake registry"),
            _ = test_manager_fut => Ok(()),
        }?;
    }
}

// The fake |Registry| tracks which fuzzers have been dis/connected, and allows tests to simulate
// errors by setting the status returned by |fuchsia.fuzzer.Registry.Connect|.
async fn serve_registry(
    stream: fuzz::RegistryRequestStream,
    test_realm: Rc<RefCell<TestRealm>>,
) -> Result<()> {
    let (fuzzers, status) = {
        let test_realm = test_realm.borrow();
        (Rc::clone(&test_realm.fuzzers), test_realm.registry_status)
    };
    stream
        .map(|request| request.context("failed request"))
        .try_for_each(|request| async {
            match request {
                fuzz::RegistryRequest::Connect {
                    fuzzer_url,
                    controller,
                    timeout: _,
                    responder,
                } => {
                    let response = match status {
                        Ok(()) => {
                            fuzzers.connect(&fuzzer_url, controller);
                            Ok(())
                        }
                        Err(err) => {
                            let _ = fuzzers.remove(&fuzzer_url);
                            Err(err.into_raw())
                        }
                    };
                    responder.send(response)
                }
                fuzz::RegistryRequest::Disconnect { fuzzer_url, responder } => {
                    let response = match fuzzers.remove(&fuzzer_url) {
                        Ok(()) => Ok(()),
                        Err(status) => Err(status.into_raw()),
                    };
                    responder.send(response)
                }
            }
            .map_err(Error::msg)
            .context("failed response")
        })
        .await
        .context("failed to serve fake Registry")?;
    Ok(())
}

// The |FakeTestManager| simulates the behavior of test_manager and provides a way to
// |serve_run_builder| streams by connecting them via |FakeSuiteRunnerEndpoint|s.
struct FakeTestManager {
    sender: mpsc::UnboundedSender<ServerEnd<SuiteRunnerMarker>>,
    receiver: RefCell<Option<mpsc::UnboundedReceiver<ServerEnd<SuiteRunnerMarker>>>>,
}

impl FakeTestManager {
    fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded::<ServerEnd<SuiteRunnerMarker>>();
        Self { sender: sender, receiver: RefCell::new(Some(receiver)) }
    }

    fn create_run_builder(&self) -> FakeSuiteRunnerEndpoint {
        FakeSuiteRunnerEndpoint::new(self.sender.clone())
    }

    async fn serve(&self, test_realm: Rc<RefCell<TestRealm>>) {
        const MAX_CONCURRENT: usize = 100;
        let receiver = self.receiver.borrow_mut().take().unwrap();
        receiver
            .for_each_concurrent(MAX_CONCURRENT, |server_end| async {
                let stream = server_end.into_stream();
                serve_run_builder(stream, Rc::clone(&test_realm))
                    .await
                    .expect("failed to serve run builder");
            })
            .await;
    }
}

// The |FakeSuiteRunnerEndpoint| provides a way to connect a proxy to a stream served by
// |serve_run_builder|.
struct FakeSuiteRunnerEndpoint {
    sender: mpsc::UnboundedSender<ServerEnd<SuiteRunnerMarker>>,
}

impl FakeSuiteRunnerEndpoint {
    fn new(sender: mpsc::UnboundedSender<ServerEnd<SuiteRunnerMarker>>) -> Self {
        Self { sender }
    }
}

impl FidlEndpoint<SuiteRunnerMarker> for FakeSuiteRunnerEndpoint {
    fn create_proxy(&self) -> Result<SuiteRunnerProxy> {
        let (proxy, server_end) = create_proxy::<SuiteRunnerMarker>();
        self.sender.unbounded_send(server_end)?;
        Ok(proxy)
    }
}

// Serves |fuchsia.test_manager.SuiteRunner| while allowing test realms to configure various errors
// and/or events retrieved by the |Suite-| and |RunController|s.
async fn serve_run_builder(
    mut stream: SuiteRunnerRequestStream,
    test_realm: Rc<RefCell<TestRealm>>,
) -> Result<()> {
    let (fuzzers, launch_error) = {
        let mut test_realm = test_realm.borrow_mut();
        (Rc::clone(&test_realm.fuzzers), test_realm.launch_error.take())
    };
    // |SuiteRunner| requests will always follow a specific order: |AddSuite|, then |Build|.
    let (url, suite_stream) = match stream.next().await {
        Some(Ok(SuiteRunnerRequest::Run {
            test_suite_url,
            options: _,
            controller,
            control_handle: _,
        })) => {
            let suite_stream = controller.into_stream();
            (test_suite_url, suite_stream)
        }
        _ => unreachable!(),
    };

    // Tests with launch errors do not have a working syslog and must not start the batch iterator.
    let (batch_client, batch_stream, syslog_sender, syslog_receiver) = match launch_error {
        Some(_) => (None, None, None, None),
        None => {
            fuzzers.launch(&url);
            let (batch_client, batch_stream) =
                create_request_stream::<fdiagnostics::BatchIteratorMarker>();
            let (syslog_sender, syslog_receiver) = mpsc::unbounded::<String>();
            (Some(batch_client), Some(batch_stream), Some(syslog_sender), Some(syslog_receiver))
        }
    };
    let suite_receiver =
        send_default_suite_events(Rc::clone(&fuzzers), &url, batch_client, syslog_sender)
            .context("failed to send default suite events")?;

    try_join!(
        async {
            serve_suite_controller(suite_stream, launch_error, suite_receiver)
                .await
                .context("failed to serve suite controller")
        },
        async {
            serve_batch_iterator(batch_stream, syslog_receiver)
                .await
                .context("failed to serve batch iterator")
        },
    )?;
    Ok(())
}

fn send_default_suite_events(
    fuzzers: Rc<FakeFuzzerMap>,
    url: &str,
    client_end: Option<ClientEnd<fdiagnostics::BatchIteratorMarker>>,
    syslog_sender: Option<mpsc::UnboundedSender<String>>,
) -> Result<mpsc::UnboundedReceiver<EventDetails>> {
    let (sender, receiver) = mpsc::unbounded::<EventDetails>();
    fuzzers.put_suite_sender(url, sender);
    fuzzers
        .send_suite_event(
            url,
            EventDetails::SuiteStarted(SuiteStartedEventDetails { ..Default::default() }),
        )
        .context("failed to send SuiteStarted suite event")?;

    if let Some(syslog_sender) = syslog_sender {
        fuzzers.put_syslog(url, syslog_sender);
    }
    if let Some(client_end) = client_end {
        fuzzers
            .send_suite_event(
                url,
                EventDetails::SuiteArtifactGenerated(SuiteArtifactGeneratedEventDetails {
                    artifact: Some(Artifact::Log(Syslog::Batch(client_end))),
                    ..Default::default()
                }),
            )
            .context("failed to send Syslog suite event")?;
    };

    let id = fuzzers.get_id(&url);
    fuzzers
        .send_suite_event(
            url,
            EventDetails::TestCaseFound(TestCaseFoundEventDetails {
                test_case_name: Some("test-case".to_string()),
                test_case_id: Some(id),
                ..Default::default()
            }),
        )
        .context("failed to send CaseFound suite event")?;
    fuzzers
        .send_suite_event(
            url,
            EventDetails::TestCaseStarted(TestCaseStartedEventDetails {
                test_case_id: Some(id),
                ..Default::default()
            }),
        )
        .context("failed to send CaseStarted suite event")?;

    let (out_rx, out_tx) = zx::Socket::create_stream();
    let stdout = fasync::Socket::from_socket(out_tx);
    fuzzers.put_stdout(&url, stdout);
    fuzzers
        .send_suite_event(
            url,
            EventDetails::SuiteArtifactGenerated(SuiteArtifactGeneratedEventDetails {
                artifact: Some(Artifact::Stdout(out_rx)),
                ..Default::default()
            }),
        )
        .context("failed to send Stdout run event")?;

    let (err_rx, err_tx) = zx::Socket::create_stream();
    let stderr = fasync::Socket::from_socket(err_tx);
    fuzzers.put_stderr(&url, stderr);
    fuzzers
        .send_suite_event(
            url,
            EventDetails::SuiteArtifactGenerated(SuiteArtifactGeneratedEventDetails {
                artifact: Some(Artifact::Stderr(err_rx)),
                ..Default::default()
            }),
        )
        .context("failed to send Stderr run event")?;

    Ok(receiver)
}

async fn serve_suite_controller(
    stream: SuiteControllerRequestStream,
    launch_error: Option<LaunchError>,
    event_details_receiver: mpsc::UnboundedReceiver<EventDetails>,
) -> Result<()> {
    let event_details_receiver_rc = RefCell::new(event_details_receiver);
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async {
            match request {
                SuiteControllerRequest::WatchEvents { responder } => {
                    let mut event_details_receiver = event_details_receiver_rc.borrow_mut();
                    let response = match launch_error {
                        Some(e) => Err(e),
                        None => {
                            let mut details = Vec::new();
                            while let Ok(Some(detail)) = event_details_receiver.try_next() {
                                details.push(detail);
                            }
                            if details.is_empty() {
                                if let Some(detail) = event_details_receiver.next().await {
                                    details.push(detail);
                                }
                            }
                            let suite_events = details
                                .drain(..)
                                .map(|details| Event {
                                    timestamp: Some(zx::MonotonicInstant::get().into_nanos()),
                                    details: Some(details),
                                    ..Default::default()
                                })
                                .collect();
                            Ok(suite_events)
                        }
                    };
                    responder.send(response)
                }
                SuiteControllerRequest::Kill { control_handle } => {
                    control_handle.shutdown();
                    Ok(())
                }
                _ => unreachable!(),
            }
            .map_err(Error::msg)
            .context("failed response")
        })
        .await
        .context("failed to serve fake SuiteController")?;
    Ok(())
}

async fn serve_batch_iterator(
    stream: Option<fdiagnostics::BatchIteratorRequestStream>,
    receiver: Option<mpsc::UnboundedReceiver<String>>,
) -> Result<()> {
    let stream = match stream {
        Some(stream) => stream,
        None => return Ok(()),
    };
    let receiver = match receiver {
        Some(receiver) => RefCell::new(receiver),
        None => return Ok(()),
    };
    stream
        .map(|request| request.context("failed request"))
        .try_for_each(|request| async {
            match request {
                fdiagnostics::BatchIteratorRequest::WaitForReady { responder } => responder.send(),
                fdiagnostics::BatchIteratorRequest::GetNext { responder } => {
                    let mut receiver = receiver.borrow_mut();
                    let mut logs = Vec::new();
                    // Collect any outstanding logs.
                    loop {
                        match receiver.try_next() {
                            Ok(Some(msg)) => logs.push(msg),
                            Ok(None) => break,
                            Err(_) => break,
                        };
                    }
                    // If no logs, "hang the get" and wait for a log or channel closure.
                    if logs.is_empty() {
                        match receiver.next().await {
                            Some(msg) => logs.push(msg),
                            None => {}
                        };
                    }
                    // Convert the logs.
                    let mut batch = Vec::new();
                    for msg in logs.drain(..) {
                        let vmo = zx::Vmo::create(BUF_SIZE).context("failed to create VMO")?;
                        let size = msg.len() as u64;
                        let buf = fmem::Buffer { vmo, size };
                        buf.vmo.write(msg.as_bytes(), 0).context("failed to write to VMO")?;
                        batch.push(fdiagnostics::FormattedContent::Json(buf));
                    }
                    responder.send(Ok(batch))
                }
                fdiagnostics::BatchIteratorRequest::_UnknownMethod { .. } => {
                    unreachable!("Not sending other requests");
                }
            }
            .map_err(Error::msg)
            .context("failed response")
        })
        .await
        .context("failed to serve fake BatchIterator")?;
    Ok(())
}

// The |FakeFuzzerMap| is simply a |HashMap| that can auto-assign test case identifiers. It holds
// the controller stream to prevent it from going out of scope. It also has the producer ends of the
// diagnostic streams, which test can use to produce fake messages. Finally, it has a oneshot
// channel that it triggers on removal by the fake registry, and that notifies the fake run_builder
// to stop the fake run and suite controllers.

#[derive(Default, Debug)]
struct FakeFuzzer {
    id: u32,
    _controller: Option<ServerEnd<fuzz::ControllerMarker>>,
    stdout: Option<fasync::Socket>,
    stderr: Option<fasync::Socket>,
    syslog: Option<mpsc::UnboundedSender<String>>,
    suite_sender: Option<mpsc::UnboundedSender<EventDetails>>,
}

impl FakeFuzzer {
    fn send_suite_event(&mut self, details: EventDetails) -> Result<()> {
        if let Some(sender) = self.suite_sender.as_ref() {
            sender.unbounded_send(details).context("failed to send EventDetails")?;
        }
        Ok(())
    }

    fn send_suite_stopped(&mut self) -> Result<()> {
        self.send_suite_event(EventDetails::SuiteStopped(SuiteStoppedEventDetails {
            result: Some(SuiteResult::Finished),
            ..Default::default()
        }))
        .context("failed to send SuiteStopped event")?;
        self.suite_sender = None;
        self.syslog = None;
        Ok(())
    }
}

#[derive(Debug)]
struct FakeFuzzerMap {
    next_id: RefCell<u32>,
    fuzzers: RefCell<HashMap<String, FakeFuzzer>>,
}

impl FakeFuzzerMap {
    fn new() -> Self {
        Self { next_id: RefCell::new(1), fuzzers: RefCell::new(HashMap::new()) }
    }

    // Adds an entry to the fuzzer map for |url| if it does not exist. Does nothing if |url| is
    // already present in the map.
    fn launch(&self, url: &str) {
        let mut id = self.next_id.borrow_mut();
        let mut fuzzers = self.fuzzers.borrow_mut();
        if fuzzers.contains_key(url) {
            return;
        }
        fuzzers.insert(
            url.to_string(),
            FakeFuzzer {
                id: *id,
                _controller: None,
                stdout: None,
                stderr: None,
                syslog: None,
                suite_sender: None,
            },
        );
        *id += 1;
    }

    fn get_id(&self, url: &str) -> u32 {
        self.fuzzers.borrow_mut().get_mut(url).and_then(|fuzzer| Some(fuzzer.id)).unwrap_or(0)
    }

    fn connect(&self, url: &str, controller: ServerEnd<fuzz::ControllerMarker>) {
        self.fuzzers.borrow_mut().get_mut(url).unwrap()._controller = Some(controller);
    }

    fn put_suite_sender(&self, url: &str, suite_sender: mpsc::UnboundedSender<EventDetails>) {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.suite_sender = Some(suite_sender);
        }
    }

    fn send_suite_event(&self, url: &str, details: EventDetails) -> Result<()> {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.send_suite_event(details).context("failed to send suite event")?;
        }
        Ok(())
    }

    fn send_suite_stopped(&self, url: &str) -> Result<()> {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.send_suite_stopped().context("failed to send 'suite stopped'")?;
        }
        Ok(())
    }

    fn put_stdout(&self, url: &str, stdout: fasync::Socket) {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.stdout = Some(stdout);
        }
    }

    fn take_stdout(&self, url: &str) -> Option<fasync::Socket> {
        self.fuzzers.borrow_mut().get_mut(url).and_then(|fuzzer| fuzzer.stdout.take())
    }

    fn put_stderr(&self, url: &str, stderr: fasync::Socket) {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.stderr = Some(stderr);
        }
    }

    fn take_stderr(&self, url: &str) -> Option<fasync::Socket> {
        self.fuzzers.borrow_mut().get_mut(url).and_then(|fuzzer| fuzzer.stderr.take())
    }

    fn put_syslog(&self, url: &str, syslog: mpsc::UnboundedSender<String>) {
        let mut fuzzers = self.fuzzers.borrow_mut();
        if let Some(fuzzer) = fuzzers.get_mut(url) {
            fuzzer.syslog = Some(syslog);
        }
    }

    fn take_syslog(&self, url: &str) -> Option<mpsc::UnboundedSender<String>> {
        self.fuzzers.borrow_mut().get_mut(url).and_then(|fuzzer| fuzzer.syslog.take())
    }

    fn remove(&self, url: &str) -> Result<(), zx::Status> {
        let mut fuzzers = self.fuzzers.borrow_mut();
        match fuzzers.remove(url) {
            Some(mut fuzzer) => {
                let _ = fuzzer.send_suite_stopped();
                Ok(())
            }
            None => Err(zx::Status::NOT_FOUND),
        }
    }
}
