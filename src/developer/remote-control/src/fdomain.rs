// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_container::FDomain;
use fdomain_container::wire::FDomainCodec;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use log::{debug, error};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::rc::Weak;
use std::task::{Context, Poll};

/// Returns a sender that you can send fio::Directory server ends to, and they
/// will have the toolbox namespace served on them.
fn serve_toolboxes(
    rcs: Weak<remote_control::RemoteControlService>,
) -> (
    UnboundedSender<fidl::endpoints::ServerEnd<fidl_fuchsia_io::DirectoryMarker>>,
    impl Future<Output = ()>,
) {
    let (toolbox_server_sender, mut toolbox_servers) = unbounded::<fidl::endpoints::ServerEnd<_>>();
    let toolbox_task = async move {
        while let Some(server_end) = toolbox_servers.next().await {
            let Some(rcs) = rcs.upgrade() else {
                error!("RCS disappeared with pending toolbox request");
                break;
            };

            if let Err(err) = rcs.open_toolbox(server_end.into_channel()).await {
                error!(err:?; "Could not open toolbox for client");
            }
        }

        // No more incoming connections, so go to sleep. We'll wait for the
        // socket to close before we stop serving requests.
        futures::future::pending::<()>().await;
    };
    (toolbox_server_sender, toolbox_task)
}

/// Given an async socket and some data to be written to it, try to
/// asynchronously write some data to the socket. Only ever returns Ready when
/// the socket is closed.
fn poll_process_out_queue(
    socket: &fuchsia_async::Socket,
    out_queue: &mut VecDeque<u8>,
    ctx: &mut Context<'_>,
) -> Poll<()> {
    if !out_queue.is_empty() {
        let (head, tail) = out_queue.as_slices();

        // If there's a gap in the buffer, just write the next available
        // contiguous bit, *unless* that bit is less than 1/3 of the data
        // available, in which case rearrange the buffer so the whole thing
        // is contiguous.
        if tail.len() >= head.len() * 2 {
            out_queue.make_contiguous();
        }

        let (head, _tail) = out_queue.as_slices();

        match socket.poll_write_ref(ctx, head) {
            Poll::Pending => (),
            Poll::Ready(Ok(size)) => {
                let _ = out_queue.drain(..size);
                if !out_queue.is_empty() {
                    ctx.waker().wake_by_ref();
                }
            }
            Poll::Ready(Err(err)) => {
                if err != fidl::Status::PEER_CLOSED {
                    error!(error:? = err; "FDomain socket write error");
                } else {
                    debug!("FDomain connection closed");
                }
                return Poll::Ready(());
            }
        }
    }
    Poll::Pending
}

/// Serves an FDomain connection over the given socket. The FDomain served has
/// the toolbox as its namespace.
pub async fn serve_fdomain_connection(
    rcs: Weak<remote_control::RemoteControlService>,
    socket: fuchsia_async::Socket,
) {
    log::debug!("Spawned new FDomain connection");

    let (toolbox_server_sender, toolbox_task) = serve_toolboxes(rcs);
    let mut toolbox_task = pin!(toolbox_task);

    let fdomain = FDomain::new(move || {
        let (client_end, server_end) = fidl::endpoints::create_endpoints();
        let _ = toolbox_server_sender.unbounded_send(server_end);

        Ok(client_end)
    });

    let mut codec = FDomainCodec::new(fdomain);

    let mut out_queue = VecDeque::new();
    let mut in_queue = VecDeque::new();

    futures::future::poll_fn(move |ctx| {
        let Poll::Pending = toolbox_task.as_mut().poll(ctx) else {
            unreachable!("Toolbox task should sleep forever but it returned!");
        };

        if let Poll::Ready(outgoing) = codec.poll_next_unpin(ctx) {
            ctx.waker().wake_by_ref();
            let Some(outgoing) = outgoing else {
                error!(
                    "FDomain hung up its outgoing message stream. \
                        This is not how we expect the library to behave!"
                );
                return Poll::Ready(());
            };

            match outgoing {
                Ok(outgoing) => {
                    let Ok(size): Result<u32, _> = outgoing.len().try_into() else {
                        error!("Tried to send too-large packet (size {})", outgoing.len());
                        return Poll::Ready(());
                    };
                    let size = size.to_le_bytes();
                    let offset = out_queue.len();
                    let new_len = out_queue.len() + size.len() + outgoing.len();
                    out_queue.resize(new_len, 0);

                    let (a, b) = out_queue.as_mut_slices();

                    let (a, b) = if a.len() >= offset {
                        (&mut a[offset..], b)
                    } else {
                        (&mut [][..], &mut b[offset - a.len()..])
                    };

                    let a_size_portion = std::cmp::min(a.len(), size.len());
                    a[..a_size_portion].copy_from_slice(&size[..a_size_portion]);
                    let a = &mut a[a_size_portion..];
                    let size = &size[a_size_portion..];

                    if a.is_empty() {
                        b[..size.len()].copy_from_slice(size);
                        b[size.len()..].copy_from_slice(outgoing.as_ref());
                    } else {
                        a.copy_from_slice(&outgoing[..a.len()]);
                        let outgoing = &outgoing[a.len()..];
                        b[..outgoing.len()].copy_from_slice(outgoing);
                    }
                }
                Err(err) => {
                    error!(err:?; "FDomain encountered an internal error");
                    return Poll::Ready(());
                }
            }
        }

        if poll_process_out_queue(&socket, &mut out_queue, ctx).is_ready() {
            return Poll::Ready(());
        }

        let new_data = (|| {
            let orig_len = in_queue.len();
            extend_buf(&mut in_queue);
            let extension = in_queue.len() - orig_len;

            let (_head, tail) = in_queue.as_slices();

            let use_head = if tail.len() > extension {
                false
            } else {
                if !tail.is_empty() {
                    in_queue.make_contiguous();
                }
                true
            };

            let (head, tail) = in_queue.as_mut_slices();

            let buf = if use_head {
                let len = head.len();
                &mut head[(len - extension)..]
            } else {
                let len = tail.len();
                &mut tail[(len - extension)..]
            };

            assert!(!buf.is_empty());
            let Poll::Ready(got) = socket.poll_read_ref(ctx, buf) else {
                in_queue.resize(orig_len, 0);
                return Ok(false);
            };
            let got = got?;
            if got == 0 {
                return Err(fidl::Status::PEER_CLOSED);
            }
            ctx.waker().wake_by_ref();
            in_queue.resize(orig_len + got, 0);

            Result::<bool, fidl::Status>::Ok(true)
        })();

        let mut new_data = match new_data {
            Ok(n) => n,
            Err(err) => {
                if err != fidl::Status::PEER_CLOSED {
                    error!(error:? = err; "FDomain socket read error")
                } else {
                    debug!("FDomain connection closed");
                }
                return Poll::Ready(());
            }
        };

        while new_data && in_queue.len() >= std::mem::size_of::<u32>() {
            if in_queue.as_slices().0.len() < std::mem::size_of::<u32>() {
                in_queue.make_contiguous();
            }

            let (head, _tail) = in_queue.as_slices();
            let len = u32::from_le_bytes(head[..4].try_into().unwrap());
            let len: usize = len.try_into().unwrap();

            if in_queue.len() < len + 4 {
                break;
            }

            if head.len() < len + 4 {
                in_queue.make_contiguous();
            }
            let (head, _) = in_queue.as_slices();
            let data = &head[4..][..len];

            if let Err(err) = codec.message(data) {
                error!(err:?; "FDomain could not interpret an incoming message");
                return Poll::Ready(());
            }

            in_queue.drain(..len + 4);
            new_data = !in_queue.is_empty();
        }

        Poll::Pending
    })
    .await;
}

/// Add padding to the end of a buffer to be used as space to read more data.
fn extend_buf(buf: &mut VecDeque<u8>) {
    const BLOCK_SIZE: usize = 4096;

    let size = buf.len();
    let target_size = (size + (BLOCK_SIZE - 1)) / BLOCK_SIZE * BLOCK_SIZE;
    let target_size =
        if target_size - size < BLOCK_SIZE / 2 { target_size + BLOCK_SIZE } else { target_size };

    buf.resize(target_size, 0);
}
