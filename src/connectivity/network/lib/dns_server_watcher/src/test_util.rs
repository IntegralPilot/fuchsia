// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test utilities.

/// Useful constants for tests.
pub(crate) mod constants {
    use fidl_fuchsia_net as fnet;
    use fidl_fuchsia_net_name as fname;

    use net_declare::fidl_socket_addr;

    pub(crate) const UNSPECIFIED_SOURCE_SOCKADDR: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.1:53");

    pub(crate) fn unspecified_source_server() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(UNSPECIFIED_SOURCE_SOCKADDR),
            source: None,
            ..Default::default()
        }
    }

    pub(crate) const STATIC_SOURCE_SOCKADDR: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.2:53");

    pub(crate) fn static_server() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(STATIC_SOURCE_SOCKADDR),
            source: Some(fname::DnsServerSource::StaticSource(
                fname::StaticDnsServerSource::default(),
            )),
            ..Default::default()
        }
    }

    pub(crate) const NDP_SOURCE_SOCKADDR: fnet::SocketAddress =
        fidl_socket_addr!("[2001:db8::1111]:53");

    pub(crate) const NDP_SERVER_INTERFACE_ID: u64 = 2;

    pub(crate) fn ndp_server() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(NDP_SOURCE_SOCKADDR),
            source: Some(fname::DnsServerSource::Ndp(fname::NdpDnsServerSource {
                source_interface: Some(NDP_SERVER_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const DHCPV4_SOURCE_SOCKADDR1: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.3:53");

    pub(crate) const DHCPV4_SERVER1_INTERFACE_ID: u64 = 3;

    pub(crate) fn dhcpv4_server1() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(DHCPV4_SOURCE_SOCKADDR1),
            source: Some(fname::DnsServerSource::Dhcp(fname::DhcpDnsServerSource {
                source_interface: Some(DHCPV4_SERVER1_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const DHCPV4_SOURCE_SOCKADDR2: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.4:53");

    pub(crate) const DHCPV4_SERVER2_INTERFACE_ID: u64 = 4;

    pub(crate) fn dhcpv4_server2() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(DHCPV4_SOURCE_SOCKADDR2),
            source: Some(fname::DnsServerSource::Dhcp(fname::DhcpDnsServerSource {
                source_interface: Some(DHCPV4_SERVER2_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const DHCPV6_SOURCE_SOCKADDR1: fnet::SocketAddress =
        fidl_socket_addr!("[2001:db8::2222]:53");

    pub(crate) const DHCPV6_SERVER1_INTERFACE_ID: u64 = 3;

    pub(crate) fn dhcpv6_server1() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(DHCPV6_SOURCE_SOCKADDR1),
            source: Some(fname::DnsServerSource::Dhcpv6(fname::Dhcpv6DnsServerSource {
                source_interface: Some(DHCPV6_SERVER1_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const DHCPV6_SOURCE_SOCKADDR2: fnet::SocketAddress =
        fidl_socket_addr!("[2001:db8::3333]:53");

    pub(crate) const DHCPV6_SERVER2_INTERFACE_ID: u64 = 4;

    pub(crate) fn dhcpv6_server2() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(DHCPV6_SOURCE_SOCKADDR2),
            source: Some(fname::DnsServerSource::Dhcpv6(fname::Dhcpv6DnsServerSource {
                source_interface: Some(DHCPV6_SERVER2_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const SOCKETPROXY_SOURCE_SOCKADDR1: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.5:53");

    pub(crate) const SOCKETPROXY_SERVER1_INTERFACE_ID: u64 = 5;

    pub(crate) fn socketproxy_server1() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(SOCKETPROXY_SOURCE_SOCKADDR1),
            source: Some(fname::DnsServerSource::SocketProxy(fname::SocketProxyDnsServerSource {
                source_interface: Some(SOCKETPROXY_SERVER1_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    pub(crate) const SOCKETPROXY_SOURCE_SOCKADDR2: fnet::SocketAddress =
        fidl_socket_addr!("192.0.2.6:53");

    pub(crate) const SOCKETPROXY_SERVER2_INTERFACE_ID: u64 = 6;

    pub(crate) fn socketproxy_server2() -> fname::DnsServer_ {
        fname::DnsServer_ {
            address: Some(SOCKETPROXY_SOURCE_SOCKADDR2),
            source: Some(fname::DnsServerSource::SocketProxy(fname::SocketProxyDnsServerSource {
                source_interface: Some(SOCKETPROXY_SERVER2_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        }
    }
}
