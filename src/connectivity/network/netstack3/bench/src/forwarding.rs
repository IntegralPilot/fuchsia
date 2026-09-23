// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! High-level benchmarks.
//!
//! This module contains microbenchmarks for the Netstack3 Core, built on top
//! of Criterion.

use net_types::Witness as _;
use net_types::ip::Ipv4;
use netstack3_base::testutil::{Bencher, TEST_ADDRS_V4};
use netstack3_base::{NetworkParsingContext, NetworkSerializationContext};
use netstack3_core::StackStateBuilder;
use netstack3_core::device::{DeviceId, EthernetLinkDevice, RecvEthernetFrameMeta};
use netstack3_core::testutil::{CtxPairExt as _, FakeCtxBuilder};
use packet::{Buf, InnerPacketBuilder, NestableSerializer as _, Serializer};
use packet_formats::ethernet::testutil::{
    ETHERNET_DST_MAC_BYTE_OFFSET, ETHERNET_HDR_LEN_NO_TAG, ETHERNET_MIN_BODY_LEN_NO_TAG,
    ETHERNET_SRC_MAC_BYTE_OFFSET,
};
use packet_formats::ethernet::{EtherType, EthernetFrameBuilder};
use packet_formats::ip::IpProto;
use packet_formats::ipv4::Ipv4PacketBuilder;
use packet_formats::ipv4::testutil::{IPV4_CHECKSUM_OFFSET, IPV4_MIN_HDR_LEN, IPV4_TTL_OFFSET};

// NOTE: Extra tests that are too expensive to run during benchmarks can be
// added by gating them on the `debug_assertions` configuration option. This
// option is disabled when running `cargo check`, but enabled when running
// `cargo test`.

// Benchmark the minimum possible time to forward an IPv4 packet by stripping
// out all interesting computation. We have the simplest possible setup - a
// forwarding table with a single entry, and a single device - and we receive an
// IPv4 packet frame which we expect will be parsed and forwarded without
// requiring any new buffers to be allocated.
fn bench_forward_minimum<B: Bencher>(b: &mut B, frame_size: usize) {
    let (mut ctx, idx_to_device_id) =
        FakeCtxBuilder::with_addrs(TEST_ADDRS_V4).build_with(StackStateBuilder::default());

    let eth_device = idx_to_device_id[0].clone();
    let device: DeviceId<_> = eth_device.clone().into();
    ctx.test_api().set_unicast_forwarding_enabled::<Ipv4>(&device, true);

    assert!(
        frame_size
            >= ETHERNET_HDR_LEN_NO_TAG
                + core::cmp::max(ETHERNET_MIN_BODY_LEN_NO_TAG, IPV4_MIN_HDR_LEN)
    );
    let body = vec![0; frame_size - (ETHERNET_HDR_LEN_NO_TAG + IPV4_MIN_HDR_LEN)];
    const TTL: u8 = 64;
    let mut buf = body
        .into_serializer()
        .wrap_in(Ipv4PacketBuilder::new(
            // Use the remote IP as the destination so that we decide to
            // forward.
            TEST_ADDRS_V4.remote_ip,
            TEST_ADDRS_V4.remote_ip,
            TTL,
            IpProto::Udp.into(),
        ))
        .wrap_in(EthernetFrameBuilder::new(
            TEST_ADDRS_V4.remote_mac.get(),
            TEST_ADDRS_V4.local_mac.get(),
            EtherType::Ipv4,
            ETHERNET_HDR_LEN_NO_TAG,
        ))
        .serialize_vec_outer(&mut NetworkSerializationContext::default())
        .unwrap();

    let buf = buf.as_mut();
    let range = 0..buf.len();

    // Store a copy of the checksum to re-write it later.
    let ipv4_checksum = [
        buf[ETHERNET_HDR_LEN_NO_TAG + IPV4_CHECKSUM_OFFSET],
        buf[ETHERNET_HDR_LEN_NO_TAG + IPV4_CHECKSUM_OFFSET + 1],
    ];

    b.iter(|| {
        B::black_box(ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
            B::black_box(RecvEthernetFrameMeta {
                device_id: eth_device.clone(),
                parsing_context: NetworkParsingContext::default(),
            }),
            B::black_box(Buf::new(&mut buf[..], range.clone())),
        ));

        #[cfg(test)]
        {
            use std::convert::TryInto as _;

            use packet_formats::ethernet::EthernetFrameLengthCheck;

            let [(device, frame)] = ctx.bindings_ctx.take_ethernet_frames().try_into().unwrap();
            assert_eq!(device, eth_device);
            let (_body, src_mac, dst_mac, src_ip, dst_ip, proto, ttl) =
                packet_formats::testutil::parse_ip_packet_in_ethernet_frame::<Ipv4>(
                    &frame[..],
                    EthernetFrameLengthCheck::NoCheck,
                )
                .expect("parse failed");
            assert_eq!(src_mac, TEST_ADDRS_V4.local_mac.get());
            assert_eq!(dst_mac, TEST_ADDRS_V4.remote_mac.get());
            assert_eq!(src_ip, TEST_ADDRS_V4.remote_ip.get());
            assert_eq!(dst_ip, TEST_ADDRS_V4.remote_ip.get());
            assert_eq!(proto, IpProto::Udp.into());
            assert_eq!(ttl, TTL - 1);
        }

        // Since we modified the buffer in-place, it now has the wrong source
        // and destination MAC addresses and IP TTL/CHECKSUM. We reset them to
        // their original values as efficiently as we can to avoid affecting the
        // results of the benchmark.
        (&mut buf[ETHERNET_SRC_MAC_BYTE_OFFSET..ETHERNET_SRC_MAC_BYTE_OFFSET + 6])
            .copy_from_slice(&TEST_ADDRS_V4.remote_mac.bytes()[..]);
        (&mut buf[ETHERNET_DST_MAC_BYTE_OFFSET..ETHERNET_DST_MAC_BYTE_OFFSET + 6])
            .copy_from_slice(&TEST_ADDRS_V4.local_mac.bytes()[..]);
        let ipv4_buf = &mut buf[ETHERNET_HDR_LEN_NO_TAG..];
        ipv4_buf[IPV4_TTL_OFFSET] = TTL;
        ipv4_buf[IPV4_CHECKSUM_OFFSET..IPV4_CHECKSUM_OFFSET + 2]
            .copy_from_slice(&ipv4_checksum[..]);
    });
}

/// Adds benchmark functions for all Netstack3 Core microbenchmarks.
pub fn add_benches(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    for size in [64, 128, 256, 512, 1024] {
        let _ = group
            .bench_function(format!("ForwardIpv4/{size}"), move |b| bench_forward_minimum(b, size));
    }
}
