// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A wrapping of Overnet that is particularly useful for overnet integration tests that don't
//! depend on the surrounding environment (and can thus be run like self contained unit tests)

#![cfg(test)]

mod drop;
mod echo;
mod triangle;

#[cfg(not(target_os = "fuchsia"))]
mod error_propagation;

use anyhow::Error;
use circuit::multi_stream::{multi_stream_node_connection, multi_stream_node_connection_to_async};
use circuit::stream as cs;
use fuchsia_async::Task;
use fuchsia_sync::Mutex;
use futures::channel::mpsc::unbounded;
use futures::prelude::*;
use overnet_core::{log_errors, ListablePeer, NodeId, NodeIdGenerator, Router};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

///////////////////////////////////////////////////////////////////////////////////////////////////
// Overnet <-> API bindings

enum OvernetCommand {
    ListPeers(futures::channel::mpsc::Sender<Vec<ListablePeer>>),
    RegisterService(String, Box<dyn Fn(fidl::Channel) -> Result<(), Error> + Send + 'static>),
    ConnectToService(NodeId, String, fidl::Channel),
    AttachCircuitSocketLink(fidl::Socket, bool),
    AttachCircuitLink(cs::Reader, cs::Writer, bool),
}

impl std::fmt::Debug for OvernetCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListPeers(arg0) => f.debug_tuple("ListPeers").field(arg0).finish(),
            Self::RegisterService(arg0, _) => {
                f.debug_tuple("RegisterService").field(arg0).field(&"<fn>".to_owned()).finish()
            }
            Self::ConnectToService(arg0, arg1, arg2) => {
                f.debug_tuple("ConnectToService").field(arg0).field(arg1).field(arg2).finish()
            }
            Self::AttachCircuitSocketLink(arg0, arg1) => {
                f.debug_tuple("AttachCircuitSocketLink").field(arg0).field(arg1).finish()
            }
            Self::AttachCircuitLink(arg0, arg1, arg2) => {
                f.debug_tuple("AttachCircuitLink").field(arg0).field(arg1).field(arg2).finish()
            }
        }
    }
}

/// Overnet implementation for integration tests
pub struct Overnet {
    tx: Mutex<futures::channel::mpsc::UnboundedSender<OvernetCommand>>,
    node_id: NodeId,
    // Main loop for the Overnet instance - once the object is dropped, the loop can stop.
    _task: Task<()>,
}

impl Overnet {
    /// Create a new instance
    pub fn new(node_id_gen: &mut NodeIdGenerator) -> Result<Arc<Overnet>, Error> {
        Self::from_router(node_id_gen.new_router()?)
    }

    /// Create a new circuit instance
    pub fn new_circuit_router(node_id_gen: &mut NodeIdGenerator) -> Result<Arc<Overnet>, Error> {
        Self::from_router(node_id_gen.new_router_circuit_router()?)
    }

    fn from_router(node: Arc<Router>) -> Result<Arc<Overnet>, Error> {
        let (tx, rx) = unbounded();
        log::info!(node_id = node.node_id().0; "SPAWN OVERNET");
        let tx = Mutex::new(tx);
        Ok(Arc::new(Overnet {
            tx,
            node_id: node.node_id(),
            _task: Task::spawn(log_errors(run_overnet(node, rx), "Main loop failed")),
        }))
    }

    fn send(&self, cmd: OvernetCommand) -> Result<(), fidl::Error> {
        Ok(self.tx.lock().unbounded_send(cmd).map_err(|_| fidl::Error::Invalid)?)
    }

    pub fn register_service(
        &self,
        service_name: String,
        provider: impl Fn(fidl::Channel) -> Result<(), Error> + Send + 'static,
    ) -> Result<(), fidl::Error> {
        self.send(OvernetCommand::RegisterService(service_name, Box::new(provider)))
    }

    pub fn connect_to_service(
        &self,
        node_id: NodeId,
        service_name: String,
        chan: fidl::Channel,
    ) -> Result<(), fidl::Error> {
        self.send(OvernetCommand::ConnectToService(node_id, service_name, chan))
    }

    pub fn list_peers(
        &self,
        sender: futures::channel::mpsc::Sender<Vec<ListablePeer>>,
    ) -> Result<(), fidl::Error> {
        self.send(OvernetCommand::ListPeers(sender))
    }

    fn attach_circuit_socket_link(
        &self,
        socket: fidl::Socket,
        is_server: bool,
    ) -> Result<(), fidl::Error> {
        self.send(OvernetCommand::AttachCircuitSocketLink(socket, is_server))
    }

    pub fn attach_circuit_link(
        &self,
        reader: cs::Reader,
        writer: cs::Writer,
        is_server: bool,
    ) -> Result<(), fidl::Error> {
        self.send(OvernetCommand::AttachCircuitLink(reader, writer, is_server))
    }

    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}

async fn run_overnet_command(node: Arc<Router>, cmd: OvernetCommand) -> Result<(), Error> {
    match cmd {
        OvernetCommand::ListPeers(mut sender) => {
            let lpc = node.new_list_peers_context().await;
            Task::spawn(async move {
                while let Ok(peers) = lpc.list_peers().await {
                    let _ = sender.send(peers).await;
                }
            })
            .detach();
            Ok(())
        }
        OvernetCommand::RegisterService(service_name, provider) => {
            node.register_service(service_name, provider).await
        }
        OvernetCommand::ConnectToService(node_id, service_name, channel) => {
            node.connect_to_service(node_id, &service_name, channel).await
        }
        OvernetCommand::AttachCircuitSocketLink(socket, is_server) => {
            let (mut rx, mut tx) = fidl::AsyncSocket::from_socket(socket).split();
            let (errors_sender, _black_hole) = unbounded();
            multi_stream_node_connection_to_async(
                node.circuit_node(),
                &mut rx,
                &mut tx,
                is_server,
                circuit::Quality::IN_PROCESS,
                errors_sender,
                "test node".to_owned(),
            )
            .await?;
            Ok(())
        }
        OvernetCommand::AttachCircuitLink(reader, writer, is_server) => {
            let (errors_sender, _black_hole) = unbounded();
            multi_stream_node_connection(
                node.circuit_node(),
                reader,
                writer,
                is_server,
                circuit::Quality::IN_PROCESS,
                errors_sender,
                "test node".to_owned(),
            )
            .await?;
            Ok(())
        }
    }
}

static NEXT_CMD_ID: AtomicU64 = AtomicU64::new(0);

async fn run_overnet(
    node: Arc<Router>,
    rx: futures::channel::mpsc::UnboundedReceiver<OvernetCommand>,
) -> Result<(), Error> {
    let node_id = node.node_id();
    log::info!(node_id:?; "RUN OVERNET");
    // Run application loop
    rx.for_each_concurrent(None, move |cmd| {
        let node = node.clone();
        async move {
            let cmd_text = format!("{:?}", cmd);
            let cmd_id = NEXT_CMD_ID.fetch_add(1, Ordering::Relaxed);
            log::info!(node_id = node_id.0, cmd = cmd_id; "START: {}", cmd_text);
            if let Err(e) = run_overnet_command(node, cmd).await {
                log::info!(node_id = node_id.0, cmd = cmd_id; "{} with error: {:?}", cmd_text, e);
            } else {
                log::info!(node_id = node_id.0, cmd = cmd_id; "SUCCEEDED: {}", cmd_text);
            }
        }
    })
    .await;
    log::info!(node_id = node_id.0; "DONE OVERNET");
    Ok(())
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Connect Overnet nodes to each other

/// Connect two test overnet instances with a stream socket.
pub fn connect(a: &Arc<Overnet>, b: &Arc<Overnet>) -> Result<(), Error> {
    log::info!(a = a.node_id().0, b = b.node_id().0; "Connect nodes");
    let (sa, sb) = fidl::Socket::create_stream();
    a.attach_circuit_socket_link(sa, true)?;
    b.attach_circuit_socket_link(sb, false)?;
    Ok(())
}
