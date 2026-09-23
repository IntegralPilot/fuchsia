// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use anyhow::{Context, Error};
use fidl_fuchsia_hardware_sockettunnel::{DeviceRequest, ServiceRequest};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};

use zx::Socket;

// This socket tunnel mock is only intended to support socket, and is only tuned for firmware_name.
pub async fn mock_socket_tunnel(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();

    // Need to keep this socket active beyond the life cycle of the FIDL
    // call to ensure we don't close it.
    let persistent_socket: Arc<Mutex<Option<Socket>>> = Arc::new(Mutex::new(None));

    // Add the Device protocol to the ServiceFs
    fs.dir("svc").add_fidl_service(ServiceRequest::SocketTunnel);

    // Run the ServiceFs on the outgoing directory handle from the mock handles
    fs.serve_connection(handles.outgoing_dir)?;

    // For each stream/connection
    fs.for_each_concurrent(0, move |ServiceRequest::SocketTunnel(stream)| {
        let local_copy = persistent_socket.clone();
        async move {
            stream
                .map(|result| result.context("failed request"))
                .try_for_each(|request| {
                    let persistent_socket_ptr = local_copy.clone();
                    async move {
                        match request {
                            DeviceRequest::RegisterSocket { payload, responder } => {
                                assert!(payload.socket_label.is_some());

                                let socket = payload.server_socket.unwrap();
                                let _ = socket.write("test_firmware_name".as_bytes());

                                // Dedicated scope here to limit the time the Mutex is locked
                                {
                                    let mut persistent_socket = persistent_socket_ptr.lock().await;
                                    *persistent_socket = Some(socket);
                                }

                                let _res = responder.send(Ok(()));
                            }
                            other => {
                                panic!("Unexpected route invoked on FakeSocketTunnel {other:?}")
                            }
                        }
                        Ok(())
                    }
                })
                .await
                .context("Failed to serve request stream")
                .unwrap_or_else(|e| eprintln!("Error encountered: {:?}", e))
        }
    })
    .await;

    Ok(())
}
