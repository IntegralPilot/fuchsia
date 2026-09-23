// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::hanging_get::client::HangingGetStream;
use fidl::endpoints::Proxy;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::{Future, StreamExt, ready};
use log::{trace, warn};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use std::task::{Context, Poll};

use super::{Connection, ConnectionBackendType};
use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use zx;

/// A client-side implementation of the Bluetooth channel transport using the FIDL protocol.
pub struct FidlClientConnection {
    proxy: fidl_bt::ChannelProxy,
    /// Hanging-get stream for receiving packets from the remote server end.
    receive_stream: HangingGetStream<fidl_bt::ChannelProxy, Vec<fidl_bt::Packet>>,
    /// The buffer for outgoing packets that are waiting to be sent.
    send_buffer: Arc<Mutex<VecDeque<Vec<u8>>>>,
    /// Waker to notify if the queue has space.
    waker: Arc<Mutex<Option<std::task::Waker>>>,
    /// The buffer for incoming packets that are waiting to be polled by the stream consumer.
    recv_buffer: VecDeque<Vec<u8>>,
    /// Scope for managing background flush tasks.
    flush_scope: fasync::Scope,
    /// Flag indicating if a background flush task is currently queued or running.
    flush_task_queued: Arc<AtomicBool>,
    /// Future that waits for all tasks in `flush_scope` to complete.
    flush_finished: Mutex<Option<BoxFuture<'static, ()>>>,
    /// Max TX packet size.
    max_tx_size: usize,
    /// Terminal error if the background flush task fails.
    terminal_error: Arc<Mutex<Option<zx::Status>>>,
}

impl std::fmt::Debug for FidlClientConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FidlClientConnection")
            .field("flush_task_queued", &self.flush_task_queued.load(Ordering::Relaxed))
            .field("send_buffer_len", &self.send_buffer.lock().len())
            .finish()
    }
}

impl FidlClientConnection {
    const SEND_BUFFER_SIZE: usize = 32;

    pub fn new(proxy: fidl_bt::ChannelProxy, max_tx_size: usize) -> Self {
        let receive_stream =
            HangingGetStream::new_with_fn_ptr(proxy.clone(), fidl_bt::ChannelProxy::receive);

        Self {
            proxy,
            receive_stream,
            send_buffer: Arc::new(Mutex::new(VecDeque::with_capacity(Self::SEND_BUFFER_SIZE))),
            waker: Arc::new(Mutex::new(None)),
            recv_buffer: VecDeque::new(),
            flush_scope: fasync::Scope::new(),
            flush_task_queued: Arc::new(AtomicBool::new(false)),
            flush_finished: Mutex::new(None),
            max_tx_size,
            terminal_error: Arc::new(Mutex::new(None)),
        }
    }

    fn collect_packets_for_batch(
        send_buffer: &Mutex<VecDeque<Vec<u8>>>,
        flush_task_queued: &AtomicBool,
    ) -> Option<Vec<fidl_bt::Packet>> {
        let mut buffer = send_buffer.lock();
        if buffer.is_empty() {
            flush_task_queued.store(false, Ordering::SeqCst);
            return None;
        }
        let mut batch = Vec::new();
        let mut batch_bytes = 0;
        while let Some(packet) = buffer.front() {
            if batch_bytes + packet.len() + super::PACKET_OVERHEAD > super::MAX_BATCH_SIZE_BYTES {
                if batch.is_empty() {
                    // If a single packet is larger than the limit, we still try to send it.
                    let item = buffer.pop_front().unwrap();
                    batch.push(fidl_bt::Packet { packet: item });
                }
                break;
            }
            let item = buffer.pop_front().unwrap();
            batch_bytes += item.len() + super::PACKET_OVERHEAD;
            batch.push(fidl_bt::Packet { packet: item });
        }
        Some(batch)
    }

    fn ensure_flush_task_running(&self) {
        if self.send_buffer.lock().is_empty() {
            return;
        }
        if self.flush_task_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        let send_buffer = self.send_buffer.clone();
        let proxy = self.proxy.clone();
        let flush_task_queued = self.flush_task_queued.clone();
        let terminal_error = self.terminal_error.clone();
        let waker = self.waker.clone();
        let _ = self.flush_scope.spawn(async move {
            loop {
                let Some(packets) =
                    Self::collect_packets_for_batch(&send_buffer, &flush_task_queued)
                else {
                    break;
                };
                if let Some(w) = waker.lock().take() {
                    w.wake();
                }
                trace!(
                    "FidlClientConnection: Sending batch of {} packets to FIDL server",
                    packets.len()
                );
                if let Err(e) = proxy.send_(&packets).await {
                    warn!(e:?; "FIDL Send error");
                    let status =
                        if e.is_closed() { zx::Status::PEER_CLOSED } else { zx::Status::INTERNAL };
                    *terminal_error.lock() = Some(status);
                    flush_task_queued.store(false, Ordering::SeqCst);
                    break;
                }
            }
        });
    }

    fn enqueue_packet(&self, packet: Vec<u8>) -> Result<(), zx::Status> {
        if packet.len() > self.max_tx_size {
            warn!("Packet size {} exceeds max_tx_size {}", packet.len(), self.max_tx_size);
            return Err(zx::Status::OUT_OF_RANGE);
        }
        let mut buffer = self.send_buffer.lock();
        if buffer.len() >= Self::SEND_BUFFER_SIZE {
            warn!(
                "FidlClientConnection: send buffer is full ({}/{})",
                buffer.len(),
                Self::SEND_BUFFER_SIZE
            );
            return Err(zx::Status::SHOULD_WAIT);
        }
        let len = packet.len();
        buffer.push_back(packet);
        trace!("FidlClientConnection: Enqueued packet of size {}", len);
        Ok(())
    }
}

impl Connection for FidlClientConnection {
    fn closed(&self) -> Pin<Box<dyn Future<Output = Result<(), zx::Status>> + Send + 'static>> {
        let proxy_cloned = self.proxy.clone();
        Box::pin(async move {
            let _ = proxy_cloned.on_closed().await;
            Ok(())
        })
    }

    fn connection_type(&self) -> ConnectionBackendType {
        ConnectionBackendType::FidlClient
    }

    fn write(&self, bytes: &[u8]) -> Result<usize, zx::Status> {
        self.enqueue_packet(bytes.to_vec())?;
        self.ensure_flush_task_running();
        Ok(bytes.len())
    }

    fn is_closed(&self) -> bool {
        self.proxy.is_closed()
    }

    fn into_fidl_channel(self: Box<Self>) -> Result<bredr::Channel, zx::Status> {
        let this = *self;
        // Drop any ongoing active hanging-get Receive request or Send request.
        drop(this.receive_stream);
        drop(this.flush_scope);

        let client_end = this.proxy.into_client_end().map_err(|_| zx::Status::UNAVAILABLE)?;
        Ok(bredr::Channel { connection: Some(client_end), ..Default::default() })
    }
}

impl Stream for FidlClientConnection {
    type Item = Result<Vec<u8>, zx::Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Return buffered items first
            if let Some(data) = this.recv_buffer.pop_front() {
                return Poll::Ready(Some(Ok(data)));
            }

            // No items in the buffer.
            let res = ready!(this.receive_stream.poll_next_unpin(cx));
            match res {
                Some(Ok(packets)) => {
                    trace!(
                        "FidlClientConnection: Received {} packets from FIDL server",
                        packets.len()
                    );
                    for packet in packets {
                        this.recv_buffer.push_back(packet.packet);
                    }
                    continue; // Loop again to return the first buffered item
                }
                Some(Err(e)) if e.is_closed() => {
                    trace!("FIDL channel is closed");
                    return Poll::Ready(None);
                }
                Some(Err(e)) => {
                    warn!("FIDL Receive error: {:?}", e);
                    return Poll::Ready(Some(Err(zx::Status::INTERNAL)));
                }
                None => {
                    trace!("FIDL channel terminated");
                    return Poll::Ready(None);
                }
            }
        }
    }
}

impl Sink<Vec<u8>> for FidlClientConnection {
    type Error = zx::Status;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if let Some(err) = *this.terminal_error.lock() {
            return Poll::Ready(Err(err));
        }
        // Limit queue size
        let buffer = this.send_buffer.lock();
        if buffer.len() >= Self::SEND_BUFFER_SIZE {
            *this.waker.lock() = Some(cx.waker().clone());
            return Poll::Pending;
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        let this = self.get_mut();
        if let Some(err) = *this.terminal_error.lock() {
            return Err(err);
        }
        this.enqueue_packet(item)?;
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        loop {
            if let Some(err) = *this.terminal_error.lock() {
                return Poll::Ready(Err(err));
            }

            this.ensure_flush_task_running();

            let mut flush_finished_guard = this.flush_finished.lock();
            let flush_finished = flush_finished_guard.get_or_insert_with(|| {
                let handle = this.flush_scope.to_handle();
                Box::pin(async move {
                    handle.on_no_tasks().await;
                })
            });

            ready!(flush_finished.as_mut().poll(cx));

            *flush_finished_guard = None;

            if let Some(err) = *this.terminal_error.lock() {
                return Poll::Ready(Err(err));
            }

            if this.send_buffer.lock().is_empty() {
                return Poll::Ready(Ok(()));
            }
        }
    }

    // TODO(https://fxbug.dev/414410187) Consider actually closing the underlying channel.
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Sink::poll_flush(self, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Channel;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::stream::FusedStream;
    use futures::{SinkExt, StreamExt};

    #[test]
    fn channel_sync_write() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let channel = Channel::from_fidl_client(proxy, Channel::DEFAULT_MAX_TX);

        let data = vec![1, 2, 3];

        // Fill up the send buffer (32 writes).
        for _ in 0..32 {
            let size = channel.write(&data).expect("sync write to succeed");
            assert_eq!(size, data.len());
        }

        // The 33rd write should fail with SHOULD_WAIT because the buffer is full.
        let result = channel.write(&data);
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT));

        // Check the stream (server end) for the request.
        let mut stream_fut = stream.next();
        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Some(Ok(fidl_bt::ChannelRequest::Send_ { packets, responder }))) => {
                assert_eq!(packets.len(), 32);
                assert_eq!(packets[0].packet, data);
                responder.send().expect("responder to send successfully");
            }
            x => panic!("Expected Send_ request, got {:?}", x),
        }

        // And now we should be able to write again because one packet was processed and acknowledged.
        let size = channel.write(&data).expect("sync write to succeed again");
        assert_eq!(size, data.len());
    }

    #[test]
    fn channel_into_fidl() {
        let _exec = fasync::TestExecutor::new();
        let (proxy, _stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let conn = FidlClientConnection::new(proxy, Channel::DEFAULT_MAX_TX);

        let fidl_channel =
            Box::new(conn).into_fidl_channel().expect("into_fidl_channel to succeed");
        println!("FIDL Channel: {:?}", fidl_channel);
        assert!(fidl_channel.connection.is_some());
        assert!(fidl_channel.socket.is_none());
    }

    #[test]
    fn channel_write_too_large() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, _stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_client(proxy, 10);

        // Test Connection::write
        let data = vec![1; 11];
        let result = channel.write(&data);
        assert_eq!(result, Err(zx::Status::OUT_OF_RANGE));

        // Test SinkExt::send
        let mut send_fut = channel.send(data);
        let result = exec.run_until_stalled(&mut send_fut);
        assert_eq!(result, Poll::Ready(Err(zx::Status::OUT_OF_RANGE)));
    }

    #[test]
    fn channel_closed() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let channel = Channel::from_fidl_client(proxy, Channel::DEFAULT_MAX_TX);

        let mut closed_fut = channel.closed();
        assert!(exec.run_until_stalled(&mut closed_fut).is_pending());
        assert!(!channel.is_closed());

        drop(stream);

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(exec.run_until_stalled(&mut closed_fut).is_ready());
        assert!(channel.is_closed());
    }

    #[fuchsia::test]
    fn channel_sink() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_client(proxy, Channel::DEFAULT_MAX_TX);

        let data = vec![1, 2, 3];
        let mut send_fut = channel.send(data.clone());
        assert!(exec.run_until_stalled(&mut send_fut).is_pending());

        // Now expect Send_ request
        let mut stream_fut = stream.next();
        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Some(Ok(fidl_bt::ChannelRequest::Send_ { packets, responder }))) => {
                assert_eq!(packets.len(), 1);
                assert_eq!(packets[0].packet, data);
                responder.send().expect("responder to send successfully");
            }
            x => panic!("Expected Send_ request, got {:?}", x),
        }

        assert!(exec.run_until_stalled(&mut send_fut).is_ready());

        // Verify start_send fails with SHOULD_WAIT when the buffer is full (32 writes).
        for _ in 0..32 {
            Pin::new(&mut channel).start_send(data.clone()).expect("start_send to succeed");
        }
        let result = Pin::new(&mut channel).start_send(data.clone());
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT));

        // Verify Sink batching: flushing the full buffer sends all 32 packets together.
        let mut flush_fut = channel.flush();
        assert!(exec.run_until_stalled(&mut flush_fut).is_pending());

        let mut stream_fut = stream.next();
        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Some(Ok(fidl_bt::ChannelRequest::Send_ { packets, responder }))) => {
                assert_eq!(packets.len(), 32);
                assert_eq!(packets[0].packet, data);
                responder.send().expect("responder to send successfully");
            }
            x => panic!("Expected Send_ request, got {:?}", x),
        }
        assert!(exec.run_until_stalled(&mut flush_fut).is_ready());
    }

    #[test]
    fn channel_stream() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, mut stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_client(proxy, Channel::DEFAULT_MAX_TX);

        let data = vec![4, 5, 6];

        // Trigger initial Receive request
        let mut next_fut = channel.next();
        assert!(exec.run_until_stalled(&mut next_fut).is_pending());

        let mut stream_fut = stream.next();

        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Some(Ok(fidl_bt::ChannelRequest::Receive { responder }))) => {
                let packets = vec![fidl_bt::Packet { packet: data.clone() }];
                responder.send(&packets).expect("responder to send packets successfully");
            }
            x => panic!("Expected Receive request, got {:?}", x),
        }

        match exec.run_until_stalled(&mut next_fut) {
            Poll::Ready(Some(Ok(received))) => {
                assert_eq!(received, data);
            }
            x => panic!("Expected data from stream, got {:?}", x),
        }

        // After the sender is dropped, the stream should terminate.
        drop(stream);

        let mut next_fut = channel.next();
        let Poll::Ready(None) = exec.run_until_stalled(&mut next_fut) else {
            panic!("Expected None from the stream")
        };

        // It should continue to report terminated.
        assert!(channel.is_terminated());
    }

    #[test]
    fn test_collect_packets_for_batch() {
        let buffer = Mutex::new(VecDeque::new());
        let flush_task_queued = AtomicBool::new(true);

        // When buffer is empty, collect_packets_for_batch should return None
        assert!(
            FidlClientConnection::collect_packets_for_batch(&buffer, &flush_task_queued).is_none()
        );
        assert!(!flush_task_queued.load(Ordering::Relaxed));

        // Reset flag
        flush_task_queued.store(true, Ordering::Relaxed);

        // When buffer has packets, it should return Some(packets)
        buffer.lock().push_back(vec![1, 2, 3]);
        buffer.lock().push_back(vec![4, 5, 6]);

        let packets = FidlClientConnection::collect_packets_for_batch(&buffer, &flush_task_queued)
            .expect("should return packets");
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].packet, vec![1, 2, 3]);
        assert_eq!(packets[1].packet, vec![4, 5, 6]);
        assert!(buffer.lock().is_empty());
        assert!(flush_task_queued.load(Ordering::Relaxed));

        // Check the max batch size logic.
        // MAX_BATCH_SIZE_BYTES = 60 * 1024 = 61440.
        // PACKET_OVERHEAD = 24.
        // Let's create two packets: one that is close to the limit, and one that exceeds the limit when combined.
        let p1 = vec![0; 60 * 1024 - 24]; // Exactly 61416 bytes packet. Total size with overhead = 61440.
        let p2 = vec![0; 10];
        buffer.lock().push_back(p1.clone());
        buffer.lock().push_back(p2.clone());

        let packets = FidlClientConnection::collect_packets_for_batch(&buffer, &flush_task_queued)
            .expect("should return packets");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].packet, p1);
        assert_eq!(buffer.lock().len(), 1);
        assert_eq!(buffer.lock().front().unwrap(), &p2);

        // When the first packet exceeds the limit by itself, it should still be popped and returned.
        let p_large = vec![0; 60 * 1024 + 10];
        buffer.lock().push_back(p_large.clone()); // buffer is [p2, p_large]

        let packets = FidlClientConnection::collect_packets_for_batch(&buffer, &flush_task_queued)
            .expect("should return packets");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].packet, p2);

        let packets = FidlClientConnection::collect_packets_for_batch(&buffer, &flush_task_queued)
            .expect("should return packets");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].packet, p_large);
        assert!(buffer.lock().is_empty());
    }
}
