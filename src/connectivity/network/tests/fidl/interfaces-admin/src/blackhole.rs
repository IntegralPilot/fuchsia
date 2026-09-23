// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_async::TimeoutExt as _;
use futures::TryStreamExt;
use netstack_testing_common::ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt};

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;

#[fuchsia::test]
async fn create_blackhole_interface() {
    const IF_NAME: &'static str = "blackholeif";

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>("create-blackhole").expect("create realm");
    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    let (control, control_server_end) =
        fnet_interfaces_ext::admin::Control::create_endpoints().expect("create proxy");
    installer
        .install_blackhole_interface(
            control_server_end,
            fidl_fuchsia_net_interfaces_admin::Options {
                name: Some(IF_NAME.to_string()),
                metric: None,
                ..Default::default()
            },
        )
        .expect("create blackhole interface");

    let iface_id = control.get_id().await.expect("get id");

    let interfaces_state = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces::StateMarker>()
        .expect("connect to protocol");
    let mut event_stream = std::pin::pin!(
        fnet_interfaces_ext::event_stream_from_state::<fnet_interfaces_ext::DefaultInterest>(
            &interfaces_state,
            Default::default(),
        )
        .expect("create watcher event stream")
    );

    let mut interface_state = fnet_interfaces_ext::existing(
        &mut event_stream,
        fnet_interfaces_ext::InterfaceState::Unknown(iface_id),
    )
    .await
    .expect("get interface state");

    {
        let properties = match &interface_state {
            fnet_interfaces_ext::InterfaceState::Known(
                fnet_interfaces_ext::PropertiesAndState { properties, state: () },
            ) => properties,
            fnet_interfaces_ext::InterfaceState::Unknown(id) => {
                panic!("failed to retrieve new interface with id {}", id)
            }
        };
        assert_eq!(
            properties,
            &fnet_interfaces_ext::Properties {
                id: iface_id.try_into().expect("should be nonzero"),
                name: IF_NAME.to_string(),
                port_class: fnet_interfaces_ext::PortClass::Blackhole,
                online: false,
                addresses: vec![],
                has_default_ipv4_route: false,
                has_default_ipv6_route: false,
                port_identity_koid: None,
            }
        );
    }

    assert!(
        control
            .enable()
            .await
            .expect("should not get terminal event")
            .expect("should enable successfully")
    );

    fnet_interfaces_ext::wait_interface_with_id(
        &mut event_stream,
        &mut interface_state,
        |properties_and_state| properties_and_state.properties.online.then_some(()),
    )
    .on_timeout(fasync::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT), || {
        panic!("interface should come online before timeout")
    })
    .await
    .expect("should not get error waiting for interface");

    const ADDED_ADDRESS_AND_SUBNET: fnet::Subnet = net_declare::fidl_subnet!("192.168.0.1/24");

    let (_address_state_provider, server_end) =
        fidl::endpoints::create_proxy::<fnet_interfaces_admin::AddressStateProviderMarker>();
    control
        .add_address(
            &ADDED_ADDRESS_AND_SUBNET,
            &fnet_interfaces_admin::AddressParameters::default(),
            server_end,
        )
        .expect("should not get FIDL error");

    let addresses = fnet_interfaces_ext::wait_interface_with_id(
        &mut event_stream,
        &mut interface_state,
        |properties_and_state| {
            Some(
                properties_and_state
                    .properties
                    .addresses
                    .iter()
                    .map(|addr| addr.addr)
                    .collect::<Vec<_>>(),
            )
            .filter(|addresses| !addresses.is_empty())
        },
    )
    .on_timeout(fasync::MonotonicInstant::after(ASYNC_EVENT_POSITIVE_CHECK_TIMEOUT), || {
        panic!("interface should be assigned addresses before timeout")
    })
    .await
    .expect("should not get error waiting for interface");

    assert_eq!(addresses, vec![ADDED_ADDRESS_AND_SUBNET]);
}

// Tests that when creating a blackhole interface fails, the control handle is closed
// and a terminal event with the corresponding error is sent.
#[fuchsia::test]
async fn install_blackhole_interface_failure() {
    const IF_NAME: &'static str = "blackholeif";

    let sandbox = netemul::TestSandbox::new().expect("create sandbox");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>("blackhole-failure").expect("create realm");

    let installer = realm
        .connect_to_protocol::<fidl_fuchsia_net_interfaces_admin::InstallerMarker>()
        .expect("connect to protocol");

    // Create a blackhole interface.
    let (control1, control_server_end1) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::ControlMarker>();

    installer
        .install_blackhole_interface(
            control_server_end1,
            fidl_fuchsia_net_interfaces_admin::Options {
                name: Some(IF_NAME.to_string()),
                ..fidl_fuchsia_net_interfaces_admin::Options::default()
            },
        )
        .expect("install_blackhole_interface FIDL error");

    // Verify that the interface was installed.
    let _iface_id = control1.get_id().await.expect("get_id FIDL error");

    // Try to install another blackhole interface with the same name; this should fail.
    let (control2, control_server_end2) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces_admin::ControlMarker>();
    installer
        .install_blackhole_interface(
            control_server_end2,
            fidl_fuchsia_net_interfaces_admin::Options {
                name: Some(IF_NAME.to_string()),
                ..fidl_fuchsia_net_interfaces_admin::Options::default()
            },
        )
        .expect("install_blackhole_interface FIDL error");

    // Check that the second control handle is closed with a terminal event.
    let event = control2
        .take_event_stream()
        .map_ok(|fidl_fuchsia_net_interfaces_admin::ControlEvent::OnInterfaceRemoved { reason }| {
            reason
        })
        .try_next()
        .await
        .expect("error getting next event")
        .expect("expected an event");

    assert_eq!(event, fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason::DuplicateName);
}
