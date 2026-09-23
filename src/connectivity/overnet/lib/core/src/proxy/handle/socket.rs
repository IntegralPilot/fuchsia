// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::signals::Collector;
use super::{
    IO, IntoProxied, Message, Proxyable, ProxyableRW, ReadValue, RouterHolder, Serializer,
};
use crate::peer::PeerConnRef;
use anyhow::Error;
use fidl::{AsHandleRef, AsyncSocket, Peered, Signals};
use futures::io::{AsyncRead, AsyncWrite};
use futures::ready;
use std::pin::Pin;
use std::task::{Context, Poll};
use zx_status;
use zx_status_ext::IoErrorKindExt;

pub(crate) struct Socket {
    socket: AsyncSocket,
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        "Socket".fmt(f)
    }
}

impl Proxyable for Socket {
    type Message = SocketMessage;

    fn from_fidl_handle(hdl: fidl::NullableHandle) -> Result<Self, Error> {
        Ok(fidl::Socket::from(hdl).into_proxied()?)
    }

    fn into_fidl_handle(self) -> Result<fidl::NullableHandle, Error> {
        Ok(self.socket.into_zx_socket().into_handle())
    }

    fn signal_peer(&self, clear: Signals, set: Signals) -> Result<(), Error> {
        self.socket.as_ref().signal_peer(clear, set)?;
        Ok(())
    }
}

impl<'a> ProxyableRW<'a> for Socket {
    type Reader = SocketReader<'a>;
    type Writer = SocketWriter;
}

impl IntoProxied for fidl::Socket {
    type Proxied = Socket;
    fn into_proxied(self) -> Result<Socket, Error> {
        // TODO(https://fxbug.dev/418249087): Handle socket disposition changes
        // better. By using the async implementation in fuchsia_async, we
        // observe closed on read or write whenever the socket is in a
        // half-closed state. Which is proxied by overnet as a PEER_CLOSED
        // instead. We should intercept the disposition signals there and send
        // them over the proxy.
        Ok(Socket { socket: AsyncSocket::from_socket(self) })
    }
}

pub(crate) struct SocketReader<'a> {
    collector: Collector<'a>,
}

impl<'a> IO<'a> for SocketReader<'a> {
    type Proxyable = Socket;
    type Output = ReadValue;
    fn new() -> Self {
        SocketReader { collector: Default::default() }
    }
    fn poll_io(
        &mut self,
        msg: &mut SocketMessage,
        socket: &'a Socket,
        fut_ctx: &mut Context<'_>,
    ) -> Poll<Result<ReadValue, zx_status::Status>> {
        const MIN_READ_LEN: usize = 65536;
        if msg.0.len() < MIN_READ_LEN {
            msg.0.resize(MIN_READ_LEN, 0u8);
        }
        let read_result = (|| {
            let n = ready!(Pin::new(&mut &socket.socket).poll_read(fut_ctx, &mut msg.0))
                .map_err(|e| e.kind().to_status())?;
            if n == 0 {
                return Poll::Ready(Err(zx_status::Status::PEER_CLOSED));
            }
            msg.0.truncate(n);
            Poll::Ready(Ok(()))
        })();
        self.collector.after_read(fut_ctx, socket.socket.as_handle_ref(), read_result, false)
    }
}

pub(crate) struct SocketWriter;

impl IO<'_> for SocketWriter {
    type Proxyable = Socket;
    type Output = ();
    fn new() -> Self {
        SocketWriter
    }
    fn poll_io(
        &mut self,
        msg: &mut SocketMessage,
        socket: &Socket,
        fut_ctx: &mut Context<'_>,
    ) -> Poll<Result<(), zx_status::Status>> {
        while !msg.0.is_empty() {
            let n = ready!(Pin::new(&mut &socket.socket).poll_write(fut_ctx, &msg.0))
                .map_err(|e| e.kind().to_status())?;
            msg.0.drain(..n);
        }
        Poll::Ready(Ok(()))
    }
}

#[derive(Default, PartialEq)]
pub(crate) struct SocketMessage(Vec<u8>);

impl std::fmt::Debug for SocketMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Message for SocketMessage {
    type Parser = SocketMessageSerializer;
    type Serializer = SocketMessageSerializer;
}

#[derive(Debug)]
pub(crate) struct SocketMessageSerializer;

impl Serializer for SocketMessageSerializer {
    type Message = SocketMessage;
    fn new() -> SocketMessageSerializer {
        SocketMessageSerializer
    }
    fn poll_ser(
        &mut self,
        msg: &mut SocketMessage,
        bytes: &mut Vec<u8>,
        _: PeerConnRef<'_>,
        _: &mut RouterHolder<'_>,
        _: &mut Context<'_>,
    ) -> Poll<Result<(), Error>> {
        std::mem::swap(bytes, &mut msg.0);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncReadExt as _;

    #[fuchsia::test]
    async fn stream_socket_partial_write() {
        let (tx, rx) = fidl::Socket::create_stream();
        let socket = tx.into_proxied().expect("create proxied socket");

        const KERNEL_BUF_SIZE: usize = 257024;
        const EXPECTED_DATA: u8 = 0xff;
        const EXPECTED_LEN: usize = KERNEL_BUF_SIZE * 2;

        let mut writer = SocketWriter::new();
        let mut msg = SocketMessage(vec![EXPECTED_DATA; EXPECTED_LEN]);
        // Write more than the size of the underlying kernel buffer into the
        // proxied socket to exercise that overnet handles partial writes to the
        // zircon socket correctly.
        fuchsia_async::Task::spawn(async {
            futures::future::poll_fn(move |cx| writer.poll_io(&mut msg, &socket, cx))
                .await
                .expect("write to socket")
        })
        .detach();

        let mut data = vec![0u8; EXPECTED_LEN];
        let mut rx = fuchsia_async::Socket::from_socket(rx);
        rx.read_exact(&mut data).await.expect("read from socket");
        assert_eq!(data, vec![EXPECTED_DATA; EXPECTED_LEN]);
    }
}
