// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fdomain_transport::FDomainTransport;
use crate::target_connector::{
    ConnectionStreamError, TargetConnection, TargetConnectionError, TargetConnector,
};
use crate::{ConnectionError, FDomainConnection};
use anyhow::Result;
use async_channel::Receiver;
use ffx_diagnostics_analytics::ResultExt;
use ffx_ssh::parse::HostAddr;
use fuchsia_async::{Task, Timer};
use futures::FutureExt;
use futures_lite::stream::StreamExt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Represents a FIDL-piping connection to a Fuchsia target.
///
/// To connect a FIDL pipe, the user must know the IP address of the Fuchsia target being connected
/// to. This may or may not include a port number.
#[derive(Debug)]
pub struct FidlPipe {
    task: Option<Task<()>>,
    error_queue: Receiver<ConnectionStreamError>,
    // Using atomic bool because turning `Task<()>` into a fused future didn't appear to behave as
    // intended, e.g. `terminated()` did not return `true` even if the task had run to completion.
    is_terminated: Arc<AtomicBool>,
    device_address: Option<SocketAddr>,
    host_ssh_address: Option<HostAddr>,
}

pub fn create_overnet_socket(
    node: Arc<overnet_core::Router>,
) -> Result<fidl::AsyncSocket, io::Error> {
    let (local_socket, remote_socket) = fidl::Socket::create_stream();
    let local_socket = fidl::AsyncSocket::from_socket(local_socket);
    let (errors_sender, errors) = futures::channel::mpsc::unbounded();
    Task::spawn(
        futures::future::join(
            async move {
                if let Err(e) = async move {
                    let (mut rx, mut tx) = futures::AsyncReadExt::split(
                        fuchsia_async::Socket::from_socket(remote_socket),
                    );
                    circuit::multi_stream::multi_stream_node_connection_to_async(
                        node.circuit_node(),
                        &mut rx,
                        &mut tx,
                        false,
                        circuit::Quality::LOCAL_SOCKET,
                        errors_sender,
                        "remote_control_runner".to_owned(),
                    )
                    .await?;
                    Result::<(), anyhow::Error>::Ok(())
                }
                .await
                {
                    log::debug!("FIDL pipe circuit closed: {:?}", e);
                }
            },
            errors
                .map(|e| {
                    log::warn!("A FIDL pipe circuit stream failed: {e:?}");
                })
                .collect::<()>(),
        )
        .map(|((), ())| ()),
    )
    .detach();

    Ok(local_socket)
}

impl FidlPipe {
    /// This is the main internal constructor for FIDL pipe. This will take a connector and attempt
    /// to connect to it. If the connector fails, this function will attempt to reconnect as long
    /// as the error is not fatal (using an exponential backoff of `t**(2n)` where `t` is the
    /// initial time and `n` is the number of iterations). Any error returned from this function,
    /// therefore, should be interpreted as a non-recoverable error.
    pub(crate) async fn start_internal<C>(
        mut connector: C,
    ) -> Result<(Self, Option<Arc<overnet_core::Router>>, Option<Arc<fdomain_client::Client>>)>
    where
        C: TargetConnector + 'static,
    {
        let mut wait_duration = Duration::from_millis(50);

        let (overnet_connection, fdomain_connection) = loop {
            let error = match connector
                .connect()
                .await
                .or_else_analytics(|e| {
                    crate::analytics::PointOfFailure::TargetConnectorFailure {
                        connection_type: C::CONNECTION_TYPE,
                        error: &e,
                    }
                    .into()
                })
                .await
            {
                Ok(TargetConnection::Overnet(o)) => break (Some(o), None),
                Ok(TargetConnection::FDomain(f)) => break (None, Some(f)),
                Err(TargetConnectionError::Fatal(e)) => return Err(e),
                Err(TargetConnectionError::NonFatal(e)) => e,
            };

            let error = format!(
                "non-fatal error connecting to device. Retrying again after {wait_duration:?}: {error:?}"
            );
            log::debug!("{}", error);
            // TODO(b/421014246): We should have a more effective way to report non-fatal
            // connectivity errors to the user. This currently only affects a small number of tools,
            // but as more adopt directly connecting to the device for diagnostics this will
            // undoubtedly create more noise than necessary.
            eprintln!("{}", error);
            Timer::new(wait_duration).await;
            wait_duration *= 2;
        };

        let device_address = connector.device_address();
        let (error_sender, error_queue) = async_channel::unbounded();

        let (node, overnet_task) = if let Some(overnet_connection) = overnet_connection {
            let node = overnet_core::Router::new(None)?;
            let socket = create_overnet_socket(node.clone())
                .map_err(|e| ConnectionError::InternalError(e.into()))?;
            let error_sender = error_sender.clone();
            let (overnet_reader, overnet_writer) = tokio::io::split(socket);
            (
                Some(node),
                Some(async move {
                    overnet_connection.run(overnet_writer, overnet_reader, error_sender).await;
                }),
            )
        } else {
            (None, None)
        };

        let (client, fdomain_task) = if let Some(fdomain_connection) = fdomain_connection {
            let FDomainConnection { output, input, errors, main_task } = fdomain_connection;
            let error_send = async move {
                let mut errors = std::pin::pin!(errors);
                while let Some(error) = errors.next().await {
                    let Ok(()) = error_sender.send(error).await else { break };
                }
            };

            let (client, fut) = fdomain_client::Client::new(FDomainTransport::new(output, input));
            (
                Some(client),
                Some(async move {
                    futures::future::join(fut, error_send).await;
                    // Explicit drop to own the task.
                    drop(main_task);
                }),
            )
        } else {
            (None, None)
        };

        let is_terminated = Arc::new(AtomicBool::new(false));
        let is_terminated_clone = Arc::clone(&is_terminated);
        let main_task = match (overnet_task, fdomain_task) {
            (None, None) | (Some(_), Some(_)) => unreachable!(),
            (Some(o), None) => Task::local(async move {
                o.await;
                is_terminated_clone.store(true, Ordering::Release);
                // Explicit drop to force the struct into the closure.
                drop(connector);
            }),
            (None, Some(f)) => Task::local(async move {
                f.await;
                is_terminated_clone.store(true, Ordering::Release);
                // Explicit drop to force the struct into the closure.
                drop(connector);
            }),
        };
        Ok((
            Self {
                task: Some(main_task),
                error_queue,
                device_address,
                host_ssh_address: None,
                is_terminated,
            },
            node,
            client,
        ))
    }

    pub fn error_stream(&self) -> Receiver<ConnectionStreamError> {
        return self.error_queue.clone();
    }

    pub fn try_drain_errors(&self) -> Option<Vec<ConnectionStreamError>> {
        let mut pipe_errors = Vec::new();
        while let Ok(err) = self.error_queue.try_recv() {
            pipe_errors.push(err)
        }
        if pipe_errors.is_empty() {
            return None;
        }
        Some(pipe_errors)
    }

    pub fn device_address(&self) -> Option<SocketAddr> {
        self.device_address.clone()
    }

    pub fn host_ssh_address(&self) -> Option<HostAddr> {
        self.host_ssh_address.clone()
    }

    pub fn fake(device_address: Option<SocketAddr>) -> Self {
        Self::fake_with_host_ssh_address(device_address, None)
    }

    pub fn fake_with_host_ssh_address(
        device_address: Option<SocketAddr>,
        host_ssh_address: Option<HostAddr>,
    ) -> Self {
        let (_error_sender, error_queue) = async_channel::unbounded();
        Self {
            task: None,
            error_queue,
            device_address,
            host_ssh_address,
            is_terminated: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_terminated(&self) -> bool {
        self.is_terminated.load(Ordering::Acquire)
    }
}

impl Drop for FidlPipe {
    fn drop(&mut self) {
        self.error_queue.close();
        drop(self.task.take());
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::target_connector::{FDomainConnection, TargetConnection};
    use std::fmt::Debug;
    use tokio::io::BufReader;

    #[derive(Debug)]
    struct FailOnceThenSucceedConnector {
        should_succeed: bool,
    }

    impl TargetConnector for FailOnceThenSucceedConnector {
        const CONNECTION_TYPE: &'static str = "fake";

        async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
            if !self.should_succeed {
                self.should_succeed = true;
                return Err(TargetConnectionError::NonFatal(anyhow::anyhow!("test error")));
            }
            let (sock1, sock2) = fidl::Socket::create_stream();
            let sock1 = fidl::AsyncSocket::from_socket(sock1);
            let sock2 = fidl::AsyncSocket::from_socket(sock2);
            let (_error_tx, error_rx) = async_channel::unbounded();
            let error_task = Task::local(async move {});
            let fdomain = FDomainConnection {
                output: Box::new(BufReader::new(sock1)),
                input: Box::new(sock2),
                errors: error_rx,
                main_task: Some(error_task),
            };

            Ok(TargetConnection::FDomain(fdomain))
        }
    }

    #[derive(Debug)]
    struct AutoFailConnector;

    impl TargetConnector for AutoFailConnector {
        const CONNECTION_TYPE: &'static str = "fake";
        async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
            let (sock1, sock2) = fidl::Socket::create_stream();
            let sock1 = fidl::AsyncSocket::from_socket(sock1);
            let sock2 = fidl::AsyncSocket::from_socket(sock2);
            let (error_tx, error_rx) = async_channel::unbounded();
            let error_task = Task::local(async move {
                let _ =
                    error_tx.send(ConnectionStreamError::Forwarded(anyhow::anyhow!("boom"))).await;
            });
            let fdomain = FDomainConnection {
                output: Box::new(BufReader::new(sock1)),
                input: Box::new(sock2),
                errors: error_rx,
                main_task: Some(error_task),
            };

            Ok(TargetConnection::FDomain(fdomain))
        }
    }

    #[derive(Debug)]
    struct DoNothingConnector;

    impl TargetConnector for DoNothingConnector {
        const CONNECTION_TYPE: &'static str = "fake";
        async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
            let (sock1, sock2) = fidl::Socket::create_stream();
            let sock1 = fidl::AsyncSocket::from_socket(sock1);
            let sock2 = fidl::AsyncSocket::from_socket(sock2);
            let (_error_tx, error_rx) = async_channel::unbounded();
            let error_task = Task::local(async move {});
            let fdomain = FDomainConnection {
                output: Box::new(BufReader::new(sock1)),
                input: Box::new(sock2),
                errors: error_rx,
                main_task: Some(error_task),
            };

            Ok(TargetConnection::FDomain(fdomain))
        }
    }

    #[fuchsia::test]
    async fn test_error_queue() {
        // These sockets will do nothing of import.
        let (fidl_pipe, _node, _client) =
            FidlPipe::start_internal(AutoFailConnector).await.unwrap();
        let mut errors = std::pin::pin!(fidl_pipe.error_stream());
        let err = errors.next().await.unwrap();
        assert_eq!(anyhow::anyhow!("boom").to_string(), err.to_string());
    }

    #[fuchsia::test]
    async fn test_retry() {
        let (fidl_pipe, _node, _client) =
            FidlPipe::start_internal(FailOnceThenSucceedConnector { should_succeed: false })
                .await
                .unwrap();
        let errs = fidl_pipe.try_drain_errors();
        assert!(errs.is_none());
    }

    #[fuchsia::test]
    async fn test_empty_error_queue() {
        let (fidl_pipe, _node, _client) =
            FidlPipe::start_internal(DoNothingConnector).await.unwrap();
        // So, this DoNothingConnector is going to ensure no errors are placed onto the queue,
        // however, even if AutoFailConnector was used here it still wouldn't work, since there is
        // no polling happening between the creation of FidlPipe and the attempt to drain errors
        // off of the queue.
        let errs = fidl_pipe.try_drain_errors();
        assert!(errs.is_none());
    }
}
