// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Echo integration test for Fuchsia (much like the echo example - indeed the initial code came
//! from there, but self contained and more adaptable to different scenarios)

#![cfg(test)]

use super::Overnet;
use anyhow::{Context as _, Error};
use fidl::prelude::*;
use fidl_test_echo as echo;
use fuchsia_async::Task;
use futures::prelude::*;
use overnet_core::NodeIdGenerator;
use std::sync::Arc;

////////////////////////////////////////////////////////////////////////////////
// Test scenarios

#[fuchsia::test]
async fn simple(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("simple", run);
    let client = Overnet::new(&mut node_id_gen)?;
    let server = Overnet::new(&mut node_id_gen)?;
    super::connect(&client, &server)?;
    run_echo_test(client, server, Some("HELLO INTEGRATION TEST WORLD")).await
}

#[fuchsia::test]
async fn kilobyte(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("kilobyte", run);
    let client = Overnet::new(&mut node_id_gen)?;
    let server = Overnet::new(&mut node_id_gen)?;
    super::connect(&client, &server)?;
    run_echo_test(client, server, Some(&std::iter::repeat('a').take(1024).collect::<String>()))
        .await
}

#[fuchsia::test]
async fn quite_large(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("quite_large", run);
    let client = Overnet::new(&mut node_id_gen)?;
    let server = Overnet::new(&mut node_id_gen)?;
    super::connect(&client, &server)?;
    run_echo_test(client, server, Some(&std::iter::repeat('a').take(60000).collect::<String>()))
        .await
}

////////////////////////////////////////////////////////////////////////////////
// Client implementation

async fn exec_client(overnet: Arc<Overnet>, text: Option<&str>) -> Result<(), Error> {
    let (peer_sender, mut peer_receiver) = futures::channel::mpsc::channel(0);
    overnet.list_peers(peer_sender)?;
    loop {
        let peers =
            peer_receiver.next().await.ok_or_else(|| anyhow::format_err!("List peers hung up"))?;
        log::info!(node_id = overnet.node_id().0; "Got peers: {:?}", peers);
        for peer in peers {
            if peer.services.iter().find(|name| *name == echo::EchoMarker::PROTOCOL_NAME).is_none()
            {
                continue;
            }
            let (s, p) = fidl::Channel::create();
            overnet
                .connect_to_service(peer.node_id, echo::EchoMarker::PROTOCOL_NAME.to_owned(), s)
                .unwrap();
            let cli = echo::EchoProxy::new(fidl::AsyncChannel::from_channel(p));
            log::info!(node_id = overnet.node_id().0; "Sending {:?} to {:?}", text, peer.node_id);
            assert_eq!(cli.echo_string(text).await.unwrap(), text.map(|s| s.to_string()));
            return Ok(());
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Server implementation

async fn exec_server(overnet: Arc<Overnet>) -> Result<(), Error> {
    let node_id = overnet.node_id();
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    overnet.register_service(echo::EchoMarker::PROTOCOL_NAME.to_owned(), move |chan| {
        let _ = sender.unbounded_send(chan);
        Ok(())
    })?;
    receiver
        .map(Result::<_, Error>::Ok)
        .try_for_each_concurrent(None, |chan| async move {
            log::info!(node_id = node_id.0; "Received service request for service");
            let mut stream =
                echo::EchoRequestStream::from_channel(fidl::AsyncChannel::from_channel(chan));
            while let Some(echo::EchoRequest::EchoString { value, responder }) =
                stream.try_next().await.context("error running echo server")?
            {
                log::info!(node_id = node_id.0; "Received echo request for string {:?}", value);
                responder.send(value.as_ref().map(|s| &**s)).context("error sending response")?;
                log::info!(node_id = node_id.0; "echo response sent successfully");
            }
            Ok(())
        })
        .await?;
    drop(overnet);
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Test driver

async fn run_echo_test(
    client: Arc<Overnet>,
    server: Arc<Overnet>,
    text: Option<&str>,
) -> Result<(), Error> {
    let server = Task::spawn(async move {
        let server_id = server.node_id();
        exec_server(server).await.unwrap();
        log::info!(server_id = server_id.0; "SERVER DONE");
    });
    let r = exec_client(client, text).await;
    drop(server);
    r
}
