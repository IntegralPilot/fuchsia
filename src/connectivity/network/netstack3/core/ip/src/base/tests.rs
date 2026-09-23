// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::rc::Rc;
use alloc::vec;
use core::cell::RefCell;
use core::convert::Infallible as Never;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use netstack3_base::testutil::{MultipleDevicesId, MultipleDevicesIdState, TestIpExt};
use netstack3_base::{CtxPair, NetworkSerializationContext, NetworkSerializer, SubnetMatcher};
use packet::{InnerPacketBuilder as _, NestableSerializer as _};
use packet_formats::ip::IpProto;

use crate::internal::routing::rules::RuleMatcher;
use crate::internal::routing::testutil;
use crate::{Destination, Entry, Metric, NextHop, RawMetric, RoutePreference};

use super::*;

struct IpFakeCoreCtx<I: IpLayerIpExt> {
    stack_wide_counters: IpCounters<I>,
    per_device_counters: MultipleDevicesIdState<IpCounters<I>>,
    rules_table: Rc<RefCell<RulesTable<I, MultipleDevicesId, FakeBindingsCtx>>>,
    main_table_id: RoutingTableId<I, MultipleDevicesId, FakeBindingsCtx>,
    routing_tables: Rc<
        RefCell<
            HashMap<
                RoutingTableId<I, MultipleDevicesId, FakeBindingsCtx>,
                PrimaryRc<BaseRoutingTableState<I, MultipleDevicesId, FakeBindingsCtx>>,
            >,
        >,
    >,
    device_mtu: Mtu,
}

impl<I: IpLayerIpExt> Default for IpFakeCoreCtx<I> {
    fn default() -> Self {
        let main_table =
            PrimaryRc::new(BaseRoutingTableState::with_bindings_id(RoutingTableCookie::Main));
        let main_table_id = RoutingTableId::new(PrimaryRc::clone_strong(&main_table));
        let route_tables =
            HashMap::from_iter(core::iter::once((main_table_id.clone(), main_table)));
        let rules_table = Rc::new(RefCell::new(RulesTable::new(main_table_id.clone())));
        Self {
            rules_table,
            routing_tables: Rc::new(RefCell::new(route_tables)),
            main_table_id,
            stack_wide_counters: Default::default(),
            per_device_counters: Default::default(),
            device_mtu: Mtu::max(),
        }
    }
}

impl<I: IpLayerIpExt> CounterContext<IpCounters<I>> for IpFakeCoreCtx<I> {
    fn counters(&self) -> &IpCounters<I> {
        &self.stack_wide_counters
    }
}

impl<I: IpLayerIpExt> ResourceCounterContext<MultipleDevicesId, IpCounters<I>>
    for IpFakeCoreCtx<I>
{
    fn per_resource_counters(&self, resource: &MultipleDevicesId) -> &IpCounters<I> {
        self.per_device_counters.state(resource)
    }
}

type FakeCoreCtx<I> =
    netstack3_base::testutil::FakeCoreCtx<IpFakeCoreCtx<I>, (), MultipleDevicesId>;
type FakeBindingsCtx = netstack3_base::testutil::FakeBindingsCtx<(), (), (), ()>;
type FakeCtx<I> = CtxPair<FakeCoreCtx<I>, FakeBindingsCtx>;

impl<I: IpLayerIpExt> IpDeviceMtuContext<I> for FakeCoreCtx<I> {
    fn get_mtu(&mut self, _device_id: &Self::DeviceId) -> Mtu {
        self.state.device_mtu
    }
}

impl<I: IpLayerIpExt, BC: TxMetadataBindingsTypes> IpDeviceSendContext<I, BC> for FakeCoreCtx<I> {
    fn send_ip_frame<S>(
        &mut self,
        _bindings_ctx: &mut BC,
        _device_id: &Self::DeviceId,
        _destination: IpPacketDestination<I, &Self::DeviceId>,
        _ip_layer_metadata: DeviceIpLayerMetadata<BC>,
        body: S,
        _egress_proof: filter::ProofOfEgressCheck,
    ) -> Result<(), netstack3_base::SendFrameError<S>>
    where
        S: NetworkSerializer,
        S::Buffer: BufferMut,
    {
        let frame = body
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .map_err(|(err, serializer)| netstack3_base::SendFrameError {
                error: err.into(),
                serializer,
            })?
            .unwrap_b()
            .into_inner();
        self.frames.push((), frame);
        Ok(())
    }
}

#[ip_test(I)]
fn no_loopback_addrs_on_the_wire<I: IpLayerIpExt + TestIpExt>() {
    let mut ctx = FakeCtx::default();
    const TTL: u8 = 1;
    let frame = I::PacketBuilder::new(
        I::TEST_ADDRS.local_ip.get(),
        I::LOOPBACK_ADDRESS.get(),
        TTL,
        IpProto::Udp.into(),
    )
    .wrap_body([].into_serializer());
    let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
    let result = send_ip_frame(
        core_ctx,
        bindings_ctx,
        &MultipleDevicesId::A,
        IpPacketDestination::Neighbor(I::TEST_ADDRS.remote_ip),
        frame,
        IpLayerPacketMetadata::default(),
        Mtu::no_limit(),
    )
    .map_err(|e| e.into_err());
    assert_eq!(result, Err(IpSendFrameErrorReason::IllegalLoopbackAddress));
}

impl<I: IpLayerIpExt> IpRoutingDeviceContext<I> for FakeCoreCtx<I> {
    fn get_routing_metric(&mut self, _device_id: &Self::DeviceId) -> RawMetric {
        unimplemented!()
    }

    fn is_ip_device_enabled(&mut self, _device_id: &Self::DeviceId) -> bool {
        true
    }
}

impl<I: IpLayerIpExt> IpDeviceEgressStateContext<I> for FakeCoreCtx<I> {
    fn with_next_packet_id<O, F: FnOnce(&I::PacketIdState) -> O>(&self, cb: F) -> O {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct WrapPacketId<I: IpLayerIpExt>(I::PacketIdState);

        // Implementing solely to satisfy traits, use an arbitrary packet ID
        // here. This should be changed if this fake is used to verify packet
        // IDs in IP packets.
        let WrapPacketId(packet_id_state) =
            I::map_ip_out((), |()| WrapPacketId(AtomicU16::new(1234)), |()| WrapPacketId(()));
        cb(&packet_id_state)
    }

    fn get_local_addr_for_remote(
        &mut self,
        _device_id: &Self::DeviceId,
        _remote: Option<SpecifiedAddr<I::Addr>>,
    ) -> Option<IpDeviceAddr<I::Addr>> {
        unimplemented!()
    }

    fn get_hop_limit(&mut self, _device_id: &Self::DeviceId) -> NonZeroU8 {
        DEFAULT_TTL
    }
}

impl<I: IpLayerIpExt> IpDeviceIngressStateContext<I> for FakeCoreCtx<I> {
    fn address_status_for_device(
        &mut self,
        _addr: SpecifiedAddr<I::Addr>,
        _device_id: &Self::DeviceId,
    ) -> AddressStatus<I::AddressStatus> {
        unimplemented!()
    }
}

impl<I: IpLayerIpExt> IpDeviceContext<I> for FakeCoreCtx<I> {
    fn is_ip_device_enabled(&mut self, _device_id: &Self::DeviceId) -> bool {
        unimplemented!()
    }

    type DeviceAndAddressStatusIter<'a> = core::iter::Empty<(Self::DeviceId, I::AddressStatus)>;

    fn with_address_statuses<F: FnOnce(Self::DeviceAndAddressStatusIter<'_>) -> R, R>(
        &mut self,
        _addr: SpecifiedAddr<I::Addr>,
        _cb: F,
    ) -> R {
        unimplemented!()
    }

    fn is_device_unicast_forwarding_enabled(&mut self, _device_id: &Self::DeviceId) -> bool {
        unimplemented!()
    }
}

impl<I: IpLayerIpExt> IpStateContext<I, FakeBindingsCtx> for FakeCoreCtx<I> {
    type IpRouteTablesCtx<'a> = Self;

    fn with_rules_table<
        O,
        F: FnOnce(
            &mut Self::IpRouteTablesCtx<'_>,
            &RulesTable<I, Self::DeviceId, FakeBindingsCtx>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let rules_table = self.state.rules_table.clone();
        let rules_table = rules_table.borrow();
        cb(self, &rules_table)
    }

    fn with_rules_table_mut<
        O,
        F: FnOnce(
            &mut Self::IpRouteTablesCtx<'_>,
            &mut RulesTable<I, Self::DeviceId, FakeBindingsCtx>,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let rules_table = self.state.rules_table.clone();
        let mut rules_table = rules_table.borrow_mut();
        cb(self, &mut rules_table)
    }
}

impl<I: IpLayerIpExt> IpRouteTablesContext<I, FakeBindingsCtx> for FakeCoreCtx<I> {
    type Ctx<'a> = Self;

    fn main_table_id(&self) -> RoutingTableId<I, Self::DeviceId, FakeBindingsCtx> {
        self.state.main_table_id.clone()
    }

    fn with_ip_routing_tables<
        O,
        F: FnOnce(
            &mut Self::Ctx<'_>,
            &HashMap<
                RoutingTableId<I, Self::DeviceId, FakeBindingsCtx>,
                PrimaryRc<BaseRoutingTableState<I, Self::DeviceId, FakeBindingsCtx>>,
            >,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let route_tables = self.state.routing_tables.clone();
        let route_tables = route_tables.borrow();
        cb(self, &route_tables)
    }

    fn with_ip_routing_tables_mut<
        O,
        F: FnOnce(
            &mut HashMap<
                RoutingTableId<I, Self::DeviceId, FakeBindingsCtx>,
                PrimaryRc<BaseRoutingTableState<I, Self::DeviceId, FakeBindingsCtx>>,
            >,
        ) -> O,
    >(
        &mut self,
        cb: F,
    ) -> O {
        let route_tables = self.state.routing_tables.clone();
        let mut route_tables = route_tables.borrow_mut();
        cb(&mut route_tables)
    }
}

impl<I: IpLayerIpExt> IpRouteTableContext<I, FakeBindingsCtx> for FakeCoreCtx<I> {
    type IpDeviceIdCtx<'a> = Self;

    fn with_ip_routing_table<
        O,
        F: FnOnce(&mut Self::IpDeviceIdCtx<'_>, &RoutingTable<I, Self::DeviceId>) -> O,
    >(
        &mut self,
        table_id: &RoutingTableId<I, Self::DeviceId, FakeBindingsCtx>,
        cb: F,
    ) -> O {
        let table = table_id.0.routing_table.read();
        cb(self, &table)
    }

    fn with_ip_routing_table_mut<
        O,
        F: FnOnce(&mut Self::IpDeviceIdCtx<'_>, &mut RoutingTable<I, Self::DeviceId>) -> O,
    >(
        &mut self,
        table_id: &RoutingTableId<I, Self::DeviceId, FakeBindingsCtx>,
        cb: F,
    ) -> O {
        let mut table = table_id.0.routing_table.write();
        cb(self, &mut table)
    }
}

#[ip_test(I)]
fn test_walk_rules<I: IpLayerIpExt + TestIpExt>() {
    let mut ctx = FakeCoreCtx::<I>::default();

    // An unreachable rule should short-circuit the lookup.
    ctx.state.rules_table.borrow_mut().rules_mut().insert(
        0,
        Rule { matcher: RuleMatcher::match_all_packets(), action: RuleAction::Unreachable },
    );

    ctx.with_rules_table(|core_ctx, rules| {
        assert_eq!(
            walk_rules(
                core_ctx,
                rules,
                (),
                &RuleInput {
                    packet_origin: PacketOrigin::Local { bound_address: None, bound_device: None },
                    marks: &Default::default(),
                },
                |(), _core_ctx, _table| panic!("should not be able to look up tables")
            ),
            ControlFlow::Break(RuleAction::<RuleWalkInfo<Never>>::Unreachable)
        );
    });

    // We setup the routing tables and rules as follows:
    // rule 1: if the source address is from the `I::TEST_ADDRS.subnet`, then lookup route_table_1
    // rule 2: by default, look up in the main route table.
    // In route_table_1, we route the packets to `MultipleDevicesId::A`.
    // In the main route table, we route the packets to `MultipleDevicesId::B`.
    let route_table_1 =
        PrimaryRc::new(BaseRoutingTableState::with_bindings_id(RoutingTableCookie::BindingsId(())));
    let table_id = RoutingTableId::new(PrimaryRc::clone_strong(&route_table_1));
    ctx.state.rules_table.borrow_mut().rules_mut()[0] = Rule {
        matcher: RuleMatcher {
            source_address_matcher: Some(SubnetMatcher(I::TEST_ADDRS.subnet)),
            traffic_origin_matcher: None,
            mark_matchers: Default::default(),
        },
        action: RuleAction::Lookup(table_id.clone()),
    };
    let _entry = testutil::add_entry(
        &mut table_id.get_mut(),
        Entry {
            subnet: I::TEST_ADDRS.subnet,
            device: MultipleDevicesId::A,
            gateway: None,
            metric: Metric::ExplicitMetric(RawMetric(0)),
            route_preference: RoutePreference::Medium,
        },
    )
    .expect("failed to install route entry");
    assert_matches!(ctx.state.routing_tables.borrow_mut().insert(table_id, route_table_1), None);

    let _entry = testutil::add_entry(
        &mut IpRouteTablesContext::<I, _>::main_table_id(&ctx).get_mut(),
        Entry {
            subnet: I::LOOPBACK_SUBNET,
            device: MultipleDevicesId::B,
            gateway: None,
            metric: Metric::ExplicitMetric(RawMetric(0)),
            route_preference: RoutePreference::Medium,
        },
    )
    .expect("failed to install route entry");

    // We try to walk the rules with a bound address that matches the rule 1's matcher, we should
    // get a route back with `MultipleDevicesId::A`.
    ctx.with_rules_table(|core_ctx, rules| {
        assert_eq!(
            walk_rules(
                core_ctx,
                rules,
                (),
                &RuleInput {
                    packet_origin: PacketOrigin::Local {
                        bound_address: Some(I::TEST_ADDRS.local_ip),
                        bound_device: None
                    },
                    marks: &Default::default(),
                },
                |(), core_ctx, table| {
                    match table.lookup(core_ctx, None, I::TEST_ADDRS.remote_ip.get()) {
                        None => ControlFlow::Continue(()),
                        Some(dest) => ControlFlow::Break(dest),
                    }
                }
            ),
            ControlFlow::Break(RuleAction::Lookup(RuleWalkInfo {
                inner: Destination {
                    device: MultipleDevicesId::A,
                    next_hop: NextHop::RemoteAsNeighbor
                },
                observed_source_address_matcher: true
            }))
        );
    });

    // Then we walk the rules with a bound address that does not match rule 1's matcher, we should
    // skip route table 1 and get a route back with `MultipleDevicesId::B`.
    ctx.with_rules_table(|core_ctx, rules| {
        assert_eq!(
            walk_rules(
                core_ctx,
                rules,
                (),
                &RuleInput {
                    packet_origin: PacketOrigin::Local {
                        bound_address: Some(I::LOOPBACK_ADDRESS),
                        bound_device: None
                    },
                    marks: &Default::default(),
                },
                |(), core_ctx, table| {
                    match table.lookup(core_ctx, None, *I::LOOPBACK_ADDRESS) {
                        None => ControlFlow::Continue(()),
                        Some(dest) => ControlFlow::Break(dest),
                    }
                }
            ),
            ControlFlow::Break(RuleAction::Lookup(RuleWalkInfo {
                inner: Destination {
                    device: MultipleDevicesId::B,
                    next_hop: NextHop::RemoteAsNeighbor
                },
                observed_source_address_matcher: true
            }))
        );
    });
}

#[ip_test(I)]
fn send_respects_device_mtu<I: IpLayerIpExt + TestIpExt>() {
    let mut core_ctx = FakeCoreCtx::<I>::default();
    let mut bindings_ctx = FakeBindingsCtx::default();

    let body = Buf::new(vec![1, 2, 3, 4, 5, 6], ..).wrap_in(());
    let meta = SendIpPacketMeta {
        device: &MultipleDevicesId::A,
        src_ip: Some(I::TEST_ADDRS.local_ip),
        dst_ip: I::TEST_ADDRS.remote_ip,
        destination: IpPacketDestination::Loopback(&MultipleDevicesId::A),
        proto: IpProto::Udp.into(),
        ttl: None,
        mtu: Mtu::no_limit(),
        dscp_and_ecn: Default::default(),
    };

    send_ip_packet_from_device(
        &mut core_ctx,
        &mut bindings_ctx,
        meta.clone(),
        body.clone(),
        IpLayerPacketMetadata::default(),
    )
    .expect("send ip packet");
    let [((), frame)] = core_ctx.frames.take_frames().try_into().expect("more than one frame sent");

    let frame_len = u32::try_from(frame.len()).unwrap();

    // Set the MTU so that it wouldn't fit the entire packet.
    core_ctx.state.device_mtu = Mtu::new(frame_len - 1);
    assert_eq!(
        send_ip_packet_from_device(
            &mut core_ctx,
            &mut bindings_ctx,
            meta.clone(),
            body.clone(),
            IpLayerPacketMetadata::default(),
        )
        .map_err(|ErrorAndSerializer { error, serializer: _ }| error),
        Err(IpSendFrameErrorReason::Device(SendFrameErrorReason::SizeConstraintsViolation))
    );

    // Still works if MTU matches exactly.
    core_ctx.state.device_mtu = Mtu::new(frame_len);
    send_ip_packet_from_device(
        &mut core_ctx,
        &mut bindings_ctx,
        meta,
        body,
        IpLayerPacketMetadata::default(),
    )
    .expect("send ip packet")
}
