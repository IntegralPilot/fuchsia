// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::Utf8PathBuf;
use ffx_config::EnvironmentContext;
use fidl_fuchsia_pkg_ext::{
    RepositoryConfig, RepositoryRegistrationAliasConflictMode, RepositoryStorageType,
};
use fuchsia_repo::repository::RepositorySpec;
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::Pid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Display;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, process};
use thiserror::Error;
use timeout::timeout;

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("terminate can only be called on non-daemon server instances")]
    NonDaemonTerminate,

    #[error("Terminate error: {0}")]
    TerminateError(String),

    #[error(
        "Multiple instances match repo-name: {name}. Disambiguate with one of {disambiguation}"
    )]
    MultipleInstances { name: String, disambiguation: String },

    #[error(
        "Cannot overrite running server with same name and a different pid: {name} existing pid: {existing_pid} new pid: {new_pid}"
    )]
    PidMismatch { name: String, existing_pid: u32, new_pid: u32 },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("IO error on {0}: {1}")]
    IoWithPath(std::path::PathBuf, #[source] std::io::Error),

    #[error("Serde error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Config error: {0}")]
    Config(#[from] ffx_config::api::ConfigError),

    #[error("Nix error: {0}")]
    Nix(#[from] nix::Error),
}

/// ServerMode is the execution mode of the server process.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    Background,
    Foreground,
    Daemon,
}

impl Display for ServerMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ServerMode::Background => "background",
                ServerMode::Foreground => "foreground",
                ServerMode::Daemon => "daemon",
            }
        )
    }
}

/// PkgServerInfo is serialized as a file that contains
/// the startup information for a running package server.
/// This includes the process id, which is intended for
/// use to troubleshoot and stop running instances.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct PkgServerInfo {
    pub name: String,
    pub address: SocketAddr,
    #[serde(default = "empty_repo_spec")]
    pub repo_spec: RepositorySpec,
    pub registration_storage_type: RepositoryStorageType,
    pub registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode,
    pub server_mode: ServerMode,
    pub repo_config: RepositoryConfig,
    pub pid: u32,
}

fn empty_repo_spec() -> RepositorySpec {
    RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() }
}

impl PkgServerInfo {
    /// Returns true if the process identified by the pid is running.
    pub fn is_running(&self) -> bool {
        let pid: Pid = nix::unistd::Pid::from_raw(
            self.pid.try_into().expect("pid to be representable as i32"),
        );
        if pid.as_raw() != 0 {
            // First do a no-hang wait to collect the process if it's defunct.
            let _ = nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG));
            // Check to see if it is running by sending signal 0. If there is no error,
            // the process is running.
            return nix::sys::signal::kill(pid, None).is_ok();
        }
        return false;
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub(crate) fn filename(&self) -> String {
        format!("{}@{}.json", self.name, self.port())
    }

    /// Terminate the running process if the server type is no Daemon.
    /// Termination is done by sending the SIGTERM signal and then waiting
    /// the specified timeout, and if the process is still running, SIGKILL is
    /// is sent.
    pub async fn terminate(
        &self,
        wait_timeout: Duration,
    ) -> std::result::Result<(), InstanceError> {
        if self.server_mode == ServerMode::Daemon {
            return Err(InstanceError::NonDaemonTerminate);
        }
        if self.is_running() {
            let pid: Pid = nix::unistd::Pid::from_raw(
                self.pid.try_into().expect("pid to be representable as i32"),
            );
            match nix::sys::signal::kill(pid, Some(nix::sys::signal::Signal::SIGTERM)) {
                Err(e) => return Err(InstanceError::Nix(e)),
                Ok(_) => {
                    match timeout(wait_timeout, async {
                        let options =
                            WaitPidFlag::union(WaitPidFlag::WNOHANG, WaitPidFlag::WEXITED);
                        loop {
                            match waitpid(pid, Some(options)) {
                                Err(e) => return Err(InstanceError::Nix(e)),
                                Ok(WaitStatus::Exited(p, code)) => {
                                    log::debug!("process {p} exited with code {code}");
                                    return Ok(());
                                }
                                Ok(WaitStatus::Signaled(_, _, _)) => return Ok(()),
                                _ => fuchsia_async::Timer::new(Duration::from_millis(100)).await,
                            }
                        }
                    })
                    .await
                    {
                        Ok(_) => return Ok(()),
                        Err(e) => {
                            log::info!("error terminating {}: {e}", self.name);
                            return nix::sys::signal::kill(
                                pid,
                                Some(nix::sys::signal::Signal::SIGKILL),
                            )
                            .map_err(|e| InstanceError::Nix(e));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn repo_path(&self) -> Option<Utf8PathBuf> {
        match &self.repo_spec {
            RepositorySpec::FileSystem { metadata_repo_path, .. } => {
                Some(metadata_repo_path.clone())
            }
            RepositorySpec::Pm { path, .. } => Some(path.clone()),
            _ => None,
        }
    }

    /// Returns a string for the repo path. This can be a file system path
    /// or a URL. Use repo_spec() to get the detailed information. This method
    /// should only be used for display purposes.
    pub fn repo_path_display(&self) -> String {
        match &self.repo_spec {
            RepositorySpec::FileSystem { metadata_repo_path, .. } => metadata_repo_path.to_string(),
            RepositorySpec::Pm { path, .. } => path.to_string(),
            RepositorySpec::Http { metadata_repo_url, .. } => metadata_repo_url.clone(),
            RepositorySpec::Gcs { metadata_repo_url, .. } => metadata_repo_url.clone(),
        }
    }

    pub fn repo_spec(&self) -> RepositorySpec {
        self.repo_spec.clone()
    }

    pub fn aliases(&self) -> BTreeSet<String> {
        self.repo_spec.aliases()
    }
}

/// PkgServerInstanceInfo is the trait implemented by
/// objects that manage package server instance info.
pub trait PkgServerInstanceInfo {
    /// Returns a list of running package servers.
    fn list_instances(&self) -> std::result::Result<Vec<PkgServerInfo>, InstanceError>;

    /// Returns the information for the package server with the
    /// provided name, or None if there is no running server with that
    /// name.
    /// Implementations must also handle the situation where there is multiple servers with the
    /// same name. This happens when the "repo_name" that is used as part of the package URL must
    /// meet some constraint. In these cases we need to support multiple repository servers, running
    /// different ports, and most likely serving different repository paths.
    /// For those, the port that the server is listening on can be used to disambiguate.
    /// An error is returned if the name is non-unique.
    fn get_instance(
        &self,
        name: String,
        port: Option<u16>,
    ) -> std::result::Result<Option<PkgServerInfo>, InstanceError>;

    ///  Removes the instance information for the server with the given name.
    /// If the name does not exist, it is ignored.
    /// Implementations must also handle the situation where there is multiple servers with the
    /// same name. This happens when the "repo_name" that is used as part of the package URL must
    /// meet some constraint. In these cases we need to support multiple repository servers, running
    /// different ports, and most likely serving different repository paths.
    /// For those, the port that the server is listening on can be used to disambiguate.
    /// An error is returned if the name is non-unique.
    fn remove_instance(
        &self,
        name: String,
        port: Option<u16>,
    ) -> std::result::Result<(), InstanceError>;

    /// Writes the instance information provided.
    fn write_instance(&self, instance: &PkgServerInfo) -> std::result::Result<(), InstanceError>;
}

pub struct PkgServerInstances {
    instance_root: PathBuf,
}

impl PkgServerInstances {
    pub fn new(instance_root: PathBuf) -> Self {
        PkgServerInstances { instance_root }
    }
}

impl PkgServerInstanceInfo for PkgServerInstances {
    fn list_instances(&self) -> std::result::Result<Vec<PkgServerInfo>, InstanceError> {
        let mut instances = Vec::<PkgServerInfo>::new();
        let root = self.instance_root.as_path();
        if root.is_dir() {
            for entry in
                root.read_dir().map_err(|e| InstanceError::IoWithPath(root.to_path_buf(), e))?
            {
                if let Ok(entry) = entry {
                    if entry.path().is_dir() {
                        continue;
                    }
                    if entry.path().extension().unwrap_or_default() == "json" {
                        // If there is a problem reading the file, return the error.
                        let data = fs::read(entry.path())
                            .map_err(|e| InstanceError::IoWithPath(entry.path(), e))?;

                        match serde_json::from_slice::<PkgServerInfo>(&data) {
                            Ok(info) => {
                                if info.is_running() {
                                    instances.push(info);
                                } else {
                                    fs::remove_file(entry.path())
                                        .map_err(|e| InstanceError::IoWithPath(entry.path(), e))?;
                                }
                            }
                            Err(e) => {
                                log::warn!(
                                    "Error deserializing {:?} into PkgServerInfo: {e}",
                                    entry.path()
                                );
                                let mut bad_name = entry.path();
                                bad_name.set_extension("json.bad");
                                fs::rename(entry.path(), &bad_name)
                                    .map_err(|e| InstanceError::IoWithPath(entry.path(), e))?;
                                log::warn!(
                                    "Renamed instance file {old} to {new}",
                                    old = entry.path().display(),
                                    new = bad_name.display()
                                );
                            }
                        };
                    }
                }
            }
        }
        Ok(instances)
    }

    fn get_instance(
        &self,
        name: String,
        port: Option<u16>,
    ) -> std::result::Result<Option<PkgServerInfo>, InstanceError> {
        let instances: Vec<PkgServerInfo> = self
            .list_instances()?
            .iter()
            .filter(|r| r.name == name)
            .filter(|r| r.is_running())
            .filter(|r| port.is_none() || r.port() == port.unwrap())
            .map(|r| r.clone())
            .collect();
        if instances.len() <= 1 {
            return Ok(instances.first().cloned());
        } else {
            let disambiguation = instances
                .iter()
                .map(|r| format!("{} port: {}", r.name, r.port()))
                .collect::<Vec<String>>()
                .join(", ");
            return Err(InstanceError::MultipleInstances { name, disambiguation });
        }
    }

    fn remove_instance(
        &self,
        name: String,
        port: Option<u16>,
    ) -> std::result::Result<(), InstanceError> {
        if let Some(instance) = self.get_instance(name, port)? {
            let filename = instance.filename();
            let instance_file = self.instance_root.join(filename);

            let fullpath = self.instance_root.join(instance_file);

            if fullpath.exists() {
                fs::remove_file(&fullpath)
                    .map_err(|e| InstanceError::IoWithPath(fullpath.clone(), e))
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    fn write_instance(&self, instance: &PkgServerInfo) -> std::result::Result<(), InstanceError> {
        if !self.instance_root.exists() {
            fs::create_dir_all(&self.instance_root)
                .map_err(|e| InstanceError::IoWithPath(self.instance_root.clone(), e))?;
        }
        // There can be multiple repository servers that are serving the same instance name
        // This is common where the repository name is constrained, such as preconfigured on the
        // device.
        // To accommodate this, the port is added to the instance name, which should make it unique.
        let filename = instance.filename();
        let instance_file = self.instance_root.join(filename);
        let contents = serde_json::to_string_pretty(&instance)?;
        fs::write(&instance_file, &contents)
            .map_err(|e| InstanceError::IoWithPath(instance_file.clone(), e))
    }
}

pub async fn write_instance_info(
    env_context: &EnvironmentContext,
    server_mode: ServerMode,
    name: &str,
    address: &SocketAddr,
    repo_spec: RepositorySpec,
    storage_type: RepositoryStorageType,
    conflict_mode: RepositoryRegistrationAliasConflictMode,
    repo_config: RepositoryConfig,
) -> std::result::Result<(), InstanceError> {
    let instance_root = env_context.get("repository.process_dir")?;
    let mgr = PkgServerInstances::new(instance_root);

    let info = PkgServerInfo {
        name: name.into(),
        address: address.clone(),
        repo_spec,
        registration_storage_type: storage_type.into(),
        registration_alias_conflict_mode: conflict_mode.into(),
        server_mode,
        pid: process::id(),
        repo_config,
    };
    if let Some(existing) = mgr.get_instance(name.into(), Some(info.port()))? {
        if existing.pid != info.pid {
            return Err(InstanceError::PidMismatch {
                name: name.to_string(),
                existing_pid: existing.pid,
                new_pid: info.pid,
            });
        }
        let message = format!("WARNING: Overwriting server info for {name}: {existing:?}");
        log::error!("{message}");
    }
    mgr.write_instance(&info)
}

#[cfg(test)]
mod tests {
    use super::*;

    use fidl_fuchsia_pkg_ext::RepositoryConfigBuilder;
    use std::collections::BTreeSet;
    use std::net::Ipv6Addr;
    use std::os::unix::fs::PermissionsExt as _;
    use std::process;

    #[fuchsia::test]
    async fn test_write_instance_info_with_context() {
        let mut builder = ffx_config::test_env();
        let repo_servers_dir = builder.isolate_root().join("repo_servers");

        let env = builder
            .user_config("repository.process_dir", repo_servers_dir.to_string_lossy())
            .build()
            .expect("test env");

        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);
        let repo_config =
            RepositoryConfigBuilder::new("fuchsia-pkg://somename".parse().unwrap()).build();
        write_instance_info(
            &env.context.clone(),
            ServerMode::Foreground,
            "somename",
            &addr,
            RepositorySpec::Http {
                metadata_repo_url: "http://metadata".into(),
                blob_repo_url: "http://blobs".into(),
                aliases: BTreeSet::new(),
            },
            RepositoryStorageType::Ephemeral,
            RepositoryRegistrationAliasConflictMode::ErrorOut,
            repo_config,
        )
        .await
        .expect("write_instance_info");

        let mgr = PkgServerInstances::new(
            env.context
                .get::<PathBuf, &str>("repository.process_dir")
                .expect("expected repo.process_dir"),
        );

        let instances = mgr.list_instances().expect("list_instances");
        assert_eq!(instances.len(), 1);
    }
    #[test]
    fn test_is_running() {
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };
        assert!(info.is_running());
    }

    #[test]
    fn test_list_not_exist() {
        let mgr = PkgServerInstances::new("/some/path".into());

        let instances = mgr.list_instances().expect("list_instances");
        assert!(instances.is_empty());
    }

    #[test]
    fn test_list_empty() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().into());

        let instances = mgr.list_instances().expect("list_instances");
        assert!(instances.is_empty());
    }

    #[test]
    fn test_write_not_exist() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");
        mgr.get_instance(instance_name.into(), None).expect("get instance");
    }

    #[test]
    fn test_writer_overwrite() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let mut info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm {
                path: Utf8PathBuf::from("path1"),
                aliases: BTreeSet::new(),
            },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");

        let got = mgr.get_instance(instance_name.into(), None).expect("get instance").unwrap();
        assert_eq!(got.repo_path(), Some(Utf8PathBuf::from("path1")));

        info.repo_spec =
            RepositorySpec::Pm { path: Utf8PathBuf::from("path2"), aliases: BTreeSet::new() };
        mgr.write_instance(&info).expect("written OK");

        let mut got = mgr.get_instance(instance_name.into(), None).expect("get instance").unwrap();
        assert_eq!(got.repo_path(), Some(Utf8PathBuf::from("path2")));

        let mut instances = mgr.list_instances().expect("list instances");
        assert_eq!(instances.len(), 1);
        got = instances.pop().unwrap();
        assert_eq!(got.name, instance_name);
    }

    #[test]
    fn test_write_same_name() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info_1 = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm {
                path: Utf8PathBuf::from("path1"),
                aliases: BTreeSet::new(),
            },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        let mut info_2 = info_1.clone();
        info_2.address = (Ipv6Addr::UNSPECIFIED, 8002).into();
        info_2.repo_spec =
            RepositorySpec::Pm { path: Utf8PathBuf::from("path2"), aliases: BTreeSet::new() };

        mgr.write_instance(&info_1).expect("written OK");
        mgr.write_instance(&info_2).expect("written OK");

        let got =
            mgr.get_instance(instance_name.into(), Some(8000)).expect("get instance").unwrap();
        assert_eq!(got.repo_path(), Some(Utf8PathBuf::from("path1")));

        let got =
            mgr.get_instance(instance_name.into(), Some(8002)).expect("get instance").unwrap();
        assert_eq!(got.repo_path(), Some(Utf8PathBuf::from("path2")));

        let res = mgr.get_instance(instance_name.into(), None);
        if let Err(e) = res {
            assert!(
                e.to_string().contains("Multiple instance"),
                "Expected multiple instance error, got {e}"
            )
        } else {
            assert!(res.is_err(), "Expected error, got {res:?}")
        }

        let instances = mgr.list_instances().expect("list instances");
        assert_eq!(instances.len(), 2);
    }

    #[test]
    fn test_list_1() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");
        let mut instances = mgr.list_instances().expect("list instance");
        assert_eq!(instances.len(), 1);
        let got = instances.pop().unwrap();
        assert_eq!(got.name, instance_name);
        assert_eq!(got.pid, info.pid);
    }

    #[test]
    fn test_list_2() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config_1 =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info_1 = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config: repo_config_1,
        };

        let another_instance_name = "another-instance";
        let repo_config_2 = RepositoryConfigBuilder::new(
            format!("fuchsia-pkg://{another_instance_name}").parse().unwrap(),
        )
        .build();

        let info_2 = PkgServerInfo {
            name: another_instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config: repo_config_2,
        };
        mgr.write_instance(&info_1).expect("written OK");
        mgr.write_instance(&info_2).expect("written OK");
        let mut instances = mgr.list_instances().expect("list instance");
        assert_eq!(instances.len(), 2);
        let got = instances.pop().unwrap();
        let other = instances.pop().unwrap();
        // order is not defined
        if got.name == instance_name {
            assert_eq!(other.name, another_instance_name);
        } else if got.name == another_instance_name {
            assert_eq!(other.name, instance_name);
        }
    }

    #[test]
    fn test_list_1_not_running() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config_1 =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info_1 = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config: repo_config_1,
        };

        let another_instance_name = "another-instance";
        let repo_config_2 = RepositoryConfigBuilder::new(
            format!("fuchsia-pkg://{another_instance_name}").parse().unwrap(),
        )
        .build();

        let info_2 = PkgServerInfo {
            name: another_instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: 0,
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config: repo_config_2,
        };
        mgr.write_instance(&info_1).expect("written OK");
        mgr.write_instance(&info_2).expect("written OK");
        let mut instances = mgr.list_instances().expect("list instance");
        assert_eq!(instances.len(), 1);
        let got = instances.pop().unwrap();
        assert_eq!(got.name, instance_name);
        assert_eq!(got.pid, info_1.pid);
    }

    #[test]
    fn test_get_instance_dir_not_exist() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());

        let got = mgr.get_instance("some-instance".into(), None).expect("get_instance");
        assert!(got.is_none());
    }

    #[test]
    fn test_get_instance_unknown() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().into());

        let got = mgr.get_instance("some-instance".into(), None).expect("get_instance");
        assert!(got.is_none());
    }

    #[test]
    fn test_get_instance_not_running() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: 0,
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");
        let got = mgr.get_instance(instance_name.into(), None).expect("get instance");
        assert!(got.is_none());
    }

    #[test]
    fn test_remove_dir_not_exist() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";

        mgr.remove_instance(instance_name.into(), None).expect("remove OK");
    }

    #[test]
    fn test_remove_running() {
        // This tests the case of the current process cleaning up itself. The process is still running, so the file
        // is still there and should be removed.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let instance_name = "some-instance";
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");

        mgr.remove_instance(instance_name.into(), None).expect("remove OK");

        let no_instance = mgr.get_instance(instance_name.into(), None).expect("get instance");
        assert!(no_instance.is_none(), "expected empty, got {no_instance:?}");
    }

    #[test]
    fn test_remove_not_running() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let repo_config =
            RepositoryConfigBuilder::new("fuchsia-pkg://some-server".parse().unwrap()).build();

        let instance_name = "some-instance";
        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: 0,
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");

        mgr.remove_instance(instance_name.into(), None).expect("remove OK");

        assert!(mgr.get_instance(instance_name.into(), None).expect("get instance").is_none());
    }

    #[test]
    fn test_remove_not_found() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let mgr = PkgServerInstances::new(tmp.path().join("instances").into());
        let repo_config =
            RepositoryConfigBuilder::new("fuchsia-pkg://some-server".parse().unwrap()).build();

        let instance_name = "some-instance";
        let another_instance_name = "another-instance";
        let info = PkgServerInfo {
            name: instance_name.into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: process::id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };

        mgr.write_instance(&info).expect("written OK");

        mgr.remove_instance(another_instance_name.into(), None).expect("remove OK");

        assert!(mgr.get_instance(instance_name.into(), None).expect("get instance").is_some());
        assert!(
            mgr.get_instance(another_instance_name.into(), None).expect("get instance").is_none()
        );
    }
    #[fuchsia::test]
    async fn test_terminate() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let fake_server = tmp.path().join(format!("fake_server.sh"));
        const FAKE_SERVER_CONTENTS: &str = r#"#!/bin/bash
        while sleep 1s; do
          echo "."
        done
        "#;

        // write out the shell script
        fs::write(&fake_server, FAKE_SERVER_CONTENTS).expect("writing fake server");
        let mut perm =
            fs::metadata(&fake_server).expect("Failed to get test server metadata").permissions();

        perm.set_mode(0o755);
        fs::set_permissions(&fake_server, perm).expect("Failed to set permissions on test runner");

        let child = process::Command::new(fake_server).spawn().expect("child process");

        let repo_config =
            RepositoryConfigBuilder::new("fuchsia-pkg://some-server".parse().unwrap()).build();

        let info = PkgServerInfo {
            name: "some-server".into(),
            address: (Ipv6Addr::UNSPECIFIED, 8000).into(),
            repo_spec: RepositorySpec::Pm { path: Utf8PathBuf::new(), aliases: BTreeSet::new() },
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: ServerMode::Foreground,
            pid: child.id(),
            registration_storage_type: RepositoryStorageType::Ephemeral,
            repo_config,
        };
        // There is race condition in which the fake server process could crash (e.g. if the bash
        // has a syntax error and spawning the process is slow) in between the is_running check and
        // the call to terminate, which results in this not actually testing that terminate
        // terminates. This sleep makes it much more likely that such a crash would occur before the
        // is_running check.
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(info.is_running(), "expect server process to be running");
        info.terminate(Duration::from_secs(10)).await.expect("terminate success");
    }
}
