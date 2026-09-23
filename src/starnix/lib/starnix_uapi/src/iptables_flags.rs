// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;
use bitflags::bitflags;

bitflags! {
    // Used for both IPv4 and IPv6.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IptIpInverseFlags: u32 {
        const INPUT_INTERFACE = uapi::IPT_INV_VIA_IN;
        const OUTPUT_INTERFACE = uapi::IPT_INV_VIA_OUT;
        const TOS = uapi::IPT_INV_TOS;
        const SOURCE_IP_ADDRESS = uapi::IPT_INV_SRCIP;
        const DESTINATION_IP_ADDRESS = uapi::IPT_INV_DSTIP;
        const FRAGMENT = uapi::IPT_INV_FRAG;
        const PROTOCOL = uapi::IPT_INV_PROTO;
    }
}

impl From<IptIpInverseFlags> for u64 {
    fn from(value: IptIpInverseFlags) -> u64 {
        value.bits() as u64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IptIpFlags {
    V4(IptIpFlagsV4),
    V6(IptIpFlagsV6),
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IptIpFlagsV4: u32 {
        const FRAGMENT = uapi::IPT_F_FRAG;
        const GOTO = uapi::IPT_F_GOTO;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct IptIpFlagsV6: u32 {
        const PROTOCOL = uapi::IP6T_F_PROTO;
        const TOS = uapi::IP6T_F_TOS;
        const GOTO = uapi::IP6T_F_GOTO;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct NfIpHooks: u32 {
        const PREROUTING = 1 << uapi::NF_IP_PRE_ROUTING;
        const INPUT = 1 << uapi::NF_IP_LOCAL_IN;
        const FORWARD = 1 << uapi::NF_IP_FORWARD;
        const OUTPUT = 1 << uapi::NF_IP_LOCAL_OUT;
        const POSTROUTING = 1 << uapi::NF_IP_POST_ROUTING;

        const FILTER = Self::INPUT.bits() | Self::FORWARD.bits() | Self::OUTPUT.bits();
        const MANGLE = Self::PREROUTING.bits() | Self::INPUT.bits() | Self::FORWARD.bits() |
                       Self::OUTPUT.bits() | Self::POSTROUTING.bits();
        const NAT = Self::PREROUTING.bits() | Self::INPUT.bits() | Self::OUTPUT.bits() |
                    Self::POSTROUTING.bits();
        const RAW = Self::PREROUTING.bits() | Self::OUTPUT.bits();

    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct NfNatRangeFlags: u32 {
        const MAP_IPS = uapi::NF_NAT_RANGE_MAP_IPS;
        const PROTO_SPECIFIED = uapi::NF_NAT_RANGE_PROTO_SPECIFIED;
        const PROTO_RANDOM = uapi::NF_NAT_RANGE_PROTO_RANDOM;
        const PERSISTENT = uapi::NF_NAT_RANGE_PERSISTENT;
        const PROTO_RANDOM_FULLY = uapi::NF_NAT_RANGE_PROTO_RANDOM_FULLY;
        const PROTO_OFFSET = uapi::NF_NAT_RANGE_PROTO_OFFSET;
        const NET_MAP = uapi::NF_NAT_RANGE_NETMAP;

        // Multi-bit flags
        const PROTO_RANDOM_ALL = uapi::NF_NAT_RANGE_PROTO_RANDOM_ALL;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct XtTcpInverseFlags: u32 {
        const SOURCE_PORT = uapi::XT_TCP_INV_SRCPT;
        const DESTINATION_PORT = uapi::XT_TCP_INV_DSTPT;
        const FLAGS = uapi::XT_TCP_INV_FLAGS;
        const OPTION = uapi::XT_TCP_INV_OPTION;
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct XtUdpInverseFlags: u32 {
        const SOURCE_PORT = uapi::XT_UDP_INV_SRCPT;
        const DESTINATION_PORT = uapi::XT_UDP_INV_DSTPT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    fn all_known_bits_same_as_mask() {
        assert_eq!(IptIpInverseFlags::all().bits(), uapi::IPT_INV_MASK);
        assert_eq!(IptIpFlagsV4::all().bits(), uapi::IPT_F_MASK);
        assert_eq!(IptIpFlagsV6::all().bits(), uapi::IP6T_F_MASK);
        assert_eq!(NfIpHooks::all().bits().count_ones(), uapi::NF_IP_NUMHOOKS);
        assert_eq!(NfNatRangeFlags::all().bits(), uapi::NF_NAT_RANGE_MASK);
        assert_eq!(XtTcpInverseFlags::all().bits(), uapi::XT_TCP_INV_MASK);
        assert_eq!(XtUdpInverseFlags::all().bits(), uapi::XT_UDP_INV_MASK);
    }
}
