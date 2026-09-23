// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::stream::FuturesUnordered;
use futures::{StreamExt as _, select_biased};
use thiserror::Error;

use fidl_next_bind::{
    DispatchLocalServerMessage, DispatchServerMessage, HasTransport, ServerEnd,
    ServerHandlerToProtocolAdapter,
};
use fidl_next_protocol::{
    LocalServerHandler, Message, ProtocolError, Responder, ServerDispatcher,
    ServerHandlerToLocalAdapter, Transport,
};

/// An error that can occur while using a multiserver.
#[derive(Debug, Error)]
pub enum MultiserverError {
    /// Failed to forward a transport to the multiserver dispatcher.
    #[error("failed to forward transport to multiserver dispatcher")]
    ForwardError,
}

/// A server that handles incoming messages from multiple transports.
///
/// Multiservers are useful when you want to use a single server handler to
/// handle messages from multiple served transports. They coalesce all of the
/// incoming requests into a single conceptual stream, and invoke the handler on
/// each one serially.
///
/// [`Multiserver`] is a handle that can forward server ends to its
/// [`MultiserverDispatcher`]. The dispatcher is what handles each of the
/// incoming messages. Call [`multiserver`] to create a handle and dispatcher
/// pair.
///
/// # Example
///
/// ```ignore
/// let (server, dispatcher) = multiserver();
///
/// spawn(dispatcher.run(my_handler));
///
/// while let Some(server_end) = incoming.next().await {
///     server.forward(server_end);
/// }
/// ```
pub struct Multiserver<P, T: Transport = <P as HasTransport>::Transport> {
    sender: mpsc::UnboundedSender<ServerEnd<P, T>>,
    _protocol: PhantomData<P>,
}

impl<P, T: Transport> Clone for Multiserver<P, T> {
    fn clone(&self) -> Self {
        Self { sender: self.sender.clone(), _protocol: PhantomData }
    }
}

impl<P, T: Transport> Multiserver<P, T> {
    /// Forwards a new transport to the multiserver dispatcher.
    pub fn forward(&self, server_end: ServerEnd<P, T>) -> Result<(), MultiserverError> {
        self.sender.unbounded_send(server_end).map_err(|_| MultiserverError::ForwardError)
    }
}

struct MutexServerHandler<'a, H> {
    handler: &'a Mutex<H>,
}

impl<H, T: Transport> LocalServerHandler<T> for MutexServerHandler<'_, H>
where
    H: LocalServerHandler<T>,
{
    async fn on_one_way(&mut self, message: Message<T>) -> Result<(), ProtocolError<T::Error>> {
        self.handler.lock().await.on_one_way(message).await
    }

    async fn on_two_way(
        &mut self,
        message: Message<T>,
        responder: Responder<T>,
    ) -> Result<(), ProtocolError<T::Error>> {
        self.handler.lock().await.on_two_way(message, responder).await
    }
}

/// A dispatcher for a multiserver.
///
/// A multiserver runs servers for multiple transmports using the same handler.
/// See [`Multiserver`] for usage details.
pub struct MultiserverDispatcher<P, T: Transport = <P as HasTransport>::Transport> {
    receiver: mpsc::UnboundedReceiver<ServerEnd<P, T>>,
    _protocol: PhantomData<P>,
}

/// Creates a new multiserver and dispatcher pair.
///
/// See [`Multiserver`] for usage details.
pub fn multiserver<P, T: Transport>() -> (Multiserver<P, T>, MultiserverDispatcher<P, T>) {
    let (sender, receiver) = mpsc::unbounded();
    (
        Multiserver { sender, _protocol: PhantomData },
        MultiserverDispatcher { receiver, _protocol: PhantomData },
    )
}

impl<P, T: Transport> MultiserverDispatcher<P, T> {
    /// Runs the multiserver with the provided handler.
    ///
    /// The handler will be called with messages from multiple transports.
    pub async fn run<H>(self, handler: H)
    where
        P: DispatchServerMessage<H, T>,
        H: Send,
    {
        self.run_inner(ServerHandlerToLocalAdapter(
            ServerHandlerToProtocolAdapter::<P, H>::from_untyped(handler),
        ))
        .await
    }

    /// Runs the multiserver with the provided local handler.
    ///
    /// The handler will be called with messages from multiple transports.
    pub async fn run_local<H>(self, handler: H)
    where
        P: DispatchLocalServerMessage<H, T>,
    {
        self.run_inner(ServerHandlerToProtocolAdapter::<P, H>::from_untyped(handler)).await
    }

    async fn run_inner<H>(mut self, handler: H)
    where
        H: LocalServerHandler<T>,
    {
        let handler = Mutex::new(handler);
        let mut futures = FuturesUnordered::new();

        let mut is_closed = false;
        loop {
            select_biased! {
                transport = self.receiver.next() => {
                    if let Some(transport) = transport {
                        let dispatcher = ServerDispatcher::new(transport.into_untyped());
                        futures.push(dispatcher.run_local(MutexServerHandler {
                            handler: &handler,
                        }));
                    } else {
                        is_closed = true;
                        if futures.is_empty() {
                            break;
                        }
                    }
                }
                output = futures.next() => {
                    if output.is_none() && is_closed {
                        break;
                    }
                }
            }
        }
    }
}
