// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/339502691): Return to the default limit once lock
// ordering no longer causes overflows.
#![recursion_limit = "256"]

use std::num::NonZeroU16;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use loom::sync::Arc;
use net_declare::{net_ip_v4, net_ip_v6, net_mac, net_subnet_v4, net_subnet_v6};
use net_types::ethernet::Mac;
use net_types::ip::{Ipv4, Ipv6, Subnet};
use net_types::{SpecifiedAddr, UnicastAddr, Witness as _, ZonedAddr};
use netstack3_base::{NetworkParsingContext, NetworkSerializationContext};
use netstack3_core::CtxPair;
use netstack3_core::device::{EthernetLinkDevice, RecvEthernetFrameMeta};
use netstack3_core::routes::{AddableEntry, AddableMetric, RawMetric};
use netstack3_core::testutil::{CtxPairExt as _, FakeBindingsCtx, FakeCtx, FakeCtxBuilder};
use netstack3_ip::icmp::testutil::{
    neighbor_advertisement_ip_packet, neighbor_solicitation_ip_packet,
};
use packet::{
    Buf, InnerPacketBuilder as _, NestableSerializer as _, ParseBuffer as _, Serializer as _,
};
use packet_formats::arp::{ArpOp, ArpPacketBuilder};
use packet_formats::ethernet::{
    ETHERNET_MIN_BODY_LEN_NO_TAG, EtherType, EthernetFrameBuilder, EthernetFrameLengthCheck,
};
use packet_formats::ip::IpProto;
use packet_formats::testutil::parse_ip_packet_in_ethernet_frame;
use packet_formats::udp::{UdpPacket, UdpParseArgs};

mod device_socket;
mod sndbuf;
mod tcp;

/// Spawns a loom thread with a safe stack size.
#[track_caller]
fn loom_spawn<F, T>(f: F) -> loom::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // Picked to allow all the tests in this file to run safely. We've had
    // problems in the past with coverage builders using too much stack.
    const THREAD_STACK_SIZE: usize = 0x10000;
    loom::thread::Builder::new().stack_size(THREAD_STACK_SIZE).spawn(f).unwrap()
}

/// A wrapper around loom's base modelling calls.
///
/// We use this function directly because it allows us to specify the stack size
/// of the "main" thread that we're running on by using `loom_spawn`. Otherwise,
/// loom uses a default value that causes segfaults in some of our tests.
///
/// TODO(https://github.com/tokio-rs/loom/issues/345): Remove this when we can
/// just set our stack size from the model builder.
fn loom_model<F>(model: loom::model::Builder, f: F)
where
    F: Fn() + Copy + Sync + Send + 'static,
{
    model.check(move || loom_spawn(f).join().unwrap())
}

// Per the loom docs [1], it can take a significant amount of time to
// exhaustively check complex models. Rather than running a completely
// exhaustive check, you can configure loom to skip executions that it deems
// unlikely to catch more issues, by setting a "thread pre-emption bound".
// In practice, this bound can be set quite low (2 or 3) while still
// allowing loom to catch most bugs.
//
// When writing this regression test, we verified that it reproduces the
// race condition even with a pre-emption bound of 3. On the other hand,
// when the pre-emption bound is left unset, the IPv6 variant of this test
// takes over 2 minutes to run when built in release mode. So we set this
// pre-emption bound to keep the runtime within a reasonable limit while
// still catching most possible race conditions.
//
// [1]: https://docs.rs/loom/0.6.0/loom/index.html#large-models
fn low_preemption_bound_model() -> loom::model::Builder {
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(3);
    model
}

const DEVICE_MAC: Mac = net_mac!("22:33:44:55:66:77");
const NEIGHBOR_MAC: Mac = net_mac!("88:88:88:88:88:88");

trait TestIpExt: netstack3_core::IpExt {
    const DEVICE_ADDR: Self::Addr;
    const DEVICE_SUBNET: Subnet<Self::Addr>;
    const DEVICE_GATEWAY: Subnet<Self::Addr>;
    const NEIGHBOR_ADDR: Self::Addr;

    fn make_neighbor_solicitation() -> Buf<Vec<u8>>;
    fn make_neighbor_confirmation() -> Buf<Vec<u8>>;
}

impl TestIpExt for Ipv4 {
    const DEVICE_ADDR: Self::Addr = net_ip_v4!("192.0.2.0");
    const DEVICE_SUBNET: Subnet<Self::Addr> = net_subnet_v4!("192.0.2.0/32");
    const DEVICE_GATEWAY: Subnet<Self::Addr> = net_subnet_v4!("192.0.2.0/24");
    const NEIGHBOR_ADDR: Self::Addr = net_ip_v4!("192.0.2.1");

    fn make_neighbor_solicitation() -> Buf<Vec<u8>> {
        ArpPacketBuilder::new(
            ArpOp::Request,
            DEVICE_MAC,
            Self::DEVICE_ADDR,
            Mac::UNSPECIFIED,
            Self::NEIGHBOR_ADDR,
        )
        .into_serializer()
        .wrap_in(EthernetFrameBuilder::new(DEVICE_MAC, Mac::BROADCAST, EtherType::Arp, 0))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }

    fn make_neighbor_confirmation() -> Buf<Vec<u8>> {
        ArpPacketBuilder::new(
            ArpOp::Response,
            NEIGHBOR_MAC,
            Self::NEIGHBOR_ADDR,
            DEVICE_MAC,
            Self::DEVICE_ADDR,
        )
        .into_serializer()
        .wrap_in(EthernetFrameBuilder::new(
            NEIGHBOR_MAC,
            DEVICE_MAC,
            EtherType::Arp,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }
}

impl TestIpExt for Ipv6 {
    const DEVICE_ADDR: Self::Addr = net_ip_v6!("2001:db8::1");
    const DEVICE_SUBNET: Subnet<Self::Addr> = net_subnet_v6!("2001:db8::1/128");
    const DEVICE_GATEWAY: Subnet<Self::Addr> = net_subnet_v6!("2001:db8::/64");
    const NEIGHBOR_ADDR: Self::Addr = net_ip_v6!("2001:db8::2");

    fn make_neighbor_solicitation() -> Buf<Vec<u8>> {
        let snmc = Self::NEIGHBOR_ADDR.to_solicited_node_address();
        neighbor_solicitation_ip_packet(
            Self::DEVICE_ADDR,
            snmc.get(),
            Self::NEIGHBOR_ADDR,
            DEVICE_MAC,
        )
        .wrap_in(EthernetFrameBuilder::new(
            DEVICE_MAC,
            Mac::from(&snmc),
            EtherType::Ipv6,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }

    fn make_neighbor_confirmation() -> Buf<Vec<u8>> {
        neighbor_advertisement_ip_packet(
            Self::NEIGHBOR_ADDR,
            Self::DEVICE_ADDR,
            false, /* router_flag */
            true,  /* solicited_flag */
            false, /* override_flag */
            NEIGHBOR_MAC,
        )
        .wrap_in(EthernetFrameBuilder::new(
            NEIGHBOR_MAC,
            DEVICE_MAC,
            EtherType::Ipv6,
            ETHERNET_MIN_BODY_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .unwrap_b()
    }
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn neighbor_resolution_and_send_queued_packets_atomic<I: TestIpExt>() {
    loom_model(low_preemption_bound_model(), move || {
        let mut builder = FakeCtxBuilder::default();
        let dev_index = builder.add_device_with_ip(
            UnicastAddr::new(DEVICE_MAC).unwrap(),
            I::DEVICE_ADDR,
            I::DEVICE_SUBNET,
        );
        let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
        let device = indexes_to_device_ids.into_iter().nth(dev_index).unwrap();
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };

        ctx.test_api()
            .add_route(
                AddableEntry::with_gateway(
                    I::DEVICE_GATEWAY,
                    device.clone().into(),
                    SpecifiedAddr::new(I::NEIGHBOR_ADDR).unwrap(),
                    AddableMetric::ExplicitMetric(RawMetric(0)),
                )
                .into(),
            )
            .unwrap();

        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
        const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(33333).unwrap();

        // Bind a UDP socket to the device we added so we can trigger link
        // resolution by sending IP packets.
        let mut udp_api = ctx.core_api().udp::<I>();
        let socket = udp_api.create();
        udp_api
            .listen(
                &socket,
                SpecifiedAddr::new(I::DEVICE_ADDR).map(|a| ZonedAddr::Unzoned(a).into()),
                Some(LOCAL_PORT),
            )
            .unwrap();
        udp_api.set_device(&socket, Some(&device.clone().into())).unwrap();

        // Trigger creation of an INCOMPLETE neighbor entry by queueing an
        // outgoing packet to that neighbor's IP address.
        udp_api
            .send_to(
                &socket,
                Some(ZonedAddr::Unzoned(SpecifiedAddr::new(I::NEIGHBOR_ADDR).unwrap()).into()),
                REMOTE_PORT.into(),
                Buf::new([1], ..),
                Default::default(),
            )
            .unwrap();

        // Expect the netstack to send a neighbor probe to resolve the link
        // address of the neighbor.
        let frames = ctx.bindings_ctx.take_ethernet_frames();
        let (sent_device, frame) = assert_matches!(&frames[..], [frame] => frame);
        assert_eq!(sent_device, &device.downgrade());
        assert_eq!(frame, &I::make_neighbor_solicitation().into_inner());

        // Race the following:
        //  - Receipt of a neighbor confirmation for the INCOMPLETE neighbor
        //    entry, which causes it to move into COMPLETE.
        //  - Queueing of another packet to that neighbor.

        let thread_vars = (ctx.clone(), device.clone());
        let resolve_neighbor = loom_spawn(move || {
            let (mut ctx, device_id) = thread_vars;
            ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
                RecvEthernetFrameMeta {
                    device_id,
                    parsing_context: NetworkParsingContext::default(),
                },
                I::make_neighbor_confirmation(),
            );
        });

        let thread_vars = (ctx.clone(), socket.clone());
        let queue_packet = loom_spawn(move || {
            let (mut ctx, socket) = thread_vars;
            ctx.core_api()
                .udp()
                .send_to(
                    &socket,
                    Some(ZonedAddr::Unzoned(SpecifiedAddr::new(I::NEIGHBOR_ADDR).unwrap()).into()),
                    REMOTE_PORT.into(),
                    Buf::new([2], ..),
                    Default::default(),
                )
                .unwrap();
        });

        resolve_neighbor.join().unwrap();
        queue_packet.join().unwrap();

        for (i, (sent_device, frame)) in
            ctx.bindings_ctx.take_ethernet_frames().into_iter().enumerate()
        {
            assert_eq!(device, sent_device);

            let (mut body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
                parse_ip_packet_in_ethernet_frame::<I>(&frame, EthernetFrameLengthCheck::NoCheck)
                    .unwrap();
            assert_eq!(src_mac, DEVICE_MAC);
            assert_eq!(dst_mac, NEIGHBOR_MAC);
            assert_eq!(src_ip, I::DEVICE_ADDR);
            assert_eq!(dst_ip, I::NEIGHBOR_ADDR);
            assert_eq!(proto, IpProto::Udp.into());
            assert_eq!(ttl, 64);

            let udp_packet =
                body.parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(src_ip, dst_ip)).unwrap();
            let body = udp_packet.body();
            let expected_payload = i as u8 + 1;
            assert_eq!(body, [expected_payload], "frame was sent out of order!");
        }

        // Remove the device so that existing NUD timers get cleaned up;
        // otherwise, they would hold dangling references to the device when the
        // core context is dropped at the end of the test.
        ctx.test_api().clear_routes_and_remove_device(device);
    })
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn new_incomplete_neighbor_schedule_timer_atomic<I: TestIpExt>() {
    loom_model(low_preemption_bound_model(), move || {
        let mut builder = FakeCtxBuilder::default();
        let dev_index = builder.add_device_with_ip(
            UnicastAddr::new(DEVICE_MAC).unwrap(),
            I::DEVICE_ADDR,
            I::DEVICE_SUBNET,
        );
        let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
        let device = indexes_to_device_ids.into_iter().nth(dev_index).unwrap();

        ctx.test_api()
            .add_route(
                AddableEntry::with_gateway(
                    I::DEVICE_GATEWAY,
                    device.clone().into(),
                    SpecifiedAddr::new(I::NEIGHBOR_ADDR).unwrap(),
                    AddableMetric::ExplicitMetric(RawMetric(0)),
                )
                .into(),
            )
            .unwrap();

        const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
        const REMOTE_PORT: NonZeroU16 = NonZeroU16::new(33333).unwrap();

        // Bind a UDP socket to the device we added so we can trigger link
        // resolution by sending IP packets.
        let mut udp_api = ctx.core_api().udp::<I>();
        let socket = udp_api.create();
        udp_api
            .listen(
                &socket,
                SpecifiedAddr::new(I::DEVICE_ADDR).map(|a| ZonedAddr::Unzoned(a).into()),
                Some(LOCAL_PORT),
            )
            .unwrap();
        udp_api.set_device(&socket, Some(&device.clone().into())).unwrap();

        // Race the following:
        //  - Creation of an INCOMPLETE neighbor entry, caused by queueing an
        //    outgoing packet to that neighbor's IP address.
        //  - Adding a static entry for that neighbor, which will cancel the
        //    entry's existing timer.
        //
        // If the entry is added as INCOMPLETE, but its timer is not scheduled
        // atomically with entry insertion, the subsequent timer cancelation
        // will cause a panic.

        let thread_vars = (ctx.clone(), socket.clone());
        let create_incomplete_neighbor = loom_spawn(move || {
            let (mut ctx, socket) = thread_vars;
            ctx.core_api()
                .udp()
                .send_to(
                    &socket,
                    Some(ZonedAddr::Unzoned(SpecifiedAddr::new(I::NEIGHBOR_ADDR).unwrap()).into()),
                    REMOTE_PORT.into(),
                    Buf::new([1], ..),
                    Default::default(),
                )
                .unwrap()
        });

        let thread_vars = (ctx.clone(), device.clone());
        let set_static_neighbor = loom_spawn(move || {
            let (mut ctx, device) = thread_vars;
            ctx.core_api()
                .neighbor::<I, EthernetLinkDevice>()
                .insert_static_entry(
                    &device,
                    I::NEIGHBOR_ADDR,
                    UnicastAddr::new(NEIGHBOR_MAC).unwrap(),
                )
                .unwrap();
        });

        create_incomplete_neighbor.join().unwrap();
        set_static_neighbor.join().unwrap();

        // Remove the device so that existing references to it get cleaned up
        // before the core context is dropped at the end of the test.
        ctx.test_api().clear_routes_and_remove_device(device);
    })
}
