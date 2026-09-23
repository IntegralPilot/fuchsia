// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 core device layer.
//!
//! This crate contains the device layer for netstack3.

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
    pub(super) mod api;
    pub(super) mod arp;
    pub(super) mod base;
    pub(super) mod blackhole;
    pub(super) mod config;
    pub(super) mod ethernet;
    pub(super) mod gro;
    pub(super) mod id;
    pub(super) mod loopback;
    pub(super) mod pure_ip;
    pub(super) mod queue;
    pub(super) mod socket;
    pub(super) mod state;
}

/// Blackhole devices.
pub mod blackhole {
    pub use crate::internal::base::BlackholeDeviceCounters;
    pub use crate::internal::blackhole::{
        BlackholeDevice, BlackholeDeviceId, BlackholePrimaryDeviceId, BlackholeWeakDeviceId,
    };
}

/// Ethernet devices.
pub mod ethernet {
    pub use crate::internal::base::EthernetDeviceCounters;
    pub use crate::internal::ethernet::{
        DynamicEthernetDeviceState, EthernetCreationProperties, EthernetDeviceEvent,
        EthernetDeviceEventBindingsContext, EthernetIpLinkDeviceDynamicStateContext,
        EthernetIpLinkDeviceStaticStateContext, EthernetLinkDevice, EthernetTimerId,
        MaxEthernetFrameSize, RecvEthernetFrameMeta, StaticEthernetDeviceState, get_mac, get_mtu,
        join_link_multicast, leave_link_multicast, send_as_ethernet_frame_to_dst, send_ip_frame,
        set_mtu,
    };
    pub use crate::internal::id::{
        EthernetDeviceId, EthernetPrimaryDeviceId, EthernetWeakDeviceId,
    };
}

/// Loopback devices.
pub mod loopback {
    pub use crate::internal::loopback::{
        LoopbackCreationProperties, LoopbackDevice, LoopbackDeviceId, LoopbackPrimaryDeviceId,
        LoopbackRxQueueMeta, LoopbackTxQueueMeta, LoopbackWeakDeviceId, send_ip_frame,
    };
}

/// Marker traits controlling Device context behavior.
pub mod marker {
    pub use crate::internal::ethernet::UseArpFrameMetadataBlanket;
}

/// Pure IP devices.
pub mod pure_ip {
    pub use crate::internal::base::PureIpDeviceCounters;
    pub use crate::internal::pure_ip::{
        DynamicPureIpDeviceState, PureIpDevice, PureIpDeviceCreationProperties, PureIpDeviceId,
        PureIpDeviceReceiveFrameMetadata, PureIpDeviceStateContext,
        PureIpDeviceTxQueueFrameMetadata, PureIpHeaderParams, PureIpPrimaryDeviceId,
        PureIpWeakDeviceId, get_mtu, send_ip_frame, set_mtu,
    };
}

/// Device sockets.
pub mod socket {
    pub use crate::internal::socket::{
        AllSockets, AnyDeviceSockets, DeviceSocketAccessor, DeviceSocketApi,
        DeviceSocketBindingsContext, DeviceSocketContext, DeviceSocketCounters, DeviceSocketId,
        DeviceSocketMetadata, DeviceSocketTypes, DeviceSockets, EthernetFrame,
        EthernetHeaderParams, Frame, HeldDeviceSockets, HeldSockets, IpFrame, ParseSentFrameError,
        PrimaryDeviceSocketId, Protocol, ReceiveFrameError, ReceivedFrame, SentFrame, SocketId,
        SocketInfo, SocketState, SocketStateAccessor, Target, TargetDevice, WeakDeviceSocketId,
    };
}

/// Device RX and TX queueing.
pub mod queue {
    pub use crate::internal::queue::api::{ReceiveQueueApi, TransmitQueueApi};
    pub use crate::internal::queue::rx::{
        ReceiveDequeContext, ReceiveQueueBindingsContext, ReceiveQueueContext, ReceiveQueueHandler,
        ReceiveQueueState, ReceiveQueueTypes,
    };
    pub use crate::internal::queue::tx::{
        BufVecU8Allocator, TransmitDequeueContext, TransmitQueueBindingsContext,
        TransmitQueueCommon, TransmitQueueConfiguration, TransmitQueueContext,
        TransmitQueueHandler, TransmitQueueState, TxBufferAllocator, TxQueuePacketMetadataCommon,
    };
    pub use crate::internal::queue::{
        BatchSize, DequeueState, DeviceBufferSpec, ReceiveQueueFullError,
    };
}

pub use internal::api::{DeviceAnyApi, DeviceApi};
pub use internal::arp::{
    ARP_OVERRIDE_LOCK_TIME, ArpConfigContext, ArpContext, ArpCounters, ArpIpLayerContext,
    ArpNudCtx, ArpSenderContext, ArpState, send_arp_request,
};
pub use internal::base::{
    DeviceBufferBindingsTypes, DeviceClassMatcher, DeviceCollectionContext, DeviceCounters,
    DeviceIdAndNameMatcher, DeviceLayerEventDispatcher, DeviceLayerState, DeviceLayerStateTypes,
    DeviceLayerTimerId, DeviceLayerTypes, DeviceSendFrameError, Devices, DevicesIter,
    Ipv6DeviceLinkLayerAddr, OriginTracker, OriginTrackerContext,
};
pub use internal::config::{
    ArpConfiguration, ArpConfigurationUpdate, DeviceConfiguration, DeviceConfigurationContext,
    DeviceConfigurationUpdate, DeviceConfigurationUpdateError, NdpConfiguration,
    NdpConfigurationUpdate,
};
pub use internal::gro::{
    BufferSlice, GroBufferDestination, GroBufferStorage, GroFrameType, GroInputItem, GroIter,
    GroOutputBuffers, GroOutputItem, MaybeContiguousBuffer,
};
pub use internal::id::{BaseDeviceId, DeviceId, DeviceProvider, WeakDeviceId};
pub use internal::state::{
    DeviceStateSpec, DeviceTxOffloadSpecContext, IpLinkDeviceState, IpLinkDeviceStateInner,
};

/// Device layer test utilities.
#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    pub use crate::internal::base::testutil::DeviceCounterExpectations;
    pub use crate::internal::ethernet::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
}
