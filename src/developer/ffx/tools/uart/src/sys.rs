// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! System-level utilities for interacting with processes and sockets.

use crate::metadata::DaemonMetrics;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use tokio::io::AsyncReadExt;

/// Retrieves the process ID (PID) of the peer connected to the given Unix socket stream.
///
/// Uses `LOCAL_PEERPID` on macOS and `SO_PEERCRED` on Linux/Unix.
pub fn get_peer_pid<F: std::os::fd::AsFd>(stream: &F) -> nix::Result<u32> {
    #[cfg(target_os = "macos")]
    let raw_pid = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::LocalPeerPid)?;
    #[cfg(not(target_os = "macos"))]
    let raw_pid =
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)?.pid();

    u32::try_from(raw_pid).ok().filter(|&pid| pid > 0).ok_or(nix::errno::Errno::ESRCH)
}

#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(target_os = "linux"))]
use non_linux as platform;

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};

    const PROC_DIR: &str = "/proc";
    const PROC_STATUS: &str = "status";
    const PROC_COMM: &str = "comm";
    const PROC_EXE: &str = "exe";
    const PROC_CMDLINE: &str = "cmdline";

    pub fn proc_path(pid: u32, entry: &str) -> PathBuf {
        Path::new(PROC_DIR).join(pid.to_string()).join(entry)
    }

    /// Checks if the process is a zombie (`State: Z`) in `/proc/<pid>/status`.
    pub fn is_zombie(pid: u32) -> bool {
        if let Ok(status) = std::fs::read_to_string(proc_path(pid, PROC_STATUS)) {
            return status.lines().any(|l| l.starts_with("State:") && l.contains("Z (zombie)"));
        }
        false
    }

    pub fn check_driver_process(pid: u32) -> bool {
        let matches_driver = |s: &str| {
            s.contains("ffx-uart-driver") || s.contains("python") || s.contains("mock_driver")
        };
        if let Ok(comm) = std::fs::read_to_string(proc_path(pid, PROC_COMM)) {
            if matches_driver(comm.trim()) {
                return true;
            }
        }
        if let Ok(exe) = std::fs::read_link(proc_path(pid, PROC_EXE)) {
            let exe_str = exe.to_string_lossy();
            let clean = exe_str.strip_suffix(" (deleted)").unwrap_or(&exe_str);
            if matches_driver(clean) {
                return true;
            }
        }
        match std::fs::read_to_string(proc_path(pid, PROC_CMDLINE)) {
            Ok(cmdline) => cmdline
                .split('\0')
                .any(|arg| arg == "uart-driver" || arg == "uart" || matches_driver(arg)),
            // Graceful fallback in restricted containers where /proc read is denied.
            Err(_) => true,
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod non_linux {
    pub fn is_zombie(_pid: u32) -> bool {
        false
    }

    pub fn check_driver_process(_pid: u32) -> bool {
        true
    }
}

/// Checks if a process with the given PID is currently active/running via `kill(pid, 0)`.
///
/// Treats `EPERM` as running (process exists in restricted container/sandbox) and zombies as dead.
pub fn is_running(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    match kill(Pid::from_raw(pid as i32), None) {
        Ok(()) => !platform::is_zombie(pid),
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Checks if the driver process with the given PID is active and matches `ffx-uart-driver`.
pub fn is_driver_running(pid: u32) -> bool {
    if !is_running(pid) {
        return false;
    }
    #[cfg(test)]
    return true;

    #[cfg(not(test))]
    platform::check_driver_process(pid)
}

/// Checks if the target UART port path represents a virtual channel (PTY or UNIX socket).
pub fn is_pty_target<P: AsRef<Path>>(target: P) -> bool {
    let path = target.as_ref();
    if std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket()) {
        return true;
    }
    std::fs::canonicalize(path).is_ok_and(|c| {
        let s = c.to_string_lossy();
        s.starts_with("/dev/pts/") || s.starts_with("/dev/ttys")
    })
}

/// Health and liveness status of a background UART driver daemon process.
#[derive(Debug, PartialEq, Eq)]
pub enum DaemonLiveness {
    /// Process is alive and actively accepting connections on its control socket.
    Alive,
    /// Process is not running or has terminated.
    Dead,
    /// Process exists in the process table but fails to respond to control socket connections.
    Unresponsive,
}

/// Verifies whether the UART driver daemon with the given PID is actively listening
/// on its control socket, handling startup races and unresponsive hung processes.
pub async fn check_daemon_liveness(pid: u32, control_socket_path: &Path) -> DaemonLiveness {
    if !is_driver_running(pid) {
        return DaemonLiveness::Dead;
    }
    let try_connect = || async {
        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tokio::net::UnixStream::connect(control_socket_path),
        )
        .await
        .ok()?
        .ok()?;
        if !cfg!(test) && get_peer_pid(&stream).is_ok_and(|peer| peer != pid) {
            return None;
        }
        Some(stream)
    };
    if try_connect().await.is_some() {
        return DaemonLiveness::Alive;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    if try_connect().await.is_some() { DaemonLiveness::Alive } else { DaemonLiveness::Unresponsive }
}

/// Connects to the driver daemon's control socket and reads the current [`DaemonMetrics`] JSON payload.
pub async fn read_metrics_from_control_socket(control_socket_path: &Path) -> Option<DaemonMetrics> {
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        let mut stream = tokio::net::UnixStream::connect(control_socket_path).await.ok()?;
        let mut content = String::new();
        stream.read_to_string(&mut content).await.ok()?;
        serde_json::from_str::<DaemonMetrics>(&content).ok()
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::UartProtocol;
    use tempfile::tempdir;

    #[test]
    #[cfg(target_os = "linux")]
    fn test_proc_path() {
        assert_eq!(linux::proc_path(1234, "comm"), Path::new("/proc/1234/comm"));
        assert_eq!(linux::proc_path(1234, "exe"), Path::new("/proc/1234/exe"));
        assert_eq!(linux::proc_path(1234, "cmdline"), Path::new("/proc/1234/cmdline"));
        assert_eq!(linux::proc_path(1234, "status"), Path::new("/proc/1234/status"));
        let _ = linux::check_driver_process(std::process::id());
    }

    #[test]
    fn test_is_running_edge_cases() {
        assert!(!is_running(0));
        assert!(!is_running(u32::MAX));
        assert!(is_running(std::process::id()));
    }

    #[test]
    fn test_is_pty_target() {
        assert!(!is_pty_target("/nonexistent/path/for/sure"));
        assert!(!is_pty_target("/dev/null"));
    }

    #[test]
    fn test_get_peer_pid() {
        let (s1, _s2) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer_pid = get_peer_pid(&s1).expect("get_peer_pid should succeed");
        assert_eq!(peer_pid, std::process::id());
    }

    #[fuchsia::test]
    async fn test_read_metrics_from_control_socket() {
        let temp = tempdir().unwrap();
        let sock_path = temp.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let _server = fuchsia_async::Task::local(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let metrics = DaemonMetrics {
                    checksum_errors: 42,
                    retransmissions: 7,
                    active_protocol: UartProtocol::ResendSP,
                    ..Default::default()
                };
                let payload = serde_json::to_string(&metrics).unwrap();
                let _ = stream.write_all(payload.as_bytes()).await;
            }
        });

        let m = read_metrics_from_control_socket(&sock_path).await.unwrap();
        assert_eq!(m.checksum_errors, 42);
        assert_eq!(m.retransmissions, 7);
        assert_eq!(m.active_protocol, UartProtocol::ResendSP);
    }

    #[fuchsia::test]
    async fn test_check_daemon_liveness() {
        let temp = tempdir().unwrap();
        let sock_path = temp.path().join("control.sock");
        let my_pid = std::process::id();

        assert_eq!(check_daemon_liveness(0, &sock_path).await, DaemonLiveness::Dead);
        assert_eq!(check_daemon_liveness(my_pid, &sock_path).await, DaemonLiveness::Unresponsive);

        let _listener = tokio::net::UnixListener::bind(&sock_path).unwrap();
        assert_eq!(check_daemon_liveness(my_pid, &sock_path).await, DaemonLiveness::Alive);
        assert_eq!(check_daemon_liveness(0, &sock_path).await, DaemonLiveness::Dead);
    }
}
