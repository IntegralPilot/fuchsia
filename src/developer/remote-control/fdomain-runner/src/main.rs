// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fidl_fuchsia_developer_remotecontrol_connector::ConnectorMarker;
use fuchsia_component::client::connect_to_protocol;
use futures::future::{poll_fn, select};
use futures::io::BufReader;
use futures::prelude::*;
use logging_util::FfxLogGuard;
use std::io::{self, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::io::AsRawFd;
use std::pin::pin;
use std::task::{Poll, ready};

const BUFFER_SIZE: usize = 65536;

#[derive(Copy, Clone)]
enum CopyDirection {
    StdIn,
    StdOut,
}

impl CopyDirection {
    fn log_read_fail(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => log::warn!("Failed receiving data from host: {err:?}"),
            CopyDirection::StdOut => log::warn!("Failed receiving data from RCS: {err:?}"),
        }
    }

    fn log_write_fail(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => log::warn!("Failed sending data to RCS: {err:?}"),
            CopyDirection::StdOut => log::warn!("Failed sending data to host: {err:?}"),
        }
    }

    fn log_write_zero(&self) {
        match self {
            CopyDirection::StdIn => log::error!(
                "Writing to RCS socket returned zero-byte success. This should be impossible?!"
            ),
            CopyDirection::StdOut => log::error!(
                "Writing to host socket returned zero-byte success. This should be impossible?!"
            ),
        }
    }

    fn log_flush_error(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => {
                log::warn!("Flushing data toward RCS after shutdown gave {err:?}")
            }
            CopyDirection::StdOut => {
                log::warn!("Flushing data toward the host after shutdown gave {err:?}")
            }
        }
    }

    fn log_closed(&self) {
        match self {
            CopyDirection::StdIn => {
                log::debug!("Stream from the host toward RCS terminated normally")
            }
            CopyDirection::StdOut => {
                log::debug!("Stream from RCS toward the host terminated normally")
            }
        }
    }
}

async fn buffered_copy<R, W>(mut from: R, to: W, dir: CopyDirection)
where
    R: AsyncRead + std::marker::Unpin,
    W: AsyncWrite + std::marker::Unpin,
{
    let mut from = pin!(BufReader::with_capacity(BUFFER_SIZE, &mut from));
    let mut to = pin!(to);
    poll_fn(move |cx| {
        loop {
            let buffer = match ready!(from.as_mut().poll_fill_buf(cx)) {
                Ok(x) => x,
                Err(e) => {
                    dir.log_read_fail(e);
                    return Poll::Ready(());
                }
            };
            if buffer.is_empty() {
                if let Err(e) = ready!(to.as_mut().poll_flush(cx)) {
                    dir.log_flush_error(e)
                }
                dir.log_closed();
                return Poll::Ready(());
            }

            let i = match ready!(to.as_mut().poll_write(cx, buffer)) {
                Ok(x) => x,
                Err(e) => {
                    dir.log_write_fail(e);
                    return Poll::Ready(());
                }
            };
            if i == 0 {
                dir.log_write_zero();
                return Poll::Ready(());
            }
            from.as_mut().consume(i);
        }
    })
    .await
}

/// Utility to bridge an FDomain/RCS connection via SSH. If you're running this
/// manually, you are probably doing something wrong.
#[derive(FromArgs)]
struct Args {
    /// log ID number from ffx to bookend connection logs.
    #[argh(option)]
    log_id: Option<String>,
}

#[fuchsia::main(logging_tags = ["remote_control_fdomain_runner"])]
async fn main() -> anyhow::Result<()> {
    let args: Args = argh::from_env();
    let _log_guard = FfxLogGuard::new(&args.log_id);

    let rcs_proxy = connect_to_protocol::<ConnectorMarker>()?;
    let (local_socket, remote_socket) = fidl::Socket::create_stream();

    rcs_proxy.fdomain_toolbox_socket(remote_socket).await?;

    let local_socket = fidl::AsyncSocket::from_socket(local_socket);
    let (mut rx_socket, mut tx_socket) = futures::AsyncReadExt::split(local_socket);

    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    // We just need something to come through on the output stream that we can
    // read before the data starts to know the command was invoked successfully.
    stdout.write_all(b"OK\n")?;
    stdout.flush()?;

    // SAFETY: In order to remove the overhead of FDIO, we want to extract out the underlying
    // handles of STDIN and STDOUT and forward them to our sockets. That requires us to transfer
    // the sockets out of fdio, but unfortunately Rust doesn't allow us to take ownership from
    // `std::io::stdin()` and `std::io::stdout()`. To work around that, we grab the STDIN and
    // STDOUT locks to prevent any other thread from accessing them while we're streaming traffic.
    let (stdin_fd, stdout_fd) = unsafe {
        (OwnedFd::from_raw_fd(stdin.as_raw_fd()), OwnedFd::from_raw_fd(stdout.as_raw_fd()))
    };

    let stdin = fdio::transfer_fd(stdin_fd)?;
    let stdout = fdio::transfer_fd(stdout_fd)?;

    let mut stdin = fidl::AsyncSocket::from_socket(fidl::Socket::from(stdin));
    let mut stdout = fidl::AsyncSocket::from_socket(fidl::Socket::from(stdout));

    let in_fut = buffered_copy(&mut stdin, &mut tx_socket, CopyDirection::StdIn);
    let out_fut = buffered_copy(&mut rx_socket, &mut stdout, CopyDirection::StdOut);
    select(pin!(in_fut), pin!(out_fut)).await;

    Ok(())
}
