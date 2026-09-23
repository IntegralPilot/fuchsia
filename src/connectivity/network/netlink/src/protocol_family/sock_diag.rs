// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for handling the `NETLINK_SOCK_DIAG` API.

mod eventloop;
mod request;

pub(crate) use eventloop::SockDiagEventLoop;
pub(crate) use request::NetlinkSockDiagRequestHandler;

use std::convert::Infallible as Never;
use std::num::NonZeroU32;

use fidl_fuchsia_net_sockets_ext as fnet_sockets_ext;
use netlink_packet_sock_diag::{SockDiagRequest, SockDiagResponse};

use crate::client::{AsyncWorkCompletionWaiter, ExternalClient};
use crate::messaging::{MessageWithPermission, Sender};
use crate::multicast_groups::{
    GroupSupport, InvalidLegacyGroupsError, InvalidModernGroupError, LegacyGroups, ModernGroup,
    MulticastCapableNetlinkFamily,
};
use crate::protocol_family::{NamedNetlinkFamily, NetlinkClient, ProtocolFamily};

/// An implementation of the `NETLINK_SOCK_DIAG` protocol family.
pub(crate) struct NetlinkSockDiag;

impl MulticastCapableNetlinkFamily for NetlinkSockDiag {
    fn check_support(group: &ModernGroup) -> Result<GroupSupport, InvalidModernGroupError> {
        if group.0 == linux_uapi::sknetlink_groups_SKNLGRP_NONE {
            Ok(GroupSupport::Unsupported)
        } else {
            NetlinkSockDiagNotifiedGroup::try_from(*group).map(|_| GroupSupport::Supported)
        }
    }
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) enum NetlinkSockDiagNotifiedGroup {
    TcpV4Destroy,
    TcpV6Destroy,
    UdpV4Destroy,
    UdpV6Destroy,
}

impl TryFrom<ModernGroup> for NetlinkSockDiagNotifiedGroup {
    type Error = InvalidModernGroupError;

    fn try_from(ModernGroup(group): ModernGroup) -> Result<Self, Self::Error> {
        match group {
            linux_uapi::sknetlink_groups_SKNLGRP_INET_TCP_DESTROY => {
                Ok(NetlinkSockDiagNotifiedGroup::TcpV4Destroy)
            }
            linux_uapi::sknetlink_groups_SKNLGRP_INET6_TCP_DESTROY => {
                Ok(NetlinkSockDiagNotifiedGroup::TcpV6Destroy)
            }
            linux_uapi::sknetlink_groups_SKNLGRP_INET_UDP_DESTROY => {
                Ok(NetlinkSockDiagNotifiedGroup::UdpV4Destroy)
            }
            linux_uapi::sknetlink_groups_SKNLGRP_INET6_UDP_DESTROY => {
                Ok(NetlinkSockDiagNotifiedGroup::UdpV6Destroy)
            }
            _ => Err(InvalidModernGroupError),
        }
    }
}

impl From<NetlinkSockDiagNotifiedGroup> for ModernGroup {
    fn from(group: NetlinkSockDiagNotifiedGroup) -> Self {
        match group {
            NetlinkSockDiagNotifiedGroup::TcpV4Destroy => {
                ModernGroup(linux_uapi::sknetlink_groups_SKNLGRP_INET_TCP_DESTROY)
            }
            NetlinkSockDiagNotifiedGroup::TcpV6Destroy => {
                ModernGroup(linux_uapi::sknetlink_groups_SKNLGRP_INET6_TCP_DESTROY)
            }
            NetlinkSockDiagNotifiedGroup::UdpV4Destroy => {
                ModernGroup(linux_uapi::sknetlink_groups_SKNLGRP_INET_UDP_DESTROY)
            }
            NetlinkSockDiagNotifiedGroup::UdpV6Destroy => {
                ModernGroup(linux_uapi::sknetlink_groups_SKNLGRP_INET6_UDP_DESTROY)
            }
        }
    }
}

impl NetlinkSockDiagNotifiedGroup {
    pub(crate) fn from_socket_state(socket: &fnet_sockets_ext::IpSocketState) -> Self {
        match socket {
            fnet_sockets_ext::IpSocketState::V4(s) => match &s.transport {
                fnet_sockets_ext::IpSocketTransportState::Tcp(_) => {
                    NetlinkSockDiagNotifiedGroup::TcpV4Destroy
                }
                fnet_sockets_ext::IpSocketTransportState::Udp(_) => {
                    NetlinkSockDiagNotifiedGroup::UdpV4Destroy
                }
            },
            fnet_sockets_ext::IpSocketState::V6(s) => match &s.transport {
                fnet_sockets_ext::IpSocketTransportState::Tcp(_) => {
                    NetlinkSockDiagNotifiedGroup::TcpV6Destroy
                }
                fnet_sockets_ext::IpSocketTransportState::Udp(_) => {
                    NetlinkSockDiagNotifiedGroup::UdpV6Destroy
                }
            },
        }
    }
}

impl MessageWithPermission for SockDiagRequest {
    fn permission(&self) -> crate::messaging::Permission {
        match self {
            SockDiagRequest::InetRequest(_) | SockDiagRequest::UnixRequest(_) => {
                crate::messaging::Permission::NetlinkSockDiagRead
            }
            SockDiagRequest::InetSockDestroy(_) => {
                crate::messaging::Permission::NetlinkSockDiagDestroy
            }
        }
    }
}

/// A connection to the `NETLINK_SOCK_DIAG` protocol family.
pub struct NetlinkSockDiagClient(pub(crate) ExternalClient<NetlinkSockDiag>);

impl NetlinkClient for NetlinkSockDiagClient {
    type Request = SockDiagRequest;

    fn set_pid(&self, pid: NonZeroU32) {
        let NetlinkSockDiagClient(client) = self;
        client.set_port_number(pid)
    }

    fn add_membership(
        &self,
        group: ModernGroup,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidModernGroupError> {
        let NetlinkSockDiagClient(client) = self;
        client.add_membership(group)
    }

    fn del_membership(&self, group: ModernGroup) -> Result<(), InvalidModernGroupError> {
        let NetlinkSockDiagClient(client) = self;
        client.del_membership(group)
    }

    fn set_legacy_memberships(
        &self,
        legacy_memberships: LegacyGroups,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidLegacyGroupsError> {
        let NetlinkSockDiagClient(client) = self;
        client.set_legacy_memberships(legacy_memberships)
    }
}

impl NamedNetlinkFamily for NetlinkSockDiag {
    const NAME: &'static str = "NETLINK_SOCK_DIAG";
}

impl ProtocolFamily for NetlinkSockDiag {
    type Request = SockDiagRequest;
    type Response = SockDiagResponse;
    type RequestHandler<S: Sender<Self::Response>> = NetlinkSockDiagRequestHandler<S>;
    type NotifiedMulticastGroup = NetlinkSockDiagNotifiedGroup;
    type AsyncWorkItem = Never;

    fn should_notify_on_group_membership_change(
        group: ModernGroup,
    ) -> Option<Self::NotifiedMulticastGroup> {
        NetlinkSockDiagNotifiedGroup::try_from(group).ok()
    }
}

#[cfg(test)]
mod testutil {
    use net_declare::{std_ip_v4, std_ip_v6};
    use net_types::ip::{Ip, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};

    pub(crate) trait TestIpExt: Ip {
        const SRC_ADDR: Self::Addr;
        const DST_ADDR: Self::Addr;
        const LINUX_FAMILY: u8;
    }

    impl TestIpExt for Ipv4 {
        const SRC_ADDR: Ipv4Addr = Ipv4Addr::new(std_ip_v4!("192.168.0.1").octets());
        const DST_ADDR: Ipv4Addr = Ipv4Addr::new(std_ip_v4!("192.168.0.2").octets());
        const LINUX_FAMILY: u8 = linux_uapi::AF_INET as u8;
    }

    impl TestIpExt for Ipv6 {
        const SRC_ADDR: Ipv6Addr = Ipv6Addr::new(std_ip_v6!("2001:db8::1").segments());
        const DST_ADDR: Ipv6Addr = Ipv6Addr::new(std_ip_v6!("2001:db8::2").segments());
        const LINUX_FAMILY: u8 = linux_uapi::AF_INET6 as u8;
    }
}
