// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations of traits defined in foreign modules for the types defined
//! in the ethernet module.

use core::time::Duration;

use alloc::vec::Vec;
use lock_order::lock::LockLevelFor;
use lock_order::relation::LockBefore;

use log::debug;
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpMarked, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use net_types::{SpecifiedAddr, UnicastAddr, Witness};
use netstack3_base::socket::SocketIpAddr;
use netstack3_base::{
    CoreTimerContext, CounterContext, DeviceIdContext, Marks, NetworkSerializer, SendFrameError,
    TxMetadata,
};
use netstack3_device::ethernet::{
    self, DynamicEthernetDeviceState, EthernetDeviceId, EthernetIpLinkDeviceDynamicStateContext,
    EthernetIpLinkDeviceStaticStateContext, EthernetLinkDevice, EthernetTimerId,
    EthernetWeakDeviceId, StaticEthernetDeviceState,
};
use netstack3_device::queue::{
    DequeueState, TransmitDequeueContext, TransmitQueueCommon, TransmitQueueContext,
    TransmitQueueState,
};
use netstack3_device::socket::{ParseSentFrameError, SentFrame};
use netstack3_device::{
    ArpConfigContext, ArpContext, ArpIpLayerContext, ArpNudCtx, ArpSenderContext, ArpState,
    DeviceId, DeviceLayerEventDispatcher, DeviceLayerTimerId, DeviceSendFrameError,
    IpLinkDeviceState,
};
use netstack3_ip::IpDeviceEgressStateContext;
use netstack3_ip::icmp::{self, IcmpErrorHandler, Icmpv4Error, Icmpv6Error, NdpCounters};
use netstack3_ip::nud::{
    DelegateNudContext, NudConfigContext, NudContext, NudIcmpContext, NudSenderContext, NudState,
    NudUserConfig, UseDelegateNudContext,
};
use packet::{Buf, BufferMut, InnerPacketBuilder as _};
use packet_formats::ethernet::EtherType;
use packet_formats::icmp::IcmpZeroCode;
use packet_formats::icmp::ndp::options::NdpOptionBuilder;
use packet_formats::icmp::ndp::{NeighborSolicitation, OptionSequenceBuilder};
use packet_formats::ip::{Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::Ipv4FragmentType;
use packet_formats::utils::NonZeroDuration;

use crate::context::WrapLockLevel;
use crate::context::prelude::*;
use crate::device::integration;
use crate::{BindingsContext, BindingsTypes, CoreCtx};

pub struct CoreCtxWithDeviceId<'a, CC: DeviceIdContext<EthernetLinkDevice>> {
    core_ctx: &'a mut CC,
    device_id: &'a CC::DeviceId,
}

impl<'a, CC: DeviceIdContext<EthernetLinkDevice>> DeviceIdContext<EthernetLinkDevice>
    for CoreCtxWithDeviceId<'a, CC>
{
    type DeviceId = CC::DeviceId;
    type WeakDeviceId = CC::WeakDeviceId;
}

impl<BC: BindingsContext, L> EthernetIpLinkDeviceStaticStateContext for CoreCtx<'_, BC, L> {
    fn with_static_ethernet_device_state<O, F: FnOnce(&StaticEthernetDeviceState) -> O>(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let state = integration::device_state(self, device_id);
        cb(&state.unlocked_access::<crate::lock_ordering::UnlockedState>().link.static_state)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::EthernetDeviceDynamicState>>
    EthernetIpLinkDeviceDynamicStateContext<BC> for CoreCtx<'_, BC, L>
{
    fn with_ethernet_state<
        O,
        F: FnOnce(&StaticEthernetDeviceState, &DynamicEthernetDeviceState) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let (dynamic_state, locked) =
            state.read_lock_and::<crate::lock_ordering::EthernetDeviceDynamicState>();
        cb(
            &locked.unlocked_access::<crate::lock_ordering::UnlockedState>().link.static_state,
            &dynamic_state,
        )
    }

    fn with_ethernet_state_mut<
        O,
        F: FnOnce(&StaticEthernetDeviceState, &mut DynamicEthernetDeviceState) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let (mut dynamic_state, locked) =
            state.write_lock_and::<crate::lock_ordering::EthernetDeviceDynamicState>();
        cb(
            &locked.unlocked_access::<crate::lock_ordering::UnlockedState>().link.static_state,
            &mut dynamic_state,
        )
    }
}

impl<BT: BindingsTypes, L> CoreTimerContext<EthernetTimerId<EthernetWeakDeviceId<BT>>, BT>
    for CoreCtx<'_, BT, L>
{
    fn convert_timer(dispatch_id: EthernetTimerId<EthernetWeakDeviceId<BT>>) -> BT::DispatchId {
        DeviceLayerTimerId::from(dispatch_id).into()
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::FilterState<Ipv6>>>
    NudContext<Ipv6, EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    type ConfigCtx<'a> = CoreCtxWithDeviceId<
        'a,
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::EthernetIpv6Nud>>,
    >;

    type SenderCtx<'a> = CoreCtxWithDeviceId<
        'a,
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::EthernetIpv6Nud>>,
    >;

    fn with_nud_state_mut_and_sender_ctx<
        O,
        F: FnOnce(&mut NudState<Ipv6, EthernetLinkDevice, BC>, &mut Self::SenderCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let (mut nud, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::EthernetIpv6Nud, _>(|c| c.right());
        let mut locked = CoreCtxWithDeviceId { device_id, core_ctx: &mut locked.cast_core_ctx() };
        cb(&mut nud, &mut locked)
    }

    fn with_nud_state_mut<
        O,
        F: FnOnce(&mut NudState<Ipv6, EthernetLinkDevice, BC>, &mut Self::ConfigCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let (mut nud, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::EthernetIpv6Nud, _>(|c| c.right());
        let mut locked = CoreCtxWithDeviceId { device_id, core_ctx: &mut locked.cast_core_ctx() };
        cb(&mut nud, &mut locked)
    }

    fn with_nud_state<O, F: FnOnce(&NudState<Ipv6, EthernetLinkDevice, BC>) -> O>(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let nud = core_ctx_and_resource
            .lock_with::<crate::lock_ordering::EthernetIpv6Nud, _>(|c| c.right());
        cb(&nud)
    }

    fn send_neighbor_solicitation(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &EthernetDeviceId<BC>,
        lookup_addr: SpecifiedAddr<Ipv6Addr>,
        remote_link_addr: Option<UnicastAddr<Mac>>,
    ) {
        let dst_ip = match remote_link_addr {
            // TODO(https://fxbug.dev/42081683): once `send_ndp_packet` does not go through
            // the normal IP egress flow, using the NUD table to resolve the link address,
            // use the specified link address to determine where to unicast the
            // solicitation.
            Some(_) => lookup_addr,
            None => lookup_addr.to_solicited_node_address().into_specified(),
        };
        let src_ip = IpDeviceEgressStateContext::<Ipv6>::get_local_addr_for_remote(
            self,
            &device_id.clone().into(),
            Some(dst_ip),
        );
        let src_ip = match src_ip {
            Some(s) => s,
            None => return,
        };

        let mac = ethernet::get_mac(self, device_id);

        CounterContext::<NdpCounters>::counters(self).tx.neighbor_solicitation.increment();
        debug!("sending NDP solicitation for {lookup_addr} to {dst_ip}");
        // TODO(https://fxbug.dev/42165912): Either panic or guarantee that this error
        // can't happen statically.
        let _: Result<(), _> = icmp::send_ndp_packet(
            self,
            bindings_ctx,
            &device_id.clone().into(),
            Some(src_ip.into()),
            dst_ip,
            OptionSequenceBuilder::<_>::new(
                [NdpOptionBuilder::SourceLinkLayerAddress(mac.bytes().as_ref())].iter(),
            )
            .into_serializer(),
            icmp::NdpMessage::NeighborSolicitation {
                message: NeighborSolicitation::new(lookup_addr.get()),
                code: IcmpZeroCode,
            },
        );
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IcmpAllSocketsSet<Ipv6>>>
    NudIcmpContext<Ipv6, EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    fn send_icmp_dest_unreachable(
        &mut self,
        bindings_ctx: &mut BC,
        frame: Buf<Vec<u8>>,
        device_id: Option<&Self::DeviceId>,
        original_src_ip: SocketIpAddr<Ipv6Addr>,
        original_dst_ip: SocketIpAddr<Ipv6Addr>,
        header_len: usize,
        proto: Ipv6Proto,
        _metadata: (),
    ) {
        <Self as IcmpErrorHandler<Ipv6, BC>>::send_icmp_error_message(
            self,
            bindings_ctx,
            device_id.map(|device_id| device_id.clone().into()).as_ref(), // This is Option<&CC::DeviceId>
            // NB: link layer address resolution only happens for packets destined for
            // a unicast address, so passing `None` as `FrameDestination` here is always
            // correct since there's never a need to not send the ICMP error due to
            // a multicast/broadcast destination.
            None,
            original_src_ip,
            original_dst_ip,
            frame,
            Icmpv6Error::AddressUnreachable,
            header_len,
            proto,
            // TODO(https://fxbug.dev/400977853): The pending frame this ICMP message is
            // responding to can either be generated from ourselves or being forwarded.
            // In the former case, the marks are irrelevant because this message will end
            // up being delivered locally. For the later case, we need to make sure the
            // marks are stored with the pending frames.
            &Marks::default(),
        );
    }
}

impl<'a, BC: BindingsContext, L: LockBefore<crate::lock_ordering::Ipv6DeviceLearnedParams>>
    NudConfigContext<Ipv6> for CoreCtxWithDeviceId<'a, CoreCtx<'a, BC, L>>
{
    fn retransmit_timeout(&mut self) -> NonZeroDuration {
        let Self { device_id, core_ctx } = self;
        let mut state = integration::device_state(core_ctx, device_id);
        let mut state = state.cast();
        // NB: This assignment is satisfying borrow checking on state.
        let x = state.read_lock::<crate::lock_ordering::Ipv6DeviceLearnedParams>().retrans_timer();
        x.unwrap_or_else(|| {
            NudConfigContext::<Ipv6>::with_nud_user_config(self, |c| c.retrans_timer)
        })
    }

    fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
        let Self { device_id, core_ctx } = self;
        let mut state = integration::device_state(core_ctx, device_id);
        let x = state.read_lock::<crate::lock_ordering::NudConfig<Ipv6>>();
        cb(&*x)
    }

    fn override_lock_time(&mut self) -> Duration {
        Duration::ZERO
    }
}

impl<'a, BC: BindingsContext, L: LockBefore<crate::lock_ordering::AllDeviceSockets>>
    NudSenderContext<Ipv6, EthernetLinkDevice, BC> for CoreCtxWithDeviceId<'a, CoreCtx<'a, BC, L>>
{
    fn send_ip_packet_to_neighbor_link_addr<S>(
        &mut self,
        bindings_ctx: &mut BC,
        dst_mac: UnicastAddr<Mac>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let Self { device_id, core_ctx } = self;
        ethernet::send_as_ethernet_frame_to_dst(
            *core_ctx,
            bindings_ctx,
            device_id,
            dst_mac.get(),
            body,
            EtherType::Ipv6,
            meta,
        )
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpState<Ipv4>>>
    ArpContext<EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    type ConfigCtx<'a> = CoreCtxWithDeviceId<
        'a,
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::EthernetIpv4Arp>>,
    >;

    type ArpSenderCtx<'a> = CoreCtxWithDeviceId<
        'a,
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::EthernetIpv4Arp>>,
    >;

    fn with_arp_state_mut_and_sender_ctx<
        O,
        F: FnOnce(&mut ArpState<EthernetLinkDevice, BC>, &mut Self::ArpSenderCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let (mut arp, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::EthernetIpv4Arp, _>(|c| c.right());
        let mut locked = CoreCtxWithDeviceId { device_id, core_ctx: &mut locked.cast_core_ctx() };
        cb(&mut arp, &mut locked)
    }

    fn get_protocol_addr(&mut self, device_id: &EthernetDeviceId<BC>) -> Option<Ipv4Addr> {
        let mut state = integration::device_state(self, device_id);
        let mut state = state.cast();
        let ipv4 = state.read_lock::<crate::lock_ordering::IpDeviceAddresses<Ipv4>>();
        // NB: This assignment is satisfying borrow checking on state.
        let x = ipv4.iter().next().map(|addr| addr.addr().get());
        x
    }

    fn get_hardware_addr(
        &mut self,
        _bindings_ctx: &mut BC,
        device_id: &EthernetDeviceId<BC>,
    ) -> UnicastAddr<Mac> {
        ethernet::get_mac(self, device_id)
    }

    fn with_arp_state_mut<
        O,
        F: FnOnce(&mut ArpState<EthernetLinkDevice, BC>, &mut Self::ConfigCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let (mut arp, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::EthernetIpv4Arp, _>(|c| c.right());
        let mut locked = CoreCtxWithDeviceId { device_id, core_ctx: &mut locked.cast_core_ctx() };
        cb(&mut arp, &mut locked)
    }

    fn with_arp_state<O, F: FnOnce(&ArpState<EthernetLinkDevice, BC>) -> O>(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let arp = core_ctx_and_resource
            .lock_with::<crate::lock_ordering::EthernetIpv4Arp, _>(|c| c.right());
        cb(&arp)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpDeviceConfiguration<Ipv4>>>
    ArpIpLayerContext<EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    fn on_arp_packet(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &EthernetDeviceId<BC>,
        frame_src: Mac,
        sender_hwaddr: UnicastAddr<Mac>,
        sender_addr: Ipv4Addr,
        target_addr: Ipv4Addr,
        is_arp_probe: bool,
    ) -> bool {
        let device_id = DeviceId::Ethernet(device_id.clone());
        netstack3_ip::device::on_arp_packet(
            self,
            bindings_ctx,
            &device_id,
            frame_src,
            sender_hwaddr,
            sender_addr,
            target_addr,
            is_arp_probe,
        )
    }
}

impl<BT: BindingsTypes, L> UseDelegateNudContext for CoreCtx<'_, BT, L> {}
impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IpState<Ipv4>>>
    DelegateNudContext<Ipv4> for CoreCtx<'_, BC, L>
{
    type Delegate<T> = ArpNudCtx<T>;
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::IcmpAllSocketsSet<Ipv4>>>
    NudIcmpContext<Ipv4, EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    fn send_icmp_dest_unreachable(
        &mut self,
        bindings_ctx: &mut BC,
        frame: Buf<Vec<u8>>,
        device_id: Option<&Self::DeviceId>,
        original_src_ip: SocketIpAddr<Ipv4Addr>,
        original_dst_ip: SocketIpAddr<Ipv4Addr>,
        header_len: usize,
        proto: Ipv4Proto,
        fragment_type: Ipv4FragmentType,
    ) {
        if fragment_type != Ipv4FragmentType::InitialFragment {
            return;
        }

        <Self as IcmpErrorHandler<Ipv4, BC>>::send_icmp_error_message(
            self,
            bindings_ctx,
            device_id.map(|device_id| device_id.clone().into()).as_ref(), // This is Option<&CC::DeviceId>
            // NB: link layer address resolution only happens for packets destined for
            // a unicast address, so passing `None` as `FrameDestination` here is always
            // correct since there's never a need to not send the ICMP error due to
            // a multicast/broadcast destination.
            None,
            original_src_ip,
            original_dst_ip,
            frame,
            Icmpv4Error::HostUnreachable,
            header_len,
            proto,
            // TODO(https://fxbug.dev/400977853): The pending frame this ICMP message is
            // responding to can either be generated from ourselves or being forwarded.
            // In the former case, the marks are irrelevant because this message will end
            // up being delivered locally. For the later case, we need to make sure the
            // marks are stored with the pending frames.
            &Marks::default(),
        );
    }
}

impl<'a, BC: BindingsContext, L: LockBefore<crate::lock_ordering::NudConfig<Ipv4>>> ArpConfigContext
    for CoreCtxWithDeviceId<'a, CoreCtx<'a, BC, L>>
{
    fn with_nud_user_config<O, F: FnOnce(&NudUserConfig) -> O>(&mut self, cb: F) -> O {
        let Self { device_id, core_ctx } = self;
        let mut state = integration::device_state(core_ctx, device_id);
        let x = state.read_lock::<crate::lock_ordering::NudConfig<Ipv4>>();
        cb(&*x)
    }
}

impl<'a, BC: BindingsContext, L: LockBefore<crate::lock_ordering::AllDeviceSockets>>
    ArpSenderContext<EthernetLinkDevice, BC> for CoreCtxWithDeviceId<'a, CoreCtx<'a, BC, L>>
{
    fn send_ip_packet_to_neighbor_link_addr<S>(
        &mut self,
        bindings_ctx: &mut BC,
        dst_mac: UnicastAddr<Mac>,
        body: S,
        meta: BC::TxMetadata,
    ) -> Result<(), SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let Self { device_id, core_ctx } = self;
        ethernet::send_as_ethernet_frame_to_dst(
            *core_ctx,
            bindings_ctx,
            device_id,
            dst_mac.get(),
            body,
            EtherType::Ipv4,
            meta,
        )
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::EthernetTxQueue>>
    TransmitQueueCommon<EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    type Meta = BC::TxMetadata;
    type DequeueContext = BC::DequeueContext;

    fn parse_outgoing_frame<'a, 'b>(
        buf: &'a [u8],
        _meta: &'b Self::Meta,
    ) -> Result<SentFrame<&'a [u8]>, ParseSentFrameError> {
        SentFrame::try_parse_as_ethernet(buf)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::EthernetTxQueue>>
    TransmitQueueContext<EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    fn with_transmit_queue_mut<
        O,
        F: FnOnce(&mut TransmitQueueState<Self::Meta, BC::TxBuffer, BC::TxAllocator>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let mut x = state.lock::<crate::lock_ordering::EthernetTxQueue>();
        cb(&mut x)
    }

    fn with_transmit_queue<
        O,
        F: FnOnce(&TransmitQueueState<Self::Meta, BC::TxBuffer, BC::TxAllocator>) -> O,
    >(
        &mut self,
        device_id: &EthernetDeviceId<BC>,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let x = state.lock::<crate::lock_ordering::EthernetTxQueue>();
        cb(&x)
    }

    fn send_frame(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        dequeue_context: Option<&mut BC::DequeueContext>,
        meta: Self::Meta,
        buf: BC::TxBuffer,
    ) -> Result<(), DeviceSendFrameError> {
        DeviceLayerEventDispatcher::send_ethernet_frame(
            bindings_ctx,
            device_id,
            buf,
            dequeue_context,
            meta.checksum_offload_result(),
        )
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::EthernetTxDequeue>>
    TransmitDequeueContext<EthernetLinkDevice, BC> for CoreCtx<'_, BC, L>
{
    type TransmitQueueCtx<'a> =
        CoreCtx<'a, BC, WrapLockLevel<crate::lock_ordering::EthernetTxDequeue>>;

    fn with_dequed_packets_and_tx_queue_ctx<
        O,
        F: FnOnce(&mut DequeueState<Self::Meta, BC::TxBuffer>, &mut Self::TransmitQueueCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut core_ctx_and_resource = integration::device_state_and_core_ctx(self, device_id);
        let (mut x, mut locked) = core_ctx_and_resource
            .lock_with_and::<crate::lock_ordering::EthernetTxDequeue, _>(|c| c.right());
        cb(&mut x, &mut locked.cast_core_ctx())
    }
}

impl<I: Ip, BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::NudConfig<I>
{
    type Data = IpMarked<I, NudUserConfig>;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::EthernetDeviceDynamicState
{
    type Data = DynamicEthernetDeviceState;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::EthernetIpv6Nud
{
    type Data = NudState<Ipv6, EthernetLinkDevice, BT>;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::EthernetIpv4Arp
{
    type Data = ArpState<EthernetLinkDevice, BT>;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::EthernetTxQueue
{
    type Data = TransmitQueueState<BT::TxMetadata, BT::TxBuffer, BT::TxAllocator>;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<EthernetLinkDevice, BT>>
    for crate::lock_ordering::EthernetTxDequeue
{
    type Data = DequeueState<BT::TxMetadata, BT::TxBuffer>;
}
