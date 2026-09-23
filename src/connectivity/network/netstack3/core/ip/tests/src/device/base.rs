// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use alloc::vec::Vec;
use core::num::{NonZeroU8, NonZeroU16};
use core::time::Duration;
use netstack3_core::ip::RouteDiscoveryConfigurationUpdate;

use assert_matches::assert_matches;
use either::Either;
use ip_test_macro::ip_test;
use net_declare::{net_ip_v4, net_ip_v6, net_mac};
use net_types::ip::{AddrSubnet, Ip, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Mtu};
use net_types::{LinkLocalAddr, SpecifiedAddr, UnicastAddr, Witness, ZonedAddr};
use test_case::test_case;

use netstack3_base::testutil::{
    FakeInstant, FakeTimerCtxExt, TestIpExt, WithFakeFrameContext, assert_empty,
};
use netstack3_base::{
    FrameDestination, InstantContext as _, IpAddressId as _, IpDeviceAddr, LocalAddressError,
    NetworkSerializationContext,
};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetDeviceEvent, EthernetLinkDevice,
    LoopbackCreationProperties, LoopbackDevice, MaxEthernetFrameSize,
};
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, DispatchedEvent, DispatchedFrame, FakeBindingsCtx,
    FakeCtx,
};
use netstack3_core::{IpExt, StackStateBuilder, TimerId};
use netstack3_device::WeakDeviceId;
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use netstack3_hashmap::HashSet;
use netstack3_ip::device::{
    AddIpAddrSubnetError, AddressRemovedReason, CommonAddressConfig, CommonAddressProperties,
    DadTimerId, IPV4_DAD_ANNOUNCE_NUM, IpAddressState, IpDeviceConfiguration,
    IpDeviceConfigurationUpdate, IpDeviceEvent, IpDeviceFlags, IpDeviceHandler,
    IpDeviceStateContext, Ipv4AddrConfig, Ipv4DadAddressInfo, Ipv4DeviceConfigurationUpdate,
    Ipv4DeviceTimerId, Ipv6DeviceConfigurationUpdate, Ipv6DeviceContext, Ipv6DeviceHandler,
    Ipv6DeviceTimerId, Ipv6NetworkLearnedParameters, Lifetime, PreferredLifetime, RsTimerId,
    SetIpAddressPropertiesError, SlaacConfigurationUpdate, StableSlaacAddressConfiguration,
    TemporarySlaacAddressConfiguration, UpdateIpConfigurationError,
};
use netstack3_ip::gmp::{IgmpConfigMode, MldConfigMode, MldTimerId};
use netstack3_ip::nud::{self, LinkResolutionResult};
use netstack3_ip::testutil::IpCounterExpectations;
use netstack3_ip::{
    AddableEntry, AddableEntryEither, AddableMetric, ResolveRouteError, RouteResolveOptions,
};
use packet::{Buf, NestablePacketBuilder, Serializer};
use packet_formats::ip::IpProto;
use packet_formats::ipv4::Ipv4PacketBuilder;
use packet_formats::testutil::ArpPacketInfo;
use packet_formats::utils::NonZeroDuration;

#[test]
fn enable_disable_ipv4() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();

    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    let set_ipv4_enabled = |ctx: &mut FakeCtx, enabled, expected_prev| {
        assert_eq!(
            ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, enabled),
            expected_prev
        );
    };

    set_ipv4_enabled(&mut ctx, true, false);

    let weak_device_id = device_id.downgrade();
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            })
        ]
    );

    let addr = SpecifiedAddr::new(net_ip_v4!("192.0.2.1")).expect("addr should be unspecified");
    let mac = UnicastAddr::new(net_mac!("02:23:45:67:89:ab")).unwrap();

    ctx.core_api()
        .neighbor::<Ipv4, EthernetLinkDevice>()
        .insert_static_entry(&ethernet_device_id, *addr, mac)
        .unwrap();
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [DispatchedEvent::NeighborIpv4(nud::Event {
            device: ethernet_device_id.downgrade(),
            addr,
            kind: nud::EventKind::Added(nud::EventState::Static(mac)),
            at: ctx.bindings_ctx.now(),
        })]
    );
    assert_matches!(
        ctx.core_api().neighbor::<Ipv4, EthernetLinkDevice>().resolve_link_addr(
            &ethernet_device_id,
            &addr,
        ),
        LinkResolutionResult::Resolved(got) => assert_eq!(got, mac)
    );

    set_ipv4_enabled(&mut ctx, false, true);
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::NeighborIpv4(nud::Event {
                device: ethernet_device_id.downgrade(),
                addr,
                kind: nud::EventKind::Removed,
                at: ctx.bindings_ctx.now(),
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: false,
            })
        ]
    );

    // Assert that static ARP entries are flushed on link down.
    nud::testutil::assert_neighbor_unknown::<Ipv4, _, _, _>(
        &mut ctx.core_ctx(),
        ethernet_device_id.clone(),
        addr,
    );

    let ipv4_addr_subnet = AddrSubnet::new(Ipv4Addr::new([192, 168, 0, 1]), 24).unwrap();
    ctx.core_api()
        .device_ip::<Ipv4>()
        .add_ip_addr_subnet(&device_id, ipv4_addr_subnet.clone())
        .expect("failed to add IPv4 Address");
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressAdded {
            device: weak_device_id.clone(),
            addr: ipv4_addr_subnet.clone(),
            state: IpAddressState::Unavailable,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    set_ipv4_enabled(&mut ctx, true, false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ipv4_addr_subnet.addr(),
                state: IpAddressState::Assigned,
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            }),
        ]
    );
    // Verify that a redundant "enable" does not generate any events.
    set_ipv4_enabled(&mut ctx, true, true);
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    let valid_until = Lifetime::Finite(FakeInstant::from(Duration::from_secs(2)));
    let preferred_lifetime =
        PreferredLifetime::preferred_until(FakeInstant::from(Duration::from_secs(1)));
    let properties = CommonAddressProperties { valid_until, preferred_lifetime };

    ctx.core_api()
        .device_ip::<Ipv4>()
        .set_addr_properties(&device_id, ipv4_addr_subnet.addr(), properties)
        .expect("set properties should succeed");
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressPropertiesChanged {
            device: weak_device_id.clone(),
            addr: ipv4_addr_subnet.addr(),
            valid_until,
            preferred_lifetime
        })]
    );

    // Verify that a redundant "set properties" does not generate any events.
    ctx.core_api()
        .device_ip::<Ipv4>()
        .set_addr_properties(&device_id, ipv4_addr_subnet.addr(), properties)
        .expect("set properties should succeed");
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    set_ipv4_enabled(&mut ctx, false, true);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ipv4_addr_subnet.addr(),
                state: IpAddressState::Unavailable,
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: weak_device_id,
                ip_enabled: false,
            }),
        ]
    );
    // Verify that a redundant "disable" does not generate any events.
    set_ipv4_enabled(&mut ctx, false, false);
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);
}

fn enable_ipv6_device(
    ctx: &mut FakeCtx,
    device_id: &DeviceId<FakeBindingsCtx>,
    ll_addr: AddrSubnet<Ipv6Addr, LinkLocalAddr<UnicastAddr<Ipv6Addr>>>,
    expected_prev: bool,
) {
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(device_id, true), expected_prev);

    assert_eq!(
        IpDeviceStateContext::<Ipv6, _>::with_address_ids(
            &mut ctx.core_ctx(),
            device_id,
            |addrs, _core_ctx| {
                addrs.map(|addr_id| addr_id.addr_sub().addr().get()).collect::<HashSet<_>>()
            }
        ),
        HashSet::from([ll_addr.ipv6_unicast_addr()]),
        "enabled device expected to generate link-local address"
    );
}

#[test]
fn enable_disable_ipv6() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let local_mac = Ipv6::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();
    let ll_addr = local_mac.to_ipv6_link_local();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(NonZeroU8::new(1)),
                // Auto-generate a link-local address.
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    gmp_enabled: Some(true),
                    // Doesn't matter as long as we perform DAD and router
                    // solicitation.
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    // Enable the device and observe an auto-generated link-local address,
    // router solicitation and DAD for the auto-generated address.
    let test_enable_device = |ctx: &mut FakeCtx, expected_prev| {
        enable_ipv6_device(ctx, &device_id, ll_addr, expected_prev);
        let timers = vec![
            (
                TimerId::from(
                    Ipv6DeviceTimerId::Rs(RsTimerId::new(device_id.downgrade())).into_common(),
                ),
                ..,
            ),
            (
                TimerId::from(
                    Ipv6DeviceTimerId::Dad(DadTimerId::new(
                        device_id.downgrade(),
                        IpDeviceStateContext::<Ipv6, _>::get_address_id(
                            &mut ctx.core_ctx(),
                            &device_id,
                            ll_addr.ipv6_unicast_addr().into(),
                        )
                        .unwrap()
                        .downgrade(),
                    ))
                    .into_common(),
                ),
                ..,
            ),
            (
                TimerId::from(
                    Ipv6DeviceTimerId::Mld(MldTimerId::new(device_id.downgrade())).into_common(),
                ),
                ..,
            ),
        ];
        ctx.bindings_ctx.timer_ctx().assert_timers_installed_range(timers);
    };
    test_enable_device(&mut ctx, false);
    let weak_device_id = device_id.downgrade();
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
                device: weak_device_id.clone(),
                addr: ll_addr.to_witness(),
                state: IpAddressState::Tentative,
                valid_until: Lifetime::Infinite,
                preferred_lifetime: PreferredLifetime::preferred_forever(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            })
        ]
    );

    // Because the added address is from SLAAC, setting properties should fail.
    assert_matches!(
        ctx.core_api().device_ip::<Ipv6>().set_addr_properties(
            &device_id,
            ll_addr.addr().into(),
            CommonAddressProperties::default(),
        ),
        Err(SetIpAddressPropertiesError::NotManual)
    );

    let addr = SpecifiedAddr::new(net_ip_v6!("2001:db8::1")).expect("addr should be unspecified");
    let mac = UnicastAddr::new(net_mac!("02:23:45:67:89:ab")).unwrap();
    ctx.core_api()
        .neighbor::<Ipv6, _>()
        .insert_static_entry(&ethernet_device_id, *addr, mac)
        .unwrap();
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [DispatchedEvent::NeighborIpv6(nud::Event {
            device: ethernet_device_id.downgrade(),
            addr,
            kind: nud::EventKind::Added(nud::EventState::Static(mac)),
            at: ctx.bindings_ctx.now(),
        })]
    );
    assert_matches!(
        ctx.core_api().neighbor::<Ipv6, _>().resolve_link_addr(
            &ethernet_device_id,
            &addr,
        ),
        LinkResolutionResult::Resolved(got) => assert_eq!(got, mac)
    );

    let test_disable_device = |ctx: &mut FakeCtx, expected_prev| {
        assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, false), expected_prev);
        ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    };
    test_disable_device(&mut ctx, true);
    assert_eq!(
        ctx.bindings_ctx.take_events(),
        [
            DispatchedEvent::NeighborIpv6(nud::Event {
                device: ethernet_device_id.downgrade(),
                addr,
                kind: nud::EventKind::Removed,
                at: ctx.bindings_ctx.now(),
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressRemoved {
                device: weak_device_id.clone(),
                addr: ll_addr.addr().into(),
                reason: AddressRemovedReason::Manual,
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: false,
            })
        ]
    );

    let mut core_ctx = ctx.core_ctx();
    let core_ctx = &mut core_ctx;
    IpDeviceStateContext::<Ipv6, _>::with_address_ids(core_ctx, &device_id, |addrs, _core_ctx| {
        assert_empty(addrs);
    });

    // Assert that static NDP entry was removed on link down.
    nud::testutil::assert_neighbor_unknown::<Ipv6, _, _, _>(
        core_ctx,
        ethernet_device_id.clone(),
        addr,
    );

    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device_id, ll_addr.replace_witness().unwrap())
        .expect("add MAC based IPv6 link-local address");
    assert_eq!(
        IpDeviceStateContext::<Ipv6, _>::with_address_ids(
            &mut ctx.core_ctx(),
            &device_id,
            |addrs, _core_ctx| {
                addrs.map(|addr_id| addr_id.addr_sub().addr().get()).collect::<HashSet<_>>()
            }
        ),
        HashSet::from([ll_addr.ipv6_unicast_addr()])
    );
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
            device: weak_device_id.clone(),
            addr: ll_addr.to_witness(),
            state: IpAddressState::Unavailable,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    test_enable_device(&mut ctx, false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ll_addr.addr().into(),
                state: IpAddressState::Tentative,
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            })
        ]
    );

    let valid_until = Lifetime::Finite(FakeInstant::from(Duration::from_secs(2)));
    let preferred_lifetime =
        PreferredLifetime::preferred_until(FakeInstant::from(Duration::from_secs(1)));
    let properties = CommonAddressProperties { valid_until, preferred_lifetime };
    ctx.core_api()
        .device_ip::<Ipv6>()
        .set_addr_properties(&device_id, ll_addr.addr().into(), properties)
        .expect("set properties should succeed");
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressPropertiesChanged {
            device: weak_device_id.clone(),
            addr: ll_addr.addr().into(),
            valid_until,
            preferred_lifetime,
        })]
    );

    // Verify that a redundant "set properties" does not generate any events.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .set_addr_properties(&device_id, ll_addr.addr().into(), properties)
        .expect("set properties should succeed");
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    test_disable_device(&mut ctx, true);
    // The address was manually added, don't expect it to be removed.
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ll_addr.addr().into(),
                state: IpAddressState::Unavailable,
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: false,
            })
        ]
    );

    // Verify that a redundant "disable" does not generate any events.
    test_disable_device(&mut ctx, false);

    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    let core_ctx = &mut core_ctx;
    assert_eq!(bindings_ctx.take_events()[..], []);

    assert_eq!(
        IpDeviceStateContext::<Ipv6, _>::with_address_ids(
            core_ctx,
            &device_id,
            |addrs, _core_ctx| {
                addrs.map(|addr_id| addr_id.addr_sub().addr().get()).collect::<HashSet<_>>()
            }
        ),
        HashSet::from([ll_addr.ipv6_unicast_addr()]),
        "manual addresses should not be removed on device disable"
    );

    test_enable_device(&mut ctx, false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ll_addr.addr().into(),
                state: IpAddressState::Tentative,
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id,
                ip_enabled: true,
            })
        ]
    );

    // Verify that a redundant "enable" does not generate any events.
    test_enable_device(&mut ctx, true);
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    // Disable device again so timers are cancelled.
    test_disable_device(&mut ctx, true);
    let _ = ctx.bindings_ctx.take_events();
}

#[test]
fn forget_learned_network_params_on_disable_ipv6() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: Ipv6::TEST_ADDRS.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.into();
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, true), false);

    // Fake discovering a retransmit timer from the network.
    const RETRANSMIT_TIMER: NonZeroDuration =
        unsafe { NonZeroDuration::new_unchecked(Duration::from_secs(1)) };
    let (mut core_ctx, mut bindings_ctx) = ctx.contexts();
    Ipv6DeviceHandler::set_discovered_retrans_timer(
        &mut core_ctx,
        &mut bindings_ctx,
        &device_id,
        RETRANSMIT_TIMER,
    );
    Ipv6DeviceContext::with_network_learned_parameters(&mut core_ctx, &device_id, |params| {
        let Ipv6NetworkLearnedParameters { retrans_timer } = params;
        assert_eq!(retrans_timer, &Some(RETRANSMIT_TIMER))
    });

    // Disable the device and verify the learned parameters are cleared.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, false), true);
    let (mut core_ctx, _bindings_ctx) = ctx.contexts();
    Ipv6DeviceContext::with_network_learned_parameters(&mut core_ctx, &device_id, |params| {
        let Ipv6NetworkLearnedParameters { retrans_timer } = params;
        assert_eq!(retrans_timer, &None);
    });
}

#[test]
fn add_ipv6_address_with_dad_disabled() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    // NB: DAD is disabled on the device by default.
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: Ipv6::TEST_ADDRS.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let ll_addr = Ipv6::TEST_ADDRS.local_mac.to_ipv6_link_local();
    let device_id: DeviceId<FakeBindingsCtx> = ethernet_device_id.clone().into();
    let weak_device_id = device_id.downgrade();

    // Enable the device
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, true), false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            })
        ]
    );

    // Add the address, and expect its assignment state to immediately be
    // `Assigned` (without traversing through `Tentative`).
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device_id, ll_addr.replace_witness().unwrap())
        .expect("add MAC based IPv6 link-local address");

    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
                device: weak_device_id,
                addr: ll_addr.to_witness(),
                state: IpAddressState::Assigned,
                valid_until: Lifetime::Infinite,
                preferred_lifetime: PreferredLifetime::preferred_forever(),
            })
        ]
    );
}

#[test]
fn enable_ipv6_dev_with_dad_disabled() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    // NB: DAD is disabled on the device by default.
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: Ipv6::TEST_ADDRS.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let ll_addr = Ipv6::TEST_ADDRS.local_mac.to_ipv6_link_local();
    let device_id: DeviceId<FakeBindingsCtx> = ethernet_device_id.clone().into();
    let weak_device_id = device_id.downgrade();

    // Add the address. Because the device is disabled, its assignment state
    // should be `Unavailable`.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device_id, ll_addr.replace_witness().unwrap())
        .expect("add MAC based IPv6 link-local address");

    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
            device: weak_device_id.clone(),
            addr: ll_addr.to_witness(),
            state: IpAddressState::Unavailable,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    // Enable the device, and expect to see the address's assignment state
    // change to `Assigned` without traversing through `Tentative`.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, true), false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressStateChanged {
                device: weak_device_id.clone(),
                addr: ll_addr.addr().into(),
                state: IpAddressState::Assigned,
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id,
                ip_enabled: true,
            })
        ]
    );
}

enum Ipv4DadTestOrder {
    // Enable the device, then add the address.
    EnableThenAdd,
    // Add the address, then enable the device.
    AddThenEnable,
}

#[test_case(Ipv4DadTestOrder::EnableThenAdd; "enable_then_add")]
#[test_case(Ipv4DadTestOrder::AddThenEnable; "add_then_enable")]
fn add_ipv4_addr_with_dad(order: Ipv4DadTestOrder) {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    const DAD_TRANSMITS: u16 = 3;

    // Install a device.
    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id: DeviceId<FakeBindingsCtx> = ethernet_device_id.clone().into();
    let _: Ipv4DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv4>()
        .update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    dad_transmits: Some(NonZeroU16::new(DAD_TRANSMITS)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("should successfully update config");

    // Enable the device and return any extra events that were emitted.
    fn enable_device(
        ctx: &mut FakeCtx,
        device_id: &DeviceId<FakeBindingsCtx>,
    ) -> Vec<DispatchedEvent> {
        assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, true), false);
        let mut events = ctx.bindings_ctx.take_events();
        assert_eq!(
            events.pop(),
            Some(DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: device_id.downgrade(),
                ip_enabled: true,
            }))
        );
        events
    }

    // Add the address, and assert that it has the expected state.
    fn add_address(
        ctx: &mut FakeCtx,
        device_id: &DeviceId<FakeBindingsCtx>,
        addr: &AddrSubnet<Ipv4Addr>,
        expected_state: IpAddressState,
    ) {
        let addr_config = Ipv4AddrConfig {
            config: CommonAddressConfig { should_perform_dad: Some(true) },
            ..Default::default()
        };
        ctx.core_api()
            .device_ip::<Ipv4>()
            .add_ip_addr_subnet_with_config(&device_id, addr.clone(), addr_config)
            .expect("failed to add IPv4 Address");
        assert_eq!(
            ctx.bindings_ctx.take_events()[..],
            [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressAdded {
                device: device_id.downgrade(),
                addr: addr.clone(),
                state: expected_state,
                valid_until: Lifetime::Infinite,
                preferred_lifetime: PreferredLifetime::preferred_forever(),
            })]
        );
    }

    let ipv4_addr_subnet = AddrSubnet::new(Ipv4Addr::new([192, 168, 0, 1]), 24).unwrap();

    match order {
        Ipv4DadTestOrder::EnableThenAdd => {
            // Enable the device and then add the address. When the address
            // is added it should immediately progress to "tentative" and start
            // DAD.
            let address_events = enable_device(&mut ctx, &device_id);
            assert_eq!(
                &address_events[..],
                [DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                    device: ethernet_device_id.downgrade(),
                    addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
                })]
            );
            ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
            add_address(&mut ctx, &device_id, &ipv4_addr_subnet, IpAddressState::Tentative);
        }
        Ipv4DadTestOrder::AddThenEnable => {
            // Add the address before enabling the device. It should be
            // "unavailable" without DAD having started. Then, once the device
            // is enabled, it should become "tentative" and start DAD.
            add_address(&mut ctx, &device_id, &ipv4_addr_subnet, IpAddressState::Unavailable);
            ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
            let address_events = enable_device(&mut ctx, &device_id);
            assert_eq!(
                &address_events[..],
                [
                    DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                        device: ethernet_device_id.downgrade(),
                        addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
                    }),
                    DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressStateChanged {
                        device: device_id.downgrade(),
                        addr: ipv4_addr_subnet.addr(),
                        state: IpAddressState::Tentative,
                    })
                ]
            );
        }
    }

    // The stack should have installed a DAD timer for PROBE_WAIT, but not yet
    // sent an ARP probe.
    let expected_timer_id = TimerId::from(
        Ipv4DeviceTimerId::Dad(DadTimerId::new(
            device_id.downgrade(),
            IpDeviceStateContext::<Ipv4, _>::get_address_id(
                &mut ctx.core_ctx(),
                &device_id,
                ipv4_addr_subnet.addr(),
            )
            .unwrap()
            .downgrade(),
        ))
        .into_common(),
    );
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(expected_timer_id.clone(), ..)]);
    ctx.bindings_ctx.with_fake_frame_ctx_mut(|ctx| {
        assert_matches!(&ctx.take_frames()[..], []);
    });

    // Trigger the DAD Timer. Verify an ARP probe was sent and an additional DAD
    // timer was scheduled (once for each DAD_TRANSMITS). There should not be
    // any events emitted yet.
    for _ in 0..DAD_TRANSMITS {
        let (mut core_ctx, bindings_ctx) = ctx.contexts();
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(expected_timer_id.clone()));
        ctx.bindings_ctx.with_fake_frame_ctx_mut(|ctx| {
            let frames = ctx.take_frames();
            let (dev, buf) = assert_matches!(&frames[..], [frame] => frame);
            let dev = assert_matches!(dev, DispatchedFrame::Ethernet(device_id) => device_id);
            assert_eq!(WeakDeviceId::Ethernet(dev.clone()), device_id.downgrade());
            let ArpPacketInfo { target_protocol_address, sender_protocol_address, .. } =
                packet_formats::testutil::parse_arp_packet_in_ethernet_frame(
                    buf,
                    packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
                )
                .expect("should successfully parse ARP packet");
            assert_eq!(target_protocol_address, ipv4_addr_subnet.addr().get());
            // Note: IPv4 ARP Probes have the sender protocol address set to all 0s.
            assert_eq!(sender_protocol_address, Ipv4::UNSPECIFIED_ADDRESS);
        });
        ctx.bindings_ctx
            .timer_ctx()
            .assert_timers_installed_range([(expected_timer_id.clone(), ..)]);
        assert_eq!(ctx.bindings_ctx.take_events()[..], []);
    }

    // Trigger the DadTimer and verify the address became assigned.
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(expected_timer_id.clone()));
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressStateChanged {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.addr(),
            state: IpAddressState::Assigned,
        })]
    );

    // Finally, verify ARP announcements are sent.
    for i in 0..IPV4_DAD_ANNOUNCE_NUM.get() {
        ctx.bindings_ctx.with_fake_frame_ctx_mut(|ctx| {
            let frames = ctx.take_frames();
            let (dev, buf) = assert_matches!(&frames[..], [frame] => frame);
            let dev = assert_matches!(dev, DispatchedFrame::Ethernet(device_id) => device_id);
            assert_eq!(WeakDeviceId::Ethernet(dev.clone()), device_id.downgrade());
            let ArpPacketInfo { target_protocol_address, sender_protocol_address, .. } =
                packet_formats::testutil::parse_arp_packet_in_ethernet_frame(
                    buf,
                    packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
                )
                .expect("should successfully parse ARP packet");
            assert_eq!(target_protocol_address, ipv4_addr_subnet.addr().get());
            // Note: IPv4 ARP Announcements have the sender protocol address set to
            // the address being announced.
            assert_eq!(sender_protocol_address, ipv4_addr_subnet.addr().get());
        });
        if i == IPV4_DAD_ANNOUNCE_NUM.get() - 1 {
            // The last announcement shouldn't have scheduled a timer.
            ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
        } else {
            // Otherwise, trigger the next timer to drive the next announcement.
            let (mut core_ctx, bindings_ctx) = ctx.contexts();
            assert_eq!(
                bindings_ctx.trigger_next_timer(&mut core_ctx),
                Some(expected_timer_id.clone())
            );
        }
    }

    // We shouldn't have sent any more events when exiting Announcing.
    assert_eq!(ctx.bindings_ctx.take_events()[..], []);

    // Simulate, at a later point in time, receiving a conflicting ARP packet.
    // The address should be removed.
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    assert_eq!(
        IpDeviceHandler::<Ipv4, _>::handle_received_dad_packet(
            &mut core_ctx,
            bindings_ctx,
            &device_id,
            ipv4_addr_subnet.addr(),
            Ipv4DadAddressInfo::SourceAddr
        ),
        Some(IpAddressState::Assigned),
    );
    assert_eq!(
        bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressRemoved {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.addr(),
            reason: AddressRemovedReason::Forfeited,
        })]
    );

    // Disable device and take all events to cleanup references.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, false), true);
    let _ = ctx.bindings_ctx.take_events();
}

enum Ipv4DadFailureTestCase {
    /// The received ARP packet will have the same source address as the
    /// address undergoing DAD.
    ConflictingSource,
    /// The received ARP packet will be an ARP probe with the same target
    /// address as the address undergoing DAD.
    ConflictingProbe,
    /// The received ARP packet will have the same target address as the
    /// address undergoing DAD, but it will not be an ARP probe.
    ConflictingNonProbe,
}

#[test_case(Ipv4DadFailureTestCase::ConflictingSource, true; "conflicting_source")]
#[test_case(Ipv4DadFailureTestCase::ConflictingProbe, true; "conflicting_probe")]
#[test_case(Ipv4DadFailureTestCase::ConflictingNonProbe, false; "conflicting_non_probe")]
fn notify_on_dad_failure_ipv4(case: Ipv4DadFailureTestCase, expect_conflict: bool) {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    // Install a device.
    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let remote_mac = Ipv4::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();
    let _: Ipv4DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv4>()
        .update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("should successfully update config");

    // Enable the device.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, true), false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: device_id.downgrade(),
                ip_enabled: true,
            })
        ]
    );

    // Add an IPv4 Address, requesting that DAD be performed.
    let ipv4_addr_subnet = AddrSubnet::new(Ipv4Addr::new([192, 168, 0, 1]), 24).unwrap();
    let config = Ipv4AddrConfig {
        config: CommonAddressConfig { should_perform_dad: Some(true) },
        ..Default::default()
    };
    ctx.core_api()
        .device_ip::<Ipv4>()
        .add_ip_addr_subnet_with_config(&device_id, ipv4_addr_subnet.clone(), config)
        .expect("failed to add IPv4 Address");
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressAdded {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.clone(),
            state: IpAddressState::Tentative,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    let (mut core_ctx, bindings_ctx) = ctx.contexts();

    // Simulate a conflicting ARP packet, and expect the address is removed.
    const OTHER_IP: Ipv4Addr = net_ip_v4!("192.168.0.2");
    let (sender_addr, target_addr, is_probe) = match case {
        Ipv4DadFailureTestCase::ConflictingSource => {
            (ipv4_addr_subnet.addr().get(), OTHER_IP, false)
        }
        Ipv4DadFailureTestCase::ConflictingProbe => {
            (Ipv4::UNSPECIFIED_ADDRESS, ipv4_addr_subnet.addr().get(), true)
        }
        Ipv4DadFailureTestCase::ConflictingNonProbe => {
            (Ipv4::UNSPECIFIED_ADDRESS, ipv4_addr_subnet.addr().get(), false)
        }
    };

    assert_eq!(
        netstack3_ip::device::on_arp_packet(
            &mut core_ctx,
            bindings_ctx,
            &device_id,
            remote_mac,
            remote_mac,
            sender_addr,
            target_addr,
            is_probe,
        ),
        false
    );

    if expect_conflict {
        assert_eq!(
            bindings_ctx.take_events()[..],
            [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressRemoved {
                device: device_id.downgrade(),
                addr: ipv4_addr_subnet.addr(),
                reason: AddressRemovedReason::DadFailed,
            })]
        );
    } else {
        assert_eq!(bindings_ctx.take_events()[..], []);
    }

    // Disable device and take all events to cleanup references.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, false), true);
    let _ = ctx.bindings_ctx.take_events();
}

fn receive_ipv4_packet(ctx: &mut FakeCtx, device_id: &DeviceId<FakeBindingsCtx>) {
    let buf = Ipv4PacketBuilder::new(
        Ipv4::TEST_ADDRS.remote_ip,
        Ipv4::TEST_ADDRS.local_ip,
        64,
        IpProto::Udp.into(),
    )
    .wrap_body(Buf::new(vec![0; 10], ..))
    .serialize_vec_outer(&mut NetworkSerializationContext::default())
    .unwrap()
    .into_inner();

    ctx.test_api().receive_ip_packet::<Ipv4, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        buf,
    );
}

enum Ipv4TentativeAddrTestCase {
    /// Source Address Selection should ignore tentative addresses.
    SourceAddressSelection,
    /// Resolving routes with the tentative address should fail.
    ResolveRoute,
    /// Received IPv4 packets destined to tentative addresses should be dropped.
    ReceivePacket,
    /// Sockets should fail to listen on tentative addresses.
    SocketListen,
}

/// Verify that an IPv4 address isn't used while tentative.
#[test_case(Ipv4TentativeAddrTestCase::SourceAddressSelection; "source_address_selection")]
#[test_case(Ipv4TentativeAddrTestCase::ResolveRoute; "resolve_route")]
#[test_case(Ipv4TentativeAddrTestCase::ReceivePacket; "receive_packet")]
#[test_case(Ipv4TentativeAddrTestCase::SocketListen; "socket_listen")]
fn ipv4_ignores_tentative_addresses(case: Ipv4TentativeAddrTestCase) {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    // Install a device.
    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();
    let _: Ipv4DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv4>()
        .update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .expect("should successfully update config");

    // Enable the device.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, true), false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: device_id.downgrade(),
                ip_enabled: true,
            })
        ]
    );

    // Run setup code based on the test case.
    match case {
        Ipv4TentativeAddrTestCase::SourceAddressSelection
        | Ipv4TentativeAddrTestCase::ResolveRoute => {
            // Add a default route via the device.
            ctx.test_api()
                .add_route(AddableEntryEither::from(AddableEntry::without_gateway(
                    Ipv4::ALL_ADDRS_SUBNET,
                    device_id.clone(),
                    AddableMetric::MetricTracksInterface,
                )))
                .expect("add default route should succeed");
        }
        Ipv4TentativeAddrTestCase::ReceivePacket | Ipv4TentativeAddrTestCase::SocketListen => {}
    }

    // Add the address, and assert that it's tentative.
    let ipv4_addr_subnet = AddrSubnet::new(Ipv4::TEST_ADDRS.local_ip.get(), 24).unwrap();
    let config = Ipv4AddrConfig {
        config: CommonAddressConfig { should_perform_dad: Some(true) },
        ..Default::default()
    };
    ctx.core_api()
        .device_ip::<Ipv4>()
        .add_ip_addr_subnet_with_config(&device_id, ipv4_addr_subnet.clone(), config)
        .expect("failed to add IPv4 Address");

    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressAdded {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.clone(),
            state: IpAddressState::Tentative,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    // Verify the tentative address is ignored for each test case.
    match case {
        Ipv4TentativeAddrTestCase::SourceAddressSelection => {
            // Route lookup should fail. `resolve_route` will use SAS to select
            // the source address for traffic using this route.
            assert_matches!(
                ctx.core_api()
                    .routes::<Ipv4>()
                    .resolve_route(None, &RouteResolveOptions::default()),
                Err(ResolveRouteError::NoSrcAddr)
            );
        }
        Ipv4TentativeAddrTestCase::ResolveRoute => {
            // Route lookup should fail. `resolve_route_with_src_addr` skips SAS
            // because we've provided the source address we'd like to use.
            let src_ip = IpDeviceAddr::new(ipv4_addr_subnet.addr().get()).unwrap();
            assert_matches!(
                ctx.test_api().resolve_route_with_src_addr::<Ipv4>(src_ip, None),
                Err(ResolveRouteError::NoSrcAddr)
            );
        }
        Ipv4TentativeAddrTestCase::ReceivePacket => {
            // Received packets should be dropped.
            receive_ipv4_packet(&mut ctx, &device_id);
            IpCounterExpectations::<Ipv4> {
                receive_ip_packet: 1,
                dropped: 1,
                drop_for_tentative: 1,
                ..Default::default()
            }
            .assert_counters(&ctx.core_ctx(), &device_id);
        }
        Ipv4TentativeAddrTestCase::SocketListen => {
            // Sockets should be unable to listen.
            let sock = ctx.core_api().udp::<Ipv4>().create();
            let addr = ZonedAddr::new(ipv4_addr_subnet.addr(), None)
                .expect("ipv4 addresses don't have zones");
            let result = ctx.core_api().udp::<Ipv4>().listen(&sock, Some(addr), None);
            assert_matches!(result, Err(Either::Right(LocalAddressError::CannotBindToAddress)));
        }
    }

    // Trigger the DAD timer and verify the address becomes assigned.
    let expected_timer_id = TimerId::from(
        Ipv4DeviceTimerId::Dad(DadTimerId::new(
            device_id.downgrade(),
            IpDeviceStateContext::<Ipv4, _>::get_address_id(
                &mut ctx.core_ctx(),
                &device_id,
                ipv4_addr_subnet.addr(),
            )
            .unwrap()
            .downgrade(),
        ))
        .into_common(),
    );
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    // Triggering the first timer progresses past the PROBE_WAIT stage.
    // After which, an ARP probe is sent, but we ignore that here because it's
    // verified in other tests.
    assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(expected_timer_id.clone()));
    // Triggering the second timer progresses past the tentative stage.
    assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(expected_timer_id));
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressStateChanged {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.addr(),
            state: IpAddressState::Assigned,
        })]
    );

    // Verify the address is no longer ignored, now that it's assigned.
    match case {
        Ipv4TentativeAddrTestCase::SourceAddressSelection => {
            // Route lookup (with SAS) should succeed.
            assert_matches!(
                ctx.core_api()
                    .routes::<Ipv4>()
                    .resolve_route(None, &RouteResolveOptions::default()),
                Ok(_)
            );
        }
        Ipv4TentativeAddrTestCase::ResolveRoute => {
            // Route lookup (without SAS) should succeed.
            let src_ip = IpDeviceAddr::new(ipv4_addr_subnet.addr().get()).unwrap();
            assert_matches!(
                ctx.test_api().resolve_route_with_src_addr::<Ipv4>(src_ip, None),
                Ok(_)
            );
        }
        Ipv4TentativeAddrTestCase::ReceivePacket => {
            // Received packets should be dispatched.
            receive_ipv4_packet(&mut ctx, &device_id);
            IpCounterExpectations::<Ipv4> {
                receive_ip_packet: 2,
                deliver_unicast: 1,
                dispatch_receive_ip_packet: 1,
                dropped: 1,
                drop_for_tentative: 1,
                ..Default::default()
            }
            .assert_counters(&ctx.core_ctx(), &device_id);
        }
        Ipv4TentativeAddrTestCase::SocketListen => {
            // Sockets should be able to listen.
            let sock = ctx.core_api().udp::<Ipv4>().create();
            let addr = ZonedAddr::new(ipv4_addr_subnet.addr(), None)
                .expect("ipv4 addresses don't have zones");
            ctx.core_api()
                .udp::<Ipv4>()
                .listen(&sock, Some(addr), None)
                .expect("listen should succeed");
        }
    }

    // Disable device and take all events to cleanup references.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, false), true);
    let _ = ctx.bindings_ctx.take_events();
}

// Adds an IPv4 address with DAD disabled, and then verifies that a conflicting
// ARP packet does not cause the address to be removed.
#[test]
fn ipv4_dad_conflict_with_dad_disabled() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    // Install a device.
    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();
    // Enable the device.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, true), false);
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv4::ALL_SYSTEMS_MULTICAST_ADDRESS.into()
            }),
            DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::EnabledChanged {
                device: device_id.downgrade(),
                ip_enabled: true,
            })
        ]
    );

    // Add the address, and assert that it immediately becomes assigned.
    let ipv4_addr_subnet = AddrSubnet::new(Ipv4Addr::new([192, 168, 0, 1]), 24).unwrap();
    ctx.core_api()
        .device_ip::<Ipv4>()
        .add_ip_addr_subnet(&device_id, ipv4_addr_subnet.clone())
        .expect("failed to add IPv4 Address");
    assert_eq!(
        ctx.bindings_ctx.take_events()[..],
        [DispatchedEvent::IpDeviceIpv4(IpDeviceEvent::AddressAdded {
            device: device_id.downgrade(),
            addr: ipv4_addr_subnet.clone(),
            state: IpAddressState::Assigned,
            valid_until: Lifetime::Infinite,
            preferred_lifetime: PreferredLifetime::preferred_forever(),
        })]
    );

    // Simulate receiving a conflicting ARP packet. The address should not be
    // removed.
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    assert_eq!(
        IpDeviceHandler::<Ipv4, _>::handle_received_dad_packet(
            &mut core_ctx,
            bindings_ctx,
            &device_id,
            ipv4_addr_subnet.addr(),
            Ipv4DadAddressInfo::SourceAddr
        ),
        Some(IpAddressState::Assigned),
    );
    assert_eq!(bindings_ctx.take_events()[..], []);

    // Disable device and take all events to cleanup references.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv4>(&device_id, false), true);
    let _ = ctx.bindings_ctx.take_events();
}

#[test]
fn notify_on_dad_failure_ipv6() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let local_mac = Ipv6::TEST_ADDRS.local_mac;
    let ethernet_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = ethernet_device_id.clone().into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(NonZeroU8::new(1)),
                // Auto-generate a link-local address.
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    gmp_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let ll_addr = local_mac.to_ipv6_link_local();

    enable_ipv6_device(&mut ctx, &device_id, ll_addr, false);
    let weak_device_id = device_id.downgrade();
    assert_eq!(
        &ctx.bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.into(),
            }),
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: ll_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
                device: weak_device_id.clone(),
                addr: ll_addr.to_witness(),
                state: IpAddressState::Tentative,
                valid_until: Lifetime::Infinite,
                preferred_lifetime: PreferredLifetime::preferred_forever(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::EnabledChanged {
                device: weak_device_id.clone(),
                ip_enabled: true,
            }),
        ]
    );

    let assigned_addr = AddrSubnet::new(net_ip_v6!("fe80::1"), 64).unwrap();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device_id, assigned_addr)
        .expect("add succeeds");
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    assert_eq!(
        bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastJoin {
                device: ethernet_device_id.downgrade(),
                addr: assigned_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressAdded {
                device: weak_device_id.clone(),
                addr: assigned_addr.to_witness(),
                state: IpAddressState::Tentative,
                valid_until: Lifetime::Infinite,
                preferred_lifetime: PreferredLifetime::preferred_forever(),
            }),
        ]
    );

    // When DAD fails, an event should be emitted and the address should be
    // removed.
    assert_eq!(
        IpDeviceHandler::<Ipv6, _>::handle_received_dad_packet(
            &mut core_ctx,
            bindings_ctx,
            &device_id,
            assigned_addr.addr(),
            None
        ),
        Some(IpAddressState::Tentative),
    );

    assert_eq!(
        bindings_ctx.take_events()[..],
        [
            DispatchedEvent::EthernetDevice(EthernetDeviceEvent::MulticastLeave {
                device: ethernet_device_id.downgrade(),
                addr: assigned_addr.addr().to_solicited_node_address().into(),
            }),
            DispatchedEvent::IpDeviceIpv6(IpDeviceEvent::AddressRemoved {
                device: weak_device_id,
                addr: assigned_addr.addr(),
                reason: AddressRemovedReason::DadFailed,
            }),
        ]
    );

    assert_eq!(
        IpDeviceStateContext::<Ipv6, _>::with_address_ids(
            &mut core_ctx,
            &device_id,
            |addrs, _core_ctx| {
                addrs.map(|addr_id| addr_id.addr_sub().addr().get()).collect::<HashSet<_>>()
            }
        ),
        HashSet::from([ll_addr.ipv6_unicast_addr()]),
        "manual addresses should be removed on DAD failure"
    );
    // Disable device and take all events to cleanup references.
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, false), true);
    let _ = ctx.bindings_ctx.take_events();
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn update_ip_device_configuration_err<I: IpExt>() {
    let mut ctx = FakeCtx::default();

    let loopback_device_id = ctx
        .core_api()
        .device::<LoopbackDevice>()
        .add_device_with_default_state(
            LoopbackCreationProperties { mtu: Mtu::new(u16::MAX as u32) },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let mut api = ctx.core_api().device_ip::<I>();
    let original_state = api.get_configuration(&loopback_device_id);
    assert_eq!(
        api.update_configuration(
            &loopback_device_id,
            IpDeviceConfigurationUpdate {
                ip_enabled: Some(!AsRef::<IpDeviceFlags>::as_ref(&original_state).ip_enabled),
                gmp_enabled: Some(
                    !AsRef::<IpDeviceConfiguration>::as_ref(&original_state).gmp_enabled
                ),
                unicast_forwarding_enabled: Some(true),
                multicast_forwarding_enabled: None,
                dad_transmits: None,
            }
            .into(),
        )
        .unwrap_err(),
        UpdateIpConfigurationError::UnicastForwardingNotSupported,
    );
    assert_eq!(original_state, api.get_configuration(&loopback_device_id));
}

#[test]
fn update_ipv4_configuration_return() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let local_mac = Ipv4::TEST_ADDRS.local_mac;
    let device_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let mut api = ctx.core_api().device_ip::<Ipv4>();
    // Perform no update.
    assert_eq!(
        api.update_configuration(&device_id, Ipv4DeviceConfigurationUpdate::default()),
        Ok(Ipv4DeviceConfigurationUpdate::default()),
    );

    // Enable all but forwarding. All features are initially disabled.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    unicast_forwarding_enabled: Some(false),
                    multicast_forwarding_enabled: Some(false),
                    gmp_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                },
                igmp_mode: Some(IgmpConfigMode::V1),
            },
        ),
        Ok(Ipv4DeviceConfigurationUpdate {
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(false),
                unicast_forwarding_enabled: Some(false),
                multicast_forwarding_enabled: Some(false),
                gmp_enabled: Some(false),
                dad_transmits: Some(None),
            },
            igmp_mode: Some(IgmpConfigMode::V3),
        }),
    );

    // Change forwarding config.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    unicast_forwarding_enabled: Some(true),
                    multicast_forwarding_enabled: Some(true),
                    gmp_enabled: None,
                    dad_transmits: None,
                },
                igmp_mode: None,
            },
        ),
        Ok(Ipv4DeviceConfigurationUpdate {
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(true),
                unicast_forwarding_enabled: Some(false),
                multicast_forwarding_enabled: Some(false),
                gmp_enabled: None,
                dad_transmits: None,
            },
            igmp_mode: None,
        }),
    );

    // No update to anything (GMP enabled set to already set
    // value).
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: None,
                    unicast_forwarding_enabled: None,
                    multicast_forwarding_enabled: None,
                    gmp_enabled: Some(true),
                    dad_transmits: None,
                },
                igmp_mode: None,
            },
        ),
        Ok(Ipv4DeviceConfigurationUpdate {
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: None,
                unicast_forwarding_enabled: None,
                multicast_forwarding_enabled: None,
                gmp_enabled: Some(true),
                dad_transmits: None,
            },
            igmp_mode: None,
        }),
    );

    // Disable/change everything.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv4DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(false),
                    unicast_forwarding_enabled: Some(false),
                    multicast_forwarding_enabled: Some(false),
                    gmp_enabled: Some(false),
                    dad_transmits: Some(None),
                },
                igmp_mode: Some(IgmpConfigMode::V3),
            },
        ),
        Ok(Ipv4DeviceConfigurationUpdate {
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(true),
                unicast_forwarding_enabled: Some(true),
                multicast_forwarding_enabled: Some(true),
                gmp_enabled: Some(true),
                dad_transmits: Some(NonZeroU16::new(1)),
            },
            igmp_mode: Some(IgmpConfigMode::V1),
        }),
    );
}

#[test]
fn update_ipv6_configuration_return() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let local_mac = Ipv6::TEST_ADDRS.local_mac;
    let device_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv6::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let mut api = ctx.core_api().device_ip::<Ipv6>();

    // Perform no update.
    assert_eq!(
        api.update_configuration(&device_id, Ipv6DeviceConfigurationUpdate::default()),
        Ok(Ipv6DeviceConfigurationUpdate::default()),
    );

    // Enable all but forwarding. All features are initially disabled.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(NonZeroU8::new(2)),
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64
                    ),
                    temporary_address_configuration: Some(
                        TemporarySlaacAddressConfiguration::enabled_with_rfc_defaults()
                    ),
                },
                route_discovery_config: RouteDiscoveryConfigurationUpdate {
                    allow_default_route: Some(false),
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    unicast_forwarding_enabled: Some(false),
                    multicast_forwarding_enabled: Some(false),
                    gmp_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                },
                mld_mode: Some(MldConfigMode::V1),
            },
        ),
        Ok(Ipv6DeviceConfigurationUpdate {
            max_router_solicitations: Some(None),
            slaac_config: SlaacConfigurationUpdate {
                stable_address_configuration: Some(StableSlaacAddressConfiguration::Disabled),
                temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Disabled),
            },
            route_discovery_config: RouteDiscoveryConfigurationUpdate {
                allow_default_route: Some(true),
            },
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(false),
                unicast_forwarding_enabled: Some(false),
                multicast_forwarding_enabled: Some(false),
                gmp_enabled: Some(false),
                dad_transmits: Some(None),
            },
            mld_mode: Some(MldConfigMode::V2),
        }),
    );

    // Change forwarding config.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: None,
                slaac_config: SlaacConfigurationUpdate::default(),
                route_discovery_config: RouteDiscoveryConfigurationUpdate::default(),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    unicast_forwarding_enabled: Some(true),
                    multicast_forwarding_enabled: Some(true),
                    gmp_enabled: None,
                    dad_transmits: None,
                },
                mld_mode: None,
            },
        ),
        Ok(Ipv6DeviceConfigurationUpdate {
            max_router_solicitations: None,
            slaac_config: SlaacConfigurationUpdate::default(),
            route_discovery_config: RouteDiscoveryConfigurationUpdate::default(),
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(true),
                unicast_forwarding_enabled: Some(false),
                multicast_forwarding_enabled: Some(false),
                gmp_enabled: None,
                dad_transmits: None,
            },
            mld_mode: None,
        }),
    );

    // No update to anything (GMP enabled set to already set
    // value).
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: None,
                slaac_config: SlaacConfigurationUpdate::default(),
                route_discovery_config: RouteDiscoveryConfigurationUpdate::default(),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: None,
                    unicast_forwarding_enabled: None,
                    multicast_forwarding_enabled: None,
                    gmp_enabled: Some(true),
                    dad_transmits: None,
                },
                mld_mode: None,
            },
        ),
        Ok(Ipv6DeviceConfigurationUpdate {
            max_router_solicitations: None,
            slaac_config: SlaacConfigurationUpdate::default(),
            route_discovery_config: RouteDiscoveryConfigurationUpdate::default(),
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: None,
                unicast_forwarding_enabled: None,
                multicast_forwarding_enabled: None,
                gmp_enabled: Some(true),
                dad_transmits: None,
            },
            mld_mode: None,
        }),
    );

    // Disable/change everything.
    assert_eq!(
        api.update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(None),
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(StableSlaacAddressConfiguration::Disabled),
                    temporary_address_configuration: Some(
                        TemporarySlaacAddressConfiguration::Disabled
                    ),
                },
                route_discovery_config: RouteDiscoveryConfigurationUpdate {
                    allow_default_route: Some(true),
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(false),
                    unicast_forwarding_enabled: Some(false),
                    multicast_forwarding_enabled: Some(false),
                    gmp_enabled: Some(false),
                    dad_transmits: Some(None),
                },
                mld_mode: Some(MldConfigMode::V2),
            },
        ),
        Ok(Ipv6DeviceConfigurationUpdate {
            max_router_solicitations: Some(NonZeroU8::new(2)),
            slaac_config: SlaacConfigurationUpdate {
                stable_address_configuration: Some(
                    StableSlaacAddressConfiguration::ENABLED_WITH_EUI64
                ),
                temporary_address_configuration: Some(
                    TemporarySlaacAddressConfiguration::enabled_with_rfc_defaults()
                ),
            },
            route_discovery_config: RouteDiscoveryConfigurationUpdate {
                allow_default_route: Some(false),
            },
            ip_config: IpDeviceConfigurationUpdate {
                ip_enabled: Some(true),
                unicast_forwarding_enabled: Some(true),
                multicast_forwarding_enabled: Some(true),
                gmp_enabled: Some(true),
                dad_transmits: Some(NonZeroU16::new(1)),
            },
            mld_mode: Some(MldConfigMode::V1),
        }),
    );
}

#[test_case(false; "stable addresses enabled generates link local")]
#[test_case(true; "stable addresses disabled does not generate link local")]
fn configure_link_local_address_generation(enable_stable_addresses: bool) {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    let local_mac = Ipv6::TEST_ADDRS.local_mac;
    let device_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let new_config = Ipv6DeviceConfigurationUpdate {
        slaac_config: SlaacConfigurationUpdate {
            stable_address_configuration: Some(if enable_stable_addresses {
                StableSlaacAddressConfiguration::ENABLED_WITH_EUI64
            } else {
                StableSlaacAddressConfiguration::Disabled
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let _prev: Ipv6DeviceConfigurationUpdate =
        ctx.core_api().device_ip::<Ipv6>().update_configuration(&device_id, new_config).unwrap();
    assert_eq!(ctx.test_api().set_ip_device_enabled::<Ipv6>(&device_id, true), false);

    let expected_addrs = if enable_stable_addresses {
        HashSet::from([local_mac.to_ipv6_link_local().ipv6_unicast_addr()])
    } else {
        HashSet::new()
    };

    assert_eq!(
        IpDeviceStateContext::<Ipv6, _>::with_address_ids(
            &mut ctx.core_ctx(),
            &device_id,
            |addrs, _core_ctx| {
                addrs.map(|addr_id| addr_id.addr_sub().addr().get()).collect::<HashSet<_>>()
            }
        ),
        expected_addrs,
    );
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn disallow_loopback_addrs_on_non_loopback_interface<I: IpExt + TestIpExt>() {
    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());
    let device_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: I::TEST_ADDRS.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let result = ctx.core_api().device_ip::<I>().add_ip_addr_subnet(
        &device_id,
        AddrSubnet::new(I::LOOPBACK_ADDRESS.get(), I::LOOPBACK_SUBNET.prefix()).unwrap(),
    );
    assert_eq!(result, Err(AddIpAddrSubnetError::InvalidAddr));
}
