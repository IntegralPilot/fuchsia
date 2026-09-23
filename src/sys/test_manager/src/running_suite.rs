// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::above_root_capabilities::AboveRootCapabilitiesForTest;
use crate::constants::{
    CUSTOM_ARTIFACTS_CAPABILITY_NAME, HERMETIC_RESOLVER_REALM_NAME, TEST_ENVIRONMENT_NAME,
    TEST_ROOT_COLLECTION, TEST_ROOT_REALM_NAME, WRAPPER_REALM_NAME,
};
use crate::debug_agent::DebugAgent;
use crate::debug_data_processor::{DebugDataSender, serve_debug_data_publisher};
use crate::error::{DebugAgentError, LaunchTestError};
use crate::offers::apply_offers;
use crate::run_events::SuiteEvents;
use crate::self_diagnostics::DiagnosticNode;
use crate::test_suite::SuiteRealm;
use crate::utilities::stream_fn;
use crate::{diagnostics, facet, resolver};
use anyhow::{Context, Error, anyhow, format_err};
use cm_rust::push_box;
use fidl::endpoints::{ClientEnd, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution::ResolverProxy;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg::PackageResolverProxy;
use fidl_fuchsia_sys2 as fsys;
use fidl_fuchsia_test as ftest;
use fidl_fuchsia_test_manager as ftest_manager;
use fidl_fuchsia_tracing_provider as ftracing_provider;
use ftest::Invocation;
use ftest_manager::{CaseStatus, LaunchError, SuiteStatus};
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_component::client::connect_to_protocol_at_dir_root;
use fuchsia_component_test::error::Error as RealmBuilderError;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use futures::channel::{mpsc, oneshot};
use futures::future::join_all;
use futures::prelude::*;
use futures::{FutureExt, lock};
use log::{debug, info, warn};
use moniker::Moniker;
use resolver::AllowedPackages;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use thiserror::Error;

const DEBUG_DATA_REALM_NAME: &'static str = "debug-data";
const ARCHIVIST_REALM_NAME: &'static str = "archivist";
const DIAGNOSTICS_DICTIONARY_NAME: &'static str = "diagnostics";
const ARCHIVIST_FOR_EMBEDDING_URL: &'static str = "#meta/archivist-for-embedding.cm";
const MEMFS_FOR_EMBEDDING_URL: &'static str = "#meta/memfs.cm";
const MEMFS_REALM_NAME: &'static str = "memfs";

pub const HERMETIC_RESOLVER_CAPABILITY_NAME: &'static str = "hermetic_resolver";

/// A |RunningSuite| represents a launched test component.
pub(crate) struct RunningSuite {
    instance: RealmInstance,
    mock_ready_task: Option<fasync::Task<()>>,
    logs_iterator_task: Option<fasync::Task<Result<(), Error>>>,
    /// A connection to the embedded archivist LogSettings protocol. This connection must be kept
    /// alive for the duration of the test execution for the configured settings to be persisted.
    log_settings: Option<fdiagnostics::LogSettingsProxy>,
    /// Server ends of event pairs used to track if a client is accessing a component's
    /// custom storage. Used to defer destruction of the realm until clients have completed
    /// reading the storage.
    custom_artifact_tokens: Vec<zx::EventPair>,

    /// `Realm` protocol for the test root.
    test_realm_proxy: fcomponent::RealmProxy,
    /// exposed directory of the test realm.
    exposed_dir: fio::DirectoryProxy,

    /// Wrapper for `DebugAgent` service.
    debug_agent: Option<DebugAgent>,
}

impl RunningSuite {
    /// Launch a suite component.
    pub(crate) async fn launch(
        test_url: &str,
        facets: facet::SuiteFacets,
        resolver: Arc<ResolverProxy>,
        pkg_resolver: Arc<PackageResolverProxy>,
        above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
        debug_data_sender: DebugDataSender,
        diagnostics: &DiagnosticNode,
        suite_realm: &Option<SuiteRealm>,
        use_debug_agent: bool,
    ) -> Result<Self, LaunchTestError> {
        info!(test_url, diagnostics:?, collection = facets.collection; "Starting test suite.");

        let test_package = fuchsia_url::AbsoluteComponentUrl::parse(test_url)
            .map_err(|e| {
                warn!(url:?=test_url; "invalid component url: {:#}", anyhow!(e));
                LaunchTestError::InvalidResolverData
            })?
            .path()
            .as_ref()
            .ok_or_else(|| {
                warn!(url:?=test_url; "component url missing path");
                LaunchTestError::InvalidResolverData
            })?
            .to_string();
        above_root_capabilities_for_test
            .validate(facets.collection)
            .map_err(LaunchTestError::ValidateTestRealm)?;
        let (builder, mock_ready_event) = get_realm(
            test_url,
            test_package.as_ref(),
            &facets,
            above_root_capabilities_for_test,
            resolver,
            pkg_resolver,
            debug_data_sender,
            suite_realm,
        )
        .await
        .map_err(LaunchTestError::InitializeTestRealm)?;
        let instance = builder.build().await.map_err(LaunchTestError::CreateTestRealm)?;
        let test_realm_proxy: fcomponent::RealmProxy = instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .map_err(|e| LaunchTestError::ConnectToTestSuite(e))?;
        let collection_ref = fdecl::CollectionRef { name: TEST_ROOT_COLLECTION.into() };
        let child_decl = fdecl::Child {
            name: Some(TEST_ROOT_REALM_NAME.into()),
            url: Some(test_url.into()),
            startup: Some(fdecl::StartupMode::Eager),
            environment: None,
            ..Default::default()
        };
        test_realm_proxy
            .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
            .await
            .map_err(|e| LaunchTestError::CreateTestFidl(e))?
            .map_err(|e| LaunchTestError::CreateTest(e))?;

        let (exposed_dir, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let child_ref = fdecl::ChildRef {
            name: TEST_ROOT_REALM_NAME.into(),
            collection: Some(TEST_ROOT_COLLECTION.into()),
        };
        test_realm_proxy
            .open_exposed_dir(&child_ref, server_end)
            .await
            .map_err(|e| LaunchTestError::ConnectToTestSuite(e.into()))?
            .map_err(|e| {
                LaunchTestError::ConnectToTestSuite(format_err!(
                    "failed to open exposed dir: {:?}",
                    e
                ))
            })?;

        Ok(RunningSuite {
            custom_artifact_tokens: vec![],
            mock_ready_task: Some(fasync::Task::spawn(
                mock_ready_event.wait_or_dropped().map(|_| ()),
            )),
            logs_iterator_task: None,
            log_settings: None,
            instance,
            test_realm_proxy,
            exposed_dir,
            debug_agent: if use_debug_agent {
                DebugAgent::new().await.map_err(log_warning).ok()
            } else {
                warn!(
                    "Not using debug_agent for test {} because it is marked create_no_exception_channel",
                    test_url
                );
                None
            },
        })
    }

    pub(crate) async fn run_tests(
        &mut self,
        test_url: &str,
        mut options: ftest_manager::RunOptions,
        mut sender: mpsc::Sender<Result<SuiteEvents, LaunchError>>,
        mut stop_recv: oneshot::Receiver<()>,
    ) {
        debug!("running test suite {}", test_url);

        let (log_iterator, syslog) = match options.log_iterator {
            Some(ftest_manager::LogsIteratorOption::SocketBatchIterator) => {
                let (local, remote) = zx::Socket::create_stream();
                (ftest_manager::LogsIterator::Stream(local), ftest_manager::Syslog::Stream(remote))
            }
            _ => {
                let (proxy, request) = fidl::endpoints::create_endpoints();
                (ftest_manager::LogsIterator::Batch(request), ftest_manager::Syslog::Batch(proxy))
            }
        };

        sender.send(Ok(SuiteEvents::suite_syslog(syslog).into())).await.unwrap();

        if let Some(log_interest) = options.log_interest.take() {
            let log_settings: fdiagnostics::LogSettingsProxy =
                match self.instance.root.connect_to_protocol_at_exposed_dir() {
                    Ok(proxy) => proxy,
                    Err(e) => {
                        warn!("Error connecting to LogSettings");
                        sender
                            .send(Err(LaunchTestError::ConnectToLogSettings(e.into()).into()))
                            .await
                            .unwrap();
                        return;
                    }
                };

            let fut = log_settings.set_component_interest(
                &fdiagnostics::LogSettingsSetComponentInterestRequest {
                    selectors: Some(log_interest),
                    persist: Some(false),
                    ..Default::default()
                },
            );
            if let Err(e) = fut.await {
                warn!("Error setting log interest");
                sender.send(Err(LaunchTestError::SetLogInterest(e.into()).into())).await.unwrap();
                return;
            }
            self.log_settings = Some(log_settings);
        }

        let archive_accessor = match self
            .instance
            .root
            .connect_to_protocol_at_exposed_dir::<fdiagnostics::ArchiveAccessorProxy>()
        {
            Ok(accessor) => match accessor.wait_for_ready().await {
                Ok(()) => accessor,
                Err(e) => {
                    warn!("Error connecting to ArchiveAccessor");
                    sender
                        .send(Err(LaunchTestError::ConnectToArchiveAccessor(e.into()).into()))
                        .await
                        .unwrap();
                    return;
                }
            },
            Err(e) => {
                warn!("Error connecting to ArchiveAccessor");
                sender
                    .send(Err(LaunchTestError::ConnectToArchiveAccessor(e.into()).into()))
                    .await
                    .unwrap();
                return;
            }
        };
        let host_archive_accessor = match self.instance.root.connect_to_protocol_at_exposed_dir() {
            Ok(accessor) => accessor,
            Err(e) => {
                warn!("Error connecting to ArchiveAccessor");
                sender
                    .send(Err(LaunchTestError::ConnectToArchiveAccessor(e.into()).into()))
                    .await
                    .unwrap();
                return;
            }
        };

        match diagnostics::serve_syslog(archive_accessor, host_archive_accessor, log_iterator) {
            Ok(diagnostics::ServeSyslogOutcome { logs_iterator_task }) => {
                self.logs_iterator_task = logs_iterator_task;
            }
            Err(e) => {
                warn!("Error spawning iterator server: {:?}", e);
                sender.send(Err(LaunchTestError::StreamIsolatedLogs(e).into())).await.unwrap();
                return;
            }
        };

        let fut = async {
            let matcher = match options.case_filters_to_run.as_ref() {
                Some(filters) => match CaseMatcher::new(filters) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        sender.send(Err(LaunchError::InvalidArgs)).await.unwrap();
                        return Err(e);
                    }
                },
                None => None,
            };

            let suite = self.connect_to_suite()?;
            let invocations = match enumerate_test_cases(&suite, matcher.as_ref()).await {
                Ok(i) if i.is_empty() && matcher.is_some() => {
                    sender.send(Err(LaunchError::NoMatchingCases)).await.unwrap();
                    return Err(format_err!("Found no matching cases using {:?}", matcher));
                }
                Ok(i) => i,
                Err(e) => {
                    sender.send(Err(LaunchError::CaseEnumeration)).await.unwrap();
                    return Err(e);
                }
            };
            if let Ok(Some(_)) = stop_recv.try_recv() {
                sender
                    .send(Ok(SuiteEvents::suite_stopped(SuiteStatus::Stopped).into()))
                    .await
                    .unwrap();
                return Ok(());
            }

            let mut suite_status = SuiteStatus::Passed;
            let mut invocations_iter = invocations.into_iter();
            let counter = AtomicU32::new(0);
            let timeout_time = match options.timeout {
                Some(t) => zx::MonotonicInstant::after(zx::MonotonicDuration::from_nanos(t)),
                None => zx::MonotonicInstant::INFINITE,
            };
            let timeout_fut = fasync::Timer::new(timeout_time).shared();

            let run_options = get_invocation_options(options);

            sender.send(Ok(SuiteEvents::suite_started().into())).await.unwrap();

            loop {
                const INVOCATIONS_CHUNK: usize = 50;
                let chunk = invocations_iter.by_ref().take(INVOCATIONS_CHUNK).collect::<Vec<_>>();
                if chunk.is_empty() {
                    break;
                }
                if let Ok(Some(_)) = stop_recv.try_recv() {
                    sender
                        .send(Ok(SuiteEvents::suite_stopped(SuiteStatus::Stopped).into()))
                        .await
                        .unwrap();
                    return self.report_custom_artifacts(&mut sender).await;
                }
                let res = match run_invocations(
                    &suite,
                    chunk,
                    run_options.clone(),
                    &counter,
                    &mut sender,
                    timeout_fut.clone(),
                )
                .await
                .context("Error running test cases")
                {
                    Ok(success) => success,
                    Err(e) => {
                        return Err(e);
                    }
                };
                if res == SuiteStatus::TimedOut {
                    if let Some(debug_agent) = &self.debug_agent {
                        debug_agent.report_all_backtraces(Some(test_url), sender.clone()).await;
                    }
                    sender
                        .send(Ok(SuiteEvents::suite_stopped(SuiteStatus::TimedOut).into()))
                        .await
                        .unwrap();
                    return self.report_custom_artifacts(&mut sender).await;
                }
                suite_status = concat_suite_status(suite_status, res);
            }
            sender.send(Ok(SuiteEvents::suite_stopped(suite_status).into())).await.unwrap();
            if let Some(debug_agent) = &mut self.debug_agent {
                let mut fatal_exception_receiver = debug_agent.take_fatal_exception_receiver();
                debug_agent.stop_sending_fatal_exceptions();
                loop {
                    match fatal_exception_receiver.next().await {
                        Some(backtrace_info) => {
                            let (client_end, mut server_end) =
                                fidl::handle::fuchsia_handles::Socket::create_stream();
                            sender
                                .send(Ok(SuiteEvents::suite_stderr(client_end).into()))
                                .await
                                .unwrap();

                            warn!("fatal exception occurred, thread {:?}", backtrace_info.thread);
                            let _ = server_end.write(
                                format!("fatal exception, thread {:?}\n", backtrace_info.thread)
                                    .as_bytes(),
                            );
                            DebugAgent::write_backtrace_info(&backtrace_info, &mut server_end)
                                .await;
                        }
                        None => {
                            break;
                        }
                    }
                }
            }
            self.report_custom_artifacts(&mut sender).await
        };
        if let Err(e) = fut.await {
            warn!("Error running test {}: {:?}", test_url, e);
        }
    }

    /// Find any custom artifact users under the test realm and report them via sender.
    async fn report_custom_artifacts(
        &mut self,
        sender: &mut mpsc::Sender<Result<SuiteEvents, LaunchError>>,
    ) -> Result<(), Error> {
        // TODO(https://fxbug.dev/42074399): Support custom artifacts when test is running in outside
        // realm.
        let artifact_storage_admin = self.connect_to_storage_admin()?;

        let root_moniker = "./";
        let (iterator, iter_server) = create_proxy::<fsys::StorageIteratorMarker>();
        artifact_storage_admin
            .list_storage_in_realm(&root_moniker, iter_server)
            .await?
            .map_err(|e| format_err!("Error listing storage users in test realm: {:?}", e))?;
        let stream = stream_fn(move || iterator.next());
        futures::pin_mut!(stream);
        while let Some(storage_moniker) = stream.try_next().await? {
            let (node, server) = fidl::endpoints::create_endpoints::<fio::NodeMarker>();
            let directory: ClientEnd<fio::DirectoryMarker> = node.into_channel().into();
            artifact_storage_admin.open_storage(&storage_moniker, server).await?.map_err(|e| {
                format_err!("Error opening component storage in test realm: {:?}", e)
            })?;
            let (event_client, event_server) = zx::EventPair::create();
            self.custom_artifact_tokens.push(event_server);

            // Monikers should be reported relative to the test root, so strip away the wrapping
            // components from the path.
            let moniker_parsed = Moniker::try_from(storage_moniker.as_str()).unwrap();
            let moniker_relative_to_test_root = if moniker_parsed.is_root() {
                moniker_parsed
            } else {
                Moniker::new_from_borrowed(&moniker_parsed.path()[1..])
            };
            sender
                .send(Ok(SuiteEvents::suite_custom_artifact(ftest_manager::CustomArtifact {
                    directory_and_token: Some(ftest_manager::DirectoryAndToken {
                        directory,
                        token: event_client,
                    }),
                    component_moniker: Some(moniker_relative_to_test_root.to_string()),
                    ..Default::default()
                })
                .into()))
                .await
                .unwrap();
        }
        Ok(())
    }

    pub(crate) fn connect_to_suite(&self) -> Result<ftest::SuiteProxy, LaunchTestError> {
        connect_to_protocol_at_dir_root::<ftest::SuiteMarker>(&self.exposed_dir)
            .map_err(|e| LaunchTestError::ConnectToTestSuite(e))
    }

    fn connect_to_storage_admin(&self) -> Result<fsys::StorageAdminProxy, LaunchTestError> {
        self.instance
            .root
            .connect_to_protocol_at_exposed_dir()
            .map_err(|e| LaunchTestError::ConnectToStorageAdmin(e))
    }

    /// Mark the resources associated with the suite for destruction, then wait for destruction to
    /// complete. Returns an error only if destruction fails.
    pub(crate) async fn destroy(self, diagnostics: DiagnosticNode) -> Result<(), Error> {
        let exposed_dir_fut = self.exposed_dir.close();
        let exposed_dir_close_task = fasync::Task::spawn(async move {
            let _ = exposed_dir_fut.await;
        });

        // before destroying the realm, wait for any clients to finish accessing storage.
        // TODO(https://fxbug.dev/42165719): Remove signal for USER_0, this is used while Overnet does not support
        // signalling ZX_EVENTPAIR_CLOSED when the eventpair is closed.
        let tokens_closed_signals = self.custom_artifact_tokens.iter().map(|token| {
            fasync::OnSignals::new(token, zx::Signals::EVENTPAIR_PEER_CLOSED | zx::Signals::USER_0)
                .unwrap_or_else(|_| zx::Signals::empty())
        });
        if let Some(mock_ready_task) = self.mock_ready_task {
            info!(diagnostics:?; "Waiting on mock_ready_task...");
            mock_ready_task.await;
        }
        info!(diagnostics:?; "Waiting on tokens_closed_signals...");
        futures::future::join_all(tokens_closed_signals).await;

        info!(diagnostics:?; "Waiting on exposed_dir_close_task...");
        exposed_dir_close_task.await;

        info!(diagnostics:?; "Start destroying test realm...");

        // TODO(https://fxbug.dev/42174479) Remove timeout once component manager hangs are removed.
        // This value is set to be slightly longer than the shutdown timeout for tests (30 sec).
        const TEARDOWN_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(32);

        // Make the call to destroy the test, before destroying the entire realm. Once this
        // completes, it guarantees that any of its service providers (archivist, storage,
        // debugdata) have received all outgoing requests from the test such as log connections,
        // etc.
        let child_ref = fdecl::ChildRef {
            name: TEST_ROOT_REALM_NAME.into(),
            collection: Some(TEST_ROOT_COLLECTION.into()),
        };
        self.test_realm_proxy
            .destroy_child(&child_ref)
            .map_err(|e| Error::from(e).context("call to destroy test failed"))
            // This should not hang, but wrap it in a timeout just in case.
            .on_timeout(TEARDOWN_TIMEOUT, || Err(anyhow!("Timeout waiting for test to destroy")))
            .await?
            .map_err(|e| format_err!("call to destroy test failed: {:?}", e))?;

        #[derive(Debug, Error)]
        enum TeardownError {
            #[error("timeout")]
            Timeout,
            #[error("{}", .0)]
            Other(Error),
        }
        // When serving logs over ArchiveIterator in the host, we should also wait for all logs to
        // be drained.
        let logs_iterator_task = self
            .logs_iterator_task
            .unwrap_or_else(|| fasync::Task::spawn(futures::future::ready(Ok(()))));
        let (logs_iterator_res, teardown_res) = futures::future::join(
            logs_iterator_task.map_err(|e| TeardownError::Other(e)).on_timeout(
                TEARDOWN_TIMEOUT,
                || {
                    // This log is detected in triage. Update the config in
                    // src/diagnostics/config/triage/test_manager.triage when changing this log.
                    warn!("Test manager timeout draining logs");
                    Err(TeardownError::Timeout)
                },
            ),
            self.instance.destroy().map_err(|e| TeardownError::Other(e.into())).on_timeout(
                TEARDOWN_TIMEOUT,
                || {
                    // This log is detected in triage. Update the config in
                    // src/diagnostics/config/triage/test_manager.triage when changing this log.
                    warn!("Test manager timeout destroying realm");
                    Err(TeardownError::Timeout)
                },
            ),
        )
        .await;
        let logs_iterator_res = match logs_iterator_res {
            Err(TeardownError::Other(e)) => {
                // Allow test teardown to proceed if failed to stream logs
                warn!("Error streaming logs: {}", e);
                Ok(())
            }
            r => r,
        };
        match (logs_iterator_res, teardown_res) {
            (Err(e1), Err(e2)) => {
                Err(anyhow!("Draining logs failed: {}. Realm teardown failed: {}", e1, e2))
            }
            (Err(e), Ok(())) => Err(anyhow!("Draining logs failed: {}", e)),
            (Ok(()), Err(e)) => Err(anyhow!("Realm teardown failed: {}", e)),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

/// Logs an error an returns it.
fn log_warning(error: DebugAgentError) -> DebugAgentError {
    warn!("{}", error);
    error
}

/// Enumerates test cases and return invocations.
pub(crate) async fn enumerate_test_cases(
    suite: &ftest::SuiteProxy,
    matcher: Option<&CaseMatcher>,
) -> Result<Vec<Invocation>, anyhow::Error> {
    debug!("enumerating tests");
    let (case_iterator, server_end) = fidl::endpoints::create_proxy();
    suite.get_tests(server_end).map_err(enumeration_error)?;
    let mut invocations = vec![];

    loop {
        let cases = case_iterator.get_next().await.map_err(enumeration_error)?;
        if cases.is_empty() {
            break;
        }
        for case in cases {
            let case_name =
                case.name.ok_or_else(|| format_err!("invocation should contain a name."))?;
            if matcher.as_ref().map_or(true, |m| m.matches(&case_name)) {
                invocations.push(Invocation {
                    name: Some(case_name),
                    tag: None,
                    ..Default::default()
                });
            }
        }
    }

    debug!("invocations: {:#?}", invocations);

    Ok(invocations)
}

pub(crate) struct CaseMatcher {
    /// Patterns specifying cases to include.
    includes: Vec<glob::Pattern>,
    /// Patterns specifying cases to exclude.
    excludes: Vec<glob::Pattern>,
}

impl fmt::Debug for CaseMatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CaseMatcher")
            .field("includes", &self.includes.iter().map(|p| p.to_string()).collect::<Vec<_>>())
            .field("excludes", &self.excludes.iter().map(|p| p.to_string()).collect::<Vec<_>>())
            .finish()
    }
}

impl CaseMatcher {
    fn new<T: AsRef<str>>(filters: &Vec<T>) -> Result<Self, anyhow::Error> {
        let mut matcher = CaseMatcher { includes: vec![], excludes: vec![] };
        filters.iter().try_for_each(|filter| {
            match filter.as_ref().chars().next() {
                Some('-') => {
                    let pattern = glob::Pattern::new(&filter.as_ref()[1..])
                        .map_err(|e| format_err!("Bad negative test filter pattern: {}", e))?;
                    matcher.excludes.push(pattern);
                }
                Some(_) | None => {
                    let pattern = glob::Pattern::new(&filter.as_ref())
                        .map_err(|e| format_err!("Bad test filter pattern: {}", e))?;
                    matcher.includes.push(pattern);
                }
            }
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(matcher)
    }

    /// Returns whether or not a case should be run.
    fn matches(&self, case: &str) -> bool {
        let matches_includes = match self.includes.is_empty() {
            true => true,
            false => self.includes.iter().any(|p| p.matches(case)),
        };
        matches_includes && !self.excludes.iter().any(|p| p.matches(case))
    }
}

fn get_allowed_package_value(suite_facet: &facet::SuiteFacets) -> AllowedPackages {
    match &suite_facet.deprecated_allowed_packages {
        Some(deprecated_allowed_packages) => {
            AllowedPackages::from_iter(deprecated_allowed_packages.iter().cloned())
        }
        None => AllowedPackages::zero_allowed_pkgs(),
    }
}

/// Create a RealmBuilder and populate it with local components and routes needed to run a
/// test. Returns a populated RealmBuilder instance, and an Event which is signalled when
/// the debug_data local component is ready to accept connection requests.
// TODO(https://fxbug.dev/42056523): The returned event is part of a synchronization mechanism to
// work around cases when a component is destroyed before starting, even though there is a
// request. In this case debug data can be lost. Remove the synchronization once it is
// provided by cm.
async fn get_realm(
    test_url: &str,
    test_package: &str,
    suite_facet: &facet::SuiteFacets,
    above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    resolver: Arc<ResolverProxy>,
    pkg_resolver: Arc<PackageResolverProxy>,
    debug_data_sender: DebugDataSender,
    suite_realm: &Option<SuiteRealm>,
) -> Result<(RealmBuilder, async_utils::event::Event), RealmBuilderError> {
    let mut realm_param = RealmBuilderParams::new();
    realm_param = match suite_realm {
        Some(suite_realm) => realm_param
            .with_realm_proxy(suite_realm.realm_proxy.clone())
            .in_collection(suite_realm.test_collection.clone()),
        None => realm_param.in_collection(suite_facet.collection.to_string()),
    };
    let builder = RealmBuilder::with_params(realm_param).await?;
    let wrapper_realm =
        builder.add_child_realm(WRAPPER_REALM_NAME, ChildOptions::new().eager()).await?;

    let hermetic_test_package_name = Arc::new(test_package.to_owned());
    let other_allowed_packages = get_allowed_package_value(&suite_facet);

    let hermetic_test_package_name_clone = hermetic_test_package_name.clone();
    let other_allowed_packages_clone = other_allowed_packages.clone();
    let resolver = wrapper_realm
        .add_local_child(
            HERMETIC_RESOLVER_REALM_NAME,
            move |handles| {
                Box::pin(resolver::serve_hermetic_resolver(
                    handles,
                    hermetic_test_package_name_clone.clone(),
                    other_allowed_packages_clone.clone(),
                    resolver.clone(),
                    pkg_resolver.clone(),
                ))
            },
            ChildOptions::new(),
        )
        .await?;

    // Provide and expose the debug data capability to the test environment.
    let owned_url = test_url.to_string();
    let mock_ready_event = async_utils::event::Event::new();
    let mock_ready_clone = mock_ready_event.clone();
    let debug_data = wrapper_realm
        .add_local_child(
            DEBUG_DATA_REALM_NAME,
            move |handles| {
                Box::pin(serve_debug_data_publisher(
                    handles,
                    owned_url.clone(),
                    debug_data_sender.clone(),
                    mock_ready_clone.clone(),
                ))
            },
            // This component is launched eagerly so that debug_data always signals ready, even
            // in absence of a connection request. TODO(https://fxbug.dev/42056523): Remove once
            // synchronization is provided by cm.
            ChildOptions::new().eager(),
        )
        .await?;

    let mut debug_data_decl = wrapper_realm.get_component_decl(&debug_data).await?;
    push_box(
        &mut debug_data_decl.exposes,
        cm_rust::ExposeDecl::Protocol(cm_rust::ExposeProtocolDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: "fuchsia.debugdata.Publisher".parse().unwrap(),
            source_dictionary: Default::default(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: "fuchsia.debugdata.Publisher".parse().unwrap(),
            availability: cm_rust::Availability::Required,
        }),
    );
    push_box(
        &mut debug_data_decl.capabilities,
        cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
            name: "fuchsia.debugdata.Publisher".parse().unwrap(),
            source_path: Some("/svc/fuchsia.debugdata.Publisher".parse().unwrap()),
            delivery: Default::default(),
        }),
    );
    wrapper_realm.replace_component_decl(&debug_data, debug_data_decl).await?;

    // Provide and expose the resolver capability from the resolver to test_wrapper.
    let mut hermetic_resolver_decl =
        wrapper_realm.get_component_decl(HERMETIC_RESOLVER_REALM_NAME).await?;
    push_box(
        &mut hermetic_resolver_decl.exposes,
        cm_rust::ExposeDecl::Resolver(cm_rust::ExposeResolverDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: HERMETIC_RESOLVER_CAPABILITY_NAME.parse().unwrap(),
            source_dictionary: Default::default(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: HERMETIC_RESOLVER_CAPABILITY_NAME.parse().unwrap(),
        }),
    );
    push_box(
        &mut hermetic_resolver_decl.capabilities,
        cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl {
            name: HERMETIC_RESOLVER_CAPABILITY_NAME.parse().unwrap(),
            source_path: Some("/svc/fuchsia.component.resolution.Resolver".parse().unwrap()),
        }),
    );

    wrapper_realm
        .replace_component_decl(HERMETIC_RESOLVER_REALM_NAME, hermetic_resolver_decl)
        .await?;

    let memfs = wrapper_realm
        .add_child(MEMFS_REALM_NAME, MEMFS_FOR_EMBEDDING_URL, ChildOptions::new())
        .await?;

    // Create the hermetic environment in the test_wrapper.
    let mut test_wrapper_decl = wrapper_realm.get_realm_decl().await?;
    push_box(
        &mut test_wrapper_decl.environments,
        cm_rust::EnvironmentDecl {
            name: TEST_ENVIRONMENT_NAME.parse().unwrap(),
            extends: fdecl::EnvironmentExtends::Realm,
            resolvers: Box::from([cm_rust::ResolverRegistration {
                resolver: HERMETIC_RESOLVER_CAPABILITY_NAME.parse().unwrap(),
                source: cm_rust::RegistrationSource::Child(String::from(
                    HERMETIC_RESOLVER_CAPABILITY_NAME,
                )),
                scheme: String::from("fuchsia-pkg"),
            }]),
            runners: Box::from([]),
            debug_capabilities: Box::from([cm_rust::DebugRegistration::Protocol(
                cm_rust::DebugProtocolRegistration {
                    source_name: "fuchsia.debugdata.Publisher".parse().unwrap(),
                    source: cm_rust::RegistrationSource::Child(DEBUG_DATA_REALM_NAME.to_string()),
                    target_name: "fuchsia.debugdata.Publisher".parse().unwrap(),
                },
            )]),
            stop_timeout_ms: None,
        },
    );

    // Add the collection to hold the test. This lets us individually destroy the test.
    push_box(
        &mut test_wrapper_decl.collections,
        cm_rust::CollectionDecl {
            name: TEST_ROOT_COLLECTION.parse().unwrap(),
            durability: fdecl::Durability::Transient,
            environment: Some(TEST_ENVIRONMENT_NAME.parse().unwrap()),
            allowed_offers: cm_types::AllowedOffers::StaticOnly,
            allow_long_names: false,
            persistent_storage: None,
        },
    );

    push_box(
        &mut test_wrapper_decl.capabilities,
        cm_rust::CapabilityDecl::Storage(cm_rust::StorageDecl {
            name: CUSTOM_ARTIFACTS_CAPABILITY_NAME.parse().unwrap(),
            source: cm_rust::StorageDirectorySource::Child(MEMFS_REALM_NAME.to_string()),
            backing_dir: "memfs".parse().unwrap(),
            subdir: "custom_artifacts".parse().unwrap(),
            storage_id: fdecl::StorageId::StaticInstanceIdOrMoniker,
        }),
    );

    push_box(
        &mut test_wrapper_decl.capabilities,
        cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: DIAGNOSTICS_DICTIONARY_NAME.parse().unwrap(),
            source_path: None,
        }),
    );

    wrapper_realm.replace_realm_decl(test_wrapper_decl).await?;

    let test_root = Ref::collection(TEST_ROOT_COLLECTION);
    let archivist = wrapper_realm
        .add_child(ARCHIVIST_REALM_NAME, ARCHIVIST_FOR_EMBEDDING_URL, ChildOptions::new().eager())
        .await?;

    // Parent to archivist
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary(DIAGNOSTICS_DICTIONARY_NAME))
                .from(Ref::parent())
                .to(&wrapper_realm),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::dictionary(DIAGNOSTICS_DICTIONARY_NAME))
                .from(Ref::parent())
                .to(&memfs),
        )
        .await?;

    // archivist to test root
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.Log"))
                .from(&archivist)
                .to(test_root.clone()),
        )
        .await?;

    // It does not seem possible to route diagnostics capabilities from Archivist to the resolver,
    // so we must route them from the parent.
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.inspect.InspectSink"))
                .from(Ref::dictionary(Ref::parent(), DIAGNOSTICS_DICTIONARY_NAME))
                .to(&resolver),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.diagnostics.LogStream"))
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.inspect.InspectSink"))
                .from(&archivist)
                .to(test_root.clone())
                .to(Ref::dictionary(Ref::self_(), DIAGNOSTICS_DICTIONARY_NAME)),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.debugdata.Publisher"))
                .from(&debug_data)
                .to(Ref::dictionary(Ref::self_(), DIAGNOSTICS_DICTIONARY_NAME)),
        )
        .await?;

    // Diagnostics dictionary to resolver and test_root
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::dictionary(DIAGNOSTICS_DICTIONARY_NAME))
                .from(Ref::self_())
                .to(test_root.clone())
                .to(&resolver),
        )
        .await?;

    // archivist to parent and test root
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                .capability(Capability::protocol::<
                    fidl_fuchsia_diagnostics_host::ArchiveAccessorMarker,
                >())
                .from(Ref::dictionary(&archivist, "diagnostics-accessors"))
                .to(Ref::parent())
                .to(test_root.clone()),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fdiagnostics::LogSettingsMarker>())
                .from(&archivist)
                .to(Ref::parent()),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::storage(CUSTOM_ARTIFACTS_CAPABILITY_NAME))
                .from(Ref::self_())
                .to(test_root.clone()),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsys::StorageAdminMarker>())
                .from(Ref::capability(CUSTOM_ARTIFACTS_CAPABILITY_NAME))
                .to(Ref::parent()),
        )
        .await?;

    // We need to expose the raw protocols so that the driver test realm's driver framework
    // can use it in order to provide support for subpackaged drivers.
    wrapper_realm
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol_by_name("fuchsia.component.resolution.Resolver-hermetic")
                        .path("/svc/fuchsia.component.resolution.Resolver"),
                )
                .from(&resolver)
                .to(test_root.clone()),
        )
        .await?;
    wrapper_realm
        .add_route(
            Route::new()
                .capability(
                    Capability::protocol_by_name("fuchsia.pkg.PackageResolver-hermetic")
                        .path("/svc/fuchsia.pkg.PackageResolver"),
                )
                .from(&resolver)
                .to(test_root.clone()),
        )
        .await?;

    // wrapper realm to parent
    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(Ref::framework())
                .to(Ref::parent()),
        )
        .await?;

    // wrapper realm to archivist

    wrapper_realm
        .add_route(
            Route::new()
                .capability(
                    Capability::event_stream("capability_requested").with_scope(test_root.clone()),
                )
                .from(Ref::parent())
                .to(&archivist),
        )
        .await?;

    wrapper_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol::<ftracing_provider::RegistryMarker>().optional())
                .from(Ref::void())
                .to(&archivist),
        )
        .await?;

    builder
        .add_route(
            Route::new()
                // from archivist
                .capability(Capability::protocol::<fdiagnostics::ArchiveAccessorMarker>())
                .capability(Capability::protocol::<
                    fidl_fuchsia_diagnostics_host::ArchiveAccessorMarker,
                >())
                .capability(Capability::protocol::<fdiagnostics::LogSettingsMarker>())
                // from test root
                .capability(Capability::protocol::<ftest::SuiteMarker>())
                // custom_artifact capability
                .capability(Capability::protocol::<fsys::StorageAdminMarker>())
                .capability(Capability::protocol::<fcomponent::RealmMarker>())
                .from(&wrapper_realm)
                .to(Ref::parent()),
        )
        .await?;
    if let Some(suite_realm) = suite_realm {
        apply_offers(&builder, &wrapper_realm, &suite_realm.offers).await?;
    } else {
        above_root_capabilities_for_test
            .apply(suite_facet.collection, &builder, &wrapper_realm)
            .await?;
    }

    Ok((builder, mock_ready_event))
}

fn get_invocation_options(options: ftest_manager::RunOptions) -> ftest::RunOptions {
    ftest::RunOptions {
        include_disabled_tests: options.run_disabled_tests,
        parallel: options.parallel,
        arguments: options.arguments,
        break_on_failure: options.break_on_failure,
        ..Default::default()
    }
}

/// Runs the test component using `suite` and collects stdout logs and results.
async fn run_invocations(
    suite: &ftest::SuiteProxy,
    invocations: Vec<Invocation>,
    run_options: fidl_fuchsia_test::RunOptions,
    counter: &AtomicU32,
    sender: &mut mpsc::Sender<Result<SuiteEvents, LaunchError>>,
    timeout_fut: futures::future::Shared<fasync::Timer>,
) -> Result<SuiteStatus, anyhow::Error> {
    let (run_listener_client, mut run_listener) = fidl::endpoints::create_request_stream();
    suite.run(&invocations, &run_options, run_listener_client)?;

    let tasks = Arc::new(lock::Mutex::new(vec![]));
    let running_test_cases = Arc::new(lock::Mutex::new(HashSet::new()));
    let tasks_clone = tasks.clone();
    let initial_suite_status: SuiteStatus;
    let mut sender_clone = sender.clone();
    let test_fut = async {
        let mut initial_suite_status = SuiteStatus::DidNotFinish;
        while let Some(result_event) =
            run_listener.try_next().await.context("error waiting for listener")?
        {
            match result_event {
                ftest::RunListenerRequest::OnTestCaseStarted {
                    invocation,
                    std_handles,
                    listener,
                    control_handle: _,
                } => {
                    let name = invocation
                        .name
                        .ok_or_else(|| format_err!("cannot find name in invocation"))?;
                    let identifier = counter.fetch_add(1, Ordering::Relaxed);
                    let events = vec![
                        Ok(SuiteEvents::case_found(identifier, name).into()),
                        Ok(SuiteEvents::case_started(identifier).into()),
                    ];
                    for event in events {
                        sender_clone.send(event).await.unwrap();
                    }
                    let listener = listener.into_stream();
                    running_test_cases.lock().await.insert(identifier);
                    let running_test_cases = running_test_cases.clone();
                    let mut sender = sender_clone.clone();
                    let task = fasync::Task::spawn(async move {
                        let _ = &std_handles;
                        let mut sender_stdout = sender.clone();
                        let mut sender_stderr = sender.clone();
                        let completion_fut = async {
                            let status = listen_for_completion(listener).await;
                            sender
                                .send(Ok(
                                    SuiteEvents::case_stopped(identifier, status.clone()).into()
                                ))
                                .await
                                .unwrap();
                            status
                        };
                        let ftest::StdHandles { out, err, .. } = std_handles;
                        let stdout_fut = async move {
                            if let Some(out) = out {
                                match fasync::OnSignals::new(
                                    &out,
                                    zx::Signals::SOCKET_READABLE | zx::Signals::SOCKET_PEER_CLOSED,
                                )
                                .await
                                {
                                    Ok(signals)
                                        if signals.contains(zx::Signals::SOCKET_READABLE) =>
                                    {
                                        sender_stdout
                                            .send(Ok(
                                                SuiteEvents::case_stdout(identifier, out).into()
                                            ))
                                            .await
                                            .unwrap();
                                    }
                                    Ok(_) | Err(_) => (),
                                }
                            }
                        };
                        let stderr_fut = async move {
                            if let Some(err) = err {
                                match fasync::OnSignals::new(
                                    &err,
                                    zx::Signals::SOCKET_READABLE | zx::Signals::SOCKET_PEER_CLOSED,
                                )
                                .await
                                {
                                    Ok(signals)
                                        if signals.contains(zx::Signals::SOCKET_READABLE) =>
                                    {
                                        sender_stderr
                                            .send(Ok(
                                                SuiteEvents::case_stderr(identifier, err).into()
                                            ))
                                            .await
                                            .unwrap();
                                    }
                                    Ok(_) | Err(_) => (),
                                }
                            }
                        };
                        let (status, (), ()) =
                            futures::future::join3(completion_fut, stdout_fut, stderr_fut).await;

                        sender
                            .send(Ok(SuiteEvents::case_finished(identifier).into()))
                            .await
                            .unwrap();
                        running_test_cases.lock().await.remove(&identifier);
                        status
                    });
                    tasks_clone.lock().await.push(task);
                }
                ftest::RunListenerRequest::OnFinished { .. } => {
                    initial_suite_status = SuiteStatus::Passed;
                    break;
                }
            }
        }
        Ok(initial_suite_status)
    }
    .fuse();

    futures::pin_mut!(test_fut);
    let timeout_fut = timeout_fut.fuse();
    futures::pin_mut!(timeout_fut);

    futures::select! {
        () = timeout_fut => {
            let mut all_tasks = vec![];
            let mut tasks = tasks.lock().await;
            all_tasks.append(&mut tasks);
            drop(tasks);
            drop(all_tasks);
            let running_test_cases = running_test_cases.lock().await;
            for i in &*running_test_cases {
                sender
                    .send(Ok(SuiteEvents::case_stopped(*i, CaseStatus::TimedOut).into()))
                    .await
                    .unwrap();
                sender
                    .send(Ok(SuiteEvents::case_finished(*i).into()))
                    .await
                    .unwrap();
            }
            return Ok(SuiteStatus::TimedOut);
        }
        r = test_fut => {
            initial_suite_status = match r {
                Err(e) => {
                    return Err(e);
                }
                Ok(s) => s,
            };
        }
    }

    let mut tasks = tasks.lock().await;
    let mut all_tasks = vec![];
    all_tasks.append(&mut tasks);
    // await for all invocations to complete for which test case never completed.
    let suite_status = join_all(all_tasks)
        .await
        .into_iter()
        .fold(initial_suite_status, get_suite_status_from_case_status);
    Ok(suite_status)
}

fn concat_suite_status(initial: SuiteStatus, new: SuiteStatus) -> SuiteStatus {
    if initial.into_primitive() > new.into_primitive() {
        return initial;
    }
    return new;
}

fn get_suite_status_from_case_status(
    initial_suite_status: SuiteStatus,
    case_status: CaseStatus,
) -> SuiteStatus {
    let status = match case_status {
        CaseStatus::Passed => SuiteStatus::Passed,
        CaseStatus::Failed => SuiteStatus::Failed,
        CaseStatus::TimedOut => SuiteStatus::TimedOut,
        CaseStatus::Skipped => SuiteStatus::Passed,
        CaseStatus::Error => SuiteStatus::DidNotFinish,
        _ => {
            panic!("this should not happen");
        }
    };
    concat_suite_status(initial_suite_status, status)
}

fn enumeration_error(err: fidl::Error) -> anyhow::Error {
    match err {
        fidl::Error::ClientChannelClosed { .. } => anyhow::anyhow!(
            "The test protocol was closed during enumeration. This may mean that the test is using
            wrong test runner or `fuchsia.test.Suite` was not configured correctly. Refer to: \
            https://fuchsia.dev/go/components/test-errors"
        ),
        err => err.into(),
    }
}

/// Listen for test completion on the given |listener|. Returns None if the channel is closed
/// before a test completion event.
async fn listen_for_completion(mut listener: ftest::CaseListenerRequestStream) -> CaseStatus {
    match listener.try_next().await.context("waiting for listener") {
        Ok(Some(request)) => {
            let ftest::CaseListenerRequest::Finished { result, control_handle: _ } = request;
            let result = match result.status {
                Some(status) => match status {
                    fidl_fuchsia_test::Status::Passed => CaseStatus::Passed,
                    fidl_fuchsia_test::Status::Failed => CaseStatus::Failed,
                    fidl_fuchsia_test::Status::Skipped => CaseStatus::Skipped,
                },
                // This will happen when test protocol is not properly implemented
                // by the test and it forgets to set the result.
                None => CaseStatus::Error,
            };
            result
        }
        Err(e) => {
            warn!("listener failed: {:?}", e);
            CaseStatus::Error
        }
        Ok(None) => CaseStatus::Error,
    }
}

#[cfg(test)]
mod tests {
    use crate::constants::{HERMETIC_TESTS_COLLECTION, SYSTEM_TESTS_COLLECTION};

    use super::*;
    use maplit::hashset;

    #[test]
    fn case_matcher_tests() {
        let all_test_case_names = hashset! {
            "Foo.Test1", "Foo.Test2", "Foo.Test3", "Bar.Test1", "Bar.Test2", "Bar.Test3"
        };

        let cases = vec![
            (vec![], all_test_case_names.clone()),
            (vec!["Foo.Test1"], hashset! {"Foo.Test1"}),
            (vec!["Foo.*"], hashset! {"Foo.Test1", "Foo.Test2", "Foo.Test3"}),
            (vec!["-Foo.*"], hashset! {"Bar.Test1", "Bar.Test2", "Bar.Test3"}),
            (vec!["Foo.*", "-*.Test2"], hashset! {"Foo.Test1", "Foo.Test3"}),
        ];

        for (filters, expected_matching_cases) in cases.into_iter() {
            let case_matcher = CaseMatcher::new(&filters).expect("Create case matcher");
            for test_case in all_test_case_names.iter() {
                match expected_matching_cases.contains(test_case) {
                    true => assert!(
                        case_matcher.matches(test_case),
                        "Expected filters {:?} to match test case name {}",
                        filters,
                        test_case
                    ),
                    false => assert!(
                        !case_matcher.matches(test_case),
                        "Expected filters {:?} to not match test case name {}",
                        filters,
                        test_case
                    ),
                }
            }
        }
    }

    #[test]
    fn suite_status() {
        let all_case_status = vec![
            CaseStatus::Error,
            CaseStatus::TimedOut,
            CaseStatus::Failed,
            CaseStatus::Skipped,
            CaseStatus::Passed,
        ];
        for status in &all_case_status {
            assert_eq!(
                get_suite_status_from_case_status(SuiteStatus::InternalError, *status),
                SuiteStatus::InternalError
            );
        }

        for status in &all_case_status {
            let s = get_suite_status_from_case_status(SuiteStatus::TimedOut, *status);
            assert_eq!(s, SuiteStatus::TimedOut);
        }

        for status in &all_case_status {
            let s = get_suite_status_from_case_status(SuiteStatus::DidNotFinish, *status);
            let mut expected = SuiteStatus::DidNotFinish;
            if status == &CaseStatus::TimedOut {
                expected = SuiteStatus::TimedOut;
            }
            assert_eq!(s, expected);
        }

        for status in &all_case_status {
            let s = get_suite_status_from_case_status(SuiteStatus::Failed, *status);
            let expected = match *status {
                CaseStatus::TimedOut => SuiteStatus::TimedOut,
                CaseStatus::Error => SuiteStatus::DidNotFinish,
                _ => SuiteStatus::Failed,
            };
            assert_eq!(s, expected);
        }

        for status in &all_case_status {
            let s = get_suite_status_from_case_status(SuiteStatus::Passed, *status);
            let mut expected = SuiteStatus::Passed;
            if status == &CaseStatus::Error {
                expected = SuiteStatus::DidNotFinish;
            }
            if status == &CaseStatus::TimedOut {
                expected = SuiteStatus::TimedOut;
            }
            if status == &CaseStatus::Failed {
                expected = SuiteStatus::Failed;
            }
            assert_eq!(s, expected);
        }

        let all_suite_status = vec![
            SuiteStatus::Passed,
            SuiteStatus::Failed,
            SuiteStatus::TimedOut,
            SuiteStatus::Stopped,
            SuiteStatus::InternalError,
        ];

        for initial_status in &all_suite_status {
            for status in &all_suite_status {
                let s = concat_suite_status(*initial_status, *status);
                let expected: SuiteStatus;
                if initial_status.into_primitive() > status.into_primitive() {
                    expected = *initial_status;
                } else {
                    expected = *status;
                }

                assert_eq!(s, expected);
            }
        }
    }

    #[test]
    fn test_allowed_packages_value() {
        let mut suite_facet = facet::SuiteFacets {
            collection: HERMETIC_TESTS_COLLECTION,
            deprecated_allowed_packages: None,
        };

        // default for hermetic realm is no other allowed package.
        assert_eq!(AllowedPackages::zero_allowed_pkgs(), get_allowed_package_value(&suite_facet));

        // deprecated_allowed_packages overrides HERMETIC_TESTS_COLLECTION
        suite_facet.deprecated_allowed_packages = Some(vec!["pkg-one".to_owned()]);
        assert_eq!(
            AllowedPackages::from_iter(["pkg-one".to_owned()]),
            get_allowed_package_value(&suite_facet)
        );

        // test with other collections
        suite_facet.collection = SYSTEM_TESTS_COLLECTION;
        assert_eq!(
            AllowedPackages::from_iter(["pkg-one".to_owned()]),
            get_allowed_package_value(&suite_facet)
        );

        // test default with other collection
        suite_facet.deprecated_allowed_packages = None;
        assert_eq!(AllowedPackages::zero_allowed_pkgs(), get_allowed_package_value(&suite_facet));
    }
}
