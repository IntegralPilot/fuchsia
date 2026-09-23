// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
use fuchsia as _;
#[cfg(test)]
use serde_json as _;

use anyhow::Context as _;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use ffx_config::EnvironmentContext;
use ffx_repository_serve_archive_args::ServeArchiveCommand;
use ffx_repository_server_start::{CommandStatus, ServerStartTool};
use ffx_repository_server_start_args::{
    StartCommand, default_alias_conflict_mode, default_tunnel_addr,
};
use ffx_writer::VerifiedMachineWriter;
use fho::{AvailabilityFlag, Deferred, FfxMain, FfxTool, Result};
use package_tool::{RepoCreateCommand, RepoPublishCommand, cmd_repo_create, cmd_repo_publish};
use std::marker::PhantomData;
use target_connector::Connector;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder};
use tempfile::TempDir;

#[async_trait(?Send)]
pub trait ServeArchiveTools {
    async fn serve_archive(
        cmd: ServeArchiveCommand,
        context: EnvironmentContext,
        target_spec: Deferred<TargetInfoQueryHolder>,
        rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
        host_address: Deferred<HostAddrHolder>,
        writer: VerifiedMachineWriter<CommandStatus>,
    ) -> Result<()>;
}

pub struct DefaultServeArchiveTools {}

#[async_trait(?Send)]
impl ServeArchiveTools for DefaultServeArchiveTools {
    async fn serve_archive(
        cmd: ServeArchiveCommand,
        context: EnvironmentContext,
        target_spec: Deferred<TargetInfoQueryHolder>,
        rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
        host_address: Deferred<HostAddrHolder>,
        writer: VerifiedMachineWriter<CommandStatus>,
    ) -> Result<()> {
        let ServeArchiveCommand { archive, repository, address, alias, tunnel_addr, no_device } =
            cmd;

        let temp_dir = TempDir::new().context("failed to create temporary directory for repo")?;
        let temp_dir_path = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
            .expect("temporary directory path is not valid UTF-8");

        let create_cmd = RepoCreateCommand {
            time_versioning: false,
            keys: None,
            repo_path: temp_dir_path.clone(),
        };

        cmd_repo_create(create_cmd)
            .await
            .map_err(|e| fho::user_error!(format!("Failed to initialize temporary repo: {}", e)))?;

        let publish_cmd = RepoPublishCommand {
            signing_keys: None,
            trusted_keys: None,
            trusted_root: None,
            package_manifests: vec![],
            package_list_manifests: vec![],
            package_archives: vec![archive],
            product_bundle: vec![],
            time_versioning: false,
            metadata_current_time: chrono::Utc::now(),
            refresh_root: false,
            clean: false,
            depfile: None,
            copy_mode: fuchsia_repo::repository::CopyMode::Copy,
            delivery_blob_type: 1,
            watch: false,
            ignore_missing_packages: false,
            blob_manifest: None,
            blob_repo_dir: None,
            repo_path: temp_dir_path.clone(),
        };

        cmd_repo_publish(publish_cmd).await.map_err(|e| {
            fho::user_error!(format!("Failed to publish archive to temporary repo: {}", e))
        })?;

        let start_cmd = StartCommand {
            address: Some(address),
            background: false,
            foreground: true,
            disconnected: false,
            repository: Some(repository),
            trusted_root: None,
            repo_path: Some(temp_dir_path.clone()),
            product_bundle: None,
            alias,
            storage_type: None,
            alias_conflict_mode: default_alias_conflict_mode(),
            port_path: None,
            tunnel_addr: tunnel_addr.or_else(|| Some(default_tunnel_addr())),
            no_device,
            refresh_metadata: false,
            auto_publish: None,
        };

        let start_tool = ServerStartTool {
            cmd: start_cmd,
            context,
            target_spec,
            rcs_proxy_connector,
            host_address: host_address,
        };

        start_tool.main(writer).await?;

        drop(temp_dir);

        Ok(())
    }
}

#[derive(FfxTool)]
#[check(AvailabilityFlag("repository.serve-archive"))]
pub struct ServeArchiveTool<T: ServeArchiveTools> {
    #[command]
    pub cmd: ServeArchiveCommand,
    pub context: EnvironmentContext,
    pub target_spec: Deferred<TargetInfoQueryHolder>,
    pub rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    pub host_address: Deferred<HostAddrHolder>,
    _phantom: PhantomData<T>,
}

fho::embedded_plugin!(ServeArchiveTool<DefaultServeArchiveTools>);

#[async_trait(?Send)]
impl<T: ServeArchiveTools> FfxMain for ServeArchiveTool<T> {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> Result<()> {
        T::serve_archive(
            self.cmd,
            self.context,
            self.target_spec,
            self.rcs_proxy_connector,
            self.host_address,
            writer,
        )
        .await
    }
}

#[cfg(test)]
mod tests {}
