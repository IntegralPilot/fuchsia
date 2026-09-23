// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fdomain::serve_fdomain_connection;
use anyhow::Result;
use circuit::multi_stream::multi_stream_node_connection_to_async;
use fidl_fuchsia_vsock as vsock;
use fuchsia_async as fasync;
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use overnet_core::Router;
use remote_control::RemoteControlService;
use std::rc::{Rc, Weak as WeakRc};
use std::sync::Weak;

const FDOMAIN_VSOCK_PORT: u32 = 203;
const OVERNET_VSOCK_PORT: u32 = 202;
const IDENTIFY_VSOCK_PORT: u32 = 201;

pub async fn run_vsocks(router: Weak<Router>, service: WeakRc<RemoteControlService>) -> Result<()> {
    let connector = fuchsia_component::client::connect_to_protocol::<vsock::ConnectorMarker>()?;
    let (client, overnet_requests) = fidl::endpoints::create_request_stream();
    connector.listen(OVERNET_VSOCK_PORT, client).await?.map_err(fidl::Status::err_from_raw)?;

    let (client, fdomain_requests) = fidl::endpoints::create_request_stream();
    connector.listen(FDOMAIN_VSOCK_PORT, client).await?.map_err(fidl::Status::err_from_raw)?;

    let mut requests = futures::stream::select(overnet_requests, fdomain_requests);

    while let Some(request) = requests.next().await {
        let vsock::AcceptorRequest::Accept { addr, responder } = request?;

        log::info!(addr:? = addr; "Accepted VSOCK connection");

        if addr.local_port == OVERNET_VSOCK_PORT {
            let (client, con) = fidl::endpoints::create_endpoints();
            let (data, socket) = fidl::Socket::create_stream();
            let socket = fuchsia_async::Socket::from_socket(socket);
            let (mut reader, mut writer) = socket.split();
            let (err_sender, mut err_receiver) = futures::channel::mpsc::unbounded();

            let scope = fasync::Scope::new();
            scope.spawn(async move {
                while let Some(error) = err_receiver.next().await {
                    log::debug!(
                        error:? = error;
                        "Stream error for VSOCK link"
                    )
                }
            });

            let Some(router) = router.upgrade() else { return Ok(()) };

            scope.spawn(async move {
                let _client = client;

                if let Err(error) = multi_stream_node_connection_to_async(
                    router.circuit_node(),
                    &mut reader,
                    &mut writer,
                    true,
                    circuit::Quality::LOCAL_SOCKET,
                    err_sender,
                    format!("VSOCK {addr:?}"),
                )
                .await
                {
                    log::info!(
                        addr:? = addr,
                        error:? = error;
                        "VSOCK link terminated",
                    );
                }
            });

            scope.detach();

            responder.send(Some(vsock::ConnectionTransport { data, con }))?;
        } else {
            debug_assert!(addr.local_port == FDOMAIN_VSOCK_PORT);

            let (client, con) = fidl::endpoints::create_endpoints();
            let (data, socket) = fidl::Socket::create_stream();
            let socket = fuchsia_async::Socket::from_socket(socket);
            fasync::Task::local({
                let service = service.clone();
                async move {
                    let _client = client;

                    serve_fdomain_connection(service, socket).await;
                }
            })
            .detach();
            responder.send(Some(vsock::ConnectionTransport { data, con }))?;
        }
    }
    Ok(())
}

pub async fn run_identify_vsock(service: Rc<RemoteControlService>) -> Result<()> {
    let connector = fuchsia_component::client::connect_to_protocol::<vsock::ConnectorMarker>()?;
    let (client, mut requests) = fidl::endpoints::create_request_stream();
    connector.listen(IDENTIFY_VSOCK_PORT, client).await?.map_err(fidl::Status::err_from_raw)?;

    while let Some(request) = requests.next().await {
        let vsock::AcceptorRequest::Accept { addr, responder } = request?;

        log::info!(addr:? = addr; "Accepted VSOCK connection");

        let (_client, con) = fidl::endpoints::create_endpoints();
        let (data, socket) = fidl::Socket::create_stream();
        let socket = fuchsia_async::Socket::from_socket(socket);
        let (_reader, mut writer) = socket.split();
        responder.send(Some(vsock::ConnectionTransport { data, con }))?;

        let header = fidl::encoding::TransactionHeader::new(
            0,
            0x6035e1ab368deee1,
            fidl::encoding::DynamicFlags::FLEXIBLE,
        );
        let identity_result = service.get_host_identity().await;

        let buf = fidl_message::encode_response_result(header, identity_result)?;
        writer.write_all(&buf).await?;
    }
    Ok(())
}
