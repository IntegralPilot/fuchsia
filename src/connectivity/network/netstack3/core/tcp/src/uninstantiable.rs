// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Groups trait implementations for uninstantiable types.

use explicit::UnreachableExt as _;

use netstack3_base::{Uninstantiable, UninstantiableWrapper, WeakDeviceIdentifier};

use crate::internal::socket::{
    AsThisStack, DemuxState, DualStackBaseIpExt, DualStackDemuxIdConverter, DualStackIpExt,
    TcpBindingsTypes, TcpDemuxContext, TcpDualStackContext, TcpSocketId,
};

impl<I: DualStackIpExt, D: WeakDeviceIdentifier, BT: TcpBindingsTypes, P: TcpDemuxContext<I, D, BT>>
    TcpDemuxContext<I, D, BT> for UninstantiableWrapper<P>
{
    type IpTransportCtx<'a> = P::IpTransportCtx<'a>;
    fn with_demux<O, F: FnOnce(&DemuxState<I, D, BT>) -> O>(&mut self, _cb: F) -> O {
        self.uninstantiable_unreachable()
    }
    fn with_demux_mut<O, F: FnOnce(&mut DemuxState<I, D, BT>) -> O>(&mut self, _cb: F) -> O {
        self.uninstantiable_unreachable()
    }
}

impl<P> AsThisStack<P> for UninstantiableWrapper<P> {
    fn as_this_stack(&mut self) -> &mut P {
        self.uninstantiable_unreachable()
    }
}

impl<I: DualStackIpExt> DualStackDemuxIdConverter<I> for Uninstantiable {
    fn convert<D: WeakDeviceIdentifier, BT: TcpBindingsTypes>(
        &self,
        _id: TcpSocketId<I, D, BT>,
    ) -> <I::OtherVersion as DualStackBaseIpExt>::DemuxSocketId<D, BT> {
        self.uninstantiable_unreachable()
    }
}

impl<
    I: DualStackIpExt,
    D: WeakDeviceIdentifier,
    BT: TcpBindingsTypes,
    P: TcpDualStackContext<I::OtherVersion, D, BT>,
> TcpDualStackContext<I, D, BT> for UninstantiableWrapper<P>
{
    type DualStackIpTransportCtx<'a> = P::DualStackIpTransportCtx<'a>;
    fn other_demux_id_converter(&self) -> impl DualStackDemuxIdConverter<I> {
        self.uninstantiable_unreachable::<Uninstantiable>()
    }

    fn dual_stack_enabled(&self, _ip_options: &I::DualStackIpOptions) -> bool {
        self.uninstantiable_unreachable()
    }

    fn set_dual_stack_enabled(&self, _ip_options: &mut I::DualStackIpOptions, _value: bool) {
        self.uninstantiable_unreachable()
    }

    fn with_both_demux_mut<
        O,
        F: FnOnce(&mut DemuxState<I, D, BT>, &mut DemuxState<I::OtherVersion, D, BT>) -> O,
    >(
        &mut self,
        _cb: F,
    ) -> O {
        self.uninstantiable_unreachable()
    }
}
