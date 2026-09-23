// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares types and functionality related to checksums for raw IP sockets.

use log::error;
use net_types::ip::{GenericOverIp, Ip, IpInvariant, Ipv4Addr, Ipv6Addr};
use netstack3_base::IpExt;
use packet::{Buf, BufferViewMut, ParsablePacket as _, ParseBuffer as _};
use packet_formats::icmp::{IcmpParseArgs, Icmpv6Packet, Icmpv6PacketRaw};
use packet_formats::ip::{IpPacket, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::Ipv4Packet;
use packet_formats::ipv6::Ipv6Packet;
use zerocopy::SplitByteSlice;

/// Errors that may occur while validating/generating a checksum.
#[derive(GenericOverIp)]
#[generic_over_ip()]
pub(super) enum ChecksumError {
    /// Checksum support has not yet been added for the protocol.
    ProtocolNotSupported,
    /// The checksum couldn't be validated/generated.
    ChecksumFailed,
}

/// Returns true if the given [`IpPacket`] has a valid checksum.
pub(super) fn has_valid_checksum<I: IpExt, B: SplitByteSlice>(packet: &I::Packet<B>) -> bool {
    match I::map_ip(
        packet,
        |packet| validate_ipv4_checksum(packet),
        |packet| validate_ipv6_checksum(packet),
    ) {
        Ok(()) => true,
        Err(ChecksumError::ChecksumFailed) => false,
        Err(ChecksumError::ProtocolNotSupported) => {
            // TODO(https://fxbug.dev/343672830): Add checksum validation for
            // other protocols beyond ICMPv6.
            error!(
                "raw IP sockets asked to validate checksum for unsupported protocol: {:?}",
                packet.proto()
            );
            false
        }
    }
}

/// Returns `Ok(())` if the given [`Ipv4Packet`] has a valid checksum.
fn validate_ipv4_checksum<B: SplitByteSlice>(packet: &Ipv4Packet<B>) -> Result<(), ChecksumError> {
    match packet.proto() {
        Ipv4Proto::Icmp
        | Ipv4Proto::Igmp
        | Ipv4Proto::Proto(IpProto::Udp)
        | Ipv4Proto::Proto(IpProto::Tcp)
        | Ipv4Proto::Proto(IpProto::Reserved)
        | Ipv4Proto::Other(_) => Err(ChecksumError::ProtocolNotSupported),
    }
}

/// Returns `Ok(())` if the given [`Ipv4Packet`] has a valid checksum.
fn validate_ipv6_checksum<B: SplitByteSlice>(packet: &Ipv6Packet<B>) -> Result<(), ChecksumError> {
    match packet.proto() {
        Ipv6Proto::Icmpv6 => {
            // Parsing validates the checksum.
            let mut buffer = Buf::new(packet.body(), ..);
            match buffer.parse_with::<_, Icmpv6Packet<_>>(IcmpParseArgs::new(
                packet.src_ip(),
                packet.dst_ip(),
            )) {
                Ok(_packet) => Ok(()),
                Err(_) => Err(ChecksumError::ChecksumFailed),
            }
        }
        Ipv6Proto::NoNextHeader
        | Ipv6Proto::Proto(IpProto::Udp)
        | Ipv6Proto::Proto(IpProto::Tcp)
        | Ipv6Proto::Proto(IpProto::Reserved)
        | Ipv6Proto::Other(_) => Err(ChecksumError::ProtocolNotSupported),
    }
}

/// Populates a checksum in the provided `body`.
///
/// True if the checksum was successfully populated. False, if the checksum,
/// could not be populated (in which case the provided body is unmodified).
pub(super) fn populate_checksum<'a, I: IpExt, B: BufferViewMut<&'a mut [u8]>>(
    src_ip: I::Addr,
    dst_ip: I::Addr,
    proto: I::Proto,
    body: B,
) -> bool {
    match I::map_ip(
        (src_ip, dst_ip, proto, IpInvariant(body)),
        |(src_ip, dst_ip, proto, IpInvariant(body))| {
            populate_ipv4_checksum(src_ip, dst_ip, proto, body)
        },
        |(src_ip, dst_ip, proto, IpInvariant(body))| {
            populate_ipv6_checksum(src_ip, dst_ip, proto, body)
        },
    ) {
        Ok(()) => true,
        Err(ChecksumError::ChecksumFailed) => false,
        Err(ChecksumError::ProtocolNotSupported) => {
            // TODO(https://fxbug.dev/343672830): Add checksum validation for
            // other protocols beyond ICMPv6.
            error!("raw IP sockets asked to generate checksum for unsupported protocol: {proto:?}");
            false
        }
    }
}

/// Returns Ok(()) if the checksum was successfully written into `body`.
fn populate_ipv4_checksum<'a, B: BufferViewMut<&'a mut [u8]>>(
    _src_ip: Ipv4Addr,
    _dst_ip: Ipv4Addr,
    proto: Ipv4Proto,
    _body: B,
) -> Result<(), ChecksumError> {
    match proto {
        Ipv4Proto::Icmp
        | Ipv4Proto::Igmp
        | Ipv4Proto::Proto(IpProto::Udp)
        | Ipv4Proto::Proto(IpProto::Tcp)
        | Ipv4Proto::Proto(IpProto::Reserved)
        | Ipv4Proto::Other(_) => Err(ChecksumError::ProtocolNotSupported),
    }
}

/// Returns Ok(()) if the checksum was successfully written into `body`.
fn populate_ipv6_checksum<'a, B: BufferViewMut<&'a mut [u8]>>(
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    proto: Ipv6Proto,
    body: B,
) -> Result<(), ChecksumError> {
    match proto {
        Ipv6Proto::Icmpv6 => {
            match Icmpv6PacketRaw::parse_mut(body, ()) {
                Ok(mut packet) => {
                    if packet.try_write_checksum(src_ip, dst_ip) {
                        Ok(())
                    } else {
                        Err(ChecksumError::ChecksumFailed)
                    }
                }
                Err(_) => {
                    // The body doesn't have all of the parts required of an
                    // ICMPv6 message; we won't be able to compute a
                    // checksum.
                    Err(ChecksumError::ChecksumFailed)
                }
            }
        }
        Ipv6Proto::NoNextHeader
        | Ipv6Proto::Proto(IpProto::Udp)
        | Ipv6Proto::Proto(IpProto::Tcp)
        | Ipv6Proto::Proto(IpProto::Reserved)
        | Ipv6Proto::Other(_) => Err(ChecksumError::ProtocolNotSupported),
    }
}
