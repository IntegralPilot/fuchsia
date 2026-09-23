// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Base type definitions for netstack3 core.
//!
//! This crate contains definitions common to the other netstack3 core crates
//! and is the base dependency for most of them.

#![no_std]
#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

extern crate alloc;

mod context;
mod convert;
mod counters;
mod data_structures;
mod device;
mod error;
mod event;
mod frame;
mod inspect;
mod ip;
mod map_deref;
mod matchers;
mod num;
mod packet;
mod port_alloc;
mod resource_references;
mod rng;
mod settings;
mod tcp;
mod test_only;
mod time;
mod uninstantiable;
mod work_queue;

pub use context::{BuildableCoreContext, ContextPair, ContextProvider, CtxPair};
pub use convert::{BidirectionalConverter, OwnedOrRefsBidirectionalConverter};
pub use counters::{
    Counter, CounterCollection, CounterCollectionSpec, CounterContext, CounterRepr,
    ResourceCounterContext,
};
pub use data_structures::token_bucket::TokenBucket;
pub use device::address::{
    AssignedAddrIpExt, IpAddressId, IpDeviceAddr, IpDeviceAddressIdContext, Ipv4DeviceAddr,
    Ipv6DeviceAddr, WeakIpAddressId,
};
pub use device::link::{LinkAddress, LinkDevice, LinkDeviceAddress};
pub use device::{
    AnyDevice, Device, DeviceIdAnyCompatContext, DeviceIdContext, DeviceIdentifier, EitherDeviceId,
    StrongDeviceIdentifier, WeakDeviceIdentifier,
};
pub use error::{
    AddressResolutionFailed, ErrorAndSerializer, ExistsError, LocalAddressError, NotFoundError,
    NotSupportedError, RemoteAddressError, SocketError, ZonedAddressError,
};
pub use event::EventContext;
pub use frame::{
    CoreTxMetadataContext, FrameDestination, LocalFrameDestination, NeverBuffer,
    ReceivableFrameMeta, RecvFrameContext, RecvIpFrameMeta, SendFrameContext, SendFrameError,
    SendFrameErrorReason, SendableFrameMeta, TxMetadata, TxMetadataBindingsTypes,
};
pub use inspect::{Inspectable, InspectableValue, Inspector, InspectorDeviceExt, InspectorExt};
pub use ip::{
    BroadcastIpExt, IcmpErrorCode, IcmpIpExt, Icmpv4ErrorCode, Icmpv6ErrorCode, IpExt,
    IpTypesIpExt, Mark, MarkDomain, MarkStorage, Marks, Mms, WrapBroadcastMarker,
};
pub use map_deref::{MapDeref, MapDerefExt};
pub use matchers::{
    AddressMatcher, AddressMatcherEither, AddressMatcherType, BoundAddressMatcherEither,
    BoundInterfaceMatcher, BoundPortMatcher, InterfaceMatcher, InterfaceProperties,
    IpSocketMatcher, IpSocketProperties, IpSocketPropertiesMatcher, MarkInDomainMatcher,
    MarkMatcher, MarkMatchers, Matcher, MatcherBindingsTypes, MaybeSocketTransportProperties,
    PortMatcher, SocketCookieMatcher, SocketTransportProtocolMatcher, SubnetMatcher,
    TcpSocketMatcher, TcpSocketProperties, TcpSocketState, TcpStateMatcher, UdpSocketMatcher,
    UdpSocketProperties, UdpSocketState, UdpStateMatcher,
};
pub use num::PositiveIsize;
pub use packet::{
    ChecksumOffloadResult, ChecksumOffloadSpec, ChecksumRxOffloading,
    DynamicNetworkPartialSerializer, DynamicNetworkSerializer, NetworkParsingContext,
    NetworkPartialSerializer, NetworkSerializationContext, NetworkSerializer,
    ProtocolSpecificOffloadSpec,
};
pub use port_alloc::{EphemeralPort, PortAllocImpl, simple_randomized_port_alloc};
pub use resource_references::{
    DeferredResourceRemovalContext, ReferenceNotifiers, ReferenceNotifiersExt,
    RemoveResourceResult, RemoveResourceResultWithContext,
};

pub use rng::RngContext;
pub use settings::{BufferSizeSettings, SettingsContext};
pub use socket::SocketDiagnosticsSeed;
pub use tcp::base::{Control, EffectiveMss, FragmentedPayload, Mss, MssSizeLimiters};
pub use tcp::segment::{
    HandshakeOptions, MalformedFlags, Options, Payload, PayloadLen, ResetOptions, SackBlock,
    SackBlocks, Segment, SegmentHeader, SegmentOptions, VerifiedTcpSegment,
};
pub use tcp::seqnum::{SeqNum, UnscaledWindowSize, WindowScale, WindowSize};
pub use tcp::timestamp::{
    Milliseconds, RxTimestampOption, Timestamp, TimestampOption, TxTimestampOption, Unitless,
};
pub use test_only::TestOnlyPartialEq;
pub use time::local_timer_heap::LocalTimerHeap;
pub use time::{
    AtomicInstant, CoreTimerContext, HandleableTimer, Instant, InstantBindingsTypes,
    InstantContext, IntoCoreTimerCtx, NestedIntoCoreTimerCtx, TimerBindingsTypes, TimerContext,
    TimerHandler,
};
pub use uninstantiable::{Uninstantiable, UninstantiableWrapper};
pub use work_queue::WorkQueueReport;

/// Reference counted hash map data structure.
pub mod ref_counted_hash_map {
    pub use crate::data_structures::ref_counted_hash_map::{
        InsertResult, RefCountedHashMap, RefCountedHashSet, RemoveResult,
    };
}

/// Common types and utilities for sockets.
pub mod socket {
    mod address;
    mod base;
    mod cookie;
    pub(crate) mod sndbuf;

    pub use address::{
        AddrIsMappedError, AddrVecIter, ConnAddr, ConnInfoAddr, ConnIpAddr, DualStackConnIpAddr,
        DualStackListenerIpAddr, DualStackLocalIp, DualStackRemoteIp, IpAddrVec, ListenerAddr,
        ListenerIpAddr, SocketIpAddr, StrictlyZonedAddr,
    };
    pub use base::{
        AddrEntry, AddrVec, Bound, BoundSocketMap, DualStackIpExt, DualStackTuple, EitherIpProto,
        EitherStack, FoundSockets, IncompatibleError, InsertError, Inserter, Listener,
        ListenerAddrInfo, MaybeDualStack, NotDualStackCapableError, RemoveResult, ReusePortOption,
        SetDualStackEnabledError, SharingDomain, Shutdown, ShutdownType, SocketAddrType,
        SocketDeviceUpdate, SocketDeviceUpdateNotAllowedError, SocketDiagnosticsSeed, SocketInfo,
        SocketIpAddrExt, SocketIpExt, SocketMapAddrSpec, SocketMapAddrStateSpec,
        SocketMapAddrStateUpdateSharingSpec, SocketMapConflictPolicy, SocketMapStateSpec,
        SocketMapUpdateSharingPolicy, SocketStateEntry, SocketZonedAddrExt, UpdateSharingError,
    };
    pub use cookie::SocketCookie;
    pub use sndbuf::{
        SendBufferFullError, SendBufferSpace, SendBufferTracking, SocketWritableListener,
    };
}

/// Defines generic data structures used to implement common application socket
/// functionality for multiple protocols.
pub mod socketmap {
    pub use crate::data_structures::socketmap::{
        Entry, IterShadows, OccupiedEntry, SocketMap, Tagged, VacantEntry,
    };
}

/// Sync utilities common to netstack3.
pub mod sync {
    // TODO(https://fxbug.dev/42062225): Support single-threaded variants of
    // types exported from this module.

    // Exclusively re-exports from the sync crate.
    pub use netstack3_sync::rc::{
        DebugReferences, DynDebugReferences, MapNotifier as MapRcNotifier, Notifier as RcNotifier,
        Primary as PrimaryRc, ResourceToken, ResourceTokenValue, Strong as StrongRc,
        Weak as WeakRc,
    };
    pub use netstack3_sync::{LockGuard, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
}

/// Test utilities provided to all crates.
#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    mod addr;
    mod benchmarks;
    mod fake_bindings;
    mod fake_core;
    mod fake_network;
    mod misc;
    mod monotonic_id;
    mod send_token;

    pub use crate::device::address::testutil::FakeWeakAddressId;
    pub use crate::device::link::testutil::{FakeLinkAddress, FakeLinkDevice, FakeLinkDeviceId};
    pub use crate::device::testutil::{
        FakeDeviceClass, FakeDeviceId, FakeReferencyDeviceId, FakeStrongDeviceId, FakeWeakDeviceId,
        MultipleDevicesId, MultipleDevicesIdState,
    };
    pub use crate::event::testutil::FakeEventCtx;
    pub use crate::frame::testutil::{FakeFrameCtx, FakeTxMetadata, WithFakeFrameContext};
    pub use crate::matchers::testutil::FakeMatcherDeviceId;
    pub use crate::rng::testutil::{FakeCryptoRng, new_rng, run_with_many_seeds};
    pub use crate::settings::testutil::AlwaysDefaultsSettingsContext;
    pub use crate::socket::sndbuf::testutil::FakeSocketWritableListener;
    pub use crate::time::testutil::{
        FakeAtomicInstant, FakeInstant, FakeInstantCtx, FakeTimerCtx, FakeTimerCtxExt, FakeTimerId,
        InstantAndData, WithFakeTimerContext,
    };
    pub use addr::{TEST_ADDRS_V4, TEST_ADDRS_V6, TestAddrs, TestDualStackIpExt, TestIpExt};
    pub use benchmarks::{Bencher, RealBencher, TestBencher};
    pub use fake_bindings::FakeBindingsCtx;
    pub use fake_core::FakeCoreCtx;
    pub use fake_network::{
        FakeNetwork, FakeNetworkLinks, FakeNetworkSpec, PendingFrame, PendingFrameData, StepResult,
    };
    pub use misc::{assert_empty, set_logger_for_test};
    pub use monotonic_id::MonotonicIdentifier;
    pub use send_token::{FakeSendToken, FakeSendTokenTracker};
}

/// Benchmarks defined in the base crate.
#[cfg(benchmark)]
pub mod benchmarks {
    /// Adds benchmarks defined in the base crate to the provided benchmarker.
    pub fn add_benches(
        group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    ) {
        crate::data_structures::token_bucket::benchmarks::add_benches(group)
    }
}
