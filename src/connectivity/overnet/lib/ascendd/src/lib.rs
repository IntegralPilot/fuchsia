// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as ErrorContext, Error, bail, format_err};
use argh::FromArgs;
use fuchsia_async::{Task, TimeoutExt};
use futures::channel::mpsc::unbounded;
use futures::prelude::*;
use netext::{TokioAsyncReadExt, UnixListenerStream};
use overnet_core::AscenddClientRouting;
use std::io::ErrorKind::{self, TimedOut};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};

pub static CIRCUIT_ID: [u8; 8] = *b"CIRCUIT\0";

/// If necessary, holds a tempdir open with a symlink to a socket path
/// that is too long to fit in the system's SUN_LEN.
#[derive(Debug)]
pub struct ShortPathLink {
    /// The shorthand path we're using to connect to the daemon
    pub short_path: PathBuf,
    /// The temporary directory handle we're putting the socket file in
    _temp_location: Option<tempfile::TempDir>,
}

impl ShortPathLink {
    // there seems to be no standard binding available for the SUN_LEN constant
    // in std, libc, or nix, so we'll just conservatively guess 100 for now.
    pub const MAX_SUN_LEN: usize = 100;
}

impl AsRef<Path> for ShortPathLink {
    fn as_ref(&self) -> &Path {
        self.short_path.as_ref()
    }
}

/// If `real_path` is too long to fit in a socket bind/connect struct,
/// creates a symlink in the tempdir that points to the 'real' socket
/// path.
///
/// Returns a [`ShortPathSocket`] that keeps the reference alive
/// while it's being connected to.
pub fn short_socket_path(real_path: &Path) -> std::io::Result<ShortPathLink> {
    #[cfg(not(target_os = "windows"))]
    use std::os::unix::fs::symlink;
    #[cfg(target_os = "windows")]
    use std::os::windows::fs::symlink_dir as symlink;

    let short_path;
    let temp_location;
    if real_path.as_os_str().len() > ShortPathLink::MAX_SUN_LEN {
        // we make a symlink from the original home of the socket to a (hopefully shorter) tmpdir path,
        // and then return a path that looks into that symlink to find the socket. This avoids a bunch of
        // annoying situations around things trying to create the socket when it doesn't already exist.
        let socket_filename = real_path.file_name().ok_or_else(|| {
            let error_str = format!(
                "{real_path} did not have a filename component",
                real_path = real_path.display()
            );
            std::io::Error::new(ErrorKind::InvalidInput, error_str)
        })?;
        let socket_dir = real_path.parent().ok_or_else(|| {
            let error_str = format!(
                "{real_path} did not have a path component",
                real_path = real_path.display()
            );
            std::io::Error::new(ErrorKind::InvalidInput, error_str)
        })?;

        let tempdir = tempfile::tempdir()?;
        let symlink_path = tempdir.path().join("root");

        short_path = symlink_path.join(socket_filename);
        if short_path.as_os_str().len() > ShortPathLink::MAX_SUN_LEN {
            let error_str = format!(
                "Even tmpdir path was too long to create a short enough socket path for {real_path} (tried: {short_path})",
                real_path = real_path.display(),
                short_path = short_path.display()
            );
            return Err(std::io::Error::new(ErrorKind::InvalidInput, error_str));
        }
        symlink(socket_dir, &symlink_path)?;
        temp_location = Some(tempdir);
    } else {
        short_path = real_path.to_owned();
        temp_location = None;
    }
    Ok(ShortPathLink { short_path, _temp_location: temp_location })
}

#[derive(FromArgs, Default)]
/// daemon to lift a non-Fuchsia device into Overnet.
pub struct Opt {
    #[argh(option, long = "sockpath")]
    /// path to the ascendd socket.
    pub sockpath: PathBuf,

    #[argh(option, long = "serial")]
    /// selector for which serial devices to communicate over.
    /// Could be 'none' to not communcate over serial, 'all' to query all serial devices and try to
    /// communicate with them, or a path to a serial device to communicate over *that* device.
    /// If not provided, this will default to 'none'.
    pub serial: Option<String>,

    #[argh(option, long = "client-routing", default = "true")]
    /// route Ascendd clients to each other. Can be turned off to avoid scaling issues
    /// when multiple ffxs are run concurrently, with no requirement to have
    /// them interact.
    pub client_routing: bool,

    #[argh(option)]
    /// paths to other ascendds which we will connect this ascendd to, causing
    /// both to be part of the same network.
    pub link: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct Ascendd {
    task: Task<Result<(), Error>>,
}

impl Ascendd {
    // Initializes and binds ascendd socket, but does not accept connections yet.
    pub async fn prime(
        mut opt: Opt,
        node: Arc<overnet_core::Router>,
    ) -> Result<impl FnOnce() -> Self, Error> {
        let link = std::mem::replace(&mut opt.link, vec![]);
        let (sockpath, client_routing, incoming) = bind_listener(opt, node.node_id()).await?;
        Ok(move || Self {
            task: Task::spawn(run_ascendd(node, sockpath, incoming, client_routing, link)),
        })
    }

    pub async fn new(opt: Opt, node: Arc<overnet_core::Router>) -> Result<Self, Error> {
        Self::prime(opt, node).await.map(|f| f())
    }
}

impl Future for Ascendd {
    type Output = Result<(), Error>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.task.poll_unpin(ctx)
    }
}

/// Error returned from [`run_stream`].
#[derive(Debug)]
pub enum RunStreamError {
    Circuit(circuit::Error),
    EarlyHangup(std::io::Error),
    Unsupported,
}

/// Run an ascendd server on the given stream IOs identified by the given labels
/// and paths, to completion.
pub async fn run_stream<'a>(
    node: Arc<overnet_core::Router>,
    rx: &'a mut (dyn AsyncRead + Unpin + Send),
    tx: &'a mut (dyn AsyncWrite + Unpin + Send),
) -> Result<(), RunStreamError> {
    let mut id = [0; 8];

    if let Err(e) = rx.read_exact(&mut id).await {
        return Err(RunStreamError::EarlyHangup(e));
    }

    if id == CIRCUIT_ID {
        let (errors_sender, errors) = unbounded();
        futures::future::join(
            circuit::multi_stream::multi_stream_node_connection_to_async(
                node.circuit_node(),
                rx,
                tx,
                true,
                circuit::Quality::LOCAL_SOCKET,
                errors_sender,
                "ascendd client".to_owned(),
            ),
            errors
                .map(|e| {
                    log::warn!("An ascendd client circuit stream failed: {e:?}");
                })
                .collect::<()>(),
        )
        .map(|(result, ())| result)
        .await
        .map_err(RunStreamError::Circuit)
    } else {
        Err(RunStreamError::Unsupported)
    }
}

/// Run a connection to another ascendd server.
/// and paths, to completion.
pub async fn run_linked_ascendd<'a>(
    node: Arc<overnet_core::Router>,
    rx: &'a mut (dyn AsyncRead + Unpin + Send),
    tx: &'a mut (dyn AsyncWrite + Unpin + Send),
) {
    let (errors_sender, errors) = unbounded();
    if let Err(e) = futures::future::join(
        async move {
            tx.write_all(&CIRCUIT_ID).await?;
            circuit::multi_stream::multi_stream_node_connection_to_async(
                node.circuit_node(),
                rx,
                tx,
                false,
                circuit::Quality::LOCAL_SOCKET,
                errors_sender,
                "ascendd client".to_owned(),
            )
            .await
        },
        errors
            .map(|e| {
                log::warn!("An ascendd linking circuit stream failed: {e:?}");
            })
            .collect::<()>(),
    )
    .map(|(result, ())| result)
    .await
    {
        log::debug!(err:? = e; "Link to other ascendd closed");
    }
}

async fn bind_listener(
    opt: Opt,
    node_id: overnet_core::NodeId,
) -> Result<(PathBuf, AscenddClientRouting, UnixListener), Error> {
    let Opt { sockpath, serial: _, client_routing, link: _ } = opt;
    let client_routing =
        if client_routing { AscenddClientRouting::Enabled } else { AscenddClientRouting::Disabled };
    log::debug!(node_id = node_id.0; "starting ascendd on {}", sockpath.display(),);

    let incoming = loop {
        let safe_socket_path = short_socket_path(&sockpath)?;
        match UnixListener::bind(&safe_socket_path) {
            Ok(listener) => {
                break listener;
            }
            Err(e) if e.kind() == ErrorKind::AddrInUse => {
                match UnixStream::connect(&safe_socket_path)
                    .on_timeout(Duration::from_secs(1), || {
                        Err(std::io::Error::new(
                            TimedOut,
                            format_err!("connecting to ascendd socket"),
                        ))
                    })
                    .await
                {
                    Ok(_) => {
                        log::error!(
                            "another ascendd is already listening at {}",
                            sockpath.display()
                        );
                        bail!("another ascendd is aleady listening at {}!", sockpath.display());
                    }
                    Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
                        log::info!(
                            "trying to clean up stale ascendd socket at {} (error: {e:?})",
                            sockpath.display()
                        );
                        std::fs::remove_file(&sockpath)?;
                    }
                    Err(e) => {
                        log::info!(
                            "An unexpected error occurred while trying to connect to the existing ascendd socket at: {}: {e:?}",
                            sockpath.display()
                        );
                        bail!(
                            "unexpected error while trying to connect to the existing ascendd socket at: {}: {e}",
                            sockpath.display()
                        );
                    }
                }
            }
            Err(e) => {
                log::info!(
                    "An unexpected error occurred while trying to bind to the ascendd socket at {}: {e:?}",
                    sockpath.display()
                );
                bail!(
                    "unexpected error while trying to bind to ascendd socket at {}: {e}",
                    sockpath.display()
                );
            }
        }
    };

    // as this file is purely advisory, we won't fail for any error, but we can log it.
    if let Err(e) = write_pidfile(&sockpath, std::process::id()) {
        log::warn!("failed to write pidfile alongside {}: {e:?}", sockpath.display());
    }
    Ok((sockpath, client_routing, incoming))
}

/// Writes a pid file alongside the socketpath so we know what pid last successfully tried to
/// create a socket there.
fn write_pidfile(sockpath: &Path, pid: u32) -> anyhow::Result<()> {
    let in_dir = sockpath.parent().context("No parent directory for socket path")?;
    let mut pidfile = tempfile::NamedTempFile::new_in(in_dir)?;
    write!(pidfile, "{pid}")?;
    pidfile.persist(sockpath.with_extension("pid"))?;
    Ok(())
}

async fn run_ascendd(
    node: Arc<overnet_core::Router>,
    sockpath: PathBuf,
    incoming: UnixListener,
    client_routing: AscenddClientRouting,
    link: Vec<PathBuf>,
) -> Result<(), Error> {
    node.set_client_routing(client_routing);

    log::debug!("ascendd listening to socket {}", sockpath.display());

    futures::future::try_join(
        futures::stream::iter(link.into_iter().map(Ok)).try_for_each_concurrent(None, |path| {
            let node = Arc::clone(&node);
            async move {
                let sock = loop {
                    match UnixStream::connect(&path).await {
                        Ok(sock) => break sock,
                        Err(e) => {
                            log::debug!(path:? = path.display(), error:? = e;
                            "Linking to socket failed. Retrying..");
                            fuchsia_async::Timer::new(std::time::Duration::from_secs(1)).await
                        }
                    }
                };
                let (mut rx, mut tx) = sock.into_multithreaded_futures_stream().split();
                run_linked_ascendd(node, &mut rx, &mut tx).await;
                Ok(())
            }
        }),
        {
            let node = Arc::clone(&node);
            async move {
                UnixListenerStream(incoming)
                    .for_each_concurrent(None, |stream| {
                        let node = Arc::clone(&node);
                        async move {
                            match stream {
                                Ok(stream) => {
                                    let stream = stream.into_multithreaded_futures_stream();
                                    let (mut rx, mut tx) = stream.split();
                                    if let Err(e) =
                                        run_stream(Arc::clone(&node), &mut rx, &mut tx).await
                                    {
                                        match e {
                                            RunStreamError::Circuit(
                                                circuit::Error::ConnectionClosed(reason),
                                            ) => {
                                                log::debug!("Circuit connection closed: {reason:?}")
                                            }
                                            RunStreamError::Circuit(other) => {
                                                log::warn!("Failed serving socket: {other:?}")
                                            }
                                            RunStreamError::EarlyHangup(e) => {
                                                log::debug!("Socket hung up early: {e:?}")
                                            }
                                            RunStreamError::Unsupported => {
                                                log::warn!("Socket was not a circuit connection.")
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::warn!("Failed starting socket: {:?}", e);
                                }
                            }
                        }
                    })
                    .await;
                Ok(())
            }
        },
    )
    .await
    .map(drop)
}
