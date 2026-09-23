// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ip_test_macro::ip_test;
use net_declare::{net_subnet_v4, net_subnet_v6};
use net_types::ip::{AddrSubnet, IpAddress as _, Ipv4Addr, Ipv6Addr};
use net_types::{SpecifiedAddr, Witness as _};
use netstack3_base::socket::SocketIpAddr;
use netstack3_base::{IpDeviceAddr, MarkDomain, Marks};
use test_case::test_case;

use netstack3_base::testutil::TestIpExt;
use netstack3_base::{MarkMatcher, MarkMatchers};
use netstack3_core::StackStateBuilder;
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetLinkDevice, MaxEthernetFrameSize,
};
use netstack3_core::error::NotFoundError;
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, FakeBindingsCtx, FakeCtx,
};
use netstack3_ip::{
    AddRouteError, AddableEntry, AddableEntryEither, AddableMetric, Entry, InternalForwarding,
    Metric, RawMetric, ResolvedRoute, RoutePreference, RouteResolveOptions, Rule, RuleAction,
    RuleMatcher,
};

#[ip_test(I)]
#[test_case(true; "when there is an on-link route to the gateway")]
#[test_case(false; "when there is no on-link route to the gateway")]
fn select_device_for_gateway<I: TestIpExt>(on_link_route: bool) {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    let device_id: DeviceId<_> = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: I::TEST_ADDRS.local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let gateway = SpecifiedAddr::new(
        // Set the last bit to make it an address inside the fake config's
        // subnet.
        I::map_ip::<_, I::Addr>(
            I::TEST_ADDRS.subnet.network(),
            |addr| {
                let mut bytes = addr.ipv4_bytes();
                bytes[bytes.len() - 1] = 1;
                Ipv4Addr::from(bytes)
            },
            |addr| {
                let mut bytes = addr.ipv6_bytes();
                bytes[bytes.len() - 1] = 1;
                Ipv6Addr::from(bytes)
            },
        )
        .to_ip_addr(),
    )
    .expect("should be specified");

    // Try to resolve a device for a gateway that we have no route to.
    assert_eq!(ctx.core_api().routes_any().select_device_for_gateway(gateway), None);

    // Add a route to the gateway.
    let route_to_add = if on_link_route {
        AddableEntryEither::from(AddableEntry::without_gateway(
            I::TEST_ADDRS.subnet,
            device_id.clone(),
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
    } else {
        AddableEntryEither::from(AddableEntry::with_gateway(
            I::TEST_ADDRS.subnet,
            device_id.clone(),
            I::TEST_ADDRS.remote_ip,
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
    };

    assert_eq!(ctx.test_api().add_route(route_to_add), Ok(()));

    // It still won't resolve successfully because the device is not enabled yet.
    assert_eq!(ctx.core_api().routes_any().select_device_for_gateway(gateway), None);

    ctx.test_api().enable_device(&device_id);

    // Now, try to resolve a device for the gateway.
    assert_eq!(
        ctx.core_api().routes_any().select_device_for_gateway(gateway),
        if on_link_route { Some(device_id) } else { None }
    );
}

struct AddGatewayRouteTestCase {
    enable_before_final_route_add: bool,
    expected_first_result: Result<(), AddRouteError>,
    expected_second_result: Result<(), AddRouteError>,
}

#[ip_test(I)]
#[test_case(AddGatewayRouteTestCase {
    enable_before_final_route_add: false,
    expected_first_result: Ok(()),
    expected_second_result: Ok(()),
}; "with_specified_device_no_enable")]
#[test_case(AddGatewayRouteTestCase {
    enable_before_final_route_add: true,
    expected_first_result: Ok(()),
    expected_second_result: Ok(()),
}; "with_specified_device_enabled")]
fn add_gateway_route<I: TestIpExt>(test_case: AddGatewayRouteTestCase) {
    let AddGatewayRouteTestCase {
        enable_before_final_route_add,
        expected_first_result,
        expected_second_result,
    } = test_case;
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();

    let gateway_subnet =
        I::map_ip((), |()| net_subnet_v4!("10.0.0.0/16"), |()| net_subnet_v6!("::0a00:0000/112"));

    let device_id: DeviceId<_> = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: I::TEST_ADDRS.local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let gateway_device = device_id.clone();

    // Attempt to add the gateway route when there is no known route to the
    // gateway.
    assert_eq!(
        ctx.test_api().add_route(AddableEntryEither::from(AddableEntry::with_gateway(
            gateway_subnet,
            gateway_device.clone(),
            I::TEST_ADDRS.remote_ip,
            AddableMetric::ExplicitMetric(RawMetric(0))
        ))),
        expected_first_result,
    );

    assert_eq!(
        ctx.test_api().del_routes_to_subnet(gateway_subnet.into()),
        expected_first_result.map_err(|_: AddRouteError| NotFoundError),
    );

    // Then, add a route to the gateway, and try again, expecting success.
    assert_eq!(
        ctx.test_api().add_route(AddableEntryEither::from(AddableEntry::without_gateway(
            I::TEST_ADDRS.subnet,
            device_id.clone(),
            AddableMetric::ExplicitMetric(RawMetric(0))
        ))),
        Ok(())
    );

    if enable_before_final_route_add {
        ctx.test_api().enable_device(&device_id);
    }
    assert_eq!(
        ctx.test_api().add_route(AddableEntryEither::from(AddableEntry::with_gateway(
            gateway_subnet,
            gateway_device,
            I::TEST_ADDRS.remote_ip,
            AddableMetric::ExplicitMetric(RawMetric(0))
        ))),
        expected_second_result,
    );
}

#[ip_test(I)]
fn test_route_tracks_interface_metric<I: TestIpExt>() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();

    let metric = RawMetric(9999);
    let device_id = ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
        EthernetCreationProperties {
            tx_offload_spec: Default::default(),
            mac: I::TEST_ADDRS.local_mac,
            max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
        },
        metric,
    );
    assert_eq!(
        ctx.test_api().add_route(AddableEntryEither::from(AddableEntry::without_gateway(
            I::TEST_ADDRS.subnet,
            device_id.clone().into(),
            AddableMetric::MetricTracksInterface
        ))),
        Ok(())
    );
    assert_eq!(
        ctx.core_api().routes_any().get_all_routes_in_main_table(),
        &[Entry {
            subnet: I::TEST_ADDRS.subnet,
            device: device_id.clone().into(),
            gateway: None,
            metric: Metric::MetricTracksInterface(metric),
            route_preference: RoutePreference::Medium
        }
        .into()]
    );

    // Remove the device and routes to clear all dangling references.
    ctx.test_api().clear_routes_and_remove_device(device_id);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_route_resolution_respects_source_address_matcher<I: TestIpExt + netstack3_core::IpExt>() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    // Creates a device with the given subnet address, and installs a default route
    // in the given table.
    let set_up_device = |ctx: &mut FakeCtx, device_addr_subnet| {
        let device_id: DeviceId<_> = ctx
            .core_api()
            .device::<EthernetLinkDevice>()
            .add_device_with_default_state(
                EthernetCreationProperties {
                    tx_offload_spec: Default::default(),
                    mac: I::TEST_ADDRS.local_mac,
                    max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
                },
                DEFAULT_INTERFACE_METRIC,
            )
            .into();
        ctx.test_api().enable_device(&device_id);
        ctx.core_api()
            .device_ip_any()
            .add_ip_addr_subnet(&device_id, device_addr_subnet)
            .expect("failed to assign IP address");
        let device_metric = ctx.core_api().device_ip::<I>().get_routing_metric(&device_id);
        (device_id, device_metric)
    };

    let main_table = ctx.core_api().routes::<I>().main_table_id();
    let (device_id_1, device_metric_1) = set_up_device(
        &mut ctx,
        AddrSubnet::new(I::TEST_ADDRS.local_ip.get(), I::TEST_ADDRS.subnet.prefix()).unwrap(),
    );
    // default route to device 1.
    ctx.core_api().routes::<I>().set_routes(
        &main_table,
        alloc::vec![
            netstack3_core::routes::Entry {
                subnet: I::ALL_ADDRS_SUBNET,
                device: device_id_1.clone(),
                gateway: None,
                metric: Metric::MetricTracksInterface(device_metric_1),
                route_preference: Default::default(),
            }
            .with_generation(netstack3_ip::Generation::initial())
        ],
    );
    let second_table = ctx.core_api().routes::<I>().new_table(());
    let (device_id_2, device_metric_2) = set_up_device(
        &mut ctx,
        AddrSubnet::new(I::get_other_ip_address(254).get(), I::TEST_ADDRS.subnet.prefix()).unwrap(),
    );
    let gateway = I::get_other_ip_address(100);
    ctx.core_api().routes::<I>().set_routes(
        &second_table,
        alloc::vec![
            // For any lookup for the test subnet, we still direct to device 1.
            // Note that we use a gateway here to differentiate with the route
            // in the main table (no gateway).
            netstack3_core::routes::Entry {
                subnet: I::TEST_ADDRS.subnet,
                device: device_id_1.clone(),
                gateway: Some(gateway),
                metric: Metric::MetricTracksInterface(device_metric_1),
                route_preference: Default::default(),
            }
            .with_generation(netstack3_ip::Generation::initial()),
            // Otherwise device 2.
            netstack3_core::routes::Entry {
                subnet: I::ALL_ADDRS_SUBNET,
                device: device_id_2,
                gateway: None,
                metric: Metric::MetricTracksInterface(device_metric_2),
                route_preference: Default::default(),
            }
            .with_generation(netstack3_ip::Generation::initial())
        ],
    );

    // We setup the rule so that there is a higher priority rule pointing to the `second_table` with
    // the test subnet as the source address matcher.
    ctx.test_api().set_rules::<I>(alloc::vec![
        Rule {
            matcher: RuleMatcher {
                source_address_matcher: Some(netstack3_base::SubnetMatcher(I::TEST_ADDRS.subnet)),
                traffic_origin_matcher: None,
                mark_matchers: Default::default(),
            },
            action: RuleAction::Lookup(second_table)
        },
        Rule { matcher: RuleMatcher::match_all_packets(), action: RuleAction::Lookup(main_table) },
    ]);

    let expected_route_no_gateway = ResolvedRoute {
        src_addr: IpDeviceAddr::new(I::TEST_ADDRS.local_ip.get()).unwrap(),
        device: device_id_1.clone(),
        local_delivery_device: None,
        next_hop: netstack3_ip::NextHop::RemoteAsNeighbor,
        internal_forwarding: InternalForwarding::NotUsed,
    };

    let expected_route_with_gateway = ResolvedRoute {
        src_addr: IpDeviceAddr::new(I::TEST_ADDRS.local_ip.get()).unwrap(),
        device: device_id_1,
        local_delivery_device: None,
        next_hop: netstack3_ip::NextHop::Gateway(gateway),
        internal_forwarding: InternalForwarding::NotUsed,
    };

    // We need to lookup the route again and in this case the destination address matches the
    // more specific prefix in `second_table`, it should yield the route with a gateway.
    assert_eq!(
        ctx.core_api().routes::<I>().resolve_route(
            SocketIpAddr::new(I::TEST_ADDRS.remote_ip.get()),
            &RouteResolveOptions::default()
        ),
        Ok(expected_route_with_gateway.clone())
    );

    // Make sure the route lookup actually converges with the same source address supplied by the
    // user - IpSock will select a source address on creation and use it to lookup routes when
    // sending packets later.
    assert_eq!(
        ctx.test_api().resolve_route_with_src_addr(
            IpDeviceAddr::new(I::TEST_ADDRS.local_ip.get()).unwrap(),
            SocketIpAddr::new(I::TEST_ADDRS.remote_ip.get()),
        ),
        Ok(expected_route_with_gateway)
    );

    // This case will hit the default route in the `second_table` during the second lookup. However
    // because of strong host model, the default route is not usable so we will continue to the
    // main table during the second lookup and yield the route without the gateway.
    assert_eq!(
        ctx.core_api().routes::<I>().resolve_route(
            SocketIpAddr::new(I::get_other_remote_ip_address(254).get()),
            &RouteResolveOptions::default()
        ),
        Ok(expected_route_no_gateway.clone())
    );

    // Make sure the route lookup actually converges with the same source address supplied by the
    // user - IpSock will select a source address on creation and use it to lookup routes when
    // sending packets later.
    assert_eq!(
        ctx.test_api().resolve_route_with_src_addr(
            IpDeviceAddr::new(I::TEST_ADDRS.local_ip.get()).unwrap(),
            SocketIpAddr::new(I::get_other_remote_ip_address(254).get())
        ),
        Ok(expected_route_no_gateway)
    );
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn route_resolution_with_marks<I: TestIpExt + netstack3_core::IpExt>() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    // Creates a device with the given subnet address, and installs a default route
    // in the given table.
    let set_up_device = |ctx: &mut FakeCtx, device_addr_subnet, table| {
        let device_id: DeviceId<_> = ctx
            .core_api()
            .device::<EthernetLinkDevice>()
            .add_device_with_default_state(
                EthernetCreationProperties {
                    tx_offload_spec: Default::default(),
                    mac: I::TEST_ADDRS.local_mac,
                    max_frame_size: MaxEthernetFrameSize::from_mtu(I::MINIMUM_LINK_MTU).unwrap(),
                },
                DEFAULT_INTERFACE_METRIC,
            )
            .into();
        ctx.test_api().enable_device(&device_id);
        ctx.core_api()
            .device_ip_any()
            .add_ip_addr_subnet(&device_id, device_addr_subnet)
            .expect("failed to assign IP address");
        let device_metric = ctx.core_api().device_ip::<I>().get_routing_metric(&device_id);

        // default route to device 1.
        ctx.core_api().routes::<I>().set_routes(
            table,
            alloc::vec![
                netstack3_core::routes::Entry {
                    subnet: I::ALL_ADDRS_SUBNET,
                    device: device_id.clone(),
                    gateway: None,
                    metric: Metric::MetricTracksInterface(device_metric),
                    route_preference: Default::default(),
                }
                .with_generation(netstack3_ip::Generation::initial())
            ],
        );

        device_id
    };

    let main_table = ctx.core_api().routes::<I>().main_table_id();
    let device_id_1 = set_up_device(
        &mut ctx,
        AddrSubnet::new(I::TEST_ADDRS.local_ip.get(), I::TEST_ADDRS.subnet.prefix()).unwrap(),
        &main_table,
    );
    let second_table = ctx.core_api().routes::<I>().new_table(());
    let device_id_2 = set_up_device(
        &mut ctx,
        AddrSubnet::new(I::get_other_ip_address(254).get(), I::TEST_ADDRS.subnet.prefix()).unwrap(),
        &second_table,
    );

    // We setup the rule so that there is a higher priority rule pointing to the `second_table` with
    // the given mark matchers and it will determine which device we will choose.
    ctx.test_api().set_rules::<I>(alloc::vec![
        Rule {
            matcher: RuleMatcher {
                mark_matchers: MarkMatchers::new(core::iter::once((
                    MarkDomain::Mark1,
                    MarkMatcher::Marked { mask: 1, start: 0, end: 1, invert: false },
                ))),
                ..RuleMatcher::match_all_packets()
            },
            action: RuleAction::Lookup(second_table)
        },
        Rule { matcher: RuleMatcher::match_all_packets(), action: RuleAction::Lookup(main_table) },
    ]);

    assert_eq!(
        ctx.test_api().resolve_route_with_marks(
            SocketIpAddr::new(I::TEST_ADDRS.remote_ip.get()),
            &Marks::new(core::iter::once((MarkDomain::Mark1, 0)))
        ),
        Ok(ResolvedRoute {
            src_addr: IpDeviceAddr::new(I::get_other_ip_address(254).get()).unwrap(),
            device: device_id_2,
            local_delivery_device: None,
            next_hop: netstack3_ip::NextHop::RemoteAsNeighbor,
            internal_forwarding: InternalForwarding::NotUsed,
        })
    );
    assert_eq!(
        ctx.test_api().resolve_route_with_marks(
            SocketIpAddr::new(I::TEST_ADDRS.remote_ip.get()),
            &Marks::UNMARKED,
        ),
        Ok(ResolvedRoute {
            src_addr: IpDeviceAddr::new(I::TEST_ADDRS.local_ip.get()).unwrap(),
            device: device_id_1,
            local_delivery_device: None,
            next_hop: netstack3_ip::NextHop::RemoteAsNeighbor,
            internal_forwarding: InternalForwarding::NotUsed,
        })
    );
}
