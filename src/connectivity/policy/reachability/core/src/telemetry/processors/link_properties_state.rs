// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/432298933): Localize the unused tag once reachability
// starts reporting LinkProperties and LinkState time series data.
#![allow(unused)]

use super::{
    InterfaceIdentifier, InterfaceTimeSeriesGrouping, InterfaceType, identifiers_from_port_class,
};

use fidl_fuchsia_net_interfaces_ext::PortClass;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::id_enum::IdEnum;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use windowed_stats::experimental::inspect::{InspectSender, InspectedTimeMatrix};
use windowed_stats::experimental::series::interpolation::LastSample;
use windowed_stats::experimental::series::metadata::{BitsetMap, BitsetNode};
use windowed_stats::experimental::series::statistic::Union;
use windowed_stats::experimental::series::{SamplingProfile, TimeMatrix};

use crate::{IpVersions, LinkState};

#[derive(Debug)]
enum TimeSeriesType {
    // LinkProperties time series report `LinkProperties` structs, with each
    // boolean field mapped to a bit in a bitset.
    LinkProperties,
    // LinkState time series report `LinkState` enum variants, with each
    // variant mapped to a bit in a bitset.
    LinkState,
}

impl std::fmt::Display for TimeSeriesType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match *self {
            Self::LinkProperties => "link_properties",
            Self::LinkState => "link_state",
        };
        write!(f, "{}", name)
    }
}

// A representation of an interface's provisioning status.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinkProperties {
    // For IPv4, indicates the acquisition of an IPv4 address. For IPv6,
    // indicates the acquisition of a non-link local IPv6 address.
    pub has_address: bool,
    pub has_default_route: bool,
    // TODO(https://fxbug.dev/42175016): Perform DNS lookups across the
    // specified network.
    //
    // This is currently a system-wide check. For example, if one interface
    // can perform DNS resolution, then all of them will report `has_dns` true.
    pub has_dns: bool,
    // TODO(https://fxbug.dev/432303907): Separate the HTTP check from DNS.
    // HTTP can succeed without requiring DNS to be enabled and functional.
    //
    // Whether a device can successfully use the HTTP protocol.
    pub has_http_reachability: bool,
}

impl LinkProperties {
    // Each field in the LinkProperties struct represents a single bit. The
    // first field, `has_address` is the least significant bit. This function
    // must maintain ordered alignment with `link_properties_metadata`
    // in `InspectMetadataNode`.
    fn to_bitset(&self) -> u64 {
        let mut result = 0u64;
        if self.has_address {
            result |= 1;
        }

        if self.has_default_route {
            result |= 1 << 1;
        }

        if self.has_dns {
            result |= 1 << 2;
        }

        if self.has_http_reachability {
            result |= 1 << 3;
        }
        result
    }
}

#[cfg(test)]
impl From<u64> for LinkProperties {
    fn from(data: u64) -> Self {
        Self {
            has_address: (data & 1) == 1,
            has_default_route: (data & 1 << 1) >= 1,
            has_dns: (data & 1 << 2) >= 1,
            has_http_reachability: (data & 1 << 3) >= 1,
        }
    }
}

impl IdEnum for LinkState {
    type Id = u8;
    // This function must maintain ordered alignment with `link_state_metadata`
    // in `InspectMetadataNode`.
    fn to_id(&self) -> Self::Id {
        match self {
            LinkState::None => 0,
            LinkState::Removed => 1,
            LinkState::Down => 2,
            LinkState::Up => 3,
            LinkState::Local => 4,
            LinkState::Gateway => 5,
            LinkState::Internet => 6,
        }
    }
}

#[cfg(test)]
impl From<u64> for LinkState {
    fn from(data: u64) -> Self {
        match data {
            0 => Self::None,
            1 => Self::Removed,
            2 => Self::Down,
            3 => Self::Up,
            4 => Self::Local,
            5 => Self::Gateway,
            6 => Self::Internet,
            unknown => {
                log::info!("invalid ordinal provided: {data:?}");
                Self::None
            }
        }
    }
}

// Create a time series for IPv4 and IPv6 stored in `IpVersions`.
fn ip_versions_time_series<S: InspectSender>(
    client: &S,
    inspect_metadata_path: &str,
    series_type: TimeSeriesType,
    identifier: InterfaceIdentifier,
) -> IpVersions<InspectedTimeMatrix<u64>> {
    // Time series of the same type report the same metadata and can use the
    // same metadata node.
    let metadata_node = match series_type {
        TimeSeriesType::LinkProperties => InspectMetadataNode::LINK_PROPERTIES,
        TimeSeriesType::LinkState => InspectMetadataNode::LINK_STATE,
    };
    let bitset_node = BitsetNode::from_path(format!("{}/{}", inspect_metadata_path, metadata_node));
    // A separate time matrix is created for IPv4 and IPv6.
    IpVersions {
        ipv4: single_time_matrix(
            client,
            format!("{}_v4_{}", series_type, identifier),
            bitset_node.clone(),
        ),
        ipv6: single_time_matrix(client, format!("{}_v6_{}", series_type, identifier), bitset_node),
    }
}

// Create a single u64 time matrix with a highly granular sampling profile.
fn single_time_matrix<S: InspectSender>(
    client: &S,
    time_series_name: String,
    bitset_node: BitsetNode,
) -> InspectedTimeMatrix<u64> {
    client.inspect_time_matrix_with_metadata(
        time_series_name,
        TimeMatrix::<Union<u64>, LastSample>::new(
            SamplingProfile::highly_granular(),
            LastSample::or(0),
        ),
        bitset_node,
    )
}

impl IpVersions<InspectedTimeMatrix<u64>> {
    // Helper functions to make logging based on protocol cleaner.
    fn log_v4(&self, data: u64) {
        self.ipv4.fold_or_log_error(data);
    }

    fn log_v6(&self, data: u64) {
        self.ipv6.fold_or_log_error(data);
    }
}

// State for tracking link properties and link state across an interface,
// backed by time series.
struct PerInterfaceTimeSeries {
    // The most recent link properties for the interface.
    link_properties: Arc<Mutex<IpVersions<LinkProperties>>>,
    // Time matrix for tracking changes in `link_properties`.
    link_properties_time_matrix: IpVersions<InspectedTimeMatrix<u64>>,
    // The most recent link state for the interface.
    link_state: Arc<Mutex<IpVersions<LinkState>>>,
    // Time matrix for tracking changes in `link_state`.
    link_state_time_matrix: IpVersions<InspectedTimeMatrix<u64>>,
}

impl PerInterfaceTimeSeries {
    pub fn new<S: InspectSender>(
        client: &S,
        inspect_metadata_path: &str,
        identifier: InterfaceIdentifier,
    ) -> Self {
        Self {
            link_properties: Arc::new(Mutex::new(IpVersions::default())),
            link_properties_time_matrix: ip_versions_time_series(
                client,
                inspect_metadata_path,
                TimeSeriesType::LinkProperties,
                identifier.clone(),
            ),
            link_state: Arc::new(Mutex::new(IpVersions::default())),
            link_state_time_matrix: ip_versions_time_series(
                client,
                inspect_metadata_path,
                TimeSeriesType::LinkState,
                identifier,
            ),
        }
    }

    fn log_link_properties_v4(&self, link_properties: LinkProperties) {
        self.link_properties_time_matrix.log_v4(link_properties.to_bitset());
    }

    fn log_link_properties_v6(&self, link_properties: LinkProperties) {
        self.link_properties_time_matrix.log_v6(link_properties.to_bitset());
    }

    fn maybe_log_link_properties(&self, new: &IpVersions<LinkProperties>) {
        let mut curr = self.link_properties.lock();

        // TODO(https://fxbug.dev/432304519): `InterfaceType` groupings need to
        // be specially handled. If multiple interfaces of the same type
        // co-exist, then the one that is last updated will take precedence.
        // It is desired that the 'highest' rating link properties of the
        // provided type will be reported.
        if new.ipv4 != curr.ipv4 {
            curr.ipv4 = new.ipv4;
            self.log_link_properties_v4(curr.ipv4);
        }

        if new.ipv6 != curr.ipv6 {
            curr.ipv6 = new.ipv6;
            self.log_link_properties_v6(curr.ipv6);
        }
    }

    fn log_link_state_v4(&self, link_state: LinkState) {
        self.link_state_time_matrix.log_v4(1 << (link_state.to_id() as u64));
    }

    fn log_link_state_v6(&self, link_state: LinkState) {
        self.link_state_time_matrix.log_v6(1 << (link_state.to_id() as u64));
    }

    fn maybe_log_link_state(&self, new: &IpVersions<LinkState>) {
        let mut curr = self.link_state.lock();

        // TODO(https://fxbug.dev/432304519): See `maybe_log_link_properties`.
        if new.ipv4 != curr.ipv4 {
            curr.ipv4 = new.ipv4;
            self.log_link_state_v4(curr.ipv4);
        }

        if new.ipv6 != curr.ipv6 {
            curr.ipv6 = new.ipv6;
            self.log_link_state_v6(curr.ipv6);
        }
    }
}

// The wrapper for the time series reporting for LinkProperties and LinkState.
pub struct LinkPropertiesStateLogger {
    // Tracks the provided `InterfaceIdentifier`s against the time series for
    // that identifier. Entries are only created during initialization.
    time_series_stats: HashMap<InterfaceIdentifier, PerInterfaceTimeSeries>,
    inspect_metadata_node: InspectMetadataNode,
}

impl LinkPropertiesStateLogger {
    pub fn new<S: InspectSender>(
        inspect_metadata_node: &InspectNode,
        inspect_metadata_path: &str,
        interface_grouping: InterfaceTimeSeriesGrouping,
        time_matrix_client: &S,
    ) -> Self {
        Self {
            // Create a time series per interface type provided.
            time_series_stats: match interface_grouping {
                InterfaceTimeSeriesGrouping::Type(tys) => tys.into_iter().map(|ty| {
                    let identifier = InterfaceIdentifier::Type(ty);
                    (
                        identifier.clone(),
                        PerInterfaceTimeSeries::new(
                            time_matrix_client,
                            inspect_metadata_path,
                            identifier,
                        ),
                    )
                }),
            }
            .collect(),
            inspect_metadata_node: InspectMetadataNode::new(inspect_metadata_node),
        }
    }

    // Update an interface's `LinkProperties`. `interface_identifiers`
    // represent the various ways that an interface can be identified. When an
    // identifier matches an attribute that is being tracked in
    // `time_series_stats`, attempt to log that `LinkProperties` update.
    pub fn update_link_properties(
        &self,
        interface_identifiers: Vec<InterfaceIdentifier>,
        link_properties: &IpVersions<LinkProperties>,
    ) {
        interface_identifiers.iter().for_each(|identifier| {
            if let Some(time_series) = self.time_series_stats.get(identifier) {
                time_series.maybe_log_link_properties(&link_properties);
            }
        });
    }

    // Update an interface's `LinkState`. `interface_identifiers` represent the
    // various ways that an interface can be identified. When an identifier
    // matches an attribute that is being tracked in `time_series_stats`,
    // attempt to log that `LinkState` update.
    pub fn update_link_state(
        &self,
        interface_identifiers: Vec<InterfaceIdentifier>,
        link_state: &IpVersions<LinkState>,
    ) {
        interface_identifiers.iter().for_each(|identifier| {
            if let Some(time_series) = self.time_series_stats.get(identifier) {
                time_series.maybe_log_link_state(&link_state);
            }
        });
    }
}

// Holds the inspect node children for the static metadata that correlates to
// bits in each of the corresponding structs / enums.
struct InspectMetadataNode {
    link_properties: InspectNode,
    link_state: InspectNode,
}

impl InspectMetadataNode {
    const LINK_PROPERTIES: &'static str = "link_properties";
    const LINK_STATE: &'static str = "link_state";

    fn new(inspect_node: &InspectNode) -> Self {
        let link_properties = inspect_node.create_child(Self::LINK_PROPERTIES);
        let link_state = inspect_node.create_child(Self::LINK_STATE);

        let link_properties_metadata = BitsetMap::from_ordered([
            "has_address",
            "has_default_route",
            "has_dns",
            "has_http_reachability",
        ]);
        let link_state_metadata = BitsetMap::from_ordered([
            "None", "Removed", "Down", "Up", "Local", "Gateway", "Internet",
        ]);

        link_properties_metadata.record(&link_properties);
        link_state_metadata.record(&link_state);

        Self { link_properties, link_state }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyBytesProperty, AnyNumericProperty, assert_data_tree};

    use crate::telemetry::testing::setup_test;
    use windowed_stats::experimental::clock::Timed;
    use windowed_stats::experimental::inspect::TimeMatrixClient;
    use windowed_stats::experimental::testing::TimeMatrixCall;

    #[fuchsia::test]
    fn test_log_time_series_metadata_to_inspect() {
        let mut harness = setup_test();

        let client =
            TimeMatrixClient::new(harness.inspect_node.create_child("link_properties_state"));
        let _link_properties_state = LinkPropertiesStateLogger::new(
            &harness.inspect_metadata_node,
            &harness.inspect_metadata_path,
            InterfaceTimeSeriesGrouping::Type(vec![InterfaceType::Ethernet]),
            &client,
        );

        let tree = harness.get_inspect_data_tree();
        assert_data_tree!(
            @executor harness.exec,
            tree,
            root: contains {
                test_stats: contains {
                    metadata: {
                        link_properties: {
                            index: {
                                "0": "has_address",
                                "1": "has_default_route",
                                "2": "has_dns",
                                "3": "has_http_reachability",
                            },
                        },
                        link_state: {
                            index: {
                                "0": "None",
                                "1": "Removed",
                                "2": "Down",
                                "3": "Up",
                                "4": "Local",
                                "5": "Gateway",
                                "6": "Internet",
                            },
                        },
                    },
                    link_properties_state: contains {
                        link_properties_v4_TYPE_ethernet: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/link_properties",
                            }
                        },
                        link_properties_v6_TYPE_ethernet: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/link_properties",
                            }
                        },
                        link_state_v4_TYPE_ethernet: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/link_state",
                            }
                        },
                        link_state_v6_TYPE_ethernet: {
                            "type": "bitset",
                            "data": AnyBytesProperty,
                            metadata: {
                                index_node_path: "root/test_stats/metadata/link_state",
                            }
                        }
                    }
                }
            }
        )
    }

    #[fuchsia::test]
    fn test_log_link_properties() {
        let harness = setup_test();

        let link_properties_state = LinkPropertiesStateLogger::new(
            &harness.inspect_metadata_node,
            &harness.inspect_metadata_path,
            InterfaceTimeSeriesGrouping::Type(vec![InterfaceType::Ethernet]),
            &harness.mock_time_matrix_client,
        );

        // Update the link properties with an interface type not present in the types
        // provided to `LinkPropertiesStateLogger`.
        link_properties_state.update_link_properties(
            vec![InterfaceIdentifier::Type(InterfaceType::WlanClient)],
            &IpVersions {
                ipv4: LinkProperties { has_address: true, ..Default::default() },
                ipv6: LinkProperties::default(),
            },
        );

        // There should be no calls to the `TYPE_ethernet` time series since the
        // update above was for `WlanClient`. There should be no calls to the
        // `TYPE_wlanclient` field either since they were not initialized.
        let mut time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_ethernet")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_ethernet")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_wlanclient")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_wlanclient")[..], &[]);

        // Update the link properties with a present interface type.
        link_properties_state.update_link_properties(
            vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
            &IpVersions {
                ipv4: LinkProperties { has_address: true, ..Default::default() },
                ipv6: LinkProperties {
                    has_address: true,
                    has_default_route: true,
                    ..Default::default()
                },
            },
        );

        time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        // The first bit is set for the v4 call, since `has_address` is true.
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << 0)),]
        );
        // The first and second bit are set for the v6 call, since `has_address`
        // and `has_default_route` are true.
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now((1 << 0) | (1 << 1))),]
        );

        // Ensure that updating the same identifier with the same properties
        // for v4 results in no calls to the v4 time matrix.
        link_properties_state.update_link_properties(
            vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
            &IpVersions {
                ipv4: LinkProperties { has_address: true, ..Default::default() },
                ipv6: LinkProperties {
                    has_address: true,
                    has_default_route: true,
                    has_dns: true,
                    ..Default::default()
                },
            },
        );
        time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_ethernet")[..], &[]);
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now((1 << 0) | (1 << 1) | (1 << 2))),]
        );
    }

    #[fuchsia::test]
    fn test_log_link_state() {
        let harness = setup_test();

        let link_properties_state = LinkPropertiesStateLogger::new(
            &harness.inspect_metadata_node,
            &harness.inspect_metadata_path,
            InterfaceTimeSeriesGrouping::Type(vec![InterfaceType::Ethernet]),
            &harness.mock_time_matrix_client,
        );

        // Update the link state with an interface type not present in the types
        // provided to `LinkPropertiesStateLogger`.
        link_properties_state.update_link_state(
            vec![InterfaceIdentifier::Type(InterfaceType::WlanClient)],
            &IpVersions { ipv4: Default::default(), ipv6: LinkState::Gateway },
        );

        // There should be no calls to the `TYPE_ethernet` time series since the
        // update above was for `WlanClient`. There should be no calls to the
        // `TYPE_wlanclient` field either since they were not initialized.
        let mut time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v4_TYPE_ethernet")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v6_TYPE_ethernet")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v4_TYPE_wlanclient")[..], &[]);
        assert_eq!(&time_matrix_calls.drain::<u64>("link_properties_v6_TYPE_wlanclient")[..], &[]);

        // Update the link state with a present interface type.
        link_properties_state.update_link_state(
            vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
            &IpVersions { ipv4: LinkState::Internet, ipv6: LinkState::Local },
        );

        time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_state_v4_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << LinkState::Internet.to_id())),]
        );
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_state_v6_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << LinkState::Local.to_id())),]
        );

        // Ensure that updating the same identifier with the same properties
        // for v4 results in no calls to the v4 time matrix.
        link_properties_state.update_link_state(
            vec![InterfaceIdentifier::Type(InterfaceType::Ethernet)],
            &IpVersions { ipv4: LinkState::Internet, ipv6: LinkState::Gateway },
        );
        time_matrix_calls = harness.mock_time_matrix_client.drain_calls();
        assert_eq!(&time_matrix_calls.drain::<u64>("link_state_v4_TYPE_ethernet")[..], &[]);
        assert_eq!(
            &time_matrix_calls.drain::<u64>("link_state_v6_TYPE_ethernet")[..],
            &[TimeMatrixCall::Fold(Timed::now(1 << LinkState::Gateway.to_id())),]
        );
    }
}
