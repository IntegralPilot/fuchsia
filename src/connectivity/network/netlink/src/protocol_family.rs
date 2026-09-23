// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for managing protocol-specific aspects of Netlink.

pub mod sock_diag;

use netlink_packet_core::{
    NetlinkDeserializable, NetlinkMessage, NetlinkPayload, NetlinkSerializable,
};
use netlink_packet_utils::DecodeError;

use std::fmt::Debug;
use std::hash::Hash;
use std::num::NonZeroU32;

use crate::client::{AsyncWorkCompletionWaiter, ExternalClient, InternalClient};
use crate::logging::{log_debug, log_warn};
use crate::messaging::{MessageWithPermission, Sender};
use crate::multicast_groups::{
    InvalidLegacyGroupsError, InvalidModernGroupError, LegacyGroups, ModernGroup,
    MulticastCapableNetlinkFamily,
};
use crate::route_tables::{NetlinkRouteTableIndex, NonZeroNetlinkRouteTableIndex};

/// A type representing a named Netlink family.
pub(crate) trait NamedNetlinkFamily {
    const NAME: &'static str;
}

/// A type representing a Netlink Protocol Family.
pub(crate) trait ProtocolFamily:
    MulticastCapableNetlinkFamily + NamedNetlinkFamily + Send + Sized
{
    /// The request message type associated with the protocol family.
    type Request: Clone
        + Debug
        + NetlinkDeserializable<Error: Into<DecodeError>>
        + MessageWithPermission
        + Send;

    /// The response message type associated with the protocol family.
    type Response: Clone + Debug + NetlinkSerializable + Send;

    /// The implementation for handling requests from this protocol family.
    type RequestHandler<S: Sender<Self::Response>>: NetlinkFamilyRequestHandler<Self, S>;

    type NotifiedMulticastGroup: PartialEq + Eq + Hash + Clone + Copy + Debug + Send;

    /// Family-specific async work to be passed through
    /// [`crate::client::AsyncWorkItem`].
    type AsyncWorkItem: Debug;

    /// Returns `true` if we may need to notify the worker event loop in
    /// response to a client joining or leaving the given `ModernGroup`.
    fn should_notify_on_group_membership_change(
        group: ModernGroup,
    ) -> Option<Self::NotifiedMulticastGroup>;
}

/// A request handler implementation for a particular Netlink protocol family.
pub(crate) trait NetlinkFamilyRequestHandler<F: ProtocolFamily, S: Sender<F::Response>>:
    Clone + Send
{
    /// Handles the given request and generates the associated response(s).
    async fn handle_request(
        &mut self,
        req: NetlinkMessage<F::Request>,
        client: &mut InternalClient<F, S>,
    );
}

/// A client of a Netlink protocol family.
pub trait NetlinkClient: Send + Sync {
    /// The request message type associated with this client's protocol family.
    type Request: Clone
        + Debug
        + NetlinkDeserializable<Error: Into<DecodeError>>
        + MessageWithPermission
        + Send;

    /// Sets the PID assigned to the client.
    fn set_pid(&self, pid: NonZeroU32);

    /// Adds the given multicast group membership.
    fn add_membership(
        &self,
        group: ModernGroup,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidModernGroupError>;

    /// Deletes the given multicast group membership.
    fn del_membership(&self, group: ModernGroup) -> Result<(), InvalidModernGroupError>;

    /// Sets the legacy multicast group memberships.
    fn set_legacy_memberships(
        &self,
        legacy_memberships: LegacyGroups,
    ) -> Result<AsyncWorkCompletionWaiter, InvalidLegacyGroupsError>;
}

pub mod route {
    //! This module implements the Route Netlink Protocol Family.

    use super::*;

    use std::fmt::Display;
    use std::num::NonZeroU64;

    use fidl_fuchsia_net_routes_ext as fnet_routes_ext;

    use either::Either;
    use futures::channel::{mpsc, oneshot};
    use futures::sink::SinkExt as _;
    use linux_uapi::{
        IFA_F_NOPREFIXROUTE, rt_class_t_RT_TABLE_COMPAT, rt_class_t_RT_TABLE_MAIN,
        rtnetlink_groups_RTNLGRP_DCB, rtnetlink_groups_RTNLGRP_DECnet_IFADDR,
        rtnetlink_groups_RTNLGRP_DECnet_ROUTE, rtnetlink_groups_RTNLGRP_DECnet_RULE,
        rtnetlink_groups_RTNLGRP_IPV4_IFADDR, rtnetlink_groups_RTNLGRP_IPV4_MROUTE,
        rtnetlink_groups_RTNLGRP_IPV4_MROUTE_R, rtnetlink_groups_RTNLGRP_IPV4_NETCONF,
        rtnetlink_groups_RTNLGRP_IPV4_ROUTE, rtnetlink_groups_RTNLGRP_IPV4_RULE,
        rtnetlink_groups_RTNLGRP_IPV6_IFADDR, rtnetlink_groups_RTNLGRP_IPV6_IFINFO,
        rtnetlink_groups_RTNLGRP_IPV6_MROUTE, rtnetlink_groups_RTNLGRP_IPV6_MROUTE_R,
        rtnetlink_groups_RTNLGRP_IPV6_NETCONF, rtnetlink_groups_RTNLGRP_IPV6_PREFIX,
        rtnetlink_groups_RTNLGRP_IPV6_ROUTE, rtnetlink_groups_RTNLGRP_IPV6_RULE,
        rtnetlink_groups_RTNLGRP_LINK, rtnetlink_groups_RTNLGRP_MDB,
        rtnetlink_groups_RTNLGRP_MPLS_NETCONF, rtnetlink_groups_RTNLGRP_MPLS_ROUTE,
        rtnetlink_groups_RTNLGRP_ND_USEROPT, rtnetlink_groups_RTNLGRP_NEIGH,
        rtnetlink_groups_RTNLGRP_NONE, rtnetlink_groups_RTNLGRP_NOP2,
        rtnetlink_groups_RTNLGRP_NOP4, rtnetlink_groups_RTNLGRP_NOTIFY,
        rtnetlink_groups_RTNLGRP_NSID, rtnetlink_groups_RTNLGRP_PHONET_IFADDR,
        rtnetlink_groups_RTNLGRP_PHONET_ROUTE, rtnetlink_groups_RTNLGRP_TC,
    };
    use net_types::SpecifiedAddr;
    use net_types::ip::{
        AddrSubnetEither, AddrSubnetError, Ip, IpAddr, IpInvariant, IpVersion, Ipv4, Ipv4Addr,
        Ipv6, Ipv6Addr, Subnet,
    };
    use netlink_packet_route::address::{AddressAttribute, AddressMessage};
    use netlink_packet_route::link::{InfoKind, LinkAttribute, LinkFlags, LinkInfo, LinkMessage};
    use netlink_packet_route::route::{RouteAttribute, RouteMessage, RouteType};
    use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};

    use crate::client::AsyncWorkCompletionWaiter;
    use crate::interfaces::AcceptRaRtTable;
    use crate::messaging::{MessageWithPermission, Permission};
    use crate::multicast_groups::GroupSupport;
    use crate::neighbors::{
        DelNeighborArgs, GetNeighborArgs, NeighborRequestArgs, NewNeighborArgs,
    };
    use crate::netlink_packet::errno::Errno;
    use crate::netlink_packet::{self, NetlinkRequestType};
    use crate::route_eventloop::UnifiedRequest;
    use crate::rules::{RuleRequest, RuleRequestArgs};
    use crate::{SysctlError, SysctlInterfaceSelector, interfaces, neighbors, routes};

    use netlink_packet_core::{NLM_F_ACK, NLM_F_DUMP, NLM_F_REPLACE, NetlinkHeader};

    /// An implementation of the Netlink Route protocol family.
    pub(crate) enum NetlinkRoute {}

    impl MulticastCapableNetlinkFamily for NetlinkRoute {
        #[allow(non_upper_case_globals)]
        fn check_support(
            ModernGroup(group): &ModernGroup,
        ) -> Result<GroupSupport, InvalidModernGroupError> {
            match *group {
                rtnetlink_groups_RTNLGRP_IPV4_IFADDR
                | rtnetlink_groups_RTNLGRP_IPV4_ROUTE
                | rtnetlink_groups_RTNLGRP_IPV6_IFADDR
                | rtnetlink_groups_RTNLGRP_IPV6_ROUTE
                | rtnetlink_groups_RTNLGRP_LINK
                | rtnetlink_groups_RTNLGRP_ND_USEROPT
                | rtnetlink_groups_RTNLGRP_NEIGH => Ok(GroupSupport::Supported),
                rtnetlink_groups_RTNLGRP_DCB
                | rtnetlink_groups_RTNLGRP_DECnet_IFADDR
                | rtnetlink_groups_RTNLGRP_DECnet_ROUTE
                | rtnetlink_groups_RTNLGRP_DECnet_RULE
                | rtnetlink_groups_RTNLGRP_IPV4_MROUTE
                | rtnetlink_groups_RTNLGRP_IPV4_MROUTE_R
                | rtnetlink_groups_RTNLGRP_IPV4_NETCONF
                | rtnetlink_groups_RTNLGRP_IPV4_RULE
                | rtnetlink_groups_RTNLGRP_IPV6_IFINFO
                | rtnetlink_groups_RTNLGRP_IPV6_MROUTE
                | rtnetlink_groups_RTNLGRP_IPV6_MROUTE_R
                | rtnetlink_groups_RTNLGRP_IPV6_NETCONF
                | rtnetlink_groups_RTNLGRP_IPV6_PREFIX
                | rtnetlink_groups_RTNLGRP_IPV6_RULE
                | rtnetlink_groups_RTNLGRP_MDB
                | rtnetlink_groups_RTNLGRP_MPLS_NETCONF
                | rtnetlink_groups_RTNLGRP_MPLS_ROUTE
                | rtnetlink_groups_RTNLGRP_NONE
                | rtnetlink_groups_RTNLGRP_NOP2
                | rtnetlink_groups_RTNLGRP_NOP4
                | rtnetlink_groups_RTNLGRP_NOTIFY
                | rtnetlink_groups_RTNLGRP_NSID
                | rtnetlink_groups_RTNLGRP_PHONET_IFADDR
                | rtnetlink_groups_RTNLGRP_PHONET_ROUTE
                | rtnetlink_groups_RTNLGRP_TC => Ok(GroupSupport::Unsupported),
                _ => Err(InvalidModernGroupError),
            }
        }
    }

    #[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
    pub(crate) enum NetlinkRouteNotifiedGroup {
        Nduseropt,
    }

    impl MessageWithPermission for RouteNetlinkMessage {
        fn permission(&self) -> Permission {
            match self {
                Self::GetLink(_)
                | Self::GetAddress(_)
                | Self::GetNeighbour(_)
                | Self::GetNeighbourTable(_)
                | Self::GetRoute(_)
                | Self::GetQueueDiscipline(_)
                | Self::GetTrafficClass(_)
                | Self::GetTrafficFilter(_)
                | Self::GetTrafficChain(_)
                | Self::GetNsId(_)
                | Self::GetRule(_) => Permission::NetlinkRouteRead,

                Self::NewLink(_)
                | Self::DelLink(_)
                | Self::SetLink(_)
                | Self::NewLinkProp(_)
                | Self::DelLinkProp(_)
                | Self::NewAddress(_)
                | Self::DelAddress(_)
                | Self::NewNeighbour(_)
                | Self::DelNeighbour(_)
                | Self::NewNeighbourTable(_)
                | Self::SetNeighbourTable(_)
                | Self::NewNeighbourDiscoveryUserOption(_)
                | Self::NewRoute(_)
                | Self::DelRoute(_)
                | Self::NewPrefix(_)
                | Self::NewQueueDiscipline(_)
                | Self::DelQueueDiscipline(_)
                | Self::NewTrafficClass(_)
                | Self::DelTrafficClass(_)
                | Self::NewTrafficFilter(_)
                | Self::DelTrafficFilter(_)
                | Self::NewTrafficChain(_)
                | Self::DelTrafficChain(_)
                | Self::NewNsId(_)
                | Self::DelNsId(_)
                | Self::NewRule(_)
                | Self::DelRule(_) => Permission::NetlinkRouteWrite,
            }
        }
    }

    impl NamedNetlinkFamily for NetlinkRoute {
        const NAME: &'static str = "NETLINK_ROUTE";
    }

    impl ProtocolFamily for NetlinkRoute {
        type Request = RouteNetlinkMessage;
        type Response = RouteNetlinkMessage;
        type RequestHandler<S: Sender<Self::Response>> = NetlinkRouteRequestHandler<S>;
        type NotifiedMulticastGroup = NetlinkRouteNotifiedGroup;
        type AsyncWorkItem = RouteAsyncWork;

        fn should_notify_on_group_membership_change(
            group: ModernGroup,
        ) -> Option<Self::NotifiedMulticastGroup> {
            (group == ModernGroup(rtnetlink_groups_RTNLGRP_ND_USEROPT))
                .then_some(NetlinkRouteNotifiedGroup::Nduseropt)
        }
    }

    #[derive(Debug)]
    pub(crate) enum RouteAsyncWork {
        SetAcceptRaRtTable {
            interface: SysctlInterfaceSelector,
            value: AcceptRaRtTable,
            responder: oneshot_sync::Sender<Result<(), SysctlError>>,
        },
        GetAcceptRaRtTable {
            interface: SysctlInterfaceSelector,
            responder: oneshot_sync::Sender<Result<AcceptRaRtTable, SysctlError>>,
        },
    }

    #[derive(Clone)]
    pub(crate) struct NetlinkRouteRequestHandler<S: Sender<RouteNetlinkMessage>> {
        pub(crate) unified_request_sink: mpsc::Sender<UnifiedRequest<S>>,
    }

    struct ExtractedAddressRequest {
        address_and_interface_id: interfaces::AddressAndInterfaceArgs,
        addr_flags: u32,
    }

    fn extract_if_id_and_addr_from_addr_message(
        message: &AddressMessage,
        client: &impl Display,
        req: &RouteNetlinkMessage,
        // `true` for new address requests; `false` for delete address requests.
        is_new: bool,
    ) -> Result<Option<ExtractedAddressRequest>, Errno> {
        let kind = if is_new { "new" } else { "del" };

        let interface_id = match NonZeroU32::new(message.header.index.into()) {
            Some(interface_id) => interface_id,
            None => {
                log_debug!(
                    "unspecified interface ID in address {} request from {}: {:?}",
                    kind,
                    client,
                    req,
                );
                return Err(Errno::EINVAL);
            }
        };

        let mut address = None;
        let mut local = None;
        let mut addr_flags = None;
        message.attributes.iter().for_each(|nla| match nla {
            AddressAttribute::Address(a) => address = Some(a),
            AddressAttribute::Local(l) => local = Some(l),
            AddressAttribute::Flags(flags) => addr_flags = Some(*flags),
            nla => {
                log_warn!(
                    "unexpected Address NLA in {} request from {}: {:?}; req = {:?}",
                    kind,
                    client,
                    nla,
                    req,
                );
            }
        });

        // Linux supports the notion of a "peer" address which is used for
        // pointtopoint interfaces. Fuchsia does not support this so we do not
        // allow different-valued `IFA_LOCAL` and `IFA_ADDRESS` values.
        //
        // Per https://www.man7.org/linux/man-pages/man8/ip-address.8.html,
        //
        //   ip address add - add new protocol address.
        //       dev IFNAME
        //            the name of the device to add the address to.
        //
        //       local ADDRESS (default)
        //            the address of the interface. The format of the address
        //            depends on the protocol. It is a dotted quad for IP and a
        //            sequence of hexadecimal halfwords separated by colons for
        //            IPv6. The ADDRESS may be followed by a slash and a decimal
        //            number which encodes the network prefix length.
        //
        //       peer ADDRESS
        //            the address of the remote endpoint for pointopoint
        //            interfaces. Again, the ADDRESS may be followed by a slash
        //            and a decimal number, encoding the network prefix length.
        //            If a peer address is specified, the local address cannot
        //            have a prefix length. The network prefix is associated
        //            with the peer rather than with the local address.
        //
        //   ...
        //
        //   ip address delete - delete protocol address
        //       Arguments: coincide with the arguments of ip addr add. The
        //       device name is a required argument. The rest are optional. If
        //       no arguments are given, the first address is deleted.
        //
        // Note that when only one of `IFA_LOCAL` or `IFA_ADDRESS` is included
        // in a message, it is treated as the "local" address on the interface
        // to be added/removed. When both are included, `IFA_LOCAL` is treated
        // as the "local" address and `IFA_ADDRESS` is treated as the "peer".
        // TODO(https://fxbug.dev/42079868): Support peer addresses.
        let addr = match (local, address) {
            (Some(local), Some(address)) => {
                if local == address {
                    address
                } else {
                    log_debug!(
                        "got different `IFA_ADDRESS` and `IFA_LOCAL` values for {} address request from {}: {:?}",
                        kind,
                        client,
                        req,
                    );
                    return Err(Errno::ENOTSUP);
                }
            }
            (Some(addr), None) | (None, Some(addr)) => addr,
            (None, None) => {
                log_debug!(
                    "missing `IFA_ADDRESS` and `IFA_LOCAL` in address {} request from {}: {:?}",
                    kind,
                    client,
                    req,
                );
                return Err(Errno::EINVAL);
            }
        };

        let addr = match message.header.family() {
            AddressFamily::Inet => {
                if addr.is_unspecified() {
                    // Linux treats adding the unspecified IPv4 address as a
                    // no-op.
                    return Ok(None);
                }
                match addr {
                    std::net::IpAddr::V4(v4) => IpAddr::V4(Ipv4Addr::new(v4.octets())),
                    std::net::IpAddr::V6(_) => {
                        log_debug!(
                            "expected IPv4 address in new address request from {}: {:?}",
                            client,
                            req
                        );
                        return Err(Errno::EINVAL);
                    }
                }
            }
            AddressFamily::Inet6 => {
                if addr.is_unspecified() {
                    // Linux returns this error when adding the unspecified IPv6
                    // address.
                    return Err(Errno::EADDRNOTAVAIL);
                }
                match addr {
                    std::net::IpAddr::V4(_) => {
                        log_debug!(
                            "expected IPv6 address in new address request from {}: {:?}",
                            client,
                            req
                        );
                        return Err(Errno::EINVAL);
                    }
                    std::net::IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::new(v6.segments())),
                }
            }
            family => {
                log_debug!(
                    "invalid address family ({:?}) in new address \
                    request from {}: {:?}",
                    family,
                    client,
                    req
                );
                return Err(Errno::EINVAL);
            }
        };

        let address = match AddrSubnetEither::new(addr, message.header.prefix_len) {
            Ok(address) => address,
            Err(
                AddrSubnetError::PrefixTooLong
                | AddrSubnetError::NotUnicastInSubnet
                | AddrSubnetError::InvalidWitness,
            ) => {
                log_debug!(
                    "invalid address in address {} request from {}: {:?}",
                    kind,
                    client,
                    req
                );
                return Err(Errno::EINVAL);
            }
        };

        Ok(Some(ExtractedAddressRequest {
            address_and_interface_id: interfaces::AddressAndInterfaceArgs { address, interface_id },
            addr_flags: addr_flags
                .map(|value| value.bits())
                .unwrap_or_else(|| message.header.flags.into()),
        }))
    }

    /// Constructs the appropriate [`GetLinkArgs`] for this GetLink request.
    fn to_get_link_args(
        link_msg: LinkMessage,
        is_dump: bool,
    ) -> Result<interfaces::GetLinkArgs, Errno> {
        // NB: In the case where the request is "malformed" and specifies
        // multiple fields, Linux prefers the dump flag over the link index, and
        // prefers the link index over the link_name.
        if is_dump {
            return Ok(interfaces::GetLinkArgs::Dump);
        }
        if let Ok(link_id) = <u32 as TryInto<NonZeroU32>>::try_into(link_msg.header.index) {
            return Ok(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Index(link_id)));
        }
        if let Some(name) = link_msg.attributes.into_iter().find_map(|nla| match nla {
            LinkAttribute::IfName(name) => Some(name),
            nla => {
                log_debug!("ignoring unexpected NLA in GetLink request: {:?}", nla);
                None
            }
        }) {
            return Ok(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Name(name)));
        }
        return Err(Errno::EINVAL);
    }

    /// Constructs the appropriate [`SetLinkArgs`] for this SetLink request.
    fn to_set_link_args(link_msg: LinkMessage) -> Result<interfaces::SetLinkArgs, Errno> {
        let link_id = NonZeroU32::new(link_msg.header.index);
        let link_name = link_msg.attributes.into_iter().find_map(|nla| match nla {
            LinkAttribute::IfName(name) => Some(name),
            nla => {
                log_debug!("ignoring unexpected NLA in SetLink request: {:?}", nla);
                None
            }
        });
        let link = match (link_id, link_name) {
            (Some(id), None) => interfaces::LinkSpecifier::Index(id),
            (None, Some(name)) => interfaces::LinkSpecifier::Name(name),
            (None, None) => return Err(Errno::EINVAL),
            // NB: If both the index and name are specified, Linux returns EBUSY
            // rather than EINVAL. Do the same here for conformance.
            (Some(_id), Some(_name)) => return Err(Errno::EBUSY),
        };
        // `change_mask` specifies which flags should be updated, while `flags`
        // specifies whether the value should be set/unset.
        let enable: Option<bool> = ((link_msg.header.change_mask & LinkFlags::Up) == LinkFlags::Up)
            .then_some((link_msg.header.flags & LinkFlags::Up) != LinkFlags::empty());

        let unsupported_changes = link_msg.header.change_mask & !LinkFlags::Up;
        if unsupported_changes != LinkFlags::empty() {
            log_warn!(
                "ignoring unsupported changes in SetLink request: {:#X}",
                unsupported_changes
            );
        }

        Ok(interfaces::SetLinkArgs { link, enable })
    }

    #[derive(Debug)]
    enum ExtractedRouteRequest<I: Ip> {
        // A gateway or direct route.
        Unicast(ExtractedUnicastRouteRequest<I>),
    }
    /// Route request information from a route of type `RTN_UNICAST`.
    /// Fields other than subnet marked as optional to allow for translation
    /// to [`UnicastRouteArgs`] and [`UnicastDelRouteArgs`].
    #[derive(Debug)]
    struct ExtractedUnicastRouteRequest<I: Ip> {
        subnet: Subnet<I::Addr>,
        outbound_interface: Option<NonZeroU64>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        priority: Option<NonZeroU32>,
        table: NonZeroNetlinkRouteTableIndex,
    }

    // Extracts unicast route information from a request.
    // Returns an error if the request is malformed. This error
    // should be returned to the client.
    fn extract_data_from_unicast_route_message<I: Ip>(
        message: &RouteMessage,
        client: &impl Display,
        req: &RouteNetlinkMessage,
        kind: &str,
    ) -> Result<ExtractedRouteRequest<I>, Errno> {
        let destination_prefix_len = message.header.destination_prefix_length;
        let mut table: NetlinkRouteTableIndex =
            NetlinkRouteTableIndex::new(message.header.table.into());

        let mut destination_addr = None;
        let mut outbound_interface = None;
        let mut next_hop_addr = None;
        let mut priority = None;
        message.attributes.iter().for_each(|nla| match nla {
            RouteAttribute::Destination(addr) => destination_addr = Some(addr),
            RouteAttribute::Oif(id) => outbound_interface = Some(id),
            RouteAttribute::Gateway(addr) => next_hop_addr = Some(addr),
            RouteAttribute::Priority(num) => priority = Some(*num),
            RouteAttribute::Table(num) => {
                // When the table is set to `RT_TABLE_COMPAT`, the table id is greater than
                // `u8::MAX` and cannot be represented in the header u8. The actual table
                // number is stored in the `RTA_TABLE` NLA.
                // We expect to see the NLA if the value in the header is `RT_TABLE_COMPAT`,
                // although we will use it if it is present, regardless of if `RT_TABLE_COMPAT`
                // is specified.
                if message.header.table != rt_class_t_RT_TABLE_COMPAT as u8 {
                    log_debug!(
                        "`RTA_TABLE` is expected only when table in header is `RT_TABLE_COMPAT`, \
                        but it was {}. Using provided value of {} in the NLA",
                        message.header.table,
                        num
                    );
                }
                table = NetlinkRouteTableIndex::new(*num)
            }
            nla => {
                log_warn!(
                    "ignoring unexpected Route NLA in {} request from {}: {:?}; req = {:?}",
                    kind,
                    client,
                    nla,
                    req,
                );
            }
        });

        let outbound_interface =
            outbound_interface.map(|id| NonZeroU64::new((*id).into())).flatten();
        let priority = priority.map(NonZeroU32::new).flatten();
        let table = NonZeroNetlinkRouteTableIndex::new(table).unwrap_or_else(|| {
            NonZeroNetlinkRouteTableIndex::new_non_zero(
                NonZeroU32::new(rt_class_t_RT_TABLE_MAIN.into()).unwrap(),
            )
        });
        let destination_addr = match destination_addr {
            Some(addr) => crate::netlink_packet::ip_addr_from_route::<I>(addr)?,
            None => {
                // Use the unspecified address if there wasn't a destination NLA present
                // and the prefix len is 0.
                if destination_prefix_len != 0 {
                    log_warn!(
                        "rejecting route {} request with prefix length {} and missing `RTA_DST` \
                    from {}: {:?}",
                        kind,
                        destination_prefix_len,
                        client,
                        req
                    );
                    return Err(Errno::EINVAL);
                }
                I::UNSPECIFIED_ADDRESS
            }
        };

        let next_hop = match next_hop_addr {
            Some(addr) => {
                // Linux ignores the provided nexthop if it is the default route. To conform
                // to Linux expectations, `SpecifiedAddr::new()` becomes `None` when the addr
                // is unspecified.
                crate::netlink_packet::ip_addr_from_route::<I>(addr).map(SpecifiedAddr::new)?
            }
            None => None,
        };

        let extracted_route_request = match Subnet::new(destination_addr, destination_prefix_len) {
            Ok(subnet) => ExtractedRouteRequest::Unicast(ExtractedUnicastRouteRequest {
                subnet,
                outbound_interface,
                next_hop,
                priority,
                table,
            }),
            Err(e) => {
                log_warn!(
                    "{:?} subnet ({}) in route {} request from {}: {:?}",
                    e,
                    destination_addr,
                    kind,
                    client,
                    req,
                );
                return Err(Errno::EINVAL);
            }
        };

        Ok(extracted_route_request)
    }

    // Translates `RouteMessage` to `RouteRequestArgs::New`.
    //
    // `RouteRequestArgs::New` requires all fields except for next_hop, so optional
    // fields from `ExtractedRouteRequest` are converted to the defaults expected
    // by Netstack.
    fn to_new_route_args<I: Ip>(
        message: &RouteMessage,
        client: &impl Display,
        req: &RouteNetlinkMessage,
    ) -> Result<routes::RouteRequestArgs<I>, Errno> {
        let extracted_request =
            extract_data_from_unicast_route_message::<I>(message, client, req, "new")?;

        let ExtractedRouteRequest::Unicast(ExtractedUnicastRouteRequest {
            subnet,
            outbound_interface,
            next_hop,
            priority,
            table,
        }) = extracted_request;

        let outbound_interface = outbound_interface.map(NonZeroU64::get).ok_or_else(|| {
            // TODO(https://issues.fuchsia.dev/292103361): Resolve destination
            // IP to find interface index if it is not provided explicitly.
            log_warn!(
                "unsupported request: missing `RTA_OIF` in new route request from {}: {:?}",
                client,
                req
            );
            Errno::ENOTSUP
        })?;

        Ok(routes::RouteRequestArgs::New(routes::NewRouteArgs::Unicast(
            routes::UnicastNewRouteArgs {
                subnet,
                target: fnet_routes_ext::RouteTarget { outbound_interface, next_hop },
                priority,
                table: table.into(),
            },
        )))
    }

    // Translates `RouteMessage` to `RouteRequestArgs::Del`.
    fn to_del_route_args<I: Ip>(
        message: &RouteMessage,
        client: &impl Display,
        req: &RouteNetlinkMessage,
    ) -> Result<routes::RouteRequestArgs<I>, Errno> {
        let extracted_request =
            extract_data_from_unicast_route_message::<I>(message, client, req, "del")?;

        let ExtractedRouteRequest::Unicast(ExtractedUnicastRouteRequest {
            subnet,
            outbound_interface,
            next_hop,
            priority,
            table,
        }) = extracted_request;

        Ok(routes::RouteRequestArgs::Del(routes::DelRouteArgs::Unicast(
            routes::UnicastDelRouteArgs { subnet, outbound_interface, next_hop, priority, table },
        )))
    }

    impl From<&RouteNetlinkMessage> for NetlinkRequestType {
        fn from(message: &RouteNetlinkMessage) -> NetlinkRequestType {
            match message {
                RouteNetlinkMessage::GetLink(_)
                | RouteNetlinkMessage::GetAddress(_)
                | RouteNetlinkMessage::GetNeighbour(_)
                | RouteNetlinkMessage::GetNeighbourTable(_)
                | RouteNetlinkMessage::GetRoute(_)
                | RouteNetlinkMessage::GetQueueDiscipline(_)
                | RouteNetlinkMessage::GetTrafficClass(_)
                | RouteNetlinkMessage::GetTrafficFilter(_)
                | RouteNetlinkMessage::GetTrafficChain(_)
                | RouteNetlinkMessage::GetNsId(_)
                | RouteNetlinkMessage::GetRule(_) => Self::Get,

                RouteNetlinkMessage::NewLink(_)
                | RouteNetlinkMessage::NewLinkProp(_)
                | RouteNetlinkMessage::NewAddress(_)
                | RouteNetlinkMessage::NewNeighbour(_)
                | RouteNetlinkMessage::NewNeighbourTable(_)
                | RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(_)
                | RouteNetlinkMessage::NewRoute(_)
                | RouteNetlinkMessage::NewPrefix(_)
                | RouteNetlinkMessage::NewQueueDiscipline(_)
                | RouteNetlinkMessage::NewTrafficClass(_)
                | RouteNetlinkMessage::NewTrafficFilter(_)
                | RouteNetlinkMessage::NewTrafficChain(_)
                | RouteNetlinkMessage::NewNsId(_)
                | RouteNetlinkMessage::NewRule(_) => Self::New,

                RouteNetlinkMessage::SetLink(_) | RouteNetlinkMessage::SetNeighbourTable(_) => {
                    Self::Set
                }

                RouteNetlinkMessage::DelLink(_)
                | RouteNetlinkMessage::DelLinkProp(_)
                | RouteNetlinkMessage::DelAddress(_)
                | RouteNetlinkMessage::DelNeighbour(_)
                | RouteNetlinkMessage::DelRoute(_)
                | RouteNetlinkMessage::DelQueueDiscipline(_)
                | RouteNetlinkMessage::DelTrafficClass(_)
                | RouteNetlinkMessage::DelTrafficFilter(_)
                | RouteNetlinkMessage::DelTrafficChain(_)
                | RouteNetlinkMessage::DelNsId(_)
                | RouteNetlinkMessage::DelRule(_) => Self::Del,
            }
        }
    }

    impl<S: Sender<RouteNetlinkMessage>> NetlinkFamilyRequestHandler<NetlinkRoute, S>
        for NetlinkRouteRequestHandler<S>
    {
        async fn handle_request(
            &mut self,
            req: NetlinkMessage<RouteNetlinkMessage>,
            client: &mut InternalClient<NetlinkRoute, S>,
        ) {
            let Self { unified_request_sink } = self;

            let (req_header, payload) = req.into_parts();
            let req = match payload {
                NetlinkPayload::InnerMessage(p) => p,
                p => {
                    log_warn!(
                        "Ignoring request from client {} with unexpected payload: {:?}",
                        client,
                        p
                    );
                    return;
                }
            };

            let is_dump = req_header.flags & NLM_F_DUMP == NLM_F_DUMP;
            let is_replace = req_header.flags & NLM_F_REPLACE == NLM_F_REPLACE;
            let expects_ack = req_header.flags & NLM_F_ACK == NLM_F_ACK;

            use RouteNetlinkMessage::*;
            match req {
                GetLink(link_msg) => {
                    let (completer, waiter) = oneshot::channel();
                    let args = match to_get_link_args(link_msg, is_dump) {
                        Ok(args) => args,
                        Err(e) => {
                            log_debug!("received invalid `GetLink` request from {}", client);
                            client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                            return;
                        }
                    };
                    unified_request_sink
                        .send(UnifiedRequest::InterfacesRequest(interfaces::Request {
                            args: interfaces::RequestArgs::Link(interfaces::LinkRequestArgs::Get(
                                args,
                            )),
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                            completer,
                        }))
                        .await
                        .expect("interface event loop should never terminate");
                    match waiter
                        .await
                        .expect("interfaces event loop should have handled the request")
                    {
                        Ok(()) => {
                            if is_dump {
                                client.send_unicast(netlink_packet::new_done(req_header))
                            } else if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                SetLink(link_msg) => {
                    let args = match to_set_link_args(link_msg) {
                        Ok(args) => args,
                        Err(e) => {
                            log_debug!("received invalid `SetLink` request from {}", client);
                            client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                            return;
                        }
                    };
                    let (completer, waiter) = oneshot::channel();
                    unified_request_sink
                        .send(UnifiedRequest::InterfacesRequest(interfaces::Request {
                            args: interfaces::RequestArgs::Link(interfaces::LinkRequestArgs::Set(
                                args,
                            )),
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                            completer,
                        }))
                        .await
                        .expect("interface event loop should never terminate");
                    match waiter
                        .await
                        .expect("interfaces event loop should have handled the request")
                    {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                NewLink(ref link_msg) => {
                    let kind = link_msg
                        .attributes
                        .iter()
                        .flat_map(|attr| match attr {
                            LinkAttribute::LinkInfo(info) => info.iter(),
                            _ => [].iter(),
                        })
                        .find_map(|info| match info {
                            LinkInfo::Kind(kind) => Some(kind),
                            _ => None,
                        });
                    match kind {
                        Some(InfoKind::Xfrm) => {
                            // Only log if the link type is one we haven't
                            // already seen in the wild.
                        }
                        Some(_) | None => log_warn!(
                            "Received RTM_NEWLINK request with unsupported link type {kind:?}"
                        ),
                    }
                    client.send_unicast(netlink_packet::new_error(Err(Errno::ENOTSUP), req_header));
                }
                DelLink(_) => {
                    client.send_unicast(netlink_packet::new_error(Err(Errno::ENOTSUP), req_header));
                }
                GetAddress(ref message) if is_dump => {
                    let ip_version_filter = match message.header.family() {
                        AddressFamily::Unspec => None,
                        AddressFamily::Inet => Some(IpVersion::V4),
                        AddressFamily::Inet6 => Some(IpVersion::V6),
                        family => {
                            log_debug!(
                                "invalid address family ({:?}) in address dump request from {}: {:?}",
                                family,
                                client,
                                req,
                            );
                            client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EINVAL),
                                req_header,
                            ));
                            return;
                        }
                    };

                    let (completer, waiter) = oneshot::channel();
                    unified_request_sink
                        .send(UnifiedRequest::InterfacesRequest(interfaces::Request {
                            args: interfaces::RequestArgs::Address(
                                interfaces::AddressRequestArgs::Get(
                                    interfaces::GetAddressArgs::Dump { ip_version_filter },
                                ),
                            ),
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                            completer,
                        }))
                        .await
                        .expect("interface event loop should never terminate");
                    waiter
                        .await
                        .expect("interfaces event loop should have handled the request")
                        .expect("addr dump requests are infallible");
                    client.send_unicast(netlink_packet::new_done(req_header))
                }
                NewAddress(ref message) => {
                    let extracted_request =
                        match extract_if_id_and_addr_from_addr_message(message, client, &req, true)
                        {
                            Ok(o) => o,
                            Err(e) => {
                                return client
                                    .send_unicast(netlink_packet::new_error(Err(e), req_header));
                            }
                        };
                    let result = if let Some(ExtractedAddressRequest {
                        address_and_interface_id,
                        addr_flags,
                    }) = extracted_request
                    {
                        let (completer, waiter) = oneshot::channel();
                        let requested_subnet_route =
                            addr_flags & IFA_F_NOPREFIXROUTE != IFA_F_NOPREFIXROUTE;
                        // Linux doesn't add subnet routes for /0 and /32 IPv4 addresses.
                        let can_add_subnet_route = match address_and_interface_id.address {
                            AddrSubnetEither::V4(addr_subnet) => {
                                let prefix = addr_subnet.subnet().prefix();
                                prefix != 0 && prefix != 32
                            }
                            AddrSubnetEither::V6(_addr_subnet) => true,
                        };
                        let add_subnet_route = requested_subnet_route && can_add_subnet_route;
                        unified_request_sink
                            .send(UnifiedRequest::InterfacesRequest(interfaces::Request {
                                args: interfaces::RequestArgs::Address(
                                    interfaces::AddressRequestArgs::New(
                                        interfaces::NewAddressArgs {
                                            address_and_interface_id,
                                            add_subnet_route,
                                        },
                                    ),
                                ),
                                sequence_number: req_header.sequence_number,
                                client: client.clone(),
                                completer,
                            }))
                            .await
                            .expect("interface event loop should never terminate");
                        waiter.await.expect("interfaces event loop should have handled the request")
                    } else {
                        Ok(())
                    };

                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                DelAddress(ref message) => {
                    let extracted_request = match extract_if_id_and_addr_from_addr_message(
                        message, client, &req, false,
                    ) {
                        Ok(o) => o,
                        Err(e) => {
                            return client
                                .send_unicast(netlink_packet::new_error(Err(e), req_header));
                        }
                    };

                    let result = if let Some(ExtractedAddressRequest {
                        address_and_interface_id,
                        addr_flags: _,
                    }) = extracted_request
                    {
                        let (completer, waiter) = oneshot::channel();
                        unified_request_sink
                            .send(UnifiedRequest::InterfacesRequest(interfaces::Request {
                                args: interfaces::RequestArgs::Address(
                                    interfaces::AddressRequestArgs::Del(
                                        interfaces::DelAddressArgs { address_and_interface_id },
                                    ),
                                ),
                                sequence_number: req_header.sequence_number,
                                client: client.clone(),
                                completer,
                            }))
                            .await
                            .expect("interface event loop should never terminate");
                        waiter.await.expect("interfaces event loop should have handled the request")
                    } else {
                        Ok(())
                    };
                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                // Non-dump GetRoute is not currently necessary for our use.
                GetRoute(ref message) if is_dump => {
                    match message.header.address_family {
                        AddressFamily::Unspec => {
                            // V4 routes are requested prior to V6 routes to conform
                            // with `ip list` output.
                            process_routes_worker_request::<_, Ipv4>(
                                unified_request_sink,
                                client,
                                req_header,
                                routes::RouteRequestArgs::Get(routes::GetRouteArgs::Dump),
                            )
                            .await
                            .expect("route dump requests are infallible");
                            process_routes_worker_request::<_, Ipv6>(
                                unified_request_sink,
                                client,
                                req_header,
                                routes::RouteRequestArgs::Get(routes::GetRouteArgs::Dump),
                            )
                            .await
                            .expect("route dump requests are infallible");
                        }
                        AddressFamily::Inet => {
                            process_routes_worker_request::<_, Ipv4>(
                                unified_request_sink,
                                client,
                                req_header,
                                routes::RouteRequestArgs::Get(routes::GetRouteArgs::Dump),
                            )
                            .await
                            .expect("route dump requests are infallible");
                        }
                        AddressFamily::Inet6 => {
                            process_routes_worker_request::<_, Ipv6>(
                                unified_request_sink,
                                client,
                                req_header,
                                routes::RouteRequestArgs::Get(routes::GetRouteArgs::Dump),
                            )
                            .await
                            .expect("route dump requests are infallible");
                        }
                        family => {
                            log_debug!(
                                "invalid address family ({:?}) in route dump request from {}: {:?}",
                                family,
                                client,
                                req
                            );
                            client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EINVAL),
                                req_header,
                            ));
                            return;
                        }
                    };

                    client.send_unicast(netlink_packet::new_done(req_header))
                }
                GetRule(msg) => {
                    if !is_dump {
                        client.send_unicast(netlink_packet::new_error(
                            Err(Errno::ENOTSUP),
                            req_header,
                        ));
                        return;
                    }
                    let ip_versions = match msg.header.family {
                        AddressFamily::Inet => Either::Left(std::iter::once(IpVersion::V4)),
                        AddressFamily::Inet6 => Either::Left(std::iter::once(IpVersion::V6)),
                        AddressFamily::Unspec => {
                            Either::Right([IpVersion::V4, IpVersion::V6].into_iter())
                        }
                        family => {
                            client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EAFNOSUPPORT),
                                req_header,
                            ));
                            log_debug!(
                                "received RTM_GETRULE req from {} with invalid address \
                                family ({:?}): {:?}",
                                client,
                                family,
                                msg
                            );
                            return;
                        }
                    };
                    for ip_version in ip_versions.into_iter() {
                        let (completer, receiver) = oneshot::channel();
                        net_types::for_any_ip_version!(ip_version, I, {
                            unified_request_sink
                                .send(UnifiedRequest::rule_request::<I>(
                                    RuleRequest {
                                        args: RuleRequestArgs::DumpRules,
                                        _ip_version_marker: <I as Ip>::VERSION_MARKER,
                                        sequence_number: req_header.sequence_number,
                                        client: client.clone(),
                                    },
                                    completer,
                                ))
                                .await
                                .expect("event loop should never terminate");
                        });

                        match receiver.await.expect("completer should not be dropped") {
                            Ok(()) => {}
                            Err(e) => {
                                client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                                return;
                            }
                        }
                    }
                    client.send_unicast(netlink_packet::new_done(req_header))
                }
                NewRule(msg) => {
                    if is_replace {
                        log_warn!("unimplemented: RTM_NEWRULE requests with NLM_F_REPLACE set.");
                        client.send_unicast(netlink_packet::new_error(
                            Err(Errno::ENOTSUP),
                            req_header,
                        ));
                        return;
                    }
                    let ip_version = match msg.header.family {
                        AddressFamily::Inet => IpVersion::V4,
                        AddressFamily::Inet6 => IpVersion::V6,
                        family => {
                            log_debug!(
                                "received RTM_NEWRULE req from {} with invalid address \
                                family ({:?}): {:?}",
                                client,
                                family,
                                msg
                            );
                            client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EAFNOSUPPORT),
                                req_header,
                            ));
                            return;
                        }
                    };
                    let (completer, receiver) = oneshot::channel();
                    net_types::for_any_ip_version!(ip_version, I, {
                        let request = RuleRequest {
                            args: RuleRequestArgs::New(msg),
                            _ip_version_marker: <I as Ip>::VERSION_MARKER,
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                        };
                        unified_request_sink
                            .send(UnifiedRequest::rule_request::<I>(request, completer))
                            .await
                            .expect("event loop should never terminate");
                    });
                    match receiver.await.expect("completer should not be dropped") {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => {
                            client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                        }
                    }
                }
                DelRule(msg) => {
                    let ip_version = match msg.header.family {
                        AddressFamily::Inet => IpVersion::V4,
                        AddressFamily::Inet6 => IpVersion::V6,
                        family => {
                            log_debug!(
                                "received RTM_DELRULE req from {} with invalid address \
                                family ({:?}): {:?}",
                                client,
                                family,
                                msg
                            );
                            client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EAFNOSUPPORT),
                                req_header,
                            ));
                            return;
                        }
                    };
                    let (completer, receiver) = oneshot::channel();
                    net_types::for_any_ip_version!(ip_version, I, {
                        let request = RuleRequest {
                            args: RuleRequestArgs::Del(msg),
                            _ip_version_marker: <I as Ip>::VERSION_MARKER,
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                        };
                        unified_request_sink
                            .send(UnifiedRequest::rule_request::<I>(request, completer))
                            .await
                            .expect("event loop should never terminate");
                    });
                    match receiver.await.expect("completer should not be dropped") {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => {
                            client.send_unicast(netlink_packet::new_error(Err(e), req_header));
                        }
                    }
                }
                NewRoute(ref message) => {
                    // TODO(https://issues.fuchsia.dev/290803327): Emulate REPLACE by
                    // dispatching a delete then add request to Netstack.
                    let is_replace = req_header.flags & NLM_F_REPLACE == NLM_F_REPLACE;
                    if is_replace {
                        log_warn!(
                            "unsupported request type: NLM_F_REPLACE flag present in new \
                            route request from {}: {:?}",
                            client,
                            req
                        );
                        client.send_unicast(netlink_packet::new_error(
                            Err(Errno::ENOTSUP),
                            req_header,
                        ));
                        return;
                    }

                    if message.header.kind != RouteType::Unicast {
                        log_warn!(
                            "unsupported request type: {:?} route present in new route \
                            request from {}: {:?}, only `RTN_UNICAST` is supported",
                            message.header.kind,
                            client,
                            req
                        );
                        client.send_unicast(netlink_packet::new_error(
                            Err(Errno::ENOTSUP),
                            req_header,
                        ));
                        return;
                    }

                    let result = match message.header.address_family {
                        AddressFamily::Inet => {
                            match to_new_route_args::<Ipv4>(message, client, &req) {
                                Ok(req) => {
                                    process_routes_worker_request::<_, Ipv4>(
                                        unified_request_sink,
                                        client,
                                        req_header,
                                        req,
                                    )
                                    .await
                                }
                                Err(e) => {
                                    return client.send_unicast(netlink_packet::new_error(
                                        Err(e),
                                        req_header,
                                    ));
                                }
                            }
                        }
                        AddressFamily::Inet6 => {
                            match to_new_route_args::<Ipv6>(message, client, &req) {
                                Ok(req) => {
                                    process_routes_worker_request::<_, Ipv6>(
                                        unified_request_sink,
                                        client,
                                        req_header,
                                        req,
                                    )
                                    .await
                                }
                                Err(e) => {
                                    return client.send_unicast(netlink_packet::new_error(
                                        Err(e),
                                        req_header,
                                    ));
                                }
                            }
                        }
                        family => {
                            log_debug!(
                                "invalid address family ({:?}) in new route \
                                request from {}: {:?}",
                                family,
                                client,
                                req
                            );
                            return client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EINVAL),
                                req_header,
                            ));
                        }
                    };

                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                DelRoute(ref message) => {
                    if message.header.kind != RouteType::Unicast {
                        log_warn!(
                            "unsupported request type: {:?} route present in new route \
                            request from {}: {:?}, only `RTN_UNICAST` is supported",
                            message.header.kind,
                            client,
                            req
                        );
                        client.send_unicast(netlink_packet::new_error(
                            Err(Errno::ENOTSUP),
                            req_header,
                        ));
                        return;
                    }

                    let result = match message.header.address_family {
                        AddressFamily::Inet => {
                            match to_del_route_args::<Ipv4>(message, client, &req) {
                                Ok(req) => {
                                    process_routes_worker_request::<_, Ipv4>(
                                        unified_request_sink,
                                        client,
                                        req_header,
                                        req,
                                    )
                                    .await
                                }
                                Err(e) => {
                                    return client.send_unicast(netlink_packet::new_error(
                                        Err(e),
                                        req_header,
                                    ));
                                }
                            }
                        }
                        AddressFamily::Inet6 => {
                            match to_del_route_args::<Ipv6>(message, client, &req) {
                                Ok(req) => {
                                    process_routes_worker_request::<_, Ipv6>(
                                        unified_request_sink,
                                        client,
                                        req_header,
                                        req,
                                    )
                                    .await
                                }
                                Err(e) => {
                                    return client.send_unicast(netlink_packet::new_error(
                                        Err(e),
                                        req_header,
                                    ));
                                }
                            }
                        }
                        family => {
                            log_debug!(
                                "invalid address family ({:?}) in new route \
                                request from {}: {:?}",
                                family,
                                client,
                                req
                            );
                            return client.send_unicast(netlink_packet::new_error(
                                Err(Errno::EINVAL),
                                req_header,
                            ));
                        }
                    };

                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client.send_unicast(netlink_packet::new_error(
                            Err(e.into_errno()),
                            req_header,
                        )),
                    }
                }
                GetNeighbour(ref message) => {
                    let (completer, waiter) = oneshot::channel();
                    let request = match GetNeighborArgs::try_from_rtnl_neighbor(message, is_dump) {
                        Ok(get_args) => neighbors::Request {
                            args: NeighborRequestArgs::Get(get_args),
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                            completer,
                        },
                        Err(e) => {
                            return client.send_unicast(netlink_packet::new_error(
                                Err(e.into()),
                                req_header,
                            ));
                        }
                    };
                    unified_request_sink
                        .send(UnifiedRequest::NeighborsRequest(request))
                        .await
                        .expect("route event loop should never terminate");
                    let result =
                        waiter.await.expect("routes event loop should have handled the request");

                    match result {
                        Ok(()) => {
                            if is_dump {
                                client.send_unicast(netlink_packet::new_done(req_header))
                            } else if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client
                            .send_unicast(netlink_packet::new_error(Err(e.into()), req_header)),
                    }
                }
                NewNeighbour(ref message) => {
                    let (completer, waiter) = oneshot::channel();
                    let request =
                        match NewNeighborArgs::try_from_rtnl_neighbor(message, req_header.flags) {
                            Ok(add_args) => neighbors::Request {
                                args: NeighborRequestArgs::New(add_args),
                                sequence_number: req_header.sequence_number,
                                client: client.clone(),
                                completer,
                            },
                            Err(e) => {
                                return client.send_unicast(netlink_packet::new_error(
                                    Err(e.into()),
                                    req_header,
                                ));
                            }
                        };
                    unified_request_sink
                        .send(UnifiedRequest::NeighborsRequest(request))
                        .await
                        .expect("route event loop should never terminate");
                    let result =
                        waiter.await.expect("routes event loop should have handled the request");

                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client
                            .send_unicast(netlink_packet::new_error(Err(e.into()), req_header)),
                    }
                }
                DelNeighbour(ref message) => {
                    let (completer, waiter) = oneshot::channel();
                    let request = match DelNeighborArgs::try_from_rtnl_neighbor(message) {
                        Ok(del_args) => neighbors::Request {
                            args: NeighborRequestArgs::Del(del_args),
                            sequence_number: req_header.sequence_number,
                            client: client.clone(),
                            completer,
                        },
                        Err(e) => {
                            return client.send_unicast(netlink_packet::new_error(
                                Err(e.into()),
                                req_header,
                            ));
                        }
                    };
                    unified_request_sink
                        .send(UnifiedRequest::NeighborsRequest(request))
                        .await
                        .expect("route event loop should never terminate");
                    let result =
                        waiter.await.expect("routes event loop should have handled the request");

                    match result {
                        Ok(()) => {
                            if expects_ack {
                                client.send_unicast(netlink_packet::new_error(Ok(()), req_header))
                            }
                        }
                        Err(e) => client
                            .send_unicast(netlink_packet::new_error(Err(e.into()), req_header)),
                    }
                }
                // Style note: avoid placing comments between the match arms
                // below as it prevents rustfmt from formatting the entire
                // `match` statement.
                //
                // TODO(https://issues.fuchsia.dev/278565021): Implement
                // GetAddress.
                GetAddress(_)
                | NewLinkProp(_)
                | DelLinkProp(_)
                | NewNeighbourTable(_)
                | GetNeighbourTable(_)
                | SetNeighbourTable(_)
                | NewTrafficClass(_)
                | GetTrafficClass(_)
                | DelTrafficClass(_)
                | NewTrafficFilter(_)
                | GetTrafficFilter(_)
                | DelTrafficFilter(_)
                | NewTrafficChain(_)
                | GetTrafficChain(_)
                | DelTrafficChain(_)
                | NewNsId(_)
                | GetNsId(_)
                | DelNsId(_)
                | GetRoute(_)
                | NewNeighbourDiscoveryUserOption(_)
                | NewPrefix(_)
                | NewQueueDiscipline(_)
                | GetQueueDiscipline(_)
                | DelQueueDiscipline(_) => {
                    let req_type = (&req).into();
                    let nl_flags_dbg =
                        netlink_packet::netlink_flags_debug_string(req_header.flags, req_type);
                    log_warn!(
                        "Received unsupported NETLINK_ROUTE {req_type:?} request: \
                        {req:?} with flags {nl_flags_dbg}"
                    );
                    if is_dump && req_type == NetlinkRequestType::Get {
                        client.send_unicast(netlink_packet::new_done(req_header));
                    } else if expects_ack {
                        client.send_unicast(netlink_packet::new_error(Ok(()), req_header));
                    }
                }
            }
        }
    }

    // Dispatch a route request to the given v4 or v6 Routes sink.
    async fn process_routes_worker_request<S: Sender<RouteNetlinkMessage>, I: Ip>(
        sink: &mut mpsc::Sender<UnifiedRequest<S>>,
        client: &mut InternalClient<NetlinkRoute, S>,
        req_header: NetlinkHeader,
        route_request: routes::RouteRequestArgs<I>,
    ) -> Result<(), routes::RequestError> {
        let (completer, waiter) = oneshot::channel();
        let request = routes::Request {
            args: routes::RequestArgs::Route(route_request),
            sequence_number: req_header.sequence_number,
            client: client.clone(),
            completer,
        };
        let fut = I::map_ip_in(
            (request, IpInvariant(sink)),
            |(request, IpInvariant(sink))| sink.send(UnifiedRequest::RoutesV4Request(request)),
            |(request, IpInvariant(sink))| sink.send(UnifiedRequest::RoutesV6Request(request)),
        );
        fut.await.expect("route event loop should never terminate");
        waiter.await.expect("routes event loop should have handled the request")
    }

    /// A connection to the Route Netlink Protocol family.
    pub struct NetlinkRouteClient(pub(crate) ExternalClient<NetlinkRoute>);

    impl NetlinkClient for NetlinkRouteClient {
        type Request = RouteNetlinkMessage;

        fn set_pid(&self, pid: NonZeroU32) {
            let NetlinkRouteClient(client) = self;
            client.set_port_number(pid)
        }

        fn add_membership(
            &self,
            group: ModernGroup,
        ) -> Result<AsyncWorkCompletionWaiter, InvalidModernGroupError> {
            let NetlinkRouteClient(client) = self;
            client.add_membership(group)
        }

        fn del_membership(&self, group: ModernGroup) -> Result<(), InvalidModernGroupError> {
            let NetlinkRouteClient(client) = self;
            client.del_membership(group)
        }

        fn set_legacy_memberships(
            &self,
            legacy_memberships: LegacyGroups,
        ) -> Result<AsyncWorkCompletionWaiter, InvalidLegacyGroupsError> {
            let NetlinkRouteClient(client) = self;
            client.set_legacy_memberships(legacy_memberships)
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    use std::convert::Infallible as Never;

    use crate::messaging::testutil::FakeCreds;
    use crate::messaging::{NetlinkMessageWithCreds, Permission};
    use crate::multicast_groups::GroupSupport;
    use netlink_packet_core::NetlinkHeader;

    pub(crate) const LEGACY_GROUP1: u32 = 0x00000001;
    pub(crate) const LEGACY_GROUP2: u32 = 0x00000002;
    pub(crate) const LEGACY_GROUP3: u32 = 0x00000004;
    pub(crate) const INVALID_LEGACY_GROUP: u32 = 0x00000008;
    pub(crate) const MODERN_GROUP1: ModernGroup = ModernGroup(1);
    pub(crate) const MODERN_GROUP2: ModernGroup = ModernGroup(2);
    pub(crate) const MODERN_GROUP3: ModernGroup = ModernGroup(3);
    pub(crate) const INVALID_MODERN_GROUP: ModernGroup = ModernGroup(4);
    pub(crate) const MODERN_GROUP_NEEDS_BLOCKING: ModernGroup = ModernGroup(5);
    // AF_PACKET can't be generated by bindgen for some reason.
    pub(crate) const AF_PACKET: u16 = 17;

    #[derive(Debug)]
    pub(crate) enum FakeProtocolFamily {}

    impl MulticastCapableNetlinkFamily for FakeProtocolFamily {
        fn check_support(group: &ModernGroup) -> Result<GroupSupport, InvalidModernGroupError> {
            match *group {
                MODERN_GROUP1 | MODERN_GROUP2 | MODERN_GROUP3 | MODERN_GROUP_NEEDS_BLOCKING => {
                    Ok(GroupSupport::Unsupported)
                }
                _ => Err(InvalidModernGroupError),
            }
        }
    }

    pub(crate) fn new_fake_netlink_message() -> NetlinkMessage<FakeNetlinkInnerMessage> {
        NetlinkMessage::new(
            NetlinkHeader::default(),
            NetlinkPayload::InnerMessage(FakeNetlinkInnerMessage),
        )
    }

    pub(crate) fn new_fake_netlink_message_with_creds()
    -> NetlinkMessageWithCreds<NetlinkMessage<FakeNetlinkInnerMessage>, FakeCreds> {
        NetlinkMessageWithCreds::new(new_fake_netlink_message(), FakeCreds::default())
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub(crate) struct FakeNetlinkInnerMessage;

    impl MessageWithPermission for FakeNetlinkInnerMessage {
        fn permission(&self) -> Permission {
            Permission::NetlinkRouteWrite
        }
    }

    impl NetlinkSerializable for FakeNetlinkInnerMessage {
        fn message_type(&self) -> u16 {
            u16::MAX
        }

        fn buffer_len(&self) -> usize {
            0
        }

        fn serialize(&self, _buffer: &mut [u8]) {}
    }

    impl NetlinkDeserializable for FakeNetlinkInnerMessage {
        type DeserializeOptions = ();
        type Error = DecodeError;

        fn deserialize(
            _header: &NetlinkHeader,
            _payload: &[u8],
            _options: (),
        ) -> Result<Self, Self::Error> {
            unimplemented!()
        }
    }

    /// Handler of [`FakeNetlinkInnerMessage`] requests.
    ///
    /// Reflects the given request back as the response.
    #[derive(Clone)]
    pub(crate) struct FakeNetlinkRequestHandler;

    impl<S: Sender<FakeNetlinkInnerMessage>> NetlinkFamilyRequestHandler<FakeProtocolFamily, S>
        for FakeNetlinkRequestHandler
    {
        async fn handle_request(
            &mut self,
            req: NetlinkMessage<FakeNetlinkInnerMessage>,
            client: &mut InternalClient<FakeProtocolFamily, S>,
        ) {
            client.send_unicast(req)
        }
    }

    impl NamedNetlinkFamily for FakeProtocolFamily {
        const NAME: &'static str = "FAKE_PROTOCOL_FAMILY";
    }

    impl ProtocolFamily for FakeProtocolFamily {
        type Request = FakeNetlinkInnerMessage;
        type Response = FakeNetlinkInnerMessage;
        type RequestHandler<S: Sender<Self::Response>> = FakeNetlinkRequestHandler;
        type NotifiedMulticastGroup = ();
        type AsyncWorkItem = Never;

        fn should_notify_on_group_membership_change(
            group: ModernGroup,
        ) -> Option<Self::NotifiedMulticastGroup> {
            (group == MODERN_GROUP_NEEDS_BLOCKING).then_some(())
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use fuchsia_sync::Mutex;
    use std::collections::VecDeque;
    use std::net::IpAddr;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::pin::pin;
    use std::sync::Arc;

    use fidl_fuchsia_net_routes_ext as fnet_routes_ext;

    use assert_matches::assert_matches;
    use fidl_fuchsia_net as fnet;
    use fuchsia_async as fasync;
    use futures::SinkExt;
    use futures::channel::mpsc;
    use futures::future::FutureExt as _;
    use futures::stream::StreamExt as _;
    use linux_uapi::{
        AF_INET, AF_INET6, AF_UNSPEC, IFA_F_NOPREFIXROUTE, RTN_MULTICAST, RTN_UNICAST,
        net_device_flags_IFF_UP, rt_class_t_RT_TABLE_COMPAT, rt_class_t_RT_TABLE_MAIN,
    };
    use net_declare::{
        fidl_ip, net_addr_subnet, net_ip_v4, net_ip_v6, net_subnet_v4, net_subnet_v6, std_ip_v4,
        std_ip_v6,
    };
    use net_types::ip::{
        AddrSubnetEither, GenericOverIp, Ip, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Subnet,
        SubnetEither,
    };
    use net_types::{SpecifiedAddr, Witness as _};
    use netlink_packet_core::{NLM_F_ACK, NLM_F_CREATE, NLM_F_DUMP, NLM_F_REPLACE, NetlinkHeader};

    use netlink_packet_route::address::{AddressAttribute, AddressFlags, AddressMessage};
    use netlink_packet_route::link::{InfoKind, LinkAttribute, LinkFlags, LinkInfo, LinkMessage};
    use netlink_packet_route::neighbour::{
        NeighbourAttribute, NeighbourHeader, NeighbourMessage, NeighbourState,
    };
    use netlink_packet_route::neighbour_discovery_user_option::{
        NeighbourDiscoveryIcmpType, NeighbourDiscoveryIcmpV6Type,
        NeighbourDiscoveryUserOptionHeader, NeighbourDiscoveryUserOptionMessage,
    };
    use netlink_packet_route::route::{RouteAddress, RouteAttribute, RouteMessage, RouteType};
    use netlink_packet_route::rule::RuleMessage;
    use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};
    use test::testutil::AF_PACKET;
    use test_case::test_case;

    use crate::messaging::testutil::{FakeSender, SentMessage};
    use crate::netlink_packet::errno::Errno;
    use crate::netlink_packet::{self};
    use crate::protocol_family::route::{NetlinkRoute, NetlinkRouteRequestHandler};
    use crate::route_eventloop::UnifiedRequest;
    use crate::rules::{RuleRequest, RuleRequestArgs};
    use crate::{interfaces, neighbors, routes};

    enum ExpectedResponse {
        Ack,
        Error(Errno),
        Done,
    }

    fn expected_sent_messages(
        expected_response: Option<ExpectedResponse>,
        nl_header: NetlinkHeader,
    ) -> Vec<SentMessage<RouteNetlinkMessage>> {
        expected_response
            .into_iter()
            .map(|expected_response| {
                SentMessage::unicast(match expected_response {
                    ExpectedResponse::Ack => netlink_packet::new_error(Ok(()), nl_header),
                    ExpectedResponse::Error(e) => netlink_packet::new_error(Err(e), nl_header),
                    ExpectedResponse::Done => netlink_packet::new_done(nl_header),
                })
            })
            .collect::<Vec<_>>()
    }

    fn header_with_flags(flags: u16) -> NetlinkHeader {
        let mut header = NetlinkHeader::default();
        header.flags = flags;
        header
    }

    fn neighbour_discovery_user_option_msg() -> NeighbourDiscoveryUserOptionMessage {
        NeighbourDiscoveryUserOptionMessage::new(
            NeighbourDiscoveryUserOptionHeader::new(
                0,
                NeighbourDiscoveryIcmpType::Inet6(
                    NeighbourDiscoveryIcmpV6Type::NeighbourAdvertisement,
                ),
            ),
            Default::default(),
            Default::default(),
        )
    }

    /// Tests that unhandled requests are treated as a no-op.
    ///
    /// Get requests are responded to with a Done message if the dump flag
    /// is set, an Ack message if the ack flag is set or nothing. New/Del
    /// requests are responded to with an Ack message if the ack flag is set
    /// or nothing.
    #[test_case(
        RouteNetlinkMessage::GetTrafficChain(Default::default()),
        0,
        None; "get_tc__with_no_flags")]
    #[test_case(
        RouteNetlinkMessage::GetTrafficChain(Default::default()),
        NLM_F_ACK,
        Some(ExpectedResponse::Ack); "get_tc_with_ack_flag")]
    #[test_case(
        RouteNetlinkMessage::GetTrafficChain(Default::default()),
        NLM_F_DUMP,
        Some(ExpectedResponse::Done); "get_tc_with_dump_flag")]
    #[test_case(
        RouteNetlinkMessage::GetTrafficChain(Default::default()),
        NLM_F_ACK | NLM_F_DUMP,
        Some(ExpectedResponse::Done); "get_tc_with_ack_and_dump_flag")]
    #[test_case(
        RouteNetlinkMessage::NewTrafficChain(Default::default()),
        0,
        None; "new_tc_with_no_flags")]
    #[test_case(
        RouteNetlinkMessage::NewTrafficChain(Default::default()),
        NLM_F_DUMP,
        None; "new_tc_with_dump_flag")]
    #[test_case(
        RouteNetlinkMessage::NewTrafficChain(Default::default()),
        NLM_F_ACK,
        Some(ExpectedResponse::Ack); "new_tc_with_ack_flag")]
    #[test_case(
        RouteNetlinkMessage::NewTrafficChain(Default::default()),
        NLM_F_ACK | NLM_F_DUMP,
        Some(ExpectedResponse::Ack); "new_tc_with_ack_and_dump_flags")]
    #[test_case(
        RouteNetlinkMessage::DelTrafficChain(Default::default()),
        0,
        None; "deltc_with_no_flags")]
    #[test_case(
        RouteNetlinkMessage::DelTrafficChain(Default::default()),
        NLM_F_DUMP,
        None; "del_tc_with_dump_flag")]
    #[test_case(
        RouteNetlinkMessage::DelTrafficChain(Default::default()),
        NLM_F_ACK,
        Some(ExpectedResponse::Ack); "del_tc_with_ack_flag")]
    #[test_case(
        RouteNetlinkMessage::DelTrafficChain(Default::default()),
        NLM_F_ACK | NLM_F_DUMP,
        Some(ExpectedResponse::Ack); "del_tc_with_ack_and_dump_flags")]
    #[test_case(
        RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(neighbour_discovery_user_option_msg()),
        NLM_F_ACK,
        Some(ExpectedResponse::Ack); "new_neighbour_user_option_with_ack_flag")]
    #[test_case(
        RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(neighbour_discovery_user_option_msg()),
        0,
        None; "new_neighbour_user_option_no_flags")]
    #[test_case(
        RouteNetlinkMessage::NewPrefix(Default::default()),
        NLM_F_ACK,
        Some(ExpectedResponse::Ack); "new_prefix_with_ack_flag")]
    #[test_case(
        RouteNetlinkMessage::NewPrefix(Default::default()),
        0,
        None; "new_prefix_no_flags")]
    #[fuchsia::test]
    async fn test_handle_unsupported_request_response(
        inner_message: RouteNetlinkMessage,
        flags: u16,
        expected_response: Option<ExpectedResponse>,
    ) {
        let (unified_request_sink, _unified_request_stream) = mpsc::channel(0);

        let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

        let (mut client_sink, mut client, async_work_drain_task) =
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            );
        let join_handle = fasync::Task::spawn(async_work_drain_task);

        let header = header_with_flags(flags);

        handler
            .handle_request(
                NetlinkMessage::new(header, NetlinkPayload::InnerMessage(inner_message)),
                &mut client,
            )
            .await;

        assert_eq!(client_sink.take_messages(), expected_sent_messages(expected_response, header));

        drop(client);
        join_handle.await;
    }

    struct RequestAndResponse<R> {
        request: R,
        response: Result<(), interfaces::RequestError>,
    }

    async fn test_request(
        request: NetlinkMessage<RouteNetlinkMessage>,
        req_and_resp: Option<RequestAndResponse<interfaces::RequestArgs>>,
    ) -> Vec<SentMessage<RouteNetlinkMessage>> {
        let (mut client_sink, mut client, async_work_drain_task) =
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            );
        let join_handle = fasync::Task::spawn(async_work_drain_task);

        let (unified_request_sink, unified_request_stream) = mpsc::channel(0);
        let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

        let mut interfaces_request_stream = unified_request_stream.filter_map(|req| {
            futures::future::ready(match req {
                UnifiedRequest::InterfacesRequest(req) => Some(req),
                _ => None,
            })
        });

        let ((), ()) = futures::future::join(handler.handle_request(request, &mut client), async {
            let next = interfaces_request_stream.next();
            match req_and_resp {
                Some(RequestAndResponse { request, response }) => {
                    let interfaces::Request { args, sequence_number: _, client: _, completer } =
                        next.await.expect("handler should send request");
                    assert_eq!(args, request);
                    completer.send(response).expect("handler should be alive");
                }
                None => assert_matches!(next.now_or_never(), None),
            }
        })
        .await;

        drop(client);
        join_handle.await;
        client_sink.take_messages()
    }

    const FAKE_INTERFACE_ID: u32 = 1;
    const FAKE_INTERFACE_NAME: &str = "interface";

    /// Test RTM_GETLINK.
    #[test_case(
        0,
        0,
        None,
        None,
        Ok(()),
        Some(ExpectedResponse::Error(Errno::EINVAL)); "no_specifiers")]
    #[test_case(
        NLM_F_DUMP,
        0,
        None,
        Some(interfaces::GetLinkArgs::Dump),
        Ok(()),
        Some(ExpectedResponse::Done); "dump")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        0,
        None,
        Some(interfaces::GetLinkArgs::Dump),
        Ok(()),
        Some(ExpectedResponse::Done); "dump_with_ack")]
    #[test_case(
        NLM_F_DUMP,
        FAKE_INTERFACE_ID,
        None,
        Some(interfaces::GetLinkArgs::Dump),
        Ok(()),
        Some(ExpectedResponse::Done); "dump_with_id")]
    #[test_case(
        NLM_F_DUMP,
        FAKE_INTERFACE_ID,
        Some(FAKE_INTERFACE_NAME),
        Some(interfaces::GetLinkArgs::Dump),
        Ok(()),
        Some(ExpectedResponse::Done); "dump_with_id_and_name")]
    #[test_case(
        0,
        FAKE_INTERFACE_ID,
        None,
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Index(
            NonZeroU32::new(FAKE_INTERFACE_ID).unwrap()))),
        Ok(()),
        None; "id")]
    #[test_case(
        NLM_F_ACK,
        FAKE_INTERFACE_ID,
        None,
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Index(
            NonZeroU32::new(FAKE_INTERFACE_ID).unwrap()))),
        Ok(()),
        Some(ExpectedResponse::Ack); "id_with_ack")]
    #[test_case(
        0,
        FAKE_INTERFACE_ID,
        Some(FAKE_INTERFACE_NAME),
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Index(
            NonZeroU32::new(FAKE_INTERFACE_ID).unwrap()))),
        Ok(()),
        None; "id_with_name")]
    #[test_case(
        0,
        0,
        Some(FAKE_INTERFACE_NAME),
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Name(
            FAKE_INTERFACE_NAME.to_string()))),
        Ok(()),
        None; "name")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Name(
            FAKE_INTERFACE_NAME.to_string()))),
        Ok(()),
        Some(ExpectedResponse::Ack); "name_with_ack")]
    #[test_case(
        0,
        FAKE_INTERFACE_ID,
        None,
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Index(
            NonZeroU32::new(FAKE_INTERFACE_ID).unwrap()))),
        Err(interfaces::RequestError::UnrecognizedInterface),
        Some(ExpectedResponse::Error(Errno::ENODEV)); "id_not_found")]
    #[test_case(
        0,
        0,
        Some(FAKE_INTERFACE_NAME),
        Some(interfaces::GetLinkArgs::Get(interfaces::LinkSpecifier::Name(
            FAKE_INTERFACE_NAME.to_string()))),
        Err(interfaces::RequestError::UnrecognizedInterface),
        Some(ExpectedResponse::Error(Errno::ENODEV)); "name_not_found")]
    #[fuchsia::test]
    async fn test_get_link(
        flags: u16,
        link_id: u32,
        link_name: Option<&str>,
        expected_request_args: Option<interfaces::GetLinkArgs>,
        interfaces_worker_result: Result<(), interfaces::RequestError>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let header = header_with_flags(flags);
        let mut link_message = LinkMessage::default();
        link_message.header.index = link_id;
        link_message.attributes =
            link_name.map(|n| LinkAttribute::IfName(n.to_string())).into_iter().collect();

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetLink(link_message)),
                ),
                expected_request_args.map(|a| RequestAndResponse {
                    request: interfaces::RequestArgs::Link(interfaces::LinkRequestArgs::Get(a)),
                    response: interfaces_worker_result,
                }),
            )
            .await,
            expected_sent_messages(expected_response, header),
        )
    }

    #[test_case(
        0,
        0,
        None,
        0,
        0,
        None,
        Ok(()),
        Some(ExpectedResponse::Error(Errno::EINVAL)); "interface_not_specified")]
    #[test_case(
        0,
        FAKE_INTERFACE_ID,
        Some(FAKE_INTERFACE_NAME),
        0,
        0,
        None,
        Ok(()),
        Some(ExpectedResponse::Error(Errno::EBUSY)); "name_and_id")]
    #[test_case(
        0,
        FAKE_INTERFACE_ID,
        None,
        0,
        0,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Index(NonZeroU32::new(FAKE_INTERFACE_ID).unwrap()),
            enable: None,
        }),
        Ok(()),
        None; "no_change_by_id")]
    #[test_case(
        0,
        0,
        Some(FAKE_INTERFACE_NAME),
        0,
        0,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: None,
        }),
        Ok(()),
        None; "no_change_by_name")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        0,
        0,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: None,
        }),
        Ok(()),
        Some(ExpectedResponse::Ack); "no_change_ack")]
    #[test_case(
        0,
        0,
        Some(FAKE_INTERFACE_NAME),
        net_device_flags_IFF_UP,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(true),
        }),
        Ok(()),
        None; "enable")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        net_device_flags_IFF_UP,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(true),
        }),
        Ok(()),
        Some(ExpectedResponse::Ack); "enable_ack")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        net_device_flags_IFF_UP,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(true),
        }),
        Err(interfaces::RequestError::UnrecognizedInterface),
        Some(ExpectedResponse::Error(Errno::ENODEV)); "enable_error")]
    #[test_case(
        0,
        0,
        Some(FAKE_INTERFACE_NAME),
        0,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(false),
        }),
        Ok(()),
        None; "disable")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        0,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(false),
        }),
        Ok(()),
        Some(ExpectedResponse::Ack); "disable_ack")]
    #[test_case(
        NLM_F_ACK,
        0,
        Some(FAKE_INTERFACE_NAME),
        0,
        net_device_flags_IFF_UP,
        Some(interfaces::SetLinkArgs{
            link: interfaces::LinkSpecifier::Name(FAKE_INTERFACE_NAME.to_string()),
            enable: Some(false),
        }),
        Err(interfaces::RequestError::UnrecognizedInterface),
        Some(ExpectedResponse::Error(Errno::ENODEV)); "disable_error")]
    #[fuchsia::test]
    async fn test_set_link(
        flags: u16,
        link_id: u32,
        link_name: Option<&str>,
        link_flags: u32,
        change_mask: u32,
        expected_request_args: Option<interfaces::SetLinkArgs>,
        interfaces_worker_result: Result<(), interfaces::RequestError>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let header = header_with_flags(flags);
        let mut link_message = LinkMessage::default();
        link_message.header.index = link_id;
        link_message.header.flags = LinkFlags::from_bits(link_flags).unwrap();
        link_message.header.change_mask = LinkFlags::from_bits(change_mask).unwrap();
        link_message.attributes =
            link_name.map(|n| LinkAttribute::IfName(n.to_string())).into_iter().collect();

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::SetLink(link_message)),
                ),
                expected_request_args.map(|a| RequestAndResponse {
                    request: interfaces::RequestArgs::Link(interfaces::LinkRequestArgs::Set(a)),
                    response: interfaces_worker_result,
                }),
            )
            .await,
            expected_sent_messages(expected_response, header),
        )
    }

    #[fuchsia::test]
    async fn test_new_link() {
        let header = header_with_flags(0);
        let mut link_message = LinkMessage::default();
        link_message.header.index = 1;
        link_message.attributes = vec![
            LinkAttribute::IfName("interface".to_string()),
            LinkAttribute::LinkInfo(vec![LinkInfo::Kind(InfoKind::Xfrm)]),
        ];

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(link_message)),
                ),
                None,
            )
            .await,
            expected_sent_messages(Some(ExpectedResponse::Error(Errno::ENOTSUP)), header)
        );
    }

    #[fuchsia::test]
    async fn test_del_link() {
        let header = header_with_flags(0);
        let mut link_message = LinkMessage::default();
        link_message.header.index = 1;
        link_message.attributes = vec![LinkAttribute::IfName("interface".to_string())];

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelLink(link_message)),
                ),
                None,
            )
            .await,
            expected_sent_messages(Some(ExpectedResponse::Error(Errno::ENOTSUP)), header)
        );
    }

    /// Test RTM_GETADDR.
    /// Conversions to u16 are safe because the constants fit within a 16-bit integer,
    #[test_case(
        0,
        AF_UNSPEC,
        None,
        None; "af_unspec_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_UNSPEC,
        None,
        Some(ExpectedResponse::Ack); "af_unspec_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_UNSPEC,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: None,
        }),
        Some(ExpectedResponse::Done); "af_unspec_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_UNSPEC,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: None,
        }),
        Some(ExpectedResponse::Done); "af_unspec_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_INET,
        None,
        None; "af_inet_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_INET,
        None,
        Some(ExpectedResponse::Ack); "af_inet_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_INET,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: Some(IpVersion::V4),
        }),
        Some(ExpectedResponse::Done); "af_inet_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_INET,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: Some(IpVersion::V4),
        }),
        Some(ExpectedResponse::Done); "af_inet_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_INET6,
        None,
        None; "af_inet6_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_INET6,
        None,
        Some(ExpectedResponse::Ack); "af_inet6_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_INET6,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: Some(IpVersion::V6),
        }),
        Some(ExpectedResponse::Done); "af_inet6_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_INET6,
        Some(interfaces::GetAddressArgs::Dump {
            ip_version_filter: Some(IpVersion::V6),
        }),
        Some(ExpectedResponse::Done); "af_inet6_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_PACKET.into(),
        None,
        None; "af_other_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_PACKET.into(),
        None,
        Some(ExpectedResponse::Ack); "af_other_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_PACKET.into(),
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "af_other_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_PACKET.into(),
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "af_other_dump_and_ack_flags")]
    #[fuchsia::test]
    async fn test_get_addr(
        flags: u16,
        family: u32,
        expected_request_args: Option<interfaces::GetAddressArgs>,
        expected_response: Option<ExpectedResponse>,
    ) {
        // This conversion is safe because family is actually a u16.
        let family = family as u16;
        let header = header_with_flags(flags);
        let address_message = {
            let mut message = AddressMessage::default();
            // Conversion is safe, because family is guaranteed to fit into an 8-bit integer.
            message.header.family = AddressFamily::from(family as u8).into();
            message
        };

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetAddress(address_message)),
                ),
                expected_request_args.map(|a| RequestAndResponse {
                    request: interfaces::RequestArgs::Address(interfaces::AddressRequestArgs::Get(
                        a
                    )),
                    response: Ok(()),
                }),
            )
            .await,
            expected_sent_messages(expected_response, header)
        )
    }

    enum AddressRequestKind {
        New { add_subnet_route: bool },
        Del,
    }

    struct TestAddrCase {
        kind: AddressRequestKind,
        flags: u16,
        family: u16,
        nlas: Vec<AddressAttribute>,
        prefix_len: u8,
        interface_id: u32,
        expected_request_args: Option<RequestAndResponse<interfaces::AddressAndInterfaceArgs>>,
        expected_response: Option<ExpectedResponse>,
    }

    fn ip_from_addr(a: AddrSubnetEither) -> IpAddr {
        match a {
            AddrSubnetEither::V4(a) => IpAddr::V4(a.addr().get().ipv4_bytes().into()),
            AddrSubnetEither::V6(a) => IpAddr::V6(a.addr().get().ipv6_bytes().into()),
        }
    }

    fn prefix_from_addr(a: AddrSubnetEither) -> u8 {
        let (_addr, prefix) = a.addr_prefix();
        prefix
    }

    fn interface_id_as_u32(id: u64) -> u32 {
        id.try_into().unwrap()
    }

    fn valid_new_del_addr_request(
        kind: AddressRequestKind,
        ack: bool,
        addr: AddrSubnetEither,
        extra_nlas: impl IntoIterator<Item = AddressAttribute>,
        interface_id: u64,
        response: Result<(), interfaces::RequestError>,
    ) -> TestAddrCase {
        TestAddrCase {
            kind,
            flags: if ack { NLM_F_ACK } else { 0 },
            family: match addr {
                // Conversions to u16 are safe as AF_INET and AF_INET6
                // fit into 16-bit integers.
                AddrSubnetEither::V4(_) => AF_INET as u16,
                AddrSubnetEither::V6(_) => AF_INET6 as u16,
            },
            nlas: [AddressAttribute::Local(ip_from_addr(addr))]
                .into_iter()
                .chain(extra_nlas)
                .collect(),
            prefix_len: prefix_from_addr(addr),
            interface_id: interface_id_as_u32(interface_id),
            expected_request_args: Some(RequestAndResponse {
                request: interfaces::AddressAndInterfaceArgs {
                    address: addr,
                    interface_id: NonZeroU32::new(interface_id_as_u32(interface_id)).unwrap(),
                },
                response,
            }),
            expected_response: ack.then_some(ExpectedResponse::Ack),
        }
    }

    fn valid_new_addr_request_with_extra_nlas(
        ack: bool,
        addr: AddrSubnetEither,
        extra_nlas: impl IntoIterator<Item = AddressAttribute>,
        interface_id: u64,
        response: Result<(), interfaces::RequestError>,
    ) -> TestAddrCase {
        valid_new_del_addr_request(
            AddressRequestKind::New { add_subnet_route: true },
            ack,
            addr,
            extra_nlas,
            interface_id,
            response,
        )
    }

    fn valid_new_addr_request(
        ack: bool,
        addr: AddrSubnetEither,
        interface_id: u64,
        response: Result<(), interfaces::RequestError>,
    ) -> TestAddrCase {
        valid_new_addr_request_with_extra_nlas(ack, addr, None, interface_id, response)
    }

    fn invalid_new_addr_request(
        ack: bool,
        addr: AddrSubnetEither,
        interface_id: u64,
        errno: Errno,
    ) -> TestAddrCase {
        TestAddrCase {
            expected_request_args: None,
            expected_response: Some(ExpectedResponse::Error(errno)),
            ..valid_new_addr_request(ack, addr, interface_id, Ok(()))
        }
    }

    fn valid_del_addr_request(
        ack: bool,
        addr: AddrSubnetEither,
        interface_id: u64,
        response: Result<(), interfaces::RequestError>,
    ) -> TestAddrCase {
        valid_new_del_addr_request(AddressRequestKind::Del, ack, addr, None, interface_id, response)
    }

    fn invalid_del_addr_request(
        ack: bool,
        addr: AddrSubnetEither,
        interface_id: u64,
        errno: Errno,
    ) -> TestAddrCase {
        TestAddrCase {
            expected_request_args: None,
            expected_response: Some(ExpectedResponse::Error(errno)),
            ..valid_del_addr_request(ack, addr, interface_id, Ok(()))
        }
    }

    fn new_del_addr_request_from_subnet(
        kind: AddressRequestKind,
        ack: bool,
        subnet: SubnetEither,
        interface_id: u64,
    ) -> TestAddrCase {
        let (net, prefix) = subnet.net_prefix();
        TestAddrCase {
            kind,
            flags: if ack { NLM_F_ACK } else { 0 },
            family: match subnet {
                SubnetEither::V4(_) => AF_INET as u16,
                SubnetEither::V6(_) => AF_INET6 as u16,
            },
            nlas: vec![AddressAttribute::Local(net.into())],
            prefix_len: prefix,
            interface_id: interface_id_as_u32(interface_id),
            expected_request_args: None,
            expected_response: ack.then_some(ExpectedResponse::Ack),
        }
    }

    /// Test RTM_NEWADDR and RTM_DELADDR
    // Add address tests cases.
    #[test_case(
        new_del_addr_request_from_subnet(
            AddressRequestKind::New { add_subnet_route: true },
            true,
            net_subnet_v4!("0.0.0.0/0").into(),
            interfaces::testutil::PPP_INTERFACE_ID,
        ); "new_v4_unspecified_address_zero_prefix_ok_ack")]
    #[test_case(
        new_del_addr_request_from_subnet(
            AddressRequestKind::New { add_subnet_route: true },
            false,
            net_subnet_v4!("0.0.0.0/24").into(),
            interfaces::testutil::PPP_INTERFACE_ID,
        ); "new_v4_unspecified_address_non_zero_prefix_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EADDRNOTAVAIL)),
            ..new_del_addr_request_from_subnet(
                AddressRequestKind::New { add_subnet_route: true },
                true,
                net_subnet_v6!("::/0").into(),
                interfaces::testutil::ETH_INTERFACE_ID,
            )
        }; "new_v6_unspecified_address_zero_prefix_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EADDRNOTAVAIL)),
            ..new_del_addr_request_from_subnet(
                AddressRequestKind::New { add_subnet_route: true },
                false,
                net_subnet_v6!("::/64").into(),
                interfaces::testutil::ETH_INTERFACE_ID,
            )
        }; "new_v6_unspecified_address_non_zero_prefix_no_ack")]
    #[test_case(
        valid_new_addr_request(
            true,
            interfaces::testutil::test_addr_subnet_v4(),
            interfaces::testutil::LO_INTERFACE_ID,
            Ok(())); "new_v4_ok_ack")]
    #[test_case(
        valid_new_addr_request(
            true,
            interfaces::testutil::test_addr_subnet_v6(),
            interfaces::testutil::LO_INTERFACE_ID,
            Ok(())); "new_v6_ok_ack")]
    #[test_case(
        valid_new_addr_request(
            false,
            interfaces::testutil::test_addr_subnet_v4(),
            interfaces::testutil::ETH_INTERFACE_ID,
            Ok(())); "new_v4_ok_no_ack")]
    #[test_case(
        valid_new_addr_request(
            false,
            interfaces::testutil::test_addr_subnet_v6(),
            interfaces::testutil::ETH_INTERFACE_ID,
            Ok(())); "new_v6_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Local(ip_from_addr(interfaces::testutil::test_addr_subnet_v4())),
            ],
            ..valid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::ETH_INTERFACE_ID,
                Ok(()),
            )
        }; "new_v4_local_nla_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Address(ip_from_addr(interfaces::testutil::test_addr_subnet_v6())),
            ],
            ..valid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::PPP_INTERFACE_ID,
                Ok(()),
            )
        }; "new_v6_address_nla_ok_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Address(ip_from_addr(interfaces::testutil::test_addr_subnet_v6())),
                AddressAttribute::Local(ip_from_addr(interfaces::testutil::test_addr_subnet_v6())),
            ],
            ..valid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::PPP_INTERFACE_ID,
                Ok(()),
            )
        }; "new_v6_same_local_and_address_nla_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            kind: AddressRequestKind::New { add_subnet_route: true },
            ..valid_new_addr_request_with_extra_nlas(
                false,
                interfaces::testutil::test_addr_subnet_v4(),
                [AddressAttribute::Flags(AddressFlags::empty())],
                interfaces::testutil::ETH_INTERFACE_ID,
                Ok(()),
            )
        }; "new_v4_with_route_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            kind: AddressRequestKind::New { add_subnet_route: false },
            ..valid_new_addr_request_with_extra_nlas(
                true,
                interfaces::testutil::test_addr_subnet_v6(),
                [AddressAttribute::Flags(AddressFlags::from_bits(IFA_F_NOPREFIXROUTE).unwrap())],
                interfaces::testutil::LO_INTERFACE_ID,
                Ok(()),
            )
        }; "new_v6_without_route_ok_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..valid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::LO_INTERFACE_ID,
                Err(interfaces::RequestError::InvalidRequest),
            )
        }; "new_v4_invalid_response_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EEXIST)),
            ..valid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::LO_INTERFACE_ID,
                Err(interfaces::RequestError::AlreadyExists),
            )
        }; "new_v6_exist_response_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::ENODEV)),
            ..valid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Err(interfaces::RequestError::UnrecognizedInterface),
            )
        }; "new_v6_unrecognized_interface_response_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EADDRNOTAVAIL)),
            ..valid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::ETH_INTERFACE_ID,
                Err(interfaces::RequestError::AddressNotFound),
            )
        }; "new_v4_not_found_response_ck")]
    #[test_case(
        TestAddrCase {
            interface_id: 0,
            ..invalid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::LO_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_zero_interface_id_ack")]
    #[test_case(
        TestAddrCase {
            interface_id: 0,
            ..invalid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_zero_interface_id_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: Vec::new(),
            ..invalid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_no_nlas_ack")]
    #[test_case(
        TestAddrCase {
            nlas: Vec::new(),
            ..invalid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_no_nlas_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![AddressAttribute::Flags(AddressFlags::empty())],
            ..invalid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_missing_address_and_local_nla_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![AddressAttribute::Flags(AddressFlags::empty())],
            ..invalid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_missing_address_and_local_nla_no_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: 0,
            kind: AddressRequestKind::New { add_subnet_route: false },
            ..valid_new_addr_request(
                true,
                net_addr_subnet!("192.0.2.123/0"),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Ok(()),
            )
        }; "new_zero_prefix_len_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: 0,
            ..valid_new_addr_request(
                false,
                net_addr_subnet!("2001:db8::1324/0"),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Ok(()),
            )
        }; "new_zero_prefix_len_no_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: u8::MAX,
            ..invalid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_invalid_prefix_len_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: u8::MAX,
            ..invalid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_invalid_prefix_len_no_ack")]
    #[test_case(
        TestAddrCase {
            family: AF_UNSPEC as u16,
            ..invalid_new_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::LO_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_invalid_family_ack")]
    #[test_case(
        TestAddrCase {
            family: AF_UNSPEC as u16,
            ..invalid_new_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::PPP_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "new_invalid_family_no_ack")]
    // Delete address tests cases.
    #[test_case(
        valid_del_addr_request(
            true,
            interfaces::testutil::test_addr_subnet_v4(),
            interfaces::testutil::LO_INTERFACE_ID,
            Ok(())); "del_v4_ok_ack")]
    #[test_case(
        valid_del_addr_request(
            true,
            interfaces::testutil::test_addr_subnet_v6(),
            interfaces::testutil::LO_INTERFACE_ID,
            Ok(())); "del_v6_ok_ack")]
    #[test_case(
        valid_del_addr_request(
            false,
            interfaces::testutil::test_addr_subnet_v4(),
            interfaces::testutil::ETH_INTERFACE_ID,
            Ok(())); "del_v4_ok_no_ack")]
    #[test_case(
        valid_del_addr_request(
            false,
            interfaces::testutil::test_addr_subnet_v6(),
            interfaces::testutil::ETH_INTERFACE_ID,
            Ok(())); "del_v6_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..valid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::LO_INTERFACE_ID,
                Err(interfaces::RequestError::InvalidRequest),
            )
        }; "del_v4_invalid_response_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Local(ip_from_addr(interfaces::testutil::test_addr_subnet_v4())),
            ],
            ..valid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::ETH_INTERFACE_ID,
                Ok(()),
            )
        }; "del_v4_local_nla_ok_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Address(ip_from_addr(interfaces::testutil::test_addr_subnet_v6())),
            ],
            ..valid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::PPP_INTERFACE_ID,
                Ok(()),
            )
        }; "del_v6_address_nla_ok_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![
                AddressAttribute::Address(ip_from_addr(interfaces::testutil::test_addr_subnet_v4())),
                AddressAttribute::Local(ip_from_addr(interfaces::testutil::test_addr_subnet_v4())),
            ],
            ..valid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::ETH_INTERFACE_ID,
                Ok(()),
            )
        }; "del_v4_same_local_and_address_nla_ok_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EEXIST)),
            ..valid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::LO_INTERFACE_ID,
                Err(interfaces::RequestError::AlreadyExists),
            )
        }; "del_v6_exist_response_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::ENODEV)),
            ..valid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Err(interfaces::RequestError::UnrecognizedInterface),
            )
        }; "del_v6_unrecognized_interface_response_no_ack")]
    #[test_case(
        TestAddrCase {
            expected_response: Some(ExpectedResponse::Error(Errno::EADDRNOTAVAIL)),
            ..valid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::ETH_INTERFACE_ID,
                Err(interfaces::RequestError::AddressNotFound),
            )
        }; "del_v4_not_found_response_ck")]
    #[test_case(
        TestAddrCase {
            interface_id: 0,
            ..invalid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::LO_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_zero_interface_id_ack")]
    #[test_case(
        TestAddrCase {
            interface_id: 0,
            ..invalid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_zero_interface_id_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: Vec::new(),
            ..invalid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_no_nlas_ack")]
    #[test_case(
        TestAddrCase {
            nlas: Vec::new(),
            ..invalid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_no_nlas_no_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![AddressAttribute::Flags(AddressFlags::empty())],
            ..invalid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_missing_address_and_local_nla_ack")]
    #[test_case(
        TestAddrCase {
            nlas: vec![AddressAttribute::Flags(AddressFlags::empty())],
            ..invalid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_missing_address_and_local_nla_no_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: 0,
            ..valid_del_addr_request(
                true,
                net_addr_subnet!("192.0.2.123/0"),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Ok(()),
            )
        }; "del_zero_prefix_len_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: 0,
            ..valid_del_addr_request(
                false,
                net_addr_subnet!("2001:db8::1324/0"),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Ok(()),
            )
        }; "del_zero_prefix_len_no_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: u8::MAX,
            ..invalid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_invalid_prefix_len_ack")]
    #[test_case(
        TestAddrCase {
            prefix_len: u8::MAX,
            ..invalid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::WLAN_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_invalid_prefix_len_no_ack")]
    #[test_case(
        TestAddrCase {
            family: AF_UNSPEC as u16,
            ..invalid_del_addr_request(
                true,
                interfaces::testutil::test_addr_subnet_v4(),
                interfaces::testutil::LO_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_invalid_family_ack")]
    #[test_case(
        TestAddrCase {
            family: AF_UNSPEC as u16,
            ..invalid_del_addr_request(
                false,
                interfaces::testutil::test_addr_subnet_v6(),
                interfaces::testutil::PPP_INTERFACE_ID,
                Errno::EINVAL,
            )
        }; "del_invalid_family_no_ack")]
    #[fuchsia::test]
    async fn test_new_del_addr(test_case: TestAddrCase) {
        let TestAddrCase {
            kind,
            flags,
            family,
            nlas,
            prefix_len,
            interface_id,
            expected_request_args,
            expected_response,
        } = test_case;

        let header = header_with_flags(flags);
        let address_message = {
            let mut message = AddressMessage::default();
            // Conversion is safe because family is guaranteed to fit into an 8-bit integer.
            message.header.family = AddressFamily::from(family as u8).into();
            message.header.index = interface_id.into();
            message.header.prefix_len = prefix_len;
            message.attributes = nlas;
            message
        };

        let (message, request) = match kind {
            AddressRequestKind::New { add_subnet_route } => (
                RouteNetlinkMessage::NewAddress(address_message),
                expected_request_args.map(|RequestAndResponse { request, response }| {
                    RequestAndResponse {
                        request: interfaces::RequestArgs::Address(
                            interfaces::AddressRequestArgs::New(interfaces::NewAddressArgs {
                                address_and_interface_id: request,
                                add_subnet_route,
                            }),
                        ),
                        response,
                    }
                }),
            ),
            AddressRequestKind::Del => (
                RouteNetlinkMessage::DelAddress(address_message),
                expected_request_args.map(|RequestAndResponse { request, response }| {
                    RequestAndResponse {
                        request: interfaces::RequestArgs::Address(
                            interfaces::AddressRequestArgs::Del(interfaces::DelAddressArgs {
                                address_and_interface_id: request,
                            }),
                        ),
                        response,
                    }
                }),
            ),
        };

        pretty_assertions::assert_eq!(
            test_request(
                NetlinkMessage::new(header, NetlinkPayload::InnerMessage(message),),
                request,
            )
            .await,
            expected_sent_messages(expected_response, header),
        )
    }

    /// Given a stream of `UnifiedRequest`s and sinks for v4 and v6 routes
    /// requests, feeds the v4 and v6 routes requests from the stream into the
    /// appropriate sinks, discarding other requests.
    async fn split_route_requests(
        unified_request_stream: mpsc::Receiver<UnifiedRequest<FakeSender<RouteNetlinkMessage>>>,
        v4_routes_request_sink: mpsc::Sender<
            crate::routes::Request<FakeSender<RouteNetlinkMessage>, Ipv4>,
        >,
        v6_routes_request_sink: mpsc::Sender<
            crate::routes::Request<FakeSender<RouteNetlinkMessage>, Ipv6>,
        >,
    ) {
        unified_request_stream
            .fold(
                (v4_routes_request_sink, v6_routes_request_sink),
                |(mut v4_routes_request_sink, mut v6_routes_request_sink), req| async move {
                    match req {
                        UnifiedRequest::RoutesV4Request(req) => {
                            v4_routes_request_sink
                                .send(req)
                                .map(|res| res.expect("send should succeed"))
                                .await
                        }
                        UnifiedRequest::RoutesV6Request(req) => {
                            v6_routes_request_sink
                                .send(req)
                                .map(|res| res.expect("send should succeed"))
                                .await
                        }
                        _ => (),
                    };
                    (v4_routes_request_sink, v6_routes_request_sink)
                },
            )
            .map(|(_v4_sink, _v6_sink)| ())
            .await
    }

    // Separate from `test_route_request`, because RTM_GETROUTE requests do not have typed
    // arguments and there needs to be an option to send a DUMP request to the
    // v4 and v6 routes worker.
    async fn test_get_route_request(
        family: u16,
        request: NetlinkMessage<RouteNetlinkMessage>,
        expected_request: Option<routes::GetRouteArgs>,
    ) -> Vec<SentMessage<RouteNetlinkMessage>> {
        let (mut client_sink, mut client, async_work_drain_task) =
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            );
        let join_handle = fasync::Task::spawn(async_work_drain_task);

        {
            let (unified_request_sink, unified_request_stream) = mpsc::channel(0);

            let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

            let (v4_routes_request_sink, mut v4_routes_request_stream) = mpsc::channel(0);
            let (v6_routes_request_sink, mut v6_routes_request_stream) = mpsc::channel(0);

            let mut split_route_requests_background_work = pin!(
                split_route_requests(
                    unified_request_stream,
                    v4_routes_request_sink,
                    v6_routes_request_sink,
                )
                .fuse()
            );

            let mut handler_fut = pin!(
                futures::future::join(handler.handle_request(request, &mut client), async {
                    // Conversions are safe as constants can fit into 16-bit integers.
                    if family == AF_UNSPEC as u16 || family == AF_INET as u16 {
                        let next = v4_routes_request_stream.next();
                        match expected_request.map(|a| {
                            routes::RequestArgs::<Ipv4>::Route(routes::RouteRequestArgs::Get(a))
                        }) {
                            Some(expected_request) => {
                                let routes::Request {
                                    args,
                                    sequence_number: _,
                                    client: _,
                                    completer,
                                } = next.await.expect("handler should send request");
                                assert_eq!(args, expected_request);
                                completer.send(Ok(())).expect("handler should be alive");
                            }
                            None => assert_matches!(next.now_or_never(), None),
                        };
                    }
                    if family == AF_UNSPEC as u16 || family == AF_INET6 as u16 {
                        let next = v6_routes_request_stream.next();
                        match expected_request.map(|a| {
                            routes::RequestArgs::<Ipv6>::Route(routes::RouteRequestArgs::Get(a))
                        }) {
                            Some(expected_request) => {
                                let routes::Request {
                                    args,
                                    sequence_number: _,
                                    client: _,
                                    completer,
                                } = next.await.expect("handler should send request");
                                assert_eq!(args, expected_request);
                                completer.send(Ok(())).expect("handler should be alive");
                            }
                            None => assert_matches!(next.now_or_never(), None),
                        };
                    }
                })
                .fuse()
            );

            futures::select! {
                ((), ()) = handler_fut => (),
                () = split_route_requests_background_work => unreachable!(),
            };
        }
        drop(client);
        join_handle.await;
        client_sink.take_messages()
    }

    /// Test RTM_GETROUTE.
    /// Conversions are safe as constants fit within 16-bit integers.
    #[test_case(
        0,
        AF_UNSPEC as u16,
        None,
        None; "af_unspec_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_UNSPEC as u16,
        None,
        Some(ExpectedResponse::Ack); "af_unspec_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_UNSPEC as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_unspec_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_UNSPEC as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_unspec_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_INET as u16,
        None,
        None; "af_inet_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_INET as u16,
        None,
        Some(ExpectedResponse::Ack); "af_inet_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_INET as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_inet_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_INET as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_inet_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_INET6 as u16,
        None,
        None; "af_inet6_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_INET6 as u16,
        None,
        Some(ExpectedResponse::Ack); "af_inet6_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_INET6 as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_inet6_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_INET6 as u16,
        Some(routes::GetRouteArgs::Dump),
        Some(ExpectedResponse::Done); "af_inet6_dump_and_ack_flags")]
    #[test_case(
        0,
        AF_PACKET,
        None,
        None; "af_other_no_flags")]
    #[test_case(
        NLM_F_ACK,
        AF_PACKET,
        None,
        Some(ExpectedResponse::Ack); "af_other_ack_flag")]
    #[test_case(
        NLM_F_DUMP,
        AF_PACKET,
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "af_other_dump_flag")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        AF_PACKET,
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "af_other_dump_and_ack_flags")]
    #[fuchsia::test]
    async fn test_get_route(
        flags: u16,
        family: u16,
        expected_request_args: Option<routes::GetRouteArgs>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let header = header_with_flags(flags);
        let route_message = {
            let mut message = RouteMessage::default();
            message.header.address_family = AddressFamily::from(family as u8);
            message
        };

        pretty_assertions::assert_eq!(
            test_get_route_request(
                family,
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetRoute(route_message)),
                ),
                expected_request_args,
            )
            .await,
            expected_sent_messages(expected_response, header)
        )
    }

    /// Represents a single expected request, and the fake response.
    #[derive(Debug)]
    pub(crate) struct FakeRuleRequestResponse {
        pub(crate) expected_request_args: RuleRequestArgs,
        pub(crate) expected_ip_version: IpVersion,
        pub(crate) response: Result<(), Errno>,
    }

    /// A fake implementation of [`RuleRequestHandler`].
    ///
    /// Handles a sequence of rule requests by pulling the expected request
    /// and fake response from the front of `requests_and_responses`.
    #[derive(Clone, Debug)]
    pub(crate) struct FakeRuleRequestHandler {
        pub(crate) requests_and_responses: Arc<Mutex<VecDeque<FakeRuleRequestResponse>>>,
    }

    impl FakeRuleRequestHandler {
        fn new(requests_and_responses: impl IntoIterator<Item = FakeRuleRequestResponse>) -> Self {
            FakeRuleRequestHandler {
                requests_and_responses: Arc::new(Mutex::new(VecDeque::from_iter(
                    requests_and_responses,
                ))),
            }
        }

        fn handle_request<S: Sender<RouteNetlinkMessage>, I: Ip>(
            &mut self,
            actual_request: RuleRequest<S, I>,
        ) -> Result<(), Errno> {
            let Self { requests_and_responses } = self;
            let FakeRuleRequestResponse { expected_request_args, expected_ip_version, response } =
                requests_and_responses.lock().pop_front().expect(
                    "FakeRuleRequest handler should have a fake request/response pre-configured",
                );
            let RuleRequest { args, _ip_version_marker, sequence_number: _, client: _ } =
                actual_request;
            assert_eq!(args, expected_request_args);
            assert_eq!(I::VERSION, expected_ip_version);
            response
        }
    }

    fn default_rule_for_family(family: u16) -> RuleMessage {
        let mut rule = RuleMessage::default();
        // Conversion is safe as family is guaranteed to fit into an 8-bit integer.
        rule.header.family = AddressFamily::from(family as u8);
        rule
    }

    const AF_INVALID: u16 = 255;
    /// Conversions are safe as constants fit into a 16-bit integer.
    #[test_case(
        RouteNetlinkMessage::GetRule,
        0,
        AF_UNSPEC as u16,
        vec![],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "get_rule_no_dump")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_INVALID,
        vec![],
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "get_rule_dump_invalid_address_family")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Done); "get_rule_dump_v4")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Done); "get_rule_dump_v6")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_UNSPEC as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        },
        FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Done); "get_rule_dump_af_unspec")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V4,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "get_rule_dump_v4_fails")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V6,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "get_rule_dump_v6_fails")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_UNSPEC as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V4,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "get_rule_dump_af_unspec_v4_fails")]
    #[test_case(
        RouteNetlinkMessage::GetRule,
        NLM_F_DUMP,
        AF_UNSPEC as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        },
        FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::DumpRules,
            expected_ip_version: IpVersion::V6,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "get_rule_dump_af_unspec_v6_fails")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        0,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        }],
        None; "new_rule_succeeds_v4")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        0,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        None; "new_rule_succeeds_v6")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        0,
        AF_UNSPEC as u16,
        vec![],
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "new_rule_af_unspec_fails")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        NLM_F_ACK,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Ack); "new_rule_v4_succeeds_with_ack")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        NLM_F_ACK,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Ack); "new_rule_v6_succeeds_with_ack")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        0,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "new_rule_v4_fails")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        0,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::New(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "new_rule_v6_fails")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        NLM_F_REPLACE,
        AF_INET as u16,
        vec![],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "new_rule_v4_replace_unimplemented")]
    #[test_case(
        RouteNetlinkMessage::NewRule,
        NLM_F_REPLACE,
        AF_INET6 as u16,
        vec![],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "new_rule_v6_replace_unimplemented")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        0,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        }],
        None; "del_rule_succeeds_v4")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        0,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        None; "del_rule_succeeds_v6")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        0,
        AF_UNSPEC as u16,
        vec![],
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "del_rule_af_unspec_fails")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        NLM_F_ACK,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Ack); "del_rule_v4_succeeds_with_ack")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        NLM_F_ACK,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Ok(()),
        }],
        Some(ExpectedResponse::Ack); "del_rule_v6_succeeds_with_ack")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        0,
        AF_INET as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET as u16)),
            expected_ip_version: IpVersion::V4,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "del_rule_v4_fails")]
    #[test_case(
        RouteNetlinkMessage::DelRule,
        0,
        AF_INET6 as u16,
        vec![FakeRuleRequestResponse{
            expected_request_args: RuleRequestArgs::Del(default_rule_for_family(AF_INET6 as u16)),
            expected_ip_version: IpVersion::V6,
            response: Err(Errno::ENOTSUP),
        }],
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "del_rule_v6_fails")]
    #[fuchsia::test]
    async fn test_rule_request(
        rule_fn: fn(RuleMessage) -> RouteNetlinkMessage,
        flags: u16,
        address_family: u16,
        requests_and_responses: Vec<FakeRuleRequestResponse>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let rules_request_handler = FakeRuleRequestHandler::new(requests_and_responses);
        let (unified_request_sink, unified_request_stream) = mpsc::channel(0);
        let unified_request_stream = pin!(unified_request_stream);
        let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

        let (mut client_sink, mut client, async_work_drain_task) = {
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            )
        };
        let join_handle = fasync::Task::spawn(async_work_drain_task);

        let header = header_with_flags(flags);

        let handle_request_fut = handler
            .handle_request(
                NetlinkMessage::new(
                    header,
                    NetlinkPayload::InnerMessage(rule_fn(default_rule_for_family(address_family))),
                ),
                &mut client,
            )
            .fuse();

        futures::select! {
            () = pin!(handle_request_fut) => {},
            _rules_request_handler = unified_request_stream.fold(
                rules_request_handler,
                |mut rules_request_handler, req| async move {
                    match req {
                        UnifiedRequest::RuleV4Request(request, completer) => {
                            completer
                                .send(rules_request_handler.handle_request(request))
                                .expect("send should succeed");
                            rules_request_handler
                        }
                        UnifiedRequest::RuleV6Request(request, completer) => {
                            completer
                                .send(rules_request_handler.handle_request(request))
                                .expect("send should succeed");
                            rules_request_handler
                        }
                        _ => panic!("not RuleRequest"),
                    }
                }
            ).fuse() => {},
        }
        assert_eq!(client_sink.take_messages(), expected_sent_messages(expected_response, header));

        drop(client);
        join_handle.await;
    }

    const TEST_V4_SUBNET: Subnet<Ipv4Addr> = net_subnet_v4!("192.0.2.0/24");
    const TEST_V4_NEXTHOP: Ipv4Addr = net_ip_v4!("192.0.2.1");
    const TEST_V6_SUBNET: Subnet<Ipv6Addr> = net_subnet_v6!("2001:db8::0/32");
    const TEST_V6_NEXTHOP: Ipv6Addr = net_ip_v6!("2001:db8::1");

    fn test_nexthop_spec_addr<I: Ip>() -> SpecifiedAddr<I::Addr> {
        I::map_ip(
            (),
            |()| SpecifiedAddr::new(TEST_V4_NEXTHOP).unwrap(),
            |()| SpecifiedAddr::new(TEST_V6_NEXTHOP).unwrap(),
        )
    }

    #[derive(Clone)]
    struct RouteRequestAndResponse<R> {
        request: R,
        response: Result<(), routes::RequestError>,
    }

    #[derive(Clone, Copy)]
    enum RouteRequestKind {
        New,
        Del,
    }

    struct TestRouteCase<I: Ip> {
        kind: RouteRequestKind,
        flags: u16,
        family: u16,
        nlas: Vec<RouteAttribute>,
        destination_prefix_len: u8,
        table: u8,
        rtm_type: u8,
        expected_request_args: Option<RouteRequestAndResponse<routes::RouteRequestArgs<I>>>,
        expected_response: Option<ExpectedResponse>,
    }

    fn route_addr_from_spec_addr<I: Ip>(a: &SpecifiedAddr<I::Addr>) -> RouteAddress {
        let bytes = I::map_ip_in(a, |a| a.ipv4_bytes().to_vec(), |a| a.ipv6_bytes().to_vec());
        match I::VERSION {
            IpVersion::V4 => RouteAddress::parse(AddressFamily::Inet, &bytes).unwrap(),
            IpVersion::V6 => RouteAddress::parse(AddressFamily::Inet6, &bytes).unwrap(),
        }
    }

    fn route_addr_from_subnet<I: Ip>(a: &Subnet<I::Addr>) -> RouteAddress {
        let bytes = I::map_ip_in(
            a,
            |a| a.network().ipv4_bytes().to_vec(),
            |a| a.network().ipv6_bytes().to_vec(),
        );
        match I::VERSION {
            IpVersion::V4 => RouteAddress::parse(AddressFamily::Inet, &bytes).unwrap(),
            IpVersion::V6 => RouteAddress::parse(AddressFamily::Inet6, &bytes).unwrap(),
        }
    }

    fn build_route_test_case<I: Ip>(
        kind: RouteRequestKind,
        flags: u16,
        dst: Subnet<I::Addr>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        extra_nlas: impl IntoIterator<Item = RouteAttribute>,
        interface_id: Option<NonZeroU64>,
        table: u8,
        rtm_type: u8,
        response: Result<(), routes::RequestError>,
    ) -> TestRouteCase<I> {
        let extra_nlas = extra_nlas.into_iter().collect::<Vec<_>>();
        let table_from_nla = extra_nlas.clone().into_iter().find_map(|nla| match nla {
            RouteAttribute::Table(table) => Some(NetlinkRouteTableIndex::new(table)),
            _ => None,
        });
        let priority_from_nla = extra_nlas.clone().into_iter().find_map(|nla| match nla {
            RouteAttribute::Priority(priority) => Some(priority),
            _ => None,
        });

        TestRouteCase::<I> {
            kind,
            flags,
            family: {
                let family = match I::VERSION {
                    IpVersion::V4 => AF_INET,
                    IpVersion::V6 => AF_INET6,
                };
                // Conversion is safe as family is guaranteed to fit into an 8-bit integer.
                family as u16
            },
            nlas: [RouteAttribute::Destination(route_addr_from_subnet::<I>(&dst))]
                .into_iter()
                .chain(interface_id.map(|id| RouteAttribute::Oif(interface_id_as_u32(id.get()))))
                .chain(extra_nlas.into_iter())
                .collect(),
            destination_prefix_len: {
                let prefix_len = I::map_ip_in(dst, |dst| dst.prefix(), |dst| dst.prefix());
                prefix_len
            },
            table,
            rtm_type,
            expected_request_args: Some(RouteRequestAndResponse {
                request: {
                    match kind {
                        RouteRequestKind::New => {
                            let interface_id =
                                interface_id.expect("new requests must have interface id");

                            routes::RouteRequestArgs::New::<I>(routes::NewRouteArgs::Unicast(
                                routes::UnicastNewRouteArgs {
                                    subnet: dst,
                                    target: fnet_routes_ext::RouteTarget {
                                        outbound_interface: interface_id.get(),
                                        next_hop,
                                    },
                                    priority: priority_from_nla.and_then(NonZeroU32::new),
                                    // Use the table value from the NLA if provided.
                                    table: table_from_nla.unwrap_or_else(|| {
                                        NetlinkRouteTableIndex::new(table as u32)
                                    }),
                                },
                            ))
                        }
                        RouteRequestKind::Del => {
                            routes::RouteRequestArgs::Del::<I>(routes::DelRouteArgs::Unicast(
                                routes::UnicastDelRouteArgs {
                                    subnet: dst,
                                    outbound_interface: interface_id,
                                    next_hop,
                                    priority: priority_from_nla
                                        .map(|priority| NonZeroU32::new(priority))
                                        .flatten(),
                                    // Use the table value from the NLA if provided. When the NLA
                                    // value is 0, use the value from the header. Default to
                                    // RT_TABLE_MAIN when the header value is unspecified.
                                    table: table_from_nla
                                        .map(|table_nla| {
                                            NonZeroNetlinkRouteTableIndex::new(table_nla)
                                        })
                                        .flatten()
                                        .unwrap_or_else(|| {
                                            NonZeroNetlinkRouteTableIndex::new_non_zero(
                                                NonZeroU32::new(table as u32).unwrap_or(
                                                    NonZeroU32::new(
                                                        rt_class_t_RT_TABLE_MAIN as u32,
                                                    )
                                                    .unwrap(),
                                                ),
                                            )
                                        }),
                                },
                            ))
                        }
                    }
                },
                response,
            }),
            expected_response: (flags & NLM_F_ACK == NLM_F_ACK).then_some(ExpectedResponse::Ack),
        }
    }

    fn build_valid_route_test_case_with_extra_nlas<I: Ip>(
        kind: RouteRequestKind,
        flags: u16,
        addr: Subnet<I::Addr>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        extra_nlas: impl IntoIterator<Item = RouteAttribute>,
        interface_id: Option<u64>,
        table: u8,
        rtm_type: u8,
        response: Result<(), routes::RequestError>,
    ) -> TestRouteCase<I> {
        build_route_test_case::<I>(
            kind,
            flags,
            addr,
            next_hop,
            extra_nlas,
            interface_id.map(|id| NonZeroU64::new(id)).flatten(),
            table,
            rtm_type,
            response,
        )
    }

    fn build_valid_route_test_case<I: Ip>(
        kind: RouteRequestKind,
        flags: u16,
        addr: Subnet<I::Addr>,
        interface_id: Option<u64>,
        table: u8,
        rtm_type: u8,
        response: Result<(), routes::RequestError>,
    ) -> TestRouteCase<I> {
        build_valid_route_test_case_with_extra_nlas::<I>(
            kind,
            flags,
            addr,
            None,
            None,
            interface_id,
            table,
            rtm_type,
            response,
        )
    }

    fn build_invalid_route_test_case<I: Ip>(
        kind: RouteRequestKind,
        flags: u16,
        addr: Subnet<I::Addr>,
        interface_id: Option<u64>,
        table: u8,
        rtm_type: u8,
        errno: Errno,
    ) -> TestRouteCase<I> {
        build_invalid_route_test_case_with_extra_nlas::<I>(
            kind,
            flags,
            addr,
            None,
            None,
            interface_id,
            table,
            rtm_type,
            errno,
        )
    }

    fn build_invalid_route_test_case_with_extra_nlas<I: Ip>(
        kind: RouteRequestKind,
        flags: u16,
        addr: Subnet<I::Addr>,
        next_hop: Option<SpecifiedAddr<I::Addr>>,
        extra_nlas: impl IntoIterator<Item = RouteAttribute>,
        interface_id: Option<u64>,
        table: u8,
        rtm_type: u8,
        errno: Errno,
    ) -> TestRouteCase<I> {
        TestRouteCase {
            expected_request_args: None,
            expected_response: Some(ExpectedResponse::Error(errno)),
            ..build_valid_route_test_case_with_extra_nlas::<I>(
                kind,
                flags,
                addr,
                next_hop,
                extra_nlas,
                interface_id,
                table,
                rtm_type,
                Ok(()),
            )
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    enum RouteRequestArgsEither {
        V4(routes::RequestArgs<Ipv4>),
        V6(routes::RequestArgs<Ipv6>),
    }

    async fn test_route_request<I: Ip>(
        request: NetlinkMessage<RouteNetlinkMessage>,
        req_and_resp: Option<RouteRequestAndResponse<routes::RouteRequestArgs<I>>>,
    ) -> Vec<SentMessage<RouteNetlinkMessage>> {
        let (mut client_sink, mut client, async_work_drain_task) = {
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            )
        };
        let join_handle = fasync::Task::spawn(async_work_drain_task);

        let (unified_request_sink, unified_request_stream) = mpsc::channel(0);
        let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

        let mut unified_request_stream = pin!(unified_request_stream);

        match req_and_resp {
            None => {
                handler.handle_request(request, &mut client).await;
                assert_matches!(unified_request_stream.next().now_or_never(), None);
            }
            Some(RouteRequestAndResponse { request: expected_request, response }) => {
                let ((), ()) =
                    futures::join!(handler.handle_request(request, &mut client), async {
                        let args = match I::VERSION {
                            IpVersion::V4 => {
                                let next = unified_request_stream.next();
                                let routes::Request {
                                    args,
                                    sequence_number: _,
                                    client: _,
                                    completer,
                                } = match next.await.expect("handler should send request") {
                                    UnifiedRequest::RoutesV4Request(request) => request,
                                    UnifiedRequest::InterfacesRequest(_)
                                    | UnifiedRequest::RoutesV6Request(_)
                                    | UnifiedRequest::RuleV4Request(_, _)
                                    | UnifiedRequest::RuleV6Request(_, _)
                                    | UnifiedRequest::NeighborsRequest(_) => {
                                        panic!("not RoutesV4Request")
                                    }
                                };
                                completer.send(response).expect("handler should be alive");
                                RouteRequestArgsEither::V4(args)
                            }
                            IpVersion::V6 => {
                                let next = unified_request_stream.next();
                                let routes::Request {
                                    args,
                                    sequence_number: _,
                                    client: _,
                                    completer,
                                } = match next.await.expect("handler should send request") {
                                    UnifiedRequest::RoutesV6Request(request) => request,
                                    UnifiedRequest::InterfacesRequest(_)
                                    | UnifiedRequest::RoutesV4Request(_)
                                    | UnifiedRequest::RuleV4Request(_, _)
                                    | UnifiedRequest::RuleV6Request(_, _)
                                    | UnifiedRequest::NeighborsRequest(_) => {
                                        panic!("not RoutesV6Request")
                                    }
                                };
                                completer.send(response).expect("handler should be alive");
                                RouteRequestArgsEither::V6(args)
                            }
                        };

                        #[derive(GenericOverIp)]
                        #[generic_over_ip(I, Ip)]
                        struct EqualityInputs<I: Ip> {
                            args: RouteRequestArgsEither,
                            expected_request: routes::RequestArgs<I>,
                        }

                        I::map_ip_in(
                            EqualityInputs {
                                args: args,
                                expected_request: routes::RequestArgs::Route(expected_request),
                            },
                            |EqualityInputs { args, expected_request }| {
                                let args = assert_matches!(
                                    args,
                                    RouteRequestArgsEither::V4(request) => request
                                );
                                assert_eq!(args, expected_request);
                            },
                            |EqualityInputs { args, expected_request }| {
                                let args = assert_matches!(
                                    args,
                                    RouteRequestArgsEither::V6(request) => request
                                );
                                assert_eq!(args, expected_request);
                            },
                        );
                    });
            }
        }

        drop(client);
        join_handle.await;
        client_sink.take_messages()
    }

    /// Test RTM_NEWROUTE and RTM_DELROUTE
    // Add route test cases.
    // Downcasts are safe as constants have values that fit within an 8-bit integer.
    #[test_case(
        TestRouteCase::<Ipv4> {
            family: AF_UNSPEC as u16,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v4_invalid_family_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            family: AF_UNSPEC as u16,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        })]
    #[test_case(
        TestRouteCase::<Ipv4> {
            flags: NLM_F_ACK | NLM_F_REPLACE,
            ..build_invalid_route_test_case::<Ipv4>(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::ENOTSUP,
            )
        }; "new_v4_replace_flag_ack")]
    #[test_case(
        build_invalid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            NLM_F_REPLACE,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Errno::ENOTSUP,
        ); "new_v6_replace_flag_no_ack")]
    #[test_case(
        build_invalid_route_test_case::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_MULTICAST as u8,
            Errno::ENOTSUP); "new_v4_non_unicast_type_ack")]
    #[test_case(
        build_invalid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_MULTICAST as u8,
            Errno::ENOTSUP); "new_v6_non_unicast_type_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            net_subnet_v4!("0.0.0.0/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_default_route_ok_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::New,
            0,
            net_subnet_v4!("0.0.0.0/24"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_unspecified_route_non_zero_prefix_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            NLM_F_ACK,
            net_subnet_v6!("::/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_default_route_prefix_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            0,
            net_subnet_v6!("::/64"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_unspecified_route_non_zero_prefix_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_ok_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_ok_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::New,
            0,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(test_nexthop_spec_addr::<Ipv4>()),
            [RouteAttribute::Gateway(route_addr_from_spec_addr::<Ipv4>(
                &test_nexthop_spec_addr::<Ipv4>()
            ))],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_with_nexthop_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            Some(test_nexthop_spec_addr::<Ipv6>()),
            [RouteAttribute::Gateway(route_addr_from_spec_addr::<Ipv6>(
                &test_nexthop_spec_addr::<Ipv6>()
            ))],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_with_nexthop_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Gateway(RouteAddress::parse(AddressFamily::Inet, &net_ip_v4!("0.0.0.0").ipv4_bytes().to_vec()).unwrap())],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_unspecified_nexthop_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Gateway(RouteAddress::parse(AddressFamily::Inet6, &net_ip_v6!("::").ipv6_bytes().to_vec()).unwrap())],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_unspecified_nexthop_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Priority(100)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_priority_nla_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Priority(100)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_priority_nla_ok_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::InvalidRequest),
            )
        }; "new_v4_invalid_request_response_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::InvalidRequest),
            )
        }; "new_v6_invalid_request_response_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::ENODEV)),
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::UnrecognizedInterface),
            )
        }; "new_v6_unrecognized_interface_response_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::ENOTSUP)),
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::Unknown),
            )
        }; "new_v6_unknown_response_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: vec![
                RouteAttribute::Destination(route_addr_from_subnet::<Ipv4>(
                    &TEST_V4_SUBNET
                )),
            ],
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::ENOTSUP,
            )
        }; "new_v4_missing_oif_nla_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: vec![
                RouteAttribute::Destination(route_addr_from_subnet::<Ipv6>(
                    &TEST_V6_SUBNET
                )),
            ],
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::ENOTSUP,
            )
        }; "new_v6_missing_oif_nla_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: vec![
                RouteAttribute::Destination(route_addr_from_subnet::<Ipv4>(
                    &TEST_V4_SUBNET
                )),
            ],
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::ENOTSUP,
            )
        }; "new_v4_missing_oif_nla_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: vec![
                RouteAttribute::Destination(route_addr_from_subnet::<Ipv6>(
                    &TEST_V6_SUBNET
                )),
            ],
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::ENOTSUP,
            )
        }; "new_v6_missing_oif_nla_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: vec![
                RouteAttribute::Oif(interfaces::testutil::ETH_INTERFACE_ID.try_into().unwrap()),
            ],
            destination_prefix_len: 1,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v4_missing_destination_nla_nonzero_prefix_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: vec![
                RouteAttribute::Oif(interfaces::testutil::ETH_INTERFACE_ID.try_into().unwrap()),
            ],
            destination_prefix_len: 1,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v6_missing_destination_nla_nonzero_prefix_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: vec![
                RouteAttribute::Oif(interfaces::testutil::LO_INTERFACE_ID.try_into().unwrap()),
            ],
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                net_subnet_v4!("0.0.0.0/0"),
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Ok(()))}; "new_v4_missing_destination_nla_zero_prefix_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: vec![
                RouteAttribute::Oif(interfaces::testutil::LO_INTERFACE_ID.try_into().unwrap()),
            ],
            ..build_valid_route_test_case(
                RouteRequestKind::New,
                0,
                net_subnet_v6!("::/0"),
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Ok(()))}; "new_v6_missing_destination_nla_zero_prefix_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: Vec::new(),
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v4_no_nlas_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: Vec::new(),
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v6_no_nlas_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            // `RT_TABLE_COMPAT` is generally used when `table` is outside the bounds of u8 values.
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_COMPAT as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_with_table_nla_rt_table_compat_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            None,
            // `RT_TABLE_COMPAT` is generally used when `table` is outside the bounds of u8 values.
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_COMPAT as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_with_table_nla_rt_table_compat_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::New,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v4_with_table_nla_rt_class_t_RT_TABLE_MAIN as u8_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::New,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "new_v6_with_table_nla_rt_class_t_RT_TABLE_MAIN as u8_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            destination_prefix_len: u8::MAX,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v4_invalid_prefix_len_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            destination_prefix_len: u8::MAX,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v6_invalid_prefix_len_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            destination_prefix_len: 0,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v4_zero_prefix_len_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            destination_prefix_len: 0,
            ..build_invalid_route_test_case(
                RouteRequestKind::New,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "new_v6_zero_prefix_len_no_ack")]
    // Delete route test cases.
    #[test_case(
        TestRouteCase::<Ipv4> {
            family: AF_UNSPEC as u16,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v4_invalid_family_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            family: AF_UNSPEC as u16,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v6_invalid_family_no_ack")]
    #[test_case(
        build_invalid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_MULTICAST as u8,
            Errno::ENOTSUP); "del_v4_non_unicast_type_ack")]
    #[test_case(
        build_invalid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_MULTICAST as u8,
            Errno::ENOTSUP); "del_v6_non_unicast_type_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            net_subnet_v4!("0.0.0.0/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_default_route_ok_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            0,
            net_subnet_v4!("0.0.0.0/24"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_unspecified_route_non_zero_prefix_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            net_subnet_v6!("::/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_default_route_prefix_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            0,
            net_subnet_v6!("::/64"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_unspecified_route_non_zero_prefix_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_only_dest_nla_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V6_SUBNET,
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_only_dest_nla_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            0,
            TEST_V4_SUBNET,
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_only_dest_nla_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_only_dest_nla_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(test_nexthop_spec_addr::<Ipv4>()),
            [RouteAttribute::Gateway(route_addr_from_spec_addr::<Ipv4>(
                &test_nexthop_spec_addr::<Ipv4>()
            ))],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_with_nexthop_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            Some(test_nexthop_spec_addr::<Ipv6>()),
            [RouteAttribute::Gateway(route_addr_from_spec_addr::<Ipv6>(
                &test_nexthop_spec_addr::<Ipv6>()
            ))],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_with_nexthop_ok_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Gateway(RouteAddress::parse(AddressFamily::Inet, &net_ip_v4!("0.0.0.0").ipv4_bytes().to_vec()).unwrap())],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_unspecified_nexthop_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Gateway(RouteAddress::parse(AddressFamily::Inet6, &net_ip_v6!("::").ipv6_bytes().to_vec()).unwrap())],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_unspecified_nexthop_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Priority(100)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_priority_nla_ok_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Priority(100)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_priority_nla_ok_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::InvalidRequest),
            )
        }; "del_v4_invalid_request_response_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::EINVAL)),
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::InvalidRequest),
            )
        }; "del_v6_invalid_request_response_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::ENODEV)),
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::UnrecognizedInterface),
            )
        }; "del_v6_unrecognized_interface_response_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            expected_response: Some(ExpectedResponse::Error(Errno::ENOTSUP)),
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Err(routes::RequestError::Unknown),
            )
        }; "del_v6_unknown_response_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v4_dest_oif_nlas_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v6_dest_oif_nlas_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            0,
            TEST_V4_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v4_dest_oif_nlas_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v6_dest_oif_nlas_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: Vec::new(),
            destination_prefix_len: 1,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v4_missing_destination_nla_nonzero_prefix_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: Vec::new(),
            destination_prefix_len: 1,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v6_missing_destination_nla_nonzero_prefix_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            nlas: Vec::new(),
            destination_prefix_len: 0,
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                net_subnet_v4!("0.0.0.0/0"),
                None,
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Ok(()),
            )
        }; "del_v4_no_nlas_zero_prefix_len_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            nlas: Vec::new(),
            destination_prefix_len: 0,
            ..build_valid_route_test_case(
                RouteRequestKind::Del,
                0,
                net_subnet_v6!("::/0"),
                None,
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Ok(()),
            )
        }; "del_v6_no_nlas_zero_prefix_len_no_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            net_subnet_v4!("0.0.0.0/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v4_missing_destination_nla_zero_prefix_ack")]
    #[test_case(
        build_valid_route_test_case::<Ipv6>(
            RouteRequestKind::Del,
            0,
            net_subnet_v6!("::/0"),
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v6_missing_destination_nla_zero_prefix_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            // `RT_TABLE_COMPAT` is generally used when `table` is outside the bounds of u8 values.
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_COMPAT as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_with_table_nla_rt_table_compat_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            None,
            // `RT_TABLE_COMPAT` is generally used when `table` is outside the bounds of u8 values.
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_COMPAT as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_with_table_nla_rt_table_compat_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v4_with_table_nla_rt_class_t_RT_TABLE_MAIN as u8_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            0,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Table(u8::MAX as u32 + 1)],
            Some(interfaces::testutil::LO_INTERFACE_ID),
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(())); "del_v6_with_table_nla_rt_class_t_RT_TABLE_MAIN as u8_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            destination_prefix_len: u8::MAX,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v4_invalid_prefix_len_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            destination_prefix_len: u8::MAX,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v6_invalid_prefix_len_no_ack")]
    #[test_case(
        TestRouteCase::<Ipv4> {
            destination_prefix_len: 0,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                NLM_F_ACK,
                TEST_V4_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v4_zero_prefix_len_ack")]
    #[test_case(
        TestRouteCase::<Ipv6> {
            destination_prefix_len: 0,
            ..build_invalid_route_test_case(
                RouteRequestKind::Del,
                0,
                TEST_V6_SUBNET,
                Some(interfaces::testutil::LO_INTERFACE_ID),
                rt_class_t_RT_TABLE_MAIN as u8,
                RTN_UNICAST as u8,
                Errno::EINVAL,
            )
        }; "del_v6_zero_prefix_len_no_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv4>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V4_SUBNET,
            None,
            [RouteAttribute::Oif(0)],
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v4_zero_interface_id_ack")]
    #[test_case(
        build_valid_route_test_case_with_extra_nlas::<Ipv6>(
            RouteRequestKind::Del,
            NLM_F_ACK,
            TEST_V6_SUBNET,
            None,
            [RouteAttribute::Oif(0)],
            None,
            rt_class_t_RT_TABLE_MAIN as u8,
            RTN_UNICAST as u8,
            Ok(()),
        ); "del_v6_zero_interface_id_no_ack")]
    #[fuchsia::test]
    async fn test_new_del_route<I: Ip>(test_case: TestRouteCase<I>) {
        let TestRouteCase {
            kind,
            flags,
            family,
            nlas,
            destination_prefix_len,
            table,
            rtm_type,
            expected_request_args,
            expected_response,
        }: TestRouteCase<I> = test_case;

        let header = header_with_flags(flags);
        let route_message = {
            let mut message = RouteMessage::default();
            message.header.address_family = AddressFamily::from(family as u8);
            message.header.destination_prefix_length = destination_prefix_len;
            message.header.table = table;
            message.header.kind = RouteType::from(rtm_type);
            message.attributes = nlas;
            message
        };

        let (message, request) = match kind {
            RouteRequestKind::New => {
                (RouteNetlinkMessage::NewRoute(route_message), expected_request_args)
            }
            RouteRequestKind::Del => {
                (RouteNetlinkMessage::DelRoute(route_message), expected_request_args)
            }
        };

        pretty_assertions::assert_eq!(
            test_route_request(
                NetlinkMessage::new(header, NetlinkPayload::InnerMessage(message)),
                request,
            )
            .await,
            expected_sent_messages(expected_response, header),
        )
    }

    async fn test_neighbor_request(
        request: NetlinkMessage<RouteNetlinkMessage>,
        expected_request: Option<neighbors::NeighborRequestArgs>,
    ) -> Vec<SentMessage<RouteNetlinkMessage>> {
        let (mut client_sink, mut client, async_work_drain_task) =
            crate::client::testutil::new_fake_client::<NetlinkRoute>(
                crate::client::testutil::CLIENT_ID_1,
                std::iter::empty(),
            );
        let join_handle = fasync::Task::spawn(async_work_drain_task);
        {
            let (unified_request_sink, mut unified_request_stream) = mpsc::channel(0);

            let mut handler = NetlinkRouteRequestHandler::<FakeSender<_>> { unified_request_sink };

            let handle_request_fut = handler.handle_request(request, &mut client).fuse();
            let mut handle_request_fut = pin!(handle_request_fut);
            let mut request_fut = unified_request_stream.next().fuse();

            futures::select! {
                _ = handle_request_fut => assert_eq!(expected_request, None),
                request = request_fut => match request.expect("request channel should be open") {
                    UnifiedRequest::NeighborsRequest(neigh_req) => {
                        let neighbors::Request { args, sequence_number: _, client: _, completer } =
                            neigh_req;
                        assert_eq!(expected_request, Some(args));
                        completer.send(Ok(())).expect("handler should be alive");
                        handle_request_fut.await;
                    },
                    _ => panic!("received unexpected request"),
                }
            }
        }
        drop(client);
        join_handle.await;
        client_sink.take_messages()
    }

    #[test_case(
        NLM_F_DUMP,
        NeighbourHeader::default(),
        vec![],
        Some(neighbors::GetNeighborArgs::Dump { ip_version: None, interface: None }),
        Some(ExpectedResponse::Done); "dump all")]
    #[test_case(
        NLM_F_DUMP,
        NeighbourHeader { family: AddressFamily::Inet, ..Default::default() },
        vec![NeighbourAttribute::IfIndex(1)],
        Some(neighbors::GetNeighborArgs::Dump {
            ip_version: Some(IpVersion::V4),
            interface: Some(NonZeroU64::new(1).unwrap()),
        }),
        Some(ExpectedResponse::Done); "dump ipv4 on interface 1")]
    #[test_case(
        NLM_F_DUMP | NLM_F_ACK,
        NeighbourHeader::default(),
        vec![],
        Some(neighbors::GetNeighborArgs::Dump { ip_version: None, interface: None }),
        Some(ExpectedResponse::Done); "dump all with ack")]
    #[test_case(
        NLM_F_DUMP,
        NeighbourHeader { family: AddressFamily::Local, ..Default::default() },
        vec![],
        None,
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "dump invalid request")]
    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        Some(neighbors::GetNeighborArgs::Get {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
        }),
        None; "get")]
    #[test_case(
        NLM_F_ACK,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        Some(neighbors::GetNeighborArgs::Get {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
        }),
        Some(ExpectedResponse::Ack); "get with ack")]
    #[test_case(
        0,
        NeighbourHeader::default(),
        vec![],
        None,
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "get invalid request")]
    #[fuchsia::test]
    async fn get_neighbor_with_faked_response(
        flags: u16,
        header: NeighbourHeader,
        attrs: Vec<NeighbourAttribute>,
        expected_request_args: Option<neighbors::GetNeighborArgs>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let nl_header = header_with_flags(flags);
        let neighbor_message = {
            let mut message = NeighbourMessage::default();
            message.header = header;
            message.attributes = attrs;
            message
        };

        pretty_assertions::assert_eq!(
            test_neighbor_request(
                NetlinkMessage::new(
                    nl_header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::GetNeighbour(
                        neighbor_message
                    )),
                ),
                expected_request_args.map(neighbors::NeighborRequestArgs::Get),
            )
            .await,
            expected_sent_messages(expected_response, nl_header),
        )
    }

    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        Some(neighbors::DelNeighborArgs {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
        }),
        None; "del ipv4")]
    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet6, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v6!("fe80::1").into())],
        Some(neighbors::DelNeighborArgs {
            ip: fidl_ip!("fe80::1"),
            interface: NonZeroU64::new(1).unwrap(),
        }),
        None; "del ipv6")]
    #[test_case(
        NLM_F_ACK,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        Some(neighbors::DelNeighborArgs {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
        }),
        Some(ExpectedResponse::Ack); "del with ack")]
    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Local, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        None,
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "del invalid family")]
    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![],
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "del missing destination")]
    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 0, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "del missing interface")]
    #[fuchsia::test]
    async fn del_neighbor_with_faked_response(
        flags: u16,
        header: NeighbourHeader,
        attrs: Vec<NeighbourAttribute>,
        expected_request_args: Option<neighbors::DelNeighborArgs>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let nl_header = header_with_flags(flags);
        let neighbor_message = {
            let mut message = NeighbourMessage::default();
            message.header = header;
            message.attributes = attrs;
            message
        };

        pretty_assertions::assert_eq!(
            test_neighbor_request(
                NetlinkMessage::new(
                    nl_header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::DelNeighbour(
                        neighbor_message
                    )),
                ),
                expected_request_args.map(neighbors::NeighborRequestArgs::Del),
            )
            .await,
            expected_sent_messages(expected_response, nl_header),
        )
    }

    #[test_case(
        0,
        NeighbourHeader { family: AddressFamily::Inet, ifindex: 1, ..Default::default() },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        None,
        Some(ExpectedResponse::Error(Errno::ENOTSUP)); "missing flags")]
    #[test_case(
        NLM_F_CREATE | NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet,
            ifindex: 1,
            state: NeighbourState::Permanent,
            ..Default::default()
        },
        vec![
            NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into()),
            NeighbourAttribute::LinkLocalAddress(vec![0, 1, 2, 3, 4, 5]),
        ],
        Some(neighbors::NeighborRequestArgs::New(neighbors::NewNeighborArgs::CreateStatic {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
            mac: fnet::MacAddress { octets: [0, 1, 2, 3, 4, 5] },
        })),
        None; "create static ipv4")]
    #[test_case(
        NLM_F_CREATE | NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet6,
            ifindex: 1,
            state: NeighbourState::Permanent,
            ..Default::default()
        },
        vec![
            NeighbourAttribute::Destination(std_ip_v6!("fe80::1").into()),
            NeighbourAttribute::LinkLocalAddress(vec![0, 1, 2, 3, 4, 5]),
        ],
        Some(neighbors::NeighborRequestArgs::New(neighbors::NewNeighborArgs::CreateStatic {
            ip: fidl_ip!("fe80::1"),
            interface: NonZeroU64::new(1).unwrap(),
            mac: fnet::MacAddress { octets: [0, 1, 2, 3, 4, 5] },
        })),
        None; "create static ipv6")]
    #[test_case(
        NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet,
            ifindex: 1,
            state: NeighbourState::Probe,
            ..Default::default()
        },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        Some(neighbors::NeighborRequestArgs::New(neighbors::NewNeighborArgs::ProbeExisting {
            ip: fidl_ip!("192.168.0.1"),
            interface: NonZeroU64::new(1).unwrap(),
        })),
        None; "probe existing")]
    #[test_case(
        NLM_F_REPLACE,
        NeighbourHeader { family: AddressFamily::Local, ifindex: 1, ..Default::default() },
        vec![
            NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into()),
        ],
        None,
        Some(ExpectedResponse::Error(Errno::EAFNOSUPPORT)); "invalid family")]
    #[test_case(
        NLM_F_CREATE | NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet,
            ifindex: 1,
            state: NeighbourState::Probe,
            ..Default::default()
        },
        vec![
            NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into()),
            NeighbourAttribute::LinkLocalAddress(vec![0, 1, 2, 3, 4, 5]),
        ],
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "invalid state for create static")]
    #[test_case(
        NLM_F_CREATE | NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet,
            ifindex: 1,
            state: NeighbourState::Permanent,
            ..Default::default()
        },
        vec![
            NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into()),
        ],
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "invalid state for probe")]
    #[test_case(
        NLM_F_CREATE | NLM_F_REPLACE,
        NeighbourHeader {
            family: AddressFamily::Inet,
            ifindex: 1,
            state: NeighbourState::Permanent,
            ..Default::default()
        },
        vec![NeighbourAttribute::Destination(std_ip_v4!("192.168.0.1").into())],
        None,
        Some(ExpectedResponse::Error(Errno::EINVAL)); "create static missing mac")]
    #[fuchsia::test]
    async fn new_neighbor_with_faked_response(
        flags: u16,
        header: NeighbourHeader,
        attrs: Vec<NeighbourAttribute>,
        expected_request_args: Option<neighbors::NeighborRequestArgs>,
        expected_response: Option<ExpectedResponse>,
    ) {
        let nl_header = header_with_flags(flags);
        let neighbor_message = {
            let mut message = NeighbourMessage::default();
            message.header = header;
            message.attributes = attrs;
            message
        };

        pretty_assertions::assert_eq!(
            test_neighbor_request(
                NetlinkMessage::new(
                    nl_header,
                    NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewNeighbour(
                        neighbor_message
                    )),
                ),
                expected_request_args,
            )
            .await,
            expected_sent_messages(expected_response, nl_header),
        )
    }
}
