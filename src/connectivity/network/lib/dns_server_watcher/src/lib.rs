// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DNS Server watcher.

mod stream;
#[cfg(test)]
mod test_util;

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use fidl_fuchsia_net::SocketAddress;
use fidl_fuchsia_net_name::{
    DhcpDnsServerSource, Dhcpv6DnsServerSource, DnsServer_, DnsServerSource, NdpDnsServerSource,
    SocketProxyDnsServerSource, StaticDnsServerSource,
};

pub use self::stream::*;

/// The default DNS server port.
pub const DEFAULT_DNS_PORT: u16 = 53;

/// The DNS servers learned from all sources.
#[derive(Debug, Default)]
pub struct DnsServers {
    /// DNS servers obtained from some default configurations.
    ///
    /// These servers will have the lowest priority of all servers.
    default: Vec<DnsServer_>,

    /// DNS servers obtained from the netstack.
    netstack: Vec<DnsServer_>,

    /// DNS servers obtained from DHCPv4 clients.
    dhcpv4: HashMap<u64, Vec<DnsServer_>>,

    /// DNS servers obtained from DHCPv6 clients.
    dhcpv6: HashMap<u64, Vec<DnsServer_>>,

    /// DNS servers obtained from NDP clients.
    ndp: HashMap<u64, Vec<DnsServer_>>,

    /// DNS servers obtained from the socketproxy.
    socketproxy: Vec<DnsServer_>,
}

impl DnsServers {
    /// Sets the DNS servers discovered from `source`.
    // TODO(https://fxbug.dev/42133326): Make sure `servers` only contain servers that could be obtained
    // from `source`.
    pub fn set_servers_from_source(
        &mut self,
        source: DnsServersUpdateSource,
        servers: Vec<DnsServer_>,
    ) {
        let Self { default, netstack, dhcpv4, dhcpv6, ndp, socketproxy } = self;

        match source {
            DnsServersUpdateSource::Default => *default = servers,
            DnsServersUpdateSource::Netstack => *netstack = servers,
            DnsServersUpdateSource::Dhcpv4 { interface_id } => {
                // We discard existing servers since they are being replaced with
                // `servers` - the old servers are useless to us now.
                let _: Option<Vec<DnsServer_>> = if servers.is_empty() {
                    dhcpv4.remove(&interface_id)
                } else {
                    dhcpv4.insert(interface_id, servers)
                };
            }
            DnsServersUpdateSource::Dhcpv6 { interface_id } => {
                // We discard existing servers since they are being replaced with
                // `servers` - the old servers are useless to us now.
                let _: Option<Vec<DnsServer_>> = if servers.is_empty() {
                    dhcpv6.remove(&interface_id)
                } else {
                    dhcpv6.insert(interface_id, servers)
                };
            }
            DnsServersUpdateSource::Ndp { interface_id } => {
                // We discard existing servers since they are being replaced with
                // `servers` - the old servers are useless to us now.
                let _: Option<Vec<DnsServer_>> = if servers.is_empty() {
                    ndp.remove(&interface_id)
                } else {
                    ndp.insert(interface_id, servers)
                };
            }
            DnsServersUpdateSource::SocketProxy => *socketproxy = servers,
        }
    }

    /// Returns a consolidated list of server addresses.
    ///
    /// The servers will be returned deduplicated by their address and sorted by the source
    /// that each server was learned from. The servers will be sorted in most to least
    /// preferred order, with the most preferred server first. The preference of the servers
    /// is NDP, DHCPv4, DHCPv6 then Static, where NDP is the most preferred. No ordering is
    /// guaranteed across servers from different sources of the same source-kind, but ordering
    /// within a source is maintained.
    ///
    /// Example, say we had servers SA1 and SA2 set for DHCPv6 interface A, and SB1 and SB2
    /// for DHCPv6 interface B. The consolidated list will be either one of [SA1, SA2, SB1, SB2]
    /// or [SB1, SB2, SA1, SA2]. Notice how the ordering across sources (DHCPv6 interface A vs
    /// DHCPv6 interface B) is not fixed, but the ordering within the source ([SA1, SA2] for DHCPv6
    /// interface A and [SB1, SB2] for DHCPv6 interface B) is maintained.
    ///
    /// Note, if multiple `DnsServer_`s have the same address but different sources, only
    /// the `DnsServer_` with the most preferred source will be present in the consolidated
    /// list of servers.
    // TODO(https://fxbug.dev/42133571): Consider ordering across sources of the same source-kind based on some
    // metric.
    pub fn consolidated(&self) -> Vec<SocketAddress> {
        self.consolidate_filter_map(|x| x.address)
    }

    /// Returns a consolidated list of [`DnsServer_`] structs.
    ///
    /// The servers will be returned deduplicated by their address and sorted by the source
    /// that each server was learned from. The servers will be sorted in most to least
    /// preferred order, with the most preferred server first. See doc comment on Self::ordering
    /// for order of preference based on ServerSource.
    ///
    /// Example, say we had servers SA1 and SA2 set for DHCPv6 interface A, and SB1 and SB2
    /// for DHCPv6 interface B. The consolidated list will be either one of [SA1, SA2, SB1, SB2]
    /// or [SB1, SB2, SA1, SA2]. Notice how the ordering across sources (DHCPv6 interface A vs
    /// DHCPv6 interface B) is not fixed, but the ordering within the source ([SA1, SA2] for DHCPv6
    /// interface A and [SB1, SB2] for DHCPv6 interface B) is maintained.
    ///
    /// Note, if multiple `DnsServer_`s have the same address but different sources, only
    /// the `DnsServer_` with the most preferred source will be present in the consolidated
    /// list of servers.
    // TODO(https://fxbug.dev/42133571): Consider ordering across sources of the
    // same source-kind based on some metric.
    pub fn consolidated_dns_servers(&self) -> Vec<DnsServer_> {
        self.consolidate_filter_map(|x| Some(x))
    }

    /// Returns a consolidated list of servers, with the mapping function `f` applied.
    ///
    /// See `consolidated` for details on ordering.
    fn consolidate_filter_map<T, F: Fn(DnsServer_) -> Option<T>>(&self, f: F) -> Vec<T> {
        let Self { default, netstack, dhcpv4, dhcpv6, ndp, socketproxy } = self;
        let mut servers = netstack
            .iter()
            .chain(socketproxy)
            .chain(dhcpv4.values().flatten())
            .chain(ndp.values().flatten())
            .chain(dhcpv6.values().flatten())
            .cloned()
            .collect::<Vec<_>>();
        // Sorting happens before deduplication to ensure that when multiple sources report the same
        // address, the highest priority source wins.
        //
        // `sort_by` maintains the order of equal elements. This is required to maintain ordering
        // within a source of DNS servers. `sort_unstable_by` may not preserve this ordering.
        servers.sort_by(Self::ordering);
        // Default servers are considered to have the lowest priority so we append them to the end
        // of the list of sorted dynamically learned servers.
        servers.extend(default.clone());
        let mut addresses = HashSet::new();
        servers.retain(move |s| addresses.insert(s.address));
        servers.into_iter().filter_map(f).collect()
    }

    /// Returns the ordering of [`DnsServer_`]s.
    ///
    /// The ordering from most to least preferred is is Socketproxy, DHCP, NDP, DHCPv6, then
    /// Static. Preference is currently informed by a goal of keeping servers discovered via the
    /// same internet protocol together and preferring network-supplied configurations over
    /// product-supplied ones. Socketproxy DNS servers discovered on a Socketproxy-enabled product
    /// are provided when the current default network was provisioned by an agent other
    /// than Fuchsia.
    ///
    /// TODO(https://fxbug.dev/42125772): We currently prioritize IPv4 servers over IPv6 as our
    /// DHCPv6 client does not yet support stateful address assignment. This can result in
    /// situations where we can discover v6 nameservers that can't be reached as we've not
    /// discovered a global address.
    ///
    /// An unspecified source will be treated as a static address.
    fn ordering(a: &DnsServer_, b: &DnsServer_) -> Ordering {
        let ordering = |source| match source {
            Some(&DnsServerSource::SocketProxy(SocketProxyDnsServerSource {
                source_interface: _,
                ..
            })) => 0,
            Some(&DnsServerSource::Dhcp(DhcpDnsServerSource { source_interface: _, .. })) => 1,
            Some(&DnsServerSource::Ndp(NdpDnsServerSource { source_interface: _, .. })) => 2,
            Some(&DnsServerSource::Dhcpv6(Dhcpv6DnsServerSource {
                source_interface: _, ..
            })) => 3,
            Some(&DnsServerSource::StaticSource(StaticDnsServerSource { .. })) => 4,
            Some(&DnsServerSource::__SourceBreaking { .. }) | None => 5,
        };
        let a = ordering(a.source.as_ref());
        let b = ordering(b.source.as_ref());
        std::cmp::Ord::cmp(&a, &b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::constants::*;

    #[test]
    fn deduplicate_within_source() {
        // Simple deduplication and sorting of repeated `DnsServer_`.
        let servers = DnsServers {
            default: vec![ndp_server(), ndp_server()],
            netstack: vec![ndp_server(), static_server(), ndp_server(), static_server()],
            // `DHCPV4/6_SERVER2` would normally only come from an interface with ID
            // `DHCPV4/6_SERVER2_INTERFACE_ID`, but we are just testing deduplication
            // logic here.
            dhcpv4: [
                (DHCPV4_SERVER1_INTERFACE_ID, vec![dhcpv4_server1(), dhcpv4_server2()]),
                (DHCPV4_SERVER2_INTERFACE_ID, vec![dhcpv4_server1(), dhcpv4_server2()]),
            ]
            .into_iter()
            .collect(),
            dhcpv6: [
                (DHCPV6_SERVER1_INTERFACE_ID, vec![dhcpv6_server1(), dhcpv6_server2()]),
                (DHCPV6_SERVER2_INTERFACE_ID, vec![dhcpv6_server1(), dhcpv6_server2()]),
            ]
            .into_iter()
            .collect(),
            ndp: [(NDP_SERVER_INTERFACE_ID, vec![ndp_server(), ndp_server()])]
                .into_iter()
                .collect(),
            socketproxy: vec![socketproxy_server1(), socketproxy_server2(), socketproxy_server1()],
        };
        // Ordering across (the DHCPv6) sources is not guaranteed, but both DHCPv6 sources
        // have the same set of servers with the same order. With deduplication, we know
        // we will only see one of the sources' servers.
        assert_eq!(
            servers.consolidated(),
            vec![
                SOCKETPROXY_SOURCE_SOCKADDR1,
                SOCKETPROXY_SOURCE_SOCKADDR2,
                DHCPV4_SOURCE_SOCKADDR1,
                DHCPV4_SOURCE_SOCKADDR2,
                NDP_SOURCE_SOCKADDR,
                DHCPV6_SOURCE_SOCKADDR1,
                DHCPV6_SOURCE_SOCKADDR2,
                STATIC_SOURCE_SOCKADDR,
            ],
        );
    }

    #[test]
    fn default_low_prio() {
        // Default servers should always have low priority, but if the same server
        // is observed by a higher priority source, then use the higher source for
        // ordering.
        let servers = DnsServers {
            default: vec![
                static_server(),
                dhcpv4_server1(),
                dhcpv6_server1(),
                socketproxy_server1(),
            ],
            netstack: vec![static_server()],
            dhcpv4: [
                (DHCPV4_SERVER1_INTERFACE_ID, vec![dhcpv4_server1()]),
                (DHCPV4_SERVER2_INTERFACE_ID, vec![dhcpv4_server1()]),
            ]
            .into_iter()
            .collect(),
            dhcpv6: [
                (DHCPV6_SERVER1_INTERFACE_ID, vec![dhcpv6_server1()]),
                (DHCPV6_SERVER2_INTERFACE_ID, vec![dhcpv6_server2()]),
            ]
            .into_iter()
            .collect(),
            ndp: [(NDP_SERVER_INTERFACE_ID, vec![ndp_server()])].into_iter().collect(),
            socketproxy: vec![socketproxy_server1()],
        };
        // No ordering is guaranteed across servers from different sources of the same
        // source-kind.
        let mut got = servers.consolidated();
        let mut got = got.drain(..);
        let want_socketproxy = [SOCKETPROXY_SOURCE_SOCKADDR1];
        assert_eq!(
            HashSet::from_iter(got.by_ref().take(want_socketproxy.len())),
            HashSet::from(want_socketproxy),
        );

        let want_dhcpv4 = [DHCPV4_SOURCE_SOCKADDR1];
        assert_eq!(
            HashSet::from_iter(got.by_ref().take(want_dhcpv4.len())),
            HashSet::from(want_dhcpv4),
        );

        let want_ndp = [NDP_SOURCE_SOCKADDR];
        assert_eq!(HashSet::from_iter(got.by_ref().take(want_ndp.len())), HashSet::from(want_ndp));

        let want_dhcpv6 = [DHCPV6_SOURCE_SOCKADDR1, DHCPV6_SOURCE_SOCKADDR2];
        assert_eq!(
            HashSet::from_iter(got.by_ref().take(want_dhcpv6.len())),
            HashSet::from(want_dhcpv6),
        );

        let want_rest = [STATIC_SOURCE_SOCKADDR];
        assert_eq!(got.as_slice(), want_rest);
    }

    #[test]
    fn deduplicate_across_sources() {
        // Deduplication and sorting of same address across different sources.

        // DHCPv6 is not as preferred as NDP so this should not be in the consolidated
        // servers list.
        let dhcpv6_with_ndp_address = || DnsServer_ {
            address: Some(NDP_SOURCE_SOCKADDR),
            source: Some(DnsServerSource::Dhcpv6(Dhcpv6DnsServerSource {
                source_interface: Some(DHCPV6_SERVER1_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut dhcpv6 = HashMap::new();
        assert_matches::assert_matches!(
            dhcpv6.insert(
                DHCPV6_SERVER1_INTERFACE_ID,
                vec![dhcpv6_with_ndp_address(), dhcpv6_server1()]
            ),
            None
        );
        let mut servers = DnsServers {
            default: vec![],
            netstack: vec![dhcpv4_server1(), static_server()],
            dhcpv4: [(DHCPV4_SERVER1_INTERFACE_ID, vec![dhcpv4_server1()])].into_iter().collect(),
            dhcpv6: [(
                DHCPV6_SERVER1_INTERFACE_ID,
                vec![dhcpv6_with_ndp_address(), dhcpv6_server1()],
            )]
            .into_iter()
            .collect(),
            ndp: [(NDP_SERVER_INTERFACE_ID, vec![ndp_server(), dhcpv6_with_ndp_address()])]
                .into_iter()
                .collect(),
            socketproxy: vec![],
        };
        let expected_servers =
            vec![dhcpv4_server1(), ndp_server(), dhcpv6_server1(), static_server()];
        assert_eq!(servers.consolidate_filter_map(Some), expected_servers);
        let expected_sockaddrs = vec![
            DHCPV4_SOURCE_SOCKADDR1,
            NDP_SOURCE_SOCKADDR,
            DHCPV6_SOURCE_SOCKADDR1,
            STATIC_SOURCE_SOCKADDR,
        ];
        assert_eq!(servers.consolidated(), expected_sockaddrs);
        servers.netstack = vec![dhcpv4_server1(), static_server(), dhcpv6_with_ndp_address()];
        assert_eq!(servers.consolidate_filter_map(Some), expected_servers);
        assert_eq!(servers.consolidated(), expected_sockaddrs);

        // NDP is more preferred than DHCPv6 so `dhcpv6_server1()` should not be in the
        // consolidated list of servers.
        let ndp_with_dhcpv6_sockaddr1 = || DnsServer_ {
            address: Some(DHCPV6_SOURCE_SOCKADDR1),
            source: Some(DnsServerSource::Ndp(NdpDnsServerSource {
                source_interface: Some(NDP_SERVER_INTERFACE_ID),
                ..Default::default()
            })),
            ..Default::default()
        };

        let mut dhcpv6 = HashMap::new();
        assert_matches::assert_matches!(
            dhcpv6.insert(DHCPV6_SERVER1_INTERFACE_ID, vec![dhcpv6_server1()]),
            None
        );
        assert_matches::assert_matches!(
            dhcpv6.insert(DHCPV6_SERVER2_INTERFACE_ID, vec![dhcpv6_server2()]),
            None
        );
        let mut servers = DnsServers {
            default: vec![],
            netstack: vec![static_server()],
            dhcpv4: Default::default(),
            dhcpv6,
            ndp: [(NDP_SERVER_INTERFACE_ID, vec![ndp_with_dhcpv6_sockaddr1()])]
                .into_iter()
                .collect(),
            socketproxy: vec![],
        };
        let expected_servers = vec![ndp_with_dhcpv6_sockaddr1(), dhcpv6_server2(), static_server()];
        assert_eq!(servers.consolidate_filter_map(Some), expected_servers);
        let expected_sockaddrs =
            vec![DHCPV6_SOURCE_SOCKADDR1, DHCPV6_SOURCE_SOCKADDR2, STATIC_SOURCE_SOCKADDR];
        assert_eq!(servers.consolidated(), expected_sockaddrs);
        servers.netstack = vec![static_server(), ndp_with_dhcpv6_sockaddr1()];
        assert_eq!(servers.consolidate_filter_map(Some), expected_servers);
        assert_eq!(servers.consolidated(), expected_sockaddrs);
    }

    #[test]
    fn test_dns_servers_ordering() {
        assert_eq!(
            DnsServers::ordering(&socketproxy_server1(), &socketproxy_server1()),
            Ordering::Equal
        );
        assert_eq!(DnsServers::ordering(&ndp_server(), &ndp_server()), Ordering::Equal);
        assert_eq!(DnsServers::ordering(&dhcpv4_server1(), &dhcpv4_server1()), Ordering::Equal);
        assert_eq!(DnsServers::ordering(&dhcpv6_server1(), &dhcpv6_server1()), Ordering::Equal);
        assert_eq!(DnsServers::ordering(&static_server(), &static_server()), Ordering::Equal);
        assert_eq!(
            DnsServers::ordering(&unspecified_source_server(), &unspecified_source_server()),
            Ordering::Equal
        );

        let mut servers = vec![
            unspecified_source_server(),
            dhcpv6_server1(),
            dhcpv4_server1(),
            static_server(),
            ndp_server(),
            socketproxy_server1(),
        ];
        servers.sort_by(DnsServers::ordering);
        assert_eq!(
            servers,
            vec![
                socketproxy_server1(),
                dhcpv4_server1(),
                ndp_server(),
                dhcpv6_server1(),
                static_server(),
                unspecified_source_server(),
            ]
        );
    }
}
