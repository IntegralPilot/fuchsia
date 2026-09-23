// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_sync::RwLock;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _};
use std::convert::Infallible;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use sl4f_lib::server::sl4f::{Sl4f, Sl4fClients, serve};
use sl4f_lib::server::sl4f_executor::run_fidl_loop;

// Config, flexible for any ip/port combination
const SERVER_PORT: u16 = 80;

#[fuchsia::main(logging_tags = ["sl4f"])]
async fn main() {
    log::info!("  Starting sl4f server");

    // State for clients that utilize the /init endpoint
    let sl4f_clients = Arc::new(RwLock::new(Sl4fClients::new()));

    // State for facades
    let sl4f = Sl4f::new(Arc::clone(&sl4f_clients)).expect("failed to create SL4F");
    let sl4f = Arc::new(sl4f);

    let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), SERVER_PORT);
    log::info!("Now listening on: {:?}", addr);
    let listener = fasync::net::TcpListener::bind(&addr).expect("bind");
    let listener = listener
        .accept_stream()
        .map_ok(|(stream, _): (_, SocketAddr)| fuchsia_hyper::TcpStream { stream });

    // Create channel for communication between http server and FIDL. This once bridged a sync/async
    // gap, but no longer does. It would be good to refactor this away.
    let (sender, async_receiver) = async_channel::unbounded();

    let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);
    let mut tasks = futures::stream::FuturesUnordered::new();
    let listener = listener.fuse();
    let fidl_loop = run_fidl_loop(sl4f, async_receiver).fuse();
    futures::pin_mut!(listener, fidl_loop);

    loop {
        futures::select! {
            stream_res = listener.next() => {
                match stream_res {
                    Some(Ok(stream)) => {
                        let sender = sender.clone();
                        let sl4f_clients = Arc::clone(&sl4f_clients);
                        let builder = builder.clone();
                        tasks.push(fasync::Task::spawn(async move {
                            if let Err(e) = builder
                                .serve_connection(
                                    hyper_util::rt::TokioIo::new(stream),
                                    hyper::service::service_fn(move |request| {
                                        serve(request, Arc::clone(&sl4f_clients), sender.clone()).map(Ok::<_, Infallible>)
                                    }),
                                )
                                .await
                            {
                                log::error!("Error serving connection: {:?}", e);
                            }
                        }));
                    }
                    Some(Err(e)) => log::error!("Error accepting connection: {:?}", e),
                    None => break,
                }
            }
            _ = tasks.next() => {}
            () = fidl_loop => panic!("FIDL handler died"),
        }
    }
}
