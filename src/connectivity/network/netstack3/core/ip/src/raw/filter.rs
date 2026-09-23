// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declare types related to the per-socket filters of raw IP sockets.

use core::fmt::{self, Display};
use net_types::ip::{GenericOverIp, Ip, IpVersion, IpVersionMarker};
use packet_formats::icmp::IcmpIpExt;

/// An ICMP filter installed on a raw IP socket.
#[derive(Debug, GenericOverIp, PartialEq, Clone)]
#[generic_over_ip(I, Ip)]
pub struct RawIpSocketIcmpFilter<I: IcmpIpExt> {
    _marker: IpVersionMarker<I>,
    /// The raw 256-bit filter. If bit `n` is set, ICMP messages with type `n`
    /// will be filtered.
    ///
    /// Note: if bit `n` is an invalid message type, the packet will be dropped
    /// regardless of if the bit is set or not.
    filter: [u8; 32],
}

impl<I: IcmpIpExt> RawIpSocketIcmpFilter<I> {
    /// An ICMP filter that allows all message types to be delivered.
    pub const ALLOW_ALL: Self = Self::new([0; 32]);

    /// An ICMP filter that prevents all message types from being delivered.
    pub const DENY_ALL: Self = Self::new([u8::MAX; 32]);

    /// Construct a `RawIpSocketIcmpFilter` from the raw bytes.
    ///
    /// The array is expected to be little endian. E.g. byte 0 in the array is
    /// used to control filters for types 0-7.
    pub const fn new(filter: [u8; 32]) -> RawIpSocketIcmpFilter<I> {
        RawIpSocketIcmpFilter { _marker: IpVersionMarker::new(), filter }
    }

    /// Convert the `RawIpSocketIcmpFilter` into the raw bytes.
    ///
    /// The array is returned in little endian format.
    pub fn into_bytes(self) -> [u8; 32] {
        let RawIpSocketIcmpFilter { _marker, filter } = self;
        filter
    }

    /// True if this `RawIpSocketIcmpFilter` allows ICMP messages of the given type.
    pub(super) fn allows_type(&self, message_type: I::IcmpMessageType) -> bool {
        let message_type: u8 = message_type.into();
        let byte: u8 = message_type / 8;
        let bit: u8 = message_type % 8;
        // NB: message_type has a max value of 255 (u8::MAX); once divided by 8
        // its maximum value becomes 31, so `byte` cannot exceed the array
        // bounds on `self.filter`, which has a length of 32.
        (self.filter[usize::from(byte)] & (1 << bit)) == 0
    }

    /// Set whether the given message type is allowed.
    #[cfg(test)]
    fn set_type(&mut self, message_type: u8, allow: bool) {
        let byte: u8 = message_type / 8;
        let bit: u8 = message_type % 8;
        // NB: message_type has a max value of 255 (u8::MAX); once divided by 8
        // its maximum value becomes 31, so `byte` cannot exceed the array
        // bounds on `self.filter`, which has a length of 32.
        match allow {
            true => self.filter[usize::from(byte)] &= !(1 << bit),
            false => self.filter[usize::from(byte)] |= 1 << bit,
        }
    }
}

impl<I: IcmpIpExt> Display for RawIpSocketIcmpFilter<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let iter = (0..=u8::MAX).filter_map(|i| {
            let message_type = I::IcmpMessageType::try_from(i).ok()?;
            self.allows_type(message_type).then_some(message_type)
        });

        match I::VERSION {
            IpVersion::V4 => write!(f, "AllowedIcmpMessageTypes [")?,
            IpVersion::V6 => write!(f, "AllowedIcmpv6MessageTypes [")?,
        }
        for (i, message_type) in iter.enumerate() {
            if i == 0 {
                write!(f, "\"{message_type:?}\"")?;
            } else {
                write!(f, ", \"{message_type:?}\"")?;
            }
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::string::{String, ToString as _};
    use alloc::vec::Vec;
    use alloc::{format, vec};
    use ip_test_macro::ip_test;
    use net_types::ip::{Ipv4, Ipv6};
    use packet_formats::icmp::{Icmpv4MessageType, Icmpv6MessageType};
    use test_case::test_case;

    /// Builds a filter to precisely allow/disallow a given message type.
    ///
    /// E.g. when allow is true, the filter will be all 1s, except for the bit
    /// at message type. The filter will have the opposite value when allow is
    /// false.
    fn build_precise_filter<I: IcmpIpExt>(
        message_type: u8,
        allow: bool,
    ) -> RawIpSocketIcmpFilter<I> {
        let mut filter = match allow {
            true => RawIpSocketIcmpFilter::<I>::DENY_ALL,
            false => RawIpSocketIcmpFilter::<I>::ALLOW_ALL,
        };
        filter.set_type(message_type, allow);
        filter
    }

    // NB: the test helper is complex enough to warrant a test of it's own.
    #[test_case(21, true, [
        0xFF, 0xFF, 0xDF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]; "allow_21")]
    #[test_case(21, false, [
        0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]; "deny_21")]
    fn build_filter(message_type: u8, allow: bool, expected: [u8; 32]) {
        assert_eq!(build_precise_filter::<Ipv4>(message_type, allow).into_bytes(), expected);
        assert_eq!(build_precise_filter::<Ipv6>(message_type, allow).into_bytes(), expected);
    }

    #[ip_test(I)]
    fn icmp_filter_allows_type<I: IcmpIpExt>() {
        for i in 0..=u8::MAX {
            match I::IcmpMessageType::try_from(i) {
                // This isn't a valid message type; skip testing it.
                Err(_) => continue,
                Ok(message_type) => {
                    let pass_filter = build_precise_filter::<I>(i, true);
                    let deny_filter = build_precise_filter::<I>(i, false);
                    assert!(pass_filter.allows_type(message_type), "Should allow MessageType:{i}");
                    assert!(!deny_filter.allows_type(message_type), "Should deny MessageType:{i}");
                }
            }
        }
    }

    #[test_case(vec![] => "AllowedIcmpMessageTypes []".to_string(); "deny_all")]
    #[test_case(vec![Icmpv4MessageType::EchoRequest] =>
        "AllowedIcmpMessageTypes [\"Echo Request\"]".to_string(); "allow_echo_request")]
    #[test_case(vec![Icmpv4MessageType::EchoReply, Icmpv4MessageType::EchoRequest] =>
        "AllowedIcmpMessageTypes [\"Echo Reply\", \"Echo Request\"]".to_string();
        "allow_echo_request_and_reply")]
    fn icmpv4_filter_display(allowed_types: Vec<Icmpv4MessageType>) -> String {
        let mut filter = RawIpSocketIcmpFilter::<Ipv4>::DENY_ALL;
        for allowed_type in allowed_types {
            filter.set_type(allowed_type.into(), true);
        }
        format!("{filter}")
    }

    #[test_case(vec![] => "AllowedIcmpv6MessageTypes []".to_string(); "deny_all")]
    #[test_case(vec![Icmpv6MessageType::EchoRequest] =>
        "AllowedIcmpv6MessageTypes [\"Echo Request\"]".to_string(); "allow_echo_request")]
    #[test_case(vec![Icmpv6MessageType::EchoRequest, Icmpv6MessageType::EchoReply] =>
        "AllowedIcmpv6MessageTypes [\"Echo Request\", \"Echo Reply\"]".to_string();
        "allow_echo_request_and_reply")]
    fn icmpv6_filter_display(allowed_types: Vec<Icmpv6MessageType>) -> String {
        let mut filter = RawIpSocketIcmpFilter::<Ipv6>::DENY_ALL;
        for allowed_type in allowed_types {
            filter.set_type(allowed_type.into(), true);
        }
        format!("{filter}")
    }
}
