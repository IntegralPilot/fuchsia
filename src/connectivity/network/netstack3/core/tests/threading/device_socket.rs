// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{loom_model, loom_spawn};
use assert_matches::assert_matches;
use loom::sync::Arc;
use net_declare::net_mac;
use net_types::UnicastAddr;
use net_types::ethernet::Mac;
use netstack3_base::{NetworkParsingContext, NetworkSerializationContext};
use netstack3_core::CtxPair;
use netstack3_core::device::{EthernetLinkDevice, RecvEthernetFrameMeta};
use netstack3_core::device_socket::{Protocol, TargetDevice};
use netstack3_core::sync::Mutex;
use netstack3_core::testutil::{CtxPairExt, FakeCtx, FakeCtxBuilder};
use packet::{Buf, NestablePacketBuilder, Serializer};
use packet_formats::ethernet::{EtherType, EthernetFrameBuilder};
use std::num::NonZeroU16;

#[test]
fn packet_socket_change_device_and_protocol_atomic() {
    const DEVICE_MAC: Mac = net_mac!("22:33:44:55:66:77");
    const SRC_MAC: Mac = net_mac!("88:88:88:88:88:88");

    let make_ethernet_frame = |ethertype| {
        EthernetFrameBuilder::new(SRC_MAC, DEVICE_MAC, ethertype, 0)
            .wrap_body(Buf::new(vec![1; 10], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
    };
    let first_proto: NonZeroU16 = NonZeroU16::new(EtherType::Ipv4.into()).unwrap();
    let second_proto: NonZeroU16 = NonZeroU16::new(EtherType::Ipv6.into()).unwrap();

    loom_model(Default::default(), move || {
        let mut builder = FakeCtxBuilder::default();
        let dev_indexes =
            [(); 2].map(|()| builder.add_device(UnicastAddr::new(DEVICE_MAC).unwrap()));
        let (FakeCtx { core_ctx, bindings_ctx }, indexes_to_device_ids) = builder.build();
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };

        let devs = dev_indexes.map(|i| indexes_to_device_ids[i].clone());
        drop(indexes_to_device_ids);

        let socket = ctx.core_api().device_socket().create(Mutex::new(Vec::new()));
        ctx.core_api().device_socket().set_device_and_protocol(
            &socket,
            TargetDevice::SpecificDevice(&devs[0].clone().into()),
            Protocol::Specific(first_proto),
        );

        let thread_vars = (ctx.clone(), devs.clone());
        let deliver = loom_spawn(move || {
            let (mut ctx, devs) = thread_vars;
            let [dev_a, dev_b] = devs;
            for (device_id, ethertype) in [
                (dev_a.clone(), first_proto.get().into()),
                (dev_a, second_proto.get().into()),
                (dev_b.clone(), first_proto.get().into()),
                (dev_b, second_proto.get().into()),
            ] {
                ctx.core_api().device::<EthernetLinkDevice>().receive_frame(
                    RecvEthernetFrameMeta {
                        device_id,
                        parsing_context: NetworkParsingContext::default(),
                    },
                    make_ethernet_frame(ethertype),
                );
            }
        });

        let thread_vars = (ctx.clone(), devs[1].clone(), socket.clone());

        let change_device = loom_spawn(move || {
            let (mut ctx, dev, socket) = thread_vars;
            ctx.core_api().device_socket().set_device_and_protocol(
                &socket,
                TargetDevice::SpecificDevice(&dev.clone().into()),
                Protocol::Specific(second_proto),
            );
        });

        deliver.join().unwrap();
        change_device.join().unwrap();

        // These are all the matching frames. Depending on how the threads
        // interleave, one or both of them should be delivered.
        let matched_frames = [
            (
                devs[0].downgrade().into(),
                make_ethernet_frame(first_proto.get().into()).into_inner(),
            ),
            (
                devs[1].downgrade().into(),
                make_ethernet_frame(second_proto.get().into()).into_inner(),
            ),
        ];
        let received_frames = socket.socket_state().lock();
        match &received_frames[..] {
            [one] => {
                assert!(one == &matched_frames[0] || one == &matched_frames[1]);
            }
            [one, two] => {
                assert_eq!(one, &matched_frames[0]);
                assert_eq!(two, &matched_frames[1]);
            }
            other => panic!("unexpected received frames {other:?}"),
        }
    });
}

/// Verify that the netstack properly closes a packet socket that races with
/// deferred device removal.
///
/// Regression test for https://fxbug.dev/353484978.
#[test]
fn packet_socket_close_and_remove_device() {
    const DEVICE_MAC: Mac = net_mac!("22:33:44:55:66:77");

    let proto: NonZeroU16 = NonZeroU16::new(EtherType::Ipv4.into()).unwrap();

    loom_model(Default::default(), move || {
        let mut builder = FakeCtxBuilder::default();
        let _dev_index = builder.add_device(UnicastAddr::new(DEVICE_MAC).unwrap());
        let (FakeCtx { core_ctx, bindings_ctx }, devices) = builder.build();
        let device = assert_matches!(&devices[..], [device] => device.clone());
        std::mem::drop(devices);
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };

        let socket = ctx.core_api().device_socket().create(Mutex::new(Vec::new()));
        ctx.core_api().device_socket().set_device_and_protocol(
            &socket,
            TargetDevice::SpecificDevice(&device.clone().into()),
            Protocol::Specific(proto),
        );

        // Keep a strong ID to the device around, which causes device removal
        // to be deferred.
        let _keep_dev_alive = device.clone();

        let thread1_vars = (ctx.clone(), device);
        let remove_device = loom_spawn(move || {
            let (mut ctx, device) = thread1_vars;
            assert_matches!(
                ctx.core_api().device::<EthernetLinkDevice>().remove_device(device),
                netstack3_core::sync::RemoveResourceResult::Deferred(_)
            );
        });

        let thread2_vars = (ctx.clone(), socket);
        let remove_socket = loom_spawn(move || {
            let (mut ctx, socket) = thread2_vars;
            // Device socket removal is deferred because the device state still
            // holds a strong reference to the socket.
            assert_matches!(
                ctx.core_api().device_socket().remove(socket),
                netstack3_core::sync::RemoveResourceResult::Deferred(_)
            );
        });

        remove_device.join().unwrap();
        remove_socket.join().unwrap();
    })
}
