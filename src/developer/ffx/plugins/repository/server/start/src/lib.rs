// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use daemonize::daemonize;
use ffx_config::EnvironmentContext;
use ffx_repository_server_start_args::StartCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{Deferred, FfxMain, FfxTool, Result};
use pkg::ServerMode;
use pkg::config::DEFAULT_REPO_NAME;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::time::Duration;
use target_connector::Connector;
use target_holders::{HostAddrHolder, RemoteControlProxyHolder, TargetInfoQueryHolder};

pub mod server;
mod server_impl;
mod target;

use server_impl::serve_impl_validate_args;

// The output is untagged and OK is flattened to match
// the legacy output. One day, we'll update the schema and
// worry about migration then.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok {
        #[serde(flatten)]
        address: ServerInfo,
    },
    /// Debug message
    Message { message: String },
    /// Unexpected error with string.
    UnexpectedError { error_message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { error_message: String },
}

impl std::fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::Ok { address } => {
                format!("Serving over addresss: {:?}", address)
            }
            Self::Message { message } => message.to_string(),
            Self::UnexpectedError { error_message } | Self::UserError { error_message } => {
                error_message.to_string()
            }
        };
        write!(f, "{}", msg)
    }
}

use fho::FfxError;
use thiserror::Error;

#[derive(FfxError, Error, Debug)]
pub enum RepoStartError {
    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[exit_with_code(1)]
    #[error(
        "Mutually exclusive arguments: --background is mutually exclusive with --foreground and --disconnected"
    )]
    MutuallyExclusiveArgs,

    #[exit_with_code(1)]
    #[error("Cannot daemonize repository server without a log file basename")]
    MissingLogBasename,

    #[exit_with_code(1)]
    #[error("Daemonization failed: {0}")]
    Daemonize(#[from] daemonize::DaemonizeError),

    #[exit_with_code(1)]
    #[error("Failed to run foreground repository server: {0}")]
    ForegroundServerFailed(#[source] server::ForegroundServerError),

    #[exit_with_code(1)]
    #[error("Failed to validate server arguments: {0}")]
    ServerValidationFailed(#[source] server_impl::ServerValidationError),

    #[exit_with_code(1)]
    #[error("Timed out waiting for the repository server to start: {0}")]
    ServerStartTimeout(#[source] server::WaitForStartError),
}

#[derive(FfxTool)]
#[main_error(RepoStartError)]
pub struct ServerStartTool {
    #[command]
    pub cmd: StartCommand,
    pub context: EnvironmentContext,
    pub target_spec: Deferred<TargetInfoQueryHolder>,
    pub rcs_proxy_connector: Connector<RemoteControlProxyHolder>,
    pub host_address: Deferred<HostAddrHolder>,
}

fho::embedded_plugin!(ServerStartTool, RepoStartError);

#[async_trait(?Send)]
impl FfxMain for ServerStartTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = RepoStartError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        let new_logname = self.log_basename();

        let server_addr = match (self.cmd.background, self.cmd.foreground || self.cmd.disconnected)
        {
            // Foreground server mode
            (false, true) | (false, false) => {
                let mode = if self.cmd.disconnected {
                    ServerMode::Background
                } else {
                    ServerMode::Foreground
                };
                Box::pin(server::run_foreground_server(
                    self.cmd,
                    self.context,
                    self.target_spec,
                    self.rcs_proxy_connector,
                    self.host_address,
                    writer,
                    mode,
                    None,
                ))
                .await
                .map_err(RepoStartError::ForegroundServerFailed)?;
                return Ok(());
            }
            // Background server mode
            (true, false) => {
                // Validate the cmd args before processing. This allows good error messages to
                // be presented to the user when running in Background mode. If the server is
                // already running, this returns Ok.
                if let Some(running) =
                    serve_impl_validate_args(&self.cmd, &self.rcs_proxy_connector, &self.context)
                        .await
                        .map_err(RepoStartError::ServerValidationFailed)?
                {
                    // The server that matches the cmd is already running.
                    writeln!(
                        writer,
                        "A server with pid {} named {} is serving on address {} \
                             the repo path: {}",
                        running.pid,
                        running.name,
                        running.address,
                        running.repo_path_display()
                    )?;
                    Some(running.address.clone())
                } else {
                    let mut args = vec![
                        "repository".to_string(),
                        "server".to_string(),
                        "start".to_string(),
                        "--disconnected".to_string(),
                    ];
                    args.extend(server::to_argv(&self.cmd));

                    if let Some(log_basename) = new_logname {
                        let wait_for_start_timeout: u64 = self
                            .context
                            .get::<u64, _>("repository.background_startup_timeout")
                            .unwrap_or_else(|e| {
                                log::warn!("Error reading startup timeout: {e}");
                                60
                            });

                        daemonize(&args, log_basename, self.context.clone(), true).await?;

                        let addr = server::wait_for_start(
                            self.context.clone(),
                            self.cmd,
                            Duration::from_secs(wait_for_start_timeout),
                        )
                        .await
                        .map_err(RepoStartError::ServerStartTimeout)?;
                        log::debug!("Daemonized server started successfully");
                        Some(addr)
                    } else {
                        return Err(RepoStartError::MissingLogBasename);
                    }
                }
            }
            // Invalid switch combinations.
            (true, true) => {
                return Err(RepoStartError::MutuallyExclusiveArgs);
            }
        };

        if let Some(server_addr) = server_addr {
            writer.machine_or(
                &CommandStatus::Ok { address: ServerInfo { address: server_addr } },
                format!("Repository server is listening on {server_addr:?}"),
            )?;
        }
        Ok(())
    }

    fn log_basename(&self) -> Option<String> {
        let basename = format!(
            "repo_{}",
            self.cmd.repository.clone().unwrap_or_else(|| DEFAULT_REPO_NAME.into())
        );
        Some(basename)
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ServerInfo {
    address: std::net::SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::TestBuffers;
    use fho::{FfxMain, FhoEnvironment, TryFromEnv};
    use fidl_fuchsia_pkg_ext::RepositoryRegistrationAliasConflictMode;

    #[fuchsia::test]
    async fn test_start_mutually_exclusive_args() {
        let env = ffx_config::test_env().build().unwrap();
        let cmd = StartCommand {
            repository: None,
            trusted_root: None,
            address: None,
            repo_path: None,
            product_bundle: None,
            alias: vec![],
            storage_type: None,
            alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            port_path: None,
            tunnel_addr: None,
            no_device: false,
            refresh_metadata: false,
            auto_publish: None,
            background: true,
            foreground: true,
            disconnected: false,
        };
        let fho_env =
            FhoEnvironment::new_with_args(&env.context, &["some", "repo", "start", "test"]);
        let tool = ServerStartTool {
            cmd,
            context: env.context.clone(),
            target_spec: Deferred::from_output(Ok(TargetInfoQueryHolder::from(
                discovery::query::TargetInfoQuery::try_from("".to_string()).unwrap(),
            ))),
            rcs_proxy_connector: Connector::try_from_env(&fho_env).await.unwrap(),
            host_address: Deferred::from_output(Ok(HostAddrHolder::from("1.2.3.4".to_string()))),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);
        let res = tool.main(writer).await;
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), RepoStartError::MutuallyExclusiveArgs));
    }
}
