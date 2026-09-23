// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use anyhow::Context as _;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::stream::{StreamExt as _, TryStreamExt as _};
use log::{debug, error};

async fn handle_provider(
    delegated_networks: Arc<fnp_socketproxy::NetworkRegistryProxy>,
    rs: fnp_socketproxy::NetworkRegistryRequestStream,
) -> Result<(), anyhow::Error> {
    rs.map(|r| r.context("fidl error"))
        .try_for_each(|req| {
            let delegated_networks = delegated_networks.clone();
            async move {
                match req {
                    fnp_socketproxy::NetworkRegistryRequest::SetDefault {
                        network_id,
                        responder,
                    } => {
                        responder.send(delegated_networks.set_default(&network_id).await?)?;
                    }
                    fnp_socketproxy::NetworkRegistryRequest::Add { network, responder } => {
                        responder.send(delegated_networks.add(&network).await?)?;
                    }
                    fnp_socketproxy::NetworkRegistryRequest::Update { network, responder } => {
                        responder.send(delegated_networks.update(&network).await?)?;
                    }
                    fnp_socketproxy::NetworkRegistryRequest::Remove { network_id, responder } => {
                        responder.send(delegated_networks.remove(network_id).await?)?;
                    }
                }
                Ok(())
            }
        })
        .await
}

enum IncomingServices {
    NetworkRegistry(fnp_socketproxy::NetworkRegistryRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    debug!("Starting fake-socket-proxy");

    let mut fs = ServiceFs::new_local();
    let delegated_networks = Arc::new(
        connect_to_protocol::<fnp_socketproxy::NetworkRegistryMarker>()
            .expect("can't connect to NetworkRegistry"),
    );
    let _ = fs.dir("svc").add_fidl_service(IncomingServices::NetworkRegistry);

    let _ = fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(10, |request| {
        let delegated_networks = delegated_networks.clone();
        async {
            if let Err(e) = match request {
                IncomingServices::NetworkRegistry(rs) => {
                    handle_provider(delegated_networks, rs).await.context("network registry")
                }
            } {
                error!("{e:?}")
            }
        }
    })
    .await;

    Ok(())
}
