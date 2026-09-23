// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! DHCP client binary for Fuchsia. Serves fuchsia.net.dhcp/ClientProvider.
//!
//! Integration tests (which comprise the test coverage for the FIDL API surface
//! of this component) live at
//! //src/connectivity/network/tests/integration/dhcp-client.

mod client;
mod inspect;
mod packetsocket;
mod provider;
mod udpsocket;

use fidl_fuchsia_net_dhcp::{ClientProviderMarker, ClientProviderRequestStream};
use fidl_fuchsia_posix_socket_packet as fpacket;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use futures::{StreamExt as _, TryStreamExt as _, future};

use anyhow::Error;

enum IncomingService {
    ClientProvider(ClientProviderRequestStream),
}

#[fuchsia::main()]
pub async fn main() {
    log::info!("starting");

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
            .expect("publish Inspect task");
    let clients_inspect_root = inspector.root().create_child("Clients");
    let clients_inspect_root = &clients_inspect_root;

    let mut fs = ServiceFs::new_local();
    let _: &mut ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(IncomingService::ClientProvider);
    let _: &mut ServiceFs<_> =
        fs.take_and_serve_directory_handle().expect("failed to serve directory handle");

    fs.then(future::ok::<_, Error>)
        .try_for_each_concurrent(None, |request| async {
            match request {
                IncomingService::ClientProvider(client_provider_request_stream) => {
                    provider::serve_client_provider(
                        client_provider_request_stream,
                        fuchsia_component::client::connect_to_protocol::<fpacket::ProviderMarker>()
                            .unwrap_or_else(|e| {
                                panic!("error {e:?} while connecting to {}",
                                    <fpacket::ProviderMarker as fidl::endpoints::ProtocolMarker>
                                    ::DEBUG_NAME)
                            }),
                        clients_inspect_root,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        log::error!(
                            "error while serving {}: {e:?}",
                            <ClientProviderMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME
                        );
                    });
                    Ok(())
                }
            }
        })
        .await
        .unwrap_or_else(|e| panic!("error while serving outgoing directory: {e:?}"))
}
