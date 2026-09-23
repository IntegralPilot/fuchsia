// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An implementation of a client for a fidl interface.

use crate::Error;
use crate::encoding::{
    Decode, Decoder, DefaultFuchsiaResourceDialect, DynamicFlags, Encode, Encoder, EpitaphBody,
    MessageBufFor, ProxyChannelBox, ProxyChannelFor, ResourceDialect, TransactionHeader,
    TransactionMessage, TransactionMessageType, TypeMarker, decode_transaction_header,
};
use fuchsia_sync::Mutex;
use futures::future::{self, FusedFuture, Future, FutureExt, Map, MaybeDone};
use futures::ready;
use futures::stream::{FusedStream, Stream};
use futures::task::{Context, Poll, Waker};
use slab::Slab;
use std::collections::VecDeque;
use std::mem;
use std::ops::ControlFlow;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable};

/// Decodes the body of `buf` as the FIDL type `T`.
#[doc(hidden)] // only exported for use in macros or generated code
pub fn decode_transaction_body<T: TypeMarker, D: ResourceDialect, const EXPECTED_ORDINAL: u64>(
    mut buf: D::MessageBufEtc,
) -> Result<T::Owned, Error>
where
    T::Owned: Decode<T, D>,
{
    let (bytes, handles) = buf.split_mut();
    let (header, body_bytes) = decode_transaction_header(bytes)?;
    if header.ordinal != EXPECTED_ORDINAL {
        return Err(Error::InvalidResponseOrdinal);
    }
    let mut output = Decode::<T, D>::new_empty();
    Decoder::<D>::decode_into::<T>(&header, body_bytes, handles, &mut output)?;
    Ok(output)
}

/// A FIDL client which can be used to send buffers and receive responses via a channel.
#[derive(Debug, Clone)]
pub struct Client<D: ResourceDialect = DefaultFuchsiaResourceDialect> {
    inner: Arc<ClientInner<D>>,
}

/// A future representing the decoded and transformed response to a FIDL query.
pub type DecodedQueryResponseFut<T, D = DefaultFuchsiaResourceDialect> = Map<
    MessageResponse<D>,
    fn(Result<<D as ResourceDialect>::MessageBufEtc, Error>) -> Result<T, Error>,
>;

/// A future representing the result of a FIDL query, with early error detection available if the
/// message couldn't be sent.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct QueryResponseFut<T, D: ResourceDialect = DefaultFuchsiaResourceDialect>(
    pub MaybeDone<DecodedQueryResponseFut<T, D>>,
);

impl<T: Unpin, D: ResourceDialect> FusedFuture for QueryResponseFut<T, D> {
    fn is_terminated(&self) -> bool {
        matches!(self.0, MaybeDone::Gone)
    }
}

impl<T: Unpin, D: ResourceDialect> Future for QueryResponseFut<T, D> {
    type Output = Result<T, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        ready!(self.0.poll_unpin(cx));
        let maybe_done = Pin::new(&mut self.0);
        Poll::Ready(maybe_done.take_output().unwrap_or(Err(Error::PollAfterCompletion)))
    }
}

impl<T> QueryResponseFut<T> {
    /// Check to see if the query has an error. If there was en error sending, this returns it and
    /// the error is returned, otherwise it returns self, which can then be awaited on:
    /// i.e. match echo_proxy.echo("something").check() {
    ///      Err(e) => error!("Couldn't send: {}", e),
    ///      Ok(fut) => fut.await
    /// }
    pub fn check(self) -> Result<Self, Error> {
        match self.0 {
            MaybeDone::Done(Err(e)) => Err(e),
            x => Ok(QueryResponseFut(x)),
        }
    }
}

const TXID_INTEREST_MASK: u32 = 0xFFFFFF;
const TXID_GENERATION_SHIFT: usize = 24;
const TXID_GENERATION_MASK: u8 = 0x7F;

/// A FIDL transaction id. Will not be zero for a message that includes a response.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Txid(u32);
/// A message interest id.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct InterestId(usize);

impl InterestId {
    fn from_txid(txid: Txid) -> Self {
        InterestId((txid.0 & TXID_INTEREST_MASK) as usize - 1)
    }
}

impl Txid {
    fn from_interest_id(int_id: InterestId, generation: u8) -> Self {
        // Base the transaction id on the slab slot + 1
        // (slab slots are zero-based and txid zero is special)
        let id = (int_id.0 as u32 + 1) & TXID_INTEREST_MASK;
        // And a 7-bit generation number.
        let generation = (generation & TXID_GENERATION_MASK) as u32;

        // Combine them:
        //  - top bit zero to indicate a userspace generated txid.
        //  - 7 bits of generation
        //  - 24 bits based on the interest id
        let txid = (generation << TXID_GENERATION_SHIFT) | id;

        Txid(txid)
    }

    /// Get the raw u32 transaction ID.
    pub fn as_raw_id(&self) -> u32 {
        self.0
    }
}

impl From<u32> for Txid {
    fn from(txid: u32) -> Self {
        Self(txid)
    }
}

impl<D: ResourceDialect> Client<D> {
    /// Create a new client.
    ///
    /// `channel` is the asynchronous channel over which data is sent and received.
    /// `event_ordinals` are the ordinals on which events will be received.
    pub fn new(channel: D::ProxyChannel, protocol_name: &'static str) -> Client<D> {
        Client {
            inner: Arc::new(ClientInner {
                channel: channel.boxed(),
                interests: Mutex::default(),
                terminal_error: Mutex::default(),
                protocol_name,
            }),
        }
    }

    /// Get a reference to the client's underlying channel.
    pub fn as_channel(&self) -> &D::ProxyChannel {
        self.inner.channel.as_channel()
    }

    /// Attempt to convert the `Client` back into a channel.
    ///
    /// This will only succeed if there are no active clones of this `Client`,
    /// no currently-alive `EventReceiver` or `MessageResponse`s that came from
    /// this `Client`, and no outstanding messages awaiting a response, even if
    /// that response will be discarded.
    pub fn into_channel(self) -> Result<D::ProxyChannel, Self> {
        // We need to check the message_interests table to make sure there are no outstanding
        // interests, since an interest might still exist even if all EventReceivers and
        // MessageResponses have been dropped. That would lead to returning an AsyncChannel which
        // could then later receive the outstanding response unexpectedly.
        //
        // We do try_unwrap before checking the message_interests to avoid a race where another
        // thread inserts a new value into message_interests after we check
        // message_interests.is_empty(), but before we get to try_unwrap. This forces us to create a
        // new Arc if message_interests isn't empty, since try_unwrap destroys the original Arc.
        match Arc::try_unwrap(self.inner) {
            Ok(inner) => {
                if inner.interests.lock().messages.is_empty() || inner.channel.is_closed() {
                    Ok(inner.channel.unbox())
                } else {
                    // This creates a new arc if there are outstanding interests. This will drop
                    // weak references, and whilst we do create a weak reference to ClientInner if
                    // we use it as a waker, it doesn't matter because if we have got this far, the
                    // waker is obsolete: no tasks are waiting.
                    Err(Self { inner: Arc::new(inner) })
                }
            }
            Err(inner) => Err(Self { inner }),
        }
    }

    /// Retrieve the stream of event messages for the `Client`.
    /// Panics if the stream was already taken.
    pub fn take_event_receiver(&self) -> EventReceiver<D> {
        {
            let mut lock = self.inner.interests.lock();

            if let EventListener::None = lock.event_listener {
                lock.event_listener = EventListener::WillPoll;
            } else {
                panic!("Event stream was already taken");
            }
        }

        EventReceiver { inner: self.inner.clone(), state: EventReceiverState::Active }
    }

    /// Encodes and sends a request without expecting a response.
    pub fn send<T: TypeMarker>(
        &self,
        body: impl Encode<T, D>,
        ordinal: u64,
        dynamic_flags: DynamicFlags,
    ) -> Result<(), Error> {
        let msg =
            TransactionMessage { header: TransactionHeader::new(0, ordinal, dynamic_flags), body };
        crate::encoding::with_tls_encoded::<TransactionMessageType<T>, D, ()>(
            msg,
            |bytes, handles| self.send_raw(bytes, handles),
        )
    }

    /// Encodes and sends a request. Returns a future that decodes the response.
    pub fn send_query<Request: TypeMarker, Response: TypeMarker, const ORDINAL: u64>(
        &self,
        body: impl Encode<Request, D>,
        dynamic_flags: DynamicFlags,
    ) -> QueryResponseFut<Response::Owned, D>
    where
        Response::Owned: Decode<Response, D>,
    {
        self.send_query_and_decode::<Request, Response::Owned>(
            body,
            ORDINAL,
            dynamic_flags,
            |buf| buf.and_then(decode_transaction_body::<Response, D, ORDINAL>),
        )
    }

    /// Encodes and sends a request. Returns a future that decodes the response
    /// using the given `decode` function.
    pub fn send_query_and_decode<Request: TypeMarker, Output>(
        &self,
        body: impl Encode<Request, D>,
        ordinal: u64,
        dynamic_flags: DynamicFlags,
        decode: fn(Result<D::MessageBufEtc, Error>) -> Result<Output, Error>,
    ) -> QueryResponseFut<Output, D> {
        let send_result = self.send_raw_query(|tx_id, bytes, handles| {
            let msg = TransactionMessage {
                header: TransactionHeader::new(tx_id.as_raw_id(), ordinal, dynamic_flags),
                body,
            };
            Encoder::encode::<TransactionMessageType<Request>>(bytes, handles, msg)?;
            Ok(())
        });

        QueryResponseFut(match send_result {
            Ok(res_fut) => future::maybe_done(res_fut.map(decode)),
            Err(e) => MaybeDone::Done(Err(e)),
        })
    }

    /// Sends a raw message without expecting a response.
    pub fn send_raw(
        &self,
        bytes: &[u8],
        handles: &mut [<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition],
    ) -> Result<(), Error> {
        match self.inner.channel.write_etc(bytes, handles) {
            Ok(()) | Err(None) => Ok(()),
            Err(Some(e)) => Err(Error::ClientWrite(e.into())),
        }
    }

    /// Sends a raw query and receives a response future.
    pub fn send_raw_query<F>(&self, encode_msg: F) -> Result<MessageResponse<D>, Error>
    where
        F: for<'a, 'b> FnOnce(
            Txid,
            &'a mut Vec<u8>,
            &'b mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
        ) -> Result<(), Error>,
    {
        let id = self.inner.interests.lock().register_msg_interest();
        crate::encoding::with_tls_encode_buf::<_, D>(|bytes, handles| {
            encode_msg(id, bytes, handles)?;
            self.send_raw(bytes, handles)
        })?;

        Ok(MessageResponse { id, client: Some(self.inner.clone()) })
    }
}

#[must_use]
/// A future which polls for the response to a client message.
#[derive(Debug)]
pub struct MessageResponse<D: ResourceDialect = DefaultFuchsiaResourceDialect> {
    id: Txid,
    // `None` if the message response has been received
    client: Option<Arc<ClientInner<D>>>,
}

impl<D: ResourceDialect> Unpin for MessageResponse<D> {}

impl<D: ResourceDialect> Future for MessageResponse<D> {
    type Output = Result<D::MessageBufEtc, Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        let res;
        {
            let client = this.client.as_ref().ok_or(Error::PollAfterCompletion)?;
            res = client.poll_recv_msg_response(this.id, cx);
        }

        // Drop the client reference if the response has been received
        if let Poll::Ready(Ok(_)) = res {
            this.client.take().expect("MessageResponse polled after completion");
        }

        res
    }
}

impl<D: ResourceDialect> Drop for MessageResponse<D> {
    fn drop(&mut self) {
        if let Some(client) = &self.client {
            client.interests.lock().deregister(self.id);
        }
    }
}

/// An enum reprenting either a resolved message interest or a task on which to alert
/// that a response message has arrived.
#[derive(Debug)]
enum MessageInterest<D: ResourceDialect> {
    /// A new `MessageInterest`
    WillPoll,
    /// A task is waiting to receive a response, and can be awoken with `Waker`.
    Waiting(Waker),
    /// A message has been received, and a task will poll to receive it.
    Received(D::MessageBufEtc),
    /// A message has not been received, but the person interested in the response
    /// no longer cares about it, so the message should be discared upon arrival.
    Discard,
}

impl<D: ResourceDialect> MessageInterest<D> {
    /// Check if a message has been received.
    fn is_received(&self) -> bool {
        matches!(*self, MessageInterest::Received(_))
    }

    fn unwrap_received(self) -> D::MessageBufEtc {
        if let MessageInterest::Received(buf) = self {
            buf
        } else {
            panic!("EXPECTED received message")
        }
    }
}

#[derive(Debug)]
enum EventReceiverState {
    Active,
    Terminal,
    Terminated,
}

/// A stream of events as `MessageBufEtc`s.
#[derive(Debug)]
pub struct EventReceiver<D: ResourceDialect = DefaultFuchsiaResourceDialect> {
    inner: Arc<ClientInner<D>>,
    state: EventReceiverState,
}

impl<D: ResourceDialect> Unpin for EventReceiver<D> {}

impl<D: ResourceDialect> FusedStream for EventReceiver<D> {
    fn is_terminated(&self) -> bool {
        matches!(self.state, EventReceiverState::Terminated)
    }
}

/// This implementation holds up two invariants
///   (1) After `None` is returned, the next poll panics
///   (2) Until this instance is dropped, no other EventReceiver may claim the
///       event channel by calling Client::take_event_receiver.
impl<D: ResourceDialect> Stream for EventReceiver<D> {
    type Item = Result<D::MessageBufEtc, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.state {
            EventReceiverState::Active => {}
            EventReceiverState::Terminated => {
                panic!("polled EventReceiver after `None`");
            }
            EventReceiverState::Terminal => {
                self.state = EventReceiverState::Terminated;
                return Poll::Ready(None);
            }
        }

        Poll::Ready(match ready!(self.inner.poll_recv_event(cx)) {
            Ok(x) => Some(Ok(x)),
            Err(Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::PeerClosed, ..
            }) => {
                // The channel is closed, with no epitaph. Set our internal state so that on
                // the next poll_next() we panic and is_terminated() returns an appropriate value.
                self.state = EventReceiverState::Terminated;
                None
            }
            err @ Err(_) => {
                // We've received a terminal error. Return it and set our internal state so that on
                // the next poll_next() we return a None and terminate the stream.
                self.state = EventReceiverState::Terminal;
                Some(err)
            }
        })
    }
}

impl<D: ResourceDialect> Drop for EventReceiver<D> {
    fn drop(&mut self) {
        self.inner.interests.lock().dropped_event_listener();
    }
}

#[derive(Debug, Default)]
enum EventListener {
    /// No one is listening for the event
    #[default]
    None,
    /// Someone is listening for the event but has not yet polled
    WillPoll,
    /// Someone is listening for the event and can be woken via the `Waker`
    Some(Waker),
}

impl EventListener {
    fn is_some(&self) -> bool {
        matches!(self, EventListener::Some(_))
    }
}

/// A shared client channel which tracks EXPECTED and received responses
#[derive(Debug)]
struct ClientInner<D: ResourceDialect> {
    /// The channel that leads to the server we are connected to.
    channel: <D::ProxyChannel as ProxyChannelFor<D>>::Boxed,

    /// Tracks the state of responses to two-way messages and events.
    interests: Mutex<Interests<D>>,

    /// A terminal error, which can be a server provided epitaph, or None if the channel is still
    /// active.
    terminal_error: Mutex<Option<Error>>,

    /// The `ProtocolMarker::DEBUG_NAME` for the service this client connects to.
    protocol_name: &'static str,
}

#[derive(Debug)]
struct Interests<D: ResourceDialect> {
    messages: Slab<MessageInterest<D>>,
    events: VecDeque<D::MessageBufEtc>,
    event_listener: EventListener,
    /// The number of wakers registered waiting for either a message or an event.
    waker_count: usize,
    /// Txid generation.
    /// This is incremented every time we mint a new txid (see register_msg_interest).
    /// The lower 7 bits are incorporated into the txid.
    /// This is so that a client repeatedly making calls will have distinct txids for each call.
    /// Not necessary for correctness but _very_ useful for tracing and debugging.
    generation: u8,
}

impl<D: ResourceDialect> Default for Interests<D> {
    fn default() -> Self {
        Interests {
            messages: Slab::new(),
            events: Default::default(),
            event_listener: Default::default(),
            waker_count: 0,
            generation: 0,
        }
    }
}

impl<D: ResourceDialect> Interests<D> {
    /// Receives an event and returns a waker, if any.
    fn push_event(&mut self, buf: D::MessageBufEtc) -> Option<Waker> {
        self.events.push_back(buf);
        self.take_event_waker()
    }

    /// Returns the waker for the task waiting for events, if any.
    fn take_event_waker(&mut self) -> Option<Waker> {
        if self.event_listener.is_some() {
            let EventListener::Some(waker) =
                mem::replace(&mut self.event_listener, EventListener::WillPoll)
            else {
                unreachable!()
            };

            // Matches the +1 in `register_event_listener`.
            self.waker_count -= 1;
            Some(waker)
        } else {
            None
        }
    }

    /// Returns a reference to the waker.
    fn event_waker(&self) -> Option<&Waker> {
        match &self.event_listener {
            EventListener::Some(waker) => Some(waker),
            _ => None,
        }
    }

    /// Receive a message, waking the waiter if they are waiting to poll and `wake` is true.
    /// Returns an error of the message isn't found.
    fn push_message(&mut self, txid: Txid, buf: D::MessageBufEtc) -> Result<Option<Waker>, Error> {
        let InterestId(raw_id) = InterestId::from_txid(txid);
        // Look for a message interest with the given ID.
        // If one is found, store the message so that it can be picked up later.
        let Some(interest) = self.messages.get_mut(raw_id) else {
            // TODO(https://fxbug.dev/42066009): Should close the channel.
            return Err(Error::InvalidResponseTxid);
        };

        let mut waker = None;
        if let MessageInterest::Discard = interest {
            self.messages.remove(raw_id);
        } else if let MessageInterest::Waiting(w) =
            mem::replace(interest, MessageInterest::Received(buf))
        {
            waker = Some(w);

            // Matches the +1 in `register`.
            self.waker_count -= 1;
        }

        Ok(waker)
    }

    /// Registers the waker from `cx` if the message has not already been received, replacing any
    /// previous waker registered.  Returns the message if it has been received.
    fn register(&mut self, txid: Txid, cx: &Context<'_>) -> Option<D::MessageBufEtc> {
        let InterestId(raw_id) = InterestId::from_txid(txid);
        let interest = self.messages.get_mut(raw_id).expect("Polled unregistered interest");
        match interest {
            MessageInterest::Received(_) => {
                return Some(self.messages.remove(raw_id).unwrap_received());
            }
            MessageInterest::Discard => panic!("Polled a discarded MessageReceiver?!"),
            MessageInterest::WillPoll => self.waker_count += 1,
            MessageInterest::Waiting(_) => {}
        }
        *interest = MessageInterest::Waiting(cx.waker().clone());
        None
    }

    /// Deregisters an interest.
    fn deregister(&mut self, txid: Txid) {
        let InterestId(raw_id) = InterestId::from_txid(txid);
        match self.messages[raw_id] {
            MessageInterest::Received(_) => {
                self.messages.remove(raw_id);
                return;
            }
            MessageInterest::WillPoll => {}
            MessageInterest::Waiting(_) => self.waker_count -= 1,
            MessageInterest::Discard => unreachable!(),
        }
        self.messages[raw_id] = MessageInterest::Discard;
    }

    /// Registers an event listener.
    fn register_event_listener(&mut self, cx: &Context<'_>) -> Option<D::MessageBufEtc> {
        self.events.pop_front().or_else(|| {
            if !mem::replace(&mut self.event_listener, EventListener::Some(cx.waker().clone()))
                .is_some()
            {
                self.waker_count += 1;
            }
            None
        })
    }

    /// Indicates the event listener has been dropped.
    fn dropped_event_listener(&mut self) {
        if self.event_listener.is_some() {
            // Matches the +1 in register_event_listener.
            self.waker_count -= 1;
        }
        self.event_listener = EventListener::None;
    }

    /// Registers interest in a response message.
    ///
    /// This function returns a new transaction ID which should be used to send a message
    /// via the channel. Responses are then received using `poll_recv_msg_response`.
    fn register_msg_interest(&mut self) -> Txid {
        self.generation = self.generation.wrapping_add(1);
        // TODO(cramertj) use `try_from` here and assert that the conversion from
        // `usize` to `u32` hasn't overflowed.
        Txid::from_interest_id(
            InterestId(self.messages.insert(MessageInterest::WillPoll)),
            self.generation,
        )
    }
}

impl<D: ResourceDialect> ClientInner<D> {
    fn poll_recv_event(
        self: &Arc<Self>,
        cx: &Context<'_>,
    ) -> Poll<Result<D::MessageBufEtc, Error>> {
        // Update the EventListener with the latest waker, remove any stale WillPoll state
        if let Some(msg_buf) = self.interests.lock().register_event_listener(cx) {
            return Poll::Ready(Ok(msg_buf));
        }

        // Process any data on the channel, registering any tasks still waiting to wake when the
        // channel becomes ready.
        let maybe_terminal_error = self.recv_all(Some(Txid(0)));

        let mut lock = self.interests.lock();

        if let Some(msg_buf) = lock.events.pop_front() {
            Poll::Ready(Ok(msg_buf))
        } else {
            maybe_terminal_error?;
            Poll::Pending
        }
    }

    /// Poll for the response to `txid`, registering the waker associated with `cx` to be awoken,
    /// or returning the response buffer if it has been received.
    fn poll_recv_msg_response(
        self: &Arc<Self>,
        txid: Txid,
        cx: &Context<'_>,
    ) -> Poll<Result<D::MessageBufEtc, Error>> {
        // Register our waker with the interest if we haven't received a message yet.
        if let Some(buf) = self.interests.lock().register(txid, cx) {
            return Poll::Ready(Ok(buf));
        }

        // Process any data on the channel, registering tasks still waiting for wake when the
        // channel becomes ready.
        let maybe_terminal_error = self.recv_all(Some(txid));

        let InterestId(raw_id) = InterestId::from_txid(txid);
        let mut interests = self.interests.lock();
        if interests.messages.get(raw_id).expect("Polled unregistered interest").is_received() {
            // If we got the result remove the received buffer and return, freeing up the
            // space for a new message.
            let buf = interests.messages.remove(raw_id).unwrap_received();
            Poll::Ready(Ok(buf))
        } else {
            maybe_terminal_error?;
            Poll::Pending
        }
    }

    /// Poll for the receipt of any response message or an event.
    /// Wakers present in any MessageInterest or the EventReceiver when this is called will be
    /// notified when their message arrives or when there is new data if the channel is empty.
    ///
    /// All errors are terminal, so once an error has been encountered, all subsequent calls will
    /// produce the same error.  The error might be due to the reception of an epitaph, the peer end
    /// of the channel being closed, a decode error or some other error.  Before using this terminal
    /// error, callers *should* check to see if a response or event has been received as they
    /// should normally, at least for the PEER_CLOSED case, be delivered before the terminal error.
    fn recv_all(self: &Arc<Self>, want_txid: Option<Txid>) -> Result<(), Error> {
        // Acquire a mutex so that only one thread can read from the underlying channel
        // at a time. Channel is already synchronized, but we need to also decode the
        // FIDL message header atomically so that epitaphs can be properly handled.
        let mut terminal_error = self.terminal_error.lock();
        if let Some(error) = terminal_error.as_ref() {
            return Err(error.clone());
        }

        let recv_once = |waker| {
            let cx = &mut Context::from_waker(&waker);

            let mut buf = D::MessageBufEtc::new();
            let result = self.channel.recv_etc_from(cx, &mut buf);
            match result {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(None)) => {
                    // The channel has been closed, and no epitaph was received.
                    // Set the epitaph to PEER_CLOSED.
                    return Err(Error::ClientChannelClosed {
                        epitaph: crate::error::Epitaph::PeerClosed,
                        protocol_name: self.protocol_name,
                        #[cfg(not(target_os = "fuchsia"))]
                        reason: self.channel.closed_reason(),
                    });
                }
                Poll::Ready(Err(Some(e))) => return Err(Error::ClientRead(e.into())),
                Poll::Pending => return Ok(ControlFlow::Break(())),
            };

            let (bytes, _) = buf.split_mut();
            let (header, body_bytes) = decode_transaction_header(bytes)?;
            if header.is_epitaph() {
                // Received an epitaph. Record this so that everyone receives the same epitaph.
                let handles = &mut [];
                let mut epitaph_body = Decode::<EpitaphBody, D>::new_empty();
                Decoder::<D>::decode_into::<EpitaphBody>(
                    &header,
                    body_bytes,
                    handles,
                    &mut epitaph_body,
                )?;
                return Err(Error::ClientChannelClosed {
                    epitaph: crate::error::Epitaph::Explicit(epitaph_body.error),
                    protocol_name: self.protocol_name,
                    #[cfg(not(target_os = "fuchsia"))]
                    reason: self.channel.closed_reason(),
                });
            }

            let txid = Txid(header.tx_id);

            let waker = {
                buf.shrink_bytes_to_fit();
                let mut interests = self.interests.lock();
                if txid == Txid(0) {
                    interests.push_event(buf)
                } else {
                    interests.push_message(txid, buf)?
                }
            };

            // Skip waking if the message was for the caller.
            if want_txid != Some(txid)
                && let Some(waker) = waker
            {
                waker.wake();
            }

            Ok(ControlFlow::Continue(()))
        };

        loop {
            let waker = {
                let interests = self.interests.lock();
                if interests.waker_count == 0 {
                    return Ok(());
                } else if interests.waker_count == 1 {
                    // There's only one waker, so just use the waker for the one interest.  This
                    // is also required to allow `into_channel` to work, which relies on
                    // `Arc::try_into` which won't always work if we use a waker based on
                    // `ClientInner` (even if it's weak), because there can be races where the
                    // reference count on ClientInner is > 1.
                    if let Some(waker) = interests.event_waker() {
                        waker.clone()
                    } else {
                        interests
                            .messages
                            .iter()
                            .find_map(|(_, interest)| {
                                if let MessageInterest::Waiting(waker) = interest {
                                    Some(waker.clone())
                                } else {
                                    None
                                }
                            })
                            .unwrap()
                    }
                } else {
                    let weak = Arc::downgrade(self);
                    let waker = ClientWaker(Arc::new(move || {
                        if let Some(strong) = weak.upgrade() {
                            // On host, we can't call recv_all because there are reentrancy issues; the waker is
                            // woken whilst locks are held on the channel which recv_all needs.
                            #[cfg(target_os = "fuchsia")]
                            if strong.recv_all(None).is_ok() {
                                return;
                            }

                            strong.wake_all();
                        }
                    }));
                    // If there's more than one waker, use a waker that points to
                    // `ClientInner` which will read the message and figure out which is
                    // the correct task to wake.
                    // SAFETY: We meet the requirements specified by RawWaker.
                    unsafe {
                        Waker::from_raw(RawWaker::new(
                            Arc::into_raw(Arc::new(waker)) as *const (),
                            &WAKER_VTABLE,
                        ))
                    }
                }
            };

            match recv_once(waker) {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(())) => return Ok(()),
                Err(error) => {
                    // Broadcast all errors.
                    self.wake_all();
                    return Err(terminal_error.insert(error).clone());
                }
            }
        }
    }

    /// Wakes all tasks that have polled on this channel.
    fn wake_all(&self) {
        let mut lock = self.interests.lock();
        for (_, interest) in &mut lock.messages {
            if let MessageInterest::Waiting(_) = interest {
                let MessageInterest::Waiting(waker) =
                    mem::replace(interest, MessageInterest::WillPoll)
                else {
                    unreachable!()
                };
                waker.wake();
            }
        }
        if let Some(waker) = lock.take_event_waker() {
            waker.wake();
        }
        lock.waker_count = 0;
    }
}

#[derive(Clone)]
struct ClientWaker(Arc<dyn Fn() + Send + Sync + 'static>);

static WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);

unsafe fn clone_waker(data: *const ()) -> RawWaker {
    unsafe { Arc::increment_strong_count(data as *const ClientWaker) };
    RawWaker::new(data, &WAKER_VTABLE)
}

unsafe fn wake(data: *const ()) {
    unsafe { Arc::from_raw(data as *const ClientWaker) }.0();
}

unsafe fn wake_by_ref(data: *const ()) {
    mem::ManuallyDrop::new(unsafe { Arc::from_raw(data as *const ClientWaker) }).0();
}

unsafe fn drop_waker(data: *const ()) {
    unsafe { Arc::from_raw(data as *const ClientWaker) };
}

#[cfg(target_os = "fuchsia")]
pub mod sync {
    //! Synchronous FIDL Client

    use super::*;
    use crate::endpoints::ProtocolMarker;
    use std::mem::MaybeUninit;
    use zx::MessageBufEtc;

    /// A synchronous client for making FIDL calls.
    #[derive(Debug)]
    pub struct Client {
        // Underlying channel
        channel: zx::Channel,
    }

    impl Client {
        /// Create a new synchronous FIDL client.
        pub fn new(channel: zx::Channel) -> Self {
            Client { channel }
        }

        /// Return a reference to the underlying channel for the client.
        pub fn as_channel(&self) -> &zx::Channel {
            &self.channel
        }

        /// Get the underlying channel out of the client.
        pub fn into_channel(self) -> zx::Channel {
            self.channel
        }

        /// Send a new message.
        pub fn send<T: TypeMarker>(
            &self,
            body: impl Encode<T, DefaultFuchsiaResourceDialect>,
            ordinal: u64,
            dynamic_flags: DynamicFlags,
        ) -> Result<(), Error> {
            let mut write_bytes = Vec::new();
            let mut write_handles = Vec::new();
            let msg = TransactionMessage {
                header: TransactionHeader::new(0, ordinal, dynamic_flags),
                body,
            };
            Encoder::encode::<TransactionMessageType<T>>(
                &mut write_bytes,
                &mut write_handles,
                msg,
            )?;
            match self.channel.write_etc(&write_bytes, &mut write_handles) {
                Ok(()) | Err(zx_status::Status::PEER_CLOSED) => Ok(()),
                Err(e) => Err(Error::ClientWrite(e.into())),
            }
        }

        /// Send a new message expecting a response.
        pub fn send_query<Request: TypeMarker, Response: TypeMarker, P: ProtocolMarker>(
            &self,
            body: impl Encode<Request, DefaultFuchsiaResourceDialect>,
            ordinal: u64,
            dynamic_flags: DynamicFlags,
            deadline: zx::MonotonicInstant,
        ) -> Result<Response::Owned, Error>
        where
            Response::Owned: Decode<Response, DefaultFuchsiaResourceDialect>,
        {
            let mut write_bytes = Vec::new();
            let mut write_handles = Vec::new();

            let msg = TransactionMessage {
                header: TransactionHeader::new(0, ordinal, dynamic_flags),
                body,
            };
            Encoder::encode::<TransactionMessageType<Request>>(
                &mut write_bytes,
                &mut write_handles,
                msg,
            )?;

            // Heap allocate the buffer, because on the stack, all the pages would be written to by
            // the compiler (see stack probing).
            let mut bytes_out =
                Vec::<MaybeUninit<u8>>::with_capacity(zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize);
            // SAFETY: Because the type is MaybeUninit, having it use uninitialized memory
            // is safe.
            unsafe { bytes_out.set_len(zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize) };

            // Stack-allocate these buffers to avoid the heap and reuse any populated pages from
            // previous function calls. Use uninitialized memory so that the only writes to this
            // array will be by the kernel for whatever's actually used for the reply.
            let handles_out = &mut [const { MaybeUninit::<zx::HandleInfo>::uninit() };
                zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize];

            // TODO: We should be able to use the same memory to back the bytes we use for writing
            // and reading.
            let (bytes_out, handles_out) = self
                .channel
                .call_etc_uninit(
                    deadline,
                    &write_bytes,
                    &mut write_handles,
                    bytes_out.as_mut_slice(),
                    handles_out,
                )
                .map_err(|e| self.wrap_error::<P, _>(Error::ClientCall, e))?;

            let (header, body_bytes) = decode_transaction_header(bytes_out)?;
            if header.ordinal != ordinal {
                return Err(Error::InvalidResponseOrdinal);
            }
            let mut output = Decode::<Response, DefaultFuchsiaResourceDialect>::new_empty();
            Decoder::<DefaultFuchsiaResourceDialect>::decode_into::<Response>(
                &header,
                body_bytes,
                handles_out,
                &mut output,
            )?;
            Ok(output)
        }

        /// Wait for an event to arrive on the underlying channel.
        pub fn wait_for_event<P: ProtocolMarker>(
            &self,
            deadline: zx::MonotonicInstant,
        ) -> Result<MessageBufEtc, Error> {
            let mut buf = zx::MessageBufEtc::new();
            buf.ensure_capacity_bytes(zx::sys::ZX_CHANNEL_MAX_MSG_BYTES as usize);
            buf.ensure_capacity_handle_infos(zx::sys::ZX_CHANNEL_MAX_MSG_HANDLES as usize);

            loop {
                self.channel
                    .wait_one(
                        zx::Signals::CHANNEL_READABLE | zx::Signals::CHANNEL_PEER_CLOSED,
                        deadline,
                    )
                    .map_err(|e| self.wrap_error::<P, _>(Error::ClientEvent, e))?;
                match self.channel.read_etc(&mut buf) {
                    Ok(()) => {
                        // We succeeded in reading the message. Check that it is
                        // an event not a two-way method reply.
                        let (header, body_bytes) = decode_transaction_header(buf.bytes())
                            .map_err(|_| Error::InvalidHeader)?;
                        if header.is_epitaph() {
                            // Received an epitaph. For the sync bindings, epitaphs are only
                            // reported by wait_for_event.
                            let handles = &mut [];
                            let mut epitaph_body =
                                Decode::<EpitaphBody, DefaultFuchsiaResourceDialect>::new_empty();
                            Decoder::<DefaultFuchsiaResourceDialect>::decode_into::<EpitaphBody>(
                                &header,
                                body_bytes,
                                handles,
                                &mut epitaph_body,
                            )?;
                            return Err(Error::ClientChannelClosed {
                                epitaph: crate::error::Epitaph::Explicit(epitaph_body.error),
                                protocol_name: P::DEBUG_NAME,
                            });
                        }
                        if header.tx_id != 0 {
                            return Err(Error::UnexpectedSyncResponse);
                        }
                        return Ok(buf);
                    }
                    Err(zx::Status::SHOULD_WAIT) => {
                        // Some other thread read the message we woke up to read.
                        continue;
                    }
                    Err(e) => {
                        return Err(self.wrap_error::<P, _>(|x| Error::ClientRead(x.into()), e));
                    }
                }
            }
        }

        /// Wraps an error in the given `variant` of the `Error` enum, except
        /// for `zx_status::Status::PEER_CLOSED`, in which case it uses the
        /// `Error::ClientChannelClosed` variant.
        fn wrap_error<P: ProtocolMarker, T: Fn(zx_status::Status) -> Error>(
            &self,
            variant: T,
            err: zx_status::Status,
        ) -> Error {
            if err == zx_status::Status::PEER_CLOSED {
                Error::ClientChannelClosed {
                    epitaph: crate::error::Epitaph::PeerClosed,
                    protocol_name: P::DEBUG_NAME,
                }
            } else {
                variant(err)
            }
        }
    }
}

#[cfg(all(test, target_os = "fuchsia"))]
mod tests {
    use super::*;
    use crate::encoding::MAGIC_NUMBER_INITIAL;
    use crate::endpoints::{ControlHandle, ProtocolMarker, Proxy, RequestStream, SynchronousProxy};
    use crate::epitaph::{self, ChannelEpitaphExt};
    use crate::{Channel, OnSignalsRef, ServeInner};
    use anyhow::{Context as _, Error};
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use fuchsia_async::{Channel as AsyncChannel, DurationExt, TimeoutExt};
    use futures::channel::oneshot;
    use futures::stream::{FuturesUnordered, Stream};
    use futures::{StreamExt, TryFutureExt, join};
    use futures_test::task::new_count_waker;
    use std::future::pending;
    use std::task::{Wake, Waker};
    use std::thread;
    use zx::MessageBufEtc;

    const SEND_ORDINAL_HIGH_BYTE: u8 = 42;
    const SEND_ORDINAL: u64 = 42 << 32;
    const SEND_DATA: u8 = 55;

    const EVENT_ORDINAL: u64 = 854 << 23;

    struct TestProtocolMarker;
    impl ProtocolMarker for TestProtocolMarker {
        type Proxy = TestProxy;
        type SynchronousProxy = TestSynchronousProxy;
        type RequestStream = TestRequestStream;
        const DEBUG_NAME: &str = "test_protocol";
    }

    struct TestProxy;
    impl Proxy for TestProxy {
        type Protocol = TestProtocolMarker;
        fn from_channel(_inner: AsyncChannel) -> Self {
            unimplemented!();
        }
        fn into_channel(self) -> Result<AsyncChannel, Self> {
            unimplemented!();
        }
        fn as_channel(&self) -> &AsyncChannel {
            unimplemented!();
        }
    }

    struct TestSynchronousProxy;
    impl SynchronousProxy for TestSynchronousProxy {
        type Proxy = TestProxy;
        type Protocol = TestProtocolMarker;
        fn from_channel(_inner: Channel) -> Self {
            unimplemented!();
        }
        fn into_channel(self) -> Channel {
            unimplemented!();
        }
        fn as_channel(&self) -> &Channel {
            unimplemented!();
        }
    }

    struct TestRequestStream;
    impl RequestStream for TestRequestStream {
        type Protocol = TestProtocolMarker;
        type ControlHandle = TestControlHandle;
        fn control_handle(&self) -> Self::ControlHandle {
            unimplemented!();
        }
        fn from_channel(_inner: AsyncChannel) -> Self {
            unimplemented!();
        }
        fn into_inner(self) -> (Arc<ServeInner>, bool) {
            unimplemented!();
        }

        fn from_inner(_inner: Arc<ServeInner>, _is_terminated: bool) -> Self {
            unimplemented!();
        }
    }
    impl Stream for TestRequestStream {
        type Item = Result<(), crate::Error>;
        fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            unimplemented!();
        }
    }

    struct TestControlHandle;
    impl ControlHandle for TestControlHandle {
        fn shutdown(&self) {
            unimplemented!();
        }
        fn shutdown_with_epitaph(&self, _status: crate::Epitaph) {
            unimplemented!();
        }
        fn is_closed(&self) -> bool {
            unimplemented!();
        }
        fn on_closed(&self) -> OnSignalsRef<'_> {
            unimplemented!();
        }
        fn signal_peer(
            &self,
            _clear_mask: zx::Signals,
            _set_mask: zx::Signals,
        ) -> Result<(), zx_status::Status> {
            unimplemented!();
        }
    }

    #[rustfmt::skip]
    fn expected_sent_bytes(txid_index: u8, txid_generation: u8) -> [u8; 24] {
        [
            txid_index, 0, 0, txid_generation, // 32 bit tx_id
            2, 0, 0, // flags
            MAGIC_NUMBER_INITIAL,
            0, 0, 0, 0, // low bytes of 64 bit ordinal
            SEND_ORDINAL_HIGH_BYTE, 0, 0, 0, // high bytes of 64 bit ordinal
            SEND_DATA, // 8 bit data
            0, 0, 0, 0, 0, 0, 0, // 7 bytes of padding after our 1 byte of data
        ]
    }

    fn expected_sent_bytes_oneway() -> [u8; 24] {
        expected_sent_bytes(0, 0)
    }

    fn send_transaction(header: TransactionHeader, channel: &zx::Channel) {
        let (bytes, handles) = (&mut vec![], &mut vec![]);
        encode_transaction(header, bytes, handles);
        channel.write_etc(bytes, handles).expect("Server channel write failed");
    }

    fn encode_transaction(
        header: TransactionHeader,
        bytes: &mut Vec<u8>,
        handles: &mut Vec<zx::HandleDisposition<'static>>,
    ) {
        let event = TransactionMessage { header, body: SEND_DATA };
        Encoder::<DefaultFuchsiaResourceDialect>::encode::<TransactionMessageType<u8>>(
            bytes, handles, event,
        )
        .expect("Encoding failure");
    }

    #[test]
    fn sync_client() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        client.send::<u8>(SEND_DATA, SEND_ORDINAL, DynamicFlags::empty()).context("sending")?;
        let mut received = MessageBufEtc::new();
        server_end.read_etc(&mut received).context("reading")?;
        assert_eq!(received.bytes(), expected_sent_bytes_oneway());
        Ok(())
    }

    #[test]
    fn sync_client_with_response() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        thread::spawn(move || {
            // Server
            let mut received = MessageBufEtc::new();
            server_end
                .wait_one(
                    zx::Signals::CHANNEL_READABLE,
                    zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
                )
                .expect("failed to wait for channel readable");
            server_end.read_etc(&mut received).expect("failed to read on server end");
            let (buf, _handles) = received.split_mut();
            let (header, _body_bytes) = decode_transaction_header(buf).expect("server decode");
            assert_eq!(header.ordinal, SEND_ORDINAL);
            send_transaction(
                TransactionHeader::new(header.tx_id, header.ordinal, DynamicFlags::empty()),
                &server_end,
            );
        });
        let response_data = client
            .send_query::<u8, u8, TestProtocolMarker>(
                SEND_DATA,
                SEND_ORDINAL,
                DynamicFlags::empty(),
                zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
            )
            .context("sending query")?;
        assert_eq!(SEND_DATA, response_data);
        Ok(())
    }

    #[test]
    fn sync_client_with_event_and_response() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        thread::spawn(move || {
            // Server
            let mut received = MessageBufEtc::new();
            server_end
                .as_handle_ref()
                .wait_one(
                    zx::Signals::CHANNEL_READABLE,
                    zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
                )
                .expect("failed to wait for channel readable");
            server_end.read_etc(&mut received).expect("failed to read on server end");
            let (buf, _handles) = received.split_mut();
            let (header, _body_bytes) = decode_transaction_header(buf).expect("server decode");
            assert_ne!(header.tx_id, 0);
            assert_eq!(header.ordinal, SEND_ORDINAL);
            // First, send an event.
            send_transaction(
                TransactionHeader::new(0, EVENT_ORDINAL, DynamicFlags::empty()),
                &server_end,
            );
            // Then send the reply. The kernel should pick the correct message to deliver based
            // on the tx_id.
            send_transaction(
                TransactionHeader::new(header.tx_id, header.ordinal, DynamicFlags::empty()),
                &server_end,
            );
        });
        let response_data = client
            .send_query::<u8, u8, TestProtocolMarker>(
                SEND_DATA,
                SEND_ORDINAL,
                DynamicFlags::empty(),
                zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5)),
            )
            .context("sending query")?;
        assert_eq!(SEND_DATA, response_data);

        let event_buf = client
            .wait_for_event::<TestProtocolMarker>(zx::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(5),
            ))
            .context("waiting for event")?;
        let (bytes, _handles) = event_buf.split();
        let (header, _body) = decode_transaction_header(&bytes).expect("event decode");
        assert_eq!(header.ordinal, EVENT_ORDINAL);

        Ok(())
    }

    #[test]
    fn sync_client_with_racing_events() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client1 = Arc::new(sync::Client::new(client_end));
        let client2 = client1.clone();

        let thread1 = thread::spawn(move || {
            let result = client1.wait_for_event::<TestProtocolMarker>(zx::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(5),
            ));
            assert!(result.is_ok());
        });

        let thread2 = thread::spawn(move || {
            let result = client2.wait_for_event::<TestProtocolMarker>(zx::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(5),
            ));
            assert!(result.is_ok());
        });

        send_transaction(
            TransactionHeader::new(0, EVENT_ORDINAL, DynamicFlags::empty()),
            &server_end,
        );
        send_transaction(
            TransactionHeader::new(0, EVENT_ORDINAL, DynamicFlags::empty()),
            &server_end,
        );

        assert!(thread1.join().is_ok());
        assert!(thread2.join().is_ok());

        Ok(())
    }

    #[test]
    fn sync_client_wait_for_event_gets_method_response() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        send_transaction(
            TransactionHeader::new(3902304923, SEND_ORDINAL, DynamicFlags::empty()),
            &server_end,
        );
        assert_matches!(
            client.wait_for_event::<TestProtocolMarker>(zx::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(5)
            )),
            Err(crate::Error::UnexpectedSyncResponse)
        );
        Ok(())
    }

    #[test]
    fn sync_client_one_way_call_suceeds_after_peer_closed() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        drop(server_end);
        assert_matches!(client.send::<u8>(SEND_DATA, SEND_ORDINAL, DynamicFlags::empty()), Ok(()));
        Ok(())
    }

    #[test]
    fn sync_client_two_way_call_fails_after_peer_closed() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        drop(server_end);
        assert_matches!(
            client.send_query::<u8, u8, TestProtocolMarker>(
                SEND_DATA,
                SEND_ORDINAL,
                DynamicFlags::empty(),
                zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5))
            ),
            Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::PeerClosed,
                protocol_name: "test_protocol",
            })
        );
        Ok(())
    }

    // TODO(https://fxbug.dev/42153053): When the sync client supports epitaphs, rename
    // these tests and change the asserts to expect zx_status::Status::UNAVAILABLE.
    #[test]
    fn sync_client_send_does_not_receive_epitaphs() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        // Close the server channel with an epitaph.
        server_end
            .close_with_epitaph(zx_status::Status::UNAVAILABLE)
            .expect("failed to write epitaph");
        assert_matches!(
            client.send_query::<u8, u8, TestProtocolMarker>(
                SEND_DATA,
                SEND_ORDINAL,
                DynamicFlags::empty(),
                zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(5))
            ),
            Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::PeerClosed,
                protocol_name: "test_protocol",
            })
        );
        Ok(())
    }

    #[test]
    fn sync_client_wait_for_events_does_receive_epitaphs() -> Result<(), Error> {
        let (client_end, server_end) = zx::Channel::create();
        let client = sync::Client::new(client_end);
        // Close the server channel with an epitaph.
        server_end
            .close_with_epitaph(zx_status::Status::UNAVAILABLE)
            .expect("failed to write epitaph");
        assert_matches!(
            client.wait_for_event::<TestProtocolMarker>(zx::MonotonicInstant::after(
                zx::MonotonicDuration::from_seconds(5)
            )),
            Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                protocol_name: "test_protocol",
            })
        );
        Ok(())
    }

    #[test]
    fn sync_client_into_channel() -> Result<(), Error> {
        let (client_end, _server_end) = zx::Channel::create();
        let client_end_raw = client_end.raw_handle();
        let client = sync::Client::new(client_end);
        assert_eq!(client.into_channel().raw_handle(), client_end_raw);
        Ok(())
    }

    #[fasync::run_singlethreaded(test)]
    async fn client() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);
        let receiver = async move {
            let mut buffer = MessageBufEtc::new();
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            assert_eq!(buffer.bytes(), expected_sent_bytes_oneway());
        };

        // add a timeout to receiver so if test is broken it doesn't take forever
        let receiver = receiver
            .on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
                panic!("did not receive message in time!")
            });

        client
            .send::<u8>(SEND_DATA, SEND_ORDINAL, DynamicFlags::empty())
            .expect("failed to send msg");

        receiver.await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_with_response() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);
        let mut buffer = MessageBufEtc::new();
        let receiver = async move {
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            let two_way_tx_id = 1u8;
            assert_eq!(buffer.bytes(), expected_sent_bytes(two_way_tx_id, 1));

            let (bytes, handles) = (&mut vec![], &mut vec![]);
            let header =
                TransactionHeader::new(two_way_tx_id as u32, SEND_ORDINAL, DynamicFlags::empty());
            encode_transaction(header, bytes, handles);
            server.write_etc(bytes, handles).expect("Server channel write failed");
        };

        // add a timeout to receiver so if test is broken it doesn't take forever
        let receiver = receiver
            .on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
                panic!("did not receiver message in time!")
            });

        let sender = client
            .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
            .map_ok(|x| assert_eq!(x, SEND_DATA))
            .unwrap_or_else(|e| panic!("fidl error: {e:?}"));

        // add a timeout to receiver so if test is broken it doesn't take forever
        let sender = sender.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
            panic!("did not receive response in time!")
        });

        let ((), ()) = join!(receiver, sender);
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_with_response_receives_epitaph() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);
        let mut buffer = zx::MessageBufEtc::new();
        let receiver = async move {
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            server
                .close_with_epitaph(zx_status::Status::UNAVAILABLE)
                .expect("failed to write epitaph");
        };
        // add a timeout to receiver so if test is broken it doesn't take forever
        let receiver = receiver
            .on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
                panic!("did not receive message in time!")
            });

        let sender = async move {
            const ORDINAL: u64 = 42 << 32;
            let result = client.send_query::<u8, u8, ORDINAL>(55, DynamicFlags::empty()).await;
            assert_matches!(
                result,
                Err(crate::Error::ClientChannelClosed {
                    epitaph: crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                    protocol_name: "test_protocol",
                })
            );
        };
        // add a timeout to sender so if test is broken it doesn't take forever
        let sender = sender.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
            panic!("did not receive response in time!")
        });

        let ((), ()) = join!(receiver, sender);
    }

    #[fasync::run_singlethreaded(test)]
    #[should_panic]
    async fn event_cant_be_taken_twice() {
        let (client_end, _) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        let _foo = client.take_event_receiver();
        client.take_event_receiver();
    }

    #[fasync::run_singlethreaded(test)]
    async fn event_can_be_taken_after_drop() {
        let (client_end, _) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        let foo = client.take_event_receiver();
        drop(foo);
        client.take_event_receiver();
    }

    #[fasync::run_singlethreaded(test)]
    async fn receiver_termination_test() {
        let (client_end, _) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        let mut foo = client.take_event_receiver();
        assert!(!foo.is_terminated(), "receiver should not report terminated before being polled");
        let _ = foo.next().await;
        assert!(
            foo.is_terminated(),
            "receiver should report terminated after seeing channel is closed"
        );
    }

    #[fasync::run_singlethreaded(test)]
    #[should_panic(expected = "polled EventReceiver after `None`")]
    async fn receiver_cant_be_polled_more_than_once_on_closed_stream() {
        let (client_end, _) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        let foo = client.take_event_receiver();
        drop(foo);
        let mut bar = client.take_event_receiver();
        assert!(bar.next().await.is_none(), "read on closed channel should return none");
        // this should panic
        let _ = bar.next().await;
    }

    #[fasync::run_singlethreaded(test)]
    #[should_panic(expected = "polled EventReceiver after `None`")]
    async fn receiver_panics_when_polled_after_receiving_epitaph_then_none() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let server_end = AsyncChannel::from_channel(server_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        let mut stream = client.take_event_receiver();

        epitaph::write_epitaph_impl(&server_end, Err(zx_status::Status::UNAVAILABLE))
            .expect("wrote epitaph");
        drop(server_end);

        assert_matches!(
            stream.next().await,
            Some(Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                protocol_name: "test_protocol",
            }))
        );
        assert_matches!(stream.next().await, None);
        // this should panic
        let _ = stream.next().await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn event_can_be_taken() {
        let (client_end, _) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");
        client.take_event_receiver();
    }

    #[fasync::run_singlethreaded(test)]
    async fn event_received() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        // Send the event from the server
        let server = AsyncChannel::from_channel(server_end);
        let (bytes, handles) = (&mut vec![], &mut vec![]);
        const ORDINAL: u64 = 5;
        let header = TransactionHeader::new(0, ORDINAL, DynamicFlags::empty());
        encode_transaction(header, bytes, handles);
        server.write_etc(bytes, handles).expect("Server channel write failed");
        drop(server);

        let recv = client
            .take_event_receiver()
            .into_future()
            .then(|(x, stream)| {
                let x = x.expect("should contain one element");
                let x = x.expect("fidl error");
                let x: i32 =
                    decode_transaction_body::<i32, DefaultFuchsiaResourceDialect, ORDINAL>(x)
                        .expect("failed to decode event");
                assert_eq!(x, 55);
                stream.into_future()
            })
            .map(|(x, _stream)| assert!(x.is_none(), "should have emptied"));

        // add a timeout to receiver so if test is broken it doesn't take forever
        let recv = recv.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
            panic!("did not receive event in time!")
        });

        recv.await;
    }

    /// Tests that the event receiver can be taken, the stream read to the end,
    /// the receiver dropped, and then a new receiver gotten from taking the
    /// stream again.
    #[fasync::run_singlethreaded(test)]
    async fn receiver_can_be_taken_after_end_of_stream() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        // Send the event from the server
        let server = AsyncChannel::from_channel(server_end);
        let (bytes, handles) = (&mut vec![], &mut vec![]);
        const ORDINAL: u64 = 5;
        let header = TransactionHeader::new(0, ORDINAL, DynamicFlags::empty());
        encode_transaction(header, bytes, handles);
        server.write_etc(bytes, handles).expect("Server channel write failed");
        drop(server);

        // Create a block to make sure the first event receiver is dropped.
        // Creating the block is a bit of paranoia, because awaiting the
        // future moves the receiver anyway.
        {
            let recv = client
                .take_event_receiver()
                .into_future()
                .then(|(x, stream)| {
                    let x = x.expect("should contain one element");
                    let x = x.expect("fidl error");
                    let x: i32 =
                        decode_transaction_body::<i32, DefaultFuchsiaResourceDialect, ORDINAL>(x)
                            .expect("failed to decode event");
                    assert_eq!(x, 55);
                    stream.into_future()
                })
                .map(|(x, _stream)| assert!(x.is_none(), "should have emptied"));

            // add a timeout to receiver so if test is broken it doesn't take forever
            let recv = recv.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
                panic!("did not receive event in time!")
            });

            recv.await;
        }

        // if we take the event stream again, we should be able to get the next
        // without a panic, but that should be none
        let mut c = client.take_event_receiver();
        assert!(
            c.next().await.is_none(),
            "receiver on closed channel should return none on first call"
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn event_incompatible_format() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        // Send the event from the server
        let server = AsyncChannel::from_channel(server_end);
        let (bytes, handles) = (&mut vec![], &mut vec![]);
        let header = TransactionHeader::new_full(
            0,
            5,
            crate::encoding::Context {
                wire_format_version: crate::encoding::WireFormatVersion::V2,
            },
            DynamicFlags::empty(),
            0,
        );
        encode_transaction(header, bytes, handles);
        server.write_etc(bytes, handles).expect("Server channel write failed");
        drop(server);

        let mut event_receiver = client.take_event_receiver();
        let recv = event_receiver.next().map(|event| {
            assert_matches!(event, Some(Err(crate::Error::IncompatibleMagicNumber(0))))
        });

        // add a timeout to receiver so if test is broken it doesn't take forever
        let recv = recv.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
            panic!("did not receive event in time!")
        });

        recv.await;
    }

    #[test]
    fn client_always_wakes_pending_futures() {
        let mut executor = fasync::TestExecutor::new();

        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let mut event_receiver = client.take_event_receiver();

        // first poll on a response
        let (response_waker, response_waker_count) = new_count_waker();
        let response_cx = &mut Context::from_waker(&response_waker);
        let mut response_txid = Txid(0);
        let mut response_future = client
            .send_raw_query(|tx_id, bytes, handles| {
                response_txid = tx_id;
                let header = TransactionHeader::new(
                    response_txid.as_raw_id(),
                    SEND_ORDINAL,
                    DynamicFlags::empty(),
                );
                encode_transaction(header, bytes, handles);
                Ok(())
            })
            .expect("Couldn't send query");
        assert!(response_future.poll_unpin(response_cx).is_pending());

        // then, poll on an event
        let (event_waker, event_waker_count) = new_count_waker();
        let event_cx = &mut Context::from_waker(&event_waker);
        assert!(event_receiver.poll_next_unpin(event_cx).is_pending());

        // at this point, nothing should have been woken
        assert_eq!(response_waker_count.get(), 0);
        assert_eq!(event_waker_count.get(), 0);

        // next, simulate an event coming in
        send_transaction(TransactionHeader::new(0, 5, DynamicFlags::empty()), &server_end);

        // get event loop to deliver readiness notifications to channels
        let _ = executor.run_until_stalled(&mut future::pending::<()>());

        // The event wake should be woken but not the response_waker.
        assert_eq!(response_waker_count.get(), 0);
        assert_eq!(event_waker_count.get(), 1);

        // we'll pretend event_waker was woken, and have that poll out the event
        assert!(event_receiver.poll_next_unpin(event_cx).is_ready());

        // next, simulate a response coming in
        send_transaction(
            TransactionHeader::new(response_txid.as_raw_id(), SEND_ORDINAL, DynamicFlags::empty()),
            &server_end,
        );

        // get event loop to deliver readiness notifications to channels
        let _ = executor.run_until_stalled(&mut future::pending::<()>());

        // response waker should now get woken.
        assert_eq!(response_waker_count.get(), 1);
    }

    #[test]
    fn client_always_wakes_pending_futures_on_epitaph() {
        let mut executor = fasync::TestExecutor::new();

        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let server_end = AsyncChannel::from_channel(server_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let mut event_receiver = client.take_event_receiver();

        // first poll on a response
        let (response1_waker, response1_waker_count) = new_count_waker();
        let response1_cx = &mut Context::from_waker(&response1_waker);
        let mut response1_future = client
            .send_raw_query(|tx_id, bytes, handles| {
                let header =
                    TransactionHeader::new(tx_id.as_raw_id(), SEND_ORDINAL, DynamicFlags::empty());
                encode_transaction(header, bytes, handles);
                Ok(())
            })
            .expect("Couldn't send query");
        assert!(response1_future.poll_unpin(response1_cx).is_pending());

        // then, poll on an event
        let (event_waker, event_waker_count) = new_count_waker();
        let event_cx = &mut Context::from_waker(&event_waker);
        assert!(event_receiver.poll_next_unpin(event_cx).is_pending());

        // poll on another response
        let (response2_waker, response2_waker_count) = new_count_waker();
        let response2_cx = &mut Context::from_waker(&response2_waker);
        let mut response2_future = client
            .send_raw_query(|tx_id, bytes, handles| {
                let header =
                    TransactionHeader::new(tx_id.as_raw_id(), SEND_ORDINAL, DynamicFlags::empty());
                encode_transaction(header, bytes, handles);
                Ok(())
            })
            .expect("Couldn't send query");
        assert!(response2_future.poll_unpin(response2_cx).is_pending());

        let wakers = vec![response1_waker_count, response2_waker_count, event_waker_count];

        // get event loop to deliver readiness notifications to channels
        let _ = executor.run_until_stalled(&mut future::pending::<()>());

        // at this point, nothing should have been woken
        assert_eq!(0, wakers.iter().fold(0, |acc, x| acc + x.get()));

        // next, simulate an epitaph without closing
        epitaph::write_epitaph_impl(&server_end, Err(zx_status::Status::UNAVAILABLE))
            .expect("wrote epitaph");

        // get event loop to deliver readiness notifications to channels
        let _ = executor.run_until_stalled(&mut future::pending::<()>());

        // All the wakers should be woken up because the channel is ready to read, and the message
        // could be for any of them.
        for wake_count in &wakers {
            assert_eq!(wake_count.get(), 1);
        }

        // pretend that response1 woke and poll that to completion.
        assert_matches!(
            response1_future.poll_unpin(response1_cx),
            Poll::Ready(Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                protocol_name: "test_protocol",
            }))
        );

        // get event loop to deliver readiness notifications to channels
        let _ = executor.run_until_stalled(&mut future::pending::<()>());

        // poll response2 to completion.
        assert_matches!(
            response2_future.poll_unpin(response2_cx),
            Poll::Ready(Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                protocol_name: "test_protocol",
            }))
        );

        // poll the event stream to completion.
        assert!(event_receiver.poll_next_unpin(event_cx).is_ready());
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_allows_take_event_stream_even_if_event_delivered() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        // first simulate an event coming in, even though nothing has polled
        send_transaction(TransactionHeader::new(0, 5, DynamicFlags::empty()), &server_end);

        // next, poll on a response
        let (response_waker, _response_waker_count) = new_count_waker();
        let response_cx = &mut Context::from_waker(&response_waker);
        let mut response_future =
            client.send_query::<u8, u8, SEND_ORDINAL>(55, DynamicFlags::empty());
        assert!(response_future.poll_unpin(response_cx).is_pending());

        // then, make sure we can still take the event receiver without panicking
        let mut _event_receiver = client.take_event_receiver();
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_reports_epitaph_from_all_read_actions() {
        #[derive(Debug, PartialEq)]
        enum Action {
            SendMsg,   // send a one-way message
            SendQuery, // send a two-way message and just call .check()
            WaitQuery, // send a two-way message and wait for the response
            RecvEvent, // wait to receive an event
        }
        impl Action {
            fn should_report_epitaph(&self) -> bool {
                match self {
                    Action::SendMsg | Action::SendQuery => false,
                    Action::WaitQuery | Action::RecvEvent => true,
                }
            }
        }
        use Action::*;
        // Test all permutations of two actions. Verify the epitaph is reported
        // twice (2 reads), once (1 read, 1 write), or not at all (2 writes).
        for two_actions in &[
            [SendMsg, SendMsg],
            [SendMsg, SendQuery],
            [SendMsg, WaitQuery],
            [SendMsg, RecvEvent],
            [SendQuery, SendMsg],
            [SendQuery, SendQuery],
            [SendQuery, WaitQuery],
            [SendQuery, RecvEvent],
            [WaitQuery, SendMsg],
            [WaitQuery, SendQuery],
            [WaitQuery, WaitQuery],
            [WaitQuery, RecvEvent],
            [RecvEvent, SendMsg],
            [RecvEvent, SendQuery],
            [RecvEvent, WaitQuery],
            // No [RecvEvent, RecvEvent] because it behaves differently: after
            // reporting an epitaph, the next call returns None.
        ] {
            let (client_end, server_end) = zx::Channel::create();
            let client_end = AsyncChannel::from_channel(client_end);
            let client = Client::new(client_end, "test_protocol");

            // Immediately close the FIDL channel with an epitaph.
            let server_end = AsyncChannel::from_channel(server_end);
            server_end
                .close_with_epitaph(zx_status::Status::UNAVAILABLE)
                .expect("failed to write epitaph");

            let mut event_receiver = client.take_event_receiver();

            // Assert that each action reports the epitaph.
            for (index, action) in two_actions.iter().enumerate() {
                let err = match action {
                    SendMsg => {
                        client.send::<u8>(SEND_DATA, SEND_ORDINAL, DynamicFlags::empty()).err()
                    }
                    WaitQuery => client
                        .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
                        .await
                        .err(),
                    SendQuery => client
                        .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
                        .check()
                        .err(),
                    RecvEvent => event_receiver.next().await.unwrap().err(),
                };
                let details = format!("index: {index:?}, two_actions: {two_actions:?}");
                match err {
                    None => assert!(
                        !action.should_report_epitaph(),
                        "expected epitaph, but succeeded.\n{details}"
                    ),
                    Some(crate::Error::ClientChannelClosed {
                        epitaph:
                            crate::error::Epitaph::Explicit(Err(zx_status::Status::UNAVAILABLE)),
                        protocol_name: "test_protocol",
                    }) => assert!(
                        action.should_report_epitaph(),
                        "got epitaph unexpectedly.\n{details}",
                    ),
                    Some(err) => panic!("unexpected error: {err:#?}.\n{details}"),
                }
            }

            // If we got the epitaph from RecvEvent, the next should return None.
            if two_actions.contains(&RecvEvent) {
                assert_matches!(event_receiver.next().await, None);
            }
        }
    }

    #[test]
    fn client_query_result_check() {
        let mut executor = fasync::TestExecutor::new();
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);

        // Sending works, and checking when a message successfully sends returns itself.
        let active_fut =
            client.send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty());

        let mut checked_fut = active_fut.check().expect("failed to check future");

        // Should be able to complete the query even after checking.
        let mut buffer = MessageBufEtc::new();
        executor.run_singlethreaded(server.recv_etc_msg(&mut buffer)).expect("failed to recv msg");
        let two_way_tx_id = 1u8;
        assert_eq!(buffer.bytes(), expected_sent_bytes(two_way_tx_id, 1));

        let (bytes, handles) = (&mut vec![], &mut vec![]);
        let header =
            TransactionHeader::new(two_way_tx_id as u32, SEND_ORDINAL, DynamicFlags::empty());
        encode_transaction(header, bytes, handles);
        server.write_etc(bytes, handles).expect("Server channel write failed");

        executor
            .run_singlethreaded(&mut checked_fut)
            .map(|x| assert_eq!(x, SEND_DATA))
            .unwrap_or_else(|e| panic!("fidl error: {e:?}"));

        // Close the server channel, meaning the next query will fail.
        drop(server);

        let query_fut = client.send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty());

        // The check succeeds, because we do not expose PEER_CLOSED on writes.
        let mut checked_fut = query_fut.check().expect("failed to check future");
        // But the query will fail when it tries to read the response.
        assert_matches!(
            executor.run_singlethreaded(&mut checked_fut),
            Err(crate::Error::ClientChannelClosed {
                epitaph: crate::error::Epitaph::PeerClosed,
                protocol_name: "test_protocol",
            })
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_into_channel() {
        // This test doesn't actually do any async work, but the fuchsia
        // executor must be set up in order to create the channel.
        let (client_end, _server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        assert!(client.into_channel().is_ok());
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_into_channel_outstanding_messages() {
        // This test doesn't actually do any async work, but the fuchsia
        // executor must be set up in order to create the channel.
        let (client_end, _server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        {
            // Create a send future to insert a message interest but drop it
            // before a response can be received.
            let _sender =
                client.send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty());
        }

        assert!(client.into_channel().is_err());
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_into_channel_active_clone() {
        // This test doesn't actually do any async work, but the fuchsia
        // executor must be set up in order to create the channel.
        let (client_end, _server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let _cloned_client = client.clone();

        assert!(client.into_channel().is_err());
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_into_channel_outstanding_messages_get_received() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);
        let mut buffer = MessageBufEtc::new();
        let receiver = async move {
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            let two_way_tx_id = 1u8;
            assert_eq!(buffer.bytes(), expected_sent_bytes(two_way_tx_id, 1));

            let (bytes, handles) = (&mut vec![], &mut vec![]);
            let header =
                TransactionHeader::new(two_way_tx_id as u32, SEND_ORDINAL, DynamicFlags::empty());
            encode_transaction(header, bytes, handles);
            server.write_etc(bytes, handles).expect("Server channel write failed");
        };

        // add a timeout to receiver so if test is broken it doesn't take forever
        let receiver = receiver
            .on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
                panic!("did not receiver message in time!")
            });

        let sender = client
            .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
            .map_ok(|x| assert_eq!(x, SEND_DATA))
            .unwrap_or_else(|e| panic!("fidl error: {e:?}"));

        // add a timeout to receiver so if test is broken it doesn't take forever
        let sender = sender.on_timeout(zx::MonotonicDuration::from_millis(300).after_now(), || {
            panic!("did not receive response in time!")
        });

        let ((), ()) = join!(receiver, sender);

        assert!(client.into_channel().is_ok());
    }

    #[fasync::run_singlethreaded(test)]
    async fn client_decode_errors_are_broadcast() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);

        let _server = fasync::Task::spawn(async move {
            let mut buffer = MessageBufEtc::new();
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            let two_way_tx_id = 1u8;
            assert_eq!(buffer.bytes(), expected_sent_bytes(two_way_tx_id, 1));

            let (bytes, handles) = (&mut vec![], &mut vec![]);
            let header =
                TransactionHeader::new(two_way_tx_id as u32, SEND_ORDINAL, DynamicFlags::empty());
            encode_transaction(header, bytes, handles);
            // Zero out the at-rest flags which will give this message an invalid version.
            bytes[4] = 0;
            server.write_etc(bytes, handles).expect("Server channel write failed");

            // Wait forever to stop the channel from being closed.
            pending::<()>().await;
        });

        let futures = FuturesUnordered::new();

        for _ in 0..4 {
            futures.push(async {
                assert_matches!(
                    client
                        .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
                        .map_ok(|x| assert_eq!(x, SEND_DATA))
                        .await,
                    Err(crate::Error::UnsupportedWireFormatVersion)
                );
            });
        }

        futures
            .collect::<Vec<_>>()
            .on_timeout(zx::MonotonicDuration::from_seconds(1).after_now(), || panic!("timed out!"))
            .await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn into_channel_from_waker_succeeds() {
        let (client_end, server_end) = zx::Channel::create();
        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "test_protocol");

        let server = AsyncChannel::from_channel(server_end);
        let mut buffer = MessageBufEtc::new();
        let receiver = async move {
            server.recv_etc_msg(&mut buffer).await.expect("failed to recv msg");
            let two_way_tx_id = 1u8;
            assert_eq!(buffer.bytes(), expected_sent_bytes(two_way_tx_id, 1));

            let (bytes, handles) = (&mut vec![], &mut vec![]);
            let header =
                TransactionHeader::new(two_way_tx_id as u32, SEND_ORDINAL, DynamicFlags::empty());
            encode_transaction(header, bytes, handles);
            server.write_etc(bytes, handles).expect("Server channel write failed");
        };

        struct Sender {
            future: Mutex<Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>>,
        }

        let (done_tx, done_rx) = oneshot::channel();

        let sender = Arc::new(Sender {
            future: Mutex::new(Box::pin(async move {
                client
                    .send_query::<u8, u8, SEND_ORDINAL>(SEND_DATA, DynamicFlags::empty())
                    .map_ok(|x| assert_eq!(x, SEND_DATA))
                    .unwrap_or_else(|e| panic!("fidl error: {e:?}"))
                    .await;

                assert!(client.into_channel().is_ok());

                let _ = done_tx.send(());
            })),
        });

        // This test isn't typically how this would work; normally, the future would get woken and
        // an executor would be responsible for running the task.  We do it this way because if this
        // works, then it means the case where `into_channel` is used after a response is received
        // on a multi-threaded executor will always work (which isn't easy to test directly).
        impl Wake for Sender {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                assert!(
                    self.future
                        .lock()
                        .poll_unpin(&mut Context::from_waker(Waker::noop()))
                        .is_ready()
                );
            }
        }

        let waker = Waker::from(sender.clone());

        assert!(sender.future.lock().poll_unpin(&mut Context::from_waker(&waker)).is_pending());

        receiver.await;

        done_rx.await.unwrap();
    }
}
