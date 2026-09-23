// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::above_root_capabilities::AboveRootCapabilitiesForTest;
use crate::debug_data_processor::{DebugDataDirectory, DebugDataProcessor};
use crate::error::TestManagerError;
use crate::offers::map_offers;
use crate::running_suite::{RunningSuite, enumerate_test_cases};
use crate::self_diagnostics::RootDiagnosticNode;
use crate::test_suite::{Suite, SuiteRealm, TestRunBuilder};
use crate::{constants, debug_data_server, facet};
use fidl::Error;
use fidl_fuchsia_component_resolution::ResolverProxy;
use fidl_fuchsia_pkg::PackageResolverProxy;
use fidl_fuchsia_test_manager as ftest_manager;
use fidl_fuchsia_test_manager::{QueryEnumerateInRealmResponder, QueryEnumerateResponder};
use ftest_manager::LaunchError;
use fuchsia_async::{self as fasync};
use futures::prelude::*;
use log::{error, warn};
use std::sync::Arc;

/// Start `RunBuilder` server and serve it over `stream`.
pub async fn run_test_manager_run_builder_server(
    mut stream: ftest_manager::RunBuilderRequestStream,
    resolver: Arc<ResolverProxy>,
    pkg_resolver: Arc<PackageResolverProxy>,
    above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    root_diagnostics: &RootDiagnosticNode,
) -> Result<(), TestManagerError> {
    let mut builder = TestRunBuilder { suites: vec![] };
    let mut scheduling_options: Option<ftest_manager::SchedulingOptions> = None;
    while let Some(event) = stream.try_next().await.map_err(TestManagerError::Stream)? {
        match event {
            ftest_manager::RunBuilderRequest::AddSuite {
                test_url,
                options,
                controller,
                control_handle: _,
            } => {
                let controller = controller.into_stream();

                builder.suites.push(Suite {
                    realm: None,
                    test_url,
                    options,
                    controller,
                    resolver: resolver.clone(),
                    pkg_resolver: pkg_resolver.clone(),
                    above_root_capabilities_for_test: above_root_capabilities_for_test.clone(),
                    facets: facet::ResolveStatus::Unresolved,
                });
            }
            ftest_manager::RunBuilderRequest::AddSuiteInRealm {
                realm,
                offers,
                test_collection,
                test_url,
                options,
                controller,
                control_handle,
            } => {
                let realm_proxy = realm.into_proxy();
                let controller = controller.into_stream();
                let offers = match map_offers(offers) {
                    Ok(offers) => offers,
                    Err(e) => {
                        warn!("Cannot add suite {}, invalid offers. error: {}", test_url, e);
                        control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                        break;
                    }
                };

                builder.suites.push(Suite {
                    realm: SuiteRealm { realm_proxy, offers, test_collection }.into(),
                    test_url,
                    options,
                    controller,
                    resolver: resolver.clone(),
                    pkg_resolver: pkg_resolver.clone(),
                    above_root_capabilities_for_test: above_root_capabilities_for_test.clone(),
                    facets: facet::ResolveStatus::Unresolved,
                });
            }
            ftest_manager::RunBuilderRequest::WithSchedulingOptions { options, .. } => {
                scheduling_options = Some(options);
            }
            ftest_manager::RunBuilderRequest::Build { controller, control_handle: _ } => {
                let controller = controller.into_stream();

                let persist_diagnostics =
                    match scheduling_options.as_ref().map(|options| options.max_parallel_suites) {
                        Some(Some(_)) => true,
                        Some(None) | None => false,
                    };
                let diagnostics = match persist_diagnostics {
                    true => root_diagnostics.persistent_child(),
                    false => root_diagnostics.child(),
                };

                builder.run(controller, diagnostics, scheduling_options).await;
                // clients should reconnect to run new tests.
                break;
            }
            ftest_manager::RunBuilderRequest::_UnknownMethod {
                ordinal, control_handle, ..
            } => {
                warn!("Unknown run builder request received: {}, closing connection", ordinal);
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                break;
            }
        }
    }
    Ok(())
}

enum QueryResponder {
    Enumerate(QueryEnumerateResponder),
    EnumerateInRealm(QueryEnumerateInRealmResponder),
}

impl QueryResponder {
    fn send(self, result: Result<(), LaunchError>) -> Result<(), fidl::Error> {
        match self {
            QueryResponder::Enumerate(responder) => responder.send(result),
            QueryResponder::EnumerateInRealm(responder) => responder.send(result),
        }
    }
}

/// Start `Query` server and serve it over `stream`.
pub async fn run_test_manager_query_server(
    mut stream: ftest_manager::QueryRequestStream,
    resolver: Arc<ResolverProxy>,
    pkg_resolver: Arc<PackageResolverProxy>,
    above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    root_diagnostics: &RootDiagnosticNode,
) -> Result<(), TestManagerError> {
    while let Some(event) = stream.try_next().await.map_err(TestManagerError::Stream)? {
        let (test_url, iterator, realm, responder) = match event {
            ftest_manager::QueryRequest::Enumerate { test_url, iterator, responder } => {
                (test_url, iterator, None, QueryResponder::Enumerate(responder))
            }
            ftest_manager::QueryRequest::EnumerateInRealm {
                test_url,
                realm,
                offers,
                test_collection,
                iterator,
                responder,
            } => {
                let realm_proxy = realm.into_proxy();
                let offers = match map_offers(offers) {
                    Ok(offers) => offers,
                    Err(e) => {
                        warn!("Cannot add suite {}, invalid offers. error: {}", test_url, e);
                        responder.send(Err(LaunchError::InvalidArgs)).ok();
                        break;
                    }
                };
                (
                    test_url,
                    iterator,
                    SuiteRealm { realm_proxy, offers, test_collection }.into(),
                    QueryResponder::EnumerateInRealm(responder),
                )
            }

            ftest_manager::QueryRequest::_UnknownMethod { ordinal, control_handle, .. } => {
                warn!("Unknown query request received: {}, closing connection", ordinal);
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                break;
            }
        };
        let mut iterator = iterator.into_stream();
        let (_processor, sender) = DebugDataProcessor::new(DebugDataDirectory::Isolated {
            parent: constants::ISOLATED_TMP,
        });
        let diagnostics = root_diagnostics.child();
        let launch_fut =
            facet::get_suite_facets(test_url.clone(), resolver.clone()).and_then(|facets| {
                RunningSuite::launch(
                    &test_url,
                    facets,
                    resolver.clone(),
                    pkg_resolver.clone(),
                    above_root_capabilities_for_test.clone(),
                    sender,
                    &diagnostics,
                    &realm,
                    false, // use_debug_agent
                )
            });
        match launch_fut.await {
            Ok(suite_instance) => {
                let suite = match suite_instance.connect_to_suite() {
                    Ok(proxy) => proxy,
                    Err(e) => {
                        responder.send(Err(e.into())).ok();
                        continue;
                    }
                };
                let enumeration_result = enumerate_test_cases(&suite, None).await;
                let t = fasync::Task::spawn(suite_instance.destroy(root_diagnostics.child()));
                match enumeration_result {
                    Ok(invocations) => {
                        const NAMES_CHUNK: usize = 50;
                        let mut names = Vec::with_capacity(invocations.len());
                        if let Ok(_) = invocations.into_iter().try_for_each(|i| match i.name {
                            Some(name) => {
                                names.push(name);
                                Ok(())
                            }
                            None => {
                                warn!("no name for a invocation in {}", test_url);
                                Err(())
                            }
                        }) {
                            responder.send(Ok(())).ok();
                            let mut names = names.chunks(NAMES_CHUNK);
                            while let Ok(Some(request)) = iterator.try_next().await {
                                match request {
                                    ftest_manager::CaseIteratorRequest::GetNext { responder } => {
                                        match names.next() {
                                            Some(names) => {
                                                responder
                                                    .send(
                                                        &names
                                                            .iter()
                                                            .map(|s| ftest_manager::Case {
                                                                name: Some(s.into()),
                                                                ..Default::default()
                                                            })
                                                            .collect::<Vec<_>>(),
                                                    )
                                                    .ok();
                                            }
                                            None => {
                                                responder.send(&[]).ok();
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            responder.send(Err(LaunchError::CaseEnumeration)).ok();
                        }
                    }
                    Err(err) => {
                        warn!(err:?; "cannot enumerate tests for {}", test_url);
                        responder.send(Err(LaunchError::CaseEnumeration)).ok();
                    }
                }
                if let Err(err) = t.await {
                    warn!(err:?; "Error destroying test realm for {}", test_url);
                }
            }
            Err(e) => {
                responder.send(Err(e.into())).ok();
            }
        }
    }
    Ok(())
}

pub async fn serve_early_boot_profiles(
    mut stream: ftest_manager::EarlyBootProfileRequestStream,
) -> Result<(), TestManagerError> {
    while let Some(req) = stream.try_next().await.map_err(TestManagerError::Stream)? {
        match req {
            ftest_manager::EarlyBootProfileRequest::RegisterWatcher {
                iterator,
                control_handle,
            } => {
                let iterator = iterator.into_stream();
                if let Err(e) = debug_data_server::send_kernel_debug_data(iterator).await {
                    warn!("Err serving kernel profiles: {}", e);
                    control_handle.shutdown_with_epitaph(zx::Status::INTERNAL);
                    break;
                }
            }
            ftest_manager::EarlyBootProfileRequest::_UnknownMethod {
                ordinal,
                control_handle,
                ..
            } => {
                warn!("Unknown EarlyBootProfile request received: {}, closing connection", ordinal);
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                break;
            }
        }
    }
    Ok(())
}

/// Start `TestCaseEnumerator` server and serve it over `stream`.
pub async fn run_test_manager_test_case_enumerator_server(
    mut stream: ftest_manager::TestCaseEnumeratorRequestStream,
    resolver: Arc<ResolverProxy>,
    pkg_resolver: Arc<PackageResolverProxy>,
    above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    root_diagnostics: &RootDiagnosticNode,
) -> Result<(), TestManagerError> {
    while let Some(req) = stream.try_next().await.map_err(TestManagerError::Stream)? {
        match req {
            ftest_manager::TestCaseEnumeratorRequest::Enumerate {
                test_suite_url,
                options,
                iterator,
                responder,
            } => {
                let realm = if let Some(realm_options) = options.realm_options {
                    let realm_proxy = match realm_options
                        .realm
                        .map(|r| Ok(r.into_proxy()))
                        .unwrap_or(Err(Error::NotNullable))
                    {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(
                                "Cannot enumerate test cases {}, invalid realm. Closing connection. error: {}",
                                test_suite_url, e
                            );
                            responder.send(Err(LaunchError::InvalidArgs)).ok();
                            break;
                        }
                    };
                    let offers = match realm_options
                        .offers
                        .map(map_offers)
                        .unwrap_or(Err(Error::NotNullable.into()))
                    {
                        Ok(offers) => offers,
                        Err(e) => {
                            warn!(
                                "Cannot enumerate test cases {}, invalid offers. error: {}",
                                test_suite_url, e
                            );
                            responder.send(Err(LaunchError::InvalidArgs)).ok();
                            break;
                        }
                    };
                    let test_collection = match realm_options.test_collection {
                        Some(test_collection) => test_collection,
                        None => {
                            warn!(
                                "Cannot enumerate test cases {}, missing test collection.",
                                test_suite_url
                            );
                            responder.send(Err(LaunchError::InvalidArgs)).ok();
                            break;
                        }
                    };
                    Some(SuiteRealm { realm_proxy, offers, test_collection })
                } else {
                    None
                };

                let iterator = iterator.into_stream();
                let (_processor, sender) = DebugDataProcessor::new(DebugDataDirectory::Isolated {
                    parent: constants::ISOLATED_TMP,
                });
                let diagnostics = root_diagnostics.child();
                let launch_fut = facet::get_suite_facets(test_suite_url.clone(), resolver.clone())
                    .and_then(|facets| {
                        RunningSuite::launch(
                            &test_suite_url,
                            facets,
                            resolver.clone(),
                            pkg_resolver.clone(),
                            above_root_capabilities_for_test.clone(),
                            sender,
                            &diagnostics,
                            &realm,
                            false, // use_debug_agent
                        )
                    });
                match launch_fut.await {
                    Ok(suite_instance) => {
                        let suite = match suite_instance.connect_to_suite() {
                            Ok(proxy) => proxy,
                            Err(e) => {
                                responder.send(Err(e.into())).ok();
                                continue;
                            }
                        };
                        let enumeration_result = enumerate_test_cases(&suite, None).await;
                        let t = fasync::Task::spawn(suite_instance.destroy(diagnostics));
                        match enumeration_result {
                            Ok(invocations) => {
                                if let Ok(names) = invocations
                                    .into_iter()
                                    .map(|i| i.name.ok_or(()))
                                    .collect::<Result<Vec<String>, ()>>()
                                {
                                    responder.send(Ok(())).ok();
                                    drain_test_case_names(iterator, names).await;
                                } else {
                                    responder.send(Err(LaunchError::CaseEnumeration)).ok();
                                }
                            }
                            Err(err) => {
                                warn!(err:?; "cannot enumerate tests for {}", test_suite_url);
                                responder.send(Err(LaunchError::CaseEnumeration)).ok();
                            }
                        }
                        if let Err(err) = t.await {
                            warn!(err:?; "Error destroying test realm for {}", test_suite_url);
                        }
                    }
                    Err(e) => {
                        responder.send(Err(e.into())).ok();
                    }
                }
            }

            ftest_manager::TestCaseEnumeratorRequest::_UnknownMethod {
                ordinal,
                control_handle,
                ..
            } => {
                warn!("Unknown query request received: {}, closing connection", ordinal);
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                break;
            }
        };
    }
    Ok(())
}

async fn drain_test_case_names(
    mut iterator: ftest_manager::TestCaseIteratorRequestStream,
    names: Vec<String>,
) {
    const NAMES_CHUNK: usize = 50;
    let mut names = names.chunks(NAMES_CHUNK);
    while let Ok(Some(request)) = iterator.try_next().await {
        match request {
            ftest_manager::TestCaseIteratorRequest::GetNext { responder } => match names.next() {
                Some(names) => {
                    responder
                        .send(
                            &names
                                .iter()
                                .map(|s| ftest_manager::TestCase {
                                    name: Some(s.into()),
                                    ..Default::default()
                                })
                                .collect::<Vec<_>>(),
                        )
                        .ok();
                }
                None => {
                    responder.send(&[]).ok();
                }
            },
        }
    }
}

/// Start `SuiteRunner` server and serve it over `stream`.
pub async fn run_test_manager_suite_runner_server(
    mut stream: ftest_manager::SuiteRunnerRequestStream,
    resolver: Arc<ResolverProxy>,
    pkg_resolver: Arc<PackageResolverProxy>,
    above_root_capabilities_for_test: Arc<AboveRootCapabilitiesForTest>,
    root_diagnostics: &RootDiagnosticNode,
) -> Result<(), TestManagerError> {
    while let Some(req) = stream.try_next().await.map_err(TestManagerError::Stream)? {
        match req {
            ftest_manager::SuiteRunnerRequest::Run {
                test_suite_url,
                options,
                controller,
                control_handle,
            } => {
                let realm = if let Some(realm_options) = options.realm_options {
                    let realm_proxy = match realm_options
                        .realm
                        .map(|r| Ok(r.into_proxy()))
                        .unwrap_or(Err(Error::NotNullable))
                    {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(
                                "Cannot run suite {}, invalid realm. Closing connection. error: {}",
                                test_suite_url, e
                            );
                            control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                            break;
                        }
                    };
                    let offers = match realm_options
                        .offers
                        .map(map_offers)
                        .unwrap_or(Err(Error::NotNullable.into()))
                    {
                        Ok(offers) => offers,
                        Err(e) => {
                            warn!(
                                "Cannot run suite {}, invalid offers. error: {}",
                                test_suite_url, e
                            );
                            control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                            break;
                        }
                    };
                    let test_collection = match realm_options.test_collection {
                        Some(test_collection) => test_collection,
                        None => {
                            warn!("Cannot run suite {}, missing test collection.", test_suite_url);
                            control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                            break;
                        }
                    };
                    Some(SuiteRealm { realm_proxy, offers, test_collection })
                } else {
                    None
                };

                // Validate log iterator type.
                match options.logs_iterator_type {
                    Some(ftest_manager::LogsIteratorType::Batch)
                    | Some(ftest_manager::LogsIteratorType::Socket)
                    | None => {
                        // Accepted
                    }
                    Some(t) => {
                        error!("Unrecognized log iterator type {:?}.", t);
                        control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
                        break;
                    }
                }

                let controller = controller.into_stream();

                let suite = Suite {
                    realm: realm.into(),
                    test_url: test_suite_url,
                    options: ftest_manager::RunOptions {
                        run_disabled_tests: options.run_disabled_tests,
                        parallel: options.max_concurrent_test_case_runs,
                        arguments: options.arguments,
                        timeout: options.timeout,
                        case_filters_to_run: options.test_case_filters,
                        log_iterator: options.logs_iterator_type.map(convert),
                        log_interest: options.log_interest,
                        break_on_failure: options.break_on_failure,
                        no_exception_channel: options.no_exception_channel,
                        ..Default::default()
                    },
                    controller,
                    resolver: resolver.clone(),
                    pkg_resolver: pkg_resolver.clone(),
                    above_root_capabilities_for_test: above_root_capabilities_for_test.clone(),
                    facets: facet::ResolveStatus::Unresolved,
                };

                let diagnostics = root_diagnostics.child();

                suite.run(diagnostics, options.accumulate_debug_data.unwrap_or(false)).await;
            }

            ftest_manager::SuiteRunnerRequest::_UnknownMethod {
                ordinal, control_handle, ..
            } => {
                warn!("Unknown run builder request received: {}, closing connection", ordinal);
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                break;
            }
        }
    }
    Ok(())
}

fn convert(item: ftest_manager::LogsIteratorType) -> ftest_manager::LogsIteratorOption {
    match item {
        ftest_manager::LogsIteratorType::Batch => ftest_manager::LogsIteratorOption::BatchIterator,
        ftest_manager::LogsIteratorType::Socket => {
            ftest_manager::LogsIteratorOption::SocketBatchIterator
        }
        _ => todo!(),
    }
}
