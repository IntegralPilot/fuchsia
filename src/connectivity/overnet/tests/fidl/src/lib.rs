// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use circuit::multi_stream::multi_stream_node_connection;
use circuit::stream::stream;
use fidl::NullableHandle;
use fuchsia_async::Task;
use futures::prelude::*;
use overnet_core::{NodeId, NodeIdGenerator, Router, log_errors};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

mod channel;
mod socket;

struct Fixture {
    dist_a_to_b: fidl::Channel,
    dist_b: fidl::AsyncChannel,
    dist_a_to_c: fidl::Channel,
    dist_c: fidl::AsyncChannel,
    test_name: String,
    _service_task: Task<()>,
}

async fn link(a: Arc<Router>, b: Arc<Router>) {
    let a = a.circuit_node();
    let b = b.circuit_node();
    let (a_reader, b_writer) = stream();
    let (b_reader, a_writer) = stream();
    let (error_sink, _) = futures::channel::mpsc::unbounded();
    let a = multi_stream_node_connection(
        a,
        a_reader,
        a_writer,
        true,
        circuit::Quality::IN_PROCESS,
        error_sink.clone(),
        "b".to_string(),
    );
    let b = multi_stream_node_connection(
        b,
        b_reader,
        b_writer,
        false,
        circuit::Quality::IN_PROCESS,
        error_sink,
        "a".to_string(),
    );
    if let Err(error) = futures::try_join!(a, b) {
        log::warn!(error:?; "Link forward returned an error");
    }
}

#[derive(Clone, Copy, Debug)]
enum Target {
    A,
    B,
    C,
}

const FIXTURE_INCREMENT: u64 = 100000;
static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(100 + FIXTURE_INCREMENT);

impl Fixture {
    async fn new(mut node_id_gen: NodeIdGenerator) -> Fixture {
        let test_name = node_id_gen.test_desc();
        let fixture_id = NEXT_FIXTURE_ID.fetch_add(FIXTURE_INCREMENT, Ordering::Relaxed);
        let router1 = node_id_gen.new_router().unwrap();
        let router2 = node_id_gen.new_router().unwrap();
        let router3 = node_id_gen.new_router().unwrap();
        let l1 = link(router1.clone(), router2.clone());
        let l2 = link(router2.clone(), router3.clone());
        let l3 = link(router3.clone(), router1.clone());
        let service_task = Task::spawn(futures::future::join3(l1, l2, l3).map(drop));
        let service = format!("distribute_handle_for_{}", test_name);
        let (send_handle, mut recv_handle) = futures::channel::mpsc::channel(1);
        log::info!(test_name:%, fixture_id:%; "register 2");
        let spawn_service = &|chan,
                              mut sender: futures::channel::mpsc::Sender<fidl::Channel>,
                              test_name: String| {
            log::info!(test_name:%; "got connection {:?}", chan);
            Task::spawn(log_errors(
                {
                    let test_name = test_name.clone();
                    async move {
                        log::info!(test_name:%; "sending the thing");
                        sender.send(chan).await?;
                        log::info!(test_name:%; "sent the thing");
                        Ok(())
                    }
                },
                format!("{test_name} failed to send incoming request handle"),
            ))
            .detach();
            Ok(())
        };
        router2
            .register_service(service.clone(), {
                let sender = send_handle.clone();
                let test_name = test_name.clone();
                move |chan| spawn_service(chan, sender.clone(), test_name.clone())
            })
            .await
            .unwrap();
        log::info!(test_name:%, fixture_id:%; "register 3");
        router3
            .register_service(service.clone(), {
                let sender = send_handle.clone();
                let test_name = test_name.clone();
                move |chan| spawn_service(chan, sender.clone(), test_name.clone())
            })
            .await
            .unwrap();
        // Wait til we can see both peers in the service map before progressing.
        let lpc = router1.new_list_peers_context().await;
        loop {
            let peers = lpc.list_peers().await.unwrap();
            let has_peer = |node_id: NodeId| {
                peers
                    .iter()
                    .find(|peer| {
                        node_id == peer.node_id
                            && peer.services.iter().find(|&s| *s == service).is_some()
                    })
                    .is_some()
            };
            if has_peer(router2.node_id()) && has_peer(router3.node_id()) {
                break;
            }
        }
        let (dist_a_to_b, dist_b) = fidl::Channel::create();
        let (dist_a_to_c, dist_c) = fidl::Channel::create();
        log::info!(test_name:%, fixture_id:%; "connect 2");
        router1.connect_to_service(router2.node_id(), &service, dist_b).await.unwrap();
        log::info!(test_name:%, fixture_id:%; "get 2");
        let dist_b = recv_handle.next().await.unwrap();
        log::info!(test_name:%, fixture_id:%; "connect 3");
        router1.connect_to_service(router3.node_id(), &service, dist_c).await.unwrap();
        log::info!(test_name:%, fixture_id:%; "get 3");
        let dist_c = recv_handle.next().await.unwrap();
        let dist_b = fidl::AsyncChannel::from_channel(dist_b);
        let dist_c = fidl::AsyncChannel::from_channel(dist_c);
        Fixture { dist_a_to_b, dist_b, dist_a_to_c, dist_c, test_name, _service_task: service_task }
    }

    async fn distribute_handle<H: Into<NullableHandle> + From<NullableHandle>>(
        &self,
        h: H,
        target: Target,
    ) -> H {
        let h: NullableHandle = h.into();
        log::info!(test_name:% = self.test_name; "distribute_handle: make {:?} on {:?}", h, target);
        let (dist_local, dist_remote) = match target {
            Target::A => return H::from(h),
            Target::B => (&self.dist_a_to_b, &self.dist_b),
            Target::C => (&self.dist_a_to_c, &self.dist_c),
        };
        assert!(dist_local.write(&[], &mut vec![h.into()]) == Ok(()));
        let mut msg = fidl::MessageBufEtc::new();
        dist_remote.recv_etc_msg(&mut msg).await.unwrap();
        let (bytes, handles) = msg.split_mut();
        assert_eq!(bytes.len(), 0);
        assert_eq!(handles.len(), 1);
        let h = std::mem::replace(handles, vec![]).into_iter().next().unwrap();
        log::info!(test_name:% = self.test_name; "distribute_handle: remote is {:?}", h);
        let handle: NullableHandle = h.handle.into();
        return H::from(handle);
    }

    pub fn log(&mut self, msg: &str) {
        log::info!(test_name:% = self.test_name; "{}", msg);
    }
}
