// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec::Vec;
use core::convert::TryInto as _;
use core::time::Duration;

use assert_matches::assert_matches;
use net_types::ip::{Ipv6, Ipv6Addr, Subnet};
use net_types::{LinkLocalUnicastAddr, Witness as _};
use packet::{BufferMut, InnerPacketBuilder as _, NestableSerializer as _, Serializer as _};
use packet_formats::icmp::ndp::options::{NdpOptionBuilder, PrefixInformation, RouteInformation};
use packet_formats::icmp::ndp::{OptionSequenceBuilder, RouterAdvertisement};
use packet_formats::icmp::{IcmpPacketBuilder, IcmpZeroCode};
use packet_formats::ip::Ipv6Proto;
use packet_formats::ipv6::Ipv6PacketBuilder;
use packet_formats::utils::NonZeroDuration;

use netstack3_base::testutil::{FakeInstant, TestAddrs, TestIpExt as _};
use netstack3_base::{FrameDestination, NetworkSerializationContext};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetDeviceEvent, EthernetLinkDevice,
};
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, DispatchedEvent, FakeBindingsCtx, FakeCtx,
};
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use netstack3_hashmap::HashMap;
use netstack3_ip::device::{
    IpDeviceBindingsContext, IpDeviceConfigurationUpdate, IpDeviceEvent,
    Ipv6DeviceConfigurationContext, Ipv6DeviceConfigurationUpdate, Ipv6DiscoveredRoute,
    Ipv6DiscoveredRouteProperties, Ipv6RouteDiscoveryBindingsContext, Ipv6RouteDiscoveryContext,
};
use netstack3_ip::{
    self as ip, AddableEntry, AddableEntryEither, AddableMetric, Entry, IPV6_DEFAULT_SUBNET,
    IpLayerEvent, Metric, RouterAdvertisementEvent,
};

const ONE_SECOND: NonZeroDuration = NonZeroDuration::from_secs(1).unwrap();
const TWO_SECONDS: NonZeroDuration = NonZeroDuration::from_secs(2).unwrap();
const THREE_SECONDS: NonZeroDuration = NonZeroDuration::from_secs(3).unwrap();

fn setup() -> (FakeCtx, DeviceId<FakeBindingsCtx>, TestAddrs<Ipv6Addr>) {
    let TestAddrs { local_mac, remote_mac: _, local_ip: _, remote_ip: _, subnet: _ } =
        Ipv6::TEST_ADDRS;

    let mut ctx = FakeCtx::default();
    let device_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    assert_timers_integration(&mut ctx.core_ctx(), &device_id, []);

    (ctx, device_id, Ipv6::TEST_ADDRS)
}

fn as_secs(d: NonZeroDuration) -> u16 {
    d.get().as_secs().try_into().unwrap()
}

const LINK_LOCAL_SUBNET: Subnet<Ipv6Addr> = net_declare::net_subnet_v6!("fe80::/64");

fn add_link_local_route(ctx: &mut FakeCtx, device: &DeviceId<FakeBindingsCtx>) {
    ctx.test_api()
        .add_route(AddableEntryEither::from(AddableEntry::without_gateway(
            LINK_LOCAL_SUBNET,
            device.clone(),
            AddableMetric::MetricTracksInterface,
        )))
        .unwrap()
}

fn discovered_route_to_entry(
    device: &DeviceId<FakeBindingsCtx>,
    route: Ipv6DiscoveredRoute,
    properties: Ipv6DiscoveredRouteProperties,
) -> Entry<Ipv6Addr, DeviceId<FakeBindingsCtx>> {
    let Ipv6DiscoveredRoute { subnet, gateway } = route;
    let Ipv6DiscoveredRouteProperties { route_preference } = properties;
    Entry {
        subnet,
        device: device.clone(),
        gateway: gateway.map(|g| (*g).into_specified()),
        metric: Metric::MetricTracksInterface(DEFAULT_INTERFACE_METRIC),
        route_preference,
    }
}

/// Returns a buffer containing a router advertisement IPv6 packet and a second
/// buffer containing the NDP options encapsulating within it.
///
/// The latter can be used for asserting on the [`RouterAdvertisementEvent`]
/// expected to be dispatched upon receiving this router advertisement.
fn router_advertisement_buf(
    src_ip: LinkLocalUnicastAddr<Ipv6Addr>,
    router_lifetime_secs: u16,
    on_link_prefix: Subnet<Ipv6Addr>,
    on_link_prefix_flag: bool,
    on_link_prefix_valid_lifetime_secs: u32,
    more_specific_route: Option<(Subnet<Ipv6Addr>, u32)>,
) -> (impl BufferMut, impl BufferMut) {
    let src_ip: Ipv6Addr = src_ip.get();
    let dst_ip = Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get();
    let p = PrefixInformation::new(
        on_link_prefix.prefix(),
        on_link_prefix_flag,
        false, /* autonomous_address_configuration_flag */
        on_link_prefix_valid_lifetime_secs,
        0, /* preferred_lifetime */
        on_link_prefix.network(),
    );
    let more_specific_route_opt = more_specific_route.map(|(subnet, secs)| {
        NdpOptionBuilder::RouteInformation(RouteInformation::new(subnet, secs, Default::default()))
    });
    let options = [NdpOptionBuilder::PrefixInformation(p)];
    let options = options.into_iter().chain(more_specific_route_opt).collect::<Vec<_>>();
    (
        OptionSequenceBuilder::new(options.iter())
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                RouterAdvertisement::new(
                    0,     /* hop_limit */
                    false, /* managed_flag */
                    false, /* other_config_flag */
                    router_lifetime_secs,
                    0, /* reachable_time */
                    0, /* retransmit_timer */
                ),
            ))
            .wrap_in(Ipv6PacketBuilder::new(
                src_ip,
                dst_ip,
                ip::icmp::REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
                Ipv6Proto::Icmpv6,
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b(),
        OptionSequenceBuilder::new(options.iter())
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b(),
    )
}

// Assert internal timers in integration tests by going through the contexts
// to get the state.
#[track_caller]
fn assert_timers_integration<CC, BC, I>(core_ctx: &mut CC, device_id: &CC::DeviceId, timers: I)
where
    CC: Ipv6DeviceConfigurationContext<BC>,
    for<'a> CC::Ipv6DeviceStateCtx<'a>: Ipv6RouteDiscoveryContext<BC>,
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId> + Ipv6RouteDiscoveryBindingsContext,
    I: IntoIterator<Item = (Ipv6DiscoveredRoute, BC::Instant)>,
{
    let want = timers.into_iter().map(|(r, t)| (r, t)).collect::<HashMap<_, _>>();
    let got = core_ctx.with_ipv6_device_configuration(device_id, |_, mut core_ctx| {
        core_ctx.with_discovered_routes_mut(device_id, |state, _| {
            state.timers().iter().map(|(k, (), t)| (*k, *t)).collect::<HashMap<_, _>>()
        })
    });
    assert_eq!(got, want);
}

#[test]
fn discovery_integration() {
    let (
        mut ctx,
        device_id,
        TestAddrs { local_mac: _, remote_mac, local_ip: _, remote_ip: _, subnet },
    ) = setup();

    add_link_local_route(&mut ctx, &device_id);

    let src_ip = remote_mac.to_ipv6_link_local().addr();

    let buf = |router_lifetime_secs,
               on_link_prefix_flag,
               prefix_valid_lifetime_secs,
               more_specified_route_lifetime_secs| {
        router_advertisement_buf(
            src_ip,
            router_lifetime_secs,
            subnet,
            on_link_prefix_flag,
            prefix_valid_lifetime_secs,
            Some((subnet, more_specified_route_lifetime_secs)),
        )
    };

    // Clear events so we can assert on route-added events later.
    let _: Vec<DispatchedEvent> = ctx.bindings_ctx.take_events();

    let (ra_buf, options_buf) = buf(0, false, as_secs(ONE_SECOND).into(), 0);
    // Do nothing as router with no valid lifetime has not been discovered
    // yet and prefix does not make on-link determination.
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    let ra_event1 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    assert_timers_integration(&mut ctx.core_ctx(), &device_id, []);

    // Discover a default router only as on-link prefix has no valid
    // lifetime.
    let (ra_buf, options_buf) = buf(as_secs(ONE_SECOND), true, 0, 0);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    let ra_event2 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    let gateway_route = Ipv6DiscoveredRoute { subnet: IPV6_DEFAULT_SUBNET, gateway: Some(src_ip) };
    let gateway_prop = Ipv6DiscoveredRouteProperties::default();
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [(gateway_route, FakeInstant::from(ONE_SECOND.get()))],
    );

    let gateway_route_entry = discovered_route_to_entry(&device_id, gateway_route, gateway_prop);
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::RouterAdvertisement(ra_event1),
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                gateway_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::RouterAdvertisement(ra_event2),
        ]
    );

    // Discover an on-link prefix and update valid lifetime for default
    // router.
    let (ra_buf, options_buf) = buf(as_secs(TWO_SECONDS), true, as_secs(ONE_SECOND).into(), 0);
    let ra_event3 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    let on_link_route = Ipv6DiscoveredRoute { subnet, gateway: None };
    let on_link_prop = Ipv6DiscoveredRouteProperties::default();
    let on_link_route_entry = discovered_route_to_entry(&device_id, on_link_route, on_link_prop);

    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [
            (gateway_route, FakeInstant::from(TWO_SECONDS.get())),
            (on_link_route, FakeInstant::from(ONE_SECOND.get())),
        ],
    );
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                on_link_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::RouterAdvertisement(ra_event3),
        ]
    );

    // Discover more-specific route.
    let (ra_buf, options_buf) =
        buf(as_secs(TWO_SECONDS), true, as_secs(ONE_SECOND).into(), as_secs(THREE_SECONDS).into());
    let ra_event4 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    let more_specific_route = Ipv6DiscoveredRoute { subnet, gateway: Some(src_ip) };
    let more_specific_prop = Ipv6DiscoveredRouteProperties::default();
    let more_specific_route_entry =
        discovered_route_to_entry(&device_id, more_specific_route, more_specific_prop);
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [
            (gateway_route, FakeInstant::from(TWO_SECONDS.get())),
            (on_link_route, FakeInstant::from(ONE_SECOND.get())),
            (more_specific_route, FakeInstant::from(THREE_SECONDS.get())),
        ],
    );

    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                more_specific_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::RouterAdvertisement(ra_event4),
        ]
    );

    // Invalidate default router and more specific route, and update valid
    // lifetime for on-link prefix.
    let (ra_buf, options_buf) = buf(0, true, as_secs(TWO_SECONDS).into(), 0);
    let ra_event5 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [(on_link_route, FakeInstant::from(TWO_SECONDS.get()))],
    );
    {
        let ip::Entry { subnet, device, gateway, .. } = gateway_route_entry;
        let event1 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        let ip::Entry { subnet, device, gateway, .. } = more_specific_route_entry;
        let event2 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        let events = ctx.bindings_ctx.take_events();
        assert_eq!(events.len(), 3);
        assert!(events.contains(&DispatchedEvent::IpLayerIpv6(event1)));
        assert!(events.contains(&DispatchedEvent::IpLayerIpv6(event2)));
        assert!(events.contains(&DispatchedEvent::RouterAdvertisement(ra_event5)));
    }

    // Do nothing as prefix does not make on-link determination and router
    // with valid lifetime is not discovered.
    let (ra_buf, options_buf) = buf(0, false, 0, 0);
    let ra_event6 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [(on_link_route, FakeInstant::from(TWO_SECONDS.get()))],
    );
    assert_eq!(ctx.bindings_ctx.take_events(), [DispatchedEvent::RouterAdvertisement(ra_event6)]);

    // Invalidate on-link prefix.
    let (ra_buf, options_buf) = buf(0, true, 0, 0);
    let ra_event7 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(&mut ctx.core_ctx(), &device_id, []);
    {
        let ip::Entry { subnet, device, gateway, .. } = on_link_route_entry;
        assert_eq!(
            ctx.bindings_ctx.take_events(),
            [
                DispatchedEvent::IpLayerIpv6(IpLayerEvent::RemoveRoutes {
                    subnet,
                    device: device.downgrade(),
                    gateway
                }),
                DispatchedEvent::RouterAdvertisement(ra_event7),
            ]
        );
    }
}

#[test]
fn discovery_integration_infinite_to_finite_to_infinite_lifetime() {
    let (
        mut ctx,
        device_id,
        TestAddrs { local_mac: _, remote_mac, local_ip: _, remote_ip: _, subnet },
    ) = setup();

    add_link_local_route(&mut ctx, &device_id);

    let src_ip = remote_mac.to_ipv6_link_local().addr();

    let buf = |router_lifetime_secs, on_link_prefix_flag, prefix_valid_lifetime_secs| {
        router_advertisement_buf(
            src_ip,
            router_lifetime_secs,
            subnet,
            on_link_prefix_flag,
            prefix_valid_lifetime_secs,
            None,
        )
    };

    let gateway_route = Ipv6DiscoveredRoute { subnet: IPV6_DEFAULT_SUBNET, gateway: Some(src_ip) };
    let gateway_prop = Ipv6DiscoveredRouteProperties::default();
    let gateway_route_entry = discovered_route_to_entry(&device_id, gateway_route, gateway_prop);
    let on_link_route = Ipv6DiscoveredRoute { subnet, gateway: None };
    let on_link_prop = Ipv6DiscoveredRouteProperties::default();
    let on_link_route_entry = discovered_route_to_entry(&device_id, on_link_route, on_link_prop);

    // Clear events so we can assert on route-added events later.
    let _: Vec<DispatchedEvent> = ctx.bindings_ctx.take_events();

    // Router with finite lifetime and on-link prefix with infinite
    // lifetime.
    let router_lifetime_secs = u16::MAX;
    let prefix_lifetime_secs = u32::MAX;
    let (ra_buf, options_buf) = buf(router_lifetime_secs, true, prefix_lifetime_secs);
    let ra_event1 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [(gateway_route, FakeInstant::from(Duration::from_secs(router_lifetime_secs.into())))],
    );
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                gateway_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                on_link_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::RouterAdvertisement(ra_event1),
        ]
    );

    // Router and prefix with finite lifetimes.
    let router_lifetime_secs = u16::MAX - 1;
    let prefix_lifetime_secs = u32::MAX - 1;
    let (ra_buf, options_buf) = buf(router_lifetime_secs, true, prefix_lifetime_secs);
    let ra_event2 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [
            (gateway_route, FakeInstant::from(Duration::from_secs(router_lifetime_secs.into()))),
            (on_link_route, FakeInstant::from(Duration::from_secs(prefix_lifetime_secs.into()))),
        ],
    );
    assert_eq!(ctx.bindings_ctx.take_events(), [DispatchedEvent::RouterAdvertisement(ra_event2)]);

    // Router with finite lifetime and on-link prefix with infinite
    // lifetime.
    let router_lifetime_secs = u16::MAX;
    let prefix_lifetime_secs = u32::MAX;
    let (ra_buf, options_buf) = buf(router_lifetime_secs, true, prefix_lifetime_secs);
    let ra_event3 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [(gateway_route, FakeInstant::from(Duration::from_secs(router_lifetime_secs.into())))],
    );
    assert_eq!(ctx.bindings_ctx.take_events(), [DispatchedEvent::RouterAdvertisement(ra_event3)]);

    // Router and prefix invalidated.
    let router_lifetime_secs = 0;
    let prefix_lifetime_secs = 0;
    let (ra_buf, options_buf) = buf(router_lifetime_secs, true, prefix_lifetime_secs);
    let ra_event4 = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(&mut ctx.core_ctx(), &device_id, []);

    {
        let ip::Entry { subnet, device, gateway, .. } = gateway_route_entry;
        let event1 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        let ip::Entry { subnet, device, gateway, .. } = on_link_route_entry;
        let event2 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        assert_eq!(
            ctx.bindings_ctx.take_events(),
            [
                DispatchedEvent::IpLayerIpv6(event1),
                DispatchedEvent::IpLayerIpv6(event2),
                DispatchedEvent::RouterAdvertisement(ra_event4)
            ]
        );
    }
}

#[test]
fn flush_routes_on_interface_disabled_integration() {
    let (
        mut ctx,
        device_id,
        TestAddrs { local_mac: _, remote_mac, local_ip: _, remote_ip: _, subnet },
    ) = setup();
    let ethernet_device_id = assert_matches!(device_id, DeviceId::Ethernet(ref id) => id);
    add_link_local_route(&mut ctx, &device_id);

    let src_ip = remote_mac.to_ipv6_link_local().addr();
    let gateway_route = Ipv6DiscoveredRoute { subnet: IPV6_DEFAULT_SUBNET, gateway: Some(src_ip) };
    let gateway_prop = Ipv6DiscoveredRouteProperties::default();
    let gateway_route_entry = discovered_route_to_entry(&device_id, gateway_route, gateway_prop);
    let on_link_route = Ipv6DiscoveredRoute { subnet, gateway: None };
    let on_link_prop = Ipv6DiscoveredRouteProperties::default();
    let on_link_route_entry = discovered_route_to_entry(&device_id, on_link_route, on_link_prop);

    // Clear events so we can assert on route-added events later.
    let _: Vec<DispatchedEvent> = ctx.bindings_ctx.take_events();

    // Discover both an on-link prefix and default router.
    let (ra_buf, options_buf) = router_advertisement_buf(
        src_ip,
        as_secs(TWO_SECONDS),
        subnet,
        true,
        as_secs(ONE_SECOND).into(),
        None,
    );
    let ra_event = RouterAdvertisementEvent {
        options_bytes: options_buf.as_ref().into(),
        device: device_id.downgrade(),
        source: **src_ip,
    };

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        ra_buf,
    );
    assert_timers_integration(
        &mut ctx.core_ctx(),
        &device_id,
        [
            (gateway_route, FakeInstant::from(TWO_SECONDS.get())),
            (on_link_route, FakeInstant::from(ONE_SECOND.get())),
        ],
    );
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                gateway_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::IpLayerIpv6(IpLayerEvent::AddRoute(
                on_link_route_entry.clone().map_device_id(|d| d.downgrade()).into(),
            )),
            DispatchedEvent::RouterAdvertisement(ra_event),
        ]
    );

    // Disable the interface.
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(false),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert_timers_integration(&mut ctx.core_ctx(), &device_id, []);

    {
        let ip::Entry { subnet, device, gateway, .. } = gateway_route_entry;
        let event1 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        let ip::Entry { subnet, device, gateway, .. } = on_link_route_entry;
        let event2 = IpLayerEvent::RemoveRoutes { subnet, device: device.downgrade(), gateway };
        let events = ctx.bindings_ctx.take_events();

        let (a, b, c, d) = assert_matches!(&events[..], [a, b, c, d] => (a, b, c, d));
        assert!([a, b].contains(&&DispatchedEvent::IpLayerIpv6(event1)));
        assert!([a, b].contains(&&DispatchedEvent::IpLayerIpv6(event2)));
        assert_eq!(
            [c, d],
            [
                &DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                    device: ethernet_device_id.downgrade(),
                    addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into(),
                }),
                &DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                    device: device.downgrade(),
                    ip_enabled: false
                }),
            ]
        );
    }
}
