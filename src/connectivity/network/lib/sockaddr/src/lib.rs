// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Crate providing utilities for coercing between types of sockaddr structs.

#![warn(missing_docs)]

use std::num::NonZeroU64;

/// Socket addresses that can be converted to `libc::sockaddr_ll`.
pub trait TryToSockaddrLl {
    /// Converts `self` to a `libc::sockaddr_ll`, returning `None` if `self`'s
    /// underlying socket address type is not `libc::sockaddr_ll`.
    fn try_to_sockaddr_ll(&self) -> Option<libc::sockaddr_ll>;
}

impl TryToSockaddrLl for socket2::SockAddr {
    fn try_to_sockaddr_ll(&self) -> Option<libc::sockaddr_ll> {
        if self.family()
            != libc::sa_family_t::try_from(libc::AF_PACKET)
                .expect("libc::AF_PACKET should fit in libc::sa_family_t")
        {
            return None;
        }

        let mut sockaddr_ll = libc::sockaddr_ll {
            sll_family: 0,
            sll_protocol: 0,
            sll_ifindex: 0,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 0,
            sll_addr: [0; 8],
        };

        let len = usize::try_from(self.len()).expect("socklen_t should fit in usize");
        // We assert `>=` rather than `==` because if `self` came from a getsockname() call, its
        // length only includes the portion of `sll_addr` that is actually used (according to
        // `sll_halen`) rather than all 8 bytes.
        assert!(std::mem::size_of::<libc::sockaddr_ll>() >= len);

        // SAFETY: we've checked that len <= size_of<sockaddr_ll>().
        unsafe {
            let sockaddr: *const libc::sockaddr = self.as_ptr().cast();
            let sockaddr_ll: *mut libc::sockaddr_ll = &mut sockaddr_ll;
            std::ptr::copy_nonoverlapping(sockaddr.cast::<u8>(), sockaddr_ll.cast::<u8>(), len);
        }

        Some(sockaddr_ll)
    }
}

/// Socket addresses that can be converted to `socket2::SockAddr`.
pub trait IntoSockAddr {
    /// Converts `self` to a `socket2::SockAddr`.
    fn into_sockaddr(self) -> socket2::SockAddr;
}

impl IntoSockAddr for libc::sockaddr_ll {
    fn into_sockaddr(self) -> socket2::SockAddr {
        if libc::c_int::from(self.sll_family) != libc::AF_PACKET {
            panic!("sll_family != AF_PACKET, got {:?}", self.sll_family);
        }
        let sockaddr_ll_len = libc::socklen_t::try_from(std::mem::size_of::<libc::sockaddr_ll>())
            .expect("size of sockaddr_ll should always fit in socklen_t");

        // SAFETY: We ensure that the address family (`AF_PACKET`) and length (`sockaddr_ll_len`)
        // match the type of storage address (`sockaddr_ll`).
        // TODO(https://fxbug.dev/42055728): Move this unsafe code upstream into `socket2`.
        let ((), sock_addr) = unsafe {
            socket2::SockAddr::try_init(|sockaddr_storage, len_ptr| {
                (sockaddr_storage as *mut libc::sockaddr_ll).write(self);
                len_ptr.write(sockaddr_ll_len);
                Ok(())
            })
        }
        .expect(
            "initialize socket address should always succeed because \
                 our init fn always returns Ok(())",
        );
        sock_addr
    }
}

/// Socket address for an Ethernet packet socket.
pub struct EthernetSockaddr {
    /// The interface identifier, or `None` for no interface.
    pub interface_id: Option<NonZeroU64>,
    /// The link address.
    pub addr: net_types::ethernet::Mac,
    /// The Ethernet frame type.
    pub protocol: packet_formats::ethernet::EtherType,
}

impl From<EthernetSockaddr> for libc::sockaddr_ll {
    fn from(value: EthernetSockaddr) -> Self {
        let EthernetSockaddr { interface_id, addr, protocol } = value;

        let mut sll_addr = [0; 8];
        let addr_bytes = addr.bytes();
        sll_addr[..addr_bytes.len()].copy_from_slice(&addr_bytes);

        let ethertype = u16::from(protocol);
        libc::sockaddr_ll {
            sll_family: libc::AF_PACKET
                .try_into()
                .expect("libc::AF_PACKET should fit in sll_family field"),
            sll_ifindex: interface_id
                .map_or(0, NonZeroU64::get)
                .try_into()
                .expect("interface_id should fit in sll_ifindex field"),
            // Network order is big endian.
            sll_protocol: ethertype.to_be(),
            sll_halen: addr_bytes
                .len()
                .try_into()
                .expect("hardware address length should fit in sll_halen field"),
            sll_addr,
            sll_hatype: 0,  // unused by sendto() or bind()
            sll_pkttype: 0, // unused by sendto() or bind()
        }
    }
}

/// Socket address for a Pure IP packet socket.
pub struct PureIpSockaddr {
    /// The interface identifier, or `None` for no interface.
    pub interface_id: Option<NonZeroU64>,
    /// The IP version.
    pub protocol: net_types::ip::IpVersion,
}

impl From<PureIpSockaddr> for libc::sockaddr_ll {
    fn from(value: PureIpSockaddr) -> Self {
        let PureIpSockaddr { interface_id, protocol } = value;
        libc::sockaddr_ll {
            sll_family: libc::AF_PACKET
                .try_into()
                .expect("libc::AF_PACKET should fit in sll_family field"),
            sll_ifindex: interface_id
                .map_or(0, NonZeroU64::get)
                .try_into()
                .expect("interface_id should fit in sll_ifindex field"),
            sll_protocol: sockaddr_ll_ip_protocol(protocol),
            // Pure IP devices don't have a hardware address.
            sll_halen: 0,
            sll_addr: [0, 0, 0, 0, 0, 0, 0, 0],
            sll_hatype: 0,  // unused by sendto() or bind()
            sll_pkttype: 0, // unused by sendto() or bind()
        }
    }
}

/// Returns the protocol, in network byte order, for the given `IpVersion`.
pub fn sockaddr_ll_ip_protocol(ip_version: net_types::ip::IpVersion) -> u16 {
    let protocol = match ip_version {
        net_types::ip::IpVersion::V4 => libc::ETH_P_IP,
        net_types::ip::IpVersion::V6 => libc::ETH_P_IPV6,
    };
    // Network order is big endian.
    u16::try_from(protocol).expect("protocol should fit in a u16 field").to_be()
}

#[cfg(test)]
mod test {
    use super::*;
    use proptest::prelude::*;

    #[derive(proptest_derive::Arbitrary, Debug)]
    struct SockaddrLl {
        sll_family: u16,
        sll_protocol: u16,
        sll_ifindex: i32,
        sll_hatype: u16,
        sll_pkttype: u8,
        sll_halen: u8,
        sll_addr: [u8; 8],
    }

    impl From<SockaddrLl> for libc::sockaddr_ll {
        fn from(
            SockaddrLl {
                sll_family,
                sll_protocol,
                sll_ifindex,
                sll_hatype,
                sll_pkttype,
                sll_halen,
                sll_addr,
            }: SockaddrLl,
        ) -> Self {
            libc::sockaddr_ll {
                sll_family,
                sll_protocol,
                sll_ifindex,
                sll_hatype,
                sll_pkttype,
                sll_halen,
                sll_addr,
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(
                Box::<proptest::test_runner::MapFailurePersistence>::default()
            ),
            ..ProptestConfig::default()
        })]

        #[test]
        fn roundtrip(addr in any::<SockaddrLl>().prop_map(|addr| SockaddrLl {
            sll_family: u16::try_from(libc::AF_PACKET).unwrap(),
            ..addr
        })) {
            let sockaddr_ll_start = libc::sockaddr_ll::from(addr);
            let sockaddr = sockaddr_ll_start.into_sockaddr();
            let sockaddr_ll_end = sockaddr.try_to_sockaddr_ll().unwrap();
            prop_assert_eq!(sockaddr_ll_start, sockaddr_ll_end);
        }
    }

    #[test]
    fn not_sockaddr_ll_returns_none() {
        let sockaddr = socket2::SockAddr::from(net_declare::std_socket_addr!("192.168.0.1:8080"));
        assert_eq!(sockaddr.try_to_sockaddr_ll(), None);
    }
}
