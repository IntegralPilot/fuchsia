// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::vec;
use core::num::NonZeroU16;

use ip_test_macro::ip_test;
use net_types::ethernet::Mac;
use net_types::{UnicastAddr, Witness as _, ZonedAddr};
use netstack3_base::WorkQueueReport;
use packet::Buf;
use test_case::test_case;
use test_util::assert_lt;

use netstack3_base::testutil::{FakeSendToken, TestIpExt, set_logger_for_test};
use netstack3_core::IpExt;
use netstack3_core::device::{BatchSize, DeviceId, EthernetLinkDevice};
use netstack3_core::testutil::{CtxPairExt as _, FakeBindingsCtx, FakeCtxBuilder};

const TEST_PORT: NonZeroU16 = NonZeroU16::new(100).unwrap();
const TEST_MESSAGE: &'static [u8] = b"Hello";
const FAKE_MAC: Mac = net_declare::net_mac!("20:00:00:00:00:00");

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_case(true; "connected")]
#[test_case(false; "unconnected")]
fn loopback_holds_metadata<I: IpExt + TestIpExt>(connected: bool) {
    set_logger_for_test();

    let (mut ctx, _local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();

    let _loopback_device_id = ctx.test_api().add_loopback();
    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let sndbuf_before = api.send_buffer_available(&socket);

    let remote = Some(ZonedAddr::Unzoned(I::LOOPBACK_ADDRESS));
    let message = Buf::new(TEST_MESSAGE.to_vec(), ..);
    if connected {
        api.connect(&socket, remote, TEST_PORT.into()).unwrap();
        api.send(&socket, message, FakeSendToken::default()).unwrap();
    } else {
        api.send_to(&socket, remote, TEST_PORT.into(), message, FakeSendToken::default()).unwrap();
    }

    // send buffer utilization is held over loopback.
    assert_lt!(api.send_buffer_available(&socket), sndbuf_before);
    assert!(ctx.test_api().handle_queued_rx_packets());
    // After handling the queued packets the send buffer is released.
    assert_eq!(ctx.core_api().udp::<I>().send_buffer_available(&socket), sndbuf_before);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn neighbor_resolution_holds_metadata<I: IpExt + TestIpExt>() {
    set_logger_for_test();
    let (mut ctx, local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let eth_device = &local_device_ids[0];

    // Pick a remote address that is not statically added in FakeCtxBuilder.
    let remote_addr = I::get_other_ip_address(10);

    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let sndbuf_before = api.send_buffer_available(&socket);
    api.send_to(
        &socket,
        Some(ZonedAddr::Unzoned(remote_addr)),
        TEST_PORT.into(),
        Buf::new(TEST_MESSAGE.to_vec(), ..),
        FakeSendToken::default(),
    )
    .unwrap();

    // send buffer utilization is held over neighbor resolution.
    assert_lt!(api.send_buffer_available(&socket), sndbuf_before);

    // Mark the neighbor as static.
    ctx.core_api()
        .neighbor::<I, EthernetLinkDevice>()
        .insert_static_entry(eth_device, remote_addr.get(), UnicastAddr::new(FAKE_MAC).unwrap())
        .unwrap();

    // After resolving the neighbor the send buffer is released.
    assert_eq!(ctx.core_api().udp::<I>().send_buffer_available(&socket), sndbuf_before);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn holds_in_tx_queue<I: IpExt + TestIpExt>() {
    set_logger_for_test();
    let (mut ctx, local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let eth_device = &local_device_ids[0];

    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let sndbuf_before = api.send_buffer_available(&socket);

    // Initially the device doesn't have tx queue set up, so send buffer is
    // immediately released when it makes it to the device layer.
    api.send_to(
        &socket,
        Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
        TEST_PORT.into(),
        Buf::new(TEST_MESSAGE.to_vec(), ..),
        FakeSendToken::default(),
    )
    .unwrap();
    assert_eq!(api.send_buffer_available(&socket), sndbuf_before);

    ctx.core_api()
        .transmit_queue::<EthernetLinkDevice>()
        .set_configuration(&eth_device, netstack3_core::device::TransmitQueueConfiguration::Fifo);

    let mut api = ctx.core_api().udp::<I>();
    // Send again, things should be held up.
    api.send_to(
        &socket,
        Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
        TEST_PORT.into(),
        Buf::new(TEST_MESSAGE.to_vec(), ..),
        FakeSendToken::default(),
    )
    .unwrap();
    assert_lt!(api.send_buffer_available(&socket), sndbuf_before);

    // Clear the tx available signal.
    let tx_avail = core::mem::take(&mut ctx.bindings_ctx.state_mut().tx_available);
    assert_eq!(tx_avail, vec![DeviceId::from(eth_device.clone())]);
    // Releases after hitting the transmit queue.
    assert_eq!(
        ctx.core_api().transmit_queue::<EthernetLinkDevice>().transmit_queued_frames(
            &eth_device,
            BatchSize::new_saturating(BatchSize::MAX),
            &mut (),
        ),
        Ok(WorkQueueReport::AllDone),
    );
    assert_eq!(ctx.core_api().udp::<I>().send_buffer_available(&socket), sndbuf_before);
}
