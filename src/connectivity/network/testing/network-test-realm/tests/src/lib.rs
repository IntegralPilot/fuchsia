// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::borrow::Cow;
use std::collections::HashMap;

use anyhow::Result;
use component_events::events::EventStream;
use derivative::Derivative;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcpv6 as fnet_dhcpv6;
use fidl_fuchsia_net_dhcpv6_ext as fnet_dhcpv6_ext;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_test_realm as fntr;
use fidl_fuchsia_posix_socket as fposix_socket;
use futures::StreamExt as _;
use net_declare::{fidl_ip_v4, fidl_ip_v6, fidl_mac, fidl_socket_addr, fidl_subnet};
use netstack_testing_common::interfaces::TestInterfaceExt as _;
use netstack_testing_common::packets;
use netstack_testing_common::realms::{KnownServiceProvider, Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet::ParsablePacket as _;
use std::convert::TryInto as _;
use std::pin::pin;
use test_case::test_case;

const INTERFACE1_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("02:03:04:05:06:07");
const INTERFACE2_MAC_ADDRESS: fnet::MacAddress = fidl_mac!("06:07:08:09:10:11");
const INTERFACE1_NAME: &'static str = "iface1";
const INTERFACE2_NAME: &'static str = "iface2";
const EXPECTED_INTERFACE_NAME: &'static str = "added-interface";
const IPV4_STUB_URL: &'static str = "#meta/unreliable-echo-v4.cm";
const IPV6_STUB_URL: &'static str = "#meta/unreliable-echo-v6.cm";
const TEST_STUB_MONIKER_REGEX: &'static str = ".*/stubs:test-stub$";

const DEFAULT_IPV4_TARGET_SUBNET: fnet::Subnet = fidl_subnet!("192.168.255.1/16");
const DEFAULT_IPV6_TARGET_SUBNET: fnet::Subnet = fidl_subnet!("3080::2/64");
const DEFAULT_IPV6_LINK_LOCAL_TARGET_SUBNET: fnet::Subnet = fidl_subnet!("fe80::1/64");
const DEFAULT_IPV4_SOURCE_SUBNET: fnet::Subnet = fidl_subnet!("192.168.254.1/16");
const DEFAULT_IPV6_SOURCE_SUBNET: fnet::Subnet = fidl_subnet!("3080::1/64");
const DEFAULT_IPV6_LINK_LOCAL_SOURCE_ADDR: fnet::Ipv6Address = fidl_ip_v6!("fe80::2");
const DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET: fnet::Subnet = fnet::Subnet {
    addr: fnet::IpAddress::Ipv6(DEFAULT_IPV6_LINK_LOCAL_SOURCE_ADDR),
    prefix_len: 64,
};

const DURATION_FIVE_MINUTES: zx::MonotonicDuration = zx::MonotonicDuration::from_minutes(5);
const MINIMUM_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_nanos(1);
const NO_WAIT_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_nanos(0);
const DEFAULT_PAYLOAD_LENGTH: u16 = 100;
const NON_EXISTENT_INTERFACE_NAME: &'static str = "non_existent";

const DEFAULT_IPV4_MULTICAST_ADDRESS: fnet::Ipv4Address = fidl_ip_v4!("224.1.2.3");
const DEFAULT_IPV6_MULTICAST_ADDRESS: fnet::Ipv6Address = fidl_ip_v6!("ff02::3");
const SOLICITED_NODE_MULTICAST_ADDRESS_PREFIX: net_types::ip::Subnet<net_types::ip::Ipv6Addr> = unsafe {
    net_types::ip::Subnet::new_unchecked(
        net_types::ip::Ipv6Addr::new([0xff02, 0, 0, 0, 0, 0x0001, 0xff00, 0]),
        104,
    )
};
const DEFAULT_INTERFACE_ID: u64 = 77;

/// Creates a `netemul::TestRealm` with a Netstack3 instance and the Network
/// Test Realm.
fn create_netstack_realm<'a>(
    name: impl Into<Cow<'a, str>>,
    sandbox: &'a netemul::TestSandbox,
) -> Result<netemul::TestRealm<'a>> {
    // NOTE: To simplify the tests and reduce the number of dependencies, netcfg
    // is intentionally omitted from the `KnownServiceProvider` list below.
    // Instead, it is expected that tests will manually register interfaces with
    // the system's Netstack as needed.
    sandbox.create_netstack_realm_with::<Netstack3, _, _>(
        name,
        &[KnownServiceProvider::NetworkTestRealm { require_outer_netstack: true }],
    )
}

/// Verifies that an interface with `interface_name` exists and has the provided
/// `expected_online_status`.
///
/// Note that this function will not return until the `expected_online_status`
/// is observed.
async fn wait_interface_online_status<'a>(
    interface_name: &'a str,
    expected_online_status: bool,
    state_proxy: &'a fnet_interfaces::StateProxy,
) {
    let id = get_interface_id(interface_name, state_proxy).await.unwrap_or_else(|| {
        panic!("failed to find interface with name {}", interface_name);
    });
    fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            state_proxy,
            Default::default(),
        )
        .expect("watcher creation failed"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(id),
        |properties_and_state| {
            (expected_online_status == properties_and_state.properties.online).then(|| ())
        },
    )
    .await
    .expect("wait for interface failed");
}

/// Verifies that an interface with `interface_name` does not exist.
async fn verify_interface_not_exist<'a>(
    interface_name: &'a str,
    state_proxy: &'a fnet_interfaces::StateProxy,
) {
    assert_eq!(get_interface_id(interface_name, state_proxy).await, None);
}

/// Returns the id for the interface with `interface_name`.
///
/// If the interface is not found then, None is returned.
async fn get_interface_id<'a>(
    interface_name: &'a str,
    state_proxy: &'a fnet_interfaces::StateProxy,
) -> Option<u64> {
    network_test_realm::get_interface_id(interface_name, state_proxy)
        .await
        .expect("failed to obtain interface id")
}

/// Returns the id of the hermetic Netstack interface with `interface_name`.
///
/// Panics if the interface could not be found.
async fn expect_hermetic_interface_id<'a>(
    interface_name: &'a str,
    realm: &netemul::TestRealm<'a>,
) -> u64 {
    let state_proxy =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces::StateMarker>(realm).await;
    get_interface_id(interface_name, &state_proxy).await.unwrap_or_else(|| {
        panic!("failed to find hermetic Netstack interface with name {}", interface_name);
    })
}

/// Connects to a protocol within the hermetic network realm.
async fn connect_to_hermetic_network_realm_protocol<
    P: fidl::endpoints::DiscoverableProtocolMarker,
>(
    realm: &netemul::TestRealm<'_>,
) -> P::Proxy {
    let directory_proxy = open_hermetic_network_realm_exposed_directory(realm).await;
    fuchsia_component::client::connect_to_protocol_at_dir_root::<P>(&directory_proxy)
        .unwrap_or_else(|e| {
            panic!(
                "failed to connect to hermetic network realm protocol {} with error: {:?}",
                P::PROTOCOL_NAME,
                e
            )
        })
}

/// Opens the exposed directory that corresponds to the hermetic network realm.
///
/// An error will be returned if the realm does not exist.
async fn open_hermetic_network_realm_exposed_directory(
    realm: &netemul::TestRealm<'_>,
) -> fio::DirectoryProxy {
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
    directory_proxy
}

/// Returns true if the hermetic network realm exists.
async fn has_hermetic_network_realm(realm: &netemul::TestRealm<'_>) -> bool {
    let realm_proxy = realm
        .connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("failed to connect to realm protocol");
    network_test_realm::has_hermetic_network_realm(&realm_proxy)
        .await
        .expect("failed to check for hermetic network realm")
}

async fn has_stub(realm: &netemul::TestRealm<'_>) -> bool {
    let realm_proxy =
        connect_to_hermetic_network_realm_protocol::<fcomponent::RealmMarker>(realm).await;
    network_test_realm::has_stub(&realm_proxy).await.expect("failed to check for stub")
}

/// Adds an enabled interface to the realm's Netstack with the provided
/// `mac_address` and `name`.
///
/// Note that the Network Test Realm will only consider the interface to be
/// valid if it is also present in devfs.
async fn add_interface_to_netstack<'a>(
    mac_address: fnet::MacAddress,
    name: &'a str,
    sandbox: &'a netemul::TestSandbox,
    realm: &'a netemul::TestRealm<'a>,
) -> netemul::TestInterface<'a> {
    let endpoint = sandbox
        .create_endpoint_with(
            name,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(mac_address)),
        )
        .await
        .expect("failed to create endpoint");
    realm
        .add_virtual_device(&endpoint, netemul::devfs_device_path(name).as_path())
        .await
        .expect("failed to add virtual device");
    // Note that calling `install_endpoint` also enables the interface.
    realm
        .install_endpoint(
            endpoint,
            netemul::InterfaceConfig { name: Some(name.into()), ..Default::default() },
        )
        .await
        .expect("failed to install endpoint")
}

/// Adds the address from the specified `subnet` to the hermetic Netstack
/// interface that has the provided `interface_name`.
///
/// A forwarding entry is also added for the relevant interface and the provided
/// `subnet`.
async fn add_address_to_hermetic_interface(
    interface_name: &str,
    subnet: fnet::Subnet,
    realm: &netemul::TestRealm<'_>,
) {
    let state_proxy =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces::StateMarker>(realm).await;
    let id = get_interface_id(interface_name, &state_proxy).await.unwrap_or_else(|| {
        panic!("failed to find interface with name {}", interface_name);
    });
    let interfaces_proxy =
        connect_to_hermetic_network_realm_protocol::<fnet_root::InterfacesMarker>(realm).await;
    let (control, server_end) =
        fnet_interfaces_ext::admin::Control::create_endpoints().expect("create_endpoints failed");
    interfaces_proxy.get_admin(id, server_end).expect("get_admin failed");

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
}

/// Adds an interface to the hermetic Netstack with `interface_name` and
/// `mac_address`.
///
/// The added interface is assigned a static IP address based on `subnet`.
/// Additionally, the interface joins the provided `network`.
async fn join_network_with_hermetic_netstack<'a>(
    realm: &'a netemul::TestRealm<'a>,
    network: &'a netemul::TestNetwork<'a>,
    network_test_realm: &'a fntr::ControllerProxy,
    interface_name: &'a str,
    mac_address: fnet::MacAddress,
    subnet: fnet::Subnet,
) -> netemul::TestInterface<'a> {
    let interface = realm
        .join_network_with(
            &network,
            interface_name,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(mac_address)),
            Default::default(),
        )
        .await
        .expect("join_network failed");
    realm
        .add_virtual_device(
            interface.endpoint(),
            netemul::devfs_device_path(interface_name).as_path(),
        )
        .await
        .expect("failed to add the device");

    network_test_realm
        .add_interface(&mac_address, interface_name, /* wait_any_ip_address= */ false)
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    add_address_to_hermetic_interface(interface_name, subnet, realm).await;
    interface
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_hermetic_network_realm(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    assert!(has_hermetic_network_realm(&realm).await);
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_hermetic_network_realm_replaces_existing_realm(
    name: &str,
    sub_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE1_MAC_ADDRESS, INTERFACE1_NAME, &sandbox, &realm).await;

    network_test_realm
        .add_interface(
            &INTERFACE1_MAC_ADDRESS,
            EXPECTED_INTERFACE_NAME,
            /* wait_any_ip_address= */ false,
        )
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let hermetic_network_state_proxy =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces::StateMarker>(&realm).await;

    // The Netstack in the replaced hermetic network realm should not have the
    // previously attached interface.
    verify_interface_not_exist(EXPECTED_INTERFACE_NAME, &hermetic_network_state_proxy).await;

    assert!(has_hermetic_network_realm(&realm).await);
}

#[netstack_test]
#[test_case("no_wait_any_ip_address_netstack2", false, fntr::Netstack::V2)]
#[test_case("wait_any_ip_address_netstack2", true, fntr::Netstack::V2)]
#[test_case("no_wait_any_ip_address_netstack3", false, fntr::Netstack::V3)]
#[test_case("wait_any_ip_address_netstack3", true, fntr::Netstack::V3)]
async fn add_interface(
    name: &str,
    sub_name: &str,
    wait_any_ip_address: bool,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE1_MAC_ADDRESS, INTERFACE1_NAME, &sandbox, &realm).await;

    let _interface: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE2_MAC_ADDRESS, INTERFACE2_NAME, &sandbox, &realm).await;

    let hermetic_network_state_proxy =
        connect_to_hermetic_network_realm_protocol::<fnet_interfaces::StateMarker>(&realm).await;

    network_test_realm
        .add_interface(&INTERFACE1_MAC_ADDRESS, EXPECTED_INTERFACE_NAME, wait_any_ip_address)
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    if wait_any_ip_address {
        let ifaces_state = fnet_interfaces_ext::existing(
            fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
                &hermetic_network_state_proxy,
                Default::default(),
            )
            .expect("failed getting interfaces event stream"),
            HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new(),
        )
        .await
        .expect("failed getting existing interfaces state");

        let iface = ifaces_state
            .values()
            .find(|iface| iface.properties.name == EXPECTED_INTERFACE_NAME)
            .unwrap_or_else(|| panic!("no interface with name {}", EXPECTED_INTERFACE_NAME));

        assert!(
            !iface.properties.addresses.is_empty(),
            "interface {:?} has no IP addresses; expected to have waited for autoconfigured IP \
             address",
            iface
        );
    }

    // An interface with a name of `EXPECTED_INTERFACE_NAME` should be enabled and
    // present in the hermetic Netstack.
    wait_interface_online_status(
        EXPECTED_INTERFACE_NAME,
        true, /* expected_online_status */
        &hermetic_network_state_proxy,
    )
    .await;
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn add_interface_already_exists(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface1: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE1_MAC_ADDRESS, INTERFACE1_NAME, &sandbox, &realm).await;

    let _interface2: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE2_MAC_ADDRESS, INTERFACE2_NAME, &sandbox, &realm).await;

    network_test_realm
        .add_interface(
            &INTERFACE1_MAC_ADDRESS,
            INTERFACE1_NAME,
            /* wait_any_ip_address= */ false,
        )
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    assert_eq!(
        network_test_realm
            .add_interface(
                &INTERFACE2_MAC_ADDRESS,
                INTERFACE1_NAME,
                /* wait_any_ip_address= */ false
            )
            .await
            .expect("add_interface failed"),
        Err(fntr::Error::AlreadyExists)
    );
}

// Tests the case where the MAC address provided to `Controller.AddInterface`
// does not match any of the interfaces on the system.
#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn add_interface_with_no_matching_interface(
    name: &str,
    sub_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE1_MAC_ADDRESS, INTERFACE1_NAME, &sandbox, &realm).await;

    // `non_matching_mac_address` doesn't match any of the MAC addresses for
    // interfaces owned by the system's Netstack.
    let non_matching_mac_address = fidl_mac!("aa:bb:cc:dd:ee:ff");
    assert_eq!(
        network_test_realm
            .add_interface(
                &non_matching_mac_address,
                EXPECTED_INTERFACE_NAME,
                /* wait_any_ip_address= */ false
            )
            .await
            .expect("failed to add interface to hermetic netstack"),
        Err(fntr::Error::InterfaceNotFound)
    );
}

// Tests the case where the MAC address provided to `Controller.AddInterface`
// matches an interface in devfs, but not in the system Netstack.
#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn add_interface_with_no_matching_interface_in_netstack(
    name: &str,
    sub_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    // The Network Test Realm requires that the matching interface be present in
    // both the system's Netstack and devfs. In this case, it is only present in
    // devfs.
    assert_eq!(
        network_test_realm
            .add_interface(
                &INTERFACE1_MAC_ADDRESS,
                EXPECTED_INTERFACE_NAME,
                /* wait_any_ip_address= */ false
            )
            .await
            .expect("failed to add interface to hermetic netstack"),
        Err(fntr::Error::InterfaceNotFound)
    );
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn stop_hermetic_network_realm(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface: netemul::TestInterface<'_> =
        add_interface_to_netstack(INTERFACE1_MAC_ADDRESS, INTERFACE1_NAME, &sandbox, &realm).await;

    network_test_realm
        .add_interface(
            &INTERFACE1_MAC_ADDRESS,
            EXPECTED_INTERFACE_NAME,
            /* wait_any_ip_address= */ false,
        )
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    network_test_realm
        .stop_hermetic_network_realm()
        .await
        .expect("stop_hermetic_network_realm failed")
        .expect("stop_hermetic_network_realm error");

    assert!(!has_hermetic_network_realm(&realm).await);
}

#[fuchsia::test]
async fn stop_hermetic_network_realm_with_no_existing_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        create_netstack_realm("stop_hermetic_network_realm_with_no_existing_realm", &sandbox)
            .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    assert_eq!(
        network_test_realm
            .stop_hermetic_network_realm()
            .await
            .expect("failed to stop hermetic network realm"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_stub(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    network_test_realm
        .start_stub(IPV4_STUB_URL)
        .await
        .expect("start_stub failed")
        .expect("start_stub error");

    assert!(has_stub(&realm).await);
}

#[netstack_test]
#[test_case("ipv4_netstack2", fposix_socket::Domain::Ipv4, fntr::Netstack::V2)]
#[test_case("ipv6_netstack2", fposix_socket::Domain::Ipv6, fntr::Netstack::V2)]
#[test_case("ipv4_netstack3", fposix_socket::Domain::Ipv4, fntr::Netstack::V3)]
#[test_case("ipv6_netstack3", fposix_socket::Domain::Ipv6, fntr::Netstack::V3)]
async fn poll_udp(
    name: &str,
    sub_name: &str,
    domain: fposix_socket::Domain,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    network_test_realm
        .start_stub(match domain {
            fposix_socket::Domain::Ipv4 => IPV4_STUB_URL,
            fposix_socket::Domain::Ipv6 => IPV6_STUB_URL,
        })
        .await
        .expect("start_stub failed")
        .expect("start_stub error");

    assert!(has_stub(&realm).await, "expected has_stub(&realm) to be true");

    let payload = "hello".as_bytes();
    let poll_addr = fnet_ext::SocketAddress(match domain {
        fposix_socket::Domain::Ipv4 => unreliable_echo::socket_addr_v4(),
        fposix_socket::Domain::Ipv6 => unreliable_echo::socket_addr_v6(),
    })
    .into();
    let response = network_test_realm
        .poll_udp(&poll_addr, payload, zx::MonotonicDuration::from_millis(10).into_nanos(), 6000)
        .await
        .expect("poll_udp FIDL error")
        .expect("poll_udp error");
    assert_eq!(response, payload);

    network_test_realm.stop_stub().await.expect("stop_stub failed").expect("stop_stub error");

    assert!(!has_stub(&realm).await, "expected has_stub(&realm) to be false");

    let response = network_test_realm
        .poll_udp(&poll_addr, payload, zx::MonotonicDuration::from_millis(10).into_nanos(), 100)
        .await
        .expect("poll_udp FIDL error");
    assert_eq!(response, Err(fntr::Error::TimeoutExceeded));
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn poll_udp_unreachable(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    // 203.0.113.0/24 is reserved for documentation only, so no address in this subnet
    // should be routable/reachable. See https://www.rfc-editor.org/rfc/rfc5737.html.
    let poll_addr = fidl_socket_addr!("203.0.113.1:10000");

    let response = network_test_realm
        .poll_udp(
            &poll_addr,
            "test payload".as_bytes(),
            zx::MonotonicDuration::from_millis(10).into_nanos(),
            100,
        )
        .await
        .expect("poll_udp FIDL error");
    assert_eq!(response, Err(fntr::Error::AddressUnreachable));
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_stub_with_existing_stub(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let mut event_stream = EventStream::open_at_path("/events/started_stopped")
        .await
        .expect("failed to subscribe to EventStream");

    network_test_realm
        .start_stub(IPV4_STUB_URL)
        .await
        .expect("start_stub failed")
        .expect("start_stub error");

    let event_matcher =
        component_events::matcher::EventMatcher::ok().moniker_regex(TEST_STUB_MONIKER_REGEX);

    let component_events::events::StartedPayload {} = event_matcher
        .clone()
        .wait::<component_events::events::Started>(&mut event_stream)
        .await
        .expect("initial test-stub observe start event failed")
        .result()
        .expect("initial test-stub observe start event error");

    network_test_realm
        .start_stub(IPV4_STUB_URL)
        .await
        .expect("start_stub replace failed")
        .expect("start_stub replace error");

    // Verify that the previously running stub was replaced. That is, check that
    // the stub was stopped and then started.
    let stopped_event = event_matcher
        .clone()
        .wait::<component_events::events::Stopped>(&mut event_stream)
        .await
        .expect("test-stub observe stop event failed");

    // Note that stopped_event.result below borrows from `stopped_event`. As a
    // result it needs to be in a different statement.
    let component_events::events::StoppedPayload { status, .. } =
        stopped_event.result().expect("test-stub observe stop event error");
    let raw_status = match *status {
        component_events::events::ExitStatus::Clean => {
            // The component is expected to have a nonzero exit status due to it having been killed
            // rather than being allowed to exit normally.
            panic!("got ExitStatus::Clean, expected Crash")
        }
        component_events::events::ExitStatus::Crash(i) => i,
    };
    let component_error =
        match raw_status.try_into().ok().and_then(fcomponent::Error::from_primitive) {
            None => panic!(
                "expected {:?} to match a fuchsia.component.Error value",
                zx::Status::err_from_raw(raw_status)
            ),
            Some(e) => e,
        };
    assert_eq!(component_error, fcomponent::Error::InstanceDied);

    let component_events::events::StartedPayload {} = event_matcher
        .clone()
        .wait::<component_events::events::Started>(&mut event_stream)
        .await
        .expect("replacement test-stub observe start event failed")
        .result()
        .expect("replacement test-stub observe start event error");

    assert!(has_stub(&realm).await);
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_stub_with_non_existent_component(
    name: &str,
    sub_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    assert_eq!(
        network_test_realm
            .start_stub("#meta/non-existent-stub.cm")
            .await
            .expect("failed to call start_stub"),
        Err(fntr::Error::ComponentNotFound),
    );
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn start_stub_with_malformed_component_url(
    name: &str,
    sub_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    assert_eq!(
        network_test_realm
            .start_stub("malformed-component-url")
            .await
            .expect("failed to call start_stub"),
        Err(fntr::Error::InvalidArguments),
    );
}

#[fuchsia::test]
async fn start_stub_with_no_hermetic_network_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm("start_stub_with_no_hermetic_network_realm", &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    assert_eq!(
        network_test_realm.start_stub(IPV4_STUB_URL).await.expect("failed to call start_stub"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn stop_stub(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    network_test_realm
        .start_stub(IPV4_STUB_URL)
        .await
        .expect("start_stub failed")
        .expect("start_stub error");

    network_test_realm.stop_stub().await.expect("stop_stub failed").expect("stop_stub error");

    assert!(!has_stub(&realm).await);
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn stop_stub_with_no_running_stub(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    assert_eq!(
        network_test_realm.stop_stub().await.expect("failed to call stop_stub"),
        Err(fntr::Error::StubNotRunning),
    );
}

#[fuchsia::test]
async fn stop_stub_with_no_hermetic_network_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm("stop_stub_with_no_hermetic_network_realm", &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    assert_eq!(
        network_test_realm.stop_stub().await.expect("failed to call stop_stub"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

/// Defaultable configuration options for ping tests.
#[derive(Debug, Derivative)]
#[derivative(Default)]
struct PingOptions {
    interface_name: Option<String>,
    #[derivative(Default(value = "DEFAULT_PAYLOAD_LENGTH"))]
    payload_length: u16,
    #[derivative(Default(value = "DURATION_FIVE_MINUTES"))]
    timeout: zx::MonotonicDuration,
    disable_target_interface: bool,
}

/// Address configuration for ping tests.
struct PingAddressConfig {
    source_subnet: fnet::Subnet,
    target_subnet: fnet::Subnet,
}

const IPV4_ADDRESS_CONFIG: PingAddressConfig = PingAddressConfig {
    source_subnet: DEFAULT_IPV4_SOURCE_SUBNET,
    target_subnet: DEFAULT_IPV4_TARGET_SUBNET,
};
const IPV6_ADDRESS_CONFIG: PingAddressConfig = PingAddressConfig {
    source_subnet: DEFAULT_IPV6_SOURCE_SUBNET,
    target_subnet: DEFAULT_IPV6_TARGET_SUBNET,
};
const IPV6_LINK_LOCAL_ADDRESS_CONFIG: PingAddressConfig = PingAddressConfig {
    source_subnet: DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    target_subnet: DEFAULT_IPV6_LINK_LOCAL_TARGET_SUBNET,
};

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions::default(),
    fntr::Netstack::V2,
    Ok(());
    "ipv4 netstack2")]
#[test_case(
    "ipv4_bind_to_existing_interface_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions {
        interface_name:  Some(INTERFACE1_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Ok(());
    "ipv4 bind to existing interface netstack2")]
#[test_case(
    "ipv4_bind_to_non_existent_interface_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions {
        interface_name: Some(NON_EXISTENT_INTERFACE_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Err(fntr::Error::InterfaceNotFound);
    "ipv4 bind to non existent interface netstack2")]
#[test_case(
    "ipv6_netstack2",
    IPV6_ADDRESS_CONFIG,
    PingOptions::default(),
    fntr::Netstack::V2,
    Ok(());
    "ipv6 netstack2")]
#[test_case(
    "ipv6_bind_to_existing_interface_netstack2",
    IPV6_ADDRESS_CONFIG,
    PingOptions {
        interface_name:  Some(INTERFACE1_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Ok(());
    "ipv6 bind to existing interface netstack2")]
#[test_case(
    "ipv6_link_local_bind_to_existing_interface_netstack2",
    IPV6_LINK_LOCAL_ADDRESS_CONFIG,
    PingOptions {
        interface_name:  Some(INTERFACE1_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Ok(());
    "ipv6 link local bind to existing interface netstack2")]
#[test_case(
    "ipv6_link_local_with_no_interface_specified_netstack2",
    IPV6_LINK_LOCAL_ADDRESS_CONFIG,
    PingOptions::default(),
    fntr::Netstack::V2,
    Err(fntr::Error::InvalidArguments);
    "ipv6 link local with no interface specified netstack2")]
#[test_case(
    "ipv6_bind_to_non_existent_interface_netstack2",
    IPV6_ADDRESS_CONFIG,
    PingOptions {
        interface_name: Some(NON_EXISTENT_INTERFACE_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Err(fntr::Error::InterfaceNotFound);
    "ipv6 bind to non existent interface netstack2")]
#[test_case(
    "timeout_exceeded_netstack2",
    IPV4_ADDRESS_CONFIG,
    // Attempting to ping a target interface that is disabled forces a timeout.
    PingOptions {
        disable_target_interface: true,
        timeout: MINIMUM_TIMEOUT, ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Err(fntr::Error::TimeoutExceeded);
    "timeout exceeded netstack2")]
#[test_case(
    "no_timeout_with_disabled_target_interface_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions {
        disable_target_interface: true,
        timeout: NO_WAIT_TIMEOUT,
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    // Since no timeout is defined, this ping should succeed.
    Ok(());
    "no timeout with disabled target interface netstack2")]
#[test_case(
    "no_timeout_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions { timeout: NO_WAIT_TIMEOUT, ..PingOptions::default() },
    fntr::Netstack::V2,
    Ok(());
    "no timeout netstack2")]
#[test_case(
    "host_unreachable_netstack2",
    PingAddressConfig {
        target_subnet: fidl_subnet!("192.167.1.1/16"),
        ..IPV4_ADDRESS_CONFIG
    },
    PingOptions {
        interface_name:  Some(INTERFACE1_NAME.to_string()),
        ..PingOptions::default()
    },
    fntr::Netstack::V2,
    Err(fntr::Error::PingFailed);
    "host unreachable netstack2")]
#[test_case(
    "oversized_payload_length_netstack2",
    IPV4_ADDRESS_CONFIG,
    PingOptions { payload_length: u16::MAX, ..PingOptions::default() },
    fntr::Netstack::V2,
    Err(fntr::Error::PingFailed);
    "oversized payload length netstack2")]
#[test_case(
        "ipv4_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions::default(),
        fntr::Netstack::V3,
        Ok(());
        "ipv4 netstack3")]
#[test_case(
        "ipv4_bind_to_existing_interface_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions {
            interface_name:  Some(INTERFACE1_NAME.to_string()),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Ok(());
        "ipv4 bind to existing interface netstack3")]
#[test_case(
        "ipv4_bind_to_non_existent_interface_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions {
            interface_name: Some(NON_EXISTENT_INTERFACE_NAME.to_string()),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Err(fntr::Error::InterfaceNotFound);
        "ipv4 bind to non existent interface netstack3")]
#[test_case(
        "ipv6_netstack3",
        IPV6_ADDRESS_CONFIG,
        PingOptions::default(),
        fntr::Netstack::V3,
        Ok(());
        "ipv6 netstack3")]
#[test_case(
        "ipv6_bind_to_existing_interface_netstack3",
        IPV6_ADDRESS_CONFIG,
        PingOptions {
            interface_name:  Some(INTERFACE1_NAME.to_string()),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Ok(());
        "ipv6 bind to existing interface netstack3")]
#[test_case(
        "ipv6_link_local_bind_to_existing_interface_netstack3",
        IPV6_LINK_LOCAL_ADDRESS_CONFIG,
        PingOptions {
            interface_name:  Some(INTERFACE1_NAME.to_string()),
            // TODO(https://fxbug.dev/42083514): Fix and use the default timeout.
            timeout: zx::MonotonicDuration::from_seconds(5),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Ok(());
        "ipv6 link local bind to existing interface netstack3")]
#[test_case(
        "ipv6_link_local_with_no_interface_specified_netstack3",
        IPV6_LINK_LOCAL_ADDRESS_CONFIG,
        PingOptions::default(),
        fntr::Netstack::V3,
        Err(fntr::Error::InvalidArguments);
        "ipv6 link local with no interface specified netstack3")]
#[test_case(
        "ipv6_bind_to_non_existent_interface_netstack3",
        IPV6_ADDRESS_CONFIG,
        PingOptions {
            interface_name: Some(NON_EXISTENT_INTERFACE_NAME.to_string()),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Err(fntr::Error::InterfaceNotFound);
        "ipv6 bind to non existent interface netstack3")]
#[test_case(
        "timeout_exceeded_netstack3",
        IPV4_ADDRESS_CONFIG,
        // Attempting to ping a target interface that is disabled forces a timeout.
        PingOptions {
            disable_target_interface: true,
            timeout: MINIMUM_TIMEOUT, ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Err(fntr::Error::TimeoutExceeded);
        "timeout exceeded netstack3")]
#[test_case(
        "no_timeout_with_disabled_target_interface_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions {
            disable_target_interface: true,
            timeout: NO_WAIT_TIMEOUT,
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        // Since no timeout is defined, this ping should succeed.
        Ok(());
        "no timeout with disabled target interface netstack3")]
#[test_case(
        "no_timeout_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions { timeout: NO_WAIT_TIMEOUT, ..PingOptions::default() },
        fntr::Netstack::V3,
        Ok(());
        "no timeout netstack3")]
#[test_case(
        "host_unreachable_netstack3",
        PingAddressConfig {
            target_subnet: fidl_subnet!("192.167.1.1/16"),
            ..IPV4_ADDRESS_CONFIG
        },
        PingOptions {
            interface_name:  Some(INTERFACE1_NAME.to_string()),
            ..PingOptions::default()
        },
        fntr::Netstack::V3,
        Err(fntr::Error::PingFailed);
        "host unreachable netstack3")]
#[test_case(
        "oversized_payload_length_netstack3",
        IPV4_ADDRESS_CONFIG,
        PingOptions { payload_length: u16::MAX, ..PingOptions::default() },
        fntr::Netstack::V3,
        Err(fntr::Error::PingFailed);
        "oversized payload length netstack3")]
async fn ping(
    name: &str,
    case_name: &str,
    address_config: PingAddressConfig,
    options: PingOptions,
    netstack: fntr::Netstack,
    expected_result: Result<(), fntr::Error>,
) {
    let PingAddressConfig { source_subnet, target_subnet } = address_config;
    let PingOptions { interface_name, payload_length, timeout, disable_target_interface } = options;
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    // Create a realm that contains a system Netstack and the Network Test
    // Realm.
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");
    let network = sandbox.create_network("network").await.expect("failed to create network");

    // Create another Netstack realm that will be pinged by the hermetic
    // Netstack.
    let target_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{}_{}_target", name, case_name))
        .expect("failed to create target netstack realm");

    let target_ep = target_realm
        .join_network_with_if_config(
            &network,
            INTERFACE2_NAME,
            netemul::InterfaceConfig { name: Some(INTERFACE2_NAME.into()), ..Default::default() },
        )
        .await
        .expect("join_network failed for target_realm");
    target_ep.add_address_and_subnet_route(target_subnet).await.expect("configure address");
    target_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");

    if disable_target_interface {
        // Disable the target interface and wait for it to achieve the disabled
        // state.
        let did_disable =
            target_ep.control().disable().await.expect("send disable").expect("disable interface");
        assert!(did_disable);
        let state_proxy = target_realm
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .expect("failed to connect to state");
        wait_interface_online_status(
            INTERFACE2_NAME,
            false, /* expected_online_status */
            &state_proxy,
        )
        .await;
    }

    let system_ep = realm
        .join_network_with(
            &network,
            INTERFACE1_NAME,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(INTERFACE1_MAC_ADDRESS)),
            Default::default(),
        )
        .await
        .expect("join_network failed for base realm");
    system_ep.apply_nud_flake_workaround().await.expect("nud flake workaround");
    realm
        .add_virtual_device(
            system_ep.endpoint(),
            netemul::devfs_device_path(INTERFACE1_NAME).as_path(),
        )
        .await
        .expect("failed to add virtual device");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    network_test_realm
        .add_interface(
            &INTERFACE1_MAC_ADDRESS,
            INTERFACE1_NAME,
            /* wait_any_ip_address= */ false,
        )
        .await
        .expect("add_interface failed")
        .expect("add_interface error");

    add_address_to_hermetic_interface(INTERFACE1_NAME, source_subnet, &realm).await;

    assert_eq!(
        network_test_realm
            .ping(
                &target_subnet.addr,
                payload_length,
                interface_name.as_deref(),
                timeout.into_nanos(),
            )
            .await
            .expect("ping failed"),
        expected_result
    );
}

#[fuchsia::test]
async fn ping_with_no_hermetic_network_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm("ping_with_no_hermetic_network_realm", &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    let target_ip = DEFAULT_IPV4_TARGET_SUBNET.addr;
    assert_eq!(
        network_test_realm
            .ping(&target_ip, DEFAULT_PAYLOAD_LENGTH, None, NO_WAIT_TIMEOUT.into_nanos())
            .await
            .expect("ping failed"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

#[netstack_test]
#[test_case("netstack2", fntr::Netstack::V2)]
#[test_case("netstack3", fntr::Netstack::V3)]
async fn ping_with_no_added_interface(name: &str, sub_name: &str, netstack: fntr::Netstack) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let target_ip = DEFAULT_IPV4_TARGET_SUBNET.addr;
    assert_eq!(
        network_test_realm
            .ping(&target_ip, DEFAULT_PAYLOAD_LENGTH, None, NO_WAIT_TIMEOUT.into_nanos())
            .await
            .expect("ping failed"),
        Err(fntr::Error::PingFailed),
    );
}

#[derive(Debug, PartialEq)]
enum MulticastEvent {
    Joined(fnet::IpAddress),
    Left(fnet::IpAddress),
}

/// Extracts Ipv4 `MulticastEvent`s from the provided `data`.
fn extract_v4_multicast_event(data: &[u8]) -> Vec<MulticastEvent> {
    let (mut payload, _src_ip, _dst_ip, proto, _ttl) =
        packet_formats::testutil::parse_ip_packet::<net_types::ip::Ipv4>(&data)
            .expect("error parsing IPv4 packet");

    if proto != packet_formats::ip::Ipv4Proto::Igmp {
        // Ignore non-IGMP packets.
        return Vec::new();
    }

    let igmp_packet = packet_formats::igmp::messages::IgmpPacket::parse(&mut payload, ())
        .expect("failed to parse IGMP packet");

    match igmp_packet {
        packet_formats::igmp::messages::IgmpPacket::MembershipReportV2(message) => {
            vec![MulticastEvent::Joined(fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                addr: message.group_addr().ipv4_bytes(),
            }))]
        }
        packet_formats::igmp::messages::IgmpPacket::LeaveGroup(message) => {
            vec![MulticastEvent::Left(fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                addr: message.group_addr().ipv4_bytes(),
            }))]
        }
        packet_formats::igmp::messages::IgmpPacket::MembershipReportV3(message) => message
            .body()
            .iter()
            .filter_map(|record| {
                use packet_formats::igmp::messages::IgmpGroupRecordType;

                assert_eq!(record.sources(), &[], "Do not expect source filtering");

                let hdr = record.header();
                let action = match hdr.record_type().expect("record type") {
                    IgmpGroupRecordType::ModeIsExclude
                    | IgmpGroupRecordType::ChangeToExcludeMode => MulticastEvent::Joined,
                    IgmpGroupRecordType::ModeIsInclude
                    | IgmpGroupRecordType::ChangeToIncludeMode => MulticastEvent::Left,
                    IgmpGroupRecordType::AllowNewSources | IgmpGroupRecordType::BlockOldSources => {
                        return None;
                    }
                };

                Some(action(fnet::IpAddress::Ipv4(fnet::Ipv4Address {
                    addr: hdr.multicast_addr().ipv4_bytes(),
                })))
            })
            .collect(),
        packet_formats::igmp::messages::IgmpPacket::MembershipReportV1(_)
        | packet_formats::igmp::messages::IgmpPacket::MembershipQueryV2(_)
        | packet_formats::igmp::messages::IgmpPacket::MembershipQueryV3(_) => {
            panic!("unexpected IgmpPacket format: {:?}", igmp_packet)
        }
    }
}

/// Extracts Ipv6 `MulticastEvent`s from the provided `data`.
fn extract_v6_multicast_event(data: &[u8]) -> Vec<MulticastEvent> {
    let (mut payload, src_ip, dst_ip, proto, _ttl) =
        packet_formats::testutil::parse_ip_packet::<net_types::ip::Ipv6>(&data)
            .expect("error parsing IPv6 packet");

    if proto != packet_formats::ip::Ipv6Proto::Icmpv6 {
        // Ignore non-ICMPv6 packets.
        return Vec::new();
    }

    let icmp_packet = packet_formats::icmp::Icmpv6Packet::parse(
        &mut payload,
        packet_formats::icmp::IcmpParseArgs::new(src_ip, dst_ip),
    )
    .expect("error parsing ICMPv6 packet");

    let mld_packet = match icmp_packet {
        packet_formats::icmp::Icmpv6Packet::Mld(mld) => mld,
        packet_formats::icmp::Icmpv6Packet::DestUnreachable(_)
        | packet_formats::icmp::Icmpv6Packet::EchoReply(_)
        | packet_formats::icmp::Icmpv6Packet::EchoRequest(_)
        | packet_formats::icmp::Icmpv6Packet::Ndp(_)
        | packet_formats::icmp::Icmpv6Packet::PacketTooBig(_)
        | packet_formats::icmp::Icmpv6Packet::ParameterProblem(_)
        | packet_formats::icmp::Icmpv6Packet::TimeExceeded(_) => return Vec::new(),
    };

    match mld_packet {
        packet_formats::icmp::mld::MldPacket::MulticastListenerReport(packet) => {
            (!SOLICITED_NODE_MULTICAST_ADDRESS_PREFIX.contains(&packet.body().group_addr))
                .then(|| {
                    MulticastEvent::Joined(fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                        addr: packet.body().group_addr.ipv6_bytes(),
                    }))
                })
                .into_iter()
                .collect()
        }
        packet_formats::icmp::mld::MldPacket::MulticastListenerDone(packet) => {
            (!SOLICITED_NODE_MULTICAST_ADDRESS_PREFIX.contains(&packet.body().group_addr))
                .then(|| {
                    MulticastEvent::Left(fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                        addr: packet.body().group_addr.ipv6_bytes(),
                    }))
                })
                .into_iter()
                .collect()
        }
        packet_formats::icmp::mld::MldPacket::MulticastListenerQuery(_) => Vec::new(),
        packet_formats::icmp::mld::MldPacket::MulticastListenerQueryV2(_) => Vec::new(),
        packet_formats::icmp::mld::MldPacket::MulticastListenerReportV2(packet) => packet
            .body()
            .iter_multicast_records()
            .filter_map(|record| {
                use packet_formats::icmp::mld::Mldv2MulticastRecordType;

                assert_eq!(record.sources(), &[], "Do not expect source filtering");

                let hdr = record.header();
                let action = match hdr.record_type().expect("record type") {
                    Mldv2MulticastRecordType::ModeIsExclude
                    | Mldv2MulticastRecordType::ChangeToExcludeMode => MulticastEvent::Joined,
                    Mldv2MulticastRecordType::ModeIsInclude
                    | Mldv2MulticastRecordType::ChangeToIncludeMode => MulticastEvent::Left,
                    Mldv2MulticastRecordType::AllowNewSources
                    | Mldv2MulticastRecordType::BlockOldSources => return None,
                };

                Some(action(fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                    addr: hdr.multicast_addr().ipv6_bytes(),
                })))
            })
            .collect(),
    }
}

/// Verifies that the `expected_event` occurred on the `fake_endpoint`.
async fn expect_multicast_event(
    fake_endpoint: &netemul::TestFakeEndpoint<'_>,
    expected_event: MulticastEvent,
) {
    let expected_event = &expected_event;
    let stream = fake_endpoint
        .frame_stream()
        .map(|r| r.expect("error getting OnData event"))
        .filter_map(|(data, _dropped)| async move {
            let mut data = &data[..];
            let eth = packet_formats::ethernet::EthernetFrame::parse(
                &mut data,
                // Do not check the frame length as the size of IGMP reports may
                // be less than the minimum ethernet frame length and our
                // virtual (netemul) interface does not pad runt ethernet frames
                // before transmission.
                packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
            )
            .expect("failed to parse ethernet frame");

            let events = match eth.ethertype().expect("ethertype missing from ethernet frame") {
                packet_formats::ethernet::EtherType::Ipv4 => extract_v4_multicast_event(data),
                packet_formats::ethernet::EtherType::Ipv6 => extract_v6_multicast_event(data),
                packet_formats::ethernet::EtherType::Arp
                | packet_formats::ethernet::EtherType::Other(_) => return None,
            };

            // The same event may be emitted multiple times. As a result, we
            // must wait for the expected event.
            events.contains(expected_event).then(|| ())
        });
    let mut stream = pin!(stream);
    stream.next().await.expect("failed to find expected multicast event");
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    fntr::Netstack::V2,
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    DEFAULT_IPV4_SOURCE_SUBNET;
    "ipv4 netstack2")]
#[test_case(
    "ipv4_netstack3",
    fntr::Netstack::V3,
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    DEFAULT_IPV4_SOURCE_SUBNET;
    "ipv4 netstack3")]
#[test_case(
    "ipv6_netstack2",
    fntr::Netstack::V2,
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET;
    "ipv6 netstack2")]
#[test_case(
    "ipv6_netstack3",
    fntr::Netstack::V3,
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET;
    "ipv6 netstack3")]
async fn join_multicast_group(
    name: &str,
    case_name: &str,
    netstack: fntr::Netstack,
    multicast_address: fnet::IpAddress,
    subnet: fnet::Subnet,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    network_test_realm
        .join_multicast_group(
            &multicast_address,
            expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await,
        )
        .await
        .expect("join_multicast_group failed")
        .expect("join_multicast_group error");

    expect_multicast_event(&fake_ep, MulticastEvent::Joined(multicast_address)).await;
}

// Tests that the persisted multicast socket is cleared when the hermetic
// network realm is stopped.
#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    fnet::IpAddress::Ipv4(fidl_ip_v4!("224.1.2.4")),
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    fnet::IpAddress::Ipv6(fidl_ip_v6!("ff02::4")),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
        fnet::IpAddress::Ipv4(fidl_ip_v4!("224.1.2.4")),
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
        fnet::IpAddress::Ipv6(fidl_ip_v6!("ff02::4")),
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn join_multicast_group_after_stop(
    name: &str,
    case_name: &str,
    multicast_address: fnet::IpAddress,
    second_multicast_address: fnet::IpAddress,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    network_test_realm
        .join_multicast_group(
            &multicast_address,
            expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await,
        )
        .await
        .expect("join_multicast_group failed")
        .expect("join_multicast_group error");

    network_test_realm
        .stop_hermetic_network_realm()
        .await
        .expect("stop_hermetic_network_realm failed")
        .expect("stop_hermetic_network_realm error");

    network_test_realm
        .start_hermetic_network_realm(fntr::Netstack::V2)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE2_NAME,
        INTERFACE2_MAC_ADDRESS,
        subnet,
    )
    .await;

    network_test_realm
        .join_multicast_group(
            &second_multicast_address,
            expect_hermetic_interface_id(INTERFACE2_NAME, &realm).await,
        )
        .await
        .expect("join_multicast_group failed")
        .expect("join_multicast_group error");

    expect_multicast_event(&fake_ep, MulticastEvent::Joined(second_multicast_address)).await;
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn leave_multicast_group(
    name: &str,
    case_name: &str,
    multicast_address: fnet::IpAddress,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    let id = expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await;

    network_test_realm
        .join_multicast_group(&multicast_address, id)
        .await
        .expect("join_multicast_group failed")
        .expect("join_multicast_group error");

    network_test_realm
        .leave_multicast_group(&multicast_address, id)
        .await
        .expect("leave_multicast_group failed")
        .expect("leave_multicast_group error");

    expect_multicast_event(&fake_ep, MulticastEvent::Left(multicast_address)).await;
}

#[fuchsia::test]
async fn join_multicast_group_with_no_hermetic_network_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        create_netstack_realm("join_multicast_group_with_no_hermetic_network_realm", &sandbox)
            .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    assert_eq!(
        network_test_realm
            .join_multicast_group(
                &fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
                DEFAULT_INTERFACE_ID
            )
            .await
            .expect("join_multicast_group failed"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

#[netstack_test]
#[test_case("v2", fntr::Netstack::V2)]
#[test_case("v3", fntr::Netstack::V3)]
async fn join_multicast_group_with_non_existent_interface(
    name: &str,
    case_name: &str,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    // TODO(https://fxbug.dev/42074274): Resolve error code discrepancy for `IP_ADD_MEMBERSHIP`
    // (called under the hood by `join_multicast_group` below).
    let expected_err = match netstack {
        fntr::Netstack::V2 => fntr::Error::InvalidArguments,
        fntr::Netstack::V3 => fntr::Error::Internal,
    };
    assert_eq!(
        network_test_realm
            .join_multicast_group(
                &fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
                // This interface id does not exist. As a result, an error
                // should be returned.
                DEFAULT_INTERFACE_ID
            )
            .await
            .expect("join_multicast_group failed"),
        Err(expected_err),
    );
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn join_multicast_group_with_non_multicast_address(
    name: &str,
    case_name: &str,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    // `address` is not within the multicast address range. Therefore, an error
    // should be returned.
    let address = subnet.addr;
    assert_eq!(
        network_test_realm
            .join_multicast_group(
                &address,
                expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await
            )
            .await
            .expect("join_multicast_group failed"),
        Err(fntr::Error::InvalidArguments),
    );
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn join_same_multicast_group_multiple_times(
    name: &str,
    case_name: &str,
    multicast_address: fnet::IpAddress,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    let id = expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await;
    network_test_realm
        .join_multicast_group(&multicast_address, id)
        .await
        .expect("join_multicast_group failed")
        .expect("join_multicast_group error");

    // Verify that the error is propagated whenever the same multicast group is
    // joined multiple times.
    assert_eq!(
        network_test_realm
            .join_multicast_group(&multicast_address, id)
            .await
            .expect("duplicate join_multicast_group failed"),
        Err(fntr::Error::AddressInUse)
    );
}

#[fuchsia::test]
async fn leave_multicast_group_with_no_hermetic_network_realm() {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let realm =
        create_netstack_realm("leave_multicast_group_with_no_hermetic_network_realm", &sandbox)
            .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    assert_eq!(
        network_test_realm
            .leave_multicast_group(
                &fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
                DEFAULT_INTERFACE_ID
            )
            .await
            .expect("leave_multicast_group failed"),
        Err(fntr::Error::HermeticNetworkRealmNotRunning),
    );
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn leave_multicast_group_with_non_multicast_address(
    name: &str,
    case_name: &str,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    // `address` is not within the multicast address range. Therefore, an error
    // should be returned.
    let address = subnet.addr;
    assert_eq!(
        network_test_realm
            .leave_multicast_group(
                &address,
                expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await
            )
            .await
            .expect("leave_multicast_group failed"),
        Err(fntr::Error::InvalidArguments),
    );
}

#[netstack_test]
#[test_case(
    "ipv4_netstack2",
    fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
    DEFAULT_IPV4_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv4 netstack2")]
#[test_case(
    "ipv6_netstack2",
    fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
    DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    fntr::Netstack::V2;
    "ipv6 netstack2")]
#[test_case(
        "ipv4_netstack3",
        fnet::IpAddress::Ipv4(DEFAULT_IPV4_MULTICAST_ADDRESS),
        DEFAULT_IPV4_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv4 netstack3")]
#[test_case(
        "ipv6_netstack3",
        fnet::IpAddress::Ipv6(DEFAULT_IPV6_MULTICAST_ADDRESS),
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
        fntr::Netstack::V3;
        "ipv6 netstack3")]
async fn leave_unjoined_multicast_group(
    name: &str,
    case_name: &str,
    multicast_address: fnet::IpAddress,
    subnet: fnet::Subnet,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let realm = create_netstack_realm(format!("{}_{}", name, case_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let _interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        subnet,
    )
    .await;

    // The multicast group must be joined before it can be left.
    assert_eq!(
        network_test_realm
            .leave_multicast_group(
                &multicast_address,
                expect_hermetic_interface_id(INTERFACE1_NAME, &realm).await
            )
            .await
            .expect("leave_multicast_group failed"),
        Err(fntr::Error::AddressNotAvailable)
    );
}

#[netstack_test]
#[test_case("na_ns2", true, true, false, fntr::Netstack::V2; "na netstack2")]
#[test_case("pd_ns2", false, true, false, fntr::Netstack::V2; "pd netstack2")]
#[test_case("na_pd_ns2", true, true, false, fntr::Netstack::V2; "na pd netstack2")]
#[test_case("stateless_ns2", false, false, true, fntr::Netstack::V2; "stateless netstack2")]
#[test_case("na_ns3", true, true, false, fntr::Netstack::V3; "na netstack3")]
#[test_case("pd_ns3", false, true, false, fntr::Netstack::V3; "pd netstack3")]
#[test_case("na_pd_ns3", true, true, false, fntr::Netstack::V3; "na pd netstack3")]
#[test_case("stateless_ns3", false, false, true, fntr::Netstack::V3; "stateless netstack3")]
async fn start_dhcpv6_client(
    name: &str,
    sub_name: &str,
    request_non_temporary_address: bool,
    request_prefix_delegation: bool,
    request_dns_servers: bool,
    netstack: fntr::Netstack,
) {
    let sandbox = netemul::TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network("network").await.expect("failed to create network");
    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let realm = create_netstack_realm(format!("{}_{}", name, sub_name), &sandbox)
        .expect("failed to create netstack realm");

    let network_test_realm = realm
        .connect_to_protocol::<fntr::ControllerMarker>()
        .expect("failed to connect to network test realm controller");

    network_test_realm
        .start_hermetic_network_realm(netstack)
        .await
        .expect("start_hermetic_network_realm failed")
        .expect("start_hermetic_network_realm error");

    let interface = join_network_with_hermetic_netstack(
        &realm,
        &network,
        &network_test_realm,
        INTERFACE1_NAME,
        INTERFACE1_MAC_ADDRESS,
        DEFAULT_IPV6_LINK_LOCAL_SOURCE_SUBNET,
    )
    .await;

    network_test_realm
        .start_dhcpv6_client(
            &fnet_dhcpv6_ext::NewClientParams {
                interface_id: interface.id(),
                address: fnet::Ipv6SocketAddress {
                    address: DEFAULT_IPV6_LINK_LOCAL_SOURCE_ADDR,
                    port: fnet_dhcpv6::DEFAULT_CLIENT_PORT,
                    zone_index: interface.id(),
                },
                config: fnet_dhcpv6_ext::ClientConfig {
                    information_config: fnet_dhcpv6_ext::InformationConfig {
                        dns_servers: request_dns_servers,
                    },
                    non_temporary_address_config: fnet_dhcpv6_ext::AddressConfig {
                        address_count: if request_non_temporary_address { 1 } else { 0 },
                        preferred_addresses: None,
                    },
                    prefix_delegation_config: request_prefix_delegation
                        .then_some(fnet_dhcpv6::PrefixDelegationConfig::Empty(fnet_dhcpv6::Empty)),
                },
                duid: (request_non_temporary_address || request_prefix_delegation)
                    .then(|| fnet_dhcpv6::Duid::Uuid(uuid::Uuid::new_v4().into_bytes())),
            }
            .into(),
        )
        .await
        .expect("FIDL error")
        .expect("start DHCPv6 client");

    let stateful = request_non_temporary_address || request_prefix_delegation;
    let want_msg_type = if stateful {
        packet_formats_dhcp::v6::MessageType::Solicit
    } else {
        packet_formats_dhcp::v6::MessageType::InformationRequest
    };
    fake_ep
        .frame_stream()
        .filter_map(|r| {
            let (data, _dropped) = r.expect("frame stream error");
            futures::future::ready(packets::parse_dhcpv6(data.as_slice()).map(|msg| {
                assert_eq!(msg.msg_type(), want_msg_type);
                if stateful {
                    let found_opts =
                        msg.options().fold((false, false), |(found_na, found_pd), opt| match opt {
                            packet_formats_dhcp::v6::ParsedDhcpOption::Iana(_) => (true, found_pd),
                            packet_formats_dhcp::v6::ParsedDhcpOption::IaPd(_) => (found_na, true),
                            _ => (found_na, found_pd),
                        });
                    assert_eq!(
                        found_opts,
                        (request_non_temporary_address, request_prefix_delegation)
                    );
                }
            }))
        })
        .next()
        .await
        .expect("frame stream terminated unexpectedly");
}

// TODO(https://fxbug.dev/42059019): Test stopping all DHCPv6 clients. Currently
// blocked on address assignment, otherwise there's no observable effect of
// stopping the clients.
