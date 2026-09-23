// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use errors;
use ffx_config::EnvironmentContext;
use ffx_repository_server_list_args::ListCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxError, FfxMain, FfxTool, Result};

use fidl_fuchsia_pkg_ext::{RepositoryRegistrationAliasConflictMode, RepositoryStorageType};
use fuchsia_repo::repository::RepositorySpec;
use pkg::{PkgServerInfo, PkgServerInstanceInfo, PkgServerInstances, ServerMode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// PathType is an enum encapulating filesystem and URL based paths.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PathType {
    File(PathBuf),
    Url(String),
}

impl From<&Path> for PathType {
    fn from(value: &Path) -> Self {
        PathType::File(value.into())
    }
}

impl From<RepositorySpec> for PathType {
    fn from(value: RepositorySpec) -> Self {
        match value {
            RepositorySpec::FileSystem { metadata_repo_path, .. } => {
                PathType::File(metadata_repo_path.into())
            }
            RepositorySpec::Pm { path, .. } => PathType::File(path.into()),
            RepositorySpec::Http { metadata_repo_url, .. } => PathType::Url(metadata_repo_url),
            RepositorySpec::Gcs { metadata_repo_url, .. } => PathType::Url(metadata_repo_url),
        }
    }
}

impl Display for PathType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathType::File(p) => write!(f, "{}", p.display()),
            PathType::Url(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct PkgServerData {
    pub name: String,
    pub address: SocketAddr,
    pub repo_path: PathType,
    pub registration_aliases: Vec<String>,
    pub registration_storage_type: RepositoryStorageType,
    pub registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode,
    pub server_mode: ServerMode,
    pub pid: u32,
}

impl From<PkgServerInfo> for PkgServerData {
    fn from(value: PkgServerInfo) -> Self {
        Self {
            name: value.name.clone(),
            address: value.address,
            repo_path: value.repo_spec().into(),
            registration_aliases: value
                .repo_spec()
                .aliases()
                .iter()
                .map(ToString::to_string)
                .collect(),
            registration_storage_type: value.registration_storage_type,
            registration_alias_conflict_mode: value.registration_alias_conflict_mode,
            server_mode: value.server_mode,
            pid: value.pid,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { data: Vec<PkgServerData> },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}

#[derive(FfxError, Error, Debug)]
pub enum RepoListError {
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
    #[error("Repository server instances enumeration failed: {0}")]
    Instances(#[from] pkg::InstanceError),

    #[transparent]
    #[error(transparent)]
    Fho(#[from] fho::Error),
}

#[derive(FfxTool)]
#[main_error(RepoListError)]
pub struct RepoListTool {
    #[command]
    cmd: ListCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(RepoListTool, RepoListError);

#[async_trait(?Send)]
impl FfxMain for RepoListTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = RepoListError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        let full = self.cmd.full;
        let names = self.cmd.names.clone();
        let info = self.list().await?;
        // filter by names
        let filtered: Vec<PkgServerData> = info
            .into_iter()
            .filter(|s| names.contains(&s.name) || names.is_empty())
            .map(Into::into)
            .collect();
        writer.machine_or_else(&CommandStatus::Ok { data: filtered.clone() }, || {
            format_text(filtered, full)
        })?;
        Ok(())
    }
}

impl RepoListTool {
    async fn list(self) -> Result<Vec<PkgServerInfo>, RepoListError> {
        let instance_root = self.context.get("repository.process_dir")?;
        let mgr = PkgServerInstances::new(instance_root);
        let instances = mgr.list_instances()?;

        Ok(instances)
    }
}

fn format_text(infos: Vec<PkgServerData>, full: bool) -> String {
    let mut lines = vec![];
    for info in infos {
        lines.push(if !full {
            format!(
                "{name: <30}\t{address}\t{repo_path}",
                name = info.name,
                address = info.address,
                repo_path = info.repo_path
            )
        } else {
            format!(
                "{name: <30}\tpid: {pid}\n{address}\t{server_mode}\t{repo_path}\n\
            \tRegistration type: {reg_type:?}\taliases: {aliases:?}\tconflict mode: {mode:?}",
                name = info.name,
                pid = info.pid,
                address = info.address,
                server_mode = info.server_mode,
                repo_path = info.repo_path,
                reg_type = info.registration_storage_type,
                aliases = info.registration_aliases,
                mode = info.registration_alias_conflict_mode
            )
        });
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;

    use ffx_writer::{Format, TestBuffers};
    use fidl_fuchsia_pkg_ext::{RepositoryConfigBuilder, RepositoryStorageType};
    use std::collections::BTreeSet;
    use std::net::SocketAddr;
    use std::process;

    #[fuchsia::test]
    async fn test_empty() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .expect("test env");

        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec![] },
            context: test_env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_text() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .expect("test env");

        let dir = test_env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);

        let instance_name = "s1";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let s1 = PkgServerInfo {
            name: instance_name.into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from("/some/repo"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
            repo_config,
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec![] },
            context: test_env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("s1                            \t[::]:8000\t/some/repo\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_text_full() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .expect("test env");
        let dir = test_env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);

        let instance_name = "s1";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let s1 = PkgServerInfo {
            name: instance_name.into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from("/some/repo"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
            repo_config,
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: true, names: vec![] },
            context: test_env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        let pid = process::id();
        let expected = format!(
            "s1                            \tpid: {pid}\
        \n[::]:8000\tforeground\t/some/repo\
        \n\tRegistration type: Ephemeral\taliases: []\tconflict mode: ErrorOut\n"
        );
        assert_eq!(expected, stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_filter_name() {
        let env = ffx_config::test_env().build().expect("test env");
        let dir = env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);

        let instance_name = "s1";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let s1 = PkgServerInfo {
            name: instance_name.into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from("/some/repo"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
            repo_config,
        };
        mgr.write_instance(&s1).expect("writing s1");

        let instance_name_2 = "s2";
        let repo_config_2 = RepositoryConfigBuilder::new(
            format!("fuchsia-pkg://{instance_name_2}").parse().unwrap(),
        )
        .build();

        let s2 = PkgServerInfo {
            name: "s2".into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from("/some/other/repo"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::Replace,
            server_mode: pkg::ServerMode::Daemon,
            pid: process::id(),
            repo_config: repo_config_2,
        };
        mgr.write_instance(&s2).expect("writing s2");
        let tool = RepoListTool {
            cmd: ListCommand { full: false, names: vec!["s1".into()] },
            context: env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("s1                            \t[::]:8000\t/some/repo\n", stdout);
        assert_eq!("", stderr);
    }

    #[fuchsia::test]
    async fn test_machine_and_schema() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let test_env = builder
            .user_config("repository.process_dir", isolate_root.to_string_lossy())
            .build()
            .expect("test env");

        let dir = test_env.context.get("repository.process_dir").expect("process_dir");
        let mgr = PkgServerInstances::new(dir);
        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);

        let instance_name = "s1";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let s1 = PkgServerInfo {
            name: instance_name.into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from("/some/repo"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: process::id(),
            repo_config,
        };
        mgr.write_instance(&s1).expect("writing s1");

        let tool = RepoListTool {
            cmd: ListCommand { full: true, names: vec![] },
            context: test_env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &buffers);

        tool.main(writer).await.expect("ok");

        let (stdout, stderr) = buffers.into_strings();
        assert_eq!("", stderr);
        let expected = serde_json::to_string(&CommandStatus::Ok { data: vec![s1.into()] })
            .expect("serialize expected");
        let data = serde_json::from_str(&stdout).expect("json value");

        assert_eq!(format!("{expected}\n"), stdout);
        match VerifiedMachineWriter::<CommandStatus>::verify_schema(&data) {
            Ok(_) => (),
            Err(e) => {
                panic!("Error verifying schema: {e} for data {data:?}");
            }
        };
    }

    #[fuchsia::test]
    async fn test_list_instances_failed() {
        use std::os::unix::fs::PermissionsExt;
        let builder = ffx_config::test_env();
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("unreadable.json");
        std::fs::write(&file_path, b"{}").unwrap();

        // Set file to be completely unreadable (000)
        let mut perms = std::fs::metadata(&file_path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&file_path, perms).unwrap();

        let env = builder
            .user_config("repository.process_dir", temp_dir.path().to_string_lossy())
            .build()
            .unwrap();

        let tool = RepoListTool {
            context: env.context.clone(),
            cmd: ListCommand { full: false, names: vec![] },
        };
        let res = tool.list().await;

        // Re-enable permissions so tempdir cleanup doesn't fail
        if let Ok(mut perms) = std::fs::metadata(&file_path).map(|m| m.permissions()) {
            perms.set_mode(0o644);
            let _ = std::fs::set_permissions(&file_path, perms);
        }

        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), RepoListError::Instances(_)));
    }
}
