// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_async as fasync;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::{Future, TryFutureExt, ready};
use log::error;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};
use zx_status_ext::IoErrorKindExt;

use super::{Connection, ConnectionBackendType};

/// A socket-based implementation of the Bluetooth channel transport.
#[derive(Debug)]
pub struct SocketConnection {
    socket: fasync::Socket,
    send_buffer: VecDeque<Vec<u8>>,
}

impl SocketConnection {
    const MAX_QUEUED_PACKETS: usize = 32;

    pub fn new(socket: zx::Socket) -> Self {
        Self {
            socket: fasync::Socket::from_socket(socket),
            send_buffer: VecDeque::with_capacity(Self::MAX_QUEUED_PACKETS),
        }
    }

    pub fn into_zx_socket(self) -> zx::Socket {
        self.socket.into_zx_socket()
    }
}

impl Connection for SocketConnection {
    fn closed(&self) -> Pin<Box<dyn Future<Output = Result<(), zx::Status>> + Send + 'static>> {
        let handle = match self.socket.as_ref().duplicate_handle(zx::Rights::SAME_RIGHTS) {
            Ok(h) => h,
            Err(e) => return Box::pin(futures::future::ready(Err(e))),
        };
        Box::pin(async move {
            let wait = fasync::OnSignals::new(handle, zx::Signals::SOCKET_PEER_CLOSED);
            wait.map_ok(|_| ()).await
        })
    }

    fn connection_type(&self) -> ConnectionBackendType {
        ConnectionBackendType::Socket
    }

    fn write(&self, bytes: &[u8]) -> Result<usize, zx::Status> {
        self.socket.as_ref().write(bytes)
    }

    fn is_closed(&self) -> bool {
        self.socket.is_closed()
    }

    fn into_fidl_channel(self: Box<Self>) -> Result<bredr::Channel, zx::Status> {
        let socket = self.into_zx_socket();
        Ok(bredr::Channel { socket: Some(socket), ..Default::default() })
    }
}

impl Stream for SocketConnection {
    type Item = Result<Vec<u8>, zx::Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut res = Vec::<u8>::new();
        loop {
            break match self.socket.poll_datagram(cx, &mut res) {
                Poll::Ready(Ok(0)) => continue,
                Poll::Ready(Ok(_size)) => Poll::Ready(Some(Ok(res))),
                Poll::Ready(Err(zx::Status::PEER_CLOSED)) => Poll::Ready(None),
                Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
                Poll::Pending => Poll::Pending,
            };
        }
    }
}

impl Sink<Vec<u8>> for SocketConnection {
    type Error = zx::Status;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let _ = Sink::poll_flush(self.as_mut(), cx)?;

        if self.send_buffer.len() >= SocketConnection::MAX_QUEUED_PACKETS {
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        self.get_mut().send_buffer.push_back(item);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        use futures::io::AsyncWrite;
        while let Some(item) = this.send_buffer.front() {
            let res =
                Pin::new(&mut this.socket).poll_write(cx, item).map_err(|e| e.kind().to_status());
            match res {
                Poll::Ready(Ok(size)) => {
                    if size == item.len() {
                        let _ = this.send_buffer.pop_front();
                    } else {
                        error!(
                            "Partial write in SocketConnection::Sink::poll_flush: wrote {} bytes of {} byte packet.",
                            size,
                            item.len()
                        );
                        let item = this.send_buffer.front_mut().unwrap();
                        *item = item.split_off(size);
                    }
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            }
        }
        Pin::new(&mut this.socket).poll_flush(cx).map_err(|e| e.kind().to_status())
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(Sink::poll_flush(self.as_mut(), cx))?;
        let this = self.get_mut();
        use futures::io::AsyncWrite as _;
        Pin::new(&mut this.socket).poll_close(cx).map_err(|e| e.kind().to_status())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Channel;
    use futures::stream::FusedStream;
    use futures::{SinkExt, StreamExt};
    use std::pin::pin;

    #[test]
    fn channel_sync_write() {
        let mut exec = fasync::TestExecutor::new();
        let (mut recv, send) = Channel::create_socket_pair();

        let heart: &[u8] = &[0xF0, 0x9F, 0x92, 0x96];
        let size = send.write(heart).expect("write to succeed");
        assert_eq!(size, heart.len());

        let mut recv_fut = recv.next();
        match exec.run_until_stalled(&mut recv_fut) {
            Poll::Ready(Some(Ok(bytes))) => {
                assert_eq!(heart, &bytes);
            }
            x => panic!("Expected Some(Ok(bytes)) from the stream, got {x:?}"),
        };
    }

    #[test]
    fn channel_into_fidl() {
        let _exec = fasync::TestExecutor::new();
        let (remote, _local) = zx::Socket::create_datagram();
        let conn = SocketConnection::new(remote);

        let fidl_channel =
            Box::new(conn).into_fidl_channel().expect("into_fidl_channel to succeed");
        assert!(fidl_channel.socket.is_some());
        assert!(fidl_channel.connection.is_none());
    }

    #[test]
    fn channel_closed() {
        let mut exec = fasync::TestExecutor::new();

        let (recv, send) = Channel::create_socket_pair();

        let closed_fut = recv.closed();
        let mut closed_fut = pin!(closed_fut);

        assert!(exec.run_until_stalled(&mut closed_fut).is_pending());
        assert!(!recv.is_closed());

        drop(send);

        assert!(exec.run_until_stalled(&mut closed_fut).is_ready());
        assert!(recv.is_closed());
    }

    #[test]
    fn channel_sink() {
        let mut exec = fasync::TestExecutor::new();
        let (mut recv, mut send) = Channel::create_socket_pair();

        let data = vec![0x01, 0x02, 0x03, 0x04];
        let mut send_fut = send.send(data.clone());

        // The send should complete immediately as the socket has space.
        match exec.run_until_stalled(&mut send_fut) {
            Poll::Ready(Ok(())) => {}
            x => panic!("Expected Ready(Ok(())), got {:?}", x),
        }

        let mut recv_fut = recv.next();
        match exec.run_until_stalled(&mut recv_fut) {
            Poll::Ready(Some(Ok(bytes))) => assert_eq!(data, bytes),
            x => panic!("Expected successful read, got {x:?}"),
        }
    }

    #[test]
    fn channel_stream() {
        let mut exec = fasync::TestExecutor::new();
        let (remote, local) = zx::Socket::create_datagram();
        let mut recv = Channel::from_socket(remote, Channel::DEFAULT_MAX_TX).unwrap();
        let send = local;

        let mut stream_fut = recv.next();

        assert!(exec.run_until_stalled(&mut stream_fut).is_pending());

        let heart: &[u8] = &[0xF0, 0x9F, 0x92, 0x96];
        let _ = send.write(heart).expect("should write successfully");

        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Some(Ok(bytes))) => {
                assert_eq!(heart.to_vec(), bytes);
            }
            x => panic!("Expected Some(Ok(bytes)) from the stream, got {x:?}"),
        }

        // After the sender is dropped, the stream should terminate.
        drop(send);

        let mut stream_fut = recv.next();
        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(None) => {}
            x => panic!("Expected None from the stream after close, got {x:?}"),
        }

        // It should continue to report terminated.
        assert!(recv.is_terminated());
    }
}
