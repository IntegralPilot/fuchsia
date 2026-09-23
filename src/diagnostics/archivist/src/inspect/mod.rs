// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::accessor::PerformanceConfig;
use crate::diagnostics::{BatchIteratorConnectionStats, TRACE_CATEGORY};
use crate::inspect::container::{ReadSnapshot, SnapshotData, UnpopulatedInspectDataContainer};
use crate::pipeline::{ComponentAllowlist, PrivacyExplicitOption};
use diagnostics_data::{self as schema, Data, Inspect, InspectDataBuilder, InspectHandleName};
use diagnostics_hierarchy::{DiagnosticsHierarchy, HierarchyMatcher};
use fidl_fuchsia_diagnostics::Selector;
use fidl_fuchsia_inspect::DEFAULT_TREE_NAME;
use fuchsia_inspect::reader::PartialNodeHierarchy;
use fuchsia_trace as ftrace;
use futures::prelude::*;
use log::error;
use selectors::SelectorExt;
use std::sync::Arc;

pub mod collector;
pub mod container;
pub mod repository;
pub mod servers;

use container::PopulatedInspectDataContainer;

/// Packet containing a node hierarchy and all the metadata needed to
/// populate a diagnostics schema for that node hierarchy.
#[derive(Debug)]
pub struct NodeHierarchyData {
    // Name of the file that created this snapshot.
    name: Option<InspectHandleName>,
    // Timestamp at which this snapshot resolved or failed.
    timestamp: zx::BootInstant,
    // Errors encountered when processing this snapshot.
    errors: Vec<schema::InspectError>,
    // Optional DiagnosticsHierarchy of the inspect hierarchy, in case reading fails
    // and we have errors to share with client.
    hierarchy: Option<DiagnosticsHierarchy>,
    // Whether or not this data comes from an escrowed VMO.
    escrowed: bool,
}

impl From<SnapshotData> for NodeHierarchyData {
    fn from(data: SnapshotData) -> NodeHierarchyData {
        match data.snapshot {
            Some(snapshot) => match convert_snapshot_to_node_hierarchy(snapshot) {
                Ok(node_hierarchy) => {
                    let errors = data
                        .errors
                        .into_iter()
                        .chain(node_hierarchy.error_iter().filter_map(|(key, mv)| {
                            mv.map(|mv| schema::InspectError {
                                message: format!(
                                    "Lazy value {} could not be loaded: {:?}",
                                    key.join("/"),
                                    mv.reason
                                ),
                            })
                        }))
                        .collect();
                    NodeHierarchyData {
                        name: data.name,
                        timestamp: data.timestamp,
                        errors,
                        hierarchy: Some(node_hierarchy),
                        escrowed: data.escrowed,
                    }
                }
                Err(e) => NodeHierarchyData {
                    name: data.name,
                    timestamp: data.timestamp,
                    errors: vec![schema::InspectError { message: format!("{e:?}") }],
                    hierarchy: None,
                    escrowed: data.escrowed,
                },
            },
            None => NodeHierarchyData {
                name: data.name,
                timestamp: data.timestamp,
                errors: data.errors,
                hierarchy: None,
                escrowed: data.escrowed,
            },
        }
    }
}

/// ReaderServer holds the state and data needed to serve Inspect data
/// reading requests for a single client.
pub struct ReaderServer {
    /// Selectors provided by the client which define what inspect data is returned by read
    /// requests. A none type implies that all available data should be returned.
    selectors: Option<Vec<Selector>>,
}

fn convert_snapshot_to_node_hierarchy(
    snapshot: ReadSnapshot,
) -> Result<DiagnosticsHierarchy, fuchsia_inspect::reader::ReaderError> {
    match snapshot {
        ReadSnapshot::Single(snapshot) => Ok(PartialNodeHierarchy::try_from(snapshot)?.into()),
        ReadSnapshot::Tree(snapshot_tree) => snapshot_tree.try_into(),
        ReadSnapshot::Finished(hierarchy) => Ok(hierarchy),
    }
}

impl ReaderServer {
    /// Create a stream of filtered inspect data, ready to serve.
    pub fn stream(
        unpopulated_diagnostics_sources: Vec<UnpopulatedInspectDataContainer>,
        performance_configuration: PerformanceConfig,
        selectors: Option<Vec<Selector>>,
        stats: Arc<BatchIteratorConnectionStats>,
        parent_trace_id: ftrace::Id,
    ) -> impl Stream<Item = Data<Inspect>> + Send + 'static {
        let server = Arc::new(Self { selectors });

        let batch_timeout = performance_configuration.batch_timeout_sec;
        let maximum_concurrent_snapshots_per_reader =
            performance_configuration.maximum_concurrent_snapshots_per_reader;

        futures::stream::iter(unpopulated_diagnostics_sources)
            .map(move |unpopulated| {
                let global_stats = Arc::clone(stats.global_stats());
                unpopulated.populate(batch_timeout, global_stats, parent_trace_id)
            })
            .flatten()
            .map(future::ready)
            // buffer a small number in memory in case later components time out
            .buffer_unordered(maximum_concurrent_snapshots_per_reader as usize)
            // filter each component's inspect
            .filter_map(move |populated| {
                let server_clone = Arc::clone(&server);
                async move { server_clone.filter_snapshot(populated, parent_trace_id) }
            })
    }

    fn filter_single_components_snapshot(
        snapshot_data: SnapshotData,
        static_allowlist: ComponentAllowlist,
        client_matcher: Option<HierarchyMatcher>,
        moniker: &str,
        parent_trace_id: ftrace::Id,
    ) -> Option<NodeHierarchyData> {
        let filename = snapshot_data.name.clone();
        let node_hierarchy_data = {
            let unfiltered_node_hierarchy_data: NodeHierarchyData = {
                let trace_id = ftrace::Id::new();
                let _trace_guard = ftrace::async_enter!(
                    trace_id,
                    TRACE_CATEGORY,
                    c"SnapshotData -> NodeHierarchyData",
                    // An async duration cannot have multiple concurrent child async durations
                    // so we include the nonce as metadata to manually determine relationship.
                    "parent_trace_id" => u64::from(parent_trace_id),
                    "trace_id" => u64::from(trace_id),
                    "moniker" => moniker,
                    "filename" => filename
                            .as_ref()
                            .and_then(InspectHandleName::as_filename)
                            .unwrap_or(""),
                    "name" => filename
                            .as_ref()
                            .and_then(InspectHandleName::as_name)
                            .unwrap_or("")
                );
                NodeHierarchyData::from(snapshot_data)
            };

            let handle_name = unfiltered_node_hierarchy_data
                .name
                .as_ref()
                .map(|name| name.as_ref())
                .unwrap_or(DEFAULT_TREE_NAME);
            match static_allowlist.matcher(handle_name) {
                PrivacyExplicitOption::Found(matcher) => {
                    let Some(node_hierarchy) = unfiltered_node_hierarchy_data.hierarchy else {
                        return Some(unfiltered_node_hierarchy_data);
                    };
                    let trace_id = ftrace::Id::new();
                    let _trace_guard = ftrace::async_enter!(
                        trace_id,
                        TRACE_CATEGORY,
                        c"ReaderServer::filter_single_components_snapshot.filter_hierarchy",
                        // An async duration cannot have multiple concurrent child async durations
                        // so we include the nonce as metadata to manually determine relationship.
                        "parent_trace_id" => u64::from(parent_trace_id),
                        "trace_id" => u64::from(trace_id),
                        "moniker" => moniker,
                        "filename"  => unfiltered_node_hierarchy_data
                                .name
                                .as_ref()
                                .and_then(InspectHandleName::as_filename)
                                .unwrap_or(""),
                        "name" => unfiltered_node_hierarchy_data
                                .name
                                .as_ref()
                                .and_then(InspectHandleName::as_name)
                                .unwrap_or(""),
                        "selector_type" => "static"
                    );
                    let filtered_hierarchy =
                        diagnostics_hierarchy::filter_hierarchy(node_hierarchy, matcher)?;
                    NodeHierarchyData {
                        name: unfiltered_node_hierarchy_data.name,
                        timestamp: unfiltered_node_hierarchy_data.timestamp,
                        errors: unfiltered_node_hierarchy_data.errors,
                        hierarchy: Some(filtered_hierarchy),
                        escrowed: unfiltered_node_hierarchy_data.escrowed,
                    }
                }
                PrivacyExplicitOption::NotFound => return None,
                PrivacyExplicitOption::FilteringDisabled => unfiltered_node_hierarchy_data,
            }
        };

        let Some(dynamic_matcher) = client_matcher else {
            // If matcher is present, and there was an HierarchyMatcher,
            // then this means the client provided their own selectors, and a subset of
            // them matched this component. So we need to filter each of the snapshots from
            // this component with the dynamically provided components.
            return Some(node_hierarchy_data);
        };
        let Some(node_hierarchy) = node_hierarchy_data.hierarchy else {
            return Some(node_hierarchy_data);
        };

        let trace_id = ftrace::Id::new();
        let _trace_guard = ftrace::async_enter!(
            trace_id,
            TRACE_CATEGORY,
            c"ReaderServer::filter_single_components_snapshot.filter_hierarchy",
            // An async duration cannot have multiple concurrent child async durations
            // so we include the nonce as metadata to manually determine relationship.
            "parent_trace_id" => u64::from(parent_trace_id),
            "trace_id" => u64::from(trace_id),
            "moniker" => moniker,
            "filename" => {
                node_hierarchy_data
                    .name
                    .as_ref()
                    .and_then(InspectHandleName::as_filename)
                    .unwrap_or("")
            },
            "name" => {
                node_hierarchy_data
                    .name
                    .as_ref()
                    .and_then(InspectHandleName::as_name)
                    .unwrap_or("")
            },
            "selector_type" => "client"
        );
        diagnostics_hierarchy::filter_hierarchy(node_hierarchy, &dynamic_matcher).map(
            |filtered_hierarchy| NodeHierarchyData {
                name: node_hierarchy_data.name,
                timestamp: node_hierarchy_data.timestamp,
                errors: node_hierarchy_data.errors,
                hierarchy: Some(filtered_hierarchy),
                escrowed: node_hierarchy_data.escrowed,
            },
        )
    }

    /// Takes a PopulatedInspectDataContainer and converts all non-error
    /// results into in-memory node hierarchies. The hierarchies are filtered
    /// such that the only diagnostics properties they contain are those
    /// configured by the static and client-provided selectors.
    ///
    // TODO(https://fxbug.dev/42122598): Error entries should still be included, but with a custom hierarchy
    //             that makes it clear to clients that snapshotting failed.
    fn filter_snapshot(
        &self,
        pumped_inspect_data: PopulatedInspectDataContainer,
        parent_trace_id: ftrace::Id,
    ) -> Option<Data<Inspect>> {
        // Since a single PopulatedInspectDataContainer shares a moniker for all pieces of data it
        // contains, we can store the result of component selector filtering to avoid reapplying
        // the selectors.
        let mut client_selectors: Option<HierarchyMatcher> = None;

        // We iterate the vector of pumped inspect data packets, consuming each inspect vmo
        // and filtering it using the provided selector regular expressions. Each filtered
        // inspect hierarchy is then added to an accumulator as a HierarchyData to be converted
        // into a JSON string and returned.
        if let Some(configured_selectors) = &self.selectors {
            client_selectors = {
                let matching_selectors = pumped_inspect_data
                    .identity
                    .moniker
                    .match_against_selectors(configured_selectors.as_slice())
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|err| {
                        error!(
                            moniker:? = pumped_inspect_data.identity.moniker, err:?;
                            "Failed to evaluate client selectors",
                        );
                        Vec::new()
                    });

                if matching_selectors.is_empty() {
                    None
                } else {
                    match matching_selectors.try_into() {
                        Ok(hierarchy_matcher) => Some(hierarchy_matcher),
                        Err(e) => {
                            error!(e:?; "Failed to create hierarchy matcher");
                            None
                        }
                    }
                }
            };

            // If there were configured matchers and none of them matched
            // this component, then we should return early since there is no data to
            // extract.
            client_selectors.as_ref()?;
        }

        let identity = Arc::clone(&pumped_inspect_data.identity);

        let hierarchy_data = ReaderServer::filter_single_components_snapshot(
            pumped_inspect_data.snapshot,
            pumped_inspect_data.component_allowlist,
            client_selectors,
            identity.to_string().as_str(),
            parent_trace_id,
        )?;
        let mut builder = InspectDataBuilder::new(
            pumped_inspect_data.identity.moniker.clone(),
            identity.url.clone(),
            hierarchy_data.timestamp,
        );
        if let Some(hierarchy) = hierarchy_data.hierarchy {
            builder = builder.with_hierarchy(hierarchy);
        }
        if let Some(name) = hierarchy_data.name {
            builder = builder.with_name(name);
        }
        if !hierarchy_data.errors.is_empty() {
            builder = builder.with_errors(hierarchy_data.errors);
        }
        Some(builder.escrowed(hierarchy_data.escrowed).build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessor::BatchIterator;
    use crate::diagnostics::AccessorStats;
    use crate::events::router::EventConsumer;
    use crate::events::types::{Event, EventPayload, InspectSinkRequestedPayload};
    use crate::identity::ComponentIdentity;
    use crate::inspect::container::InspectHandle;
    use crate::inspect::repository::InspectRepository;
    use crate::inspect::servers::InspectSinkServer;
    use crate::pipeline::Pipeline;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl::endpoints::{ClientEnd, create_proxy_and_stream};
    use fidl_fuchsia_diagnostics::{BatchIteratorMarker, BatchIteratorProxy, Format, StreamMode};
    use fidl_fuchsia_inspect::{
        InspectSinkMarker, InspectSinkPublishRequest, TreeMarker, TreeProxy,
    };
    use fuchsia_async::{self as fasync, Task};
    use fuchsia_inspect::{Inspector, InspectorConfig};

    use futures::StreamExt;
    use inspect_runtime::{TreeServerSendPreference, service};
    use moniker::ExtendedMoniker;
    use selectors::VerboseError;
    use serde_json::json;
    use std::collections::HashMap;
    use test_case::test_case;
    use zx::Peered;

    const TEST_URL: &str = "fuchsia-pkg://test";
    const BATCH_RETRIEVAL_TIMEOUT_SECONDS: i64 = 300;

    #[fuchsia::test]
    async fn read_server_formatting_tree_inspect_sink() {
        let inspector = inspector_for_reader_test();
        let scope = fasync::Scope::new();
        let tree_client =
            service::spawn_tree_server(inspector, TreeServerSendPreference::default(), &scope);
        verify_reader(tree_client).await;
    }

    #[fuchsia::test]
    async fn reader_server_reports_errors() {
        // This inspector doesn't contain valid inspect data.
        let vmo = zx::Vmo::create(4096).unwrap();
        let inspector = Inspector::new(InspectorConfig::default().vmo(vmo));
        let scope = fasync::Scope::new();
        let tree_client =
            service::spawn_tree_server(inspector, TreeServerSendPreference::default(), &scope);
        let (done0, done1) = zx::Channel::create();
        // Run the actual test in a separate thread so that it does not block on FS operations.
        // Use signalling on a zx::Channel to indicate that the test is done.
        std::thread::spawn(move || {
            let done = done1;
            let mut executor = fasync::LocalExecutor::default();
            executor.run_singlethreaded(async {
                verify_reader_with_mode(tree_client, VerifyMode::ExpectComponentFailure).await;
                done.signal_peer(zx::Signals::NONE, zx::Signals::USER_0).expect("signalling peer");
            });
        });

        fasync::OnSignals::new(&done0, zx::Signals::USER_0).await.unwrap();
    }

    #[test_case(vec![63, 65], vec![64, 64] ; "merge_errorful_component_into_next_batch")]
    #[test_case(vec![64, 65, 64, 64], vec![64, 64, 64, 64, 1] ; "errorful_component_doesnt_halt_iteration")]
    #[test_case(vec![65], vec![64, 1] ; "component_with_more_than_max_batch_size_is_split_in_two")]
    #[test_case(vec![1usize; 64], vec![64] ; "sixty_four_vmos_packed_into_one_batch")]
    #[test_case(vec![64, 63, 1], vec![64, 64] ; "max_batch_intact_two_batches_merged")]
    #[test_case(vec![33, 33, 33], vec![64, 35] ; "three_directories_two_batches")]
    #[fuchsia::test]
    async fn stress_test_diagnostics_repository(
        component_handle_counts: Vec<usize>,
        expected_batch_results: Vec<usize>,
    ) {
        let component_name_handle_counts: Vec<(String, usize)> = component_handle_counts
            .into_iter()
            .enumerate()
            .map(|(index, handle_count)| (format!("diagnostics_{index}"), handle_count))
            .collect();

        let inspector = inspector_for_reader_test();

        let mut clients = HashMap::<String, Vec<TreeProxy>>::new();
        let scope = fasync::Scope::new();
        for (component_name, handle_count) in component_name_handle_counts.clone() {
            for _ in 0..handle_count {
                let inspector_dup = Inspector::new(
                    InspectorConfig::default()
                        .vmo(inspector.duplicate_vmo().expect("failed to duplicate vmo")),
                );
                let client = service::spawn_tree_server(
                    inspector_dup,
                    TreeServerSendPreference::default(),
                    &scope,
                );
                clients.entry(component_name.clone()).or_default().push(client.into_proxy());
            }
        }

        let pipeline = Arc::new(Pipeline::for_test(None));
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], fasync::Scope::new()));

        for (component, handles) in clients {
            let moniker = ExtendedMoniker::parse_str(&component).unwrap();
            let identity = Arc::new(ComponentIdentity::new(moniker, TEST_URL));
            for (i, handle) in handles.into_iter().enumerate() {
                inspect_repo.add_inspect_handle(
                    Arc::clone(&identity),
                    InspectHandle::tree(handle, Some(format!("tree_{i}"))),
                );
            }
        }

        let inspector = Inspector::default();
        let root = inspector.root();
        let test_archive_accessor_node = root.create_child("test_archive_accessor_node");

        let test_accessor_stats = Arc::new(AccessorStats::new(test_archive_accessor_node));
        let test_batch_iterator_stats1 = Arc::new(test_accessor_stats.new_inspect_batch_iterator());

        let _result_json = read_snapshot_verify_batch_count_and_batch_size(
            Arc::clone(&inspect_repo),
            Arc::clone(&pipeline),
            expected_batch_results,
            test_batch_iterator_stats1,
        )
        .await;
    }

    fn inspector_for_reader_test() -> Inspector {
        let inspector = Inspector::default();
        let root = inspector.root();
        let child_1 = root.create_child("child_1");
        child_1.record_int("some-int", 2);
        let child_1_1 = child_1.create_child("child_1_1");
        child_1_1.record_int("some-int", 3);
        child_1_1.record_int("not-wanted-int", 4);
        root.record(child_1_1);
        root.record(child_1);
        let child_2 = root.create_child("child_2");
        child_2.record_int("some-int", 2);
        root.record(child_2);
        inspector
    }

    enum VerifyMode {
        ExpectSuccess,
        ExpectComponentFailure,
    }

    /// Verify that data can be read via InspectRepository, and that `AccessorStats` are updated
    /// accordingly.
    async fn verify_reader(tree_client: ClientEnd<TreeMarker>) {
        verify_reader_with_mode(tree_client, VerifyMode::ExpectSuccess).await;
    }

    async fn verify_reader_with_mode(tree_client: ClientEnd<TreeMarker>, mode: VerifyMode) {
        let child_1_1_selector =
            selectors::parse_selector::<VerboseError>(r#"*:root/child_1/*:some-int"#).unwrap();
        let child_2_selector =
            selectors::parse_selector::<VerboseError>(r#"test_component:root/child_2:*"#).unwrap();

        let static_selectors_opt = Some(vec![child_1_1_selector, child_2_selector]);

        let pipeline = Arc::new(Pipeline::for_test(static_selectors_opt));
        let inspect_repo =
            Arc::new(InspectRepository::new(vec![Arc::downgrade(&pipeline)], fasync::Scope::new()));

        // The moniker here is made up since the selector is a glob
        // selector, so any path would match.
        let component_id = ExtendedMoniker::parse_str("./test_component").unwrap();
        let inspector = Inspector::default();
        let root = inspector.root();
        let test_archive_accessor_node = root.create_child("test_archive_accessor_node");

        assert_data_tree!(inspector, root: {test_archive_accessor_node: {}});

        let test_accessor_stats = Arc::new(AccessorStats::new(test_archive_accessor_node));

        let test_batch_iterator_stats1 = Arc::new(test_accessor_stats.new_inspect_batch_iterator());

        assert_data_tree!(inspector, root: {
            test_archive_accessor_node: {
                connections_closed: 0u64,
                connections_opened: 0u64,
                inspect: {
                    batch_iterator_connections: {
                        "0": {
                            get_next: {
                                terminal_responses: 0u64,
                                responses: 0u64,
                                requests: 0u64,
                            }
                        }
                    },
                    batch_iterator: {
                        connections_closed: 0u64,
                        connections_opened: 0u64,
                        get_next: {
                            time_usec: AnyProperty,
                            requests: 0u64,
                            responses: 0u64,
                            result_count: 0u64,
                            result_errors: 0u64,
                        }
                    },
                    component_timeouts_count: 0u64,
                    reader_servers_constructed: 1u64,
                    reader_servers_destroyed: 0u64,
                    schema_truncation_count: 0u64,
                    max_snapshot_sizes_bytes: AnyProperty,
                    snapshot_schema_truncation_percentage: AnyProperty,
                },
                logs: {
                    batch_iterator_connections: {},
                    batch_iterator: {
                        connections_closed: 0u64,
                        connections_opened: 0u64,
                        get_next: {
                            requests: 0u64,
                            responses: 0u64,
                            result_count: 0u64,
                            result_errors: 0u64,
                            time_usec: AnyProperty,
                        }
                    },
                    component_timeouts_count: 0u64,
                    reader_servers_constructed: 0u64,
                    reader_servers_destroyed: 0u64,
                    max_snapshot_sizes_bytes: AnyProperty,
                    snapshot_schema_truncation_percentage: AnyProperty,
                    schema_truncation_count: 0u64,
                },
                stream_diagnostics_requests: 0u64,
            },
        });

        let inspector_arc = Arc::new(inspector);

        let identity = Arc::new(ComponentIdentity::new(component_id, TEST_URL));

        let (proxy, request_stream) = create_proxy_and_stream::<InspectSinkMarker>();
        proxy
            .publish(InspectSinkPublishRequest { tree: Some(tree_client), ..Default::default() })
            .unwrap();

        let scope = fasync::Scope::new();
        let inspect_sink_server =
            Arc::new(InspectSinkServer::new(Arc::clone(&inspect_repo), scope.new_child()));
        Arc::clone(&inspect_sink_server).handle(Event {
            timestamp: zx::BootInstant::get(),
            payload: EventPayload::InspectSinkRequested(InspectSinkRequestedPayload {
                component: Arc::clone(&identity),
                request_stream,
            }),
        });

        drop(proxy);

        scope.close().await;

        let expected_get_next_result_errors = match mode {
            VerifyMode::ExpectComponentFailure => 1u64,
            _ => 0u64,
        };

        {
            let result_json = read_snapshot(
                Arc::clone(&inspect_repo),
                Arc::clone(&pipeline),
                Arc::clone(&inspector_arc),
                test_batch_iterator_stats1,
            )
            .await;

            let result_array = result_json.as_array().expect("unit test json should be array.");
            assert_eq!(result_array.len(), 1, "Expect only one schema to be returned.");

            let result_map =
                result_array[0].as_object().expect("entries in the schema array are json objects.");

            let result_payload =
                result_map.get("payload").expect("diagnostics schema requires payload entry.");

            let expected_payload = match mode {
                VerifyMode::ExpectSuccess => json!({
                    "root": {
                        "child_1": {
                            "child_1_1": {
                                "some-int": 3
                            }
                        },
                        "child_2": {
                            "some-int": 2
                        }
                    }
                }),
                VerifyMode::ExpectComponentFailure => json!(null),
            };
            assert_eq!(*result_payload, expected_payload);

            // stream_diagnostics_requests is 0 since its tracked via archive_accessor server,
            // which isn't running in this unit test.
            assert_data_tree!(Arc::clone(&inspector_arc), root: {
                test_archive_accessor_node: {
                    connections_closed: 0u64,
                    connections_opened: 0u64,
                    inspect: {
                        batch_iterator_connections: {},
                        batch_iterator: {
                            connections_closed: 1u64,
                            connections_opened: 1u64,
                            get_next: {
                                time_usec: AnyProperty,
                                requests: 2u64,
                                responses: 2u64,
                                result_count: 1u64,
                                result_errors: expected_get_next_result_errors,
                            }
                        },
                        component_timeouts_count: 0u64,
                        component_time_usec: AnyProperty,
                        reader_servers_constructed: 1u64,
                        reader_servers_destroyed: 1u64,
                        schema_truncation_count: 0u64,
                        max_snapshot_sizes_bytes: AnyProperty,
                        snapshot_schema_truncation_percentage: AnyProperty,
                        longest_processing_times: contains {
                            "test_component": contains {
                                "@time": AnyProperty,
                                duration_seconds: AnyProperty,
                            }
                        },
                    },
                    logs: {
                        batch_iterator_connections: {},
                        batch_iterator: {
                            connections_closed: 0u64,
                            connections_opened: 0u64,
                            get_next: {
                                requests: 0u64,
                                responses: 0u64,
                                result_count: 0u64,
                                result_errors: 0u64,
                                time_usec: AnyProperty,
                            }
                        },
                        component_timeouts_count: 0u64,
                        reader_servers_constructed: 0u64,
                        reader_servers_destroyed: 0u64,
                        max_snapshot_sizes_bytes: AnyProperty,
                        snapshot_schema_truncation_percentage: AnyProperty,
                        schema_truncation_count: 0u64,
                    },
                    stream_diagnostics_requests: 0u64,
                },
            });
        }

        let test_batch_iterator_stats2 = Arc::new(test_accessor_stats.new_inspect_batch_iterator());

        inspect_repo.terminate_inspect(identity);
        {
            let result_json = read_snapshot(
                Arc::clone(&inspect_repo),
                Arc::clone(&pipeline),
                Arc::clone(&inspector_arc),
                test_batch_iterator_stats2,
            )
            .await;

            let result_array = result_json.as_array().expect("unit test json should be array.");
            assert_eq!(result_array.len(), 0, "Expect no schemas to be returned.");

            assert_data_tree!(Arc::clone(&inspector_arc), root: {
                test_archive_accessor_node: {
                    connections_closed: 0u64,
                    connections_opened: 0u64,
                    inspect: {
                        batch_iterator_connections: {},
                        batch_iterator: {
                            connections_closed: 2u64,
                            connections_opened: 2u64,
                            get_next: {
                                time_usec: AnyProperty,
                                requests: 3u64,
                                responses: 3u64,
                                result_count: 1u64,
                                result_errors: expected_get_next_result_errors,
                            }
                        },
                        component_timeouts_count: 0u64,
                        component_time_usec: AnyProperty,
                        reader_servers_constructed: 2u64,
                        reader_servers_destroyed: 2u64,
                        schema_truncation_count: 0u64,
                        max_snapshot_sizes_bytes: AnyProperty,
                        snapshot_schema_truncation_percentage: AnyProperty,
                        longest_processing_times: contains {
                            "test_component": contains {
                                "@time": AnyProperty,
                                duration_seconds: AnyProperty,
                            }
                        },
                    },
                    logs: {
                        batch_iterator_connections: {},
                        batch_iterator: {
                            connections_closed: 0u64,
                            connections_opened: 0u64,
                            get_next: {
                                requests: 0u64,
                                responses: 0u64,
                                result_count: 0u64,
                                result_errors: 0u64,
                                time_usec: AnyProperty,
                            }
                        },
                        component_timeouts_count: 0u64,
                        reader_servers_constructed: 0u64,
                        reader_servers_destroyed: 0u64,
                        max_snapshot_sizes_bytes: AnyProperty,
                        snapshot_schema_truncation_percentage: AnyProperty,
                        schema_truncation_count: 0u64,
                    },
                    stream_diagnostics_requests: 0u64,
                },
            });
        }
    }

    fn start_snapshot(
        inspect_repo: Arc<InspectRepository>,
        pipeline: Arc<Pipeline>,
        stats: Arc<BatchIteratorConnectionStats>,
    ) -> (BatchIteratorProxy, Task<()>) {
        let test_performance_config = PerformanceConfig {
            batch_timeout_sec: BATCH_RETRIEVAL_TIMEOUT_SECONDS,
            aggregated_content_limit_bytes: None,
            maximum_concurrent_snapshots_per_reader: 4,
        };

        let trace_id = ftrace::Id::new();
        let static_hierarchy_allowlist = pipeline.static_hierarchy_allowlist();
        let reader_server = ReaderServer::stream(
            inspect_repo.fetch_inspect_data(&None, static_hierarchy_allowlist),
            test_performance_config,
            // No selectors
            None,
            Arc::clone(&stats),
            trace_id,
        );
        let (consumer, batch_iterator_requests) = create_proxy_and_stream::<BatchIteratorMarker>();
        (
            consumer,
            Task::spawn(async {
                BatchIterator::new(
                    reader_server,
                    batch_iterator_requests.peekable(),
                    StreamMode::Snapshot,
                    stats,
                    None,
                    ftrace::Id::new(),
                    Format::Json,
                )
                .unwrap()
                .run()
                .await
                .unwrap()
            }),
        )
    }

    async fn read_snapshot(
        inspect_repo: Arc<InspectRepository>,
        pipeline: Arc<Pipeline>,
        _test_inspector: Arc<Inspector>,
        stats: Arc<BatchIteratorConnectionStats>,
    ) -> serde_json::Value {
        let (consumer, server) = start_snapshot(inspect_repo, pipeline, stats);

        let mut result_vec: Vec<String> = Vec::new();
        loop {
            let next_batch: Vec<fidl_fuchsia_diagnostics::FormattedContent> =
                consumer.get_next().await.unwrap().unwrap();

            if next_batch.is_empty() {
                break;
            }
            for formatted_content in next_batch {
                match formatted_content {
                    fidl_fuchsia_diagnostics::FormattedContent::Json(data) => {
                        let mut buf = vec![0; data.size as usize];
                        data.vmo.read(&mut buf, 0).expect("reading vmo");
                        let hierarchy_string = std::str::from_utf8(&buf).unwrap();
                        result_vec.push(hierarchy_string.to_string());
                    }
                    _ => panic!("test only produces json formatted data"),
                }
            }
        }

        // ensures connection is marked as closed, wait for stream to terminate
        drop(consumer);
        server.await;

        let result_string = format!("[{}]", result_vec.join(","));
        serde_json::from_str(&result_string).unwrap_or_else(|_| {
            panic!("unit tests shouldn't be creating malformed json: {result_string}")
        })
    }

    async fn read_snapshot_verify_batch_count_and_batch_size(
        inspect_repo: Arc<InspectRepository>,
        pipeline: Arc<Pipeline>,
        expected_batch_sizes: Vec<usize>,
        stats: Arc<BatchIteratorConnectionStats>,
    ) -> serde_json::Value {
        let (consumer, server) = start_snapshot(inspect_repo, pipeline, stats);

        let mut result_vec: Vec<String> = Vec::new();
        let mut batch_counts = Vec::new();
        loop {
            let next_batch: Vec<fidl_fuchsia_diagnostics::FormattedContent> =
                consumer.get_next().await.unwrap().unwrap();

            if next_batch.is_empty() {
                assert_eq!(expected_batch_sizes, batch_counts);
                break;
            }

            batch_counts.push(next_batch.len());

            for formatted_content in next_batch {
                match formatted_content {
                    fidl_fuchsia_diagnostics::FormattedContent::Json(data) => {
                        let mut buf = vec![0; data.size as usize];
                        data.vmo.read(&mut buf, 0).expect("reading vmo");
                        let hierarchy_string = std::str::from_utf8(&buf).unwrap();
                        result_vec.push(hierarchy_string.to_string());
                    }
                    _ => panic!("test only produces json formatted data"),
                }
            }
        }

        // ensures connection is marked as closed, wait for stream to terminate
        drop(consumer);
        server.await;

        let result_string = format!("[{}]", result_vec.join(","));
        serde_json::from_str(&result_string).unwrap_or_else(|_| {
            panic!("unit tests shouldn't be creating malformed json: {result_string}")
        })
    }
}
