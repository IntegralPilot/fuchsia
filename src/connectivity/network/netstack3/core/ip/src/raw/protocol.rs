// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares types related to the protocols of raw IP sockets.

use net_types::ip::{GenericOverIp, Ip, IpVersion};
use netstack3_base::IpExt;
use packet_formats::ip::{IpProto, Ipv4Proto, Ipv6Proto};

/// A witness type enforcing that the contained protocol is not the
/// "reserved" IANA Internet protocol.
#[derive(Clone, Copy, Debug, Eq, GenericOverIp, Ord, PartialEq, PartialOrd)]
#[generic_over_ip(I, Ip)]
pub struct Protocol<I: IpExt>(I::Proto);

impl<I: IpExt> Protocol<I> {
    fn new(proto: I::Proto) -> Option<Protocol<I>> {
        I::map_ip(
            proto,
            |v4_proto| match v4_proto {
                Ipv4Proto::Proto(IpProto::Reserved) => None,
                _ => Some(Protocol(v4_proto)),
            },
            |v6_proto| match v6_proto {
                Ipv6Proto::Proto(IpProto::Reserved) => None,
                _ => Some(Protocol(v6_proto)),
            },
        )
    }
}

/// The supported protocols of raw IP sockets.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RawIpSocketProtocol<I: IpExt> {
    /// Analogous to `IPPROTO_RAW` on Linux.
    ///
    /// This configures the socket as send only (no packets will be received).
    /// When sending the provided message must include the IP header, as when
    /// the `IP_HDRINCL` socket option is set.
    Raw,
    /// An IANA Internet Protocol.
    Proto(Protocol<I>),
}

impl<I: IpExt> RawIpSocketProtocol<I> {
    /// Construct a new [`RawIpSocketProtocol`] from the given IP protocol.
    pub fn new(proto: I::Proto) -> RawIpSocketProtocol<I> {
        Protocol::new(proto).map_or(RawIpSocketProtocol::Raw, RawIpSocketProtocol::Proto)
    }

    /// Extract the plain IP protocol from this [`RawIpSocketProtocol`].
    pub fn proto(&self) -> I::Proto {
        match self {
            RawIpSocketProtocol::Raw => IpProto::Reserved.into(),
            RawIpSocketProtocol::Proto(Protocol(proto)) => *proto,
        }
    }

    /// True, if the protocol is ICMP for the associated IP version `I`.
    pub fn is_icmp(&self) -> bool {
        match self {
            RawIpSocketProtocol::Raw => false,
            RawIpSocketProtocol::Proto(Protocol(p)) => *p == I::ICMP_IP_PROTO,
        }
    }

    /// True if the protocol requires system checksum support.
    pub fn requires_system_checksums(&self) -> bool {
        // Per RFC 2292, Section 3.1:
        //
        // The kernel will calculate and insert the ICMPv6 checksum for ICMPv6
        // raw sockets, since this checksum is mandatory.
        //
        // https://www.rfc-editor.org/rfc/rfc2292#section-3.1
        self.is_icmp() && I::VERSION == IpVersion::V6
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ip_test_macro::ip_test;
    use net_types::ip::{Ipv4, Ipv6};
    use test_case::test_case;

    #[ip_test(I)]
    #[test_case(IpProto::Udp, Some(Protocol(IpProto::Udp.into())); "valid")]
    #[test_case(IpProto::Reserved, None; "reserved")]
    fn new_protocol<I: IpExt>(p: IpProto, expected: Option<Protocol<I>>) {
        assert_eq!(Protocol::new(p.into()), expected);
    }

    #[ip_test(I)]
    #[test_case(IpProto::Udp, RawIpSocketProtocol::Proto(Protocol(IpProto::Udp.into())); "valid")]
    #[test_case(IpProto::Reserved, RawIpSocketProtocol::Raw; "reserved")]
    fn new_raw_ip_socket_protocol<I: IpExt>(p: IpProto, expected: RawIpSocketProtocol<I>) {
        assert_eq!(RawIpSocketProtocol::new(p.into()), expected);
    }

    #[test_case(Ipv4Proto::Icmp, true; "icmpv4")]
    #[test_case(Ipv4Proto::Other(58), false; "icmpv6")]
    #[test_case(Ipv4Proto::Proto(IpProto::Udp), false; "udp")]
    #[test_case(Ipv4Proto::Proto(IpProto::Reserved), false; "reserved")]
    fn is_icmpv4(proto: Ipv4Proto, expected_result: bool) {
        assert_eq!(RawIpSocketProtocol::<Ipv4>::new(proto).is_icmp(), expected_result)
    }

    #[test_case(Ipv6Proto::Icmpv6, true; "icmpv6")]
    #[test_case(Ipv6Proto::Other(1), false; "icmpv4")]
    #[test_case(Ipv6Proto::Proto(IpProto::Udp), false; "udp")]
    #[test_case(Ipv6Proto::Proto(IpProto::Reserved), false; "reserved")]
    fn is_icmpv6(proto: Ipv6Proto, expected_result: bool) {
        assert_eq!(RawIpSocketProtocol::<Ipv6>::new(proto).is_icmp(), expected_result)
    }
}
