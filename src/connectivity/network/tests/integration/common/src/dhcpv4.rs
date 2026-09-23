// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for interacting with DHCPv4 client/server during integration
//! tests.

#![warn(missing_docs)]

use std::ops::Range;

use dhcpv4::protocol::IntoFidlExt as _;

use futures::StreamExt as _;
use net_declare::net::prefix_length_v4;
use net_types::ip::{Ipv4, PrefixLength};

/// Encapsulates a minimal configuration needed to test a DHCP client/server combination.
pub struct TestConfig {
    /// Server IP address.
    pub server_addr: fidl_fuchsia_net::Ipv4Address,

    /// Address pool for the DHCP server.
    pub managed_addrs: dhcpv4::configuration::ManagedAddresses,
}

impl TestConfig {
    /// Given offsets for the server's own address and the managed address pool,
    /// constructs a `TestConfig` for a DHCP server managing addresses in the
    /// 192.168.0.0/25 subnet.
    pub const fn new(server_addr_offset: u8, pool_offsets: Range<u8>) -> Self {
        const fn from_offset(offset: u8) -> std::net::Ipv4Addr {
            std::net::Ipv4Addr::new(192, 168, 0, offset)
        }

        let Range { start, end } = pool_offsets;

        let max_offset =
            1u8 << 32u8.checked_sub(DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH.get()).unwrap();

        assert!(
            server_addr_offset < max_offset,
            "server_addr_offset must fit within default address pool prefix"
        );
        assert!(pool_offsets.start < pool_offsets.end, "pool_offsets start must be less than end");
        assert!(
            pool_offsets.start < max_offset,
            "pool_offsets.start must fit within default address pool prefix"
        );
        assert!(
            pool_offsets.end < max_offset,
            "pool_offsets.end must fit within default address pool prefix"
        );

        let server_addr =
            fidl_fuchsia_net::Ipv4Address { addr: from_offset(server_addr_offset).octets() };
        let pool_range_start = from_offset(start);
        let pool_range_stop = from_offset(end);

        Self {
            server_addr,
            managed_addrs: dhcpv4::configuration::ManagedAddresses {
                mask: dhcpv4::configuration::SubnetMask::new(
                    DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH,
                ),
                pool_range_start,
                pool_range_stop,
            },
        }
    }

    /// The IPv4 address a client will acquire from the server.
    pub fn expected_acquired(&self) -> fidl_fuchsia_net::Subnet {
        let Self {
            server_addr: _,
            managed_addrs:
                dhcpv4::configuration::ManagedAddresses { mask, pool_range_start, pool_range_stop: _ },
        } = self;
        fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv4(pool_range_start.into_fidl()),
            prefix_len: mask.ones(),
        }
    }

    /// The IPv4 address and prefix the server will assign to itself.
    pub fn server_addr_with_prefix(&self) -> fidl_fuchsia_net::Ipv4AddressWithPrefix {
        let Self {
            server_addr,
            managed_addrs:
                dhcpv4::configuration::ManagedAddresses {
                    mask,
                    pool_range_start: _,
                    pool_range_stop: _,
                },
        } = self;
        fidl_fuchsia_net::Ipv4AddressWithPrefix { addr: *server_addr, prefix_len: mask.ones() }
    }

    /// The FIDL parameters a DHCPv4 server should be configured with.
    pub fn dhcp_parameters(&self) -> Vec<fidl_fuchsia_net_dhcp::Parameter> {
        let Self { server_addr, managed_addrs } = self;
        vec![
            fidl_fuchsia_net_dhcp::Parameter::IpAddrs(vec![*server_addr]),
            fidl_fuchsia_net_dhcp::Parameter::AddressPool(managed_addrs.into_fidl()),
        ]
    }
}

/// Default prefix length of default configuration's address pool.
pub const DEFAULT_TEST_ADDRESS_POOL_PREFIX_LENGTH: PrefixLength<Ipv4> = prefix_length_v4!(25);

/// Default configuration.
pub const DEFAULT_TEST_CONFIG: TestConfig = TestConfig::new(1, 2..5);

/// Set DHCPv4 server settings.
pub async fn set_server_settings(
    dhcp_server: &fidl_fuchsia_net_dhcp::Server_Proxy,
    parameters: impl IntoIterator<Item = fidl_fuchsia_net_dhcp::Parameter>,
    options: impl IntoIterator<Item = fidl_fuchsia_net_dhcp::Option_>,
) {
    let parameters = futures::stream::iter(parameters.into_iter()).for_each_concurrent(
        None,
        |parameter| async move {
            dhcp_server
                .set_parameter(&parameter)
                .await
                .expect("failed to call dhcp/Server.SetParameter")
                .map_err(zx::Status::err_from_raw)
                .unwrap_or_else(|e| {
                    panic!("dhcp/Server.SetParameter({:?}) returned error: {:?}", parameter, e)
                })
        },
    );
    let options =
        futures::stream::iter(options.into_iter()).for_each_concurrent(None, |option| async move {
            dhcp_server
                .set_option(&option)
                .await
                .expect("failed to call dhcp/Server.SetOption")
                .map_err(zx::Status::err_from_raw)
                .unwrap_or_else(|e| {
                    panic!("dhcp/Server.SetOption({:?}) returned error: {:?}", option, e)
                })
        });
    let ((), ()) = futures::future::join(parameters, options).await;
}
