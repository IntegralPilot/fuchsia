// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Integration tests that exercise net-cli commands within a network-test-realm.
//! For simply integration testing net-cli, network-test-realm would be unnecessary -- netemul is
//! sufficient and running network-test-realm via netemul is redundant.
//! However, because there are out-of-tree end-to-end test usages of net-cli within
//! network-test-realm, it's helpful to exercise net-cli-within-network-test-realm in this way
//! in order to help validate that the correct capabilities are available in-tree.

#![cfg(test)]

mod capture;
mod filter;

use std::collections::HashMap;

use anyhow::Result;
use argh::FromArgs as _;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_net_test_realm as fntr;
use futures::StreamExt as _;
use net_cli::{UserFacingError, underlying_user_facing_error};
use net_declare::{fidl_ip_v6, fidl_mac};
use net_types::ip::{IpVersion, Ipv4, Ipv6};
use netemul::TestRealm;
use netstack_testing_common::realms::KnownServiceProvider;
use test_case::test_case;

struct NetworkTestRealmConnector<'a> {
    realm: &'a netemul::TestRealm<'a>,
}

#[async_trait::async_trait]
impl<'a, P: fidl::endpoints::DiscoverableProtocolMarker> net_cli::ServiceConnector<P>
    for NetworkTestRealmConnector<'a>
{
    async fn connect(
        &self,
    ) -> Result<<P as fidl::endpoints::ProtocolMarker>::Proxy, anyhow::Error> {
        Ok(connect_to_hermetic_network_realm_protocol::<P>(self.realm).await)
    }
}

struct TestRealmConnector<'a> {
    pub(crate) realm: &'a netemul::TestRealm<'a>,
}

#[async_trait::async_trait]
impl<'a, P: fidl::endpoints::DiscoverableProtocolMarker> net_cli::ServiceConnector<P>
    for TestRealmConnector<'a>
{
    async fn connect(
        &self,
    ) -> Result<<P as fidl::endpoints::ProtocolMarker>::Proxy, anyhow::Error> {
        self.realm.connect_to_protocol::<P>()
    }
}

pub(crate) struct TestDeps;
impl net_cli::CaptureDeps for TestDeps {
    type OutputWriter = std::io::Sink;
    fn create_output_writer(
        &self,
        _path: &std::path::Path,
    ) -> Result<Self::OutputWriter, anyhow::Error> {
        Ok(std::io::sink())
    }
}

impl<'a> NetworkTestRealmConnector<'a> {
    async fn connect_to_protocol<P: fidl::endpoints::DiscoverableProtocolMarker>(
        &self,
    ) -> <P as fidl::endpoints::ProtocolMarker>::Proxy {
        connect_to_hermetic_network_realm_protocol::<P>(self.realm).await
    }
}

/// Connects to a protocol within the hermetic network realm.
async fn connect_to_hermetic_network_realm_protocol<
    P: fidl::endpoints::DiscoverableProtocolMarker,
>(
    realm: &netemul::TestRealm<'_>,
) -> P::Proxy {
    let realm_proxy = realm
        .connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("failed to connect to realm protocol");
    let (directory_proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    let child_ref = network_test_realm::create_hermetic_network_realm_child_ref();
    realm_proxy
        .open_exposed_dir(&child_ref, server_end)
        .await
        .expect("open_exposed_dir failed")
        .expect("open_exposed_dir error");
    fuchsia_component::client::connect_to_protocol_at_dir_root::<P>(&directory_proxy)
        .unwrap_or_else(|e| {
            panic!(
                "failed to connect to hermetic network realm protocol {} with error: {:?}",
                P::PROTOCOL_NAME,
                e
            )
        })
}

/// Adds an interface to the hermetic Netstack with `interface_name` and
/// `mac_address`.
///
/// The added interface is assigned a static IP address based on `subnet`.
/// Additionally, the interface joins the provided `network`.
async fn join_network_with_hermetic_netstack<'a>(
    realm: &'a netemul::TestRealm<'a>,
    network: &'a netemul::TestNetwork<'a>,
    interface_name: &'a str,
    mac_address: fnet::MacAddress,
    subnet: fnet::Subnet,
) -> (
    netemul::TestEndpoint<'a>,
    u64,
    fnet_interfaces_ext::admin::Control,
    fnet_interfaces_admin::DeviceControlProxy,
) {
    let installer =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces_admin::InstallerMarker>(realm)
            .await;

    let interface_state =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces::StateMarker>(realm).await;

    let (endpoint, id, control, device_control) = realm
        .join_network_with_installer(
            &network,
            installer,
            interface_state,
            interface_name,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(mac_address)),
            netemul::InterfaceConfig { name: Some(interface_name.into()), ..Default::default() },
        )
        .await
        .expect("join_network failed");

    let address_state_provider = netstack_testing_common::interfaces::add_address_wait_assigned(
        &control,
        subnet,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("add_address_wait_assigned failed");

    // Allow the address to live beyond the `address_state_provider` handle.
    address_state_provider.detach().expect("detatch failed");

    (endpoint, id, control, device_control)
}

const INTERFACE1_NAME: &'static str = "iface1";
const INTERFACE1_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:03:04:05:06:07");
const DEFAULT_IPV6_LINK_LOCAL_SOURCE_ADDR: fnet::Ipv6Address = fidl_ip_v6!("fe80::2");
const DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET: fnet::Subnet = fnet::Subnet {
    addr: fnet::IpAddress::Ipv6(DEFAULT_IPV6_LINK_LOCAL_SOURCE_ADDR),
    prefix_len: 64,
};

#[test_case(IpVersion::V4)]
#[test_case(IpVersion::V6)]
#[fuchsia::test]
async fn add_del_route(ip_version: IpVersion) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = sandbox
        .create_realm(
            "net-cli-test-realm",
            &[KnownServiceProvider::NetworkTestRealm { require_outer_netstack: false }],
        )
        .expect("creating realm should succeed");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(fntr::Netstack::V3)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let (_endpoint, nicid, _control, _device_control) = join_network_with_hermetic_netstack(
        &realm,
        &network,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    )
    .await;

    let connector = NetworkTestRealmConnector { realm: &realm };

    let list_routes = || async {
        let buffers = writer::TestBuffers::default();
        net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(&["net"], &["route", "list"])
                .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
        let list: Vec<serde_json::Value> =
            serde_json::from_slice(&buffers.into_stdout()[..]).expect("should be valid JSON array");
        list
    };

    let initial_routes = list_routes().await;

    let (destination, gateway, prefix_len) = match ip_version {
        IpVersion::V4 => ("1.2.3.4", "1.2.3.5", 32),
        IpVersion::V6 => ("::1:2:3:4:5", "::1:2:3:4:6", 128),
    };

    let metric = 111;

    let table_id = match ip_version {
        IpVersion::V4 => 0,
        IpVersion::V6 => 1,
    };

    let route_record = serde_json::json!({
        "destination": {
            "addr": destination,
            "prefix_len": prefix_len,
        },
        "gateway": gateway,
        "metric": metric,
        "nicid": nicid,
        "table_id": table_id,
    });

    assert!(
        !initial_routes.contains(&route_record),
        "{initial_routes:?} should not contain {route_record:?}"
    );

    {
        let prefix_len = prefix_len.to_string();
        let nicid = nicid.to_string();
        let metric = metric.to_string();
        net_cli::do_root(
            writer::JsonWriter::new(None),
            net_cli::Command::from_args(
                &["net"],
                &[
                    "route",
                    "add",
                    "--destination",
                    destination,
                    "--netmask",
                    prefix_len.as_str(),
                    "--gateway",
                    gateway,
                    "--nicid",
                    nicid.as_str(),
                    "--metric",
                    metric.as_str(),
                ],
            )
            .expect("should parse successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
    }

    let after_add_routes = list_routes().await;

    assert!(
        after_add_routes.contains(&route_record),
        "{after_add_routes:?} should contain {route_record:?}"
    );

    {
        let prefix_len = prefix_len.to_string();
        let nicid = nicid.to_string();
        let metric = metric.to_string();
        net_cli::do_root(
            writer::JsonWriter::new(None),
            net_cli::Command::from_args(
                &["net"],
                &[
                    "route",
                    "del",
                    "--destination",
                    destination,
                    "--netmask",
                    prefix_len.as_str(),
                    "--gateway",
                    gateway,
                    "--nicid",
                    nicid.as_str(),
                    "--metric",
                    metric.as_str(),
                ],
            )
            .expect("should parse successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
    }

    let after_remove_routes = list_routes().await;

    assert!(
        !after_remove_routes.contains(&route_record),
        "{after_remove_routes:?} should not contain {route_record:?}"
    );
}

#[fuchsia::test]
async fn rule_list() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = sandbox
        .create_realm(
            "net-cli-test-realm",
            &[KnownServiceProvider::NetworkTestRealm { require_outer_netstack: false }],
        )
        .expect("creating realm should succeed");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(fntr::Netstack::V3)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let (_endpoint, _nicid, _control, _device_control) = join_network_with_hermetic_netstack(
        &realm,
        &network,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    )
    .await;

    async fn default_rule<
        I: fnet_routes_ext::admin::FidlRouteAdminIpExt + fnet_routes_ext::FidlRouteIpExt,
    >(
        realm: &TestRealm<'_>,
    ) -> serde_json::Value {
        let main_table =
            connect_to_hermetic_network_realm_protocol::<I::RouteTableMarker>(&realm).await;
        let main_table_id = fnet_routes_ext::admin::get_table_id::<I>(&main_table)
            .await
            .expect("get main table id");
        serde_json::json!({
            "action": format!("lookup {main_table_id}"),
            "bound_device": null,
            "from": null,
            "index": 0,
            "locally_generated": null,
            "mark_1": null,
            "mark_2": null,
            "rule_set_priority": u32::from(fnet_routes_ext::rules::DEFAULT_RULE_SET_PRIORITY),
        })
    }
    let v4_default_rule = default_rule::<Ipv4>(&realm).await;
    let v6_default_rule = default_rule::<Ipv6>(&realm).await;

    let connector = NetworkTestRealmConnector { realm: &realm };
    let list_rules = || async {
        let buffers = writer::TestBuffers::default();
        net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(&["net"], &["rule", "list"])
                .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
        let list: Vec<serde_json::Value> =
            serde_json::from_slice(&buffers.into_stdout()[..]).expect("should be valid JSON array");
        list
    };

    let initial_rules = list_rules().await;

    // Initially, only the default rules are installed.
    assert_eq!(&initial_rules[..], &[v4_default_rule.clone(), v6_default_rule.clone()]);

    let rule_table_v6 =
        connect_to_hermetic_network_realm_protocol::<fnet_routes_admin::RuleTableV6Marker>(&realm)
            .await;

    const RULE_SET_PRIORITY: u32 = 42;
    let rule_set_v6 =
        fnet_routes_ext::rules::new_rule_set::<Ipv6>(&rule_table_v6, RULE_SET_PRIORITY.into())
            .expect("new rule set");

    let main_route_table_v6 =
        connect_to_hermetic_network_realm_protocol::<fnet_routes_admin::RouteTableV6Marker>(&realm)
            .await;
    let fnet_routes_admin::GrantForRouteTableAuthorization { table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<Ipv6>(&main_route_table_v6)
            .await
            .expect("get authorization for route table");
    fnet_routes_ext::rules::authenticate_for_route_table::<Ipv6>(&rule_set_v6, table_id, token)
        .await
        .expect("should not get FIDL error")
        .expect("should successfully authenticate route table");

    const RULE_INDEX: u32 = 123;
    fnet_routes_ext::rules::add_rule::<Ipv6>(
        &rule_set_v6,
        RULE_INDEX.into(),
        fnet_routes_ext::rules::RuleMatcher {
            from: Some(net_declare::net_subnet_v6!("::1:2:3:4/128")),
            locally_generated: None,
            bound_device: None,
            mark_1: None,
            mark_2: None,
        },
        fnet_routes_ext::rules::RuleAction::Lookup(fnet_routes_ext::TableId::new(table_id)),
    )
    .await
    .expect("should not get FIDL error")
    .expect("should successfully add rule");

    let v6_rule = serde_json::json!({
        "action": format!("lookup {table_id}"),
        "bound_device": null,
        "from": "::1:2:3:4/128",
        "index": RULE_INDEX,
        "locally_generated": null,
        "mark_1": null,
        "mark_2": null,
        "rule_set_priority": RULE_SET_PRIORITY,
    });

    let rules = list_rules().await;
    assert_eq!(
        &rules[..],
        &[v4_default_rule.clone(), v6_rule.clone(), v6_default_rule.clone()][..]
    );

    let rule_table_v4 =
        connect_to_hermetic_network_realm_protocol::<fnet_routes_admin::RuleTableV4Marker>(&realm)
            .await;

    let rule_set_v4 =
        fnet_routes_ext::rules::new_rule_set::<Ipv4>(&rule_table_v4, RULE_SET_PRIORITY.into())
            .expect("new rule set");

    let main_route_table_v4 =
        connect_to_hermetic_network_realm_protocol::<fnet_routes_admin::RouteTableV4Marker>(&realm)
            .await;
    let fnet_routes_admin::GrantForRouteTableAuthorization { table_id, token } =
        fnet_routes_ext::admin::get_authorization_for_route_table::<Ipv4>(&main_route_table_v4)
            .await
            .expect("get authorization for route table");
    fnet_routes_ext::rules::authenticate_for_route_table::<Ipv4>(&rule_set_v4, table_id, token)
        .await
        .expect("should not get FIDL error")
        .expect("should successfully authenticate route table");

    fnet_routes_ext::rules::add_rule::<Ipv4>(
        &rule_set_v4,
        RULE_INDEX.into(),
        fnet_routes_ext::rules::RuleMatcher {
            from: Some(net_declare::net_subnet_v4!("1.2.3.4/32")),
            locally_generated: None,
            bound_device: None,
            mark_1: None,
            mark_2: None,
        },
        fnet_routes_ext::rules::RuleAction::Lookup(fnet_routes_ext::TableId::new(table_id)),
    )
    .await
    .expect("should not get FIDL error")
    .expect("should successfully add rule");

    let v4_rule = serde_json::json!({
        "action": format!("lookup {table_id}"),
        "bound_device": null,
        "from": "1.2.3.4/32",
        "index": RULE_INDEX,
        "locally_generated": null,
        "mark_1": null,
        "mark_2": null,
        "rule_set_priority": RULE_SET_PRIORITY,
    });

    let rules = list_rules().await;
    assert_eq!(&rules[..], &[v4_rule, v4_default_rule, v6_rule, v6_default_rule][..]);
}

#[fuchsia::test]
async fn add_remove_blackhole() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = sandbox
        .create_realm(
            "net-cli-test-realm",
            &[KnownServiceProvider::NetworkTestRealm { require_outer_netstack: false }],
        )
        .expect("creating realm should succeed");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(fntr::Netstack::V3)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let connector = NetworkTestRealmConnector { realm: &realm };

    let interfaces_state = connector.connect_to_protocol::<fnet_interfaces::StateMarker>().await;
    let mut event_stream = std::pin::pin!(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::AllInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("should succeed")
    );
    let mut state = fnet_interfaces_ext::existing(
        event_stream.by_ref(),
        HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("should succeed");

    {
        let buffers = writer::TestBuffers::default();
        net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(&["net"], &["if", "add", "blackhole", INTERFACE1_NAME])
                .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
        let (stdout, stderr) = buffers.into_strings();
        assert_eq!(stdout, String::new());
        assert_eq!(stderr, String::new());
    }

    fnet_interfaces_ext::wait_interface(event_stream.by_ref(), &mut state, |state| {
        state.values().find_map(|state| (INTERFACE1_NAME == &state.properties.name).then_some(()))
    })
    .await
    .expect("should successfully wait for blackhole interface to be installed");

    {
        let buffers = writer::TestBuffers::default();
        net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(
                &["net"],
                &["if", "get", &format!("name:{INTERFACE1_NAME}")],
            )
            .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");

        let interface: serde_json::Value =
            serde_json::from_slice(&buffers.into_stdout()[..]).expect("should be valid JSON array");
        assert_eq!(
            interface,
            serde_json::json!({
                "addresses": {
                    "ipv4": [],
                    "ipv6": [],
                },
                "device_class": "Blackhole",
                "has_default_ipv4_route": false,
                "has_default_ipv6_route": false,
                "mac": null,
                "name": INTERFACE1_NAME,
                "nicid": 2,
                "online": false,
                "port_identity_koid": null,
            })
        );
    }

    {
        let buffers = writer::TestBuffers::default();
        net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(
                &["net"],
                &["if", "remove", &format!("name:{INTERFACE1_NAME}")],
            )
            .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await
        .expect("should succeed");
        let (stdout, stderr) = buffers.into_strings();
        assert_eq!(stdout, String::new());
        assert_eq!(stderr, String::new());
    }

    fnet_interfaces_ext::wait_interface(event_stream.by_ref(), &mut state, |state| {
        state
            .values()
            .find_map(|state| (INTERFACE1_NAME == &state.properties.name).then_some(()))
            .is_none()
            .then_some(())
    })
    .await
    .expect("should successfully wait for blackhole interface to be uninstalled");

    {
        let buffers = writer::TestBuffers::default();
        let result = net_cli::do_root(
            writer::JsonWriter::new_test(Some(writer::Format::JsonPretty), &buffers),
            net_cli::Command::from_args(
                &["net"],
                &["if", "get", &format!("name:{INTERFACE1_NAME}")],
            )
            .expect("should parse args successfully"),
            &connector,
            &TestDeps,
        )
        .await;
        let error = result.unwrap_err();
        let UserFacingError { msg } =
            underlying_user_facing_error(&error).expect("should be user-facing error");
        assert_eq!(msg, format!("No interface with name {INTERFACE1_NAME}"));
    }
}
