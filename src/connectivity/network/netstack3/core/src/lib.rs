// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A networking stack.

#![no_std]
// TODO(https://fxbug.dev/339502691): Return to the default limit once lock
// ordering no longer causes overflows.
#![recursion_limit = "256"]
// In case we roll the toolchain and something we're using as a feature has been
// stabilized.
#![allow(stable_features)]
#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]
extern crate alloc;

mod api;
mod context;
mod counters;
mod lock_ordering;
mod marker;
mod state;
mod time;
mod transport;

#[cfg(any(test, feature = "testutils"))]
pub mod testutil;

/// The device layer.
pub mod device {
    #[path = "."]
    pub(crate) mod integration {
        mod base;
        mod blackhole;
        mod ethernet;
        mod loopback;
        mod pure_ip;
        mod socket;

        pub(crate) use base::{
            device_state, device_state_and_core_ctx, get_mtu, ip_device_state,
            ip_device_state_and_core_ctx,
        };
    }

    // Re-exported types.
    pub use netstack3_base::{
        BoundInterfaceMatcher, InterfaceMatcher, InterfaceProperties, StrongDeviceIdentifier,
    };
    pub use netstack3_device::blackhole::{BlackholeDevice, BlackholeDeviceId};
    pub use netstack3_device::ethernet::{
        EthernetCreationProperties, EthernetDeviceEvent, EthernetDeviceId, EthernetLinkDevice,
        EthernetWeakDeviceId, MaxEthernetFrameSize, RecvEthernetFrameMeta,
    };
    pub use netstack3_device::loopback::{
        LoopbackCreationProperties, LoopbackDevice, LoopbackDeviceId, LoopbackWeakDeviceId,
    };
    pub use netstack3_device::pure_ip::{
        PureIpDevice, PureIpDeviceCreationProperties, PureIpDeviceId,
        PureIpDeviceReceiveFrameMetadata, PureIpHeaderParams, PureIpWeakDeviceId,
    };
    pub use netstack3_device::queue::{
        BatchSize, ReceiveQueueBindingsContext, TransmitQueueBindingsContext,
        TransmitQueueConfiguration, TxBufferAllocator,
    };
    pub use netstack3_device::{
        ArpConfiguration, ArpConfigurationUpdate, BufferSlice, DeviceBufferBindingsTypes,
        DeviceClassMatcher, DeviceConfiguration, DeviceConfigurationUpdate,
        DeviceConfigurationUpdateError, DeviceCounters, DeviceId, DeviceIdAndNameMatcher,
        DeviceLayerEventDispatcher, DeviceLayerStateTypes, DeviceProvider, DeviceSendFrameError,
        GroBufferDestination, GroBufferStorage, GroFrameType, GroInputItem, GroIter,
        GroOutputBuffers, GroOutputItem, MaybeContiguousBuffer, NdpConfiguration,
        NdpConfigurationUpdate, WeakDeviceId,
    };
}

/// Device socket API.
pub mod device_socket {
    pub use netstack3_base::{FrameDestination, SendFrameErrorReason};
    pub use netstack3_device::socket::{
        DeviceSocketBindingsContext, DeviceSocketMetadata, DeviceSocketTypes, EthernetFrame,
        EthernetHeaderParams, Frame, IpFrame, Protocol, ReceiveFrameError, ReceivedFrame,
        SentFrame, SocketId, SocketInfo, TargetDevice, WeakDeviceSocketId,
    };
}

/// Generic netstack errors.
pub mod error {
    pub use netstack3_base::{
        AddressResolutionFailed, ExistsError, LocalAddressError, NotFoundError, NotSupportedError,
        RemoteAddressError, SocketError, ZonedAddressError,
    };
}

/// Framework for packet filtering.
pub mod filter {
    mod integration;

    pub use netstack3_filter::{
        Action, BindingsPacketMatcher, EitherIpProto, FilterApi, FilterBindingsContext,
        FilterBindingsTypes, FilterIpExt, FilterIpPacket, FilterPacketMetadata, Hook, Interfaces,
        IpPacket, IpRoutines, MarkAction, NatRoutines, PacketMatcher, ProofOfEgressCheck,
        RejectType, Routine, Routines, Rule, SocketEgressFilterResult, SocketInfo,
        SocketIngressFilterResult, SocketOpsFilter, SocketOpsFilterBindingContext,
        TransparentProxy, TransportProtocolMatcher, Tuple, UninstalledRoutine, ValidationError,
    };
}

/// Facilities for inspecting stack state for debugging.
pub mod inspect {
    pub use netstack3_base::{
        Inspectable, InspectableValue, Inspector, InspectorDeviceExt, InspectorExt,
    };
}

/// Methods for dealing with ICMP sockets.
pub mod icmp {
    pub use netstack3_icmp_echo::{
        IcmpEchoBindingsContext, IcmpEchoBindingsTypes, IcmpEchoSettings, IcmpSocketId,
        ReceiveIcmpEchoError,
    };
}

/// The Internet Protocol, versions 4 and 6.
pub mod ip {
    #[path = "."]
    pub(crate) mod integration {
        mod base;
        mod device;
        mod multicast_forwarding;
        mod raw;

        pub(crate) use device::CoreCtxWithIpDeviceConfiguration;
    }

    // Re-exported types.
    pub use netstack3_base::{
        AddressMatcher, AddressMatcherEither, AddressMatcherType, BoundAddressMatcherEither,
        BoundPortMatcher, Mark, MarkDomain, MarkInDomainMatcher, MarkMatcher, MarkMatchers, Marks,
        PortMatcher, SubnetMatcher, WrapBroadcastMarker,
    };
    pub use netstack3_ip::device::{
        AddIpAddrSubnetError, AddrSubnetAndManualConfigEither, AddressRemovedReason,
        CommonAddressConfig, CommonAddressProperties, IidGenerationConfiguration, IidSecret,
        IpAddressState, IpDeviceConfiguration, IpDeviceConfigurationAndFlags,
        IpDeviceConfigurationUpdate, IpDeviceEvent, IpDeviceIpExt, Ipv4AddrConfig,
        Ipv4DeviceConfiguration, Ipv4DeviceConfigurationUpdate, Ipv6AddrManualConfig,
        Ipv6DeviceConfiguration, Ipv6DeviceConfigurationUpdate, Lifetime,
        PendingIpDeviceConfigurationUpdate, PreferredLifetime, RouteDiscoveryConfiguration,
        RouteDiscoveryConfigurationUpdate, SetIpAddressPropertiesError, SlaacConfiguration,
        SlaacConfigurationUpdate, StableSlaacAddressConfiguration,
        TemporarySlaacAddressConfiguration, UpdateIpConfigurationError,
    };
    pub use netstack3_ip::gmp::{IgmpConfigMode, MldConfigMode};
    pub use netstack3_ip::multicast_forwarding::{
        ForwardMulticastRouteError, MulticastForwardingDisabledError, MulticastForwardingEvent,
        MulticastRoute, MulticastRouteKey, MulticastRouteStats, MulticastRouteTarget,
    };
    pub use netstack3_ip::raw::{
        RawIpSocketIcmpFilter, RawIpSocketIcmpFilterError, RawIpSocketId, RawIpSocketProtocol,
        RawIpSocketSendToError, RawIpSocketsBindingsContext, RawIpSocketsBindingsTypes,
        ReceivePacketError, WeakRawIpSocketId,
    };
    pub use netstack3_ip::socket::{
        IpSockCreateAndSendError, IpSockCreationError, IpSockSendError,
    };
    pub use netstack3_ip::{
        IpLayerEvent, IpRoutingBindingsTypes, MarksBindingsContext, ResolveRouteError,
        RouterAdvertisementEvent, SocketMetadata,
    };
}

/// Types and utilities for dealing with neighbors.
pub mod neighbor {
    // Re-exported types.
    pub use netstack3_ip::nud::{
        Event, EventDynamicState, EventKind, EventState, LinkResolutionContext,
        LinkResolutionNotifier, LinkResolutionResult, MAX_ENTRIES, NeighborRemovalError,
        NudUserConfig, NudUserConfigUpdate, StaticNeighborInsertionError,
        TriggerNeighborProbeError,
    };
}

/// Types and utilities for dealing with routes.
pub mod routes {
    // Re-exported types.
    pub use netstack3_base::{Marks, WrapBroadcastMarker};
    pub use netstack3_ip::{
        AddRouteError, AddableEntry, AddableEntryEither, AddableMetric, Entry, EntryEither,
        Generation, Metric, NextHop, RawMetric, ResolvedRoute, RoutableIpAddr, RoutePreference,
        RouteResolveOptions, RoutingTableCookie, RoutingTableId, Rule, RuleAction, RuleMatcher,
        TrafficOriginMatcher,
    };
}

/// Common types for dealing with sockets.
pub mod socket {
    pub use netstack3_datagram::{
        ConnInfo, ConnectError, ExpectedConnError, ExpectedUnboundError, ListenerInfo,
        MulticastInterfaceSelector, MulticastMembershipInterfaceSelector,
        PendingDatagramSocketError, SendError, SendToError, SetMulticastMembershipError,
        SocketInfo,
    };

    pub use netstack3_base::socket::{
        AddrIsMappedError, NotDualStackCapableError, ReusePortOption, SetDualStackEnabledError,
        SharingDomain, ShutdownType, SocketCookie, SocketWritableListener, StrictlyZonedAddr,
    };

    pub use netstack3_base::{
        IpSocketMatcher, SocketCookieMatcher, SocketTransportProtocolMatcher, TcpSocketMatcher,
        TcpStateMatcher, UdpSocketMatcher, UdpStateMatcher,
    };
}

/// Useful synchronization primitives.
pub mod sync {
    // We take all of our dependencies directly from base for symmetry with the
    // other crates. However, we want to explicitly have all the dependencies in
    // GN so we can assert the dependencies on the crate variants. This defeats
    // rustc's unused dependency check.
    use netstack3_sync as _;

    pub use netstack3_base::sync::{
        DebugReferences, DynDebugReferences, LockGuard, MapRcNotifier, Mutex, PrimaryRc,
        RcNotifier, ResourceToken, ResourceTokenValue, RwLock, RwLockReadGuard, RwLockWriteGuard,
        StrongRc, WeakRc,
    };
    pub use netstack3_base::{RemoveResourceResult, RemoveResourceResultWithContext};
}

/// Methods for dealing with TCP sockets.
pub mod tcp {
    pub use netstack3_base::{FragmentedPayload, Payload, PayloadLen, TcpSocketState};
    pub use netstack3_tcp::{
        AcceptError, BindError, BoundInfo, Buffer, BufferLimits, BufferSizes,
        CongestionControlState, ConnectError, ConnectionError, ConnectionInfo,
        DEFAULT_FIN_WAIT2_TIMEOUT, IntoBuffers, ListenError, ListenerNotifier, NoConnection,
        OriginalDestinationError, ReceiveBuffer, SendBuffer, SetDeviceError, SetReuseAddrError,
        SocketAddr, SocketInfo, SocketOptions, TcpBindingsTypes, TcpSettings,
        TcpSocketDestructionContext, TcpSocketDiagnosticTuple, TcpSocketDiagnostics, TcpSocketId,
        TcpSocketInfo, UnboundInfo,
    };
}

/// Tracing utilities.
pub mod trace {
    // Re-export all of the trace crate to match how the rest of core works.
    pub use netstack3_trace::*;
}

/// Miscellaneous and common types.
pub mod types {
    pub use netstack3_base::{BufferSizeSettings, Counter, PositiveIsize, WorkQueueReport};
}

/// Methods for dealing with UDP sockets.
pub mod udp {
    pub use netstack3_udp::{
        ReceiveUdpError, SendError, SendToError, UdpBindingsTypes, UdpPacketMeta,
        UdpReceiveBindingsContext, UdpRemotePort, UdpSettings, UdpSocketDiagnosticTuple,
        UdpSocketDiagnostics, UdpSocketId,
    };
}

pub use api::CoreApi;
pub use context::{CoreCtx, UnlockedCoreCtx};
pub use inspect::Inspector;
pub use marker::{BindingsContext, BindingsTypes, CoreContext, IpBindingsContext, IpExt};
pub use netstack3_base::{
    ChecksumOffloadResult, ChecksumOffloadSpec, ChecksumRxOffloading, CtxPair,
    DeferredResourceRemovalContext, EventContext, InstantBindingsTypes, InstantContext,
    MapDerefExt, MatcherBindingsTypes, NetworkParsingContext, NetworkSerializationContext,
    ProtocolSpecificOffloadSpec, ReferenceNotifiers, RngContext, SettingsContext,
    SocketDiagnosticsSeed, TimerBindingsTypes, TimerContext, TxMetadata, TxMetadataBindingsTypes,
};
pub use netstack3_datagram::PendingDatagramSocketError;
pub use state::{StackState, StackStateBuilder};
pub use time::{AtomicInstant, Instant, TimerId};
pub use transport::CoreTxMetadata;

// Re-export useful macros.
pub use netstack3_device::for_any_device_id;
pub use netstack3_macros::context_ip_bounds;

// Rust compiler spinning workaround (https://fxbug.dev/395694598):
//
// If you find yourself with a spinning rustc because you're changing traits
// that need to be implemented by bindings, uncomment the lines below and give
// it a go. See attached bug for details.
//
// unsafe impl<BT: BindingsTypes> Send for TimerId<BT> {}
// unsafe impl<BT: BindingsTypes> Sync for TimerId<BT> {}
// unsafe impl<BT: BindingsTypes> Send for TxMetadata<BT> {}
// unsafe impl<BT: BindingsTypes> Sync for TxMetadata<BT> {}
