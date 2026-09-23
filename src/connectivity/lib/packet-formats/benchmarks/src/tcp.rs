// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;
use std::num::NonZeroU16;

use itertools::Itertools as _;
use net_types::ip::IpVersion;
use packet::{
    Buf, BufferAlloc, InnerPacketBuilder as _, NestablePacketBuilder as _, ParseBuffer,
    ReusableBuffer,
};
use packet_formats::ethernet::{EthernetFrame, EthernetFrameLengthCheck};
use packet_formats::ip::{IpPacket, IpProto};
use packet_formats::tcp::options::{TcpOptions as _, TcpOptionsBuilder, TimestampOption};
use packet_formats::tcp::{
    TcpParseArgs, TcpSegment, TcpSegmentBuilder, TcpSegmentBuilderWithOptions,
};

use crate::ip::{ExtractedIpInfo, IpBenchmarkConfig, IpExt};
use crate::{Bencher, BenchmarkGroup, BufSliceAlloc};

const SRC_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
const DST_PORT: NonZeroU16 = NonZeroU16::new(4321).unwrap();
const SEQ_NUM: u32 = 1313;
const ACK_NUM: u32 = 2020;
const WINDOW_SIZE: u16 = 65000;

const TSVAL: u32 = 0xA0A0A0A0;
const TSECHO: u32 = 0xB0B0B0B0;
const TIMESTAMP: TimestampOption = TimestampOption::new(TSVAL, TSECHO);

fn make_tcp_segment<I: IpExt, B: ReusableBuffer, A: BufferAlloc<B, Error: Debug>>(
    alloc: A,
    options: &TcpBenchmarkConfig,
    payload: &[u8],
) -> B {
    let TcpBenchmarkConfig { ip, tcp_options, payload_size: _ } = options;
    let seg = TcpSegmentBuilder::new(
        I::SRC_ADDR,
        I::DST_ADDR,
        SRC_PORT,
        DST_PORT,
        SEQ_NUM,
        Some(ACK_NUM),
        WINDOW_SIZE,
    );
    if *tcp_options {
        let options = TcpOptionsBuilder { timestamp: Some(TIMESTAMP), ..Default::default() };
        I::make_packet(
            alloc,
            ip,
            IpProto::Tcp,
            TcpSegmentBuilderWithOptions::new(seg, options)
                .unwrap()
                .wrap_body(payload.into_serializer()),
        )
    } else {
        I::make_packet(alloc, ip, IpProto::Tcp, seg.wrap_body(payload.into_serializer()))
    }
}

#[derive(Debug, Copy, Clone)]
struct TcpBenchmarkConfig {
    ip: IpBenchmarkConfig,
    tcp_options: bool,
    payload_size: usize,
}

impl TcpBenchmarkConfig {
    fn combinations() -> impl Iterator<Item = Self> + Clone {
        IpBenchmarkConfig::combinations()
            .cartesian_product([true, false])
            .cartesian_product([0, 1, 5, 25])
            .map(|((ip, tcp_options), payload_size)| Self {
                ip,
                tcp_options,
                payload_size: payload_size << 10,
            })
    }
}

#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(not(test), allow(unused))]
struct ExtractedTcpInfo<I: IpExt> {
    ip_info: ExtractedIpInfo<I>,
    src_port: NonZeroU16,
    dst_port: NonZeroU16,
    seq_num: u32,
    ack_num: Option<u32>,
    window_size: u16,
    timestamp: Option<TimestampOption>,
    payload_size: usize,
}

impl<I: IpExt> ExtractedTcpInfo<I> {
    /// Returns the expected value with the same options given to
    /// [`make_tcp_segment`].
    #[cfg(test)]
    fn expected(options: &TcpBenchmarkConfig) -> Self {
        let TcpBenchmarkConfig { ip, tcp_options, payload_size } = options;
        Self {
            ip_info: ExtractedIpInfo::expected(ip, IpProto::Tcp.into()),
            src_port: SRC_PORT,
            dst_port: DST_PORT,
            seq_num: SEQ_NUM,
            ack_num: Some(ACK_NUM),
            window_size: WINDOW_SIZE,
            timestamp: tcp_options.then_some(TIMESTAMP),
            payload_size: *payload_size,
        }
    }
}

fn bench_parse<I: IpExt, B: Bencher>(bencher: &mut B, options: &TcpBenchmarkConfig) {
    let payload = (0..options.payload_size).into_iter().map(|x| x as u8).collect::<Vec<_>>();
    let seg = make_tcp_segment::<I, _, _>(packet::serialize::new_buf_vec, options, &payload[..])
        .into_inner();
    bencher.iter(|| {
        let mut buffer = Buf::new(&seg[..], ..);
        if options.ip.ethernet {
            // Don't do anything with the Ethernet header, we only have variants
            // for it to catch alignment variations.
            let _ = buffer
                .parse_with::<_, EthernetFrame<&[u8]>>(EthernetFrameLengthCheck::NoCheck)
                .unwrap();
        }
        let packet = buffer.parse::<I::Packet<&[u8]>>().unwrap();
        let ip_info = I::extract_info(&packet);
        let args = TcpParseArgs::new(packet.src_ip(), packet.dst_ip());
        drop(packet);
        let packet = buffer.parse_with::<_, TcpSegment<&[u8]>>(args).unwrap();
        let tcp_info = ExtractedTcpInfo {
            ip_info,
            src_port: packet.src_port(),
            dst_port: packet.dst_port(),
            seq_num: packet.seq_num(),
            ack_num: packet.ack_num(),
            window_size: packet.window_size(),
            timestamp: packet.options().timestamp().copied(),
            payload_size: packet.body().len(),
        };

        #[cfg(test)]
        assert_eq!(tcp_info, ExtractedTcpInfo::expected(&options));

        tcp_info
    });
}

fn bench_serialize<I: IpExt, B: Bencher>(bencher: &mut B, options: &TcpBenchmarkConfig) {
    // Prepare a serialization that has the right size.
    let payload = (0..options.payload_size).into_iter().map(|x| x as u8).collect::<Vec<_>>();
    let mut segment =
        make_tcp_segment::<I, _, _>(packet::new_buf_vec, options, &payload[..]).into_inner();
    let segment = &mut segment[..];
    bencher.iter(|| {
        // Given the parse benchmark is using the same function to verify
        // output, we don't need to verify here.
        let _: Buf<&mut [u8]> =
            make_tcp_segment::<I, _, _>(BufSliceAlloc(segment), options, &payload[..]);
    });
}

pub(crate) fn get_benches<G: BenchmarkGroup>(group: &mut G) {
    let iter = [IpVersion::V4, IpVersion::V6]
        .into_iter()
        .cartesian_product(TcpBenchmarkConfig::combinations());
    for (ip_version, tcp) in iter {
        let TcpBenchmarkConfig { ip, tcp_options, payload_size } = &tcp;
        let name = format!(
            "{}/TCP{}/{}KiB",
            ip.bench_name_particle(ip_version),
            if *tcp_options { "-options" } else { "" },
            *payload_size >> 10
        );

        group.register(format!("parse/{name}"), move |bencher| {
            net_types::for_any_ip_version!(ip_version, I, bench_parse::<I, _>(bencher, &tcp));
        });
        group.register(format!("serialize/{name}"), move |bencher| {
            net_types::for_any_ip_version!(ip_version, I, bench_serialize::<I, _>(bencher, &tcp));
        });
    }
}

#[cfg(test)]
mod tests {}
