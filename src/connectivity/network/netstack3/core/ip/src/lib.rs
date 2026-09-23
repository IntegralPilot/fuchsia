// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 core IP layer.
//!
//! This crate contains the IP layer for netstack3.

#![no_std]
#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

extern crate alloc;

#[path = "."]
mod internal {
    #[macro_use]
    pub(super) mod path_mtu;

    pub(super) mod api;
    pub(super) mod base;
    pub(super) mod counters;
    pub(super) mod device;
    pub(super) mod fragmentation;
    pub(super) mod gmp;
    pub(super) mod icmp;
    pub(super) mod ipv6;
    pub(super) mod local_delivery;
    pub(super) mod multicast_forwarding;
    pub(super) mod raw;
    pub(super) mod reassembly;
    pub(super) mod routing;
    pub(super) mod sas;
    pub(super) mod socket;
    pub(super) mod types;
    pub(super) mod uninstantiable;
}

/// Definitions for devices at the IP layer.
pub mod device {
    pub use crate::internal::device::api::{
        AddIpAddrSubnetError, AddrSubnetAndManualConfigEither, DeviceIpAnyApi, DeviceIpApi,
        SetIpAddressPropertiesError,
    };
    pub use crate::internal::device::config::{
        IpDeviceConfigurationAndFlags, IpDeviceConfigurationHandler, IpDeviceConfigurationUpdate,
        Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfigurationUpdate,
        PendingIpDeviceConfigurationUpdate, UpdateIpConfigurationError,
    };
    pub use crate::internal::device::dad::{
        DadAddressContext, DadAddressStateRef, DadContext, DadHandler, DadState, DadStateRef,
        DadTimerId, IPV4_DAD_ANNOUNCE_NUM, Ipv4DadAddressInfo, Ipv4DadSendData,
        Ipv6DadAddressContext, Ipv6DadSendData, OwnedNdpNonce,
    };
    pub use crate::internal::device::opaque_iid::{IidSecret, OpaqueIid, OpaqueIidNonce};
    pub use crate::internal::device::route_discovery::{
        Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties, Ipv6DiscoveredRoutesContext,
        Ipv6RouteDiscoveryBindingsContext, Ipv6RouteDiscoveryContext, Ipv6RouteDiscoveryState,
        RouteDiscoveryConfiguration, RouteDiscoveryConfigurationUpdate,
    };
    pub use crate::internal::device::router_solicitation::{
        MAX_RTR_SOLICITATION_DELAY, RTR_SOLICITATION_INTERVAL, RsContext, RsHandler, RsState,
        RsTimerId,
    };
    pub use crate::internal::device::slaac::{
        IidGenerationConfiguration, InnerSlaacTimerId, SLAAC_MIN_REGEN_ADVANCE, SlaacAddressEntry,
        SlaacAddressEntryMut, SlaacAddresses, SlaacBindingsContext, SlaacConfigAndState,
        SlaacConfiguration, SlaacConfigurationUpdate, SlaacContext, SlaacCounters, SlaacState,
        SlaacTimerId, StableSlaacAddressConfiguration, TemporarySlaacAddressConfiguration,
    };
    pub use crate::internal::device::state::{
        AddressId, AddressIdIter, CommonAddressConfig, CommonAddressProperties, DefaultHopLimit,
        DualStackIpDeviceState, IpAddressData, IpAddressEntry, IpAddressFlags, IpDeviceAddresses,
        IpDeviceConfiguration, IpDeviceFlags, IpDeviceMulticastGroups, IpDeviceStateBindingsTypes,
        IpDeviceStateIpExt, Ipv4AddrConfig, Ipv4DeviceConfiguration, Ipv6AddrConfig,
        Ipv6AddrManualConfig, Ipv6AddrSlaacConfig, Ipv6DeviceConfiguration,
        Ipv6NetworkLearnedParameters, Lifetime, PreferredLifetime, PrimaryAddressId, SlaacConfig,
        TemporarySlaacConfig, WeakAddressId,
    };
    pub use crate::internal::device::{
        AddressRemovedReason, DelIpAddr, IpAddressState, IpDeviceAddAddressContext,
        IpDeviceAddressContext, IpDeviceBindingsContext, IpDeviceConfigurationContext,
        IpDeviceEvent, IpDeviceHandler, IpDeviceIpExt, IpDeviceSendContext, IpDeviceStateContext,
        IpDeviceTimerId, Ipv4DeviceTimerId, Ipv6DeviceConfigurationContext, Ipv6DeviceContext,
        Ipv6DeviceHandler, Ipv6DeviceTimerId, Ipv6LinkLayerAddr, WithIpDeviceConfigurationMutInner,
        WithIpv6DeviceConfigurationMutInner, add_ip_addr_subnet_with_config,
        clear_ipv4_device_state, clear_ipv6_device_state, del_ip_addr_inner, get_ipv4_addr_subnet,
        get_ipv6_hop_limit, is_ip_device_enabled, is_ip_multicast_forwarding_enabled,
        is_ip_unicast_forwarding_enabled, join_ip_multicast, join_ip_multicast_with_config,
        leave_ip_multicast, leave_ip_multicast_with_config, on_arp_packet, receive_igmp_packet,
    };

    /// IP device test utilities.
    #[cfg(any(test, feature = "testutils"))]
    pub mod testutil {
        pub use crate::internal::device::slaac::testutil::{
            calculate_slaac_addr_sub, collect_slaac_timers_integration,
        };
        pub use crate::internal::device::testutil::{
            with_assigned_ipv4_addr_subnets, with_assigned_ipv6_addr_subnets,
        };
    }
}

/// Group management protocols.
pub mod gmp {
    pub use crate::internal::gmp::igmp::{
        IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL, IgmpConfigMode, IgmpContext, IgmpContextMarker,
        IgmpCounters, IgmpSendContext, IgmpStateContext, IgmpTimerId, IgmpTypeLayout,
    };
    pub use crate::internal::gmp::mld::{
        MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL, MldConfigMode, MldContext, MldContextMarker,
        MldCounters, MldSendContext, MldStateContext, MldTimerId, MldTypeLayout,
    };
    pub use crate::internal::gmp::{
        GmpGroupState, GmpHandler, GmpQueryHandler, GmpState, GmpStateRef, GmpTimerId, IpExt,
        MulticastGroupSet,
    };
}

/// The Internet Control Message Protocol (ICMP).
pub mod icmp {
    pub use crate::internal::icmp::counters::{
        IcmpCountersIpExt, IcmpRxCounters, IcmpTxCounters, Icmpv4DestUnreachableCounters,
        Icmpv4ParameterProblemCounters, Icmpv4TimeExceededCounters, Icmpv6DestUnreachableCounters,
        Icmpv6ParameterProblemCounters, Icmpv6TimeExceededCounters,
    };
    pub use crate::internal::icmp::{
        EchoTransportContextMarker, IcmpBindingsContext, IcmpBindingsTypes, IcmpError,
        IcmpErrorHandler, IcmpHandlerIpExt, IcmpIpTransportContext, IcmpSendContext, IcmpState,
        IcmpStateContext, Icmpv4Error, Icmpv4StateBuilder, Icmpv6Error, InnerIcmpContext,
        InnerIcmpv4Context, NdpCounters, NdpMessage, NdpRxCounters, NdpTxCounters,
        REQUIRED_NDP_IP_PACKET_HOP_LIMIT, send_ndp_packet,
    };

    /// ICMP test utilities.
    #[cfg(any(test, feature = "testutils"))]
    pub mod testutil {
        pub use crate::internal::icmp::testutil::{
            neighbor_advertisement_ip_packet, neighbor_solicitation_ip_packet,
        };
    }
}

/// Marker traits controlling IP context behavior.
pub mod marker {
    pub use crate::internal::base::{UseIpSocketContextBlanket, UseTransportIpContextBlanket};
    pub use crate::internal::socket::{
        OptionDelegationMarker, UseDeviceIpSocketHandlerBlanket, UseIpSocketHandlerBlanket,
    };
}

/// Neighbor Unreachability Detection.
pub mod nud {
    pub use crate::internal::device::nud::api::{
        NeighborApi, NeighborRemovalError, StaticNeighborInsertionError, TriggerNeighborProbeError,
    };
    pub use crate::internal::device::nud::{
        ConfirmationFlags, DEFAULT_RETRANS_TIMER, Delay, DelegateNudContext, DynamicNeighborState,
        DynamicNeighborUpdateSource, Event, EventDynamicState, EventKind, EventState, Incomplete,
        LinkResolutionContext, LinkResolutionNotifier, LinkResolutionResult, MAX_ENTRIES,
        NeighborState, NudBindingsContext, NudBindingsTypes, NudConfigContext, NudContext,
        NudCounters, NudCountersInner, NudHandler, NudIcmpContext, NudIpHandler, NudSenderContext,
        NudState, NudTimerId, NudUserConfig, NudUserConfigUpdate, Reachable, Stale,
        UseDelegateNudContext, confirm_reachable,
    };

    /// NUD test utilities.
    #[cfg(any(test, feature = "testutils"))]
    pub mod testutil {
        pub use crate::internal::device::nud::testutil::{
            FakeLinkResolutionNotifier, assert_dynamic_neighbor_state,
            assert_dynamic_neighbor_with_addr, assert_neighbor_unknown,
        };
    }
}

/// IP Layer definitions supporting sockets.
pub mod socket {
    pub use crate::internal::socket::{
        DefaultIpSocketOptions, DelegatedRouteResolutionOptions, DelegatedSendOptions,
        DeviceIpSocketHandler, IpSock, IpSockCreateAndSendError, IpSockCreationError,
        IpSockDefinition, IpSockSendError, IpSocketArgs, IpSocketBindingsContext, IpSocketContext,
        IpSocketHandler, MmsError, RouteResolutionOptions, SendOneShotIpPacketError, SendOptions,
        SocketHopLimits,
    };

    /// IP Socket test utilities.
    #[cfg(any(test, feature = "testutils"))]
    pub mod testutil {
        pub use crate::internal::socket::testutil::{
            FakeDeviceConfig, FakeDualStackIpSocketCtx, FakeIpSocketCtx, InnerFakeIpSocketCtx,
        };
    }
}

/// Multicast Forwarding
pub mod multicast_forwarding {
    pub use crate::internal::multicast_forwarding::api::{
        MulticastForwardingApi, MulticastForwardingDisabledError,
    };
    pub use crate::internal::multicast_forwarding::counters::MulticastForwardingCounters;
    pub use crate::internal::multicast_forwarding::packet_queue::MulticastForwardingPendingPackets;
    pub use crate::internal::multicast_forwarding::route::{
        ForwardMulticastRouteError, MulticastRoute, MulticastRouteKey, MulticastRouteStats,
        MulticastRouteTarget, MulticastRouteTargets,
    };
    pub use crate::internal::multicast_forwarding::state::{
        MulticastForwardingEnabledState, MulticastForwardingPendingPacketsContext,
        MulticastForwardingState, MulticastForwardingStateContext, MulticastRouteTable,
        MulticastRouteTableContext,
    };
    pub use crate::internal::multicast_forwarding::{
        MulticastForwardingBindingsContext, MulticastForwardingBindingsTypes,
        MulticastForwardingDeviceContext, MulticastForwardingEvent, MulticastForwardingTimerId,
    };
}

/// Raw IP sockets.
pub mod raw {
    pub use crate::internal::raw::counters::RawIpSocketCounters;
    pub use crate::internal::raw::filter::RawIpSocketIcmpFilter;
    pub use crate::internal::raw::protocol::RawIpSocketProtocol;
    pub use crate::internal::raw::state::{RawIpSocketLockedState, RawIpSocketState};
    pub use crate::internal::raw::{
        RawIpSocketApi, RawIpSocketIcmpFilterError, RawIpSocketId, RawIpSocketMap,
        RawIpSocketMapContext, RawIpSocketSendToError, RawIpSocketStateContext,
        RawIpSocketsBindingsContext, RawIpSocketsBindingsTypes, ReceivePacketError,
        WeakRawIpSocketId,
    };
}

pub use internal::api::{RouteResolveOptions, RoutesAnyApi, RoutesApi, RoutesApiTableId};
pub use internal::base::{
    AddressStatus, BaseRoutingTableState, BaseTransportIpContext, DEFAULT_HOP_LIMITS, DEFAULT_TTL,
    DeviceIpLayerMetadata, DropReason, FilterHandlerProvider, HopLimits, IPV6_DEFAULT_SUBNET,
    IpDeviceConfirmReachableContext, IpDeviceContext, IpDeviceEgressStateContext,
    IpDeviceIngressStateContext, IpDeviceMtuContext, IpLayerBindingsContext, IpLayerContext,
    IpLayerEvent, IpLayerHandler, IpLayerIpExt, IpLayerTimerId, IpPacketDestination,
    IpRouteTableContext, IpRouteTablesContext, IpSendFrameError, IpSendFrameErrorReason,
    IpStateContext, IpStateInner, IpTransportContext, IpTransportDispatchContext,
    Ipv4PresentAddressStatus, Ipv4State, Ipv4StateBuilder, Ipv6PresentAddressStatus, Ipv6State,
    Ipv6StateBuilder, MarksBindingsContext, MulticastMembershipHandler, NdpBindingsContext,
    ReceivePacketAction, ResolveRouteError, RouterAdvertisementEvent, RoutingTableCookie,
    RoutingTableId, SendIpPacketMeta, SocketMetadata, TransportIpContext, gen_ip_packet_id,
    receive_ipv4_packet, receive_ipv4_packet_action, receive_ipv6_packet,
    receive_ipv6_packet_action, resolve_output_route_to_destination,
};
pub use internal::counters::{IpCounters, Ipv6RxCounters};
pub use internal::fragmentation::FragmentationCounters;
pub use internal::local_delivery::{
    IpHeaderInfo, LocalDeliveryPacketInfo, ReceiveIpPacketMeta, TransparentLocalDelivery,
};
pub use internal::path_mtu::{PmtuCache, PmtuContext};
pub use internal::reassembly::{FragmentContext, FragmentTimerId, IpPacketFragmentCache};
pub use internal::routing::rules::{
    Rule, RuleAction, RuleMatcher, RulesTable, TrafficOriginMatcher,
};
pub use internal::routing::{
    AddRouteError, IpRoutingBindingsTypes, IpRoutingDeviceContext, NonLocalSrcAddrPolicy,
    PacketOrigin, RoutingTable, request_context_add_route, request_context_del_routes,
};
pub use internal::sas::IpSasHandler;
pub use internal::types::{
    AddableEntry, AddableEntryEither, AddableMetric, Destination, Entry, EntryEither, Generation,
    InternalForwarding, Metric, NextHop, RawMetric, ResolvedRoute, RoutableIpAddr, RoutePreference,
};

/// IP layer test utilities.
#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    pub use crate::internal::base::testutil::DualStackSendIpPacketMeta;
    pub use crate::internal::counters::testutil::IpCounterExpectations;
    pub use crate::internal::local_delivery::testutil::FakeIpHeaderInfo;
    pub use crate::internal::routing::testutil::{
        add_route, del_device_routes, del_routes_to_subnet, set_rules,
    };
}
