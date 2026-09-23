// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use discovery::TargetEvent;
use discovery::gce_watcher::{self, GceInstanceData, GceWatcher};
use ffx_config::EnvironmentContext;
use ffx_config::logging::LogDirHandling;
use ffx_ssh::SshKeyFiles;
use fuchsia_async::{MonotonicInstant, TimeoutExt, Timer};
use futures::StreamExt;
use futures::channel::mpsc;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// RAII guard for a spawned [`Child`] process that kills the process group and reaps
/// the child process on `Drop` unless explicitly detached via [`ChildGuard::detach`].
struct ChildGuard {
    child: Option<Child>,
    pgid: Option<i32>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        let pgid = i32::try_from(child.id()).ok();
        Self { child: Some(child), pgid }
    }

    fn id(&self) -> u32 {
        self.child.as_ref().map(|c| c.id()).unwrap_or(0)
    }

    /// Checks whether the child process has exited or is no longer valid, reaping its exit status if available.
    fn has_exited(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {
                    self.child = None;
                    return true;
                }
                Ok(None) => return false,
            }
        }
        true
    }

    /// Consumes the guard without killing the child process or process group, returning
    /// the underlying [`Child`] so it can continue running in the background.
    fn detach(mut self) -> Option<Child> {
        self.pgid = None;
        self.child.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(-pgid),
                Some(nix::sys::signal::Signal::SIGKILL),
            );
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// RAII guard for an active tunnel attempt whose instance state file has been written to disk.
///
/// Unless [`ActiveTunnelGuard::commit`] is called, dropping this guard will:
/// 1. Terminate and reap the spawned tunnel process group (via [`ChildGuard`]).
/// 2. Remove the instance state file from disk (via [`gce_watcher::Instance::stop`]).
struct ActiveTunnelGuard<'a> {
    ctx: &'a EnvironmentContext,
    instance: &'a gce_watcher::Instance,
    child: Option<ChildGuard>,
}

impl<'a> ActiveTunnelGuard<'a> {
    fn new(
        ctx: &'a EnvironmentContext,
        instance: &'a gce_watcher::Instance,
        child: ChildGuard,
        data: &GceInstanceData,
    ) -> Result<Self> {
        instance
            .write(ctx, data)
            .with_context(|| format!("Failed to write GCE instance state for {}", instance.name))?;
        Ok(Self { ctx, instance, child: Some(child) })
    }

    fn has_exited(&mut self) -> bool {
        self.child.as_mut().map_or(true, |c| c.has_exited())
    }

    fn commit(mut self) -> Option<Child> {
        self.child.take().and_then(|c| c.detach())
    }
}

impl Drop for ActiveTunnelGuard<'_> {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            drop(child);
            let _ = self.instance.stop(self.ctx);
        }
    }
}

fn is_discovered_by_watcher(ctx: &EnvironmentContext, instance_name: &str) -> bool {
    let (tx, mut rx) = mpsc::unbounded();
    let Ok(_watcher) = GceWatcher::from_context(ctx, tx) else {
        return false;
    };
    while let Ok(Some(event)) = rx.try_next() {
        if let TargetEvent::Added(handle) = event {
            if handle.node_name.as_deref() == Some(instance_name) {
                return true;
            }
        }
    }
    false
}

fn is_sso_credential_error(log_content: &str) -> bool {
    log_content.contains("got stuck at SSO")
        || log_content.contains("login page detected")
        || log_content.contains("try running gcert")
}

/// Maximum number of SSH connection attempts before giving up.
const MAX_TUNNEL_ATTEMPTS: usize = 12;

/// Delay between successive SSH connection attempts.
const RETRY_DELAY: Duration = Duration::from_secs(3);

/// Maximum number of polls checking whether the local tunnel port is accepting connections.
const MAX_PORT_CHECK_POLLS: usize = 60;

/// Interval between local port connection checks.
const PORT_CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// Timeout waiting for [`GceWatcher`] to report the newly added instance target handle.
const WATCHER_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration options for establishing a local background SSH tunnel to a GCE instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GceTunnelConfig {
    pub project: String,
    pub zone: String,
    pub instance_name: String,
    pub private_key: Option<PathBuf>,
    pub reverse_ports: Vec<u16>,
    pub serial_number: Option<String>,
    pub ssh_binary: PathBuf,
}

impl GceTunnelConfig {
    /// Creates a new tunnel configuration with reverse forwarding for the package repository server
    /// if configured in `ctx`.
    pub fn new(
        ctx: &EnvironmentContext,
        project: impl Into<String>,
        zone: impl Into<String>,
        instance_name: impl Into<String>,
    ) -> Result<Self> {
        let reverse_ports = pkg::config::repository_listen_addr(ctx)?
            .map(|addr| vec![addr.port()])
            .unwrap_or_default();
        Ok(Self {
            project: project.into(),
            zone: zone.into(),
            instance_name: instance_name.into(),
            private_key: None,
            reverse_ports,
            serial_number: None,
            ssh_binary: PathBuf::from("ssh"),
        })
    }

    /// Overrides the SSH private key path used to authenticate.
    pub fn with_private_key(mut self, private_key: impl Into<PathBuf>) -> Self {
        self.private_key = Some(private_key.into());
        self
    }

    /// Sets the list of reverse forwarded TCP ports (`-R <port>:localhost:<port>`).
    pub fn with_reverse_ports(mut self, reverse_ports: Vec<u16>) -> Self {
        self.reverse_ports = reverse_ports;
        self
    }

    /// Sets the instance serial number (`GC-...`) to record in the discovery target handle.
    pub fn with_serial_number(mut self, serial_number: impl Into<String>) -> Self {
        self.serial_number = Some(serial_number.into());
        self
    }

    /// Overrides the SSH executable path (defaults to `"ssh"`).
    pub fn with_ssh_binary(mut self, ssh_binary: impl Into<PathBuf>) -> Self {
        self.ssh_binary = ssh_binary.into();
        self
    }
}

/// Helper for managing a local background TCP forwarding tunnel to a GCE instance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GceTunnel;

impl GceTunnel {
    /// Starts a background SSH port forwarding tunnel process to GCE VM port 22 and waits for it
    /// to bind a local port and be discovered by [`GceWatcher`].
    ///
    /// By default, sets up reverse port forwarding for the package repository port configured in `ctx`.
    pub async fn start_tunnel(
        ctx: &EnvironmentContext,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<GceInstanceData> {
        let config = GceTunnelConfig::new(ctx, project, zone, instance_name)?;
        Self::start_tunnel_with_config(ctx, config).await
    }

    /// Starts a background SSH port forwarding tunnel process using the provided [`GceTunnelConfig`],
    /// writes the instance state file, and waits for [`GceWatcher`] to discover the target.
    pub async fn start_tunnel_with_config(
        ctx: &EnvironmentContext,
        config: GceTunnelConfig,
    ) -> Result<GceInstanceData> {
        let instance =
            gce_watcher::Instance::new(&config.project, &config.zone, &config.instance_name)?;

        match instance.read(ctx) {
            Ok(Some(data)) => {
                let config_satisfied =
                    config.reverse_ports.iter().all(|p| data.reverse_ports.contains(p))
                        && (config.serial_number.is_none()
                            || data.serial_number == config.serial_number);
                if config_satisfied
                    && data.is_running()
                    && std::net::TcpStream::connect(("127.0.0.1", data.ssh_port)).is_ok()
                    && is_discovered_by_watcher(ctx, &instance.name)
                {
                    log::info!(
                        "Reusing active GCE SSH tunnel for {} (pid {}, port {})",
                        instance.name,
                        data.pid,
                        data.ssh_port
                    );
                    return Ok(data);
                }
                log::info!(
                    "Cleaning up stale GCE SSH tunnel state for {} (pid {}, port {})",
                    instance.name,
                    data.pid,
                    data.ssh_port
                );
                if let Err(e) = instance.stop(ctx) {
                    log::warn!("Failed to clean up stale GCE tunnel for {}: {e}", instance.name);
                }
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!(
                    "Cleaning up unreadable GCE instance state file for {}: {e}",
                    instance.name
                );
                let _ = instance.stop(ctx);
            }
        }

        let private_key = match &config.private_key {
            Some(key) => key.clone(),
            None => {
                let ssh_keys = SshKeyFiles::load(ctx)
                    .map_err(|e| anyhow!("Failed to load SSH key configuration: {e}"))?;
                ssh_keys
                    .create_keys_if_needed(false)
                    .map_err(|e| anyhow!("Failed to create SSH keys if needed: {e}"))?;
                ssh_keys.private_key
            }
        };

        let log_filename = PathBuf::from(format!("gce_{}.log", instance.to_file_stem()));
        let (log_file, log_file_path) = ffx_config::logging::log_file_with_info(
            ctx,
            &log_filename,
            LogDirHandling::WithDirWithRotate,
        )?;

        let gcpnode_host = format!(
            "nic0.{}.{}.c.{}.internal.gcpnode.com",
            instance.name, instance.zone, instance.project
        );

        let mut last_err = String::new();

        for attempt in 0..MAX_TUNNEL_ATTEMPTS {
            if attempt > 0 {
                Timer::new(RETRY_DELAY).await;
                let _ = log_file.set_len(0);
            }

            let (tx, mut rx) = mpsc::unbounded();
            let _watcher = GceWatcher::from_context(ctx, tx)
                .map_err(|e| anyhow!("Failed to initialize GceWatcher: {e}"))?;

            let port = port_picker::pick_unused_port()
                .ok_or_else(|| anyhow!("Failed to pick an unused local TCP port"))?;

            let stdout_log = log_file
                .try_clone()
                .map_err(|e| anyhow!("Failed to clone log file handle: {e}"))?;
            let stderr_log = log_file
                .try_clone()
                .map_err(|e| anyhow!("Failed to clone log file handle: {e}"))?;

            let mut cmd = Command::new(&config.ssh_binary);
            cmd.arg("-N").arg("-L").arg(format!("{port}:localhost:22"));

            for &rport in &config.reverse_ports {
                cmd.arg("-R").arg(format!("{rport}:localhost:{rport}"));
            }

            cmd.arg("-i")
                .arg(&private_key)
                .arg("-o")
                .arg("StrictHostKeyChecking=no")
                .arg("-o")
                .arg("UserKnownHostsFile=/dev/null")
                .arg("-o")
                .arg("ExitOnForwardFailure=yes")
                .arg(&gcpnode_host)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout_log))
                .stderr(Stdio::from(stderr_log))
                .process_group(0);

            let mut child = match cmd.spawn() {
                Ok(c) => ChildGuard::new(c),
                Err(e) => {
                    last_err = format!("Failed to spawn SSH tunnel: {e}");
                    log::warn!(
                        "GCE SSH tunnel attempt {}/{} for {} failed: {}",
                        attempt + 1,
                        MAX_TUNNEL_ATTEMPTS,
                        instance.name,
                        last_err
                    );
                    continue;
                }
            };

            let mut port_open = false;
            for _ in 0..MAX_PORT_CHECK_POLLS {
                if child.has_exited() {
                    break;
                }

                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    port_open = true;
                    break;
                }
                Timer::new(PORT_CHECK_INTERVAL).await;
            }

            if !port_open {
                drop(child);
                last_err = std::fs::read_to_string(&log_file_path).unwrap_or_default();
                if is_sso_credential_error(&last_err) {
                    bail!(
                        "Corp SSH relay requires fresh SSO credentials. Please run `gcert` in your terminal and try again."
                    );
                }
                log::warn!(
                    "GCE SSH tunnel attempt {}/{} for {} failed to bind port {}: {}",
                    attempt + 1,
                    MAX_TUNNEL_ATTEMPTS,
                    instance.name,
                    port,
                    last_err.trim()
                );
                continue;
            }

            let data = GceInstanceData {
                instance_name: instance.name.clone(),
                project: instance.project.clone(),
                zone: instance.zone.clone(),
                pid: child.id(),
                ssh_port: port,
                reverse_ports: config.reverse_ports.clone(),
                serial_number: config.serial_number.clone(),
            };

            let mut tunnel_guard = ActiveTunnelGuard::new(ctx, &instance, child, &data)?;

            // Wait for GceWatcher to observe and emit TargetEvent::Added for the new instance,
            // while monitoring the child process in case it exits prematurely.
            let mut confirmed = false;
            let deadline = MonotonicInstant::now() + WATCHER_TIMEOUT;
            while MonotonicInstant::now() < deadline {
                if tunnel_guard.has_exited() {
                    break;
                }
                let step_deadline =
                    std::cmp::min(deadline, MonotonicInstant::now() + PORT_CHECK_INTERVAL);
                if let Some(TargetEvent::Added(handle)) =
                    rx.next().on_timeout(step_deadline, || None).await
                {
                    if handle.node_name.as_deref() == Some(&instance.name) {
                        log::debug!("GceWatcher confirmed target handle for {}", instance.name);
                        confirmed = true;
                        break;
                    }
                }
            }

            if !confirmed
                || tunnel_guard.has_exited()
                || std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
            {
                drop(tunnel_guard);
                let log_err = std::fs::read_to_string(&log_file_path).unwrap_or_default();
                if is_sso_credential_error(&log_err) {
                    bail!(
                        "Corp SSH relay requires fresh SSO credentials. Please run `gcert` in your terminal and try again."
                    );
                }
                let err_detail = log_err.trim();
                last_err = if !confirmed {
                    if err_detail.is_empty() {
                        format!("Timed out waiting for GceWatcher to discover {}", instance.name)
                    } else {
                        format!("SSH tunnel exited before GceWatcher discovery: {err_detail}")
                    }
                } else if err_detail.is_empty() {
                    format!(
                        "SSH tunnel for {} exited unexpectedly after binding port",
                        instance.name
                    )
                } else {
                    format!(
                        "SSH tunnel for {} exited unexpectedly after binding port: {err_detail}",
                        instance.name
                    )
                };
                log::warn!(
                    "GCE SSH tunnel attempt {}/{} for {} failed: {}",
                    attempt + 1,
                    MAX_TUNNEL_ATTEMPTS,
                    instance.name,
                    last_err
                );
                continue;
            }

            let _ = tunnel_guard.commit();
            return Ok(data);
        }

        let err_detail = last_err.trim();
        if err_detail.is_empty() {
            bail!(
                "Failed to establish SSH tunnel after {MAX_TUNNEL_ATTEMPTS} attempts (see log at {})",
                log_file_path.display()
            );
        } else {
            bail!(
                "Failed to establish SSH tunnel after {MAX_TUNNEL_ATTEMPTS} attempts: {err_detail}"
            );
        }
    }

    /// Stops the tunnel process associated with an instance if running and removes its state file.
    pub fn stop_tunnel(
        ctx: &EnvironmentContext,
        project: &str,
        zone: &str,
        instance_name: &str,
    ) -> Result<()> {
        let instance = gce_watcher::Instance::new(project, zone, instance_name)?;
        instance.stop(ctx)?;
        Ok(())
    }
}

/// Reads public key content from FFX SSH key configuration (`SshKeyFiles`).
/// Returns a list of public keys from the configured `authorized_keys` file.
pub fn read_gce_ssh_pubkeys(ctx: &EnvironmentContext) -> Result<Vec<String>> {
    let ssh_keys =
        SshKeyFiles::load(ctx).map_err(|e| anyhow!("Failed to load SSH key configuration: {e}"))?;
    ssh_keys
        .create_keys_if_needed(false)
        .map_err(|e| anyhow!("Failed to create SSH keys if needed: {e}"))?;
    let content = std::fs::read_to_string(&ssh_keys.authorized_keys)
        .with_context(|| format!("Failed to read {}", ssh_keys.authorized_keys.display()))?;
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect())
}

/// Reads the first available public key content from FFX SSH key configuration (`SshKeyFiles`).
pub fn read_gce_ssh_pubkey(ctx: &EnvironmentContext) -> Result<Option<String>> {
    Ok(read_gce_ssh_pubkeys(ctx)?.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use discovery::instance_watcher::is_pid_running;
    use std::os::unix::fs::PermissionsExt as _;

    #[fuchsia::test]
    async fn test_tunnel_config_uses_repository_listen_addr() {
        let env = ffx_config::test_env()
            .runtime_config("repository.server.listen", "127.0.0.1:9090")
            .build()
            .expect("test env");
        let config =
            GceTunnelConfig::new(&env.context, "my-project", "us-central1-a", "test-vm").unwrap();
        assert_eq!(config.reverse_ports, vec![9090]);
        assert_eq!(config.ssh_binary, PathBuf::from("ssh"));
    }

    #[fuchsia::test]
    async fn test_child_guard_kills_on_drop_and_preserves_on_detach() {
        let child = Command::new("/bin/sleep").arg("60").spawn().expect("spawn sleep");
        let pid = child.id();
        assert!(is_pid_running(pid));
        {
            let _guard = ChildGuard::new(child);
        }
        assert!(!is_pid_running(pid));

        let child2 = Command::new("/bin/sleep").arg("60").spawn().expect("spawn sleep 2");
        let pid2 = child2.id();
        let detached = {
            let guard2 = ChildGuard::new(child2);
            guard2.detach()
        };
        assert!(is_pid_running(pid2));
        if let Some(mut child) = detached {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(!is_pid_running(pid2));
    }

    #[fuchsia::test]
    async fn test_read_gce_ssh_pubkeys_from_config() {
        let temp = tempfile::tempdir().expect("temp dir");
        let auth_keys_path = temp.path().join("authorized_keys");
        let priv_key_path = temp.path().join("private_key");
        std::fs::write(
            &auth_keys_path,
            "# comment\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI test@fuchsia\n",
        )
        .expect("write auth keys");

        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::SSH_PUB_KEY, auth_keys_path.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PRIVATE_KEY, priv_key_path.to_str().unwrap())
            .build()
            .expect("test env");

        let keys = read_gce_ssh_pubkeys(&env.context).expect("read pubkeys");
        assert!(keys.contains(&"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI test@fuchsia".to_string()));
        assert_eq!(
            read_gce_ssh_pubkey(&env.context).expect("read pubkey").as_deref(),
            Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI test@fuchsia")
        );
    }

    #[fuchsia::test]
    async fn test_start_tunnel_reuses_running_instance_via_gce_watcher() {
        let temp = tempfile::tempdir().expect("temp dir");
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .build()
            .expect("test env");

        let mut child = Command::new("/bin/sleep").arg("60").spawn().expect("spawn sleep");
        let pid = child.id();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock ssh port");
        let port = listener.local_addr().unwrap().port();

        let instance =
            gce_watcher::Instance::new("my-project", "us-central1-a", "running-vm").unwrap();
        let data = GceInstanceData {
            instance_name: "running-vm".to_string(),
            project: "my-project".to_string(),
            zone: "us-central1-a".to_string(),
            pid,
            ssh_port: port,
            reverse_ports: vec![8083],
            serial_number: Some("GC-REUSE123".to_string()),
        };
        instance.write(&env.context, &data).expect("write instance data");

        // Starting the tunnel should detect the running healthy instance via GceWatcher and reuse it immediately.
        let reused =
            GceTunnel::start_tunnel(&env.context, "my-project", "us-central1-a", "running-vm")
                .await
                .expect("reuse running tunnel");
        assert_eq!(reused, data);

        drop(listener);
        GceTunnel::stop_tunnel(&env.context, "my-project", "us-central1-a", "running-vm")
            .expect("stop tunnel");
        let _ = child.wait();
    }

    #[fuchsia::test]
    async fn test_start_tunnel_spawns_and_discovers_via_gce_watcher() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");
        let auth_keys_path = temp.path().join("authorized_keys");
        let priv_key_path = temp.path().join("private_key");

        let mock_ssh_path = temp.path().join("mock_ssh.py");
        let script = r#"#!/usr/bin/python3
import socket
import sys
import time

port = None
args = sys.argv[1:]
for i, arg in enumerate(args):
    if arg == "-L" and i + 1 < len(args):
        port = int(args[i + 1].split(":")[0])
        break

if port is not None:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    while True:
        time.sleep(1)
"#;
        std::fs::write(&mock_ssh_path, script).expect("write mock ssh script");
        std::fs::set_permissions(&mock_ssh_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod mock ssh script");

        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .runtime_config("log.dir", log_dir.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PUB_KEY, auth_keys_path.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PRIVATE_KEY, priv_key_path.to_str().unwrap())
            .build()
            .expect("test env");

        let config =
            GceTunnelConfig::new(&env.context, "my-project", "us-central1-a", "spawned-vm")
                .expect("tunnel config")
                .with_serial_number("GC-SPAWN123")
                .with_ssh_binary(&mock_ssh_path);

        let started = GceTunnel::start_tunnel_with_config(&env.context, config)
            .await
            .expect("start tunnel with mock ssh");

        assert_eq!(started.instance_name, "spawned-vm");
        assert_eq!(started.project, "my-project");
        assert_eq!(started.zone, "us-central1-a");
        assert_eq!(started.serial_number.as_deref(), Some("GC-SPAWN123"));
        assert!(started.is_running());

        GceTunnel::stop_tunnel(&env.context, "my-project", "us-central1-a", "spawned-vm")
            .expect("stop spawned tunnel");
    }

    #[fuchsia::test]
    async fn test_start_tunnel_retries_and_cleans_up_when_tunnel_exits_after_bind() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");
        let auth_keys_path = temp.path().join("authorized_keys");
        let priv_key_path = temp.path().join("private_key");
        let counter_path = temp.path().join("attempt_count");

        let mock_ssh_path = temp.path().join("mock_ssh_flaky.py");
        let script = format!(
            r#"#!/usr/bin/python3
import socket
import sys
import time

counter_file = "{}"
try:
    with open(counter_file, "r") as f:
        attempt = int(f.read().strip())
except Exception:
    attempt = 0

attempt += 1
with open(counter_file, "w") as f:
    f.write(str(attempt))

port = None
args = sys.argv[1:]
for i, arg in enumerate(args):
    if arg == "-L" and i + 1 < len(args):
        port = int(args[i + 1].split(":")[0])
        break

if port is not None:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port))
    s.listen(5)
    if attempt == 1:
        # Accept the initial port_open probe connection, then immediately close and exit
        # to simulate OpenSSH binding locally and then failing remote port forwarding.
        conn, _ = s.accept()
        conn.close()
        s.close()
        sys.stderr.write("remote port forwarding failed\n")
        sys.exit(255)
    while True:
        time.sleep(1)
"#,
            counter_path.display()
        );
        std::fs::write(&mock_ssh_path, script).expect("write flaky mock ssh script");
        std::fs::set_permissions(&mock_ssh_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod flaky mock ssh script");

        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .runtime_config("log.dir", log_dir.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PUB_KEY, auth_keys_path.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PRIVATE_KEY, priv_key_path.to_str().unwrap())
            .build()
            .expect("test env");

        let config = GceTunnelConfig::new(&env.context, "my-project", "us-central1-a", "flaky-vm")
            .expect("tunnel config")
            .with_ssh_binary(&mock_ssh_path);

        let started = GceTunnel::start_tunnel_with_config(&env.context, config)
            .await
            .expect("start tunnel should succeed on retry");

        assert!(started.is_running());
        let attempts: usize =
            std::fs::read_to_string(&counter_path).unwrap().trim().parse().unwrap();
        assert_eq!(attempts, 2);

        GceTunnel::stop_tunnel(&env.context, "my-project", "us-central1-a", "flaky-vm")
            .expect("stop flaky tunnel");
    }

    #[fuchsia::test]
    async fn test_start_tunnel_cleans_up_corrupt_state_file_and_leaves_no_stale_file_on_failure() {
        let temp = tempfile::tempdir().expect("temp dir");
        let log_dir = temp.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");
        let auth_keys_path = temp.path().join("authorized_keys");
        let priv_key_path = temp.path().join("private_key");

        let mock_ssh_path = temp.path().join("mock_ssh_fail.py");
        let script = r#"#!/usr/bin/python3
import sys
sys.stderr.write("ERROR: try running gcert\n")
sys.exit(255)
"#;
        std::fs::write(&mock_ssh_path, script).expect("write failing mock ssh script");
        std::fs::set_permissions(&mock_ssh_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod failing mock ssh script");

        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::GCE_INSTANCE_ROOT_DIR, temp.path().to_str().unwrap())
            .runtime_config("log.dir", log_dir.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PUB_KEY, auth_keys_path.to_str().unwrap())
            .runtime_config(ffx_config::keys::SSH_PRIVATE_KEY, priv_key_path.to_str().unwrap())
            .build()
            .expect("test env");

        let instance =
            gce_watcher::Instance::new("my-project", "us-central1-a", "corrupt-fail-vm").unwrap();
        let state_path = instance.to_path(temp.path());
        std::fs::write(&state_path, b"corrupt non-json pre-existing state").unwrap();
        assert!(state_path.exists());

        let config =
            GceTunnelConfig::new(&env.context, "my-project", "us-central1-a", "corrupt-fail-vm")
                .expect("tunnel config")
                .with_ssh_binary(&mock_ssh_path);

        let res = GceTunnel::start_tunnel_with_config(&env.context, config).await;
        assert!(res.is_err());
        assert!(!state_path.exists(), "corrupt pre-existing state file must be cleaned up");
    }
}
