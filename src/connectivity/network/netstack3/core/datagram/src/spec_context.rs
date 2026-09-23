// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Datagram spec context definitions.
//!
//! This module contains the mirror traits for the datagram core context traits.
//! They exist in a separate module so that:
//!
//! * They're not in scope in the main datagram module, avoiding conflicting
//!   names.
//! * The boilerplate around the mirror traits is mostly out of the way.

use netstack3_base::socket::MaybeDualStack;
use netstack3_base::{AnyDevice, CoreTxMetadataContext, DeviceIdContext};
use netstack3_ip::{MulticastMembershipHandler, TransportIpContext};

use crate::internal::datagram::{
    BoundSocketsFromSpec, DatagramBindingsTypes, DatagramBoundStateContext,
    DatagramIpSpecificSocketOptions, DatagramSocketMapSpec, DatagramSocketSet, DatagramSocketSpec,
    DatagramStateContext, DualStackConverter, DualStackDatagramBoundStateContext, DualStackIpExt,
    IpExt, IpOptions, NonDualStackConverter, NonDualStackDatagramBoundStateContext, SocketState,
};
use crate::internal::sndbuf::TxMetadata;

/// A mirror trait of [`DatagramStateContext`] allowing foreign crates to
/// provide blanket impls for it.
///
/// This trait repeats all the definitions in [`DatagramStateContext`] to
/// provide blanket impls from an implementation on the type implementing
/// [`DatagramSocketSpec`], which is local for foreign implementers.
///
/// See [`DatagramStateContext`] for details.
#[allow(missing_docs)]
pub trait DatagramSpecStateContext<
    I: IpExt,
    CC: DeviceIdContext<AnyDevice>,
    BC: DatagramBindingsTypes,
>: DatagramSocketSpec
{
    type SocketsStateCtx<'a>: DatagramBoundStateContext<I, BC, Self>
        + DeviceIdContext<AnyDevice, DeviceId = CC::DeviceId, WeakDeviceId = CC::WeakDeviceId>;

    fn with_all_sockets_mut<O, F: FnOnce(&mut DatagramSocketSet<I, CC::WeakDeviceId, Self>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn with_all_sockets<O, F: FnOnce(&DatagramSocketSet<I, CC::WeakDeviceId, Self>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &SocketState<I, CC::WeakDeviceId, Self>) -> O,
    >(
        core_ctx: &mut CC,
        id: &Self::SocketId<I, CC::WeakDeviceId>,
        cb: F,
    ) -> O;

    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &mut SocketState<I, CC::WeakDeviceId, Self>) -> O,
    >(
        core_ctx: &mut CC,
        id: &Self::SocketId<I, CC::WeakDeviceId>,
        cb: F,
    ) -> O;

    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketsStateCtx<'_>,
            &Self::SocketId<I, CC::WeakDeviceId>,
            &SocketState<I, CC::WeakDeviceId, Self>,
        ),
    >(
        core_ctx: &mut CC,
        cb: F,
    );
}

impl<I, CC, BC, S> DatagramStateContext<I, BC, S> for CC
where
    I: IpExt,
    BC: DatagramBindingsTypes,
    CC: DeviceIdContext<AnyDevice>,
    S: DatagramSpecStateContext<I, CC, BC>,
{
    type SocketsStateCtx<'a> = S::SocketsStateCtx<'a>;

    fn with_all_sockets_mut<O, F: FnOnce(&mut DatagramSocketSet<I, Self::WeakDeviceId, S>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        S::with_all_sockets_mut(self, cb)
    }

    fn with_all_sockets<O, F: FnOnce(&DatagramSocketSet<I, Self::WeakDeviceId, S>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        S::with_all_sockets(self, cb)
    }

    fn with_socket_state<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &SocketState<I, Self::WeakDeviceId, S>) -> O,
    >(
        &mut self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
        cb: F,
    ) -> O {
        S::with_socket_state(self, id, cb)
    }

    fn with_socket_state_mut<
        O,
        F: FnOnce(&mut Self::SocketsStateCtx<'_>, &mut SocketState<I, Self::WeakDeviceId, S>) -> O,
    >(
        &mut self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
        cb: F,
    ) -> O {
        S::with_socket_state_mut(self, id, cb)
    }

    fn for_each_socket<
        F: FnMut(
            &mut Self::SocketsStateCtx<'_>,
            &S::SocketId<I, Self::WeakDeviceId>,
            &SocketState<I, Self::WeakDeviceId, S>,
        ),
    >(
        &mut self,
        cb: F,
    ) {
        S::for_each_socket(self, cb)
    }
}

/// A mirror trait of [`DatagramBoundStateContext`] allowing foreign crates to
/// provide blanket impls for it.
///
/// This trait repeats all the definitions in [`DatagramBoundStateContext`] to
/// provide blanket impls from an implementation on the type implementing
/// [`DatagramSocketSpec`], which is local for foreign implementers.
///
/// See [`DatagramBoundStateContext`] for details.
#[allow(missing_docs)]
pub trait DatagramSpecBoundStateContext<
    I: IpExt + DualStackIpExt,
    CC: DeviceIdContext<AnyDevice>,
    BC: DatagramBindingsTypes,
>: DatagramSocketSpec
{
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + CoreTxMetadataContext<TxMetadata<I, CC::WeakDeviceId, Self>, BC>
        + MulticastMembershipHandler<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = CC::DeviceId, WeakDeviceId = CC::WeakDeviceId>;

    type DualStackContext: DualStackDatagramBoundStateContext<
            I,
            BC,
            Self,
            DeviceId = CC::DeviceId,
            WeakDeviceId = CC::WeakDeviceId,
        >;

    type NonDualStackContext: NonDualStackDatagramBoundStateContext<
            I,
            BC,
            Self,
            DeviceId = CC::DeviceId,
            WeakDeviceId = CC::WeakDeviceId,
        >;

    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &BoundSocketsFromSpec<I, CC, Self>) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn with_bound_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut BoundSocketsFromSpec<I, CC, Self>) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn dual_stack_context(
        core_ctx: &CC,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext>;

    fn dual_stack_context_mut(
        core_ctx: &mut CC,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext>;

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;
}

impl<I, CC, BC, S> DatagramBoundStateContext<I, BC, S> for CC
where
    I: IpExt + DualStackIpExt,
    CC: DeviceIdContext<AnyDevice>,
    BC: DatagramBindingsTypes,
    S: DatagramSpecBoundStateContext<I, CC, BC>,
{
    type IpSocketsCtx<'a> = S::IpSocketsCtx<'a>;
    type DualStackContext = S::DualStackContext;
    type NonDualStackContext = S::NonDualStackContext;

    fn with_bound_sockets<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &BoundSocketsFromSpec<I, CC, S>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        S::with_bound_sockets(self, cb)
    }

    fn with_bound_sockets_mut<
        O,
        F: FnOnce(&mut Self::IpSocketsCtx<'_>, &mut BoundSocketsFromSpec<I, CC, S>) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        S::with_bound_sockets_mut(self, cb)
    }

    fn dual_stack_context(
        &self,
    ) -> MaybeDualStack<&Self::DualStackContext, &Self::NonDualStackContext> {
        S::dual_stack_context(self)
    }

    fn dual_stack_context_mut(
        &mut self,
    ) -> MaybeDualStack<&mut Self::DualStackContext, &mut Self::NonDualStackContext> {
        S::dual_stack_context_mut(self)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        S::with_transport_context(self, cb)
    }
}

/// A mirror trait of [`DualStackDatagramBoundStateContext`] allowing foreign
/// crates to provide blanket impls for it.
///
/// This trait repeats all the definitions in
/// [`DualStackDatagramBoundStateContext`] to provide blanket impls from an
/// implementation on the type implementing [`DatagramSocketSpec`], which is
/// local for foreign implementers.
///
/// See [`DualStackDatagramBoundStateContext`] for details.
#[allow(missing_docs)]
pub trait DualStackDatagramSpecBoundStateContext<
    I: IpExt,
    CC: DeviceIdContext<AnyDevice>,
    BC: DatagramBindingsTypes,
>: DatagramSocketSpec
{
    type IpSocketsCtx<'a>: TransportIpContext<I, BC>
        + CoreTxMetadataContext<TxMetadata<I, CC::WeakDeviceId, Self>, BC>
        + DeviceIdContext<AnyDevice, DeviceId = CC::DeviceId, WeakDeviceId = CC::WeakDeviceId>
        + TransportIpContext<I::OtherVersion, BC>
        + CoreTxMetadataContext<TxMetadata<I::OtherVersion, CC::WeakDeviceId, Self>, BC>;

    fn dual_stack_enabled(core_ctx: &CC, ip_options: &IpOptions<I, CC::WeakDeviceId, Self>)
    -> bool;

    fn to_other_socket_options<'a>(
        core_ctx: &CC,
        state: &'a IpOptions<I, CC::WeakDeviceId, Self>,
    ) -> &'a DatagramIpSpecificSocketOptions<I::OtherVersion, CC::WeakDeviceId>;

    fn ds_converter(core_ctx: &CC) -> impl DualStackConverter<I, CC::WeakDeviceId, Self>;

    fn to_other_bound_socket_id(
        core_ctx: &CC,
        id: &Self::SocketId<I, CC::WeakDeviceId>,
    ) -> <Self::SocketMapSpec<I::OtherVersion, CC::WeakDeviceId> as DatagramSocketMapSpec<
        I::OtherVersion,
        CC::WeakDeviceId,
        Self::AddrSpec,
    >>::BoundSocketId;

    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSocketsFromSpec<I, CC, Self>,
            &mut BoundSocketsFromSpec<I::OtherVersion, CC, Self>,
        ) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSocketsFromSpec<I::OtherVersion, CC, Self>,
        ) -> O,
    >(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        core_ctx: &mut CC,
        cb: F,
    ) -> O;
}

impl<I, CC, BC, S> DualStackDatagramBoundStateContext<I, BC, S> for CC
where
    I: IpExt,
    CC: DeviceIdContext<AnyDevice>,
    BC: DatagramBindingsTypes,
    S: DualStackDatagramSpecBoundStateContext<I, CC, BC>,
{
    type IpSocketsCtx<'a> = S::IpSocketsCtx<'a>;

    fn dual_stack_enabled(&self, ip_options: &IpOptions<I, Self::WeakDeviceId, S>) -> bool {
        S::dual_stack_enabled(self, ip_options)
    }

    fn to_other_socket_options<'a>(
        &self,
        ip_options: &'a IpOptions<I, Self::WeakDeviceId, S>,
    ) -> &'a DatagramIpSpecificSocketOptions<I::OtherVersion, Self::WeakDeviceId> {
        S::to_other_socket_options(self, ip_options)
    }

    fn ds_converter(&self) -> impl DualStackConverter<I, Self::WeakDeviceId, S> {
        S::ds_converter(self)
    }

    fn to_other_bound_socket_id(
        &self,
        id: &S::SocketId<I, Self::WeakDeviceId>,
    ) -> <S::SocketMapSpec<I::OtherVersion, Self::WeakDeviceId> as DatagramSocketMapSpec<
        I::OtherVersion,
        Self::WeakDeviceId,
        S::AddrSpec,
    >>::BoundSocketId {
        S::to_other_bound_socket_id(self, id)
    }

    fn with_both_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSocketsFromSpec<I, Self, S>,
            &mut BoundSocketsFromSpec<I::OtherVersion, Self, S>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        S::with_both_bound_sockets_mut(self, cb)
    }

    fn with_other_bound_sockets_mut<
        O,
        F: FnOnce(
            &mut Self::IpSocketsCtx<'_>,
            &mut BoundSocketsFromSpec<I::OtherVersion, Self, S>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        S::with_other_bound_sockets_mut(self, cb)
    }

    fn with_transport_context<O, F: FnOnce(&mut Self::IpSocketsCtx<'_>) -> O>(
        &mut self,
        cb: F,
    ) -> O {
        S::with_transport_context(self, cb)
    }
}

/// A mirror trait of [`NonDualStackDatagramBoundStateContext`] allowing foreign
/// crates to provide blanket impls for it.
///
/// This trait repeats all the definitions in
/// [`NonDualStackDatagramBoundStateContext`] to provide blanket impls from an
/// implementation on the type implementing [`DatagramSocketSpec`], which is
/// local for foreign implementers.
///
/// See [`NonDualStackDatagramBoundStateContext`] for details.
#[allow(missing_docs)]
pub trait NonDualStackDatagramSpecBoundStateContext<I: IpExt, CC: DeviceIdContext<AnyDevice>, BC>:
    DatagramSocketSpec
{
    fn nds_converter(core_ctx: &CC) -> impl NonDualStackConverter<I, CC::WeakDeviceId, Self>;
}

impl<I, CC, BC, S> NonDualStackDatagramBoundStateContext<I, BC, S> for CC
where
    I: IpExt,
    CC: DeviceIdContext<AnyDevice>,
    S: NonDualStackDatagramSpecBoundStateContext<I, CC, BC>,
{
    fn nds_converter(&self) -> impl NonDualStackConverter<I, CC::WeakDeviceId, S> {
        S::nds_converter(self)
    }
}
