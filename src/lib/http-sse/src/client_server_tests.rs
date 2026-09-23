// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use super::*;
use assert_matches::assert_matches;
use fuchsia_async::net::TcpListener;
use fuchsia_async::{self as fasync};
use fuchsia_hyper::new_https_client;
use futures::future::join;
use futures::stream::StreamExt;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

fn spawn_server(buffer_size: usize) -> (String, EventSender) {
    let (listener, url) = {
        let listener = TcpListener::bind(&SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)).unwrap();
        let local_addr = listener.local_addr().unwrap();
        (listener.accept_stream(), format!("http://{}", local_addr))
    };
    let (sse_response_creator, event_sender) =
        SseResponseCreator::with_additional_buffer_size(buffer_size);
    let sse_response_creator = Arc::new(sse_response_creator);
    let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);

    fasync::Task::spawn(async move {
        let mut tasks = futures::stream::FuturesUnordered::new();
        let mut listener = listener.fuse();
        loop {
            futures::select! {
                res = listener.next() => {
                    match res {
                        Some(Ok((stream, _addr))) => {
                            let stream = fuchsia_hyper::TcpStream { stream };
                            let sse_response_creator = Arc::clone(&sse_response_creator);
                            let service = service_fn(move |_req| {
                                let sse_response_creator = Arc::clone(&sse_response_creator);
                                async move { Ok::<_, std::convert::Infallible>(sse_response_creator.create().await) }
                            });
                            let builder = builder.clone();
                            tasks.push(fasync::Task::spawn(async move {
                                let _ = builder.serve_connection(TokioIo::new(stream), service).await;
                            }));
                        }
                        _ => break,
                    }
                }
                _ = tasks.next() => {}
            }
        }
    }).detach();
    (url, event_sender)
}

#[fasync::run_singlethreaded(test)]
async fn single_client_single_event() {
    let (url, event_sender) = spawn_server(1);
    let mut client = Client::from_hyper_client(&new_https_client(), &url).await.unwrap();
    let event = Event::from_type_and_data("event_type", "event_data").unwrap();

    let (_, recv) = join(event_sender.send(&event), client.next()).await;

    assert_matches!(recv, Some(Ok(e)) if e == event);
}

#[fasync::run_singlethreaded(test)]
async fn multiple_clients_multiple_events() {
    let (url, event_sender) = spawn_server(2);
    let client0 = Client::from_hyper_client(&new_https_client(), &url).await.unwrap();
    let client1 = Client::from_hyper_client(&new_https_client(), &url).await.unwrap();
    let events = vec![
        Event::from_type_and_data("event_type0", "event_data0").unwrap(),
        Event::from_type_and_data("event_type1", "event_data1").unwrap(),
    ];

    for event in events.iter() {
        event_sender.send(event).await;
    }
    let client0_events = client0.take(2).collect::<Vec<_>>();
    let client1_events = client1.take(2).collect::<Vec<_>>();
    let (client0_events, client1_events) = join(client0_events, client1_events).await;

    assert_eq!(client0_events.into_iter().map(|r| r.unwrap()).collect::<Vec<_>>(), events);
    assert_eq!(client1_events.into_iter().map(|r| r.unwrap()).collect::<Vec<_>>(), events);
}
