// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{connect, Overnet};

use anyhow::{format_err, Error};
use fuchsia_async::OnSignalsRef;
use futures::prelude::*;
use overnet_core::NodeIdGenerator;
use std::pin::pin;

#[fuchsia::test]
async fn error_propagation(run: usize) {
    let mut node_id_gen = NodeIdGenerator::new("error_propagation", run);
    let a = Overnet::new(&mut node_id_gen).unwrap();
    let b = Overnet::new(&mut node_id_gen).unwrap();

    connect(&a, &b).unwrap();
    let (wait_sender, waiter) = futures::channel::oneshot::channel();
    futures::future::try_join(
        async move {
            let (sender, mut receiver) = futures::channel::mpsc::unbounded();
            b.register_service("test".to_owned(), move |chan| {
                let _ = sender.unbounded_send(chan);
                Ok::<(), Error>(())
            })?;
            let chan =
                receiver.next().await.ok_or_else(|| format_err!("No test request received"))?;
            let chan = fidl::AsyncChannel::from_channel(chan);
            log::info!(node_id = b.node_id().0; "CLIENT CONNECTED TO SERVER");
            assert_eq!(
                chan.recv_msg(&mut Default::default()).await,
                Err(fidl::Status::PEER_CLOSED)
            );
            let reason = chan.closed_reason().unwrap();
            assert_eq!("Proxy shut down (result: Ok(()))", &reason);
            let _ = wait_sender.send(());
            Ok::<(), Error>(())
        },
        async move {
            let chan = {
                let (peer_sender, mut peer_receiver) = futures::channel::mpsc::channel(0);
                a.list_peers(peer_sender)?;
                'retry: loop {
                    let peers =
                        peer_receiver.next().await.ok_or_else(|| format_err!("Lost connection"))?;
                    for peer in peers {
                        if peer.services.iter().find(|name| *name == "test").is_none() {
                            continue;
                        }
                        let (s, p) = fidl::Channel::create();
                        a.connect_to_service(peer.node_id, "test".to_owned(), s)?;
                        break 'retry p;
                    }
                }
            };
            drop(chan);
            let _ = waiter.await;
            Ok::<(), Error>(())
        },
    )
    .map_ok(drop)
    .await
    .unwrap();
}

#[fuchsia::test]
async fn error_propagation_link_fail(run: usize) {
    let mut node_id_gen = NodeIdGenerator::new("error_propagation", run);
    let a = Overnet::new(&mut node_id_gen).unwrap();
    let b = Overnet::new(&mut node_id_gen).unwrap();
    let (a_reads_middle, middle_writes_a) = circuit::stream::stream();
    let (middle_reads_b, b_writes_middle) = circuit::stream::stream();
    let (b_reads_a, a_writes_b) = circuit::stream::stream();
    a.attach_circuit_link(a_reads_middle, a_writes_b, true).unwrap();
    b.attach_circuit_link(b_reads_a, b_writes_middle, false).unwrap();

    let (crash_sender, mut crash_waiter) = futures::channel::oneshot::channel();
    futures::future::try_join3(
        async move {
            let mut buf = [0u8; 4096];
            loop {
                let size = {
                    match futures::future::select(pin!(middle_reads_b.read(1, |data| {
                        let read = std::cmp::min(data.len(), buf.len());
                        buf[..read].copy_from_slice(&data[..read]);
                        Ok((read, read))
                    })), pin!(&mut crash_waiter)).await {
                        future::Either::Left((size, _)) => Some(size.unwrap()),
                        future::Either::Right(_) => None,
                    }
                };

                if let Some(size) = size {
                    middle_writes_a.write(size, |out| {
                        out[..size].copy_from_slice(&buf[..size]);
                        Ok(size)
                    }).unwrap()
                } else {
                    middle_reads_b.close("Solitude is a foreign concept to me.".to_owned());
                    middle_writes_a.close("Do they even exist anymore?".to_owned());
                    break;
                }
            }
            #[allow(unreachable_code)]
            Ok(())
        },
        async move {
            let (sender, mut receiver) = futures::channel::mpsc::unbounded();
            b.register_service("test".to_owned(), move |chan| {
                let _ = sender.unbounded_send(chan);
                Ok::<(), Error>(())
            })?;
            let chan =
                receiver.next().await.ok_or_else(|| format_err!("No test request received"))?;
            let chan = fidl::AsyncChannel::from_channel(chan);
            log::info!(node_id = b.node_id().0; "CLIENT CONNECTED TO SERVER");
            assert_eq!(
                chan.recv_msg(&mut Default::default()).await,
                Err(fidl::Status::PEER_CLOSED)
            );
            let reason = chan.closed_reason().unwrap();
            assert_eq!("stream_to_handle\n\nCaused by:\n    0: stream.next()\n    1: unexpected end of stream (Multi-stream terminated: Err(ConnectionClosed(Some(\"Solitude is a foreign concept to me.\"))))", &reason);
            Ok::<(), Error>(())
        },
        async move {
            let chan = {
                let (peer_sender, mut peer_receiver) = futures::channel::mpsc::channel(0);
                a.list_peers(peer_sender)?;
                'retry: loop {
                    let peers =
                        peer_receiver.next().await.ok_or_else(|| format_err!("Lost connection"))?;
                    for peer in peers {
                        if peer.services.iter().find(|name| *name == "test").is_none() {
                            continue;
                        }
                        let (s, p) = fidl::Channel::create();
                        a.connect_to_service(peer.node_id, "test".to_owned(), s)?;
                        break 'retry p;
                    }
                }
            };
            crash_sender.send(()).unwrap();
            assert_eq!(fidl::Signals::CHANNEL_PEER_CLOSED, OnSignalsRef::new(&chan, fidl::Signals::CHANNEL_PEER_CLOSED).await.unwrap());
            let reason = chan.closed_reason().unwrap();
            assert_eq!("stream_to_handle\n\nCaused by:\n    0: stream.next()\n    1: unexpected end of stream (Multi-stream terminated: Err(ConnectionClosed(Some(\"Do they even exist anymore?\"))))", &reason);
            Ok::<(), Error>(())
        },
    )
    .map_ok(drop)
    .await
    .unwrap();
}
