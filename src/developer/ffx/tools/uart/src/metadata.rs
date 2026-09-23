// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Connection metadata management for UART targets.
//!
//! Provides functions to resolve, write, read, and delete connection metadata
//! which associates active UART target connections with their driver processes.

use ffx_config::EnvironmentContext;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

pub use uart_driver_api::{ConnectionError, ConnectionMetadata, ConnectionStatus, UartProtocol};
use uart_fpl::ProtocolId;

/// Diagnostic and operational metrics exported by an active UART driver daemon.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DaemonMetrics {
    /// Cumulative count of corrupted frames or CRC checksum failures detected.
    pub checksum_errors: u64,
    /// Cumulative count of frame retransmission timeouts triggered by the sender.
    pub retransmissions: u64,
    /// Currently active framing protocol variant (e.g. `ResendSP`).
    pub active_protocol: UartProtocol,
    /// Smoothed round-trip time estimate in milliseconds measured from ACK arrivals.
    pub estimated_rtt_ms: u32,
    /// Cumulative count of physical connection drops or TTY I/O disconnections.
    pub connection_drops: u64,
    /// Cumulative count of failed Channel 0 protocol negotiation attempts.
    pub handshake_failures: u64,
    /// Unix timestamp in milliseconds when data was last received from the UART port.
    pub last_read_timestamp_ms: u64,
    /// Unix timestamp in milliseconds when data was last written to the UART port.
    pub last_write_timestamp_ms: u64,
    /// Number of outbound frames currently buffered in the transmission queue.
    pub outgoing_queue_len: u32,
}

impl Default for DaemonMetrics {
    fn default() -> Self {
        Self {
            checksum_errors: 0,
            retransmissions: 0,
            active_protocol: UartProtocol::ResendSP,
            estimated_rtt_ms: 0,
            connection_drops: 0,
            handshake_failures: 0,
            last_read_timestamp_ms: 0,
            last_write_timestamp_ms: 0,
            outgoing_queue_len: 0,
        }
    }
}

/// Normalizes and canonicalizes a target string into a standard format.
///
/// Normalization rules include:
/// 1. Stripping trailing slashes (`/` or `\`) from the target string.
/// 2. Treating targets as filesystem paths:
///    - If the path is relative, it is joined to the current working directory to make it absolute.
///    - Path components are cleaned by resolving relative indicators like `.` (current directory)
///      and `..` (parent directory).
pub fn canonicalize_target(target: &str) -> String {
    if target.is_empty() {
        return String::new();
    }
    let trimmed_target = target.trim_end_matches(&['/', '\\']);
    let target = if trimmed_target.is_empty() { &target[..1] } else { trimmed_target };
    let path = std::path::Path::new(target);
    let absolute = if path.is_relative() {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    let normalized = uart_driver_api::clean_path(&absolute);
    normalized.to_string_lossy().into_owned()
}

/// Computes a unique ID for the given target.
///
/// Delegates directly to [`uart_driver_api::get_target_id_from_target_path`].
pub fn get_target_id(target: &str) -> String {
    uart_driver_api::get_target_id_from_target_path(Path::new(target))
}

/// Retrieves the Unix socket path corresponding to the given UART target.
///
/// The socket path is derived from the canonical target filesystem path.
///
/// # Errors
/// Returns an error if the socket path cannot be resolved or retrieved from target configuration.
pub fn get_socket_path(
    context: &EnvironmentContext,
    target: &str,
) -> std::result::Result<PathBuf, fho::Error> {
    let target_path = uart_driver_api::friendly_name_to_target_path(target, context)
        .map_err(|e| fho::user_error!("{e}"))?;
    uart_driver_api::get_socket_path_from_target_path(&target_path, context)
        .map_err(|e| fho::user_error!("{e}"))
}

/// Returns the path to the connection metadata file for a given target.
///
/// Connection metadata is stored as JSON companion files alongside the UNIX domain socket
/// under the `shared_data/ffx_uart/` directory (e.g., `ffx_uart_<target_id>.json`).
///
/// # Errors
/// Returns an error if querying the environment context for the shared data directory fails.
pub fn get_metadata_path(
    context: &EnvironmentContext,
    target: &str,
) -> std::result::Result<PathBuf, fho::Error> {
    Ok(get_socket_path(context, target)?.with_extension("json"))
}

/// Atomically writes serializable metadata to `path` using a temporary file in the same parent directory.
fn atomic_write_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> std::result::Result<(), fho::Error> {
    let parent = path.parent().ok_or_else(|| {
        fho::user_error!("Invalid path without parent directory: {}", path.display())
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temp, value).map_err(|e| fho::bug!("{e}"))?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| fho::Error::IoError(e.error))?;
    Ok(())
}

/// Writes connection metadata to the metadata path, using a specified target name inside the metadata file.
///
/// Creates parent directories as needed if they do not exist.
///
/// # Errors
/// Returns an error if the directories cannot be created or writing to the file fails.
pub fn write_metadata_with_target_name(
    context: &EnvironmentContext,
    socket_path: &std::path::Path,
    target: &str,
    store_target: &str,
    pid: u32,
    baud: Option<u32>,
    protocol: ProtocolId,
) -> std::result::Result<(), fho::Error> {
    let path = socket_path.with_extension("json");
    let id = get_target_id(target);
    let query =
        context.build().name(Some("log.level")).level(Some(ffx_config::ConfigLevel::Runtime));
    let log_level = context.get::<String, _>(query).ok();
    let metadata = ConnectionMetadata {
        pid,
        target: canonicalize_target(store_target),
        status: ConnectionStatus::Connecting,
        id: Some(id),
        baud,
        protocol: match protocol {
            ProtocolId::ResendSP => UartProtocol::ResendSP,
            ProtocolId::Unknown(_) => UartProtocol::Unknown,
        },
        log_level,
        nodename: None,
        serial: None,
    };
    atomic_write_json(&path, &metadata)
}

/// Writes connection metadata (PID and target name) to the metadata file for the given target.
///
/// # Errors
/// Returns an error if the directories cannot be created or writing to the file fails.
pub fn write_metadata(
    context: &EnvironmentContext,
    socket_path: &std::path::Path,
    target: &str,
    pid: u32,
    baud: Option<u32>,
) -> std::result::Result<(), fho::Error> {
    write_metadata_with_target_name(
        context,
        socket_path,
        target,
        target,
        pid,
        baud,
        ProtocolId::ResendSP,
    )
}

/// Reads connection metadata from a file path.
///
/// Returns `None` if the file does not exist (`ErrorKind::NotFound`).
/// If the metadata file contains invalid JSON, returns `Some` with a default fallback metadata
/// struct with `pid` set to 0 and status set to `Corrupt JSON`. If reading the file fails for
/// any other reason, returns `Some` with status set to `Unreadable`.
pub fn load_metadata_from_path(
    path: &std::path::Path,
    fallback_target: &str,
    fallback_id: Option<String>,
) -> Option<ConnectionMetadata> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("Failed to read metadata file at {}: {e}", path.display());
            return Some(ConnectionMetadata {
                pid: 0,
                target: fallback_target.to_string(),
                status: ConnectionStatus::Error(ConnectionError::raw("Unreadable")),
                id: fallback_id,
                baud: None,
                protocol: UartProtocol::Unknown,
                log_level: None,
                nodename: None,
                serial: None,
            });
        }
    };
    Some(serde_json::from_str(&content).unwrap_or_else(|e| {
        tracing::warn!("Failed to parse metadata JSON at {}: {e}", path.display());
        ConnectionMetadata {
            pid: 0,
            target: fallback_target.to_string(),
            status: ConnectionStatus::Error(ConnectionError::raw("Corrupt JSON")),
            id: fallback_id,
            baud: None,
            protocol: UartProtocol::Unknown,
            log_level: None,
            nodename: None,
            serial: None,
        }
    }))
}

/// Reads connection metadata from the metadata file for the given target.
///
/// Returns `None` if no metadata file exists. If the metadata file contains invalid JSON,
/// returns a default fallback metadata struct with `pid` set to 0.
///
/// # Errors
/// Returns an error if resolving the metadata path fails.
pub fn read_metadata(
    context: &EnvironmentContext,
    target: &str,
) -> std::result::Result<Option<ConnectionMetadata>, fho::Error> {
    let path = get_metadata_path(context, target)?;
    let id = get_target_id(target);
    Ok(load_metadata_from_path(&path, target, Some(id)))
}

/// Searches the connection metadata directory for any metadata file that associates
/// with the given original target name.
///
/// Returns the path to the metadata file and the metadata struct itself if found.
///
/// # Errors
/// Returns an error if querying the shared data directory path fails.
pub fn find_metadata_by_target(
    context: &EnvironmentContext,
    target_name: &str,
) -> std::result::Result<Option<(PathBuf, ConnectionMetadata)>, fho::Error> {
    let mut dir_path = match context.get::<PathBuf, _>("shared_data") {
        Ok(p) => p,
        Err(_) => context.get_shared_data_path().map_err(|e| fho::user_error!("{e}"))?,
    };
    dir_path.push("ffx_uart");
    let entries = match fs::read_dir(&dir_path) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };
    let canonical_target = canonicalize_target(target_name);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("ffx_uart_") && file_name.ends_with(".json") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(metadata) = serde_json::from_str::<ConnectionMetadata>(&content) {
                            if canonicalize_target(&metadata.target) == canonical_target {
                                return Ok(Some((path, metadata)));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Deletes the metadata file associated with the given target if it exists.
///
/// # Errors
/// Returns an error if deleting the file fails.
pub fn delete_metadata(
    context: &EnvironmentContext,
    target: &str,
) -> std::result::Result<(), fho::Error> {
    let path = get_metadata_path(context, target)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fho::Error::IoError(e)),
    }
}

/// Updates only the connection status in the metadata file for a target.
///
/// # Errors
/// Returns an error if the metadata file cannot be read, updated, or written.
pub fn update_metadata_status(
    context: &EnvironmentContext,
    target: &str,
    status: ConnectionStatus,
) -> std::result::Result<(), fho::Error> {
    let path = get_metadata_path(context, target)?;
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    let mut metadata: ConnectionMetadata =
        serde_json::from_reader(file).map_err(|e| fho::bug!("{e}"))?;
    metadata.status = status;
    if metadata.status != ConnectionStatus::Connected {
        metadata.nodename = None;
        metadata.serial = None;
    }
    atomic_write_json(&path, &metadata)
}

/// Returns true if the target string looks like a UART target (filesystem path or friendly name).
pub fn is_uart_target(target: &str) -> bool {
    target.starts_with("uart:") || target.starts_with('/') || target.starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_canonicalize_target() {
        assert_eq!(canonicalize_target("/dev/ttyUSB0/"), "/dev/ttyUSB0");
        assert_eq!(canonicalize_target("/dev/ttyUSB0///"), "/dev/ttyUSB0");
        assert_eq!(canonicalize_target("/"), "/");
        assert_eq!(canonicalize_target(""), "");
    }

    #[test]
    fn test_get_target_id() {
        let id1 = get_target_id("/dev/ttyUSB0");
        assert_eq!(id1.len(), 16);
        let id2 = get_target_id("/dev/ttyUSB0/");
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_is_uart_target() {
        assert!(is_uart_target("uart:/dev/ttyUSB0"));
        assert!(is_uart_target("/dev/ttyUSB0"));
        assert!(is_uart_target("./ttyUSB0"));
        assert!(!is_uart_target("tcp:127.0.0.1:8080"));
        assert!(!is_uart_target("127.0.0.1:8080"));
        assert!(!is_uart_target("fe80::1"));
        assert!(!is_uart_target("some-device-name"));
    }

    #[test]
    fn test_metadata_roundtrip() {
        let temp = tempdir().unwrap();
        let env = ffx_config::test_env()
            .runtime_config("shared_data", temp.path().to_str().unwrap())
            .build()
            .expect("test env");

        let target = "/dev/ttyUSB0";
        let socket_path = get_socket_path(&env.context, target).unwrap();

        // Write metadata (pass trailing slashes to verify stored target is canonicalized)
        write_metadata(&env.context, &socket_path, "/dev/ttyUSB0///", 12345, Some(115200)).unwrap();

        // Read metadata
        let meta = read_metadata(&env.context, target).unwrap().expect("metadata exists");
        assert_eq!(meta.pid, 12345);
        assert_eq!(meta.target, target);
        assert_eq!(meta.baud, Some(115200));
        assert_eq!(meta.status, ConnectionStatus::Connecting);

        // Update status
        update_metadata_status(&env.context, target, ConnectionStatus::Connected).unwrap();
        let meta = read_metadata(&env.context, target).unwrap().expect("metadata exists");
        assert_eq!(meta.status, ConnectionStatus::Connected);

        // Find metadata by target
        let found = find_metadata_by_target(&env.context, target).unwrap();
        assert!(found.is_some());
        let (found_path, found_meta) = found.unwrap();
        assert_eq!(found_meta.pid, 12345);
        assert!(found_path.exists());

        // Delete metadata
        delete_metadata(&env.context, target).unwrap();
        assert!(read_metadata(&env.context, target).unwrap().is_none());
    }
}
