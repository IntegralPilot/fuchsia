// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides async Socket type wrapped around an emulated zircon socket.

// TODO(ctiller): merge this implementation with the implementation in zircon_handle?

use futures::future::poll_fn;
use futures::prelude::*;
use futures::ready;
use std::pin::Pin;
use std::task::{Context, Poll};
use zx_status;
use zx_status_ext::StatusExt;

/// An I/O object representing a `Socket`.
pub struct Socket {
    socket: super::Socket,
}

impl AsRef<super::Socket> for Socket {
    fn as_ref(&self) -> &super::Socket {
        &self.socket
    }
}

impl super::AsHandleRef for Socket {
    fn as_handle_ref(&self) -> super::HandleRef<'_> {
        self.socket.as_handle_ref()
    }
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.socket.fmt(f)
    }
}

impl Socket {
    /// Construct an `Socket` from an existing `emulated_handle::Socket`
    pub fn from_socket(socket: super::Socket) -> Self {
        Socket { socket }
    }

    /// Convert AsyncSocket back into a regular socket
    pub fn into_zx_socket(self) -> super::Socket {
        self.socket
    }

    /// Polls for the next data on the socket, appending it to the end of |out| if it has arrived.
    /// Not very useful for a non-datagram socket as it will return all available data
    /// on the socket.
    pub fn poll_datagram(
        &self,
        cx: &mut Context<'_>,
        out: &mut Vec<u8>,
    ) -> Poll<Result<usize, zx_status::Status>> {
        let avail = self.socket.outstanding_read_bytes()?;
        let len = out.len();
        out.resize(len + avail, 0);
        let (_, tail) = out.split_at_mut(len);
        match ready!(self.socket.poll_read(tail, cx)) {
            Err(zx_status::Status::PEER_CLOSED) => Poll::Ready(Ok(0)),
            Err(e) => Poll::Ready(Err(e)),
            Ok(bytes) => {
                if bytes == avail {
                    Poll::Ready(Ok(bytes))
                } else {
                    Poll::Ready(Err(zx_status::Status::BAD_STATE))
                }
            }
        }
    }

    /// Attempt to read from the socket, registering for wakeup if the socket doesn’t have any
    /// contents available. Used internally in the `AsyncRead` implementation, exposed for users who
    /// know the concrete type they’re using and don’t want to pin the socket.
    pub fn poll_read_ref(
        &self,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<Result<usize, zx_status::Status>> {
        self.socket.poll_read(out, cx)
    }

    /// Reads the next datagram that becomes available onto the end of |out|.  Note: Using this
    /// multiple times concurrently is an error and the first one will never complete.
    pub async fn read_datagram<'a>(
        &'a self,
        out: &'a mut Vec<u8>,
    ) -> Result<usize, zx_status::Status> {
        poll_fn(move |cx| self.poll_datagram(cx, out)).await
    }
}

impl tokio::io::AsyncWrite for Socket {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, futures::io::Error>> {
        futures::io::AsyncWrite::poll_write(self, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), tokio::io::Error>> {
        futures::io::AsyncWrite::poll_flush(self, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), tokio::io::Error>> {
        futures::io::AsyncWrite::poll_close(self, cx)
    }
}

impl tokio::io::AsyncRead for Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        futures::io::AsyncRead::poll_read(self, cx, buf.initialize_unfilled())
            .map(|value| value.map(|size| buf.advance(size)))
    }
}

impl AsyncWrite for Socket {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready(self.socket.write(bytes).map_err(|e| e.into_io_error()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match ready!(self.socket.poll_read(bytes, cx)) {
            Err(zx_status::Status::PEER_CLOSED) => Poll::Ready(Ok(0)),
            Ok(x) => {
                assert_ne!(x, 0);
                Poll::Ready(Ok(x))
            }
            Err(x) => Poll::Ready(Err(x.into_io_error())),
        }
    }
}

impl AsyncWrite for &'_ Socket {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready(self.socket.write(bytes).map_err(|e| e.into_io_error()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for &'_ Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match ready!(self.socket.poll_read(bytes, cx)) {
            Err(zx_status::Status::PEER_CLOSED) => Poll::Ready(Ok(0)),
            Ok(x) => {
                assert_ne!(x, 0);
                Poll::Ready(Ok(x))
            }
            Err(x) => Poll::Ready(Err(x.into_io_error())),
        }
    }
}

#[cfg(test)]
mod test {
    use super::super::Socket;
    use super::Socket as AsyncSocket;
    use futures::executor::block_on;
    use futures::prelude::*;
    use std::pin::Pin;
    use std::task::Context;

    #[test]
    fn async_socket_write_read() {
        block_on(async move {
            let (a, b) = Socket::create_stream();
            let (mut a, mut b) = (AsyncSocket::from_socket(a), AsyncSocket::from_socket(b));
            let mut buf = [0u8; 128];

            let mut cx = Context::from_waker(std::task::Waker::noop());

            let mut rx = b.read(&mut buf);
            assert!(Pin::new(&mut rx).poll(&mut cx).is_pending());
            assert!(Pin::new(&mut a.write(&[1, 2, 3])).poll(&mut cx).is_ready());
            rx.await.unwrap();
            assert_eq!(&buf[0..3], &[1, 2, 3]);

            let mut rx = a.read(&mut buf);
            assert!(Pin::new(&mut rx).poll(&mut cx).is_pending());
            assert!(Pin::new(&mut b.write(&[1, 2, 3])).poll(&mut cx).is_ready());
            rx.await.unwrap();
            assert_eq!(&buf[0..3], &[1, 2, 3]);
        })
    }
}
