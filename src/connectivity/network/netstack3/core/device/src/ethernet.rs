// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Ethernet protocol.

use core::fmt::Debug;
use core::num::NonZeroU32;
use lock_order::lock::{OrderedLockAccess, OrderedLockRef};

use log::{debug, trace};
use net_types::ethernet::Mac;
use net_types::ip::{GenericOverIp, Ip, IpMarked, Ipv4, Ipv6, Mtu};
use net_types::{MulticastAddr, UnicastAddr, Witness};
use netstack3_base::ref_counted_hash_map::{InsertResult, RefCountedHashSet, RemoveResult};
use netstack3_base::sync::{Mutex, RwLock};
use netstack3_base::{
    BroadcastIpExt, ChecksumOffloadSpec, CoreTimerContext, Device, DeviceIdContext, EventContext,
    FrameDestination, HandleableTimer, LinkDevice, NestedIntoCoreTimerCtx, NetworkParsingContext,
    NetworkSerializer, ReceivableFrameMeta, RecvFrameContext, RecvIpFrameMeta,
    ResourceCounterContext, RngContext, SendFrameError, SendFrameErrorReason, SendableFrameMeta,
    TimerContext, TimerHandler, TxMetadataBindingsTypes, WeakDeviceIdentifier, WrapBroadcastMarker,
};
use netstack3_ip::nud::{LinkResolutionContext, NudHandler, NudState, NudTimerId, NudUserConfig};
use netstack3_ip::{DeviceIpLayerMetadata, IpCounters, IpPacketDestination};
use netstack3_trace::trace_duration;
use packet::{BufferMut, NestablePacketBuilder as _, NestableSerializer as _};
use packet_formats::arp::{ArpHardwareType, ArpNetworkType, peek_arp_types};
use packet_formats::ethernet::{
    ETHERNET_HDR_LEN_NO_TAG, EtherType, EthernetFrame, EthernetFrameBuilder,
    EthernetFrameLengthCheck, EthernetIpExt,
};

use crate::internal::arp::{ArpFrameMetadata, ArpPacketHandler, ArpState, ArpTimerId};
use crate::internal::base::{
    DeviceBufferBindingsTypes, DeviceCounters, DeviceLayerTypes, DeviceReceiveFrameSpec,
    EthernetDeviceCounters,
};
use crate::internal::id::{DeviceId, EthernetDeviceId};
use crate::internal::queue::tx::{TransmitQueue, TransmitQueueHandler, TransmitQueueState};
use crate::internal::queue::{DequeueState, DeviceBufferSpec, TransmitQueueFrameError};
use crate::internal::socket::{
    DeviceSocketHandler, DeviceSocketMetadata, DeviceSocketSendTypes, EthernetHeaderParams,
    ReceivedFrame,
};
use crate::internal::state::{DeviceStateSpec, IpLinkDeviceState};

const ETHERNET_HDR_LEN_NO_TAG_U32: u32 = ETHERNET_HDR_LEN_NO_TAG as u32;

/// The execution context for an Ethernet device provided by bindings.
pub trait EthernetIpLinkDeviceBindingsContext:
    RngContext + TimerContext + DeviceLayerTypes + TxMetadataBindingsTypes
{
}
impl<BC: RngContext + TimerContext + DeviceLayerTypes + TxMetadataBindingsTypes>
    EthernetIpLinkDeviceBindingsContext for BC
{
}

/// The execution context for an Ethernet device provided by bindings.
///
/// This context trait is separate from `EthernetIpLinkDeviceBindingsContext` to prevent
/// trait bound resolution cycles.
pub trait EthernetDeviceEventBindingsContext<DeviceId>:
    EventContext<EthernetDeviceEvent<DeviceId>>
{
}
impl<BC: EventContext<EthernetDeviceEvent<DeviceId>>, DeviceId>
    EthernetDeviceEventBindingsContext<DeviceId> for BC
{
}

/// Provides access to an ethernet device's static state.
pub trait EthernetIpLinkDeviceStaticStateContext: DeviceIdContext<EthernetLinkDevice> {
    /// Calls the function with an immutable reference to the ethernet device's
    /// static state.
    fn with_static_ethernet_device_state<O, F: FnOnce(&StaticEthernetDeviceState) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// Provides access to an ethernet device's dynamic state.
pub trait EthernetIpLinkDeviceDynamicStateContext<BC: EthernetIpLinkDeviceBindingsContext>:
    EthernetIpLinkDeviceStaticStateContext
{
    /// Calls the function with the ethernet device's static state and immutable
    /// reference to the dynamic state.
    fn with_ethernet_state<
        O,
        F: FnOnce(&StaticEthernetDeviceState, &DynamicEthernetDeviceState) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with the ethernet device's static state and mutable
    /// reference to the dynamic state.
    fn with_ethernet_state_mut<
        O,
        F: FnOnce(&StaticEthernetDeviceState, &mut DynamicEthernetDeviceState) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// Events emitted from ethernet devices.
#[derive(Debug, PartialEq, Eq, Hash)]
pub enum EthernetDeviceEvent<D> {
    /// Device joined a new multicast group.
    MulticastJoin {
        /// The device.
        device: D,
        /// The address of the multicast group.
        addr: MulticastAddr<Mac>,
    },

    /// Device left a multicast group.
    MulticastLeave {
        /// The device.
        device: D,
        /// The address of the multicast group.
        addr: MulticastAddr<Mac>,
    },
}

impl<D> EthernetDeviceEvent<D> {
    /// Maps the contained device ID type.
    pub fn map_device<N, F: FnOnce(D) -> N>(self, map: F) -> EthernetDeviceEvent<N> {
        match self {
            Self::MulticastJoin { device, addr } => {
                EthernetDeviceEvent::MulticastJoin { device: map(device), addr }
            }
            Self::MulticastLeave { device, addr } => {
                EthernetDeviceEvent::MulticastLeave { device: map(device), addr }
            }
        }
    }
}

/// Send an Ethernet frame `body` directly to `dst_mac` with `ether_type`.
pub fn send_as_ethernet_frame_to_dst<S, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    dst_mac: Mac,
    body: S,
    ether_type: EtherType,
    meta: BC::TxMetadata,
) -> Result<(), SendFrameError<S>>
where
    S: NetworkSerializer,
    S::Buffer: BufferMut,
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + TransmitQueueHandler<EthernetLinkDevice, BC, Meta = BC::TxMetadata>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
{
    /// The minimum body length for the Ethernet frame.
    ///
    /// Using a frame length of 0 improves efficiency by avoiding unnecessary
    /// padding at this layer. The expectation is that the implementation of
    /// bindings will add any padding required by the implementation.
    const MIN_BODY_LEN: usize = 0;

    let local_mac = get_mac(core_ctx, device_id);
    let max_frame_size = get_max_frame_size(core_ctx, device_id);
    let frame = EthernetFrameBuilder::new(local_mac.get(), dst_mac, ether_type, MIN_BODY_LEN)
        .wrap_body(body)
        .with_size_limit(max_frame_size.into());
    send_ethernet_frame(core_ctx, bindings_ctx, device_id, frame, meta)
        .map_err(|err| err.into_inner().into_inner())
}

fn send_ethernet_frame<S, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    frame: S,
    meta: BC::TxMetadata,
) -> Result<(), SendFrameError<S>>
where
    S: NetworkSerializer,
    S::Buffer: BufferMut,
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + TransmitQueueHandler<EthernetLinkDevice, BC, Meta = BC::TxMetadata>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
{
    core_ctx.increment_both(device_id, |counters| &counters.send_total_frames);
    match TransmitQueueHandler::<EthernetLinkDevice, _>::queue_tx_frame(
        core_ctx,
        bindings_ctx,
        device_id,
        meta,
        frame,
    ) {
        Ok(len) => {
            core_ctx.add_both_usize(device_id, len, |counters| &counters.send_bytes);
            core_ctx.increment_both(device_id, |counters| &counters.send_frame);
            Ok(())
        }
        Err(TransmitQueueFrameError::NoQueue(err)) => {
            core_ctx.increment_both(device_id, |counters| &counters.send_dropped_no_queue);
            debug!("device {device_id:?} failed to send frame: {err:?}.");
            Ok(())
        }
        Err(TransmitQueueFrameError::QueueFull(serializer)) => {
            core_ctx.increment_both(device_id, |counters| &counters.send_queue_full);
            Err(SendFrameError { serializer, error: SendFrameErrorReason::QueueFull })
        }
        Err(TransmitQueueFrameError::SerializeError(err)) => {
            core_ctx.increment_both(device_id, |counters| &counters.send_serialize_error);
            Err(err.err_into())
        }
    }
}

/// The maximum frame size one ethernet device can send.
///
/// The frame size includes the ethernet header, the data payload, but excludes
/// the 4 bytes from FCS (frame check sequence) as we don't calculate CRC and it
/// is normally handled by the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MaxEthernetFrameSize(NonZeroU32);

impl MaxEthernetFrameSize {
    /// The minimum ethernet frame size.
    ///
    /// We don't care about FCS, so the minimum frame size for us is 64 - 4.
    pub const MIN: MaxEthernetFrameSize = MaxEthernetFrameSize(NonZeroU32::new(60).unwrap());

    /// Creates from the maximum size of ethernet header and ethernet payload,
    /// checks that it is valid, i.e., larger than the minimum frame size.
    pub const fn new(frame_size: u32) -> Option<Self> {
        if frame_size < Self::MIN.get().get() {
            return None;
        }
        Some(Self(NonZeroU32::new(frame_size).unwrap()))
    }

    const fn get(&self) -> NonZeroU32 {
        let Self(frame_size) = *self;
        frame_size
    }

    /// Converts the maximum frame size to its corresponding MTU.
    pub const fn as_mtu(&self) -> Mtu {
        // MTU must be positive because of the limit on minimum ethernet frame size
        Mtu::new(self.get().get().saturating_sub(ETHERNET_HDR_LEN_NO_TAG_U32))
    }

    /// Creates the maximum ethernet frame size from MTU.
    pub const fn from_mtu(mtu: Mtu) -> Option<MaxEthernetFrameSize> {
        let frame_size = mtu.get().saturating_add(ETHERNET_HDR_LEN_NO_TAG_U32);
        Self::new(frame_size)
    }
}

impl From<MaxEthernetFrameSize> for usize {
    fn from(MaxEthernetFrameSize(v): MaxEthernetFrameSize) -> Self {
        v.get().try_into().expect("u32 doesn't fit in usize")
    }
}

/// Base properties to create a new Ethernet device.
#[derive(Debug)]
pub struct EthernetCreationProperties {
    /// The device's MAC address.
    pub mac: UnicastAddr<Mac>,
    /// The maximum frame size this device supports.
    // TODO(https://fxbug.dev/42072516): Add a minimum frame size for all
    // Ethernet devices such that you can't create an `EthernetDeviceState`
    // with a `MaxEthernetFrameSize` smaller than the minimum. The absolute minimum
    // needs to be at least the minimum body size of an Ethernet frame. For
    // IPv6-capable devices, the minimum needs to be higher - the frame size
    // implied by the IPv6 minimum MTU. The easy path is to simply use that
    // frame size as the minimum in all cases, although we may at some point
    // want to figure out how to configure devices which don't support IPv6,
    // and allow smaller frame sizes for those devices.
    pub max_frame_size: MaxEthernetFrameSize,
    /// The checksum offload capabilities of the device.
    pub tx_offload_spec: netstack3_base::ChecksumOffloadSpec,
}

/// Ethernet device state that can change at runtime.
pub struct DynamicEthernetDeviceState {
    /// The value this netstack assumes as the device's maximum frame size.
    max_frame_size: MaxEthernetFrameSize,

    /// Link multicast groups this device has joined.
    link_multicast_groups: RefCountedHashSet<MulticastAddr<Mac>>,
}

impl DynamicEthernetDeviceState {
    fn new(max_frame_size: MaxEthernetFrameSize) -> Self {
        Self { max_frame_size, link_multicast_groups: Default::default() }
    }
}

/// Ethernet device state that is fixed after creation.
pub struct StaticEthernetDeviceState {
    /// Mac address of the device this state is for.
    mac: UnicastAddr<Mac>,

    /// The maximum frame size allowed by the hardware.
    max_frame_size: MaxEthernetFrameSize,
}

/// The state associated with an Ethernet device.
pub struct EthernetDeviceState<BT: DeviceLayerTypes> {
    /// Ethernet device counters.
    pub counters: EthernetDeviceCounters,
    /// Immutable Ethernet device state.
    pub static_state: StaticEthernetDeviceState,
    /// Ethernet device transmit queue.
    pub tx_queue: TransmitQueue<
        BT::TxMetadata,
        <EthernetLinkDevice as DeviceBufferSpec<BT>>::TxBuffer,
        <EthernetLinkDevice as DeviceBufferSpec<BT>>::TxAllocator,
    >,
    ipv4_arp: Mutex<ArpState<EthernetLinkDevice, BT>>,
    ipv6_nud: Mutex<NudState<Ipv6, EthernetLinkDevice, BT>>,
    ipv4_nud_config: RwLock<IpMarked<Ipv4, NudUserConfig>>,
    ipv6_nud_config: RwLock<IpMarked<Ipv6, NudUserConfig>>,
    dynamic_state: RwLock<DynamicEthernetDeviceState>,
}

impl<BT: DeviceLayerTypes, I: Ip> OrderedLockAccess<IpMarked<I, NudUserConfig>>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = RwLock<IpMarked<I, NudUserConfig>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(I::map_ip(
            (),
            |()| &self.link.ipv4_nud_config,
            |()| &self.link.ipv6_nud_config,
        ))
    }
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<DynamicEthernetDeviceState>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = RwLock<DynamicEthernetDeviceState>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.dynamic_state)
    }
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<NudState<Ipv6, EthernetLinkDevice, BT>>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = Mutex<NudState<Ipv6, EthernetLinkDevice, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.ipv6_nud)
    }
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<ArpState<EthernetLinkDevice, BT>>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = Mutex<ArpState<EthernetLinkDevice, BT>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.ipv4_arp)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<TransmitQueueState<BT::TxMetadata, BT::TxBuffer, BT::TxAllocator>>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = Mutex<TransmitQueueState<BT::TxMetadata, BT::TxBuffer, BT::TxAllocator>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.queue)
    }
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<DequeueState<BT::TxMetadata, BT::TxBuffer>>
    for IpLinkDeviceState<EthernetLinkDevice, BT>
{
    type Lock = Mutex<DequeueState<BT::TxMetadata, BT::TxBuffer>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.deque)
    }
}

/// A timer ID for Ethernet devices.
///
/// `D` is the type of device ID that identifies different Ethernet devices.
#[derive(Clone, Eq, PartialEq, Debug, Hash, GenericOverIp)]
#[generic_over_ip()]
#[allow(missing_docs)]
pub enum EthernetTimerId<D: WeakDeviceIdentifier> {
    Arp(ArpTimerId<EthernetLinkDevice, D>),
    Nudv6(NudTimerId<Ipv6, EthernetLinkDevice, D>),
}

impl<I: Ip, D: WeakDeviceIdentifier> From<NudTimerId<I, EthernetLinkDevice, D>>
    for EthernetTimerId<D>
{
    fn from(id: NudTimerId<I, EthernetLinkDevice, D>) -> EthernetTimerId<D> {
        I::map_ip(id, EthernetTimerId::Arp, EthernetTimerId::Nudv6)
    }
}

impl<CC, BC> HandleableTimer<CC, BC> for EthernetTimerId<CC::WeakDeviceId>
where
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + TimerHandler<BC, NudTimerId<Ipv6, EthernetLinkDevice, CC::WeakDeviceId>>
        + TimerHandler<BC, ArpTimerId<EthernetLinkDevice, CC::WeakDeviceId>>,
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId) {
        match self {
            EthernetTimerId::Arp(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
            EthernetTimerId::Nudv6(id) => core_ctx.handle_timer(bindings_ctx, id, timer),
        }
    }
}

/// Send an IP packet in an Ethernet frame.
///
/// `send_ip_frame` accepts a device ID, a local IP address, and a
/// serializer. It computes the routing information, serializes
/// the serializer, and sends the resulting buffer in a new Ethernet
/// frame.
pub fn send_ip_frame<BC, CC, I, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    destination: IpPacketDestination<I, &DeviceId<BC>>,
    body: S,
    meta: BC::TxMetadata,
) -> Result<(), SendFrameError<S>>
where
    BC: EthernetIpLinkDeviceBindingsContext + LinkResolutionContext<EthernetLinkDevice>,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + NudHandler<I, EthernetLinkDevice, BC>
        + TransmitQueueHandler<EthernetLinkDevice, BC, Meta = BC::TxMetadata>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
    I: EthernetIpExt + BroadcastIpExt,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
{
    core_ctx.increment_both(device_id, DeviceCounters::send_frame::<I>);

    trace!("ethernet::send_ip_frame: destination = {:?}; device = {:?}", destination, device_id);

    match destination {
        IpPacketDestination::Broadcast(marker) => {
            I::map_ip::<_, ()>(
                WrapBroadcastMarker(marker),
                |WrapBroadcastMarker(())| (),
                |WrapBroadcastMarker(never)| match never {},
            );
            send_as_ethernet_frame_to_dst(
                core_ctx,
                bindings_ctx,
                device_id,
                Mac::BROADCAST,
                body,
                I::ETHER_TYPE,
                meta,
            )
        }
        IpPacketDestination::Multicast(multicast_ip) => send_as_ethernet_frame_to_dst(
            core_ctx,
            bindings_ctx,
            device_id,
            Mac::from(&multicast_ip),
            body,
            I::ETHER_TYPE,
            meta,
        ),
        IpPacketDestination::Neighbor(ip) => NudHandler::<I, _, _>::send_ip_packet_to_neighbor(
            core_ctx,
            bindings_ctx,
            device_id,
            ip,
            body,
            meta,
        ),
        IpPacketDestination::Loopback(_) => {
            unreachable!("Loopback packets must be delivered through the loopback device")
        }
    }
}

/// Metadata for received ethernet frames.
pub struct RecvEthernetFrameMeta<D> {
    /// The device a frame was received on.
    pub device_id: D,
    /// The parsing context for the received frame.
    pub parsing_context: NetworkParsingContext,
}

impl DeviceReceiveFrameSpec for EthernetLinkDevice {
    type FrameMetadata<D> = RecvEthernetFrameMeta<D>;
}

impl<CC, BC> ReceivableFrameMeta<CC, BC> for RecvEthernetFrameMeta<CC::DeviceId>
where
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + RecvFrameContext<RecvIpFrameMeta<CC::DeviceId, DeviceIpLayerMetadata<BC>, Ipv4>, BC>
        + RecvFrameContext<RecvIpFrameMeta<CC::DeviceId, DeviceIpLayerMetadata<BC>, Ipv6>, BC>
        + ArpPacketHandler<EthernetLinkDevice, BC>
        + DeviceSocketHandler<EthernetLinkDevice, BC>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>
        + ResourceCounterContext<CC::DeviceId, EthernetDeviceCounters>
        + ResourceCounterContext<CC::DeviceId, IpCounters<Ipv4>>
        + ResourceCounterContext<CC::DeviceId, IpCounters<Ipv6>>,
{
    fn receive_meta<B: BufferMut + Debug>(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        mut buffer: B,
    ) {
        trace_duration!("device::ethernet::receive_frame");
        let Self { device_id, parsing_context } = self;
        trace!("ethernet::receive_frame: device_id = {:?}", device_id);
        core_ctx.increment_both(&device_id, |counters: &DeviceCounters| &counters.recv_frame);
        core_ctx.add_both_usize(&device_id, buffer.len(), |counters: &DeviceCounters| {
            &counters.recv_bytes
        });
        // NOTE(joshlf): We do not currently validate that the Ethernet frame
        // satisfies the minimum length requirement. We expect that if this
        // requirement is necessary (due to requirements of the physical medium),
        // the driver or hardware will have checked it, and that if this requirement
        // is not necessary, it is acceptable for us to operate on a smaller
        // Ethernet frame. If this becomes insufficient in the future, we may want
        // to consider making this behavior configurable (at compile time, at
        // runtime on a global basis, or at runtime on a per-device basis).
        let (ethernet, whole_frame) = if let Ok(frame) =
            buffer.parse_with_view::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::NoCheck)
        {
            frame
        } else {
            core_ctx
                .increment_both(&device_id, |counters: &DeviceCounters| &counters.recv_parse_error);
            trace!("ethernet::receive_frame: failed to parse ethernet frame");
            return;
        };

        let src = ethernet.src_mac();
        let dst = ethernet.dst_mac();

        let frame_dst = core_ctx.with_static_ethernet_device_state(&device_id, |static_state| {
            FrameDestination::from_dest(dst, static_state.mac.get())
        });

        let ethertype = ethernet.ethertype();

        core_ctx.handle_frame(
            bindings_ctx,
            &device_id,
            ReceivedFrame::from_ethernet(ethernet, frame_dst).into(),
            whole_frame,
        );

        match ethertype {
            Some(EtherType::Arp) => {
                let types = if let Ok(types) = peek_arp_types(buffer.as_ref()) {
                    types
                } else {
                    return;
                };
                match types {
                    (ArpHardwareType::Ethernet, ArpNetworkType::Ipv4) => {
                        ArpPacketHandler::handle_packet(
                            core_ctx,
                            bindings_ctx,
                            device_id,
                            src,
                            frame_dst,
                            buffer,
                        )
                    }
                }
            }
            Some(EtherType::Ipv4) => {
                let local_frame_dst = match frame_dst.check_local() {
                    Some(dst) => dst,
                    None => {
                        core_ctx.increment_both(&device_id, |counters: &IpCounters<Ipv4>| {
                            &counters.drop_ip_packet_other_host
                        });
                        return;
                    }
                };
                core_ctx.increment_both(&device_id, |counters: &DeviceCounters| {
                    &counters.recv_ipv4_delivered
                });
                core_ctx.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv4>::new(
                        device_id,
                        Some(local_frame_dst),
                        DeviceIpLayerMetadata::default(),
                        parsing_context,
                    ),
                    buffer,
                )
            }
            Some(EtherType::Ipv6) => {
                let local_frame_dst = match frame_dst.check_local() {
                    Some(dst) => dst,
                    None => {
                        core_ctx.increment_both(&device_id, |counters: &IpCounters<Ipv6>| {
                            &counters.drop_ip_packet_other_host
                        });
                        return;
                    }
                };
                core_ctx.increment_both(&device_id, |counters: &DeviceCounters| {
                    &counters.recv_ipv6_delivered
                });
                core_ctx.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv6>::new(
                        device_id,
                        Some(local_frame_dst),
                        DeviceIpLayerMetadata::default(),
                        parsing_context,
                    ),
                    buffer,
                )
            }
            Some(EtherType::Other(_)) => {
                core_ctx.increment_both(&device_id, |counters: &EthernetDeviceCounters| {
                    &counters.recv_unsupported_ethertype
                });
            }
            None => {
                core_ctx.increment_both(&device_id, |counters: &EthernetDeviceCounters| {
                    &counters.recv_no_ethertype
                });
            }
        }
    }
}

/// Add `device_id` to a link multicast group `multicast_addr`.
///
/// Calling `join_link_multicast` with the same `device_id` and `multicast_addr`
/// is completely safe. A counter will be kept for the number of times
/// `join_link_multicast` has been called with the same `device_id` and
/// `multicast_addr` pair. To completely leave a multicast group,
/// [`leave_link_multicast`] must be called the same number of times
/// `join_link_multicast` has been called for the same `device_id` and
/// `multicast_addr` pair. The first time `join_link_multicast` is called for a
/// new `device` and `multicast_addr` pair, the device will actually join the
/// multicast group.
///
/// `join_link_multicast` is different from [`join_ip_multicast`] as
/// `join_link_multicast` joins an L2 multicast group, whereas
/// `join_ip_multicast` joins an L3 multicast group.
pub fn join_link_multicast<
    BC: EthernetIpLinkDeviceBindingsContext + EthernetDeviceEventBindingsContext<CC::DeviceId>,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<Mac>,
) {
    core_ctx.with_ethernet_state_mut(device_id, |_static_state, dynamic_state| {
        let groups = &mut dynamic_state.link_multicast_groups;

        match groups.insert(multicast_addr) {
            InsertResult::Inserted(()) => {
                trace!(
                    "ethernet::join_link_multicast: joining link multicast {:?}",
                    multicast_addr
                );
                bindings_ctx.on_event(EthernetDeviceEvent::MulticastJoin {
                    device: device_id.clone(),
                    addr: multicast_addr,
                });
            }
            InsertResult::AlreadyPresent => {
                trace!(
                    "ethernet::join_link_multicast: already joined link multicast {:?}",
                    multicast_addr,
                );
            }
        }
    })
}

/// Remove `device_id` from a link multicast group `multicast_addr`.
///
/// `leave_link_multicast` will attempt to remove `device_id` from the multicast
/// group `multicast_addr`. `device_id` may have "joined" the same multicast
/// address multiple times, so `device_id` will only leave the multicast group
/// once `leave_ip_multicast` has been called for each corresponding
/// [`join_link_multicast`]. That is, if `join_link_multicast` gets called 3
/// times and `leave_link_multicast` gets called two times (after all 3
/// `join_link_multicast` calls), `device_id` will still be in the multicast
/// group until the next (final) call to `leave_link_multicast`.
///
/// `leave_link_multicast` is different from [`leave_ip_multicast`] as
/// `leave_link_multicast` leaves an L2 multicast group, whereas
/// `leave_ip_multicast` leaves an L3 multicast group.
///
/// # Panics
///
/// If `device_id` is not in the multicast group `multicast_addr`.
pub fn leave_link_multicast<
    BC: EthernetIpLinkDeviceBindingsContext + EthernetDeviceEventBindingsContext<CC::DeviceId>,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    multicast_addr: MulticastAddr<Mac>,
) {
    core_ctx.with_ethernet_state_mut(device_id, |_static_state, dynamic_state| {
        let groups = &mut dynamic_state.link_multicast_groups;

        match groups.remove(multicast_addr) {
            RemoveResult::Removed(()) => {
                trace!(
                    "ethernet::leave_link_multicast: \
                    leaving link multicast {:?}",
                    multicast_addr
                );
                bindings_ctx.on_event(EthernetDeviceEvent::MulticastLeave {
                    device: device_id.clone(),
                    addr: multicast_addr,
                });
            }
            RemoveResult::StillPresent => {
                trace!(
                    "ethernet::leave_link_multicast: not leaving link multicast \
                    {:?} as there are still listeners for it",
                    multicast_addr,
                );
            }
            RemoveResult::NotPresent => {
                panic!(
                    "ethernet::leave_link_multicast: device {:?} has not yet \
                    joined link multicast {:?}",
                    device_id, multicast_addr,
                );
            }
        }
    })
}

pub fn get_max_frame_size<
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> MaxEthernetFrameSize {
    core_ctx
        .with_ethernet_state(device_id, |_static_state, dynamic_state| dynamic_state.max_frame_size)
}

/// Get the MTU associated with this device.
pub fn get_mtu<
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
) -> Mtu {
    get_max_frame_size(core_ctx, device_id).as_mtu()
}

/// Enables a blanket implementation of [`SendableFrameData`] for
/// [`ArpFrameMetadata`].
///
/// Implementing this marker trait for a type enables a blanket implementation
/// of `SendableFrameData` given the other requirements are met.
pub trait UseArpFrameMetadataBlanket {}

impl<
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + TransmitQueueHandler<EthernetLinkDevice, BC, Meta = BC::TxMetadata>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>
        + UseArpFrameMetadataBlanket,
> SendableFrameMeta<CC, BC> for ArpFrameMetadata<EthernetLinkDevice, CC::DeviceId>
{
    fn send_meta<S>(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        body: S,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let Self { device_id, dst_addr } = self;
        let meta: BC::TxMetadata = Default::default();
        send_as_ethernet_frame_to_dst(
            core_ctx,
            bindings_ctx,
            &device_id,
            dst_addr.get(),
            body,
            EtherType::Arp,
            meta,
        )
    }
}

impl DeviceSocketSendTypes for EthernetLinkDevice {
    /// When `None`, data will be sent as a raw Ethernet frame without any
    /// system-applied headers.
    type Metadata = Option<EthernetHeaderParams>;
}

impl<
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>
        + TransmitQueueHandler<EthernetLinkDevice, BC, Meta = BC::TxMetadata>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
> SendableFrameMeta<CC, BC> for DeviceSocketMetadata<EthernetLinkDevice, EthernetDeviceId<BC>>
where
    CC: DeviceIdContext<EthernetLinkDevice, DeviceId = EthernetDeviceId<BC>>,
{
    fn send_meta<S>(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        body: S,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let Self { device_id, metadata } = self;
        // TODO(https://fxbug.dev/391946195): Apply send buffer enforcement from
        // device sockets instead of using default.
        let tx_meta: BC::TxMetadata = Default::default();
        match metadata {
            Some(EthernetHeaderParams { dest_addr, protocol }) => send_as_ethernet_frame_to_dst(
                core_ctx,
                bindings_ctx,
                &device_id,
                dest_addr,
                body,
                protocol,
                tx_meta,
            ),
            None => send_ethernet_frame(core_ctx, bindings_ctx, &device_id, body, tx_meta),
        }
    }
}

/// Gets `device_id`'s MAC address.
pub fn get_mac<
    'a,
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &'a mut CC,
    device_id: &CC::DeviceId,
) -> UnicastAddr<Mac> {
    core_ctx.with_static_ethernet_device_state(device_id, |state| state.mac)
}

/// Sets `device_id`'s MTU.
pub fn set_mtu<
    BC: EthernetIpLinkDeviceBindingsContext,
    CC: EthernetIpLinkDeviceDynamicStateContext<BC>,
>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
    mtu: Mtu,
) {
    core_ctx.with_ethernet_state_mut(device_id, |static_state, dynamic_state| {
        if let Some(mut frame_size ) = MaxEthernetFrameSize::from_mtu(mtu) {
            // If `frame_size` is greater than what the device supports, set it
            // to maximum frame size the device supports.
            if frame_size > static_state.max_frame_size {
                trace!("ethernet::ndp_device::set_mtu: MTU of {:?} is greater than the device {:?}'s max MTU of {:?}, using device's max MTU instead", mtu, device_id, static_state.max_frame_size.as_mtu());
                frame_size = static_state.max_frame_size;
            }
            trace!("ethernet::ndp_device::set_mtu: setting link MTU to {:?}", mtu);
            dynamic_state.max_frame_size = frame_size;
        }
    })
}

/// An implementation of the [`LinkDevice`] trait for Ethernet devices.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum EthernetLinkDevice {}

impl Device for EthernetLinkDevice {}

impl LinkDevice for EthernetLinkDevice {
    type Address = Mac;
}

impl<BT: DeviceBufferBindingsTypes> DeviceBufferSpec<BT> for EthernetLinkDevice {
    type TxBuffer = BT::TxBuffer;
    type TxAllocator = BT::TxAllocator;
}

impl DeviceStateSpec for EthernetLinkDevice {
    type State<BT: DeviceLayerTypes> = EthernetDeviceState<BT>;
    type External<BT: DeviceLayerTypes> = BT::EthernetDeviceState;
    type CreationProperties = EthernetCreationProperties;
    type Counters = EthernetDeviceCounters;
    type TimerId<D: WeakDeviceIdentifier> = EthernetTimerId<D>;

    fn new_device_state<
        CC: CoreTimerContext<Self::TimerId<CC::WeakDeviceId>, BC> + DeviceIdContext<Self>,
        BC: DeviceLayerTypes + TimerContext,
    >(
        bindings_ctx: &mut BC,
        self_id: CC::WeakDeviceId,
        EthernetCreationProperties { mac, max_frame_size, tx_offload_spec }: Self::CreationProperties,
        tx_allocator: <Self as DeviceBufferSpec<BC>>::TxAllocator,
    ) -> Self::State<BC>
    where
        Self: DeviceBufferSpec<BC>,
    {
        let ipv4_arp = Mutex::new(ArpState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(
            bindings_ctx,
            self_id.clone(),
        ));
        let ipv6_nud =
            Mutex::new(NudState::new::<_, NestedIntoCoreTimerCtx<CC, _>>(bindings_ctx, self_id));
        EthernetDeviceState {
            counters: Default::default(),
            ipv4_arp,
            ipv6_nud,
            ipv4_nud_config: Default::default(),
            ipv6_nud_config: Default::default(),
            static_state: StaticEthernetDeviceState { mac, max_frame_size },
            dynamic_state: RwLock::new(DynamicEthernetDeviceState::new(max_frame_size)),
            tx_queue: TransmitQueue::new(tx_allocator, tx_offload_spec),
        }
    }
    const IS_LOOPBACK: bool = false;
    const DEBUG_TYPE: &'static str = "Ethernet";

    fn tx_offload_spec<BT: DeviceLayerTypes>(
        state: &Self::State<BT>,
    ) -> Option<ChecksumOffloadSpec> {
        Some(state.tx_queue.tx_offload_spec())
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    /// The mimum implied maximum Ethernet frame size for IPv6.
    pub const IPV6_MIN_IMPLIED_MAX_FRAME_SIZE: MaxEthernetFrameSize =
        MaxEthernetFrameSize::from_mtu(Ipv6::MINIMUM_LINK_MTU).unwrap();
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::convert::Infallible as Never;
    use netstack3_hashmap::HashSet;

    use net_types::SpecifiedAddr;
    use net_types::ip::{Ipv4Addr, Ipv6Addr};
    use netstack3_base::testutil::{
        FakeDeviceId, FakeInstant, FakeTxMetadata, FakeWeakDeviceId, TEST_ADDRS_V4,
    };
    use netstack3_base::{CounterContext, CtxPair, IntoCoreTimerCtx};
    use netstack3_ip::nud::{
        self, DelegateNudContext, DynamicNeighborUpdateSource, NeighborApi, UseDelegateNudContext,
    };
    use packet::Buf;
    use packet_formats::testutil::parse_ethernet_frame;

    use super::*;
    use crate::internal::arp::{
        ArpConfigContext, ArpContext, ArpCounters, ArpNudCtx, ArpSenderContext,
    };
    use crate::internal::base::{DeviceBufferBindingsTypes, DeviceSendFrameError};
    use crate::internal::ethernet::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
    use crate::internal::queue::tx::{
        BufVecU8Allocator, TransmitQueueBindingsContext, TransmitQueueCommon, TransmitQueueContext,
    };
    use crate::internal::socket::{Frame, ParseSentFrameError, SentFrame};

    struct FakeEthernetCtx {
        static_state: StaticEthernetDeviceState,
        dynamic_state: DynamicEthernetDeviceState,
        tx_queue: TransmitQueueState<FakeTxMetadata, Buf<Vec<u8>>, BufVecU8Allocator>,
        counters: DeviceCounters,
        per_device_counters: DeviceCounters,
        ethernet_counters: EthernetDeviceCounters,
        arp_counters: ArpCounters,
    }

    impl FakeEthernetCtx {
        fn new(mac: UnicastAddr<Mac>, max_frame_size: MaxEthernetFrameSize) -> FakeEthernetCtx {
            FakeEthernetCtx {
                static_state: StaticEthernetDeviceState { max_frame_size, mac },
                dynamic_state: DynamicEthernetDeviceState::new(max_frame_size),
                tx_queue: Default::default(),
                counters: Default::default(),
                per_device_counters: Default::default(),
                ethernet_counters: Default::default(),
                arp_counters: Default::default(),
            }
        }
    }

    type FakeBindingsCtx = netstack3_base::testutil::FakeBindingsCtx<
        EthernetTimerId<FakeWeakDeviceId<FakeDeviceId>>,
        nud::Event<Mac, FakeDeviceId, Ipv4, FakeInstant>,
        FakeBindingsState,
        (),
    >;

    #[derive(Default)]
    struct FakeBindingsState {
        link_multicast_group_memberships: HashSet<(FakeDeviceId, MulticastAddr<Mac>)>,
    }

    type FakeInnerCtx =
        netstack3_base::testutil::FakeCoreCtx<FakeEthernetCtx, FakeDeviceId, FakeDeviceId>;

    struct FakeCoreCtx {
        arp_state: ArpState<EthernetLinkDevice, FakeBindingsCtx>,
        inner: FakeInnerCtx,
    }

    fn new_context() -> CtxPair<FakeCoreCtx, FakeBindingsCtx> {
        CtxPair::with_default_bindings_ctx(|bindings_ctx| FakeCoreCtx {
            arp_state: ArpState::new::<_, IntoCoreTimerCtx>(
                bindings_ctx,
                FakeWeakDeviceId(FakeDeviceId),
            ),
            inner: FakeInnerCtx::with_state(FakeEthernetCtx::new(
                TEST_ADDRS_V4.local_mac,
                IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            )),
        })
    }

    impl DeviceSocketHandler<EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtx {
        fn handle_frame(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtx,
            device: &Self::DeviceId,
            frame: Frame<&[u8]>,
            whole_frame: &[u8],
        ) {
            self.inner.handle_frame(bindings_ctx, device, frame, whole_frame)
        }
    }

    impl CounterContext<DeviceCounters> for FakeCoreCtx {
        fn counters(&self) -> &DeviceCounters {
            &self.inner.state.counters
        }
    }

    impl CounterContext<DeviceCounters> for FakeInnerCtx {
        fn counters(&self) -> &DeviceCounters {
            &self.state.counters
        }
    }

    impl ResourceCounterContext<FakeDeviceId, DeviceCounters> for FakeCoreCtx {
        fn per_resource_counters<'a>(
            &'a self,
            &FakeDeviceId: &'a FakeDeviceId,
        ) -> &'a DeviceCounters {
            &self.inner.state.per_device_counters
        }
    }

    impl ResourceCounterContext<FakeDeviceId, DeviceCounters> for FakeInnerCtx {
        fn per_resource_counters<'a>(
            &'a self,
            &FakeDeviceId: &'a FakeDeviceId,
        ) -> &'a DeviceCounters {
            &self.state.per_device_counters
        }
    }

    impl CounterContext<EthernetDeviceCounters> for FakeCoreCtx {
        fn counters(&self) -> &EthernetDeviceCounters {
            &self.inner.state.ethernet_counters
        }
    }

    impl CounterContext<EthernetDeviceCounters> for FakeInnerCtx {
        fn counters(&self) -> &EthernetDeviceCounters {
            &self.state.ethernet_counters
        }
    }

    impl DeviceSocketHandler<EthernetLinkDevice, FakeBindingsCtx> for FakeInnerCtx {
        fn handle_frame(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            _device: &Self::DeviceId,
            _frame: Frame<&[u8]>,
            _whole_frame: &[u8],
        ) {
            // No-op: don't deliver frames.
        }
    }

    impl EthernetIpLinkDeviceStaticStateContext for FakeCoreCtx {
        fn with_static_ethernet_device_state<O, F: FnOnce(&StaticEthernetDeviceState) -> O>(
            &mut self,
            device_id: &FakeDeviceId,
            cb: F,
        ) -> O {
            self.inner.with_static_ethernet_device_state(device_id, cb)
        }
    }

    impl EthernetIpLinkDeviceStaticStateContext for FakeInnerCtx {
        fn with_static_ethernet_device_state<O, F: FnOnce(&StaticEthernetDeviceState) -> O>(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            cb(&self.state.static_state)
        }
    }

    impl EthernetIpLinkDeviceDynamicStateContext<FakeBindingsCtx> for FakeCoreCtx {
        fn with_ethernet_state<
            O,
            F: FnOnce(&StaticEthernetDeviceState, &DynamicEthernetDeviceState) -> O,
        >(
            &mut self,
            device_id: &FakeDeviceId,
            cb: F,
        ) -> O {
            self.inner.with_ethernet_state(device_id, cb)
        }

        fn with_ethernet_state_mut<
            O,
            F: FnOnce(&StaticEthernetDeviceState, &mut DynamicEthernetDeviceState) -> O,
        >(
            &mut self,
            device_id: &FakeDeviceId,
            cb: F,
        ) -> O {
            self.inner.with_ethernet_state_mut(device_id, cb)
        }
    }

    impl EthernetIpLinkDeviceDynamicStateContext<FakeBindingsCtx> for FakeInnerCtx {
        fn with_ethernet_state<
            O,
            F: FnOnce(&StaticEthernetDeviceState, &DynamicEthernetDeviceState) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let FakeEthernetCtx { static_state, dynamic_state, .. } = &self.state;
            cb(static_state, dynamic_state)
        }

        fn with_ethernet_state_mut<
            O,
            F: FnOnce(&StaticEthernetDeviceState, &mut DynamicEthernetDeviceState) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let FakeEthernetCtx { static_state, dynamic_state, .. } = &mut self.state;
            cb(static_state, dynamic_state)
        }
    }

    impl NudHandler<Ipv6, EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtx {
        fn handle_neighbor_update(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            _device_id: &Self::DeviceId,
            _neighbor: SpecifiedAddr<Ipv6Addr>,
            _source: DynamicNeighborUpdateSource<Mac>,
        ) {
            unimplemented!()
        }

        fn flush(&mut self, _bindings_ctx: &mut FakeBindingsCtx, _device_id: &Self::DeviceId) {
            unimplemented!()
        }

        fn send_ip_packet_to_neighbor<S>(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            _device_id: &Self::DeviceId,
            _neighbor: SpecifiedAddr<Ipv6Addr>,
            _body: S,
            _tx_meta: FakeTxMetadata,
        ) -> Result<(), SendFrameError<S>> {
            unimplemented!()
        }
    }

    struct FakeCoreCtxWithDeviceId<'a> {
        core_ctx: &'a mut FakeInnerCtx,
        device_id: &'a FakeDeviceId,
    }

    impl<'a> DeviceIdContext<EthernetLinkDevice> for FakeCoreCtxWithDeviceId<'a> {
        type DeviceId = FakeDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
    }

    impl<'a> ArpConfigContext for FakeCoreCtxWithDeviceId<'a> {
        fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
            cb(&NudUserConfig::default())
        }
    }

    impl UseArpFrameMetadataBlanket for FakeCoreCtx {}

    impl ArpContext<EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtx {
        type ConfigCtx<'a> = FakeCoreCtxWithDeviceId<'a>;

        type ArpSenderCtx<'a> = FakeCoreCtxWithDeviceId<'a>;

        fn with_arp_state_mut_and_sender_ctx<
            O,
            F: FnOnce(
                &mut ArpState<EthernetLinkDevice, FakeBindingsCtx>,
                &mut Self::ArpSenderCtx<'_>,
            ) -> O,
        >(
            &mut self,
            device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            let Self { arp_state, inner } = self;
            cb(arp_state, &mut FakeCoreCtxWithDeviceId { core_ctx: inner, device_id })
        }

        fn get_protocol_addr(&mut self, _device_id: &Self::DeviceId) -> Option<Ipv4Addr> {
            unimplemented!()
        }

        fn get_hardware_addr(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            _device_id: &Self::DeviceId,
        ) -> UnicastAddr<Mac> {
            self.inner.state.static_state.mac
        }

        fn with_arp_state_mut<
            O,
            F: FnOnce(
                &mut ArpState<EthernetLinkDevice, FakeBindingsCtx>,
                &mut Self::ConfigCtx<'_>,
            ) -> O,
        >(
            &mut self,
            device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            let Self { arp_state, inner } = self;
            cb(arp_state, &mut FakeCoreCtxWithDeviceId { core_ctx: inner, device_id })
        }

        fn with_arp_state<O, F: FnOnce(&ArpState<EthernetLinkDevice, FakeBindingsCtx>) -> O>(
            &mut self,
            FakeDeviceId: &Self::DeviceId,
            cb: F,
        ) -> O {
            cb(&mut self.arp_state)
        }
    }

    impl UseDelegateNudContext for FakeCoreCtx {}
    impl DelegateNudContext<Ipv4> for FakeCoreCtx {
        type Delegate<T> = ArpNudCtx<T>;
    }

    impl ArpConfigContext for FakeInnerCtx {
        fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
            cb(&NudUserConfig::default())
        }
    }

    impl<'a> ArpSenderContext<EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtxWithDeviceId<'a> {
        fn send_ip_packet_to_neighbor_link_addr<S>(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtx,
            link_addr: UnicastAddr<Mac>,
            body: S,
            tx_meta: FakeTxMetadata,
        ) -> Result<(), SendFrameError<S>>
        where
            S: NetworkSerializer,
            S::Buffer: BufferMut,
        {
            let Self { core_ctx, device_id } = self;
            send_as_ethernet_frame_to_dst(
                *core_ctx,
                bindings_ctx,
                device_id,
                link_addr.get(),
                body,
                EtherType::Ipv4,
                tx_meta,
            )
        }
    }

    impl TransmitQueueBindingsContext<FakeDeviceId> for FakeBindingsCtx {
        fn wake_tx_task(&mut self, FakeDeviceId: &FakeDeviceId) {
            unimplemented!("unused by tests")
        }
    }

    impl EventContext<EthernetDeviceEvent<FakeDeviceId>> for FakeBindingsCtx {
        fn on_event(&mut self, event: EthernetDeviceEvent<FakeDeviceId>) {
            // Panic if we get more than one join or leave event per group.
            match event {
                EthernetDeviceEvent::MulticastJoin { device, addr } => {
                    assert!(
                        self.state.link_multicast_group_memberships.insert((device, addr)),
                        "membership should not be present"
                    );
                }
                EthernetDeviceEvent::MulticastLeave { device, addr } => {
                    assert!(
                        self.state.link_multicast_group_memberships.remove(&(device, addr)),
                        "membership should be present"
                    );
                }
            }
        }
    }

    impl TransmitQueueCommon<EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtx {
        type Meta = FakeTxMetadata;

        type DequeueContext = Never;

        fn parse_outgoing_frame<'a>(
            buf: &'a [u8],
            meta: &'a Self::Meta,
        ) -> Result<SentFrame<&'a [u8]>, ParseSentFrameError> {
            FakeInnerCtx::parse_outgoing_frame(buf, meta)
        }
    }

    impl TransmitQueueCommon<EthernetLinkDevice, FakeBindingsCtx> for FakeInnerCtx {
        type Meta = FakeTxMetadata;

        type DequeueContext = Never;

        fn parse_outgoing_frame<'a, 'b>(
            buf: &'a [u8],
            _tx_meta: &'b Self::Meta,
        ) -> Result<SentFrame<&'a [u8]>, ParseSentFrameError> {
            SentFrame::try_parse_as_ethernet(buf)
        }
    }

    impl TransmitQueueContext<EthernetLinkDevice, FakeBindingsCtx> for FakeCoreCtx {
        fn with_transmit_queue_mut<
            O,
            F: FnOnce(
                &mut TransmitQueueState<Self::Meta, packet::Buf<Vec<u8>>, BufVecU8Allocator>,
            ) -> O,
        >(
            &mut self,
            device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            self.inner.with_transmit_queue_mut(device_id, cb)
        }

        fn with_transmit_queue<
            O,
            F: FnOnce(&TransmitQueueState<Self::Meta, packet::Buf<Vec<u8>>, BufVecU8Allocator>) -> O,
        >(
            &mut self,
            device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            self.inner.with_transmit_queue(device_id, cb)
        }

        fn send_frame(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtx,
            device_id: &Self::DeviceId,
            dequeue_context: Option<&mut Never>,
            tx_meta: Self::Meta,
            buf: packet::Buf<Vec<u8>>,
        ) -> Result<(), DeviceSendFrameError> {
            TransmitQueueContext::send_frame(
                &mut self.inner,
                bindings_ctx,
                device_id,
                dequeue_context,
                tx_meta,
                buf,
            )
        }
    }

    impl TransmitQueueContext<EthernetLinkDevice, FakeBindingsCtx> for FakeInnerCtx
    where
        Self: TransmitQueueCommon<EthernetLinkDevice, FakeBindingsCtx, Meta = FakeTxMetadata>,
    {
        fn with_transmit_queue_mut<
            O,
            F: FnOnce(
                &mut TransmitQueueState<
                    Self::Meta,
                    <FakeBindingsCtx as DeviceBufferBindingsTypes>::TxBuffer,
                    <FakeBindingsCtx as DeviceBufferBindingsTypes>::TxAllocator,
                >,
            ) -> O,
        >(
            &mut self,
            _device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            cb(&mut self.state.tx_queue)
        }

        fn with_transmit_queue<
            O,
            F: FnOnce(
                &TransmitQueueState<
                    Self::Meta,
                    <FakeBindingsCtx as DeviceBufferBindingsTypes>::TxBuffer,
                    <FakeBindingsCtx as DeviceBufferBindingsTypes>::TxAllocator,
                >,
            ) -> O,
        >(
            &mut self,
            _device_id: &Self::DeviceId,
            cb: F,
        ) -> O {
            cb(&self.state.tx_queue)
        }

        fn send_frame(
            &mut self,
            _bindings_ctx: &mut FakeBindingsCtx,
            device_id: &Self::DeviceId,
            dequeue_context: Option<&mut Never>,
            _tx_meta: Self::Meta,
            buf: packet::Buf<Vec<u8>>,
        ) -> Result<(), DeviceSendFrameError> {
            match dequeue_context {
                Some(never) => match *never {},
                None => (),
            }
            self.frames.push(device_id.clone(), buf.as_ref().to_vec());
            Ok(())
        }
    }

    impl DeviceIdContext<EthernetLinkDevice> for FakeCoreCtx {
        type DeviceId = FakeDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
    }

    impl DeviceIdContext<EthernetLinkDevice> for FakeInnerCtx {
        type DeviceId = FakeDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
    }

    impl CounterContext<ArpCounters> for FakeCoreCtx {
        fn counters(&self) -> &ArpCounters {
            &self.inner.state.arp_counters
        }
    }

    #[test]
    fn test_mtu() {
        // Test that we send an Ethernet frame whose size is less than the MTU,
        // and that we don't send an Ethernet frame whose size is greater than
        // the MTU.
        fn test(size: usize, expect_frames_sent: bool) {
            let mut ctx = new_context();
            NeighborApi::<Ipv4, EthernetLinkDevice, _>::new(ctx.as_mut())
                .insert_static_entry(
                    &FakeDeviceId,
                    TEST_ADDRS_V4.remote_ip.get(),
                    TEST_ADDRS_V4.remote_mac,
                )
                .unwrap();
            let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
            let result = send_ip_frame::<FakeBindingsCtx, FakeCoreCtx, Ipv4, _>(
                core_ctx,
                bindings_ctx,
                &FakeDeviceId,
                IpPacketDestination::<Ipv4, _>::Neighbor(TEST_ADDRS_V4.remote_ip),
                Buf::new(&mut vec![0; size], ..),
                FakeTxMetadata::default(),
            )
            .map_err(|_serializer| ());
            let sent_frames = core_ctx.inner.frames().len();
            if expect_frames_sent {
                assert_eq!(sent_frames, 1);
                result.expect("should succeed");
            } else {
                assert_eq!(sent_frames, 0);
                result.expect_err("should fail");
            }
        }

        test(usize::try_from(u32::from(Ipv6::MINIMUM_LINK_MTU)).unwrap(), true);
        test(usize::try_from(u32::from(Ipv6::MINIMUM_LINK_MTU)).unwrap() + 1, false);
    }

    #[test]
    fn broadcast() {
        let mut ctx = new_context();
        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;
        send_ip_frame::<FakeBindingsCtx, FakeCoreCtx, Ipv4, _>(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            IpPacketDestination::<Ipv4, _>::Broadcast(()),
            Buf::new(&mut vec![0; 100], ..),
            FakeTxMetadata::default(),
        )
        .map_err(|_serializer| ())
        .expect("send_ip_frame should succeed");
        let sent_frames = core_ctx.inner.frames().len();
        assert_eq!(sent_frames, 1);
        let (FakeDeviceId, frame) = core_ctx.inner.frames()[0].clone();
        let (_body, _src_mac, dst_mac, _ether_type) =
            parse_ethernet_frame(&frame, EthernetFrameLengthCheck::NoCheck).unwrap();
        assert_eq!(dst_mac, Mac::BROADCAST);
    }

    #[test]
    fn test_join_link_multicast() {
        let mut ctx = new_context();
        let CtxPair { core_ctx, bindings_ctx } = &mut ctx;

        let address1: MulticastAddr<Mac> =
            MulticastAddr::new(Ipv4Addr::new([224, 0, 0, 200])).unwrap().into();
        let address2: MulticastAddr<Mac> =
            MulticastAddr::new(Ipv4Addr::new([224, 0, 10, 30])).unwrap().into();

        join_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address1);
        join_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address2);
        join_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address2);

        assert_eq!(
            bindings_ctx.state.link_multicast_group_memberships,
            HashSet::from_iter([(FakeDeviceId, address1), (FakeDeviceId, address2)])
        );

        leave_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address1);

        assert_eq!(
            bindings_ctx.state.link_multicast_group_memberships,
            HashSet::from_iter([(FakeDeviceId, address2)])
        );

        // Since we joined address2 twice, we need to leave it twice as well.
        leave_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address2);

        assert_eq!(
            bindings_ctx.state.link_multicast_group_memberships,
            HashSet::from_iter([(FakeDeviceId, address2)])
        );

        leave_link_multicast(core_ctx, bindings_ctx, &FakeDeviceId, address2);

        assert_eq!(bindings_ctx.state.link_multicast_group_memberships, HashSet::new());
    }
}
