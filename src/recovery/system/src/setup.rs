// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fuchsia_async as fasync;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};

const SERVER_PORT: u16 = 8880;

pub type Body = Full<hyper::body::Bytes>;

pub enum SetupEvent {
    Root,
    DevhostOta { cfg: DevhostConfig },
}

/// Devhost configuration, passed to the actual OTA process.
pub struct DevhostConfig {
    pub url: String,
}

#[derive(Deserialize, Serialize)]
/// Configuration provided by the host for the devhost OTA. Only used for de/serialization.
struct DevhostRequestInfo {
    /// We assume that the OTA server is running on the requester's address
    /// at the given port.
    pub port: u16,
}

async fn parse_ota_json(
    request: Request<hyper::body::Incoming>,
    remote_addr: IpAddr,
) -> Result<DevhostConfig, Error> {
    let body = request.into_body().collect().await.context("read request")?.to_bytes();
    let DevhostRequestInfo { port } =
        serde_json::from_slice(&body).context("Failed to parse JSON")?;

    let url = format!("http://{}/config.json", SocketAddr::new(remote_addr, port));
    Ok(DevhostConfig { url })
}

async fn serve<Fut, F>(
    request: Request<hyper::body::Incoming>,
    remote_addr: SocketAddr,
    handler: F,
) -> Response<Body>
where
    Fut: Future<Output = ()>,
    F: FnOnce(SetupEvent) -> Fut,
{
    use hyper::{Method, StatusCode};

    match (request.method(), request.uri().path()) {
        (&Method::GET, "/") => {
            let () = handler(SetupEvent::Root).await;
            Response::new(Full::new("Root document".into()))
        }
        (&Method::POST, "/ota/devhost") => {
            // get devhost info out of POST request.
            match parse_ota_json(request, remote_addr.ip()).await {
                Err(e) => {
                    let mut response =
                        Response::new(Full::new(format!("Bad request: {:?}", e).into()));
                    *response.status_mut() = StatusCode::BAD_REQUEST;
                    response
                }
                Ok(cfg) => {
                    let () = handler(SetupEvent::DevhostOta { cfg }).await;
                    Response::new(Full::new("Started OTA".into()))
                }
            }
        }
        _ => {
            let mut response = Response::new(Full::new("Unknown command".into()));
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    }
}

pub fn start_server<Fut, F>(handler: F) -> impl Future<Output = Result<(), hyper::Error>>
where
    Fut: Future<Output = ()>,
    F: FnOnce(SetupEvent) -> Fut,
    Fut: Send + 'static,
    F: Clone + Send + 'static,
{
    use futures::{FutureExt as _, StreamExt as _};
    use hyper::service::service_fn;
    use hyper_util::rt::TokioIo;

    println!("recovery: start_server");

    let addr = SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), SERVER_PORT);
    let listener = fasync::net::TcpListener::bind(&addr).expect("bind");
    let listener = listener.accept_stream();

    let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);

    async move {
        let mut listener = listener.fuse();
        let mut tasks = futures::stream::FuturesUnordered::new();
        loop {
            futures::select! {
                res = listener.next() => {
                    match res {
                        Some(Ok((stream, remote_addr))) => {
                            let handler = handler.clone();
                            let service = service_fn(move |request| {
                                let handler = handler.clone();
                                serve(request, remote_addr, handler).map(Ok::<_, Infallible>)
                            });
                            let builder = builder.clone();
                            let stream = fuchsia_hyper::TcpStream { stream };
                            tasks.push(fasync::Task::spawn(async move {
                                if let Err(e) = builder.serve_connection(TokioIo::new(stream), service).await {
                                    println!("recovery server connection error: {e}");
                                }
                            }));
                        }
                        Some(Err(e)) => {
                            println!("recovery accept error: {e}");
                        }
                        None => break,
                    }
                }
                _ = tasks.next() => {}
            }
        }
        while let Some(_) = tasks.next().await {}
        Ok(())
    }
}
