// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_hardware_qualcomm_router as fqrtr;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::prelude::*;
use zx::{self, Peered};

enum Incoming {
    QrtrConnector(fqrtr::ClientServiceRequest),
}

pub async fn mock_qrtr_client_service(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service_instance("default", Incoming::QrtrConnector);
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(0, |request| async move {
        match request {
            Incoming::QrtrConnector(fqrtr::ClientServiceRequest::QrtrConnector(stream)) => {
                run_qrtr_connector_server(stream)
                    .await
                    .unwrap_or_else(|e| eprintln!("Error while serving QrtrConnector: {:?}", e))
            }
        }
    })
    .await;

    Ok(())
}

async fn run_qrtr_connector_server(stream: fqrtr::QrtrConnectorRequestStream) -> Result<(), Error> {
    stream
        .map(|result| result.context("failed request"))
        .try_for_each(|request| async move {
            match request {
                fqrtr::QrtrConnectorRequest::GetConnection { options, proxy, responder } => {
                    qrtr_connector_get_connection(options, proxy, responder)
                }
                _ => unreachable!("Unexpected QrtrConnectorRequest"),
            }
        })
        .await
}

fn qrtr_connector_get_connection(
    _options: fqrtr::ConnectionOptions,
    proxy: fidl::endpoints::ServerEnd<fqrtr::QrtrClientConnectionMarker>,
    responder: fqrtr::QrtrConnectorGetConnectionResponder,
) -> Result<(), Error> {
    responder.send(Ok(())).context("sending GetConnection response")?;
    fuchsia_async::Task::spawn(async move {
        if let Err(e) = run_qrtr_client_connection_server(proxy.into_stream()).await {
            eprintln!("Error while serving QrtrClientConnection: {:?}", e);
        }
    })
    .detach();
    Ok(())
}

async fn run_qrtr_client_connection_server(
    mut stream: fqrtr::QrtrClientConnectionRequestStream,
) -> Result<(), Error> {
    let mut server_local = None;

    while let Some(Ok(request)) = stream.next().await {
        match request {
            fqrtr::QrtrClientConnectionRequest::GetNodeId { responder } => {
                responder.send(1).context("sending GetNodeId response")?;
            }
            fqrtr::QrtrClientConnectionRequest::GetPortId { responder } => {
                responder.send(10).context("sending GetPortId response")?;
            }
            fqrtr::QrtrClientConnectionRequest::GetSignals { responder } => {
                let (client, server) = zx::EventPair::create();
                server
                    .signal_peer(
                        zx::Signals::NONE,
                        zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE),
                    )
                    .context("signaling peer")?;
                server_local = Some(server);
                let _ = responder.send(client);
            }
            fqrtr::QrtrClientConnectionRequest::Read { responder } => {
                responder.send(Ok((2, 20, b"recv_data"))).context("sending Read response")?;
            }
            fqrtr::QrtrClientConnectionRequest::Write {
                dst_node_id: _,
                dst_port,
                data,
                responder,
            } => {
                // We are using "in-band" signaling to control the mock server. Writes to port
                // 9999 will be interpreted as control commands.
                if dst_port == 9999 {
                    // Control message
                    if let Some(server) = server_local.as_ref() {
                        let cmd = std::str::from_utf8(&data).unwrap_or("");
                        match cmd {
                            "BLOCK_READ" => {
                                server
                                    .signal_peer(
                                        zx::Signals::from_bits_truncate(fqrtr::SIGNAL_READABLE),
                                        zx::Signals::NONE,
                                    )
                                    .context("clearing readable")?;
                            }
                            "UNBLOCK_READ" => {
                                server
                                    .signal_peer(
                                        zx::Signals::NONE,
                                        zx::Signals::from_bits_truncate(fqrtr::SIGNAL_READABLE),
                                    )
                                    .context("setting readable")?;
                            }
                            "BLOCK_WRITE" => {
                                server
                                    .signal_peer(
                                        zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE),
                                        zx::Signals::NONE,
                                    )
                                    .context("clearing writable")?;
                            }
                            "UNBLOCK_WRITE" => {
                                server
                                    .signal_peer(
                                        zx::Signals::NONE,
                                        zx::Signals::from_bits_truncate(fqrtr::SIGNAL_WRITABLE),
                                    )
                                    .context("setting writable")?;
                            }
                            _ => {
                                eprintln!("Unknown control command: {}", cmd);
                            }
                        }
                    }
                }
                responder.send(Ok(())).context("sending Write response")?;
            }
            fqrtr::QrtrClientConnectionRequest::CloseConnection { responder } => {
                server_local.take();
                responder.send().context("sending CloseConnection response")?;
                return Ok(());
            }
            _ => {
                // Ignore unknown methods
                return Err(anyhow::anyhow!("Unknown QrtrClientConnectionRequest"));
            }
        }
    }
    Ok(())
}
