// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use fidl::endpoints;
use fidl_fuchsia_starnix_container::{ControllerMarker, ControllerVsockConnectRequest};
use fidl_fuchsia_vsock::{
    AcceptorMarker, AcceptorRequest, ConnectionMarker, ConnectionProxy, ConnectionTransport,
    ConnectorMarker,
};
use fuchsia_component::client::connect_to_protocol;

use futures::StreamExt;

pub struct ProxyServer {
    connections: Vec<ConnectionProxy>,
}

const ADB_DEFAULT_PORT: u32 = 5555;

async fn handle_connection(host_socket: fidl::Socket) -> Result<()> {
    let controller = connect_to_protocol::<ControllerMarker>()
        .context("Failed to connect to starnix controller service")?;

    controller
        .vsock_connect(ControllerVsockConnectRequest {
            port: Some(ADB_DEFAULT_PORT),
            bridge_socket: Some(host_socket),
            ..Default::default()
        })
        .map_err(|e| anyhow!("Error connecting to adbd: {:?}", e))?;

    Ok(())
}

fn make_con() -> Result<(fidl::Socket, ConnectionProxy, ConnectionTransport), anyhow::Error> {
    let (client_socket, server_socket) = fidl::Socket::create_stream();
    let (client_end, server_end) = endpoints::create_endpoints::<ConnectionMarker>();
    let client_end = client_end.into_proxy();
    let con = ConnectionTransport { data: server_socket, con: server_end };
    Ok((client_socket, client_end, con))
}

impl ProxyServer {
    pub fn new() -> Self {
        ProxyServer { connections: Vec::new() }
    }

    pub async fn start(&mut self, port: u32) -> Result<()> {
        log::info!("Connecting to VSOCK protocol");
        let app_client = connect_to_protocol::<ConnectorMarker>()?;

        let (acceptor_remote, acceptor_client) = endpoints::create_endpoints::<AcceptorMarker>();
        let mut acceptor_client = acceptor_client.into_stream();

        app_client.listen(port, acceptor_remote).await?.map_err(zx::Status::err_from_raw)?;

        log::info!("Listening for VSOCK connections.");

        while let Some(Ok(AcceptorRequest::Accept { addr: _addr, responder })) =
            acceptor_client.next().await
        {
            log::info!("Incoming VSOCK connection.");
            let (data_socket, client_end, con) = make_con().context("Making connection")?;
            self.connections.push(client_end);
            responder.send(Some(con)).context("Sending con")?;

            match handle_connection(data_socket).await {
                Ok(_) => (),
                Err(e) => {
                    log::error!("Failed to handle connection: {:?}", e);
                    continue;
                }
            };
        }

        Ok(())
    }
}
