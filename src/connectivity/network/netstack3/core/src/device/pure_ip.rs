// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementations of traits defined in foreign modules for the types defined
//! in the pure_ip module.

use lock_order::lock::LockLevelFor;
use lock_order::relation::LockBefore;
use lock_order::wrap::LockedWrapperApi;
use net_types::ip::Ip;
use netstack3_base::{DeviceIdContext, TxMetadata};
use netstack3_device::pure_ip::{
    DynamicPureIpDeviceState, PureIpDevice, PureIpDeviceId, PureIpDeviceStateContext,
    PureIpDeviceTxQueueFrameMetadata, PureIpPrimaryDeviceId, PureIpWeakDeviceId,
};
use netstack3_device::queue::{
    DequeueState, TransmitDequeueContext, TransmitQueueCommon, TransmitQueueContext,
    TransmitQueueState,
};
use netstack3_device::socket::{IpFrame, ParseSentFrameError, SentFrame};
use netstack3_device::{
    DeviceCollectionContext, DeviceConfigurationContext, DeviceLayerEventDispatcher,
    DeviceSendFrameError, IpLinkDeviceState,
};
use netstack3_ip::nud::NudUserConfig;

use crate::device::integration;
use crate::{BindingsContext, BindingsTypes, CoreCtx};

impl<BT: BindingsTypes, L> DeviceIdContext<PureIpDevice> for CoreCtx<'_, BT, L> {
    type DeviceId = PureIpDeviceId<BT>;
    type WeakDeviceId = PureIpWeakDeviceId<BT>;
}

impl<'a, BT, L> DeviceCollectionContext<PureIpDevice, BT> for CoreCtx<'a, BT, L>
where
    BT: BindingsTypes,
    L: LockBefore<crate::lock_ordering::DeviceLayerState>,
{
    fn insert(&mut self, device: PureIpPrimaryDeviceId<BT>) {
        let mut devices = self.write_lock::<crate::lock_ordering::DeviceLayerState>();
        let strong = device.clone_strong();
        assert!(devices.pure_ip.insert(strong, device).is_none());
    }

    fn remove(&mut self, device: &PureIpDeviceId<BT>) -> Option<PureIpPrimaryDeviceId<BT>> {
        let mut devices = self.write_lock::<crate::lock_ordering::DeviceLayerState>();
        devices.pure_ip.remove(device)
    }
}

impl<'a, BT, L> DeviceConfigurationContext<PureIpDevice> for CoreCtx<'a, BT, L>
where
    BT: BindingsTypes,
{
    fn with_nud_config<I: Ip, O, F: FnOnce(Option<&NudUserConfig>) -> O>(
        &mut self,
        _device_id: &Self::DeviceId,
        f: F,
    ) -> O {
        // PureIp doesn't support NUD.
        f(None)
    }

    fn with_nud_config_mut<I: Ip, O, F: FnOnce(Option<&mut NudUserConfig>) -> O>(
        &mut self,
        _device_id: &Self::DeviceId,
        f: F,
    ) -> O {
        // PureIp doesn't support NUD.
        f(None)
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::PureIpDeviceTxQueue>>
    TransmitQueueCommon<PureIpDevice, BC> for CoreCtx<'_, BC, L>
{
    type Meta = PureIpDeviceTxQueueFrameMetadata<BC>;
    type DequeueContext = BC::DequeueContext;

    fn parse_outgoing_frame<'a, 'b>(
        buf: &'a [u8],
        meta: &'b Self::Meta,
    ) -> Result<SentFrame<&'a [u8]>, ParseSentFrameError> {
        let PureIpDeviceTxQueueFrameMetadata { ip_version, tx_metadata: _ } = meta;
        // NB: For conformance with Linux, don't verify that the contents of
        // of the buffer are a valid IPv4/IPv6 packet. Device sockets are
        // allowed to receive malformed packets.
        Ok(SentFrame::Ip(IpFrame { ip_version: *ip_version, body: buf }))
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::PureIpDeviceTxQueue>>
    TransmitQueueContext<PureIpDevice, BC> for CoreCtx<'_, BC, L>
{
    fn with_transmit_queue_mut<
        O,
        F: FnOnce(&mut TransmitQueueState<Self::Meta, BC::TxBuffer, BC::TxAllocator>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let mut x = state.lock::<crate::lock_ordering::PureIpDeviceTxQueue>();
        cb(&mut x)
    }

    fn with_transmit_queue<
        O,
        F: FnOnce(&TransmitQueueState<Self::Meta, BC::TxBuffer, BC::TxAllocator>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let x = state.lock::<crate::lock_ordering::PureIpDeviceTxQueue>();
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
        let PureIpDeviceTxQueueFrameMetadata { ip_version, tx_metadata } = meta;
        DeviceLayerEventDispatcher::send_ip_packet(
            bindings_ctx,
            device_id,
            buf,
            ip_version,
            dequeue_context,
            tx_metadata.checksum_offload_result(),
        )
        // Metadata is dropped at the end of the scope after publishing to
        // bindings.
    }
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::PureIpDeviceTxDequeue>>
    TransmitDequeueContext<PureIpDevice, BC> for CoreCtx<'_, BC, L>
{
    type TransmitQueueCtx<'a> = CoreCtx<'a, BC, crate::lock_ordering::PureIpDeviceTxDequeue>;

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
            .lock_with_and::<crate::lock_ordering::PureIpDeviceTxDequeue, _>(|c| c.right());
        cb(&mut x, &mut locked.cast_core_ctx())
    }
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<PureIpDevice, BT>>
    for crate::lock_ordering::PureIpDeviceDynamicState
{
    type Data = DynamicPureIpDeviceState;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<PureIpDevice, BT>>
    for crate::lock_ordering::PureIpDeviceTxQueue
{
    type Data =
        TransmitQueueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer, BT::TxAllocator>;
}

impl<BT: BindingsTypes> LockLevelFor<IpLinkDeviceState<PureIpDevice, BT>>
    for crate::lock_ordering::PureIpDeviceTxDequeue
{
    type Data = DequeueState<PureIpDeviceTxQueueFrameMetadata<BT>, BT::TxBuffer>;
}

impl<BC: BindingsContext, L: LockBefore<crate::lock_ordering::PureIpDeviceDynamicState>>
    PureIpDeviceStateContext for CoreCtx<'_, BC, L>
{
    fn with_pure_ip_state<O, F: FnOnce(&DynamicPureIpDeviceState) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let dynamic_state = state.read_lock::<crate::lock_ordering::PureIpDeviceDynamicState>();
        cb(&dynamic_state)
    }

    fn with_pure_ip_state_mut<O, F: FnOnce(&mut DynamicPureIpDeviceState) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        let mut state = integration::device_state(self, device_id);
        let mut dynamic_state =
            state.write_lock::<crate::lock_ordering::PureIpDeviceDynamicState>();
        cb(&mut dynamic_state)
    }
}
