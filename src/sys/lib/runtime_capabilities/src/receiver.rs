// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::DirConnectorMessage;
use derivative::Derivative;
use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedReceiver};
use futures::lock::Mutex;
use std::sync::Arc;

/// Type that represents the receiving end of a [Connector]. Every [Connector] is coupled to
/// some [Receiver] to which connection requests to that [Connector] (or any of its clones) are
/// delivered.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct Receiver {
    /// `inner` uses an async mutex because it will be locked across an await point
    /// when asynchronously waiting for the next message.
    inner: Arc<Mutex<UnboundedReceiver<fidl::Channel>>>,
}

impl Clone for Receiver {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl Receiver {
    pub fn new(receiver: mpsc::UnboundedReceiver<fidl::Channel>) -> Self {
        Self { inner: Arc::new(Mutex::new(receiver)) }
    }

    /// Waits to receive a message, or return `None` if there are no more messages and all
    /// senders are dropped.
    pub async fn receive(&self) -> Option<fidl::Channel> {
        let mut receiver_guard = self.inner.lock().await;
        receiver_guard.next().await
    }
}

/// Type that represents the receiving end of a [DirConnector]. The [DirConnector] counterpart of
/// [Receiver]. Every [DirConnector] is coupled to some [Receiver] to which connection requests to
/// that [DirConnector] (or any of its clones) are delivered.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct DirReceiver {
    /// `inner` uses an async mutex because it will be locked across an await point
    /// when asynchronously waiting for the next message.
    inner: Arc<Mutex<UnboundedReceiver<DirConnectorMessage>>>,
}

impl Clone for DirReceiver {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl DirReceiver {
    pub fn new(receiver: mpsc::UnboundedReceiver<DirConnectorMessage>) -> Self {
        Self { inner: Arc::new(Mutex::new(receiver)) }
    }

    /// Waits to receive a message, or return `None` if there are no more messages and all
    /// senders are dropped.
    pub async fn receive(&self) -> Option<DirConnectorMessage> {
        let mut receiver_guard = self.inner.lock().await;
        receiver_guard.next().await
    }
}

// These tests do not run on host because the `wait_handle` function below is not supported in the
// handle emulation layer.
#[cfg(target_os = "fuchsia")]
#[cfg(test)]
mod tests {
    use crate::Connector;
    use assert_matches::assert_matches;
    use fidl_fuchsia_component_sandbox as fsandbox;
    use fuchsia_async as fasync;
    use futures::future::{self, Either};
    use std::pin::pin;
    use zx::{self as zx, Peered};

    use super::*;

    #[fuchsia::test]
    async fn send_and_receive() {
        let (receiver, sender) = Connector::new();

        let (ch1, ch2) = zx::Channel::create();
        sender.send(ch1).unwrap();

        let channel = receiver.receive().await.unwrap();

        // Check connectivity.
        channel.signal_peer(zx::Signals::empty(), zx::Signals::USER_1).unwrap();
        ch2.wait_one(zx::Signals::USER_1, zx::MonotonicInstant::INFINITE).unwrap();
    }

    #[fuchsia::test]
    async fn send_fail_when_receiver_dropped() {
        let (receiver, sender) = Connector::new();

        drop(receiver);

        let (ch1, _ch2) = zx::Channel::create();
        sender.send(ch1).unwrap_err();
    }

    #[test]
    fn receive_blocks_while_connector_alive() {
        let mut ex = fasync::TestExecutor::new();
        let (receiver, sender) = Connector::new();

        {
            let mut fut = std::pin::pin!(receiver.receive());
            assert!(ex.run_until_stalled(&mut fut).is_pending());
        }

        drop(sender);

        let mut fut = std::pin::pin!(receiver.receive());
        let output = ex.run_until_stalled(&mut fut);
        assert_matches!(output, std::task::Poll::Ready(None));
    }

    /// It should be possible to conclusively ensure that no more messages will arrive.
    #[fuchsia::test]
    async fn drain_receiver() {
        let (receiver, sender) = Connector::new();

        let (ch1, _ch2) = zx::Channel::create();
        sender.send(ch1).unwrap();

        // Even if all the senders are closed after sending a message, it should still be
        // possible to receive that message.
        drop(sender);

        // Receive the message.
        assert!(receiver.receive().await.is_some());

        // Receiving again will fail.
        assert!(receiver.receive().await.is_none());
    }

    #[fuchsia::test]
    async fn receiver_fidl() {
        let (receiver, sender) = Connector::new();

        let (ch1, ch2) = zx::Channel::create();
        sender.send(ch1).unwrap();

        let (receiver_proxy, mut receiver_stream) =
            fidl::endpoints::create_proxy_and_stream::<fsandbox::ReceiverMarker>();

        let handler_fut = receiver.handle_receiver(receiver_proxy);
        let receive_fut = receiver_stream.next();
        let Either::Right((message, _)) =
            future::select(pin!(handler_fut), pin!(receive_fut)).await
        else {
            panic!("Handler should not finish");
        };
        let message = message.unwrap().unwrap();
        match message {
            fsandbox::ReceiverRequest::Receive { channel, .. } => {
                // Check connectivity.
                channel.signal_peer(zx::Signals::empty(), zx::Signals::USER_1).unwrap();
                ch2.wait_one(zx::Signals::USER_1, zx::MonotonicInstant::INFINITE).unwrap();
            }
            _ => panic!("Unexpected message"),
        }
    }
}
