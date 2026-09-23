// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::telemetry::NetworkEventMetadata;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::nodes::LruCacheNode;
use fuchsia_inspect_derive::Unit;
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use windowed_stats::experimental::series::interpolation::LastSample;
use windowed_stats::experimental::series::metadata::{BitsetMap, BitsetNode};
use windowed_stats::experimental::series::statistic::Union;
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};

pub struct NetworkPropertiesProcessor<S: InspectSender> {
    default_network_detailed_matrix: InspectedTimeMatrix<u64>,
    default_network_type_matrix: InspectedTimeMatrix<u64>,
    inspect_metadata_node: InspectMetadataNode,
    connectivity_matrices: Vec<Option<NetworkConnectivityTimeSeries<S>>>,
    inspect_metadata_path: String,
}

struct NetworkConnectivityTimeSeries<S> {
    network_id: u64,
    _client: S,
    matrix: InspectedTimeMatrix<u64>,
}

const METADATA_NODE_NAME: &str = "metadata";

impl<S: InspectSender> NetworkPropertiesProcessor<S> {
    pub fn new(parent: &InspectNode, parent_path: &str, client: &S) -> Self {
        let inspect_metadata_node = parent.create_child(METADATA_NODE_NAME);
        let inspect_metadata_path = format!("{}/{}", parent_path, METADATA_NODE_NAME);
        let detailed_time_matrix = TimeMatrix::<Union<u64>, LastSample>::new(
            SamplingProfile::granular(),
            LastSample::or(0),
        );
        let default_network_detailed_matrix = client.inspect_time_matrix_with_metadata(
            "default_network_detailed",
            detailed_time_matrix,
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::NETWORK_REGISTRY
            )),
        );

        let types_time_matrix = TimeMatrix::<Union<u64>, LastSample>::new(
            SamplingProfile::granular(),
            LastSample::or(0),
        );
        let default_network_type_matrix = client.inspect_time_matrix_with_metadata(
            "default_network_type",
            types_time_matrix,
            BitsetNode::from_path(format!(
                "{}/{}",
                inspect_metadata_path,
                InspectMetadataNode::NETWORK_TYPES
            )),
        );

        let mut connectivity_matrices = Vec::with_capacity(NETWORKS_METADATA_CACHE_SIZE);
        for _ in 0..NETWORKS_METADATA_CACHE_SIZE {
            connectivity_matrices.push(None);
        }

        Self {
            default_network_detailed_matrix,
            default_network_type_matrix,
            inspect_metadata_node: InspectMetadataNode::new(inspect_metadata_node),
            connectivity_matrices,
            inspect_metadata_path,
        }
    }

    pub fn log_default_network_lost(&mut self) {
        self.default_network_detailed_matrix.fold_or_log_error(0);
        self.default_network_type_matrix.fold_or_log_error(0);
    }

    pub fn log_default_network_changed(&mut self, metadata: NetworkEventMetadata) {
        let data = NetworkData::from(metadata);
        let types_mapped_id =
            self.inspect_metadata_node.network_types.insert(data.transport.clone());
        self.default_network_type_matrix.fold_or_log_error(1u64 << types_mapped_id);

        let detailed_mapped_id = self.inspect_metadata_node.network_registry.insert(data);
        self.default_network_detailed_matrix.fold_or_log_error(1u64 << detailed_mapped_id);
    }

    pub fn log_network_changed(
        &mut self,
        metadata: crate::telemetry::NetworkEventMetadata,
        client: &S,
    ) {
        let network_id = metadata.id;
        let connectivity_state = metadata.connectivity_state;

        let data = NetworkData::from(metadata);
        let detailed_mapped_id = self.inspect_metadata_node.network_registry.insert(data);

        let connectivity_state = match connectivity_state {
            Some(state) => state,
            None => return,
        };

        let bit_index = match connectivity_state_to_bit_index(connectivity_state) {
            Some(index) => index,
            None => return,
        };

        // If the slot holds a different network_id, overwrite it.
        let needs_new_matrix = match self.connectivity_matrices.get(detailed_mapped_id) {
            Some(Some(time_series)) => time_series.network_id != network_id,
            Some(None) | None => true,
        };

        if needs_new_matrix {
            // Overwriting with None drops the old node and evicts the time series.
            self.connectivity_matrices[detailed_mapped_id] = None;

            let node_name = format!("network_{}", network_id);
            let scoped_client = client.clone_with_child(&node_name);

            let time_matrix = TimeMatrix::<Union<u64>, LastSample>::new(
                SamplingProfile::highly_granular(),
                LastSample::or(0),
            );

            let matrix = scoped_client.inspect_time_matrix_with_metadata(
                "connectivity",
                time_matrix,
                BitsetNode::from_path(format!(
                    "{}/connectivity_states",
                    self.inspect_metadata_path
                )),
            );

            self.connectivity_matrices[detailed_mapped_id] =
                Some(NetworkConnectivityTimeSeries { network_id, _client: scoped_client, matrix });
        }

        if let Some(ts) = &self.connectivity_matrices[detailed_mapped_id] {
            ts.matrix.fold_or_log_error(1u64 << bit_index);
        }
    }
}

#[derive(Unit, PartialEq, Eq, Hash)]
struct NetworkData {
    pub id: u64,
    pub name: String,
    pub transport: String,
    pub is_fuchsia_provisioned: bool,
}

impl From<NetworkEventMetadata> for NetworkData {
    fn from(metadata: NetworkEventMetadata) -> Self {
        let NetworkEventMetadata { id, name, transport, is_fuchsia_provisioned, .. } = metadata;
        Self {
            id: id,
            name: name.unwrap_or_else(|| "unknown".to_string()),
            transport: format!("{:?}", transport),
            is_fuchsia_provisioned,
        }
    }
}

fn get_ordered_connectivity_states() -> [&'static str; 4] {
    ["NoConnectivity", "LocalConnectivity", "PartialConnectivity", "FullConnectivity"]
}

fn connectivity_state_to_bit_index(state: fnp_socketproxy::ConnectivityState) -> Option<u8> {
    match state {
        fnp_socketproxy::ConnectivityState::NoConnectivity => Some(0),
        fnp_socketproxy::ConnectivityState::LocalConnectivity => Some(1),
        fnp_socketproxy::ConnectivityState::PartialConnectivity => Some(2),
        fnp_socketproxy::ConnectivityState::FullConnectivity => Some(3),
        _ => None,
    }
}

const NETWORKS_METADATA_CACHE_SIZE: usize = 16;
const NETWORK_TYPES_CACHE_SIZE: usize = 8;

// Holds the inspect node children for the metadata that correlates to
// bits in the default network bitsets.
struct InspectMetadataNode {
    _node: InspectNode,
    network_registry: LruCacheNode<NetworkData>,
    network_types: LruCacheNode<String>,
    _connectivity_states: InspectNode,
}

impl InspectMetadataNode {
    const NETWORK_REGISTRY: &'static str = "network_registry";
    const NETWORK_TYPES: &'static str = "network_types";

    fn new(inspect_node: InspectNode) -> Self {
        // Record the network registry, which is dynamically updated as networks
        // are added.
        let network_registry = LruCacheNode::new(
            inspect_node.create_child(Self::NETWORK_REGISTRY),
            NETWORKS_METADATA_CACHE_SIZE,
        );

        // Record the observed network types for the default network type time matrix.
        let network_types = LruCacheNode::new(
            inspect_node.create_child(Self::NETWORK_TYPES),
            NETWORK_TYPES_CACHE_SIZE,
        );

        let connectivity_states = inspect_node.create_child("connectivity_states");
        let connectivity_metadata = BitsetMap::from_ordered(get_ordered_connectivity_states());
        connectivity_metadata.record(&connectivity_states);

        Self {
            _node: inspect_node,
            network_registry,
            network_types,
            _connectivity_states: connectivity_states,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyBytesProperty, assert_data_tree};
    use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
    use fuchsia_inspect::Inspector;
    use fuchsia_inspect::reader::DiagnosticsHierarchy;
    use futures::task::Poll;
    use std::pin::pin;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::inspect::TimeMatrixClient;
    use windowed_stats::experimental::testing::{MockTimeMatrixClient, TimeMatrixCall};

    pub struct TestHelper {
        pub inspector: Inspector,
        pub inspect_node: InspectNode,
        pub parent_path: String,
        pub mock_time_matrix_client: MockTimeMatrixClient,

        // Note: keep the executor field last in the struct so it gets dropped last.
        pub exec: fuchsia_async::TestExecutor,
    }

    impl TestHelper {
        pub fn get_inspect_data_tree(&mut self) -> DiagnosticsHierarchy {
            let read_fut = fuchsia_inspect::reader::read(&self.inspector);
            let mut read_fut = pin!(read_fut);
            match self.exec.run_until_stalled(&mut read_fut) {
                Poll::Pending => {
                    panic!("Unexpected pending state");
                }
                Poll::Ready(result) => result.expect("failed to get hierarchy"),
            }
        }
    }

    pub fn setup_test() -> TestHelper {
        let exec = fuchsia_async::TestExecutor::new_with_fake_time();
        exec.set_fake_time(fuchsia_async::MonotonicInstant::from_nanos(0));

        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("test_stats");
        let parent_path = "root/test_stats".to_string();

        TestHelper {
            inspector,
            inspect_node,
            parent_path,
            mock_time_matrix_client: MockTimeMatrixClient::new(),
            exec,
        }
    }

    fn log_network_events<S: InspectSender>(processor: &mut NetworkPropertiesProcessor<S>) {
        let eth_metadata = NetworkEventMetadata {
            id: 0,
            name: Some("eth0".to_string()),
            transport: fnp_socketproxy::NetworkType::Ethernet,
            is_fuchsia_provisioned: true,
            connectivity_state: None,
        };

        let wlan_metadata = NetworkEventMetadata {
            id: 1,
            name: Some("wlan0".to_string()),
            transport: fnp_socketproxy::NetworkType::Wifi,
            is_fuchsia_provisioned: false,
            connectivity_state: None,
        };

        processor.log_default_network_changed(eth_metadata);
        processor.log_default_network_lost();
        processor.log_default_network_changed(wlan_metadata);
    }

    #[fuchsia::test]
    fn log_default_network_time_series_calls() {
        let harness = setup_test();
        let mut processor = NetworkPropertiesProcessor::new(
            &harness.inspect_node,
            &harness.parent_path,
            &harness.mock_time_matrix_client,
        );
        log_network_events(&mut processor);

        let mut time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("default_network_detailed")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)),
                TimeMatrixCall::Fold(Timed::now(0)),
                TimeMatrixCall::Fold(Timed::now(1 << 1)),
            ]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("default_network_type")[..],
            &[
                TimeMatrixCall::Fold(Timed::now(1 << 0)),
                TimeMatrixCall::Fold(Timed::now(0)),
                TimeMatrixCall::Fold(Timed::now(1 << 1)),
            ]
        );
    }

    #[fuchsia::test]
    fn log_network_connectivity_time_series_calls() {
        let harness = setup_test();
        let mut processor = NetworkPropertiesProcessor::new(
            &harness.inspect_node,
            &harness.parent_path,
            &harness.mock_time_matrix_client,
        );

        let eth_metadata = NetworkEventMetadata {
            id: 0,
            name: Some("eth0".to_string()),
            transport: fnp_socketproxy::NetworkType::Ethernet,
            is_fuchsia_provisioned: true,
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::FullConnectivity),
        };

        let wlan_metadata = NetworkEventMetadata {
            id: 1,
            name: Some("wlan0".to_string()),
            transport: fnp_socketproxy::NetworkType::Wifi,
            is_fuchsia_provisioned: false,
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::LocalConnectivity),
        };

        processor.log_network_changed(eth_metadata, &harness.mock_time_matrix_client);
        processor.log_network_changed(wlan_metadata, &harness.mock_time_matrix_client);

        let mut time_matrix_calls = harness.mock_time_matrix_client.drain_calls();

        // FullConnectivity maps to bit 3 -> value 2^3 = 8
        assert_eq!(
            &time_matrix_calls.drain::<u64>("network_0/connectivity")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 3))]
        );

        // LocalConnectivity maps to bit 1 -> value 2^1 = 2
        assert_eq!(
            &time_matrix_calls.drain::<u64>("network_1/connectivity")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 1))]
        );
    }

    #[fuchsia::test]
    fn log_network_connectivity_inspect_tree() {
        let mut harness = setup_test();
        let time_matrix_client =
            TimeMatrixClient::new(harness.inspect_node.create_child("time_series"));
        let mut processor = NetworkPropertiesProcessor::new(
            &harness.inspect_node,
            &harness.parent_path,
            &time_matrix_client,
        );

        let eth_metadata = NetworkEventMetadata {
            id: 0,
            name: Some("eth0".to_string()),
            transport: fnp_socketproxy::NetworkType::Ethernet,
            is_fuchsia_provisioned: true,
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::FullConnectivity),
        };

        processor.log_network_changed(eth_metadata, &time_matrix_client);

        let hierarchy = harness.get_inspect_data_tree();

        assert_data_tree!(
            @executor harness.exec,
            hierarchy,
            root: contains {
                test_stats: contains {
                    metadata: contains {
                        connectivity_states: contains {
                            index: contains {
                                "0": "NoConnectivity",
                                "1": "LocalConnectivity",
                                "2": "PartialConnectivity",
                                "3": "FullConnectivity",
                            }
                        }
                    },
                    time_series: contains {
                        network_0: contains {
                            connectivity: contains {
                                "type": "bitset",
                                "data": AnyBytesProperty,
                                metadata: {
                                    index_node_path: "root/test_stats/metadata/connectivity_states",
                                }
                            }
                        }
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    fn log_default_network_inspect_tree() {
        let mut harness = setup_test();
        let time_matrix_client =
            TimeMatrixClient::new(harness.inspect_node.create_child("time_series"));
        let mut processor = NetworkPropertiesProcessor::new(
            &harness.inspect_node,
            &harness.parent_path,
            &time_matrix_client,
        );
        log_network_events(&mut processor);

        let hierarchy = harness.get_inspect_data_tree();

        assert_data_tree!(
            @executor harness.exec,
            hierarchy,
            root: contains {
                test_stats: contains {
                    metadata: contains {
                        network_registry: contains {
                            "0": contains {
                                data: {
                                    id: 0u64,
                                    name: "eth0",
                                    transport: "Ethernet",
                                    is_fuchsia_provisioned: true,
                                }
                            },
                            "1": contains {
                                data: {
                                    id: 1u64,
                                    name: "wlan0",
                                    transport: "Wifi",
                                    is_fuchsia_provisioned: false,
                                }
                            }
                        },
                        network_types: contains {
                            "0": contains {
                                data: "Ethernet",
                            },
                            "1": contains {
                                data: "Wifi",
                            }
                        }
                    },
                    time_series: contains {
                        default_network_detailed: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/network_registry",
                            }
                        },
                        default_network_type: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/network_types",
                            }
                        },
                    }
                }
            }
        );
    }

    #[fuchsia::test]
    fn log_network_connectivity_eviction() {
        let mut harness = setup_test();
        let time_matrix_client =
            TimeMatrixClient::new(harness.inspect_node.create_child("time_series"));
        let mut processor = NetworkPropertiesProcessor::new(
            &harness.inspect_node,
            &harness.parent_path,
            &time_matrix_client,
        );

        // Log 16 unique networks to fill the cache.
        for i in 0..16 {
            let metadata = NetworkEventMetadata {
                id: i as u64,
                name: Some(format!("eth{}", i)),
                transport: fnp_socketproxy::NetworkType::Ethernet,
                is_fuchsia_provisioned: true,
                connectivity_state: Some(fnp_socketproxy::ConnectivityState::FullConnectivity),
            };
            processor.log_network_changed(metadata, &time_matrix_client);
        }

        // network_0 should exist before eviction.
        let hierarchy_before = harness.get_inspect_data_tree();
        assert_data_tree!(
            @executor harness.exec,
            hierarchy_before,
            root: contains {
                test_stats: contains {
                    time_series: contains {
                        network_0: contains {}
                    }
                }
            }
        );

        // Log a 17th network. This should trigger eviction of slot 0 (network 0).
        let metadata = NetworkEventMetadata {
            id: 16,
            name: Some("eth16".to_string()),
            transport: fnp_socketproxy::NetworkType::Ethernet,
            is_fuchsia_provisioned: true,
            connectivity_state: Some(fnp_socketproxy::ConnectivityState::LocalConnectivity),
        };
        processor.log_network_changed(metadata, &time_matrix_client);

        let hierarchy = harness.get_inspect_data_tree();

        // Verify that network_16 is present.
        assert_data_tree!(
            @executor harness.exec,
            hierarchy,
            root: contains {
                test_stats: contains {
                    time_series: contains {
                        network_16: contains {}
                    }
                }
            }
        );

        // Verify that network_0 is not in the tree. There is no assert_data_tree! macro for
        // negative assertions.
        let test_stats = hierarchy.children.iter().find(|c| c.name == "test_stats").unwrap();
        let time_series = test_stats.children.iter().find(|c| c.name == "time_series").unwrap();
        assert!(
            !time_series.children.iter().any(|c| c.name == "network_0"),
            "network_0 was not evicted from Inspect!"
        );
    }
}
