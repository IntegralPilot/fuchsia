// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fdomain_client::FDomainTransport;
use fdomain_container::FDomain;
use fdomain_container::wire::FDomainCodec;
use fidl::endpoints::{ClientEnd, Proxy, RequestStream};
use fidl_fuchsia_examples as echo;
use futures::future::Either;
use futures::stream::Stream;
use futures::{FutureExt, StreamExt, TryStreamExt};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll};
use vfs::directory::helper::DirectlyMutable;

pub struct LocalFDomainTransport(FDomainCodec);

impl FDomainTransport for LocalFDomainTransport {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        _ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Option<std::io::Error>>> {
        Poll::Ready(self.0.message(msg).map_err(|x| Some(std::io::Error::other(x))))
    }
}

impl Stream for LocalFDomainTransport {
    type Item = std::io::Result<Box<[u8]>>;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.as_mut().0).poll_next(ctx).map_err(std::io::Error::other)
    }
}

async fn echo_server(chan: fidl::Channel, prefix: String, quiet: bool) -> Result<(), Error> {
    let chan = fidl::AsyncChannel::from_channel(chan);
    let mut stream = echo::EchoRequestStream::from_channel(chan);
    while let Some(echo::EchoRequest::EchoString { value, responder }) =
        stream.try_next().await.context("error running echo server")?
    {
        if !quiet {
            log::info!("Received echo request for string {:?}", value);
        }
        responder.send(&format!("{prefix}{value}")).context("error sending response")?;
        if !quiet {
            log::info!("echo response sent successfully");
        }
    }
    Ok(())
}

async fn echo_launcher_server(chan: fidl::AsyncChannel, quiet: bool) -> Result<(), Error> {
    println!("In launcher server");
    let mut stream = echo::EchoLauncherRequestStream::from_channel(chan);
    let (task_sender, tasks) = futures::channel::mpsc::unbounded();

    let mut tasks_fut = pin!(tasks.for_each_concurrent(None, move |task| async move {
        if let Err(e) = task.await {
            if !quiet {
                log::warn!(error:? = e; "Echo server failed");
            }
        }
    }));

    while let Some(request) = futures::future::select(stream.try_next(), tasks_fut.as_mut())
        .map(|x| match x {
            Either::Left((v, _)) => v,
            Either::Right(((), _)) => unreachable!("Task sender disappeared!"),
        })
        .await?
    {
        match request {
            echo::EchoLauncherRequest::GetEcho { responder, echo_prefix } => {
                if !quiet {
                    log::info!(
                        "Received echo launcher request with prefix string {:?}",
                        echo_prefix
                    );
                }

                let (client, server) = fidl::Channel::create();
                task_sender
                    .unbounded_send(echo_server(server, echo_prefix, quiet))
                    .expect("Task future disappeared!");
                responder
                    .send(fidl::endpoints::ClientEnd::new(client))
                    .context("error sending response")?;
                if !quiet {
                    log::info!("echo launcher response sent successfully");
                }
            }
            echo::EchoLauncherRequest::GetEchoPipelined {
                echo_prefix,
                request,
                control_handle: _,
            } => {
                if !quiet {
                    log::info!(
                        "Received pipelined echo launcher request with prefix string {:?}",
                        echo_prefix
                    );
                }

                task_sender
                    .unbounded_send(echo_server(request.into_channel(), echo_prefix, quiet))
                    .expect("Task future disappeared!");
                if !quiet {
                    log::info!("echo launcher pipeline request handled");
                }
            }
        }
    }
    Ok(())
}

pub fn exec_server(quiet: bool) -> LocalFDomainTransport {
    let service = vfs::service::endpoint(move |scope, channel| {
        println!("Spawned endpoint");
        let task = echo_launcher_server(channel, quiet);
        scope.spawn(async move {
            if let Err(e) = task.await {
                log::warn!(error:? = e; "Echo server terminated");
            }
        });
    });
    let namespace = vfs::directory::immutable::simple();
    namespace.add_entry("echo", service).expect("Could not build namespace!");
    LocalFDomainTransport(FDomainCodec::new(FDomain::new(move || {
        println!("Spawning vfs client");
        Ok(ClientEnd::new(
            vfs::directory::serve_read_only(
                Arc::clone(&namespace),
                vfs::execution_scope::ExecutionScope::new(),
            )
            .into_channel()
            .unwrap()
            .into_zx_channel(),
        ))
    })))
}
