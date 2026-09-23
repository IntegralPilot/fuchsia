// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_async::Task;
use futures_util::future::FutureExt;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::low_level::pipe;
use tokio::io::AsyncReadExt;
use tokio::net::UnixStream;

use crate::debug_agent::DebugAgentSocket;

pub fn spawn_forward_task(socket: DebugAgentSocket) -> Task<()> {
    fuchsia_async::Task::local(async move {
        let _ = socket.forward_one_connection().await.map_err(|e| {
            eprintln!("Connection to debug_agent broken: {}", e);
        });
    })
}

pub async fn forward_to_agent(socket: DebugAgentSocket) -> Result<()> {
    // We have to construct these Async objects ourselves instead of using
    // async_net::UnixStream to force the use of std::os::unix::UnixStream,
    // which implements IntoRawFd - a requirement for the pipe::register
    // calls below.
    let (mut sigterm_receiver, sigterm_sender) = UnixStream::pair()?;
    let (mut sigint_receiver, sigint_sender) = UnixStream::pair()?;

    // Note: This does not remove the non-blocking nature of Async from the
    // UnixStream objects or file descriptors.
    pipe::register(SIGTERM, sigterm_sender.into_std()?)?;
    pipe::register(SIGINT, sigint_sender.into_std()?)?;

    let _forward_task = fuchsia_async::Task::local(async move {
        loop {
            // Here we always want to spawn a new DebugAgent
            let _ = socket.forward_one_connection().await.map_err(|e| {
                eprintln!("Connection to debug_agent broken: {}", e);
            });
        }
    });

    let mut sigterm_buf = [0u8; 4];
    let mut sigint_buf = [0u8; 4];

    futures::select! {
        res = sigterm_receiver.read(&mut sigterm_buf).fuse() => res?,
        res = sigint_receiver.read(&mut sigint_buf).fuse() => res?,
    };

    Ok(())
}
