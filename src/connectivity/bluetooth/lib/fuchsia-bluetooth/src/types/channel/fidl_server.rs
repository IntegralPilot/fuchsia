// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::Responder;
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::future::{BoxFuture, FusedFuture};
use futures::sink::Sink;
use futures::stream::Stream;
use futures::{Future, FutureExt, SinkExt, StreamExt, ready};
use log::{trace, warn};

use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_async as fasync;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use zx;

use super::{Connection, ConnectionBackendType};

struct FlushState {
    outstanding_packets: std::sync::atomic::AtomicUsize,
    waker: Mutex<Option<std::task::Waker>>,
}

/// A server-side implementation of the Bluetooth channel transport using the FIDL protocol.
pub struct FidlServerConnection {
    /// Shared state for tracking flush status between writer and background task.
    flush_state: Arc<FlushState>,
    /// Stores the first terminal error encountered by background tasks.
    terminal_error: Arc<OnceLock<zx::Status>>,
    /// Channel to send data to the background task (outgoing data).
    send_tx: Mutex<mpsc::Sender<Vec<u8>>>,
    /// Channel to receive data from the background task (incoming data).
    recv_rx: mpsc::Receiver<Vec<u8>>,
    /// The background task that handles FIDL requests.
    _task: fasync::Task<()>,
    /// True if the connection is closed.
    is_closed: Arc<AtomicBool>,
    /// Future that completes when the connection is closed.
    close_fut: futures::future::Shared<BoxFuture<'static, Result<(), zx::Status>>>,
    /// Max TX packet size.
    max_tx_size: usize,
}

impl std::fmt::Debug for FidlServerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FidlServerConnection")
            .field("is_closed", &self.is_closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl FidlServerConnection {
    pub const SEND_BUFFER_SIZE: usize = 32;

    pub fn new(request_stream: fidl_bt::ChannelRequestStream, max_tx_size: usize) -> Self {
        // We use a channel of size SEND_BUFFER_SIZE to match client-side capacity and support back-to-back writes.
        let (send_tx, send_rx) = mpsc::channel(Self::SEND_BUFFER_SIZE - 1);
        let (recv_tx, recv_rx) = mpsc::channel(Self::SEND_BUFFER_SIZE);
        let is_closed = Arc::new(AtomicBool::new(false));
        let (close_tx, close_rx) = oneshot::channel();
        let flush_state = Arc::new(FlushState {
            outstanding_packets: std::sync::atomic::AtomicUsize::new(0),
            waker: Mutex::new(None),
        });

        let task = fasync::Task::spawn(Self::background_task(
            request_stream,
            send_rx,
            recv_tx,
            is_closed.clone(),
            close_tx,
            flush_state.clone(),
        ));

        let close_fut = async move {
            let _ = close_rx.await;
            Ok(())
        }
        .boxed()
        .shared();

        Self {
            flush_state,
            terminal_error: Arc::new(OnceLock::new()),
            send_tx: Mutex::new(send_tx),
            recv_rx,
            _task: task,
            is_closed,
            close_fut,
            max_tx_size,
        }
    }

    fn send_packet(&self, packet: Vec<u8>) -> Result<(), zx::Status> {
        trace!("FidlServerConnection: Enqueuing outgoing packet of size {}", packet.len());
        if packet.len() > self.max_tx_size {
            return Err(zx::Status::OUT_OF_RANGE);
        }
        let mut send_tx = self.send_tx.lock();
        let _ = self.flush_state.outstanding_packets.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = send_tx.try_send(packet) {
            let _ = self.flush_state.outstanding_packets.fetch_sub(1, Ordering::Relaxed);
            if err.is_full() {
                return Err(zx::Status::SHOULD_WAIT);
            } else {
                return Err(zx::Status::PEER_CLOSED);
            }
        }
        Ok(())
    }

    fn batch_packets(send_queue: &mut VecDeque<Vec<u8>>) -> Vec<fidl_bt::Packet> {
        let mut batch = Vec::new();
        let mut batch_bytes = 0;

        while let Some(packet) = send_queue.front() {
            if batch_bytes + packet.len() + super::PACKET_OVERHEAD > super::MAX_BATCH_SIZE_BYTES {
                if batch.is_empty() {
                    // If a single packet is larger than the limit, we still try to send it.
                    let packet = send_queue.pop_front().unwrap();
                    batch.push(fidl_bt::Packet { packet });
                }
                break;
            }
            let packet = send_queue.pop_front().unwrap();
            batch_bytes += packet.len() + super::PACKET_OVERHEAD;
            batch.push(fidl_bt::Packet { packet });
        }
        batch
    }

    async fn background_task(
        mut stream: fidl_bt::ChannelRequestStream,
        mut send_rx: mpsc::Receiver<Vec<u8>>,
        recv_tx: mpsc::Sender<Vec<u8>>,
        is_closed: Arc<AtomicBool>,
        close_tx: oneshot::Sender<()>,
        flush_state: Arc<FlushState>,
    ) {
        let mut pending_receive: Option<fidl_bt::ChannelReceiveResponder> = None;
        let mut send_queue = VecDeque::<Vec<u8>>::new();

        // Queue of Send requests from the FIDL client.
        let mut incoming_data =
            VecDeque::<(VecDeque<Vec<u8>>, fidl_bt::ChannelSend_Responder)>::new();
        // Future for forwarding packets from Send FIDL request to recv_tx.
        let mut incoming_data_forward_fut =
            futures::future::Fuse::<BoxFuture<'static, Result<(), mpsc::SendError>>>::terminated();

        trace!("FidlServerConnection: background task started");

        loop {
            // If we are not currently forwarding a packet, check if we have pending work.
            if incoming_data_forward_fut.is_terminated() {
                if let Some((packets, _)) = incoming_data.front_mut() {
                    // Use let-else to handle the case where the current request is empty.
                    let Some(packet) = packets.pop_front() else {
                        // All packets for this write request are forwarded. Acknowledge it.
                        if let Some((_, responder)) = incoming_data.pop_front() {
                            let _ = responder.send();
                        }
                        continue; // Restart loop to immediately process the next request
                    };

                    // Start forwarding this packet. We clone the sender to avoid borrow checker issues.
                    let mut tx = recv_tx.clone();
                    incoming_data_forward_fut =
                        (async move { tx.send(packet).await }).boxed().fuse();
                }
            }

            // Only pull more packets from the channel if the internal queue is not full.
            // If the queue is full, we yield pending to apply backpressure to the senders.
            let mut outgoing_fut = if send_queue.len() < Self::SEND_BUFFER_SIZE {
                send_rx.next().left_future()
            } else {
                futures::future::pending().right_future()
            };

            futures::select! {
                forward_res = incoming_data_forward_fut => {
                    if let Err(e) = forward_res {
                        warn!(e:?; "FidlServerConnection: Failed to forward packet to bt-rfcomm");
                        break;
                    }
                }
                request = stream.next() => {
                    let Some(item) = request else {
                        warn!("FidlServerConnection: FIDL request stream closed");
                        break;
                    };
                    let Ok(request) = item else {
                        warn!("FIDL request stream error: {:?}", item.unwrap_err());
                        break;
                    };
                    match request {
                        fidl_bt::ChannelRequest::Send_ { packets, responder } => {
                            trace!("FidlServerConnection: Received Send_ request with {} packets", packets.len());
                            let packet_data: Vec<Vec<u8>> = packets.into_iter().map(|p| p.packet).collect();
                            incoming_data.push_back((VecDeque::from(packet_data), responder));
                        }
                        fidl_bt::ChannelRequest::Receive { responder } => {
                            trace!("FidlServerConnection: Received Receive request, send_queue len: {}", send_queue.len());
                            if !send_queue.is_empty() {
                                let packets = Self::batch_packets(&mut send_queue);
                                let _ = responder.send(&packets);
                            } else {
                                if let Some(_old) = pending_receive.replace(responder) {
                                    warn!("Multiple outstanding Receive requests are not allowed!");
                                    break;
                                }
                            }
                        }
                        fidl_bt::ChannelRequest::WatchChannelParameters { responder } => {
                            warn!("FidlServerConnection: got WatchChannelParameters request, which is currently not handled");
                            responder.drop_without_shutdown();
                        }
                        other => {
                            warn!("Unknown FIDL method received: {other:?}");
                        }
                    }
                }
                outgoing = outgoing_fut => {
                    let Some(data) = outgoing else {
                        warn!("FidlServerConnection: send_rx closed");
                        break;
                    };
                    send_queue.push_back(data);
                    let mut processed = 1;
                    while send_queue.len() < Self::SEND_BUFFER_SIZE {
                        match send_rx.try_next() {
                            Ok(Some(packet)) => {
                                send_queue.push_back(packet);
                                processed += 1;
                            }
                            _ => break,
                        }
                    }
                    let outstanding = flush_state.outstanding_packets.fetch_sub(processed, Ordering::Relaxed) - processed;
                    if outstanding == 0 {
                        if let Some(waker) = flush_state.waker.lock().take() {
                            waker.wake();
                        }
                    }
                    if let Some(resp) = pending_receive.take() {
                        let packets = Self::batch_packets(&mut send_queue);
                        let _ = resp.send(&packets);
                    }
                }
            }
        }
        trace!("FidlServerConnection: background task exiting");
        is_closed.store(true, Ordering::Relaxed);
        let _ = close_tx.send(());
    }
}

impl Connection for FidlServerConnection {
    fn closed(&self) -> Pin<Box<dyn Future<Output = Result<(), zx::Status>> + Send + 'static>> {
        Box::pin(self.close_fut.clone())
    }

    fn connection_type(&self) -> ConnectionBackendType {
        ConnectionBackendType::FidlServer
    }

    fn write(&self, bytes: &[u8]) -> Result<usize, zx::Status> {
        if let Some(err) = self.terminal_error.get() {
            return Err(*err);
        }
        self.send_packet(bytes.to_vec())?;
        Ok(bytes.len())
    }

    fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Relaxed)
    }

    fn into_fidl_channel(self: Box<Self>) -> Result<bredr::Channel, zx::Status> {
        Err(zx::Status::NOT_SUPPORTED)
    }
}

impl Stream for FidlServerConnection {
    type Item = Result<Vec<u8>, zx::Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.recv_rx.poll_next_unpin(cx).map(|opt| opt.map(Ok))
    }
}

impl Sink<Vec<u8>> for FidlServerConnection {
    type Error = zx::Status;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if let Some(err) = this.terminal_error.get() {
            return Poll::Ready(Err(*err));
        }
        let mut send_tx = this.send_tx.lock();
        Pin::new(&mut *send_tx).poll_ready(cx).map_err(|_| zx::Status::PEER_CLOSED)
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        if let Some(err) = self.terminal_error.get() {
            return Err(*err);
        }
        self.send_packet(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if let Some(err) = this.terminal_error.get() {
            return Poll::Ready(Err(*err));
        }

        if this.flush_state.outstanding_packets.load(Ordering::Relaxed) == 0 {
            return Poll::Ready(Ok(()));
        }

        *this.flush_state.waker.lock() = Some(cx.waker().clone());

        if this.flush_state.outstanding_packets.load(Ordering::Relaxed) == 0 {
            let _ = this.flush_state.waker.lock().take();
            return Poll::Ready(Ok(()));
        }

        Poll::Pending
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        ready!(Sink::poll_flush(self.as_mut(), cx))?;
        let this = self.get_mut();
        this.send_tx.lock().close_channel();
        Poll::Ready(Ok(()))
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
    use std::pin::pin;

    #[test]
    fn channel_sync_write() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let channel = Channel::from_fidl_server(stream, Channel::DEFAULT_MAX_TX);

        let data = vec![1, 2, 3];

        // Fill up the send buffer (32 writes).
        for _ in 0..32 {
            let size = channel.write(&data).expect("sync write to succeed");
            assert_eq!(size, data.len());
        }

        // The 33rd write should fail with SHOULD_WAIT because the buffer is full.
        let result = channel.write(&data);
        assert_eq!(result, Err(zx::Status::SHOULD_WAIT));

        // Check the stream (client end) for the request.
        let stream_fut = proxy.receive();
        let mut stream_fut = pin!(stream_fut);

        // Run executor to let background task process the send.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        match exec.run_until_stalled(&mut stream_fut) {
            Poll::Ready(Ok(packets)) => {
                assert_eq!(packets.len(), 32);
                for packet in packets {
                    assert_eq!(packet.packet, data);
                }
            }
            x => panic!("Expected packets from Receive, got {:?}", x),
        }

        // And now we should be able to write again because one packet was processed and acknowledged.
        let size = channel.write(&data).expect("sync write to succeed again");
        assert_eq!(size, data.len());
    }

    #[test]
    fn channel_write_too_large() {
        let mut exec = fasync::TestExecutor::new();
        let (_proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_server(stream, 10);

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
    fn channel_into_fidl() {
        let _exec = fasync::TestExecutor::new();
        let (_proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let conn = FidlServerConnection::new(stream, Channel::DEFAULT_MAX_TX);

        let result = Box::new(conn).into_fidl_channel();
        assert_eq!(result.unwrap_err(), zx::Status::NOT_SUPPORTED);
    }

    #[test]
    fn channel_closed() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let channel = Channel::from_fidl_server(stream, Channel::DEFAULT_MAX_TX);

        let mut closed_fut = channel.closed();
        assert!(exec.run_until_stalled(&mut closed_fut).is_pending());
        assert!(!channel.is_closed());

        drop(proxy); // Drop the client proxy, closing the channel.

        // Let the background task run to detect the closure.
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        assert!(exec.run_until_stalled(&mut closed_fut).is_ready());
        assert!(channel.is_closed());
    }

    #[test]
    fn channel_sink() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_server(stream, Channel::DEFAULT_MAX_TX);

        let data = vec![1, 2, 3];

        // Simulate client calling Receive.
        let receive_fut = proxy.receive();
        let mut receive_fut = pin!(receive_fut);

        // Initially it should be pending because no data has been sent.
        assert!(exec.run_until_stalled(&mut receive_fut).is_pending());

        // Send data from server side.
        let mut send_fut = channel.send(data.clone());

        // It should be ready immediately because TestExecutor drives tasks to completion.
        assert!(exec.run_until_stalled(&mut send_fut).is_ready());

        // And the receive should also be ready with the data!
        match exec.run_until_stalled(&mut receive_fut) {
            Poll::Ready(Ok(packets)) => {
                assert_eq!(packets.len(), 1);
                assert_eq!(packets[0].packet, data);
            }
            x => panic!("Expected packets from Receive, got {:?}", x),
        }

        // Force proxy to stay alive until the end.
        let _ = proxy;
    }

    #[test]
    fn channel_stream() {
        let mut exec = fasync::TestExecutor::new();
        let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
        let mut channel = Channel::from_fidl_server(stream, Channel::DEFAULT_MAX_TX);

        let data = vec![4, 5, 6];

        // Simulate client calling Send_
        let send_fut = proxy.send_(&[fidl_bt::Packet { packet: data.clone() }]);
        let mut send_fut = pin!(send_fut);

        // Let the background task run
        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        assert!(exec.run_until_stalled(&mut send_fut).is_ready());

        let mut next_fut = channel.next();
        match exec.run_until_stalled(&mut next_fut) {
            Poll::Ready(Some(Ok(received))) => {
                assert_eq!(received, data);
            }
            x => panic!("Expected data from stream, got {:?}", x),
        }

        // After the client is dropped, the stream should terminate.
        drop(proxy);

        let _ = exec.run_until_stalled(&mut futures::future::pending::<()>());

        let mut next_fut = channel.next();
        let Poll::Ready(None) = exec.run_until_stalled(&mut next_fut) else {
            panic!("Expected None from the stream")
        };

        assert!(channel.is_terminated());
    }
}
