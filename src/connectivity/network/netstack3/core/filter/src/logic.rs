// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub(crate) mod nat;

use core::fmt::Debug;
use core::num::NonZeroU16;
use core::ops::RangeInclusive;

use derivative::Derivative;
use log::{debug, error};
use net_types::ip::{GenericOverIp, Ip, IpVersionMarker};
use netstack3_base::{
    AnyDevice, DeviceIdContext, HandleableTimer, InterfaceProperties, IpDeviceAddressIdContext,
};
use packet_formats::ip::IpExt;

use crate::conntrack::{Connection, FinalizeConnectionError, GetConnectionError};
use crate::context::{FilterBindingsContext, FilterBindingsTypes, FilterIpContext};
use crate::packets::{FilterIpExt, FilterIpPacket, MaybeTransportPacket};
use crate::state::{
    Action, FilterIpMetadata, FilterPacketMetadata, Hook, RejectType, Routine, Rule,
    TransparentProxy,
};

/// The final result of packet processing at a given filtering hook.
///
/// The type parameters depend on the hook:
/// - `S` is returned with `Stop` and specifies the reason for stopping or
///   additional actions to take.
/// - `P` is returned with `Proceed` and carries context for further processing
///   (e.g. NAT results).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict<S, P = Accept> {
    /// The packet should continue traversing the stack.
    Proceed(P),
    /// The packet processing should be stopped. The argument specifies
    /// additional actions to take.
    Stop(S),
}

/// A value returned by a filter to indicate that the packet should be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accept;

impl<S, P> Verdict<S, P> {
    fn is_stop(&self) -> bool {
        matches!(self, Verdict::Stop(_))
    }
}

/// A stop reason for hooks that can only drop packets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropPacket;

/// The reason for stopping packet processing at the ingress hook.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IngressStopReason<I: IpExt> {
    /// The packet should be dropped.
    Drop,
    /// The packet should be redirected to a local socket.
    TransparentLocalDelivery {
        /// The bound address of the local socket to redirect the packet to.
        addr: I::Addr,
        /// The bound port of the local socket to redirect the packet to.
        port: NonZeroU16,
    },
}

/// A stop reason for hooks that can drop or reject packets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropOrReject {
    /// The packet should be dropped.
    Drop,
    /// The packet should be rejected.
    Reject(RejectType),
}

/// The verdict for the ingress hook.
pub type IngressVerdict<I> = Verdict<IngressStopReason<I>>;

impl<I: IpExt> From<RoutineResult<I>> for IngressVerdict<I> {
    fn from(verdict: RoutineResult<I>) -> Self {
        match verdict {
            RoutineResult::Accept | RoutineResult::Return => Verdict::Proceed(Accept),
            RoutineResult::Drop => Verdict::Stop(IngressStopReason::Drop),
            RoutineResult::TransparentLocalDelivery { addr, port } => {
                Verdict::Stop(IngressStopReason::TransparentLocalDelivery { addr, port })
            }
            result @ (RoutineResult::Redirect { .. } | RoutineResult::Masquerade { .. }) => {
                unreachable!("NAT actions are only valid in NAT routines; got {result:?}")
            }
            RoutineResult::Reject { .. } => {
                unreachable!("Reject actions are not allowed in ingress routines")
            }
        }
    }
}

pub type LocalIngressVerdict = Verdict<DropOrReject>;
pub type ForwardVerdict = Verdict<DropOrReject>;
pub type EgressVerdict = Verdict<DropPacket>;
pub type LocalEgressVerdict = Verdict<DropOrReject>;

impl<I: IpExt> From<RoutineResult<I>> for Verdict<DropPacket> {
    fn from(result: RoutineResult<I>) -> Self {
        match result {
            RoutineResult::Accept | RoutineResult::Return => Verdict::Proceed(Accept),
            RoutineResult::Drop => Verdict::Stop(DropPacket),
            result @ RoutineResult::TransparentLocalDelivery { .. } => {
                unreachable!(
                    "transparent local delivery is only valid in INGRESS hook; got {result:?}"
                )
            }
            result @ (RoutineResult::Redirect { .. } | RoutineResult::Masquerade { .. }) => {
                unreachable!("NAT actions are only valid in NAT routines; got {result:?}")
            }
            RoutineResult::Reject(_reject_type) => {
                unreachable!(
                    "Reject action is allowed only in FORWARD, LOCAL_INGRESS and LOCAL_EGRESS hooks"
                )
            }
        }
    }
}

impl<I: IpExt> From<RoutineResult<I>> for Verdict<DropOrReject> {
    fn from(result: RoutineResult<I>) -> Self {
        match result {
            RoutineResult::Accept | RoutineResult::Return => Verdict::Proceed(Accept),
            RoutineResult::Drop => Verdict::Stop(DropOrReject::Drop),
            RoutineResult::TransparentLocalDelivery { .. } => {
                unreachable!(
                    "transparent local delivery is only valid in INGRESS hook; got {result:?}"
                )
            }
            result @ (RoutineResult::Redirect { .. } | RoutineResult::Masquerade { .. }) => {
                unreachable!("NAT actions are only valid in NAT routines; got {result:?}")
            }
            RoutineResult::Reject(reject_type) => Verdict::Stop(DropOrReject::Reject(reject_type)),
        }
    }
}

/// A witness type to indicate that the egress filtering hook has been run.
#[derive(Debug)]
pub struct ProofOfEgressCheck {
    _private_field_to_prevent_construction_outside_of_module: (),
}

impl ProofOfEgressCheck {
    /// Clones this proof of egress check.
    ///
    /// May only be used in case of fragmentation after going through the egress
    /// hook.
    pub fn clone_for_fragmentation(&self) -> Self {
        Self { _private_field_to_prevent_construction_outside_of_module: () }
    }
}

#[derive(Debug, Derivative)]
#[derivative(Clone(bound = ""), Copy(bound = ""))]
/// References to the ingress and egress interfaces for a packet.
pub struct Interfaces<'a, D> {
    /// The ingress interface if any. Not set if the packet was produced
    /// locally.
    pub ingress: Option<&'a D>,
    /// The egress interface if known. Not set if the the packet is being
    /// delivered locally or has't been routed yet.
    pub egress: Option<&'a D>,
}

/// The result of packet processing for a given routine.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(crate) enum RoutineResult<I: IpExt> {
    /// The packet should stop traversing the rest of the current installed
    /// routine, but continue travsering other routines installed in the hook.
    Accept,
    /// The packet should continue at the next rule in the calling chain.
    Return,
    /// The packet should be dropped immediately.
    Drop,
    /// The packet should be immediately redirected to a local socket without its
    /// header being changed in any way.
    TransparentLocalDelivery {
        /// The bound address of the local socket to redirect the packet to.
        addr: I::Addr,
        /// The bound port of the local socket to redirect the packet to.
        port: NonZeroU16,
    },
    /// Destination NAT (DNAT) should be performed to redirect the packet to the
    /// local host.
    Redirect {
        /// The optional range of destination ports used to rewrite the packet.
        ///
        /// If absent, the destination port of the packet is not rewritten.
        dst_port: Option<RangeInclusive<NonZeroU16>>,
    },
    /// Source NAT (SNAT) should be performed to rewrite the source address of the
    /// packet to one owned by the outgoing interface.
    Masquerade {
        /// The optional range of source ports used to rewrite the packet.
        ///
        /// If absent, the source port of the packet is not rewritten.
        src_port: Option<RangeInclusive<NonZeroU16>>,
    },
    Reject(RejectType),
}

impl<I: IpExt> RoutineResult<I> {
    fn is_terminal(&self) -> bool {
        match self {
            RoutineResult::Accept
            | RoutineResult::Drop
            | RoutineResult::TransparentLocalDelivery { .. }
            | RoutineResult::Redirect { .. }
            | RoutineResult::Masquerade { .. }
            | RoutineResult::Reject(_) => true,
            RoutineResult::Return => false,
        }
    }
}

fn apply_transparent_proxy<I: IpExt, P: MaybeTransportPacket>(
    proxy: &TransparentProxy<I>,
    dst_addr: I::Addr,
    maybe_transport_packet: P,
) -> RoutineResult<I> {
    let (addr, port) = match proxy {
        TransparentProxy::LocalPort(port) => (dst_addr, *port),
        TransparentProxy::LocalAddr(addr) => {
            let Some(transport_packet_data) = maybe_transport_packet.transport_packet_data() else {
                // We ensure that TransparentProxy rules are always accompanied by a
                // TCP or UDP matcher when filtering state is provided to Core, but
                // given this invariant is enforced far from here, we log an error
                // and drop the packet, which would likely happen at the transport
                // layer anyway.
                error!(
                    "transparent proxy action is only valid on a rule that matches \
                    on transport protocol, but this packet has no transport header",
                );
                return RoutineResult::Drop;
            };
            // TCP and UDP don't support a destination port of 0, so we have no
            // choice but to drop the packet.
            //
            // TODO(https://fxbug.dev/341128580): Revisit this once filtering is
            // able to rewrite a port to 0.
            let Some(port) = NonZeroU16::new(transport_packet_data.dst_port()) else {
                // TODO(https://fxbug.dev/517102537): This should have an
                // Inspect counter.
                debug!("attempted to TPROXY packet to port 0");
                return RoutineResult::Drop;
            };
            (*addr, port)
        }
        TransparentProxy::LocalAddrAndPort(addr, port) => (*addr, *port),
    };
    RoutineResult::TransparentLocalDelivery { addr, port }
}

fn check_routine<I, P, D, BC, M>(
    Routine { rules }: &Routine<I, BC, ()>,
    packet: &P,
    interfaces: Interfaces<'_, D>,
    metadata: &mut M,
) -> RoutineResult<I>
where
    I: FilterIpExt,
    P: FilterIpPacket<I>,
    D: InterfaceProperties<BC::DeviceClass>,
    BC: FilterBindingsContext<D>,
    M: FilterPacketMetadata,
{
    for Rule { matcher, action, validation_info: () } in rules {
        if matcher.matches(packet, interfaces, metadata) {
            match action {
                Action::Accept => return RoutineResult::Accept,
                Action::Return => return RoutineResult::Return,
                Action::Drop => return RoutineResult::Drop,
                // TODO(https://fxbug.dev/332739892): enforce some kind of maximum depth on the
                // routine graph to prevent a stack overflow here.
                Action::Jump(target) => {
                    let result = check_routine(target.get(), packet, interfaces, metadata);
                    if result.is_terminal() {
                        return result;
                    }
                    continue;
                }
                Action::TransparentProxy(proxy) => {
                    return apply_transparent_proxy(
                        proxy,
                        packet.dst_addr(),
                        packet.maybe_transport_packet(),
                    );
                }
                Action::Redirect { dst_port } => {
                    return RoutineResult::Redirect { dst_port: dst_port.clone() };
                }
                Action::Masquerade { src_port } => {
                    return RoutineResult::Masquerade { src_port: src_port.clone() };
                }
                Action::Mark { domain, action } => {
                    // Mark is a non-terminating action, it will not yield a `RoutineResult` but
                    // it will continue on processing the next rule in the routine.
                    metadata.apply_mark_action(*domain, *action);
                }
                Action::None => {
                    continue;
                }
                Action::Reject(reject_type) => {
                    return RoutineResult::Reject(*reject_type);
                }
            }
        }
    }
    RoutineResult::Return
}

fn check_routines_for_hook<I, P, D, BC, M, SR>(
    hook: &Hook<I, BC, ()>,
    packet: &P,
    interfaces: Interfaces<'_, D>,
    metadata: &mut M,
) -> Verdict<SR>
where
    I: FilterIpExt,
    P: FilterIpPacket<I>,
    D: InterfaceProperties<BC::DeviceClass>,
    BC: FilterBindingsContext<D>,
    M: FilterPacketMetadata,
    Verdict<SR>: From<RoutineResult<I>>,
{
    let Hook { routines } = hook;
    for routine in routines {
        let verdict: Verdict<SR> = check_routine(&routine, packet, interfaces, metadata).into();
        match verdict {
            Verdict::Proceed(Accept) => (),
            Verdict::Stop(stop_reason) => return Verdict::Stop(stop_reason),
        }
    }
    Verdict::Proceed(Accept)
}

/// An implementation of packet filtering logic, providing entry points at
/// various stages of packet processing.
pub trait FilterHandler<I: FilterIpExt, BC: FilterBindingsTypes>:
    IpDeviceAddressIdContext<I, DeviceId: InterfaceProperties<BC::DeviceClass>>
{
    /// The ingress hook intercepts incoming traffic before a routing decision
    /// has been made.
    fn ingress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> IngressVerdict<I>
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>;

    /// The local ingress hook intercepts incoming traffic that is destined for
    /// the local host.
    fn local_ingress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> LocalIngressVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>;

    /// The forwarding hook intercepts incoming traffic that is destined for
    /// another host.
    fn forwarding_hook<P, M>(
        &mut self,
        packet: &mut P,
        in_interface: &Self::DeviceId,
        out_interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> ForwardVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>;

    /// The local egress hook intercepts locally-generated traffic before a
    /// routing decision has been made.
    fn local_egress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> LocalEgressVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>;

    /// The egress hook intercepts all outgoing traffic after a routing decision
    /// has been made.
    fn egress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> (EgressVerdict, ProofOfEgressCheck)
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>;
}

/// The "production" implementation of packet filtering.
///
/// Provides an implementation of [`FilterHandler`] for any `CC` that implements
/// [`FilterIpContext`].
pub struct FilterImpl<'a, CC>(pub &'a mut CC);

impl<CC: DeviceIdContext<AnyDevice>> DeviceIdContext<AnyDevice> for FilterImpl<'_, CC> {
    type DeviceId = CC::DeviceId;
    type WeakDeviceId = CC::WeakDeviceId;
}

impl<I, CC> IpDeviceAddressIdContext<I> for FilterImpl<'_, CC>
where
    I: FilterIpExt,
    CC: IpDeviceAddressIdContext<I>,
{
    type AddressId = CC::AddressId;
    type WeakAddressId = CC::WeakAddressId;
}

impl<I, BC, CC> FilterHandler<I, BC> for FilterImpl<'_, CC>
where
    I: FilterIpExt,
    BC: FilterBindingsContext<CC::DeviceId>,
    CC: FilterIpContext<I, BC>,
{
    fn ingress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> IngressVerdict<I>
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
    {
        let Self(this) = self;
        this.with_filter_state_and_nat_ctx(|state, core_ctx| {
            // There usually isn't going to be an existing connection in the metadata before
            // this hook, but it's possible in the case of looped-back packets, so check for
            // one first before looking in the conntrack table.
            let conn = match metadata.take_connection_and_direction() {
                Some((c, d)) => Some((c, d)),
                None => {
                    packet.conntrack_packet().and_then(|packet| {
                        match state
                            .conntrack
                            .get_connection_for_packet_and_update(bindings_ctx, packet)
                        {
                            Ok(result) => result,
                            // TODO(https://fxbug.dev/328064909): Support configurable dropping of
                            // invalid packets.
                            Err(GetConnectionError::InvalidPacket(c, d)) => Some((c, d)),
                        }
                    })
                }
            };

            let verdict = check_routines_for_hook(
                &state.installed_routines.get().ip.ingress,
                packet,
                Interfaces { ingress: Some(interface), egress: None },
                metadata,
            );

            if verdict.is_stop() {
                return verdict;
            }

            if let Some((mut conn, direction)) = conn {
                // TODO(https://fxbug.dev/343683914): provide a way to run filter routines
                // post-NAT, but in the same hook. Currently all filter routines are run before
                // all NAT routines in the same hook.
                match nat::perform_nat::<nat::IngressHook, _, _, _, _>(
                    core_ctx,
                    bindings_ctx,
                    state.nat_installed.get(),
                    &state.conntrack,
                    &mut conn,
                    direction,
                    &state.installed_routines.get().nat.ingress,
                    packet,
                    Interfaces { ingress: Some(interface), egress: None },
                ) {
                    Verdict::Stop(DropPacket) => return Verdict::Stop(IngressStopReason::Drop),
                    Verdict::Proceed(Accept) => (),
                }

                let res = metadata.replace_connection_and_direction(conn, direction);
                debug_assert!(res.is_none());
            }

            verdict
        })
    }

    fn local_ingress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> LocalIngressVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
    {
        let Self(this) = self;
        this.with_filter_state_and_nat_ctx(|state, core_ctx| {
            let conn = match metadata.take_connection_and_direction() {
                Some((c, d)) => Some((c, d)),
                // It's possible that there won't be a connection in the metadata by this point;
                // this could be, for example, because the packet is for a protocol not tracked
                // by conntrack.
                None => packet.conntrack_packet().and_then(|packet| {
                    match state.conntrack.get_connection_for_packet_and_update(bindings_ctx, packet)
                    {
                        Ok(result) => result,
                        // TODO(https://fxbug.dev/328064909): Support configurable dropping of
                        // invalid packets.
                        Err(GetConnectionError::InvalidPacket(c, d)) => Some((c, d)),
                    }
                }),
            };

            let verdict = check_routines_for_hook(
                &state.installed_routines.get().ip.local_ingress,
                packet,
                Interfaces { ingress: Some(interface), egress: None },
                metadata,
            );

            if verdict.is_stop() {
                return verdict;
            }

            if let Some((mut conn, direction)) = conn {
                // TODO(https://fxbug.dev/343683914): provide a way to run filter routines
                // post-NAT, but in the same hook. Currently all filter routines are run before
                // all NAT routines in the same hook.
                match nat::perform_nat::<nat::LocalIngressHook, _, _, _, _>(
                    core_ctx,
                    bindings_ctx,
                    state.nat_installed.get(),
                    &state.conntrack,
                    &mut conn,
                    direction,
                    &state.installed_routines.get().nat.local_ingress,
                    packet,
                    Interfaces { ingress: Some(interface), egress: None },
                ) {
                    Verdict::Stop(DropPacket) => return Verdict::Stop(DropOrReject::Drop),
                    Verdict::Proceed(Accept) => (),
                }

                match state.conntrack.finalize_connection(bindings_ctx, conn) {
                    Ok((_inserted, _weak_conn)) => {}
                    // If finalizing the connection would result in a conflict in the connection
                    // tracking table, or if the table is at capacity, drop the packet.
                    Err(FinalizeConnectionError::Conflict | FinalizeConnectionError::TableFull) => {
                        return Verdict::Stop(DropOrReject::Drop);
                    }
                }
            }

            verdict
        })
    }

    fn forwarding_hook<P, M>(
        &mut self,
        packet: &mut P,
        in_interface: &Self::DeviceId,
        out_interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> ForwardVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
    {
        let Self(this) = self;
        this.with_filter_state(|state| {
            check_routines_for_hook(
                &state.installed_routines.get().ip.forwarding,
                packet,
                Interfaces { ingress: Some(in_interface), egress: Some(out_interface) },
                metadata,
            )
        })
    }

    fn local_egress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> LocalEgressVerdict
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
    {
        let Self(this) = self;
        this.with_filter_state_and_nat_ctx(|state, core_ctx| {
            // There isn't going to be an existing connection in the metadata
            // before this hook, so we don't have to look.
            let conn = packet.conntrack_packet().and_then(|packet| {
                match state.conntrack.get_connection_for_packet_and_update(bindings_ctx, packet) {
                    Ok(result) => result,
                    // TODO(https://fxbug.dev/328064909): Support configurable dropping of invalid
                    // packets.
                    Err(GetConnectionError::InvalidPacket(c, d)) => Some((c, d)),
                }
            });

            let verdict = check_routines_for_hook(
                &state.installed_routines.get().ip.local_egress,
                packet,
                Interfaces { ingress: None, egress: Some(interface) },
                metadata,
            );

            if verdict.is_stop() {
                return verdict;
            }

            if let Some((mut conn, direction)) = conn {
                // TODO(https://fxbug.dev/343683914): provide a way to run filter routines
                // post-NAT, but in the same hook. Currently all filter routines are run before
                // all NAT routines in the same hook.
                match nat::perform_nat::<nat::LocalEgressHook, _, _, _, _>(
                    core_ctx,
                    bindings_ctx,
                    state.nat_installed.get(),
                    &state.conntrack,
                    &mut conn,
                    direction,
                    &state.installed_routines.get().nat.local_egress,
                    packet,
                    Interfaces { ingress: None, egress: Some(interface) },
                ) {
                    Verdict::Stop(DropPacket) => return Verdict::Stop(DropOrReject::Drop),
                    Verdict::Proceed(Accept) => (),
                }

                let res = metadata.replace_connection_and_direction(conn, direction);
                debug_assert!(res.is_none());
            }

            verdict
        })
    }

    fn egress_hook<P, M>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: &mut P,
        interface: &Self::DeviceId,
        metadata: &mut M,
    ) -> (EgressVerdict, ProofOfEgressCheck)
    where
        P: FilterIpPacket<I>,
        M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
    {
        let Self(this) = self;
        let verdict = this.with_filter_state_and_nat_ctx(|state, core_ctx| {
            let conn = match metadata.take_connection_and_direction() {
                Some((c, d)) => Some((c, d)),
                // It's possible that there won't be a connection in the metadata by this point;
                // this could be, for example, because the packet is for a protocol not tracked
                // by conntrack.
                None => packet.conntrack_packet().and_then(|packet| {
                    match state.conntrack.get_connection_for_packet_and_update(bindings_ctx, packet)
                    {
                        Ok(result) => result,
                        // TODO(https://fxbug.dev/328064909): Support configurable dropping of
                        // invalid packets.
                        Err(GetConnectionError::InvalidPacket(c, d)) => Some((c, d)),
                    }
                }),
            };

            let verdict = check_routines_for_hook(
                &state.installed_routines.get().ip.egress,
                packet,
                Interfaces { ingress: None, egress: Some(interface) },
                metadata,
            );

            if verdict.is_stop() {
                return verdict;
            }

            if let Some((mut conn, direction)) = conn {
                // TODO(https://fxbug.dev/343683914): provide a way to run filter routines
                // post-NAT, but in the same hook. Currently all filter routines are run before
                // all NAT routines in the same hook.
                match nat::perform_nat::<nat::EgressHook, _, _, _, _>(
                    core_ctx,
                    bindings_ctx,
                    state.nat_installed.get(),
                    &state.conntrack,
                    &mut conn,
                    direction,
                    &state.installed_routines.get().nat.egress,
                    packet,
                    Interfaces { ingress: None, egress: Some(interface) },
                ) {
                    Verdict::Stop(DropPacket) => return Verdict::Stop(DropPacket),
                    Verdict::Proceed(Accept) => (),
                }

                match state.conntrack.finalize_connection(bindings_ctx, conn) {
                    Ok((_inserted, conn)) => {
                        if let Some(conn) = conn {
                            let res = metadata.replace_connection_and_direction(
                                Connection::Shared(conn),
                                direction,
                            );
                            debug_assert!(res.is_none());
                        }
                    }
                    // If finalizing the connection would result in a conflict in the connection
                    // tracking table, or if the table is at capacity, drop the packet.
                    Err(FinalizeConnectionError::Conflict | FinalizeConnectionError::TableFull) => {
                        return Verdict::Stop(DropPacket);
                    }
                }
            }

            verdict
        });
        (
            verdict,
            ProofOfEgressCheck { _private_field_to_prevent_construction_outside_of_module: () },
        )
    }
}

/// A timer ID for the filtering crate.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, GenericOverIp, Hash)]
#[generic_over_ip(I, Ip)]
pub enum FilterTimerId<I: Ip> {
    /// A trigger for the conntrack module to perform garbage collection.
    ConntrackGc(IpVersionMarker<I>),
}

impl<I, BC, CC> HandleableTimer<CC, BC> for FilterTimerId<I>
where
    I: FilterIpExt,
    BC: FilterBindingsContext<CC::DeviceId>,
    CC: FilterIpContext<I, BC>,
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        match self {
            FilterTimerId::ConntrackGc(_) => core_ctx.with_filter_state(|state| {
                state.conntrack.perform_gc(bindings_ctx);
            }),
        }
    }
}

#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    use core::marker::PhantomData;

    use net_types::ip::AddrSubnet;
    use netstack3_base::AssignedAddrIpExt;
    use netstack3_base::testutil::{FakeStrongDeviceId, FakeWeakAddressId, FakeWeakDeviceId};

    use super::*;

    /// A no-op implementation of packet filtering that accepts any packet that
    /// passes through it, useful for unit tests of other modules where trait bounds
    /// require that a `FilterHandler` is available but no filtering logic is under
    /// test.
    ///
    /// Provides an implementation of [`FilterHandler`].
    pub struct NoopImpl<DeviceId>(PhantomData<DeviceId>);

    impl<DeviceId> Default for NoopImpl<DeviceId> {
        fn default() -> Self {
            Self(PhantomData)
        }
    }

    impl<DeviceId: FakeStrongDeviceId> DeviceIdContext<AnyDevice> for NoopImpl<DeviceId> {
        type DeviceId = DeviceId;
        type WeakDeviceId = FakeWeakDeviceId<DeviceId>;
    }

    impl<I: AssignedAddrIpExt, DeviceId: FakeStrongDeviceId> IpDeviceAddressIdContext<I>
        for NoopImpl<DeviceId>
    {
        type AddressId = AddrSubnet<I::Addr, I::AssignedWitness>;
        type WeakAddressId = FakeWeakAddressId<Self::AddressId>;
    }

    impl<I, BC, DeviceId> FilterHandler<I, BC> for NoopImpl<DeviceId>
    where
        I: FilterIpExt + AssignedAddrIpExt,
        BC: FilterBindingsTypes,
        DeviceId: FakeStrongDeviceId + InterfaceProperties<BC::DeviceClass>,
    {
        fn ingress_hook<P, M>(
            &mut self,
            _: &mut BC,
            _: &mut P,
            _: &Self::DeviceId,
            _: &mut M,
        ) -> IngressVerdict<I>
        where
            P: FilterIpPacket<I>,
            M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
        {
            Verdict::Proceed(Accept)
        }

        fn local_ingress_hook<P, M>(
            &mut self,
            _: &mut BC,
            _: &mut P,
            _: &Self::DeviceId,
            _: &mut M,
        ) -> LocalIngressVerdict
        where
            P: FilterIpPacket<I>,
            M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
        {
            Verdict::Proceed(Accept)
        }

        fn forwarding_hook<P, M>(
            &mut self,
            _: &mut P,
            _: &Self::DeviceId,
            _: &Self::DeviceId,
            _: &mut M,
        ) -> ForwardVerdict
        where
            P: FilterIpPacket<I>,
            M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
        {
            Verdict::Proceed(Accept)
        }

        fn local_egress_hook<P, M>(
            &mut self,
            _: &mut BC,
            _: &mut P,
            _: &Self::DeviceId,
            _: &mut M,
        ) -> LocalEgressVerdict
        where
            P: FilterIpPacket<I>,
            M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
        {
            Verdict::Proceed(Accept)
        }

        fn egress_hook<P, M>(
            &mut self,
            _: &mut BC,
            _: &mut P,
            _: &Self::DeviceId,
            _: &mut M,
        ) -> (EgressVerdict, ProofOfEgressCheck)
        where
            P: FilterIpPacket<I>,
            M: FilterIpMetadata<I, Self::WeakAddressId, BC>,
        {
            (Verdict::Proceed(Accept), ProofOfEgressCheck::forge_proof_for_test())
        }
    }

    impl ProofOfEgressCheck {
        /// For tests where it's not feasible to run the egress hook.
        pub(crate) fn forge_proof_for_test() -> Self {
            ProofOfEgressCheck { _private_field_to_prevent_construction_outside_of_module: () }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;

    use assert_matches::assert_matches;
    use derivative::Derivative;
    use ip_test_macro::ip_test;
    use net_types::ip::{AddrSubnet, Ipv4};
    use netstack3_base::testutil::{FakeDeviceClass, FakeMatcherDeviceId};
    use netstack3_base::{
        AddressMatcher, AddressMatcherType, AssignedAddrIpExt, InterfaceMatcher, MarkDomain, Marks,
        PortMatcher, SegmentHeader,
    };
    use netstack3_hashmap::HashMap;
    use test_case::test_case;

    use super::*;
    use crate::actions::MarkAction;
    use crate::conntrack::{self, ConnectionDirection};
    use crate::context::testutil::{FakeBindingsCtx, FakeCtx, FakeWeakAddressId};
    use crate::logic::nat::NatConfig;
    use crate::matchers::{PacketMatcher, TransportProtocolMatcher};
    use crate::packets::IpPacket;
    use crate::packets::testutil::internal::{
        ArbitraryValue, FakeIpPacket, FakeTcpSegment, FakeUdpPacket, TransportPacketExt,
    };
    use crate::state::{FakePacketMetadata, IpRoutines, NatRoutines, UninstalledRoutine};
    use crate::testutil::TestIpExt;

    impl<I: IpExt> Rule<I, FakeBindingsCtx<I>, ()> {
        pub(crate) fn new(
            matcher: PacketMatcher<I, FakeBindingsCtx<I>>,
            action: Action<I, FakeBindingsCtx<I>, ()>,
        ) -> Self {
            Rule { matcher, action, validation_info: () }
        }
    }

    #[test]
    fn return_by_default_if_no_matching_rules_in_routine() {
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &Routine { rules: Vec::new() },
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Return
        );

        // A subroutine should also yield `Return` if no rules match, allowing
        // the calling routine to continue execution after the `Jump`.
        let routine = Routine {
            rules: vec![
                Rule::new(
                    PacketMatcher::default(),
                    Action::Jump(UninstalledRoutine::new(Vec::new(), 0)),
                ),
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Drop
        );
    }

    #[derive(Derivative)]
    #[derivative(Default(bound = ""))]
    struct PacketMetadata<I: IpExt + AssignedAddrIpExt, A, BT: FilterBindingsTypes> {
        conn: Option<(Connection<I, NatConfig<I, A>, BT>, ConnectionDirection)>,
        marks: Marks,
    }

    impl<I: TestIpExt, A, BT: FilterBindingsTypes> FilterIpMetadata<I, A, BT>
        for PacketMetadata<I, A, BT>
    {
        fn take_connection_and_direction(
            &mut self,
        ) -> Option<(Connection<I, NatConfig<I, A>, BT>, ConnectionDirection)> {
            let Self { conn, marks: _ } = self;
            conn.take()
        }

        fn replace_connection_and_direction(
            &mut self,
            new_conn: Connection<I, NatConfig<I, A>, BT>,
            direction: ConnectionDirection,
        ) -> Option<Connection<I, NatConfig<I, A>, BT>> {
            let Self { conn, marks: _ } = self;
            conn.replace((new_conn, direction)).map(|(conn, _dir)| conn)
        }
    }

    impl<I, A, BT> FilterPacketMetadata for PacketMetadata<I, A, BT>
    where
        I: TestIpExt,
        BT: FilterBindingsTypes,
    {
        fn apply_mark_action(&mut self, domain: MarkDomain, action: MarkAction) {
            action.apply(self.marks.get_mut(domain))
        }

        fn socket_info(&self) -> Option<crate::SocketInfo> {
            None
        }

        fn marks(&self) -> &Marks {
            &self.marks
        }
    }

    #[test]
    fn accept_by_default_if_no_matching_rules_in_hook() {
        assert_eq!(
            check_routines_for_hook::<
                Ipv4,
                _,
                FakeMatcherDeviceId,
                FakeBindingsCtx<Ipv4>,
                _,
                DropPacket,
            >(
                &Hook::default(),
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Proceed(Accept)
        );
    }

    #[test]
    fn accept_by_default_if_return_from_routine() {
        let hook = Hook {
            routines: vec![Routine {
                rules: vec![Rule::new(PacketMatcher::default(), Action::Return)],
            }],
        };

        assert_eq!(
            check_routines_for_hook::<
                Ipv4,
                _,
                FakeMatcherDeviceId,
                FakeBindingsCtx<Ipv4>,
                _,
                DropPacket,
            >(
                &hook,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Proceed(Accept)
        );
    }

    #[test]
    fn accept_terminal_for_installed_routine() {
        let routine = Routine {
            rules: vec![
                // Accept all traffic.
                Rule::new(PacketMatcher::default(), Action::Accept),
                // Drop all traffic.
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Accept
        );

        // `Accept` should also be propagated from subroutines.
        let routine = Routine {
            rules: vec![
                // Jump to a routine that accepts all traffic.
                Rule::new(
                    PacketMatcher::default(),
                    Action::Jump(UninstalledRoutine::new(
                        vec![Rule::new(PacketMatcher::default(), Action::Accept)],
                        0,
                    )),
                ),
                // Drop all traffic.
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Accept
        );

        // Now put that routine in a hook that also includes *another* installed
        // routine which drops all traffic. The first installed routine should
        // terminate at its `Accept` result, but the hook should terminate at
        // the `Drop` result in the second routine.
        let hook = Hook {
            routines: vec![
                routine,
                Routine {
                    rules: vec![
                        // Drop all traffic.
                        Rule::new(PacketMatcher::default(), Action::Drop),
                    ],
                },
            ],
        };

        assert_eq!(
            check_routines_for_hook::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _, _>(
                &hook,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(DropPacket)
        );
    }

    #[test]
    fn drop_terminal_for_entire_hook() {
        let hook = Hook {
            routines: vec![
                Routine {
                    rules: vec![
                        // Drop all traffic.
                        Rule::new(PacketMatcher::default(), Action::Drop),
                    ],
                },
                Routine {
                    rules: vec![
                        // Accept all traffic.
                        Rule::new(PacketMatcher::default(), Action::Accept),
                    ],
                },
            ],
        };

        assert_eq!(
            check_routines_for_hook::<
                Ipv4,
                _,
                FakeMatcherDeviceId,
                FakeBindingsCtx<Ipv4>,
                _,
                DropPacket,
            >(
                &hook,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(DropPacket)
        );
    }

    #[test]
    fn transparent_proxy_terminal_for_entire_hook() {
        const TPROXY_PORT: NonZeroU16 = NonZeroU16::new(8080).unwrap();

        let ingress = Hook {
            routines: vec![
                Routine {
                    rules: vec![Rule::new(
                        PacketMatcher::default(),
                        Action::TransparentProxy(TransparentProxy::LocalPort(TPROXY_PORT)),
                    )],
                },
                Routine {
                    rules: vec![
                        // Accept all traffic.
                        Rule::new(PacketMatcher::default(), Action::Accept),
                    ],
                },
            ],
        };

        assert_eq!(
            check_routines_for_hook::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _, _>(
                &ingress,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            IngressVerdict::Stop(IngressStopReason::TransparentLocalDelivery {
                addr: <Ipv4 as crate::packets::testutil::internal::TestIpExt>::DST_IP,
                port: TPROXY_PORT
            })
        );
    }

    #[test]
    fn jump_recursively_evaluates_target_routine() {
        // Drop result from a target routine is propagated to the calling
        // routine.
        let routine = Routine {
            rules: vec![Rule::new(
                PacketMatcher::default(),
                Action::Jump(UninstalledRoutine::new(
                    vec![Rule::new(PacketMatcher::default(), Action::Drop)],
                    0,
                )),
            )],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Drop
        );

        // Accept result from a target routine is also propagated to the calling
        // routine.
        let routine = Routine {
            rules: vec![
                Rule::new(
                    PacketMatcher::default(),
                    Action::Jump(UninstalledRoutine::new(
                        vec![Rule::new(PacketMatcher::default(), Action::Accept)],
                        0,
                    )),
                ),
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Accept
        );

        // Return from a target routine results in continued evaluation of the
        // calling routine.
        let routine = Routine {
            rules: vec![
                Rule::new(
                    PacketMatcher::default(),
                    Action::Jump(UninstalledRoutine::new(
                        vec![Rule::new(PacketMatcher::default(), Action::Return)],
                        0,
                    )),
                ),
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };
        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Drop
        );
    }

    #[test]
    fn return_terminal_for_single_routine() {
        let routine = Routine {
            rules: vec![
                Rule::new(PacketMatcher::default(), Action::Return),
                // Drop all traffic.
                Rule::new(PacketMatcher::default(), Action::Drop),
            ],
        };

        assert_eq!(
            check_routine::<Ipv4, _, FakeMatcherDeviceId, FakeBindingsCtx<Ipv4>, _>(
                &routine,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            RoutineResult::Return
        );
    }

    #[ip_test(I)]
    fn filter_handler_implements_ip_hooks_correctly<I: TestIpExt>() {
        fn drop_all_traffic<I: TestIpExt>(
            matcher: PacketMatcher<I, FakeBindingsCtx<I>>,
        ) -> Hook<I, FakeBindingsCtx<I>, ()> {
            Hook { routines: vec![Routine { rules: vec![Rule::new(matcher, Action::Drop)] }] }
        }

        let mut bindings_ctx = FakeBindingsCtx::new();

        // Ingress hook should use ingress routines and check the input
        // interface.
        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                ingress: drop_all_traffic(PacketMatcher {
                    in_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            FilterImpl(&mut ctx).ingress_hook(
                &mut bindings_ctx,
                &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                &FakeMatcherDeviceId::wlan_interface(),
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(IngressStopReason::Drop)
        );

        // Local ingress hook should use local ingress routines and check the
        // input interface.
        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                local_ingress: drop_all_traffic(PacketMatcher {
                    in_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            FilterImpl(&mut ctx).local_ingress_hook(
                &mut bindings_ctx,
                &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                &FakeMatcherDeviceId::wlan_interface(),
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(DropOrReject::Drop)
        );

        // Forwarding hook should use forwarding routines and check both the
        // input and output interfaces.
        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                forwarding: drop_all_traffic(PacketMatcher {
                    in_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)),
                    out_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Ethernet)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            FilterImpl(&mut ctx).forwarding_hook(
                &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                &FakeMatcherDeviceId::wlan_interface(),
                &FakeMatcherDeviceId::ethernet_interface(),
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(DropOrReject::Drop)
        );

        // Local egress hook should use local egress routines and check the
        // output interface.
        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                local_egress: drop_all_traffic(PacketMatcher {
                    out_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            FilterImpl(&mut ctx).local_egress_hook(
                &mut bindings_ctx,
                &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                &FakeMatcherDeviceId::wlan_interface(),
                &mut FakePacketMetadata::default(),
            ),
            Verdict::Stop(DropOrReject::Drop)
        );

        // Egress hook should use egress routines and check the output
        // interface.
        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                egress: drop_all_traffic(PacketMatcher {
                    out_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Wlan)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            FilterImpl(&mut ctx)
                .egress_hook(
                    &mut bindings_ctx,
                    &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                    &FakeMatcherDeviceId::wlan_interface(),
                    &mut FakePacketMetadata::default(),
                )
                .0,
            Verdict::Stop(DropPacket)
        );
    }

    #[ip_test(I)]
    #[test_case(22 => Verdict::Proceed(Accept); "port 22 allowed for SSH")]
    #[test_case(80 => Verdict::Proceed(Accept); "port 80 allowed for HTTP")]
    #[test_case(1024 => Verdict::Proceed(Accept); "ephemeral port 1024 allowed")]
    #[test_case(65535 => Verdict::Proceed(Accept); "ephemeral port 65535 allowed")]
    #[test_case(1023 => Verdict::Stop(DropOrReject::Drop); "privileged port 1023 blocked")]
    #[test_case(53 => Verdict::Stop(DropOrReject::Drop); "privileged port 53 blocked")]
    fn block_privileged_ports_except_ssh_http<I: TestIpExt>(port: u16) -> Verdict<DropOrReject> {
        fn tcp_port_rule<I: FilterIpExt>(
            src_port: Option<PortMatcher>,
            dst_port: Option<PortMatcher>,
            action: Action<I, FakeBindingsCtx<I>, ()>,
        ) -> Rule<I, FakeBindingsCtx<I>, ()> {
            Rule::new(
                PacketMatcher {
                    transport_protocol: Some(TransportProtocolMatcher {
                        proto: <&FakeTcpSegment as TransportPacketExt<I>>::proto().unwrap(),
                        src_port,
                        dst_port,
                    }),
                    ..Default::default()
                },
                action,
            )
        }

        fn default_filter_rules<I: FilterIpExt>() -> Routine<I, FakeBindingsCtx<I>, ()> {
            Routine {
                rules: vec![
                    // pass in proto tcp to port 22;
                    tcp_port_rule(
                        /* src_port */ None,
                        Some(PortMatcher { range: 22..=22, invert: false }),
                        Action::Accept,
                    ),
                    // pass in proto tcp to port 80;
                    tcp_port_rule(
                        /* src_port */ None,
                        Some(PortMatcher { range: 80..=80, invert: false }),
                        Action::Accept,
                    ),
                    // pass in proto tcp to range 1024:65535;
                    tcp_port_rule(
                        /* src_port */ None,
                        Some(PortMatcher { range: 1024..=65535, invert: false }),
                        Action::Accept,
                    ),
                    // drop in proto tcp to range 1:6553;
                    tcp_port_rule(
                        /* src_port */ None,
                        Some(PortMatcher { range: 1..=65535, invert: false }),
                        Action::Drop,
                    ),
                ],
            }
        }

        let mut bindings_ctx = FakeBindingsCtx::new();

        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                local_ingress: Hook { routines: vec![default_filter_rules()] },
                ..Default::default()
            },
        );

        FilterImpl(&mut ctx).local_ingress_hook(
            &mut bindings_ctx,
            &mut FakeIpPacket::<I, _> {
                body: FakeTcpSegment {
                    dst_port: port,
                    src_port: 11111,
                    segment: SegmentHeader::arbitrary_value(),
                    payload_len: 8888,
                },
                ..ArbitraryValue::arbitrary_value()
            },
            &FakeMatcherDeviceId::wlan_interface(),
            &mut FakePacketMetadata::default(),
        )
    }

    #[ip_test(I)]
    #[test_case(
        FakeMatcherDeviceId::ethernet_interface() => Verdict::Proceed(Accept);
        "allow incoming traffic on ethernet interface"
    )]
    #[test_case(
        FakeMatcherDeviceId::wlan_interface() => Verdict::Stop(DropOrReject::Drop);
        "drop incoming traffic on wlan interface"
    )]
    fn filter_on_wlan_only<I: TestIpExt>(interface: FakeMatcherDeviceId) -> Verdict<DropOrReject> {
        fn drop_wlan_traffic<I: IpExt>() -> Routine<I, FakeBindingsCtx<I>, ()> {
            Routine {
                rules: vec![Rule::new(
                    PacketMatcher {
                        in_interface: Some(InterfaceMatcher::Id(
                            FakeMatcherDeviceId::wlan_interface().id,
                        )),
                        ..Default::default()
                    },
                    Action::Drop,
                )],
            }
        }

        let mut bindings_ctx = FakeBindingsCtx::new();

        let mut ctx = FakeCtx::with_ip_routines(
            &mut bindings_ctx,
            IpRoutines {
                local_ingress: Hook { routines: vec![drop_wlan_traffic()] },
                ..Default::default()
            },
        );

        FilterImpl(&mut ctx).local_ingress_hook(
            &mut bindings_ctx,
            &mut FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
            &interface,
            &mut FakePacketMetadata::default(),
        )
    }

    #[test]
    fn ingress_reuses_cached_connection_when_available() {
        let mut bindings_ctx = FakeBindingsCtx::new();
        let mut core_ctx = FakeCtx::new(&mut bindings_ctx);

        // When a connection is finalized in the EGRESS hook, it should stash a shared
        // reference to the connection in the packet metadata.
        let mut packet = FakeIpPacket::<Ipv4, FakeUdpPacket>::arbitrary_value();
        let mut metadata = PacketMetadata::default();
        let (verdict, _proof) = FilterImpl(&mut core_ctx).egress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));

        // The stashed reference should point to the connection that is in the table.
        let (stashed, _dir) =
            metadata.take_connection_and_direction().expect("metadata should include connection");
        let tuple = packet.conntrack_packet().expect("packet should be trackable").tuple();
        let table = core_ctx
            .conntrack()
            .get_connection(&tuple)
            .expect("packet should be inserted in table");
        assert_matches!(
            (table, stashed),
            (Connection::Shared(table), Connection::Shared(stashed)) => {
                assert!(Arc::ptr_eq(&table, &stashed));
            }
        );

        // Provided with the connection, the INGRESS hook should reuse it rather than
        // creating a new one.
        let verdict = FilterImpl(&mut core_ctx).ingress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));

        // As a result, rather than there being a new connection in the packet metadata,
        // it should contain the same connection that is still in the table.
        let (after_ingress, _dir) =
            metadata.take_connection_and_direction().expect("metadata should include connection");
        let table = core_ctx
            .conntrack()
            .get_connection(&tuple)
            .expect("packet should be inserted in table");
        assert_matches!(
            (table, after_ingress),
            (Connection::Shared(before), Connection::Shared(after)) => {
                assert!(Arc::ptr_eq(&before, &after));
            }
        );
    }

    #[ip_test(I)]
    fn drop_packet_on_finalize_connection_failure<I: TestIpExt>() {
        let mut bindings_ctx = FakeBindingsCtx::new();
        let mut ctx = FakeCtx::new(&mut bindings_ctx);

        for i in 0..u32::try_from(conntrack::MAXIMUM_ENTRIES / 2).unwrap() {
            let (mut packet, mut reply_packet) = conntrack::testutils::make_test_udp_packets(i);
            let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
                &mut bindings_ctx,
                &mut packet,
                &FakeMatcherDeviceId::ethernet_interface(),
                &mut FakePacketMetadata::default(),
            );
            assert_eq!(verdict, Verdict::Proceed(Accept));

            let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
                &mut bindings_ctx,
                &mut reply_packet,
                &FakeMatcherDeviceId::ethernet_interface(),
                &mut FakePacketMetadata::default(),
            );
            assert_eq!(verdict, Verdict::Proceed(Accept));

            let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
                &mut bindings_ctx,
                &mut packet,
                &FakeMatcherDeviceId::ethernet_interface(),
                &mut FakePacketMetadata::default(),
            );
            assert_eq!(verdict, Verdict::Proceed(Accept));
        }

        // Finalizing the connection should fail when the conntrack table is at maximum
        // capacity and there are no connections to remove, because all existing
        // connections are considered established.
        let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
            &mut bindings_ctx,
            &mut FakeIpPacket::<I, FakeUdpPacket>::arbitrary_value(),
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut FakePacketMetadata::default(),
        );
        assert_eq!(verdict, Verdict::Stop(DropPacket));
    }

    #[ip_test(I)]
    fn implicit_snat_to_prevent_tuple_clash<I: TestIpExt>() {
        let mut bindings_ctx = FakeBindingsCtx::new();
        let mut ctx = FakeCtx::with_nat_routines_and_device_addrs(
            &mut bindings_ctx,
            NatRoutines {
                egress: Hook {
                    routines: vec![Routine {
                        rules: vec![Rule::new(
                            PacketMatcher {
                                src_address: Some(AddressMatcher {
                                    matcher: AddressMatcherType::Range(I::SRC_IP_2..=I::SRC_IP_2),
                                    invert: false,
                                }),
                                ..Default::default()
                            },
                            Action::Masquerade { src_port: None },
                        )],
                    }],
                },
                ..Default::default()
            },
            HashMap::from([(
                FakeMatcherDeviceId::ethernet_interface(),
                AddrSubnet::new(I::SRC_IP, I::SUBNET.prefix()).unwrap(),
            )]),
        );

        // Simulate a forwarded packet, originally from I::SRC_IP_2, that is masqueraded
        // to be from I::SRC_IP. The packet should have had SNAT performed.
        let mut packet = FakeIpPacket {
            src_ip: I::SRC_IP_2,
            dst_ip: I::DST_IP,
            body: FakeUdpPacket::arbitrary_value(),
        };
        let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut FakePacketMetadata::default(),
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));
        assert_eq!(packet.src_ip, I::SRC_IP);

        // Now simulate a locally-generated packet that conflicts with this flow; it is
        // from I::SRC_IP to I::DST_IP and has the same source and destination ports.
        // Finalizing the connection would typically fail, causing the packet to be
        // dropped, because the reply tuple conflicts with the reply tuple of the
        // masqueraded flow. So instead this new flow is implicitly SNATed to a free
        // port and the connection should be successfully finalized.
        let mut packet = FakeIpPacket::<I, FakeUdpPacket>::arbitrary_value();
        let src_port = packet.body.src_port;
        let (verdict, _proof) = FilterImpl(&mut ctx).egress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut FakePacketMetadata::default(),
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));
        assert_ne!(packet.body.src_port, src_port);
    }

    #[ip_test(I)]
    fn packet_adopts_tracked_connection_in_table_if_identical<I: TestIpExt>() {
        let mut bindings_ctx = FakeBindingsCtx::new();
        let mut core_ctx = FakeCtx::new(&mut bindings_ctx);

        // Simulate a race where two packets in the same flow both end up
        // creating identical exclusive connections.
        let mut first_packet = FakeIpPacket::<I, FakeUdpPacket>::arbitrary_value();
        let mut first_metadata = PacketMetadata::default();
        let verdict = FilterImpl(&mut core_ctx).local_egress_hook(
            &mut bindings_ctx,
            &mut first_packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut first_metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));

        let mut second_packet = FakeIpPacket::<I, FakeUdpPacket>::arbitrary_value();
        let mut second_metadata = PacketMetadata::default();
        let verdict = FilterImpl(&mut core_ctx).local_egress_hook(
            &mut bindings_ctx,
            &mut second_packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut second_metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));

        // Finalize the first connection; it should get inserted in the table.
        let (verdict, _proof) = FilterImpl(&mut core_ctx).egress_hook(
            &mut bindings_ctx,
            &mut first_packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut first_metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));

        // The second packet conflicts with the connection that's in the table, but it's
        // identical to the first one, so it should adopt the finalized connection.
        let (verdict, _proof) = FilterImpl(&mut core_ctx).egress_hook(
            &mut bindings_ctx,
            &mut second_packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut second_metadata,
        );
        assert_eq!(second_packet.body.src_port, first_packet.body.src_port);
        assert_eq!(verdict, Verdict::Proceed(Accept));

        let (first_conn, _dir) = first_metadata.take_connection_and_direction().unwrap();
        let (second_conn, _dir) = second_metadata.take_connection_and_direction().unwrap();
        assert_matches!(
            (first_conn, second_conn),
            (Connection::Shared(first), Connection::Shared(second)) => {
                assert!(Arc::ptr_eq(&first, &second));
            }
        );
    }

    #[ip_test(I)]
    fn both_source_and_destination_nat_configured<I: TestIpExt>() {
        let mut bindings_ctx = FakeBindingsCtx::new();
        // Install NAT rules to perform both DNAT (in LOCAL_EGRESS) and SNAT (in
        // EGRESS).
        let mut core_ctx = FakeCtx::with_nat_routines_and_device_addrs(
            &mut bindings_ctx,
            NatRoutines {
                local_egress: Hook {
                    routines: vec![Routine {
                        rules: vec![Rule::new(
                            PacketMatcher::default(),
                            Action::Redirect { dst_port: None },
                        )],
                    }],
                },
                egress: Hook {
                    routines: vec![Routine {
                        rules: vec![Rule::new(
                            PacketMatcher::default(),
                            Action::Masquerade { src_port: None },
                        )],
                    }],
                },
                ..Default::default()
            },
            HashMap::from([(
                FakeMatcherDeviceId::ethernet_interface(),
                AddrSubnet::new(I::SRC_IP_2, I::SUBNET.prefix()).unwrap(),
            )]),
        );

        // Even though the packet is modified after the first hook, where DNAT is
        // configured...
        let mut packet = FakeIpPacket::<I, FakeUdpPacket>::arbitrary_value();
        let mut metadata = PacketMetadata::default();
        let verdict = FilterImpl(&mut core_ctx).local_egress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));
        assert_eq!(packet.dst_ip, *I::LOOPBACK_ADDRESS);

        // ...SNAT is also successfully configured for the packet, because the packet's
        // [`ConnectionDirection`] is cached in the metadata.
        let (verdict, _proof) = FilterImpl(&mut core_ctx).egress_hook(
            &mut bindings_ctx,
            &mut packet,
            &FakeMatcherDeviceId::ethernet_interface(),
            &mut metadata,
        );
        assert_eq!(verdict, Verdict::Proceed(Accept));
        assert_eq!(packet.src_ip, I::SRC_IP_2);
    }

    #[ip_test(I)]
    #[test_case(
        Hook {
            routines: vec![
                Routine {
                    rules: vec![
                        Rule::new(
                            PacketMatcher::default(),
                            Action::Mark {
                                domain: MarkDomain::Mark1,
                                action: MarkAction::SetMark { clearing_mask: 0, mark: 1 },
                            },
                        ),
                        Rule::new(PacketMatcher::default(), Action::Drop),
                    ],
                },
            ],
        }; "non terminal for routine"
    )]
    #[test_case(
        Hook {
            routines: vec![
                Routine {
                    rules: vec![Rule::new(
                        PacketMatcher::default(),
                        Action::Mark {
                            domain: MarkDomain::Mark1,
                            action: MarkAction::SetMark { clearing_mask: 0, mark: 1 },
                        },
                    )],
                },
                Routine {
                    rules: vec![
                        Rule::new(PacketMatcher::default(), Action::Drop),
                    ],
                },
            ],
        }; "non terminal for hook"
    )]
    fn mark_action<I: TestIpExt>(ingress: Hook<I, FakeBindingsCtx<I>, ()>) {
        let mut metadata = PacketMetadata::<I, FakeWeakAddressId<I>, FakeBindingsCtx<I>>::default();
        assert_eq!(
            check_routines_for_hook::<I, _, FakeMatcherDeviceId, FakeBindingsCtx<I>, _, _>(
                &ingress,
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut metadata,
            ),
            IngressVerdict::Stop(IngressStopReason::Drop),
        );
        assert_eq!(metadata.marks, Marks::new([(MarkDomain::Mark1, 1)]));
    }

    #[ip_test(I)]
    fn mark_action_applied_in_succession<I: TestIpExt>() {
        fn hook_with_single_mark_action<I: TestIpExt>(
            domain: MarkDomain,
            action: MarkAction,
        ) -> Hook<I, FakeBindingsCtx<I>, ()> {
            Hook {
                routines: vec![Routine {
                    rules: vec![Rule::new(
                        PacketMatcher::default(),
                        Action::Mark { domain, action },
                    )],
                }],
            }
        }
        let mut metadata = PacketMetadata::<I, FakeWeakAddressId<I>, FakeBindingsCtx<I>>::default();
        assert_eq!(
            check_routines_for_hook::<I, _, FakeMatcherDeviceId, FakeBindingsCtx<I>, _, _>(
                &hook_with_single_mark_action(
                    MarkDomain::Mark1,
                    MarkAction::SetMark { clearing_mask: 0, mark: 1 }
                ),
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &mut metadata,
            ),
            IngressVerdict::Proceed(Accept),
        );
        assert_eq!(metadata.marks, Marks::new([(MarkDomain::Mark1, 1)]));

        assert_eq!(
            check_routines_for_hook(
                &hook_with_single_mark_action::<I>(
                    MarkDomain::Mark2,
                    MarkAction::SetMark { clearing_mask: 0, mark: 1 }
                ),
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces::<FakeMatcherDeviceId> { ingress: None, egress: None },
                &mut metadata,
            ),
            IngressVerdict::Proceed(Accept)
        );
        assert_eq!(metadata.marks, Marks::new([(MarkDomain::Mark1, 1), (MarkDomain::Mark2, 1)]));

        assert_eq!(
            check_routines_for_hook(
                &hook_with_single_mark_action::<I>(
                    MarkDomain::Mark1,
                    MarkAction::SetMark { clearing_mask: 1, mark: 2 }
                ),
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces::<FakeMatcherDeviceId> { ingress: None, egress: None },
                &mut metadata,
            ),
            IngressVerdict::Proceed(Accept)
        );
        assert_eq!(metadata.marks, Marks::new([(MarkDomain::Mark1, 2), (MarkDomain::Mark2, 1)]));
    }

    // Regression test for https://fxbug.dev/517102537.
    #[ip_test(I)]
    fn transparent_proxy_drop_on_port_0<I: TestIpExt>() {
        let ingress = Hook {
            routines: vec![Routine {
                rules: vec![Rule::new(
                    PacketMatcher::default(),
                    Action::TransparentProxy(TransparentProxy::LocalAddr(I::DST_IP)),
                )],
            }],
        };

        let packet = FakeIpPacket::<I, FakeTcpSegment> {
            body: FakeTcpSegment {
                dst_port: 0,
                src_port: 11111,
                segment: SegmentHeader::arbitrary_value(),
                payload_len: 0,
            },
            ..FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value()
        };

        assert_eq!(
            check_routines_for_hook::<
                I,
                _,
                FakeMatcherDeviceId,
                FakeBindingsCtx<I>,
                _,
                IngressStopReason<I>,
            >(
                &ingress,
                &packet,
                Interfaces { ingress: None, egress: None },
                &mut FakePacketMetadata::default(),
            ),
            IngressVerdict::Stop(IngressStopReason::Drop),
        );
    }
}
