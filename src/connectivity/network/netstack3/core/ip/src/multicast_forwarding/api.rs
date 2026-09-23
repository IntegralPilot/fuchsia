// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Declares the API for configuring multicast forwarding within the netstack.

use alloc::collections::btree_map;
use core::sync::atomic::Ordering;

use log::warn;
use net_types::SpecifiedAddr;
use net_types::ip::{Ip, IpVersionMarker};
use netstack3_base::{
    AnyDevice, AtomicInstant, ContextPair, CoreTimerContext, CounterContext, DeviceIdContext,
    Inspector, InspectorDeviceExt, InstantBindingsTypes, InstantContext, StrongDeviceIdentifier,
    WeakDeviceIdentifier,
};

use crate::internal::base::IpLayerForwardingContext;
use crate::internal::multicast_forwarding::counters::MulticastForwardingCounters;
use crate::internal::multicast_forwarding::packet_queue::{PacketQueue, QueuedPacket};
use crate::internal::multicast_forwarding::route::{
    Action, MulticastRoute, MulticastRouteEntry, MulticastRouteKey, MulticastRouteStats,
    MulticastRouteTarget,
};
use crate::internal::multicast_forwarding::state::{
    MulticastForwardingEnabledState, MulticastForwardingPendingPacketsContext as _,
    MulticastForwardingState, MulticastForwardingStateContext, MulticastRouteTableContext as _,
};
use crate::internal::multicast_forwarding::{
    MulticastForwardingBindingsTypes, MulticastForwardingDeviceContext, MulticastForwardingEvent,
    MulticastForwardingTimerId,
};
use crate::{IpLayerBindingsContext, IpLayerIpExt, IpPacketDestination};

/// The API action can not be performed while multicast forwarding is disabled.
#[derive(Debug, Eq, PartialEq)]
pub struct MulticastForwardingDisabledError {}

trait MulticastForwardingStateExt<
    I: IpLayerIpExt,
    D: StrongDeviceIdentifier,
    BT: MulticastForwardingBindingsTypes,
>
{
    fn try_enabled(
        &self,
    ) -> Result<&MulticastForwardingEnabledState<I, D, BT>, MulticastForwardingDisabledError>;
}

impl<I: IpLayerIpExt, D: StrongDeviceIdentifier, BT: MulticastForwardingBindingsTypes>
    MulticastForwardingStateExt<I, D, BT> for MulticastForwardingState<I, D, BT>
{
    fn try_enabled(
        &self,
    ) -> Result<&MulticastForwardingEnabledState<I, D, BT>, MulticastForwardingDisabledError> {
        self.enabled().ok_or(MulticastForwardingDisabledError {})
    }
}

/// The multicast forwarding API.
pub struct MulticastForwardingApi<I: Ip, C> {
    ctx: C,
    _ip_mark: IpVersionMarker<I>,
}

impl<I: Ip, C> MulticastForwardingApi<I, C> {
    /// Constructs a new multicast forwarding API.
    pub fn new(ctx: C) -> Self {
        Self { ctx, _ip_mark: IpVersionMarker::new() }
    }
}

impl<I: IpLayerIpExt, C> MulticastForwardingApi<I, C>
where
    C: ContextPair,
    C::CoreContext: MulticastForwardingStateContext<I, C::BindingsContext>
        + MulticastForwardingDeviceContext<I>
        + IpLayerForwardingContext<I, C::BindingsContext>
        + CounterContext<MulticastForwardingCounters<I>>
        + CoreTimerContext<MulticastForwardingTimerId<I>, C::BindingsContext>,
    C::BindingsContext:
        IpLayerBindingsContext<I, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
{
    pub(crate) fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self { ctx, _ip_mark } = self;
        ctx.core_ctx()
    }

    pub(crate) fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self { ctx, _ip_mark } = self;
        ctx.contexts()
    }

    /// Enables multicast forwarding.
    ///
    /// Returns whether multicast forwarding was newly enabled.
    pub fn enable(&mut self) -> bool {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.with_state_mut(|state, _ctx| match state {
            MulticastForwardingState::Enabled(_) => false,
            MulticastForwardingState::Disabled => {
                *state = MulticastForwardingState::Enabled(MulticastForwardingEnabledState::new::<
                    C::CoreContext,
                >(bindings_ctx));
                true
            }
        })
    }

    /// Disables multicast forwarding.
    ///
    /// Returns whether multicast forwarding was newly disabled.
    ///
    /// Upon being disabled, the multicast route table will be cleared,
    /// and all pending packets will be dropped.
    pub fn disable(&mut self) -> bool {
        self.core_ctx().with_state_mut(|state, _ctx| match state {
            MulticastForwardingState::Disabled => false,
            MulticastForwardingState::Enabled(_) => {
                *state = MulticastForwardingState::Disabled;
                true
            }
        })
    }

    /// Add the route to the multicast route table.
    ///
    /// If a route already exists with the same key, it will be replaced, and
    /// the original route will be returned.
    pub fn add_multicast_route(
        &mut self,
        key: MulticastRouteKey<I>,
        route: MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    ) -> Result<
        Option<MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>>,
        MulticastForwardingDisabledError,
    > {
        let (core_ctx, bindings_ctx) = self.contexts();
        let (orig_route, packet_queue_and_new_route) = core_ctx.with_state_mut(|state, ctx| {
            let state = state.try_enabled()?;
            ctx.with_route_table_mut(state, |route_table, ctx| {
                let stats = MulticastRouteStats { last_used: bindings_ctx.now_atomic() };
                match route_table.entry(key.clone()) {
                    btree_map::Entry::Occupied(mut entry) => {
                        // NB: We consider the stats to be associated with the
                        // `route` rather than the route's key. As such we
                        // replace the stats instead of preserving them.
                        let MulticastRouteEntry { route: orig_route, stats: _ } =
                            entry.insert(MulticastRouteEntry { route, stats });
                        // NB: Check the invariant that any key present in the
                        // route table is not also present in the pending table.
                        #[cfg(debug_assertions)]
                        ctx.with_pending_table_mut(state, |pending_table| {
                            debug_assert!(!pending_table.contains(&key));
                        });
                        Ok((Some(orig_route), None))
                    }
                    btree_map::Entry::Vacant(entry) => {
                        let MulticastRouteEntry { route: new_route_ref, stats: _ } =
                            entry.insert(MulticastRouteEntry { route, stats });
                        let packet_queue_and_new_route = ctx
                            .with_pending_table_mut(state, |pending_table| {
                                pending_table.remove(&key, bindings_ctx)
                            })
                            .map(|packet_queue| (packet_queue, new_route_ref.clone()));
                        Ok((None, packet_queue_and_new_route))
                    }
                }
            })
        })?;

        if let Some((packet_queue, new_route)) = packet_queue_and_new_route {
            // NB: we cloned the route out to a context that's no longer holding
            // the routing table lock. This means the route could have been
            // removed. In general, that's okay. We'll operate on the
            // potentially stale route as if it still exists. This mirrors the
            // lookup pattern used by the unicast/multicast route tables in
            // other parts of the stack.
            handle_pending_packets(core_ctx, bindings_ctx, packet_queue, key, new_route)
        }

        Ok(orig_route)
    }

    /// Remove the route from the multicast route table.
    ///
    /// Returns `None` if the route did not exist.
    pub fn remove_multicast_route(
        &mut self,
        key: &MulticastRouteKey<I>,
    ) -> Result<
        Option<MulticastRoute<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>>,
        MulticastForwardingDisabledError,
    > {
        self.core_ctx().with_state_mut(|state, ctx| {
            let state = state.try_enabled()?;
            ctx.with_route_table_mut(state, |route_table, _ctx| {
                Ok(route_table.remove(key).map(|MulticastRouteEntry { route, stats: _ }| route))
            })
        })
    }

    /// Remove all references to the device from the multicast forwarding state.
    ///
    /// Typically, this is called as part of device removal to purge all strong
    /// device references.
    ///
    /// Any routes that reference the device as an `input_interface` will be
    /// removed. Any routes that reference the device as a
    /// [`MulticastRouteTarget`] will have that target removed (and will
    /// themselves be removed if it's the only target).
    pub fn remove_references_to_device(
        &mut self,
        dev: &<C::CoreContext as DeviceIdContext<AnyDevice>>::WeakDeviceId,
    ) {
        self.core_ctx().with_state_mut(|state, ctx| {
            let Some(state) = state.enabled() else {
                // There's no state to update if forwarding is disabled.
                return;
            };
            ctx.with_route_table_mut(state, |route_table, _ctx| {
                route_table.retain(
                    |_route_key,
                     MulticastRouteEntry {
                         route: MulticastRoute { action, input_interface },
                         stats: _,
                     }| {
                        if dev == &*input_interface {
                            return false;
                        }
                        match action {
                            Action::Forward(targets) => {
                                // If all targets reference the device, we should
                                // discard the route entirely.
                                if targets.iter().all(|target| dev == &target.output_interface) {
                                    return false;
                                }
                                // Otherwise, if any target references the device,
                                // we should remove it from the set of targets.
                                if targets.iter().any(|target| dev == &target.output_interface) {
                                    *targets = targets
                                        .iter()
                                        .filter(|target| dev != &target.output_interface)
                                        .cloned()
                                        .collect();
                                }
                            }
                        }
                        true
                    },
                )
            })
        })
    }

    /// Returns the [`MulticastRouteStats`], if any, for the given key.
    pub fn get_route_stats(
        &mut self,
        key: &MulticastRouteKey<I>,
    ) -> Result<
        Option<MulticastRouteStats<<C::BindingsContext as InstantBindingsTypes>::Instant>>,
        MulticastForwardingDisabledError,
    > {
        self.core_ctx().with_state(|state, ctx| {
            let state = state.try_enabled()?;
            ctx.with_route_table(state, |route_table, _ctx| {
                Ok(route_table.get(key).map(
                    |MulticastRouteEntry { route: _, stats: MulticastRouteStats { last_used } }| {
                        MulticastRouteStats { last_used: last_used.load(Ordering::Relaxed) }
                    },
                ))
            })
        })
    }

    /// Writes multicast routing table information to the provided `inspector`.
    pub fn inspect<
        N: Inspector + InspectorDeviceExt<<C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    >(
        &mut self,
        inspector: &mut N,
    ) {
        self.core_ctx().with_state(|state, ctx| match state {
            MulticastForwardingState::Disabled => {
                inspector.record_bool("ForwardingEnabled", false);
            }
            MulticastForwardingState::Enabled(state) => {
                inspector.record_bool("ForwardingEnabled", true);
                inspector.record_child("Routes", |inspector| {
                    ctx.with_route_table(state, |route_table, _ctx| {
                        for (route_key, route_entry) in route_table.iter() {
                            inspector.record_unnamed_child(|inspector| {
                                inspector.delegate_inspectable(route_key);
                                route_entry.inspect::<_, N>(inspector);
                            })
                        }
                    })
                });
                // NB: All other operations on the pending table require mutable
                // access; don't bother introducing an immutable accessor just
                // for inspect.
                ctx.with_pending_table_mut(state, |pending_table| {
                    inspector.record_inspectable("PendingRoutes", pending_table);
                });
            }
        })
    }
}

/// Attempt to forward the packets from a pending [`PacketQueue`] according to a
/// newly installed [`MulticastRoute`].
fn handle_pending_packets<I: IpLayerIpExt, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    packet_queue: PacketQueue<I, CC::WeakDeviceId, BC>,
    key: MulticastRouteKey<I>,
    route: MulticastRoute<CC::DeviceId>,
) where
    CC: IpLayerForwardingContext<I, BC>
        + MulticastForwardingDeviceContext<I>
        + CounterContext<MulticastForwardingCounters<I>>,
    BC: IpLayerBindingsContext<I, CC::DeviceId>,
{
    let MulticastRoute { input_interface, action } = route;

    // NB: We checked that forwarding was enabled on the device before the
    // packet was enqueued in the pending table. However, the packet may sit in
    // the queue for an extended period of time, during which forwarding may
    // have been disabled on the device. Check again here just in case.
    if !core_ctx.is_device_multicast_forwarding_enabled(&input_interface) {
        // The user just installed a multicast route, but also disabled
        // forwarding on the device. Log a warning because that likely indicates
        // incorrect API usage.
        warn!(
            "Dropping pending packets for newly installed multicast route: {key:?}. \
            Multicast forwarding is disabled on input interface: {input_interface:?}"
        );
        CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx)
            .pending_packet_drops_disabled_dev
            .increment();
        return;
    }

    let MulticastRouteKey { src_addr, dst_addr } = key.clone();
    let dst_ip: SpecifiedAddr<I::Addr> = dst_addr.into();
    let src_ip: I::RecvSrcAddr = src_addr.into();

    for QueuedPacket { device, packet, frame_dst, max_fragment_len } in packet_queue.into_iter() {
        let device = match device.upgrade() {
            // Short circuit if the device was removed while the packet was
            // pending.
            None => continue,
            Some(d) => d,
        };
        // Short circuit if the queued packet arrived on the wrong device.
        if device != input_interface {
            CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx)
                .pending_packet_drops_wrong_dev
                .increment();
            bindings_ctx.on_event(
                MulticastForwardingEvent::WrongInputInterface {
                    key: key.clone(),
                    actual_input_interface: device,
                    expected_input_interface: input_interface.clone(),
                }
                .into(),
            );
            continue;
        }

        // NB: We could choose to update the `last_used` value on the route's
        // statistics here, but that's probably overkill. We only end up in this
        // function as part of route installation, which will have appropriately
        // initialized `last_used`. It's not worth re-acquiring the route table
        // lock to update it again here, as the change in time will be
        // negligible.

        match &action {
            Action::Forward(targets) => {
                CounterContext::<MulticastForwardingCounters<I>>::counters(core_ctx)
                    .pending_packet_tx
                    .increment();
                let packet_iter = core::iter::repeat_n(packet, targets.len());
                for (mut packet, MulticastRouteTarget { output_interface, min_ttl }) in
                    packet_iter.zip(targets.iter())
                {
                    let packet_metadata = Default::default();
                    crate::internal::base::determine_ip_packet_forwarding_action::<I, _, _>(
                        core_ctx,
                        packet.parse_ip_packet_mut(),
                        packet_metadata,
                        Some(*min_ttl),
                        &input_interface,
                        &output_interface,
                        IpPacketDestination::from_addr(dst_ip),
                        frame_dst,
                        src_ip,
                        dst_ip,
                    )
                    .perform_action_with_buffer(
                        core_ctx,
                        bindings_ctx,
                        packet.into_inner(),
                        max_fragment_len,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use alloc::vec;
    use core::num::NonZeroU8;
    use core::ops::Deref;
    use core::time::Duration;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_types::MulticastAddr;
    use netstack3_base::testutil::MultipleDevicesId;
    use netstack3_base::{LocalFrameDestination, StrongDeviceIdentifier};
    use packet::ParseBuffer;
    use test_case::test_case;

    use crate::IpLayerEvent;
    use crate::internal::multicast_forwarding;
    use crate::internal::multicast_forwarding::packet_queue::QueuePacketOutcome;
    use crate::internal::multicast_forwarding::testutil::{SentPacket, TestIpExt};
    use crate::multicast_forwarding::{MulticastRoute, MulticastRouteKey, MulticastRouteTarget};

    const MIN_TTL: NonZeroU8 = NonZeroU8::new(1).unwrap();

    #[ip_test(I)]
    fn enable_disable<I: IpLayerIpExt>() {
        let mut api = multicast_forwarding::testutil::new_api::<I>();

        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Disabled
        );
        assert!(api.enable());
        assert!(!api.enable());
        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Enabled(_)
        );
        assert!(api.disable());
        assert!(!api.disable());
        assert_matches!(
            api.core_ctx().state.multicast_forwarding.borrow().deref(),
            &MulticastForwardingState::Disabled
        );
    }

    #[ip_test(I)]
    fn add_remove_route<I: TestIpExt>() {
        let key1 = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();
        let forward_to_b = MulticastRoute::new_forward(
            MultipleDevicesId::A,
            [MulticastRouteTarget { output_interface: MultipleDevicesId::B, min_ttl: MIN_TTL }]
                .into(),
        )
        .unwrap();
        let forward_to_c = MulticastRoute::new_forward(
            MultipleDevicesId::A,
            [MulticastRouteTarget { output_interface: MultipleDevicesId::C, min_ttl: MIN_TTL }]
                .into(),
        )
        .unwrap();

        let mut api = multicast_forwarding::testutil::new_api::<I>();

        // Adding/removing routes before multicast forwarding is enabled should
        // fail.
        assert_eq!(
            api.add_multicast_route(key1.clone(), forward_to_b.clone()),
            Err(MulticastForwardingDisabledError {})
        );
        assert_eq!(api.remove_multicast_route(&key1), Err(MulticastForwardingDisabledError {}));

        // Enable the API and observe success.
        assert!(api.enable());
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_b.clone())));

        // Removing a route that doesn't exist should return `None`.
        assert_eq!(api.remove_multicast_route(&key1), Ok(None));

        // Adding a route with the same key as an existing route should
        // overwrite the original.
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(
            api.add_multicast_route(key1.clone(), forward_to_c.clone()),
            Ok(Some(forward_to_b.clone()))
        );
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_c.clone())));

        // Routes with different keys can co-exist.
        assert_eq!(api.add_multicast_route(key1.clone(), forward_to_b.clone()), Ok(None));
        assert_eq!(api.add_multicast_route(key2.clone(), forward_to_c.clone()), Ok(None));
        assert_eq!(api.remove_multicast_route(&key1), Ok(Some(forward_to_b)));
        assert_eq!(api.remove_multicast_route(&key2), Ok(Some(forward_to_c)));
    }

    #[ip_test(I)]
    #[test_case(false, true; "forwarding_disabled")]
    #[test_case(true, false; "forwarding_enabled_and_wrong_dev")]
    #[test_case(true, true; "forwarding_enabled_and_right_dev")]
    fn add_route_with_pending_packets<I: TestIpExt>(
        forwarding_enabled_for_dev: bool,
        right_dev: bool,
    ) {
        const FRAME_DST: Option<LocalFrameDestination> = None;
        const MAX_FRAGMENT_LEN: Option<usize> = None;
        const OUTPUT_DEV: MultipleDevicesId = MultipleDevicesId::C;
        let right_key = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let wrong_key = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();
        let expected_dev = MultipleDevicesId::A;
        let actual_dev = if right_dev { expected_dev } else { MultipleDevicesId::B };

        let route = MulticastRoute::new_forward(
            expected_dev,
            [MulticastRouteTarget { output_interface: OUTPUT_DEV, min_ttl: MIN_TTL }].into(),
        )
        .unwrap();

        let mut api = multicast_forwarding::testutil::new_api::<I>();
        assert!(api.enable());
        api.core_ctx()
            .state
            .set_multicast_forwarding_enabled_for_dev(expected_dev, forwarding_enabled_for_dev);

        // Setup a queued packet for `right_key`.
        let (core_ctx, bindings_ctx) = api.contexts();
        multicast_forwarding::testutil::with_pending_table(core_ctx, |pending_table| {
            let buf = multicast_forwarding::testutil::new_ip_packet_buf::<I>(I::SRC1, I::DST1);
            let mut buf_ref = buf.as_ref();
            let packet = buf_ref.parse::<I::Packet<_>>().expect("parse should succeed");
            assert_eq!(
                pending_table.try_queue_packet(
                    bindings_ctx,
                    right_key.clone(),
                    &packet,
                    &actual_dev,
                    FRAME_DST,
                    MAX_FRAGMENT_LEN,
                ),
                QueuePacketOutcome::QueuedInNewQueue,
            );
        });

        // Add a route with the wrong key and expect that the packet queue is
        // unaffected.
        assert_eq!(api.add_multicast_route(wrong_key, route.clone()), Ok(None));
        assert!(multicast_forwarding::testutil::with_pending_table(
            api.core_ctx(),
            |pending_table| pending_table.contains(&right_key)
        ));

        // Add a route with the right key and expect that the packet queue is
        // removed.
        assert_eq!(api.add_multicast_route(right_key.clone(), route), Ok(None));
        assert!(multicast_forwarding::testutil::with_pending_table(
            api.core_ctx(),
            |pending_table| !pending_table.contains(&right_key)
        ));

        let expect_sent_packet = forwarding_enabled_for_dev && right_dev;
        let mut expected_sent_packets = vec![];
        if expect_sent_packet {
            expected_sent_packets.push(SentPacket {
                dst: MulticastAddr::new(right_key.dst_addr()).unwrap(),
                device: OUTPUT_DEV,
            });
        }
        assert_eq!(api.core_ctx().state.take_sent_packets(), expected_sent_packets);

        // Verify that multicast routing events are generated.
        let mut expected_events = vec![];
        if !right_dev {
            expected_events.push(IpLayerEvent::MulticastForwarding(
                MulticastForwardingEvent::WrongInputInterface {
                    key: right_key,
                    actual_input_interface: actual_dev,
                    expected_input_interface: expected_dev,
                },
            ));
        }

        let (_core_ctx, bindings_ctx) = api.contexts();
        assert_eq!(bindings_ctx.take_events(), expected_events);

        // Verify that counters are updated.
        let counters: &MulticastForwardingCounters<I> = api.core_ctx().counters();
        assert_eq!(counters.pending_packet_tx.get(), if expect_sent_packet { 1 } else { 0 });
        assert_eq!(
            counters.pending_packet_drops_disabled_dev.get(),
            if forwarding_enabled_for_dev { 0 } else { 1 }
        );
        assert_eq!(counters.pending_packet_drops_wrong_dev.get(), if right_dev { 0 } else { 1 });
    }

    #[ip_test(I)]
    fn remove_references_to_device<I: TestIpExt>() {
        // NB: 4 arbitrary keys, that are unique from each other.
        let key1 = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();
        let key2 = MulticastRouteKey::new(I::SRC2, I::DST1).unwrap();
        let key3 = MulticastRouteKey::new(I::SRC1, I::DST2).unwrap();
        let key4 = MulticastRouteKey::new(I::SRC2, I::DST2).unwrap();

        // Create 4 routes, each exercising a different edge case.
        const GOOD_DEV1: MultipleDevicesId = MultipleDevicesId::A;
        const GOOD_DEV2: MultipleDevicesId = MultipleDevicesId::B;
        const BAD_DEV: MultipleDevicesId = MultipleDevicesId::C;
        const GOOD_TARGET1: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: GOOD_DEV1, min_ttl: MIN_TTL };
        const GOOD_TARGET2: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: GOOD_DEV2, min_ttl: MIN_TTL };
        const BAD_TARGET: MulticastRouteTarget<MultipleDevicesId> =
            MulticastRouteTarget { output_interface: BAD_DEV, min_ttl: MIN_TTL };
        let dev_is_input = MulticastRoute::new_forward(BAD_DEV, [GOOD_TARGET1].into()).unwrap();
        let dev_is_only_output =
            MulticastRoute::new_forward(GOOD_DEV1, [BAD_TARGET].into()).unwrap();
        let dev_is_one_output =
            MulticastRoute::new_forward(GOOD_DEV1, [GOOD_TARGET2, BAD_TARGET].into()).unwrap();
        let no_ref_to_dev = MulticastRoute::new_forward(GOOD_DEV1, [GOOD_TARGET2].into()).unwrap();

        // Verify that removing device references is a no-op when multicast
        // forwarding is disabled.
        let mut api = multicast_forwarding::testutil::new_api::<I>();
        api.remove_references_to_device(&BAD_DEV.downgrade());
        assert!(api.enable());

        // Add the four routes, remove references to `Dev`, and verify that:
        // * `dev_is_input` & `dev_is_only_output`, were both removed.
        // * `dev_is_one_output` was updated to not list the dev in its
        //    targets.
        // * `no_ref_to_dev` was not updated.
        assert_eq!(api.add_multicast_route(key1.clone(), dev_is_input), Ok(None));
        assert_eq!(api.add_multicast_route(key2.clone(), dev_is_only_output), Ok(None));
        assert_eq!(api.add_multicast_route(key3.clone(), dev_is_one_output), Ok(None));
        assert_eq!(api.add_multicast_route(key4.clone(), no_ref_to_dev.clone()), Ok(None));
        api.remove_references_to_device(&BAD_DEV.downgrade());
        assert_eq!(api.remove_multicast_route(&key1), Ok(None));
        assert_eq!(api.remove_multicast_route(&key2), Ok(None));
        // NB: Equal to `dev_is_one_output`, but with `BAD_TARGET` removed.
        assert_eq!(
            api.remove_multicast_route(&key3),
            Ok(Some(MulticastRoute::new_forward(GOOD_DEV1, [GOOD_TARGET2].into()).unwrap()))
        );
        assert_eq!(api.remove_multicast_route(&key4), Ok(Some(no_ref_to_dev)));
    }

    #[ip_test(I)]
    fn get_route_stats<I: TestIpExt>() {
        let key = MulticastRouteKey::new(I::SRC1, I::DST1).unwrap();

        let mut api = multicast_forwarding::testutil::new_api::<I>();

        // Verify that get_route_stats fails when forwarding is disabled.
        assert_eq!(api.get_route_stats(&key), Err(MulticastForwardingDisabledError {}));

        // Verify that get_route_stats returns `None` if the route doesn't exist.
        assert!(api.enable());
        assert_eq!(api.get_route_stats(&key), Ok(None));

        // Install a route and verify that get_route_stats succeeds.
        let route = MulticastRoute::new_forward(
            MultipleDevicesId::A,
            [MulticastRouteTarget { output_interface: MultipleDevicesId::B, min_ttl: MIN_TTL }]
                .into(),
        )
        .unwrap();
        assert_eq!(api.add_multicast_route(key.clone(), route.clone()), Ok(None));
        let original_time = api.ctx.bindings_ctx().now();
        let expected_stats = MulticastRouteStats { last_used: original_time };
        assert_eq!(api.get_route_stats(&key), Ok(Some(expected_stats)));

        // Advance the timer and overwrite the route to prove we initialize
        // stats with an up-to-date instant.
        api.ctx.bindings_ctx().timers.instant.sleep(Duration::from_secs(5));
        let new_time = api.ctx.bindings_ctx().now();
        assert!(new_time > original_time);
        let expected_stats = MulticastRouteStats { last_used: new_time };
        assert_eq!(api.add_multicast_route(key.clone(), route.clone()), Ok(Some(route)));
        assert_eq!(api.get_route_stats(&key), Ok(Some(expected_stats)));
    }
}
