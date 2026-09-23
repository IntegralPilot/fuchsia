// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Tests for dropping connections

#![cfg(test)]

use super::{connect, Overnet};
use anyhow::{format_err, Context as _, Error};
use futures::prelude::*;
use overnet_core::NodeIdGenerator;
use std::sync::Arc;
use zx_status;

#[fuchsia::test]
async fn drop_connection_2node(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("drop_connection_2node", run);
    // Two nodes, connected A->B.
    // Create a channel from A->B, then drop A.
    // See that the channel drops on B.
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new(&mut node_id_gen)?;
    connect(&a, &b)?;
    run_drop_test(a, b).await
}

#[fuchsia::test]
async fn drop_connection_3node(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("drop_connection_3node", run);
    // Three nodes, connected A->B->C.
    // Create a channel from A->C, then drop A.
    // See that the channel drops on C.
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new_circuit_router(&mut node_id_gen)?;
    let c = Overnet::new(&mut node_id_gen)?;
    connect(&a, &b)?;
    connect(&b, &c)?;
    run_drop_test(a, c).await
}

async fn run_drop_test(a: Arc<Overnet>, b: Arc<Overnet>) -> Result<(), Error> {
    let a_clone = Arc::clone(&a);
    futures::future::try_join(
        async move {
            let (sender, mut receiver) = futures::channel::mpsc::unbounded();
            b.register_service("test".to_owned(), move |chan| {
                let _ = sender.unbounded_send(chan);
                Ok(())
            })?;
            let chan =
                receiver.next().await.ok_or_else(|| format_err!("No test request received"))?;
            let chan = fidl::AsyncChannel::from_channel(chan);
            log::info!(node_id = b.node_id().0; "CLIENT CONNECTED TO SERVER");
            chan.write(&[], &mut vec![]).context("writing to client")?;
            log::info!(node_id = b.node_id().0; "WAITING FOR CLOSE of {chan:?}");
            assert_eq!(
                chan.recv_msg(&mut Default::default()).await,
                Err(zx_status::Status::PEER_CLOSED)
            );
            log::info!("RETURNING OBSERVER");
            // TODO(b/306655845): Figure out why the test hangs without this and delete it.
            drop(a_clone);
            Ok(())
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
            log::info!(node_id = a.node_id().0; "GOT CLIENT CHANNEL");
            let chan = fidl::AsyncChannel::from_channel(chan);
            chan.recv_msg(&mut Default::default()).await.context("waiting for server message")?;
            log::info!(node_id = a.node_id().0; "GOT MESSAGE FROM SERVER - DROPPING CLIENT");
            drop(a);
            drop(chan);
            log::info!("RETURNING DROPPER");
            Ok(())
        },
    )
    .map_ok(drop)
    .await
}
