// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines shareable address constants for use in tests.

use net_declare::{net_ip_v4, net_ip_v6, net_mac, net_subnet_v4, net_subnet_v6};
use net_types::ethernet::Mac;
use net_types::ip::{
    GenericOverIp, Ip, IpAddress, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Ipv6Scope, Subnet,
};
use net_types::{
    MulticastAddr, NonMappedAddr, SpecifiedAddr, UnicastAddr, UnicastAddress, Witness as _,
};

use crate::device::address::AssignedAddrIpExt;
use crate::socket::DualStackIpExt;

/// An extension trait for `Ip` providing test-related functionality.
///
/// This trait acts as a marker for [`TestIpExt`] for both `Self` and
/// `Self::OtherVersion`.
pub trait TestDualStackIpExt: TestIpExt + DualStackIpExt<OtherVersion: TestIpExt> {}

impl<I> TestDualStackIpExt for I where I: TestIpExt + DualStackIpExt<OtherVersion: TestIpExt> {}

/// An extension trait for `Ip` providing test-related functionality.
pub trait TestIpExt: Ip + packet_formats::ip::IpExt + AssignedAddrIpExt {
    /// An instance of [`TestAddrs`] providing addresses for use in tests.
    const TEST_ADDRS: TestAddrs<Self::Addr>;

    /// Get an IP address in the same subnet as `Self::TEST_ADDRS`.
    ///
    /// `last` is the value to be put in the last octet of the IP address.
    fn get_other_ip_address(last: u8) -> SpecifiedAddr<Self::Addr>;

    /// Get an IP address in a different subnet from `Self::TEST_ADDRS`.
    ///
    /// `last` is the value to be put in the last octet of the IP address.
    fn get_other_remote_ip_address(last: u8) -> SpecifiedAddr<Self::Addr>;

    /// Get a multicast IP address.
    ///
    /// `last` is the value to be put in the last octet of the IP address.
    fn get_multicast_addr(last: u8) -> MulticastAddr<Self::Addr>;
}

impl TestIpExt for Ipv4 {
    const TEST_ADDRS: TestAddrs<Ipv4Addr> = TEST_ADDRS_V4;

    fn get_other_ip_address(last: u8) -> SpecifiedAddr<Ipv4Addr> {
        let mut bytes = Self::TEST_ADDRS.local_ip.get().ipv4_bytes();
        bytes[bytes.len() - 1] = last;
        SpecifiedAddr::new(Ipv4Addr::new(bytes)).unwrap()
    }

    fn get_other_remote_ip_address(last: u8) -> SpecifiedAddr<Self::Addr> {
        let mut bytes = Self::TEST_ADDRS.local_ip.get().ipv4_bytes();
        bytes[0] += 1;
        bytes[bytes.len() - 1] = last;
        SpecifiedAddr::new(Ipv4Addr::new(bytes)).unwrap()
    }

    fn get_multicast_addr(last: u8) -> MulticastAddr<Self::Addr> {
        assert!(u32::from(Self::Addr::BYTES * 8 - Self::MULTICAST_SUBNET.prefix()) > u8::BITS);
        let mut bytes = Self::MULTICAST_SUBNET.network().ipv4_bytes();
        bytes[bytes.len() - 1] = last;
        MulticastAddr::new(Ipv4Addr::new(bytes)).unwrap()
    }
}

impl TestIpExt for Ipv6 {
    const TEST_ADDRS: TestAddrs<Ipv6Addr> = TEST_ADDRS_V6;

    fn get_other_ip_address(last: u8) -> SpecifiedAddr<Ipv6Addr> {
        let mut bytes = Self::TEST_ADDRS.local_ip.get().ipv6_bytes();
        bytes[bytes.len() - 1] = last;
        SpecifiedAddr::new(Ipv6Addr::from(bytes)).unwrap()
    }

    fn get_other_remote_ip_address(last: u8) -> SpecifiedAddr<Self::Addr> {
        let mut bytes = Self::TEST_ADDRS.local_ip.get().ipv6_bytes();
        bytes[0] += 1;
        bytes[bytes.len() - 1] = last;
        SpecifiedAddr::new(Ipv6Addr::from(bytes)).unwrap()
    }

    fn get_multicast_addr(last: u8) -> MulticastAddr<Self::Addr> {
        assert!((Self::Addr::BYTES * 8 - Self::MULTICAST_SUBNET.prefix()) as u32 > u8::BITS);
        let mut bytes = Self::MULTICAST_SUBNET.network().ipv6_bytes();
        // Don't use the reserved (0) scope for these addresses.
        bytes[1] = Ipv6Scope::MULTICAST_SCOPE_ID_GLOBAL;
        bytes[bytes.len() - 1] = last;
        MulticastAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap()
    }
}

/// A configuration for a simple network.
///
/// `TestAddrs` describes a simple network with two IP hosts
/// - one remote and one local - both on the same Ethernet network.
#[derive(Clone, GenericOverIp)]
#[generic_over_ip(A, IpAddress)]
pub struct TestAddrs<A: IpAddress> {
    /// The subnet of the local Ethernet network.
    pub subnet: Subnet<A>,
    /// The IP address of our interface to the local network (must be in
    /// subnet).
    pub local_ip: SpecifiedAddr<A>,
    /// The MAC address of our interface to the local network.
    pub local_mac: UnicastAddr<Mac>,
    /// The remote host's IP address (must be in subnet if provided).
    pub remote_ip: SpecifiedAddr<A>,
    /// The remote host's MAC address.
    pub remote_mac: UnicastAddr<Mac>,
}

const LOCAL_MAC: UnicastAddr<Mac> =
    unsafe { UnicastAddr::new_unchecked(net_mac!("00:01:02:03:04:05")) };
const REMOTE_MAC: UnicastAddr<Mac> =
    unsafe { UnicastAddr::new_unchecked(net_mac!("06:07:08:09:0A:0B")) };

/// A `TestAddrs` with reasonable values for an IPv4 network.
pub const TEST_ADDRS_V4: TestAddrs<Ipv4Addr> = TestAddrs {
    subnet: net_subnet_v4!("192.0.2.0/24"),
    local_ip: unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.0.2.1")) },
    local_mac: LOCAL_MAC,
    remote_ip: unsafe { SpecifiedAddr::new_unchecked(net_ip_v4!("192.0.2.2")) },
    remote_mac: REMOTE_MAC,
};

/// A `TestAddrs` with reasonable values for an IPv6 network.
pub const TEST_ADDRS_V6: TestAddrs<Ipv6Addr> = TestAddrs {
    subnet: net_subnet_v6!("2001:db8::/32"),
    local_ip: unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("2001:db8::1")) },
    local_mac: LOCAL_MAC,
    remote_ip: unsafe { SpecifiedAddr::new_unchecked(net_ip_v6!("2001:db8::2")) },
    remote_mac: REMOTE_MAC,
};

impl<A: IpAddress> TestAddrs<A> {
    /// Creates a copy of `self` with all the remote and local fields reversed.
    pub fn swap(&self) -> Self {
        Self {
            subnet: self.subnet,
            local_ip: self.remote_ip,
            local_mac: self.remote_mac,
            remote_ip: self.local_ip,
            remote_mac: self.local_mac,
        }
    }

    /// Gets the local address with the non mapped and unicast witnesses
    /// applied.
    pub fn local_non_mapped_unicast(&self) -> NonMappedAddr<UnicastAddr<A>>
    where
        A: UnicastAddress,
    {
        NonMappedAddr::new(UnicastAddr::try_from(self.local_ip).unwrap()).unwrap()
    }

    /// Gets the remote address with the non mapped and unicast witnesses
    /// applied.
    pub fn remote_non_mapped_unicast(&self) -> NonMappedAddr<UnicastAddr<A>>
    where
        A: UnicastAddress,
    {
        NonMappedAddr::new(UnicastAddr::try_from(self.remote_ip).unwrap()).unwrap()
    }
}
