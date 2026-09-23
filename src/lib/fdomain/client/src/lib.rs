// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_fdomain as proto;
use fidl_message::TransactionHeader;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use futures::channel::oneshot::Sender as OneshotSender;
use futures::stream::Stream as StreamTrait;
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Weak};
use std::task::{Context, Poll, Waker, ready};

mod channel;
mod event;
mod event_pair;
mod handle;
mod responder;
mod socket;
mod vmo;

#[cfg(test)]
mod test;

pub mod fidl;
pub mod fidl_next;

use responder::Responder;

pub use channel::{
    AnyHandle, Channel, ChannelMessageStream, ChannelWriter, HandleInfo, HandleOp, MessageBuf,
};
pub use event::Event;
pub use event_pair::Eventpair as EventPair;
pub use handle::unowned::Unowned;
pub use handle::{
    AsHandleRef, Handle, HandleBased, HandleRef, NullableHandle, OnFDomainSignals, Peered,
};
pub use proto::{Error as FDomainError, VmoOptions, WriteChannelError, WriteSocketError};
pub use socket::{Socket, SocketDisposition, SocketReadStream, SocketWriter};
pub use vmo::Vmo;

// Unsupported handle types.
#[rustfmt::skip]
pub use Handle as Clock;
#[rustfmt::skip]
pub use Handle as Exception;
#[rustfmt::skip]
pub use Handle as Fifo;
#[rustfmt::skip]
pub use Handle as Iob;
#[rustfmt::skip]
pub use Handle as Job;
#[rustfmt::skip]
pub use Handle as Process;
#[rustfmt::skip]
pub use Handle as Resource;
#[rustfmt::skip]
pub use Handle as Stream;
#[rustfmt::skip]
pub use Handle as Thread;
#[rustfmt::skip]
pub use Handle as Vmar;
#[rustfmt::skip]
pub use Handle as Counter;
#[rustfmt::skip]
pub use Handle as Port;

use proto::f_domain_ordinals as ordinals;

fn write_fdomain_error(error: &FDomainError, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match error {
        FDomainError::TargetError(e) => {
            let e = zx_status::Status::err_from_raw(*e);
            write!(f, "Target-side error {e}")
        }
        FDomainError::BadHandleId(proto::BadHandleId { id }) => {
            write!(f, "Tried to use invalid handle id {id}")
        }
        FDomainError::WrongHandleType(proto::WrongHandleType { expected, got }) => write!(
            f,
            "Tried to use handle as {expected:?} but target reported handle was of type {got:?}"
        ),
        FDomainError::StreamingReadInProgress(proto::StreamingReadInProgress {}) => {
            write!(f, "Handle is occupied delivering streaming reads")
        }
        FDomainError::NoReadInProgress(proto::NoReadInProgress {}) => {
            write!(f, "No streaming read was in progress")
        }
        FDomainError::NewHandleIdOutOfRange(proto::NewHandleIdOutOfRange { id }) => {
            write!(
                f,
                "Tried to create a handle with id {id}, which is outside the valid range for client handles"
            )
        }
        FDomainError::NewHandleIdReused(proto::NewHandleIdReused { id, same_call }) => {
            if *same_call {
                write!(f, "Tried to create two or more new handles with the same id {id}")
            } else {
                write!(
                    f,
                    "Tried to create a new handle with id {id}, which is already the id of an existing handle"
                )
            }
        }
        FDomainError::WroteToSelf(proto::WroteToSelf {}) => {
            write!(f, "Tried to write a channel into itself")
        }
        FDomainError::ClosedDuringRead(proto::ClosedDuringRead {}) => {
            write!(f, "Handle closed while being read")
        }
        FDomainError::SignalsUnknown(signals_unknown) => {
            write!(f, "Unknown signals: {:x}", signals_unknown.signals)
        }
        FDomainError::RightsUnknown(rights_unknown) => {
            write!(f, "Unknown rights: {:x}", rights_unknown.rights)
        }
        FDomainError::SocketDispositionUnknown(socket_disposition_unknown) => {
            write!(f, "Unknown socket disposition: {:?}", socket_disposition_unknown.disposition)
        }
        FDomainError::SocketTypeUnknown(socket_type_unknown) => {
            write!(f, "Unknown socket type: {:?}", socket_type_unknown.type_)
        }
        e => write!(f, "Unknown FDomain error: {e:?}"),
    }
}

/// Result type alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Error type emitted by FDomain operations.
#[derive(Clone)]
pub enum Error {
    SocketWrite(WriteSocketError),
    ChannelWrite(WriteChannelError),
    FDomain(FDomainError),
    Protocol(::fidl::Error),
    ProtocolObjectTypeIncompatible,
    ProtocolRightsIncompatible,
    ProtocolSignalsIncompatible,
    ProtocolStreamEventIncompatible,
    Transport(Option<Arc<std::io::Error>>),
    ConnectionMismatch,
    StreamingAborted,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SocketWrite(proto::WriteSocketError { error, wrote }) => {
                write!(f, "While writing socket (after {wrote} bytes written successfully): ")?;
                write_fdomain_error(error, f)
            }
            Self::ChannelWrite(proto::WriteChannelError::Error(error)) => {
                write!(f, "While writing channel: ")?;
                write_fdomain_error(error, f)
            }
            Self::ChannelWrite(proto::WriteChannelError::OpErrors(errors)) => {
                write!(f, "Couldn't write all handles into a channel:")?;
                for (pos, error) in
                    errors.iter().enumerate().filter_map(|(num, x)| x.as_ref().map(|y| (num, &**y)))
                {
                    write!(f, "\n  Handle in position {pos}: ")?;
                    write_fdomain_error(error, f)?;
                }
                Ok(())
            }
            Self::ProtocolObjectTypeIncompatible => {
                write!(
                    f,
                    "The FDomain protocol received an unrecognized or incompatible object type"
                )
            }
            Self::ProtocolRightsIncompatible => {
                write!(
                    f,
                    "The FDomain protocol received unrecognized or incompatible handle rights"
                )
            }
            Self::ProtocolSignalsIncompatible => {
                write!(f, "The FDomain protocol received unrecognized or incompatible signals")
            }
            Self::ProtocolStreamEventIncompatible => {
                write!(
                    f,
                    "The FDomain protocol received an unrecognized or incompatible streaming IO event"
                )
            }
            Self::FDomain(e) => write_fdomain_error(e, f),
            Self::Protocol(e) => write!(f, "Protocol error: {e}"),
            Self::Transport(Some(e)) => write!(f, "Transport error: {e}"),
            Self::Transport(None) => {
                write!(f, "Transport error: Connection to the device has been lost")
            }
            Self::ConnectionMismatch => {
                write!(
                    f,
                    "Tried to use an FDomain handle with a different connection than the one it was created on"
                )
            }
            Self::StreamingAborted => write!(f, "Streaming on this channel has been aborted"),
        }
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SocketWrite(e) => f.debug_tuple("SocketWrite").field(e).finish(),
            Self::ChannelWrite(e) => f.debug_tuple("ChannelWrite").field(e).finish(),
            Self::FDomain(e) => f.debug_tuple("FDomain").field(e).finish(),
            Self::Protocol(e) => f.debug_tuple("Protocol").field(e).finish(),
            Self::Transport(e) => f.debug_tuple("Transport").field(e).finish(),
            Self::ProtocolObjectTypeIncompatible => write!(f, "ProtocolObjectTypeIncompatible "),
            Self::ProtocolRightsIncompatible => write!(f, "ProtocolRightsIncompatible "),
            Self::ProtocolSignalsIncompatible => write!(f, "ProtocolSignalsIncompatible "),
            Self::ProtocolStreamEventIncompatible => write!(f, "ProtocolStreamEventIncompatible"),
            Self::ConnectionMismatch => write!(f, "ConnectionMismatch"),
            Self::StreamingAborted => write!(f, "StreamingAborted"),
        }
    }
}

impl std::error::Error for Error {}

impl From<FDomainError> for Error {
    fn from(other: FDomainError) -> Self {
        Self::FDomain(other)
    }
}

impl From<::fidl::Error> for Error {
    fn from(other: ::fidl::Error) -> Self {
        Self::Protocol(other)
    }
}

impl From<WriteSocketError> for Error {
    fn from(other: WriteSocketError) -> Self {
        Self::SocketWrite(other)
    }
}

impl From<WriteChannelError> for Error {
    fn from(other: WriteChannelError) -> Self {
        Self::ChannelWrite(other)
    }
}

/// An error emitted internally by the client. Similar to [`Error`] but does not
/// contain several variants which are irrelevant in the contexts where it is
/// used.
#[derive(Clone)]
enum InnerError {
    Protocol(::fidl::Error),
    ProtocolStreamEventIncompatible,
    Transport(Option<Arc<std::io::Error>>),
}

impl From<InnerError> for Error {
    fn from(other: InnerError) -> Self {
        match other {
            InnerError::Protocol(p) => Error::Protocol(p),
            InnerError::ProtocolStreamEventIncompatible => Error::ProtocolStreamEventIncompatible,
            InnerError::Transport(t) => Error::Transport(t),
        }
    }
}

impl From<::fidl::Error> for InnerError {
    fn from(other: ::fidl::Error) -> Self {
        InnerError::Protocol(other)
    }
}

// TODO(399717689) Figure out if we could just use AsyncRead/Write instead of a special trait.
/// Implemented by objects which provide a transport over which we can speak the
/// FDomain protocol.
///
/// The implementer must provide two things:
/// 1) An incoming stream of messages presented as `Vec<u8>`. This is provided
///    via the `Stream` trait, which this trait requires.
/// 2) A way to send messages. This is provided by implementing the
///    `poll_send_message` method.
pub trait FDomainTransport: StreamTrait<Item = Result<Box<[u8]>, std::io::Error>> + Send {
    /// Attempt to send a message asynchronously. Messages should be sent so
    /// that they arrive at the target in order.
    fn poll_send_message(
        self: Pin<&mut Self>,
        msg: &[u8],
        ctx: &mut Context<'_>,
    ) -> Poll<Result<(), Option<std::io::Error>>>;

    /// Optional debug information outlet.
    fn debug_fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }

    /// Whether `debug_fmt` does anything.
    fn has_debug_fmt(&self) -> bool {
        false
    }
}

/// Wrapper for an `FDomainTransport` implementer that:
/// 1) Provides a queue for outgoing messages so we need not have an await point
///    when we submit a message.
/// 2) Drops the transport on error, then returns the last observed error for
///    all future operations.
enum Transport {
    Transport(Pin<Box<dyn FDomainTransport>>, VecDeque<Box<[u8]>>, Vec<Waker>),
    Error(InnerError),
}

impl Transport {
    /// Get the failure mode of the transport if it has failed.
    fn error(&self) -> Option<InnerError> {
        match self {
            Transport::Transport(_, _, _) => None,
            Transport::Error(inner_error) => Some(inner_error.clone()),
        }
    }

    /// Enqueue a message to be sent on this transport.
    fn push_msg(&mut self, msg: Box<[u8]>) -> Result<(), InnerError> {
        match self {
            Transport::Transport(_, v, w) => {
                v.push_back(msg);
                w.drain(..).for_each(Waker::wake);
                Ok(())
            }
            Transport::Error(e) => Err(e.clone()),
        }
    }

    /// Push messages in the send queue out through the transport.
    fn poll_send_messages(&mut self, ctx: &mut Context<'_>) -> Poll<InnerError> {
        match self {
            Transport::Error(e) => Poll::Ready(e.clone()),
            Transport::Transport(t, v, w) => {
                while let Some(msg) = v.front() {
                    match t.as_mut().poll_send_message(msg, ctx) {
                        Poll::Ready(Ok(())) => {
                            v.pop_front();
                        }
                        Poll::Ready(Err(e)) => {
                            let e = e.map(Arc::new);
                            return Poll::Ready(InnerError::Transport(e));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }

                if v.is_empty() {
                    w.push(ctx.waker().clone());
                } else {
                    ctx.waker().wake_by_ref();
                }
                Poll::Pending
            }
        }
    }

    /// Get the next incoming message from the transport.
    fn poll_next(&mut self, ctx: &mut Context<'_>) -> Poll<Result<Box<[u8]>, InnerError>> {
        match self {
            Transport::Error(e) => Poll::Ready(Err(e.clone())),
            Transport::Transport(t, _, _) => match ready!(t.as_mut().poll_next(ctx)) {
                Some(Ok(x)) => Poll::Ready(Ok(x)),
                Some(Err(e)) => Poll::Ready(Err(InnerError::Transport(Some(Arc::new(e))))),
                Option::None => Poll::Ready(Err(InnerError::Transport(None))),
            },
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        if let Transport::Transport(_, _, wakers) = self {
            wakers.drain(..).for_each(Waker::wake);
        }
    }
}

/// State of a socket that is or has been read from.
struct SocketReadState {
    wakers: Vec<Waker>,
    queued: VecDeque<Result<proto::SocketData, Error>>,
    read_request_pending: bool,
    is_streaming: bool,
}

impl SocketReadState {
    /// Handle an incoming message, which is either a channel streaming event or
    /// response to a `ChannelRead` request.
    fn handle_incoming_message(&mut self, msg: Result<proto::SocketData, Error>) -> Vec<Waker> {
        self.queued.push_back(msg);
        std::mem::replace(&mut self.wakers, Vec::new())
    }
}

/// State of a channel that is or has been read from.
struct ChannelReadState {
    wakers: Vec<Waker>,
    queued: VecDeque<Result<proto::ChannelMessage, Error>>,
    read_request_pending: bool,
    is_streaming: bool,
}

impl ChannelReadState {
    /// Handle an incoming message, which is either a channel streaming event or
    /// response to a `ChannelRead` request.
    fn handle_incoming_message(&mut self, msg: Result<proto::ChannelMessage, Error>) -> Vec<Waker> {
        self.queued.push_back(msg);
        std::mem::replace(&mut self.wakers, Vec::new())
    }
}

/// Lock-protected interior of `Client`
struct ClientInner {
    transport: Transport,
    transactions: HashMap<NonZeroU32, responder::Responder>,
    channel_read_states: HashMap<proto::HandleId, ChannelReadState>,
    socket_read_states: HashMap<proto::HandleId, SocketReadState>,
    handles: HashSet<proto::HandleId>,
    next_tx_id: u32,
    waiting_to_close: Vec<proto::HandleId>,
    waiting_to_close_waker: Waker,

    /// There is a lock around `ClientInner`, and sometimes the FIDL bindings
    /// give us wakers that want to do handle operations synchronously on wake,
    /// which means we can double-take the lock if we wake a waker while we hold
    /// it. This is a place to store wakers that we'd like to be woken as soon
    /// as we're not holding that lock, to avoid these weird reentrancy issues.
    wakers_to_wake: Vec<Waker>,
}

impl ClientInner {
    /// Allocate a new HID, which should be suitable for use with the connected FDomain.
    fn new_hid(&mut self) -> proto::NewHandleId {
        self.process_waiting_to_close();
        loop {
            let id = rand::random::<u32>() >> 1;
            let hid = proto::HandleId { id };
            if id != 0 && self.handles.insert(hid) {
                return proto::NewHandleId { id };
            }
        }
    }

    /// Serialize and enqueue a new transaction, including header and transaction ID.
    fn request<S: fidl_message::Body>(&mut self, ordinal: u64, request: S, responder: Responder) {
        if ordinal != ordinals::CLOSE {
            self.process_waiting_to_close();
        }
        let tx_id = self.next_tx_id;

        let header = TransactionHeader::new(tx_id, ordinal, fidl_message::DynamicFlags::FLEXIBLE);
        let msg = fidl_message::encode_message(header, request).expect("Could not encode request!");
        self.next_tx_id += 1;
        if let Err(e) = self.transport.push_msg(msg.into()) {
            let _ = responder.handle(self, Err(e.into()));
        } else {
            assert!(
                self.transactions.insert(tx_id.try_into().unwrap(), responder).is_none(),
                "Allocated same tx id twice!"
            );
        }
    }

    fn process_waiting_to_close(&mut self) {
        if !self.waiting_to_close.is_empty() {
            let handles = std::mem::replace(&mut self.waiting_to_close, Vec::new());
            // We've dropped the handle object. Nobody is going to wait to read
            // the buffers anymore. This is a safe time to drop the read state.
            for handle in &handles {
                let _ = self.channel_read_states.remove(handle);
                let _ = self.socket_read_states.remove(handle);
                self.handles.remove(handle);
            }
            self.request(
                ordinals::CLOSE,
                proto::FDomainCloseRequest { handles },
                Responder::Ignore,
            );
        }
    }

    /// Polls the underlying transport to ensure any incoming or outgoing
    /// messages are processed as far as possible. Errors if the transport has failed.
    fn try_poll_transport(
        &mut self,
        ctx: &mut Context<'_>,
    ) -> Poll<Result<Infallible, InnerError>> {
        self.process_waiting_to_close();

        self.waiting_to_close_waker = ctx.waker().clone();

        loop {
            if let Poll::Ready(e) = self.transport.poll_send_messages(ctx) {
                return Poll::Ready(Err(e));
            }
            let Poll::Ready(result) = self.transport.poll_next(ctx) else {
                return Poll::Pending;
            };
            let data = result?;
            let (header, data) = fidl_message::decode_transaction_header(&data)?;

            let Some(tx_id) = NonZeroU32::new(header.tx_id) else {
                let wakers = self.process_event(header, data)?;
                self.wakers_to_wake.extend(wakers);
                continue;
            };

            let tx = self.transactions.remove(&tx_id).ok_or(::fidl::Error::InvalidResponseTxid)?;
            tx.handle(self, Ok((header, data)))?;
        }
    }

    /// Process an incoming message that arose from an event rather than a transaction reply.
    fn process_event(
        &mut self,
        header: TransactionHeader,
        data: &[u8],
    ) -> Result<Vec<Waker>, InnerError> {
        match header.ordinal {
            ordinals::ON_SOCKET_STREAMING_DATA => {
                let msg = fidl_message::decode_message::<proto::SocketOnSocketStreamingDataRequest>(
                    header, data,
                )?;
                let o =
                    self.socket_read_states.entry(msg.handle).or_insert_with(|| SocketReadState {
                        wakers: Vec::new(),
                        queued: VecDeque::new(),
                        is_streaming: false,
                        read_request_pending: false,
                    });
                match msg.socket_message {
                    proto::SocketMessage::Data(data) => Ok(o.handle_incoming_message(Ok(data))),
                    proto::SocketMessage::Stopped(proto::AioStopped { error }) => {
                        let ret = if let Some(error) = error {
                            o.handle_incoming_message(Err(Error::FDomain(*error)))
                        } else {
                            Vec::new()
                        };
                        o.is_streaming = false;
                        Ok(ret)
                    }
                    _ => Err(InnerError::ProtocolStreamEventIncompatible),
                }
            }
            ordinals::ON_CHANNEL_STREAMING_DATA => {
                let msg = fidl_message::decode_message::<
                    proto::ChannelOnChannelStreamingDataRequest,
                >(header, data)?;
                let o = self.channel_read_states.entry(msg.handle).or_insert_with(|| {
                    ChannelReadState {
                        wakers: Vec::new(),
                        queued: VecDeque::new(),
                        is_streaming: false,
                        read_request_pending: false,
                    }
                });
                match msg.channel_sent {
                    proto::ChannelSent::Message(data) => Ok(o.handle_incoming_message(Ok(data))),
                    proto::ChannelSent::Stopped(proto::AioStopped { error }) => {
                        let ret = if let Some(error) = error {
                            o.handle_incoming_message(Err(Error::FDomain(*error)))
                        } else {
                            Vec::new()
                        };
                        o.is_streaming = false;
                        Ok(ret)
                    }
                    _ => Err(InnerError::ProtocolStreamEventIncompatible),
                }
            }
            _ => Err(::fidl::Error::UnknownOrdinal {
                ordinal: header.ordinal,
                protocol_name:
                    <proto::FDomainMarker as ::fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
            }
            .into()),
        }
    }

    /// Polls the underlying transport to ensure any incoming or outgoing
    /// messages are processed as far as possible. If a failure occurs, puts the
    /// transport into an error state and fails all pending transactions.
    fn poll_transport(&mut self, ctx: &mut Context<'_>) -> Poll<()> {
        if let Poll::Ready(Err(e)) = self.try_poll_transport(ctx) {
            for (_, v) in std::mem::take(&mut self.transactions) {
                let _ = v.handle(self, Err(e.clone()));
            }
            for mut state in std::mem::take(&mut self.socket_read_states).into_values() {
                state.queued.push_back(Err(Error::from(e.clone())));
                self.wakers_to_wake.extend(state.wakers);
            }
            for (_, mut state) in self.channel_read_states.drain() {
                state.queued.push_back(Err(Error::from(e.clone())));
                self.wakers_to_wake.extend(state.wakers);
            }
            if matches!(self.transport, Transport::Transport(_, _, _)) {
                self.transport = Transport::Error(e);
            }

            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Handles the response to a `SocketRead` protocol message.
    pub(crate) fn handle_socket_read_response(
        &mut self,
        msg: Result<proto::SocketData, Error>,
        id: proto::HandleId,
    ) {
        let state = self.socket_read_states.entry(id).or_insert_with(|| SocketReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });
        let wakers = state.handle_incoming_message(msg);
        self.wakers_to_wake.extend(wakers);
        state.read_request_pending = false;
    }

    /// Handles the response to a `ChannelRead` protocol message.
    pub(crate) fn handle_channel_read_response(
        &mut self,
        msg: Result<proto::ChannelMessage, Error>,
        id: proto::HandleId,
    ) {
        let state = self.channel_read_states.entry(id).or_insert_with(|| ChannelReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });
        let wakers = state.handle_incoming_message(msg);
        self.wakers_to_wake.extend(wakers);
        state.read_request_pending = false;
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        let responders = self.transactions.drain().map(|x| x.1).collect::<Vec<_>>();
        for responder in responders {
            let _ = responder.handle(self, Err(InnerError::Transport(None)));
        }
        for state in self.channel_read_states.values_mut() {
            state.wakers.drain(..).for_each(Waker::wake);
        }
        for state in self.socket_read_states.values_mut() {
            state.wakers.drain(..).for_each(Waker::wake);
        }
        self.waiting_to_close_waker.wake_by_ref();
        self.wakers_to_wake.drain(..).for_each(Waker::wake);
    }
}

/// Represents a connection to an FDomain.
///
/// The client is constructed by passing it a transport object which represents
/// the raw connection to the remote FDomain. The `Client` wrapper then allows
/// us to construct and use handles which behave similarly to their counterparts
/// on a Fuchsia device.
pub struct Client(pub(crate) Mutex<ClientInner>);

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.0.lock();
        match &inner.transport {
            Transport::Transport(transport, ..) if transport.has_debug_fmt() => {
                write!(f, "Client(")?;
                transport.debug_fmt(f)?;
                write!(f, ")")
            }
            Transport::Error(error) => {
                let error = Error::from(error.clone());
                write!(f, "Client(Failed: {error})")
            }
            _ => f.debug_tuple("Client").field(&"<transport>").finish(),
        }
    }
}

/// A client which is always disconnected. Handles that lose their clients
/// connect to this client instead, which always returns a "Client Lost"
/// transport failure.
pub(crate) static DEAD_CLIENT: LazyLock<Arc<Client>> = LazyLock::new(|| {
    Arc::new(Client(Mutex::new(ClientInner {
        transport: Transport::Error(InnerError::Transport(None)),
        transactions: HashMap::new(),
        channel_read_states: HashMap::new(),
        socket_read_states: HashMap::new(),
        handles: HashSet::new(),
        next_tx_id: 1,
        waiting_to_close: Vec::new(),
        waiting_to_close_waker: std::task::Waker::noop().clone(),
        wakers_to_wake: Vec::new(),
    })))
});

/// A wrapper around the FDomain client background future that ensures
/// all pending transactions and reads are failed if the loop is dropped.
///
/// This prevents hangs when the transport is abruptly closed (e.g. during target reboot)
/// by waking up any futures waiting for responses or data on channels/sockets.
pub struct ClientLoop {
    client: Weak<Client>,
    fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}

impl Future for ClientLoop {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.fut.as_mut().poll(cx)
    }
}

impl Drop for ClientLoop {
    fn drop(&mut self) {
        let Some(client) = self.client.upgrade() else {
            return;
        };

        let (channel_read_states, socket_read_states, deferred_wakers) = {
            let mut inner = client.0.lock();
            if matches!(inner.transport, Transport::Transport(_, _, _)) {
                inner.transport = Transport::Error(InnerError::Transport(None));
            }

            let transactions = std::mem::take(&mut inner.transactions);
            log::debug!("ClientLoop dropped, failing {} transactions", transactions.len());
            for (_, v) in transactions {
                let _ = v.handle(&mut *inner, Err(InnerError::Transport(None)));
            }

            let channel_read_states = std::mem::take(&mut inner.channel_read_states);
            let socket_read_states = std::mem::take(&mut inner.socket_read_states);

            let deferred_wakers = std::mem::replace(&mut inner.wakers_to_wake, Vec::new());

            (channel_read_states, socket_read_states, deferred_wakers)
        };

        log::debug!("Failing reads on {} channels", channel_read_states.len());
        for (_, mut state) in channel_read_states {
            state.queued.push_back(Err(Error::Transport(None)));
            state.wakers.into_iter().for_each(Waker::wake);
        }

        log::debug!("Failing reads on {} sockets", socket_read_states.len());
        for (_, mut state) in socket_read_states {
            state.queued.push_back(Err(Error::Transport(None)));
            state.wakers.into_iter().for_each(Waker::wake);
        }

        deferred_wakers.into_iter().for_each(Waker::wake);
    }
}

impl Client {
    pub fn transport_status(&self) -> Result<()> {
        match &self.0.lock().transport {
            Transport::Error(e) => Err(e.clone().into()),
            Transport::Transport(_, _, _) => Ok(()),
        }
    }
    /// Create a new FDomain client. The `transport` argument should contain the
    /// established connection to the target, ready to communicate the FDomain
    /// protocol.
    ///
    /// The second return item is a future that must be polled to keep
    /// transactions running.
    pub fn new(
        transport: impl FDomainTransport + 'static,
    ) -> (Arc<Self>, impl Future<Output = ()> + Send + 'static) {
        let ret = Arc::new(Client(Mutex::new(ClientInner {
            transport: Transport::Transport(Box::pin(transport), VecDeque::new(), Vec::new()),
            transactions: HashMap::new(),
            socket_read_states: HashMap::new(),
            channel_read_states: HashMap::new(),
            handles: HashSet::new(),
            next_tx_id: 1,
            waiting_to_close: Vec::new(),
            waiting_to_close_waker: std::task::Waker::noop().clone(),
            wakers_to_wake: Vec::new(),
        })));

        let client_weak = Arc::downgrade(&ret);
        let fut = futures::future::poll_fn(move |ctx| {
            let Some(client) = client_weak.upgrade() else {
                return Poll::Ready(());
            };

            let (ret, deferred_wakers) = {
                let mut inner = client.0.lock();
                let ret = inner.poll_transport(ctx);
                let deferred_wakers = std::mem::replace(&mut inner.wakers_to_wake, Vec::new());
                (ret, deferred_wakers)
            };
            deferred_wakers.into_iter().for_each(Waker::wake);
            ret
        });

        let client_loop = ClientLoop { client: Arc::downgrade(&ret), fut: Box::pin(fut) };

        (ret, client_loop)
    }

    /// Get the namespace for the connected FDomain. Calling this more than once is an error.
    pub async fn namespace(self: &Arc<Self>) -> Result<Channel, Error> {
        let new_handle = self.new_hid();
        let channel = Channel(Handle { id: new_handle.id, client: Arc::downgrade(self) });
        self.transaction(
            ordinals::GET_NAMESPACE,
            proto::FDomainGetNamespaceRequest { new_handle },
            Responder::Namespace,
        )
        .await?;
        Ok(channel)
    }

    /// Create a new channel in the connected FDomain.
    pub fn create_channel(self: &Arc<Self>) -> (Channel, Channel) {
        let id_a = self.new_hid();
        let id_b = self.new_hid();
        let fut = self.transaction(
            ordinals::CREATE_CHANNEL,
            proto::ChannelCreateChannelRequest { handles: [id_a, id_b] },
            Responder::CreateChannel,
        );

        fuchsia_async::Task::spawn(async move {
            if let Err(e) = fut.await {
                log::debug!("FDomain channel creation failed: {e}");
            }
        })
        .detach();

        (
            Channel(Handle { id: id_a.id, client: Arc::downgrade(self) }),
            Channel(Handle { id: id_b.id, client: Arc::downgrade(self) }),
        )
    }

    /// Create a channel from a handle ID allocated by the connected FDomain.
    pub fn channel_from_handle_id(self: &Arc<Self>, id: proto::HandleId) -> Channel {
        Channel(Handle { id: id.id, client: Arc::downgrade(self) })
    }

    /// Creates client and server endpoints connected to by a channel.
    pub fn create_endpoints<F: crate::fidl::ProtocolMarker>(
        self: &Arc<Self>,
    ) -> (crate::fidl::ClientEnd<F>, crate::fidl::ServerEnd<F>) {
        let (client, server) = self.create_channel();
        let client_end = crate::fidl::ClientEnd::<F>::new(client);
        let server_end = crate::fidl::ServerEnd::new(server);
        (client_end, server_end)
    }

    /// Creates a client proxy and a server endpoint connected by a channel.
    pub fn create_proxy<F: crate::fidl::ProtocolMarker>(
        self: &Arc<Self>,
    ) -> (F::Proxy, crate::fidl::ServerEnd<F>) {
        let (client_end, server_end) = self.create_endpoints::<F>();
        (client_end.into_proxy(), server_end)
    }

    /// Creates a client proxy and a server request stream connected by a channel.
    pub fn create_proxy_and_stream<F: crate::fidl::ProtocolMarker>(
        self: &Arc<Self>,
    ) -> (F::Proxy, F::RequestStream) {
        let (client_end, server_end) = self.create_endpoints::<F>();
        (client_end.into_proxy(), server_end.into_stream())
    }

    /// Creates a client end and a server request stream connected by a channel.
    pub fn create_request_stream<F: crate::fidl::ProtocolMarker>(
        self: &Arc<Self>,
    ) -> (crate::fidl::ClientEnd<F>, F::RequestStream) {
        let (client_end, server_end) = self.create_endpoints::<F>();
        (client_end, server_end.into_stream())
    }

    /// Create a new socket in the connected FDomain.
    fn create_socket(self: &Arc<Self>, options: proto::SocketType) -> (Socket, Socket) {
        let id_a = self.new_hid();
        let id_b = self.new_hid();
        let fut = self.transaction(
            ordinals::CREATE_SOCKET,
            proto::SocketCreateSocketRequest { handles: [id_a, id_b], options },
            Responder::CreateSocket,
        );

        fuchsia_async::Task::spawn(async move {
            if let Err(e) = fut.await {
                log::debug!("FDomain socket creation failed: {e}");
            }
        })
        .detach();

        (
            Socket(Handle { id: id_a.id, client: Arc::downgrade(self) }),
            Socket(Handle { id: id_b.id, client: Arc::downgrade(self) }),
        )
    }

    /// Create a new streaming socket in the connected FDomain.
    pub fn create_stream_socket(self: &Arc<Self>) -> (Socket, Socket) {
        self.create_socket(proto::SocketType::Stream)
    }

    /// Create a new datagram socket in the connected FDomain.
    pub fn create_datagram_socket(self: &Arc<Self>) -> (Socket, Socket) {
        self.create_socket(proto::SocketType::Datagram)
    }

    /// Create a new event pair in the connected FDomain.
    pub fn create_event_pair(self: &Arc<Self>) -> (EventPair, EventPair) {
        let id_a = self.new_hid();
        let id_b = self.new_hid();
        let fut = self.transaction(
            ordinals::CREATE_EVENT_PAIR,
            proto::EventPairCreateEventPairRequest { handles: [id_a, id_b] },
            Responder::CreateEventPair,
        );

        fuchsia_async::Task::spawn(async move {
            if let Err(e) = fut.await {
                log::debug!("FDomain event pair creation failed: {e}");
            }
        })
        .detach();

        (
            EventPair(Handle { id: id_a.id, client: Arc::downgrade(self) }),
            EventPair(Handle { id: id_b.id, client: Arc::downgrade(self) }),
        )
    }

    /// Create a new event handle in the connected FDomain.
    pub fn create_event(self: &Arc<Self>) -> Event {
        let id = self.new_hid();
        let fut = self.transaction(
            ordinals::CREATE_EVENT,
            proto::EventCreateEventRequest { handle: id },
            Responder::CreateEvent,
        );

        fuchsia_async::Task::spawn(async move {
            if let Err(e) = fut.await {
                log::debug!("FDomain event creation failed: {e}");
            }
        })
        .detach();

        Event(Handle { id: id.id, client: Arc::downgrade(self) })
    }

    /// Create a new VMO in the connected FDomain with the given options.
    pub fn create_vmo(self: &Arc<Self>, options: proto::VmoOptions, size: u64) -> Vmo {
        let id = self.new_hid();
        let fut = self.transaction(
            ordinals::CREATE_VMO,
            proto::VmoCreateVmoRequest { handle: id, size, options },
            Responder::CreateVmo,
        );

        fuchsia_async::Task::spawn(async move {
            if let Err(e) = fut.await {
                log::debug!("FDomain vmo creation failed: {e}");
            }
        })
        .detach();

        Vmo(Handle { id: id.id, client: Arc::downgrade(self) })
    }

    /// Allocate a new HID, which should be suitable for use with the connected FDomain.
    pub(crate) fn new_hid(&self) -> proto::NewHandleId {
        self.0.lock().new_hid()
    }

    /// Create a future which sends a FIDL message to the connected FDomain and
    /// waits for a response.
    ///
    /// Calling this method queues the transaction synchronously. Awaiting is
    /// only necessary to wait for the response.
    pub(crate) fn transaction<S: fidl_message::Body, R: 'static, F>(
        self: &Arc<Self>,
        ordinal: u64,
        request: S,
        f: F,
    ) -> impl Future<Output = Result<R, Error>> + 'static + use<S, R, F>
    where
        F: Fn(OneshotSender<Result<R, Error>>) -> Responder,
    {
        let mut inner = self.0.lock();

        let (sender, receiver) = futures::channel::oneshot::channel();
        inner.request(ordinal, request, f(sender));
        receiver.map(|x| x.expect("Oneshot went away without reply!"))
    }

    /// Start getting streaming events for socket reads.
    pub(crate) fn start_socket_streaming(&self, id: proto::HandleId) -> Result<(), Error> {
        let mut inner = self.0.lock();
        if let Some(e) = inner.transport.error() {
            return Err(e.into());
        }

        let state = inner.socket_read_states.entry(id).or_insert_with(|| SocketReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });

        assert!(!state.is_streaming, "Initiated streaming twice!");
        state.is_streaming = true;

        inner.request(
            ordinals::READ_SOCKET_STREAMING_START,
            proto::SocketReadSocketStreamingStartRequest { handle: id },
            Responder::Ignore,
        );
        Ok(())
    }

    /// Stop getting streaming events for socket reads. Doesn't return errors
    /// because it's exclusively called in destructors where we have nothing to
    /// do with them.
    pub(crate) fn stop_socket_streaming(&self, id: proto::HandleId) {
        let mut inner = self.0.lock();
        if let Some(state) = inner.socket_read_states.get_mut(&id) {
            if state.is_streaming {
                state.is_streaming = false;
                // TODO: Log?
                let _ = inner.request(
                    ordinals::READ_SOCKET_STREAMING_STOP,
                    proto::SocketReadSocketStreamingStopRequest { handle: id },
                    Responder::Ignore,
                );
            }
        }
    }

    /// Start getting streaming events for socket reads.
    pub(crate) fn start_channel_streaming(&self, id: proto::HandleId) -> Result<(), Error> {
        let mut inner = self.0.lock();
        if let Some(e) = inner.transport.error() {
            return Err(e.into());
        }
        let state = inner.channel_read_states.entry(id).or_insert_with(|| ChannelReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });

        assert!(!state.is_streaming, "Initiated streaming twice!");
        state.is_streaming = true;

        inner.request(
            ordinals::READ_CHANNEL_STREAMING_START,
            proto::ChannelReadChannelStreamingStartRequest { handle: id },
            Responder::Ignore,
        );

        Ok(())
    }

    /// Stop getting streaming events for socket reads. Doesn't return errors
    /// because it's exclusively called in destructors where we have nothing to
    /// do with them.
    pub(crate) fn stop_channel_streaming(&self, id: proto::HandleId) {
        let mut inner = self.0.lock();
        if let Some(state) = inner.channel_read_states.get_mut(&id) {
            if state.is_streaming {
                state.is_streaming = false;
                // TODO: Log?
                let _ = inner.request(
                    ordinals::READ_CHANNEL_STREAMING_STOP,
                    proto::ChannelReadChannelStreamingStopRequest { handle: id },
                    Responder::Ignore,
                );
            }
        }
    }

    /// Execute a read from a channel.
    pub(crate) fn poll_socket(
        &self,
        id: proto::HandleId,
        ctx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<Result<usize, Error>> {
        let mut inner = self.0.lock();
        if let Some(error) = inner.transport.error() {
            return Poll::Ready(Err(error.into()));
        }

        let state = inner.socket_read_states.entry(id).or_insert_with(|| SocketReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });

        if let Some(got) = state.queued.front_mut() {
            match got.as_mut() {
                Ok(data) => {
                    let read_size = std::cmp::min(data.data.len(), out.len());
                    out[..read_size].copy_from_slice(&data.data[..read_size]);

                    if data.data.len() > read_size && !data.is_datagram {
                        let _ = data.data.drain(..read_size);
                    } else {
                        let _ = state.queued.pop_front();
                    }

                    return Poll::Ready(Ok(read_size));
                }
                Err(_) => {
                    let err = state.queued.pop_front().unwrap().unwrap_err();
                    return Poll::Ready(Err(err));
                }
            }
        } else if !state.wakers.iter().any(|x| ctx.waker().will_wake(x)) {
            state.wakers.push(ctx.waker().clone());
        }

        if !state.read_request_pending && !state.is_streaming {
            state.read_request_pending = true;
            inner.request(
                ordinals::READ_SOCKET,
                proto::SocketReadSocketRequest { handle: id, max_bytes: out.len() as u64 },
                Responder::ReadSocket(id),
            );
        }

        Poll::Pending
    }

    /// Execute a read from a channel.
    pub(crate) fn poll_channel(
        &self,
        id: proto::HandleId,
        ctx: &mut Context<'_>,
        for_stream: bool,
    ) -> Poll<Option<Result<proto::ChannelMessage, Error>>> {
        let mut inner = self.0.lock();
        if let Some(error) = inner.transport.error() {
            return Poll::Ready(Some(Err(error.into())));
        }

        let state = inner.channel_read_states.entry(id).or_insert_with(|| ChannelReadState {
            wakers: Vec::new(),
            queued: VecDeque::new(),
            is_streaming: false,
            read_request_pending: false,
        });

        if let Some(got) = state.queued.pop_front() {
            return Poll::Ready(Some(got));
        } else if for_stream && !state.is_streaming {
            return Poll::Ready(None);
        } else if !state.wakers.iter().any(|x| ctx.waker().will_wake(x)) {
            state.wakers.push(ctx.waker().clone());
        }

        if !state.read_request_pending && !state.is_streaming {
            state.read_request_pending = true;
            inner.request(
                ordinals::READ_CHANNEL,
                proto::ChannelReadChannelRequest { handle: id },
                Responder::ReadChannel(id),
            );
        }

        Poll::Pending
    }

    /// Check whether this channel is streaming
    pub(crate) fn channel_is_streaming(&self, id: proto::HandleId) -> bool {
        let inner = self.0.lock();
        let Some(state) = inner.channel_read_states.get(&id) else {
            return false;
        };
        state.is_streaming
    }

    /// Check that all the given handles are safe to transfer through a channel
    /// e.g. that there's no chance of in-flight reads getting dropped.
    pub(crate) fn clear_handles_for_transfer(&self, handles: &proto::Handles) {
        let mut inner = self.0.lock();
        match handles {
            proto::Handles::Handles(handles) => {
                for handle in handles {
                    assert!(
                        !(inner.channel_read_states.contains_key(handle)
                            || inner.socket_read_states.contains_key(handle)),
                        "Tried to transfer handle after reading"
                    );
                    inner.handles.remove(handle);
                }
            }
            proto::Handles::Dispositions(dispositions) => {
                for disposition in dispositions {
                    match &disposition.handle {
                        proto::HandleOp::Move_(handle) => {
                            assert!(
                                !(inner.channel_read_states.contains_key(handle)
                                    || inner.socket_read_states.contains_key(handle)),
                                "Tried to transfer handle after reading"
                            );
                            inner.handles.remove(handle);
                        }
                        // Pretty sure this should be fine regardless of read state.
                        proto::HandleOp::Duplicate(_) => (),
                    }
                }
            }
        }
    }
}
