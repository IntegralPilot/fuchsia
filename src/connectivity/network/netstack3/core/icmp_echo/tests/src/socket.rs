// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use core::num::NonZeroU16;

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use net_types::ZonedAddr;
use packet::{Buf, NestablePacketBuilder, Serializer};
use packet_formats::icmp::{IcmpEchoRequest, IcmpPacketBuilder, IcmpZeroCode};
use test_case::test_case;

use netstack3_base::NetworkSerializationContext;
use netstack3_base::testutil::{TestIpExt, set_logger_for_test};
use netstack3_core::IpExt;
use netstack3_core::testutil::{
    CtxPairExt as _, FakeBindingsCtx, FakeCtxBuilder, new_simple_fake_network,
};

const REMOTE_ID: u16 = 1;

enum IcmpConnectionType {
    Local,
    Remote,
}

enum IcmpSendType {
    Send,
    SendTo,
}

// TODO(https://fxbug.dev/42084713): Add test cases with local delivery and a
// bound device once delivery of looped-back packets is corrected in the
// socket map.
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(IcmpConnectionType::Remote, IcmpSendType::Send, true)]
#[test_case(IcmpConnectionType::Remote, IcmpSendType::SendTo, true)]
#[test_case(IcmpConnectionType::Local, IcmpSendType::Send, false)]
#[test_case(IcmpConnectionType::Local, IcmpSendType::SendTo, false)]
#[test_case(IcmpConnectionType::Remote, IcmpSendType::Send, false)]
#[test_case(IcmpConnectionType::Remote, IcmpSendType::SendTo, false)]
fn test_icmp_connection<I: TestIpExt + IpExt>(
    conn_type: IcmpConnectionType,
    send_type: IcmpSendType,
    bind_to_device: bool,
) {
    set_logger_for_test();

    let config = I::TEST_ADDRS;

    const LOCAL_CTX_NAME: &str = "alice";
    const REMOTE_CTX_NAME: &str = "bob";
    let (local, local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let (remote, remote_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS.swap()).build();
    let mut net = new_simple_fake_network(
        LOCAL_CTX_NAME,
        local,
        local_device_ids[0].downgrade(),
        REMOTE_CTX_NAME,
        remote,
        remote_device_ids[0].downgrade(),
    );

    let icmp_id = 13;

    let (remote_addr, ctx_name_receiving_req) = match conn_type {
        IcmpConnectionType::Local => (config.local_ip, LOCAL_CTX_NAME),
        IcmpConnectionType::Remote => (config.remote_ip, REMOTE_CTX_NAME),
    };

    let _loopback_device_id = net.with_context(LOCAL_CTX_NAME, |ctx| ctx.test_api().add_loopback());

    let pb = IcmpPacketBuilder::<I, _>::new(
        *config.local_ip,
        *remote_addr,
        IcmpZeroCode,
        IcmpEchoRequest::new(0, 1),
    );
    let echo_body = vec![1, 2, 3, 4];
    let buf = pb
        .wrap_body(Buf::new(echo_body.clone(), ..))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap()
        .into_inner();
    let conn = net.with_context(LOCAL_CTX_NAME, |ctx| {
        let mut socket_api = ctx.core_api().icmp_echo::<I>();
        let conn = socket_api.create();
        if bind_to_device {
            let device = local_device_ids[0].clone().into();
            socket_api.set_device(&conn, Some(&device)).expect("failed to set SO_BINDTODEVICE");
        }
        core::mem::drop((local_device_ids, remote_device_ids));
        socket_api.bind(&conn, None, NonZeroU16::new(icmp_id)).unwrap();
        match send_type {
            IcmpSendType::Send => {
                socket_api
                    .connect(&conn, Some(ZonedAddr::Unzoned(remote_addr)), REMOTE_ID)
                    .unwrap();
                socket_api.send(&conn, buf, Default::default()).unwrap();
            }
            IcmpSendType::SendTo => {
                socket_api
                    .send_to(&conn, Some(ZonedAddr::Unzoned(remote_addr)), buf, Default::default())
                    .unwrap();
            }
        }
        conn
    });

    net.run_until_idle();

    assert_eq!(
        net.context(LOCAL_CTX_NAME).core_ctx.common_icmp::<I>().rx_counters.echo_reply.get(),
        1
    );
    assert_eq!(
        net.context(ctx_name_receiving_req)
            .core_ctx
            .common_icmp::<I>()
            .rx_counters
            .echo_request
            .get(),
        1
    );
    let replies = net.context(LOCAL_CTX_NAME).bindings_ctx.take_icmp_replies(&conn);
    let expected = IcmpPacketBuilder::<I, _>::new(
        *config.local_ip,
        *remote_addr,
        IcmpZeroCode,
        packet_formats::icmp::IcmpEchoReply::new(icmp_id, 1),
    )
    .wrap_body(Buf::new(echo_body, ..))
    .serialize_vec_outer(&mut NetworkSerializationContext::default())
    .unwrap()
    .into_inner()
    .into_inner();
    assert_matches!(&replies[..], [body] if *body == expected);
}
