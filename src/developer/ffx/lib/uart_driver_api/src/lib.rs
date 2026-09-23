// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common API definitions, schema data structures, and socket path resolution utilities
//! for the `ffx` UART transport driver and tooling.

/// Error types emitted during UART driver connection establishment and lifecycle.
pub mod errors;
pub use errors::*;

use anyhow::{Result, anyhow};
use ffx_config::EnvironmentContext;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn validate_and_resolve_socket_path(path: PathBuf) -> Result<PathBuf> {
    let absolute_path = if path.is_relative() {
        std::env::current_dir().map(|cwd| cwd.join(&path)).unwrap_or_else(|_| path.clone())
    } else {
        path
    };

    let resolved_path = if let Ok(abs) = std::fs::canonicalize(&absolute_path) {
        abs
    } else if let Some(parent) = absolute_path.parent() {
        if let Ok(parent_abs) = std::fs::canonicalize(parent) {
            if let Some(file_name) = absolute_path.file_name() {
                parent_abs.join(file_name)
            } else {
                absolute_path
            }
        } else {
            absolute_path
        }
    } else {
        absolute_path
    };

    let path_len = resolved_path.to_string_lossy().as_bytes().len();
    let max_len = if cfg!(target_os = "macos") { 104 } else { 108 };
    if path_len > max_len {
        return Err(anyhow!(
            "UNIX socket path exceeds limit ({} bytes, max {} bytes for OS)",
            path_len,
            max_len
        ));
    }
    Ok(resolved_path)
}

/// Normalizes a filesystem path by stripping redundant current directory (`.`) components
/// and resolving parent directory (`..`) components without accessing the filesystem.
pub fn clean_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            c => {
                clean.push(c.as_os_str());
            }
        }
    }
    clean
}

/// Computes a deterministic 16-character hexadecimal target identifier from a target path.
///
/// This identifier is used for UNIX domain socket and companion metadata filenames
/// (e.g., `ffx_uart_<id>.sock` and `ffx_uart_<id>.json`).
///
/// Hashing the target path guarantees:
/// 1. Bounded socket path lengths: UNIX domain sockets (`sockaddr_un`) have a hard
///    OS-level limit (108 bytes on Linux, 104 bytes on macOS). Hashing guarantees
///    a constant 29-byte filename regardless of target path depth or length.
/// 2. Filesystem character safety: Target paths contain slashes and colons that cannot
///    appear in flat filenames.
/// 3. Deterministic pairing: Any client can derive the exact socket and metadata paths
///    without consulting a centralized daemon.
pub fn get_target_id_from_target_path(target_path: &Path) -> String {
    let absolute = if target_path.is_relative() {
        std::env::current_dir()
            .map(|cwd| cwd.join(target_path))
            .unwrap_or_else(|_| target_path.to_path_buf())
    } else {
        target_path.to_path_buf()
    };
    let resolved_target = clean_path(&absolute);
    let target_hash = Sha256::digest(resolved_target.to_string_lossy().as_bytes());
    let hex_id = target_hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    hex_id[..16].to_string()
}

/// Resolves the UNIX domain socket path corresponding to a target path within the
/// configured `shared_data` directory.
///
/// Ensures the resulting socket path adheres to OS-specific path length limits
/// (108 bytes on Linux, 104 bytes on macOS).
///
/// # Errors
/// Returns an error if the shared data path cannot be retrieved from `context`
/// or if the resolved socket path exceeds OS limits.
pub fn get_socket_path_from_target_path(
    target_path: &Path,
    context: &EnvironmentContext,
) -> Result<PathBuf> {
    let mut path = match context.get::<PathBuf, _>("shared_data") {
        Ok(p) => p,
        Err(_) => context
            .get_shared_data_path()
            .map_err(|e| anyhow!("Failed to get shared data path: {}", e))?,
    };
    path.push("ffx_uart");
    let hex_id = get_target_id_from_target_path(target_path);
    path.push(format!("ffx_uart_{}.sock", hex_id));

    validate_and_resolve_socket_path(path)
}

/// Resolves a user-provided friendly target name (e.g. `/dev/ttyUSB0` or direct socket path)
/// into an active or expected UNIX domain socket path.
///
/// # Errors
/// Returns an error if the friendly name cannot be resolved to a valid target path
/// or if the derived socket path exceeds OS length limitations.
pub fn friendly_name_to_socket_path(
    friendly: &str,
    context: &EnvironmentContext,
) -> Result<PathBuf> {
    let target_path = friendly_name_to_target_path(friendly, context)?;
    if target_path.exists() {
        if let Ok(metadata) = std::fs::metadata(&target_path) {
            use std::os::unix::fs::FileTypeExt as _;
            if metadata.file_type().is_socket() {
                return validate_and_resolve_socket_path(target_path);
            }
        }
    }
    // Heuristic fallback: if path ends with .sock, treat as socket even if it doesn't exist yet
    if target_path.extension().and_then(|ext| ext.to_str()) == Some("sock") {
        return validate_and_resolve_socket_path(target_path);
    }
    get_socket_path_from_target_path(&target_path, context)
}

/// Parses a user-supplied target string into an absolute target filesystem path.
///
/// # Errors
/// Returns an error if the string is not an absolute path starting with `/`.
pub fn friendly_name_to_target_path(
    friendly: &str,
    _context: &EnvironmentContext,
) -> Result<PathBuf> {
    // 1. Absolute Path
    if friendly.starts_with('/') {
        return Ok(PathBuf::from(friendly));
    }

    // Fallback:
    Err(anyhow!(
        "Target '{}' is not recognized. It must be an absolute path (starting with '/').",
        friendly
    ))
}

/// Converts a UNIX domain socket path back into a human-readable friendly device path,
/// first inspecting companion JSON metadata, then querying `/dev` serial ports, and
/// finally falling back to the raw socket path string.
pub fn socket_path_to_friendly_name(socket_path: &Path, context: &EnvironmentContext) -> String {
    // Try to read metadata JSON to get the real target path and use it to build a friendly name.
    let json_path = socket_path.with_extension("json");
    if let Ok(content) = std::fs::read_to_string(&json_path) {
        if let Ok(metadata) = serde_json::from_str::<ConnectionMetadata>(&content) {
            return metadata.target;
        }
    }

    // Try to match ttyUSB ports
    if let Ok(entries) = std::fs::read_dir("/dev") {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if filename.starts_with("ttyUSB")
                        || filename.starts_with("ttyS")
                        || filename.starts_with("ttyACM")
                        || filename.starts_with("tty.usbserial")
                    {
                        if let Ok(expected_socket) =
                            get_socket_path_from_target_path(&path, context)
                        {
                            if expected_socket == socket_path {
                                return path.to_string_lossy().into_owned();
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback
    socket_path.to_string_lossy().into_owned()
}

/// Current operational status of a UART target driver connection.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum ConnectionStatus {
    /// Driver process is currently opening the port and negotiating protocol with the target.
    Connecting,
    /// Connection and protocol negotiation succeeded; channel is open and ready.
    Connected,
    /// Connection failed with the enclosed error details.
    Error(ConnectionError),
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionStatus::Connecting => write!(f, "Connecting"),
            ConnectionStatus::Connected => write!(f, "Connected"),
            ConnectionStatus::Error(e) => write!(f, "Error: {}", e),
        }
    }
}

/// Framing protocol variant active on the UART transport link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UartProtocol {
    /// ResendSP sliding-window Go-Back-N protocol with CRC-32 integrity.
    ResendSP,
    /// Test loopback protocol used in unit and integration test fixtures.
    TestProtocol,
    /// Protocol unrecognized or not yet negotiated.
    Unknown,
}

impl std::fmt::Display for UartProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UartProtocol::ResendSP => write!(f, "ResendSP"),
            UartProtocol::TestProtocol => write!(f, "TestProtocol"),
            UartProtocol::Unknown => write!(f, "Unknown"),
        }
    }
}

impl std::str::FromStr for UartProtocol {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "resend" | "resendsp" => Ok(UartProtocol::ResendSP),
            "testprotocol" => Ok(UartProtocol::TestProtocol),
            "unknown" => Ok(UartProtocol::Unknown),
            other => Err(format!("Unknown protocol: {other}")),
        }
    }
}

/// Persisted connection metadata stored in companion JSON files for an active UART driver.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionMetadata {
    /// Process ID (PID) of the active driver daemon.
    pub pid: u32,
    /// Target port specification (e.g. `/dev/ttyUSB0`).
    pub target: String,
    /// Current connection status.
    pub status: ConnectionStatus,
    /// 16-character hexadecimal target identifier hash.
    pub id: Option<String>,
    /// Configured baud rate (for TTY serial ports).
    pub baud: Option<u32>,
    /// Active framing protocol.
    pub protocol: UartProtocol,
    /// Configured log level of the driver process.
    pub log_level: Option<String>,
    /// Discovered target nodename, if available and connected.
    #[serde(default)]
    pub nodename: Option<String>,
    /// Discovered target serial number, if available and connected.
    #[serde(default)]
    pub serial: Option<String>,
}

/// Reads cached target identity (`(nodename, serial)`) from the driver metadata JSON file.
/// Returns `Some((nodename, serial))` only if the driver is currently in `Connected` status
/// and a cached `nodename` is present.
pub fn load_cached_identity(socket_path: &Path) -> Option<(String, Option<String>)> {
    let json_path = socket_path.with_extension("json");
    if let Ok(content) = std::fs::read_to_string(&json_path) {
        if let Ok(meta) = serde_json::from_str::<ConnectionMetadata>(&content) {
            if meta.status == ConnectionStatus::Connected {
                return meta.nodename.map(|n| (n, meta.serial));
            }
        }
    }
    None
}

/// Updates the metadata JSON file with discovered target identity (`nodename` and `serial`),
/// atomically replacing the file via a temporary file.
///
/// # Errors
/// Returns an error if reading existing metadata, serializing updated JSON, or atomic file replacement fails.
pub fn update_metadata_identity(
    socket_path: &Path,
    nodename: Option<String>,
    serial: Option<String>,
) -> Result<()> {
    let json_path = socket_path.with_extension("json");
    let content = std::fs::read_to_string(&json_path)
        .map_err(|e| anyhow!("Failed to read metadata at {}: {e}", json_path.display()))?;
    let mut meta: ConnectionMetadata = serde_json::from_str(&content)
        .map_err(|e| anyhow!("Failed to parse metadata at {}: {e}", json_path.display()))?;
    meta.nodename = nodename;
    meta.serial = serial;
    let new_content = serde_json::to_string(&meta)?;
    let rand_suffix: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let temp_path = json_path.with_extension(format!("{}_{}.tmp", std::process::id(), rand_suffix));
    std::fs::write(&temp_path, new_content.as_bytes())?;
    std::fs::rename(&temp_path, &json_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_friendly_name_mapping() {
        let env = ffx_config::test_env().build().expect("test env");
        let context = &env.context;

        // Fallback for random socket path (returns absolute path as-is)
        let random_socket = PathBuf::from("/tmp/ffx_uart_random.sock");
        let friendly = socket_path_to_friendly_name(&random_socket, context);
        assert_eq!(friendly, "/tmp/ffx_uart_random.sock");

        let resolved = friendly_name_to_socket_path(&friendly, context).unwrap();
        assert_eq!(resolved, random_socket);

        // Fallback for direct path
        let resolved_direct = friendly_name_to_socket_path("/tmp/direct.sock", context).unwrap();
        assert_eq!(resolved_direct, PathBuf::from("/tmp/direct.sock"));
    }

    #[test]
    fn test_socket_path_length_validation() {
        // Test short path succeeds
        let short_path = format!("/{}", "a".repeat(9));
        let env = ffx_config::test_env()
            .runtime_config("shared_data", short_path.as_str())
            .build()
            .expect("test env");
        let context = &env.context;

        let target_path = PathBuf::from("/dev/ttyUSB0");
        let res = get_socket_path_from_target_path(&target_path, context);
        assert!(res.is_ok());

        // Test long path fails
        let long_path = format!("/{}", "a".repeat(99));
        let env = ffx_config::test_env()
            .runtime_config("shared_data", long_path.as_str())
            .build()
            .expect("test env");
        let context = &env.context;

        let res = get_socket_path_from_target_path(&target_path, context);
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("UNIX socket path exceeds limit"), "Err message was: {}", err_msg);
    }

    #[test]
    fn test_socket_path_length_validation_boundaries() {
        let max_len = if cfg!(target_os = "macos") { 104 } else { 108 };
        let suffix_len = 40;
        let exact_fit_len = max_len - suffix_len;
        let exceeds_len = exact_fit_len + 1;

        let target_path = PathBuf::from("/dev/ttyUSB0");

        // Exact fit length succeeds
        let fit_path = format!("/{}", "a".repeat(exact_fit_len - 1));
        let env = ffx_config::test_env()
            .runtime_config("shared_data", fit_path.as_str())
            .build()
            .expect("test env");
        let res = get_socket_path_from_target_path(&target_path, &env.context);
        assert!(
            res.is_ok(),
            "Expected OK for exact fit path length {}, got: {:?}",
            exact_fit_len,
            res
        );

        // One byte over fails
        let long_path = format!("/{}", "a".repeat(exceeds_len - 1));
        let env = ffx_config::test_env()
            .runtime_config("shared_data", long_path.as_str())
            .build()
            .expect("test env");
        let res = get_socket_path_from_target_path(&target_path, &env.context);
        assert!(res.is_err(), "Expected error for exceeding path length {}", exceeds_len);
    }

    #[test]
    fn test_metadata_identity_caching() {
        let temp = tempdir().unwrap();
        let socket_path = temp.path().join("ffx_uart_test.sock");
        let json_path = temp.path().join("ffx_uart_test.json");

        // Initially, no metadata file exists.
        assert_eq!(load_cached_identity(&socket_path), None);

        // Write metadata with status = Connected, but without cached identity.
        let mut meta = ConnectionMetadata {
            pid: 1234,
            target: "/dev/ttyUSB0".to_string(),
            status: ConnectionStatus::Connected,
            id: Some("testid".to_string()),
            baud: Some(1000000),
            protocol: UartProtocol::ResendSP,
            log_level: None,
            nodename: None,
            serial: None,
        };
        std::fs::write(&json_path, serde_json::to_string(&meta).unwrap()).unwrap();

        // Nodename is None, so load_cached_identity returns None.
        assert_eq!(load_cached_identity(&socket_path), None);

        // Update identity in metadata file.
        update_metadata_identity(
            &socket_path,
            Some("iris-target".to_string()),
            Some("SER12345".to_string()),
        )
        .unwrap();

        // Now load_cached_identity returns the cached nodename and serial.
        assert_eq!(
            load_cached_identity(&socket_path),
            Some(("iris-target".to_string(), Some("SER12345".to_string())))
        );

        // When connection status is not Connected, cached identity should not be used.
        meta.status = ConnectionStatus::Connecting;
        meta.nodename = Some("iris-target".to_string());
        meta.serial = Some("SER12345".to_string());
        std::fs::write(&json_path, serde_json::to_string(&meta).unwrap()).unwrap();
        assert_eq!(load_cached_identity(&socket_path), None);

        // Test backward-compatibility deserialization of legacy JSON missing nodename/serial fields.
        let legacy_json = r#"{"pid":1234,"target":"/dev/ttyUSB0","status":"Connected","id":"testid","baud":1000000,"protocol":"ResendSP","log_level":null}"#;
        let legacy_meta: ConnectionMetadata = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy_meta.nodename, None);
        assert_eq!(legacy_meta.serial, None);
    }

    #[test]
    fn test_get_target_id_from_target_path() {
        let p1 = Path::new("/dev/ttyUSB0");
        let p2 = Path::new("/dev/ttyUSB0/");
        let id1 = get_target_id_from_target_path(p1);
        let id2 = get_target_id_from_target_path(p2);
        assert_eq!(id1.len(), 16);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_connection_error_serde_roundtrip() {
        let err1 = ConnectionError::PathNotFound { path: "/dev/ttyUSB0".to_string() };
        let json1 = serde_json::to_string(&err1).unwrap();
        assert_eq!(json1, r#"{"type":"PathNotFound","path":"/dev/ttyUSB0"}"#);
        let roundtrip1: ConnectionError = serde_json::from_str(&json1).unwrap();
        assert_eq!(err1, roundtrip1);

        let err2 = ConnectionError::raw("Connection lost");
        let json2 = serde_json::to_string(&err2).unwrap();
        assert_eq!(json2, r#"{"type":"Raw","message":"Connection lost"}"#);
        let roundtrip2: ConnectionError = serde_json::from_str(&json2).unwrap();
        assert_eq!(err2, roundtrip2);
    }
}
