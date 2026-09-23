// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::TargetEvent;
use crate::error::Error;
use crate::events::{TargetHandle, TargetState};
use crate::instance_watcher::{InstanceSource, InstanceWatcher, is_pid_running};
use addr::TargetAddr;
use ffx_config::EnvironmentContext;
use futures::channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};
use std::io::{BufWriter, Write as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct GceInstanceData {
    #[serde(alias = "instance")]
    pub instance_name: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub zone: String,
    pub pid: u32,
    #[serde(alias = "port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub reverse_ports: Vec<u16>,
    #[serde(default)]
    pub serial_number: Option<String>,
}

impl GceInstanceData {
    pub fn is_running(&self) -> bool {
        is_pid_running(self.pid)
    }

    /// Terminates the local background SSH tunnel process group for this instance, if running.
    pub fn terminate(&self) {
        if self.pid != 0 {
            if let Ok(raw_pid) = i32::try_from(self.pid) {
                let pgid = nix::unistd::Pid::from_raw(-raw_pid);
                if nix::sys::signal::kill(pgid, Some(nix::sys::signal::Signal::SIGTERM)).is_err() {
                    let p = nix::unistd::Pid::from_raw(raw_pid);
                    let _ = nix::sys::signal::kill(p, Some(nix::sys::signal::Signal::SIGTERM));
                }
            }
        }
    }

    pub fn to_target_handle(&self) -> Option<TargetHandle> {
        if !self.is_running() || self.ssh_port == 0 {
            return None;
        }
        let sock_addr = SocketAddr::from(([127, 0, 0, 1], self.ssh_port));
        Some(TargetHandle {
            node_name: Some(self.instance_name.clone()),
            state: TargetState::Product {
                addrs: vec![TargetAddr::Net(sock_addr)],
                serial: self.serial_number.clone(),
            },
            manual: false,
        })
    }
}

fn read_instance_file(path: &Path) -> Result<Option<GceInstanceData>, std::io::Error> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let data = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(data))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstanceError {
    #[error("instance name cannot be empty")]
    Empty,
    #[error("instance name '{name}' cannot exceed 63 characters (got {len})")]
    TooLong { name: String, len: usize },
    #[error("instance name '{name}' must start with a lowercase letter")]
    InvalidStart { name: String },
    #[error("instance name '{name}' must end with a lowercase letter or digit")]
    InvalidEnd { name: String },
    #[error("instance name '{name}' cannot contain underscores ('_'); use hyphens ('-') instead")]
    ContainsUnderscore { name: String },
    #[error(
        "instance name '{name}' contains invalid character '{char}'; must contain only lowercase letters, digits, and hyphens"
    )]
    InvalidCharacter { name: String, char: char },
    #[error("project cannot be empty")]
    ProjectEmpty,
    #[error("project '{project}' cannot contain underscores ('_'); use hyphens ('-') instead")]
    ProjectContainsUnderscore { project: String },
    #[error(
        "project '{project}' contains invalid character '{char}'; must contain only lowercase letters, digits, and hyphens"
    )]
    ProjectInvalidCharacter { project: String, char: char },
    #[error("zone cannot be empty")]
    ZoneEmpty,
    #[error("zone '{zone}' cannot contain underscores ('_'); use hyphens ('-') instead")]
    ZoneContainsUnderscore { zone: String },
    #[error(
        "zone '{zone}' contains invalid character '{char}'; must contain only lowercase letters, digits, and hyphens"
    )]
    ZoneInvalidCharacter { zone: String, char: char },
    #[error("invalid instance file stem format: expected '<project>_<zone>_<name>', got '{stem}'")]
    InvalidFileStem { stem: String },
    #[error("path is not in instance root")]
    NotInRoot,
    #[error("path is not a direct child of instance root")]
    NotDirectChild,
    #[error("path does not have .json extension")]
    NotJsonFile,
}

pub type InstanceNameError = InstanceError;

/// Represents a validated GCE instance identifier consisting of project, zone, and instance name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Instance {
    pub project: String,
    pub zone: String,
    pub name: String,
}

pub type GceInstance = Instance;
pub type InstanceName = Instance;

impl Instance {
    /// Constructs a new validated `Instance`.
    pub fn new(
        project: impl Into<String>,
        zone: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Self, InstanceError> {
        let project = project.into();
        let zone = zone.into();
        let name = name.into();
        Self::validate(&project, &zone, &name)?;
        Ok(Self { project, zone, name })
    }

    /// Constructs an `Instance` without validating fields.
    pub fn new_unchecked(
        project: impl Into<String>,
        zone: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self { project: project.into(), zone: zone.into(), name: name.into() }
    }

    /// Validates all components (project, zone, and instance name).
    pub fn validate(project: &str, zone: &str, name: &str) -> Result<(), InstanceError> {
        Self::validate_project(project)?;
        Self::validate_zone(zone)?;
        Self::validate_name(name)?;
        Ok(())
    }

    /// Validates project ID format.
    pub fn validate_project(project: &str) -> Result<(), InstanceError> {
        if project.is_empty() {
            return Err(InstanceError::ProjectEmpty);
        }
        if project.contains('_') {
            return Err(InstanceError::ProjectContainsUnderscore { project: project.to_string() });
        }
        for c in project.chars() {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(InstanceError::ProjectInvalidCharacter {
                    project: project.to_string(),
                    char: c,
                });
            }
        }
        Ok(())
    }

    /// Validates zone format.
    pub fn validate_zone(zone: &str) -> Result<(), InstanceError> {
        if zone.is_empty() {
            return Err(InstanceError::ZoneEmpty);
        }
        if zone.contains('_') {
            return Err(InstanceError::ZoneContainsUnderscore { zone: zone.to_string() });
        }
        for c in zone.chars() {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(InstanceError::ZoneInvalidCharacter {
                    zone: zone.to_string(),
                    char: c,
                });
            }
        }
        Ok(())
    }

    /// Validates that an instance name complies with RFC 1035 / GCE naming rules:
    /// 1-63 lowercase alphanumeric characters or hyphens, starting with a letter,
    /// ending with a letter or digit, and containing no underscores.
    pub fn validate_name(name: &str) -> Result<(), InstanceError> {
        if name.is_empty() {
            return Err(InstanceError::Empty);
        }
        if name.len() > 63 {
            return Err(InstanceError::TooLong { name: name.to_string(), len: name.len() });
        }
        if name.contains('_') {
            return Err(InstanceError::ContainsUnderscore { name: name.to_string() });
        }
        let first = name.chars().next().unwrap();
        if !first.is_ascii_lowercase() {
            return Err(InstanceError::InvalidStart { name: name.to_string() });
        }
        let last = name.chars().last().unwrap();
        if !last.is_ascii_lowercase() && !last.is_ascii_digit() {
            return Err(InstanceError::InvalidEnd { name: name.to_string() });
        }
        for c in name.chars() {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' {
                return Err(InstanceError::InvalidCharacter { name: name.to_string(), char: c });
            }
        }
        Ok(())
    }

    /// Splits a file stem into (project, zone, instance_name).
    /// Returns None if the stem does not contain at least 2 underscores or any part is empty.
    pub fn split_file_stem(stem: &str) -> Option<(&str, &str, &str)> {
        let mut parts = stem.splitn(3, '_');
        let project = parts.next()?;
        let zone = parts.next()?;
        let name = parts.next()?;
        if project.is_empty() || zone.is_empty() || name.is_empty() {
            return None;
        }
        Some((project, zone, name))
    }

    /// Parses and validates an `Instance` from a file stem string.
    pub fn from_file_stem(stem: &str) -> Result<Self, InstanceError> {
        let (project, zone, name) = Self::split_file_stem(stem)
            .ok_or_else(|| InstanceError::InvalidFileStem { stem: stem.to_string() })?;
        Self::new(project, zone, name)
    }

    /// Parses and validates an `Instance` from a file path within `instance_root`.
    pub fn from_path(instance_root: &Path, path: &Path) -> Result<Self, InstanceError> {
        let rel = path.strip_prefix(instance_root).map_err(|_| InstanceError::NotInRoot)?;
        if rel.parent() != Some(Path::new("")) {
            return Err(InstanceError::NotDirectChild);
        }
        if rel.extension() != Some(std::ffi::OsStr::new("json")) {
            return Err(InstanceError::NotJsonFile);
        }
        let stem = rel.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
            InstanceError::InvalidFileStem { stem: path.to_string_lossy().to_string() }
        })?;
        Self::from_file_stem(stem)
    }

    /// Formats the instance identifier as a file stem (`{project}_{zone}_{name}`).
    pub fn to_file_stem(&self) -> String {
        format!("{}_{}_{}", self.project, self.zone, self.name)
    }

    /// Formats the instance state JSON file name (`{project}_{zone}_{name}.json`).
    pub fn to_file_name(&self) -> String {
        format!("{}_{}_{}.json", self.project, self.zone, self.name)
    }

    /// Returns the full path to the instance state JSON file in `root`.
    pub fn to_path(&self, root: &Path) -> PathBuf {
        root.join(self.to_file_name())
    }

    /// Reads instance data from the configured GCE instance directory.
    pub fn read(
        &self,
        ctx: &EnvironmentContext,
    ) -> Result<Option<GceInstanceData>, std::io::Error> {
        let root = instance_root(ctx)?;
        read_instance_file(&self.to_path(&root))
    }

    /// Writes instance data to the state file in the configured GCE instance directory, creating the directory if needed.
    pub fn write(
        &self,
        ctx: &EnvironmentContext,
        data: &GceInstanceData,
    ) -> Result<(), std::io::Error> {
        let root = instance_root(ctx)?;
        write_file_atomically(&self.to_path(&root), |writer| {
            serde_json::to_writer_pretty(writer, data)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        })
    }

    /// Stops the background SSH tunnel for this instance and removes its state file.
    pub fn stop(&self, ctx: &EnvironmentContext) -> Result<(), std::io::Error> {
        let root = instance_root(ctx)?;
        let path = self.to_path(&root);
        let read_res = read_instance_file(&path);
        if let Ok(Some(instance)) = &read_res {
            instance.terminate();
        }
        let remove_res = match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        };
        read_res?;
        remove_res
    }
}

/// Writes a file atomically by creating a temporary file in the target's parent directory,
/// invoking `write_fn` with a [`BufWriter`], flushing and syncing the underlying file,
/// and atomically renaming it to `path`.
pub fn write_file_atomically<F, E>(path: &Path, write_fn: F) -> Result<(), E>
where
    F: FnOnce(&mut BufWriter<&mut NamedTempFile>) -> Result<(), E>,
    E: From<std::io::Error>,
{
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(parent)?;
    let mut temp_file = NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(&mut temp_file);
        write_fn(&mut writer)?;
        writer.flush()?;
    }
    temp_file.as_file_mut().sync_all()?;
    temp_file.persist(path).map_err(|e| E::from(e.error))?;
    Ok(())
}

fn instance_root(ctx: &EnvironmentContext) -> Result<PathBuf, std::io::Error> {
    ctx.get::<PathBuf, _>(ffx_config::keys::GCE_INSTANCE_ROOT_DIR).map_err(std::io::Error::other)
}

impl std::fmt::Display for Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}_{}", self.project, self.zone, self.name)
    }
}

impl std::str::FromStr for Instance {
    type Err = InstanceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_file_stem(s)
    }
}

pub fn get_all_gce_targets(instance_root: &Path) -> Vec<TargetHandle> {
    GceSource.get_all_targets(instance_root).into_iter().map(|(_, h)| h).collect()
}

#[derive(Debug)]
pub struct GceSource;

impl InstanceSource for GceSource {
    fn recursive(&self) -> bool {
        false
    }

    fn instance_id_from_path(&self, instance_root: &Path, path: &Path) -> Option<String> {
        Instance::from_path(instance_root, path).ok().map(|i| i.to_string())
    }

    fn read_target_handle(&self, _instance_root: &Path, path: &Path) -> Option<TargetHandle> {
        read_instance_file(path).ok()??.to_target_handle()
    }
}

pub struct GceWatcher {
    _watcher: InstanceWatcher,
}

impl std::fmt::Debug for GceWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GceWatcher").finish_non_exhaustive()
    }
}

impl GceWatcher {
    pub fn new(
        instance_root: PathBuf,
        sender: UnboundedSender<TargetEvent>,
    ) -> Result<Self, Error> {
        let watcher = InstanceWatcher::new(instance_root, sender, GceSource, |path, err| {
            Error::GceWatcher { path, err }
        })?;
        Ok(Self { _watcher: watcher })
    }

    pub fn from_context(
        ctx: &EnvironmentContext,
        sender: UnboundedSender<TargetEvent>,
    ) -> Result<Self, Error> {
        let root: PathBuf = ctx
            .get(ffx_config::keys::GCE_INSTANCE_ROOT_DIR)
            .map_err(|e| Error::GceWatcher { path: PathBuf::new(), err: e.to_string() })?;
        Self::new(root, sender)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[fuchsia::test]
    fn test_read_instance_file_and_to_target_handle() {
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("test-project_us-central1-a_my-test-vm.json");
        let data = serde_json::json!({
            "instance_name": "my-test-vm",
            "project": "test-project",
            "zone": "us-central1-a",
            "pid": std::process::id(),
            "ssh_port": 2222,
            "serial_number": "GC-TESTSERIAL",
        });
        std::fs::write(&file_path, serde_json::to_string(&data).unwrap()).unwrap();

        let instance = read_instance_file(&file_path).unwrap().expect("instance parsed");
        assert_eq!(instance.instance_name, "my-test-vm");
        assert_eq!(instance.ssh_port, 2222);
        assert_eq!(instance.serial_number.as_deref(), Some("GC-TESTSERIAL"));
        assert!(instance.is_running());

        let handle = instance.to_target_handle().expect("target handle created");
        assert_eq!(handle.node_name.as_deref(), Some("my-test-vm"));
        assert!(!handle.manual);
        match handle.state {
            TargetState::Product { addrs, serial } => {
                assert_eq!(serial.as_deref(), Some("GC-TESTSERIAL"));
                assert_eq!(addrs, vec![TargetAddr::Net(SocketAddr::from(([127, 0, 0, 1], 2222)))]);
            }
            _ => panic!("expected Product target state"),
        }
    }

    #[fuchsia::test]
    fn test_stopped_instance_returns_none() {
        let data = GceInstanceData {
            instance_name: "dead-vm".to_string(),
            project: "test-project".to_string(),
            zone: "us-central1-a".to_string(),
            pid: 0,
            ssh_port: 2222,
            reverse_ports: vec![],
            serial_number: None,
        };
        assert!(!data.is_running());
        assert!(data.to_target_handle().is_none());
    }

    #[fuchsia::test]
    async fn test_gce_watcher_drain_and_watch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let file_path = root.join("test-proj_us-central1-a_running-vm.json");
        let data = serde_json::json!({
            "instance_name": "running-vm",
            "project": "test-proj",
            "zone": "us-central1-a",
            "pid": std::process::id(),
            "ssh_port": 3333,
            "serial_number": "GC-RUNNING123",
        });
        std::fs::write(&file_path, serde_json::to_string(&data).unwrap()).unwrap();

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let _watcher = GceWatcher::new(root.clone(), tx).expect("watcher created");

        // Should get the existing running instance right away
        let initial_event = rx.next().await.expect("received initial event");
        match initial_event {
            TargetEvent::Added(h) => {
                assert_eq!(h.node_name.as_deref(), Some("running-vm"));
                assert_eq!(
                    h.state,
                    TargetState::Product {
                        addrs: vec![TargetAddr::Net(SocketAddr::from(([127, 0, 0, 1], 3333)))],
                        serial: Some("GC-RUNNING123".to_string()),
                    }
                );
            }
            _ => panic!("expected Added event"),
        }

        // Delete the file
        std::fs::remove_file(&file_path).unwrap();

        // Should receive Removed event
        let remove_event = rx.next().await.expect("received remove event");
        match remove_event {
            TargetEvent::Removed(h) => {
                assert_eq!(h.node_name.as_deref(), Some("running-vm"));
            }
            _ => panic!("expected Removed event"),
        }
    }

    #[fuchsia::test]
    async fn test_stop_instance() {
        let temp = tempfile::tempdir().unwrap();
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .build()
            .expect("test env");
        let file_path = temp.path().join("test-proj_us-central1-a_test-stop.json");
        let mut child = std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let child_pid = child.id();

        let data = serde_json::json!({
            "instance_name": "test-stop",
            "project": "test-proj",
            "zone": "us-central1-a",
            "pid": child_pid,
            "ssh_port": 2222,
        });
        std::fs::write(&file_path, serde_json::to_string(&data).unwrap()).unwrap();

        let instance = Instance::new("test-proj", "us-central1-a", "test-stop").unwrap();
        assert!(instance.read(&env.context).unwrap().is_some());
        instance.stop(&env.context).unwrap();
        assert!(instance.read(&env.context).unwrap().is_none());
        assert!(!file_path.exists());

        let _ = child.wait();
    }

    #[fuchsia::test]
    async fn test_write_and_read_instance() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("nested_root");
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, root.to_str().unwrap())
            .build()
            .expect("test env");
        let instance = Instance::new("test-proj", "us-central1-a", "test-write").unwrap();
        let data = GceInstanceData {
            instance_name: "test-write".to_string(),
            project: "test-proj".to_string(),
            zone: "us-central1-a".to_string(),
            pid: 12345,
            ssh_port: 2222,
            reverse_ports: vec![8083],
            serial_number: Some("GC-123".to_string()),
        };

        instance.write(&env.context, &data).unwrap();
        assert!(instance.to_path(&root).exists());

        let read_data = instance.read(&env.context).unwrap().expect("instance read");
        assert_eq!(read_data, data);
    }

    #[fuchsia::test]
    fn test_instance_new_invalid_name() {
        assert!(Instance::new("test-proj", "us-central1-a", "invalid_name").is_err());
    }

    #[fuchsia::test]
    fn test_validate_name() {
        assert_eq!(Instance::validate_name("fuchsia-gce-vm"), Ok(()));
        assert_eq!(Instance::validate_name("a"), Ok(()));
        assert_eq!(Instance::validate_name("a1"), Ok(()));
        assert_eq!(Instance::validate_name("a-b-c-1-2-3"), Ok(()));
        assert_eq!(Instance::validate_name(&"a".repeat(63)), Ok(()));

        assert_eq!(Instance::validate_name(""), Err(InstanceError::Empty));
        assert_eq!(
            Instance::validate_name(&"a".repeat(64)),
            Err(InstanceError::TooLong { name: "a".repeat(64), len: 64 })
        );
        assert_eq!(
            Instance::validate_name("my_vm"),
            Err(InstanceError::ContainsUnderscore { name: "my_vm".to_string() })
        );
        assert_eq!(
            Instance::validate_name("_vm"),
            Err(InstanceError::ContainsUnderscore { name: "_vm".to_string() })
        );
        assert_eq!(
            Instance::validate_name("vm_"),
            Err(InstanceError::ContainsUnderscore { name: "vm_".to_string() })
        );
        assert_eq!(
            Instance::validate_name("My-Vm"),
            Err(InstanceError::InvalidStart { name: "My-Vm".to_string() })
        );
        assert_eq!(
            Instance::validate_name("1vm"),
            Err(InstanceError::InvalidStart { name: "1vm".to_string() })
        );
        assert_eq!(
            Instance::validate_name("-vm"),
            Err(InstanceError::InvalidStart { name: "-vm".to_string() })
        );
        assert_eq!(
            Instance::validate_name("vm-"),
            Err(InstanceError::InvalidEnd { name: "vm-".to_string() })
        );
        assert_eq!(
            Instance::validate_name("vm.test"),
            Err(InstanceError::InvalidCharacter { name: "vm.test".to_string(), char: '.' })
        );
    }

    #[fuchsia::test]
    fn test_instance_id_from_path() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let valid = root.join("my-project_us-central1-a_my-instance.json");
        assert_eq!(
            GceSource.instance_id_from_path(root, &valid),
            Some("my-project_us-central1-a_my-instance".to_string())
        );

        let legacy = root.join("my-instance.json");
        assert_eq!(GceSource.instance_id_from_path(root, &legacy), None);

        let not_json = root.join("my-project_us-central1-a_my-instance.txt");
        assert_eq!(GceSource.instance_id_from_path(root, &not_json), None);

        let too_few = root.join("my-project_my-instance.json");
        assert_eq!(GceSource.instance_id_from_path(root, &too_few), None);

        let empty_proj = root.join("_us-central1-a_my-instance.json");
        assert_eq!(GceSource.instance_id_from_path(root, &empty_proj), None);

        let empty_zone = root.join("my-project__my-instance.json");
        assert_eq!(GceSource.instance_id_from_path(root, &empty_zone), None);

        let empty_name = root.join("my-project_us-central1-a_.json");
        assert_eq!(GceSource.instance_id_from_path(root, &empty_name), None);
    }

    #[fuchsia::test]
    async fn test_gce_watcher_same_name_different_zones() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let file1 = root.join("proj-1_zone-a_shared-name.json");
        let file2 = root.join("proj-2_zone-b_shared-name.json");
        let data1 = serde_json::json!({
            "instance_name": "shared-name",
            "project": "proj-1",
            "zone": "zone-a",
            "pid": std::process::id(),
            "ssh_port": 1111,
        });
        let data2 = serde_json::json!({
            "instance_name": "shared-name",
            "project": "proj-2",
            "zone": "zone-b",
            "pid": std::process::id(),
            "ssh_port": 2222,
        });
        std::fs::write(&file1, serde_json::to_string(&data1).unwrap()).unwrap();
        std::fs::write(&file2, serde_json::to_string(&data2).unwrap()).unwrap();

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let _watcher = GceWatcher::new(root.clone(), tx).expect("watcher created");

        // Should receive both Added events (in either order)
        let event1 = rx.next().await.expect("event 1");
        let event2 = rx.next().await.expect("event 2");
        let mut ports = vec![];
        for e in [event1, event2] {
            if let TargetEvent::Added(h) = e {
                assert_eq!(h.node_name.as_deref(), Some("shared-name"));
                if let TargetState::Product { addrs, .. } = h.state {
                    if let TargetAddr::Net(addr) = &addrs[0] {
                        ports.push(addr.port());
                    }
                }
            }
        }
        ports.sort();
        assert_eq!(ports, vec![1111, 2222]);

        // Remove the first file
        std::fs::remove_file(&file1).unwrap();
        let remove_event = rx.next().await.expect("remove event");
        if let TargetEvent::Removed(h) = remove_event {
            assert_eq!(h.node_name.as_deref(), Some("shared-name"));
            if let TargetState::Product { addrs, .. } = h.state {
                if let TargetAddr::Net(addr) = &addrs[0] {
                    assert_eq!(addr.port(), 1111);
                }
            }
        } else {
            panic!("expected Removed event");
        }

        // Remove the second file
        std::fs::remove_file(&file2).unwrap();
        let remove_event2 = rx.next().await.expect("remove event 2");
        if let TargetEvent::Removed(h) = remove_event2 {
            assert_eq!(h.node_name.as_deref(), Some("shared-name"));
            if let TargetState::Product { addrs, .. } = h.state {
                if let TargetAddr::Net(addr) = &addrs[0] {
                    assert_eq!(addr.port(), 2222);
                }
            }
        } else {
            panic!("expected Removed event 2");
        }
    }

    #[fuchsia::test]
    fn test_instance_validation() {
        let valid = Instance::new("my-project", "us-central1-a", "my-vm");
        assert!(valid.is_ok());
        let inst = valid.unwrap();
        assert_eq!(inst.project, "my-project");
        assert_eq!(inst.zone, "us-central1-a");
        assert_eq!(inst.name, "my-vm");

        // Project validation
        assert_eq!(Instance::new("", "us-central1-a", "my-vm"), Err(InstanceError::ProjectEmpty));
        assert_eq!(
            Instance::new("my_project", "us-central1-a", "my-vm"),
            Err(InstanceError::ProjectContainsUnderscore { project: "my_project".to_string() })
        );
        assert_eq!(
            Instance::new("my-project!", "us-central1-a", "my-vm"),
            Err(InstanceError::ProjectInvalidCharacter {
                project: "my-project!".to_string(),
                char: '!'
            })
        );

        // Zone validation
        assert_eq!(Instance::new("my-project", "", "my-vm"), Err(InstanceError::ZoneEmpty));
        assert_eq!(
            Instance::new("my-project", "us_central1_a", "my-vm"),
            Err(InstanceError::ZoneContainsUnderscore { zone: "us_central1_a".to_string() })
        );
        assert_eq!(
            Instance::new("my-project", "us-central1-a!", "my-vm"),
            Err(InstanceError::ZoneInvalidCharacter {
                zone: "us-central1-a!".to_string(),
                char: '!'
            })
        );

        // Name validation
        assert_eq!(Instance::new("my-project", "us-central1-a", ""), Err(InstanceError::Empty));
        assert_eq!(
            Instance::new("my-project", "us-central1-a", "my_vm"),
            Err(InstanceError::ContainsUnderscore { name: "my_vm".to_string() })
        );
        assert_eq!(
            Instance::new("my-project", "us-central1-a", &"a".repeat(64)),
            Err(InstanceError::TooLong { name: "a".repeat(64), len: 64 })
        );
        assert_eq!(
            Instance::new("my-project", "us-central1-a", "1vm"),
            Err(InstanceError::InvalidStart { name: "1vm".to_string() })
        );
        assert_eq!(
            Instance::new("my-project", "us-central1-a", "vm-"),
            Err(InstanceError::InvalidEnd { name: "vm-".to_string() })
        );
    }

    #[fuchsia::test]
    fn test_instance_methods_and_paths() {
        let inst = Instance::new("proj-1", "us-west1-b", "fuchsia-dev").unwrap();
        assert_eq!(inst.to_file_stem(), "proj-1_us-west1-b_fuchsia-dev");
        assert_eq!(inst.to_file_name(), "proj-1_us-west1-b_fuchsia-dev.json");
        assert_eq!(inst.to_string(), "proj-1_us-west1-b_fuchsia-dev");

        let parsed: Instance = "proj-1_us-west1-b_fuchsia-dev".parse().unwrap();
        assert_eq!(parsed, inst);

        let root = Path::new("/tmp/gce_instances");
        assert_eq!(
            inst.to_path(root),
            PathBuf::from("/tmp/gce_instances/proj-1_us-west1-b_fuchsia-dev.json")
        );

        // split_file_stem
        assert_eq!(
            Instance::split_file_stem("proj-1_us-west1-b_fuchsia-dev"),
            Some(("proj-1", "us-west1-b", "fuchsia-dev"))
        );
        assert_eq!(
            Instance::split_file_stem("p_z_name_with_underscores"),
            Some(("p", "z", "name_with_underscores"))
        );
        assert_eq!(Instance::split_file_stem("p_z"), None);
        assert_eq!(Instance::split_file_stem("_z_n"), None);
        assert_eq!(Instance::split_file_stem("p__n"), None);
        assert_eq!(Instance::split_file_stem("p_z_"), None);

        // from_file_stem
        assert_eq!(Instance::from_file_stem("proj-1_us-west1-b_fuchsia-dev"), Ok(inst.clone()));
        assert!(Instance::from_file_stem("invalid_format").is_err());
        assert!(Instance::from_file_stem("p_z_vm_with_underscores").is_err());

        // from_path
        let temp = tempfile::tempdir().unwrap();
        let instance_root = temp.path();
        let valid_path = inst.to_path(instance_root);
        assert_eq!(Instance::from_path(instance_root, &valid_path), Ok(inst));

        let wrong_ext = instance_root.join("proj-1_us-west1-b_fuchsia-dev.txt");
        assert_eq!(Instance::from_path(instance_root, &wrong_ext), Err(InstanceError::NotJsonFile));

        let sub_dir = instance_root.join("nested").join("proj-1_us-west1-b_fuchsia-dev.json");
        assert_eq!(
            Instance::from_path(instance_root, &sub_dir),
            Err(InstanceError::NotDirectChild)
        );

        let outside = Path::new("/other/path/proj-1_us-west1-b_fuchsia-dev.json");
        assert_eq!(Instance::from_path(instance_root, outside), Err(InstanceError::NotInRoot));
    }

    #[fuchsia::test]
    fn test_instance_stop_removes_corrupt_file() {
        let temp = tempfile::tempdir().unwrap();
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .build()
            .unwrap();
        let inst = Instance::new("my-project", "us-central1-a", "corrupt-vm").unwrap();
        let path = inst.to_path(temp.path());
        std::fs::write(&path, b"corrupted non-json data").unwrap();
        assert!(path.exists());

        assert!(inst.stop(&env.context).is_err());
        assert!(!path.exists());
    }
}
