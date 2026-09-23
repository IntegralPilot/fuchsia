// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use ffx_config::EnvironmentContext;
use ffx_config::logging::LogDirHandling;
use fuchsia_async::TimeoutExt;
use fuchsia_sync::Mutex;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct ConnectionInfo {
    ssh_connection: String,
    #[serde(default)]
    overnet_id: Option<u64>,
    #[serde(default)]
    compatibility: Option<LegacyCompatInfo>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct LegacyCompatInfo {
    #[serde(default)]
    overnet_id: Option<u64>,
}

pub const BUFSIZE: usize = 1024;
pub struct LineBuffer {
    buffer: [u8; BUFSIZE],
    pos: usize,
}

// 1K should be enough for the initial line, which just looks something like
//    "++ 192.168.1.1 1234 10.0.0.1 22 ++\n"
impl LineBuffer {
    pub fn new() -> Self {
        Self { buffer: [0; BUFSIZE], pos: 0 }
    }

    pub fn line(&self) -> &[u8] {
        &self.buffer[..self.pos]
    }
}

impl ToString for LineBuffer {
    fn to_string(&self) -> String {
        String::from_utf8_lossy(self.line()).into()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseSshConnectionError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Parse error: {:?}", .0)]
    Parse(String),
    #[error("Unexpected EOF: {:?}", .0)]
    UnexpectedEOF(String),
    #[error("Read-line timeout")]
    Timeout,
}

pub async fn read_ssh_line<R: AsyncRead + Unpin>(
    lb: &mut LineBuffer,
    rdr: &mut R,
) -> std::result::Result<String, ParseSshConnectionError> {
    loop {
        // We're reading a byte at a time, which would be bad if we were doing it a lot,
        // but it's only used for stderr (which should normally not produce much data),
        // and the first line of stdout.
        let mut b = [0u8];
        let n = rdr.read(&mut b[..]).await.map_err(ParseSshConnectionError::Io)?;
        let b = b[0];
        if n == 0 {
            return Err(ParseSshConnectionError::UnexpectedEOF(lb.to_string()));
        }
        if b == b'\n' {
            let s = lb.to_string();
            // Clear for next read
            lb.pos = 0;
            return Ok(s);
        }
        lb.buffer[lb.pos] = b;
        lb.pos += 1;
        if lb.pos >= lb.buffer.len() {
            return Err(ParseSshConnectionError::Parse(format!(
                "Buffer full: {:?}...",
                &lb.buffer[..64]
            )));
        }
    }
}

pub async fn read_ssh_line_with_timeouts<R: AsyncBufRead + Unpin>(
    rdr: &mut R,
) -> Result<String, ParseSshConnectionError> {
    let mut time = 0;
    let wait_time = 2;
    let mut lb = LineBuffer::new();
    loop {
        match read_ssh_line(&mut lb, rdr)
            .on_timeout(Duration::from_secs(wait_time), || Err(ParseSshConnectionError::Timeout))
            .await
        {
            Ok(s) => {
                return Ok(s);
            }
            Err(ParseSshConnectionError::Timeout) => {
                time += wait_time;
                log::debug!(
                    "No new lines received from SSH cmd after {time}s, total lines received from SSH so far: {:?}",
                    lb.line()
                );
            }
            Err(e) => {
                return Err(e);
            }
        }
    }
}

fn parse_ssh_connection_legacy(
    line: &str,
) -> std::result::Result<(String, Option<u64>), ParseSshConnectionError> {
    let mut parts = line.split(" ");
    // The first part should be our anchor.
    match parts.next() {
        Some("++") => {}
        Some(_) | None => {
            log::error!("Failed to read first anchor: {line}");
            return Err(ParseSshConnectionError::Parse(line.into()));
        }
    }

    // SSH_CONNECTION identifies the client and server ends of the connection.
    // The variable contains four space-separated values: client IP address,
    // client port number, server IP address, and server port number.
    // This is left as a string since std::net::IpAddr does not support string scope_ids.
    let client_address = if let Some(part) = parts.next() {
        part
    } else {
        log::error!("Failed to read client_address: {line}");
        return Err(ParseSshConnectionError::Parse(line.into()));
    };

    // Followed by the client port.
    let _client_port = if let Some(part) = parts.next() {
        part
    } else {
        log::error!("Failed to read port: {line}");
        return Err(ParseSshConnectionError::Parse(line.into()));
    };

    // Followed by the server address.
    let _server_address = if let Some(part) = parts.next() {
        part
    } else {
        log::error!("Failed to read port: {line}");
        return Err(ParseSshConnectionError::Parse(line.into()));
    };

    // Followed by the server port.
    let _server_port = if let Some(part) = parts.next() {
        part
    } else {
        log::error!("Failed to read server_port: {line}");
        return Err(ParseSshConnectionError::Parse(line.into()));
    };

    // The last part should be our anchor.
    match parts.next() {
        Some("++") => {}
        None | Some(_) => {
            return Err(ParseSshConnectionError::Parse(line.into()));
        }
    };

    // Finally, there should be nothing left.
    if let Some(_) = parts.next() {
        log::error!("Extra data: {line}");
        return Err(ParseSshConnectionError::Parse(line.into()));
    }

    Ok((client_address.to_string(), None))
}

async fn parse_ssh_connection<R: AsyncBufRead + Unpin>(
    stdout: &mut R,
    ctx: &EnvironmentContext,
) -> std::result::Result<(String, Option<u64>), ParseSshConnectionError> {
    let line = read_ssh_line_with_timeouts(stdout).await?;
    if line.is_empty() {
        log::error!("Failed to read first line from stdout");
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
    }
    if let Some(file) = ssh_log_file(ctx) {
        write_ssh_log("O", &line, file);
    }
    if line.starts_with("{") {
        parse_ssh_connection_with_info(&line)
    } else {
        parse_ssh_connection_legacy(&line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostAddr(pub String);

impl fmt::Display for HostAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for HostAddr {
    fn from(s: &str) -> Self {
        HostAddr(s.to_string())
    }
}

impl From<String> for HostAddr {
    fn from(s: String) -> Self {
        HostAddr(s)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum PipeError {
    #[error("compatibility check not supported")]
    NoCompatibilityCheck,
    #[error("log-id option not supported")]
    NoLogIdSupport,
    #[error("could not establish connection: {0}")]
    ConnectionFailed(String),
    #[error("io error: {0}")]
    IoError(#[from] io::Error),
    #[error("error {0}")]
    Error(String),
    #[error("target referenced has gone")]
    TargetGone,
    #[error("creating pipe to {1} failed: {0}")]
    PipeCreationFailed(String, String),
    #[error("no ssh address to {0}")]
    NoAddress(String),
    #[error("running target overnet pipe: {0}")]
    SpawnError(String),
    #[error("error with address")]
    AddressError(#[from] netext::InvalidInterfaceIdError),
}

pub async fn parse_ssh_output(
    stdout: &mut BufReader<ChildStdout>,
    stderr: &mut BufReader<ChildStderr>,
    ctx: &EnvironmentContext,
) -> std::result::Result<(HostAddr, Option<u64>), PipeError> {
    let res = match parse_ssh_connection(stdout, ctx).await.context("reading ssh connection") {
        Ok((addr, overnet_id)) => {
            if overnet_id.is_none() {
                log::info!(
                    "Did not receive overnet_id from remote host, presumably it is an old device. Warning: without the overnet_id we cannot determine whether this connection is to an already-known target"
                );
            }
            (Some(HostAddr(addr)), overnet_id)
        }
        Err(e) => {
            // Upon a successful connection, OpenSSH sets several environment variables.
            // SSH_CONNECTION shows the address of the client, the outgoing port on the client,
            // the address of the server and the incoming port on the server. In this error
            // message, parse_ssh_connection_with_info function failed to parse the client
            // address.
            log::debug!("Failed to parse ssh connection from stdout: {:?}", e);
            let error_message = format!("Could not parse the client address from $SSH_CONNECTION.");
            log::warn!("{error_message}");
            (None, None)
        }
    };
    // Check for early exit.
    if let (Some(addr), overnet_id) = res {
        Ok((addr, overnet_id))
    } else {
        // If we failed to parse the ssh connection, there might be information in stderr
        // We seem to always come here even if the error above was successfully parsed because we
        // return (None, None) for res.
        Err(parse_ssh_error(stderr, ctx).await)
    }
}

pub fn ssh_stderr_to_pipe_error(line: &String) -> Option<PipeError> {
    // If we are running with "ssh -v", the stderr will also contain the initial
    // "OpenSSH" line.
    if line.contains("OpenSSH") {
        return None;
    }
    // It also may contain a warning about adding an address to the list of known hosts
    if line.starts_with("Warning: Permanently added") {
        return None;
    }
    // Or a warning about authentication
    if line.starts_with("Authenticated to ") {
        return None;
    }
    // Additional debugging messages will begin with "debug{1,2,3}" based on the various
    // verbosity settings.
    if line.starts_with("debug1:") || line.starts_with("debug2:") || line.starts_with("debug3:") {
        return None;
    }
    // At this point, we just want to look at one line to see if it is the compatibility
    // failure.
    log::debug!("Reading stderr:  {line}");
    if line.contains("Unrecognized argument: --abi-revision") {
        // It is an older image, so use the legacy command.
        log::info!("Target does not support abi compatibility check");
        Some(PipeError::NoCompatibilityCheck)
    } else if line.contains("Unrecognized argument: --log-id") {
        log::info!("Target does not support log-id");
        Some(PipeError::NoLogIdSupport)
    } else {
        Some(PipeError::ConnectionFailed(format!("{:?}", line)))
    }
}

async fn parse_ssh_error<R: AsyncBufRead + Unpin>(
    stderr: &mut R,
    ctx: &EnvironmentContext,
) -> PipeError {
    let log_file = ssh_log_file(ctx);
    loop {
        let line = match read_ssh_line_with_timeouts(stderr).await {
            Err(e) => {
                log::error!("reading ssh stderr: {e:?}");
                return PipeError::NoCompatibilityCheck;
            }
            Ok(l) => l,
        };
        if let Some(file) = log_file {
            write_ssh_log("E", &line, file);
        }
        if let Some(e) = ssh_stderr_to_pipe_error(&line) {
            break e;
        }
    }
}

fn parse_ssh_connection_with_info(
    line: &str,
) -> std::result::Result<(String, Option<u64>), ParseSshConnectionError> {
    let connection_info: ConnectionInfo =
        serde_json::from_str(&line).map_err(|e| ParseSshConnectionError::Parse(e.to_string()))?;
    let mut parts = connection_info.ssh_connection.split(" ");
    // SSH_CONNECTION identifies the client and server ends of the connection.
    // The variable contains four space-separated values: client IP address,
    // client port number, server IP address, and server port number.
    let overnet_id = connection_info
        .overnet_id
        .or_else(|| connection_info.compatibility.and_then(|c| c.overnet_id));
    if let Some(client_address) = parts.nth(0) {
        Ok((client_address.to_string(), overnet_id))
    } else {
        Err(ParseSshConnectionError::Parse(line.into()))
    }
}

// We'd like to cache the log file, if only so that our debug logs don't
// include multiple config-query lines for every line of ssh logging.
pub fn ssh_log_file(ctx: &EnvironmentContext) -> Option<&'static Mutex<File>> {
    // This is a OnceLock so it's effectively global, an Option because we don't
    // always want to get a log file at all, and a Mutex because we need mutable
    // access to the File. (We can't use OnceLock::get_mut() because the static
    // INSTANCE will only provide an immutable reference.)
    static INSTANCE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    INSTANCE
        .get_or_init(|| {
            if !ffx_config::logging::debugging_on(ctx) {
                None
            } else {
                match ffx_config::logging::log_file_with_info(
                    &ctx,
                    &PathBuf::from("ssh.log"),
                    LogDirHandling::WithDirWithRotate,
                ) {
                    Ok((f, _)) => Some(Mutex::new(f)),
                    Err(e) => {
                        eprintln!("Couldn't open ssh log file: {e:?}");
                        None
                    }
                }
            }
        })
        .as_ref()
}

pub fn write_ssh_log(prefix: &str, line: &String, file: &Mutex<File>) {
    // Skip keepalives, which will show up in the steady-state
    if line.contains("keepalive") {
        return;
    }
    let mut f = file.lock();
    const TIME_FORMAT: &str = "%b %d %H:%M:%S%.3f";
    let timestamp = chrono::Local::now().format(TIME_FORMAT);
    let log_id: u64 = *ffx_config::logging::LOGGING_ID;
    writeln!(&mut f, "{timestamp}[{log_id:0>20?}]: {prefix} {line}")
        .unwrap_or_else(|e| log::warn!("Couldn't write ssh log: {e:?}"));
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    async fn test_parse_ssh_output_doesnt_fail_with_debug2() {
        let env = ffx_config::test_init().expect("test env init");
        let line = "debug1: we are debugging\ndebug2: more debugging\ndebug1: debug one again\ndebug3: man this sure is verbose";
        let err = parse_ssh_error(&mut line.as_bytes(), &env.context).await;
        // Can't assert_eq because IoError doesn't impl PartialEq.
        assert!(matches!(err, PipeError::NoCompatibilityCheck));
    }

    #[fuchsia::test]
    async fn test_parse_ssh_connection_works() {
        let env = ffx_config::test_init().expect("test env init");
        for (line, expected) in [
            (&"++ 192.168.1.1 1234 10.0.0.1 22 ++\n"[..], ("192.168.1.1".to_string(), None)),
            (
                &"++ fe80::111:2222:3333:444 56671 10.0.0.1 22 ++\n",
                ("fe80::111:2222:3333:444".to_string(), None),
            ),
            (
                &"++ fe80::111:2222:3333:444%ethxc2 56671 10.0.0.1 22 ++\n",
                ("fe80::111:2222:3333:444%ethxc2".to_string(), None),
            ),
        ] {
            match parse_ssh_connection(&mut line.as_bytes(), &env.context).await {
                Ok(actual) => assert_eq!(expected, actual),
                res => panic!(
                    "unexpected result for {:?}: expected {:?}, got {:?}",
                    line, expected, res
                ),
            }
        }
    }

    #[fuchsia::test]
    async fn test_parse_ssh_connection_errors() {
        let env = ffx_config::test_init().expect("test env init");
        for line in [
            // Test for invalid anchors
            &"192.168.1.1 1234 10.0.0.1 22\n"[..],
            &"++192.168.1.1 1234 10.0.0.1 22++\n"[..],
            &"++192.168.1.1 1234 10.0.0.1 22 ++\n"[..],
            &"++ 192.168.1.1 1234 10.0.0.1 22++\n"[..],
            &"++ ++\n"[..],
            &"## 192.168.1.1 1234 10.0.0.1 22 ##\n"[..],
        ] {
            let res = parse_ssh_connection(&mut line.as_bytes(), &env.context).await;
            assert_matches!(res, Err(ParseSshConnectionError::Parse(_)));
        }
        for line in [
            // Truncation
            &"++"[..],
            &"++ 192.168.1.1"[..],
            &"++ 192.168.1.1 1234"[..],
            &"++ 192.168.1.1 1234 "[..],
            &"++ 192.168.1.1 1234 10.0.0.1"[..],
            &"++ 192.168.1.1 1234 10.0.0.1 22"[..],
            &"++ 192.168.1.1 1234 10.0.0.1 22 "[..],
            &"++ 192.168.1.1 1234 10.0.0.1 22 ++"[..],
        ] {
            let res = parse_ssh_connection(&mut line.as_bytes(), &env.context).await;
            assert_matches!(res, Err(ParseSshConnectionError::UnexpectedEOF(_)));
        }
    }

    #[fuchsia::test]
    async fn test_read_ssh_line() {
        let mut lb = LineBuffer::new();
        let input = &"++ 192.168.1.1 1234 10.0.0.1 22 ++\n"[..];
        match read_ssh_line(&mut lb, &mut input.as_bytes()).await {
            Ok(s) => assert_eq!(s, String::from("++ 192.168.1.1 1234 10.0.0.1 22 ++")),
            res => panic!("unexpected result: {res:?}"),
        }

        let mut lb = LineBuffer::new();
        let input = &"no newline"[..];
        let res = read_ssh_line(&mut lb, &mut input.as_bytes()).await;
        assert_matches!(res, Err(ParseSshConnectionError::UnexpectedEOF(_)));

        let mut lb = LineBuffer::new();
        let input = [b'A'; 1024];
        let res = read_ssh_line(&mut lb, &mut &input[..]).await;
        assert_matches!(res, Err(ParseSshConnectionError::Parse(_)));

        // Can continue after reading partial result
        let mut lb = LineBuffer::new();
        let input1 = &"foo"[..];
        let _ = read_ssh_line(&mut lb, &mut input1.as_bytes()).await;
        // We'll get a no-newline error, but it has the same semantics as
        // being interrupted due to a timeout
        let input2 = &"bar\n"[..];
        match read_ssh_line(&mut lb, &mut input2.as_bytes()).await {
            Ok(s) => assert_eq!(s, String::from("foobar")),
            res => panic!("unexpected result: {res:?}"),
        }
    }

    #[fuchsia::test]
    async fn test_compat_works_without_overnet_id() {
        let env = ffx_config::test_init().expect("test env init");
        let line = &"{\"ssh_connection\":\"10.0.2.2 34502 10.0.2.15 22\",\"compatibility\":{\"status\":\"supported\",\"platform_abi\":12345,\"message\":\"foo\"}}\n"
            [..];
        let expected = ("10.0.2.2".to_string(), None);
        match parse_ssh_connection(&mut line.as_bytes(), &env.context).await {
            Ok(actual) => assert_eq!(expected, actual),
            res => {
                panic!("unexpected result for {:?}: expected {:?}, got {:?}", line, expected, res)
            }
        }
    }

    #[fuchsia::test]
    async fn test_compat_works_with_overnet_id() {
        let env = ffx_config::test_init().expect("test env init");
        let line = &"{\"ssh_connection\":\"10.0.2.2 34502 10.0.2.15 22\",\"compatibility\":{\"status\":\"supported\",\"platform_abi\":12345,\"message\":\"foo\", \"overnet_id\": 6789}}\n"
            [..];
        let expected = ("10.0.2.2".to_string(), Some(6789));
        match parse_ssh_connection(&mut line.as_bytes(), &env.context).await {
            Ok(actual) => assert_eq!(expected, actual),
            res => {
                panic!("unexpected result for {:?}: expected {:?}, got {:?}", line, expected, res)
            }
        }
    }

    #[test]
    fn test_empty_linebuffer() {
        let lb = LineBuffer::new();
        assert_eq!(lb.to_string(), "".to_owned());
    }
}
