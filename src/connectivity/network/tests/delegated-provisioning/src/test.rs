// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A test that verifies the client interface, whose provisioning is delegated,
//! has been assigned a DHCP address.

#![cfg(test)]

use std::collections::HashMap;

use delegated_provisioning_constants as constants;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsia_async as fasync;
use futures::StreamExt as _;
use log::{info, warn};

const START_LOGGING_WARNINGS_AFTER: std::time::Duration = std::time::Duration::from_secs(3 * 60);
const WARNING_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

#[fuchsia::test]
async fn delegated_provisioning_test() {
    info!("test starting...");

    let state_proxy =
        fuchsia_component::client::connect_to_protocol::<fnet_interfaces::StateMarker>()
            .expect("Failed to connect to State proxy");
    let event_stream = fnet_interfaces_ext::event_stream_from_state::<
        fnet_interfaces_ext::DefaultInterest,
    >(&state_proxy, Default::default())
    .expect("failed to get interface event stream");

    info!(
        "waiting for interface ({}) to be assigned address ({:?})...",
        constants::CLIENT_IFACE_NAME,
        constants::DHCP_DYNAMIC_IP
    );

    let _log_warning_if_taking_too_log_task = fasync::Task::spawn(async {
        // NB: Don't move the timer into the `futures::stream::once` as we want to defer actually
        // invoking `fasync::Interval::new` until after the initial timer expires.
        fasync::Timer::new(START_LOGGING_WARNINGS_AFTER).await;
        futures::stream::once(futures::future::ready(()))
            .chain(fasync::Interval::new(WARNING_LOG_INTERVAL.into()))
            .for_each(|()| async move {
                warn!(
                    "still waiting for interface ({}) to be assigned address ({:?})...",
                    constants::CLIENT_IFACE_NAME,
                    constants::DHCP_DYNAMIC_IP
                );
                warn!(
                    "NOTE FOR TRIAGERS: this test simply waits for the DhcpClient of the product \
                    running in the container to successfully acquire an IPv4 address.\n\
                    This means that if the session running in the starnix container is unable \
                    to start up properly, this test will fail via timing out.\n\
                    Please check whether other end-to-end tests for your product indicate failure\n\
                    to start up properly before assuming that this is failing due to specifically\n\
                    a networking issue."
                );
            })
            .await;
    });

    let mut interfaces = HashMap::<u64, fnet_interfaces_ext::PropertiesAndState<(), _>>::new();
    fnet_interfaces_ext::wait_interface(event_stream, &mut interfaces, |interfaces| {
        info!("Observed change on interfaces watcher. Current State: {:?}", interfaces);
        interfaces
            .values()
            .any(
                |fnet_interfaces_ext::PropertiesAndState {
                     properties: fnet_interfaces_ext::Properties { name, addresses, .. },
                     state: _,
                 }| {
                    name == constants::CLIENT_IFACE_NAME
                        && addresses.iter().any(
                            |&fnet_interfaces_ext::Address {
                                 addr: fnet::Subnet { addr, prefix_len: _ },
                                 valid_until: _,
                                 assignment_state: _,
                                 preferred_lifetime_info: _,
                             }| match addr {
                                fnet::IpAddress::Ipv4(addr) => addr == constants::DHCP_DYNAMIC_IP,
                                fnet::IpAddress::Ipv6(_) => false,
                            },
                        )
                },
            )
            .then_some(())
    })
    .await
    .expect("interface event stream unexpectedly closed");

    info!(
        "observed interface ({}) with address ({:?})",
        constants::CLIENT_IFACE_NAME,
        constants::DHCP_DYNAMIC_IP
    );
}
