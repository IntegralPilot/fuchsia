// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::CommandStatus;
use crate::server_impl::{ServeError, ServeStarted, get_repo_base_name, serve_impl};

use ffx_config::EnvironmentContext;
use ffx_repository_server_start_args::StartCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{Deferred, FfxError};
use fidl_fuchsia_pkg_ext::{RepositoryRegistrationAliasConflictMode, RepositoryStorageType};
use futures::StreamExt as _;
use pkg::{PkgServerInstanceInfo, PkgServerInstances, ServerMode};
use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;
use target_connector::Connector;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder};
use thiserror::Error;
use timeout::TimeoutError;

pub(crate) fn to_argv(cmd: &StartCommand) -> Vec<String> {
    let mut argv: Vec<String> = vec![];
    if let Some(addr) = cmd.address {
        argv.extend_from_slice(&["--address".into(), addr.to_string()]);
    }
    for a in &cmd.alias {
        argv.extend_from_slice(&["--alias".into(), a.to_string()]);
    }
    let mode = match cmd.alias_conflict_mode {
        RepositoryRegistrationAliasConflictMode::ErrorOut => "error-out".to_string(),
        RepositoryRegistrationAliasConflictMode::Replace => "replace".to_string(),
    };
    argv.extend_from_slice(&["--alias-conflict-mode".into(), mode]);

    if cmd.no_device {
        argv.push("--no-device".into());
    }

    if let Some(pb) = &cmd.product_bundle {
        argv.extend_from_slice(&["--product-bundle".into(), pb.as_str().to_string()]);
    }

    if let Some(p) = &cmd.repo_path {
        argv.extend_from_slice(&["--repo-path".into(), p.as_str().to_string()]);
    }
    if let Some(name) = &cmd.repository {
        argv.extend_from_slice(&["--repository".into(), name.to_string()]);
    }
    if let Some(s) = &cmd.storage_type {
        let st = match s {
            RepositoryStorageType::Ephemeral => "ephemeral",
            RepositoryStorageType::Persistent => "persistent",
        };
        argv.extend_from_slice(&["--storage-type".into(), st.to_string()]);
    }

    if let Some(p) = &cmd.trusted_root {
        argv.extend_from_slice(&["--trusted-root".into(), p.as_str().to_string()]);
    }
    if cmd.refresh_metadata {
        argv.push("--refresh-metadata".into());
    }
    if let Some(manifest) = &cmd.auto_publish {
        argv.extend_from_slice(&["--auto-publish".into(), manifest.as_str().to_string()]);
    }
    if let Some(tunnel_addr) = &cmd.tunnel_addr {
        argv.extend_from_slice(&["--tunnel-addr".into(), tunnel_addr.to_string()]);
    }
    argv
}

#[derive(FfxError, Error, Debug)]
pub enum ForegroundServerError {
    #[transparent]
    #[error(transparent)]
    Server(#[from] ServeError),

    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),
}

/// Runs the repository server in the foreground.
///
/// # Arguments
/// * `repo_url_tx` - Channel on which the server will send the repo url when the server starts
///   and any time it changes.
pub async fn run_foreground_server(
    start_cmd: StartCommand,
    context: EnvironmentContext,
    target_spec: Deferred<TargetInfoQueryHolder>,
    rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    host_addr: Deferred<HostAddrHolder>,
    mut w: VerifiedMachineWriter<CommandStatus>,
    mode: ServerMode,
    repo_url_tx: Option<futures::channel::mpsc::UnboundedSender<String>>,
) -> std::result::Result<(), ForegroundServerError> {
    let (mut tx, mut rx) = futures::channel::mpsc::unbounded();
    let drain_task = async {
        while let Some(msg) = rx.next().await {
            let _ = w.item(&CommandStatus::Message { message: format!("{}", msg) });
        }
    };
    // The "async move" moves tx into the main_task future, which causes it to drop on completion,
    // allowing the drain_task to complete. Otherwise, the join on both futures would hang.
    let main_task = async move {
        serve_impl(
            target_spec,
            rcs_proxy_connector,
            host_addr,
            start_cmd,
            context,
            mode,
            &mut tx,
            repo_url_tx,
        )
        .await
    };
    let (_, res) = futures::join!(drain_task, main_task);
    let res = res.map_err(ForegroundServerError::Server)?;
    // Write the message.
    match res {
        ServeStarted::AlreadyRunning { address, repo_path, name, pid } => {
            let s = format!(
                "A server with pid {pid} named {name} is already serving on address {address} \
                 the repo path: {repo_path}"
            );
            log::warn!("{s}");
            writeln!(w, "{s}")?;
        }
        ServeStarted::Started { address, repo_path } => {
            let s = format!("Serving repository '{repo_path}' over address '{}'.", address);
            w.item(&CommandStatus::Message { message: s.clone() })?;
            log::info!("{}", s);
        }
    };
    Ok(())
}

#[derive(Error, Debug)]
pub enum WaitForStartError {
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[error("Timed out waiting for the server to start: {0}")]
    Timeout(#[from] TimeoutError),
}

pub(crate) async fn wait_for_start(
    context: EnvironmentContext,
    cmd: StartCommand,
    time_to_wait: Duration,
) -> std::result::Result<SocketAddr, WaitForStartError> {
    let instance_root = context.get("repository.process_dir")?;
    let mgr = PkgServerInstances::new(instance_root);

    let repo_base_name = get_repo_base_name(&cmd.repository, &context)?;
    log::debug!("waiting up to {time_to_wait:?} for {repo_base_name} to start.");
    let addr = timeout::timeout(time_to_wait, async move {
        loop {
            match mgr.list_instances() {
                Ok(running_instances) => {
                    if let Some(instance) = running_instances
                        .iter()
                        .find(|instance| instance.name.starts_with(&repo_base_name))
                    {
                        return instance.address.clone();
                    }
                    log::debug!(
                        "waiting for {repo_base_name} to start. Got: {running_instances:?}"
                    );
                }
                Err(e) => {
                    log::debug!("list_instances returned {e} wait for {repo_base_name} to start.");
                }
            }
            fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await;
        }
    })
    .await?;
    Ok(addr)
}

#[cfg(test)]
mod test {
    use super::*;
    use camino::Utf8PathBuf;
    use std::str::FromStr as _;

    #[fuchsia::test]
    fn test_to_argv() {
        let start_cmd = StartCommand {
            address: Some(([127, 0, 0, 1], 8787).into()),
            background: true,
            foreground: false,
            disconnected: false,
            repository: Some("repo-name".into()),
            trusted_root: Some(Utf8PathBuf::from_str("/trusted/root").expect("UTF8 path")),
            repo_path: Some(Utf8PathBuf::from_str("/repo/path/root").expect("UTF8 path")),
            product_bundle: Some(Utf8PathBuf::from_str("/product/bundle/path").expect("UTF8 path")),
            alias: vec!["alias1".into(), "alias2".into()],
            storage_type: Some(RepositoryStorageType::Ephemeral),
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
            port_path: Some("/path/port/file".into()),
            tunnel_addr: Some(([127, 0, 0, 1], 1313).into()),
            no_device: false,
            refresh_metadata: true,
            auto_publish: Some(Utf8PathBuf::from_str("/auto/publish/list").expect("UTF8 path")),
        };
        let actual = to_argv(&start_cmd);
        let expected: Vec<String> = [
            "--address",
            "127.0.0.1:8787",
            "--alias",
            "alias1",
            "--alias",
            "alias2",
            "--alias-conflict-mode",
            "replace",
            "--product-bundle",
            "/product/bundle/path",
            "--repo-path",
            "/repo/path/root",
            "--repository",
            "repo-name",
            "--storage-type",
            "ephemeral",
            "--trusted-root",
            "/trusted/root",
            "--refresh-metadata",
            "--auto-publish",
            "/auto/publish/list",
            "--tunnel-addr",
            "127.0.0.1:1313",
        ]
        .iter()
        .map(ToString::to_string)
        .collect();

        assert_eq!(actual, expected);
    }
}
