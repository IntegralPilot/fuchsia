// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::{NonZeroU16, NonZeroUsize};

use ip_test_macro::ip_test;
use loom::sync::Arc;
use net_types::ZonedAddr;
use netstack3_core::device::LoopbackDevice;
use netstack3_core::socket::ShutdownType;
use netstack3_core::testutil::{CtxPairExt as _, FakeBindingsCtx, FakeCtx};
use netstack3_core::types::WorkQueueReport;
use netstack3_core::{CtxPair, IpExt};
use netstack3_tcp::testutil::{ProvidedBuffers, WriteBackClientBuffers};
use test_case::test_matrix;

use super::{loom_model, loom_spawn, low_preemption_bound_model};

#[derive(Debug, Copy, Clone)]
enum ServerOrClient {
    Server,
    Client,
}

#[derive(Debug, Copy, Clone)]
enum CloseOrShutdown {
    Close,
    Shutdown,
}

#[netstack3_core::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
#[test_matrix(
    [ServerOrClient::Server, ServerOrClient::Client],
    [CloseOrShutdown::Close, CloseOrShutdown::Shutdown]
)]
fn race_connect_close<I: IpExt>(which: ServerOrClient, close_or_shutdown: CloseOrShutdown) {
    loom_model(low_preemption_bound_model(), move || {
        const SERVER_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
        const BACKLOG: NonZeroUsize = NonZeroUsize::new(1).unwrap();
        let FakeCtx { core_ctx, bindings_ctx } = FakeCtx::default();
        let mut ctx = CtxPair { core_ctx: Arc::new(core_ctx), bindings_ctx };
        let lo = ctx.test_api().add_loopback();
        let mut tcp_api = ctx.core_api().tcp::<I>();
        let server = tcp_api.create(ProvidedBuffers::Buffers(WriteBackClientBuffers::default()));

        tcp_api.bind(&server, None, Some(SERVER_PORT)).unwrap();
        tcp_api.listen(&server, BACKLOG).unwrap();
        let client = tcp_api.create(ProvidedBuffers::Buffers(WriteBackClientBuffers::default()));
        tcp_api
            .connect(&client, ZonedAddr::Unzoned(I::LOOPBACK_ADDRESS).into(), SERVER_PORT)
            .unwrap();

        // The client's initial SYN is sitting in the loopback rx queue.
        //
        // Race two operations:
        //
        // 1. Closing or shutting down one of the sockets.
        // 2. Operating the loopback queue, which will advance the server
        //    state-machine and potentially send a SYN-ACK back.

        let (action_socket, other_socket) = match which {
            ServerOrClient::Server => (server, client),
            ServerOrClient::Client => (client, server),
        };

        let (racy_action, cleanup): (Box<dyn FnOnce() + Send + Sync>, Box<dyn FnOnce()>) =
            match close_or_shutdown {
                CloseOrShutdown::Close => {
                    let thread_vars = (ctx.clone(), action_socket);
                    let racy = move || {
                        let (mut ctx, socket) = thread_vars;
                        ctx.core_api().tcp::<I>().close(socket);
                    };
                    let mut ctx = ctx.clone();
                    let cleanup = move || {
                        ctx.core_api().tcp::<I>().close(other_socket);
                    };
                    (Box::new(racy), Box::new(cleanup))
                }
                CloseOrShutdown::Shutdown => {
                    let thread_vars = (ctx.clone(), action_socket.clone());
                    let racy = move || {
                        let (mut ctx, socket) = thread_vars;
                        // The return of shutdown is different for listening and
                        // connected sockets.
                        let expect_shutdown = match which {
                            ServerOrClient::Server => false,
                            ServerOrClient::Client => true,
                        };
                        assert_eq!(
                            ctx.core_api()
                                .tcp::<I>()
                                .shutdown(&socket, ShutdownType::SendAndReceive),
                            Ok(expect_shutdown)
                        );
                    };
                    let mut ctx = ctx.clone();
                    let cleanup = move || {
                        ctx.core_api().tcp::<I>().close(action_socket);
                        ctx.core_api().tcp::<I>().close(other_socket);
                    };
                    (Box::new(racy), Box::new(cleanup))
                }
            };

        let t_action = loom_spawn(racy_action);
        let thread_vars = (ctx.clone(), lo.clone());
        let t_recv = loom_spawn(move || {
            let (mut ctx, lo) = thread_vars;

            // Run the loopback queue for as long as we observe rx available
            // signals in the bindings context.
            while !core::mem::take(&mut ctx.bindings_ctx.state_mut().rx_available).is_empty() {
                assert_eq!(
                    ctx.core_api().receive_queue::<LoopbackDevice>().handle_queued_frames(&lo),
                    WorkQueueReport::AllDone
                );
            }
        });

        t_action.join().unwrap();
        t_recv.join().unwrap();

        // Clean up all resources.
        cleanup();
        {
            let mut state = ctx.bindings_ctx.state_mut();
            state.rx_available.clear();
            // Ensure that all the deferred resource removals have completed.
            state.deferred_receivers.iter().for_each(|r| r.assert_signalled());
        }
        ctx.test_api().clear_routes_and_remove_device(lo);
    })
}
