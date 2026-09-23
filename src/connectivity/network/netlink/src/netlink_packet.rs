// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for interacting with the `netlink-packet-*` suite 3p crates.

use std::num::NonZeroI32;

use log::warn;
use net_types::ip::{Ip, Ipv4Addr, Ipv6Addr};
use netlink_packet_core::buffer::NETLINK_HEADER_LEN;
use netlink_packet_core::constants::{
    NLM_F_ACK, NLM_F_APPEND, NLM_F_ATOMIC, NLM_F_CREATE, NLM_F_DUMP, NLM_F_ECHO, NLM_F_EXCL,
    NLM_F_MATCH, NLM_F_MULTIPART, NLM_F_REPLACE, NLM_F_REQUEST, NLM_F_ROOT,
};
use netlink_packet_core::{
    DoneMessage, ErrorMessage, NetlinkHeader, NetlinkMessage, NetlinkPayload, NetlinkSerializable,
};
use netlink_packet_route::route::RouteAddress;
use netlink_packet_utils::Emitable as _;

use crate::netlink_packet::errno::Errno;

pub(crate) const UNSPECIFIED_SEQUENCE_NUMBER: u32 = 0;

/// The error code used by `Done` messages.
const DONE_ERROR_CODE: i32 = 0;

/// Returns a `Done` message.
pub(crate) fn new_done<T: NetlinkSerializable>(req_header: NetlinkHeader) -> NetlinkMessage<T> {
    let mut done = DoneMessage::default();
    done.code = DONE_ERROR_CODE;
    let payload = NetlinkPayload::<T>::Done(done);
    let mut resp_header = NetlinkHeader::default();
    resp_header.sequence_number = req_header.sequence_number;
    resp_header.flags |= NLM_F_MULTIPART;
    let mut message = NetlinkMessage::new(resp_header, payload);
    // Sets the header `length` and `message_type` based on the payload.
    message.finalize();
    message
}

/// Produces an `I::Addr` from the given `RouteAddress`
pub(crate) fn ip_addr_from_route<I: Ip>(route_addr: &RouteAddress) -> Result<I::Addr, Errno> {
    I::map_ip(
        (),
        |()| match route_addr {
            RouteAddress::Inet(v4_addr) => Ok(Ipv4Addr::new(v4_addr.octets())),
            RouteAddress::Inet6(_) => {
                warn!("expected IPv4 address from route but got an IPv6 address");
                Err(Errno::EINVAL)
            }
            RouteAddress::Mpls(_) | RouteAddress::Other(_) | _ => Err(Errno::ENOTSUP),
        },
        |()| match route_addr {
            RouteAddress::Inet6(v6_addr) => Ok(Ipv6Addr::new(v6_addr.segments())),
            RouteAddress::Inet(_) => {
                warn!("expected IPv6 address from route but got an IPv4 address");
                Err(Errno::EINVAL)
            }
            RouteAddress::Mpls(_) | RouteAddress::Other(_) | _ => Err(Errno::ENOTSUP),
        },
    )
}

pub(crate) mod errno {
    use net_types::ip::GenericOverIp;

    use super::*;

    /// Represents a Error number, aka `errno`.
    ///
    /// These values are negated when sent in Netlink error messages.
    #[derive(Copy, Clone, Debug, PartialEq, GenericOverIp)]
    #[generic_over_ip()]
    pub struct Errno(i32);

    impl Errno {
        pub(crate) const EADDRNOTAVAIL: Errno = Errno::new(libc::EADDRNOTAVAIL).unwrap();
        pub(crate) const EAFNOSUPPORT: Errno = Errno::new(libc::EAFNOSUPPORT).unwrap();
        pub(crate) const EBUSY: Errno = Errno::new(libc::EBUSY).unwrap();
        pub(crate) const EEXIST: Errno = Errno::new(libc::EEXIST).unwrap();
        pub(crate) const EINVAL: Errno = Errno::new(libc::EINVAL).unwrap();
        pub(crate) const ENODEV: Errno = Errno::new(libc::ENODEV).unwrap();
        pub(crate) const ENOENT: Errno = Errno::new(libc::ENOENT).unwrap();
        pub(crate) const ENOTSUP: Errno = Errno::new(libc::ENOTSUP).unwrap();
        pub(crate) const ESRCH: Errno = Errno::new(libc::ESRCH).unwrap();
        pub(crate) const ETOOMANYREFS: Errno = Errno::new(libc::ETOOMANYREFS).unwrap();
        pub(crate) const ENOBUFS: Errno = Errno::new(libc::ENOBUFS).unwrap();

        /// Construct a new [`Errno`] from the given positive integer.
        ///
        /// Returns `None` when the code is non-positive (which includes 0).
        pub const fn new(code: i32) -> Option<Self> {
            if code.is_positive() { Some(Errno(code)) } else { None }
        }
    }

    impl From<Errno> for NonZeroI32 {
        fn from(Errno(code): Errno) -> Self {
            NonZeroI32::new(code).expect("Errno's code must be non-zero")
        }
    }

    impl From<Errno> for i32 {
        fn from(Errno(code): Errno) -> Self {
            code
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use test_case::test_case;

        #[test_case(i32::MIN, None; "min")]
        #[test_case(-10, None; "negative")]
        #[test_case(0, None; "zero")]
        #[test_case(10, Some(10); "positive")]
        #[test_case(i32::MAX, Some(i32::MAX); "max")]
        fn test_new_errno(raw_code: i32, expected_code: Option<i32>) {
            assert_eq!(Errno::new(raw_code).map(Into::<i32>::into), expected_code)
        }
    }
}

/// Returns an `Error` message.
///
/// `Ok(())` represents an ACK while `Err(Errno)` represents a NACK.
pub(crate) fn new_error<T: NetlinkSerializable>(
    error: Result<(), errno::Errno>,
    req_header: NetlinkHeader,
) -> NetlinkMessage<T> {
    let error = {
        assert_eq!(req_header.buffer_len(), NETLINK_HEADER_LEN);
        let mut buffer = vec![0; NETLINK_HEADER_LEN];
        req_header.emit(&mut buffer);

        let code = match error {
            Ok(()) => None,

            // Netlink error codes are negative errno's.
            Err(e) => Some(-NonZeroI32::from(e)),
        };

        let mut error = ErrorMessage::default();
        error.code = code;
        error.header = buffer;
        error
    };

    let payload = NetlinkPayload::<T>::Error(error);
    // Note that the following header fields are unset as they don't appear to
    // be used by any of our clients: `flags`.
    let mut resp_header = NetlinkHeader::default();
    resp_header.sequence_number = req_header.sequence_number;
    let mut message = NetlinkMessage::new(resp_header, payload);
    // Sets the header `length` and `message_type` based on the payload.
    message.finalize();
    message
}

/// Broad categories of Netlink requests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NetlinkRequestType {
    /// RTM_NEW*.
    New,
    /// RTM_GET*.
    Get,
    /// RTM_SET*.
    Set,
    /// RTM_DEL*.
    Del,
}

/// Produces a debug string indicating the Netlink flags set on a request.
///
/// See https://man7.org/linux/man-pages/man7/netlink.7.html (section starting
/// with "Standard flag bits ...").
pub(crate) fn netlink_flags_debug_string(flags: u16, request_type: NetlinkRequestType) -> String {
    let mut flags_dbg = vec![];
    if (flags & NLM_F_REQUEST) == NLM_F_REQUEST {
        flags_dbg.push("REQUEST");
    }
    if (flags & NLM_F_MULTIPART) == NLM_F_MULTIPART {
        flags_dbg.push("MULTI");
    }
    if (flags & NLM_F_ACK) == NLM_F_ACK {
        flags_dbg.push("ACK");
    }
    if (flags & NLM_F_ECHO) == NLM_F_ECHO {
        flags_dbg.push("ECHO");
    }
    match request_type {
        NetlinkRequestType::Get => {
            if (flags & NLM_F_DUMP) == NLM_F_DUMP {
                flags_dbg.push("DUMP");
            } else {
                // NLM_F_DUMP is a convenience macro for NLM_F_ROOT|NLM_F_MATCH.
                if (flags & NLM_F_ROOT) == NLM_F_ROOT {
                    flags_dbg.push("ROOT");
                }
                if (flags & NLM_F_MATCH) == NLM_F_MATCH {
                    flags_dbg.push("MATCH");
                }
            }
            if (flags & NLM_F_ATOMIC) == NLM_F_ATOMIC {
                flags_dbg.push("ATOMIC");
            }
        }
        NetlinkRequestType::New => {
            if (flags & NLM_F_REPLACE) == NLM_F_REPLACE {
                flags_dbg.push("REPLACE");
            }
            if (flags & NLM_F_EXCL) == NLM_F_EXCL {
                flags_dbg.push("EXCL");
            }
            if (flags & NLM_F_CREATE) == NLM_F_CREATE {
                flags_dbg.push("CREATE");
            }
            if (flags & NLM_F_APPEND) == NLM_F_APPEND {
                flags_dbg.push("APPEND");
            }
        }
        NetlinkRequestType::Set | NetlinkRequestType::Del => {}
    }
    flags_dbg.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use netlink_packet_core::{NLMSG_DONE, NLMSG_ERROR, NetlinkBuffer};
    use netlink_packet_route::RouteNetlinkMessage;
    use netlink_packet_utils::Parseable as _;
    use test_case::test_case;

    use crate::netlink_packet::errno::Errno;

    #[test_case(0, Ok(()); "ACK")]
    #[test_case(0, Err(Errno::EINVAL); "EINVAL")]
    #[test_case(1, Err(Errno::ENODEV); "ENODEV")]
    fn test_new_error(sequence_number: u32, expected_error: Result<(), Errno>) {
        // Header with arbitrary values
        let mut expected_header = NetlinkHeader::default();
        expected_header.length = 0x01234567;
        expected_header.message_type = 0x89AB;
        expected_header.flags = 0xCDEF;
        expected_header.sequence_number = sequence_number;
        expected_header.port_number = 0x00000000;

        let error = new_error::<RouteNetlinkMessage>(expected_error, expected_header);
        // `serialize` will panic if the message is malformed.
        let mut buf = vec![0; error.buffer_len()];
        error.serialize(&mut buf);

        let (header, payload) = error.into_parts();
        assert_eq!(header.message_type, NLMSG_ERROR);
        assert_eq!(header.sequence_number, sequence_number);
        assert_matches!(
            payload,
            NetlinkPayload::Error(ErrorMessage{ code, header, .. }) => {
                let expected_code = match expected_error {
                    Ok(()) => None,
                    Err(e) => Some(-NonZeroI32::from(e)),
                };
                assert_eq!(code, expected_code);
                assert_eq!(
                    // NB: The error response only contains the header, so we
                    // must use unchecked to parse it.
                    NetlinkHeader::parse(&NetlinkBuffer::new_unchecked(&header)).unwrap(),
                    expected_header,
                );
            }
        );
    }

    #[test_case(0; "seq_0")]
    #[test_case(1; "seq_1")]
    fn test_new_done(sequence_number: u32) {
        let mut req_header = NetlinkHeader::default();
        req_header.sequence_number = sequence_number;

        let done = new_done::<RouteNetlinkMessage>(req_header);
        // `serialize` will panic if the message is malformed.
        let mut buf = vec![0; done.buffer_len()];
        done.serialize(&mut buf);

        let (header, payload) = done.into_parts();
        assert_eq!(header.sequence_number, sequence_number);
        assert_eq!(header.message_type, NLMSG_DONE);
        assert_eq!(header.flags, NLM_F_MULTIPART);
        assert_matches!(
            payload,
            NetlinkPayload::Done(DoneMessage {code, extended_ack, ..}) => {
                assert_eq!(code, DONE_ERROR_CODE);
                assert_eq!(extended_ack, Vec::<u8>::new());
            }
        );
    }

    #[test_case(
        0,
        NetlinkRequestType::Get => "";
        "no flags"
    )]
    #[test_case(
        NLM_F_REQUEST,
        NetlinkRequestType::Get => "REQUEST";
        "request only"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_MULTIPART|NLM_F_ACK|NLM_F_ECHO,
        NetlinkRequestType::Get => "REQUEST|MULTI|ACK|ECHO";
        "all generic flags"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_DUMP,
        NetlinkRequestType::Get => "REQUEST|DUMP";
        "dump request"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_MATCH|NLM_F_ROOT,
        NetlinkRequestType::Get => "REQUEST|DUMP";
        "dump is alias for match|root"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_ATOMIC,
        NetlinkRequestType::Get => "REQUEST|ATOMIC";
        "other Get flags"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_REPLACE|NLM_F_EXCL|NLM_F_CREATE|NLM_F_APPEND,
        NetlinkRequestType::New
        => "REQUEST|REPLACE|EXCL|CREATE|APPEND";
        "New flags"
    )]
    #[test_case(
        NLM_F_REQUEST|NLM_F_REPLACE|NLM_F_EXCL|NLM_F_CREATE|NLM_F_APPEND,
        NetlinkRequestType::Del => "REQUEST";
        "type-inappropriate flags ignored"
    )]
    fn netlink_flags_debug_string_tests(flags: u16, request_type: NetlinkRequestType) -> String {
        netlink_flags_debug_string(flags, request_type)
    }
}
