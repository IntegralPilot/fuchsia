// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;

use net_types::ethernet::Mac;
use net_types::ip::{AddrSubnet, Ip, Ipv4, Ipv4Addr, Ipv6};
use net_types::{MulticastAddr, SpecifiedAddr, Witness as _};
use packet::ParsablePacket as _;
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::icmp::IcmpSenderZeroCode;
use packet_formats::icmp::mld::{MulticastListenerDone, MulticastListenerReport};
use packet_formats::igmp::messages::IgmpPacket;
use packet_formats::ip::Ipv4Proto;
use packet_formats::testutil::{
    parse_icmp_packet_in_ip_packet_in_ethernet_frame, parse_ip_packet_in_ethernet_frame,
};

use netstack3_base::testutil::{TestAddrs, TestIpExt as _};
use netstack3_core::device::{
    DeviceId, EthernetCreationProperties, EthernetLinkDevice, MaxEthernetFrameSize,
};
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, FakeBindingsCtx, FakeCtx,
};
use netstack3_core::{InstantContext as _, StackStateBuilder, TimerId};
use netstack3_device::testutil::IPV6_MIN_IMPLIED_MAX_FRAME_SIZE;
use netstack3_ip::device::{
    IpDeviceConfigurationUpdate, Ipv4DeviceConfigurationUpdate, Ipv4DeviceTimerId,
    Ipv6DeviceConfigurationUpdate, Ipv6DeviceTimerId, SlaacConfigurationUpdate,
    StableSlaacAddressConfiguration,
};
use netstack3_ip::gmp::{
    IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL, IgmpConfigMode, IgmpTimerId,
    MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL, MldConfigMode, MldTimerId,
};

const V4_HOST_ADDR: SpecifiedAddr<Ipv4Addr> =
    unsafe { SpecifiedAddr::new_unchecked(Ipv4Addr::new([192, 168, 0, 2])) };
const V4_GROUP_ADDR: MulticastAddr<Ipv4Addr> = Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS;

#[test]
fn test_igmpv2_enable_disable_integration() {
    let TestAddrs { local_mac, remote_mac: _, local_ip: _, remote_ip: _, subnet: _ } =
        Ipv4::TEST_ADDRS;

    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    let eth_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: MaxEthernetFrameSize::from_mtu(Ipv4::MINIMUM_LINK_MTU).unwrap(),
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id: DeviceId<_> = eth_device_id.clone().into();
    ctx.core_api()
        .device_ip::<Ipv4>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(V4_HOST_ADDR.get(), 24).unwrap())
        .unwrap();
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();

    let now = ctx.bindings_ctx.now();
    // NB: The assertions made on this timer_id are valid because we only
    // ever join a single group for the duration of the test. Given that,
    // the timer ID in bindings matches the state of the single timer id in
    // the local timer heap in GMP.
    let timer_id = TimerId::from(
        Ipv4DeviceTimerId::from(IgmpTimerId::new(device_id.downgrade())).into_common(),
    );
    let range = now..=(now + IGMP_DEFAULT_UNSOLICITED_REPORT_INTERVAL);
    struct TestConfig {
        ip_enabled: bool,
        gmp_enabled: bool,
    }

    let set_config = |ctx: &mut FakeCtx, TestConfig { ip_enabled, gmp_enabled }| {
        let _: Ipv4DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv4>()
            .update_configuration(
                &device_id,
                Ipv4DeviceConfigurationUpdate {
                    ip_config: IpDeviceConfigurationUpdate {
                        ip_enabled: Some(ip_enabled),
                        gmp_enabled: Some(gmp_enabled),
                        ..Default::default()
                    },
                    igmp_mode: Some(IgmpConfigMode::V2),
                    ..Default::default()
                },
            )
            .unwrap();
    };
    let check_sent_report = |bindings_ctx: &mut FakeBindingsCtx| {
        let frames = bindings_ctx.take_ethernet_frames();
        let (egress_device, frame) = assert_matches!(&frames[..], [x] => x);
        assert_eq!(egress_device, &eth_device_id);
        let (body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
            parse_ip_packet_in_ethernet_frame::<Ipv4>(frame, EthernetFrameLengthCheck::NoCheck)
                .unwrap();
        assert_eq!(src_mac, local_mac.get());
        assert_eq!(dst_mac, Mac::from(&V4_GROUP_ADDR));
        assert_eq!(src_ip, V4_HOST_ADDR.get());
        assert_eq!(dst_ip, V4_GROUP_ADDR.get());
        assert_eq!(proto, Ipv4Proto::Igmp);
        assert_eq!(ttl, 1);
        let mut bv = &body[..];
        assert_matches!(
            IgmpPacket::parse(&mut bv, ()).unwrap(),
            IgmpPacket::MembershipReportV2(msg) => {
                assert_eq!(msg.group_addr(), V4_GROUP_ADDR.get());
            }
        );
    };
    let check_sent_leave = |bindings_ctx: &mut FakeBindingsCtx| {
        let frames = bindings_ctx.take_ethernet_frames();
        let (egress_device, frame) = assert_matches!(&frames[..], [x] => x);

        assert_eq!(egress_device, &eth_device_id);
        let (body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
            parse_ip_packet_in_ethernet_frame::<Ipv4>(frame, EthernetFrameLengthCheck::NoCheck)
                .unwrap();
        assert_eq!(src_mac, local_mac.get());
        assert_eq!(dst_mac, Mac::from(&Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS));
        assert_eq!(src_ip, V4_HOST_ADDR.get());
        assert_eq!(dst_ip, Ipv4::ALL_ROUTERS_MULTICAST_ADDRESS.get());
        assert_eq!(proto, Ipv4Proto::Igmp);
        assert_eq!(ttl, 1);
        let mut bv = &body[..];
        assert_matches!(
            IgmpPacket::parse(&mut bv, ()).unwrap(),
            IgmpPacket::LeaveGroup(msg) => {
                assert_eq!(msg.group_addr(), V4_GROUP_ADDR.get());
            }
        );
    };

    // Enable IPv4 and IGMP, then join `V4_GROUP_ADDR`.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.test_api().join_ip_multicast(&device_id, V4_GROUP_ADDR);
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(timer_id.clone(), range.clone())]);
    check_sent_report(&mut ctx.bindings_ctx);

    // Disable IGMP.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: false });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    check_sent_leave(&mut ctx.bindings_ctx);

    // Enable IGMP but disable IPv4.
    //
    // Should do nothing.
    set_config(&mut ctx, TestConfig { ip_enabled: false, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Disable IGMP but enable IPv4.
    //
    // Should do nothing.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: false });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Enable IGMP.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(timer_id.clone(), range.clone())]);
    check_sent_report(&mut ctx.bindings_ctx);

    // Disable IPv4.
    set_config(&mut ctx, TestConfig { ip_enabled: false, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    check_sent_leave(&mut ctx.bindings_ctx);

    // Enable IPv4.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(timer_id, range)]);
    check_sent_report(&mut ctx.bindings_ctx);

    core::mem::drop(device_id);
    ctx.core_api().device().remove_device(eth_device_id).into_removed();
}

#[test]
fn test_mldv1_enable_disable_integration() {
    let TestAddrs { local_mac, remote_mac: _, local_ip: _, remote_ip: _, subnet: _ } =
        Ipv6::TEST_ADDRS;

    let mut ctx = FakeCtx::new_with_builder(StackStateBuilder::default());

    let eth_device_id =
        ctx.core_api().device::<EthernetLinkDevice>().add_device_with_default_state(
            EthernetCreationProperties {
                tx_offload_spec: Default::default(),
                mac: local_mac,
                max_frame_size: IPV6_MIN_IMPLIED_MAX_FRAME_SIZE,
            },
            DEFAULT_INTERFACE_METRIC,
        );
    let device_id: DeviceId<_> = eth_device_id.clone().into();

    let now = ctx.bindings_ctx.now();
    let ll_addr = local_mac.to_ipv6_link_local().addr();
    let snmc_addr = ll_addr.to_solicited_node_address();

    // NB: The assertions made on this timer_id are valid because we only
    // ever join a single group for the duration of the test. Given that,
    // the timer ID in bindings matches the state of the single timer id in
    // the local timer heap in GMP.
    let snmc_timer_id =
        TimerId::from(Ipv6DeviceTimerId::Mld(MldTimerId::new(device_id.downgrade())).into_common());
    let range = now..=(now + MLD_DEFAULT_UNSOLICITED_REPORT_INTERVAL);
    struct TestConfig {
        ip_enabled: bool,
        gmp_enabled: bool,
    }
    let set_config = |ctx: &mut FakeCtx, TestConfig { ip_enabled, gmp_enabled }| {
        let _: Ipv6DeviceConfigurationUpdate = ctx
            .core_api()
            .device_ip::<Ipv6>()
            .update_configuration(
                &device_id,
                Ipv6DeviceConfigurationUpdate {
                    max_router_solicitations: Some(None),
                    // Auto-generate a link-local address.
                    slaac_config: SlaacConfigurationUpdate {
                        stable_address_configuration: Some(
                            StableSlaacAddressConfiguration::ENABLED_WITH_EUI64,
                        ),
                        ..Default::default()
                    },
                    ip_config: IpDeviceConfigurationUpdate {
                        ip_enabled: Some(ip_enabled),
                        gmp_enabled: Some(gmp_enabled),
                        // TODO(https://fxbug.dev/42180878): Make sure that DAD resolving
                        // for a link-local address results in reports sent with a
                        // specified source address.
                        dad_transmits: Some(None),
                        ..Default::default()
                    },
                    mld_mode: Some(MldConfigMode::V1),
                    ..Default::default()
                },
            )
            .unwrap();
    };
    let check_sent_report = |bindings_ctx: &mut FakeBindingsCtx| {
        let frames = bindings_ctx.take_ethernet_frames();
        let (egress_device, frame) = assert_matches!(&frames[..], [x] => x);
        assert_eq!(egress_device, &eth_device_id);
        let (src_mac, dst_mac, src_ip, dst_ip, ttl, _message, code) =
                parse_icmp_packet_in_ip_packet_in_ethernet_frame::<
                    Ipv6,
                    _,
                    MulticastListenerReport,
                    _,
                >(frame, EthernetFrameLengthCheck::NoCheck, |icmp| {
                    assert_eq!(icmp.body().group_addr, snmc_addr.get());
                })
                .unwrap();
        assert_eq!(src_mac, local_mac.get());
        assert_eq!(dst_mac, Mac::from(&snmc_addr));
        assert_eq!(src_ip, ll_addr.get());
        assert_eq!(dst_ip, snmc_addr.get());
        assert_eq!(ttl, 1);
        assert_eq!(code, IcmpSenderZeroCode);
        assert_eq!(dst_ip, snmc_addr.get());
        assert_eq!(ttl, 1);
        assert_eq!(code, IcmpSenderZeroCode);
    };
    let check_sent_done = |bindings_ctx: &mut FakeBindingsCtx, specified_source: bool| {
        let frames = bindings_ctx.take_ethernet_frames();
        let (egress_device, frame) = assert_matches!(&frames[..], [x] => x);
        assert_eq!(egress_device, &eth_device_id);
        let (src_mac, dst_mac, src_ip, dst_ip, ttl, _message, code) =
            parse_icmp_packet_in_ip_packet_in_ethernet_frame::<Ipv6, _, MulticastListenerDone, _>(
                frame,
                EthernetFrameLengthCheck::NoCheck,
                |icmp| {
                    assert_eq!(icmp.body().group_addr, snmc_addr.get());
                },
            )
            .unwrap();
        assert_eq!(src_mac, local_mac.get());
        assert_eq!(dst_mac, Mac::from(&Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS));
        assert_eq!(
            src_ip,
            if specified_source { ll_addr.get() } else { Ipv6::UNSPECIFIED_ADDRESS }
        );
        assert_eq!(dst_ip, Ipv6::ALL_ROUTERS_LINK_LOCAL_MULTICAST_ADDRESS.get());
        assert_eq!(ttl, 1);
        assert_eq!(code, IcmpSenderZeroCode);
    };

    // Enable IPv6 and MLD.
    //
    // MLD should be performed for the auto-generated link-local address's
    // solicited-node multicast address.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.bindings_ctx
        .timer_ctx()
        .assert_timers_installed_range([(snmc_timer_id.clone(), range.clone())]);
    check_sent_report(&mut ctx.bindings_ctx);

    // Disable MLD.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: false });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    check_sent_done(&mut ctx.bindings_ctx, true);

    // Enable MLD but disable IPv6.
    //
    // Should do nothing.
    set_config(&mut ctx, TestConfig { ip_enabled: false, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Disable MLD but enable IPv6.
    //
    // Should do nothing.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: false });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Enable MLD.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.bindings_ctx
        .timer_ctx()
        .assert_timers_installed_range([(snmc_timer_id.clone(), range.clone())]);
    check_sent_report(&mut ctx.bindings_ctx);

    // Disable IPv6.
    set_config(&mut ctx, TestConfig { ip_enabled: false, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_no_timers_installed();
    check_sent_done(&mut ctx.bindings_ctx, false);

    // Enable IPv6.
    set_config(&mut ctx, TestConfig { ip_enabled: true, gmp_enabled: true });
    ctx.bindings_ctx.timer_ctx().assert_timers_installed_range([(snmc_timer_id, range)]);
    check_sent_report(&mut ctx.bindings_ctx);

    // Remove the device to cleanup all dangling references.
    core::mem::drop(device_id);
    ctx.core_api().device().remove_device(eth_device_id).into_removed();
}
