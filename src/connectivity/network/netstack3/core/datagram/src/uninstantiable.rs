// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use explicit::UnreachableExt as _;
use netstack3_base::socket::MaybeDualStack;
use netstack3_base::{Uninstantiable, UninstantiableWrapper};

use crate::internal::datagram::{
    BoundDatagramSocketMap, DatagramBindingsTypes, DatagramBoundStateContext,
    DatagramIpSpecificSocketOptions, DatagramSocketMapSpec, DatagramSocketSpec, DualStackConverter,
    IpExt, IpOptions, NonDualStackConverter,
};
use crate::internal::spec_context::{
    DatagramSpecBoundStateContext, DualStackDatagramSpecBoundStateContext,
    NonDualStackDatagramSpecBoundStateContext,
};

impl<
    I: IpExt,
    S: DatagramSocketSpec,
    P: DatagramBoundStateContext<I, BC, S>,
    BC: DatagramBindingsTypes,
> DatagramSpecBoundStateContext<I, UninstantiableWrapper<P>, BC> for S
{
    type IpSocketsCtx<'a> = P::IpSocketsCtx<'a>;
    type DualStackContext = P::DualStackContext;
    type NonDualStackContext = P::NonDualStackContext;
    fn dual_stack_context(
        core_ctx: &UninstantiableWrapper<P>,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        core_ctx.uninstantiable_unreachable()
    }
    fn dual_stack_context_mut(
        core_ctx: &mut UninstantiableWrapper<P>,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        core_ctx.uninstantiable_unreachable()
    }
    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &BoundDatagramSocketMap<I, P::WeakDeviceId, S>) -> O,
    >(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }
    fn with_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundDatagramSocketMap<I, P::WeakDeviceId, S>,
        ) -> O,
    >(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }
    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }
}

impl<
    I: IpExt,
    S: DatagramSocketSpec,
    P: DatagramBoundStateContext<I, BC, S>,
    BC: DatagramBindingsTypes,
> NonDualStackDatagramSpecBoundStateContext<I, UninstantiableWrapper<P>, BC> for S
{
    fn nds_converter(
        core_ctx: &UninstantiableWrapper<P>,
    ) -> impl NonDualStackConverter<I, P::WeakDeviceId, Self> {
        core_ctx.uninstantiable_unreachable::<Uninstantiable>()
    }
}

impl<
    I: IpExt,
    S: DatagramSocketSpec,
    P: DatagramBoundStateContext<I, BC, S>,
    BC: DatagramBindingsTypes,
> DualStackDatagramSpecBoundStateContext<I, UninstantiableWrapper<P>, BC> for S
{
    type IpSocketsCtx<'a> = UninstantiableWrapper<P::IpSocketsCtx<'a>>;

    fn dual_stack_enabled(
        core_ctx: &UninstantiableWrapper<P>,
        _state: &IpOptions<I, P::WeakDeviceId, S>,
    ) -> bool {
        core_ctx.uninstantiable_unreachable()
    }

    fn to_other_socket_options<'a>(
        core_ctx: &UninstantiableWrapper<P>,
        _state: &'a IpOptions<I, P::WeakDeviceId, S>,
    ) -> &'a DatagramIpSpecificSocketOptions<I::OtherVersion, P::WeakDeviceId> {
        core_ctx.uninstantiable_unreachable()
    }

    fn ds_converter(
        core_ctx: &UninstantiableWrapper<P>,
    ) -> impl DualStackConverter<I, P::WeakDeviceId, S> {
        core_ctx.uninstantiable_unreachable::<Uninstantiable>()
    }

    fn to_other_bound_socket_id(
        core_ctx: &UninstantiableWrapper<P>,
        _id: &S::SocketId<I, P::WeakDeviceId>,
    ) -> <S::SocketMapSpec<I::OtherVersion, P::WeakDeviceId> as DatagramSocketMapSpec<
        I::OtherVersion,
        P::WeakDeviceId,
        S::AddrSpec,
    >>::BoundSocketId {
        core_ctx.uninstantiable_unreachable()
    }

    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundDatagramSocketMap<I, P::WeakDeviceId, S>,
            &mut BoundDatagramSocketMap<I::OtherVersion, P::WeakDeviceId, S>,
        ) -> O,
    >(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }

    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundDatagramSocketMap<I::OtherVersion, P::WeakDeviceId, S>,
        ) -> O,
    >(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut UninstantiableWrapper<P>,
        _cb: F,
    ) -> O {
        core_ctx.uninstantiable_unreachable()
    }
}
