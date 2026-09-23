// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_debugger as fdebugger;
use futures_util::future::FutureExt;
use std::path::{Path, PathBuf};
use std::{env, io};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use zx_status::Status;

pub enum DebuggerProxy {
    LauncherProxy(fdebugger::LauncherProxy),
    DebugAgentProxy(fdebugger::DebugAgentProxy),
}

/// Represents a connectable socket to the remote debug_agent. It's essentially a FIDL socket and a
/// UNIX socket proxied by us. If |proxy| is a fucshia.debugger.Launcher proxy, a new DebugAgent
/// will be launched to connect this socket, otherwise, the existing DebugAgent on the other end of
/// the fuchsia.debugger.DebugAgent proxy will be connected.
pub struct DebugAgentSocket {
    proxy: DebuggerProxy,
    unix_socket_path: PathBuf,
    unix_socket: UnixListener,
}

impl DebugAgentSocket {
    /// Create a UNIX socket on the host side for zxdb/fidlcat to connect.
    pub fn create(proxy: DebuggerProxy) -> Result<DebugAgentSocket> {
        let (unix_socket_path, unix_socket) = make_temp_unix_socket()?;
        return Ok(DebugAgentSocket { proxy, unix_socket_path, unix_socket });
    }

    /// The path to the UNIX socket.
    pub fn unix_socket_path(&self) -> &Path {
        &self.unix_socket_path
    }

    /// Create, accept and start forwarding one connection. The call is blocking until the
    /// connection closes, either by the remote debug_agent or by the local zxdb.
    pub async fn forward_one_connection(&self) -> Result<()> {
        // Wait for a connection on the UNIX socket (connection from zxdb).
        // Accept this first, otherwise zxdb will hang forever on connecting.
        let (mut unix_conn, _) = self.unix_socket.accept().await?;

        // Create a FIDL socket to the debug_agent on the device.
        let (fidl_conn, fidl_right) = match &self.proxy {
            DebuggerProxy::DebugAgentProxy(agent) => agent.domain().create_stream_socket(),
            DebuggerProxy::LauncherProxy(launcher) => launcher.domain().create_stream_socket(),
        };

        let (mut unix_rx, mut unix_tx) = unix_conn.split();
        let (mut fidl_rx, mut fidl_tx) = futures::io::AsyncReadExt::split(fidl_conn);

        let agent = match &self.proxy {
            DebuggerProxy::DebugAgentProxy(agent) => agent.clone(),
            DebuggerProxy::LauncherProxy(launcher) => {
                // No choice given, launch a new DebugAgent.
                let (client_proxy, server_end) =
                    launcher.domain().create_proxy::<fdebugger::DebugAgentMarker>();
                launcher
                    .launch(server_end, &fdebugger::LaunchOptions::default())
                    .await?
                    .map_err(Status::err_from_raw)?;
                client_proxy
            }
        };

        agent.connect(fidl_right).await?.map_err(Status::err_from_raw)?;

        // Forward from UNIX socket to FIDL socket.
        let unix_to_fidl = async {
            let mut buffer = [0; 4096];
            loop {
                let n = unix_rx.read(&mut buffer).await?;
                if n == 0 {
                    return Ok(()) as Result<()>;
                }
                let mut ofs = 0;
                while ofs != n {
                    let wrote =
                        futures::io::AsyncWriteExt::write(&mut fidl_tx, &buffer[ofs..n]).await?;
                    ofs += wrote;
                    if wrote == 0 {
                        return Ok(()) as Result<()>;
                    }
                }
            }
        };

        // Forward from FIDL socket to UNIX socket.
        let fidl_to_unix = async {
            let mut buffer = [0; 4096];
            loop {
                let n = futures::io::AsyncReadExt::read(&mut fidl_rx, &mut buffer).await?;
                if n == 0 {
                    return Ok(()) as Result<()>;
                }
                let mut ofs = 0;
                while ofs != n {
                    let wrote = unix_tx.write(&buffer[ofs..n]).await?;
                    ofs += wrote;
                    if wrote == 0 {
                        return Ok(()) as Result<()>;
                    }
                }
            }
        };

        // Exit on close or any error.
        futures::select! {
            res = unix_to_fidl.fuse() => res?,
            res = fidl_to_unix.fuse() => res?,
        };

        Ok(())
    }
}

impl Drop for DebugAgentSocket {
    fn drop(&mut self) {
        std::fs::remove_file(&self.unix_socket_path).unwrap_or_default();
    }
}

/// This mimics tempfile::util::create_helper but unfortunately that function is private.
fn make_temp_unix_socket() -> std::io::Result<(PathBuf, UnixListener)> {
    use rand::distr::{Alphanumeric, SampleString};

    let retries = 10;
    let prefix = "debug_agent_";
    let rand_str_length = 6;
    let suffix = ".socket";

    for _ in 0..retries {
        let rand_str = Alphanumeric.sample_string(&mut rand::rng(), rand_str_length);

        let mut path = env::temp_dir().into_os_string();
        path.extend(["/".as_ref(), prefix.as_ref(), rand_str.as_ref(), suffix.as_ref()]);

        match UnixListener::bind(&path) {
            Ok(socket) => return Ok((path.into(), socket)),
            Err(e) => {
                if e.kind() == io::ErrorKind::AlreadyExists {
                    continue;
                } else {
                    return Err(e);
                }
            }
        };
    }

    Err(io::Error::new(io::ErrorKind::AlreadyExists, "cannot create temp unix socket"))
}
