// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL protocol clients.

use core::future::Future;
use core::mem::ManuallyDrop;
use core::pin::Pin;
use core::ptr;
use core::task::{Context, Poll, ready};

use fidl_constants::EPITAPH_ORDINAL;
use fidl_next_codec::{
    AsDecoder as _, DecoderExt as _, Encode, EncodeError, EncoderExt, Wire, wire,
};
use fuchsia_loom::sync::{Arc, Mutex};
use pin_project::{pin_project, pinned_drop};

use crate::endpoints::connection::{Connection, SendFutureState};
use crate::endpoints::lockers::{LockerError, Lockers};
use crate::wire::{Epitaph, MessageHeader};
use crate::{
    Flexibility, FrameworkError, Message, NonBlockingTransport, ProtocolError, SendFuture,
    Transport,
};

const ORD_FRAMEWORK_ERR: u64 = 3;
const ERR_UNKNOWN_METHOD: i32 = FrameworkError::UnknownMethod as i32;

struct ClientInner<T: Transport> {
    connection: Connection<T>,
    responses: Mutex<Lockers<Message<T>>>,
}

impl<T: Transport> ClientInner<T> {
    fn new(shared: T::Shared) -> Self {
        Self { connection: Connection::new(shared), responses: Mutex::new(Lockers::new()) }
    }
}

/// A client endpoint.
pub struct Client<T: Transport> {
    inner: Arc<ClientInner<T>>,
}

impl<T: Transport> Drop for Client<T> {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 2 {
            // This was the last reference to the connection other than the one
            // in the dispatcher itself. Stop the connection.
            self.close();
        }
    }
}

impl<T: Transport> Client<T> {
    /// Closes the channel from the client end.
    pub fn close(&self) {
        self.inner.connection.stop();
    }

    /// Send a request.
    pub fn send_one_way<W>(
        &self,
        ordinal: u64,
        flexibility: Flexibility,
        request: impl Encode<W, T::SendBuffer>,
    ) -> Result<SendFuture<'_, T>, EncodeError>
    where
        W: Wire<Constraint = ()>,
    {
        Ok(SendFuture::from_raw_parts(
            &self.inner.connection,
            self.send_message_raw(0, ordinal, flexibility, request)?,
        ))
    }

    /// Send a request and await for a response.
    pub fn send_two_way<W>(
        &self,
        ordinal: u64,
        flexibility: Flexibility,
        request: impl Encode<W, T::SendBuffer>,
    ) -> Result<TwoWayRequestFuture<'_, T>, EncodeError>
    where
        W: Wire<Constraint = ()>,
    {
        let index = self.inner.responses.lock().unwrap().alloc(ordinal);

        // Send with txid = index + 1 because indices start at 0.
        match self.send_message_raw(index + 1, ordinal, flexibility, request) {
            Ok(state) => Ok(TwoWayRequestFuture { inner: &self.inner, index: Some(index), state }),
            Err(e) => {
                self.inner.responses.lock().unwrap().free(index);
                Err(e)
            }
        }
    }

    fn send_message_raw<W>(
        &self,
        txid: u32,
        ordinal: u64,
        flexibility: Flexibility,
        message: impl Encode<W, T::SendBuffer>,
    ) -> Result<SendFutureState<T>, EncodeError>
    where
        W: Wire<Constraint = ()>,
    {
        self.inner.connection.send_message_raw(|buffer| {
            buffer.encode_next(MessageHeader::new(txid, ordinal, flexibility))?;
            buffer.encode_next(message)
        })
    }
}

impl<T: Transport> Clone for Client<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

/// A future for a pending response to a two-way message.
pub struct TwoWayResponseFuture<'a, T: Transport> {
    inner: &'a ClientInner<T>,
    index: Option<u32>,
}

impl<T: Transport> Drop for TwoWayResponseFuture<'_, T> {
    fn drop(&mut self) {
        // If `index` is `Some`, then we still need to free our locker.
        if let Some(index) = self.index {
            let mut responses = self.inner.responses.lock().unwrap();
            if responses.get(index).unwrap().cancel() {
                responses.free(index);
            }
        }
    }
}

fn handle_flexible_response<T: Transport>(
    mut message: Message<T>,
) -> Result<Message<T>, ProtocolError<T::Error>> {
    if matches!(message.header().flexibility(), Flexibility::Flexible) {
        let mut decoder = message.as_decoder();
        let mut union = decoder
            .take_slot::<wire::Union>()
            .map_err(|_| ProtocolError::InvalidFlexibleResponse)?;
        let ordinal = wire::Union::encoded_ordinal(union.as_mut());
        if ordinal == ORD_FRAMEWORK_ERR {
            wire::Union::decode_as::<_, wire::Int32>(union.as_mut(), &mut decoder, ())
                .map_err(|_| ProtocolError::InvalidFlexibleResponse)?;
            let union = unsafe { union.deref_unchecked() };
            let error = unsafe { union.get().read_unchecked::<wire::Int32>() };
            match error.0 {
                ERR_UNKNOWN_METHOD => {
                    return Err(ProtocolError::FrameworkError(FrameworkError::UnknownMethod));
                }
                ordinal => return Err(ProtocolError::UnknownFrameworkError { ordinal }),
            }
        }
    }

    Ok(message)
}

impl<T: Transport> Future for TwoWayResponseFuture<'_, T> {
    type Output = Result<Message<T>, ProtocolError<T::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = Pin::into_inner(self);
        let Some(index) = this.index else {
            panic!("TwoWayResponseFuture polled after returning `Poll::Ready`");
        };

        let mut responses = this.inner.responses.lock().unwrap();
        let message = if let Some(message) = responses.get(index).unwrap().read(cx.waker()) {
            handle_flexible_response(message)
        } else if let Some(termination_reason) = this.inner.connection.get_termination_reason() {
            Err(termination_reason)
        } else {
            return Poll::Pending;
        };

        responses.free(index);
        this.index = None;

        Poll::Ready(message)
    }
}

/// A future for a sending a two-way FIDL message.
#[pin_project(PinnedDrop)]
pub struct TwoWayRequestFuture<'a, T: Transport> {
    inner: &'a ClientInner<T>,
    index: Option<u32>,
    #[pin]
    state: SendFutureState<T>,
}

#[pinned_drop]
impl<T: Transport> PinnedDrop for TwoWayRequestFuture<'_, T> {
    fn drop(self: Pin<&mut Self>) {
        if let Some(index) = self.index {
            let mut responses = self.inner.responses.lock().unwrap();

            // The future was canceled before it could be sent. The transaction
            // ID was never used, so it's safe to immediately reuse.
            responses.free(index);
        }
    }
}

impl<'a, T: Transport> Future for TwoWayRequestFuture<'a, T> {
    type Output = Result<TwoWayResponseFuture<'a, T>, ProtocolError<T::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let Some(index) = *this.index else {
            panic!("TwoWayRequestFuture polled after returning `Poll::Ready`");
        };

        let result = ready!(this.state.poll_send(cx, &this.inner.connection));
        *this.index = None;
        if let Err(error) = result {
            // The send failed. Free the locker and return an error.
            this.inner.responses.lock().unwrap().free(index);
            Poll::Ready(Err(error))
        } else {
            Poll::Ready(Ok(TwoWayResponseFuture { inner: this.inner, index: Some(index) }))
        }
    }
}

impl<'a, T: NonBlockingTransport> TwoWayRequestFuture<'a, T> {
    /// Completes the send operation synchronously and without blocking.
    ///
    /// Using this method prevents transports from applying backpressure. Prefer
    /// awaiting when possible to allow for backpressure.
    ///
    /// Because failed sends return immediately, `send_immediately` may observe
    /// transport closure prematurely. This can manifest as this method
    /// returning `Err(PeerClosed)` or `Err(Stopped)` when it should have
    /// returned `Err(PeerClosedWithEpitaph)`. Prefer awaiting when possible for
    /// correctness.
    pub fn send_immediately(self) -> Result<TwoWayResponseFuture<'a, T>, ProtocolError<T::Error>> {
        let inner = self.inner;
        let index = self.index;
        let state = unsafe { ptr::read(&ManuallyDrop::new(self).state) };
        if let Err(e) = state.send_immediately(&inner.connection) {
            inner.responses.lock().unwrap().free(index.unwrap());
            return Err(e);
        }

        Ok(TwoWayResponseFuture { inner, index })
    }
}

/// A type which handles incoming events for a local client.
///
/// This is a variant of [`ClientHandler`] that does not require implementing
/// `Send` and only supports local-thread executors.
pub trait LocalClientHandler<T: Transport> {
    /// Handles a received client event.
    ///
    /// See [`ClientHandler::on_event`] for more information.
    fn on_event(
        &mut self,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>>;
}

/// A type which handles incoming events for a client.
pub trait ClientHandler<T: Transport>: Send {
    /// Handles a received client event.
    ///
    /// The client cannot handle more messages until `on_event` completes. If
    /// `on_event` may block, or would perform asynchronous work that takes a
    /// long time, it should offload work to an async task and return.
    fn on_event(
        &mut self,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>> + Send;
}

/// An adapter for a [`ClientHandler`] which implements [`LocalClientHandler`].
#[repr(transparent)]
pub struct ClientHandlerToLocalAdapter<H>(H);

impl<T, H> LocalClientHandler<T> for ClientHandlerToLocalAdapter<H>
where
    T: Transport,
    H: ClientHandler<T>,
{
    #[inline]
    fn on_event(
        &mut self,
        message: Message<T>,
    ) -> impl Future<Output = Result<(), ProtocolError<T::Error>>> {
        self.0.on_event(message)
    }
}

/// A dispatcher for a client endpoint.
///
/// A client dispatcher receives all of the incoming messages and dispatches them to the client
/// handler and two-way futures. It acts as the message pump for the client.
///
/// The dispatcher must be actively polled to receive events and two-way message responses. If the
/// dispatcher is not [`run`](ClientDispatcher::run) concurrently, then events will not be received
/// and two-way message futures will not receive their responses.
pub struct ClientDispatcher<T: Transport> {
    inner: Arc<ClientInner<T>>,
    exclusive: T::Exclusive,
    is_terminated: bool,
}

impl<T: Transport> Drop for ClientDispatcher<T> {
    fn drop(&mut self) {
        if !self.is_terminated {
            // SAFETY: We checked that the connection has not been terminated.
            unsafe {
                self.terminate(ProtocolError::Stopped);
            }
        }
    }
}

impl<T: Transport> ClientDispatcher<T> {
    /// Creates a new client from a transport.
    pub fn new(transport: T) -> Self {
        let (shared, exclusive) = transport.split();
        Self { inner: Arc::new(ClientInner::new(shared)), exclusive, is_terminated: false }
    }

    /// # Safety
    ///
    /// The connection must not yet be terminated.
    unsafe fn terminate(&mut self, error: ProtocolError<T::Error>) {
        // SAFETY: We checked that the connection has not been terminated.
        unsafe {
            self.inner.connection.terminate(error);
        }
        self.inner.responses.lock().unwrap().wake_all();
    }

    /// Returns a client for the dispatcher.
    ///
    /// When the last `Client` is dropped, the dispatcher will be stopped.
    pub fn client(&self) -> Client<T> {
        Client { inner: self.inner.clone() }
    }

    /// Runs the client with the provided handler.
    pub async fn run<H>(self, handler: H) -> Result<H, ProtocolError<T::Error>>
    where
        H: ClientHandler<T>,
    {
        // The bounds on `H` prove that the future returned by `run_local` is
        // `Send`.
        self.run_local(ClientHandlerToLocalAdapter(handler)).await.map(|adapter| adapter.0)
    }

    /// Runs the client with the provided handler.
    pub async fn run_local<H>(mut self, mut handler: H) -> Result<H, ProtocolError<T::Error>>
    where
        H: LocalClientHandler<T>,
    {
        // We may assume that the connection has not been terminated because
        // connections are only terminated by `run` and `drop`. Neither of those
        // could have been called before this method because `run` consumes
        // `self` and `drop` is only ever called once.

        let error = loop {
            // SAFETY: The connection has not been terminated.
            let result = unsafe { self.run_one(&mut handler).await };
            if let Err(error) = result {
                break error;
            }
        };

        // SAFETY: The connection has not been terminated.
        unsafe {
            self.terminate(error.clone());
        }
        self.is_terminated = true;

        match error {
            // We consider clients to have finished successfully only if they
            // stop themselves manually.
            ProtocolError::Stopped => Ok(handler),

            // Otherwise, the client finished with an error.
            _ => Err(error),
        }
    }

    /// # Safety
    ///
    /// The connection must not be terminated.
    async unsafe fn run_one<H>(&mut self, handler: &mut H) -> Result<(), ProtocolError<T::Error>>
    where
        H: LocalClientHandler<T>,
    {
        // SAFETY: The caller guaranteed that the connection is not terminated.
        let buffer = unsafe { self.inner.connection.recv(&mut self.exclusive).await? };

        // This expression is really awkward due to a limitation in rustc's
        // liveness analysis for local variables. We need to avoid holding
        // `decoder` across `.await`s because it may not be `Send` and tasks may
        // migrate threads between polls. We should be able to just
        // `drop(decoder)` before any `.await`, but rustc is overly conservative
        // and still considers `decoder` as live at the `.await` for that
        // analysis. The only way to convince rustc that `decoder` is not live
        // at that await point is to keep the lexical scope containing `decoder`
        // free of `.await`s.
        //
        // See https://github.com/rust-lang/rust/issues/63768 for more details.
        let mut message =
            Message::<T>::decode(buffer).map_err(ProtocolError::InvalidMessageHeader)?;
        let txid = *message.header().txid;
        let ordinal = *message.header().ordinal;

        // Check if the ordinal is the epitaph so we can immediately decode and
        // return it.
        if ordinal == EPITAPH_ORDINAL {
            let mut decoder = message.as_decoder();
            let epitaph = decoder.decode::<Epitaph>().map_err(ProtocolError::InvalidEpitaphBody)?;
            return Err(ProtocolError::PeerClosedWithEpitaph(*epitaph.error));
        }

        if txid == 0 {
            handler.on_event(message).await?;
        } else {
            let mut responses = self.inner.responses.lock().unwrap();
            let locker = responses
                .get(txid - 1)
                .ok_or_else(|| ProtocolError::UnrequestedResponse { txid })?;

            match locker.write(ordinal, message) {
                // Reader didn't cancel
                Ok(false) => (),
                // Reader canceled, we can drop the entry
                Ok(true) => responses.free(txid - 1),
                Err(LockerError::NotWriteable) => {
                    return Err(ProtocolError::UnrequestedResponse { txid });
                }
                Err(LockerError::MismatchedOrdinal { expected, actual }) => {
                    return Err(ProtocolError::InvalidResponseOrdinal { expected, actual });
                }
            }
        }

        Ok(())
    }
}
