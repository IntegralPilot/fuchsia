// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_log::PublishOptions;
use fidl::prelude::*;
use fuchsia_component::server::ServiceFs;
use futures::channel::mpsc::unbounded;
use futures::join;
use futures::prelude::*;
use log::{debug, error, info};
use remote_control::{ConnectionRequest, RemoteControlService, http};
use remote_control_config::Config;
use std::rc::Rc;
use std::sync::Arc;
use {fidl_fuchsia_developer_remotecontrol as rcs, fuchsia_async as fasync};

mod fdomain;
mod vsock;

async fn exec_server(config: &Config) -> Result<(), Error> {
    diagnostics_log::initialize(PublishOptions::default().tags(&["remote-control"]))?;

    let router = overnet_core::Router::new(None)?;

    let connector = {
        let router = Arc::clone(&router);
        move |request, weak_self| match request {
            ConnectionRequest::Overnet(socket, nodeid_sender) => {
                let router = Arc::clone(&router);
                if let Err(e) = nodeid_sender.send(router.node_id().0) {
                    error!("Error sending node id: {e:?}");
                }
                fasync::Task::spawn(async move {
                    let socket = fidl::AsyncSocket::from_socket(socket);
                    let (mut rx, mut tx) = socket.split();
                    let (errors_sender, errors) = unbounded();
                    if let Err(e) = futures::future::join(
                        circuit::multi_stream::multi_stream_node_connection_to_async(
                            router.circuit_node(),
                            &mut rx,
                            &mut tx,
                            true,
                            circuit::Quality::NETWORK,
                            errors_sender,
                            "client".to_owned(),
                        ),
                        errors
                            .map(|e| {
                                log::warn!("A client circuit stream failed: {e:?}");
                            })
                            .collect::<()>(),
                    )
                    .map(|(result, ())| result)
                    .await
                    {
                        if let circuit::Error::ConnectionClosed(msg) = e {
                            debug!("Overnet link closed: {:?}", msg);
                        } else {
                            error!("Error handling Overnet link: {:?}", e);
                        }
                    }
                })
                .detach();
            }
            ConnectionRequest::FDomain(socket) => {
                let socket = fidl::AsyncSocket::from_socket(socket);
                fasync::Task::local(fdomain::serve_fdomain_connection(weak_self, socket)).detach();
            }
        }
    };

    let service = if config.use_default_identity {
        Rc::new(RemoteControlService::new_with_default_allocator(connector).await)
    } else {
        Rc::new(RemoteControlService::new(connector).await)
    };
    let (sender, receiver) = unbounded();

    router
        .register_service(rcs::RemoteControlMarker::PROTOCOL_NAME.to_owned(), move |chan| {
            let _ = sender.unbounded_send(chan);
            Ok(())
        })
        .await?;

    let sc = Rc::clone(&service);
    let onet_fut = receiver.for_each_concurrent(None, move |chan| {
        let chan = fidl::AsyncChannel::from_channel(chan);

        let sc = Rc::clone(&sc);
        sc.serve_stream(rcs::RemoteControlRequestStream::from_channel(chan))
    });

    let weak_router_vsock = Arc::downgrade(&router);
    let weak_service_vsock = Rc::downgrade(&service);
    std::mem::drop(router);
    let vsock_fut = async move {
        // TODO(https://fxbug.dev/296283299): Change this info! to error! Once
        // we can return normally if VSOCK support is disabled
        if let Err(e) = vsock::run_vsocks(weak_router_vsock, weak_service_vsock).await {
            info!("VSOCK serving failed with error {e:?}");
        }
    };
    let sc = service.clone();
    let vsock_ident_fut = async move {
        // TODO(https://fxbug.dev/296283299): Change this info! to error! Once
        // we can return normally if VSOCK support is disabled
        if let Err(e) = vsock::run_identify_vsock(sc).await {
            info!("VSOCK identity serving failed with error {e:?}");
        }
    };

    let sc1 = service.clone();
    let sc2 = service.clone();
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |req| {
        fasync::Task::local(sc1.clone().serve_stream(req)).detach();
    });
    fs.dir("svc").add_fidl_service(move |req| {
        fasync::Task::local(sc2.clone().serve_connector_stream(req)).detach();
    });

    fs.take_and_serve_directory_handle()?;
    let fidl_fut = fs.collect::<()>();

    join!(fidl_fut, onet_fut, vsock_fut, vsock_ident_fut);
    Ok(())
}

#[fasync::run_singlethreaded]
async fn main() -> Result<(), Error> {
    let config = Config::take_from_startup_handle();
    let rcs_fut = exec_server(&config);
    let http_fut = http::run_http_server();
    let (rcs_res, http_res) = join!(rcs_fut, http_fut);
    if let Err(err) = rcs_res {
        error!(err:%; "Error executing rcs server");
    }
    if let Err(err) = http_res {
        error!(err:%; "Error executing http server");
    }
    Ok(())
}
