// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate provides helper functions for testing architecture tests.

use anyhow::{Context as _, Error, bail};
use fidl_fuchsia_test_manager::{
    self as ftest_manager, SuiteControllerProxy, SuiteEvent as FidlSuiteEvent,
    SuiteEventPayload as FidlSuiteEventPayload, SuiteEventPayloadUnknown,
};
use futures::channel::mpsc;
use futures::prelude::*;
use linked_hash_map::LinkedHashMap;
use log::*;
use moniker::ExtendedMoniker;
use std::collections::HashMap;
use std::sync::Arc;
use test_diagnostics::zstd_compress::Decoder;
use test_diagnostics::{LogStream, collect_and_send_string_output, collect_string_from_socket};
use {fidl_fuchsia_io as fio, fuchsia_async as fasync};

pub fn default_run_option() -> ftest_manager::RunOptions {
    ftest_manager::RunOptions {
        parallel: None,
        arguments: None,
        run_disabled_tests: Some(false),
        timeout: None,
        case_filters_to_run: None,
        log_iterator: None,
        ..Default::default()
    }
}

pub fn default_run_suite_options() -> ftest_manager::RunSuiteOptions {
    ftest_manager::RunSuiteOptions { run_disabled_tests: Some(false), ..Default::default() }
}

#[derive(Debug, Eq, PartialEq)]
pub struct AttributedLog {
    pub log: String,
    pub moniker: ExtendedMoniker,
}

pub async fn collect_suite_events(
    suite_instance: SuiteRunInstance,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let (sender, mut recv) = mpsc::channel(1);
    let execution_task =
        fasync::Task::spawn(async move { suite_instance.collect_events(sender).await });
    let mut events = vec![];
    let mut log_tasks = vec![];
    while let Some(event) = recv.next().await {
        match event.payload {
            SuiteEventPayload::RunEvent(RunEvent::CaseStdout { name, mut stdout_message }) => {
                if stdout_message.ends_with("\n") {
                    stdout_message.truncate(stdout_message.len() - 1)
                }
                let logs = stdout_message.split("\n");
                for log in logs {
                    // gtest produces this line when tests are randomized. As of
                    // this writing, our gtest_main binary *always* randomizes.
                    if log.contains("Note: Randomizing tests' orders with a seed of") {
                        continue;
                    }
                    events.push(RunEvent::case_stdout(name.clone(), log.to_string()));
                }
            }
            SuiteEventPayload::RunEvent(RunEvent::CaseStderr { name, mut stderr_message }) => {
                if stderr_message.ends_with("\n") {
                    stderr_message.truncate(stderr_message.len() - 1)
                }
                let logs = stderr_message.split("\n");
                for log in logs {
                    events.push(RunEvent::case_stderr(name.clone(), log.to_string()));
                }
            }
            SuiteEventPayload::RunEvent(e) => events.push(e),
            SuiteEventPayload::SuiteLog { log_stream } => {
                let t = fasync::Task::spawn(log_stream.collect::<Vec<_>>());
                log_tasks.push(t);
            }
            SuiteEventPayload::TestCaseLog { .. } => {
                panic!("not supported yet!")
            }
            SuiteEventPayload::DebugData { .. } => {
                panic!("not supported yet!")
            }
        }
    }
    execution_task.await.context("test execution failed")?;

    let mut collected_logs = vec![];
    for t in log_tasks {
        let logs = t.await;
        for log_result in logs {
            let log = log_result?;
            collected_logs
                .push(AttributedLog { log: log.msg().unwrap().to_string(), moniker: log.moniker });
        }
    }

    Ok((events, collected_logs))
}

pub async fn collect_suite_events_with_watch(
    suite_instance: SuiteRunInstance,
    filter_debug_data: bool,
    compressed_debug_data: bool,
) -> Result<(Vec<RunEvent>, Vec<AttributedLog>), Error> {
    let (sender, mut recv) = mpsc::channel(1);
    let execution_task = fasync::Task::spawn(async move {
        suite_instance
            .collect_events_with_watch(sender, filter_debug_data, compressed_debug_data)
            .await
    });
    let mut events = vec![];
    let mut log_tasks = vec![];
    while let Some(event) = recv.next().await {
        match event.payload {
            SuiteEventPayload::RunEvent(RunEvent::CaseStdout { name, mut stdout_message }) => {
                if stdout_message.ends_with("\n") {
                    stdout_message.truncate(stdout_message.len() - 1)
                }
                let logs = stdout_message.split("\n");
                for log in logs {
                    // gtest produces this line when tests are randomized. As of
                    // this writing, our gtest_main binary *always* randomizes.
                    if log.contains("Note: Randomizing tests' orders with a seed of") {
                        continue;
                    }
                    events.push(RunEvent::case_stdout(name.clone(), log.to_string()));
                }
            }
            SuiteEventPayload::RunEvent(RunEvent::CaseStderr { name, mut stderr_message }) => {
                if stderr_message.ends_with("\n") {
                    stderr_message.truncate(stderr_message.len() - 1)
                }
                let logs = stderr_message.split("\n");
                for log in logs {
                    events.push(RunEvent::case_stderr(name.clone(), log.to_string()));
                }
            }
            SuiteEventPayload::RunEvent(e) => events.push(e),
            SuiteEventPayload::SuiteLog { log_stream } => {
                let t = fasync::Task::spawn(log_stream.collect::<Vec<_>>());
                log_tasks.push(t);
            }
            SuiteEventPayload::TestCaseLog { .. } => {
                panic!("not supported yet!")
            }
            SuiteEventPayload::DebugData { filename, socket } => {
                events.push(RunEvent::DebugData { filename, socket })
            }
        }
    }
    execution_task.await.context("test execution failed")?;

    let mut collected_logs = vec![];
    for t in log_tasks {
        let logs = t.await;
        for log_result in logs {
            let log = log_result?;
            collected_logs
                .push(AttributedLog { log: log.msg().unwrap().to_string(), moniker: log.moniker });
        }
    }

    Ok((events, collected_logs))
}

/// Collect bytes from the socket, decompress if required and return the string
pub async fn collect_string_from_socket_helper(
    socket: fidl::Socket,
    compressed_debug_data: bool,
) -> Result<String, anyhow::Error> {
    if !compressed_debug_data {
        return collect_string_from_socket(socket).await;
    }
    let mut async_socket = fidl::AsyncSocket::from_socket(socket);
    let mut buf = vec![0u8; 1024 * 32];

    let (mut decoder, mut receiver) = Decoder::new();
    let task: fasync::Task<Result<(), anyhow::Error>> = fasync::Task::spawn(async move {
        loop {
            let l = async_socket.read(&mut buf).await?;
            match l {
                0 => {
                    decoder.finish().await?;
                    break;
                }
                _ => {
                    decoder.decompress(&buf[..l]).await?;
                }
            }
        }
        Ok(())
    });

    let mut decompressed_data = Vec::new();
    while let Some(chunk) = receiver.next().await {
        decompressed_data.extend_from_slice(&chunk);
    }
    task.await?;
    Ok(String::from_utf8_lossy(decompressed_data.as_slice()).into())
}
/// Runs a test suite.
pub struct SuiteRunner {
    proxy: ftest_manager::SuiteRunnerProxy,
}

impl SuiteRunner {
    /// Create new instance
    pub fn new(proxy: ftest_manager::SuiteRunnerProxy) -> Self {
        Self { proxy }
    }

    pub fn take_proxy(self) -> ftest_manager::SuiteRunnerProxy {
        self.proxy
    }

    /// Starts the suite run, returning the suite run controller wrapped in a SuiteRunInstance.
    pub fn start_suite_run(
        &self,
        test_url: &str,
        options: ftest_manager::RunSuiteOptions,
    ) -> Result<SuiteRunInstance, Error> {
        let (controller_proxy, controller) = fidl::endpoints::create_proxy();
        self.proxy.run(test_url, options, controller).context("Error starting tests")?;

        return Ok(SuiteRunInstance { controller_proxy: controller_proxy.into() });
    }
}

/// Builds and runs test suite(s).
pub struct TestBuilder {
    proxy: ftest_manager::RunBuilderProxy,
    filter_debug_data: bool,
}

impl TestBuilder {
    /// Create new instance
    pub fn new(proxy: ftest_manager::RunBuilderProxy) -> Self {
        Self { proxy, filter_debug_data: false }
    }

    /// Filter out debug data. On coverage builders, tests executed under
    /// test_manager produce coverage profile. This option is useful for
    /// ignoring these and ensuring the caller observes the same events on
    /// all builders.
    pub fn filter_debug_data(self) -> Self {
        let Self { proxy, .. } = self;
        Self { proxy, filter_debug_data: true }
    }

    pub fn take_proxy(self) -> ftest_manager::RunBuilderProxy {
        self.proxy
    }

    pub fn set_scheduling_options(&self, accumulate_debug_data: bool) -> Result<(), Error> {
        self.proxy
            .with_scheduling_options(&ftest_manager::SchedulingOptions {
                accumulate_debug_data: Some(accumulate_debug_data),
                ..Default::default()
            })
            .map_err(Error::from)
    }

    /// Add suite to run.
    pub async fn add_suite(
        &self,
        test_url: &str,
        run_options: ftest_manager::RunOptions,
    ) -> Result<SuiteRunInstance, Error> {
        let (controller_proxy, controller) = fidl::endpoints::create_proxy();
        self.proxy.add_suite(test_url, &run_options, controller)?;
        Ok(SuiteRunInstance { controller_proxy: controller_proxy.into() })
    }

    /// Add suite to run in a realm.
    pub async fn add_suite_in_realm(
        &self,
        realm: fidl::endpoints::ClientEnd<fidl_fuchsia_component::RealmMarker>,
        offers: &[fidl_fuchsia_component_decl::Offer],
        test_collection: &str,
        test_url: &str,
        run_options: ftest_manager::RunOptions,
    ) -> Result<SuiteRunInstance, Error> {
        let (controller_proxy, controller) = fidl::endpoints::create_proxy();
        self.proxy.add_suite_in_realm(
            realm,
            offers,
            test_collection,
            test_url,
            &run_options,
            controller,
        )?;
        Ok(SuiteRunInstance { controller_proxy: controller_proxy.into() })
    }

    /// Runs all tests to completion and collects events alongside uncompressed debug_data.
    /// We will remove this function and merge it with run_with_option function once we remove open to get uncompressed
    /// debug_data.
    pub async fn run(self) -> Result<Vec<TestRunEvent>, Error> {
        self.run_with_option(false).await
    }

    /// Runs all tests to completion and collects events alongside compressed debug_data.
    /// We will remove this function and merge it with run once we remove open to get uncompressed
    /// debug_data.
    pub async fn run_with_option(
        self,
        get_compressed_debug_data: bool,
    ) -> Result<Vec<TestRunEvent>, Error> {
        let (controller_proxy, controller) = fidl::endpoints::create_proxy();
        self.proxy.build(controller).context("Error starting tests")?;
        // wait for test to end
        let mut events = vec![];
        loop {
            let fidl_events = controller_proxy.get_events().await.context("Get run events")?;
            if fidl_events.is_empty() {
                break;
            }
            for fidl_event in fidl_events {
                match fidl_event.payload.expect("Details cannot be empty") {
                    ftest_manager::RunEventPayload::Artifact(
                        ftest_manager::Artifact::DebugData(iterator),
                    ) => {
                        if !self.filter_debug_data {
                            let proxy = iterator.into_proxy();
                            loop {
                                let data = match get_compressed_debug_data {
                                    true => proxy.get_next_compressed().await?,
                                    false => proxy.get_next().await?,
                                };
                                if data.is_empty() {
                                    break;
                                }
                                for data_file in data {
                                    let socket = data_file.socket.expect("File cannot be empty");
                                    events.push(TestRunEvent::debug_data(
                                        fidl_event.timestamp,
                                        data_file.name.expect("Name cannot be empty"),
                                        socket,
                                    ));
                                }
                            }
                        }
                    }
                    other => bail!("Expected only debug data run events but got {:?}", other),
                }
            }
        }
        Ok(events)
    }
}

#[derive(Debug)]
pub struct TestRunEvent {
    pub timestamp: Option<i64>,
    pub payload: TestRunEventPayload,
}

impl TestRunEvent {
    pub fn debug_data<S: Into<String>>(
        timestamp: Option<i64>,
        filename: S,
        socket: fidl::Socket,
    ) -> Self {
        Self {
            timestamp,
            payload: TestRunEventPayload::DebugData { filename: filename.into(), socket },
        }
    }
}

#[derive(Debug)]
pub enum TestRunEventPayload {
    DebugData { filename: String, socket: fidl::Socket },
}

/// Events produced by test suite.
pub struct SuiteEvent {
    pub timestamp: Option<i64>,
    pub payload: SuiteEventPayload,
}

impl SuiteEvent {
    // Note: This is only used with SuiteRunner, not RunBuilder.
    pub fn debug_data<S: Into<String>>(
        timestamp: Option<i64>,
        filename: S,
        socket: fidl::Socket,
    ) -> Self {
        Self {
            timestamp,
            payload: SuiteEventPayload::DebugData { filename: filename.into(), socket },
        }
    }

    pub fn case_found(timestamp: Option<i64>, name: String) -> Self {
        SuiteEvent { timestamp, payload: SuiteEventPayload::RunEvent(RunEvent::case_found(name)) }
    }

    pub fn case_started(timestamp: Option<i64>, name: String) -> Self {
        SuiteEvent { timestamp, payload: SuiteEventPayload::RunEvent(RunEvent::case_started(name)) }
    }

    pub fn case_stdout<N, L>(timestamp: Option<i64>, name: N, stdout_message: L) -> Self
    where
        N: Into<String>,
        L: Into<String>,
    {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::case_stdout(
                name.into(),
                stdout_message.into(),
            )),
        }
    }

    pub fn case_stderr<N, L>(timestamp: Option<i64>, name: N, stderr_message: L) -> Self
    where
        N: Into<String>,
        L: Into<String>,
    {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::case_stderr(
                name.into(),
                stderr_message.into(),
            )),
        }
    }

    pub fn case_stopped(
        timestamp: Option<i64>,
        name: String,
        status: ftest_manager::CaseStatus,
    ) -> Self {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::case_stopped(name, status)),
        }
    }

    pub fn case_finished(timestamp: Option<i64>, name: String) -> Self {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::case_finished(name)),
        }
    }

    pub fn suite_stopped(timestamp: Option<i64>, status: ftest_manager::SuiteStatus) -> Self {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::suite_stopped(status)),
        }
    }

    pub fn suite_custom(
        timestamp: Option<i64>,
        component: String,
        filename: String,
        contents: String,
    ) -> Self {
        SuiteEvent {
            timestamp,
            payload: SuiteEventPayload::RunEvent(RunEvent::suite_custom(
                component, filename, contents,
            )),
        }
    }

    pub fn suite_log(timestamp: Option<i64>, log_stream: LogStream) -> Self {
        SuiteEvent { timestamp, payload: SuiteEventPayload::SuiteLog { log_stream } }
    }

    pub fn test_case_log(timestamp: Option<i64>, name: String, log_stream: LogStream) -> Self {
        SuiteEvent { timestamp, payload: SuiteEventPayload::TestCaseLog { name, log_stream } }
    }
}

pub enum SuiteEventPayload {
    /// Logger for test suite
    SuiteLog {
        log_stream: LogStream,
    },

    /// Logger for a test case in suite.
    TestCaseLog {
        name: String,
        log_stream: LogStream,
    },

    /// Test events.
    RunEvent(RunEvent),

    // Debug data. Note: This is only used with SuiteRunner, not RunBuilder.
    DebugData {
        filename: String,
        socket: fidl::Socket,
    },
}

#[derive(PartialEq, Debug, Eq, Hash, Ord, PartialOrd)]
pub enum RunEvent {
    CaseFound { name: String },
    CaseStarted { name: String },
    CaseStdout { name: String, stdout_message: String },
    CaseStderr { name: String, stderr_message: String },
    CaseStopped { name: String, status: ftest_manager::CaseStatus },
    CaseFinished { name: String },
    SuiteStarted,
    SuiteCustom { component: String, filename: String, contents: String },
    SuiteStopped { status: ftest_manager::SuiteStatus },
    DebugData { filename: String, socket: fidl::Socket },
}

impl RunEvent {
    pub fn case_found<S>(name: S) -> Self
    where
        S: Into<String>,
    {
        Self::CaseFound { name: name.into() }
    }

    pub fn case_started<S>(name: S) -> Self
    where
        S: Into<String>,
    {
        Self::CaseStarted { name: name.into() }
    }

    pub fn case_stdout<S, L>(name: S, stdout_message: L) -> Self
    where
        S: Into<String>,
        L: Into<String>,
    {
        Self::CaseStdout { name: name.into(), stdout_message: stdout_message.into() }
    }

    pub fn case_stderr<S, L>(name: S, stderr_message: L) -> Self
    where
        S: Into<String>,
        L: Into<String>,
    {
        Self::CaseStderr { name: name.into(), stderr_message: stderr_message.into() }
    }

    pub fn case_stopped<S>(name: S, status: ftest_manager::CaseStatus) -> Self
    where
        S: Into<String>,
    {
        Self::CaseStopped { name: name.into(), status }
    }

    pub fn case_finished<S>(name: S) -> Self
    where
        S: Into<String>,
    {
        Self::CaseFinished { name: name.into() }
    }

    pub fn suite_started() -> Self {
        Self::SuiteStarted
    }

    pub fn suite_custom<T, U, V>(component: T, filename: U, contents: V) -> Self
    where
        T: Into<String>,
        U: Into<String>,
        V: Into<String>,
    {
        Self::SuiteCustom {
            component: component.into(),
            filename: filename.into(),
            contents: contents.into(),
        }
    }

    pub fn suite_stopped(status: ftest_manager::SuiteStatus) -> Self {
        Self::SuiteStopped { status }
    }

    pub fn debug_data<S>(filename: S, socket: fidl::Socket) -> Self
    where
        S: Into<String>,
    {
        Self::DebugData { filename: filename.into(), socket }
    }

    /// Returns the name of the test case to which the event belongs, if applicable.
    pub fn test_case_name(&self) -> Option<&String> {
        match self {
            RunEvent::CaseFound { name }
            | RunEvent::CaseStarted { name }
            | RunEvent::CaseStdout { name, .. }
            | RunEvent::CaseStderr { name, .. }
            | RunEvent::CaseStopped { name, .. }
            | RunEvent::CaseFinished { name } => Some(name),
            RunEvent::SuiteStarted
            | RunEvent::SuiteStopped { .. }
            | RunEvent::SuiteCustom { .. }
            | RunEvent::DebugData { .. } => None,
        }
    }

    /// Same as `test_case_name`, but returns an owned `Option<String>`.
    pub fn owned_test_case_name(&self) -> Option<String> {
        self.test_case_name().map(String::from)
    }
}

/// Groups events by stdout, stderr and non stdout/stderr events to make it easy to compare them
/// in tests.
#[derive(Default, Debug, Eq, PartialEq)]
pub struct GroupedRunEvents {
    // order of events is maintained.
    pub non_artifact_events: Vec<RunEvent>,
    // order of stdout events is maintained.
    pub stdout_events: Vec<RunEvent>,
    // order of stderr events is maintained.
    pub stderr_events: Vec<RunEvent>,
}

/// Trait allowing iterators over `RunEvent` to be partitioned by test case name.
pub trait GroupRunEventByTestCase: Iterator<Item = RunEvent> + Sized {
    /// Groups the `RunEvent`s by test case name into a map that preserves insertion order of
    /// various types of events.
    /// The overall order of test cases (by first event) and the orders of events within each test
    /// case are preserved, but events from different test cases are effectively de-interleaved.
    ///
    /// Example:
    /// ```rust
    /// use test_diagnostics::{RunEvent, GroupRunEventByTestCase as _};
    /// use linked_hash_map::LinkedHashMap;
    ///
    /// let events: Vec<RunEvent> = get_events();
    /// let grouped: LinkedHashMap<Option<String>, GroupedRunEvents> =
    ///     events.into_iter().group_by_test_case_ordered();
    /// ```
    fn group_by_test_case_ordered(self) -> LinkedHashMap<Option<String>, GroupedRunEvents> {
        let mut map = LinkedHashMap::new();
        for run_event in self {
            match run_event {
                RunEvent::CaseStderr { .. } => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .stderr_events
                    .push(run_event),

                RunEvent::CaseStdout { .. } => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .stdout_events
                    .push(run_event),

                _ => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .non_artifact_events
                    .push(run_event),
            }
        }
        map
    }

    /// Groups the `RunEvent`s by test case name into an unordered map. The orders of events within
    /// each test case are preserved, but the test cases themselves are not in a defined order.
    fn group_by_test_case_unordered(self) -> HashMap<Option<String>, GroupedRunEvents> {
        let mut map = HashMap::new();
        for run_event in self {
            match run_event {
                RunEvent::CaseStderr { .. } => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .stderr_events
                    .push(run_event),

                RunEvent::CaseStdout { .. } => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .stdout_events
                    .push(run_event),

                _ => map
                    .entry(run_event.owned_test_case_name())
                    .or_insert_with(|| GroupedRunEvents::default())
                    .non_artifact_events
                    .push(run_event),
            }
        }
        map
    }

    /// Group `RunEvent`s by stdout, stderr and non-stdout/err events and returns `GroupedRunEvents`.
    fn group(self) -> GroupedRunEvents {
        let mut events = GroupedRunEvents::default();
        for run_event in self {
            match run_event {
                RunEvent::CaseStderr { .. } => events.stderr_events.push(run_event),

                RunEvent::CaseStdout { .. } => events.stdout_events.push(run_event),

                _ => events.non_artifact_events.push(run_event),
            }
        }
        events
    }
}

impl<T> GroupRunEventByTestCase for T where T: Iterator<Item = RunEvent> + Sized {}

#[derive(Default)]
struct FidlSuiteEventProcessor {
    case_map: HashMap<u32, String>,
    std_output_map: HashMap<u32, Vec<fasync::Task<Result<(), Error>>>>,
}

impl FidlSuiteEventProcessor {
    fn new() -> Self {
        FidlSuiteEventProcessor::default()
    }

    fn get_test_case_name(&self, identifier: u32) -> String {
        self.case_map
            .get(&identifier)
            .unwrap_or_else(|| panic!("invalid test case identifier: {:?}", identifier))
            .clone()
    }

    async fn process(
        &mut self,
        event: FidlSuiteEvent,
        mut sender: mpsc::Sender<SuiteEvent>,
    ) -> Result<(), Error> {
        let timestamp = event.timestamp;
        let e = match event.payload.expect("Details cannot be null, please file bug.") {
            FidlSuiteEventPayload::CaseFound(cf) => {
                self.case_map.insert(cf.identifier, cf.test_case_name.clone());
                SuiteEvent::case_found(timestamp, cf.test_case_name).into()
            }
            FidlSuiteEventPayload::CaseStarted(cs) => {
                let test_case_name = self.get_test_case_name(cs.identifier);
                SuiteEvent::case_started(timestamp, test_case_name).into()
            }
            FidlSuiteEventPayload::CaseStopped(cs) => {
                let test_case_name = self.get_test_case_name(cs.identifier);
                if let Some(outputs) = self.std_output_map.remove(&cs.identifier) {
                    for s in outputs {
                        s.await.context(format!(
                            "error collecting stdout/stderr of {}",
                            test_case_name
                        ))?;
                    }
                }
                SuiteEvent::case_stopped(timestamp, test_case_name, cs.status).into()
            }
            FidlSuiteEventPayload::CaseFinished(cf) => {
                let test_case_name = self.get_test_case_name(cf.identifier);
                SuiteEvent::case_finished(timestamp, test_case_name).into()
            }
            FidlSuiteEventPayload::CaseArtifact(ca) => {
                let name = self.get_test_case_name(ca.identifier);
                match ca.artifact {
                    ftest_manager::Artifact::Stdout(stdout) => {
                        let (s, mut r) = mpsc::channel(1024);
                        let stdout_task =
                            fasync::Task::spawn(collect_and_send_string_output(stdout, s));
                        let mut sender_clone = sender.clone();
                        let send_stdout_task = fasync::Task::spawn(async move {
                            while let Some(msg) = r.next().await {
                                sender_clone
                                    .send(SuiteEvent::case_stdout(None, &name, msg))
                                    .await
                                    .context(format!("cannot send logs for {}", name))?;
                            }
                            Ok(())
                        });
                        match self.std_output_map.get_mut(&ca.identifier) {
                            Some(v) => {
                                v.push(stdout_task);
                                v.push(send_stdout_task);
                            }
                            None => {
                                self.std_output_map
                                    .insert(ca.identifier, vec![stdout_task, send_stdout_task]);
                            }
                        }
                        None
                    }
                    ftest_manager::Artifact::Stderr(stderr) => {
                        let (s, mut r) = mpsc::channel(1024);
                        let stderr_task =
                            fasync::Task::spawn(collect_and_send_string_output(stderr, s));
                        let mut sender_clone = sender.clone();
                        let send_stderr_task = fasync::Task::spawn(async move {
                            while let Some(msg) = r.next().await {
                                sender_clone
                                    .send(SuiteEvent::case_stderr(None, &name, msg))
                                    .await
                                    .context(format!("cannot send logs for {}", name))?;
                            }
                            Ok(())
                        });
                        match self.std_output_map.get_mut(&ca.identifier) {
                            Some(v) => {
                                v.push(stderr_task);
                                v.push(send_stderr_task);
                            }
                            None => {
                                self.std_output_map
                                    .insert(ca.identifier, vec![stderr_task, send_stderr_task]);
                            }
                        }
                        None
                    }
                    ftest_manager::Artifact::Log(log) => match LogStream::from_syslog(log) {
                        Ok(log_stream) => {
                            SuiteEvent::test_case_log(timestamp, name, log_stream).into()
                        }
                        Err(e) => {
                            warn!("Cannot collect logs for test suite: {:?}", e);
                            None
                        }
                    },
                    _ => {
                        panic!("not supported")
                    }
                }
            }
            FidlSuiteEventPayload::SuiteArtifact(sa) => match sa.artifact {
                ftest_manager::Artifact::Stdout(_) => {
                    panic!("not supported")
                }
                ftest_manager::Artifact::Stderr(_) => {
                    panic!("not supported")
                }
                ftest_manager::Artifact::Log(log) => match LogStream::from_syslog(log) {
                    Ok(log_stream) => SuiteEvent::suite_log(timestamp, log_stream).into(),
                    Err(e) => {
                        warn!("Cannot collect logs for test suite: {:?}", e);
                        None
                    }
                },
                ftest_manager::Artifact::Custom(custom_artifact) => {
                    let ftest_manager::DirectoryAndToken { directory, token } =
                        custom_artifact.directory_and_token.unwrap();
                    let component_moniker = custom_artifact.component_moniker.unwrap();
                    let mut sender_clone = sender.clone();
                    fasync::Task::spawn(async move {
                        let directory = directory.into_proxy();
                        let entries: Vec<_> =
                            fuchsia_fs::directory::readdir_recursive(&directory, None)
                                .try_collect()
                                .await
                                .expect("read custom artifact directory");
                        for entry in entries.into_iter() {
                            let file = fuchsia_fs::directory::open_file_async(
                                &directory,
                                &entry.name,
                                fio::PERM_READABLE,
                            )
                            .unwrap();
                            let contents = fuchsia_fs::file::read_to_string(&file).await.unwrap();
                            sender_clone
                                .send(SuiteEvent::suite_custom(
                                    timestamp,
                                    component_moniker.clone(),
                                    entry.name,
                                    contents,
                                ))
                                .await
                                .unwrap();
                        }
                        // Drop the token here - we must keep the token open for the duration that
                        // the directory is in use.
                        drop(token);
                    })
                    .detach();
                    None
                }
                _ => {
                    panic!("not supported")
                }
            },
            FidlSuiteEventPayload::SuiteStarted(_started) => SuiteEvent {
                timestamp,
                payload: SuiteEventPayload::RunEvent(RunEvent::SuiteStarted),
            }
            .into(),
            FidlSuiteEventPayload::SuiteStopped(stopped) => SuiteEvent {
                timestamp,
                payload: SuiteEventPayload::RunEvent(RunEvent::SuiteStopped {
                    status: stopped.status,
                }),
            }
            .into(),
            SuiteEventPayloadUnknown!() => panic!("Unrecognized SuiteEvent"),
        };
        if let Some(item) = e {
            sender.send(item).await.context("Cannot send event")?;
        }
        Ok(())
    }

    async fn process_event(
        &mut self,
        event: ftest_manager::Event,
        mut sender: mpsc::Sender<SuiteEvent>,
        filter_debug_data: bool,
        compressed_debug_data: bool,
    ) -> Result<(), Error> {
        let timestamp = event.timestamp;
        let e = match event.details.expect("Details cannot be null, please file bug.") {
            ftest_manager::EventDetails::TestCaseFound(cf) => {
                let test_case_name =
                    cf.test_case_name.expect("test_case_name must be specified, please file bug.");
                self.case_map.insert(
                    cf.test_case_id.expect("test_case_id must be specified, please file bug."),
                    test_case_name.clone(),
                );
                SuiteEvent::case_found(timestamp, test_case_name).into()
            }
            ftest_manager::EventDetails::TestCaseStarted(cs) => {
                let test_case_name = self.get_test_case_name(
                    cs.test_case_id.expect("test_case_id must be specified, please file bug."),
                );
                SuiteEvent::case_started(timestamp, test_case_name).into()
            }
            ftest_manager::EventDetails::TestCaseStopped(cs) => {
                let test_case_name = self.get_test_case_name(
                    cs.test_case_id.expect("test_case_id must be specified, please file bug."),
                );
                if let Some(outputs) = self.std_output_map.remove(
                    &cs.test_case_id.expect("test_case_id must be specified, please file bug."),
                ) {
                    for s in outputs {
                        s.await.context(format!(
                            "error collecting stdout/stderr of {}",
                            test_case_name
                        ))?;
                    }
                }
                SuiteEvent::case_stopped(
                    timestamp,
                    test_case_name,
                    to_case_status(cs.result.expect("result must be specified, please file bug.")),
                )
                .into()
            }
            ftest_manager::EventDetails::TestCaseFinished(cf) => {
                let test_case_name = self.get_test_case_name(
                    cf.test_case_id.expect("test_case_id must be specified, please file bug."),
                );
                SuiteEvent::case_finished(timestamp, test_case_name).into()
            }
            ftest_manager::EventDetails::TestCaseArtifactGenerated(ca) => {
                let name = self.get_test_case_name(
                    ca.test_case_id.expect("test_case_id must be specified, please file bug."),
                );
                match ca.artifact.expect("artifact must be specified, please file bug.") {
                    ftest_manager::Artifact::Stdout(stdout) => {
                        let (s, mut r) = mpsc::channel(1024);
                        let stdout_task =
                            fasync::Task::spawn(collect_and_send_string_output(stdout, s));
                        let mut sender_clone = sender.clone();
                        let send_stdout_task = fasync::Task::spawn(async move {
                            while let Some(msg) = r.next().await {
                                sender_clone
                                    .send(SuiteEvent::case_stdout(None, &name, msg))
                                    .await
                                    .context(format!("cannot send logs for {}", name))?;
                            }
                            Ok(())
                        });
                        match self.std_output_map.get_mut(
                            &ca.test_case_id
                                .expect("test_case_id must be specified, please file bug."),
                        ) {
                            Some(v) => {
                                v.push(stdout_task);
                                v.push(send_stdout_task);
                            }
                            None => {
                                self.std_output_map.insert(
                                    ca.test_case_id
                                        .expect("test_case_id must be specified, please file bug."),
                                    vec![stdout_task, send_stdout_task],
                                );
                            }
                        }
                        None
                    }
                    ftest_manager::Artifact::Stderr(stderr) => {
                        let (s, mut r) = mpsc::channel(1024);
                        let stderr_task =
                            fasync::Task::spawn(collect_and_send_string_output(stderr, s));
                        let mut sender_clone = sender.clone();
                        let send_stderr_task = fasync::Task::spawn(async move {
                            while let Some(msg) = r.next().await {
                                sender_clone
                                    .send(SuiteEvent::case_stderr(None, &name, msg))
                                    .await
                                    .context(format!("cannot send logs for {}", name))?;
                            }
                            Ok(())
                        });
                        match self.std_output_map.get_mut(
                            &ca.test_case_id
                                .expect("test_case_id must be specified, please file bug."),
                        ) {
                            Some(v) => {
                                v.push(stderr_task);
                                v.push(send_stderr_task);
                            }
                            None => {
                                self.std_output_map.insert(
                                    ca.test_case_id
                                        .expect("test_case_id must be specified, please file bug."),
                                    vec![stderr_task, send_stderr_task],
                                );
                            }
                        }
                        None
                    }
                    ftest_manager::Artifact::Log(log) => match LogStream::from_syslog(log) {
                        Ok(log_stream) => {
                            SuiteEvent::test_case_log(timestamp, name, log_stream).into()
                        }
                        Err(e) => {
                            warn!("Cannot collect logs for test suite: {:?}", e);
                            None
                        }
                    },
                    _ => {
                        panic!("not supported")
                    }
                }
            }
            ftest_manager::EventDetails::SuiteArtifactGenerated(sa) => {
                match sa.artifact.expect("artifact must be specified, please file bug.") {
                    ftest_manager::Artifact::Stdout(_) => {
                        panic!("not supported")
                    }
                    ftest_manager::Artifact::Stderr(_) => {
                        panic!("not supported")
                    }
                    ftest_manager::Artifact::Log(log) => match LogStream::from_syslog(log) {
                        Ok(log_stream) => SuiteEvent::suite_log(timestamp, log_stream).into(),
                        Err(e) => {
                            warn!("Cannot collect logs for test suite: {:?}", e);
                            None
                        }
                    },
                    ftest_manager::Artifact::Custom(custom_artifact) => {
                        let ftest_manager::DirectoryAndToken { directory, token } =
                            custom_artifact.directory_and_token.unwrap();
                        let component_moniker = custom_artifact.component_moniker.unwrap();
                        let mut sender_clone = sender.clone();
                        fasync::Task::spawn(async move {
                            let directory = directory.into_proxy();
                            let entries: Vec<_> =
                                fuchsia_fs::directory::readdir_recursive(&directory, None)
                                    .try_collect()
                                    .await
                                    .expect("read custom artifact directory");
                            for entry in entries.into_iter() {
                                let file = fuchsia_fs::directory::open_file_async(
                                    &directory,
                                    &entry.name,
                                    fio::PERM_READABLE,
                                )
                                .unwrap();
                                let contents =
                                    fuchsia_fs::file::read_to_string(&file).await.unwrap();
                                sender_clone
                                    .send(SuiteEvent::suite_custom(
                                        timestamp,
                                        component_moniker.clone(),
                                        entry.name,
                                        contents,
                                    ))
                                    .await
                                    .unwrap();
                            }
                            // Drop the token here - we must keep the token open for the duration that
                            // the directory is in use.
                            drop(token);
                        })
                        .detach();
                        None
                    }
                    ftest_manager::Artifact::DebugData(iterator) => {
                        if !filter_debug_data {
                            let mut sender_clone = sender.clone();
                            let proxy = iterator.into_proxy();
                            fasync::Task::spawn(async move {
                                loop {
                                    let data = match compressed_debug_data {
                                        true => proxy.get_next_compressed().await.unwrap(),
                                        false => proxy.get_next().await.unwrap(),
                                    };
                                    if data.is_empty() {
                                        break;
                                    }
                                    for data_file in data {
                                        let socket =
                                            data_file.socket.expect("File cannot be empty");
                                        sender_clone
                                            .send(SuiteEvent::debug_data(
                                                timestamp,
                                                data_file.name.expect("Name cannot be empty"),
                                                socket,
                                            ))
                                            .await
                                            .unwrap();
                                    }
                                }
                            })
                            .detach();
                        }
                        None
                    }
                    _ => {
                        panic!("not supported")
                    }
                }
            }
            ftest_manager::EventDetails::SuiteStarted(_started) => SuiteEvent {
                timestamp,
                payload: SuiteEventPayload::RunEvent(RunEvent::SuiteStarted),
            }
            .into(),
            ftest_manager::EventDetails::SuiteStopped(stopped) => SuiteEvent {
                timestamp,
                payload: SuiteEventPayload::RunEvent(RunEvent::SuiteStopped {
                    status: to_suite_status(
                        stopped.result.expect("result must be specified, please file bug."),
                    ),
                }),
            }
            .into(),
            SuiteEventPayloadUnknown!() => panic!("Unrecognized SuiteEvent"),
        };
        if let Some(item) = e {
            sender.send(item).await.context("Cannot send event")?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, Eq, PartialEq, Copy, Clone)]
pub enum SuiteLaunchError {
    #[error("Cannot enumerate tests")]
    CaseEnumeration,

    #[error("Cannot resolve test url")]
    InstanceCannotResolve,

    #[error("Invalid arguments passed")]
    InvalidArgs,

    #[error("Cannot connect to test suite")]
    FailedToConnectToTestSuite,

    #[error("resource unavailable")]
    ResourceUnavailable,

    #[error("Some internal error occurred. Please file bug")]
    InternalError,

    #[error("No test cases matched the provided filters")]
    NoMatchingCases,
}

impl From<ftest_manager::LaunchError> for SuiteLaunchError {
    fn from(error: ftest_manager::LaunchError) -> Self {
        match error {
            ftest_manager::LaunchError::ResourceUnavailable => {
                SuiteLaunchError::ResourceUnavailable
            }
            ftest_manager::LaunchError::InstanceCannotResolve => {
                SuiteLaunchError::InstanceCannotResolve
            }
            ftest_manager::LaunchError::InvalidArgs => SuiteLaunchError::InvalidArgs,
            ftest_manager::LaunchError::FailedToConnectToTestSuite => {
                SuiteLaunchError::FailedToConnectToTestSuite
            }
            ftest_manager::LaunchError::CaseEnumeration => SuiteLaunchError::CaseEnumeration,
            ftest_manager::LaunchError::InternalError => SuiteLaunchError::InternalError,
            ftest_manager::LaunchError::NoMatchingCases => SuiteLaunchError::NoMatchingCases,
            ftest_manager::LaunchErrorUnknown!() => panic!("Encountered unknown launch error"),
        }
    }
}

/// Instance to control a single test suite run.
pub struct SuiteRunInstance {
    controller_proxy: Arc<SuiteControllerProxy>,
}

impl SuiteRunInstance {
    pub fn controller(&self) -> Arc<SuiteControllerProxy> {
        self.controller_proxy.clone()
    }

    pub async fn collect_events(&self, sender: mpsc::Sender<SuiteEvent>) -> Result<(), Error> {
        let controller_proxy = self.controller_proxy.clone();
        let mut processor = FidlSuiteEventProcessor::new();
        loop {
            match controller_proxy.get_events().await? {
                Err(e) => return Err(SuiteLaunchError::from(e).into()),
                Ok(events) => {
                    if events.len() == 0 {
                        break;
                    }
                    for event in events {
                        if let Err(e) = processor.process(event, sender.clone()).await {
                            warn!("error running test suite: {:?}", e);
                            let _ = controller_proxy.kill();
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn collect_events_with_watch(
        &self,
        sender: mpsc::Sender<SuiteEvent>,
        filter_debug_data: bool,
        compressed_debug_data: bool,
    ) -> Result<(), Error> {
        let controller_proxy = self.controller_proxy.clone();
        let mut processor = FidlSuiteEventProcessor::new();
        loop {
            match controller_proxy.watch_events().await? {
                Err(e) => return Err(SuiteLaunchError::from(e).into()),
                Ok(events) => {
                    if events.len() == 0 {
                        break;
                    }
                    for event in events {
                        if let Err(e) = processor
                            .process_event(
                                event,
                                sender.clone(),
                                filter_debug_data,
                                compressed_debug_data,
                            )
                            .await
                        {
                            warn!("error running test suite: {:?}", e);
                            let _ = controller_proxy.kill();
                            return Ok(());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

fn to_case_status(outcome: ftest_manager::TestCaseResult) -> ftest_manager::CaseStatus {
    match outcome {
        ftest_manager::TestCaseResult::Passed => ftest_manager::CaseStatus::Passed,
        ftest_manager::TestCaseResult::Failed => ftest_manager::CaseStatus::Failed,
        ftest_manager::TestCaseResult::TimedOut => ftest_manager::CaseStatus::TimedOut,
        ftest_manager::TestCaseResult::Skipped => ftest_manager::CaseStatus::Skipped,
        ftest_manager::TestCaseResult::Error => ftest_manager::CaseStatus::Error,
        _ => ftest_manager::CaseStatus::Error,
    }
}

fn to_suite_status(outcome: ftest_manager::SuiteResult) -> ftest_manager::SuiteStatus {
    match outcome {
        ftest_manager::SuiteResult::Finished => ftest_manager::SuiteStatus::Passed,
        ftest_manager::SuiteResult::Failed => ftest_manager::SuiteStatus::Failed,
        ftest_manager::SuiteResult::DidNotFinish => ftest_manager::SuiteStatus::DidNotFinish,
        ftest_manager::SuiteResult::TimedOut => ftest_manager::SuiteStatus::TimedOut,
        ftest_manager::SuiteResult::Stopped => ftest_manager::SuiteStatus::Stopped,
        ftest_manager::SuiteResult::InternalError => ftest_manager::SuiteStatus::InternalError,
        _ => ftest_manager::SuiteStatus::InternalError,
    }
}
