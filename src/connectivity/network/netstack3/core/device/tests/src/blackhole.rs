// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use ip_test_macro::ip_test;
use net_types::ip::{AddrSubnet, IpAddress as _};
use net_types::{Witness, ZonedAddr};
use netstack3_base::testutil::TestIpExt;
use netstack3_core::IpExt;
use netstack3_core::routes::{AddableEntry, AddableMetric, RawMetric};
use netstack3_core::sync::RemoveResourceResult;
use netstack3_core::testutil::{
    CtxPairExt as _, DEFAULT_INTERFACE_METRIC, FakeBindingsCtx, FakeCtx,
};
use netstack3_core::udp::UdpRemotePort;
use netstack3_device::blackhole::BlackholeDevice;

// Smoke test verifying [`BlackholeDevice`] implements the traits required to
// satisfy the [`DeviceApi`].
#[test]
fn add_remove_blackhole_device() {
    let mut ctx = FakeCtx::default();
    let mut device_api = ctx.core_api().device::<BlackholeDevice>();
    let device = device_api.add_device_with_default_state((), DEFAULT_INTERFACE_METRIC);
    assert_matches!(device_api.remove_device(device), RemoveResourceResult::Removed(_));
}

// Verify that a socket can listen on an IP address that is assigned to a blackhole device, and send
// on that device. Note that this will never receive any traffic, since the device drops all
// packets.
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn available_to_socket_layer<I: TestIpExt + IpExt>() {
    let mut ctx = FakeCtx::default();
    let device = ctx
        .core_api()
        .device::<BlackholeDevice>()
        .add_device_with_default_state((), DEFAULT_INTERFACE_METRIC)
        .into();
    ctx.test_api().enable_device(&device);

    let prefix = I::Addr::BYTES * 8;
    let addr = AddrSubnet::new(I::TEST_ADDRS.local_ip.get(), prefix).unwrap();
    ctx.core_api()
        .device_ip::<I>()
        .add_ip_addr_subnet(&device, addr)
        .expect("add address should succeed");

    let socket = ctx.core_api().udp::<I>().create();
    ctx.core_api()
        .udp::<I>()
        .listen(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.local_ip)), None)
        .expect("listen should succeed");

    ctx.test_api()
        .add_route(
            AddableEntry::without_gateway(
                I::TEST_ADDRS.subnet,
                device,
                AddableMetric::ExplicitMetric(RawMetric(0)),
            )
            .into(),
        )
        .expect("add route through blackhole device");

    let buf = packet::Buf::new(b"hello".to_vec(), ..);
    ctx.core_api()
        .udp::<I>()
        .send_to(
            &socket,
            Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)),
            UdpRemotePort::from(8080),
            buf,
            Default::default(),
        )
        .expect("send should succeed");
}
