// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Neighbor API structs.

use core::fmt::Display;
use core::marker::PhantomData;

use log::warn;
use net_types::ip::{Ip, IpAddress, IpVersionMarker, Ipv4, Ipv6};
use net_types::{SpecifiedAddr, UnicastAddr, UnicastAddress as _, Witness as _};
use netstack3_base::{
    ContextPair, DeviceIdContext, EventContext as _, Inspector, InstantContext as _, LinkDevice,
    NotFoundError,
};
use thiserror::Error;

use crate::internal::device::nud::{
    Delay, DynamicNeighborState, EnterProbeError, Entry, Event, Incomplete, LinkResolutionContext,
    LinkResolutionNotifier, LinkResolutionResult, NeighborState, NudBindingsContext, NudContext,
    NudHandler, NudState, Probe, Reachable, Stale, TableFullError, Unreachable,
};

/// Error when a static neighbor entry cannot be inserted.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum StaticNeighborInsertionError {
    /// The IP address is invalid as the address of a neighbor. A valid address
    /// is:
    /// - specified,
    /// - not multicast,
    /// - not loopback,
    /// - not an IPv4-mapped address, and
    /// - not the limited broadcast address of `255.255.255.255`.
    #[error("IP address is invalid")]
    IpAddressInvalid,

    /// The neighbor table is full and the entry cannot be added.
    #[error("The neighbor table is full")]
    TableFull,
}

/// Error when a probe cannot be triggered on a neighbor.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum TriggerNeighborProbeError {
    /// The IP address is invalid as the address of a neighbor.
    #[error("IP address is invalid")]
    IpAddressInvalid,

    /// Entry cannot be found.
    #[error(transparent)]
    NotFound(#[from] NotFoundError),

    /// The link address of the neighbor is unknown.
    #[error("link address is unknown")]
    LinkAddressUnknown,
}

/// Error when a neighbor table entry cannot be removed.
#[derive(Debug, PartialEq, Eq, Error)]
pub enum NeighborRemovalError {
    /// The IP address is invalid as the address of a neighbor.
    #[error("IP address is invalid")]
    IpAddressInvalid,

    /// Entry cannot be found.
    #[error(transparent)]
    NotFound(#[from] NotFoundError),
}

// TODO(https://fxbug.dev/42083952): Use NeighborAddr to witness these properties.
fn validate_neighbor_addr<A: IpAddress>(addr: A) -> Option<SpecifiedAddr<A>> {
    let is_valid: bool = A::Version::map_ip(
        addr,
        |v4| {
            !Ipv4::LOOPBACK_SUBNET.contains(&v4)
                && !Ipv4::MULTICAST_SUBNET.contains(&v4)
                && v4 != Ipv4::LIMITED_BROADCAST_ADDRESS.get()
        },
        |v6| v6 != Ipv6::LOOPBACK_ADDRESS.get() && v6.to_ipv4_mapped().is_none() && v6.is_unicast(),
    );
    is_valid.then_some(()).and_then(|()| SpecifiedAddr::new(addr))
}

/// The neighbor API.
pub struct NeighborApi<I: Ip, D, C>(C, IpVersionMarker<I>, PhantomData<D>);

impl<I: Ip, D, C> NeighborApi<I, D, C> {
    /// Creates a new API instance.
    pub fn new(ctx: C) -> Self {
        Self(ctx, IpVersionMarker::new(), PhantomData)
    }
}

impl<I, D, C> NeighborApi<I, D, C>
where
    I: Ip,
    D: LinkDevice,
    C: ContextPair,
    C::CoreContext: NudContext<I, D, C::BindingsContext>,
    C::BindingsContext: NudBindingsContext<I, D, <C::CoreContext as DeviceIdContext<D>>::DeviceId>,
{
    fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, IpVersionMarker { .. }, PhantomData) = self;
        pair.core_ctx()
    }

    fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, IpVersionMarker { .. }, PhantomData) = self;
        pair.contexts()
    }

    /// Resolve the link-address for a given device's neighbor.
    ///
    /// Lookup the given destination IP address in the neighbor table for given
    /// device, returning either the associated link-address if it is available,
    /// or an observer that can be used to wait for link address resolution to
    /// complete.
    pub fn resolve_link_addr(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
    // TODO(https://fxbug.dev/42076887): Use IPv4 subnet information to
    // disallow subnet and subnet broadcast addresses.
    // TODO(https://fxbug.dev/42083952): Use NeighborAddr when available.
        dst: &SpecifiedAddr<I::Addr>,
    ) -> LinkResolutionResult<
        UnicastAddr<D::Address>,
        <<C::BindingsContext as LinkResolutionContext<D>>::Notifier as LinkResolutionNotifier<
            D,
        >>::Observer,
    >{
        let (core_ctx, bindings_ctx) = self.contexts();
        let (result, do_multicast_solicit) = core_ctx.with_nud_state_mut(
            device_id,
            |NudState { neighbors, gc_state, timer_heap }, core_ctx| match neighbors.get_mut(dst) {
                None => {
                    // Initiate link resolution.
                    let (notifier, observer) =
                        <C::BindingsContext as LinkResolutionContext<D>>::Notifier::new();
                    let neighbor = NeighborState::Dynamic(DynamicNeighborState::Incomplete(
                        Incomplete::new_with_notifier(
                            core_ctx,
                            bindings_ctx,
                            timer_heap,
                            *dst,
                            notifier,
                        ),
                    ));
                    let result = crate::internal::device::nud::insert_new_entry(
                        neighbors,
                        gc_state,
                        timer_heap,
                        bindings_ctx,
                        device_id,
                        *dst,
                        neighbor,
                    );
                    match result {
                        Ok(_entry) => (LinkResolutionResult::Pending(observer), true),
                        Err(TableFullError { entry }) => {
                            warn!("Neighbor table full; failed to insert {entry:?}");
                            let (notifier, observer) =
                                <C::BindingsContext as LinkResolutionContext<D>>::Notifier::new();
                            notifier.notify(Err(netstack3_base::AddressResolutionFailed));
                            (LinkResolutionResult::Pending(observer), false)
                        }
                    }
                }
                Some(entry) => match entry {
                    NeighborState::Static(link_address) => {
                        (LinkResolutionResult::Resolved(*link_address), false)
                    }
                    NeighborState::Dynamic(e) => {
                        e.resolve_link_addr(core_ctx, bindings_ctx, timer_heap, device_id, *dst)
                    }
                },
            },
        );

        if do_multicast_solicit {
            core_ctx.send_neighbor_solicitation(
                bindings_ctx,
                &device_id,
                *dst,
                /* multicast */ None,
            );
        }

        result
    }

    /// Flush neighbor table entries.
    pub fn flush_table(&mut self, device: &<C::CoreContext as DeviceIdContext<D>>::DeviceId) {
        let (core_ctx, bindings_ctx) = self.contexts();
        NudHandler::<I, D, _>::flush(core_ctx, bindings_ctx, device)
    }

    /// Sets a static neighbor entry for the neighbor.
    ///
    /// If no entry exists, a new one may be created. If an entry already
    /// exists, it will be updated with the provided link address and set to be
    /// a static entry.
    ///
    /// Dynamic updates for the neighbor will be ignored for static entries.
    pub fn insert_static_entry(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        neighbor: I::Addr,
        // TODO(https://fxbug.dev/42076887): Use IPv4 subnet information to
        // disallow the address with all host bits equal to 0, and the
        // subnet broadcast addresses with all host bits equal to 1.
        // TODO(https://fxbug.dev/42083952): Use NeighborAddr when available.
        link_address: UnicastAddr<D::Address>,
    ) -> Result<(), StaticNeighborInsertionError> {
        let neighbor = validate_neighbor_addr(neighbor)
            .ok_or(StaticNeighborInsertionError::IpAddressInvalid)?;
        let (core_ctx, bindings_ctx) = self.contexts();

        core_ctx.with_nud_state_mut_and_sender_ctx(
            device_id,
            |NudState { neighbors, gc_state, timer_heap }, core_ctx| match neighbors
                .get_mut(&neighbor)
            {
                Some(entry) => {
                    let previous = core::mem::replace(entry, NeighborState::Static(link_address));
                    let event_state = entry.to_event_state();
                    if event_state != previous.to_event_state() {
                        bindings_ctx.on_event(Event::changed(
                            device_id,
                            event_state,
                            neighbor,
                            bindings_ctx.now(),
                        ));
                    }
                    match previous {
                        NeighborState::Dynamic(entry) => {
                            entry.cancel_timer_and_complete_resolution(
                                core_ctx,
                                bindings_ctx,
                                timer_heap,
                                neighbor,
                                link_address,
                            );
                        }
                        NeighborState::Static(_) => {}
                    }
                    Ok(())
                }
                None => {
                    let neighbor_state = NeighborState::Static(link_address);
                    let result = crate::internal::device::nud::insert_new_entry(
                        neighbors,
                        gc_state,
                        timer_heap,
                        bindings_ctx,
                        device_id,
                        neighbor,
                        neighbor_state,
                    );
                    match result {
                        Ok(_entry) => Ok(()),
                        Err(TableFullError { entry }) => {
                            warn!("Neighbor table full; failed to insert {entry:?}");
                            Err(StaticNeighborInsertionError::TableFull)
                        }
                    }
                }
            },
        )
    }

    /// Immediately triggers a unicast probe to be sent to `neighbor`.
    ///
    /// For IPv6, this probe is an NDP Neighbor Solicitation, while for IPv4
    /// it's an ARP Request.
    ///
    /// Returns an error if the probe was not sent, unless the neighbor was
    /// already in the Probe state, in which case it succeeds without sending
    /// another probe.
    pub fn probe_entry(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        neighbor: I::Addr,
    ) -> Result<(), TriggerNeighborProbeError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        let neighbor =
            validate_neighbor_addr(neighbor).ok_or(TriggerNeighborProbeError::IpAddressInvalid)?;

        let probe_to_send = core_ctx.with_nud_state_mut(device_id, |nud_state, config_ctx| {
            let mut neighbor_state = match nud_state.neighbors.entry(neighbor) {
                Entry::Occupied(neighbor_state) => neighbor_state,
                Entry::Vacant(_) => return Err(TriggerNeighborProbeError::NotFound(NotFoundError)),
            };
            neighbor_state
                .get_mut()
                .enter_probe(
                    config_ctx,
                    bindings_ctx,
                    &mut nud_state.timer_heap,
                    neighbor,
                    device_id,
                )
                .map_err(|e| match e {
                    EnterProbeError::LinkAddressUnknown => {
                        TriggerNeighborProbeError::LinkAddressUnknown
                    }
                })
        })?;
        match probe_to_send {
            Some(link_addr) => core_ctx.send_neighbor_solicitation(
                bindings_ctx,
                &device_id,
                neighbor,
                Some(link_addr),
            ),
            None => {}
        }
        Ok(())
    }

    /// Remove a static or dynamic neighbor table entry.
    pub fn remove_entry(
        &mut self,
        device_id: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        // TODO(https://fxbug.dev/42076887): Use IPv4 subnet information to
        // disallow the address with all host bits equal to 0, and the
        // subnet broadcast addresses with all host bits equal to 1.
        // TODO(https://fxbug.dev/42083952): Use NeighborAddr when available.
        neighbor: I::Addr,
    ) -> Result<(), NeighborRemovalError> {
        let (core_ctx, bindings_ctx) = self.contexts();
        let neighbor =
            validate_neighbor_addr(neighbor).ok_or(NeighborRemovalError::IpAddressInvalid)?;

        core_ctx.with_nud_state_mut(
            device_id,
            |NudState { neighbors, gc_state: _, timer_heap }, _config| {
                match neighbors.remove(&neighbor).ok_or(NotFoundError)? {
                    NeighborState::Dynamic(mut entry) => {
                        entry.cancel_timer(bindings_ctx, timer_heap, neighbor);
                    }
                    NeighborState::Static(_) => {}
                }
                bindings_ctx.on_event(Event::removed(device_id, neighbor, bindings_ctx.now()));
                Ok(())
            },
        )
    }

    /// Writes `device`'s neighbor state information into `inspector`.
    pub fn inspect_neighbors<N: Inspector>(
        &mut self,
        device: &<C::CoreContext as DeviceIdContext<D>>::DeviceId,
        inspector: &mut N,
    ) where
        D::Address: Display,
    {
        self.core_ctx().with_nud_state(device, |nud| {
            nud.neighbors.iter().for_each(|(ip_address, state)| {
                let (state, link_address, last_confirmed_at) = match state {
                    NeighborState::Static(addr) => ("Static", Some(addr), None),
                    NeighborState::Dynamic(dynamic_state) => match dynamic_state {
                        DynamicNeighborState::Incomplete(Incomplete {
                            transmit_counter: _,
                            pending_frames: _,
                            notifiers: _,
                            _marker,
                        }) => ("Incomplete", None, None),
                        DynamicNeighborState::Reachable(Reachable {
                            link_address,
                            last_confirmed_at,
                        }) => ("Reachable", Some(link_address), Some(last_confirmed_at)),
                        DynamicNeighborState::Stale(Stale { link_address }) => {
                            ("Stale", Some(link_address), None)
                        }
                        DynamicNeighborState::Delay(Delay { link_address }) => {
                            ("Delay", Some(link_address), None)
                        }
                        DynamicNeighborState::Probe(Probe {
                            link_address,
                            transmit_counter: _,
                        }) => ("Probe", Some(link_address), None),
                        DynamicNeighborState::Unreachable(Unreachable {
                            link_address,
                            mode: _,
                        }) => ("Unreachable", Some(link_address), None),
                    },
                };
                inspector.record_unnamed_child(|inspector| {
                    inspector.record_str("State", state);
                    inspector.record_ip_addr("IpAddress", ip_address.get());
                    if let Some(link_address) = link_address {
                        inspector.record_display("LinkAddress", link_address);
                    };
                    if let Some(last_confirmed_at) = last_confirmed_at {
                        inspector.record_inspectable_value("LastConfirmedAt", last_confirmed_at);
                    }
                });
            })
        })
    }
}
