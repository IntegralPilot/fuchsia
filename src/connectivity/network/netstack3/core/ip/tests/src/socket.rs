// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec::Vec;
use core::num::NonZeroU8;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;

use net_types::ip::{AddrSubnet, GenericOverIp, Ip, IpAddr, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, Mtu};
use net_types::{SpecifiedAddr, Witness};
use packet::{Buf, InnerPacketBuilder, NestablePacketBuilder as _, ParseBuffer, Serializer as _};
use packet_formats::ethernet::EthernetFrameLengthCheck;
use packet_formats::icmp::{IcmpIpExt, IcmpZeroCode};
use packet_formats::ip::IpPacket;
use packet_formats::ipv4::Ipv4Packet;
use packet_formats::testutil::{parse_ethernet_frame, parse_ip_packet_in_ethernet_frame};
use test_case::test_case;

use netstack3_base::socket::SocketIpAddr;
use netstack3_base::testutil::{TestAddrs, TestIpExt, set_logger_for_test};
use netstack3_base::{
    CounterCollection as _, CounterContext, EitherDeviceId, IpDeviceAddr, Mms,
    NetworkSerializationContext, ResourceCounterContext,
};
use netstack3_core::device::{DeviceId, EthernetLinkDevice};
use netstack3_core::testutil::{CtxPairExt as _, FakeBindingsCtx, FakeCtx, FakeCtxBuilder};
use netstack3_core::{CoreTxMetadata, IpExt};
use netstack3_ip::device::IpDeviceIpExt;
use netstack3_ip::marker::OptionDelegationMarker;
use netstack3_ip::socket::{
    DefaultIpSocketOptions, DelegatedRouteResolutionOptions, DelegatedSendOptions,
    DeviceIpSocketHandler, IpSockCreationError, IpSockDefinition, IpSockSendError, IpSocketArgs,
    IpSocketHandler, MmsError, RouteResolutionOptions, SendOptions,
};
use netstack3_ip::testutil::IpCounterExpectations;
use netstack3_ip::{
    self as ip, AddableEntryEither, AddableMetric, IpCounters, IpDeviceMtuContext, RawMetric,
    ResolveRouteError, device,
};

enum AddressType {
    LocallyOwned,
    Remote,
    Unspecified {
        // Indicates whether or not it should be possible for the stack to
        // select an address when the client fails to specify one.
        can_select: bool,
    },
    Unroutable,
}

enum DeviceType {
    Unspecified,
    OtherDevice,
    LocalDevice,
}

struct NewSocketTestCase {
    local_ip_type: AddressType,
    remote_ip_type: AddressType,
    device_type: DeviceType,
    transparent: bool,
    expected_result: Result<(), IpSockCreationError>,
}

trait IpSocketIpExt: TestIpExt + IcmpIpExt + IpExt + netstack3_base::IpExt {
    fn multicast_addr(host: u8) -> SpecifiedAddr<Self::Addr>;
}

impl IpSocketIpExt for Ipv4 {
    fn multicast_addr(host: u8) -> SpecifiedAddr<Self::Addr> {
        let [a, b, c, _] = Ipv4::MULTICAST_SUBNET.network().ipv4_bytes();
        SpecifiedAddr::new(Ipv4Addr::new([a, b, c, host])).unwrap()
    }
}
impl IpSocketIpExt for Ipv6 {
    fn multicast_addr(host: u8) -> SpecifiedAddr<Self::Addr> {
        let mut bytes = Ipv6::MULTICAST_SUBNET.network().ipv6_bytes();
        bytes[15] = host;
        SpecifiedAddr::new(Ipv6Addr::from_bytes(bytes)).unwrap()
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct WithHopLimit(Option<NonZeroU8>);

impl OptionDelegationMarker for WithHopLimit {}

impl<I: IpExt> DelegatedRouteResolutionOptions<I> for WithHopLimit {}

impl<I: IpExt> DelegatedSendOptions<I> for WithHopLimit {
    fn hop_limit(&self, _destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
        let Self(hop_limit) = self;
        *hop_limit
    }
}

struct RestrictMtu<T> {
    mtu: u32,
    inner: T,
}

impl<T> OptionDelegationMarker for RestrictMtu<T> {}

impl<T: SendOptions<I>, I: IpExt> DelegatedSendOptions<I> for RestrictMtu<T> {
    fn delegate(&self) -> &impl SendOptions<I> {
        &self.inner
    }

    fn mtu(&self) -> Mtu {
        Mtu::new(self.mtu)
    }
}

impl<T: RouteResolutionOptions<I>, I: Ip> DelegatedRouteResolutionOptions<I> for RestrictMtu<T> {
    fn delegate(&self) -> &impl RouteResolutionOptions<I> {
        &self.inner
    }
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
fn remove_all_local_addrs<I: IpExt>(ctx: &mut FakeCtx) {
    let devices = device::IpDeviceConfigurationContext::<I, _>::with_devices_and_state(
        &mut ctx.core_ctx(),
        |devices, _ctx| devices.collect::<Vec<_>>(),
    );
    for device in devices {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct WrapVecAddrSubnet<I: Ip + IpDeviceIpExt>(
            Vec<AddrSubnet<I::Addr, I::AssignedWitness>>,
        );

        let WrapVecAddrSubnet(subnets) = I::map_ip_out(
            (&mut ctx.core_ctx(), &device),
            |(core_ctx, device)| {
                ip::device::testutil::with_assigned_ipv4_addr_subnets(core_ctx, device, |addrs| {
                    WrapVecAddrSubnet(addrs.collect::<Vec<_>>())
                })
            },
            |(core_ctx, device)| {
                ip::device::testutil::with_assigned_ipv6_addr_subnets(core_ctx, device, |addrs| {
                    WrapVecAddrSubnet(addrs.collect::<Vec<_>>())
                })
            },
        );

        for subnet in subnets {
            assert_eq!(
                ctx.core_api()
                    .device_ip::<I>()
                    .del_ip_addr(&device, subnet.addr().into())
                    .expect("failed to remove addr from device")
                    .into_removed(),
                subnet.to_witness()
            );
        }
    }
}

struct WithTransparent(bool);

impl OptionDelegationMarker for WithTransparent {}

impl<I: IpExt> DelegatedSendOptions<I> for WithTransparent {}

impl<I: IpExt> DelegatedRouteResolutionOptions<I> for WithTransparent {
    fn transparent(&self) -> bool {
        let Self(transparent) = self;
        *transparent
    }
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unroutable,
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Err(ResolveRouteError::NoSrcAddr.into()),
    }; "unroutable local to remote")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::LocallyOwned,
        remote_ip_type: AddressType::Unroutable,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Err(ResolveRouteError::Unreachable.into()),
    }; "local to unroutable remote")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::LocallyOwned,
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Ok(()),
    }; "local to remote")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unspecified { can_select: true },
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Ok(()),
    }; "unspecified to remote")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unspecified { can_select: true },
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::LocalDevice,
        transparent: false,
        expected_result: Ok(()),
    }; "unspecified to remote through local device")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unspecified { can_select: true },
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::OtherDevice,
        transparent: false,
        expected_result: Err(ResolveRouteError::Unreachable.into()),
    }; "unspecified to remote through other device")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unspecified { can_select: false },
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Err(ResolveRouteError::NoSrcAddr.into()),
    }; "new unspcified to remote can't select")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Remote,
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Err(ResolveRouteError::NoSrcAddr.into()),
    }; "new remote to remote")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::LocallyOwned,
        remote_ip_type: AddressType::LocallyOwned,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Ok(()),
    }; "new local to local")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Unspecified { can_select: true },
        remote_ip_type: AddressType::LocallyOwned,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Ok(()),
    }; "new unspecified to local")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Remote,
        remote_ip_type: AddressType::LocallyOwned,
        device_type: DeviceType::Unspecified,
        transparent: false,
        expected_result: Err(ResolveRouteError::NoSrcAddr.into()),
    }; "new remote to local")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Remote,
        remote_ip_type: AddressType::LocallyOwned,
        device_type: DeviceType::Unspecified,
        transparent: true,
        expected_result: Ok(()),
    }; "new transparent remote to local")]
#[test_case(NewSocketTestCase {
        local_ip_type: AddressType::Remote,
        remote_ip_type: AddressType::Remote,
        device_type: DeviceType::Unspecified,
        transparent: true,
        expected_result: Ok(()),
    }; "new transparent remote to remote")]
fn test_new<I: IpSocketIpExt + IpExt>(test_case: NewSocketTestCase) {
    let cfg = I::TEST_ADDRS;
    let proto = I::ICMP_IP_PROTO;

    let TestAddrs { local_ip, remote_ip, subnet, local_mac: _, remote_mac: _ } = cfg;
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(cfg).build();
    let loopback_device_id = ctx.test_api().add_loopback();

    let NewSocketTestCase {
        local_ip_type,
        remote_ip_type,
        transparent,
        device_type,
        expected_result,
    } = test_case;

    let local_device: Option<DeviceId<_>> = match device_type {
        DeviceType::Unspecified => None,
        DeviceType::LocalDevice => Some(device_ids[0].clone().into()),
        DeviceType::OtherDevice => Some(loopback_device_id.into()),
    };

    let (expected_from_ip, from_ip) = match local_ip_type {
        AddressType::LocallyOwned => (local_ip, Some(local_ip)),
        AddressType::Remote => (remote_ip, Some(remote_ip)),
        AddressType::Unspecified { can_select } => {
            if !can_select {
                remove_all_local_addrs::<I>(&mut ctx);
            }
            (local_ip, None)
        }
        AddressType::Unroutable => {
            remove_all_local_addrs::<I>(&mut ctx);
            (local_ip, Some(local_ip))
        }
    };
    let to_ip = match remote_ip_type {
        AddressType::LocallyOwned => local_ip,
        AddressType::Remote => remote_ip,
        AddressType::Unspecified { can_select: _ } => {
            panic!("remote_ip_type cannot be unspecified")
        }
        AddressType::Unroutable => {
            ctx.test_api().del_routes_to_subnet(subnet.into()).unwrap();
            remote_ip
        }
    };
    let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

    let get_expected_result = |template| expected_result.map(|()| template);
    let weak_local_device = local_device.as_ref().map(|d| d.downgrade());
    let template = IpSockDefinition {
        remote_ip: SocketIpAddr::try_from(to_ip).unwrap(),
        local_ip: IpDeviceAddr::try_from(expected_from_ip).unwrap(),
        device: weak_local_device.clone(),
        proto,
    };

    let res = IpSocketHandler::<I, _>::new_ip_socket(
        &mut core_ctx.context(),
        bindings_ctx,
        IpSocketArgs {
            device: weak_local_device.as_ref().map(EitherDeviceId::Weak),
            local_ip: from_ip.map(|a| IpDeviceAddr::try_from(a).unwrap()),
            remote_ip: SocketIpAddr::try_from(to_ip).unwrap(),
            proto,
            options: &WithTransparent(transparent),
        },
    );
    assert_eq!(res.map(|s| s.definition().clone()), get_expected_result(template));
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(AddressType::LocallyOwned, AddressType::LocallyOwned; "local to local")]
#[test_case(AddressType::Unspecified { can_select: true },
        AddressType::LocallyOwned; "unspecified to local")]
#[test_case(AddressType::LocallyOwned, AddressType::Remote; "local to remote")]
fn test_send_local<I: IpSocketIpExt + IpExt>(
    from_addr_type: AddressType,
    to_addr_type: AddressType,
) {
    set_logger_for_test();

    use packet_formats::icmp::{IcmpEchoRequest, IcmpPacketBuilder};

    let TestAddrs::<I::Addr> { subnet, local_ip, remote_ip, local_mac, remote_mac: _ } =
        I::TEST_ADDRS;

    let mut builder = FakeCtxBuilder::default();
    let device_idx = builder.add_device(local_mac);
    let (mut ctx, device_ids) = builder.build();
    let device_id: DeviceId<_> = device_ids[device_idx].clone().into();

    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(local_ip.get(), 16).unwrap())
        .unwrap();
    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(remote_ip.get(), 16).unwrap())
        .unwrap();
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            subnet.into(),
            device_id.clone(),
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
        .unwrap();

    let loopback_device_id: DeviceId<FakeBindingsCtx> = ctx.test_api().add_loopback().into();
    let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

    let (expected_from_ip, from_ip) = match from_addr_type {
        AddressType::LocallyOwned => (local_ip, Some(local_ip)),
        AddressType::Remote => panic!("from_addr_type cannot be remote"),
        AddressType::Unspecified { can_select: _ } => (local_ip, None),
        AddressType::Unroutable => panic!("from_addr_type cannot be unroutable"),
    };

    let to_ip = match to_addr_type {
        AddressType::LocallyOwned => local_ip,
        AddressType::Remote => remote_ip,
        AddressType::Unspecified { can_select: _ } => {
            panic!("to_addr_type cannot be unspecified")
        }
        AddressType::Unroutable => panic!("to_addr_type cannot be unroutable"),
    };

    let sock = IpSocketHandler::<I, _>::new_ip_socket(
        &mut core_ctx.context(),
        bindings_ctx,
        IpSocketArgs {
            device: None,
            local_ip: from_ip.map(|a| IpDeviceAddr::try_from(a).unwrap()),
            remote_ip: SocketIpAddr::try_from(to_ip).unwrap(),
            proto: I::ICMP_IP_PROTO,
            options: &DefaultIpSocketOptions,
        },
    )
    .unwrap();

    let reply = IcmpEchoRequest::new(0, 0).reply();
    let body = &[1, 2, 3, 4];
    let buffer =
        IcmpPacketBuilder::<I, _>::new(expected_from_ip.get(), to_ip.get(), IcmpZeroCode, reply)
            .wrap_body(Buf::new(body.to_vec(), ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();

    // Send an echo packet on the socket and validate that the packet is
    // delivered locally.
    IpSocketHandler::<I, _>::send_ip_packet(
        &mut core_ctx.context(),
        bindings_ctx,
        &sock,
        buffer.into_inner().buffer_view().as_ref().into_serializer(),
        &DefaultIpSocketOptions,
        CoreTxMetadata::default(),
    )
    .unwrap();

    assert!(ctx.test_api().handle_queued_rx_packets());

    assert_matches!(ctx.bindings_ctx.take_ethernet_frames()[..], []);

    // Verify the stack-wide IP counters
    assert_eq!(
        IpCounterExpectations::<I> {
            receive_ip_packet: 1,
            dispatch_receive_ip_packet: 1,
            deliver_unicast: 1,
            send_ip_packet: 1,
            ..Default::default()
        },
        CounterContext::<IpCounters<I>>::counters(&ctx.core_ctx()).cast(),
    );
    // Verify the per-device counters for each device.
    assert_eq!(
        IpCounterExpectations::<I> {
            receive_ip_packet: 1,
            dispatch_receive_ip_packet: 1,
            deliver_unicast: 1,
            ..Default::default()
        },
        ctx.core_ctx().per_resource_counters(&device_id).cast()
    );
    assert_eq!(
        IpCounterExpectations::<I> { send_ip_packet: 1, ..Default::default() },
        ctx.core_ctx().per_resource_counters(&loopback_device_id).cast()
    );
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_send<I: IpSocketIpExt + IpExt>() {
    // Test various edge cases of the
    // IpSocketContext::send_ip_packet` method.

    let cfg = I::TEST_ADDRS;
    let proto = I::ICMP_IP_PROTO;
    let socket_options = WithHopLimit(Some(NonZeroU8::new(1).unwrap()));

    let TestAddrs::<_> { local_mac, remote_mac, local_ip, remote_ip, subnet } = cfg;

    let (FakeCtx { core_ctx, mut bindings_ctx }, device_ids) =
        FakeCtxBuilder::with_addrs(cfg).build();
    // Create a normal, routable socket.
    let sock = IpSocketHandler::<I, _>::new_ip_socket(
        &mut core_ctx.context(),
        &mut bindings_ctx,
        IpSocketArgs {
            device: None,
            local_ip: None,
            remote_ip: SocketIpAddr::try_from(remote_ip).unwrap(),
            proto,
            options: &socket_options,
        },
    )
    .unwrap();

    let check_frame = move |frame: &[u8]| match [local_ip.get(), remote_ip.get()].into() {
        IpAddr::V4([local_ip, remote_ip]) => {
            let (mut body, src_mac, dst_mac, _ethertype) =
                parse_ethernet_frame(frame, EthernetFrameLengthCheck::NoCheck).unwrap();
            let packet = (&mut body).parse::<Ipv4Packet<&[u8]>>().unwrap();
            assert_eq!(src_mac, local_mac.get());
            assert_eq!(dst_mac, remote_mac.get());
            assert_eq!(packet.src_ip(), local_ip);
            assert_eq!(packet.dst_ip(), remote_ip);
            assert_eq!(packet.proto(), Ipv4::ICMP_IP_PROTO);
            assert_eq!(packet.ttl(), 1);
        }
        IpAddr::V6([local_ip, remote_ip]) => {
            let (body, src_mac, dst_mac, src_ip, dst_ip, ip_proto, ttl) =
                parse_ip_packet_in_ethernet_frame::<Ipv6>(frame, EthernetFrameLengthCheck::NoCheck)
                    .unwrap();
            assert_eq!(body, [0]);
            assert_eq!(src_mac, local_mac.get());
            assert_eq!(dst_mac, remote_mac.get());
            assert_eq!(src_ip, local_ip);
            assert_eq!(dst_ip, remote_ip);
            assert_eq!(ip_proto, Ipv6::ICMP_IP_PROTO);
            assert_eq!(ttl, 1);
        }
    };
    assert_matches!(bindings_ctx.take_ethernet_frames()[..], []);

    // Send a packet on the socket and make sure that the right contents
    // are sent.
    IpSocketHandler::<I, _>::send_ip_packet(
        &mut core_ctx.context(),
        &mut bindings_ctx,
        &sock,
        (&[0u8][..]).into_serializer(),
        &socket_options,
        CoreTxMetadata::default(),
    )
    .unwrap();
    let check_sent_frame = |bindings_ctx: &mut FakeBindingsCtx| {
        let frames = bindings_ctx.take_ethernet_frames();
        let (dev, frame) = assert_matches!(&frames[..], [frame] => frame);
        assert_eq!(dev, &device_ids[0]);
        check_frame(&frame);
    };
    check_sent_frame(&mut bindings_ctx);

    // Send a packet while imposing an MTU that is large enough to fit the
    // packet.
    let small_body = [0; 1];
    let small_body_serializer = (&small_body).into_serializer();
    let res = IpSocketHandler::<I, _>::send_ip_packet(
        &mut core_ctx.context(),
        &mut bindings_ctx,
        &sock,
        small_body_serializer,
        &RestrictMtu { mtu: Ipv6::MINIMUM_LINK_MTU.into(), inner: socket_options },
        CoreTxMetadata::default(),
    );
    assert_eq!(res, Ok(()));
    check_sent_frame(&mut bindings_ctx);

    // Send a packet on the socket while imposing an MTU which will not
    // allow a packet to be sent.
    // The MTU used here is so small that fragmentation can't even be attempted.
    let res = IpSocketHandler::<I, _>::send_ip_packet(
        &mut core_ctx.context(),
        &mut bindings_ctx,
        &sock,
        small_body_serializer,
        &RestrictMtu { mtu: 1, inner: socket_options },
        CoreTxMetadata::default(),
    );
    assert_eq!(res, Err(IpSockSendError::Mtu));

    assert_matches!(bindings_ctx.take_ethernet_frames()[..], []);

    // Make sure that sending on an unroutable socket fails.
    ip::testutil::del_routes_to_subnet::<I, _, _>(&mut core_ctx.context(), subnet).unwrap();
    let res = IpSocketHandler::<I, _>::send_ip_packet(
        &mut core_ctx.context(),
        &mut bindings_ctx,
        &sock,
        small_body_serializer,
        &socket_options,
        CoreTxMetadata::default(),
    );
    assert_eq!(res, Err(IpSockSendError::Unroutable(ResolveRouteError::Unreachable)));
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn test_send_hop_limits<I: IpSocketIpExt + IpExt>() {
    set_logger_for_test();

    #[derive(Copy, Clone, Debug)]
    struct SetHopLimitFor<A>(SpecifiedAddr<A>);

    const SET_HOP_LIMIT: NonZeroU8 = NonZeroU8::new(42).unwrap();

    impl<A> OptionDelegationMarker for SetHopLimitFor<A> {}

    impl<I: IpExt> DelegatedRouteResolutionOptions<I> for SetHopLimitFor<I::Addr> {}

    impl<I: IpExt> DelegatedSendOptions<I> for SetHopLimitFor<I::Addr> {
        fn hop_limit(&self, destination: &SpecifiedAddr<I::Addr>) -> Option<NonZeroU8> {
            let Self(expected_destination) = self;
            (destination == expected_destination).then_some(SET_HOP_LIMIT)
        }
    }

    let TestAddrs::<I::Addr> { local_ip, remote_ip: _, local_mac, subnet: _, remote_mac: _ } =
        I::TEST_ADDRS;

    let mut builder = FakeCtxBuilder::default();
    let device_idx = builder.add_device(local_mac);
    let (mut ctx, device_ids) = builder.build();
    let device_id: DeviceId<_> = device_ids[device_idx].clone().into();

    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(local_ip.get(), 16).unwrap())
        .unwrap();

    // Use multicast remote addresses since unicast addresses would trigger
    // ARP/NDP requests.
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            I::MULTICAST_SUBNET.into(),
            device_id,
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
        .expect("add device route");
    let remote_ip = I::multicast_addr(0);
    let options = SetHopLimitFor(remote_ip);
    let other_remote_ip = I::multicast_addr(1);

    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    let mut send_to = |destination_ip| {
        let sock = IpSocketHandler::<I, _>::new_ip_socket(
            &mut core_ctx,
            bindings_ctx,
            IpSocketArgs {
                device: None,
                local_ip: None,
                remote_ip: destination_ip,
                proto: I::ICMP_IP_PROTO,
                options: &options,
            },
        )
        .unwrap();

        IpSocketHandler::<I, _>::send_ip_packet(
            &mut core_ctx,
            bindings_ctx,
            &sock,
            (&[0u8][..]).into_serializer(),
            &options,
            CoreTxMetadata::default(),
        )
        .unwrap();
    };

    // Send to two remote addresses: `remote_ip` and `other_remote_ip` and
    // check that the frames were sent with the correct hop limits.
    send_to(SocketIpAddr::try_from(remote_ip).unwrap());
    send_to(SocketIpAddr::try_from(other_remote_ip).unwrap());

    let frames = bindings_ctx.take_ethernet_frames();
    let [df_remote, df_other_remote] = assert_matches!(&frames[..], [df1, df2] => [df1, df2]);
    {
        let (_dev, frame) = df_remote;
        let (_body, _src_mac, _dst_mac, _src_ip, dst_ip, _ip_proto, hop_limit) =
            parse_ip_packet_in_ethernet_frame::<I>(&frame, EthernetFrameLengthCheck::NoCheck)
                .unwrap();
        assert_eq!(dst_ip, remote_ip.get());
        // The `SetHopLimit`-returned value should take precedence.
        assert_eq!(hop_limit, SET_HOP_LIMIT.get());
    }

    {
        let (_dev, frame) = df_other_remote;
        let (_body, _src_mac, _dst_mac, _src_ip, dst_ip, _ip_proto, hop_limit) =
            parse_ip_packet_in_ethernet_frame::<I>(&frame, EthernetFrameLengthCheck::NoCheck)
                .unwrap();
        assert_eq!(dst_ip, other_remote_ip.get());
        // When the options object does not provide a hop limit the default
        // is used.
        assert_eq!(hop_limit, ip::DEFAULT_HOP_LIMITS.unicast.get());
    }
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(true; "remove device")]
#[test_case(false; "dont remove device")]
fn get_mms_device_removed<I: IpSocketIpExt + IpExt>(remove_device: bool) {
    set_logger_for_test();

    let TestAddrs::<I::Addr> { local_ip, remote_ip: _, local_mac, subnet: _, remote_mac: _ } =
        I::TEST_ADDRS;

    let mut builder = FakeCtxBuilder::default();
    let device_idx = builder.add_device(local_mac);
    let (mut ctx, device_ids) = builder.build();
    let eth_device_id = device_ids[device_idx].clone();
    core::mem::drop(device_ids);
    let device_id: DeviceId<_> = eth_device_id.clone().into();

    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet(&device_id, AddrSubnet::new(local_ip.get(), 16).unwrap())
        .unwrap();
    ctx.test_api()
        .add_route(AddableEntryEither::without_gateway(
            I::MULTICAST_SUBNET.into(),
            device_id.clone(),
            AddableMetric::ExplicitMetric(RawMetric(0)),
        ))
        .unwrap();

    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    let ip_sock = IpSocketHandler::<I, _>::new_ip_socket(
        &mut core_ctx,
        bindings_ctx,
        IpSocketArgs {
            device: None,
            local_ip: None,
            remote_ip: SocketIpAddr::try_from(I::multicast_addr(1)).unwrap(),
            proto: I::ICMP_IP_PROTO,
            options: &DefaultIpSocketOptions,
        },
    )
    .unwrap();

    let expected = if remove_device {
        // Clear routes on the device before removing it.
        ctx.test_api().del_device_routes(&device_id);

        // Don't keep any strong device IDs to the device before removing.
        core::mem::drop(device_id);
        ctx.core_api().device::<EthernetLinkDevice>().remove_device(eth_device_id).into_removed();
        Err(MmsError::NoDevice(ResolveRouteError::Unreachable))
    } else {
        Ok(Mms::from_mtu::<I>(
            IpDeviceMtuContext::<I>::get_mtu(&mut ctx.core_ctx(), &device_id),
            0, /* no ip options/ext hdrs used */
        )
        .unwrap())
    };
    let (mut core_ctx, bindings_ctx) = ctx.contexts();
    assert_eq!(
        DeviceIpSocketHandler::get_mms(
            &mut core_ctx,
            bindings_ctx,
            &ip_sock,
            &DefaultIpSocketOptions
        ),
        expected
    );
}
