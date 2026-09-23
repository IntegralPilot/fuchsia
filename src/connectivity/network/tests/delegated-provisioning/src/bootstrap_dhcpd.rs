// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A minimal script to configure and start a DHCP server.

use delegated_provisioning_constants as constants;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use log::info;

/// Computes the IPv4 address immediately following the given address.
///
/// # Panics
///
/// Panics if the last byte of the given address is equal to [`u8::MAX`].
fn next_addr(fnet::Ipv4Address { addr: [b1, b2, b3, b4] }: fnet::Ipv4Address) -> fnet::Ipv4Address {
    let b4 = u8::checked_add(b4, 1).unwrap();
    fnet::Ipv4Address { addr: [b1, b2, b3, b4] }
}

#[fuchsia::main]
async fn main() {
    info!("bootstrap_dhcpd starting...");

    let dhcp_config = vec![
        fnet_dhcp::Parameter::IpAddrs(vec![constants::SERVER_STATIC_IP]),
        fnet_dhcp::Parameter::AddressPool(
            // Establish an address pool that contains exactly 1 address, which
            // makes it easier to assert that the address has been assigned.
            fnet_dhcp::AddressPool {
                prefix_length: Some(31),
                range_start: Some(constants::DHCP_DYNAMIC_IP),
                range_stop: Some(next_addr(constants::DHCP_DYNAMIC_IP)),
                ..fnet_dhcp::AddressPool::default()
            },
        ),
        fnet_dhcp::Parameter::BoundDeviceNames(vec![constants::SERVER_IFACE_NAME.to_string()]),
    ];

    let dhcp_server = fuchsia_component::client::connect_to_protocol::<fnet_dhcp::Server_Marker>()
        .expect("Failed to connect to DHCP server proxy");
    for param in dhcp_config {
        info!("setting dhcpd parameter: {:?}", param);
        dhcp_server
            .set_parameter(&param)
            .await
            .expect("failed to call fnet_dhcp::Server::set_parameter")
            .expect("failed to set parameter");
    }

    dhcp_server
        .set_option(&fnet_dhcp::Option_::Router(vec![constants::SERVER_STATIC_IP]))
        .await
        .expect("failed to call fnet_dhcp::Server::set_option")
        .expect("failed to set option");

    dhcp_server
        .start_serving()
        .await
        .expect("failed to call fnet_dhcp::Server::start")
        .expect("starting the DHCP server failed");

    info!("dhcpd serving started successfully");
    info!("bootstrap_dhcpd exiting.");
}
