// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A pure IP device, capable of directly sending/receiving IPv4 & IPv6 packets.

use core::convert::Infallible as Never;
use core::fmt::Debug;

use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use log::debug;
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6, Mtu};
use netstack3_base::sync::{Mutex, RwLock};
use netstack3_base::{
    BroadcastIpExt, ChecksumOffloadSpec, CoreTimerContext, Device, DeviceIdContext,
    NetworkParsingContext, NetworkSerializer, ReceivableFrameMeta, RecvFrameContext,
    RecvIpFrameMeta, ResourceCounterContext, SendFrameError, SendFrameErrorReason,
    SendableFrameMeta, TimerContext, TxMetadataBindingsTypes, WeakDeviceIdentifier,
};
use netstack3_ip::{DeviceIpLayerMetadata, IpPacketDestination};
use packet::BufferMut;

use crate::internal::base::{
    DeviceBufferBindingsTypes, DeviceCounters, DeviceLayerTypes, DeviceReceiveFrameSpec,
    PureIpDeviceCounters,
};
use crate::internal::id::{BaseDeviceId, BasePrimaryDeviceId, BaseWeakDeviceId, DeviceId};
use crate::internal::queue::tx::{
    TransmitQueue, TransmitQueueHandler, TransmitQueueState, TxQueuePacketMetadataCommon,
};
use crate::internal::queue::{DequeueState, DeviceBufferSpec, TransmitQueueFrameError};
use crate::internal::socket::{
    DeviceSocketHandler, DeviceSocketMetadata, DeviceSocketSendTypes, Frame, IpFrame, ReceivedFrame,
};
use crate::internal::state::{DeviceStateSpec, IpLinkDeviceState};

/// A weak device ID identifying a pure IP device.
///
/// This device ID is like [`WeakDeviceId`] but specifically for pure IP
/// devices.
///
/// [`WeakDeviceId`]: crate::device::WeakDeviceId
pub type PureIpWeakDeviceId<BT> = BaseWeakDeviceId<PureIpDevice, BT>;

/// A strong device ID identifying a pure IP device.
///
/// This device ID is like [`DeviceId`] but specifically for pure IP devices.
///
/// [`DeviceId`]: crate::device::DeviceId
pub type PureIpDeviceId<BT> = BaseDeviceId<PureIpDevice, BT>;

/// The primary reference for a pure IP device.
pub type PureIpPrimaryDeviceId<BT> = BasePrimaryDeviceId<PureIpDevice, BT>;

/// A marker type identifying a pure IP device.
#[derive(Copy, Clone)]
pub enum PureIpDevice {}

/// The parameters required to create a pure IP device.
#[derive(Debug)]
pub struct PureIpDeviceCreationProperties {
    /// The MTU of the device.
    pub mtu: Mtu,
    /// The checksum offload capabilities of the device.
    pub tx_offload_spec: netstack3_base::ChecksumOffloadSpec,
}

/// Metadata for IP packets held in the TX queue.
pub struct PureIpDeviceTxQueueFrameMetadata<BT: TxMetadataBindingsTypes> {
    /// The IP version of the sent packet.
    pub ip_version: IpVersion,
    /// Tx metadata associated with the frame.
    pub tx_metadata: BT::TxMetadata,
}

impl<BT: TxMetadataBindingsTypes> TxQueuePacketMetadataCommon
    for PureIpDeviceTxQueueFrameMetadata<BT>
where
    BT::TxMetadata: TxQueuePacketMetadataCommon,
{
    fn set_checksum_offload_result(
        &mut self,
        result: Option<netstack3_base::ChecksumOffloadResult>,
    ) {
        self.tx_metadata.set_checksum_offload_result(result);
    }
}

/// Metadata for sending IP packets from a device socket.
#[derive(Debug, PartialEq)]
pub struct PureIpHeaderParams {
    /// The IP version of the packet to send.
    pub ip_version: IpVersion,
}

/// State for a pure IP device.
pub struct PureIpDeviceState<BT: DeviceLayerTypes> {
    /// The device's dynamic state.
    dynamic_state: RwLock<DynamicPureIpDeviceState>,
    /// The device's transmit queue.
    pub tx_queue: TransmitQueue<
        PureIpDeviceTxQueueFrameMetadata<BT>,
        <PureIpDevice as DeviceBufferSpec<BT>>::TxBuffer,
        <PureIpDevice as DeviceBufferSpec<BT>>::TxAllocator,
    >,
    /// Counters specific to pure IP devices.
    pub counters: PureIpDeviceCounters,
}

/// Dynamic state for a pure IP device.
pub struct DynamicPureIpDeviceState {
    /// The MTU of the device.
    pub(crate) mtu: Mtu,
}

impl Device for PureIpDevice {}

impl<BT: DeviceBufferBindingsTypes> DeviceBufferSpec<BT> for PureIpDevice {
    type TxBuffer = BT::TxBuffer;
    type TxAllocator = BT::TxAllocator;
}

impl DeviceStateSpec for PureIpDevice {
    type State<BT: DeviceLayerTypes> = PureIpDeviceState<BT>;
    type External<BT: DeviceLayerTypes> = BT::PureIpDeviceState;
    type CreationProperties = PureIpDeviceCreationProperties;
    type Counters = PureIpDeviceCounters;
    const IS_LOOPBACK: bool = false;
    const DEBUG_TYPE: &'static str = "PureIP";
    type TimerId<D: WeakDeviceIdentifier> = Never;

    fn new_device_state<
        CC: CoreTimerContext<Self::TimerId<CC::WeakDeviceId>, BC> + DeviceIdContext<Self>,
        BC: DeviceLayerTypes + TimerContext,
    >(
        _bindings_ctx: &mut BC,
        _self_id: CC::WeakDeviceId,
        PureIpDeviceCreationProperties { mtu, tx_offload_spec }: Self::CreationProperties,
        tx_allocator: <Self as DeviceBufferSpec<BC>>::TxAllocator,
    ) -> Self::State<BC>
    where
        Self: DeviceBufferSpec<BC>,
    {
        PureIpDeviceState {
            dynamic_state: RwLock::new(DynamicPureIpDeviceState { mtu }),
            tx_queue: TransmitQueue::new(tx_allocator, tx_offload_spec),
            counters: PureIpDeviceCounters::default(),
        }
    }

    fn tx_offload_spec<BT: DeviceLayerTypes>(
        state: &Self::State<BT>,
    ) -> Option<ChecksumOffloadSpec> {
        Some(state.tx_queue.tx_offload_spec())
    }
}

/// Metadata for IP packets received on a pure IP device.
pub struct PureIpDeviceReceiveFrameMetadata<D> {
    /// The device a packet was received on.
    pub device_id: D,
    /// The IP version of the received packet.
    pub ip_version: IpVersion,
    /// The parsing context for the received packet.
    pub parsing_context: NetworkParsingContext,
}

impl DeviceReceiveFrameSpec for PureIpDevice {
    type FrameMetadata<D> = PureIpDeviceReceiveFrameMetadata<D>;
}

/// Provides access to a pure IP device's state.
pub trait PureIpDeviceStateContext: DeviceIdContext<PureIpDevice> {
    /// Calls the function with an immutable reference to the pure IP device's
    /// dynamic state.
    fn with_pure_ip_state<O, F: FnOnce(&DynamicPureIpDeviceState) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the function with a mutable reference to the pure IP device's
    /// dynamic state.
    fn with_pure_ip_state_mut<O, F: FnOnce(&mut DynamicPureIpDeviceState) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

impl DeviceSocketSendTypes for PureIpDevice {
    type Metadata = PureIpHeaderParams;
}

impl<CC, BC> ReceivableFrameMeta<CC, BC> for PureIpDeviceReceiveFrameMetadata<CC::DeviceId>
where
    CC: DeviceIdContext<PureIpDevice>
        + RecvFrameContext<RecvIpFrameMeta<CC::DeviceId, DeviceIpLayerMetadata<BC>, Ipv4>, BC>
        + RecvFrameContext<RecvIpFrameMeta<CC::DeviceId, DeviceIpLayerMetadata<BC>, Ipv6>, BC>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>
        + DeviceSocketHandler<PureIpDevice, BC>,
    BC: TxMetadataBindingsTypes,
{
    fn receive_meta<B: BufferMut + Debug>(
        self,
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        buffer: B,
    ) {
        let Self { device_id, ip_version, parsing_context } = self;

        core_ctx.add_both_usize(&device_id, buffer.len(), |counters: &DeviceCounters| {
            &counters.recv_bytes
        });
        core_ctx.increment_both(&device_id, |counters: &DeviceCounters| &counters.recv_frame);

        // NB: For conformance with Linux, don't verify that the contents of
        // of the buffer are a valid IPv4/IPv6 packet. Device sockets are
        // allowed to receive malformed packets.
        core_ctx.handle_frame(
            bindings_ctx,
            &device_id,
            Frame::Received(ReceivedFrame::Ip(IpFrame { ip_version, body: buffer.as_ref() })),
            buffer.as_ref(),
        );

        match ip_version {
            IpVersion::V4 => {
                core_ctx.increment_both(&device_id, |counters: &DeviceCounters| {
                    &counters.recv_ipv4_delivered
                });
                core_ctx.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv4>::new(
                        device_id,
                        None,
                        DeviceIpLayerMetadata::default(),
                        parsing_context,
                    ),
                    buffer,
                )
            }
            IpVersion::V6 => {
                core_ctx.increment_both(&device_id, |counters: &DeviceCounters| {
                    &counters.recv_ipv6_delivered
                });
                core_ctx.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv6>::new(
                        device_id,
                        None,
                        DeviceIpLayerMetadata::default(),
                        parsing_context,
                    ),
                    buffer,
                )
            }
        }
    }
}

impl<CC, BC> SendableFrameMeta<CC, BC> for DeviceSocketMetadata<PureIpDevice, CC::DeviceId>
where
    CC: TransmitQueueHandler<PureIpDevice, BC, Meta = PureIpDeviceTxQueueFrameMetadata<BC>>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
    BC: TxMetadataBindingsTypes,
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
        let Self { device_id, metadata: PureIpHeaderParams { ip_version } } = self;
        // TODO(https://fxbug.dev/391946195): Apply send buffer enforcement from
        // device sockets instead of using default.
        let tx_meta: BC::TxMetadata = Default::default();
        net_types::for_any_ip_version!(
            ip_version,
            I,
            queue_ip_frame::<_, _, I, _>(core_ctx, bindings_ctx, &device_id, body, tx_meta)
        )
    }
}

/// Enqueues the given IP packet on the TX queue for the given [`PureIpDevice`].
pub fn send_ip_frame<BC, CC, I, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    destination: IpPacketDestination<I, &DeviceId<BC>>,
    packet: S,
    tx_meta: BC::TxMetadata,
) -> Result<(), SendFrameError<S>>
where
    BC: DeviceLayerTypes,
    CC: TransmitQueueHandler<PureIpDevice, BC, Meta = PureIpDeviceTxQueueFrameMetadata<BC>>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
    I: Ip + BroadcastIpExt,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
{
    core_ctx.increment_both(device_id, |counters| &counters.send_total_frames);
    core_ctx.increment_both(device_id, DeviceCounters::send_frame::<I>);

    match destination {
        IpPacketDestination::Broadcast(_)
        | IpPacketDestination::Multicast(_)
        | IpPacketDestination::Neighbor(_) => (),
        IpPacketDestination::Loopback(_) => {
            unreachable!("Loopback packets must be delivered through the loopback device");
        }
    };

    queue_ip_frame::<_, _, I, _>(core_ctx, bindings_ctx, device_id, packet, tx_meta)
}

fn queue_ip_frame<BC, CC, I, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    packet: S,
    tx_metadata: BC::TxMetadata,
) -> Result<(), SendFrameError<S>>
where
    CC: TransmitQueueHandler<PureIpDevice, BC, Meta = PureIpDeviceTxQueueFrameMetadata<BC>>
        + ResourceCounterContext<CC::DeviceId, DeviceCounters>,
    BC: TxMetadataBindingsTypes,
    I: Ip,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
{
    let result = TransmitQueueHandler::<PureIpDevice, _>::queue_tx_frame(
        core_ctx,
        bindings_ctx,
        device_id,
        PureIpDeviceTxQueueFrameMetadata { ip_version: I::VERSION, tx_metadata },
        packet,
    );
    match result {
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

/// Gets the MTU of the given [`PureIpDevice`].
pub fn get_mtu<CC: PureIpDeviceStateContext>(core_ctx: &mut CC, device_id: &CC::DeviceId) -> Mtu {
    core_ctx.with_pure_ip_state(device_id, |DynamicPureIpDeviceState { mtu }| *mtu)
}

/// Updates the MTU of the given [`PureIpDevice`].
pub fn set_mtu<CC: PureIpDeviceStateContext>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
    new_mtu: Mtu,
) {
    core_ctx.with_pure_ip_state_mut(device_id, |DynamicPureIpDeviceState { mtu }| *mtu = new_mtu)
}

impl<BT: DeviceLayerTypes> OrderedLockAccess<DynamicPureIpDeviceState>
    for IpLinkDeviceState<PureIpDevice, BT>
{
    type Lock = RwLock<DynamicPureIpDeviceState>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.dynamic_state)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<
        TransmitQueueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer, BT::TxAllocator>,
    > for IpLinkDeviceState<PureIpDevice, BT>
{
    type Lock = Mutex<
        TransmitQueueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer, BT::TxAllocator>,
    >;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.queue)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<DequeueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer>>
    for IpLinkDeviceState<PureIpDevice, BT>
{
    type Lock = Mutex<DequeueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.deque)
    }
}
