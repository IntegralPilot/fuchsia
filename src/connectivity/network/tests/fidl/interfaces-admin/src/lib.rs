// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

mod blackhole;

use assert_matches::assert_matches;
use fidl::endpoints::Proxy;
use fidl_fuchsia_hardware_network::{self as fhardware_network, FrameType};
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_ext::IntoExt;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_admin as finterfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_resources::GrantForInterfaceAuthorization;
use fidl_fuchsia_net_root as fnet_root;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_posix_socket as fposix_socket;
use fnet_interfaces_ext::admin::TerminalError;
use fuchsia_async::net::{DatagramSocket, UdpSocket};
use fuchsia_async::{self as fasync, DurationExt as _, TimeoutExt as _, Timer};
use futures::{FutureExt as _, StreamExt as _, TryFutureExt as _, TryStreamExt as _};
use net_declare::{
    fidl_ip, fidl_mac, fidl_subnet, net_ip, net_subnet_v6, std_ip, std_ip_v6, std_socket_addr,
};
use net_types::ip::{Ip, IpAddr, IpAddress as _, IpVersion, Ipv4, Ipv6};
use netemul::{InterfaceConfig, RealmUdpSocket as _, TestFakeEndpoint};
use netstack_testing_common::constants::ipv6 as ipv6_consts;
use netstack_testing_common::devices::{
    TUN_DEFAULT_PORT_ID, add_pure_ip_interface, create_ip_tun_port, create_tun_device,
    create_tun_port_with, install_device,
};
use netstack_testing_common::interfaces::{self, TestInterfaceExt as _, add_address_wait_assigned};
use netstack_testing_common::ndp::{send_ra_with_router_lifetime, wait_for_router_solicitation};
use netstack_testing_common::realms::{Netstack3, TestRealmExt as _, TestSandboxExt as _};
use netstack_testing_common::{
    ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
};
use netstack_testing_macros::netstack_test;
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::icmp::ndp::NeighborSolicitation;
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation};
use packet_formats::testutil::ArpPacketInfo;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto as _;
use std::ops::Not as _;
use std::pin::pin;
use test_case::test_case;

#[netstack_test]
async fn address_deprecation(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");

    const ADDR1: std::net::Ipv6Addr = std_ip_v6!("abcd::1");
    const ADDR2: std::net::Ipv6Addr = std_ip_v6!("abcd::2");
    // Cannot be const because `std::net::SocketAddrV6:new` isn't const.
    let sock_addr = std_socket_addr!("[abcd::3]:12345");
    // Note that the absence of the preferred_lifetime_info field implies infinite
    // preferred lifetime.
    let preferred_properties = fidl_fuchsia_net_interfaces_admin::AddressProperties::default();
    let deprecated_properties = fidl_fuchsia_net_interfaces_admin::AddressProperties {
        preferred_lifetime_info: Some(
            fidl_fuchsia_net_interfaces::PreferredLifetimeInfo::Deprecated(
                fidl_fuchsia_net_interfaces::Empty,
            ),
        ),
        ..Default::default()
    };
    let addr1_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                addr: ADDR1.octets(),
            }),
            prefix_len: 16,
        },
        // Note that an empty AddressParameters means that the address has
        // infinite preferred lifetime.
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            initial_properties: Some(preferred_properties.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("failed to add preferred address");

    let addr2_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                addr: ADDR2.octets(),
            }),
            prefix_len: (ADDR2.octets().len() * 8).try_into().unwrap(),
        },
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            initial_properties: Some(deprecated_properties.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("failed to add deprecated address");

    let get_source_addr = || async {
        let sock = realm
            .datagram_socket(
                fidl_fuchsia_posix_socket::Domain::Ipv6,
                fidl_fuchsia_posix_socket::DatagramSocketProtocol::Udp,
            )
            .await
            .expect("failed to create UDP socket");
        sock.connect(&socket2::SockAddr::from(sock_addr)).expect("failed to connect with socket");
        *sock
            .local_addr()
            .expect("failed to get socket local addr")
            .as_socket_ipv6()
            .expect("socket local addr not IPv6")
            .ip()
    };
    assert_eq!(get_source_addr().await, ADDR1);

    addr1_state_provider
        .update_address_properties(&deprecated_properties)
        .await
        .expect("FIDL error deprecating address");
    addr2_state_provider
        .update_address_properties(&preferred_properties)
        .await
        .expect("FIDL error setting address to preferred");

    assert_eq!(get_source_addr().await, ADDR2);
}

#[netstack_test]
async fn update_address_lifetimes(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, Default::default())
        .await
        .expect("install endpoint into Netstack");

    const ADDR: fidl_fuchsia_net::Subnet = fidl_subnet!("abcd::1/64");
    let addr_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        ADDR,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route: Some(true),
            ..Default::default()
        },
    )
    .await
    .expect("failed to add preferred address");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fnet_interfaces_ext::AllInterest,
    >(&interface_state, Default::default())
    .expect("event stream from state")
    .fuse();
    let mut event_stream = pin!(event_stream);

    let mut if_state =
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(interface.id());
    async fn wait_for_lifetimes(
        event_stream: impl futures::Stream<
            Item = Result<
                fidl_fuchsia_net_interfaces_ext::EventWithInterest<
                    fidl_fuchsia_net_interfaces_ext::AllInterest,
                >,
                fidl::Error,
            >,
        >,
        if_state: &mut fidl_fuchsia_net_interfaces_ext::InterfaceState<
            (),
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >,
        want_valid_until: fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant,
    ) -> Result<(), anyhow::Error> {
        fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
            event_stream,
            if_state,
            move |iface| {
                iface.properties.addresses.iter().find_map(
                    |fidl_fuchsia_net_interfaces_ext::Address {
                         addr,
                         valid_until,
                         assignment_state,
                         preferred_lifetime_info: _,
                     }| {
                        (*addr == ADDR
                            && *valid_until == want_valid_until
                            && *assignment_state
                                == fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned)
                            .then_some(())
                    },
                )
            },
        )
        .await
        .map_err(Into::into)
    }
    wait_for_lifetimes(
        event_stream.by_ref(),
        &mut if_state,
        fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::INFINITE_FUTURE,
    )
    .await
    .expect("failed to observe address with default (infinite) lifetimes");

    let no_interest_event_stream = {
        let (watcher, server) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces::WatcherMarker>();
        interface_state
            .get_watcher(
                // Register interest in no fields.
                &fnet_interfaces::WatcherOptions {
                    address_properties_interest: None,
                    ..Default::default()
                },
                server,
            )
            .expect("should succeed");
        futures::stream::try_unfold(watcher, |watcher| async {
            Ok::<_, fidl::Error>(Some((watcher.watch().await?, watcher)))
        })
    };

    let mut no_interest_event_stream = pin!(no_interest_event_stream);

    // We should observe two Existing events, one for loopback and one for the
    // test interface. (The order is not guaranteed).

    let mut seen_existing_ids = Vec::new();

    for _ in 0..2 {
        seen_existing_ids.push(assert_matches!(
            no_interest_event_stream
                .try_next()
                .await
                .expect("should succeed")
                .expect("should not have ended"),
            fnet_interfaces::Event::Existing(fnet_interfaces::Properties {
                id: Some(id),
                ..
            }) => id
        ));
    }

    seen_existing_ids.sort();

    const LOCALHOST_ID: u64 = 1;
    assert_eq!(seen_existing_ids, [LOCALHOST_ID, interface.id()]);

    assert_matches!(
        no_interest_event_stream
            .try_next()
            .await
            .expect("should succeed")
            .expect("should not have ended"),
        fnet_interfaces::Event::Idle(fnet_interfaces::Empty)
    );

    {
        const VALID_UNTIL: fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant =
            fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::from_nanos(123_000_000_000)
                .unwrap();
        addr_state_provider
            .update_address_properties(&fidl_fuchsia_net_interfaces_admin::AddressProperties {
                preferred_lifetime_info: None,
                valid_lifetime_end: Some(VALID_UNTIL.into_nanos()),
                ..Default::default()
            })
            .await
            .expect("FIDL error updating address lifetimes");

        wait_for_lifetimes(event_stream.by_ref(), &mut if_state, VALID_UNTIL)
            .await
            .expect("failed to observe address with updated lifetimes");
    }

    drop(addr_state_provider);

    // Should observe address removal without ever observing address lifetime update.
    let addresses = assert_matches!(
        no_interest_event_stream
            .try_next()
            .await
            .expect("should succeed")
            .expect("should not have ended"),
        fnet_interfaces::Event::Changed(fnet_interfaces::Properties {
            id: Some(id),
            addresses: Some(addresses),
            ..
        }) if id == interface.id() => addresses
    );

    assert!(
        addresses.iter().all(|fnet_interfaces::Address { addr, .. }| addr.clone() != Some(ADDR))
    );
}

#[netstack_test]
async fn add_address_sets_correct_valid_until(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install endpoint into Netstack");

    const VALID_UNTIL: fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant =
        fidl_fuchsia_net_interfaces_ext::PositiveMonotonicInstant::from_nanos(123_000_000_000)
            .unwrap();

    const ADDR: fidl_fuchsia_net::Subnet = fidl_subnet!("2001:0db8::1/64");
    let _addr_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        ADDR,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            initial_properties: Some(fidl_fuchsia_net_interfaces_admin::AddressProperties {
                valid_lifetime_end: Some(VALID_UNTIL.into_nanos()),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .expect("failed to add preferred address");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::AllInterest,
    >(&interface_state, Default::default())
    .expect("event stream from state")
    .fuse();
    let event_stream = pin!(event_stream);
    let mut if_state = fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(interface.id());

    let valid_until = fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream,
        &mut if_state,
        |fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
             properties: fidl_fuchsia_net_interfaces_ext::Properties { addresses, .. },
             state: (),
         }| {
            addresses.iter().find_map(
                |fidl_fuchsia_net_interfaces_ext::Address { addr, valid_until, .. }| {
                    (*addr == ADDR).then_some(*valid_until)
                },
            )
        },
    )
    .await
    .expect("should succeed");

    assert_eq!(valid_until, VALID_UNTIL);
}

#[netstack_test]
async fn add_address_errors(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let fidl_fuchsia_net_interfaces_ext::Properties { id: loopback_id, addresses, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let control = realm
        .interface_control(loopback_id.get())
        .expect("failed to get loopback interface control client proxy");

    let valid_address_parameters = fidl_fuchsia_net_interfaces_admin::AddressParameters::default();

    // Removing non-existent address.
    {
        let address = fidl_subnet!("1.1.1.1/32");
        let did_remove = control
            .remove_address(&address)
            .await
            .expect("FIDL error calling fuchsia.net.interfaces.admin/Control.RemoveAddress")
            .expect("RemoveAddress failed");
        assert!(!did_remove);
    }

    let (control, v4_addr, v6_addr) = futures::stream::iter(addresses).fold((control, None, None), |(control, v4, v6), fidl_fuchsia_net_interfaces_ext::Address {
        addr,
        valid_until: _,
        preferred_lifetime_info: _,
        assignment_state,
    }| {
        assert_eq!(assignment_state, fnet_interfaces::AddressAssignmentState::Assigned);

        let (v4, v6) = {
            let fidl_fuchsia_net::Subnet { addr, prefix_len } = addr;
            match addr {
                fidl_fuchsia_net::IpAddress::Ipv4(addr) => {
                    let nt_addr = net_types::ip::Ipv4Addr::new(addr.addr);
                    assert!(nt_addr.is_loopback(), "{} is not a loopback address", nt_addr);
                    let addr = fidl_fuchsia_net::Ipv4AddressWithPrefix {
                        addr,
                        prefix_len,
                    };
                    assert_eq!(v4, None, "v4 address already present, found {:?}", addr);
                    (Some(addr), v6)
                }
                fidl_fuchsia_net::IpAddress::Ipv6(addr) => {
                    let nt_addr = net_types::ip::Ipv6Addr::from_bytes(addr.addr);
                    assert!(nt_addr.is_loopback(), "{} is not a loopback address", nt_addr);
                    assert_eq!(v6, None, "v6 address already present, found {:?}", addr);
                    let addr = fidl_fuchsia_net::Ipv6AddressWithPrefix {
                        addr,
                        prefix_len,
                    };
                    (v4, Some(addr))
                }
            }
        };
        let valid_address_parameters = valid_address_parameters.clone();
        async move {
            assert_matches::assert_matches!(
                interfaces::add_address_wait_assigned(&control, addr.clone(), valid_address_parameters).await,
                Err(fidl_fuchsia_net_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(
                    fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::AlreadyAssigned
                )));
            (control, v4, v6)
        }
    }).await;
    assert_ne!(v4_addr, None, "expected v4 address");
    assert_ne!(v6_addr, None, "expected v6 address");

    // Adding an invalid address returns error.
    {
        // NB: fidl_subnet! doesn't allow invalid prefix lengths.
        let invalid_address =
            fidl_fuchsia_net::Subnet { addr: fidl_ip!("1.1.1.1"), prefix_len: 33 };
        assert_matches::assert_matches!(
            interfaces::add_address_wait_assigned(
                &control,
                invalid_address,
                valid_address_parameters
            )
            .await,
            Err(fidl_fuchsia_net_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(
                fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::Invalid
            ))
        );
    }
}

#[netstack_test]
async fn invalid_address_properties(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "testif1").await.expect("join network");
    let control = iface.control();

    let invalid_properties = [
        finterfaces_admin::AddressProperties { valid_lifetime_end: Some(-1), ..Default::default() },
        finterfaces_admin::AddressProperties { valid_lifetime_end: Some(0), ..Default::default() },
        finterfaces_admin::AddressProperties {
            preferred_lifetime_info: Some(fnet_interfaces::PreferredLifetimeInfo::PreferredUntil(
                -1,
            )),
            ..Default::default()
        },
        finterfaces_admin::AddressProperties {
            preferred_lifetime_info: Some(fnet_interfaces::PreferredLifetimeInfo::PreferredUntil(
                0,
            )),
            ..Default::default()
        },
    ];
    const ADDR: fnet::Subnet = fidl_subnet!("1.1.1.1/24");
    // Adding an address with invalid properties always fails.
    for p in invalid_properties {
        assert_matches::assert_matches!(
            interfaces::add_address_wait_assigned(
                control,
                ADDR,
                finterfaces_admin::AddressParameters {
                    initial_properties: Some(p.clone()),
                    ..Default::default()
                }
            )
            .await,
            Err(fnet_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(
                finterfaces_admin::AddressRemovalReason::InvalidProperties
            ))
        );

        // Now do the same thing but attempting to update properties.
        let asp = interfaces::add_address_wait_assigned(control, ADDR, Default::default())
            .await
            .expect("add address");
        let mut events = asp.take_event_stream().filter_map(|m| {
            futures::future::ready(match m.expect("stream error") {
                finterfaces_admin::AddressStateProviderEvent::OnAddressAdded { .. } => None,
                finterfaces_admin::AddressStateProviderEvent::OnAddressRemoved { error } => {
                    Some(error)
                }
            })
        });
        assert_matches!(asp.update_address_properties(&p).await, Err(e) if e.is_closed());
        assert_eq!(
            events.next().await,
            Some(finterfaces_admin::AddressRemovalReason::InvalidProperties)
        );
    }
}

#[netstack_test]
async fn add_ipv4_mapped_ipv6_address(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let fidl_fuchsia_net_interfaces_ext::Properties { id: loopback_id, .. } = realm
        .loopback_properties()
        .await
        .expect("failed to get loopback properties")
        .expect("loopback not found");

    let control = realm
        .interface_control(loopback_id.get())
        .expect("failed to get loopback interface control client proxy");

    let mapped_address =
        fidl_fuchsia_net::Subnet { addr: fidl_ip!("::FFFF:192.0.2.1"), prefix_len: 128 };

    assert_matches::assert_matches!(
        interfaces::add_address_wait_assigned(
            &control,
            mapped_address,
            fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
        )
        .await,
        Err(fnet_interfaces_ext::admin::AddressStateProviderError::AddressRemoved(
            finterfaces_admin::AddressRemovalReason::Invalid,
        ))
    )
}

#[netstack_test]
#[test_case([FrameType::Ethernet], [FrameType::Ethernet],
    Some(fidl_mac!("02:03:04:05:06:07")), Ok(()); "ethernet")]
#[test_case([FrameType::Ipv4], [FrameType::Ipv4], None, Err(()); "ipv4-only-fails")]
#[test_case([FrameType::Ipv6], [FrameType::Ipv6], None, Err(()); "ipv6-only-fails")]
#[test_case([FrameType::Ipv4, FrameType::Ipv6], [FrameType::Ipv4, FrameType::Ipv6],
    None, Ok(()); "pure-ip")]
#[test_case([FrameType::Ethernet, FrameType::Ipv4, FrameType::Ipv6],
    [FrameType::Ethernet, FrameType::Ipv4, FrameType::Ipv6],
    Some(fidl_mac!("02:03:04:05:06:07")), Err(()); "mixed-fails")]
#[test_case([FrameType::Ethernet], [FrameType::Ipv4, FrameType::Ipv6],
    Some(fidl_mac!("02:03:04:05:06:07")), Err(()); "asymmetric-fails")]
async fn supported_port_frame_types(
    name: &str,
    rx_frame_types: impl IntoIterator<Item = FrameType>,
    tx_frame_types: impl IntoIterator<Item = FrameType>,
    mac: Option<fnet::MacAddress>,
    expected_result: Result<(), ()>,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let (tun_device, network_device) = create_tun_device();
    let admin_device_control = install_device(&realm, network_device);
    let (tun_port, network_port) =
        create_tun_port_with(&tun_device, TUN_DEFAULT_PORT_ID, rx_frame_types, tx_frame_types, mac)
            .await;
    tun_port.set_online(true).await.expect("set port online");
    let fhardware_network::PortInfo { id, .. } = network_port.get_info().await.expect("get info");
    let port_id = id.expect("port id");
    let (admin_control, server_end) =
        fidl::endpoints::create_proxy::<finterfaces_admin::ControlMarker>();
    let admin_control = fnet_interfaces_ext::admin::Control::new(admin_control);

    admin_device_control
        .create_interface(
            &port_id,
            server_end,
            finterfaces_admin::Options {
                name: Some("tun-interface".to_string()),
                ..Default::default()
            },
        )
        .expect("create interface");
    match expected_result {
        Ok(()) => {
            // NB: Use get_id as a proxy metric for successful installation.
            let _id = admin_control.get_id().await.expect("get_id");
        }
        Err(()) => {
            assert_matches::assert_matches!(
                admin_control.wait_termination().await,
                fnet_interfaces_ext::admin::TerminalError::Terminal(
                    finterfaces_admin::InterfaceRemovedReason::BadPort
                )
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum AddressRemovalMethod {
    InterfaceControl,
    AddressStateProviderExplicitRemove,
}

#[netstack_test]
#[test_case(false, AddressRemovalMethod::InterfaceControl ; "eth removing address via Control.Remove")]
#[test_case(false, AddressRemovalMethod::AddressStateProviderExplicitRemove ; "eth removing address via AddressStateProvider.Remove")]
#[test_case(true, AddressRemovalMethod::InterfaceControl ; "tun removing address via Control.Remove")]
#[test_case(true, AddressRemovalMethod::AddressStateProviderExplicitRemove ; "tun removing address via AddressStateProvider.Remove")]
async fn add_address_removal(name: &str, tun: bool, removal_method: AddressRemovalMethod) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    enum KeepResource<'a> {
        Interface {
            _interface: netemul::TestInterface<'a>,
        },
        Tun {
            _dev: fidl_fuchsia_net_tun::DeviceProxy,
            _port: fidl_fuchsia_net_tun::PortProxy,
            _dev_control: fidl_fuchsia_net_interfaces_admin::DeviceControlProxy,
        },
    }

    let (_interface_or_tun, control): (KeepResource<'_>, _) = if tun {
        let (tun_device, network_device) = create_tun_device();
        let admin_device_control = install_device(&realm, network_device);
        // Retain `_tun_port` to keep the FIDL channel open.
        let (tun_port, network_port) = create_ip_tun_port(&tun_device, TUN_DEFAULT_PORT_ID).await;
        tun_port.set_online(true).await.expect("set port online");
        let admin_control =
            add_pure_ip_interface(&network_port, &admin_device_control, "tunif").await;
        let admin_control = fidl_fuchsia_net_interfaces_ext::admin::Control::new(admin_control);
        assert!(
            admin_control
                .enable()
                .await
                .expect("send enable tun interface request")
                .expect("enable tun interface")
        );
        (
            KeepResource::Tun {
                _dev: tun_device,
                _port: tun_port,
                _dev_control: admin_device_control,
            },
            admin_control,
        )
    } else {
        let device = sandbox.create_endpoint(name).await.expect("create endpoint");
        let interface = realm
            .install_endpoint(device, InterfaceConfig::default())
            .await
            .expect("install interface");
        let id = interface.id();
        let root_control = realm
            .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
            .expect(<fidl_fuchsia_net_root::InterfacesMarker as fidl::endpoints::DiscoverableProtocolMarker>::PROTOCOL_NAME);
        let (admin_control, server) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create Control proxy");
        root_control.get_admin(id, server).expect("get admin");
        (KeepResource::Interface { _interface: interface }, admin_control)
    };

    let valid_address_parameters = fidl_fuchsia_net_interfaces_admin::AddressParameters::default();

    // Adding a valid address and observing the address removal.
    {
        let address = fidl_subnet!("3.3.3.3/32");

        let address_state_provider = interfaces::add_address_wait_assigned(
            &control,
            address,
            valid_address_parameters.clone(),
        )
        .await
        .expect("add address failed unexpectedly");

        match removal_method {
            AddressRemovalMethod::InterfaceControl => {
                let did_remove = control
                    .remove_address(&address)
                    .await
                    .expect("FIDL error calling Control.RemoveAddress")
                    .expect("error calling Control.RemoveAddress");
                assert!(did_remove);
            }
            AddressRemovalMethod::AddressStateProviderExplicitRemove => {
                address_state_provider.remove().expect("should not get FIDL error");
            }
        };

        let event = address_state_provider
            .take_event_stream()
            .try_next()
            .await
            .expect("read AddressStateProvider event")
            .expect("AddressStateProvider event stream ended unexpectedly");
        assert_matches!(
            event,
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                error: reason,
            } => {
                assert_eq!(
                    reason,
                    fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::UserRemoved,
                )
            }
        );
    }

    // Adding a valid address and removing the interface.
    {
        let address = fidl_subnet!("4.4.4.4/32");

        let address_state_provider = interfaces::add_address_wait_assigned(
            &control,
            address,
            valid_address_parameters.clone(),
        )
        .await
        .expect("add address failed unexpectedly");

        control.remove().await.expect("send remove interface request").expect("remove interface");
        let event = address_state_provider
            .take_event_stream()
            .try_next()
            .await
            .expect("read AddressStateProvider event")
            .expect("AddressStateProvider event stream ended unexpectedly");
        assert_matches!(
            event,
            fidl_fuchsia_net_interfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                error: reason,
            } => {
                assert_eq!(
                    reason,
                    fidl_fuchsia_net_interfaces_admin::AddressRemovalReason::InterfaceRemoved
                );
            }
        );

        assert_matches::assert_matches!(
            control.wait_termination().await,
            fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(
                fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User
            )
        );
    }
}

// Races address and interface removal and verifies that the end state is as
// expected. Guards against regression for ASP race in Netstack3.
#[netstack_test]
async fn race_address_and_interface_removal(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");
    let iface_id = interface.id();

    let address_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        fidl_subnet!("192.0.2.1/24"),
        fidl_fuchsia_net_interfaces_admin::AddressParameters::default(),
    )
    .await
    .expect("add address failed unexpectedly");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interfaces_state, Default::default())
    .expect("create event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut watcher = pin!(watcher);

    let _existing = fidl_fuchsia_net_interfaces_ext::existing(
        watcher.by_ref().map(Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");

    // Drop both the interface and state provider, poll on the watcher until we
    // see the interface removed.
    core::mem::drop((interface, address_state_provider));
    loop {
        let event = watcher.next().await.expect("watcher closed");
        match event.into_inner() {
            e @ fnet_interfaces::Event::Existing(_)
            | e @ fnet_interfaces::Event::Idle(_)
            | e @ fnet_interfaces::Event::Added(_) => panic!("unexpected event {e:?}"),
            fnet_interfaces::Event::Removed(id) => {
                assert_eq!(id, iface_id);
                break;
            }
            fnet_interfaces::Event::Changed(fnet_interfaces::Properties { id, .. }) => {
                // We may see interface changed events depending on how the race
                // plays out, the only thing we can assert on is that it relates
                // to the interface we're racing.
                assert_eq!(id, Some(iface_id));
            }
        }
    }
}

// Add an address while the interface is offline, bring the interface online and ensure that the
// assignment state is set correctly.
#[netstack_test]
async fn add_address_offline(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = device.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let id = interface.id();

    let root_control = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect(<fidl_fuchsia_net_root::InterfacesMarker as fidl::endpoints::DiscoverableProtocolMarker>::PROTOCOL_NAME);

    let (control, server) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::ControlMarker>();
    root_control.get_admin(id, server).expect("get admin");

    let valid_address_parameters = fidl_fuchsia_net_interfaces_admin::AddressParameters::default();

    // Adding a valid address and observing the address removal.
    let address = fidl_subnet!("5.5.5.5/32");

    let (address_state_provider, server) = fidl::endpoints::create_proxy::<
        fidl_fuchsia_net_interfaces_admin::AddressStateProviderMarker,
    >();
    control
        .add_address(&address, &valid_address_parameters, server)
        .expect("Control.AddAddress FIDL error");

    let state_stream = fidl_fuchsia_net_interfaces_ext::admin::assignment_state_stream(
        address_state_provider.clone(),
    );
    let mut state_stream = pin!(state_stream);
    fidl_fuchsia_net_interfaces_ext::admin::wait_assignment_state(
        &mut state_stream,
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Unavailable,
    )
    .await
    .expect("wait for UNAVAILABLE address assignment state");

    let did_enable = interface.control().enable().await.expect("send enable").expect("enable");
    assert!(did_enable);
    interface.set_link_up(true).await.expect("bring device up");

    fidl_fuchsia_net_interfaces_ext::admin::wait_assignment_state(
        &mut state_stream,
        fidl_fuchsia_net_interfaces::AddressAssignmentState::Assigned,
    )
    .await
    .expect("wait for ASSIGNED address assignment state");
}

// Verify that a request to `WatchAddressAssignmentState` while an existing
// request is pending causes the `AddressStateProvider` protocol to close,
// regardless of whether the protocol is detached.
#[netstack_test]
#[test_case(false; "no_detach")]
#[test_case(true; "detach")]
async fn duplicate_watch_address_assignment_state(name: &str, detach: bool) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");

    let valid_address_parameters = fidl_fuchsia_net_interfaces_admin::AddressParameters::default();
    let address = fidl_subnet!("1.1.1.1/32");
    let address_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        address,
        valid_address_parameters,
    )
    .await
    .expect("add address failed unexpectedly");

    if detach {
        address_state_provider.detach().expect("failed to detach");
    }

    // Invoke `WatchAddressAssignmentState` twice and assert that the two
    // requests and the AddressStateProvider protocol are closed.
    assert_matches!(
        futures::future::join(
            address_state_provider.watch_address_assignment_state(),
            address_state_provider.watch_address_assignment_state(),
        )
        .await,
        (
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
            Err(fidl::Error::ClientChannelClosed { epitaph: fidl::Epitaph::PeerClosed, .. }),
        )
    );
    assert_matches!(
        address_state_provider
            .take_event_stream()
            .try_next()
            .await
            .expect("read AddressStateProvider event"),
        None
    );
}

/// Creates a realm in the provided sandbox and an interface in that realm.
async fn create_realm_and_interface<'a>(
    name: &'a str,
    sandbox: &'a netemul::TestSandbox,
) -> (
    netemul::TestRealm<'a>,
    fidl_fuchsia_net_interfaces::StateProxy,
    u64,
    fidl_fuchsia_net_interfaces_ext::admin::Control,
) {
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect(<fidl_fuchsia_net_interfaces::StateMarker as fidl::endpoints::DiscoverableProtocolMarker>::PROTOCOL_NAME);

    let interfaces = fidl_fuchsia_net_interfaces_ext::existing(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::DefaultInterest,
        >(&interface_state, Default::default())
        .expect("create watcher event stream"),
        HashMap::<_, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("initial");
    assert_eq!(interfaces.len(), 1);
    let id = *interfaces
        .keys()
        .next()
        .expect("interface properties map unexpectedly does not include loopback");

    let root_control = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect(<fidl_fuchsia_net_root::InterfacesMarker as fidl::endpoints::DiscoverableProtocolMarker>::PROTOCOL_NAME);

    let (control, server) = fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
        .expect("create Control proxy");
    root_control.get_admin(id, server).expect("get admin");

    (realm, interface_state, id, control)
}

async fn ipv4_routing_table(
    realm: &netemul::TestRealm<'_>,
) -> Vec<fnet_routes_ext::InstalledRoute<Ipv4>> {
    let state_v4 =
        realm.connect_to_protocol::<fnet_routes::StateV4Marker>().expect("connect to protocol");
    let stream = fnet_routes_ext::event_stream_from_state::<Ipv4>(&state_v4)
        .expect("failed to connect to watcher");
    let stream = pin!(stream);
    fnet_routes_ext::collect_routes_until_idle::<_, Vec<_>>(stream)
        .await
        .expect("failed to get routing table")
}

enum AddressRemoval {
    DropHandle,
    CallControlRemove,
    CallAddressStateProviderRemove,
}
use AddressRemoval::*;

#[netstack_test]
#[test_case(
    None,
    false,
    CallControlRemove;
    "default add_subnet_route explicit remove via Control"
)]
#[test_case(
    None,
    false,
    CallAddressStateProviderRemove;
    "default add_subnet_route explicit remove via AddressStateProvider"
)]
#[test_case(
    None,
    false,
    DropHandle;
    "default add_subnet_route implicit remove"
)]
#[test_case(
    Some(false),
    false,
    CallControlRemove;
    "add_subnet_route is false explicit remove via Control"
)]
#[test_case(
    Some(false),
    false,
    CallAddressStateProviderRemove;
    "add_subnet_route is false explicit remove via AddressStateProvider"
)]
#[test_case(
    Some(false),
    false,
    DropHandle;
    "add_subnet_route is false implicit remove"
)]
#[test_case(
    Some(true),
    true,
    CallControlRemove;
    "add_subnet_route is true explicit remove via Control"
)]
#[test_case(
    Some(true),
    true,
    CallAddressStateProviderRemove;
    "add_subnet_route is true explicit remove via AddressStateProvider"
)]
#[test_case(
    Some(true),
    true,
    DropHandle;
    "add_subnet_route is true implicit remove"
)]
async fn add_address_and_remove(
    name: &str,
    add_subnet_route: Option<bool>,
    expect_subnet_route: bool,
    remove_address: AddressRemoval,
) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let (realm, interface_state, id, control) = create_realm_and_interface(name, &sandbox).await;

    // Adding a valid address succeeds.
    let subnet = fidl_subnet!("1.1.1.1/32");
    let address_state_provider = interfaces::add_address_wait_assigned(
        &control,
        subnet,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route,
            ..Default::default()
        },
    )
    .await
    .expect("add address failed unexpectedly");

    // Ensure that a subnet route was added if requested.
    let subnet_route_is_present = ipv4_routing_table(&realm).await.iter().any(|route| {
        <net_types::ip::Subnet<net_types::ip::Ipv4Addr> as IntoExt<fnet::Subnet>>::into_ext(
            route.route.destination,
        ) == subnet
    });
    assert_eq!(subnet_route_is_present, expect_subnet_route);

    let event_stream = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interface_state, Default::default())
    .expect("event stream from state");
    let mut event_stream = pin!(event_stream);
    let mut properties = fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(id);
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        event_stream.by_ref(),
        &mut properties,
        |iface| {
            iface
                .properties
                .addresses
                .iter()
                .any(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr,
                         valid_until: _,
                         preferred_lifetime_info: _,
                         assignment_state,
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        addr == subnet
                    },
                )
                .then(|| ())
        },
    )
    .await
    .expect("wait for address presence");

    match remove_address {
        AddressRemoval::DropHandle => {
            // Explicitly drop the AddressStateProvider channel to cause address deletion.
            std::mem::drop(address_state_provider);

            fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
                event_stream.by_ref(),
                &mut properties,
                |iface| {
                    iface
                        .properties
                        .addresses
                        .iter()
                        .all(
                            |&fidl_fuchsia_net_interfaces_ext::Address {
                                 addr,
                                 valid_until: _,
                                 preferred_lifetime_info: _,
                                 assignment_state,
                             }| {
                                assert_eq!(
                                    assignment_state,
                                    fnet_interfaces::AddressAssignmentState::Assigned
                                );
                                addr != subnet
                            },
                        )
                        .then(|| ())
                },
            )
            .await
            .expect("wait for address absence");
        }
        AddressRemoval::CallControlRemove => {
            assert_eq!(control.remove_address(&subnet).await.expect("fidl success"), Ok(true));
            let event = address_state_provider
                .take_event_stream()
                .try_next()
                .await
                .expect("fidl success")
                .expect("should not have ended");
            assert_matches!(
                event,
                finterfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                    error: finterfaces_admin::AddressRemovalReason::UserRemoved,
                }
            );
        }
        AddressRemoval::CallAddressStateProviderRemove => {
            address_state_provider.remove().expect("should succeed");
            let event = address_state_provider
                .take_event_stream()
                .try_next()
                .await
                .expect("fidl success")
                .expect("should not have ended");
            assert_matches!(
                event,
                finterfaces_admin::AddressStateProviderEvent::OnAddressRemoved {
                    error: finterfaces_admin::AddressRemovalReason::UserRemoved,
                }
            );
        }
    }

    // The address should disappear. Unfortunately, we can't assume it to be
    // synchronously gone from interface watchers as there's no synchronization
    // guarantee between interfaces-admin and interface watchers.
    fnet_interfaces_ext::wait_interface_with_id(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interface_state,
            Default::default(),
        )
        .expect("event stream from state"),
        &mut fnet_interfaces_ext::InterfaceState::Unknown(id),
        |fnet_interfaces_ext::PropertiesAndState {
             properties: fnet_interfaces_ext::Properties { addresses, .. },
             state: (),
         }| {
            addresses
                .iter()
                .all(|fnet_interfaces_ext::Address { addr, .. }| addr != &subnet)
                .then_some(())
        },
    )
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
        panic!("timed out waiting for address to disappear")
    })
    .await
    .expect("should succeed");

    // The subnet route should also disappear.
    let routes_state = realm
        .connect_to_protocol::<<Ipv4 as fnet_routes_ext::FidlRouteIpExt>::StateMarker>()
        .expect("connect to routes state");
    let routes_stream =
        fnet_routes_ext::event_stream_from_state::<Ipv4>(&routes_state).expect("should succeed");
    let mut routes_stream = pin!(routes_stream);

    let mut routes =
        fnet_routes_ext::collect_routes_until_idle::<Ipv4, HashSet<_>>(&mut routes_stream)
            .await
            .expect("collect routes should succeed");

    let is_subnet_route = |route: &fnet_routes_ext::InstalledRoute<Ipv4>| {
        <net_types::ip::Subnet<net_types::ip::Ipv4Addr> as IntoExt<fnet::Subnet>>::into_ext(
            route.route.destination,
        ) == subnet
    };

    if !routes.iter().any(|route| is_subnet_route(route)) {
        // Success, the subnet route is gone.
        return;
    }

    // Otherwise, it might just be removed asynchronously, so we should wait a bit.
    fnet_routes_ext::wait_for_routes::<Ipv4, _, _>(&mut routes_stream, &mut routes, |routes| {
        !routes.iter().any(is_subnet_route)
    })
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT, || {
        panic!("timed out waiting for subnet route to disappear")
    })
    .await
    .expect("should succeed");
}

#[netstack_test]
#[test_case(None, false; "default add_subnet_route")]
#[test_case(Some(false), false; "add_subnet_route is false")]
#[test_case(Some(true), true; "add_subnet_route is true")]
async fn add_address_and_detach(
    name: &str,
    add_subnet_route: Option<bool>,
    expect_subnet_route: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let (realm, interface_state, id, control) = create_realm_and_interface(name, &sandbox).await;

    // Adding a valid address and detaching does not cause the address (or the
    // subnet, if one was requested) to be removed.
    let subnet = fidl_subnet!("2.2.2.2/32");
    let address_state_provider = interfaces::add_address_wait_assigned(
        &control,
        subnet,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            add_subnet_route,
            ..Default::default()
        },
    )
    .await
    .expect("add address failed unexpectedly");

    address_state_provider
        .detach()
        .expect("FIDL error calling fuchsia.net.interfaces.admin/Control.Detach");

    std::mem::drop(address_state_provider);

    let mut properties = fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(id);
    fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::DefaultInterest,
        >(&interface_state, Default::default())
        .expect("get interface event stream"),
        &mut properties,
        |iface| {
            iface
                .properties
                .addresses
                .iter()
                .all(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr,
                         valid_until: _,
                         preferred_lifetime_info: _,
                         assignment_state,
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        addr != subnet
                    },
                )
                .then(|| ())
        },
    )
    .map_ok(|()| panic!("address deleted after detaching and closing channel"))
    .on_timeout(
        fuchsia_async::MonotonicInstant::after(zx::MonotonicDuration::from_millis(100)),
        || Ok(()),
    )
    .await
    .expect("wait for address to not be removed");

    let subnet_route_is_still_present = ipv4_routing_table(&realm).await.iter().any(|route| {
        <net_types::ip::Subnet<net_types::ip::Ipv4Addr> as IntoExt<fnet::Subnet>>::into_ext(
            route.route.destination,
        ) == subnet
    });
    assert_eq!(subnet_route_is_still_present, expect_subnet_route);
}

#[netstack_test]
async fn add_remove_address_on_loopback(name: &str) {
    const IPV4_LOOPBACK: fidl_fuchsia_net::Subnet = fidl_subnet!("127.0.0.1/8");
    const IPV6_LOOPBACK: fidl_fuchsia_net::Subnet = fidl_subnet!("::1/128");

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let (loopback_id, addresses) = assert_matches::assert_matches!(
        realm.loopback_properties().await,
        Ok(Some(
            fidl_fuchsia_net_interfaces_ext::Properties {
                id,
                online: true,
                addresses,
                ..
            },
        )) => (id, addresses)
    );
    let addresses: Vec<_> = addresses
        .into_iter()
        .map(|fidl_fuchsia_net_interfaces_ext::Address { addr, .. }| addr)
        .collect();
    assert_eq!(addresses[..], [IPV4_LOOPBACK, IPV6_LOOPBACK]);

    let root = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect("connect to protocol");

    let (control, server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    root.get_admin(loopback_id.get(), server_end).expect("get admin");

    futures::stream::iter([IPV4_LOOPBACK, IPV6_LOOPBACK].into_iter())
        .for_each_concurrent(None, |addr| {
            let control = &control;
            async move {
                let did_remove = control
                    .remove_address(&addr)
                    .await
                    .expect("remove_address")
                    .expect("remove address");
                assert!(did_remove, "{:?}", addr);
            }
        })
        .await;

    futures::stream::iter([fidl_subnet!("1.1.1.1/24"), fidl_subnet!("a::1/64")].into_iter())
        .for_each_concurrent(None, |addr| {
            add_address_wait_assigned(
                &control,
                addr,
                finterfaces_admin::AddressParameters::default(),
            )
            .map(|res| {
                let _: finterfaces_admin::AddressStateProviderProxy = res.expect("add address");
            })
        })
        .await;
}

#[netstack_test]
async fn remove_slaac_address(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "testif1").await.expect("join network");

    // Wait for the SLAAC address to appear.
    let slaac_addr = fnet_interfaces_ext::wait_interface_with_id(
        realm.get_interface_event_stream().expect("get interface event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |fnet_interfaces_ext::PropertiesAndState {
             state: (),
             properties: fnet_interfaces_ext::Properties { addresses, .. },
         }| {
            addresses.iter().find_map(
                |&fnet_interfaces_ext::Address {
                     addr: subnet @ fnet::Subnet { addr, prefix_len: _ },
                     ..
                 }| {
                    match addr {
                        fnet::IpAddress::Ipv4(_) => None,
                        fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                            let addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                            addr.is_unicast_link_local().then_some(subnet)
                        }
                    }
                },
            )
        },
    )
    .await
    .expect("wait for SLAAC address to appear");

    let remove_addr_result = iface
        .control()
        .remove_address(&slaac_addr)
        .await
        .expect("interface should not have been removed");
    assert_eq!(remove_addr_result, Ok(true));
}

#[netstack_test]
async fn device_control_create_interface(name: &str) {
    // NB: interface names are limited to fuchsia.net.interfaces/INTERFACE_NAME_LENGTH.
    const IF_NAME: &'static str = "ctrl_create_if";

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (device, port_id) = endpoint.get_netdevice().await.expect("get netdevice");
    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    let (control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    device_control
        .create_interface(
            &port_id,
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options {
                name: Some(IF_NAME.to_string()),
                metric: None,
                ..Default::default()
            },
        )
        .expect("create interface");

    let iface_id = control.get_id().await.expect("get id");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let interface_state = fidl_fuchsia_net_interfaces_ext::existing(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::DefaultInterest,
        >(&interfaces_state, Default::default())
        .expect("create watcher event stream"),
        fidl_fuchsia_net_interfaces_ext::InterfaceState::<(), _>::Unknown(iface_id),
    )
    .await
    .expect("get interface state");
    let properties = match interface_state {
        fidl_fuchsia_net_interfaces_ext::InterfaceState::Known(
            fidl_fuchsia_net_interfaces_ext::PropertiesAndState { properties, state: () },
        ) => properties,
        fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(id) => {
            panic!("failed to retrieve new interface with id {}", id)
        }
    };

    let port_identity_koid = Some(endpoint.get_port_identity_koid().await.expect("get id event"));

    assert_eq!(
        properties,
        fidl_fuchsia_net_interfaces_ext::Properties {
            id: iface_id.try_into().expect("should be nonzero"),
            name: IF_NAME.to_string(),
            port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Virtual,
            online: false,
            // We haven't enabled the interface, it mustn't have any addresses assigned
            // to it yet.
            addresses: vec![],
            has_default_ipv4_route: false,
            has_default_ipv6_route: false,
            port_identity_koid,
        }
    );
}

// Tests that when a DeviceControl instance is dropped, all interfaces created
// from it are dropped as well.
#[netstack_test]
#[test_case(false; "no_detach")]
#[test_case(true; "detach")]
async fn device_control_owns_interfaces_lifetimes(name: &str, detach: bool) {
    let detach_str = if detach { "detach" } else { "no_detach" };
    let name = format!("{name}_{detach_str}");
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create tun interfaces directly to attach ports to different interfaces.
    let (tun_dev, netdevice_client_end) = create_tun_device();

    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");
    installer
        .install_device(netdevice_client_end, device_control_server_end)
        .expect("install device");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interfaces_state, Default::default())
    .expect("create event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut watcher = pin!(watcher);

    // Consume the watcher until we see the idle event.
    let existing = fidl_fuchsia_net_interfaces_ext::existing(
        watcher.by_ref().map(Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");
    // Only loopback should exist.
    assert_eq!(existing.len(), 1, "unexpected interfaces in existing: {:?}", existing);

    const PORT_COUNT: u8 = 5;
    let mut interfaces = HashSet::new();
    let mut ports_detached_stream = futures::stream::FuturesUnordered::new();
    let mut control_proxies = Vec::new();
    // NB: For loop here is much more friendly to lifetimes than a closure
    // chain.
    for index in 1..=PORT_COUNT {
        let (iface_id, port, control) = async {
            let (port, port_server_end) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_net_tun::PortMarker>();
            tun_dev
                .add_port(
                    &fidl_fuchsia_net_tun::DevicePortConfig {
                        base: Some(fidl_fuchsia_net_tun::BasePortConfig {
                            id: Some(index),
                            rx_types: Some(vec![
                                fidl_fuchsia_hardware_network::FrameType::Ethernet,
                            ]),
                            tx_types: Some(vec![fidl_fuchsia_hardware_network::FrameTypeSupport {
                                type_: fidl_fuchsia_hardware_network::FrameType::Ethernet,
                                features: fidl_fuchsia_hardware_network::FRAME_FEATURES_RAW,
                                supported_flags: fidl_fuchsia_hardware_network::TxFlags::empty(),
                            }]),
                            mtu: Some(netemul::DEFAULT_MTU.into()),
                            ..Default::default()
                        }),
                        mac: Some(fidl_mac!("02:03:04:05:06:07")),
                        ..Default::default()
                    },
                    port_server_end,
                )
                .expect("add port");
            let port_id = {
                let (device_port, server) =
                    fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::PortMarker>();
                port.get_port(server).expect("get port");
                device_port.get_info().await.expect("get info").id.expect("missing port id")
            };

            let (control, control_server_end) =
                fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                    .expect("create proxy");

            device_control
                .create_interface(
                    &port_id,
                    control_server_end,
                    fidl_fuchsia_net_interfaces_admin::Options::default(),
                )
                .expect("create interface");

            let iface_id = control.get_id().await.expect("get id");

            // Observe interface creation in watcher.
            let event = watcher.select_next_some().await;
            assert_matches::assert_matches!(
                event.into_inner(),
                fidl_fuchsia_net_interfaces::Event::Added(
                    fidl_fuchsia_net_interfaces::Properties { id: Some(id), .. }
                ) if id == iface_id
            );

            (iface_id, port, control)
        }
        .await;
        assert!(
            interfaces.insert(iface_id),
            "unexpected duplicate interface iface_id: {}, interfaces={:?}",
            iface_id,
            interfaces
        );
        // Enable the interface and wait for port to be attached.
        assert!(control.enable().await.expect("calling enable").expect("enable failed"));
        let mut port_has_session_stream = futures::stream::unfold(port, |port| {
            port.watch_state().map(move |state| {
                let fidl_fuchsia_net_tun::InternalState { mac: _, has_session, .. } =
                    state.expect("calling watch_state");
                Some((has_session.expect("has_session missing from table"), port))
            })
        });
        loop {
            if port_has_session_stream.next().await.expect("port stream ended unexpectedly") {
                break;
            }
        }
        let port_detached = port_has_session_stream
            .filter_map(move |has_session| {
                futures::future::ready((!has_session).then(move || index))
            })
            .into_future()
            .map(|(i, _stream)| i.expect("port stream ended unexpectedly"));
        ports_detached_stream.push(port_detached);
        control_proxies.push(control);
    }

    let mut control_wait_termination_stream = control_proxies
        .into_iter()
        .map(|control| control.wait_termination())
        .collect::<futures::stream::FuturesUnordered<_>>();

    if detach {
        // Drop detached device_control and ensure none of the futures resolve.
        device_control.detach().expect("detach");
        std::mem::drop(device_control);

        let watcher_fut = watcher.next().map(|e| panic!("unexpected watcher event {:?}", e));
        let ports_fut = ports_detached_stream
            .next()
            .map(|item| panic!("session detached from port unexpectedly {:?}", item));
        let control_closed_fut = control_wait_termination_stream
            .next()
            .map(|termination| panic!("unexpected control termination event {:?}", termination));

        let ((), (), ()) = futures::future::join3(watcher_fut, ports_fut, control_closed_fut)
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(
                    netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
                ),
                || ((), (), ()),
            )
            .await;
    } else {
        // Drop device_control and wait for futures to resolve.
        std::mem::drop(device_control);

        let interfaces_removed_fut =
            async_utils::fold::fold_while(watcher, interfaces, |mut interfaces, event| match event
                .into_inner()
            {
                fidl_fuchsia_net_interfaces::Event::Removed(id) => {
                    assert!(interfaces.remove(&id));
                    futures::future::ready(if interfaces.is_empty() {
                        async_utils::fold::FoldWhile::Done(())
                    } else {
                        async_utils::fold::FoldWhile::Continue(interfaces)
                    })
                }
                event => panic!("unexpected event {:?}", event),
            })
            .map(|fold_result| fold_result.short_circuited().expect("watcher ended"));

        let ports_are_detached_fut =
            ports_detached_stream.map(|_port_index: u8| ()).collect::<()>();
        let control_closed_fut = control_wait_termination_stream.for_each(|termination| {
            assert_matches::assert_matches!(
                termination,
                fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(
                    fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortClosed
                )
            );
            futures::future::ready(())
        });

        let ((), (), ()) = futures::future::join3(
            interfaces_removed_fut,
            ports_are_detached_fut,
            control_closed_fut,
        )
        .await;
    }
}

#[netstack_test]
#[test_case(
fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName;
"DuplicateName"
)]
#[test_case(
fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortAlreadyBound;
"PortAlreadyBound"
)]
#[test_case(fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::BadPort; "BadPort")]
#[test_case(fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortClosed; "PortClosed")]
#[test_case(fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User; "User")]
async fn control_terminal_events(
    name: &str,
    reason: fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason,
) {
    let name = format!("{}_{:?}", name, reason);

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(&name).expect("create realm");

    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (tun_dev, device) = create_tun_device();

    const BASE_PORT_ID: u8 = 13;
    let base_port_config = fidl_fuchsia_net_tun::BasePortConfig {
        id: Some(BASE_PORT_ID),
        rx_types: Some(vec![fidl_fuchsia_hardware_network::FrameType::Ethernet]),
        tx_types: Some(vec![fidl_fuchsia_hardware_network::FrameTypeSupport {
            type_: fidl_fuchsia_hardware_network::FrameType::Ethernet,
            features: fidl_fuchsia_hardware_network::FRAME_FEATURES_RAW,
            supported_flags: fidl_fuchsia_hardware_network::TxFlags::empty(),
        }]),
        mtu: Some(netemul::DEFAULT_MTU.into()),
        ..Default::default()
    };

    let create_port = |config: fidl_fuchsia_net_tun::BasePortConfig| {
        let (port, port_server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_tun::PortMarker>();
        tun_dev
            .add_port(
                &fidl_fuchsia_net_tun::DevicePortConfig {
                    base: Some(config),
                    mac: Some(fidl_mac!("02:aa:bb:cc:dd:ee")),
                    ..Default::default()
                },
                port_server_end,
            )
            .expect("add port");
        async move {
            // Interact with port to make sure it's installed.
            port.set_online(false).await.expect("calling set_online");

            let (device_port, server) =
                fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::PortMarker>();
            port.get_port(server).expect("get port");
            let id = device_port.get_info().await.expect("get info").id.expect("missing port id");

            (port, id)
        }
    };

    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    let create_interface = |port_id, options| {
        let (control, control_server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::ControlMarker>();
        device_control
            .create_interface(&port_id, control_server_end, options)
            .expect("create interface");
        control
    };

    enum KeepResource {
        Control { _control: fidl_fuchsia_net_interfaces_ext::admin::Control },
        Port { _port: fidl_fuchsia_net_tun::PortProxy },
    }

    let (control, _keep_alive): (_, Vec<KeepResource>) = match reason {
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortAlreadyBound => {
            let (port, port_id) = create_port(base_port_config).await;
            let control1 = {
                let control =
                    fidl_fuchsia_net_interfaces_ext::admin::Control::new(create_interface(
                        port_id.clone(),
                        fidl_fuchsia_net_interfaces_admin::Options::default(),
                    ));
                // Verify that interface was created.
                let _: u64 = control.get_id().await.expect("get id");
                control
            };

            // Create a new interface with the same port identifier.
            let control2 =
                create_interface(port_id, fidl_fuchsia_net_interfaces_admin::Options::default());
            (
                control2,
                vec![
                    KeepResource::Control { _control: control1 },
                    KeepResource::Port { _port: port },
                ],
            )
        }
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName => {
            let (port1, port1_id) = create_port(base_port_config.clone()).await;
            let if_name = "test_same_name";
            let control1 = {
                let control =
                    fidl_fuchsia_net_interfaces_ext::admin::Control::new(create_interface(
                        port1_id,
                        fidl_fuchsia_net_interfaces_admin::Options {
                            name: Some(if_name.to_string()),
                            ..Default::default()
                        },
                    ));
                // Verify that interface was created.
                let _: u64 = control.get_id().await.expect("get id");
                control
            };

            // Create a new interface with the same name.
            let (port2, port2_id) = create_port(fidl_fuchsia_net_tun::BasePortConfig {
                id: Some(BASE_PORT_ID + 1),
                ..base_port_config
            })
            .await;

            let control2 = create_interface(
                port2_id,
                fidl_fuchsia_net_interfaces_admin::Options {
                    name: Some(if_name.to_string()),
                    ..Default::default()
                },
            );
            (
                control2,
                vec![
                    KeepResource::Control { _control: control1 },
                    KeepResource::Port { _port: port1 },
                    KeepResource::Port { _port: port2 },
                ],
            )
        }
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::BadPort => {
            let (port, port_id) = create_port(fidl_fuchsia_net_tun::BasePortConfig {
                // netdevice/client.go only accepts IP devices that support both
                // IPv4 and IPv6.
                rx_types: Some(vec![fidl_fuchsia_hardware_network::FrameType::Ipv4]),
                ..base_port_config
            })
            .await;
            let control =
                create_interface(port_id, fidl_fuchsia_net_interfaces_admin::Options::default());
            (control, vec![KeepResource::Port { _port: port }])
        }
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::PortClosed => {
            // Port closed is equivalent to port doesn't exist.
            let control = create_interface(
                fidl_fuchsia_hardware_network::PortId { base: BASE_PORT_ID, salt: 0 },
                fidl_fuchsia_net_interfaces_admin::Options::default(),
            );
            (control, vec![])
        }
        fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User => {
            let (port, port_id) = create_port(base_port_config).await;
            let control =
                create_interface(port_id, fidl_fuchsia_net_interfaces_admin::Options::default());
            let interface_id = control.get_id().await.expect("get id");
            // Setup a control handle via the root API, and drop the original control handle.
            let root_interfaces = realm
                .connect_to_protocol::<fnet_root::InterfacesMarker>()
                .expect("connect to protocol");
            let (root_control, server_end) =
                fidl::endpoints::create_proxy::<finterfaces_admin::ControlMarker>();
            root_interfaces.get_admin(interface_id, server_end).expect("get admin failed");
            // Wait for the root handle to be fully installed by synchronizing on `get_id`.
            assert_eq!(root_control.get_id().await.expect("get id"), interface_id);
            (root_control, vec![KeepResource::Port { _port: port }])
        }
        unknown_reason => panic!("unknown reason {:?}", unknown_reason),
    };

    // Observe a terminal event and channel closure.
    let got_reason = control
        .take_event_stream()
        .map_ok(|fidl_fuchsia_net_interfaces_admin::ControlEvent::OnInterfaceRemoved { reason }| {
            reason
        })
        .try_collect::<Vec<_>>()
        .await
        .expect("waiting for terminal event");
    assert_eq!(got_reason, [reason]);
}

// Test that destroying a device causes device control instance to close.
#[netstack_test]
async fn device_control_closes_on_device_close(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");

    // Create a watcher, we'll use it to ensure the Netstack didn't crash.
    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fnet_interfaces_ext::DefaultInterest,
    >(&interfaces_state, Default::default())
    .expect("create watcher");
    let mut watcher = pin!(watcher);

    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (device, port_id) = endpoint.get_netdevice().await.expect("get netdevice");
    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    // Create an interface and get its identifier to ensure the device is
    // installed.
    let (control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    device_control
        .create_interface(
            &port_id,
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options::default(),
        )
        .expect("create interface");
    let _iface_id: u64 = control.get_id().await.expect("get id");

    // Drop the device and observe the control channel closing because the
    // device was destroyed.
    std::mem::drop(endpoint);
    assert_matches::assert_matches!(device_control.take_event_stream().next().await, None);

    // The channel could've been closed by a Netstack crash, consume from the
    // watcher to ensure that's not the case.
    let _: fnet_interfaces_ext::EventWithInterest<_> =
        watcher.try_next().await.expect("watcher error").expect("watcher ended uexpectedly");
}

// A regression test for https://fxbug.dev/478844205.
//
// Ensure that creating an interface will not panic if the netdevice instance
// exits.
#[netstack_test]
async fn interface_create_close_race(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Open an interfaces_state connection with the netstack. We'll use this
    // to verify the netstack doesn't crash.
    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    // Run the test for several iterations to ensure we're exercising the race.
    const N: u32 = 1000;
    for _ in 0..N {
        let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");
        let (device, port_id) = endpoint.get_netdevice().await.expect("get netdevice");
        let (device_control, device_control_server_end) = fidl::endpoints::create_proxy::<
            fidl_fuchsia_net_interfaces_admin::DeviceControlMarker,
        >();
        installer.install_device(device, device_control_server_end).expect("install device");
        let (_control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create proxy");

        // Race the creation of the interface with the closing of the endpoint.
        // Ignore the result of interface creation. It may fail if the netstack
        // processes the endpoint closure first.
        let _result = device_control.create_interface(
            &port_id,
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options::default(),
        );
        std::mem::drop(endpoint);

        // Observe the control channel closing because the device was destroyed.
        assert_matches::assert_matches!(device_control.take_event_stream().next().await, None);

        // Verify that the Netstack did not crash by observing the interfaces state
        // protocol is still alive.
        assert!(!interfaces_state.is_closed());
    }
}

// TODO(https://fxbug.dev/42061838) Remove this trait once the source of the
// timeout-induced-flake has been identified.
/// A wrapper for a [`futures::future::Future`] that panics if the future has
/// not resolved within [`ASYNC_EVENT_POSITIVE_CHECK_TIME`].
trait PanicOnTimeout: fasync::TimeoutExt {
    /// Wraps the [`futures::future::Future`] in an [`fasync::OnTimeout`] that
    /// panics if the future has not resolved within
    /// [`ASYNC_EVENT_POSITIVE_CHECK_TIME`].
    fn panic_on_timeout<S: std::fmt::Display + 'static>(
        self,
        name: S,
    ) -> fasync::OnTimeout<Self, Box<dyn FnOnce() -> Self::Output>> {
        self.on_timeout(
            ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT,
            Box::new(move || {
                panic!("{}: Timed out after {:?}", name, ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT)
            }),
        )
    }
}

impl<T: fasync::TimeoutExt> PanicOnTimeout for T {}

// Tests that interfaces created through installer have a valid datapath.
#[netstack_test]
#[variant(I, Ip)]
async fn installer_creates_datapath<I: Ip>(test_name: &str) {
    const ALICE_IP_V4: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.1/24");
    const BOB_IP_V4: fidl_fuchsia_net::Subnet = fidl_subnet!("192.168.0.2/24");
    const ALICE_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:01");
    const BOB_MAC: fnet::MacAddress = fidl_mac!("02:00:00:00:00:02");

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox
        .create_network("net")
        .panic_on_timeout("creating network")
        .await
        .expect("create network");

    struct RealmInfo<'a> {
        realm: netemul::TestRealm<'a>,
        endpoint: netemul::TestEndpoint<'a>,
        addr: std::net::IpAddr,
        iface_id: u64,
        device_control: fidl_fuchsia_net_interfaces_admin::DeviceControlProxy,
        control: fidl_fuchsia_net_interfaces_ext::admin::Control,
        address_state_provider:
            Option<fidl_fuchsia_net_interfaces_admin::AddressStateProviderProxy>,
    }

    let realms_stream = futures::stream::iter([
        ("alice", ALICE_IP_V4, ALICE_MAC),
        ("bob", BOB_IP_V4, BOB_MAC),
    ])
    .then(|(name, ipv4_addr, mac)| {
        let sandbox = &sandbox;
        let network = &network;
        async move {
            let test_name = format!("{}_{}", test_name, name);
            let realm = sandbox
                .create_netstack_realm::<Netstack3, _>(test_name.clone())
                .expect("create realm");
            let endpoint = network
                .create_endpoint_with(
                    test_name,
                    netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(mac)),
                )
                .panic_on_timeout(format!("create {} endpoint", name))
                .await
                .expect("create endpoint");
            endpoint
                .set_link_up(true)
                .panic_on_timeout(format!("set {} link up", name))
                .await
                .expect("set link up");
            let installer = realm
                .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
                .expect("connect to protocol");

            let (device, port_id) = endpoint
                .get_netdevice()
                .panic_on_timeout(format!("get {} netdevice", name))
                .await
                .expect("get netdevice");
            let (device_control, device_control_server_end) = fidl::endpoints::create_proxy::<
                fidl_fuchsia_net_interfaces_admin::DeviceControlMarker,
            >();
            installer.install_device(device, device_control_server_end).expect("install device");

            let (control, control_server_end) =
                fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                    .expect("create proxy");
            device_control
                .create_interface(
                    &port_id,
                    control_server_end,
                    fidl_fuchsia_net_interfaces_admin::Options {
                        name: Some(name.to_string()),
                        metric: None,
                        ..Default::default()
                    },
                )
                .expect("create interface");
            let iface_id = control
                .get_id()
                .panic_on_timeout(format!("get {} interface id", name))
                .await
                .expect("get id");

            let did_enable = control
                .enable()
                .panic_on_timeout(format!("enable {} interface", name))
                .await
                .expect("calling enable")
                .expect("enable failed");
            assert!(did_enable);

            let (addr, address_state_provider) = match I::VERSION {
                net_types::ip::IpVersion::V4 => {
                    let address_state_provider = interfaces::add_address_wait_assigned(
                        &control,
                        ipv4_addr,
                        fidl_fuchsia_net_interfaces_admin::AddressParameters {
                            add_subnet_route: Some(true),
                            ..Default::default()
                        },
                    )
                    .panic_on_timeout(format!("add {} ipv4 address", name))
                    .await
                    .expect("add address");

                    let fidl_fuchsia_net_ext::IpAddress(addr) = ipv4_addr.addr.into();
                    (addr, Some(address_state_provider))
                }
                net_types::ip::IpVersion::V6 => {
                    let ipv6 = netstack_testing_common::interfaces::wait_for_v6_ll(
                        &realm
                            .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
                            .expect("connect to protocol"),
                        iface_id,
                    )
                    .panic_on_timeout(format!("wait for {} ipv6 address", name))
                    .await
                    .expect("get ipv6 link local");
                    (net_types::ip::IpAddr::V6(ipv6).into(), None)
                }
            };
            RealmInfo {
                realm,
                addr,
                iface_id,
                endpoint,
                device_control,
                control,
                address_state_provider,
            }
        }
    });
    let mut realms_stream = pin!(realms_stream);

    // Can't drop any of the fields of RealmInfo to maintain objects alive.
    let RealmInfo {
        realm: alice_realm,
        endpoint: _alice_endpoint,
        addr: alice_addr,
        iface_id: alice_iface_id,
        device_control: _alice_device_control,
        control: _alice_control,
        address_state_provider: _alice_asp,
    } = realms_stream
        .next()
        .panic_on_timeout("setup alice fixture")
        .await
        .expect("create alice realm");
    let RealmInfo {
        realm: bob_realm,
        endpoint: _bob_endpoint,
        addr: bob_addr,
        iface_id: bob_iface_id,
        device_control: _bob_device_control,
        control: _bob_control,
        address_state_provider: _bob_asp,
    } = realms_stream
        .next()
        .panic_on_timeout("setup bob fixture")
        .await
        .expect("create alice realm");

    // Put Alice and Bob in each other's neighbor table. We've observed flakes
    // in CQ due to ARP timeouts and ARP resolution is immaterial to the tests
    // we run here.
    alice_realm
        .add_neighbor_entry(
            alice_iface_id,
            fidl_fuchsia_net_ext::IpAddress(bob_addr).into(),
            BOB_MAC,
        )
        .panic_on_timeout("wait for Alice to add a static neighbor")
        .await
        .expect("add static neighbor");
    bob_realm
        .add_neighbor_entry(
            bob_iface_id,
            fidl_fuchsia_net_ext::IpAddress(alice_addr).into(),
            ALICE_MAC,
        )
        .panic_on_timeout("wait for Bob to add a static neighbor")
        .await
        .expect("add static neighbor");

    const PORT: u16 = 8080;
    let (bob_addr, bind_ip) = match bob_addr {
        std::net::IpAddr::V4(addr) => {
            (std::net::SocketAddrV4::new(addr, PORT).into(), std::net::Ipv4Addr::UNSPECIFIED.into())
        }
        std::net::IpAddr::V6(addr) => (
            std::net::SocketAddrV6::new(
                addr,
                PORT,
                0,
                alice_iface_id.try_into().expect("doesn't fit scope id"),
            )
            .into(),
            std::net::Ipv6Addr::UNSPECIFIED.into(),
        ),
    };
    let alice_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &alice_realm,
        std::net::SocketAddr::new(bind_ip, 0),
    )
    .panic_on_timeout("bind alice socket")
    .await
    .expect("bind alice sock");
    let bob_sock = fuchsia_async::net::UdpSocket::bind_in_realm(
        &bob_realm,
        std::net::SocketAddr::new(bind_ip, PORT),
    )
    .panic_on_timeout("bind bob socket")
    .await
    .expect("bind bob sock");

    const PAYLOAD: &'static str = "hello bob";
    let payload_bytes = PAYLOAD.as_bytes();
    assert_eq!(
        alice_sock
            .send_to(payload_bytes, bob_addr)
            .panic_on_timeout("alice sendto bob")
            .await
            .expect("sendto"),
        payload_bytes.len()
    );

    let mut buff = [0; PAYLOAD.len() + 1];
    let (read, from) = bob_sock
        .recv_from(&mut buff[..])
        .panic_on_timeout("alice recvfrom bob")
        .await
        .expect("recvfrom");
    assert_eq!(from.ip(), alice_addr);

    assert_eq!(read, payload_bytes.len());
    assert_eq!(&buff[..read], payload_bytes);
}

#[netstack_test]
async fn control_enable_disable(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let endpoint = sandbox.create_endpoint(name).await.expect("create endpoint");
    endpoint.set_link_up(true).await.expect("set link up");
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (device, port_id) = endpoint.get_netdevice().await.expect("get netdevice");
    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    let (control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interfaces_state, Default::default())
    .expect("create event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut watcher = pin!(watcher);

    // Consume the watcher until we see the idle event.
    let existing = fidl_fuchsia_net_interfaces_ext::existing(
        watcher.by_ref().map(Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");
    // Only loopback should exist.
    assert_eq!(existing.len(), 1, "unexpected interfaces in existing: {:?}", existing);

    device_control
        .create_interface(
            &port_id,
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options::default(),
        )
        .expect("create interface");
    let iface_id = control.get_id().await.expect("get id");

    // Expect the added event.
    let event = watcher.select_next_some().await;
    assert_matches::assert_matches!(event.into_inner(),
        fidl_fuchsia_net_interfaces::Event::Added(
                fidl_fuchsia_net_interfaces::Properties {
                    id: Some(id), online: Some(online), ..
                },
        ) if id == iface_id && !online
    );

    // Starts disabled, it's a no-op.
    let did_disable = control.disable().await.expect("calling disable").expect("disable failed");
    assert!(!did_disable);

    // Enable and observe online.
    let did_enable = control.enable().await.expect("calling enable").expect("enable failed");
    assert!(did_enable);
    watcher
        .by_ref()
        .filter_map(|event| match event.into_inner() {
            fidl_fuchsia_net_interfaces::Event::Changed(
                fidl_fuchsia_net_interfaces::Properties { id: Some(id), online, .. },
            ) if id == iface_id => {
                futures::future::ready(online.and_then(|online| online.then(|| ())))
            }
            event => panic!("unexpected event {:?}", event),
        })
        .select_next_some()
        .await;

    // Enable again should be no-op.
    let did_enable = control.enable().await.expect("calling enable").expect("enable failed");
    assert!(!did_enable);

    // Disable again, expect offline.
    let did_disable = control.disable().await.expect("calling disable").expect("disable failed");
    assert!(did_disable);
    watcher
        .filter_map(|event| match event.into_inner() {
            fidl_fuchsia_net_interfaces::Event::Changed(
                fidl_fuchsia_net_interfaces::Properties { id: Some(id), online, .. },
            ) if id == iface_id => {
                futures::future::ready(online.and_then(|online| (!online).then(|| ())))
            }
            event => panic!("unexpected event {:?}", event),
        })
        .select_next_some()
        .await;
}

#[netstack_test]
async fn link_state_interface_state_interaction(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("new sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = device.into_interface_in_realm(&realm).await.expect("add endpoint to Netstack");
    let iface_id = interface.id();

    interface.set_link_up(false).await.expect("bring device ");

    // Setup the interface watcher.
    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::AllInterest,
    >(&interfaces_state, Default::default())
    .expect("create event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut watcher = pin!(watcher);
    // Consume the watcher until we see the idle event.
    let existing = fidl_fuchsia_net_interfaces_ext::existing(
        watcher.by_ref().map(Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");
    assert_matches!(
        existing.get(&iface_id),
        Some(fidl_fuchsia_net_interfaces_ext::PropertiesAndState {
            properties: fidl_fuchsia_net_interfaces_ext::Properties { online: false, .. },
            state: _,
        })
    );

    // Map the `watcher` to only produce `Events` when `online` changes.
    let watcher =
        watcher.filter_map(|event| match event.into_inner() {
            fidl_fuchsia_net_interfaces::Event::Changed(
                fidl_fuchsia_net_interfaces::Properties { id: Some(id), online, .. },
            ) if id == iface_id => futures::future::ready(online),
            event => panic!("unexpected event {:?}", event),
        });
    let watcher = pin!(watcher);

    // Helper function that polls the watcher and panics if `online` changes.
    async fn expect_online_not_changed<S: futures::Stream<Item = bool> + std::marker::Unpin>(
        mut watcher: S,
        iface_id: u64,
    ) -> S {
        watcher
            .next()
            .map(|online: Option<bool>| match online {
                None => panic!("stream unexpectedly ended"),
                Some(online) => {
                    panic!("online unexpectedly changed to {} for {}", online, iface_id)
                }
            })
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(
                    netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
                ),
                || (),
            )
            .await;
        watcher
    }

    // Set the link up, and observe no change in interface state (because the
    // interface is still disabled).
    interface.set_link_up(true).await.expect("bring device up");
    let watcher = expect_online_not_changed(watcher, iface_id).await;
    // Set the link down, and observe no change in interface state.
    interface.set_link_up(false).await.expect("bring device down");
    let watcher = expect_online_not_changed(watcher, iface_id).await;
    // Enable the interface, and observe no change in interface state (because
    // the link is still down).
    assert!(interface.control().enable().await.expect("send enable").expect("enable"));
    let mut watcher = expect_online_not_changed(watcher, iface_id).await;
    // Set the link up, and observe the interface is online.
    interface.set_link_up(true).await.expect("bring device up");
    let online = watcher.next().await.expect("stream unexpectedly ended");
    assert!(online);
    // Set the link down and observe the interface is offline.
    interface.set_link_up(false).await.expect("bring device down");
    let online = watcher.next().await.expect("stream unexpectedly ended");
    assert!(!online);
}

enum SetupOrder {
    PreferredInterfaceFirst,
    PreferredInterfaceLast,
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(SetupOrder::PreferredInterfaceFirst; "setup_preferred_interface_first")]
#[test_case(SetupOrder::PreferredInterfaceLast; "setup_preferred_interface_last")]
// Verify that interfaces with lower routing metrics are preferred.
//
// Setup two test realms (Netstacks) connected by an underlying network. On the
// send side, install two interfaces with differing metrics. On the receive side
// install a single interface. Configure all interfaces with differing IPs
// within the same subnet (and add subnet routes). Finally, verify (by checking
// the source addr) that sending from the send side to the receive side uses the
// preferred send interface.
async fn interface_routing_metric<I: Ip>(name: &str, order: SetupOrder) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let send_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_send"))
        .expect("create realm");
    let recv_realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_recv"))
        .expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");

    async fn setup_interface_in_realm<'a>(
        metric: Option<u32>,
        addr_subnet: fnet::Subnet,
        name: &'static str,
        realm: &netemul::TestRealm<'a>,
        network: &netemul::TestNetwork<'a>,
    ) -> netemul::TestInterface<'a> {
        let interface = realm
            .join_network_with_if_config(
                network,
                name,
                netemul::InterfaceConfig { name: None, metric, ..Default::default() },
            )
            .await
            .expect("install interface");
        interface.add_address_and_subnet_route(addr_subnet).await.expect("configure address");
        interface.apply_nud_flake_workaround().await.expect("nud flake workaround");

        interface
    }

    const LESS_PREFERRED_METRIC: Option<u32> = Some(100);
    const MORE_PREFERRED_METRIC: Option<u32> = Some(50);
    let (send_addr_sub1, send_addr_sub2, recv_addr_sub) = match I::VERSION {
        IpVersion::V4 => (
            fidl_subnet!("192.168.0.1/24"),
            fidl_subnet!("192.168.0.2/24"),
            fidl_subnet!("192.168.0.3/24"),
        ),
        IpVersion::V6 => {
            (fidl_subnet!("ff::1/64"), fidl_subnet!("ff::2/64"), fidl_subnet!("ff::3/64"))
        }
    };
    let (metric1, metric2, expected_src_addr) = match order {
        SetupOrder::PreferredInterfaceFirst => {
            (MORE_PREFERRED_METRIC, LESS_PREFERRED_METRIC, send_addr_sub1.addr)
        }
        SetupOrder::PreferredInterfaceLast => {
            (LESS_PREFERRED_METRIC, MORE_PREFERRED_METRIC, send_addr_sub2.addr)
        }
    };

    let _send_interface1: netemul::TestInterface<'_> =
        setup_interface_in_realm(metric1, send_addr_sub1, "send-interface1", &send_realm, &network)
            .await;
    let _send_interface2: netemul::TestInterface<'_> =
        setup_interface_in_realm(metric2, send_addr_sub2, "send-interface2", &send_realm, &network)
            .await;
    let _recv_interface: netemul::TestInterface<'_> = setup_interface_in_realm(
        None,
        recv_addr_sub.clone(),
        "recv-interface",
        &recv_realm,
        &network,
    )
    .await;

    async fn create_socket_in_realm<I: Ip>(realm: &netemul::TestRealm<'_>, port: u16) -> UdpSocket {
        let (domain, addr) = match I::VERSION {
            IpVersion::V4 => (
                fposix_socket::Domain::Ipv4,
                std::net::SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)),
            ),
            IpVersion::V6 => (
                fposix_socket::Domain::Ipv6,
                std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)),
            ),
        };
        let socket = realm
            .datagram_socket(domain, fposix_socket::DatagramSocketProtocol::Udp)
            .await
            .expect("failed to create socket");
        socket.bind(&addr.into()).expect("failed to bind socket");
        UdpSocket::from_datagram(DatagramSocket::new_from_socket(socket).unwrap()).unwrap()
    }
    const PORT: u16 = 999;
    let send_socket = create_socket_in_realm::<I>(&send_realm, PORT).await;
    let recv_socket = create_socket_in_realm::<I>(&recv_realm, PORT).await;

    let to_socket_addr = |addr: fnet::IpAddress| -> std::net::SocketAddr {
        let fnet_ext::IpAddress(addr) = addr.into();
        std::net::SocketAddr::from((addr, PORT))
    };

    const BUF: &str = "HELLO WORLD";
    assert_eq!(
        send_socket
            .send_to(BUF.as_bytes(), to_socket_addr(recv_addr_sub.addr))
            .await
            .expect("send failed"),
        BUF.len()
    );

    let mut buffer = [0u8; BUF.len() + 1];
    let (len, source_addr) = recv_socket.recv_from(&mut buffer).await.expect("receive failed");
    assert_eq!(source_addr, to_socket_addr(expected_src_addr));
    assert_eq!(len, BUF.len());
    assert_eq!(&buffer[..BUF.len()], BUF.as_bytes());
}

// Test add/remove address and observe the events in InterfaceWatcher.
#[netstack_test]
async fn control_add_remove_address(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let interface = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");
    let id = interface.id();
    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");

    async fn add_address(
        address: &fnet::Subnet,
        valid_until: zx::MonotonicInstant,
        control: &fidl_fuchsia_net_interfaces_ext::admin::Control,
        id: u64,
        interface_state: &fidl_fuchsia_net_interfaces::StateProxy,
    ) -> finterfaces_admin::AddressStateProviderProxy {
        let (address_state_provider, server_end) =
            fidl::endpoints::create_proxy::<finterfaces_admin::AddressStateProviderMarker>();
        control
            .add_address(
                address,
                &fidl_fuchsia_net_interfaces_admin::AddressParameters {
                    initial_properties: Some(
                        fidl_fuchsia_net_interfaces_admin::AddressProperties {
                            valid_lifetime_end: (valid_until != zx::MonotonicInstant::INFINITE)
                                .then(|| valid_until.into_nanos()),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
                server_end,
            )
            .expect("failed to add_address");
        interfaces::wait_for_addresses(&interface_state, id, |addresses| {
            addresses
                .iter()
                .any(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr,
                         valid_until: got_valid_until,
                         assignment_state,
                         preferred_lifetime_info: _,
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        if addr == *address {
                            assert_eq!(zx::MonotonicInstant::from(got_valid_until), valid_until);
                            true
                        } else {
                            false
                        }
                    },
                )
                .then(|| ())
        })
        .await
        .expect("wait for address presence");
        address_state_provider
    }

    let addresses = [fidl_subnet!("1.1.1.1/32"), fidl_subnet!("3ffe::1/128")];
    for address in addresses {
        // Add an address with infinite valid_until and explicitly remove it.
        let _address_state_provider = add_address(
            &address,
            zx::MonotonicInstant::INFINITE,
            &interface.control(),
            id,
            &interface_state,
        )
        .await;
        let did_remove = interface
            .control()
            .remove_address(&address)
            .await
            .expect("failed to send remove address request")
            .expect("failed to remove address");
        assert!(did_remove);
        interfaces::wait_for_addresses(&interface_state, id, |addresses| {
            addresses
                .iter()
                .all(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr, assignment_state, ..
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        addr != address
                    },
                )
                .then(|| ())
        })
        .await
        .expect("wait for address absence");

        // Add an address with finite valid_until and explicitly remove it.
        let _address_state_provider = add_address(
            &address,
            zx::MonotonicInstant::from_nanos(1234),
            &interface.control(),
            id,
            &interface_state,
        )
        .await;
        let did_remove = interface
            .control()
            .remove_address(&address)
            .await
            .expect("failed to send remove address request")
            .expect("failed to remove address");
        assert!(did_remove);
        interfaces::wait_for_addresses(&interface_state, id, |addresses| {
            addresses
                .iter()
                .all(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr, assignment_state, ..
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        addr != address
                    },
                )
                .then(|| ())
        })
        .await
        .expect("wait for address absence");

        // Add an address and drop the AddressStateProvider handle, verifying
        // the address was removed.
        let address_state_provider = add_address(
            &address,
            zx::MonotonicInstant::INFINITE,
            &interface.control(),
            id,
            &interface_state,
        )
        .await;
        std::mem::drop(address_state_provider);
        interfaces::wait_for_addresses(&interface_state, id, |addresses| {
            addresses
                .iter()
                .all(
                    |&fidl_fuchsia_net_interfaces_ext::Address {
                         addr, assignment_state, ..
                     }| {
                        assert_eq!(
                            assignment_state,
                            fnet_interfaces::AddressAssignmentState::Assigned
                        );
                        addr != address
                    },
                )
                .then(|| ())
        })
        .await
        .expect("wait for address absence");
    }
}

#[netstack_test]
#[test_case(false; "no_detach")]
#[test_case(true; "detach")]
async fn control_owns_interface_lifetime(name: &str, detach: bool) {
    let detach_str = if detach { "detach" } else { "no_detach" };
    let name = format!("{}_{}", name, detach_str);

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(&name).expect("create realm");
    let endpoint = sandbox.create_endpoint(&name).await.expect("create endpoint");
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (device, port_id) = endpoint.get_netdevice().await.expect("get netdevice");
    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    let (control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let watcher = fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
        fidl_fuchsia_net_interfaces_ext::DefaultInterest,
    >(&interfaces_state, Default::default())
    .expect("create event stream")
    .map(|r| r.expect("watcher error"))
    .fuse();
    let mut watcher = pin!(watcher);

    // Consume the watcher until we see the idle event.
    let existing = fidl_fuchsia_net_interfaces_ext::existing(
        watcher.by_ref().map(Result::<_, fidl::Error>::Ok),
        HashMap::<u64, fidl_fuchsia_net_interfaces_ext::PropertiesAndState<(), _>>::new(),
    )
    .await
    .expect("existing");
    // Only loopback should exist.
    assert_eq!(existing.len(), 1, "unexpected interfaces in existing: {:?}", existing);

    device_control
        .create_interface(
            &port_id,
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options::default(),
        )
        .expect("create interface");
    let iface_id = control.get_id().await.expect("get id");

    // Expect the added event.
    let event = watcher.select_next_some().await;
    assert_matches::assert_matches!(event.into_inner(),
        fidl_fuchsia_net_interfaces::Event::Added(
                fidl_fuchsia_net_interfaces::Properties {
                    id: Some(id), ..
                },
        ) if id == iface_id
    );

    let root = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect("connect to protocol");
    let (root_control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    root.get_admin(iface_id, control_server_end).expect("get admin");
    let same_iface_id = root_control.get_id().await.expect("get id");
    assert_eq!(same_iface_id, iface_id);

    if detach {
        control.detach().expect("detach");
        // Drop control and expect the interface to NOT be removed.
        std::mem::drop(control);
        let watcher_fut =
            watcher.select_next_some().map(|event| panic!("unexpected event {:?}", event));

        let root_control_fut = root_control
            .wait_termination()
            .map(|event| panic!("unexpected termination {:?}", event));

        let ((), ()) = futures::future::join(watcher_fut, root_control_fut)
            .on_timeout(
                fuchsia_async::MonotonicInstant::after(
                    netstack_testing_common::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT,
                ),
                || ((), ()),
            )
            .await;
    } else {
        // Drop control and expect the interface to be removed.
        std::mem::drop(control);

        let event = watcher.select_next_some().await;
        assert_matches::assert_matches!(event.into_inner(),
            fidl_fuchsia_net_interfaces::Event::Removed(id) if id == iface_id
        );

        // The root control channel is a weak ref, it didn't prevent destruction,
        // but is closed now.
        assert_matches::assert_matches!(
            root_control.wait_termination().await,
            fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(
                fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::User
            )
        );
    }
}

#[derive(Default, Debug, PartialEq)]
struct IpForwarding {
    v4_unicast: Option<bool>,
    v4_multicast: Option<bool>,
    v6_unicast: Option<bool>,
    v6_multicast: Option<bool>,
}

impl IpForwarding {
    // Returns the expected response when calling `get_forwarding` on
    // an interface that was previously configured using the given config.
    fn expected_next_get_forwarding_response(&self) -> IpForwarding {
        const fn false_if_none(val: Option<bool>) -> Option<bool> {
            // Manual implementation of `Option::and` since it is not yet
            // stable as a const fn.
            match val {
                None => Some(false),
                Some(v) => Some(v),
            }
        }
        IpForwarding {
            v4_unicast: false_if_none(self.v4_unicast),
            v4_multicast: false_if_none(self.v4_multicast),
            v6_unicast: false_if_none(self.v6_unicast),
            v6_multicast: false_if_none(self.v6_multicast),
        }
    }

    // Returns the expected response when calling `set_forwarding` for the first
    // time with the given config.
    fn expected_first_set_forwarding_response(&self) -> IpForwarding {
        fn false_if_some(val: Option<bool>) -> Option<bool> {
            val.and(Some(false))
        }
        IpForwarding {
            v4_unicast: false_if_some(self.v4_unicast),
            v4_multicast: false_if_some(self.v4_multicast),
            v6_unicast: false_if_some(self.v6_unicast),
            v6_multicast: false_if_some(self.v6_multicast),
        }
    }

    fn as_configuration(&self) -> finterfaces_admin::Configuration {
        let IpForwarding { v4_unicast, v4_multicast, v6_unicast, v6_multicast } = *self;
        finterfaces_admin::Configuration {
            ipv4: Some(finterfaces_admin::Ipv4Configuration {
                unicast_forwarding: v4_unicast,
                multicast_forwarding: v4_multicast,
                ..Default::default()
            }),
            ipv6: Some(finterfaces_admin::Ipv6Configuration {
                unicast_forwarding: v6_unicast,
                multicast_forwarding: v6_multicast,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

async fn get_ip_forwarding(iface: &fnet_interfaces_ext::admin::Control) -> IpForwarding {
    let finterfaces_admin::Configuration { ipv4: ipv4_config, ipv6: ipv6_config, .. } = iface
        .get_configuration()
        .await
        .expect("get_configuration FIDL error")
        .expect("error getting configuration");

    let finterfaces_admin::Ipv4Configuration {
        unicast_forwarding: v4_unicast,
        multicast_forwarding: v4_multicast,
        ..
    } = ipv4_config.expect("IPv4 configuration should be populated");
    let finterfaces_admin::Ipv6Configuration {
        unicast_forwarding: v6_unicast,
        multicast_forwarding: v6_multicast,
        ..
    } = ipv6_config.expect("IPv6 configuration should be populated");

    IpForwarding { v4_unicast, v4_multicast, v6_unicast, v6_multicast }
}

#[netstack_test]
#[test_case(
    IpForwarding { v4_unicast: None, v4_multicast: None, v6_unicast: None, v6_multicast: None },
    None
; "set_none")]
#[test_case(
    IpForwarding { v4_unicast: Some(false), v4_multicast: None, v6_unicast: Some(false), v6_multicast: None },
    None
; "set_ip_false")]
#[test_case(
    IpForwarding { v4_unicast: Some(true), v4_multicast: None, v6_unicast: Some(false), v6_multicast: None },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4ForwardingUnsupported)
; "set_ipv4_true")]
#[test_case(
    IpForwarding { v4_unicast: Some(false), v4_multicast: None, v6_unicast: Some(true), v6_multicast: None },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv6ForwardingUnsupported)
; "set_ipv6_true")]
#[test_case(
    IpForwarding { v4_unicast: Some(true), v4_multicast: None, v6_unicast: Some(true), v6_multicast: None },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4ForwardingUnsupported)
; "set_ip_true")]
#[test_case(
    IpForwarding { v4_unicast: None, v4_multicast: Some(false), v6_unicast: None, v6_multicast: Some(false) },
    None
; "set_multicast_ip_false")]
#[test_case(
    IpForwarding { v4_unicast: None, v4_multicast: Some(true), v6_unicast: None, v6_multicast: Some(false) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4MulticastForwardingUnsupported)
; "set_multicast_ipv4_true")]
#[test_case(
    IpForwarding { v4_unicast: None, v4_multicast: Some(false), v6_unicast: None, v6_multicast: Some(true) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv6MulticastForwardingUnsupported)
; "set_multicast_ipv6_true")]
#[test_case(
    IpForwarding { v4_unicast: None, v4_multicast: Some(true), v6_unicast: None, v6_multicast: Some(true) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4MulticastForwardingUnsupported)
; "set_multicast_ip_true")]
#[test_case(
    IpForwarding { v4_unicast: Some(true), v4_multicast: Some(false), v6_unicast: Some(true), v6_multicast: Some(false) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4ForwardingUnsupported)
; "set_ip_true_and_multicast_ip_false")]
#[test_case(
    IpForwarding { v4_unicast: Some(false), v4_multicast: Some(true), v6_unicast: Some(false), v6_multicast: Some(true) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4MulticastForwardingUnsupported)
; "set_ip_false_and_multicast_ip_true")]
#[test_case(
    IpForwarding { v4_unicast: Some(true), v4_multicast: Some(true), v6_unicast: Some(true), v6_multicast: Some(true) },
    Some(finterfaces_admin::ControlSetConfigurationError::Ipv4ForwardingUnsupported)
; "set_ip_and_multicast_ip_true")]
async fn get_set_forwarding_loopback(
    name: &str,
    forwarding_config: IpForwarding,
    expected_err: Option<finterfaces_admin::ControlSetConfigurationError>,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let loopback_control = realm
        .interface_control(assert_matches::assert_matches!(
            realm.loopback_properties().await,
            Ok(Some(fnet_interfaces_ext::Properties {
                id,
                ..
            })) => id.get()
        ))
        .unwrap();

    let expected_get_forwarding_response_when_previously_unset =
        IpForwarding::default().expected_next_get_forwarding_response();
    // Initially, interfaces have IP forwarding disabled.
    assert_eq!(
        get_ip_forwarding(&loopback_control).await,
        expected_get_forwarding_response_when_previously_unset
    );

    assert_eq!(
        loopback_control
            .set_configuration(&forwarding_config.as_configuration())
            .await
            .expect("set_configuration FIDL error"),
        expected_err.map_or_else(
            || Ok(forwarding_config.expected_first_set_forwarding_response().as_configuration()),
            Err,
        ),
    );
    // The configuration should not have changed.
    assert_eq!(
        get_ip_forwarding(&loopback_control).await,
        expected_get_forwarding_response_when_previously_unset,
    );
}

#[netstack_test]
#[test_case(IpForwarding { v4_unicast: None, v4_multicast: None, v6_unicast: None, v6_multicast: None }; "set_none")]
#[test_case(IpForwarding { v4_unicast: Some(false), v4_multicast: None, v6_unicast: Some(false), v6_multicast: None }; "set_ip_false")]
#[test_case(IpForwarding { v4_unicast: Some(true), v4_multicast: None, v6_unicast: Some(false), v6_multicast: None }; "set_ipv4_true")]
#[test_case(IpForwarding { v4_unicast: Some(false), v4_multicast: None, v6_unicast: Some(true), v6_multicast: None }; "set_ipv6_true")]
#[test_case(IpForwarding { v4_unicast: Some(true), v4_multicast: None, v6_unicast: Some(true), v6_multicast: None }; "set_ip_true")]
#[test_case(IpForwarding { v4_unicast: None, v4_multicast: Some(false), v6_unicast: None, v6_multicast: Some(false) }; "set_multicast_ip_false")]
#[test_case(IpForwarding { v4_unicast: None, v4_multicast: Some(true), v6_unicast: None, v6_multicast: Some(false) }; "set_multicast_ipv4_true")]
#[test_case(IpForwarding { v4_unicast: None, v4_multicast: Some(false), v6_unicast: None, v6_multicast: Some(true) }; "set_multicast_ipv6_true")]
#[test_case(IpForwarding { v4_unicast: None, v4_multicast: Some(true), v6_unicast: None, v6_multicast: Some(true) }; "set_multicast_ip_true")]
#[test_case(IpForwarding { v4_unicast: Some(true), v4_multicast: Some(false), v6_unicast: Some(true), v6_multicast: Some(false) }; "set_ip_true_and_multicast_ip_false")]
#[test_case(IpForwarding { v4_unicast: Some(false), v4_multicast: Some(true), v6_unicast: Some(false), v6_multicast: Some(true) }; "set_ip_false_and_multicast_ip_true")]
#[test_case(IpForwarding { v4_unicast: Some(true), v4_multicast: Some(true), v6_unicast: Some(true), v6_multicast: Some(true) }; "set_ip_and_multicast_ip_true")]
async fn get_set_forwarding(name: &str, forwarding_config: IpForwarding) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let net = sandbox.create_network("net").await.expect("create network");
    let iface1 = realm.join_network(&net, "iface1").await.expect("create iface1");
    let iface2 = realm.join_network(&net, "iface2").await.expect("create iface1");

    let expected_get_forwarding_response_when_previously_unset =
        IpForwarding::default().expected_next_get_forwarding_response();
    // Initially, interfaces have IP forwarding disabled.
    assert_eq!(
        get_ip_forwarding(iface1.control()).await,
        expected_get_forwarding_response_when_previously_unset
    );
    assert_eq!(
        get_ip_forwarding(iface2.control()).await,
        expected_get_forwarding_response_when_previously_unset
    );

    /// Sets the forwarding configuration and checks the configuration before
    /// the update was applied.
    async fn set_ip_forwarding(
        iface: &netemul::TestInterface<'_>,
        enable: &IpForwarding,
        expected_previous: &IpForwarding,
    ) {
        let configuration = iface
            .control()
            .set_configuration(&enable.as_configuration())
            .await
            .expect("set_configuration FIDL error")
            .expect("error setting configuration");

        assert_eq!(configuration, expected_previous.as_configuration())
    }

    set_ip_forwarding(
        &iface1,
        &forwarding_config,
        &forwarding_config.expected_first_set_forwarding_response(),
    )
    .await;
    let expected_get_forwarding_response_after_set =
        forwarding_config.expected_next_get_forwarding_response();
    assert_eq!(
        get_ip_forwarding(iface1.control()).await,
        expected_get_forwarding_response_after_set
    );
    assert_eq!(
        get_ip_forwarding(iface2.control()).await,
        expected_get_forwarding_response_when_previously_unset
    );

    // Setting the same config should be a no-op.
    set_ip_forwarding(&iface1, &forwarding_config, &forwarding_config).await;
    assert_eq!(
        get_ip_forwarding(iface1.control()).await,
        expected_get_forwarding_response_after_set,
    );
    assert_eq!(
        get_ip_forwarding(iface2.control()).await,
        expected_get_forwarding_response_when_previously_unset
    );

    // Modifying an interface's IP forwarding should not affect another
    // interface/protocol.
    let reverse_if_some = |val: Option<bool>| val.map(bool::not);
    let reversed_forwarding_config = IpForwarding {
        v4_unicast: reverse_if_some(forwarding_config.v4_unicast),
        v4_multicast: reverse_if_some(forwarding_config.v4_multicast),
        v6_unicast: reverse_if_some(forwarding_config.v6_unicast),
        v6_multicast: reverse_if_some(forwarding_config.v6_multicast),
    };
    set_ip_forwarding(
        &iface2,
        &reversed_forwarding_config,
        &reversed_forwarding_config.expected_first_set_forwarding_response(),
    )
    .await;
    let expected_get_forwarding_response_after_reverse =
        reversed_forwarding_config.expected_next_get_forwarding_response();
    assert_eq!(
        get_ip_forwarding(iface2.control()).await,
        expected_get_forwarding_response_after_reverse,
    );
    assert_eq!(
        get_ip_forwarding(iface1.control()).await,
        expected_get_forwarding_response_after_set
    );

    // Unset forwarding.
    set_ip_forwarding(&iface1, &reversed_forwarding_config, &forwarding_config).await;
    assert_eq!(
        get_ip_forwarding(iface1.control()).await,
        expected_get_forwarding_response_after_reverse
    );
}

// Test that reinstalling a port with the same base port identifier works.
#[netstack_test]
async fn reinstall_same_port(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (tun_dev, device) = create_tun_device();

    let (device_control, device_control_server_end) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::DeviceControlMarker>();
    installer.install_device(device, device_control_server_end).expect("install device");

    const PORT_ID: u8 = 15;

    for index in 0..3 {
        let (tun_port, port_server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_tun::PortMarker>();
        tun_dev
            .add_port(
                &fidl_fuchsia_net_tun::DevicePortConfig {
                    base: Some(fidl_fuchsia_net_tun::BasePortConfig {
                        id: Some(PORT_ID),
                        rx_types: Some(vec![fidl_fuchsia_hardware_network::FrameType::Ethernet]),
                        tx_types: Some(vec![fidl_fuchsia_hardware_network::FrameTypeSupport {
                            type_: fidl_fuchsia_hardware_network::FrameType::Ethernet,
                            features: fidl_fuchsia_hardware_network::FRAME_FEATURES_RAW,
                            supported_flags: fidl_fuchsia_hardware_network::TxFlags::empty(),
                        }]),
                        mtu: Some(netemul::DEFAULT_MTU.into()),
                        ..Default::default()
                    }),
                    mac: Some(fidl_mac!("02:03:04:05:06:07")),
                    ..Default::default()
                },
                port_server_end,
            )
            .expect("add port");

        let (dev_port, port_server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_hardware_network::PortMarker>();

        tun_port.get_port(port_server_end).expect("get port");
        let port_id = dev_port.get_info().await.expect("get info").id.expect("missing port id");

        let (control, control_server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create proxy");
        device_control
            .create_interface(
                &port_id,
                control_server_end,
                fidl_fuchsia_net_interfaces_admin::Options {
                    name: Some(format!("test{}", index)),
                    metric: None,
                    ..Default::default()
                },
            )
            .expect("create interface");

        let did_enable = control.enable().await.expect("calling enable").expect("enable");
        assert!(did_enable);

        {
            // Give the stream a clone of the port proxy. We do this in a closed
            // scope to make sure we have no references to the proxy anymore
            // when we decide to drop it to delete the port.
            let attached_stream = futures::stream::unfold(tun_port.clone(), |port| async move {
                let fidl_fuchsia_net_tun::InternalState { has_session, .. } =
                    port.watch_state().await.expect("watch state");
                Some((has_session.expect("missing session information"), port))
            });
            let mut attached_stream = pin!(attached_stream);
            attached_stream
                .by_ref()
                .filter_map(|attached| futures::future::ready(attached.then(|| ())))
                .next()
                .await
                .expect("stream ended");

            // Drop the interface control handle.
            drop(control);

            // Wait for the session to detach.
            attached_stream
                .filter_map(|attached| futures::future::ready((!attached).then(|| ())))
                .next()
                .await
                .expect("stream ended");
        }

        tun_port.remove().expect("triggered port removal");
        // Wait for the port to close, ensuring we can safely add the port again
        // with the same ID in the next iteration.
        assert_matches::assert_matches!(
            tun_port.take_event_stream().try_next().await.expect("failed to read next event"),
            None
        );
    }
}

#[netstack_test]
async fn synchronous_remove(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let ep = sandbox.create_endpoint(name).await.expect("create endpoint");
    let iface = ep.into_interface_in_realm(&realm).await.expect("install interface");
    iface.control().remove().await.expect("remove completes").expect("remove succeeds");

    let reason = iface.wait_removal().await.expect("wait removal");
    assert_eq!(reason, finterfaces_admin::InterfaceRemovedReason::User);
}

#[netstack_test]
async fn no_remove_loopback(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let fidl_fuchsia_net_interfaces_ext::Properties { id, .. } = realm
        .loopback_properties()
        .await
        .expect("fetching loopback properties")
        .expect("no loopback interface");

    let control = realm.interface_control(id.get()).expect("get interface control");

    assert_eq!(
        control.remove().await.expect("remove"),
        Err(finterfaces_admin::ControlRemoveError::NotAllowed)
    );

    // Reach out again to ensure the interface hasn't been removed and that the
    // channel is still open.
    assert_eq!(control.get_id().await.expect("get id"), id.get());
}

#[netstack_test]
async fn epitaph_is_sent_after_interface_removal(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");
    let ep = sandbox.create_endpoint(name).await.expect("create endpoint");

    let (device, port_id) = ep.get_netdevice().await.expect("get netdevice");
    let installer = realm
        .connect_to_protocol::<finterfaces_admin::InstallerMarker>()
        .expect("connect to protocol");
    let device_control = {
        let (control, server_end) =
            fidl::endpoints::create_proxy::<finterfaces_admin::DeviceControlMarker>();
        installer.install_device(device, server_end).expect("install device");
        control
    };

    // Remove and reinstall the interface from the same port on the same device
    // multiple times to race observing the epitaph with adding the new
    // interface. If the epitaph is sent early, creating the interface will fail
    // with `PortAlreadyBound`.
    for _ in 0..10 {
        let (control, server_end) =
            fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints()
                .expect("create endpoints");
        device_control
            .create_interface(
                &port_id,
                server_end,
                finterfaces_admin::Options {
                    name: Some("testif".to_string()),
                    ..Default::default()
                },
            )
            .expect("create interface");

        control.remove().await.expect("remove completes").expect("remove succeeds");

        assert_matches!(
            control.wait_termination().await,
            fidl_fuchsia_net_interfaces_ext::admin::TerminalError::Terminal(
                finterfaces_admin::InterfaceRemovedReason::User
            )
        );
    }
}

fn to_ip_nud_configuration<I: Ip>(
    config: finterfaces_admin::Configuration,
) -> Option<finterfaces_admin::NudConfiguration> {
    match I::VERSION {
        net_types::ip::IpVersion::V4 => config.ipv4.and_then(|x| x.arp).and_then(|x| x.nud),
        net_types::ip::IpVersion::V6 => config.ipv6.and_then(|x| x.ndp).and_then(|x| x.nud),
    }
}

fn new_nud_config<I: Ip>(
    nud: finterfaces_admin::NudConfiguration,
) -> finterfaces_admin::Configuration {
    match I::VERSION {
        net_types::ip::IpVersion::V4 => finterfaces_admin::Configuration {
            ipv4: Some(finterfaces_admin::Ipv4Configuration {
                arp: Some(finterfaces_admin::ArpConfiguration {
                    nud: Some(nud),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        net_types::ip::IpVersion::V6 => finterfaces_admin::Configuration {
            ipv6: Some(finterfaces_admin::Ipv6Configuration {
                ndp: Some(finterfaces_admin::NdpConfiguration {
                    nud: Some(nud),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn nud_max_multicast_solicitations<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create netstack realm");

    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");

    let fake_ep = network.create_fake_endpoint().expect("create fake ep");

    let (client_addr, server_addr) = match I::VERSION {
        net_types::ip::IpVersion::V4 => (fidl_subnet!("192.0.2.1/24"), std_ip!("192.0.2.2")),
        net_types::ip::IpVersion::V6 => (fidl_subnet!("2001:db8::1/32"), std_ip!("2001:db8::2")),
    };
    let solicit_stream =
        netstack_testing_common::nud::create_metadata_stream(&fake_ep).filter_map(|meta| {
            futures::future::ready(match meta.expect("frame error") {
                netstack_testing_common::nud::FrameMetadata::NeighborSolicitation(dst) => {
                    let fnet_ext::IpAddress(dst) = dst.into();
                    (dst == server_addr).then_some(())
                }
                _ => None,
            })
        });

    iface.add_address_and_subnet_route(client_addr).await.expect("add address");

    let make_nud_config = |v: u16| {
        let nud = finterfaces_admin::NudConfiguration {
            max_multicast_solicitations: Some(v),
            ..Default::default()
        };
        new_nud_config::<I>(nud)
    };

    // Setting a zero value should fail.
    assert_matches!(
        iface.control().set_configuration(&make_nud_config(0)).await,
        Ok(Err(finterfaces_admin::ControlSetConfigurationError::IllegalZeroValue)),
        "can't set to zero"
    );

    // Set a higher value than the default and attempt a neighbor resolution
    // that will not complete, counting the number of solicitations we see on
    // the wire.
    const WANT_SOLICITS: u16 = 4;
    let config = iface
        .control()
        .set_configuration(&make_nud_config(WANT_SOLICITS))
        .await
        .expect("setting configuration")
        .expect("setting more solicitations");
    let finterfaces_admin::NudConfiguration { max_multicast_solicitations, .. } =
        to_ip_nud_configuration::<I>(config).expect("missing nud config");
    // Previous value is the default as defined in RFC 4861.
    const DEFAULT_MAX_MULTICAST_SOLICITATIONS: u16 = 3;
    assert_eq!(max_multicast_solicitations, Some(DEFAULT_MAX_MULTICAST_SOLICITATIONS));
    let finterfaces_admin::NudConfiguration { max_multicast_solicitations, .. } = {
        let config = iface
            .control()
            .get_configuration()
            .await
            .expect("get configuration failed")
            .expect("get configuration error");
        to_ip_nud_configuration::<I>(config).expect("nud present")
    };
    assert_eq!(max_multicast_solicitations, Some(WANT_SOLICITS));

    // Try to ping the server and wait until we observe the number of
    // solicitations.
    let ping_fut = match server_addr {
        std::net::IpAddr::V4(v4) => {
            realm.ping_once::<Ipv4>(std::net::SocketAddrV4::new(v4, 1), 1).left_future()
        }
        std::net::IpAddr::V6(v6) => {
            realm.ping_once::<Ipv6>(std::net::SocketAddrV6::new(v6, 1, 0, 0), 1).right_future()
        }
    }
    .fuse();
    let mut ping_fut = pin!(ping_fut);
    let mut stream_fut = solicit_stream.take(WANT_SOLICITS.into()).collect::<()>().fuse();
    futures::select! {
        () = stream_fut => {},
        r = ping_fut => panic!("ping should not complete {r:?}"),
    }
}

#[netstack_test]
#[variant(I, Ip)]
async fn nud_config_not_supported_on_loopback<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create netstack realm");

    // Check that the behavior is as expected for loopback, which doesn't
    // support these configurations.
    let loopback_id = realm
        .loopback_properties()
        .await
        .expect("error getting loopback properties")
        .expect("loopback must exist")
        .id;
    let loopback_control = realm
        .interface_control(loopback_id.get())
        .expect("failed to get loopback interface control client proxy");

    let set_config = new_nud_config::<I>(finterfaces_admin::NudConfiguration {
        max_multicast_solicitations: Some(2),
        max_unicast_solicitations: Some(3),
        ..Default::default()
    });
    let expect_err = match I::VERSION {
        net_types::ip::IpVersion::V4 => {
            finterfaces_admin::ControlSetConfigurationError::ArpNotSupported
        }
        net_types::ip::IpVersion::V6 => {
            finterfaces_admin::ControlSetConfigurationError::NdpNotSupported
        }
    };

    // Can't set.
    let err = loopback_control
        .set_configuration(&set_config)
        .await
        .expect("set_configuration")
        .expect_err("set configuration should fail");
    assert_eq!(err, expect_err);

    // Not present on get.
    let nud_config = loopback_control
        .get_configuration()
        .await
        .expect("get configuration failed")
        .expect("get configuration error");
    let nud_config = to_ip_nud_configuration::<I>(nud_config);
    assert_matches!(nud_config, None);
}

/// Tests that setting and getting maximum unicast solicitations works.
///
/// Note that differently from the multicast solicitations test, we don't look
/// at the wire behavior after setting. We rely on unit tests for that because
/// the set up is more trouble than it's worth.
#[netstack_test]
#[variant(I, Ip)]
async fn nud_max_unicast_solicitations<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create netstack realm");

    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");

    let make_nud_config = |v: u16| {
        let nud = finterfaces_admin::NudConfiguration {
            max_unicast_solicitations: Some(v),
            ..Default::default()
        };
        new_nud_config::<I>(nud)
    };

    // Setting a zero value should fail.
    assert_matches!(
        iface.control().set_configuration(&make_nud_config(0)).await,
        Ok(Err(finterfaces_admin::ControlSetConfigurationError::IllegalZeroValue)),
        "can't set to zero"
    );

    // Set a higher value than the default.
    const WANT_SOLICITS: u16 = 4;
    let config = iface
        .control()
        .set_configuration(&make_nud_config(WANT_SOLICITS))
        .await
        .expect("setting configuration")
        .expect("setting more solicitations");
    let finterfaces_admin::NudConfiguration { max_unicast_solicitations, .. } =
        to_ip_nud_configuration::<I>(config).expect("missing nud config");
    // Previous value is the default as defined in RFC 4861.
    const DEFAULT_MAX_UNICAST_SOLICITATIONS: u16 = 3;
    assert_eq!(max_unicast_solicitations, Some(DEFAULT_MAX_UNICAST_SOLICITATIONS));
    let finterfaces_admin::NudConfiguration { max_unicast_solicitations, .. } = {
        let config = iface
            .control()
            .get_configuration()
            .await
            .expect("get configuration failed")
            .expect("get configuration error");
        to_ip_nud_configuration::<I>(config).expect("nud present")
    };
    assert_eq!(max_unicast_solicitations, Some(WANT_SOLICITS));
}

/// Test that setting/getting base reachable time works.
///
/// Note that this test does not assert that the time a neighbor entry spends
/// in REACHABLE changes as a result of changing base reachable time due to
/// timing sensitivity.
#[netstack_test]
#[variant(I, Ip)]
async fn nud_base_reachable_time<I: Ip>(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox
        .create_netstack_realm::<Netstack3, _>(format!("{name}_client"))
        .expect("create netstack realm");

    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");

    let make_nud_config = |v: i64| {
        let nud = finterfaces_admin::NudConfiguration {
            base_reachable_time: Some(v),
            ..Default::default()
        };
        new_nud_config::<I>(nud)
    };

    // Setting a zero value should fail.
    assert_matches!(
        iface.control().set_configuration(&make_nud_config(0)).await,
        Ok(Err(finterfaces_admin::ControlSetConfigurationError::IllegalZeroValue)),
        "can't set to zero"
    );

    // Setting a negative value should fail.
    assert_matches!(
        iface.control().set_configuration(&make_nud_config(-1)).await,
        Ok(Err(finterfaces_admin::ControlSetConfigurationError::IllegalNegativeValue)),
        "can't set to negative value"
    );

    // Set a lower value than the default.
    const DEFAULT_BASE_REACHABLE_TIME: zx::MonotonicDuration =
        zx::MonotonicDuration::from_seconds(30);
    let want_base_reachable_time = DEFAULT_BASE_REACHABLE_TIME / 2;
    let config = iface
        .control()
        .set_configuration(&make_nud_config(want_base_reachable_time.into_nanos()))
        .await
        .expect("setting configuration")
        .expect("setting lower base reachable time");
    let finterfaces_admin::NudConfiguration { base_reachable_time, .. } =
        to_ip_nud_configuration::<I>(config).expect("missing nud config");
    // Previous value is the default as defined in RFC 4861.
    assert_eq!(base_reachable_time, Some(DEFAULT_BASE_REACHABLE_TIME.into_nanos()));
    let finterfaces_admin::NudConfiguration { base_reachable_time, .. } = {
        let config = iface
            .control()
            .get_configuration()
            .await
            .expect("get configuration failed")
            .expect("get configuration error");
        to_ip_nud_configuration::<I>(config).expect("nud present")
    };
    assert_eq!(base_reachable_time, Some(want_base_reachable_time.into_nanos()));
}

#[netstack_test]
async fn dad_transmits(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");

    let transmits_from_config = |config: finterfaces_admin::Configuration| {
        let ipv4 = config
            .ipv4
            .and_then(|ipv4| ipv4.arp)
            .and_then(|arp| arp.dad)
            .and_then(|dad| dad.transmits);
        let ipv6 = config
            .ipv6
            .and_then(|ipv6| ipv6.ndp)
            .and_then(|ndp| ndp.dad)
            .and_then(|dad| dad.transmits);
        (ipv4, ipv6)
    };

    let transmits_to_config =
        |ipv4_transmits: u16, ipv6_transmits: u16| finterfaces_admin::Configuration {
            ipv4: Some(finterfaces_admin::Ipv4Configuration {
                arp: Some(finterfaces_admin::ArpConfiguration {
                    dad: Some(finterfaces_admin::DadConfiguration {
                        transmits: Some(ipv4_transmits),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ipv6: Some(finterfaces_admin::Ipv6Configuration {
                ndp: Some(finterfaces_admin::NdpConfiguration {
                    dad: Some(finterfaces_admin::DadConfiguration {
                        transmits: Some(ipv6_transmits),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

    let get_transmits = || async {
        transmits_from_config(
            iface
                .control()
                .get_configuration()
                .await
                .expect("get configuration failed")
                .expect("get configuration error"),
        )
    };

    // Default number of DAD transmits. Defined in RFC 5227 for IPv4 and
    // RFC 4862 for IPv6.
    const DEFAULT_DAD_TRANSMITS_IPV4: u16 = 3;
    const DEFAULT_DAD_TRANSMITS_IPV6: u16 = 1;

    let (initial_ipv4, initial_ipv6) = get_transmits().await;
    assert_eq!(initial_ipv4, Some(DEFAULT_DAD_TRANSMITS_IPV4));
    assert_eq!(initial_ipv6, Some(DEFAULT_DAD_TRANSMITS_IPV6));

    // Arbitrary Values, that are distinguishable from one another.
    const WANT_TRANSMITS_IPV4: u16 = 4;
    const WANT_TRANSMITS_IPV6: u16 = 6;
    let update = iface
        .control()
        .set_configuration(&transmits_to_config(WANT_TRANSMITS_IPV4, WANT_TRANSMITS_IPV6))
        .await
        .expect("set configuration failed")
        .expect("set configuration error");

    let (ipv4_from_update, ipv6_from_update) = transmits_from_config(update);
    assert_eq!(ipv4_from_update, Some(DEFAULT_DAD_TRANSMITS_IPV4));
    assert_eq!(ipv6_from_update, Some(DEFAULT_DAD_TRANSMITS_IPV6));

    let (new_ipv4, new_ipv6) = get_transmits().await;
    assert_eq!(new_ipv4, Some(WANT_TRANSMITS_IPV4));
    assert_eq!(new_ipv6, Some(WANT_TRANSMITS_IPV6));
}

#[netstack_test]
async fn temporary_address_generation(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");

    let set_and_assert = |want| {
        let iface_ref = &iface;
        async move {
            iface_ref
                .set_temporary_address_generation_enabled(want)
                .await
                .expect("set temporary address generation");
            let config = iface_ref
                .control()
                .get_configuration()
                .await
                .expect("get configuration")
                .expect("get configuration FIDL");
            let got = config
                .ipv6
                .and_then(|x| x.ndp)
                .and_then(|x| x.slaac)
                .and_then(|finterfaces_admin::SlaacConfiguration { temporary_address, .. }| {
                    temporary_address
                })
                .expect("temporary address enabled configuration missing");
            assert_eq!(got, want);
        }
    };
    set_and_assert(false).await;

    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");
    wait_for_router_solicitation(&fake_ep).await;

    async fn send_ra(
        fake_ep: &TestFakeEndpoint<'_>,
        prefix: net_types::ip::Subnet<net_types::ip::Ipv6Addr>,
    ) {
        let options = [NdpOptionBuilder::PrefixInformation(PrefixInformation::new(
            prefix.prefix(),
            false, /* on_link_flag */
            true,  /* autonomous_address_configuration_flag */
            99999, /* valid_lifetime */
            99999, /* preferred_lifetime */
            prefix.network(),
        ))];
        send_ra_with_router_lifetime(fake_ep, 0, &options, ipv6_consts::LINK_LOCAL_ADDR)
            .await
            .expect("failed to send router advertisement");
    }
    const PREFIX1: net_types::ip::Subnet<net_types::ip::Ipv6Addr> =
        net_subnet_v6!("2001:db8:1::/64");
    const PREFIX2: net_types::ip::Subnet<net_types::ip::Ipv6Addr> =
        net_subnet_v6!("2001:db8:2::/64");
    send_ra(&fake_ep, PREFIX1).await;
    send_ra(&fake_ep, PREFIX2).await;

    let interface_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("failed to connect to fuchsia.net.interfaces/State");
    fnet_interfaces_ext::wait_interface_with_id(
        fidl_fuchsia_net_interfaces_ext::event_stream_from_state::<
            fidl_fuchsia_net_interfaces_ext::AllInterest,
        >(
            &interface_state,
            fidl_fuchsia_net_interfaces_ext::WatchOptions {
                included_addresses: fidl_fuchsia_net_interfaces_ext::IncludedAddresses::All,
                ..Default::default()
            },
        )
        .expect("error getting interface state event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            // When temporary address assignment is enabled, the expected
            // order of events is the following:
            //
            // 1. Stable addr for PREFIX1 is tentative.
            // 2. Stable addr for PREFIX2 is tentative.
            // 3. Stable addr for PREFIX1 is assigned and temporary addr
            //    for PREFIX1 is tentative.
            // 4. Stable addr for PREFIX2 is assigned and temporary addr
            //    for PREFIX2 is tentative.
            //
            // In order to ensure that temporary addresses are not generated,
            // we wait for the stable PREFIX2 addr to be fully assigned, but
            // look for addresses from PREFIX1 as soon as they're tentative
            // and expect to not see more than 1.
            let (prefix1_count, prefix2_assigned_count) = iface.properties.addresses.iter().fold(
                (0, 0),
                |(prefix1_count, prefix2_assigned_count),
                 &fnet_interfaces_ext::Address {
                     addr: fnet::Subnet { addr, prefix_len: _ },
                     assignment_state,
                     ..
                 }| {
                    match addr {
                        fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => {}
                        fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => {
                            let addr = net_types::ip::Ipv6Addr::from_bytes(addr);
                            if PREFIX1.contains(&addr) {
                                return (prefix1_count + 1, prefix2_assigned_count);
                            } else if PREFIX2.contains(&addr)
                                && assignment_state
                                    == fnet_interfaces::AddressAssignmentState::Assigned
                            {
                                return (prefix1_count, prefix2_assigned_count + 1);
                            }
                        }
                    }
                    (prefix1_count, prefix2_assigned_count)
                },
            );
            assert!(prefix1_count <= 1, "should not generate more than 1 address for prefix 1");
            assert!(
                prefix2_assigned_count <= 1,
                "should not generate more than 1 address for prefix 2"
            );
            (prefix1_count == 1 && prefix2_assigned_count == 1).then_some(())
        },
    )
    .map_err(anyhow::Error::from)
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        Err(anyhow::anyhow!("timed out"))
    })
    .await
    .expect("failed to wait for stable SLAAC addresses to be generated");

    set_and_assert(true).await;
    send_ra(&fake_ep, PREFIX1).await;
    fnet_interfaces_ext::wait_interface_with_id(
        realm.get_interface_event_stream().expect("error getting interface state event stream"),
        &mut fnet_interfaces_ext::InterfaceState::<(), _>::Unknown(iface.id()),
        |iface| {
            (iface
                .properties
                .addresses
                .iter()
                .filter_map(
                    |&fnet_interfaces_ext::Address {
                         addr: fnet::Subnet { addr, prefix_len: _ },
                         ..
                     }| {
                        match addr {
                            fnet::IpAddress::Ipv4(fnet::Ipv4Address { .. }) => None,
                            fnet::IpAddress::Ipv6(fnet::Ipv6Address { addr }) => PREFIX1
                                .contains(&net_types::ip::Ipv6Addr::from_bytes(addr))
                                .then_some(()),
                        }
                    },
                )
                .count()
                == 2)
                .then_some(())
        },
    )
    .map_err(anyhow::Error::from)
    .on_timeout(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT.after_now(), || {
        Err(anyhow::anyhow!("timed out"))
    })
    .await
    .expect("failed to wait for stable and temporary SLAAC addresses to be generated");
}

#[netstack_test]
async fn router_solicitation_configuration(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm.join_network(&network, "client").await.expect("join network");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");

    // Verify that by default, when enabled, the interface sends a router solicitation.
    wait_for_router_solicitation(&fake_ep).await;

    let set_max_router_solicitations = |max| {
        let iface_ref = &iface;
        async move {
            let config = finterfaces_admin::Configuration {
                ipv6: Some(finterfaces_admin::Ipv6Configuration {
                    ndp: Some(finterfaces_admin::NdpConfiguration {
                        router_solicitations: Some(
                            finterfaces_admin::RouterSolicitationConfiguration {
                                max: Some(max),
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let _prev: finterfaces_admin::Configuration = iface_ref
                .control()
                .set_configuration(&config)
                .await
                .expect("set configuration FIDL")
                .expect("error setting configuration");
        }
    };

    let disable_and_enable_interface = || {
        let iface_ref = &iface;
        async move {
            let did_disable = iface_ref
                .control()
                .disable()
                .await
                .expect("disable interface FIDL")
                .expect("error disabling interface");
            assert!(did_disable);

            let did_enable = iface_ref
                .control()
                .enable()
                .await
                .expect("enable interface FIDL")
                .expect("error enabling interface");
            assert!(did_enable);
        }
    };

    // Disable solicitations by setting max router solicitations to 0.
    set_max_router_solicitations(0).await;
    disable_and_enable_interface().await;
    let router_solicitation_fut = wait_for_router_solicitation(&fake_ep).fuse();
    futures::pin_mut!(router_solicitation_fut);
    futures::select!(
        () = router_solicitation_fut => panic!("shouldn't receive a router solicitation"),
        () = fasync::Timer::new(crate::ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT) => {},
    );

    // Re-enable solicitations by setting max router solicitations to 3.
    set_max_router_solicitations(3).await;
    disable_and_enable_interface().await;
    router_solicitation_fut.await;
}

#[netstack_test]
async fn interface_authorization(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let device_1 = sandbox.create_endpoint(format!("{name}1")).await.expect("create endpoint");
    let endpoint_1 = realm
        .install_endpoint(device_1, InterfaceConfig::default())
        .await
        .expect("install interface");
    let control_1 = endpoint_1.control();

    let iface_id_1 = control_1.get_id().await.expect("get id");
    let GrantForInterfaceAuthorization { token: grant_token_1, interface_id: grant_iface_id_1 } =
        control_1.get_authorization_for_interface().await.expect("interface authorization");

    assert_eq!(grant_iface_id_1, iface_id_1);
    let koid_1 = grant_token_1.basic_info().expect("basic_info").koid;

    // Check that the same object is returned for a second call.
    {
        let grant_1_again =
            control_1.get_authorization_for_interface().await.expect("interface authorization");
        assert_eq!(grant_1_again.interface_id, iface_id_1);
        let koid_1_again = grant_1_again.token.basic_info().expect("basic_info").koid;
        assert_eq!(koid_1_again, koid_1);
    }

    let device_2 = sandbox.create_endpoint(format!("{name}2")).await.expect("create endpoint");
    let endpoint_2 = realm
        .install_endpoint(device_2, InterfaceConfig::default())
        .await
        .expect("install interface");
    let control_2 = endpoint_2.control();

    let iface_id_2 = control_2.get_id().await.expect("get id");
    let GrantForInterfaceAuthorization { token: grant_token_2, interface_id: grant_iface_id_2 } =
        control_2.get_authorization_for_interface().await.expect("interface authorization");

    assert_eq!(grant_iface_id_2, iface_id_2);
    let koid_2 = grant_token_2.basic_info().expect("basic_info").koid;

    assert_ne!(koid_1, koid_2);
}

#[netstack_test]
async fn interface_authorization_root_access(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let device = sandbox.create_endpoint(name).await.expect("create endpoint");
    let endpoint = realm
        .install_endpoint(device, InterfaceConfig::default())
        .await
        .expect("install interface");
    let control = endpoint.control().clone();

    let iface_id = control.get_id().await.expect("get id");
    let grant = control.get_authorization_for_interface().await.expect("interface authorization");

    assert_eq!(grant.interface_id, iface_id);
    let koid = grant.token.basic_info().expect("basic_info").koid;

    let root_interfaces = realm
        .connect_to_protocol::<fidl_fuchsia_net_root::InterfacesMarker>()
        .expect("connect to protocol");
    let (root_control, control_server_end) =
        fidl_fuchsia_net_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    root_interfaces
        .get_admin(iface_id, control_server_end)
        .expect("create root interfaces connection");

    let root_grant =
        root_control.get_authorization_for_interface().await.expect("interface authorization");

    // Objects returned through different channels should be the same underlying object.
    assert_eq!(root_grant.interface_id, iface_id);
    let root_koid = grant.token.basic_info().expect("basic_info").koid;

    assert_eq!(koid, root_koid);

    // Remove the interface, so subsequent calls for getting an authorization
    // token should fail.
    let _ = control.remove().await.expect("removing interface");
    let _ = control.wait_termination().await;

    // TerminalError doesn't impl PartialEq/Eq, so we have to do some
    // destructuring to get at the InterfaceRemovedReason directly.
    assert_matches!(
        root_control.get_authorization_for_interface().await,
        Err(TerminalError::Terminal(_))
    );
}

// Verify that the `perform_dad` AddressParameter correctly influences the DAD
// behavior when installing the address.
#[netstack_test]
#[variant(I, Ip)]
#[test_case(true, 1, true; "enabled")]
#[test_case(false, 1, false; "disabled")]
#[test_case(true, 0, false; "interface_disabled")]
#[test_case(false, 0, false; "both_disabled")]
async fn perform_dad_parameter<I: Ip>(
    name: &str,
    perform_dad: bool,
    dad_transmits: u16,
    expect_probe: bool,
) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");

    let config = match I::VERSION {
        IpVersion::V4 => {
            InterfaceConfig { ipv4_dad_transmits: Some(dad_transmits), ..Default::default() }
        }
        IpVersion::V6 => {
            InterfaceConfig { ipv6_dad_transmits: Some(dad_transmits), ..Default::default() }
        }
    };
    // NB: The `perform_dad` AddressParameter is only supported by Netstack3.
    let (_network, _realm, interface, endpoint) =
        netstack_testing_common::setup_network_with::<Netstack3, _>(
            &sandbox,
            name,
            config,
            std::iter::empty::<fnetemul::ChildDef>(),
        )
        .await
        .expect("error setting up network");

    let (fidl_addr, addr) = match I::VERSION {
        IpVersion::V4 => (fidl_subnet!("192.0.2.1/24"), net_ip!("192.0.2.1")),
        IpVersion::V6 => (fidl_subnet!("2001:0db8::1/64"), net_ip!("2001:0db8::1")),
    };
    let _addr_state_provider = interfaces::add_address_wait_assigned(
        interface.control(),
        fidl_addr,
        fidl_fuchsia_net_interfaces_admin::AddressParameters {
            perform_dad: Some(perform_dad),
            ..Default::default()
        },
    )
    .await
    .expect("failed to add address");

    // Check whether a DAD probe is sent.
    let timeout = if expect_probe {
        ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT
    } else {
        ASYNC_EVENT_NEGATIVE_CHECK_TIMEOUT
    };

    /// Returns whether the given data is a correctly formatted DAD probe.
    fn is_probe(data: Vec<u8>, addr: IpAddr) -> bool {
        match addr {
            IpAddr::V4(addr) => {
                // Try to parse the data as an ARP probe. Ignore errors or ARP
                // packets with the wrong target address.
                match packet_formats::testutil::parse_arp_packet_in_ethernet_frame(
                    &data,
                    EthernetFrameLengthCheck::NoCheck,
                ) {
                    Ok(ArpPacketInfo { target_protocol_address: dst_ip, .. }) => dst_ip == addr,
                    Err(_) => false,
                }
            }
            IpAddr::V6(addr) => {
                // Try to parse the data as a neighbor solicitation. Ignore errors
                // or neighbor solicitations with the wrong target address.
                match packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                    Ipv6,
                    _,
                    NeighborSolicitation,
                    _,
                >(&data, EthernetFrameLengthCheck::NoCheck, |_| {})
                {
                    Ok((_src_mac, _dst_mac, _src_ip, _dst_ip, _ttl, ns, _code)) => {
                        *ns.target_address() == addr
                    }
                    Err(_) => false,
                }
            }
        }
    }

    let sent_probe = endpoint
        .frame_stream()
        .take_until(Timer::new(timeout.after_now()))
        .any(|frame| {
            let (data, _dropped) = frame.expect("error in fake endpoint frame stream");
            futures::future::ready(is_probe(data, addr))
        })
        .await;

    assert_eq!(sent_probe, expect_probe)
}

#[netstack_test]
#[variant(I, Ip)]
#[test_case(true; "initial_admin_up")]
#[test_case(false; "initial_admin_down")]
async fn interface_single_ip_version_enablement<I: Ip>(name: &str, initial_admin_up: bool) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let network = sandbox.create_network(name).await.expect("create network");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");
    let ep = realm.join_network(&network, "ep").await.expect("join network");
    if initial_admin_up {
        // Assign some addresses to the interface so that we can observe them being removed.
        ep.add_address(fidl_subnet!("192.0.2.1/24")).await.expect("add v4 address");
        ep.add_address(fidl_subnet!("2001:db8::1/32")).await.expect("add v6 address");
    } else {
        assert!(
            ep.control().disable().await.expect("interface removed").expect("disable interface")
        );
    }

    // Disable an IP version.
    let _old_config = ep
        .control()
        .set_configuration(&finterfaces_admin::Configuration {
            ipv4: Some(finterfaces_admin::Ipv4Configuration {
                enabled: Some(I::VERSION == IpVersion::V4),
                ..Default::default()
            }),
            ipv6: Some(finterfaces_admin::Ipv6Configuration {
                enabled: Some(I::VERSION == IpVersion::V6),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("FIDL error")
        .expect("set configuration");

    if !initial_admin_up {
        assert!(ep.control().enable().await.expect("interface removed").expect("enable interface"));
    }

    // Make sure that addresses are removed for the disabled version.
    let addrs = ep
        .get_addrs(fnet_interfaces_ext::IncludedAddresses::OnlyAssigned)
        .await
        .expect("get addrs");
    assert!(
        addrs.iter().all(|addr| {
            if I::VERSION == IpVersion::V4 {
                matches!(addr.addr.addr, fnet::IpAddress::Ipv4(_))
            } else {
                matches!(addr.addr.addr, fnet::IpAddress::Ipv6(_))
            }
        }),
        "cannot have addresses of the version not enabled"
    );

    // Verify that the address of the version not enabled cannot be added.
    ep.add_address_and_wait_until(
        fidl_subnet!("192.0.2.2/24"),
        if I::VERSION == IpVersion::V4 {
            fnet_interfaces::AddressAssignmentState::Assigned
        } else {
            fnet_interfaces::AddressAssignmentState::Unavailable
        },
    )
    .await
    .expect("add v4 address");
    ep.add_address_and_wait_until(
        fidl_subnet!("2001:db8::2/32"),
        if I::VERSION == IpVersion::V6 {
            fnet_interfaces::AddressAssignmentState::Assigned
        } else {
            fnet_interfaces::AddressAssignmentState::Unavailable
        },
    )
    .await
    .expect("add v6 address");
}

// Regression test for https://fxbug.dev/474390739. Racing a user address
// removal in NS3 with a network-initiated address removal caused a panic.
#[netstack_test]
async fn dad_failure_race_with_user_removal(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    // We run this test only against NS3 because it's the stack that we're
    // keeping from regressing.
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create a new network, new iface, and new fake-ep for each run to
    // prevent frame contamination from each loop.
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm
        .join_network_with_if_config(
            &network,
            "client",
            netemul::InterfaceConfig { ipv6_dad_transmits: Some(u16::MAX), ..Default::default() },
        )
        .await
        .expect("join network");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");

    let ipv6_addr = netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR;

    // Wait for DAD NS.
    let mut stream = fake_ep
        .frame_stream()
        .map(|r| r.expect("frame error").0)
        .filter_map(|data| {
            let is_dad =
                packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                    net_types::ip::Ipv6,
                    _,
                    packet_formats::icmp::ndp::NeighborSolicitation,
                    _,
                >(
                    &data, packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck, |_| {}
                )
                .map_or(
                    false,
                    |(_src_mac, _dst_mac, _src_ip, _dst_ip, _ttl, ns, _code)| {
                        *ns.target_address() == ipv6_addr
                    },
                );
            futures::future::ready(is_dad.then_some(()))
        })
        .fuse();

    // Loop this a number of times so we exercise the racy condition.
    for _ in 0..30 {
        let (asp, asp_server_end) =
            fidl::endpoints::create_proxy::<finterfaces_admin::AddressStateProviderMarker>();

        iface
            .control()
            .add_address(
                &fnet::Subnet { addr: IpAddr::V6(ipv6_addr).into_ext(), prefix_len: 64 },
                &finterfaces_admin::AddressParameters {
                    perform_dad: Some(true),
                    ..Default::default()
                },
                asp_server_end,
            )
            .expect("add address");

        let mut address_removed_event = asp
            .take_event_stream()
            .try_filter_map(|e| {
                futures::future::ok(match e {
                    finterfaces_admin::AddressStateProviderEvent::OnAddressRemoved { error } => {
                        Some(error)
                    }
                    finterfaces_admin::AddressStateProviderEvent::OnAddressAdded { .. } => None,
                })
            })
            .into_future()
            .map(|(r, _stream)| r.expect("stream ended without event").expect("FIDL error"));

        futures::select! {
            r = stream.next() => r.expect("wait for DAD NS"),
            // It's possible an NA was stuck in the pipes from a previous run,
            // just go again if that happens.
            e = address_removed_event => {
                assert_eq!(e, finterfaces_admin::AddressRemovalReason::DadFailed);
                continue;
            }
        }

        // Race a reply that causes DAD to fail with a remove request.
        netstack_testing_common::ndp::fail_dad_with_na(&fake_ep).await;
        asp.remove().expect("calling remove address");

        assert_matches!(
            address_removed_event.await,
            finterfaces_admin::AddressRemovalReason::DadFailed
                | finterfaces_admin::AddressRemovalReason::UserRemoved
        );
    }
}

// Regression test for https://fxbug.dev/498241018. Racing a user address
// removal in NS3 using the interface `Control` after ASP detach with a
// network-initiated address removal caused a panic.
#[netstack_test]
async fn dad_failure_race_with_user_removal_detached(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    // We run this test only against NS3 because it's the stack that we're
    // keeping from regressing.
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    // Create a new network, new iface, and new fake-ep for each run to
    // prevent frame contamination from each loop.
    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm
        .join_network_with_if_config(
            &network,
            "client",
            netemul::InterfaceConfig { ipv6_dad_transmits: Some(u16::MAX), ..Default::default() },
        )
        .await
        .expect("join network");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");

    let ipv6_addr = netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR;

    // Wait for DAD NS.
    let mut stream = fake_ep
        .frame_stream()
        .map(|r| r.expect("frame error").0)
        .filter_map(|data| {
            let is_dad =
                packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                    net_types::ip::Ipv6,
                    _,
                    packet_formats::icmp::ndp::NeighborSolicitation,
                    _,
                >(
                    &data, packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck, |_| {}
                )
                .map_or(
                    false,
                    |(_src_mac, _dst_mac, _src_ip, _dst_ip, _ttl, ns, _code)| {
                        *ns.target_address() == ipv6_addr
                    },
                );
            futures::future::ready(is_dad.then_some(()))
        })
        .fuse();

    // Loop this a number of times so we exercise the racy condition.
    for _ in 0..30 {
        let (asp, asp_server_end) =
            fidl::endpoints::create_proxy::<finterfaces_admin::AddressStateProviderMarker>();
        asp.detach().expect("calling detach ASP");
        drop(asp);

        iface
            .control()
            .add_address(
                &fnet::Subnet { addr: IpAddr::V6(ipv6_addr).into_ext(), prefix_len: 64 },
                &finterfaces_admin::AddressParameters {
                    perform_dad: Some(true),
                    ..Default::default()
                },
                asp_server_end,
            )
            .expect("add address");

        stream.next().await.expect("wait for DAD NS");

        // Race a reply that causes DAD to fail with a remove request.
        netstack_testing_common::ndp::fail_dad_with_na(&fake_ep).await;
        let removed = iface
            .control()
            .remove_address(&fnet::Subnet {
                addr: IpAddr::V6(ipv6_addr).into_ext(),
                prefix_len: 64,
            })
            .await
            .expect("remove address");

        // Don't check the inner value because it differs depending on which arm
        // of the race wins.
        assert_matches!(removed, Ok(_));
    }
}

// Regression test for https://fxbug.dev/524633320. Racing a properties update
// with a Core-initiated address removal.
#[netstack_test]
async fn dad_failure_race_with_update_properties(name: &str) {
    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm = sandbox.create_netstack_realm::<Netstack3, _>(name).expect("create realm");

    let network = sandbox.create_network(name).await.expect("create network");
    let iface = realm
        .join_network_with_if_config(
            &network,
            "client",
            netemul::InterfaceConfig { ipv6_dad_transmits: Some(u16::MAX), ..Default::default() },
        )
        .await
        .expect("join network");
    let fake_ep = network.create_fake_endpoint().expect("create fake endpoint");

    let ipv6_addr = netstack_testing_common::constants::ipv6::LINK_LOCAL_ADDR;

    // Wait for DAD NS.
    let mut stream = fake_ep
        .frame_stream()
        .map(|r| r.expect("frame error").0)
        .filter_map(|data| {
            let is_dad =
                packet_formats::testutil::parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                    net_types::ip::Ipv6,
                    _,
                    packet_formats::icmp::ndp::NeighborSolicitation,
                    _,
                >(
                    &data, packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck, |_| {}
                )
                .map_or(
                    false,
                    |(_src_mac, _dst_mac, _src_ip, _dst_ip, _ttl, ns, _code)| {
                        *ns.target_address() == ipv6_addr
                    },
                );
            futures::future::ready(is_dad.then_some(()))
        })
        .fuse();

    // Try a number of times to draw out the race.
    for _ in 0..30 {
        let (asp, asp_server_end) =
            fidl::endpoints::create_proxy::<finterfaces_admin::AddressStateProviderMarker>();

        iface
            .control()
            .add_address(
                &fnet::Subnet { addr: IpAddr::V6(ipv6_addr).into_ext(), prefix_len: 64 },
                &finterfaces_admin::AddressParameters {
                    perform_dad: Some(true),
                    ..Default::default()
                },
                asp_server_end,
            )
            .expect("add address");

        let mut address_removed_event = asp
            .take_event_stream()
            .try_filter_map(|e| {
                futures::future::ok(match e {
                    finterfaces_admin::AddressStateProviderEvent::OnAddressRemoved { error } => {
                        Some(error)
                    }
                    finterfaces_admin::AddressStateProviderEvent::OnAddressAdded { .. } => None,
                })
            })
            .into_future()
            .map(|(r, _stream)| r.expect("stream ended without event").expect("FIDL error"));

        futures::select! {
            r = stream.next() => r.expect("wait for DAD NS"),
            e = address_removed_event => {
                assert_eq!(e, finterfaces_admin::AddressRemovalReason::DadFailed);
                continue;
            }
        }

        // Race a reply that causes DAD to fail with an update address properties request.
        netstack_testing_common::ndp::fail_dad_with_na(&fake_ep).await;

        let mut update_futures = Vec::new();
        for _ in 0..20 {
            update_futures.push(asp.update_address_properties(
                &finterfaces_admin::AddressProperties {
                    valid_lifetime_end: Some(123_000_000_000),
                    ..Default::default()
                },
            ));
        }

        let update_results = futures::future::join_all(update_futures).await;
        for update_result in update_results {
            match update_result {
                Ok(()) => {}
                Err(e) => {
                    assert!(e.is_closed(), "unexpected error: {:?}", e);
                }
            }
        }

        assert_eq!(address_removed_event.await, finterfaces_admin::AddressRemovalReason::DadFailed);
    }
}
