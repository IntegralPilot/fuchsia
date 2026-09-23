// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Target related functions used by the repository server.

use anyhow::{Context as _, anyhow};
use camino::Utf8Path;
use fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy;
use fdomain_fuchsia_pkg::{RepositoryManagerMarker, RepositoryManagerProxy};
use fdomain_fuchsia_pkg_rewrite::{EngineMarker, EngineProxy};
use ffx_command_error::Result;
use ffx_config::EnvironmentContext;
use ffx_repository_server_start_args::StartCommand;
use ffx_target::{KnockError, RcsKnocker, TargetInfoQuery};
use ffx_target_net::{SocketProvider, TargetTcpStream};
use fidl_fuchsia_pkg_ext::{
    RepositoryRegistrationAliasConflictMode, RepositoryStorageType, RepositoryTarget,
};
use fuchsia_repo::manager::RepositoryManager;
use fuchsia_repo::server::ConnectionStream;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::{FutureExt, SinkExt, Stream, StreamExt, pin_mut, select};
use pkg::repo;
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;
use timeout::timeout;

const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// Connects to the target and registers the repositories.
///
/// Returns a tuple containing:
/// * `String` - The repository host address as seen by the target.
/// * `impl Stream` - A stream of forwarded TCP connections from the target.
async fn connect_to_target(
    target_spec: Option<String>,
    host_address: Option<String>,
    aliases: Vec<String>,
    storage_type: Option<RepositoryStorageType>,
    repo_server_listen_addr: SocketAddr,
    connect_timeout: std::time::Duration,
    repo_manager: Arc<RepositoryManager>,
    rcs_proxy: &RemoteControlProxy,
    alias_conflict_mode: RepositoryRegistrationAliasConflictMode,
    tunnel_addr: SocketAddr,
    should_register_repo: bool,
) -> Result<(String, impl Stream<Item = anyhow::Result<TargetTcpStream>>), anyhow::Error> {
    let port_forward = SocketProvider::new_with_rcs(connect_timeout, &rcs_proxy)
        .await
        .context("connecting to socket provider protocols")?;

    let (repo_host, forwarding_stream) = repo::create_repo_host_and_listener(
        repo_server_listen_addr,
        host_address,
        &port_forward,
        tunnel_addr,
    )
    .await
    .context("resolving repository host")?;

    if should_register_repo {
        let repo_proxy: RepositoryManagerProxy = rcs::toolbox::connect_with_timeout::<
            RepositoryManagerMarker,
        >(&rcs_proxy, connect_timeout)
        .await
        .context("connecting to repository manager")?;

        let engine_proxy: EngineProxy =
            rcs::toolbox::connect_with_timeout::<EngineMarker>(&rcs_proxy, connect_timeout)
                .await
                .context("binding engine to stream")?;

        for (repo_name, repo) in repo_manager.repositories() {
            let repo_spec = repo.read().await.spec();
            let repo_target = RepositoryTarget {
                repo_name,
                target_identifier: target_spec.clone(),
                aliases: if aliases.is_empty() {
                    Some(repo_spec.aliases().iter().map(ToString::to_string).collect())
                } else {
                    Some(BTreeSet::from_iter(aliases.iter().map(|a| a.clone())))
                },
                storage_type: storage_type.clone(),
            };

            // Construct RepositoryTarget from same args as `ffx target repository register`
            let repo_target_info =
                RepositoryTarget::try_from(repo_target).context("build RepositoryTarget")?;

            repo::register_target_with_fidl_proxies(
                repo_proxy.clone(),
                engine_proxy.clone(),
                &repo_target_info,
                &repo_host,
                &repo,
                alias_conflict_mode.clone(),
            )
            .await
            .context("register repository")?;
        }
    }
    Ok((
        repo_host,
        forwarding_stream
            .map(|s| s.into_stream().map(|r| r.context("target tcp listener")).left_stream())
            .unwrap_or_else(|| futures::stream::pending().right_stream()),
    ))
}

async fn inner_connect_loop(
    ctx: &EnvironmentContext,
    cmd: &StartCommand,
    repo_path: &Utf8Path,
    server_addr: core::net::SocketAddr,
    connect_timeout: Duration,
    repo_manager: &Arc<RepositoryManager>,
    target_spec: &TargetInfoQuery,
    rcs_proxy: &Connector<RemoteControlProxyHolder>,
    knocker: &impl RcsKnocker,
    tx: &mut mpsc::UnboundedSender<ConnectEvent>,
    host_address: Option<String>,
    tunnel_addr: core::net::SocketAddr,
    connection_sink: &mut mpsc::UnboundedSender<anyhow::Result<ConnectionStream>>,
    repo_url_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
) -> Result<()> {
    let mut target_spec_from_rcs_proxy: Option<String> = None;
    let rcs_proxy = timeout(
        connect_timeout,
        rcs_proxy.try_connect(|target, _err| {
            log::info!(
                "RCS proxy: Waiting for target '{}' to return",
                match target {
                    Some(s) => s,
                    _ => "None",
                }
            );
            target_spec_from_rcs_proxy = target.clone();
            Ok(())
        }),
    )
    .await;
    let rcs_proxy = match rcs_proxy {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            return Err(e);
        }
        Err(e) => {
            fho::return_user_error!(
                "Timeout connecting to rcs: {}. Ensure the device is online and reachable, or try verifying RCS status with `ffx target list`.",
                e
            );
        }
    };

    let connection = connect_to_target(
        target_spec_from_rcs_proxy.clone(),
        host_address.clone(),
        cmd.alias.clone(),
        cmd.storage_type.clone(),
        server_addr,
        connect_timeout,
        Arc::clone(&repo_manager),
        &rcs_proxy,
        cmd.alias_conflict_mode.clone(),
        tunnel_addr,
        !cmd.no_device,
    )
    .await
    .with_context(|| format!("connect to target {target_spec:?}"));
    match connection {
        Ok((repo_host, proxy_stream)) => {
            if let Some(tx) = repo_url_tx {
                let (repo_name, _repo_client) = repo_manager.repositories().next().unwrap();
                let url = format!("http://{repo_host}/{repo_name}");
                if let Err(e) = tx.unbounded_send(url) {
                    log::warn!("Error sending repo url message: {e}");
                }
            }

            let s = match target_spec_from_rcs_proxy {
                Some(ref t) => ConnectEvent::StartServeToTarget {
                    repo_path: repo_path.to_string(),
                    target: t.to_string(),
                    addr: server_addr,
                },
                None => {
                    ConnectEvent::StartServe { repo_path: repo_path.to_string(), addr: server_addr }
                }
            };
            log::info!("{}", s);
            if let Err(e) = tx.send(s).await {
                log::warn!("Error sending start serve message: {}", e);
            }

            let mut timer_knock = pin!(
                async {
                    loop {
                        fuchsia_async::Timer::new(std::time::Duration::from_secs(10)).await;
                        match knocker.knock_rcs(target_spec, ctx).await {
                            Ok(_) => {
                                // Nothing to do, continue checking connection
                            }
                            Err(e) => {
                                let s = ConnectEvent::LostConnection { knock_error: e };
                                log::warn!("{}", s);
                                if let Err(send_err) = tx.send(s).await {
                                    log::warn!(
                                        "Error sending lost connection message: {}",
                                        send_err
                                    );
                                }
                                break;
                            }
                        }
                    }
                }
                .fuse()
            );
            let mut proxy_drive = pin!(
                proxy_stream
                    .map(|t| Ok(t.map(ConnectionStream::TargetTcp)))
                    .forward(
                        connection_sink.sink_map_err(|e| anyhow!("connection sink error: {e:?}")),
                    )
                    .fuse()
            );
            select! {
                () = timer_knock => {},
                r = proxy_drive => {
                    log::error!("driving forwarded connections exited unexpectedly: {r:?}")
                }
            }
        }
        Err(e) => {
            return Err(fho::Error::User(e));
        }
    };
    Ok(())
}

#[derive(Debug)]
pub(crate) enum ConnectEvent {
    StartServeToTarget { repo_path: String, target: String, addr: SocketAddr },
    StartServe { repo_path: String, addr: SocketAddr },
    LostConnection { knock_error: KnockError },
}

impl std::fmt::Display for ConnectEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LostConnection { knock_error } => {
                write!(f, "Connection to target lost, retrying. Error: {}", knock_error)
            }
            Self::StartServe { repo_path, addr } => {
                write!(f, "Serving repository '{}' over address '{}'.", repo_path, addr)
            }
            Self::StartServeToTarget { repo_path, target, addr } => {
                write!(
                    f,
                    "Serving repository '{}' to target '{}' over address '{}'.",
                    repo_path, target, addr
                )
            }
        }
    }
}

pub(crate) async fn main_connect_loop(
    ctx: &EnvironmentContext,
    cmd: &StartCommand,
    repo_path: &Utf8Path,
    server_addr: core::net::SocketAddr,
    connect_timeout: Duration,
    repo_manager: Arc<RepositoryManager>,
    mut loop_stop_rx: futures::channel::mpsc::Receiver<()>,
    target_spec: &TargetInfoQuery,
    rcs_proxy: Connector<RemoteControlProxyHolder>,
    knocker: &impl RcsKnocker,
    tx: &mut UnboundedSender<ConnectEvent>,
    host_address: Option<String>,
    tunnel_addr: core::net::SocketAddr,
    mut connection_sink: mpsc::UnboundedSender<anyhow::Result<ConnectionStream>>,
    repo_url_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
) -> Result<()> {
    let mut attempts = 0;

    // Outer connection loop, retries when disconnected.
    loop {
        attempts += 1;

        let cancel = async {
            // Block until a loop stop request comes in
            loop_stop_rx.next().await;
        }
        .fuse();

        let connect = inner_connect_loop(
            ctx,
            cmd,
            repo_path,
            server_addr,
            connect_timeout,
            &repo_manager,
            target_spec,
            &rcs_proxy,
            knocker,
            tx,
            host_address.clone(),
            tunnel_addr,
            &mut connection_sink,
            repo_url_tx.clone(),
        )
        .fuse();

        pin_mut!(cancel, connect);

        select! {
            () = cancel => {
                break Ok(());
            },
            r = connect => {
                match r {
                    // After successfully serving to the target, reset attempts counter before reconnect
                    Ok(()) => {
                        attempts = 0;
                    }
                    Err(e) => {
                        log::info!(
                            "Attempt {attempts}: {e}. Retrying in {} seconds...",
                            RECONNECT_DELAY.as_secs()
                        );
                        let timer = fuchsia_async::Timer::new(RECONNECT_DELAY).fuse();
                        pin_mut!(timer);
                        select! {
                            () = cancel => {
                                break Ok(());
                            }
                            _ = timer => {},
                        }
                    }
                }
            },
        };
    }
}
