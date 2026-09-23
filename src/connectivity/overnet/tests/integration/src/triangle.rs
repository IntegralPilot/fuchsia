// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Triangular overnet tests - tests that involve passing a channel amongst at least three nodes

#![cfg(test)]

use super::Overnet;
use anyhow::{Context as _, Error};
use fidl::endpoints::{ClientEnd, ServerEnd};
use fidl::prelude::*;
use fuchsia_async::Task;
use futures::prelude::*;
use overnet_core::{ListablePeer, NodeId, NodeIdGenerator};
use std::sync::Arc;
use {fidl_test_echo as echo, fidl_test_triangle as triangle};

////////////////////////////////////////////////////////////////////////////////
// Test scenarios

#[fuchsia::test]
async fn simple_loop(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("simple_loop", run);
    // Three nodes, fully connected
    // A creates a channel, passes either end to B, C to do an echo request
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new(&mut node_id_gen)?;
    let c = Overnet::new(&mut node_id_gen)?;
    super::connect(&a, &b)?;
    super::connect(&b, &c)?;
    super::connect(&a, &c)?;
    run_triangle_echo_test(
        b.node_id(),
        c.node_id(),
        a,
        vec![],
        vec![b, c],
        Some("HELLO INTEGRATION TEST WORLD"),
    )
    .await
}

#[fuchsia::test]
async fn simple_flat(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("simple_flat", run);
    // Three nodes, connected linearly: C - A - B
    // A creates a channel, passes either end to B, C to do an echo request
    let a = Overnet::new_circuit_router(&mut node_id_gen)?;
    let b = Overnet::new(&mut node_id_gen)?;
    let c = Overnet::new(&mut node_id_gen)?;
    super::connect(&a, &b)?;
    super::connect(&a, &c)?;
    run_triangle_echo_test(
        b.node_id(),
        c.node_id(),
        a,
        vec![],
        vec![b, c],
        Some("HELLO INTEGRATION TEST WORLD"),
    )
    .await
}

#[fuchsia::test]
async fn full_transfer(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("full_transfer", run);
    // Two nodes connected
    // A creates a channel, passes both ends to B to do an echo request
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new(&mut node_id_gen)?;
    super::connect(&a, &b)?;
    run_triangle_echo_test(
        b.node_id(),
        b.node_id(),
        a,
        vec![],
        vec![b],
        Some("HELLO INTEGRATION TEST WORLD"),
    )
    .await
}

#[fuchsia::test]
async fn forwarded_twice_to_separate_nodes(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("forwarded_twice_to_separate_nodes", run);
    // Five nodes connected in a line: A - B - C - D - E
    // A creates a channel, passes either end to B & C
    // B & C forward to D & E (respectively) and then do an echo request
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new_circuit_router(&mut node_id_gen)?;
    let c = Overnet::new_circuit_router(&mut node_id_gen)?;
    let d = Overnet::new_circuit_router(&mut node_id_gen)?;
    let e = Overnet::new(&mut node_id_gen)?;
    log::info!(
        a = a.node_id().0,
        b = b.node_id().0,
        c = c.node_id().0,
        d = d.node_id().0,
        e = e.node_id().0;
        "NODEIDS"
    );
    super::connect(&a, &b)?;
    super::connect(&b, &c)?;
    super::connect(&c, &d)?;
    super::connect(&d, &e)?;
    run_triangle_echo_test(
        b.node_id(),
        c.node_id(),
        a,
        vec![(b, d.node_id()), (c, e.node_id())],
        vec![d, e],
        Some("HELLO INTEGRATION TEST WORLD"),
    )
    .await
}

#[fuchsia::test]
async fn forwarded_twice_full_transfer(run: usize) -> Result<(), Error> {
    let mut node_id_gen = NodeIdGenerator::new("forwarded_twice_full_transfer", run);
    // Four nodes connected in a line: A - B - C - D
    // A creates a channel, passes either end to B & C
    // B & C forward to D which then does an echo request
    let a = Overnet::new(&mut node_id_gen)?;
    let b = Overnet::new_circuit_router(&mut node_id_gen)?;
    let c = Overnet::new_circuit_router(&mut node_id_gen)?;
    let d = Overnet::new(&mut node_id_gen)?;
    super::connect(&a, &b)?;
    super::connect(&b, &c)?;
    super::connect(&c, &d)?;
    run_triangle_echo_test(
        b.node_id(),
        c.node_id(),
        a,
        vec![(b, d.node_id()), (c, d.node_id())],
        vec![d],
        Some("HELLO INTEGRATION TEST WORLD"),
    )
    .await
}

////////////////////////////////////////////////////////////////////////////////
// Utilities

fn has_peer_conscript(peers: &[ListablePeer], peer_id: NodeId) -> bool {
    let is_peer_ready = |peer: &ListablePeer| -> bool {
        peer.node_id == peer_id
            && peer.services.iter().any(|name| *name == triangle::ConscriptMarker::PROTOCOL_NAME)
    };
    peers.iter().any(|p| is_peer_ready(p))
}

fn connect_peer(
    overnet: &Arc<Overnet>,
    node_id: NodeId,
) -> Result<triangle::ConscriptProxy, Error> {
    let (s, p) = fidl::Channel::create();
    overnet
        .connect_to_service(node_id, triangle::ConscriptMarker::PROTOCOL_NAME.to_owned(), s)
        .unwrap();
    Ok(triangle::ConscriptProxy::new(fidl::AsyncChannel::from_channel(p)))
}

////////////////////////////////////////////////////////////////////////////////
// Client implementation

async fn exec_captain(
    client: NodeId,
    server: NodeId,
    overnet: Arc<Overnet>,
    text: Option<&str>,
) -> Result<(), Error> {
    let (peer_sender, mut peer_receiver) = futures::channel::mpsc::channel(0);
    overnet.list_peers(peer_sender)?;
    loop {
        let peers = peer_receiver
            .next()
            .await
            .ok_or_else(|| anyhow::format_err!("Peer receiver hung up"))?;
        log::info!(node_id = overnet.node_id().0; "Got peers: {:?}", peers);
        if has_peer_conscript(&peers, client) && has_peer_conscript(&peers, server) {
            let client = connect_peer(&overnet, client)?;
            let server = connect_peer(&overnet, server)?;
            let (s, p) = fidl::Channel::create();
            log::info!(node_id = overnet.node_id().0; "server/proxy hdls: {:?} {:?}", s, p);
            log::info!(node_id = overnet.node_id().0; "ENGAGE CONSCRIPTS");
            server.serve(ServerEnd::new(s))?;
            let response = client
                .issue(ClientEnd::new(p), text)
                .await
                .context(format!("awaiting issue response for captain {:?}", overnet.node_id()))?;
            log::info!(node_id = overnet.node_id().0; "Captain got response: {:?}", response);
            assert_eq!(response, text.map(|s| s.to_string()));
            return Ok(());
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Conscript implementation

async fn exec_server(node_id: NodeId, server: ServerEnd<echo::EchoMarker>) -> Result<(), Error> {
    let mut stream = server.into_stream();
    log::info!(node_id = node_id.0; "server begins");
    while let Some(echo::EchoRequest::EchoString { value, responder }) =
        stream.try_next().await.context("error running echo server")?
    {
        log::info!(node_id = node_id.0; "Received echo request for string {:?}", value);
        responder.send(value.as_ref().map(|s| &**s)).context("error sending response")?;
        log::info!(node_id = node_id.0; "echo response sent successfully");
    }
    log::info!(node_id = node_id.0; "server done");
    Ok(())
}

async fn exec_client(
    node_id: NodeId,
    client: ClientEnd<echo::EchoMarker>,
    text: Option<String>,
    responder: triangle::ConscriptIssueResponder,
) -> Result<(), Error> {
    log::info!(node_id = node_id.0; "CLIENT SEND REQUEST: {:?}", text);
    let response = client.into_proxy().echo_string(text.as_deref()).await.unwrap();
    log::info!(node_id = node_id.0; "CLIENT GETS RESPONSE: {:?}", response);
    responder.send(response.as_deref())?;
    Ok(())
}

async fn exec_conscript<
    F: 'static + Send + Clone + Fn(triangle::ConscriptRequest) -> Fut,
    Fut: Future<Output = Result<(), Error>>,
>(
    overnet: Arc<Overnet>,
    action: F,
) -> Result<(), Error> {
    let (sender, receiver) = futures::channel::mpsc::unbounded();
    let node_id = overnet.node_id();
    overnet.register_service(triangle::ConscriptMarker::PROTOCOL_NAME.to_owned(), move |chan| {
        let _ = sender.unbounded_send(chan);
        Ok(())
    })?;
    receiver
        .map(Ok)
        .try_for_each_concurrent(None, |chan| {
            let action = action.clone();
            async move {
                log::info!(node_id = node_id.0; "Received service request for service");
                let chan = fidl::AsyncChannel::from_channel(chan);
                log::info!(node_id = node_id.0; "Started service handler");
                triangle::ConscriptRequestStream::from_channel(chan)
                    .map_err(Into::into)
                    .try_for_each_concurrent(None, |request| {
                        let action = action.clone();
                        async move {
                            log::info!(node_id = node_id.0; "Received request {:?}", request);
                            action(request).await
                        }
                    })
                    .await?;
                log::info!(node_id = node_id.0; "Finished service handler");
                Ok(())
            }
        })
        .await
}

async fn conscript_leaf_action(
    own_id: NodeId,
    request: triangle::ConscriptRequest,
) -> Result<(), Error> {
    log::info!(node_id = own_id.0; "Handling it");
    match request {
        triangle::ConscriptRequest::Serve { iface, control_handle: _ } => {
            exec_server(own_id, iface)
                .await
                .context(format!("running conscript server {:?}", own_id))
        }
        triangle::ConscriptRequest::Issue { iface, request, responder } => {
            exec_client(own_id, iface, request, responder)
                .await
                .context(format!("running conscript client {:?}", own_id))
        }
    }
}

async fn conscript_forward_action(
    own_id: NodeId,
    node_id: NodeId,
    request: triangle::ConscriptRequest,
    target: triangle::ConscriptProxy,
) -> Result<(), Error> {
    log::info!(node_id = own_id.0; "Forwarding request to {:?}", node_id);
    match request {
        triangle::ConscriptRequest::Serve { iface, control_handle: _ } => {
            target.serve(iface)?;
        }
        triangle::ConscriptRequest::Issue { iface, request, responder } => {
            let response = target.issue(iface, request.as_deref()).await?;
            log::info!("Forwarder got response: {:?}", response);
            responder.send(response.as_deref())?;
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Test driver

async fn run_triangle_echo_test(
    client: NodeId,
    server: NodeId,
    captain: Arc<Overnet>,
    forwarders: Vec<(Arc<Overnet>, NodeId)>,
    conscripts: Vec<Arc<Overnet>>,
    text: Option<&str>,
) -> Result<(), Error> {
    let captain_node_id = captain.node_id();
    let mut background_tasks = Vec::new();
    for (forwarder, target_node_id) in forwarders.into_iter() {
        background_tasks.push(Task::spawn(
            async move {
                let (sender, mut receiver) = futures::channel::mpsc::channel(0);
                forwarder.list_peers(sender)?;
                loop {
                    let peers = receiver
                        .next()
                        .await
                        .ok_or_else(|| anyhow::format_err!("List peers hung up"))?;
                    log::info!(
                        "Waiting for forwarding target {:?}; got peers {:?}",
                        target_node_id,
                        peers
                    );
                    if has_peer_conscript(&peers, target_node_id) {
                        break;
                    }
                }
                let target = connect_peer(&forwarder, target_node_id)?;
                let own_id = forwarder.node_id();
                exec_conscript(forwarder, move |request| {
                    conscript_forward_action(own_id, target_node_id, request, target.clone())
                })
                .await
            }
            .unwrap_or_else(|e: Error| log::warn!("{:?}", e)),
        ));
    }
    for conscript in conscripts.into_iter() {
        let conscript = conscript.clone();
        let own_id = conscript.node_id();
        background_tasks.push(Task::spawn(async move {
            exec_conscript(conscript, move |request| conscript_leaf_action(own_id, request))
                .await
                .unwrap()
        }));
    }
    exec_captain(client, server, captain, text).await?;
    for (i, task) in background_tasks.into_iter().enumerate() {
        log::info!(node_id = captain_node_id.0; "drop background task {}", i);
        drop(task);
    }
    log::info!(node_id = captain_node_id.0; "returning from test driver");
    Ok(())
}
