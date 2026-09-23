// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The loopback device.

use alloc::vec::Vec;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use derivative::Derivative;

use lock_order::lock::{OrderedLockAccess, OrderedLockRef};
use log::trace;
use net_types::ethernet::Mac;
use net_types::ip::{Ipv4, Ipv6, Mtu};
use netstack3_base::sync::Mutex;
use netstack3_base::{
    AnyDevice, BroadcastIpExt, ChecksumOffloadSpec, ChecksumRxOffloading, CoreTimerContext, Device,
    DeviceIdAnyCompatContext, DeviceIdContext, FrameDestination, NetworkParsingContext,
    NetworkSerializer, RecvFrameContext, RecvIpFrameMeta, ResourceCounterContext, SendFrameError,
    SendFrameErrorReason, SendableFrameMeta, StrongDeviceIdentifier, TimerContext,
    TxMetadataBindingsTypes, WeakDeviceIdentifier,
};
use netstack3_ip::{DeviceIpLayerMetadata, IpCounters, IpPacketDestination};
use packet::{Buf, Buffer as _, BufferMut, FragmentedBuffer as _, NestablePacketBuilder as _};
use packet_formats::ethernet::{
    EtherType, EthernetFrame, EthernetFrameBuilder, EthernetFrameLengthCheck, EthernetIpExt,
};

use crate::internal::base::{
    DeviceCounters, DeviceLayerTypes, DeviceReceiveFrameSpec, EthernetDeviceCounters,
};
use crate::internal::id::{BaseDeviceId, BasePrimaryDeviceId, BaseWeakDeviceId, WeakDeviceId};
use crate::internal::queue::rx::{
    ReceiveDequeFrameContext, ReceiveQueue, ReceiveQueueState, ReceiveQueueTypes,
};
use crate::internal::queue::tx::{
    BufVecU8Allocator, TransmitQueue, TransmitQueueHandler, TransmitQueueState,
    TxQueuePacketMetadataCommon,
};
use crate::internal::queue::{DequeueState, DeviceBufferSpec, TransmitQueueFrameError};
use crate::internal::socket::{
    DeviceSocketHandler, DeviceSocketMetadata, DeviceSocketSendTypes, EthernetHeaderParams,
    ReceivedFrame,
};
use crate::internal::state::{DeviceStateSpec, IpLinkDeviceState};

/// The MAC address corresponding to the loopback interface.
const LOOPBACK_MAC: Mac = Mac::UNSPECIFIED;

/// A weak device ID identifying a loopback device.
///
/// This device ID is like [`WeakDeviceId`] but specifically for loopback
/// devices.
///
/// [`WeakDeviceId`]: crate::device::WeakDeviceId
pub type LoopbackWeakDeviceId<BT> = BaseWeakDeviceId<LoopbackDevice, BT>;

/// A strong device ID identifying a loopback device.
///
/// This device ID is like [`DeviceId`] but specifically for loopback devices.
///
/// [`DeviceId`]: crate::device::DeviceId
pub type LoopbackDeviceId<BT> = BaseDeviceId<LoopbackDevice, BT>;

/// The primary reference for a loopback device.
pub type LoopbackPrimaryDeviceId<BT> = BasePrimaryDeviceId<LoopbackDevice, BT>;

/// Loopback device domain.
#[derive(Copy, Clone)]
pub enum LoopbackDevice {}

impl Device for LoopbackDevice {}

impl<BT> DeviceBufferSpec<BT> for LoopbackDevice {
    type TxBuffer = Buf<Vec<u8>>;
    type TxAllocator = BufVecU8Allocator;
}

impl DeviceStateSpec for LoopbackDevice {
    type State<BT: DeviceLayerTypes> = LoopbackDeviceState<WeakDeviceId<BT>, BT>;
    type External<BT: DeviceLayerTypes> = BT::LoopbackDeviceState;
    type CreationProperties = LoopbackCreationProperties;
    type Counters = EthernetDeviceCounters;
    type TimerId<D: WeakDeviceIdentifier> = Never;

    fn new_device_state<
        CC: CoreTimerContext<Self::TimerId<CC::WeakDeviceId>, BC> + DeviceIdContext<Self>,
        BC: DeviceLayerTypes + TimerContext,
    >(
        _bindings_ctx: &mut BC,
        _self_id: CC::WeakDeviceId,
        LoopbackCreationProperties { mtu }: Self::CreationProperties,
        tx_allocator: <Self as DeviceBufferSpec<BC>>::TxAllocator,
    ) -> Self::State<BC>
    where
        Self: DeviceBufferSpec<BC>,
    {
        LoopbackDeviceState {
            counters: Default::default(),
            mtu,
            rx_queue: Default::default(),
            tx_queue: TransmitQueue::new(tx_allocator, ChecksumOffloadSpec::generic()),
        }
    }

    const IS_LOOPBACK: bool = true;
    const DEBUG_TYPE: &'static str = "Loopback";

    fn tx_offload_spec<BT: DeviceLayerTypes>(
        state: &Self::State<BT>,
    ) -> Option<ChecksumOffloadSpec> {
        Some(state.tx_queue.tx_offload_spec())
    }
}

/// Properties used to create a loopback device.
#[derive(Debug)]
pub struct LoopbackCreationProperties {
    /// The device's MTU.
    pub mtu: Mtu,
}

/// State for a loopback device.
pub struct LoopbackDeviceState<D: WeakDeviceIdentifier, BT: TxMetadataBindingsTypes> {
    /// Loopback device counters.
    pub counters: EthernetDeviceCounters,
    /// The MTU this device was created with (immutable).
    pub mtu: Mtu,
    /// Loopback device receive queue.
    pub rx_queue: ReceiveQueue<LoopbackRxQueueMeta<D, BT>, Buf<Vec<u8>>>,
    /// Loopback device transmit queue.
    pub tx_queue: TransmitQueue<
        LoopbackTxQueueMeta<D, BT>,
        <LoopbackDevice as DeviceBufferSpec<BT>>::TxBuffer,
        <LoopbackDevice as DeviceBufferSpec<BT>>::TxAllocator,
    >,
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
/// Metadata associated with a frame in the Loopback TX queue.
pub struct LoopbackTxQueueMeta<D: WeakDeviceIdentifier, BT: TxMetadataBindingsTypes> {
    /// Device that should be used to deliver the packet. If not set then the
    /// packet delivered as if it came from the loopback device.
    target_device: Option<D>,
    /// Metadata that is produced and consumed by the IP layer but which traverses
    /// the device layer through the loopback device.
    ip_layer_metadata: DeviceIpLayerMetadata<BT>,
}

impl<D: WeakDeviceIdentifier, BT: TxMetadataBindingsTypes> TxQueuePacketMetadataCommon
    for LoopbackTxQueueMeta<D, BT>
{
    fn set_checksum_offload_result(
        &mut self,
        _result: Option<netstack3_base::ChecksumOffloadResult>,
    ) {
        // Loopback doesn't need checksum offload result because it skips
        // verification on receive.
    }
}

/// Metadata associated with a frame in the Loopback RX queue.
#[derive(Derivative)]
#[derivative(Debug(bound = ""))]
pub struct LoopbackRxQueueMeta<D: WeakDeviceIdentifier, BT: TxMetadataBindingsTypes> {
    /// Device that should be used to deliver the packet. If not set then the
    /// packet delivered as if it came from the loopback device.
    target_device: Option<D>,
    /// Metadata that is produced and consumed by the IP layer but which traverses
    /// the device layer through the loopback device.
    ip_layer_metadata: DeviceIpLayerMetadata<BT>,
}

impl<D: WeakDeviceIdentifier, BT: TxMetadataBindingsTypes> From<LoopbackTxQueueMeta<D, BT>>
    for LoopbackRxQueueMeta<D, BT>
{
    fn from(
        LoopbackTxQueueMeta { target_device, ip_layer_metadata }: LoopbackTxQueueMeta<D, BT>,
    ) -> Self {
        Self { target_device, ip_layer_metadata }
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<ReceiveQueueState<LoopbackRxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>
    for IpLinkDeviceState<LoopbackDevice, BT>
{
    type Lock = Mutex<ReceiveQueueState<LoopbackRxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.rx_queue.queue)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<DequeueState<LoopbackRxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>
    for IpLinkDeviceState<LoopbackDevice, BT>
{
    type Lock = Mutex<DequeueState<LoopbackRxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.rx_queue.deque)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<
        TransmitQueueState<
            LoopbackTxQueueMeta<WeakDeviceId<BT>, BT>,
            Buf<Vec<u8>>,
            BufVecU8Allocator,
        >,
    > for IpLinkDeviceState<LoopbackDevice, BT>
{
    type Lock = Mutex<
        TransmitQueueState<
            LoopbackTxQueueMeta<WeakDeviceId<BT>, BT>,
            Buf<Vec<u8>>,
            BufVecU8Allocator,
        >,
    >;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.queue)
    }
}

impl<BT: DeviceLayerTypes>
    OrderedLockAccess<DequeueState<LoopbackTxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>
    for IpLinkDeviceState<LoopbackDevice, BT>
{
    type Lock = Mutex<DequeueState<LoopbackTxQueueMeta<WeakDeviceId<BT>, BT>, Buf<Vec<u8>>>>;
    fn ordered_lock_access(&self) -> OrderedLockRef<'_, Self::Lock> {
        OrderedLockRef::new(&self.link.tx_queue.deque)
    }
}

impl DeviceSocketSendTypes for LoopbackDevice {
    /// When `None`, data will be sent as a raw Ethernet frame without any
    /// system-applied headers.
    type Metadata = Option<EthernetHeaderParams>;
}

impl<CC, BC> ReceiveDequeFrameContext<LoopbackDevice, BC> for CC
where
    CC: DeviceIdContext<LoopbackDevice>
        + ResourceCounterContext<Self::DeviceId, EthernetDeviceCounters>
        + ReceiveQueueTypes<
            LoopbackDevice,
            BC,
            Meta = LoopbackRxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        >,
    // Loopback needs to deliver messages to `AnyDevice`.
    CC: DeviceIdAnyCompatContext<LoopbackDevice>
        + RecvFrameContext<
            RecvIpFrameMeta<
                <CC as DeviceIdContext<AnyDevice>>::DeviceId,
                DeviceIpLayerMetadata<BC>,
                Ipv4,
            >,
            BC,
        > + RecvFrameContext<
            RecvIpFrameMeta<
                <CC as DeviceIdContext<AnyDevice>>::DeviceId,
                DeviceIpLayerMetadata<BC>,
                Ipv6,
            >,
            BC,
        > + ResourceCounterContext<<CC as DeviceIdContext<AnyDevice>>::DeviceId, DeviceCounters>
        + ResourceCounterContext<<CC as DeviceIdContext<AnyDevice>>::DeviceId, IpCounters<Ipv4>>
        + ResourceCounterContext<<CC as DeviceIdContext<AnyDevice>>::DeviceId, IpCounters<Ipv6>>
        + DeviceSocketHandler<AnyDevice, BC>,
    CC::Buffer: BufferMut + Debug,
    BC: DeviceLayerTypes,
{
    fn handle_frame(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        rx_meta: Self::Meta,
        mut buf: Self::Buffer,
    ) {
        // NOTE: At the time of writing, netdevice_worker in bindings uses a
        // Buf<&mut [u8]> to feed receive frames into core. Matching the same
        // type as bindings gives us the benefit of not generating rx path code
        // twice.
        //
        // TODO(https://fxbug.dev/42051635): This might no longer be true when
        // we revisit owned netdevice buffers, at which point we must consider
        // the binary size tradeoff here.
        let mut buf = Buf::new(buf.as_mut(), ..);
        let buflen = buf.len();

        let (frame, whole_body) =
            match buf.parse_with_view::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::NoCheck) {
                Err(e) => {
                    self.increment_both(&device_id.clone().into(), |counters: &DeviceCounters| {
                        &counters.recv_parse_error
                    });
                    trace!("dropping invalid ethernet frame over loopback: {:?}", e);
                    return;
                }
                Ok(e) => e,
            };

        let LoopbackRxQueueMeta { target_device, ip_layer_metadata } = rx_meta;
        let target_device: <CC as DeviceIdContext<AnyDevice>>::DeviceId =
            match target_device.map(|d| d.upgrade()) {
                // This is a packet that should be delivered on `target_device`.
                Some(Some(dev)) => dev,

                // `target_device` is gone. Drop the packet.
                Some(None) => return,

                // This is a packet sent to the loopback device.
                None => device_id.clone().into(),
            };

        self.add_both_usize(&target_device, buflen, |counters: &DeviceCounters| {
            &counters.recv_bytes
        });
        self.increment_both(&target_device, |counters: &DeviceCounters| &counters.recv_frame);

        let frame_dest = FrameDestination::from_dest(frame.dst_mac(), Mac::UNSPECIFIED);
        let ethertype = frame.ethertype();

        DeviceSocketHandler::<AnyDevice, _>::handle_frame(
            self,
            bindings_ctx,
            &target_device,
            ReceivedFrame::from_ethernet(frame, frame_dest).into(),
            whole_body,
        );

        match ethertype {
            Some(EtherType::Ipv4) => {
                let local_frame_dst = match frame_dest.check_local() {
                    Some(dst) => dst,
                    None => {
                        self.increment_both(&target_device, |counters: &IpCounters<Ipv4>| {
                            &counters.drop_ip_packet_other_host
                        });
                        return;
                    }
                };
                self.increment_both(&target_device, |counters: &DeviceCounters| {
                    &counters.recv_ipv4_delivered
                });
                self.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv4>::new(
                        target_device,
                        Some(local_frame_dst),
                        ip_layer_metadata,
                        NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded),
                    ),
                    buf,
                );
            }
            Some(EtherType::Ipv6) => {
                let local_frame_dst = match frame_dest.check_local() {
                    Some(dst) => dst,
                    None => {
                        self.increment_both(&target_device, |counters: &IpCounters<Ipv6>| {
                            &counters.drop_ip_packet_other_host
                        });
                        return;
                    }
                };
                self.increment_both(&target_device, |counters: &DeviceCounters| {
                    &counters.recv_ipv6_delivered
                });
                self.receive_frame(
                    bindings_ctx,
                    RecvIpFrameMeta::<_, _, Ipv6>::new(
                        target_device,
                        Some(local_frame_dst),
                        ip_layer_metadata,
                        NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded),
                    ),
                    buf,
                );
            }
            Some(ethertype @ (EtherType::Arp | EtherType::Other(_))) => {
                self.increment_both(device_id, |counters: &EthernetDeviceCounters| {
                    &counters.recv_unsupported_ethertype
                });
                trace!("not handling loopback frame of type {:?}", ethertype)
            }
            None => {
                self.increment_both(device_id, |counters: &EthernetDeviceCounters| {
                    &counters.recv_no_ethertype
                });
                trace!("dropping ethernet frame without ethertype");
            }
        }
    }
}

impl<CC, BC> SendableFrameMeta<CC, BC>
    for DeviceSocketMetadata<LoopbackDevice, <CC as DeviceIdContext<LoopbackDevice>>::DeviceId>
where
    CC: TransmitQueueHandler<
            LoopbackDevice,
            BC,
            Meta = LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        > + ResourceCounterContext<<CC as DeviceIdContext<LoopbackDevice>>::DeviceId, DeviceCounters>
        + DeviceIdContext<AnyDevice>,
    BC: DeviceLayerTypes,
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
        let tx_meta = LoopbackTxQueueMeta::default();
        match metadata {
            Some(EthernetHeaderParams { dest_addr, protocol }) => send_as_ethernet_frame_to_dst(
                core_ctx,
                bindings_ctx,
                &device_id,
                body,
                protocol,
                dest_addr,
                tx_meta,
            ),
            None => send_ethernet_frame(core_ctx, bindings_ctx, &device_id, body, tx_meta),
        }
    }
}

/// Sends an IP frame `packet` over `device_id`.
pub fn send_ip_frame<CC, BC, I, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &<CC as DeviceIdContext<LoopbackDevice>>::DeviceId,
    destination: IpPacketDestination<I, &<CC as DeviceIdContext<AnyDevice>>::DeviceId>,
    ip_layer_metadata: DeviceIpLayerMetadata<BC>,
    packet: S,
) -> Result<(), SendFrameError<S>>
where
    CC: TransmitQueueHandler<
            LoopbackDevice,
            BC,
            Meta = LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        > + ResourceCounterContext<<CC as DeviceIdContext<LoopbackDevice>>::DeviceId, DeviceCounters>
        + DeviceIdContext<AnyDevice>,
    BC: DeviceLayerTypes,
    I: EthernetIpExt + BroadcastIpExt,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
{
    core_ctx.increment_both(device_id, DeviceCounters::send_frame::<I>);

    let target_device = match destination {
        IpPacketDestination::Loopback(device) => Some(device.downgrade()),
        IpPacketDestination::Broadcast(_)
        | IpPacketDestination::Multicast(_)
        | IpPacketDestination::Neighbor(_) => None,
    };
    send_as_ethernet_frame_to_dst(
        core_ctx,
        bindings_ctx,
        device_id,
        packet,
        I::ETHER_TYPE,
        LOOPBACK_MAC,
        LoopbackTxQueueMeta { target_device, ip_layer_metadata },
    )
}

fn send_as_ethernet_frame_to_dst<CC, BC, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &<CC as DeviceIdContext<LoopbackDevice>>::DeviceId,
    packet: S,
    protocol: EtherType,
    dst_mac: Mac,
    meta: LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
) -> Result<(), SendFrameError<S>>
where
    CC: TransmitQueueHandler<
            LoopbackDevice,
            BC,
            Meta = LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        > + ResourceCounterContext<<CC as DeviceIdContext<LoopbackDevice>>::DeviceId, DeviceCounters>
        + DeviceIdContext<AnyDevice>,
    BC: DeviceLayerTypes,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
{
    /// The minimum length of bodies of Ethernet frames sent over the loopback
    /// device.
    ///
    /// Use zero since the frames are never sent out a physical device, so it
    /// doesn't matter if they are shorter than would be required.
    const MIN_BODY_LEN: usize = 0;

    let frame =
        EthernetFrameBuilder::new(LOOPBACK_MAC, dst_mac, protocol, MIN_BODY_LEN).wrap_body(packet);

    send_ethernet_frame(core_ctx, bindings_ctx, device_id, frame, meta)
        .map_err(|err| err.into_inner())
}

fn send_ethernet_frame<CC, BC, S>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &<CC as DeviceIdContext<LoopbackDevice>>::DeviceId,
    frame: S,
    meta: LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
) -> Result<(), SendFrameError<S>>
where
    CC: TransmitQueueHandler<
            LoopbackDevice,
            BC,
            Meta = LoopbackTxQueueMeta<<CC as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        > + ResourceCounterContext<<CC as DeviceIdContext<LoopbackDevice>>::DeviceId, DeviceCounters>
        + DeviceIdContext<AnyDevice>,
    S: NetworkSerializer,
    S::Buffer: BufferMut,
    BC: DeviceLayerTypes,
{
    core_ctx.increment_both(device_id, |counters: &DeviceCounters| &counters.send_total_frames);
    match TransmitQueueHandler::<LoopbackDevice, _>::queue_tx_frame(
        core_ctx,
        bindings_ctx,
        device_id,
        meta,
        frame,
    ) {
        Ok(len) => {
            core_ctx.add_both_usize(device_id, len, |counters| &counters.send_bytes);
            core_ctx.increment_both(device_id, |counters: &DeviceCounters| &counters.send_frame);
            Ok(())
        }
        Err(TransmitQueueFrameError::NoQueue(err)) => {
            unreachable!("loopback never fails to send a frame: {err:?}")
        }
        Err(TransmitQueueFrameError::QueueFull(serializer)) => {
            core_ctx
                .increment_both(device_id, |counters: &DeviceCounters| &counters.send_queue_full);
            Err(SendFrameError { serializer, error: SendFrameErrorReason::QueueFull })
        }
        Err(TransmitQueueFrameError::SerializeError(err)) => {
            core_ctx.increment_both(device_id, |counters: &DeviceCounters| {
                &counters.send_serialize_error
            });
            Err(err.err_into())
        }
    }
}

impl DeviceReceiveFrameSpec for LoopbackDevice {
    // Loopback never receives frames from bindings, so make it impossible to
    // instantiate it.
    type FrameMetadata<D> = Never;
}
