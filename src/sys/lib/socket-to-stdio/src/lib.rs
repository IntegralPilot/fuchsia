// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fidl_fuchsia_io as fio;
use futures::future::Either;
use futures::stream::StreamExt as _;
use futures::{AsyncReadExt as _, AsyncWriteExt as _};
use std::io::StdoutLock;
use termion::raw::IntoRawMode as _;

/// Abstracts stdout for `connect_socket_to_stdio`. Allows callers to determine if stdout should be
/// exclusively owned for the duration of the call.
pub enum Stdout<'a> {
    /// Exclusive ownership of stdout (nothing else can write to stdout while this exists),
    /// put into raw mode.
    Raw(termion::raw::RawTerminal<StdoutLock<'a>>),
    /// Shared ownership of stdout (output may be interleaved with output from other sources).
    Buffered,
}

impl std::io::Write for Stdout<'_> {
    fn flush(&mut self) -> Result<(), std::io::Error> {
        match self {
            Self::Raw(r) => r.flush(),
            Self::Buffered => std::io::stdout().flush(),
        }
    }
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        match self {
            Self::Raw(r) => r.write(buf),
            Self::Buffered => std::io::stdout().write(buf),
        }
    }
}

impl Stdout<'_> {
    pub fn raw() -> anyhow::Result<Self> {
        let stdout = std::io::stdout();

        if !termion::is_tty(&stdout) {
            anyhow::bail!("interactive mode does not support piping");
        }

        // Put the host terminal into raw mode, so input characters are not echoed, streams are not
        // buffered and newlines are not changed.
        let term_out =
            stdout.lock().into_raw_mode().context("could not set raw mode on terminal")?;

        Ok(Self::Raw(term_out))
    }

    pub fn buffered() -> Self {
        Self::Buffered
    }
}

/// Concurrently:
///   1. locks stdin and copies the input to `socket`
///   2. reads data from `socket` and writes it to `stdout`
/// Finishes when the remote end of the socket closes (when (2) completes).
pub async fn connect_socket_to_stdio(
    socket: fidl::Socket,
    stdout: Stdout<'_>,
) -> anyhow::Result<()> {
    connect_socket_to_stdio_impl(
        fuchsia_async::Socket::from_socket(socket),
        || std::io::stdin().lock(),
        stdout,
    )?
    .await
}

/// Same as `connect_socket_to_stdio` but operates on an FDomain socket.
pub async fn connect_fdomain_socket_to_stdio(
    socket: fdomain_client::Socket,
    stdout: Stdout<'_>,
) -> anyhow::Result<()> {
    connect_socket_to_stdio_impl(socket, || std::io::stdin().lock(), stdout)?.await
}

fn connect_socket_to_stdio_impl<R>(
    socket: impl futures::AsyncRead + futures::AsyncWrite,
    stdin: impl FnOnce() -> R + Send + 'static,
    mut stdout: impl std::io::Write,
) -> anyhow::Result<impl futures::Future<Output = anyhow::Result<()>>>
where
    R: std::io::Read,
{
    // Use a separate thread to read from stdin without blocking the executor.
    let (stdin_send, mut stdin_recv) = futures::channel::mpsc::unbounded();
    let _: std::thread::JoinHandle<_> = std::thread::Builder::new()
        .name("connect_socket_to_stdio stdin thread".into())
        .spawn(move || {
            let mut stdin = stdin();
            let mut buf = [0u8; fio::MAX_BUF as usize];
            loop {
                let bytes_read = stdin.read(&mut buf)?;
                if bytes_read == 0 {
                    return Ok::<(), anyhow::Error>(());
                }
                let () = stdin_send.unbounded_send(buf[..bytes_read].to_vec())?;
            }
        })
        .context("spawning stdin thread")?;

    let (mut socket_in, mut socket_out) = socket.split();

    let stdin_to_socket = async move {
        while let Some(stdin) = stdin_recv.next().await {
            socket_out.write_all(&stdin).await.context("writing to socket")?;
            socket_out.flush().await.context("flushing socket")?;
        }
        Ok::<(), anyhow::Error>(())
    };

    let socket_to_stdout = async move {
        let mut buf = vec![0u8; fio::MAX_BUF as usize];
        loop {
            let bytes_read = socket_in.read(&mut buf).await.context("reading from socket")?;
            if bytes_read == 0 {
                break;
            }
            stdout.write_all(&buf[..bytes_read]).context("writing to stdout")?;
            stdout.flush().context("flushing stdout")?;
        }
        Ok::<(), anyhow::Error>(())
    };

    Ok(async move {
        futures::pin_mut!(stdin_to_socket);
        futures::pin_mut!(socket_to_stdout);
        Ok(match futures::future::select(stdin_to_socket, socket_to_stdout).await {
            Either::Left((stdin_to_socket, socket_to_stdout)) => {
                let () = stdin_to_socket?;
                // Wait for output even after stdin closes. The remote may be responding to the
                // final input, or the remote may not be reading from stdin at all (consider
                // "bash -c $CMD").
                let () = socket_to_stdout.await?;
            }
            Either::Right((socket_to_stdout, _)) => {
                let () = socket_to_stdout?;
                // No reason to wait for stdin because the socket is closed so writing stdin to it
                // would fail.
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn stdin_to_socket() {
        let (socket, socket_remote) = fidl::Socket::create_stream();
        let socket_remote = fuchsia_async::Socket::from_socket(socket_remote);

        let connect_fut =
            connect_socket_to_stdio_impl(socket_remote, || &b"test input"[..], vec![]).unwrap();

        let (connect_res, bytes_from_socket) = futures::join!(connect_fut, async move {
            let mut socket = fuchsia_async::Socket::from_socket(socket);
            let mut out = vec![0u8; 100];
            let bytes_read = socket.read(&mut out).await.unwrap();
            drop(socket);
            out.resize(bytes_read, 0);
            out
        });
        let () = connect_res.unwrap();

        assert_eq!(bytes_from_socket, &b"test input"[..]);
    }

    #[fuchsia::test]
    async fn socket_to_stdout() {
        let (socket, socket_remote) = fidl::Socket::create_stream();
        assert_eq!(socket.write(&b"test input"[..]).unwrap(), 10);
        drop(socket);
        let mut stdout = vec![];
        let (unblocker, block_until) = std::sync::mpsc::channel();

        let socket_remote = fuchsia_async::Socket::from_socket(socket_remote);
        let () = connect_socket_to_stdio_impl(
            socket_remote,
            move || {
                let () = block_until.recv().unwrap();
                &[][..]
            },
            &mut stdout,
        )
        .unwrap()
        .await
        .unwrap();

        // let the stdin_to_socket thread finish before test cleanup
        unblocker.send(()).unwrap();

        assert_eq!(&stdout[..], &b"test input"[..]);
    }
}
