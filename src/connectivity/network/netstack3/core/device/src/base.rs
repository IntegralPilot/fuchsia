// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::{Debug, Display};
use core::num::NonZeroU64;

use derivative::Derivative;
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use netstack3_base::sync::RwLock;
use netstack3_base::{
    Counter, CounterRepr, Device, DeviceIdContext, HandleableTimer, Inspectable, Inspector,
    InspectorExt as _, InstantContext, ReferenceNotifiers, TimerBindingsTypes, TimerHandler,
    TxMetadataBindingsTypes,
};
use netstack3_filter::FilterBindingsTypes;
use netstack3_hashmap::HashMap;
use netstack3_ip::device::Ipv6LinkLayerAddr;
use netstack3_ip::nud::{LinkResolutionContext, NudCounters};

use crate::blackhole::{BlackholeDeviceId, BlackholePrimaryDeviceId};
use crate::internal::arp::ArpCounters;
use crate::internal::ethernet::{EthernetLinkDevice, EthernetTimerId};
use crate::internal::id::{
    BaseDeviceId, BasePrimaryDeviceId, DeviceId, EthernetDeviceId, EthernetPrimaryDeviceId,
    EthernetWeakDeviceId,
};
use crate::internal::loopback::{LoopbackDeviceId, LoopbackPrimaryDeviceId};
use crate::internal::pure_ip::{PureIpDeviceId, PureIpPrimaryDeviceId};
use crate::internal::queue::rx::ReceiveQueueBindingsContext;
use crate::internal::queue::tx::{TransmitQueueBindingsContext, TxBufferAllocator};
use crate::internal::socket::{self, DeviceSocketCounters, HeldSockets};
use crate::internal::state::DeviceStateSpec;

/// Iterator over devices.
///
/// Implements `Iterator<Item=DeviceId<C>>` by pulling from provided loopback
/// and ethernet device ID iterators. This struct only exists as a named type
/// so it can be an associated type on impls of the [`IpDeviceContext`] trait.
pub struct DevicesIter<'s, BT: DeviceLayerTypes> {
    pub(super) ethernet:
        netstack3_hashmap::hash_map::Values<'s, EthernetDeviceId<BT>, EthernetPrimaryDeviceId<BT>>,
    pub(super) pure_ip:
        netstack3_hashmap::hash_map::Values<'s, PureIpDeviceId<BT>, PureIpPrimaryDeviceId<BT>>,
    pub(super) blackhole: netstack3_hashmap::hash_map::Values<
        's,
        BlackholeDeviceId<BT>,
        BlackholePrimaryDeviceId<BT>,
    >,
    pub(super) loopback: core::option::Iter<'s, LoopbackPrimaryDeviceId<BT>>,
}

impl<'s, BT: DeviceLayerTypes> Iterator for DevicesIter<'s, BT> {
    type Item = DeviceId<BT>;

    fn next(&mut self) -> Option<Self::Item> {
        let Self { ethernet, pure_ip, blackhole, loopback } = self;
        ethernet
            .map(|primary| primary.clone_strong().into())
            .chain(pure_ip.map(|primary| primary.clone_strong().into()))
            .chain(blackhole.map(|primary| primary.clone_strong().into()))
            .chain(loopback.map(|primary| primary.clone_strong().into()))
            .next()
    }
}

/// Supported link layer address types for IPv6.
#[allow(missing_docs)]
pub enum Ipv6DeviceLinkLayerAddr {
    Mac(Mac),
    // Add other link-layer address types as needed.
}

impl Ipv6LinkLayerAddr for Ipv6DeviceLinkLayerAddr {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Ipv6DeviceLinkLayerAddr::Mac(a) => a.as_ref(),
        }
    }

    fn eui64_iid(&self) -> [u8; 8] {
        match self {
            Ipv6DeviceLinkLayerAddr::Mac(a) => a.to_eui64(),
        }
    }
}

/// The identifier for timer events in the device layer.
#[derive(Derivative)]
#[derivative(
    Clone(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = ""),
    Debug(bound = "")
)]
pub struct DeviceLayerTimerId<BT: DeviceLayerTypes>(DeviceLayerTimerIdInner<BT>);

#[derive(Derivative)]
#[derivative(
    Clone(bound = ""),
    Eq(bound = ""),
    PartialEq(bound = ""),
    Hash(bound = ""),
    Debug(bound = "")
)]
#[allow(missing_docs)]
enum DeviceLayerTimerIdInner<BT: DeviceLayerTypes> {
    Ethernet(EthernetTimerId<EthernetWeakDeviceId<BT>>),
}

impl<BT: DeviceLayerTypes> From<EthernetTimerId<EthernetWeakDeviceId<BT>>>
    for DeviceLayerTimerId<BT>
{
    fn from(id: EthernetTimerId<EthernetWeakDeviceId<BT>>) -> DeviceLayerTimerId<BT> {
        DeviceLayerTimerId(DeviceLayerTimerIdInner::Ethernet(id))
    }
}

impl<CC, BT> HandleableTimer<CC, BT> for DeviceLayerTimerId<BT>
where
    BT: DeviceLayerTypes,
    CC: TimerHandler<BT, EthernetTimerId<EthernetWeakDeviceId<BT>>>,
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BT, timer: BT::UniqueTimerId) {
        let Self(id) = self;
        match id {
            DeviceLayerTimerIdInner::Ethernet(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
        }
    }
}

/// The collection of devices within [`DeviceLayerState`].
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct Devices<BT: DeviceLayerTypes> {
    /// Collection of Ethernet devices.
    pub ethernet: HashMap<EthernetDeviceId<BT>, EthernetPrimaryDeviceId<BT>>,
    /// Collection of PureIP devices.
    pub pure_ip: HashMap<PureIpDeviceId<BT>, PureIpPrimaryDeviceId<BT>>,
    /// Collection of blackhole devices.
    pub blackhole: HashMap<BlackholeDeviceId<BT>, BlackholePrimaryDeviceId<BT>>,
    /// The loopback device, if installed.
    pub loopback: Option<LoopbackPrimaryDeviceId<BT>>,
}

impl<BT: DeviceLayerTypes> Devices<BT> {
    /// Gets an iterator over available devices.
    pub fn iter(&self) -> DevicesIter<'_, BT> {
        let Self { ethernet, pure_ip, blackhole, loopback } = self;
        DevicesIter {
            ethernet: ethernet.values(),
            pure_ip: pure_ip.values(),
            blackhole: blackhole.values(),
            loopback: loopback.iter(),
        }
    }
}

/// The state associated with the device layer.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct DeviceLayerState<BT: DeviceLayerTypes> {
    devices: RwLock<Devices<BT>>,
    /// Device layer origin tracker.
    pub origin: OriginTracker,
    /// Collection of all device sockets.
    pub shared_sockets: HeldSockets<BT>,
    /// Device socket counters.
    pub device_socket_counters: DeviceSocketCounters,
    /// Common device counters.
    pub counters: DeviceCounters,
    /// Ethernet counters.
    pub ethernet_counters: EthernetDeviceCounters,
    /// PureIp counters.
    pub pure_ip_counters: PureIpDeviceCounters,
    /// IPv4 NUD counters.
    pub nud_v4_counters: NudCounters<Ipv4>,
    /// IPv6 NUD counters.
    pub nud_v6_counters: NudCounters<Ipv6>,
    /// ARP counters.
    pub arp_counters: ArpCounters,
}

impl<BT: DeviceLayerTypes> DeviceLayerState<BT> {
    /// Helper to access NUD counters for an IP version.
    pub fn nud_counters<I: Ip>(&self) -> &NudCounters<I> {
        I::map_ip((), |()| &self.nud_v4_counters, |()| &self.nud_v6_counters)
    }
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<Devices<BT>> for DeviceLayerState<BT> {
    type Lock = RwLock<Devices<BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.devices)
    }
}

/// Counters for ethernet devices.
#[derive(Default)]
pub struct EthernetDeviceCounters {
    /// Count of incoming frames dropped due to an unsupported ethertype.
    pub recv_unsupported_ethertype: Counter,
    /// Count of incoming frames dropped due to an empty ethertype.
    pub recv_no_ethertype: Counter,
    /// Count of incoming frames with a single checksum offloaded.
    pub recv_single_csum_offloaded: Counter,
    /// Count of incoming frames with multiple checksums offloaded.
    pub recv_multiple_csums_offloaded: Counter,
    /// Count of incoming frames with all checksums offloaded.
    pub recv_all_csums_offloaded: Counter,
}

impl Inspectable for EthernetDeviceCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        inspector.record_child("Ethernet", |inspector| {
            let Self {
                recv_no_ethertype,
                recv_unsupported_ethertype,
                recv_single_csum_offloaded,
                recv_multiple_csums_offloaded,
                recv_all_csums_offloaded,
            } = self;
            inspector.record_child("Rx", |inspector| {
                inspector.record_counter("NoEthertype", recv_no_ethertype);
                inspector.record_counter("UnsupportedEthertype", recv_unsupported_ethertype);
                inspector.record_counter("SingleCsumOffloaded", recv_single_csum_offloaded);
                inspector.record_counter("MultipleCsumsOffloaded", recv_multiple_csums_offloaded);
                inspector.record_counter("AllCsumsOffloaded", recv_all_csums_offloaded);
            });
        })
    }
}

/// Counters for pure IP devices.
#[derive(Default)]
pub struct PureIpDeviceCounters {
    /// Count of incoming frames with a single checksum offloaded.
    pub recv_single_csum_offloaded: Counter,
    /// Count of incoming frames with multiple checksums offloaded.
    pub recv_multiple_csums_offloaded: Counter,
    /// Count of incoming frames with all checksums offloaded.
    pub recv_all_csums_offloaded: Counter,
}

impl Inspectable for PureIpDeviceCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        inspector.record_child("PureIp", |inspector| {
            let Self {
                recv_single_csum_offloaded,
                recv_multiple_csums_offloaded,
                recv_all_csums_offloaded,
            } = self;
            inspector.record_child("Rx", |inspector| {
                inspector.record_counter("SingleCsumOffloaded", recv_single_csum_offloaded);
                inspector.record_counter("MultipleCsumsOffloaded", recv_multiple_csums_offloaded);
                inspector.record_counter("AllCsumsOffloaded", recv_all_csums_offloaded);
            });
        })
    }
}

/// Counters for blackhole devices.
pub struct BlackholeDeviceCounters;

impl Inspectable for BlackholeDeviceCounters {
    fn record<I: Inspector>(&self, _inspector: &mut I) {}
}

/// Device layer counters.
#[derive(Default, Debug)]
#[cfg_attr(
    any(test, feature = "testutils"),
    derive(PartialEq, netstack3_macros::CounterCollection)
)]
pub struct DeviceCounters<C: CounterRepr = Counter> {
    /// Count of outgoing frames which enter the device layer (but may or may
    /// not have been dropped prior to reaching the wire).
    pub send_total_frames: C,
    /// Count of frames sent.
    pub send_frame: C,
    /// Count of bytes sent.
    pub send_bytes: C,
    /// Count of frames that failed to send because of a full Tx queue.
    pub send_queue_full: C,
    /// Count of frames that failed to send because of a serialization error.
    pub send_serialize_error: C,
    /// Count of frames received.
    pub recv_frame: C,
    /// Count of bytes received.
    pub recv_bytes: C,
    /// Count of incoming frames dropped due to a parsing error.
    pub recv_parse_error: C,
    /// Count of incoming frames containing an IPv4 packet delivered.
    pub recv_ipv4_delivered: C,
    /// Count of incoming frames containing an IPv6 packet delivered.
    pub recv_ipv6_delivered: C,
    /// Count of sent frames containing an IPv4 packet.
    pub send_ipv4_frame: C,
    /// Count of sent frames containing an IPv6 packet.
    pub send_ipv6_frame: C,
    /// Count of frames that failed to send because there was no Tx queue.
    pub send_dropped_no_queue: C,
    /// Count of frames that were dropped during Tx queue dequeuing.
    pub send_dropped_dequeue: C,
}

impl DeviceCounters {
    /// Either `send_ipv4_frame` or `send_ipv6_frame` depending on `I`.
    pub fn send_frame<I: Ip>(&self) -> &Counter {
        match I::VERSION {
            IpVersion::V4 => &self.send_ipv4_frame,
            IpVersion::V6 => &self.send_ipv6_frame,
        }
    }
}

impl Inspectable for DeviceCounters {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        let Self {
            recv_frame,
            recv_bytes,
            recv_ipv4_delivered,
            recv_ipv6_delivered,
            recv_parse_error,
            send_dropped_no_queue,
            send_frame,
            send_bytes,
            send_ipv4_frame,
            send_ipv6_frame,
            send_queue_full,
            send_serialize_error,
            send_total_frames,
            send_dropped_dequeue,
        } = self;
        inspector.record_child("Rx", |inspector| {
            inspector.record_counter("TotalFrames", recv_frame);
            inspector.record_counter("TotalBytes", recv_bytes);
            inspector.record_counter("Malformed", recv_parse_error);
            inspector.record_counter("Ipv4Delivered", recv_ipv4_delivered);
            inspector.record_counter("Ipv6Delivered", recv_ipv6_delivered);
        });
        inspector.record_child("Tx", |inspector| {
            inspector.record_counter("TotalFrames", send_total_frames);
            inspector.record_counter("Sent", send_frame);
            inspector.record_counter("SentBytes", send_bytes);
            inspector.record_counter("SendIpv4Frame", send_ipv4_frame);
            inspector.record_counter("SendIpv6Frame", send_ipv6_frame);
            inspector.record_counter("NoQueue", send_dropped_no_queue);
            inspector.record_counter("QueueFull", send_queue_full);
            inspector.record_counter("SerializeError", send_serialize_error);
            inspector.record_counter("DequeueDrop", send_dropped_dequeue);
        });
    }
}
/// Light-weight tracker for recording the source of some instance.
///
/// This should be held as a field in a parent type that is cloned into each
/// child instance. Then, the origin of a child instance can be verified by
/// asserting equality against the parent's field.
///
/// This is only enabled in debug builds; in non-debug builds, all
/// `OriginTracker` instances are identical so all operations are no-ops.
// TODO(https://fxbug.dev/320078167): Move this and OriginTrackerContext out of
// the device module and apply to more places.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginTracker(#[cfg(debug_assertions)] u64);

impl Default for OriginTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginTracker {
    /// Creates a new `OriginTracker` that isn't derived from any other
    /// instance.
    ///
    /// In debug builds, this creates a unique `OriginTracker` that won't be
    /// equal to any instances except those cloned from it. In non-debug builds
    /// all `OriginTracker` instances are identical.
    #[cfg_attr(not(debug_assertions), inline)]
    fn new() -> Self {
        Self(
            #[cfg(debug_assertions)]
            {
                static COUNTER: core::sync::atomic::AtomicU64 =
                    core::sync::atomic::AtomicU64::new(0);
                COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
            },
        )
    }
}

/// A trait abstracting a context containing an [`OriginTracker`].
///
/// This allows API structs to extract origin from contexts when creating
/// resources.
pub trait OriginTrackerContext {
    /// Gets the origin tracker for this context.
    fn origin_tracker(&mut self) -> OriginTracker;
}

/// A context providing facilities to store and remove primary device IDs.
///
/// This allows the device layer APIs to be written generically on `D`.
pub trait DeviceCollectionContext<D: Device + DeviceStateSpec, BT: DeviceLayerTypes>:
    DeviceIdContext<D>
{
    /// Adds `device` to the device collection.
    fn insert(&mut self, device: BasePrimaryDeviceId<D, BT>);

    /// Removes `device` from the collection, if it exists.
    fn remove(&mut self, device: &BaseDeviceId<D, BT>) -> Option<BasePrimaryDeviceId<D, BT>>;
}

/// Provides abstractions over the frame metadata received from bindings for
/// implementers of [`Device`].
///
/// This trait allows [`api::DeviceApi`] to provide a single entrypoint for
/// frames from bindings.
pub trait DeviceReceiveFrameSpec {
    /// The frame metadata for ingress frames, where `D` is a device identifier.
    type FrameMetadata<D>;
}

/// Provides associated types used in the device layer.
pub trait DeviceLayerStateTypes: InstantContext + FilterBindingsTypes {
    /// The state associated with loopback devices.
    type LoopbackDeviceState: Send + Sync + DeviceClassMatcher<Self::DeviceClass>;

    /// The state associated with ethernet devices.
    type EthernetDeviceState: Send + Sync + DeviceClassMatcher<Self::DeviceClass>;

    /// The state associated with pure IP devices.
    type PureIpDeviceState: Send + Sync + DeviceClassMatcher<Self::DeviceClass>;

    /// The state associated with blackhole devices.
    type BlackholeDeviceState: Send + Sync + DeviceClassMatcher<Self::DeviceClass>;

    /// An opaque identifier that is available from both strong and weak device
    /// references.
    type DeviceIdentifier: Send + Sync + Debug + Display + DeviceIdAndNameMatcher;
}

/// Provides matching functionality for the device class of a device installed
/// in the netstack.
pub trait DeviceClassMatcher<DeviceClass> {
    /// Returns whether the provided device class matches the class of the
    /// device.
    fn device_class_matches(&self, device_class: &DeviceClass) -> bool;
}

/// Provides matching functionality for the ID and name of a device installed in
/// the netstack.
pub trait DeviceIdAndNameMatcher {
    /// Returns whether the provided ID matches the ID of the device.
    fn id_matches(&self, id: &NonZeroU64) -> bool;

    /// Returns whether the provided name matches the name of the device.
    fn name_matches(&self, name: &str) -> bool;
}

/// Trait for associated types used in the device layer.
pub trait DeviceBufferBindingsTypes: 'static {
    /// The buffer type stored in transmit queues.
    type TxBuffer: packet::FragmentedBuffer + AsMut<[u8]> + Send + 'static;
    /// The allocator for transmit queue buffers.
    type TxAllocator: TxBufferAllocator<Self::TxBuffer> + Send + 'static;
}

/// Provides associated types used in the device layer.
///
/// This trait groups together state types used throughout the device layer. It
/// is blanket-implemented for all types that implement
/// [`socket::DeviceSocketTypes`] and [`DeviceLayerStateTypes`].
pub trait DeviceLayerTypes:
    DeviceBufferBindingsTypes
    + DeviceLayerStateTypes
    + socket::DeviceSocketTypes
    + LinkResolutionContext<EthernetLinkDevice>
    + TimerBindingsTypes
    + ReferenceNotifiers
    + TxMetadataBindingsTypes
    + 'static
{
}
impl<
    BC: DeviceLayerStateTypes
        + socket::DeviceSocketTypes
        + LinkResolutionContext<EthernetLinkDevice>
        + TimerBindingsTypes
        + ReferenceNotifiers
        + TxMetadataBindingsTypes
        + 'static
        + DeviceBufferBindingsTypes,
> DeviceLayerTypes for BC
{
}

/// An event dispatcher for the device layer.
pub trait DeviceLayerEventDispatcher:
    DeviceLayerTypes
    + ReceiveQueueBindingsContext<LoopbackDeviceId<Self>>
    + TransmitQueueBindingsContext<EthernetDeviceId<Self>>
    + TransmitQueueBindingsContext<LoopbackDeviceId<Self>>
    + TransmitQueueBindingsContext<PureIpDeviceId<Self>>
    + Sized
{
    /// The transmit queue dequeueing context used by bindings.
    ///
    /// `DequeueContext` is a passthrough type from bindings (i.e. entirely
    /// opaque to core) when using `TransmitQueueApi` to trigger the transmit
    /// queue to send frames to the underlying devices.
    type DequeueContext;

    /// Send a frame to an Ethernet device driver.
    ///
    /// See [`DeviceSendFrameError`] for the ways this call may fail; all other
    /// errors are silently ignored and reported as success. Implementations are
    /// expected to gracefully handle non-conformant but correctable input, e.g.
    /// by padding too-small frames.
    ///
    /// `dequeue_context` is `Some` iff this is called from the context of
    /// operating the transmit queue via `TransmitQueueApi`.
    fn send_ethernet_frame(
        &mut self,
        device: &EthernetDeviceId<Self>,
        frame: Self::TxBuffer,
        dequeue_context: Option<&mut Self::DequeueContext>,
        csum_offload: Option<netstack3_base::ChecksumOffloadResult>,
    ) -> Result<(), DeviceSendFrameError>;

    /// Send an IP packet to an IP device driver.
    ///
    /// See [`DeviceSendFrameError`] for the ways this call may fail; all other
    /// errors are silently ignored and reported as success. Implementations are
    /// expected to gracefully handle non-conformant but correctable input, e.g.
    /// by padding too-small frames.
    ///
    /// `dequeue_context` is `Some` iff this is called from the context of
    /// operating the transmit queue via `TransmitQueueApi`.
    fn send_ip_packet(
        &mut self,
        device: &PureIpDeviceId<Self>,
        packet: Self::TxBuffer,
        ip_version: IpVersion,
        dequeue_context: Option<&mut Self::DequeueContext>,
        csum_offload: Option<netstack3_base::ChecksumOffloadResult>,
    ) -> Result<(), DeviceSendFrameError>;
}

/// An error encountered when sending a frame.
#[derive(Debug, PartialEq, Eq)]
pub enum DeviceSendFrameError {
    /// The device doesn't have available buffers to send frames.
    NoBuffers,
}

#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    use alloc::vec::Vec;
    use netstack3_base::testutil::FakeBindingsCtx;
    use netstack3_base::{CounterCollection, ResourceCounterContext};

    use super::*;
    use crate::internal::queue::tx::BufVecU8Allocator;

    /// Expected values of [`DeviceCounters`].
    pub type DeviceCounterExpectations = DeviceCounters<u64>;

    impl DeviceCounterExpectations {
        /// Assert that the counters tracked by `core_ctx` match expectations.
        #[track_caller]
        pub fn assert_counters<D, CC: ResourceCounterContext<D, DeviceCounters>>(
            &self,
            core_ctx: &CC,
            device: &D,
        ) {
            assert_eq!(&core_ctx.counters().cast::<u64>(), self, "stack-wide counters");
            assert_eq!(
                &core_ctx.per_resource_counters(device).cast::<u64>(),
                self,
                "per-device counters"
            );
        }
    }

    impl<TimerId: 'static, Event: Debug + 'static, State: 'static, FrameMeta: 'static>
        DeviceBufferBindingsTypes for FakeBindingsCtx<TimerId, Event, State, FrameMeta>
    {
        type TxBuffer = packet::Buf<Vec<u8>>;
        type TxAllocator = BufVecU8Allocator;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_tracker() {
        let tracker = OriginTracker::new();
        if cfg!(debug_assertions) {
            assert_ne!(tracker, OriginTracker::new());
        } else {
            assert_eq!(tracker, OriginTracker::new());
        }
        assert_eq!(tracker.clone(), tracker);
    }
}
