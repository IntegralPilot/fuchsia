// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::time::Duration;

use async_trait::async_trait;
use errors;
use ffx_config::EnvironmentContext;
use ffx_repository_server_stop_args::StopCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use pkg::{PkgServerInfo, PkgServerInstanceInfo as _, PkgServerInstances};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { message: String },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}
use fho::FfxError;
use thiserror::Error;

#[derive(FfxError, Error, Debug)]
pub enum RepoStopError {
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
        "no running server named {0} is found. Try checking running servers with `ffx repository server list`."
    )]
    ServerNotFoundByName(String),

    #[exit_with_code(1)]
    #[error("no running server serving a product bundle {0} is found.")]
    ServerNotFoundByProductBundle(String),

    #[exit_with_code(1)]
    #[error(
        "more than 1 server running. Use --all or specify the name and port (if needed) of the server to stop."
    )]
    MultipleServersRunning,

    #[exit_with_code(1)]
    #[error("Could not terminate server: {0}")]
    TerminateServerFailed(#[source] pkg::InstanceError),

    #[exit_with_code(1)]
    #[error("Failed to list running server instances: {0}")]
    ListInstancesFailed(#[from] pkg::InstanceError),

    #[transparent]
    #[error(transparent)]
    Fho(#[from] fho::Error),
}

#[derive(FfxTool)]
#[main_error(RepoStopError)]
pub struct RepoStopTool {
    #[command]
    cmd: StopCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(RepoStopTool, RepoStopError);

#[async_trait(?Send)]
impl FfxMain for RepoStopTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = RepoStopError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        let info = self.stop().await?;
        let message = info.unwrap_or_else(|| "Stopped the repository server".into());
        writer.machine_or(&CommandStatus::Ok { message: message.clone() }, message)?;
        Ok(())
    }
}

impl RepoStopTool {
    pub async fn stop(self) -> Result<Option<String>, RepoStopError> {
        let instance_root = self.context.get("repository.process_dir")?;
        let mgr = PkgServerInstances::new(instance_root);
        let instances: Vec<PkgServerInfo> = mgr.list_instances()?;
        if instances.is_empty() {
            return Ok(Some("no running servers".into()));
        }
        let repo_port = self.cmd.port;

        if self.cmd.all {
            for instance in instances {
                Self::stop_instance(&instance).await?;
            }
            return Ok(None);
        } else if let Some(repo_name) = &self.cmd.name {
            if let Some(instance) = instances.iter().find(|s| {
                &s.name == repo_name && (repo_port.is_none() || repo_port.unwrap() == s.port())
            }) {
                return Self::stop_instance(instance).await;
            } else {
                return Err(RepoStopError::ServerNotFoundByName(repo_name.clone()));
            }
        } else if let Some(product_bundle) = &self.cmd.product_bundle {
            if let Some(instance) = instances.iter().find(|s| {
                s.repo_path_display() == *product_bundle
                    && (repo_port.is_none() || repo_port.unwrap() == s.port())
            }) {
                return Self::stop_instance(instance).await;
            } else {
                return Err(RepoStopError::ServerNotFoundByProductBundle(
                    product_bundle.to_string(),
                ));
            }
        } else {
            match instances.len() {
                0 => return Ok(Some("no running servers".into())),
                1 => {
                    return {
                        let instance = instances.get(0).unwrap();
                        Self::stop_instance(instance).await
                    };
                }
                _ => return Err(RepoStopError::MultipleServersRunning),
            }
        }
    }

    async fn stop_instance(instance: &PkgServerInfo) -> Result<Option<String>, RepoStopError> {
        match instance.terminate(Duration::from_secs(3)).await {
            Ok(_) => Ok(None),
            Err(e) => Err(RepoStopError::TerminateServerFailed(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use ffx_config::TestEnv;
    use fho::Result;
    use fidl_fuchsia_pkg_ext::{
        RepositoryConfigBuilder, RepositoryRegistrationAliasConflictMode, RepositoryStorageType,
    };
    use fuchsia_repo::repository::RepositorySpec;
    use pkg::ServerMode;
    use std::collections::BTreeSet;
    use std::fs;
    use std::io::Write;
    use std::net::Ipv4Addr;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process::{Child, Command};
    use std::sync::Mutex;

    const FAKE_SERVER_CONTENTS: &str = r#"#!/bin/bash
       while sleep 1s
       do
         echo "."
       done
    "#;

    // Need to have a guard around our Command::new::spawn() due to this bug:
    // https://github.com/rust-lang/rust/issues/114554
    // TLDR: the `spawn` for the first Test will call `fork(2)` which inherits
    // the file descriptors opened by the current process. This includes
    // the file descripotor for the second test (happening in another thread).
    // This causes an `ExecutableFileBusy` (`ETXTBSY`) error since the fd is
    // open for write when we are trying to execute it.
    static COMMAND_LOCK: std::sync::LazyLock<Mutex<()>> =
        std::sync::LazyLock::new(|| Mutex::new(()));

    fn make_standalone_instance(
        name: String,
        product_bundle_path: Option<Utf8PathBuf>,
        context: &EnvironmentContext,
        test_env: &TestEnv,
    ) -> Result<(PkgServerInstances, Child)> {
        let command_guard = COMMAND_LOCK.lock().expect("lock command");
        let fake_server = test_env.isolate_root.path().join(format!("{name}_fake_server.sh"));
        // write out the shell script
        {
            let mut file = fs::File::create(&fake_server).expect("creating fake server");
            file.write_all(FAKE_SERVER_CONTENTS.as_bytes()).expect("writing fake server");
            let mut perm = fs::metadata(&fake_server)
                .expect("Failed to get test server metadata")
                .permissions();

            perm.set_mode(0o755);
            file.set_permissions(perm).expect("Failed to set permissions on test runner");
        }

        let child = Command::new(fake_server).spawn().expect("child process");

        drop(command_guard);

        let instance_root = context.get("repository.process_dir").expect("instance dir");
        let mgr = PkgServerInstances::new(instance_root);

        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{name}").parse().unwrap()).build();

        let address = (Ipv4Addr::LOCALHOST, 1234).into();

        let repo_path: Utf8PathBuf =
            if let Some(pb) = product_bundle_path { pb } else { Utf8PathBuf::from("/somewhere") };

        mgr.write_instance(&PkgServerInfo {
            name,
            address,
            repo_spec: RepositorySpec::Pm { path: repo_path, aliases: BTreeSet::new() }.into(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: child.id(),
            repo_config,
        })
        .expect("writing instance");
        Ok((mgr, child))
    }

    #[fuchsia::test]
    async fn test_standalone_stop() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let env = builder
            .user_config(
                "repository.process_dir",
                isolate_root.join("repo_servers").to_string_lossy(),
            )
            .build()
            .unwrap();

        let (_mgr, _server_proc) =
            make_standalone_instance("default".into(), None, &env.context, &env)
                .expect("test daemon instance");

        let tool = RepoStopTool {
            context: env.context.clone(),
            cmd: StopCommand { all: true, name: None, port: None, product_bundle: None },
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);
        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!(stdout, "Stopped the repository server\n");
        assert_eq!(stderr, "");
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_product_bundle_stop() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let env = builder
            .user_config(
                "repository.process_dir",
                isolate_root.join("repo_servers").to_string_lossy(),
            )
            .build()
            .unwrap();

        let product_bundle_path =
            Utf8PathBuf::from_path_buf(env.isolate_root.path().join("pb")).expect("utf8 path");

        let (_mgr, mut server_proc) = make_standalone_instance(
            "some-pb.com".into(),
            Some(product_bundle_path.clone()),
            &env.context,
            &env,
        )
        .expect("test daemon instance");

        let tool = RepoStopTool {
            context: env.context.clone(),
            cmd: StopCommand {
                all: false,
                name: None,
                port: None,
                product_bundle: Some(product_bundle_path),
            },
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);
        let res = tool.main(writer).await;
        let (stdout, stderr) = buffers.into_strings();

        // clean up the server process, if still present
        let _ = server_proc.kill();
        assert!(res.is_ok(), "Expected ok, got {res:?} {stdout} {stderr}");
        assert_eq!(stdout, "Stopped the repository server\n", "stderr: {stderr}");
        assert_eq!(stderr, "");
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_stop_server_not_found_by_name() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let env = builder
            .user_config(
                "repository.process_dir",
                isolate_root.join("repo_servers").to_string_lossy(),
            )
            .build()
            .unwrap();

        let (_mgr, mut server_proc) =
            make_standalone_instance("default".into(), None, &env.context, &env)
                .expect("test daemon instance");

        let tool = RepoStopTool {
            context: env.context.clone(),
            cmd: StopCommand {
                all: false,
                name: Some("nonexistent_server".to_string()),
                port: None,
                product_bundle: None,
            },
        };
        let res = tool.stop().await;
        let _ = server_proc.kill();

        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err(),
            RepoStopError::ServerNotFoundByName(name) if name == "nonexistent_server"
        ));
    }

    #[fuchsia::test]
    async fn test_stop_server_not_found_by_product_bundle() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let env = builder
            .user_config(
                "repository.process_dir",
                isolate_root.join("repo_servers").to_string_lossy(),
            )
            .build()
            .unwrap();

        let (_mgr, mut server_proc) =
            make_standalone_instance("default".into(), None, &env.context, &env)
                .expect("test daemon instance");

        let tool = RepoStopTool {
            context: env.context.clone(),
            cmd: StopCommand {
                all: false,
                name: None,
                port: None,
                product_bundle: Some(camino::Utf8PathBuf::from("/nonexistent/pb")),
            },
        };
        let res = tool.stop().await;
        let _ = server_proc.kill();

        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err(),
            RepoStopError::ServerNotFoundByProductBundle(path) if path == "/nonexistent/pb"
        ));
    }

    #[fuchsia::test]
    async fn test_stop_multiple_servers_running() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let env = builder
            .user_config(
                "repository.process_dir",
                isolate_root.join("repo_servers").to_string_lossy(),
            )
            .build()
            .unwrap();

        let (_mgr1, mut server_proc1) =
            make_standalone_instance("server1".into(), None, &env.context, &env)
                .expect("test daemon instance 1");
        let (_mgr2, mut server_proc2) =
            make_standalone_instance("server2".into(), None, &env.context, &env)
                .expect("test daemon instance 2");

        let tool = RepoStopTool {
            context: env.context.clone(),
            cmd: StopCommand { all: false, name: None, port: None, product_bundle: None },
        };
        let res = tool.stop().await;
        let _ = server_proc1.kill();
        let _ = server_proc2.kill();

        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), RepoStopError::MultipleServersRunning));
    }
}
