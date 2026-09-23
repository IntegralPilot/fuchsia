// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vfs::FsString;
use crate::vfs::socket::NetlinkAddress;
use bitflags::bitflags;
use starnix_uapi::errors::Errno;
use starnix_uapi::{
    AF_INET, AF_INET6, AF_NETLINK, AF_PACKET, AF_QIPCRTR, AF_UNIX, AF_UNSPEC, AF_VSOCK, MSG_BATCH,
    MSG_CMSG_CLOEXEC, MSG_CONFIRM, MSG_CTRUNC, MSG_DONTROUTE, MSG_DONTWAIT, MSG_EOR, MSG_ERRQUEUE,
    MSG_FASTOPEN, MSG_FIN, MSG_MORE, MSG_NOSIGNAL, MSG_PEEK, MSG_PROXY, MSG_RST, MSG_SYN,
    MSG_TRUNC, MSG_TRYHARD, MSG_WAITALL, MSG_WAITFORONE, SOCK_DCCP, SOCK_DGRAM, SOCK_PACKET,
    SOCK_RAW, SOCK_RDM, SOCK_SEQPACKET, SOCK_STREAM, error, sockaddr_in, sockaddr_in6, sockaddr_ll,
    sockaddr_nl, sockaddr_qrtr, sockaddr_un, sockaddr_vm, uapi,
};
use zerocopy::{FromBytes, IntoBytes};

pub use syncio::ZxioShutdownFlags as SocketShutdownFlags;

uapi::check_arch_independent_layout! {
    __kernel_sa_family_t {}

    sockaddr_storage {}

    sockaddr {
        sa_family,
        sa_data,
    }

    sockaddr_in {
        sin_family,
        sin_port,
        sin_addr,
    }

    sockaddr_in6 {
        sin6_family,
        sin6_port,
        sin6_flowinfo,
        sin6_addr,
        sin6_scope_id
    }

    in_addr {
        s_addr,
    }
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct SocketMessageFlags: u32 {
        const PEEK = MSG_PEEK;
        const DONTROUTE = MSG_DONTROUTE;
        const TRYHARD = MSG_TRYHARD;
        const CTRUNC = MSG_CTRUNC;
        const PROXY = MSG_PROXY;
        const TRUNC = MSG_TRUNC;
        const DONTWAIT = MSG_DONTWAIT;
        const EOR = MSG_EOR;
        const WAITALL = MSG_WAITALL;
        const FIN = MSG_FIN;
        const SYN = MSG_SYN;
        const CONFIRM = MSG_CONFIRM;
        const RST = MSG_RST;
        const ERRQUEUE = MSG_ERRQUEUE;
        const NOSIGNAL = MSG_NOSIGNAL;
        const MORE = MSG_MORE;
        const WAITFORONE = MSG_WAITFORONE;
        const BATCH = MSG_BATCH;
        const FASTOPEN = MSG_FASTOPEN;
        const CMSG_CLOEXEC = MSG_CMSG_CLOEXEC;
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SocketDomain {
    /// The `Unix` socket domain contains sockets that were created with the `AF_UNIX` domain. These
    /// sockets communicate locally, with other sockets on the same host machine.
    Unix,

    /// An AF_VSOCK socket for communication from a controlling operating system
    Vsock,

    /// An AF_INET socket.
    Inet,

    /// An AF_INET6 socket.
    Inet6,

    /// An IF_NETLINK socket (currently stubbed out).
    Netlink,

    /// An AF_PACKET socket.
    Packet,

    /// An AF_KEY socket.
    Key,

    /// An AF_QIPCRTR socket.
    Qipcrtr,
}

/// The AF_KEY constant does not appear in the Linux UAPI headers. Instead, the value is defined in
/// libc headers.
const AF_KEY: u16 = 15;

impl SocketDomain {
    pub fn from_raw(raw: u16) -> Option<SocketDomain> {
        match raw {
            AF_UNIX => Some(Self::Unix),
            AF_VSOCK => Some(Self::Vsock),
            AF_INET => Some(Self::Inet),
            AF_INET6 => Some(Self::Inet6),
            AF_NETLINK => Some(Self::Netlink),
            AF_PACKET => Some(Self::Packet),
            AF_KEY => Some(Self::Key),
            AF_QIPCRTR => Some(Self::Qipcrtr),
            _ => None,
        }
    }

    pub fn as_raw(self) -> u16 {
        match self {
            Self::Unix => AF_UNIX,
            Self::Vsock => AF_VSOCK,
            Self::Inet => AF_INET,
            Self::Inet6 => AF_INET6,
            Self::Netlink => AF_NETLINK,
            Self::Packet => AF_PACKET,
            Self::Key => AF_KEY,
            Self::Qipcrtr => AF_QIPCRTR,
        }
    }

    pub fn is_inet(self) -> bool {
        matches!(self, SocketDomain::Inet | SocketDomain::Inet6)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SocketType {
    Stream,
    Datagram,
    Raw,
    Rdm,
    SeqPacket,
    Dccp,
    Packet,
}

impl SocketType {
    pub fn from_raw(raw: u32) -> Option<SocketType> {
        match raw {
            SOCK_STREAM => Some(SocketType::Stream),
            SOCK_DGRAM => Some(SocketType::Datagram),
            SOCK_RAW => Some(SocketType::Raw),
            SOCK_RDM => Some(SocketType::Rdm),
            SOCK_SEQPACKET => Some(SocketType::SeqPacket),
            SOCK_DCCP => Some(SocketType::Dccp),
            SOCK_PACKET => Some(SocketType::Packet),
            _ => None,
        }
    }

    pub fn as_raw(&self) -> u32 {
        match self {
            SocketType::Stream => SOCK_STREAM,
            SocketType::Datagram => SOCK_DGRAM,
            SocketType::Raw => SOCK_RAW,
            SocketType::Rdm => SOCK_RDM,
            SocketType::SeqPacket => SOCK_SEQPACKET,
            SocketType::Dccp => SOCK_DCCP,
            SocketType::Packet => SOCK_PACKET,
        }
    }

    pub fn is_connection_oriented(&self) -> bool {
        match self {
            SocketType::Stream | SocketType::SeqPacket => true,
            SocketType::Datagram
            | SocketType::Raw
            | SocketType::Rdm
            | SocketType::Dccp
            | SocketType::Packet => false,
        }
    }
}

#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct SocketProtocol(u32);

impl SocketProtocol {
    pub const UDP: SocketProtocol = SocketProtocol(uapi::IPPROTO_UDP);
    pub const UDPLITE: SocketProtocol = SocketProtocol(uapi::IPPROTO_UDPLITE);
    pub const TCP: SocketProtocol = SocketProtocol(uapi::IPPROTO_TCP);
    pub const IP: SocketProtocol = SocketProtocol(uapi::IPPROTO_IP);
    pub const ICMP: SocketProtocol = SocketProtocol(uapi::IPPROTO_ICMP);
    pub const ICMPV6: SocketProtocol = SocketProtocol(uapi::IPPROTO_ICMPV6);
    pub const SCTP: SocketProtocol = SocketProtocol(uapi::IPPROTO_SCTP);

    pub fn from_raw(protocol: u32) -> Self {
        SocketProtocol(protocol)
    }

    pub fn as_raw(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddress {
    /// An address in the AF_UNSPEC domain.
    Unspecified,

    /// A `Unix` socket address contains the filesystem path that was used to bind the socket.
    Unix(FsString),

    /// An AF_VSOCK socket is referred to by its listening port and cid.
    Vsock { port: u32, cid: u32 },

    /// AF_INET socket addresses are passed through as a sockaddr* to zxio.
    Inet(Vec<u8>),

    /// AF_INET6 socket addresses are passed through as a sockaddr* to zxio.
    Inet6(Vec<u8>),

    /// AF_NETLINK addresses contain a unicast pid and multicast groups.
    Netlink(NetlinkAddress),

    /// AF_PACKET socket addresses are passed through as a sockaddr* to zxio.
    Packet(Vec<u8>),

    /// AF_QIPCRTR socket addresses are passed through as a sockaddr* to zxio.
    Qipcrtr(Vec<u8>),
}

pub const SA_FAMILY_SIZE: usize = std::mem::size_of::<uapi::__kernel_sa_family_t>();
pub const SA_STORAGE_SIZE: usize = std::mem::size_of::<uapi::sockaddr_storage>();

impl SocketAddress {
    pub fn default_for_domain(domain: SocketDomain) -> SocketAddress {
        match domain {
            SocketDomain::Unix => SocketAddress::Unix(FsString::default()),
            SocketDomain::Vsock => SocketAddress::Vsock { port: 0xffff, cid: 0 },
            SocketDomain::Inet => {
                SocketAddress::Inet(uapi::sockaddr_in::default().as_bytes().to_vec())
            }
            SocketDomain::Inet6 => {
                SocketAddress::Inet6(uapi::sockaddr_in6::default().as_bytes().to_vec())
            }
            SocketDomain::Netlink => SocketAddress::Netlink(NetlinkAddress::default()),
            SocketDomain::Packet => {
                SocketAddress::Packet(uapi::sockaddr_ll::default().as_bytes().to_vec())
            }
            SocketDomain::Key => {
                // We currently stub AF_KEY domain sockets as Unix domain sockets.
                SocketAddress::Unix(FsString::default())
            }
            SocketDomain::Qipcrtr => {
                SocketAddress::Qipcrtr(uapi::sockaddr_qrtr::default().as_bytes().to_vec())
            }
        }
    }

    pub fn from_bytes(address: Vec<u8>) -> Result<SocketAddress, Errno> {
        let mut family_bytes = [0u8; SA_FAMILY_SIZE];
        family_bytes[..SA_FAMILY_SIZE].copy_from_slice(&address[..SA_FAMILY_SIZE]);
        let family = uapi::__kernel_sa_family_t::from_ne_bytes(family_bytes);

        let address = match family {
            AF_UNIX => {
                let template = sockaddr_un::default();
                let sun_path = &address[SA_FAMILY_SIZE..];
                if sun_path.len() > template.sun_path.len() {
                    return error!(EINVAL);
                }
                if sun_path.is_empty() {
                    // Possibly an autobind address, depending on context.
                    SocketAddress::Unix(Default::default())
                } else {
                    let null_index =
                        sun_path.iter().position(|&r| r == b'\0').unwrap_or(sun_path.len());
                    if null_index == 0 {
                        // If there is a null byte at the start of the sun_path, then the
                        // address is abstract.
                        SocketAddress::Unix(sun_path.into())
                    } else {
                        // Otherwise, the name is a path.
                        SocketAddress::Unix(sun_path[..null_index].into())
                    }
                }
            }
            AF_VSOCK => {
                let vsock_address = sockaddr_vm::read_from_bytes(&*address);
                if let Ok(address) = vsock_address {
                    SocketAddress::Vsock { port: address.svm_port, cid: address.svm_cid }
                } else {
                    SocketAddress::Unspecified
                }
            }
            AF_INET => {
                let sockaddr_len = std::mem::size_of::<sockaddr_in>();
                let addrlen = std::cmp::min(address.len(), sockaddr_len);
                SocketAddress::Inet(address[..addrlen].to_vec())
            }
            AF_INET6 => {
                let sockaddr_len = std::mem::size_of::<sockaddr_in6>();
                let addrlen = std::cmp::min(address.len(), sockaddr_len);
                SocketAddress::Inet6(address[..addrlen].to_vec())
            }
            AF_NETLINK => match sockaddr_nl::read_from_bytes(&*address) {
                Ok(addr) => {
                    SocketAddress::Netlink(NetlinkAddress::new(addr.nl_pid, addr.nl_groups))
                }
                Err(_) => return error!(EINVAL),
            },
            AF_PACKET => {
                let sockaddr_len = std::mem::size_of::<sockaddr_ll>();
                let addrlen = std::cmp::min(address.len(), sockaddr_len);
                SocketAddress::Packet(address[..addrlen].to_vec())
            }
            AF_QIPCRTR => {
                let sockaddr_len = std::mem::size_of::<sockaddr_qrtr>();
                let addrlen = std::cmp::min(address.len(), sockaddr_len);
                SocketAddress::Qipcrtr(address[..addrlen].to_vec())
            }
            _ => SocketAddress::Unspecified,
        };
        Ok(address)
    }

    pub fn valid_for_domain(&self, domain: SocketDomain) -> bool {
        match self {
            SocketAddress::Unspecified => false,
            SocketAddress::Unix(_) => domain == SocketDomain::Unix,
            SocketAddress::Vsock { .. } => domain == SocketDomain::Vsock,
            SocketAddress::Inet(_) => domain == SocketDomain::Inet,
            SocketAddress::Inet6(_) => domain == SocketDomain::Inet6,
            SocketAddress::Netlink(_) => domain == SocketDomain::Netlink,
            SocketAddress::Packet(_) => domain == SocketDomain::Packet,
            SocketAddress::Qipcrtr(_) => domain == SocketDomain::Qipcrtr,
        }
    }

    pub fn maybe_inet_port(&self) -> Option<u16> {
        match self {
            SocketAddress::Inet(addr) => {
                if let Ok((parsed, _)) = sockaddr_in::ref_from_prefix(addr) {
                    Some(u16::from_be(parsed.sin_port))
                } else {
                    None
                }
            }
            SocketAddress::Inet6(addr) => {
                if let Ok((parsed, _)) = sockaddr_in6::ref_from_prefix(addr) {
                    Some(u16::from_be(parsed.sin6_port))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            SocketAddress::Unspecified => AF_UNSPEC.to_ne_bytes().to_vec(),
            SocketAddress::Unix(name) => match name.first() {
                None => AF_UNIX.to_ne_bytes().to_vec(),
                Some(&b'\0') => [&AF_UNIX.to_ne_bytes(), name.as_bytes()].concat(),
                Some(_) => {
                    let template = sockaddr_un::default();
                    let path_length = std::cmp::min(template.sun_path.len() - 1, name.len());
                    [&AF_UNIX.to_ne_bytes(), &name[..path_length], &[b'\0']].concat()
                }
            },
            SocketAddress::Vsock { port, cid } => {
                let mut bytes = vec![0u8; std::mem::size_of::<sockaddr_vm>()];
                let vm_addr = sockaddr_vm {
                    svm_family: AF_VSOCK,
                    svm_port: *port,
                    svm_cid: *cid,
                    ..sockaddr_vm::default()
                };
                let _ = vm_addr.write_to(&mut bytes[..]);
                bytes
            }
            SocketAddress::Inet(addr)
            | SocketAddress::Inet6(addr)
            | SocketAddress::Packet(addr)
            | SocketAddress::Qipcrtr(addr) => addr.to_vec(),
            SocketAddress::Netlink(addr) => addr.to_bytes(),
        }
    }

    pub fn is_abstract_unix(&self) -> bool {
        match self {
            SocketAddress::Unix(name) => name.first() == Some(&b'\0'),
            _ => false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum SocketBpfState {
    Established,
    SynSent,
    SynRecv,
    FinWait1,
    FinWait2,
    TimeWait,
    #[default]
    Close,
    CloseWait,
    LastAck,
    Listen,
    Closing,
    NewSynRecv,
}

impl SocketBpfState {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            linux_uapi::BPF_TCP_ESTABLISHED => Some(Self::Established),
            linux_uapi::BPF_TCP_SYN_SENT => Some(Self::SynSent),
            linux_uapi::BPF_TCP_SYN_RECV => Some(Self::SynRecv),
            linux_uapi::BPF_TCP_FIN_WAIT1 => Some(Self::FinWait1),
            linux_uapi::BPF_TCP_FIN_WAIT2 => Some(Self::FinWait2),
            linux_uapi::BPF_TCP_TIME_WAIT => Some(Self::TimeWait),
            linux_uapi::BPF_TCP_CLOSE => Some(Self::Close),
            linux_uapi::BPF_TCP_CLOSE_WAIT => Some(Self::CloseWait),
            linux_uapi::BPF_TCP_LAST_ACK => Some(Self::LastAck),
            linux_uapi::BPF_TCP_LISTEN => Some(Self::Listen),
            linux_uapi::BPF_TCP_CLOSING => Some(Self::Closing),
            linux_uapi::BPF_TCP_NEW_SYN_RECV => Some(Self::NewSynRecv),
            _ => None,
        }
    }

    pub fn as_raw(&self) -> u32 {
        match self {
            Self::Established => linux_uapi::BPF_TCP_ESTABLISHED,
            Self::SynSent => linux_uapi::BPF_TCP_SYN_SENT,
            Self::SynRecv => linux_uapi::BPF_TCP_SYN_RECV,
            Self::FinWait1 => linux_uapi::BPF_TCP_FIN_WAIT1,
            Self::FinWait2 => linux_uapi::BPF_TCP_FIN_WAIT2,
            Self::TimeWait => linux_uapi::BPF_TCP_TIME_WAIT,
            Self::Close => linux_uapi::BPF_TCP_CLOSE,
            Self::CloseWait => linux_uapi::BPF_TCP_CLOSE_WAIT,
            Self::LastAck => linux_uapi::BPF_TCP_LAST_ACK,
            Self::Listen => linux_uapi::BPF_TCP_LISTEN,
            Self::Closing => linux_uapi::BPF_TCP_CLOSING,
            Self::NewSynRecv => linux_uapi::BPF_TCP_NEW_SYN_RECV,
        }
    }
}

impl From<SocketBpfState> for u32 {
    fn from(state: SocketBpfState) -> Self {
        state.as_raw()
    }
}
