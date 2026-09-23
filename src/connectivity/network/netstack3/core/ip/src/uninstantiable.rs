// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use explicit::UnreachableExt as _;
use net_types::SpecifiedAddr;
use netstack3_base::{
    AnyDevice, DeviceIdContext, IpExt, Mms, TxMetadataBindingsTypes, Uninstantiable,
    UninstantiableWrapper,
};
use netstack3_filter::{FilterIpExt, Tuple};

use crate::internal::base::{BaseTransportIpContext, HopLimits, IpLayerIpExt};
use crate::internal::device::Ipv6LinkLayerAddr;
use crate::internal::socket::{
    DeviceIpSocketHandler, IpSock, IpSockCreationError, IpSockSendError, IpSocketArgs,
    IpSocketHandler, MmsError, RouteResolutionOptions,
};

impl<I: IpExt, C, P: DeviceIdContext<AnyDevice>> BaseTransportIpContext<I, C>
    for UninstantiableWrapper<P>
{
    type DevicesWithAddrIter<'s> = UninstantiableWrapper<core::iter::Empty<P::DeviceId>>;
    fn with_devices_with_assigned_addr<O, F: FnOnce(Self::DevicesWithAddrIter<'_>) -> O>(
        &mut self,
        _addr: SpecifiedAddr<I::Addr>,
        _cb: F,
    ) -> O {
        self.uninstantiable_unreachable()
    }
    fn get_default_hop_limits(&mut self, _device: Option<&Self::DeviceId>) -> HopLimits {
        self.uninstantiable_unreachable()
    }
    fn get_original_destination(&mut self, _tuple: &Tuple<I>) -> Option<(I::Addr, u16)> {
        self.uninstantiable_unreachable()
    }
}

impl<I: IpExt + FilterIpExt, BC: TxMetadataBindingsTypes, P: DeviceIdContext<AnyDevice>>
    IpSocketHandler<I, BC> for UninstantiableWrapper<P>
{
    fn new_ip_socket<O>(
        &mut self,
        _ctx: &mut BC,
        _args: IpSocketArgs<'_, Self::DeviceId, I, O>,
    ) -> Result<IpSock<I, Self::WeakDeviceId>, IpSockCreationError> {
        self.uninstantiable_unreachable()
    }

    fn send_ip_packet<S, O>(
        &mut self,
        _ctx: &mut BC,
        _socket: &IpSock<I, Self::WeakDeviceId>,
        _body: S,
        _options: &O,
        _tx_metadata: BC::TxMetadata,
    ) -> Result<(), IpSockSendError> {
        self.uninstantiable_unreachable()
    }

    fn confirm_reachable<O>(
        &mut self,
        _bindings_ctx: &mut BC,
        _socket: &IpSock<I, Self::WeakDeviceId>,
        _options: &O,
    ) {
        self.uninstantiable_unreachable()
    }
}

impl<I: IpLayerIpExt, C, P: DeviceIpSocketHandler<I, C>> DeviceIpSocketHandler<I, C>
    for UninstantiableWrapper<P>
{
    fn get_mms<O: RouteResolutionOptions<I>>(
        &mut self,
        _bindings_ctx: &mut C,
        _ip_sock: &IpSock<I, Self::WeakDeviceId>,
        _options: &O,
    ) -> Result<Mms, MmsError> {
        self.uninstantiable_unreachable()
    }
}

impl Ipv6LinkLayerAddr for Uninstantiable {
    fn as_bytes(&self) -> &[u8] {
        self.uninstantiable_unreachable()
    }

    fn eui64_iid(&self) -> [u8; 8] {
        self.uninstantiable_unreachable()
    }
}
