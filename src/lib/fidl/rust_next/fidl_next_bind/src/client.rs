// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use core::marker::PhantomData;
use core::ops::Deref;

use fidl_next_protocol::{
    self as protocol, ClientHandler, LocalClientHandler, Message, ProtocolError, Transport,
};

use crate::{ClientEnd, HasConnectionHandles, HasTransport};

/// A strongly typed client.
#[repr(transparent)]
pub struct Client<P, T: Transport = <P as HasTransport>::Transport> {
    client: protocol::Client<T>,
    _protocol: PhantomData<P>,
}

unsafe impl<P, T> Send for Client<P, T>
where
    T: Transport,
    protocol::Client<T>: Send,
{
}

impl<P, T: Transport> Client<P, T> {
    /// Creates a new client handle from an untyped client handle.
    pub fn from_untyped(client: protocol::Client<T>) -> Self {
        Self { client, _protocol: PhantomData }
    }

    /// Closes the channel from the client end.
    pub fn close(&self) {
        self.client.close();
    }
}

impl<P, T: Transport> Clone for Client<P, T> {
    fn clone(&self) -> Self {
        Self { client: self.client.clone(), _protocol: PhantomData }
    }
}

impl<P: HasConnectionHandles<T>, T: Transport> Deref for Client<P, T> {
    type Target = P::Client;

    fn deref(&self) -> &Self::Target {
        // SAFETY: `P::Client` is a `#[repr(transparent)]` wrapper around
        // `Client<T>`.
        unsafe { &*(self as *const Self).cast::<P::Client>() }
    }
}

/// A protocol which dispatches incoming client messages to a local handler.
///
/// This is a variant of [`DispatchClientMessage`] that does not require
/// implementing `Send` and only supports local-thread executors.
pub trait DispatchLocalClientMessage<H, T: Transport>: Sized + 'static {
    /// Handles a received client event with the given handler.
    fn on_event(
        handler: &mut H,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>>;
}

/// A protocol which dispatches incoming client messages to a handler.
pub trait DispatchClientMessage<H: Send, T: Transport>: Sized + 'static {
    /// Handles a received client event with the given handler.
    fn on_event(
        handler: &mut H,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>> + Send;
}

/// An adapter for a client protocol handler.
pub struct ClientHandlerToProtocolAdapter<P, H> {
    handler: H,
    _protocol: PhantomData<P>,
}

unsafe impl<P, H> Send for ClientHandlerToProtocolAdapter<P, H> where H: Send {}

impl<P, H> ClientHandlerToProtocolAdapter<P, H> {
    /// Creates a new protocol client handler from a supported handler.
    pub fn from_untyped(handler: H) -> Self {
        Self { handler, _protocol: PhantomData }
    }
}

impl<P, H, T> LocalClientHandler<T> for ClientHandlerToProtocolAdapter<P, H>
where
    P: DispatchLocalClientMessage<H, T>,
    T: Transport,
{
    fn on_event(
        &mut self,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>> {
        P::on_event(&mut self.handler, message)
    }
}

impl<P, H, T> ClientHandler<T> for ClientHandlerToProtocolAdapter<P, H>
where
    P: DispatchClientMessage<H, T>,
    H: Send,
    T: Transport,
{
    fn on_event(
        &mut self,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>> + Send {
        P::on_event(&mut self.handler, message)
    }
}

/// A strongly typed client dispatcher.
pub struct ClientDispatcher<P, T: Transport = <P as HasTransport>::Transport> {
    dispatcher: protocol::ClientDispatcher<T>,
    _protocol: PhantomData<P>,
}

unsafe impl<P, T> Send for ClientDispatcher<P, T>
where
    T: Transport,
    protocol::Client<T>: Send,
{
}

impl<P, T: Transport> ClientDispatcher<P, T> {
    /// Creates a new client from a client end.
    pub fn new(client_end: ClientEnd<P, T>) -> Self {
        Self {
            dispatcher: protocol::ClientDispatcher::new(client_end.into_untyped()),
            _protocol: PhantomData,
        }
    }

    /// Returns the dispatcher's client.
    pub fn client(&self) -> Client<P, T> {
        Client::from_untyped(self.dispatcher.client())
    }

    /// Creates a new client from an untyped client.
    pub fn from_untyped(dispatcher: protocol::ClientDispatcher<T>) -> Self {
        Self { dispatcher, _protocol: PhantomData }
    }

    /// Runs the client with the provided handler.
    pub async fn run<H>(self, handler: H) -> Result<H, ProtocolError<T::Error>>
    where
        P: DispatchClientMessage<H, T>,
        H: Send,
    {
        self.dispatcher
            .run(ClientHandlerToProtocolAdapter { handler, _protocol: PhantomData::<P> })
            .await
            .map(|adapter| adapter.handler)
    }

    /// Runs the client locally with the provided handler.
    pub async fn run_local<H>(self, handler: H) -> Result<H, ProtocolError<T::Error>>
    where
        P: DispatchLocalClientMessage<H, T>,
    {
        self.dispatcher
            .run_local(ClientHandlerToProtocolAdapter { handler, _protocol: PhantomData::<P> })
            .await
            .map(|adapter| adapter.handler)
    }

    /// Runs the client, ignoring any incoming events.
    pub async fn run_client(self) -> Result<(), ProtocolError<T::Error>>
    where
        P: DispatchClientMessage<IgnoreEvents, T>,
    {
        self.run(IgnoreEvents).await.map(|_| ())
    }
}

/// A handler which ignores incoming events.
pub struct IgnoreEvents;
