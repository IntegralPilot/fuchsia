// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

use core::fmt::{Debug, Formatter};
use num_derive::FromPrimitive;

/// Represents a NAT64 Translator State.
///
/// Functional equivalent of [`otsys::otNat64State`](crate::otsys::otNat64State).
#[derive(Debug, Copy, Clone, Eq, Ord, PartialOrd, PartialEq, FromPrimitive)]
pub enum Nat64State {
    /// Functional equivalent of [`otsys::OT_NAT64_STATE_DISABLED`](crate::otsys::OT_NAT64_STATE_DISABLED).
    Disabled = OT_NAT64_STATE_DISABLED as isize,

    /// Functional equivalent of [`otsys::OT_NAT64_STATE_NOT_RUNNING`](crate::otsys::OT_NAT64_STATE_NOT_RUNNING).
    NotRunning = OT_NAT64_STATE_NOT_RUNNING as isize,

    /// Functional equivalent of [`otsys::OT_NAT64_STATE_IDLE`](crate::otsys::OT_NAT64_STATE_IDLE).
    Idle = OT_NAT64_STATE_IDLE as isize,

    /// Functional equivalent of [`otsys::OT_NAT64_STATE_ACTIVE`](crate::otsys::OT_NAT64_STATE_ACTIVE).
    Active = OT_NAT64_STATE_ACTIVE as isize,
}

impl From<otNat64State> for Nat64State {
    fn from(x: otNat64State) -> Self {
        use num::FromPrimitive;
        Self::from_u32(x).unwrap_or_else(|| panic!("Unknown otNat64State value: {x}"))
    }
}

impl From<Nat64State> for otNat64State {
    fn from(x: Nat64State) -> Self {
        x as otNat64State
    }
}

/// Data type representing an IPv4 CIDR.
///
/// Functional equivalent of [`otsys::otIp4Cidr`](crate::otsys::otIp4Cidr).
#[derive(Default, Clone, Copy)]
#[repr(transparent)]
pub struct Ip4Cidr(pub otIp4Cidr);

impl_ot_castable!(Ip4Cidr, otIp4Cidr);

impl Debug for Ip4Cidr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IpAddr:{}, PrefixLength:{}",
            std::net::Ipv4Addr::from(self.get_address_bytes()),
            self.get_length()
        )
    }
}

impl PartialEq for Ip4Cidr {
    fn eq(&self, other: &Ip4Cidr) -> bool {
        self.get_length() == other.get_length()
            && self.get_address_bytes() == other.get_address_bytes()
    }
}

impl Eq for Ip4Cidr {}

/// IPv4 address type
pub type Ip4Address = std::net::Ipv4Addr;

impl Ip4Cidr {
    /// create a new Ip4Cidr
    pub fn new(addr: [u8; 4], len: u8) -> Ip4Cidr {
        Ip4Cidr(otIp4Cidr {
            mAddress: otIp4Address { mFields: otIp4Address__bindgen_ty_1 { m8: addr } },
            mLength: len,
        })
    }

    /// Get the address of IPv4 CIDR
    pub fn get_address_bytes(&self) -> [u8; 4] {
        unsafe { self.0.mAddress.mFields.m8 }
    }

    /// Get the length of IPv4 CIDR
    pub fn get_length(&self) -> u8 {
        self.0.mLength
    }
}

#[derive(Default, Clone, Copy)]
#[repr(transparent)]
/// NAT64 Address Mapping, which is part of NAT64 telemetry
pub struct Nat64AddressMapping(pub otNat64AddressMapping);

impl_ot_castable!(Nat64AddressMapping, otNat64AddressMapping);

impl Nat64AddressMapping {
    /// Get NAT64 mapping ID
    pub fn get_mapping_id(&self) -> u64 {
        self.0.mId
    }

    /// Get IPv4 address
    pub fn get_ipv4_addr(&self) -> std::net::Ipv4Addr {
        unsafe { self.0.mIp4.mFields.m8.into() }
    }

    /// Get IPv6 address
    pub fn get_ipv6_addr(&self) -> std::net::Ipv6Addr {
        unsafe { self.0.mIp6.mFields.m8.into() }
    }

    /// Get the remaining time in ms
    pub fn get_remaining_time_ms(&self) -> u32 {
        self.0.mRemainingTimeMs
    }

    /// Get the protocol counters for this mapping
    pub fn get_protocol_counters(&self) -> Nat64ProtocolCounters {
        self.0.mCounters.into()
    }
}

impl Debug for Nat64AddressMapping {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mapping_id:{:?},ip4_addr:{:?},ip6_addr:{:?},remaining_time:{:?}",
            self.get_mapping_id(),
            self.get_ipv4_addr(),
            self.get_ipv6_addr(),
            self.get_remaining_time_ms()
        )
    }
}

#[derive(Default, Clone, Copy, Debug)]
#[repr(transparent)]
/// Counters for sum of all protocols for NAT64
pub struct Nat64ProtocolCounters(pub otNat64ProtocolCounters);

impl_ot_castable!(Nat64ProtocolCounters, otNat64ProtocolCounters);

impl Nat64ProtocolCounters {
    /// Get total counters
    pub fn get_total_counters(&self) -> Nat64Counters {
        self.0.mTotal.into()
    }

    /// Get ICMP counters
    pub fn get_icmp_counters(&self) -> Nat64Counters {
        self.0.mIcmp.into()
    }

    /// Get UDP counters
    pub fn get_udp_counters(&self) -> Nat64Counters {
        self.0.mUdp.into()
    }

    /// Get TCP counters
    pub fn get_tcp_counters(&self) -> Nat64Counters {
        self.0.mTcp.into()
    }
}

#[derive(Default, Clone, Copy, Debug)]
#[repr(transparent)]
/// Represents the counters for NAT64
pub struct Nat64Counters(pub otNat64Counters);

impl_ot_castable!(Nat64Counters, otNat64Counters);

impl Nat64Counters {
    /// Get IPv4 to IPv6 packets
    pub fn get_4_to_6_packets(&self) -> u64 {
        self.0.m4To6Packets
    }

    /// Get IPv4 to IPv6 bytes
    pub fn get_4_to_6_bytes(&self) -> u64 {
        self.0.m4To6Bytes
    }

    /// Get IPv6 to IPv4 packets
    pub fn get_6_to_4_packets(&self) -> u64 {
        self.0.m6To4Packets
    }

    /// Get IPv6 to IPv4 bytes
    pub fn get_6_to_4_bytes(&self) -> u64 {
        self.0.m6To4Bytes
    }
}

#[derive(Default, Clone, Copy, Debug)]
#[repr(transparent)]
/// Represents the error counters for NAT64
pub struct Nat64ErrorCounters(pub otNat64ErrorCounters);

impl_ot_castable!(Nat64ErrorCounters, otNat64ErrorCounters);

impl Nat64ErrorCounters {
    /// Get IPv4 to IPv6 counters
    pub fn get_counter_4_to_6(&self) -> [u64; 4usize] {
        self.0.mCount4To6
    }

    /// Get IPv6 to IPv4 counters
    pub fn get_counter_6_to_4(&self) -> [u64; 4usize] {
        self.0.mCount6To4
    }
}
