// Copyright 2026 The Fuchsia Authors. All rights reserved.
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

use netstack3_base::testutil::{FakeSendTokenTracker, TestIpExt, set_logger_for_test};
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
fn send_token_held_over_loopback<I: IpExt + TestIpExt>(connected: bool) {
    set_logger_for_test();

    let (mut ctx, _local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();

    let _loopback_device_id = ctx.test_api().add_loopback();
    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let tracker = FakeSendTokenTracker::default();

    let remote = Some(ZonedAddr::Unzoned(I::LOOPBACK_ADDRESS));
    let message = Buf::new(TEST_MESSAGE.to_vec(), ..);
    if connected {
        api.connect(&socket, remote, TEST_PORT.into()).unwrap();
        api.send(&socket, message, tracker.token()).unwrap();
    } else {
        api.send_to(&socket, remote, TEST_PORT.into(), message, tracker.token()).unwrap();
    }

    // Token is held over loopback.
    assert_eq!(tracker.live_tokens(), 1);
    assert!(ctx.test_api().handle_queued_rx_packets());
    // After handling the queued packets the token is released.
    assert_eq!(tracker.live_tokens(), 0);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn send_token_held_during_neighbor_resolution<I: IpExt + TestIpExt>() {
    set_logger_for_test();
    let (mut ctx, local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let eth_device = &local_device_ids[0];

    // Pick a remote address that is not statically added in FakeCtxBuilder.
    let remote_addr = I::get_other_ip_address(10);

    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let tracker = FakeSendTokenTracker::default();
    api.send_to(
        &socket,
        Some(ZonedAddr::Unzoned(remote_addr)),
        TEST_PORT.into(),
        Buf::new(TEST_MESSAGE.to_vec(), ..),
        tracker.token(),
    )
    .unwrap();

    // Token is held over neighbor resolution.
    assert_eq!(tracker.live_tokens(), 1);

    // Mark the neighbor as static.
    ctx.core_api()
        .neighbor::<I, EthernetLinkDevice>()
        .insert_static_entry(eth_device, remote_addr.get(), UnicastAddr::new(FAKE_MAC).unwrap())
        .unwrap();

    // After resolving the neighbor the token is released.
    assert_eq!(tracker.live_tokens(), 0);
}

#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn send_token_held_in_tx_queue<I: IpExt + TestIpExt>() {
    set_logger_for_test();
    let (mut ctx, local_device_ids) = FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build();
    let eth_device = &local_device_ids[0];

    let mut api = ctx.core_api().udp::<I>();
    let socket = api.create();
    let tracker = FakeSendTokenTracker::default();

    // Initially the device doesn't have tx queue set up, so send token is
    // immediately released when it makes it to the device layer.
    api.send_to(
        &socket,
        Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
        TEST_PORT.into(),
        Buf::new(TEST_MESSAGE.to_vec(), ..),
        tracker.token(),
    )
    .unwrap();
    assert_eq!(tracker.live_tokens(), 0);

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
        tracker.token(),
    )
    .unwrap();
    assert_eq!(tracker.live_tokens(), 1);

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
    assert_eq!(tracker.live_tokens(), 0);
}
