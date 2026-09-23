// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_client::Client;
use fdomain_client::fidl::{ProtocolMarker, Proxy};
use fdomain_fuchsia_developer_remotecontrol as rcs;
use fdomain_fuchsia_io as fio;
use fidl_fuchsia_developer_remotecontrol_connector::ConnectorMarker;
use fuchsia_component::client::connect_to_protocol;
use futures::stream::Stream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use zx_status_ext::StatusExt;

/// An FDomain client transport that works over a Fuchsia socket. Uses 32-bit
/// little endian frame sizes before each frame per the documentation of the
/// Connector protocol.
struct SocketTransport {
    socket: fuchsia_async::Socket,
    out_buf: Vec<u8>,
    in_buf: Vec<u8>,
}

impl fdomain_client::FDomainTransport for SocketTransport {
    fn poll_send_message(
        mut self: Pin<&mut Self>,
        msg: &[u8],
        ctx: &mut Context<'_>,
    ) -> Poll<fdomain_client::Result<(), Option<io::Error>>> {
        if self.out_buf.is_empty() {
            let len = msg.len();
            let len: u32 = len.try_into().unwrap();
            self.out_buf.extend_from_slice(&len.to_le_bytes());
            self.out_buf.extend_from_slice(msg);
        }

        while !self.out_buf.is_empty() {
            let res = ready!(self.socket.poll_write_ref(ctx, &self.out_buf))
                .map_err(|x| Some(x.into_io_error()))?;
            if res == 0 {
                return Poll::Ready(Err(None));
            }
            self.out_buf.drain(..res);
        }

        Poll::Ready(Ok(()))
    }
}

impl Stream for SocketTransport {
    type Item = io::Result<Box<[u8]>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut orig_size = self.in_buf.len();
        let this = self.get_mut();
        loop {
            let mut blocked = false;
            extend_buf(&mut this.in_buf);
            let sock = &mut this.socket;
            let buf = &mut this.in_buf;

            match sock.poll_read_ref(cx, &mut buf[orig_size..]) {
                Poll::Ready(Ok(got)) => {
                    if got == 0 {
                        return Poll::Ready(None);
                    }
                    orig_size += got;
                }
                Poll::Ready(Err(e)) => {
                    this.in_buf.resize(orig_size, 0);
                    if e == fidl::Status::PEER_CLOSED {
                        return Poll::Ready(None);
                    } else {
                        return Poll::Ready(Some(Err(e.into_io_error())));
                    }
                }
                Poll::Pending => {
                    this.in_buf.resize(orig_size, 0);
                    blocked = true;
                }
            }

            if orig_size < 4 {
                if blocked {
                    return Poll::Pending;
                } else {
                    continue;
                }
            }

            let packet_size = u32::from_le_bytes(this.in_buf[..4].try_into().unwrap());
            let packet_size: usize = packet_size.try_into().unwrap();

            if orig_size < packet_size + 4 {
                if blocked {
                    return Poll::Pending;
                } else {
                    continue;
                }
            }

            let tail = this.in_buf.split_off(packet_size + 4);
            let mut packet = std::mem::replace(&mut this.in_buf, tail);
            this.in_buf.resize(orig_size - (packet_size + 4), 0);
            packet.drain(..4);

            return Poll::Ready(Some(Ok(packet.into())));
        }
    }
}

/// Add padding to the end of a buffer to be used as space to read more data.
fn extend_buf(buf: &mut Vec<u8>) {
    const BLOCK_SIZE: usize = 4096;

    let size = buf.len();
    let target_size = (size + (BLOCK_SIZE - 1)) / BLOCK_SIZE * BLOCK_SIZE;
    let target_size =
        if target_size - size < BLOCK_SIZE / 2 { target_size + BLOCK_SIZE } else { target_size };

    buf.resize(target_size, 0);
}

#[fuchsia::test]
async fn test_fdomain_socket() {
    let rcs_proxy = connect_to_protocol::<ConnectorMarker>().unwrap();
    let (local_socket, remote_socket) = fidl::Socket::create_stream();

    rcs_proxy.fdomain_toolbox_socket(remote_socket).await.unwrap();
    let local_socket = fuchsia_async::Socket::from_socket(local_socket);
    let (client, fut) = Client::new(SocketTransport {
        socket: local_socket,
        in_buf: Vec::new(),
        out_buf: Vec::new(),
    });
    fuchsia_async::Task::spawn(fut).detach();

    let ns = client.namespace().await.unwrap();
    let ns = fio::DirectoryProxy::from_channel(ns);
    let (rcs_client, server_end) = client.create_proxy::<rcs::RemoteControlMarker>();
    ns.open(
        rcs::RemoteControlMarker::DEBUG_NAME,
        fio::Flags::PROTOCOL_SERVICE,
        &fio::Options::default(),
        server_end.into_channel(),
    )
    .unwrap();

    assert_eq!("bob", rcs_client.echo_string("bob").await.unwrap());
}
