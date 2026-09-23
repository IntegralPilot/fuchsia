// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::handle::handle_type;
use crate::responder::Responder;
use crate::{Error, Handle, ordinals};
use fidl_fuchsia_fdomain as proto;
use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

/// A socket in a remote FDomain.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Socket(pub(crate) Handle);

handle_type!(Socket SOCKET peered);

/// Disposition of a socket.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SocketDisposition {
    WriteEnabled,
    WriteDisabled,
}

impl SocketDisposition {
    /// Convert to a proto::SocketDisposition
    fn proto(self) -> proto::SocketDisposition {
        match self {
            SocketDisposition::WriteEnabled => proto::SocketDisposition::WriteEnabled,
            SocketDisposition::WriteDisabled => proto::SocketDisposition::WriteDisabled,
        }
    }
}

impl Socket {
    /// Read up to the given buffer's length from the socket.
    pub fn fdomain_read<'a>(
        &self,
        buf: &'a mut [u8],
    ) -> impl Future<Output = Result<usize, Error>> + 'a {
        let client = Arc::downgrade(&self.0.client());
        let handle = self.0.proto();

        futures::future::poll_fn(move |ctx| {
            client
                .upgrade()
                .unwrap_or_else(|| Arc::clone(&crate::DEAD_CLIENT))
                .poll_socket(handle, ctx, buf)
        })
    }

    /// Polls on reading this socket. Not to be confused with `AsyncRead::poll_read` which has a
    /// different method of error reporting. That will handle errors, whereas this will return them
    /// directly.
    pub fn poll_socket(&self, ctx: &mut Context<'_>, out: &mut [u8]) -> Poll<Result<usize, Error>> {
        let client = self.0.client();
        client.poll_socket(self.0.proto(), ctx, out)
    }

    /// Write all of the given data to the socket.
    pub fn fdomain_write_all(
        &self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Error>> + use<> {
        let data = bytes.to_vec();
        let len = bytes.len();
        let hid = self.0.proto();

        let client = self.0.client();
        client
            .transaction(
                ordinals::WRITE_SOCKET,
                proto::SocketWriteSocketRequest { handle: hid, data },
                move |x| Responder::WriteSocket(x),
            )
            .map(move |x| x.map(|y| assert!(y.wrote as usize == len)))
    }

    /// Set the disposition of this socket and/or its peer.
    pub fn set_socket_disposition(
        &self,
        disposition: Option<SocketDisposition>,
        disposition_peer: Option<SocketDisposition>,
    ) -> impl Future<Output = Result<(), Error>> {
        let disposition =
            disposition.map(SocketDisposition::proto).unwrap_or(proto::SocketDisposition::NoChange);
        let disposition_peer = disposition_peer
            .map(SocketDisposition::proto)
            .unwrap_or(proto::SocketDisposition::NoChange);
        let client = self.0.client();
        let handle = self.0.proto();
        client.transaction(
            ordinals::SET_SOCKET_DISPOSITION,
            proto::SocketSetSocketDispositionRequest { handle, disposition, disposition_peer },
            Responder::SetSocketDisposition,
        )
    }

    /// Split this socket into a streaming reader and a writer. This is more
    /// efficient on the read side if you intend to consume all of the data from
    /// the socket. However it will prevent you from transferring the handle in
    /// the future. It also means data will build up in the buffer, so it may
    /// lead to memory issues if you don't intend to use the data from the
    /// socket as fast as it comes.
    pub fn stream(self) -> Result<(SocketReadStream, SocketWriter), Error> {
        self.0.client().start_socket_streaming(self.0.proto())?;

        let a = Arc::new(self);
        let b = Arc::clone(&a);

        Ok((SocketReadStream(a), SocketWriter(b)))
    }
}

/// A write-only handle to a socket.
pub struct SocketWriter(Arc<Socket>);

impl SocketWriter {
    /// Write all of the given data to the socket.
    pub fn write_all(&self, bytes: &[u8]) -> impl Future<Output = Result<(), Error>> {
        self.0.fdomain_write_all(bytes)
    }
}

/// A stream of data issuing from a socket.
pub struct SocketReadStream(Arc<Socket>);

impl SocketReadStream {
    /// Read from the socket into the supplied buffer. Returns the number of bytes read.
    pub async fn fdomain_read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.0.fdomain_read(buf).await
    }

    /// Turn a `SocketReadStream` and its accompanying `SocketWriter` back
    /// into a `Socket`.
    ///
    /// # Panics
    /// If this stream and the writer passed didn't come from the same call to
    /// `Socket::stream`, or if there is more than one writer.
    pub fn rejoin(mut self, writer: SocketWriter) -> Socket {
        assert!(Arc::ptr_eq(&self.0, &writer.0), "Tried to join stream with wrong writer!");
        if let Some(client) = self.0.0.client.upgrade() {
            client.stop_socket_streaming(self.0.0.proto());
        }
        std::mem::drop(writer);
        let socket = std::mem::replace(&mut self.0, Arc::new(Socket(Handle::invalid())));
        Arc::try_unwrap(socket).expect("Stream pointer no longer unique!")
    }
}

impl futures::AsyncRead for SocketReadStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        convert_poll_res_to_async_read(self.0.poll_socket(cx, buf))
    }
}

impl futures::AsyncRead for &SocketReadStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        convert_poll_res_to_async_read(self.0.poll_socket(cx, buf))
    }
}

impl Drop for SocketReadStream {
    fn drop(&mut self) {
        if let Some(client) = self.0.0.client.upgrade() {
            client.stop_socket_streaming(self.0.0.proto());
        }
    }
}

/// Wrapper for [`Client::poll_socket`] that adapts the return value semantics
/// to what Unix prescribes, and what `futures::io` thus prescribes.
fn convert_poll_res_to_async_read(
    poll_res: Poll<Result<usize, Error>>,
) -> Poll<std::io::Result<usize>> {
    let res = ready!(poll_res).or_else(|e| match e {
        Error::FDomain(proto::Error::TargetError(e))
            if e == zx_status::Status::PEER_CLOSED.into_raw()
                || e == zx_status::Status::BAD_STATE.into_raw() =>
        {
            Ok(0)
        }
        other => Err(std::io::Error::other(other)),
    });
    Poll::Ready(res)
}

impl futures::AsyncRead for Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        convert_poll_res_to_async_read(self.poll_socket(cx, buf))
    }
}

impl futures::AsyncRead for &Socket {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        convert_poll_res_to_async_read(self.poll_socket(cx, buf))
    }
}

impl futures::AsyncWrite for Socket {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let _ = self.fdomain_write_all(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.0 = Handle::invalid();
        Poll::Ready(Ok(()))
    }
}
