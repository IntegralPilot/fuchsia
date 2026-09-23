// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryInto as _;
use core::fmt::Debug;
use core::num::{NonZeroU8, NonZeroU16};
use core::time::Duration;

use assert_matches::assert_matches;
use log::trace;
use net_declare::net::{mac, subnet_v6};
use net_declare::net_ip_v6;
use net_types::ethernet::Mac;
use net_types::ip::{AddrSubnet, Ip as _, Ipv6, Ipv6Addr, Ipv6Scope, Mtu, Subnet};
use net_types::{NonMappedAddr, ScopeableAddress as _, UnicastAddr, Witness as _};
use packet::{
    Buf, EmptyBuf, InnerPacketBuilder as _, NestablePacketBuilder as _, NestableSerializer as _,
    Serializer as _,
};
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::icmp::ndp::options::{NdpOption, NdpOptionBuilder, PrefixInformation};
use packet_formats::icmp::ndp::{
    NeighborAdvertisement, NeighborSolicitation, OptionSequenceBuilder, Options,
    RouterAdvertisement, RouterSolicitation,
};
use packet_formats::icmp::{IcmpEchoRequest, IcmpPacketBuilder, IcmpZeroCode};
use packet_formats::ip::{DscpAndEcn, IpProto, Ipv6Proto};
use packet_formats::ipv6::Ipv6PacketBuilder;
use packet_formats::testutil::{
    parse_ethernet_frame, parse_icmp_packet_in_ip_packet_in_ethernet_frame,
};
use packet_formats::utils::NonZeroDuration;
use rand::RngExt as _;
use test_case::test_case;
use zerocopy::SplitByteSlice;

use netstack3_base::testutil::{
    FakeInstant, FakeNetwork, FakeNetworkLinks, StepResult, TEST_ADDRS_V6, TestIpExt, assert_empty,
    set_logger_for_test,
};
use netstack3_base::{
    FrameDestination, InstantContext as _, IpAddressId as _, Ipv6DeviceAddr, LinkAddress,
    NetworkSerializationContext, RngContext as _,
};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetDeviceId, EthernetLinkDevice,
    MaxEthernetFrameSize,
};
use netstack3_core::testutil::{
    Ctx, CtxPairExt as _, DEFAULT_INTERFACE_METRIC, DispatchedFrame, FakeBindingsCtx, FakeCtx,
    FakeCtxBuilder, FakeCtxNetworkSpec, new_simple_fake_network,
};
use netstack3_core::{BindingsTypes, Instant, TimerId};
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use netstack3_hashmap::{HashMap, HashSet};
use netstack3_ip::device::testutil::{calculate_slaac_addr_sub, with_assigned_ipv6_addr_subnets};
use netstack3_ip::device::{
    IidGenerationConfiguration, InnerSlaacTimerId, IpAddressData, IpAddressFlags,
    IpDeviceBindingsContext, IpDeviceConfigurationUpdate, IpDeviceStateContext,
    Ipv4DeviceConfigurationUpdate, Ipv6AddrConfig, Ipv6AddrSlaacConfig,
    Ipv6DeviceConfigurationContext, Ipv6DeviceConfigurationUpdate, Ipv6DeviceHandler,
    Ipv6DeviceTimerId, Lifetime, MAX_RTR_SOLICITATION_DELAY, OpaqueIid, OpaqueIidNonce,
    PreferredLifetime, RTR_SOLICITATION_INTERVAL, SlaacBindingsContext, SlaacConfig,
    SlaacConfigurationUpdate, SlaacContext, SlaacTimerId, StableSlaacAddressConfiguration,
    TemporarySlaacAddressConfiguration, TemporarySlaacConfig, get_ipv6_hop_limit,
};
use netstack3_ip::icmp::REQUIRED_NDP_IP_PACKET_HOP_LIMIT;
use netstack3_ip::{self as ip, IpPacketDestination, SendIpPacketMeta};

#[derive(Debug, PartialEq, Copy, Clone)]
struct SlaacAddr<I> {
    addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
    flags: IpAddressFlags,
    config: Ipv6AddrConfig<I>,
}

impl<I> SlaacAddr<I> {
    fn addr_sub(&self) -> &AddrSubnet<Ipv6Addr, Ipv6DeviceAddr> {
        &self.addr_sub
    }
}

fn get_slaac_addrs(
    ctx: &FakeCtx,
    device_id: &DeviceId<FakeBindingsCtx>,
    scope: Option<Ipv6Scope>,
) -> Vec<SlaacAddr<FakeInstant>> {
    ip::device::IpDeviceStateContext::<Ipv6, _>::with_address_ids(
        &mut ctx.core_ctx(),
        device_id,
        |addrs, core_ctx| {
            addrs
                .filter_map(|addr_id| {
                    let addr_sub = addr_id.addr_sub();
                    if scope.map(|scope| scope != addr_sub.addr().scope()).unwrap_or(false) {
                        return None;
                    }
                    ip::device::IpDeviceAddressContext::<Ipv6, _>::with_ip_address_data(
                        core_ctx,
                        device_id,
                        &addr_id,
                        |IpAddressData { flags, config }| {
                            Some(SlaacAddr { addr_sub, flags: *flags, config: config.unwrap() })
                        },
                    )
                })
                .collect()
        },
    )
}

fn get_global_ipv6_addrs(
    ctx: &FakeCtx,
    device_id: &DeviceId<FakeBindingsCtx>,
) -> Vec<SlaacAddr<FakeInstant>> {
    get_slaac_addrs(ctx, device_id, Some(Ipv6Scope::Global))
}

// TODO(https://github.com/rust-lang/rust/issues/67441): Make these constants once const
// Option::unwrap is stablized
fn local_mac() -> UnicastAddr<Mac> {
    UnicastAddr::new(Mac::new([0, 1, 2, 3, 4, 5])).unwrap()
}

fn remote_mac() -> UnicastAddr<Mac> {
    UnicastAddr::new(Mac::new([6, 7, 8, 9, 10, 11])).unwrap()
}

fn local_ip() -> Ipv6DeviceAddr {
    TEST_ADDRS_V6.local_non_mapped_unicast()
}

fn remote_ip() -> Ipv6DeviceAddr {
    TEST_ADDRS_V6.remote_non_mapped_unicast()
}

fn is_temporary_slaac_addr<Instant>(config: &Ipv6AddrConfig<Instant>) -> bool {
    match config {
        Ipv6AddrConfig::Manual(_) => false,
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => match inner {
            SlaacConfig::Stable { .. } => false,
            SlaacConfig::Temporary(_) => true,
        },
    }
}

fn setup_net() -> (
    FakeNetwork<
        FakeCtxNetworkSpec,
        &'static str,
        impl FakeNetworkLinks<DispatchedFrame, EthernetDeviceId<FakeBindingsCtx>, &'static str>,
    >,
    EthernetDeviceId<FakeBindingsCtx>,
    EthernetDeviceId<FakeBindingsCtx>,
) {
    let mut local = FakeCtxBuilder::default();
    let local_dev_idx = local.add_device_with_config(
        local_mac(),
        Ipv4DeviceConfigurationUpdate::default(),
        Ipv6DeviceConfigurationUpdate {
            slaac_config: SlaacConfigurationUpdate {
                stable_address_configuration: Some(
                    StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                ),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let mut remote = FakeCtxBuilder::default();
    let remote_dev_idx = remote.add_device_with_config(
        remote_mac(),
        Ipv4DeviceConfigurationUpdate::default(),
        Ipv6DeviceConfigurationUpdate {
            slaac_config: SlaacConfigurationUpdate {
                stable_address_configuration: Some(
                    StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                ),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let (local, local_device_ids) = local.build();
    let (remote, remote_device_ids) = remote.build();

    let local_eth_device_id = local_device_ids[local_dev_idx].clone();
    let remote_eth_device_id = remote_device_ids[remote_dev_idx].clone();
    let net = new_simple_fake_network(
        "local",
        local,
        local_eth_device_id.downgrade(),
        "remote",
        remote,
        remote_eth_device_id.downgrade(),
    );

    (net, local_eth_device_id, remote_eth_device_id)
}

#[test]
fn test_address_resolution() {
    set_logger_for_test();
    let (net, local_eth_device_id, remote_eth_device_id) = setup_net();

    // Remove the devices so that existing NUD timers get cleaned up;
    // otherwise, they would hold dangling references to the device when the
    // core context is dropped at the end of the test.
    let local_device_id_clone = local_eth_device_id.clone();
    let remote_device_id_clone = remote_eth_device_id.clone();
    let mut net = scopeguard::guard(net, move |mut net| {
        net.with_context("local", |ctx| {
            ctx.core_api().device().remove_device(local_device_id_clone).into_removed();
        });
        net.with_context("remote", |ctx| {
            ctx.core_api().device().remove_device(remote_device_id_clone).into_removed();
        });
    });

    let local_device_id = local_eth_device_id.into();
    let remote_device_id = remote_eth_device_id.into();

    // Let's try to ping the remote device from the local device:
    let req = IcmpEchoRequest::new(0, 0);
    let req_body = &[1, 2, 3, 4];
    let body = IcmpPacketBuilder::<Ipv6, _>::new(local_ip(), remote_ip(), IcmpZeroCode, req)
        .wrap_body(Buf::new(req_body.to_vec(), ..));
    // Manually assigning the addresses.
    net.with_context("remote", |ctx| {
        ctx.core_api()
            .device_ip::<Ipv6>()
            .add_ip_addr_subnet(
                &remote_device_id,
                AddrSubnet::new(remote_ip().into(), 128).unwrap(),
            )
            .unwrap();
        assert_matches!(ctx.bindings_ctx.copy_ethernet_frames()[..], []);
    });
    net.with_context("local", |mut ctx| {
        ctx.core_api()
            .device_ip::<Ipv6>()
            .add_ip_addr_subnet(&local_device_id, AddrSubnet::new(local_ip().into(), 128).unwrap())
            .unwrap();
        let Ctx { core_ctx, bindings_ctx } = &mut ctx;
        assert_matches!(bindings_ctx.copy_ethernet_frames()[..], []);

        ip::IpLayerHandler::<Ipv6, _>::send_ip_packet_from_device(
            &mut core_ctx.context(),
            bindings_ctx,
            SendIpPacketMeta {
                device: &local_device_id,
                src_ip: Some(local_ip().into_specified()),
                dst_ip: remote_ip().into_specified(),
                destination: IpPacketDestination::from_addr(remote_ip().into()),
                proto: Ipv6Proto::Icmpv6,
                ttl: None,
                mtu: Mtu::no_limit(),
                dscp_and_ecn: DscpAndEcn::default(),
            },
            body,
        )
        .unwrap();
        // This should've triggered a neighbor solicitation to come out of
        // local.
        assert_matches!(&bindings_ctx.copy_ethernet_frames()[..], [_frame]);
        // A timer should've been started.
        assert_eq!(bindings_ctx.timer_ctx().timers().len(), 1);
    });

    let _: StepResult = net.step();
    assert_eq!(
        net.context("remote").core_ctx.ipv6().icmp.ndp_counters.rx.neighbor_solicitation.get(),
        1,
        "remote received solicitation"
    );
    assert_matches!(&net.context("remote").bindings_ctx.copy_ethernet_frames()[..], [_frame]);

    // Forward advertisement response back to local.
    let _: StepResult = net.step();

    assert_eq!(
        net.context("local").core_ctx.ipv6().icmp.ndp_counters.rx.neighbor_advertisement.get(),
        1,
        "local received advertisement"
    );

    // Upon link layer resolution, the original ping request should've been
    // sent out.
    net.with_context("local", |Ctx { core_ctx: _, bindings_ctx }| {
        assert_matches!(&bindings_ctx.copy_ethernet_frames()[..], [_frame]);
    });
    let _: StepResult = net.step();
    assert_eq!(
        net.context("remote").core_ctx.common_icmp::<Ipv6>().rx_counters.echo_request.get(),
        1
    );

    // TODO(brunodalbo): We should be able to verify that remote also sends
    //  back an echo reply, but we're having some trouble with IPv6 link
    //  local addresses.
}

#[test]
fn test_dad_duplicate_address_detected_solicitation() {
    // Tests whether a duplicate address will get detected by solicitation
    // In this test, two nodes having the same MAC address will come up at
    // the same time. And both of them will use the EUI address. Each of
    // them should be able to detect each other is using the same address,
    // so they will both give up using that address.
    set_logger_for_test();
    let mac = UnicastAddr::new(Mac::new([6, 5, 4, 3, 2, 1])).unwrap();
    let ll_addr: Ipv6Addr = mac.to_ipv6_link_local().addr().get();
    let multicast_addr = ll_addr.to_solicited_node_address();

    // Create the devices (will start DAD at the same time).
    let make_ctx_and_dev = || {
        let mut ctx = FakeCtx::default();
        let device_id =
            ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
                EthernetCreationProperties {
                    tx_offload_spec: Default::default(),
                    mac,
                    max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
                },
                DEFAULT_INTERFACE_METRIC,
            );
        (ctx, device_id)
    };

    let (local, local_device_id) = make_ctx_and_dev();
    let (remote, remote_device_id) = make_ctx_and_dev();

    // Seeded to avoid the local and remote stacks generating the same nonces.
    local.bindings_ctx.seed_rng(1);
    remote.bindings_ctx.seed_rng(2);

    let mut net = new_simple_fake_network(
        "local",
        local,
        local_device_id.downgrade(),
        "remote",
        remote,
        remote_device_id.downgrade(),
    );
    let local_device_id = local_device_id.into();
    let remote_device_id = remote_device_id.into();

    // Create the devices (will start DAD at the same time).
    let update = Ipv6DeviceConfigurationUpdate {
        slaac_config: SlaacConfigurationUpdate {
            stable_address_configuration: Some(StableSlaacAddressConfiguration::ENABLED_WITH_EUI64),
            ..Default::default()
        },
        ip_config: IpDeviceConfigurationUpdate {
            ip_enabled: Some(true),
            // Doesn't matter as long as we perform DAD.
            dad_transmits: Some(NonZeroU16::new(1)),
            ..Default::default()
        },
        ..Default::default()
    };
    net.with_context("local", |ctx| {
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(&local_device_id, update)
            .unwrap();
        assert_matches!(&ctx.bindings_ctx.copy_ethernet_frames()[..], [_frame]);
    });
    net.with_context("remote", |ctx| {
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(&remote_device_id, update)
            .unwrap();
        assert_matches!(&ctx.bindings_ctx.copy_ethernet_frames()[..], [_frame]);
    });

    // Both devices should be in the solicited-node multicast group.
    assert!(net.context("local").test_api().is_in_ip_multicast(&local_device_id, multicast_addr));
    assert!(net.context("remote").test_api().is_in_ip_multicast(&remote_device_id, multicast_addr));

    let _: StepResult = net.step();

    // They should now realize the address they intend to use has a
    // duplicate in the local network.
    with_assigned_ipv6_addr_subnets(
        &mut net.context("local").core_ctx(),
        &local_device_id,
        |addrs| assert_empty(addrs),
    );
    with_assigned_ipv6_addr_subnets(
        &mut net.context("remote").core_ctx(),
        &remote_device_id,
        |addrs| assert_empty(addrs),
    );

    // Both devices should not be in the multicast group
    assert!(!net.context("local").test_api().is_in_ip_multicast(&local_device_id, multicast_addr));
    assert!(
        !net.context("remote").test_api().is_in_ip_multicast(&remote_device_id, multicast_addr)
    );
}

fn dad_timer_id(
    ctx: &mut FakeCtx,
    id: EthernetDeviceId<FakeBindingsCtx>,
    addr: Ipv6DeviceAddr,
) -> TimerId<FakeBindingsCtx> {
    TimerId::from(
        Ipv6DeviceTimerId::Dad(ip::device::DadTimerId::new(
            id.downgrade().into(),
            IpDeviceStateContext::<Ipv6, _>::get_address_id(
                &mut ctx.core_ctx(),
                &id.into(),
                addr.into_specified(),
            )
            .unwrap()
            .downgrade(),
        ))
        .into_common(),
    )
}

fn rs_timer_id(id: EthernetDeviceId<FakeBindingsCtx>) -> TimerId<FakeBindingsCtx> {
    TimerId::from(
        Ipv6DeviceTimerId::Rs(ip::device::RsTimerId::new(id.downgrade().into())).into_common(),
    )
}

#[test]
fn test_dad_duplicate_address_detected_advertisement() {
    // Tests whether a duplicate address will get detected by advertisement
    // In this test, one of the node first assigned itself the local_ip(),
    // then the second node comes up and it should be able to find out that
    // it cannot use the address because someone else has already taken that
    // address.
    set_logger_for_test();
    let (mut net, local_eth_device_id, remote_eth_device_id) = setup_net();
    let local_device_id = local_eth_device_id.clone().into();
    let remote_device_id = remote_eth_device_id.into();

    // Enable DAD.
    let update = Ipv6DeviceConfigurationUpdate {
        ip_config: IpDeviceConfigurationUpdate {
            ip_enabled: Some(true),
            // Doesn't matter as long as we perform DAD.
            dad_transmits: Some(NonZeroU16::new(1)),
            ..Default::default()
        },
        ..Default::default()
    };
    let addr = AddrSubnet::<Ipv6Addr, _>::new(local_ip().into(), 128).unwrap();
    let multicast_addr = local_ip().to_solicited_node_address();
    net.with_context("local", |ctx| {
        // Seeded to avoid the local and remote stacks generating the same nonces.
        ctx.bindings_ctx.seed_rng(1);

        let mut api = ctx.core_api().device_ip::<Ipv6>();
        let _: Ipv6DeviceConfigurationUpdate =
            api.update_configuration(&local_device_id, update).unwrap();
        api.add_ip_addr_subnet(&local_device_id, addr).unwrap();
    });
    net.with_context("remote", |ctx| {
        // Seeded to avoid the local and remote stacks generating the same nonces.
        ctx.bindings_ctx.seed_rng(2);
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(&remote_device_id, update)
            .unwrap();
    });

    // Only local should be in the solicited node multicast group.
    assert!(net.context("local").test_api().is_in_ip_multicast(&local_device_id, multicast_addr));
    assert!(
        !net.context("remote").test_api().is_in_ip_multicast(&remote_device_id, multicast_addr)
    );

    net.with_context("local", |ctx| {
        assert_eq!(
            ctx.trigger_next_timer().unwrap(),
            dad_timer_id(ctx, local_eth_device_id, local_ip())
        );
    });

    assert_eq!(
        get_address_assigned(net.context("local"), &local_device_id, local_ip()),
        Some(true)
    );

    net.with_context("remote", |ctx| {
        ctx.core_api().device_ip::<Ipv6>().add_ip_addr_subnet(&remote_device_id, addr).unwrap();
    });
    // Local & remote should be in the multicast group.
    assert!(net.context("local").test_api().is_in_ip_multicast(&local_device_id, multicast_addr));
    assert!(net.context("remote").test_api().is_in_ip_multicast(&remote_device_id, multicast_addr));

    let _: StepResult = net.step();

    assert_eq!(
        with_assigned_ipv6_addr_subnets(
            &mut net.context("remote").core_ctx(),
            &remote_device_id,
            |addrs| addrs.count()
        ),
        1
    );

    // Let's make sure that only our local node still can use that address.
    assert_eq!(
        get_address_assigned(net.context("local"), &local_device_id, local_ip()),
        Some(true)
    );
    assert_eq!(get_address_assigned(net.context("remote"), &remote_device_id, local_ip()), None,);

    // Only local should be in the solicited node multicast group.
    assert!(net.context("local").test_api().is_in_ip_multicast(&local_device_id, multicast_addr));
    assert!(
        !net.context("remote").test_api().is_in_ip_multicast(&remote_device_id, multicast_addr)
    );
}

#[test]
fn test_dad_set_ipv6_address_when_ongoing() {
    // Test that we can make our tentative address change when DAD is
    // ongoing.

    let mut ctx = FakeCtx::default();
    let dev_id = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac(),
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &dev_id,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    let addr = local_ip();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(addr.into(), 128).unwrap())
        .unwrap();
    assert_eq!(get_address_assigned(&ctx, &dev_id, addr,), Some(false));

    let addr = remote_ip();
    assert_eq!(get_address_assigned(&ctx, &dev_id, addr,), None,);
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(addr.into(), 128).unwrap())
        .unwrap();
    assert_eq!(get_address_assigned(&ctx, &dev_id, addr,), Some(false));

    // Clear all device references.
    ctx.core_api().device().remove_device(dev_id.unwrap_ethernet()).into_removed();
}

#[test]
fn test_dad_three_transmits_no_conflicts() {
    let mut ctx = FakeCtx::default();
    let eth_dev_id = ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
        EthernetCreationProperties {
            tx_offload_spec: Default::default(),
            mac: local_mac(),
            max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
        },
        DEFAULT_INTERFACE_METRIC,
    );
    let dev_id = eth_dev_id.clone().into();
    ctx.test_api().enable_device(&dev_id);

    // Enable DAD.
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &dev_id,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(3)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(local_ip().into(), 128).unwrap())
        .unwrap();
    for _ in 0..3 {
        assert_eq!(
            ctx.trigger_next_timer().unwrap(),
            dad_timer_id(&mut ctx, eth_dev_id.clone(), local_ip())
        );
    }
    assert_eq!(get_address_assigned(&ctx, &dev_id, local_ip(),), Some(true));
}

#[test]
fn test_dad_three_transmits_with_conflicts() {
    // Test if the implementation is correct when we have more than 1 NS
    // packets to send.
    set_logger_for_test();
    let (mut net, local_eth_device_id, remote_eth_device_id) = setup_net();
    let local_device_id = local_eth_device_id.clone().into();
    let remote_device_id = remote_eth_device_id.into();

    let update = Ipv6DeviceConfigurationUpdate {
        ip_config: IpDeviceConfigurationUpdate {
            ip_enabled: Some(true),
            dad_transmits: Some(NonZeroU16::new(3)),
            ..Default::default()
        },
        ..Default::default()
    };
    net.with_context("local", |ctx| {
        // Seed RNG to avoid both stacks generating the same nonces.
        ctx.bindings_ctx.seed_rng(1);

        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(&local_device_id, update)
            .unwrap();

        ctx.core_api()
            .device_ip::<Ipv6>()
            .add_ip_addr_subnet(&local_device_id, AddrSubnet::new(local_ip().into(), 128).unwrap())
            .unwrap();
    });
    net.with_context("remote", |ctx| {
        // Seed RNG to avoid both stacks generating the same nonces.
        ctx.bindings_ctx.seed_rng(2);

        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(&remote_device_id, update)
            .unwrap();
    });

    // During the first and second period, the remote host is still down.
    net.with_context("local", |ctx| {
        let expected_timer_id = dad_timer_id(ctx, local_eth_device_id, local_ip());
        assert_eq!(ctx.trigger_next_timer().unwrap(), expected_timer_id);
        assert_eq!(ctx.trigger_next_timer().unwrap(), expected_timer_id);
    });
    net.with_context("remote", |ctx| {
        ctx.core_api()
            .device_ip::<Ipv6>()
            .add_ip_addr_subnet(&remote_device_id, AddrSubnet::new(local_ip().into(), 128).unwrap())
            .unwrap();
    });
    // The local host should have sent out 3 packets while the remote one
    // should only have sent out 1.
    assert_matches!(
        &net.context("local").bindings_ctx.copy_ethernet_frames()[..],
        [_frame1, _frame2, _frame3]
    );
    assert_matches!(&net.context("remote").bindings_ctx.copy_ethernet_frames()[..], [_frame1]);

    let _: StepResult = net.step();

    // Let's make sure that all timers are cancelled properly.
    net.with_context("local", |Ctx { core_ctx: _, bindings_ctx }| {
        assert_empty(bindings_ctx.timer_ctx().timers());
    });
    net.with_context("remote", |Ctx { core_ctx: _, bindings_ctx }| {
        assert_empty(bindings_ctx.timer_ctx().timers());
    });

    // They should now realize the address they intend to use has a
    // duplicate in the local network.
    assert_eq!(
        with_assigned_ipv6_addr_subnets(
            &mut net.context("local").core_ctx(),
            &local_device_id,
            |a| a.count()
        ),
        1
    );
    assert_eq!(
        with_assigned_ipv6_addr_subnets(
            &mut net.context("remote").core_ctx(),
            &remote_device_id,
            |a| a.count()
        ),
        1
    );
}

fn get_address_assigned(
    ctx: &FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    addr: Ipv6DeviceAddr,
) -> Option<bool> {
    ip::device::IpDeviceStateContext::<Ipv6, _>::with_address_ids(
        &mut ctx.core_ctx(),
        device,
        |mut addrs, core_ctx| {
            addrs.find_map(|addr_id| {
                ip::device::IpDeviceAddressContext::<Ipv6, _>::with_ip_address_data(
                    core_ctx,
                    device,
                    &addr_id,
                    |IpAddressData { flags: IpAddressFlags { assigned }, config: _ }| {
                        (addr_id.addr_sub().addr() == addr).then_some(*assigned)
                    },
                )
            })
        },
    )
}

#[test]
fn test_dad_multiple_ips_simultaneously() {
    let mut ctx = FakeCtx::default();
    let eth_dev_id = ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
        EthernetCreationProperties {
            tx_offload_spec: Default::default(),
            mac: local_mac(),
            max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
        },
        DEFAULT_INTERFACE_METRIC,
    );
    let dev_id = eth_dev_id.clone().into();
    ctx.test_api().enable_device(&dev_id);

    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &dev_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(None),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(3)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    // Add an IP.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(local_ip().into(), 128).unwrap())
        .unwrap();

    assert_matches!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Send another NS.
    let local_timer_id = dad_timer_id(&mut ctx, eth_dev_id.clone(), local_ip());
    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1)), [local_timer_id.clone()]);
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Add another IP
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(remote_ip().into(), 128).unwrap())
        .unwrap();
    assert_matches!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(false));
    assert_matches!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Run to the end for DAD for local ip
    let remote_timer_id = dad_timer_id(&mut ctx, eth_dev_id, remote_ip());
    assert_eq!(
        ctx.trigger_timers_for(Duration::from_secs(2)),
        [local_timer_id.clone(), remote_timer_id.clone(), local_timer_id, remote_timer_id.clone()]
    );
    assert_eq!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(true));
    assert_matches!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame1, _frame2, _frame3]);

    // Run to the end for DAD for local ip
    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1)), [remote_timer_id]);
    assert_eq!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(true));
    assert_eq!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(true));
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // No more timers.
    assert_eq!(ctx.trigger_next_timer(), None);
}

#[test]
fn test_dad_cancel_when_ip_removed() {
    let mut ctx = FakeCtx::default();
    let eth_dev_id = ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
        EthernetCreationProperties {
            tx_offload_spec: Default::default(),
            mac: local_mac(),
            max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
        },
        DEFAULT_INTERFACE_METRIC,
    );
    let dev_id = eth_dev_id.clone().into();
    ctx.test_api().enable_device(&dev_id);

    // Enable DAD.
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &dev_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(None),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(3)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    let addr_sub = AddrSubnet::new(local_ip().into(), 128).unwrap();
    // Add an IP.
    ctx.core_api().device_ip::<Ipv6>().add_ip_addr_subnet(&dev_id, addr_sub).unwrap();
    assert_matches!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Send another NS.
    let local_timer_id = dad_timer_id(&mut ctx, eth_dev_id.clone(), local_ip());
    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1)), [local_timer_id.clone()]);
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Add another IP
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&dev_id, AddrSubnet::new(remote_ip().into(), 128).unwrap())
        .unwrap();
    assert_matches!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(false));
    assert_matches!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // Run 1s
    let remote_timer_id = dad_timer_id(&mut ctx, eth_dev_id, remote_ip());
    assert_eq!(
        ctx.trigger_timers_for(Duration::from_secs(1)),
        [local_timer_id, remote_timer_id.clone()]
    );
    assert_matches!(get_address_assigned(&ctx, &dev_id, local_ip()), Some(false));
    assert_matches!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame1, _frame2]);

    // Remove local ip
    assert_eq!(
        ctx.core_api()
            .device_ip::<Ipv6>()
            .del_ip_addr(&dev_id, local_ip().into_specified())
            .unwrap()
            .into_removed(),
        addr_sub
    );
    assert_eq!(get_address_assigned(&ctx, &dev_id, local_ip()), None);
    assert_matches!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(false));
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Run to the end for DAD for local ip
    assert_eq!(
        ctx.trigger_timers_for(Duration::from_secs(2)),
        [remote_timer_id.clone(), remote_timer_id]
    );
    assert_eq!(get_address_assigned(&ctx, &dev_id, local_ip()), None);
    assert_eq!(get_address_assigned(&ctx, &dev_id, remote_ip()), Some(true));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);

    // No more timers.
    assert_eq!(ctx.trigger_next_timer(), None);
}

#[test]
fn test_receiving_router_solicitation_validity_check() {
    let config = Ipv6::TEST_ADDRS;
    let src_ip = Ipv6Addr::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 10]);
    let src_mac = [10, 11, 12, 13, 14, 15];
    let options = vec![NdpOptionBuilder::SourceLinkLayerAddress(&src_mac[..])];

    // Test receiving NDP RS when not a router (should not receive)

    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(config).build();
    let device_id: DeviceId<_> = device_ids[0].clone().into();

    let icmpv6_packet_buf = OptionSequenceBuilder::new(options.iter())
        .into_serializer()
        .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
            src_ip,
            Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
            IcmpZeroCode,
            RouterSolicitation::default(),
        ))
        .wrap_in(Ipv6PacketBuilder::new(
            src_ip,
            Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
            REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
            Ipv6Proto::Icmpv6,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_solicitation.get(), 0);
}

#[test]
fn test_receiving_router_advertisement_validity_check() {
    fn router_advertisement_message(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        hop_limit: u8,
    ) -> Buf<Vec<u8>> {
        EmptyBuf
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                RouterAdvertisement::new(
                    0,     /* current_hop_limit */
                    false, /* managed_flag */
                    false, /* other_config_flag */
                    0,     /* router_lifetime */
                    0,     /* reachable_time */
                    0,     /* retransmit_timer */
                ),
            ))
            .wrap_in(Ipv6PacketBuilder::new(src_ip, dst_ip, hop_limit, Ipv6Proto::Icmpv6))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
    }

    let config = Ipv6::TEST_ADDRS;
    let src_mac = [10, 11, 12, 13, 14, 15];
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(config.clone()).build();
    let device_id: DeviceId<_> = device_ids[0].clone().into();

    let invalid_src_ip = Ipv6Addr::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 192, 168, 0, 10]);
    let src_ip = Mac::new(src_mac).to_ipv6_link_local().addr().get();
    let invalid_hop_limit = 3;
    let hop_limit = REQUIRED_NDP_IP_PACKET_HOP_LIMIT;

    // Test receiving NDP RA where source IP is not a link local address
    // (should not receive).
    let icmpv6_packet_buf =
        router_advertisement_message(invalid_src_ip, config.local_ip.get(), hop_limit);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 0);

    // Test receiving NDP RA where hop limit is not 255 (should not receive).
    let icmpv6_packet_buf =
        router_advertisement_message(src_ip, config.local_ip.get(), invalid_hop_limit);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 0);

    // Test receiving NDP RA where source IP is a link local address and hop
    // limit is 255 (should receive).
    let icmpv6_packet_buf = router_advertisement_message(src_ip, config.local_ip.get(), hop_limit);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 1);
}

const TARGET_ADDR: Ipv6Addr = net_ip_v6!("2001:db8::3");
const NS_SOURCE_LINK_ADDR: Mac = mac!("aa:bb:cc:dd:ee:ff");

// Neighbor Solicitations with a hop-limit != 255 should be ignored.
#[test_case(TEST_ADDRS_V6.remote_ip.get(), TEST_ADDRS_V6.local_ip.get(),
    3, &[], false; "invalid_hop_limit")]
#[test_case(TEST_ADDRS_V6.remote_ip.get(), TEST_ADDRS_V6.local_ip.get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT, &[], true; "valid_hop_limit")]
// Neighbor Solicitations with an unspecified source must use the
// solicited-node multicast group as their destination.
#[test_case(Ipv6::UNSPECIFIED_ADDRESS, TEST_ADDRS_V6.local_ip.get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT, &[], false; "invalid_dst_ip")]
#[test_case(Ipv6::UNSPECIFIED_ADDRESS, TARGET_ADDR.to_solicited_node_address().get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT, &[], true; "valid_dst_ip")]
// Neighbor Solicitations with an unspecified source and the source link-layer
// address option should be ignored
#[test_case(
    Ipv6::UNSPECIFIED_ADDRESS,
    TEST_ADDRS_V6.local_ip.get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
    &[NdpOptionBuilder::SourceLinkLayerAddress(NS_SOURCE_LINK_ADDR.as_ref())],
    false;
    "unspecified_source_with_source_link_addr")]
fn test_receiving_neighbor_solicitation_validity_check(
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    hop_limit: u8,
    options: &[NdpOptionBuilder<'_>],
    expect_delivered: bool,
) {
    let icmpv6_packet_buf = OptionSequenceBuilder::new(options.iter())
        .into_serializer()
        .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
            src_ip,
            dst_ip,
            IcmpZeroCode,
            NeighborSolicitation::new(TARGET_ADDR),
        ))
        .wrap_in(Ipv6PacketBuilder::new(src_ip, dst_ip, hop_limit, Ipv6Proto::Icmpv6))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b();

    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(Ipv6::TEST_ADDRS).build();
    let device_id: DeviceId<_> = device_ids[0].clone().into();

    ctx.test_api().join_ip_multicast(&device_id, TARGET_ADDR.to_solicited_node_address());

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        icmpv6_packet_buf,
    );

    let expected_ns_count = if expect_delivered { 1 } else { 0 };
    assert_eq!(
        ctx.core_ctx.ipv6().icmp.ndp_counters.rx.neighbor_solicitation.get(),
        expected_ns_count
    );
}

#[test_case(
    TEST_ADDRS_V6.remote_ip.get(),
    TEST_ADDRS_V6.local_ip.get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
    false,
    true; "accepted")]
#[test_case(
    Ipv6::UNSPECIFIED_ADDRESS,
    TEST_ADDRS_V6.local_ip.get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
    false,
    true; "accepted_with_unspecified_source")]
#[test_case(
    TEST_ADDRS_V6.remote_ip.get(),
    TEST_ADDRS_V6.local_ip.get(),
    3,
    false,
    false; "rejected_with_invalid_hop_limit")]
#[test_case(
    TEST_ADDRS_V6.remote_ip.get(),
    TARGET_ADDR.to_solicited_node_address().get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
    false,
    true; "accepted_with_multicast_dst")]
#[test_case(
    TEST_ADDRS_V6.remote_ip.get(),
    TARGET_ADDR.to_solicited_node_address().get(),
    REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
    true,
    false; "rejected_with_multicast_dst_and_solicited_flag")]
fn test_receiving_neighbor_advertisement_validity_check(
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    hop_limit: u8,
    solicited_flag: bool,
    expect_delivered: bool,
) {
    let icmpv6_packet_buf = EmptyBuf
        .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
            src_ip,
            dst_ip,
            IcmpZeroCode,
            NeighborAdvertisement::new(
                false, // router_flag
                solicited_flag,
                false, // override_flag
                TARGET_ADDR,
            ),
        ))
        .wrap_in(Ipv6PacketBuilder::new(src_ip, dst_ip, hop_limit, Ipv6Proto::Icmpv6))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b();

    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(Ipv6::TEST_ADDRS).build();
    let device_id: DeviceId<_> = device_ids[0].clone().into();

    ctx.test_api().join_ip_multicast(&device_id, TARGET_ADDR.to_solicited_node_address());

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device_id,
        Some(FrameDestination::Individual { local: () }),
        icmpv6_packet_buf,
    );
    let expected_na_count = if expect_delivered { 1 } else { 0 };
    assert_eq!(
        ctx.core_ctx.ipv6().icmp.ndp_counters.rx.neighbor_advertisement.get(),
        expected_na_count
    );
}

#[test]
fn test_sending_ipv6_packet_after_hop_limit_change() {
    // Sets the hop limit with a router advertisement and sends a packet to
    // make sure the packet uses the new hop limit.
    fn inner_test(ctx: &mut FakeCtx, device_id: &DeviceId<FakeBindingsCtx>, hop_limit: u8) {
        let config = Ipv6::TEST_ADDRS;
        let src_ip = config.remote_mac.to_ipv6_link_local().addr();
        let src_ip: Ipv6Addr = src_ip.get();

        let icmpv6_packet_buf = EmptyBuf
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
                IcmpZeroCode,
                RouterAdvertisement::new(hop_limit, false, false, 0, 0, 0),
            ))
            .wrap_in(Ipv6PacketBuilder::new(
                src_ip,
                Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
                REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
                Ipv6Proto::Icmpv6,
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();
        ctx.test_api().receive_ip_packet::<Ipv6, _>(
            device_id,
            Some(FrameDestination::Multicast),
            icmpv6_packet_buf,
        );
        let (mut core_ctx, bindings_ctx) = ctx.contexts();
        assert_eq!(get_ipv6_hop_limit(&mut core_ctx, device_id).get(), hop_limit);
        ip::IpLayerHandler::<Ipv6, _>::send_ip_packet_from_device(
            &mut core_ctx,
            bindings_ctx,
            SendIpPacketMeta {
                device: device_id,
                src_ip: Some(config.local_ip),
                dst_ip: config.remote_ip,
                destination: IpPacketDestination::from_addr(config.remote_ip),
                proto: IpProto::Tcp.into(),
                ttl: None,
                mtu: Mtu::no_limit(),
                dscp_and_ecn: DscpAndEcn::default(),
            },
            [].into_serializer(),
        )
        .unwrap();
        let frames = bindings_ctx.take_ethernet_frames();
        let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
        let (buf, _, _, _) =
            parse_ethernet_frame(&frame[..], EthernetFrameLengthCheck::NoCheck).unwrap();
        // Packet's hop limit should be 100.
        assert_eq!(buf[7], hop_limit);
    }

    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(Ipv6::TEST_ADDRS).build();
    let device_id: DeviceId<_> = device_ids[0].clone().into();

    // Set hop limit to 100.
    inner_test(&mut ctx, &device_id, 100);

    // Set hop limit to 30.
    inner_test(&mut ctx, &device_id, 30);
}

#[test]
fn test_receiving_router_advertisement_mtu_option() {
    fn packet_buf(src_ip: Ipv6Addr, dst_ip: Ipv6Addr, mtu: u32) -> Buf<Vec<u8>> {
        let options = &[NdpOptionBuilder::Mtu(mtu)];
        OptionSequenceBuilder::new(options.iter())
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                RouterAdvertisement::new(0, false, false, 0, 0, 0),
            ))
            .wrap_in(Ipv6PacketBuilder::new(
                src_ip,
                dst_ip,
                REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
                Ipv6Proto::Icmpv6,
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
    }

    let mut ctx = FakeCtx::default();
    let hw_mtu = Mtu::new(5000);
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac(),
                max_frame_size: MaxEthernetFrameSize::from_mtu(hw_mtu).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let src_mac = Mac::new([10, 11, 12, 13, 14, 15]);
    let src_ip = src_mac.to_ipv6_link_local().addr();

    ctx.test_api().enable_device(&device);

    // Receive a new RA with a valid MTU option (but the new MTU should only
    // be 5000 as that is the max MTU of the device).

    let icmpv6_packet_buf =
        packet_buf(src_ip.get(), Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(), 5781);
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 1);
    assert_eq!(ip::IpDeviceMtuContext::<Ipv6>::get_mtu(&mut ctx.core_ctx(), &device), hw_mtu);

    // Receive a new RA with an invalid MTU option (value is lower than IPv6
    // min MTU).

    let icmpv6_packet_buf = packet_buf(
        src_ip.get(),
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        u32::from(Ipv6::MINIMUM_LINK_MTU) - 1,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 2);
    assert_eq!(ip::IpDeviceMtuContext::<Ipv6>::get_mtu(&mut ctx.core_ctx(), &device), hw_mtu);

    // Receive a new RA with a valid MTU option (value is exactly IPv6 min
    // MTU).

    let icmpv6_packet_buf = packet_buf(
        src_ip.get(),
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        Ipv6::MINIMUM_LINK_MTU.into(),
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
    assert_eq!(ctx.core_ctx.ipv6().icmp.ndp_counters.rx.router_advertisement.get(), 3);
    assert_eq!(
        ip::IpDeviceMtuContext::<Ipv6>::get_mtu(&mut ctx.core_ctx(), &device),
        Ipv6::MINIMUM_LINK_MTU,
    );
}

fn get_source_link_layer_option<L: LinkAddress, B>(options: &Options<B>) -> Option<L>
where
    B: SplitByteSlice,
{
    options.iter().find_map(|o| match o {
        NdpOption::SourceLinkLayerAddress(a) => {
            if a.len() >= L::BYTES_LENGTH {
                Some(L::from_bytes(&a[..L::BYTES_LENGTH]))
            } else {
                None
            }
        }
        _ => None,
    })
}

#[test]
fn test_host_send_router_solicitations() {
    #[track_caller]
    fn validate_params(
        src_mac: Mac,
        src_ip: Ipv6Addr,
        message: RouterSolicitation,
        code: IcmpZeroCode,
    ) {
        let fake_config = Ipv6::TEST_ADDRS;
        assert_eq!(src_mac, fake_config.local_mac.get());
        assert_eq!(src_ip, fake_config.local_mac.to_ipv6_link_local().addr().get());
        assert_eq!(message, RouterSolicitation::default());
        assert_eq!(code, IcmpZeroCode);
    }

    let fake_config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
    let eth_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: fake_config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = eth_device_id.clone().into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                // Test expects to send 3 RSs.
                max_router_solicitations: Some(NonZeroU8::new(3)),
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    let time = ctx.bindings_ctx.now();
    assert_eq!(ctx.trigger_next_timer().unwrap(), rs_timer_id(eth_device_id.clone()));
    // Initial router solicitation should be a random delay between 0 and
    // `MAX_RTR_SOLICITATION_DELAY`.
    assert!(
        ctx.bindings_ctx.now().checked_duration_since(time).unwrap() < MAX_RTR_SOLICITATION_DELAY
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    let (src_mac, _, src_ip, _, _, message, code) =
        parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, RouterSolicitation, _>(
            &frame,
            EthernetFrameLengthCheck::NoCheck,
            |_| {},
        )
        .unwrap();
    validate_params(src_mac, src_ip, message, code);

    // Should get 2 more router solicitation messages
    let time = ctx.bindings_ctx.now();
    assert_eq!(ctx.trigger_next_timer().unwrap(), rs_timer_id(eth_device_id.clone()));
    assert_eq!(
        ctx.bindings_ctx.now().checked_duration_since(time).unwrap(),
        RTR_SOLICITATION_INTERVAL
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    let (src_mac, _, src_ip, _, _, message, code) =
        parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, RouterSolicitation, _>(
            &frame,
            EthernetFrameLengthCheck::NoCheck,
            |_| {},
        )
        .unwrap();
    validate_params(src_mac, src_ip, message, code);

    // Before the next one, lets assign an IP address (DAD won't be
    // performed so it will be assigned immediately). The router solicitation
    // message should continue to use the link-local address.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(fake_config.local_ip.get(), 128).unwrap())
        .unwrap();

    let time = ctx.bindings_ctx.now();
    assert_eq!(ctx.trigger_next_timer().unwrap(), rs_timer_id(eth_device_id));
    assert_eq!(
        ctx.bindings_ctx.now().checked_duration_since(time).unwrap(),
        RTR_SOLICITATION_INTERVAL
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame) = assert_matches!(&frames[..], [frame] => frame);
    let (src_mac, _, src_ip, _, _, message, code) =
        parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, RouterSolicitation, _>(
            &frame,
            EthernetFrameLengthCheck::NoCheck,
            |p| {
                // We should have a source link layer option now because we
                // have a source IP address set.
                assert_eq!(p.body().iter().count(), 1);
                if let Some(ll) = get_source_link_layer_option::<Mac, _>(p.body()) {
                    assert_eq!(ll, fake_config.local_mac.get());
                } else {
                    panic!("Should have a source link layer option");
                }
            },
        )
        .unwrap();
    validate_params(src_mac, src_ip, message, code);

    // No more timers.
    assert_eq!(ctx.trigger_next_timer(), None);
    // Should have only sent 3 packets (Router solicitations).
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    let mut ctx = FakeCtx::default();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
    let eth_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: fake_config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id = eth_device_id.clone().into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device_id,
            Ipv6DeviceConfigurationUpdate {
                max_router_solicitations: Some(NonZeroU8::new(2)),
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    let time = ctx.bindings_ctx.now();
    assert_eq!(ctx.trigger_next_timer().unwrap(), rs_timer_id(eth_device_id.clone()));
    // Initial router solicitation should be a random delay between 0 and
    // `MAX_RTR_SOLICITATION_DELAY`.
    assert!(
        ctx.bindings_ctx.now().checked_duration_since(time).unwrap() < MAX_RTR_SOLICITATION_DELAY
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame1) = assert_matches!(&frames[..], [frame] => frame);

    // Should trigger 1 more router solicitations
    let time = ctx.bindings_ctx.now();
    assert_eq!(ctx.trigger_next_timer().unwrap(), rs_timer_id(eth_device_id));
    assert_eq!(
        ctx.bindings_ctx.now().checked_duration_since(time).unwrap(),
        RTR_SOLICITATION_INTERVAL
    );
    let frames = ctx.bindings_ctx.take_ethernet_frames();
    let (_dev, frame2) = assert_matches!(&frames[..], [frame] => frame);

    // Each packet would be the same.
    for f in [frame1, frame2] {
        let (src_mac, _, src_ip, _, _, message, code) =
            parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, RouterSolicitation, _>(
                &f,
                EthernetFrameLengthCheck::NoCheck,
                |_| {},
            )
            .unwrap();
        validate_params(src_mac, src_ip, message, code);
    }

    // No more timers.
    assert_eq!(ctx.trigger_next_timer(), None);
}

#[test]
fn test_set_ndp_config_dup_addr_detect_transmits() {
    // Test that updating the DupAddrDetectTransmits parameter on an
    // interface updates the number of DAD messages (NDP Neighbor
    // Solicitations) sent before concluding that an address is not a
    // duplicate.

    let fake_config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: fake_config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
    assert_empty(ctx.bindings_ctx.timer_ctx().timers());

    // Updating the IP should resolve immediately since DAD is turned off by
    // `FakeCtxBuilder::build`.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device, AddrSubnet::new(fake_config.local_ip.get(), 128).unwrap())
        .unwrap();

    let device_id = device.clone().try_into().unwrap();
    assert_eq!(get_address_assigned(&ctx, &device, local_ip()), Some(true));
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
    assert_empty(ctx.bindings_ctx.timer_ctx().timers());

    // Enable DAD for the device.
    const DUP_ADDR_DETECT_TRANSMITS: u16 = 3;
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(DUP_ADDR_DETECT_TRANSMITS)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    // Updating the IP should start the DAD process.
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device, AddrSubnet::new(fake_config.remote_ip.get(), 128).unwrap())
        .unwrap();
    assert_eq!(get_address_assigned(&ctx, &device, local_ip()), Some(true));
    assert_eq!(get_address_assigned(&ctx, &device, remote_ip()), Some(false));
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);
    assert_eq!(ctx.bindings_ctx.timer_ctx().timers().len(), 1);

    // Disable DAD during DAD.
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    dad_transmits: Some(None),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let expected_timer_id = dad_timer_id(&mut ctx, device_id, remote_ip());
    // Allow already started DAD to complete (2 more more NS, 3 more timers).
    assert_eq!(ctx.trigger_next_timer().unwrap(), expected_timer_id);
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);
    assert_eq!(ctx.trigger_next_timer().unwrap(), expected_timer_id);
    assert_matches!(&ctx.bindings_ctx.take_ethernet_frames()[..], [_frame]);
    assert_eq!(ctx.trigger_next_timer().unwrap(), expected_timer_id);
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);
    assert_eq!(get_address_assigned(&ctx, &device, remote_ip()), Some(true));

    // Updating the IP should resolve immediately since DAD has just been
    // turned off.
    let new_ip = Ipv6::get_other_ip_address(3);
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device, AddrSubnet::new(new_ip.get(), 128).unwrap())
        .unwrap();
    assert_eq!(get_address_assigned(&ctx, &device, local_ip()), Some(true));
    assert_eq!(get_address_assigned(&ctx, &device, remote_ip()), Some(true));
    assert_eq!(
        get_address_assigned(
            &ctx,
            &device,
            NonMappedAddr::new(new_ip.try_into().unwrap()).unwrap()
        ),
        Some(true)
    );
}

fn slaac_packet_buf(
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    prefix: Ipv6Addr,
    prefix_length: u8,
    on_link_flag: bool,
    autonomous_address_configuration_flag: bool,
    valid_lifetime_secs: u32,
    preferred_lifetime_secs: u32,
) -> Buf<Vec<u8>> {
    let p = PrefixInformation::new(
        prefix_length,
        on_link_flag,
        autonomous_address_configuration_flag,
        valid_lifetime_secs,
        preferred_lifetime_secs,
        prefix,
    );
    let options = &[NdpOptionBuilder::PrefixInformation(p)];
    OptionSequenceBuilder::new(options.iter())
        .into_serializer()
        .wrap_in(IcmpPacketBuilder::<Ipv6, _>::new(
            src_ip,
            dst_ip,
            IcmpZeroCode,
            RouterAdvertisement::new(0, false, false, 0, 0, 0),
        ))
        .wrap_in(Ipv6PacketBuilder::new(
            src_ip,
            dst_ip,
            REQUIRED_NDP_IP_PACKET_HOP_LIMIT,
            Ipv6Proto::Icmpv6,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
}

#[test]
fn test_router_stateless_address_autoconfiguration() {
    // Routers should not perform SLAAC for global addresses.

    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv6>(&device, true);

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;

    // Receive a new RA with new prefix (autonomous).
    //
    // Should not get a new IP.

    let icmpv6_packet_buf = slaac_packet_buf(
        src_ip,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        prefix,
        prefix_length,
        false,
        false,
        100,
        0,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );

    assert_empty(get_global_ipv6_addrs(&ctx, &device));

    // No timers.
    assert_eq!(ctx.trigger_next_timer(), None);
}

#[derive(Copy, Clone, Debug)]
struct TestSlaacPrefix {
    prefix: Subnet<Ipv6Addr>,
    valid_for: u32,
    preferred_for: u32,
}
impl TestSlaacPrefix {
    fn send_prefix_update(
        &self,
        ctx: &mut FakeCtx,
        device: &DeviceId<FakeBindingsCtx>,
        src_ip: Ipv6Addr,
    ) {
        let Self { prefix, valid_for, preferred_for } = *self;

        receive_prefix_update(ctx, device, src_ip, prefix, preferred_for, valid_for);
    }

    fn valid_until<I: Instant>(&self, now: I) -> I {
        now.checked_add(Duration::from_secs(self.valid_for.into())).unwrap()
    }
}

fn slaac_address<I: Instant>(
    entry: SlaacAddr<I>,
) -> Option<(UnicastAddr<Ipv6Addr>, SlaacConfig<I>)> {
    match entry.config {
        Ipv6AddrConfig::Manual(_manual_config) => None,
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => {
            Some((entry.addr_sub.addr().get(), inner))
        }
    }
}

/// Extracts the single stable and temporary address config from the provided iterator and
/// returns them as (stable, temporary).
///
/// Panics
///
/// Panics if the iterator doesn't contain exactly one stable and one temporary SLAAC entry.
fn single_stable_and_temporary<
    I: Copy + Debug,
    A: Copy + Debug,
    It: Iterator<Item = (A, SlaacConfig<I>)>,
>(
    slaac_configs: It,
) -> ((A, SlaacConfig<I>), (A, SlaacConfig<I>)) {
    {
        let (stable_addresses, temporary_addresses): (Vec<_>, Vec<_>) = slaac_configs
            .partition(|(_, s)| if let SlaacConfig::Stable { .. } = s { true } else { false });

        let stable_addresses: [_; 1] =
            stable_addresses.try_into().expect("expected a single stable address");
        let temporary_addresses: [_; 1] =
            temporary_addresses.try_into().expect("expected a single temporary address");
        (stable_addresses[0], temporary_addresses[0])
    }
}

fn initialize_with_temporary_addresses_enabled()
-> (FakeCtx, DeviceId<FakeBindingsCtx>, SlaacConfigurationUpdate) {
    set_logger_for_test();
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    let max_valid_lifetime = Duration::from_secs(60 * 60);
    let max_preferred_lifetime = Duration::from_secs(30 * 60);
    let idgen_retries = 3;
    let slaac_config = SlaacConfigurationUpdate {
        temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
            temp_valid_lifetime: NonZeroDuration::new(max_valid_lifetime).unwrap(),
            temp_preferred_lifetime: NonZeroDuration::new(max_preferred_lifetime).unwrap(),
            temp_idgen_retries: idgen_retries,
        }),
        ..Default::default()
    };

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate { slaac_config, ..Default::default() },
        )
        .unwrap();
    (ctx, device, slaac_config)
}

#[test]
fn test_host_stateless_address_autoconfiguration_multiple_prefixes() {
    let (mut ctx, device, _): (_, _, SlaacConfigurationUpdate) =
        initialize_with_temporary_addresses_enabled();

    let mut api = ctx.core_api().device_ip::<Ipv6>();
    let _: Ipv6DeviceConfigurationUpdate = api
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let prefix1 =
        TestSlaacPrefix { prefix: subnet_v6!("1:2:3:4::/64"), valid_for: 1500, preferred_for: 900 };
    let prefix2 =
        TestSlaacPrefix { prefix: subnet_v6!("5:6:7:8::/64"), valid_for: 1200, preferred_for: 600 };

    let config = Ipv6::TEST_ADDRS;
    let src_mac = config.remote_mac;
    let src_ip: Ipv6Addr = src_mac.to_ipv6_link_local().addr().get();

    // After the RA for the first prefix, we should have two addresses, one
    // stable and one temporary.
    prefix1.send_prefix_update(&mut ctx, &device, src_ip);

    let (prefix_1_stable, prefix_1_temporary) = {
        let slaac_configs = get_global_ipv6_addrs(&ctx, &device)
            .into_iter()
            .filter_map(slaac_address)
            .filter(|(a, _)| prefix1.prefix.contains(a));

        let (stable_address, temporary_address) = single_stable_and_temporary(slaac_configs);

        let now = ctx.bindings_ctx.now();
        let prefix1_valid_until = prefix1.valid_until(now);
        assert_matches!(
            stable_address,
            (_addr, SlaacConfig::Stable {
                valid_until,
                creation_time: _,
                regen_counter: _,
                dad_counter: _,
            }) => {
                assert_eq!(valid_until, Lifetime::Finite(prefix1_valid_until))
            }
        );
        assert_matches!(temporary_address, (_addr,
            SlaacConfig::Temporary(TemporarySlaacConfig {
                valid_until,
                creation_time,
                desync_factor: _,
                dad_counter: _ })) => {
                assert_eq!(creation_time, now);
                assert_eq!(valid_until, prefix1_valid_until);
        });
        (stable_address.0, temporary_address.0)
    };

    // When the RA for the second prefix comes in, we should leave the entries for the addresses
    // in the first prefix alone.
    prefix2.send_prefix_update(&mut ctx, &device, src_ip);

    {
        // Check prefix 1 addresses again.
        let slaac_configs = get_global_ipv6_addrs(&ctx, &device)
            .into_iter()
            .filter_map(slaac_address)
            .filter(|(a, _)| prefix1.prefix.contains(a));
        let (stable_address, temporary_address) = single_stable_and_temporary(slaac_configs);

        let now = ctx.bindings_ctx.now();
        let prefix1_valid_until = prefix1.valid_until(now);
        assert_matches!(
            stable_address,
            (addr, SlaacConfig::Stable {
                valid_until,
                creation_time: _,
                regen_counter: _,
                dad_counter: _,
            }) => {
                assert_eq!(addr, prefix_1_stable);
                assert_eq!(valid_until, Lifetime::Finite(prefix1_valid_until));
            }
        );
        assert_matches!(
            temporary_address,
            (addr, SlaacConfig::Temporary(TemporarySlaacConfig {
                valid_until, creation_time, desync_factor: _, dad_counter: 0 })) => {
                assert_eq!(addr, prefix_1_temporary);
                assert_eq!(creation_time, now);
                assert_eq!(valid_until, prefix1_valid_until);
            }
        );
    }
    {
        // Check prefix 2 addresses.
        let slaac_configs = get_global_ipv6_addrs(&ctx, &device)
            .into_iter()
            .filter_map(slaac_address)
            .filter(|(a, _)| prefix2.prefix.contains(a));
        let (stable_address, temporary_address) = single_stable_and_temporary(slaac_configs);

        let now = ctx.bindings_ctx.now();
        let prefix2_valid_until = prefix2.valid_until(now);
        assert_matches!(
            stable_address,
            (_, SlaacConfig::Stable {
                valid_until,
                creation_time: _,
                regen_counter: _,
                dad_counter: _,
            }) => {
                assert_eq!(valid_until, Lifetime::Finite(prefix2_valid_until))
            }
        );
        assert_matches!(
            temporary_address,
            (_, SlaacConfig::Temporary(TemporarySlaacConfig {
                valid_until, creation_time, desync_factor: _, dad_counter: 0 })) => {
                assert_eq!(creation_time, now);
                assert_eq!(valid_until, prefix2_valid_until);
            }
        );
    }

    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

fn test_host_generate_temporary_slaac_address(
    valid_lifetime_in_ra: u32,
    preferred_lifetime_in_ra: u32,
) -> (FakeCtx, DeviceId<FakeBindingsCtx>, UnicastAddr<Ipv6Addr>) {
    set_logger_for_test();
    let (mut ctx, device, slaac_config) = initialize_with_temporary_addresses_enabled();

    let max_valid_lifetime = assert_matches!(
        slaac_config,
        SlaacConfigurationUpdate {
            temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
                temp_valid_lifetime,
                temp_preferred_lifetime: _,
                temp_idgen_retries: _,
            }),
            stable_address_configuration: _,
        } => temp_valid_lifetime.get()
    );
    let config = Ipv6::TEST_ADDRS;

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let subnet = subnet_v6!("0102:0304:0506:0708::/64");
    let interface_identifier = OpaqueIid::new(
        subnet,
        &*config.local_mac,
        None::<[_; 0]>,
        // Clone the RNG so we can see what the next value (which will be
        // used to generate the temporary address) will be.
        OpaqueIidNonce::Random(ctx.bindings_ctx.rng().deep_clone().random()),
        &ctx.core_ctx.ipv6().slaac_temp_secret_key,
    );
    let mut expected_addr = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    expected_addr[8..].copy_from_slice(&interface_identifier.to_be_bytes()[..8]);
    let expected_addr = UnicastAddr::new(Ipv6Addr::from(expected_addr)).unwrap();
    let expected_addr_sub = AddrSubnet::from_witness(expected_addr, subnet.prefix()).unwrap();
    assert_eq!(expected_addr_sub.subnet(), subnet);

    // Receive a new RA with new prefix (autonomous).
    //
    // Should get a new temporary IP.

    receive_prefix_update(
        &mut ctx,
        &device,
        src_ip,
        subnet,
        preferred_lifetime_in_ra,
        valid_lifetime_in_ra,
    );

    // Should have gotten a new temporary IP.
    let temporary_slaac_addresses = get_global_ipv6_addrs(&ctx, &device)
        .into_iter()
        .filter_map(|entry| match entry.config {
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
                inner: SlaacConfig::Stable { .. },
                ..
            }) => None,
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
                inner:
                    SlaacConfig::Temporary(TemporarySlaacConfig {
                        creation_time: _,
                        desync_factor: _,
                        valid_until,
                        dad_counter: _,
                    }),
                ..
            }) => Some((*entry.addr_sub(), entry.flags.assigned, valid_until)),
            Ipv6AddrConfig::Manual(_manual_config) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(temporary_slaac_addresses.len(), 1);
    let (addr_sub, assigned, valid_until) = temporary_slaac_addresses.into_iter().next().unwrap();
    assert_eq!(addr_sub.subnet(), subnet);
    assert!(assigned);
    assert!(valid_until <= ctx.bindings_ctx.now().checked_add(max_valid_lifetime).unwrap());

    (ctx, device, expected_addr)
}

const INFINITE_LIFETIME: u32 = u32::MAX;

#[test]
fn test_host_temporary_slaac_and_manual_addresses_conflict() {
    // Verify that if the temporary SLAAC address generation picks an
    // address that is already assigned, it tries again. The difficulty here
    // is that the test uses an RNG to pick an address. To make sure we
    // assign the address that the code _would_ pick, we run the code twice
    // with the same RNG seed and parameters. The first time is lets us
    // figure out the temporary address that is generated. Then, we run the
    // same code with the address already assigned to verify the behavior.
    const RNG_SEED: [u8; 16] = [1; 16];
    let config = Ipv6::TEST_ADDRS;
    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let subnet = subnet_v6!("0102:0304:0506:0708::/64");

    // Receive an RA to figure out the temporary address that is assigned.
    let conflicted_addr = {
        let (mut ctx, device, _config) = initialize_with_temporary_addresses_enabled();

        *ctx.bindings_ctx.rng().rng() = rand::SeedableRng::from_seed(RNG_SEED);

        // Receive an RA and determine what temporary address was assigned, then return it.
        receive_prefix_update(&mut ctx, &device, src_ip, subnet, 9000, 10000);
        let conflicted_addr = *get_matching_slaac_address_entry(&ctx, &device, |entry| {
            is_temporary_slaac_addr(&entry.config)
        })
        .unwrap()
        .addr_sub();
        // Clean up device references.
        ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
        conflicted_addr
    };
    assert!(subnet.contains(&conflicted_addr.addr().get()));

    // Now that we know what address will be assigned, create a new instance
    // of the stack and assign that same address manually.
    let (mut ctx, device, _config) = initialize_with_temporary_addresses_enabled();
    ctx.core_api()
        .device_ip::<Ipv6>()
        .add_ip_addr_subnet(&device, conflicted_addr.to_witness())
        .expect("adding address failed");

    // Sanity check: `conflicted_addr` is already assigned on the device.
    assert_matches!(
        get_global_ipv6_addrs(&ctx, &device)
            .into_iter()
            .find(|entry| entry.addr_sub() == &conflicted_addr),
        Some(_)
    );

    // Seed the RNG right before the RA is received, just like in our
    // earlier run above.
    *ctx.bindings_ctx.rng().rng() = rand::SeedableRng::from_seed(RNG_SEED);

    // Receive a new RA with new prefix (autonomous). The system will assign
    // a temporary and stable SLAAC address. The first temporary address
    // tried will conflict with `conflicted_addr` assigned above, so a
    // different one will be generated.
    receive_prefix_update(&mut ctx, &device, src_ip, subnet, 9000, 10000);

    // Verify that `conflicted_addr` was generated and rejected.
    assert_eq!(ctx.core_ctx.ipv6().slaac_counters.generated_slaac_addr_exists.get(), 1);

    // Should have gotten a new temporary IP.
    let temporary_slaac_addresses = get_matching_slaac_address_entries(&ctx, &device, |entry| {
        is_temporary_slaac_addr(&entry.config)
    })
    .map(|entry| *entry.addr_sub())
    .collect::<Vec<_>>();
    assert_matches!(&temporary_slaac_addresses[..], [temporary_addr] => {
        assert_eq!(temporary_addr.subnet(), conflicted_addr.subnet());
        assert_ne!(temporary_addr, &conflicted_addr);
    });

    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

#[test]
fn test_host_slaac_invalid_prefix_information() {
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;

    assert_empty(get_global_ipv6_addrs(&ctx, &device));

    // Receive a new RA with new prefix (autonomous), but preferred lifetime
    // is greater than valid.
    //
    // Should not get a new IP.

    let icmpv6_packet_buf = slaac_packet_buf(
        src_ip,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        prefix,
        prefix_length,
        false,
        true,
        9000,
        10000,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
    assert_empty(get_global_ipv6_addrs(&ctx, &device));

    // Address invalidation timers were added.
    assert_empty(ctx.bindings_ctx.timer_ctx().timers());
}

// Assert internal timers in integration tests by going through the contexts
// to get the state.
#[track_caller]
fn assert_slaac_timers_integration<CC, BC, I>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
    timers: I,
) where
    CC: Ipv6DeviceConfigurationContext<BC>,
    for<'a> CC::Ipv6DeviceStateCtx<'a>: SlaacContext<BC>,
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId> + SlaacBindingsContext<CC::DeviceId>,
    I: IntoIterator<Item = (InnerSlaacTimerId, BC::Instant)>,
{
    let want = timers.into_iter().collect::<HashMap<_, _>>();
    let got = ip::device::testutil::collect_slaac_timers_integration(core_ctx, device_id);
    assert_eq!(got, want);
}

#[track_caller]
fn assert_next_slaac_timer_integration<CC, BC>(
    core_ctx: &mut CC,
    device_id: &CC::DeviceId,
    want: &InnerSlaacTimerId,
) where
    CC: Ipv6DeviceConfigurationContext<BC>,
    for<'a> CC::Ipv6DeviceStateCtx<'a>: SlaacContext<BC>,
    BC: IpDeviceBindingsContext<Ipv6, CC::DeviceId> + SlaacBindingsContext<CC::DeviceId>,
{
    let got = ip::device::testutil::collect_slaac_timers_integration(core_ctx, device_id)
        .into_iter()
        .min_by_key(|(_, v)| *v)
        .map(|(k, _)| k);
    assert_eq!(got.as_ref(), Some(want));
}

fn new_slaac_timer_id<BT: BindingsTypes>(device_id: &DeviceId<BT>) -> TimerId<BT> {
    TimerId::from(Ipv6DeviceTimerId::Slaac(SlaacTimerId::new(device_id.downgrade())).into_common())
}

#[test]
fn test_host_slaac_address_deprecate_while_tentative() {
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    ctx.test_api().enable_device(&device);

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let prefix = subnet_v6!("0102:0304:0506:0708::/64");
    let expected_addr_sub = calculate_slaac_addr_sub(prefix, config.local_mac.to_eui64());

    // Have no addresses yet.
    assert_empty(get_global_ipv6_addrs(&ctx, &device));

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                // Doesn't matter as long as we perform DAD.
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    let Ctx { core_ctx, bindings_ctx } = &mut ctx;

    // Set the retransmit timer between neighbor solicitations to be greater
    // than the preferred lifetime of the prefix.
    Ipv6DeviceHandler::set_discovered_retrans_timer(
        &mut core_ctx.context(),
        bindings_ctx,
        &device,
        NonZeroDuration::from_secs(10).unwrap(),
    );

    // Receive a new RA with new prefix (autonomous).
    //
    // Should get a new IP and set preferred lifetime to 1s.

    let valid_lifetime = 2;
    let preferred_lifetime = 1;

    receive_prefix_update(&mut ctx, &device, src_ip, prefix, preferred_lifetime, valid_lifetime);

    // Should have gotten a new IP.
    let now = ctx.bindings_ctx.now();
    let valid_until = now + Duration::from_secs(valid_lifetime.into());
    let preferred_until = now + Duration::from_secs(preferred_lifetime.into());
    let expected_address_entry_config = Ipv6AddrSlaacConfig {
        inner: SlaacConfig::Stable {
            valid_until: Lifetime::Finite(valid_until),
            creation_time: now,
            regen_counter: 0,
            dad_counter: 0,
        },
        preferred_lifetime: PreferredLifetime::preferred_until(preferred_until),
    };
    let expected_address_entry = SlaacAddr {
        addr_sub: expected_addr_sub,
        config: Ipv6AddrConfig::Slaac(expected_address_entry_config),
        flags: IpAddressFlags { assigned: false },
    };
    assert_eq!(get_global_ipv6_addrs(&ctx, &device), [expected_address_entry]);

    // Make sure deprecation and invalidation timers are set.
    assert_slaac_timers_integration(
        &mut ctx.core_ctx(),
        &device,
        [
            (
                InnerSlaacTimerId::DeprecateSlaacAddress { addr: expected_addr_sub.addr() },
                now + Duration::from_secs(preferred_lifetime.into()),
            ),
            (
                InnerSlaacTimerId::InvalidateSlaacAddress { addr: expected_addr_sub.addr() },
                valid_until,
            ),
        ],
    );

    // Trigger the deprecation timer.
    assert_eq!(ctx.trigger_next_timer().unwrap(), new_slaac_timer_id(&device));
    assert_eq!(
        get_global_ipv6_addrs(&ctx, &device),
        [SlaacAddr {
            config: Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
                preferred_lifetime: PreferredLifetime::Deprecated,
                ..expected_address_entry_config
            }),
            ..expected_address_entry
        }]
    );

    // Clear all device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

fn receive_prefix_update(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    src_ip: Ipv6Addr,
    subnet: Subnet<Ipv6Addr>,
    preferred_lifetime: u32,
    valid_lifetime: u32,
) {
    let prefix = subnet.network();
    let prefix_length = subnet.prefix();

    let icmpv6_packet_buf = slaac_packet_buf(
        src_ip,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        prefix,
        prefix_length,
        false,
        true,
        valid_lifetime,
        preferred_lifetime,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );
}

fn get_matching_slaac_address_entries<F: FnMut(&SlaacAddr<FakeInstant>) -> bool>(
    ctx: &FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    filter: F,
) -> impl Iterator<Item = SlaacAddr<FakeInstant>> + use<F> {
    get_slaac_addrs(ctx, device, None).into_iter().filter(filter)
}

fn get_matching_slaac_address_entry<F: FnMut(&SlaacAddr<FakeInstant>) -> bool>(
    ctx: &FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    filter: F,
) -> Option<SlaacAddr<FakeInstant>> {
    let mut matching_addrs = get_matching_slaac_address_entries(ctx, device, filter);
    let entry = matching_addrs.next();
    assert_eq!(matching_addrs.next(), None);
    entry
}

fn get_slaac_address_entry(
    ctx: &FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
) -> Option<SlaacAddr<FakeInstant>> {
    let mut matching_addrs = get_slaac_addrs(ctx, device, None)
        .into_iter()
        .filter(|entry| *entry.addr_sub() == addr_sub);
    let entry = matching_addrs.next();
    assert_eq!(matching_addrs.next(), None);
    entry
}

fn assert_slaac_lifetimes_enforced(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    entry: SlaacAddr<FakeInstant>,
    valid_until: FakeInstant,
    preferred_until: FakeInstant,
) {
    assert!(entry.flags.assigned);
    assert_matches!(entry.config, Ipv6AddrConfig::Slaac(_));
    assert_eq!(
        entry.config.preferred_lifetime(),
        PreferredLifetime::preferred_until(preferred_until)
    );
    assert_eq!(entry.config.valid_until(), Lifetime::Finite(valid_until));
    let timers =
        ip::device::testutil::collect_slaac_timers_integration(&mut ctx.core_ctx(), device);
    assert_eq!(
        timers.get(&InnerSlaacTimerId::DeprecateSlaacAddress { addr: entry.addr_sub().addr() }),
        Some(&preferred_until)
    );
    assert_eq!(
        timers.get(&InnerSlaacTimerId::InvalidateSlaacAddress { addr: entry.addr_sub().addr() }),
        Some(&valid_until)
    );
}

#[test]
fn test_host_stable_slaac_valid_lifetime_updates() {
    // Make sure we update the valid lifetime only in certain scenarios
    // to prevent denial-of-service attacks as outlined in RFC 4862 section
    // 5.5.3.e. Note, the preferred lifetime should always be updated.

    set_logger_for_test();
    fn inner_test(
        ctx: &mut FakeCtx,
        device: &DeviceId<FakeBindingsCtx>,
        src_ip: Ipv6Addr,
        subnet: Subnet<Ipv6Addr>,
        addr_sub: AddrSubnet<Ipv6Addr, Ipv6DeviceAddr>,
        preferred_lifetime: u32,
        valid_lifetime: u32,
        expected_valid_lifetime: u32,
    ) {
        receive_prefix_update(ctx, device, src_ip, subnet, preferred_lifetime, valid_lifetime);
        let entry = get_slaac_address_entry(&ctx, &device, addr_sub).unwrap();
        let now = ctx.bindings_ctx.now();
        let valid_until =
            now.checked_add(Duration::from_secs(expected_valid_lifetime.into())).unwrap();
        let preferred_until =
            now.checked_add(Duration::from_secs(preferred_lifetime.into())).unwrap();

        assert_slaac_lifetimes_enforced(ctx, &device, entry, valid_until, preferred_until);
    }

    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;
    let subnet = Subnet::new(prefix, prefix_length).unwrap();
    let expected_addr_sub = calculate_slaac_addr_sub(subnet, config.local_mac.to_eui64());

    // Have no addresses yet.
    assert_empty(get_global_ipv6_addrs(&ctx, &device));

    // Receive a new RA with new prefix (autonomous).
    //
    // Should get a new IP and set preferred lifetime to 1s.

    // Make sure deprecate and invalidation timers are set.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 30, 60, 60);

    // If the valid lifetime is greater than the remaining lifetime, update
    // the valid lifetime.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 70, 70, 70);

    // If the valid lifetime is greater than 2 hrs, update the valid
    // lifetime.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 7201, 7201);

    // Make remaining lifetime < 2 hrs.
    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1000)), []);

    // If the remaining lifetime is <= 2 hrs & valid lifetime is less than
    // that, don't update valid lifetime.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1000, 2000, 6201);

    // Make the remaining lifetime > 2 hours.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1000, 10800, 10800);

    // If the remaining lifetime is > 2 hours, and new valid lifetime is < 2
    // hours, set the valid lifetime to 2 hours.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1000, 1000, 7200);

    // If the remaining lifetime is <= 2 hrs & valid lifetime is less than
    // that, don't update valid lifetime.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1000, 2000, 7200);

    // Increase valid lifetime twice while it is greater than 2 hours.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 7201, 7201);
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 7202, 7202);

    // Make remaining lifetime < 2 hrs.
    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1000)), []);

    // If the remaining lifetime is <= 2 hrs & valid lifetime is less than
    // that, don't update valid lifetime.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 6202, 6202);

    // Increase valid lifetime twice while it is less than 2 hours.
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 6203, 6203);
    inner_test(&mut ctx, &device, src_ip, subnet, expected_addr_sub, 1001, 6204, 6204);

    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

enum SlaacType {
    LinkLocal,
    GlobalTemporary,
    GlobalStable,
}

impl SlaacType {
    const MAX_VALID_LIFETIME: Duration = Duration::from_secs(15000);
    const MAX_PREFERRED_LIFETIME: Duration = Duration::from_secs(5000);
    const IDGEN_RETRIES: u8 = 3;

    fn config_update(&self) -> SlaacConfigurationUpdate {
        match self {
            Self::LinkLocal | Self::GlobalStable => SlaacConfigurationUpdate {
                stable_address_configuration: Some(StableSlaacAddressConfiguration::Enabled {
                    iid_generation: IidGenerationConfiguration::Opaque {
                        idgen_retries: Self::IDGEN_RETRIES,
                    },
                }),
                ..Default::default()
            },
            Self::GlobalTemporary => SlaacConfigurationUpdate {
                temporary_address_configuration: Some(
                    TemporarySlaacAddressConfiguration::Enabled {
                        temp_valid_lifetime: NonZeroDuration::new(Self::MAX_VALID_LIFETIME)
                            .unwrap(),
                        temp_preferred_lifetime: NonZeroDuration::new(Self::MAX_PREFERRED_LIFETIME)
                            .unwrap(),
                        temp_idgen_retries: Self::IDGEN_RETRIES,
                    },
                ),
                ..Default::default()
            },
        }
    }

    fn maybe_advertise_prefix(
        &self,
        ctx: &mut FakeCtx,
        device: &DeviceId<FakeBindingsCtx>,
    ) -> Subnet<Ipv6Addr> {
        match self {
            Self::LinkLocal => Subnet::new(Ipv6::LINK_LOCAL_UNICAST_SUBNET.network(), 64).unwrap(),
            Self::GlobalStable | Self::GlobalTemporary => {
                let router_mac = Ipv6::TEST_ADDRS.remote_mac;
                let router_ip = router_mac.to_ipv6_link_local().addr().get();
                let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
                let prefix_length = 64;
                let subnet = Subnet::new(prefix, prefix_length).unwrap();

                // Send an update with lifetimes that are smaller than the ones specified in the
                // preferences.
                let valid_lifetime = 10000;
                let preferred_lifetime = 4000;

                receive_prefix_update(
                    ctx,
                    device,
                    router_ip,
                    subnet,
                    preferred_lifetime,
                    valid_lifetime,
                );

                subnet
            }
        }
    }

    fn matches_config_and_scope<Instant>(
        &self,
        config: &Ipv6AddrConfig<Instant>,
        scope: Ipv6Scope,
    ) -> bool {
        let config = match config {
            Ipv6AddrConfig::Manual(_) => return false,
            Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => inner,
        };
        match (self, config) {
            (Self::LinkLocal | Self::GlobalStable, SlaacConfig::Stable { .. }) => {}
            (Self::GlobalTemporary, SlaacConfig::Temporary(_)) => {}
            _ => return false,
        }
        match (self, scope) {
            (Self::LinkLocal, Ipv6Scope::LinkLocal) => {}
            (Self::GlobalStable | Self::GlobalTemporary, Ipv6Scope::Global) => {}
            _ => return false,
        }
        true
    }
}

#[test_case(SlaacType::LinkLocal; "link-local")]
#[test_case(SlaacType::GlobalTemporary; "temporary")]
#[test_case(SlaacType::GlobalStable; "stable")]
fn test_host_slaac_regenerates_address_on_dad_failure(slaac_type: SlaacType) {
    // Check that when a SLAAC address is detected as a duplicate, a new address
    // gets created.
    set_logger_for_test();
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                // Doesn't matter as long as we perform DAD.
                slaac_config: slaac_type.config_update(),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let subnet = slaac_type.maybe_advertise_prefix(&mut ctx, &device);

    let first_addr_entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet
            && slaac_type.matches_config_and_scope(&entry.config, entry.addr_sub().addr().scope())
    })
    .unwrap();
    assert!(!first_addr_entry.flags.assigned);

    receive_neighbor_advertisement_for_duplicate_address(
        &mut ctx,
        &device,
        first_addr_entry.addr_sub().addr(),
    );

    // In response to the advertisement with the duplicate address, a
    // different address should be selected.
    let second_addr_entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet
            && slaac_type.matches_config_and_scope(&entry.config, entry.addr_sub().addr().scope())
    })
    .unwrap();
    let first_addr_sub = first_addr_entry.addr_sub();
    let second_addr_sub = second_addr_entry.addr_sub();
    assert_eq!(first_addr_sub.subnet(), second_addr_sub.subnet());
    assert_ne!(first_addr_sub.addr(), second_addr_sub.addr());

    let (first_addr_valid, dad_counter) = match first_addr_entry.config {
        Ipv6AddrConfig::Manual(_) => panic!("found manual addr instead of SLAAC"),
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => match inner {
            SlaacConfig::Temporary(TemporarySlaacConfig { valid_until, dad_counter, .. }) => {
                (Lifetime::Finite(valid_until), dad_counter)
            }
            SlaacConfig::Stable { valid_until, dad_counter, .. } => (valid_until, dad_counter),
        },
    };
    assert_eq!(dad_counter, 0);

    let (valid_until, dad_counter, creation_time) = match second_addr_entry.config {
        Ipv6AddrConfig::Manual(_) => panic!("found manual addr instead of SLAAC"),
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner, .. }) => match inner {
            SlaacConfig::Temporary(TemporarySlaacConfig {
                valid_until,
                creation_time,
                dad_counter,
                ..
            }) => (Lifetime::Finite(valid_until), dad_counter, creation_time),
            SlaacConfig::Stable { valid_until, creation_time, dad_counter, .. } => {
                (valid_until, dad_counter, creation_time)
            }
        },
    };
    assert_eq!(dad_counter, 1);
    assert_eq!(valid_until, first_addr_valid);
    assert_eq!(creation_time, ctx.bindings_ctx.now());

    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

fn receive_neighbor_advertisement_for_duplicate_address(
    ctx: &mut FakeCtx,
    device: &DeviceId<FakeBindingsCtx>,
    source_ip: Ipv6DeviceAddr,
) {
    let peer_mac = mac!("00:11:22:33:44:55");
    let dest_ip = Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get();
    let router_flag = false;
    let solicited_flag = false;
    let override_flag = true;

    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        ip::icmp::testutil::neighbor_advertisement_ip_packet(
            source_ip.into(),
            dest_ip,
            router_flag,
            solicited_flag,
            override_flag,
            peer_mac,
        ),
    )
}

#[test_case(SlaacType::LinkLocal; "link-local")]
#[test_case(SlaacType::GlobalTemporary; "temporary")]
#[test_case(SlaacType::GlobalStable; "stable")]
fn test_host_slaac_gives_up_after_dad_failures(slaac_type: SlaacType) {
    // Check that when the chosen tentative SLAAC addresses are detected as
    // duplicates enough times, the system gives up.
    set_logger_for_test();
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                // Doesn't matter as long as we perform DAD.
                slaac_config: slaac_type.config_update(),
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let subnet = slaac_type.maybe_advertise_prefix(&mut ctx, &device);

    let match_slaac_prefix_and_type = |entry: &SlaacAddr<FakeInstant>| {
        entry.addr_sub().subnet() == subnet
            && slaac_type.matches_config_and_scope(&entry.config, entry.addr_sub().addr().scope())
    };

    // The system should try several (1 initial + # retries) times to
    // generate an address. In the loop below, each generated address is
    // detected as a duplicate.
    let attempted_addresses: Vec<_> = (0..=SlaacType::IDGEN_RETRIES)
        .map(|_| {
            // An address should be selected. This must be checked using DAD
            // against other hosts on the network.
            let addr_entry =
                get_matching_slaac_address_entry(&ctx, &device, match_slaac_prefix_and_type)
                    .unwrap();
            assert!(!addr_entry.flags.assigned);

            // A response is received to the DAD request indicating that it
            // is a duplicate.
            receive_neighbor_advertisement_for_duplicate_address(
                &mut ctx,
                &device,
                addr_entry.addr_sub().addr(),
            );

            // The address should be unassigned from the device.
            assert_eq!(get_slaac_address_entry(&ctx, &device, *addr_entry.addr_sub()), None);
            *addr_entry.addr_sub()
        })
        .collect();

    // After the last failed try, the system should have given up, and there
    // should be no matching SLAAC address for the subnet.
    assert_eq!(get_matching_slaac_address_entry(&ctx, &device, match_slaac_prefix_and_type), None);

    // All the attempted addresses should be unique.
    let unique_addresses = attempted_addresses.iter().collect::<HashSet<_>>();
    assert_eq!(
        unique_addresses.len(),
        usize::from(1 + SlaacType::IDGEN_RETRIES),
        "not all addresses are unique: {attempted_addresses:?}"
    );
}

#[test]
fn test_host_temporary_slaac_deprecate_before_regen() {
    // Check that if there are multiple non-deprecated addresses in a subnet
    // and the regen timer goes off, no new address is generated. This tests
    // the following scenario:
    //
    // 1. At time T, an address A is created for a subnet whose preferred
    //    lifetime is PA. This results in a regen timer set at T + PA - R.
    // 2. At time T + PA - R, a new address B is created for the same
    //    subnet when the regen timer for A expires, with a preferred
    //    lifetime of PB (PA != PB because of the desync values).
    // 3. Before T + PA, an advertisement is received for the prefix with
    //    preferred lifetime X. Address A is now preferred until T + PA + X
    //    and regenerated at T + PA + X - R and address B is preferred until
    //    (T + PA - R) + PB + X.
    //
    // Since both addresses are preferred, we expect that when the regen
    // timer for address A goes off, it is ignored since there is already
    // another preferred address (namely B) for the subnet.
    set_logger_for_test();
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();

    let router_mac = config.remote_mac;
    let router_ip = router_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;
    let subnet = Subnet::new(prefix, prefix_length).unwrap();

    const MAX_VALID_LIFETIME: Duration = Duration::from_secs(15000);
    const MAX_PREFERRED_LIFETIME: Duration = Duration::from_secs(5000);
    let slaac_config = SlaacConfigurationUpdate {
        temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
            temp_valid_lifetime: NonZeroDuration::new(MAX_VALID_LIFETIME).unwrap(),
            temp_preferred_lifetime: NonZeroDuration::new(MAX_PREFERRED_LIFETIME).unwrap(),
            temp_idgen_retries: 0,
        }),
        ..Default::default()
    };

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                slaac_config,
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    // The prefix updates contains a shorter preferred lifetime than
    // the preferences allow.
    let prefix_preferred_for: Duration = MAX_PREFERRED_LIFETIME * 2 / 3;
    receive_prefix_update(
        &mut ctx,
        &device,
        router_ip,
        subnet,
        prefix_preferred_for.as_secs().try_into().unwrap(),
        MAX_VALID_LIFETIME.as_secs().try_into().unwrap(),
    );

    let first_addr_entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .unwrap();
    let regen_timer_id =
        InnerSlaacTimerId::RegenerateTemporaryAddress { addr_subnet: *first_addr_entry.addr_sub() };
    trace!("advancing to regen for first address");
    assert_next_slaac_timer_integration(&mut ctx.core_ctx(), &device, &regen_timer_id);
    assert_eq!(ctx.trigger_next_timer(), Some(new_slaac_timer_id(&device)));

    // The regeneration timer should cause a new address to be created in
    // the same subnet.
    assert_matches!(
        get_matching_slaac_address_entry(&ctx, &device, |entry| {
            entry.addr_sub().subnet() == subnet
                && entry.addr_sub() != first_addr_entry.addr_sub()
                && is_temporary_slaac_addr(&entry.config)
        }),
        Some(_)
    );

    // Now the router sends a new update that extends the preferred lifetime
    // of addresses.
    receive_prefix_update(
        &mut ctx,
        &device,
        router_ip,
        subnet,
        prefix_preferred_for.as_secs().try_into().unwrap(),
        MAX_VALID_LIFETIME.as_secs().try_into().unwrap(),
    );
    let addresses = get_matching_slaac_address_entries(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .map(|entry| entry.addr_sub().addr())
    .collect::<Vec<_>>();

    let slaac_timers =
        ip::device::testutil::collect_slaac_timers_integration(&mut ctx.core_ctx(), &device);
    for address in &addresses {
        assert_matches!(
            slaac_timers.get(&InnerSlaacTimerId::DeprecateSlaacAddress{
                addr: *address,
            }).copied(),
            Some(deprecate_at) => {
                let preferred_for = deprecate_at - ctx.bindings_ctx.now();
                assert!(preferred_for <= prefix_preferred_for, "{:?} <= {:?}", preferred_for, prefix_preferred_for);
            }
        );
    }

    trace!("advancing to new regen for first address");
    // Running the context forward until the first address is again eligible
    // for regeneration doesn't result in a new address being created.
    assert_next_slaac_timer_integration(&mut ctx.core_ctx(), &device, &regen_timer_id);
    assert_eq!(ctx.trigger_next_timer(), Some(new_slaac_timer_id(&device)));
    assert_eq!(
        get_matching_slaac_address_entries(&ctx, &device, |entry| entry.addr_sub().subnet()
            == subnet
            && is_temporary_slaac_addr(&entry.config))
        .map(|entry| entry.addr_sub().addr())
        .collect::<HashSet<_>>(),
        addresses.iter().cloned().collect()
    );

    trace!("advancing to deprecation for first address");
    // If we continue on until the first address is deprecated, we still
    // shouldn't regenerate since the second address is active.
    assert_next_slaac_timer_integration(
        &mut ctx.core_ctx(),
        &device,
        &InnerSlaacTimerId::DeprecateSlaacAddress { addr: first_addr_entry.addr_sub().addr() },
    );
    assert_eq!(ctx.trigger_next_timer(), Some(new_slaac_timer_id(&device)));

    let remaining_addresses = addresses
        .into_iter()
        .filter(|addr| addr != &first_addr_entry.addr_sub().addr())
        .collect::<HashSet<_>>();
    assert_eq!(
        get_matching_slaac_address_entries(&ctx, &device, |entry| entry.addr_sub().subnet()
            == subnet
            && !entry.config.is_deprecated()
            && is_temporary_slaac_addr(&entry.config))
        .map(|entry| entry.addr_sub().addr())
        .collect::<HashSet<_>>(),
        remaining_addresses
    );
    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

#[test]
fn test_host_temporary_slaac_config_update_skips_regen() {
    // If the NDP configuration gets updated such that the target regen time
    // for an address is moved earlier than the current time, the address
    // should be regenerated immediately.
    set_logger_for_test();
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let eth_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device = eth_device_id.clone().into();
    // No DAD for the auto-generated link-local address.
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    dad_transmits: Some(None),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let router_mac = config.remote_mac;
    let router_ip = router_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;
    let subnet = Subnet::new(prefix, prefix_length).unwrap();

    const MAX_VALID_LIFETIME: Duration = Duration::from_secs(15000);
    let max_preferred_lifetime = Duration::from_secs(5000);
    let slaac_config = SlaacConfigurationUpdate {
        temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
            temp_valid_lifetime: NonZeroDuration::new(MAX_VALID_LIFETIME).unwrap(),
            temp_preferred_lifetime: NonZeroDuration::new(max_preferred_lifetime).unwrap(),
            temp_idgen_retries: 1,
        }),
        ..Default::default()
    };

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                slaac_config,
                ip_config: IpDeviceConfigurationUpdate {
                    // Perform DAD for later addresses.
                    dad_transmits: Some(NonZeroU16::new(1)),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let Ctx { core_ctx, bindings_ctx } = &mut ctx;

    // Set a large value for the retransmit period. This forces
    // REGEN_ADVANCE to be large, which increases the window between when an
    // address is regenerated and when it becomes deprecated.
    Ipv6DeviceHandler::set_discovered_retrans_timer(
        &mut core_ctx.context(),
        bindings_ctx,
        &device,
        NonZeroDuration::new(max_preferred_lifetime / 4).unwrap(),
    );

    receive_prefix_update(
        &mut ctx,
        &device,
        router_ip,
        subnet,
        max_preferred_lifetime.as_secs().try_into().unwrap(),
        MAX_VALID_LIFETIME.as_secs().try_into().unwrap(),
    );

    let first_addr_entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .unwrap();
    let regen_at =
        ip::device::testutil::collect_slaac_timers_integration(&mut ctx.core_ctx(), &device)
            .get(&InnerSlaacTimerId::RegenerateTemporaryAddress {
                addr_subnet: *first_addr_entry.addr_sub(),
            })
            .copied()
            .unwrap();

    let before_regen = regen_at - Duration::from_secs(10);
    // The only events that run before regen should be the DAD timers for
    // the stable and temporary address that were created earlier.
    let dad_timer_ids = get_matching_slaac_address_entries(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet
    })
    .map(|entry| dad_timer_id(&mut ctx, eth_device_id.clone(), entry.addr_sub().addr()))
    .collect::<Vec<_>>();
    ctx.trigger_timers_until_and_expect_unordered(before_regen, dad_timer_ids);

    let preferred_until =
        ip::device::testutil::collect_slaac_timers_integration(&mut ctx.core_ctx(), &device)
            .get(&InnerSlaacTimerId::DeprecateSlaacAddress {
                addr: first_addr_entry.addr_sub().addr(),
            })
            .copied()
            .unwrap();

    let max_preferred_lifetime = max_preferred_lifetime * 4 / 5;
    let slaac_config = SlaacConfigurationUpdate {
        temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
            temp_valid_lifetime: NonZeroDuration::new(MAX_VALID_LIFETIME).unwrap(),
            temp_preferred_lifetime: NonZeroDuration::new(max_preferred_lifetime).unwrap(),
            temp_idgen_retries: 1,
        }),
        ..Default::default()
    };
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate { slaac_config, ..Default::default() },
        )
        .unwrap();

    // Receiving this update should result in requiring a regen time that is
    // before the current time. The address should be regenerated
    // immediately.
    let prefix_preferred_for = preferred_until - ctx.bindings_ctx.now();

    receive_prefix_update(
        &mut ctx,
        &device,
        router_ip,
        subnet,
        prefix_preferred_for.as_secs().try_into().unwrap(),
        MAX_VALID_LIFETIME.as_secs().try_into().unwrap(),
    );

    // The regeneration is still handled by timer, so handle any pending
    // events.
    assert_next_slaac_timer_integration(
        &mut ctx.core_ctx(),
        &device,
        &InnerSlaacTimerId::RegenerateTemporaryAddress {
            addr_subnet: *first_addr_entry.addr_sub(),
        },
    );
    assert_eq!(ctx.trigger_timers_for(Duration::ZERO), vec![new_slaac_timer_id(&device)]);

    let addresses = get_matching_slaac_address_entries(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .map(|entry| entry.addr_sub().addr())
    .collect::<HashSet<_>>();
    assert!(addresses.contains(&first_addr_entry.addr_sub().addr()));
    assert_eq!(addresses.len(), 2);

    // Clean up device references.
    core::mem::drop(device);
    ctx.core_api().device().remove_device(eth_device_id).into_removed();
}

#[test]
fn test_host_temporary_slaac_lifetime_updates_respect_max() {
    // Make sure that the preferred and valid lifetimes of the NDP
    // configuration are respected.

    let src_mac = Ipv6::TEST_ADDRS.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let subnet = subnet_v6!("0102:0304:0506:0708::/64");
    let (mut ctx, device, config) = initialize_with_temporary_addresses_enabled();
    let now = ctx.bindings_ctx.now();
    let start = now;
    let (max_valid_lifetime, max_preferred_lifetime) = assert_matches!(
        config,
        SlaacConfigurationUpdate {
            temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
                temp_valid_lifetime,
                temp_preferred_lifetime,
                temp_idgen_retries: _,
            }),
            stable_address_configuration: _,
        } => (
            temp_valid_lifetime,
            temp_preferred_lifetime,
        )
    );
    let max_valid_until = now.checked_add(max_valid_lifetime.get()).unwrap();
    let max_preferred_until = now.checked_add(max_preferred_lifetime.get()).unwrap();

    let interface_identifier = OpaqueIid::new(
        subnet,
        &*Ipv6::TEST_ADDRS.local_mac,
        None::<[_; 0]>,
        // Clone the RNG so we can see what the next value (which will be
        // used to generate the temporary address) will be.
        OpaqueIidNonce::Random(ctx.bindings_ctx.rng().deep_clone().random()),
        &ctx.core_ctx.ipv6().slaac_temp_secret_key,
    );
    let mut expected_addr = [1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0];
    expected_addr[8..].copy_from_slice(&interface_identifier.to_be_bytes()[..8]);
    let expected_addr =
        NonMappedAddr::new(UnicastAddr::new(Ipv6Addr::from(expected_addr)).unwrap()).unwrap();
    let expected_addr_sub = AddrSubnet::from_witness(expected_addr, subnet.prefix()).unwrap();

    // Send an update with lifetimes that are smaller than the ones specified in the preferences.
    let valid_lifetime = 2000;
    let preferred_lifetime = 1500;
    assert!(u64::from(valid_lifetime) < max_valid_lifetime.get().as_secs());
    assert!(u64::from(preferred_lifetime) < max_preferred_lifetime.get().as_secs());
    receive_prefix_update(&mut ctx, &device, src_ip, subnet, preferred_lifetime, valid_lifetime);

    let entry = get_slaac_address_entry(&ctx, &device, expected_addr_sub).unwrap();
    let expected_valid_until = now.checked_add(Duration::from_secs(valid_lifetime.into())).unwrap();
    let expected_preferred_until =
        now.checked_add(Duration::from_secs(preferred_lifetime.into())).unwrap();
    assert!(
        expected_valid_until < max_valid_until,
        "expected {:?} < {:?}",
        expected_valid_until,
        max_valid_until
    );
    assert!(expected_preferred_until < max_preferred_until);

    assert_slaac_lifetimes_enforced(
        &mut ctx,
        &device,
        entry,
        expected_valid_until,
        expected_preferred_until,
    );

    // After some time passes, another update is received with the same lifetimes for the
    // prefix. Per RFC 8981 Section 3.4.1, the lifetimes for the address should obey the
    // overall constraints expressed in the preferences.

    assert_eq!(ctx.trigger_timers_for(Duration::from_secs(1000)), []);
    let now = ctx.bindings_ctx.now();
    let expected_valid_until = now.checked_add(Duration::from_secs(valid_lifetime.into())).unwrap();
    let expected_preferred_until =
        now.checked_add(Duration::from_secs(preferred_lifetime.into())).unwrap();

    // The preferred lifetime advertised by the router is now past the max allowed by
    // the NDP configuration.
    assert!(expected_preferred_until > max_preferred_until);

    receive_prefix_update(&mut ctx, &device, src_ip, subnet, preferred_lifetime, valid_lifetime);

    let entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .unwrap();
    let desync_factor = match entry.config {
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
            inner:
                SlaacConfig::Temporary(TemporarySlaacConfig {
                    desync_factor,
                    creation_time: _,
                    valid_until: _,
                    dad_counter: _,
                }),
            ..
        }) => desync_factor,
        other => panic!("got {other:?} expected temporary "),
    };
    assert_slaac_lifetimes_enforced(
        &mut ctx,
        &device,
        entry,
        expected_valid_until,
        max_preferred_until - desync_factor,
    );

    // Update the max allowed lifetime in the NDP configuration. This won't take effect until
    // the next router advertisement is reeived.
    let max_valid_lifetime = max_preferred_lifetime;
    let idgen_retries = 3;
    let slaac_config = SlaacConfigurationUpdate {
        temporary_address_configuration: Some(TemporarySlaacAddressConfiguration::Enabled {
            temp_valid_lifetime: max_valid_lifetime,
            temp_preferred_lifetime: max_preferred_lifetime,
            temp_idgen_retries: idgen_retries,
        }),
        ..Default::default()
    };

    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate { slaac_config, ..Default::default() },
        )
        .unwrap();

    // The new valid time is measured from the time at which the address was created (`start`),
    // not the current time (`now`). That means the max valid lifetime takes precedence over
    // the router's advertised valid lifetime.
    let max_valid_until = start.checked_add(max_valid_lifetime.get()).unwrap();
    assert!(expected_valid_until > max_valid_until);
    receive_prefix_update(&mut ctx, &device, src_ip, subnet, preferred_lifetime, valid_lifetime);

    let entry = get_matching_slaac_address_entry(&ctx, &device, |entry| {
        entry.addr_sub().subnet() == subnet && is_temporary_slaac_addr(&entry.config)
    })
    .unwrap();
    assert_slaac_lifetimes_enforced(
        &mut ctx,
        &device,
        entry,
        max_valid_until,
        max_preferred_until - desync_factor,
    );
    // Clean up device references.
    ctx.core_api().device().remove_device(device.unwrap_ethernet()).into_removed();
}

#[test]
fn test_remove_stable_slaac_address() {
    let config = Ipv6::TEST_ADDRS;
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<EthernetLinkDevice>()
        .add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: config.local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        )
        .into();
    let _: Ipv6DeviceConfigurationUpdate = ctx
        .core_api()
        .device_ip::<Ipv6>()
        .update_configuration(
            &device,
            Ipv6DeviceConfigurationUpdate {
                slaac_config: SlaacConfigurationUpdate {
                    stable_address_configuration: Some(
                        StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                    ),
                    ..Default::default()
                },
                ip_config: IpDeviceConfigurationUpdate {
                    ip_enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();

    let src_mac = config.remote_mac;
    let src_ip = src_mac.to_ipv6_link_local().addr().get();
    let prefix = Ipv6Addr::from([1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, 0, 0, 0, 0]);
    let prefix_length = 64;
    let subnet = Subnet::new(prefix, prefix_length).unwrap();
    let expected_addr = calculate_slaac_addr_sub(subnet, config.local_mac.to_eui64()).addr();

    // Receive a new RA with new prefix (autonomous).
    //
    // Should get a new IP.

    const VALID_LIFETIME_SECS: u32 = 10000;
    const PREFERRED_LIFETIME_SECS: u32 = 9000;

    let icmpv6_packet_buf = slaac_packet_buf(
        src_ip,
        Ipv6::ALL_NODES_LINK_LOCAL_MULTICAST_ADDRESS.get(),
        prefix,
        prefix_length,
        false,
        true,
        VALID_LIFETIME_SECS,
        PREFERRED_LIFETIME_SECS,
    );
    ctx.test_api().receive_ip_packet::<Ipv6, _>(
        &device,
        Some(FrameDestination::Multicast),
        icmpv6_packet_buf,
    );

    // Should have gotten a new IP.
    let now = ctx.bindings_ctx.now();
    let valid_until = now + Duration::from_secs(VALID_LIFETIME_SECS.into());
    let preferred_until = now + Duration::from_secs(PREFERRED_LIFETIME_SECS.into());
    let expected_address_entry = SlaacAddr {
        addr_sub: AddrSubnet::<Ipv6Addr, _>::new(expected_addr.into(), prefix_length).unwrap(),
        config: Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
            inner: SlaacConfig::Stable {
                valid_until: Lifetime::Finite(valid_until),
                creation_time: now,
                regen_counter: 0,
                dad_counter: 0,
            },
            preferred_lifetime: PreferredLifetime::preferred_until(preferred_until),
        }),
        flags: IpAddressFlags { assigned: true },
    };
    assert_eq!(get_global_ipv6_addrs(&ctx, &device), [expected_address_entry]);
    // Make sure deprecate and invalidation timers are set.
    assert_slaac_timers_integration(
        &mut ctx.core_ctx(),
        &device,
        [
            (
                InnerSlaacTimerId::DeprecateSlaacAddress { addr: expected_addr },
                now + Duration::from_secs(PREFERRED_LIFETIME_SECS.into()),
            ),
            (InnerSlaacTimerId::InvalidateSlaacAddress { addr: expected_addr }, valid_until),
        ],
    );

    // Deleting the address should cancel its SLAAC timers.
    let expected_addr_sub = AddrSubnet::new(expected_addr.get(), prefix_length).unwrap();
    assert_eq!(
        ctx.core_api()
            .device_ip::<Ipv6>()
            .del_ip_addr(&device, expected_addr.into_specified())
            .unwrap()
            .into_removed(),
        expected_addr_sub
    );
    assert_empty(get_global_ipv6_addrs(&ctx, &device));
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
}

#[test]
fn test_remove_temporary_slaac_address() {
    // We use the infinite lifetime so that the stable address does not have
    // any timers as it is valid and preferred forever. As a result, we will
    // only observe timers for temporary addresses.
    let (mut ctx, device, expected_addr) =
        test_host_generate_temporary_slaac_address(INFINITE_LIFETIME, INFINITE_LIFETIME);

    let expected_addr_sub = AddrSubnet::new(expected_addr.get(), 64).unwrap();
    // Deleting the address should cancel its SLAAC timers.
    assert_eq!(
        ctx.core_api()
            .device_ip::<Ipv6>()
            .del_ip_addr(&device, expected_addr.into_specified())
            .unwrap()
            .into_removed(),
        expected_addr_sub
    );
    assert_empty(get_global_ipv6_addrs(&ctx, &device).into_iter().filter(|e| match e.config {
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig { inner: SlaacConfig::Temporary(_), .. }) => true,
        Ipv6AddrConfig::Slaac(Ipv6AddrSlaacConfig {
            inner:
                SlaacConfig::Stable {
                    valid_until: _,
                    creation_time: _,
                    regen_counter: _,
                    dad_counter: _,
                },
            ..
        }) => false,
        Ipv6AddrConfig::Manual(_manual_config) => false,
    }));
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
}
