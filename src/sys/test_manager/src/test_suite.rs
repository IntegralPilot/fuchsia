// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::above_root_capabilities::AboveRootCapabilitiesForTest;
use crate::constants::{self, HERMETIC_TESTS_COLLECTION};
use crate::debug_data_processor::{DebugDataDirectory, DebugDataProcessor, DebugDataSender};
use crate::error::*;
use crate::facet::SuiteFacets;
use crate::run_events::{RunEvent, SuiteEvents};
use crate::scheduler::Scheduler;
use crate::self_diagnostics::DiagnosticNode;
use crate::{facet, running_suite, scheduler};
use anyhow::Error;
use fidl::endpoints::Responder;
use fidl_fuchsia_component::RealmProxy;
use fidl_fuchsia_component_resolution::ResolverProxy;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_pkg::PackageResolverProxy;
use fidl_fuchsia_test_manager as ftest_manager;
use ftest_manager::{
    LaunchError, RunControllerRequest, RunControllerRequestStream, SchedulingOptions,
    SuiteControllerRequest, SuiteControllerRequestStream,
};
use fuchsia_async as fasync;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::future::Either;
use futures::prelude::*;
use log::{error, info, warn};
use std::sync::Arc;

const EXECUTION_PROPERTY: &'static str = "execution";

pub(crate) struct SuiteRealm {
    pub realm_proxy: RealmProxy,
    pub offers: Vec<ftest::Capability>,
    pub test_collection: String,
}

pub(crate) struct Suite {
    pub test_url: String,
    pub options: ftest_manager::RunOptions,
    pub controller: SuiteControllerRequestStream,
    pub resolver: Arc<ResolverProxy>,
    pub pkg_resolver: Arc<PackageResolverProxy>,
    pub above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    pub facets: facet::ResolveStatus,
    pub realm: Option<SuiteRealm>,
}

pub(crate) struct TestRunBuilder {
    pub suites: Vec<Suite>,
}

impl TestRunBuilder {
    /// Serve a RunControllerRequestStream. Returns Err if the client stops the test
    /// prematurely or there is an error serving he stream.
    /// Be careful, |run_task| is dropped once the other end of |controller| is dropped
    /// or |kill| on the controller is called.
    async fn run_controller(
        mut controller: RunControllerRequestStream,
        run_task: futures::future::RemoteHandle<()>,
        stop_sender: oneshot::Sender<()>,
        event_recv: mpsc::Receiver<RunEvent>,
        diagnostics: DiagnosticNode,
    ) {
        let mut run_task = Some(run_task);
        let mut stop_sender = Some(stop_sender);
        let (events_responder_sender, mut events_responder_recv) = mpsc::unbounded();
        let diagnostics_ref = &diagnostics;

        let serve_controller_fut = async move {
            while let Some(request) = controller.try_next().await? {
                match request {
                    RunControllerRequest::Stop { .. } => {
                        diagnostics_ref.set_flag("stopped");
                        if let Some(stop_sender) = stop_sender.take() {
                            // no need to check error.
                            let _ = stop_sender.send(());
                            // after this all `senders` go away and subsequent GetEvent call will
                            // return rest of events and eventually a empty array and will close the
                            // connection after that.
                        }
                    }
                    RunControllerRequest::Kill { .. } => {
                        diagnostics_ref.set_flag("killed");
                        // dropping the remote handle cancels it.
                        drop(run_task.take());
                        // after this all `senders` go away and subsequent GetEvent call will
                        // return rest of events and eventually a empty array and will close the
                        // connection after that.
                    }
                    RunControllerRequest::GetEvents { responder } => {
                        events_responder_sender.unbounded_send(responder).unwrap_or_else(|e| {
                            // If the handler is already done, drop responder without closing the
                            // channel.
                            e.into_inner().drop_without_shutdown();
                        })
                    }
                    RunControllerRequest::_UnknownMethod { ordinal, control_handle, .. } => {
                        warn!(
                            "Unknown run controller request received: {}, closing connection",
                            ordinal
                        );
                        // dropping the remote handle cancels it.
                        drop(run_task.take());
                        control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                        break;
                    }
                }
            }
            Result::<(), Error>::Ok(())
        };

        let get_events_fut = async move {
            let mut event_chunks = event_recv.map(RunEvent::into).ready_chunks(EVENTS_THRESHOLD);
            while let Some(responder) = events_responder_recv.next().await {
                diagnostics_ref.set_property("events", "awaiting");
                let next_chunk = event_chunks.next().await.unwrap_or_default();
                diagnostics_ref.set_property("events", "idle");
                let done = next_chunk.is_empty();
                responder.send(next_chunk)?;
                if done {
                    diagnostics_ref.set_flag("events_drained");
                    break;
                }
            }
            // Drain remaining responders without dropping them so that the channel doesn't
            // close.
            events_responder_recv.close();
            events_responder_recv
                .for_each(|responder| async move {
                    responder.drop_without_shutdown();
                })
                .await;
            Result::<(), Error>::Ok(())
        };

        match futures::future::select(serve_controller_fut.boxed(), get_events_fut.boxed()).await {
            Either::Left((serve_result, _fut)) => {
                if let Err(e) = serve_result {
                    warn!(diagnostics:?; "Error serving RunController: {:?}", e);
                }
            }
            Either::Right((get_events_result, serve_fut)) => {
                if let Err(e) = get_events_result {
                    warn!(diagnostics:?; "Error sending events for RunController: {:?}", e);
                }
                // Wait for the client to close the channel.
                // TODO(https://fxbug.dev/42169156) once https://fxbug.dev/42169061 is fixed, this is no longer
                // necessary.
                if let Err(e) = serve_fut.await {
                    warn!(diagnostics:?; "Error serving RunController: {:?}", e);
                }
            }
        }
    }

    pub(crate) async fn run(
        self,
        controller: RunControllerRequestStream,
        diagnostics: DiagnosticNode,
        scheduling_options: Option<SchedulingOptions>,
    ) {
        let (stop_sender, mut stop_recv) = oneshot::channel::<()>();
        let (event_sender, event_recv) = mpsc::channel::<RunEvent>(16);

        let diagnostics_ref = &diagnostics;

        let max_parallel_suites = match &scheduling_options {
            Some(options) => options.max_parallel_suites,
            None => None,
        };
        let max_parallel_suites_ref = &max_parallel_suites;
        let accumulate_debug_data = scheduling_options
            .as_ref()
            .and_then(|options| options.accumulate_debug_data)
            .unwrap_or(false);
        let debug_data_directory = match accumulate_debug_data {
            true => DebugDataDirectory::Accumulating { dir: constants::DEBUG_DATA_FOR_SCP },
            false => DebugDataDirectory::Isolated { parent: constants::ISOLATED_TMP },
        };
        let (debug_data_processor, debug_data_sender) =
            DebugDataProcessor::new(debug_data_directory);

        let debug_task = fasync::Task::local(
            debug_data_processor
                .collect_and_serve(event_sender)
                .unwrap_or_else(|err| warn!(err:?; "Error serving debug data")),
        );

        // This future returns the task which needs to be completed before completion.
        let suite_scheduler_fut = async move {
            diagnostics_ref.set_property(EXECUTION_PROPERTY, "executing");

            let serial_executor = scheduler::SerialScheduler {};

            match max_parallel_suites_ref {
                Some(max_parallel_suites) => {
                    let parallel_executor = scheduler::ParallelScheduler {
                        suite_runner: scheduler::RunSuiteObj {},
                        max_parallel_suites: *max_parallel_suites,
                    };
                    let get_facets_fn = |test_url, resolver| async move {
                        facet::get_suite_facets(test_url, resolver).await
                    };
                    let (serial_suites, parallel_suites) =
                        split_suites_by_hermeticity(self.suites, get_facets_fn).await;

                    parallel_executor
                        .execute(
                            parallel_suites,
                            diagnostics_ref.child("parallel_executor"),
                            &mut stop_recv,
                            debug_data_sender.clone(),
                        )
                        .await;
                    serial_executor
                        .execute(
                            serial_suites,
                            diagnostics_ref.child("serial_executor"),
                            &mut stop_recv,
                            debug_data_sender.clone(),
                        )
                        .await;
                }
                None => {
                    serial_executor
                        .execute(
                            self.suites,
                            diagnostics_ref.child("serial_executor"),
                            &mut stop_recv,
                            debug_data_sender.clone(),
                        )
                        .await;
                }
            }

            drop(debug_data_sender); // needed for debug_data_processor to complete.

            diagnostics_ref.set_property(EXECUTION_PROPERTY, "complete");
        };

        let (remote, remote_handle) = suite_scheduler_fut.remote_handle();

        let ((), ()) = futures::future::join(
            remote.then(|_| async move {
                debug_task.await;
            }),
            Self::run_controller(
                controller,
                remote_handle,
                stop_sender,
                event_recv,
                diagnostics.child("controller"),
            ),
        )
        .await;
    }
}

// max events to send so that we don't cross fidl limits.
// TODO(https://fxbug.dev/42051179): Use tape measure to calculate limit.
const EVENTS_THRESHOLD: usize = 50;

impl Suite {
    pub(crate) async fn run(self, diagnostics: DiagnosticNode, accumulate_debug_data: bool) {
        let diagnostics_ref = &diagnostics;

        let debug_data_directory = match accumulate_debug_data {
            true => DebugDataDirectory::Accumulating { dir: constants::DEBUG_DATA_FOR_SCP },
            false => DebugDataDirectory::Isolated { parent: constants::ISOLATED_TMP },
        };
        let (debug_data_processor, debug_data_sender) =
            DebugDataProcessor::new(debug_data_directory);

        let (event_sender, event_receiver) = mpsc::channel(1024);

        let debug_task = fasync::Task::local(
            debug_data_processor
                .collect_and_serve_for_suite(event_sender.clone())
                .unwrap_or_else(|err| warn!(err:?; "Error serving debug data")),
        );

        // This future returns the task which needs to be completed before completion.
        let suite_run_fut = async move {
            diagnostics_ref.set_property(EXECUTION_PROPERTY, "executing");

            let suite_node = diagnostics_ref.child("serial_executor").child("suite-0");
            suite_node.set_property("url", self.test_url.clone());
            run_single_suite_for_suite_runner(
                self,
                debug_data_sender,
                suite_node,
                event_sender,
                event_receiver,
            )
            .await;

            diagnostics_ref.set_property(EXECUTION_PROPERTY, "complete");
        };

        suite_run_fut
            .then(|_| async move {
                debug_task.await;
            })
            .await;
    }

    async fn run_controller(
        mut controller: SuiteControllerRequestStream,
        stop_sender: oneshot::Sender<()>,
        run_suite_remote_handle: futures::future::RemoteHandle<()>,
        event_recv: mpsc::Receiver<Result<SuiteEvents, LaunchError>>,
    ) -> Result<(), Error> {
        let mut task = Some(run_suite_remote_handle);
        let mut stop_sender = Some(stop_sender);
        let (events_responder_sender, mut events_responder_recv) = mpsc::unbounded();

        let serve_controller_fut = async move {
            while let Some(event) = controller.try_next().await? {
                match event {
                    SuiteControllerRequest::Stop { .. } => {
                        // no need to handle error as task might already have finished.
                        if let Some(stop) = stop_sender.take() {
                            let _ = stop.send(());
                            // after this all `senders` go away and subsequent GetEvent call will
                            // return rest of event. Eventually an empty array and will close the
                            // connection after that.
                        }
                    }
                    SuiteControllerRequest::Kill { .. } => {
                        // Dropping the remote handle for the suite execution task cancels it.
                        drop(task.take());
                        // after this all `senders` go away and subsequent GetEvent call will
                        // return rest of event. Eventually an empty array and will close the
                        // connection after that.
                    }
                    SuiteControllerRequest::WatchEvents { responder } => {
                        events_responder_sender
                            .unbounded_send(EventResponder::New(responder))
                            .unwrap_or_else(|e| {
                                // If the handler is already done, drop responder without closing the
                                // channel.
                                e.into_inner().drop_without_shutdown();
                            })
                    }
                    SuiteControllerRequest::GetEvents { responder } => {
                        events_responder_sender
                            .unbounded_send(EventResponder::Deprecated(responder))
                            .unwrap_or_else(|e| {
                                // If the handler is already done, drop responder without closing the
                                // channel.
                                e.into_inner().drop_without_shutdown();
                            })
                    }
                    SuiteControllerRequest::_UnknownMethod { ordinal, control_handle, .. } => {
                        warn!(
                            "Unknown suite controller request received: {}, closing connection",
                            ordinal
                        );
                        // Dropping the remote handle for the suite execution task cancels it.
                        drop(task.take());
                        control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                        break;
                    }
                }
            }
            Ok(())
        };

        let get_events_fut = async move {
            let mut event_chunks = event_recv.ready_chunks(EVENTS_THRESHOLD);
            while let Some(responder) = events_responder_recv.next().await {
                let next_chunk_results: Vec<Result<_, _>> =
                    event_chunks.next().await.unwrap_or_default();
                if responder.send(next_chunk_results)? {
                    break;
                }
            }
            // Drain remaining responders without dropping them so that the channel doesn't
            // close.
            events_responder_recv.close();
            events_responder_recv
                .for_each(|responder| async move {
                    responder.drop_without_shutdown();
                })
                .await;
            Result::<(), Error>::Ok(())
        };

        match futures::future::select(serve_controller_fut.boxed(), get_events_fut.boxed()).await {
            Either::Left((serve_result, _fut)) => serve_result,
            Either::Right((get_events_result, serve_fut)) => {
                get_events_result?;
                // Wait for the client to close the channel.
                // TODO(https://fxbug.dev/42169156) once https://fxbug.dev/42169061 is fixed, this is no longer
                // necessary.
                serve_fut.await
            }
        }
    }
}

enum EventResponder {
    Deprecated(ftest_manager::SuiteControllerGetEventsResponder),
    New(ftest_manager::SuiteControllerWatchEventsResponder),
}

impl EventResponder {
    fn drop_without_shutdown(self) {
        match self {
            EventResponder::Deprecated(inner) => inner.drop_without_shutdown(),
            EventResponder::New(inner) => inner.drop_without_shutdown(),
        }
    }

    pub fn send(self, results: Vec<Result<SuiteEvents, LaunchError>>) -> Result<bool, fidl::Error> {
        match self {
            EventResponder::Deprecated(inner) => {
                let result: Result<Vec<_>, _> =
                    results.into_iter().map(|r| r.map(SuiteEvents::into)).collect();
                let done = match &result {
                    Ok(events) => events.is_empty(),
                    Err(_) => true,
                };
                inner.send(result).map(|_| done)
            }
            EventResponder::New(inner) => {
                let result: Result<Vec<_>, _> =
                    results.into_iter().map(|r| r.map(SuiteEvents::into)).collect();
                let done = match &result {
                    Ok(events) => events.is_empty(),
                    Err(_) => true,
                };
                inner.send(result).map(|_| done)
            }
        }
    }
}

async fn run_single_suite_for_suite_runner(
    suite: Suite,
    debug_data_sender: DebugDataSender,
    diagnostics: DiagnosticNode,
    mut event_sender: mpsc::Sender<Result<SuiteEvents, LaunchError>>,
    event_recv: mpsc::Receiver<Result<SuiteEvents, LaunchError>>,
) {
    let (stop_sender, stop_recv) = oneshot::channel::<()>();

    let Suite {
        test_url,
        options,
        controller,
        resolver,
        pkg_resolver,
        above_root_capabilities_for_test,
        facets,
        realm: suite_realm,
    } = suite;

    let run_test_fut = async {
        diagnostics.set_property(EXECUTION_PROPERTY, "get_facets");

        let facets = match facets {
            // Currently, all suites are passed in with unresolved facets by the
            // SerialScheduler. ParallelScheduler will pass in Resolved facets
            // once it is implemented.
            facet::ResolveStatus::Resolved(result) => {
                match result {
                    Ok(facets) => facets,

                    // This error is reported here instead of when the error was
                    // first encountered because here is where it has access to
                    // the SuiteController protocol server (Suite::run_controller)
                    // which can report the error back to the test_manager client
                    Err(error) => {
                        event_sender.send(Err(error.into())).await.unwrap();
                        return;
                    }
                }
            }
            facet::ResolveStatus::Unresolved => {
                match facet::get_suite_facets(test_url.clone(), resolver.clone()).await {
                    Ok(facets) => facets,
                    Err(error) => {
                        event_sender.send(Err(error.into())).await.unwrap();
                        return;
                    }
                }
            }
        };
        diagnostics.set_property(EXECUTION_PROPERTY, "launch");
        match running_suite::RunningSuite::launch(
            &test_url,
            facets,
            resolver,
            pkg_resolver,
            above_root_capabilities_for_test,
            debug_data_sender,
            &diagnostics,
            &suite_realm,
            use_debug_agent_for_runs(&options),
        )
        .await
        {
            Ok(mut instance) => {
                diagnostics.set_property(EXECUTION_PROPERTY, "run_tests");
                instance.run_tests(&test_url, options, event_sender, stop_recv).await;
                diagnostics.set_property(EXECUTION_PROPERTY, "tests_done");
                diagnostics.set_property(EXECUTION_PROPERTY, "tear_down");
                if let Err(err) = instance.destroy(diagnostics.child("destroy")).await {
                    // Failure to destroy an instance could mean that some component events fail to send.
                    error!(
                        diagnostics:?,
                        err:?;
                        "Failed to destroy instance. Debug data may be lost."
                    );
                }
            }
            Err(e) => {
                event_sender.send(Err(e.into())).await.unwrap();
            }
        }
    };
    let (run_test_remote, run_test_handle) = run_test_fut.remote_handle();

    let controller_fut =
        Suite::run_controller(controller, stop_sender, run_test_handle, event_recv);
    let ((), controller_ret) = futures::future::join(run_test_remote, controller_fut).await;

    if let Err(e) = controller_ret {
        warn!(diagnostics:?; "Ended test {}: {:?}", test_url, e);
    }

    diagnostics.set_property(EXECUTION_PROPERTY, "complete");
    info!(diagnostics:?; "Test destruction complete");
}

pub(crate) async fn run_single_suite(
    suite: Suite,
    debug_data_sender: DebugDataSender,
    diagnostics: DiagnosticNode,
) {
    let (mut sender, recv) = mpsc::channel(1024);
    let (stop_sender, stop_recv) = oneshot::channel::<()>();
    let mut maybe_instance = None;

    let Suite {
        test_url,
        options,
        controller,
        resolver,
        pkg_resolver,
        above_root_capabilities_for_test,
        facets,
        realm: suite_realm,
    } = suite;

    let run_test_fut = async {
        diagnostics.set_property(EXECUTION_PROPERTY, "get_facets");

        let facets = match facets {
            // Currently, all suites are passed in with unresolved facets by the
            // SerialScheduler. ParallelScheduler will pass in Resolved facets
            // once it is implemented.
            facet::ResolveStatus::Resolved(result) => {
                match result {
                    Ok(facets) => facets,

                    // This error is reported here instead of when the error was
                    // first encountered because here is where it has access to
                    // the SuiteController protocol server (Suite::run_controller)
                    // which can report the error back to the test_manager client
                    Err(error) => {
                        sender.send(Err(error.into())).await.unwrap();
                        return;
                    }
                }
            }
            facet::ResolveStatus::Unresolved => {
                match facet::get_suite_facets(test_url.clone(), resolver.clone()).await {
                    Ok(facets) => facets,
                    Err(error) => {
                        sender.send(Err(error.into())).await.unwrap();
                        return;
                    }
                }
            }
        };
        diagnostics.set_property(EXECUTION_PROPERTY, "launch");
        match running_suite::RunningSuite::launch(
            &test_url,
            facets,
            resolver,
            pkg_resolver,
            above_root_capabilities_for_test,
            debug_data_sender,
            &diagnostics,
            &suite_realm,
            use_debug_agent_for_runs(&options),
        )
        .await
        {
            Ok(instance) => {
                diagnostics.set_property(EXECUTION_PROPERTY, "run_tests");
                let instance_ref = maybe_instance.insert(instance);
                instance_ref.run_tests(&test_url, options, sender, stop_recv).await;
                diagnostics.set_property(EXECUTION_PROPERTY, "tests_done");
            }
            Err(e) => {
                sender.send(Err(e.into())).await.unwrap();
            }
        }
    };
    let (run_test_remote, run_test_handle) = run_test_fut.remote_handle();

    let controller_fut = Suite::run_controller(controller, stop_sender, run_test_handle, recv);
    let ((), controller_ret) = futures::future::join(run_test_remote, controller_fut).await;

    if let Err(e) = controller_ret {
        warn!(diagnostics:?; "Ended test {}: {:?}", test_url, e);
    }

    if let Some(instance) = maybe_instance.take() {
        diagnostics.set_property(EXECUTION_PROPERTY, "tear_down");
        info!(diagnostics:?; "Test suite has finished, destroying instance...");
        if let Err(err) = instance.destroy(diagnostics.child("destroy")).await {
            // Failure to destroy an instance could mean that some component events fail to send.
            error!(diagnostics:?, err:?; "Failed to destroy instance. Debug data may be lost.");
        }
    }
    diagnostics.set_property(EXECUTION_PROPERTY, "complete");
    info!(diagnostics:?; "Test destruction complete");
}

// Separate suite into a hermetic and a non-hermetic collection
// Note: F takes String and Arc<ResolverProxy> to circumvent the
// borrow checker.
async fn split_suites_by_hermeticity<F, Fut>(
    suites: Vec<Suite>,
    get_facets_fn: F,
) -> (Vec<Suite>, Vec<Suite>)
where
    F: Fn(String, Arc<ResolverProxy>) -> Fut,
    Fut: futures::future::Future<Output = Result<SuiteFacets, LaunchTestError>>,
{
    let mut serial_suites: Vec<Suite> = Vec::new();
    let mut parallel_suites: Vec<Suite> = Vec::new();

    for mut suite in suites {
        if suite.realm.is_some() {
            serial_suites.push(suite);
            continue;
        }
        let test_url = suite.test_url.clone();
        let resolver = suite.resolver.clone();
        let facet_result = get_facets_fn(test_url, resolver).await;
        let can_run_in_parallel = match &facet_result {
            Ok(facets) => facets.collection == HERMETIC_TESTS_COLLECTION,
            Err(_) => false,
        };
        suite.facets = facet::ResolveStatus::Resolved(facet_result);
        if can_run_in_parallel {
            parallel_suites.push(suite);
        } else {
            serial_suites.push(suite);
        }
    }
    (serial_suites, parallel_suites)
}

// Determine whether debug_agent should be used for test runs.
fn use_debug_agent_for_runs(options: &ftest_manager::RunOptions) -> bool {
    match options.no_exception_channel {
        Some(true) => {
            // Do not use debug_agent when the option is set to true.
            false
        }
        Some(false) | None => {
            // Use debug_agent when the option is set to false or not specified.
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_component_resolution as fresolution;
    use fuchsia_async as fasync;

    fn new_run_inspect_node() -> DiagnosticNode {
        DiagnosticNode::new("root", Arc::new(fuchsia_inspect::types::Node::default()))
    }

    #[fuchsia::test]
    async fn run_controller_stop_test() {
        let (sender, recv) = mpsc::channel(1024);
        let (stop_sender, stop_recv) = oneshot::channel::<()>();
        let (task, remote_handle) = async move {
            stop_recv.await.unwrap();
            // drop event sender so that fake test can end.
            drop(sender);
        }
        .remote_handle();
        let _task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::RunControllerMarker>();
        let run_controller = fasync::Task::spawn(async move {
            TestRunBuilder::run_controller(
                controller,
                remote_handle,
                stop_sender,
                recv,
                new_run_inspect_node(),
            )
            .await
        });
        // sending a get event first should not prevent stop from cancelling the suite.
        let get_events_task = fasync::Task::spawn(proxy.get_events());
        proxy.stop().unwrap();
        assert_eq!(get_events_task.await.unwrap(), vec![]);

        drop(proxy);
        run_controller.await;
    }

    #[fuchsia::test]
    async fn run_controller_abort_when_channel_closed() {
        let (_sender, recv) = mpsc::channel(1024);
        let (stop_sender, _stop_recv) = oneshot::channel::<()>();
        // Create a future that normally never resolves.
        let (task, remote_handle) = futures::future::pending().remote_handle();
        let pending_task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::RunControllerMarker>();
        let run_controller = fasync::Task::spawn(async move {
            TestRunBuilder::run_controller(
                controller,
                remote_handle,
                stop_sender,
                recv,
                new_run_inspect_node(),
            )
            .await
        });
        // sending a get event first should not prevent killing the controller.
        let get_events_task = fasync::Task::spawn(proxy.get_events());
        drop(proxy);
        drop(get_events_task);
        // After controller is dropped, both the controller future and the task it was
        // controlling should terminate.
        pending_task.await;
        run_controller.await;
    }

    #[fuchsia::test]
    async fn suite_controller_stop_test() {
        let (sender, recv) = mpsc::channel(1024);
        let (stop_sender, stop_recv) = oneshot::channel::<()>();
        let (task, remote_handle) = async move {
            stop_recv.await.unwrap();
            // drop event sender so that fake test can end.
            drop(sender);
        }
        .remote_handle();
        let _task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::SuiteControllerMarker>();
        let run_controller = fasync::Task::spawn(async move {
            Suite::run_controller(controller, stop_sender, remote_handle, recv).await
        });
        // sending a get event first should not prevent stop from cancelling the suite.
        let get_events_task = fasync::Task::spawn(proxy.get_events());
        proxy.stop().unwrap();

        assert_eq!(get_events_task.await.unwrap(), Ok(vec![]));
        // run controller should end after channel is closed.
        drop(proxy);
        run_controller.await.unwrap();
    }

    #[fuchsia::test]
    async fn suite_controller_abort_remote_when_controller_closed() {
        let (_sender, recv) = mpsc::channel(1024);
        let (stop_sender, _stop_recv) = oneshot::channel::<()>();
        // Create a future that normally never resolves.
        let (task, remote_handle) = futures::future::pending().remote_handle();
        let pending_task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::SuiteControllerMarker>();
        let run_controller = fasync::Task::spawn(async move {
            Suite::run_controller(controller, stop_sender, remote_handle, recv).await
        });
        // sending a get event first should not prevent killing the controller.
        let get_events_task = fasync::Task::spawn(proxy.get_events());
        drop(proxy);
        drop(get_events_task);
        // After controller is dropped, both the controller future and the task it was
        // controlling should terminate.
        pending_task.await;
        run_controller.await.unwrap();
    }

    #[fuchsia::test]
    async fn suite_controller_get_events() {
        let (mut sender, recv) = mpsc::channel(1024);
        let (stop_sender, stop_recv) = oneshot::channel::<()>();
        let (task, remote_handle) = async {}.remote_handle();
        let _task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::SuiteControllerMarker>();
        let run_controller = fasync::Task::spawn(async move {
            Suite::run_controller(controller, stop_sender, remote_handle, recv).await
        });
        sender.send(Ok(SuiteEvents::case_found(1, "case1".to_string()).into())).await.unwrap();
        sender.send(Ok(SuiteEvents::case_found(2, "case2".to_string()).into())).await.unwrap();

        let events = proxy.get_events().await.unwrap().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].payload,
            SuiteEvents::case_found(1, "case1".to_string()).into_suite_run_event().payload,
        );
        assert_eq!(
            events[1].payload,
            SuiteEvents::case_found(2, "case2".to_string()).into_suite_run_event().payload,
        );
        sender.send(Ok(SuiteEvents::case_started(2).into())).await.unwrap();
        proxy.stop().unwrap();

        // test that controller collects event after stop is called.
        sender.send(Ok(SuiteEvents::case_started(1).into())).await.unwrap();
        sender.send(Ok(SuiteEvents::case_found(3, "case3".to_string()).into())).await.unwrap();

        stop_recv.await.unwrap();
        // drop event sender so that fake test can end.
        drop(sender);
        let events = proxy.get_events().await.unwrap().unwrap();
        assert_eq!(events.len(), 3);

        assert_eq!(events[0].payload, SuiteEvents::case_started(2).into_suite_run_event().payload,);
        assert_eq!(events[1].payload, SuiteEvents::case_started(1).into_suite_run_event().payload,);
        assert_eq!(
            events[2].payload,
            SuiteEvents::case_found(3, "case3".to_string()).into_suite_run_event().payload,
        );

        let events = proxy.get_events().await.unwrap().unwrap();
        assert_eq!(events, vec![]);
        // run controller should end after channel is closed.
        drop(proxy);
        run_controller.await.unwrap();
    }

    async fn create_fake_suite(test_url: String) -> Suite {
        let (_controller_proxy, controller_stream) =
            create_proxy_and_stream::<ftest_manager::SuiteControllerMarker>();
        let (resolver_proxy, _resolver_stream) =
            create_proxy_and_stream::<fresolution::ResolverMarker>();
        let resolver_proxy = Arc::new(resolver_proxy);
        let (pkg_resolver_proxy, _pkg_resolver_stream) =
            create_proxy_and_stream::<fidl_fuchsia_pkg::PackageResolverMarker>();
        let pkg_resolver_proxy = Arc::new(pkg_resolver_proxy);
        let routing_info = Arc::new(AboveRootCapabilitiesForTest::new_empty_for_tests());
        Suite {
            realm: None,
            test_url,
            options: ftest_manager::RunOptions {
                parallel: None,
                arguments: None,
                run_disabled_tests: Some(false),
                timeout: None,
                case_filters_to_run: None,
                log_iterator: None,
                ..Default::default()
            },
            controller: controller_stream,
            resolver: resolver_proxy,
            pkg_resolver: pkg_resolver_proxy,
            above_root_capabilities_for_test: routing_info,
            facets: facet::ResolveStatus::Unresolved,
        }
    }

    #[fuchsia::test]
    async fn split_suites_by_hermeticity_test() {
        let hermetic_suite = create_fake_suite("hermetic_suite".to_string()).await;
        let non_hermetic_suite = create_fake_suite("non_hermetic_suite".to_string()).await;
        let suites = vec![hermetic_suite, non_hermetic_suite];

        // call split_suites_by_hermeticity
        let get_facets_fn = |test_url, _resolver| async move {
            if test_url == "hermetic_suite".to_string() {
                Ok(SuiteFacets {
                    collection: HERMETIC_TESTS_COLLECTION,
                    deprecated_allowed_packages: None,
                })
            } else {
                Ok(SuiteFacets {
                    collection: crate::constants::SYSTEM_TESTS_COLLECTION,
                    deprecated_allowed_packages: None,
                })
            }
        };
        let (serial_suites, parallel_suites) =
            split_suites_by_hermeticity(suites, get_facets_fn).await;

        assert_eq!(parallel_suites[0].test_url, "hermetic_suite".to_string());
        assert_eq!(serial_suites[0].test_url, "non_hermetic_suite".to_string());
    }

    #[test]
    fn suite_controller_hanging_get_events() {
        let mut executor = fasync::TestExecutor::new();
        let (mut sender, recv) = mpsc::channel(1024);
        let (stop_sender, _stop_recv) = oneshot::channel::<()>();
        let (task, remote_handle) = async {}.remote_handle();
        let _task = fasync::Task::spawn(task);
        let (proxy, controller) = create_proxy_and_stream::<ftest_manager::SuiteControllerMarker>();
        let _run_controller = fasync::Task::spawn(async move {
            Suite::run_controller(controller, stop_sender, remote_handle, recv).await
        });

        // send get event call which would hang as there are no events.
        let mut get_events =
            fasync::Task::spawn(async move { proxy.get_events().await.unwrap().unwrap() });
        assert_eq!(executor.run_until_stalled(&mut get_events), std::task::Poll::Pending);
        executor.run_singlethreaded(async {
            sender.send(Ok(SuiteEvents::case_found(1, "case1".to_string()).into())).await.unwrap();
            sender.send(Ok(SuiteEvents::case_found(2, "case2".to_string()).into())).await.unwrap();
        });
        let events = executor.run_singlethreaded(get_events);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].payload,
            SuiteEvents::case_found(1, "case1".to_string()).into_suite_run_event().payload,
        );
        assert_eq!(
            events[1].payload,
            SuiteEvents::case_found(2, "case2".to_string()).into_suite_run_event().payload,
        );
    }
}
