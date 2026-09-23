// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::AsHandleRef;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_fdomain as proto;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use futures::prelude::*;
use replace_with::replace_with;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::num::NonZeroU32;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll, Waker};

mod handles;
pub mod wire;

#[cfg(test)]
mod test;

pub type Result<T, E = proto::Error> = std::result::Result<T, E>;

use handles::{AnyHandle, HandleType as _, IsDatagramSocket};

/// A queue. Wraps a `VecDeque` but insures we are waking a waker when we push
/// items into the queue.
struct Queue<T>(VecDeque<T>);

impl<T> Queue<T> {
    /// Create a new queue.
    fn new() -> Self {
        Queue(VecDeque::new())
    }

    /// Whether the queue is empty.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Removes and discards the first element in the queue.
    ///
    /// # Panics
    /// There *must* be a first element or this will panic.
    fn destroy_front(&mut self) {
        assert!(self.0.pop_front().is_some(), "Expected to find a value!");
    }

    /// Pop the first element from the queue if available.
    fn pop_front(&mut self) -> Option<T> {
        self.0.pop_front()
    }

    /// Return an element to the front of the queue. Does not wake any waiters
    /// as it is assumed the waiter is the one who popped it to begin with.
    ///
    /// This is used when we'd *like* to use `front_mut` but we can't borrow the
    /// source of `self` for that long without giving ourselves lifetime
    /// headaches.
    fn push_front_no_wake(&mut self, t: T) {
        self.0.push_front(t)
    }

    /// Push a new element to the back of the queue, waking the given waker.
    fn push_back(&mut self, t: T, waker: &Waker) {
        self.0.push_back(t);
        waker.wake_by_ref();
    }

    /// Get a mutable reference to the first element in the queue.
    fn front_mut(&mut self) -> Option<&mut T> {
        self.0.front_mut()
    }
}

/// Maximum amount to read for an async socket read.
const ASYNC_READ_BUFSIZE: u64 = 65536;

/// Wraps the various FIDL Event types that can be produced by an FDomain
#[derive(Debug)]
pub enum FDomainEvent {
    ChannelStreamingReadStart(NonZeroU32, Result<()>),
    ChannelStreamingReadStop(NonZeroU32, Result<()>),
    SocketStreamingReadStart(NonZeroU32, Result<()>),
    SocketStreamingReadStop(NonZeroU32, Result<()>),
    WaitForSignals(NonZeroU32, Result<proto::FDomainWaitForSignalsResponse>),
    SocketData(NonZeroU32, Result<proto::SocketData>),
    SocketStreamingData(proto::SocketOnSocketStreamingDataRequest),
    SocketDispositionSet(NonZeroU32, Result<()>),
    WroteSocket(NonZeroU32, Result<proto::SocketWriteSocketResponse, proto::WriteSocketError>),
    ChannelData(NonZeroU32, Result<proto::ChannelMessage>),
    ChannelStreamingData(proto::ChannelOnChannelStreamingDataRequest),
    WroteChannel(NonZeroU32, Result<(), proto::WriteChannelError>),
    ClosedHandle(NonZeroU32, Result<()>),
    ReplacedHandle(NonZeroU32, Result<()>),
}

/// An [`FDomainEvent`] that needs a bit more processing before it can be sent.
/// I.e. it still contains `fidl::NullableHandle` objects that need to be replaced with
/// FDomain IDs.
enum UnprocessedFDomainEvent {
    Ready(FDomainEvent),
    ChannelData(NonZeroU32, fidl::MessageBufEtc),
    ChannelStreamingData(proto::HandleId, fidl::MessageBufEtc),
}

impl From<FDomainEvent> for UnprocessedFDomainEvent {
    fn from(other: FDomainEvent) -> UnprocessedFDomainEvent {
        UnprocessedFDomainEvent::Ready(other)
    }
}

/// Operations on a handle which are processed from the read queue.
enum ReadOp {
    /// Enable or disable async reads on a channel.
    StreamingChannel(NonZeroU32, bool),
    /// Enable or disable async reads on a socket.
    StreamingSocket(NonZeroU32, bool),
    Socket(NonZeroU32, u64),
    Channel(NonZeroU32),
}

/// An in-progress socket write. It may take multiple syscalls to write to a
/// socket, so this tracks how many bytes were written already and how many
/// remain to be written.
struct SocketWrite {
    tid: NonZeroU32,
    wrote: usize,
    to_write: Vec<u8>,
}

/// Operations on a handle which are processed from the write queue.
enum WriteOp {
    Socket(SocketWrite),
    Channel(NonZeroU32, Vec<u8>, HandlesToWrite),
    SetDisposition(NonZeroU32, proto::SocketDisposition, proto::SocketDisposition),
}

/// A handle which is being moved out of the FDomain by a channel write call or
/// closure/replacement.  There may still be operations to perform on this
/// handle, so the write should not proceed while the handle is in the `InUse`
/// state.
enum ShuttingDownHandle {
    InUse(proto::HandleId, HandleState),
    Ready(AnyHandle),
}

impl ShuttingDownHandle {
    fn poll_ready(
        &mut self,
        event_queue: &mut VecDeque<UnprocessedFDomainEvent>,
        ctx: &mut Context<'_>,
    ) -> Poll<()> {
        replace_with(self, |this| match this {
            this @ ShuttingDownHandle::Ready(_) => this,
            ShuttingDownHandle::InUse(hid, mut state) => {
                state.poll(event_queue, ctx);

                if state.write_queue.is_empty() {
                    while let Some(op) = state.read_queue.pop_front() {
                        match op {
                            ReadOp::StreamingChannel(tid, start) => {
                                let err = Err(proto::Error::BadHandleId(proto::BadHandleId {
                                    id: hid.id,
                                }));
                                let event = if start {
                                    FDomainEvent::ChannelStreamingReadStart(tid, err)
                                } else {
                                    FDomainEvent::ChannelStreamingReadStop(tid, err)
                                };
                                event_queue.push_back(event.into());
                            }
                            ReadOp::StreamingSocket(tid, start) => {
                                let err = Err(proto::Error::BadHandleId(proto::BadHandleId {
                                    id: hid.id,
                                }));
                                let event = if start {
                                    FDomainEvent::SocketStreamingReadStart(tid, err)
                                } else {
                                    FDomainEvent::SocketStreamingReadStop(tid, err)
                                };
                                event_queue.push_back(event.into());
                            }
                            ReadOp::Channel(tid) => {
                                let err = state
                                    .handle
                                    .expected_type(fidl::ObjectType::CHANNEL)
                                    .err()
                                    .unwrap_or(proto::Error::ClosedDuringRead(
                                        proto::ClosedDuringRead,
                                    ));
                                event_queue
                                    .push_back(FDomainEvent::ChannelData(tid, Err(err)).into());
                            }
                            ReadOp::Socket(tid, _max_bytes) => {
                                let err = state
                                    .handle
                                    .expected_type(fidl::ObjectType::SOCKET)
                                    .err()
                                    .unwrap_or(proto::Error::ClosedDuringRead(
                                        proto::ClosedDuringRead,
                                    ));
                                event_queue
                                    .push_back(FDomainEvent::SocketData(tid, Err(err)).into());
                            }
                        }
                    }

                    if state.async_read_in_progress {
                        match &*state.handle {
                            AnyHandle::Channel(_) => event_queue.push_back(
                                FDomainEvent::ChannelStreamingData(
                                    proto::ChannelOnChannelStreamingDataRequest {
                                        handle: hid,
                                        channel_sent: proto::ChannelSent::Stopped(
                                            proto::AioStopped { error: None },
                                        ),
                                    },
                                )
                                .into(),
                            ),
                            AnyHandle::Socket(_) => event_queue.push_back(
                                FDomainEvent::SocketStreamingData(
                                    proto::SocketOnSocketStreamingDataRequest {
                                        handle: hid,
                                        socket_message: proto::SocketMessage::Stopped(
                                            proto::AioStopped { error: None },
                                        ),
                                    },
                                )
                                .into(),
                            ),
                            AnyHandle::EventPair(_)
                            | AnyHandle::Event(_)
                            | AnyHandle::Vmo(_)
                            | AnyHandle::Unknown(_) => unreachable!(),
                        }
                    }

                    state.signal_waiters.clear();
                    state.io_waiter = None;

                    ShuttingDownHandle::Ready(
                        Arc::into_inner(state.handle).expect("Unaccounted-for handle reference!"),
                    )
                } else {
                    ShuttingDownHandle::InUse(hid, state)
                }
            }
        });

        if matches!(self, ShuttingDownHandle::Ready(_)) { Poll::Ready(()) } else { Poll::Pending }
    }
}

/// A vector of [`ShuttingDownHandle`] paired with rights for the new handles, which
/// can transition into being a vector of [`fidl::HandleDisposition`] when all the
/// handles are ready.
enum HandlesToWrite {
    SomeInUse(Vec<(ShuttingDownHandle, fidl::Rights)>),
    AllReady(Vec<fidl::HandleDisposition<'static>>),
}

impl HandlesToWrite {
    fn poll_ready(
        &mut self,
        event_queue: &mut VecDeque<UnprocessedFDomainEvent>,
        ctx: &mut Context<'_>,
    ) -> Poll<&mut Vec<fidl::HandleDisposition<'static>>> {
        match self {
            HandlesToWrite::AllReady(s) => Poll::Ready(s),
            HandlesToWrite::SomeInUse(handles) => {
                let mut ready = true;
                for (handle, _) in handles.iter_mut() {
                    ready = ready && handle.poll_ready(event_queue, ctx).is_ready();
                }

                if !ready {
                    return Poll::Pending;
                }

                *self = HandlesToWrite::AllReady(
                    handles
                        .drain(..)
                        .map(|(handle, rights)| {
                            let ShuttingDownHandle::Ready(handle) = handle else { unreachable!() };

                            fidl::HandleDisposition::new(
                                fidl::HandleOp::Move(handle.into()),
                                fidl::ObjectType::NONE,
                                rights,
                                Ok(()),
                            )
                        })
                        .collect(),
                );

                let HandlesToWrite::AllReady(s) = self else { unreachable!() };
                Poll::Ready(s)
            }
        }
    }
}

struct AnyHandleRef(Arc<AnyHandle>);

impl AsHandleRef for AnyHandleRef {
    fn as_handle_ref(&self) -> fidl::HandleRef<'_> {
        self.0.as_handle_ref()
    }
}

#[cfg(target_os = "fuchsia")]
type OnSignals = fasync::OnSignals<'static, AnyHandleRef>;

#[cfg(not(target_os = "fuchsia"))]
type OnSignals = fasync::OnSignalsRef<'static>;

/// Represents a `WaitForSignals` transaction from a client. When the contained
/// `OnSignals` polls to completion we can reply to the transaction.
struct SignalWaiter {
    tid: NonZeroU32,
    waiter: Pin<Box<OnSignals>>,
}

/// Information about a single handle within the [`FDomain`].
struct HandleState {
    /// The handle itself.
    handle: Arc<AnyHandle>,
    /// Our handle ID.
    hid: proto::HandleId,
    /// Whether this is a datagram socket. We have to handle data coming out of
    /// datagram sockets a bit differently to preserve their semantics from the
    /// perspective of the host and avoid data loss.
    is_datagram_socket: bool,
    /// Indicates we are sending `On*StreamingData` events to the client
    /// presently. It is an error for the user to try to move the handle out of
    /// the FDomain (e.g. send it through a channel or close it) until after
    /// they request that streaming events stop.
    async_read_in_progress: bool,
    /// Queue of client requests to read from the handle. We have to queue read
    /// requests because they may block, and we don't want to block the event
    /// loop or be unable to handle further requests while a long read request
    /// is blocking. Also we want to retire read requests in the order they were
    /// submitted, otherwise pipelined reads could return data in a strange order.
    read_queue: Queue<ReadOp>,
    /// Queue of client requests to write to the handle. We have to queue write
    /// requests for the same reason we have to queue read requests. Since we
    /// process the queue one at a time, we need a separate queue for writes
    /// otherwise we'd effectively make handles half-duplex, with read requests
    /// unable to proceed if a write request is blocked at the head of the
    /// queue.
    write_queue: Queue<WriteOp>,
    /// List of outstanding `WaitForSignals` transactions.
    signal_waiters: Vec<SignalWaiter>,
    /// Contains a waiter on this handle for IO reading and writing. Populated
    /// whenever we need to block on IO to service a request.
    io_waiter: Option<Pin<Box<OnSignals>>>,
}

impl HandleState {
    fn new(handle: AnyHandle, hid: proto::HandleId) -> Result<Self, proto::Error> {
        let is_datagram_socket = match handle.is_datagram_socket() {
            IsDatagramSocket::Unknown => {
                return Err(proto::Error::SocketTypeUnknown(proto::SocketTypeUnknown {
                    type_: proto::SocketType::unknown(),
                }));
            }
            other => other.is_datagram(),
        };
        Ok(HandleState {
            handle: Arc::new(handle),
            hid,
            async_read_in_progress: false,
            is_datagram_socket,
            read_queue: Queue::new(),
            write_queue: Queue::new(),
            signal_waiters: Vec::new(),
            io_waiter: None,
        })
    }

    /// Poll this handle state. Lets us handle our IO queues and wait for the
    /// next IO event.
    fn poll(&mut self, event_queue: &mut VecDeque<UnprocessedFDomainEvent>, ctx: &mut Context<'_>) {
        self.signal_waiters.retain_mut(|x| {
            let Poll::Ready(result) = x.waiter.poll_unpin(ctx) else {
                return true;
            };

            event_queue.push_back(
                FDomainEvent::WaitForSignals(
                    x.tid,
                    result
                        .map(|x| proto::FDomainWaitForSignalsResponse { signals: x.bits() })
                        .map_err(|e| proto::Error::TargetError(e.into_raw())),
                )
                .into(),
            );

            false
        });

        let read_signals = self.handle.read_signals();
        let write_signals = self.handle.write_signals();

        loop {
            if let Some(signal_waiter) = self.io_waiter.as_mut() {
                if let Poll::Ready(sigs) = signal_waiter.poll_unpin(ctx) {
                    if let Ok(sigs) = sigs {
                        if sigs.intersects(read_signals) {
                            self.process_read_queue(event_queue);
                        }
                        if sigs.intersects(write_signals) {
                            self.process_write_queue(event_queue, ctx);
                        }
                    }
                } else {
                    let need_read = matches!(
                        self.read_queue.front_mut(),
                        Some(ReadOp::StreamingChannel(_, _) | ReadOp::StreamingSocket(_, _))
                    );
                    let need_write = matches!(
                        self.write_queue.front_mut(),
                        Some(WriteOp::SetDisposition(_, _, _))
                    );

                    self.process_read_queue(event_queue);
                    self.process_write_queue(event_queue, ctx);

                    if !(need_read || need_write) {
                        break;
                    }
                }
            }

            let subscribed_signals =
                if self.async_read_in_progress || !self.read_queue.is_empty() {
                    read_signals
                } else {
                    fidl::Signals::NONE
                } | if !self.write_queue.is_empty() { write_signals } else { fidl::Signals::NONE };

            if !subscribed_signals.is_empty() {
                self.io_waiter = Some(Box::pin(OnSignals::new(
                    AnyHandleRef(Arc::clone(&self.handle)),
                    subscribed_signals,
                )));
            } else {
                self.io_waiter = None;
                break;
            }
        }
    }

    /// Set `async_read_in_progress` to `true`. Return an error if it was already `true`.
    fn try_enable_async_read(&mut self) -> Result<()> {
        if self.async_read_in_progress {
            Err(proto::Error::StreamingReadInProgress(proto::StreamingReadInProgress))
        } else {
            self.async_read_in_progress = true;
            Ok(())
        }
    }

    /// Set `async_read_in_progress` to `false`. Return an error if it was already `false`.
    fn try_disable_async_read(&mut self) -> Result<()> {
        if !self.async_read_in_progress {
            Err(proto::Error::NoReadInProgress(proto::NoReadInProgress))
        } else {
            self.async_read_in_progress = false;
            Ok(())
        }
    }

    /// Handle events from the front of the read queue.
    fn process_read_queue(&mut self, event_queue: &mut VecDeque<UnprocessedFDomainEvent>) {
        while let Some(op) = self.read_queue.front_mut() {
            match op {
                ReadOp::StreamingChannel(tid, true) => {
                    let tid = *tid;
                    let result = self.try_enable_async_read();
                    event_queue
                        .push_back(FDomainEvent::ChannelStreamingReadStart(tid, result).into());
                    self.read_queue.destroy_front();
                }
                ReadOp::StreamingChannel(tid, false) => {
                    let tid = *tid;
                    let result = self.try_disable_async_read();
                    event_queue
                        .push_back(FDomainEvent::ChannelStreamingReadStop(tid, result).into());
                    self.read_queue.destroy_front();
                }
                ReadOp::StreamingSocket(tid, true) => {
                    let tid = *tid;
                    let result = self.try_enable_async_read();
                    event_queue
                        .push_back(FDomainEvent::SocketStreamingReadStart(tid, result).into());
                    self.read_queue.destroy_front();
                }
                ReadOp::StreamingSocket(tid, false) => {
                    let tid = *tid;
                    let result = self.try_disable_async_read();
                    event_queue
                        .push_back(FDomainEvent::SocketStreamingReadStop(tid, result).into());
                    self.read_queue.destroy_front();
                }
                ReadOp::Socket(tid, max_bytes) => {
                    let (tid, max_bytes) = (*tid, *max_bytes);
                    if let Some(event) = self.do_read_socket(tid, max_bytes) {
                        let _ = self.read_queue.pop_front();
                        event_queue.push_back(event.into());
                    } else {
                        break;
                    }
                }
                ReadOp::Channel(tid) => {
                    let tid = *tid;
                    if let Some(event) = self.do_read_channel(tid) {
                        let _ = self.read_queue.pop_front();
                        event_queue.push_back(event.into());
                    } else {
                        break;
                    }
                }
            }
        }

        if self.async_read_in_progress {
            // We should have error'd out of any blocking operations if we had a
            // read in progress.
            assert!(self.read_queue.is_empty());
            self.process_async_read(event_queue);
        }
    }

    fn process_async_read(&mut self, event_queue: &mut VecDeque<UnprocessedFDomainEvent>) {
        assert!(self.async_read_in_progress);

        match &*self.handle {
            AnyHandle::Channel(_) => {
                'read_loop: while let Some(result) = self.handle.read_channel().transpose() {
                    match result {
                        Ok(msg) => event_queue.push_back(
                            UnprocessedFDomainEvent::ChannelStreamingData(self.hid, msg),
                        ),
                        Err(e) => {
                            event_queue.push_back(
                                FDomainEvent::ChannelStreamingData(
                                    proto::ChannelOnChannelStreamingDataRequest {
                                        handle: self.hid,
                                        channel_sent: proto::ChannelSent::Stopped(
                                            proto::AioStopped { error: Some(Box::new(e)) },
                                        ),
                                    },
                                )
                                .into(),
                            );
                            self.async_read_in_progress = false;
                            break 'read_loop;
                        }
                    }
                }
            }

            AnyHandle::Socket(_) => {
                'read_loop: while let Some(result) =
                    self.handle.read_socket(ASYNC_READ_BUFSIZE).transpose()
                {
                    match result {
                        Ok(data) => {
                            event_queue.push_back(
                                FDomainEvent::SocketStreamingData(
                                    proto::SocketOnSocketStreamingDataRequest {
                                        handle: self.hid,
                                        socket_message: proto::SocketMessage::Data(
                                            proto::SocketData {
                                                data,
                                                is_datagram: self.is_datagram_socket,
                                            },
                                        ),
                                    },
                                )
                                .into(),
                            );
                        }
                        Err(e) => {
                            event_queue.push_back(
                                FDomainEvent::SocketStreamingData(
                                    proto::SocketOnSocketStreamingDataRequest {
                                        handle: self.hid,
                                        socket_message: proto::SocketMessage::Stopped(
                                            proto::AioStopped { error: Some(Box::new(e)) },
                                        ),
                                    },
                                )
                                .into(),
                            );
                            self.async_read_in_progress = false;
                            break 'read_loop;
                        }
                    }
                }
            }

            _ => unreachable!("Processed async read for unreadable handle type!"),
        }
    }

    /// Handle events from the front of the write queue.
    fn process_write_queue(
        &mut self,
        event_queue: &mut VecDeque<UnprocessedFDomainEvent>,
        ctx: &mut Context<'_>,
    ) {
        // We want to mutate and *maybe* pop the front of the write queue, but
        // lifetime shenanigans mean we can't do that and also access `self`,
        // which we need. So we pop the item always, and then maybe push it to
        // the front again if we didn't actually want to pop it.
        while let Some(op) = self.write_queue.pop_front() {
            match op {
                WriteOp::Socket(mut op) => {
                    if let Some(event) = self.do_write_socket(&mut op) {
                        event_queue.push_back(event.into());
                    } else {
                        self.write_queue.push_front_no_wake(WriteOp::Socket(op));
                        break;
                    }
                }
                WriteOp::SetDisposition(tid, disposition, disposition_peer) => {
                    let result = { self.handle.socket_disposition(disposition, disposition_peer) };
                    event_queue.push_back(FDomainEvent::SocketDispositionSet(tid, result).into())
                }
                WriteOp::Channel(tid, data, mut handles) => {
                    if self
                        .do_write_channel(tid, &data, &mut handles, event_queue, ctx)
                        .is_pending()
                    {
                        self.write_queue.push_front_no_wake(WriteOp::Channel(tid, data, handles));
                        break;
                    }
                }
            }
        }
    }

    /// Attempt to read from the handle in this [`HandleState`] as if it were a
    /// socket. If the read succeeds or produces an error that should not be
    /// retried, produce an [`FDomainEvent`] containing the result.
    fn do_read_socket(&mut self, tid: NonZeroU32, max_bytes: u64) -> Option<FDomainEvent> {
        if self.async_read_in_progress {
            return Some(
                FDomainEvent::SocketData(
                    tid,
                    Err(proto::Error::StreamingReadInProgress(proto::StreamingReadInProgress)),
                )
                .into(),
            );
        }

        let max_bytes = if self.is_datagram_socket {
            let AnyHandle::Socket(s) = &*self.handle else {
                unreachable!("Read socket from state that wasn't for a socket!");
            };
            match s.info() {
                Ok(x) => x.rx_buf_available as u64,
                // We should always succeed. The only failures are if we don't
                // have the rights or something's screwed up with the handle. We
                // know we have the rights because figuring out this was a
                // datagram socket to begin with meant calling the same call on
                // the same handle earlier.
                Err(e) => {
                    return Some(FDomainEvent::SocketData(
                        tid,
                        Err(proto::Error::TargetError(e.into_raw())),
                    ));
                }
            }
        } else {
            max_bytes
        };
        self.handle.read_socket(max_bytes).transpose().map(|x| {
            FDomainEvent::SocketData(
                tid,
                x.map(|data| proto::SocketData { data, is_datagram: self.is_datagram_socket }),
            )
        })
    }

    /// Attempt to write to the handle in this [`HandleState`] as if it were a
    /// socket. If the write succeeds or produces an error that should not be
    /// retried, produce an [`FDomainEvent`] containing the result.
    fn do_write_socket(&mut self, op: &mut SocketWrite) -> Option<FDomainEvent> {
        match self.handle.write_socket(&op.to_write) {
            Ok(wrote) => {
                op.wrote += wrote;
                op.to_write.drain(..wrote);

                if op.to_write.is_empty() {
                    Some(FDomainEvent::WroteSocket(
                        op.tid,
                        Ok(proto::SocketWriteSocketResponse {
                            wrote: op.wrote.try_into().unwrap(),
                        }),
                    ))
                } else {
                    None
                }
            }
            Err(error) => Some(FDomainEvent::WroteSocket(
                op.tid,
                Err(proto::WriteSocketError { error, wrote: op.wrote.try_into().unwrap() }),
            )),
        }
    }

    /// Attempt to write to the handle in this [`HandleState`] as if it were a
    /// channel. If the write succeeds or produces an error that should not be
    /// retried, produce an [`FDomainEvent`] containing the result.
    fn do_write_channel(
        &mut self,
        tid: NonZeroU32,
        data: &[u8],
        handles: &mut HandlesToWrite,
        event_queue: &mut VecDeque<UnprocessedFDomainEvent>,
        ctx: &mut Context<'_>,
    ) -> Poll<()> {
        let Poll::Ready(handles) = handles.poll_ready(event_queue, ctx) else {
            return Poll::Pending;
        };

        let ret = self.handle.write_channel(data, handles);
        if let Some(ret) = ret {
            event_queue.push_back(FDomainEvent::WroteChannel(tid, ret).into())
        }
        Poll::Ready(())
    }

    /// Attempt to read from the handle in this [`HandleState`] as if it were a
    /// channel. If the read succeeds or produces an error that should not be
    /// retried, produce an [`FDomainEvent`] containing the result.
    fn do_read_channel(&mut self, tid: NonZeroU32) -> Option<UnprocessedFDomainEvent> {
        if self.async_read_in_progress {
            return Some(
                FDomainEvent::ChannelData(
                    tid,
                    Err(proto::Error::StreamingReadInProgress(proto::StreamingReadInProgress)),
                )
                .into(),
            );
        }
        match self.handle.read_channel() {
            Ok(x) => x.map(|x| UnprocessedFDomainEvent::ChannelData(tid, x)),
            Err(e) => Some(FDomainEvent::ChannelData(tid, Err(e)).into()),
        }
    }
}

/// State for a handle which is closing, but which needs its read and write
/// queues flushed first.
struct ClosingHandle {
    action: Arc<CloseAction>,
    state: Option<ShuttingDownHandle>,
}

impl ClosingHandle {
    fn poll_ready(&mut self, fdomain: &mut FDomain, ctx: &mut Context<'_>) -> Poll<()> {
        if let Some(state) = self.state.as_mut() {
            if state.poll_ready(&mut fdomain.event_queue, ctx).is_ready() {
                let state = self.state.take().unwrap();
                let ShuttingDownHandle::Ready(handle) = state else {
                    unreachable!();
                };
                self.action.perform(fdomain, handle);
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        } else {
            Poll::Ready(())
        }
    }
}

/// When the client requests a handle to be closed or moved or otherwise
/// destroyed, it goes into limbo for a bit while pending read and write actions
/// are flushed. This is how we mark what should happen to the handle after that
/// period ends.
enum CloseAction {
    Close { tid: NonZeroU32, count: AtomicU32, result: Result<()> },
    Replace { tid: NonZeroU32, new_hid: proto::NewHandleId, rights: fidl::Rights },
}

impl CloseAction {
    fn perform(&self, fdomain: &mut FDomain, handle: AnyHandle) {
        match self {
            CloseAction::Close { tid, count, result } => {
                if count.fetch_sub(1, Ordering::Relaxed) == 1 {
                    fdomain
                        .event_queue
                        .push_back(FDomainEvent::ClosedHandle(*tid, result.clone()).into());
                }
            }
            CloseAction::Replace { tid, new_hid, rights } => {
                let result = handle
                    .replace(*rights)
                    .and_then(|handle| fdomain.alloc_client_handles([*new_hid], [handle]));
                fdomain.event_queue.push_back(FDomainEvent::ReplacedHandle(*tid, result).into());
            }
        }
    }
}

enum Namespace {
    Native(Box<dyn Fn() -> Result<ClientEnd<fio::DirectoryMarker>, fidl::Status> + Send>),
    Channel(Box<dyn Fn(proto::HandleId) + Send>),
}

/// This is a container of handles that is manipulable via the FDomain protocol.
/// See [RFC-0228].
///
/// Most of the methods simply handle FIDL requests from the FDomain protocol.
#[pin_project::pin_project]
pub struct FDomain {
    namespace: Namespace,
    handles: HashMap<proto::HandleId, HandleState>,
    closing_handles: Vec<ClosingHandle>,
    event_queue: VecDeque<UnprocessedFDomainEvent>,
    waker: Waker,
}

impl FDomain {
    /// Create a new FDomain. The new FDomain is empty and ready to be connected
    /// to by a client.
    pub fn new_empty() -> Self {
        Self::new(|| Err(fidl::Status::NOT_FOUND))
    }

    /// Create a new FDomain populated with the given namespace entries.
    pub fn new(
        namespace: impl Fn() -> Result<ClientEnd<fio::DirectoryMarker>, fidl::Status> + Send + 'static,
    ) -> Self {
        FDomain {
            namespace: Namespace::Native(Box::new(namespace)),
            handles: HashMap::new(),
            closing_handles: Vec::new(),
            event_queue: VecDeque::new(),
            waker: Waker::noop().clone(),
        }
    }

    /// Create a new FDomain with a callback that receives a channel handle ID to serve.
    pub fn new_with_namespace_channel(
        namespace: impl Fn(proto::HandleId) + Send + 'static,
    ) -> Self {
        FDomain {
            namespace: Namespace::Channel(Box::new(namespace)),
            handles: HashMap::new(),
            closing_handles: Vec::new(),
            event_queue: VecDeque::new(),
            waker: Waker::noop().clone(),
        }
    }

    /// Add an event to be emitted by this FDomain.
    fn push_event(&mut self, event: impl Into<UnprocessedFDomainEvent>) {
        self.event_queue.push_back(event.into());
        self.waker.wake_by_ref();
    }

    /// Given a [`fidl::MessageBufEtc`], load all of the handles from it into this
    /// FDomain and return a [`ReadChannelPayload`](proto::ReadChannelPayload)
    /// with the same data and the IDs for the handles.
    fn process_message(
        &mut self,
        message: fidl::MessageBufEtc,
    ) -> Result<proto::ChannelMessage, proto::Error> {
        let (data, handles) = message.split();
        let handles = handles
            .into_iter()
            .map(|info| {
                let type_ = info.object_type;

                let handle = match info.object_type {
                    fidl::ObjectType::CHANNEL => {
                        AnyHandle::Channel(fidl::Channel::from(info.handle))
                    }
                    fidl::ObjectType::SOCKET => AnyHandle::Socket(fidl::Socket::from(info.handle)),
                    fidl::ObjectType::EVENTPAIR => {
                        AnyHandle::EventPair(fidl::EventPair::from(info.handle))
                    }
                    fidl::ObjectType::EVENT => AnyHandle::Event(fidl::Event::from(info.handle)),
                    fidl::ObjectType::VMO => AnyHandle::Vmo(fidl::Vmo::from(info.handle)),
                    _ => AnyHandle::Unknown(handles::Unknown(info.handle, info.object_type)),
                };

                Ok(proto::HandleInfo {
                    rights: info.rights,
                    handle: self.alloc_fdomain_handle(handle)?,
                    type_,
                })
            })
            .collect::<Result<Vec<_>, proto::Error>>()?;

        Ok(proto::ChannelMessage { data, handles })
    }

    /// Allocate `N` new handle IDs. These are allocated from
    /// [`NewHandleId`](proto::NewHandleId) and are expected to follow the protocol
    /// rules for client-allocated handle IDs.
    ///
    /// If any of the handles passed fail to allocate, none of the handles will
    /// be allocated.
    fn alloc_client_handles<const N: usize>(
        &mut self,
        ids: [proto::NewHandleId; N],
        handles: [AnyHandle; N],
    ) -> Result<(), proto::Error> {
        for id in ids {
            if id.id & (1 << 31) != 0 {
                return Err(proto::Error::NewHandleIdOutOfRange(proto::NewHandleIdOutOfRange {
                    id: id.id,
                }));
            }

            if self.handles.contains_key(&proto::HandleId { id: id.id }) {
                return Err(proto::Error::NewHandleIdReused(proto::NewHandleIdReused {
                    id: id.id,
                    same_call: false,
                }));
            }
        }

        let mut sorted_ids = ids;
        sorted_ids.sort();

        if let Some([a, _]) = sorted_ids.array_windows().find(|&[a, b]| a == b) {
            Err(proto::Error::NewHandleIdReused(proto::NewHandleIdReused {
                id: a.id,
                same_call: true,
            }))
        } else {
            let ids = ids.into_iter().map(|id| proto::HandleId { id: id.id });
            let handles = ids
                .zip(handles.into_iter())
                .map(|(id, h)| HandleState::new(h, id).map(|x| (id, x)))
                .collect::<Result<Vec<_>, proto::Error>>()?;

            self.handles.extend(handles);

            Ok(())
        }
    }

    /// Allocate a new handle ID. These are allocated internally and are
    /// expected to follow the protocol rules for FDomain-allocated handle IDs.
    fn alloc_fdomain_handle(&mut self, handle: AnyHandle) -> Result<proto::HandleId, proto::Error> {
        loop {
            let id = proto::HandleId { id: rand::random::<u32>() | (1u32 << 31) };
            if let Entry::Vacant(v) = self.handles.entry(id) {
                v.insert(HandleState::new(handle, id)?);
                break Ok(id);
            }
        }
    }

    /// If a handle exists in this FDomain, remove it.
    fn take_handle(&mut self, handle: proto::HandleId) -> Result<HandleState, proto::Error> {
        self.handles
            .remove(&handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: handle.id }))
    }

    /// Use a handle in our handle table, if it exists.
    fn using_handle<T>(
        &mut self,
        id: proto::HandleId,
        f: impl FnOnce(&mut HandleState, &Waker) -> Result<T, proto::Error>,
    ) -> Result<T, proto::Error> {
        let waker = &self.waker;
        if let Some(s) = self.handles.get_mut(&id) {
            f(s, waker)
        } else {
            Err(proto::Error::BadHandleId(proto::BadHandleId { id: id.id }))
        }
    }

    pub fn get_namespace(&mut self, request: proto::FDomainGetNamespaceRequest) -> Result<()> {
        match &self.namespace {
            Namespace::Native(namespace) => {
                let endpoint = namespace();
                match endpoint {
                    Ok(endpoint) => self.alloc_client_handles(
                        [request.new_handle],
                        [AnyHandle::Channel(endpoint.into_channel())],
                    ),
                    Err(e) => Err(proto::Error::TargetError(e.into_raw())),
                }
            }
            Namespace::Channel(_) => {
                let (client_chan, server_chan) = fidl::Channel::create();
                self.alloc_client_handles([request.new_handle], [AnyHandle::Channel(client_chan)])?;
                let server_hid = self.alloc_fdomain_handle(AnyHandle::Channel(server_chan))?;
                if let Namespace::Channel(callback) = &self.namespace {
                    callback(server_hid);
                }
                Ok(())
            }
        }
    }

    pub fn create_channel(&mut self, request: proto::ChannelCreateChannelRequest) -> Result<()> {
        let (a, b) = fidl::Channel::create();
        self.alloc_client_handles(request.handles, [AnyHandle::Channel(a), AnyHandle::Channel(b)])
    }

    pub fn create_socket(&mut self, request: proto::SocketCreateSocketRequest) -> Result<()> {
        let (a, b) = match request.options {
            proto::SocketType::Stream => fidl::Socket::create_stream(),
            proto::SocketType::Datagram => fidl::Socket::create_datagram(),
            type_ => {
                return Err(proto::Error::SocketTypeUnknown(proto::SocketTypeUnknown { type_ }));
            }
        };

        self.alloc_client_handles(request.handles, [AnyHandle::Socket(a), AnyHandle::Socket(b)])
    }

    pub fn create_event_pair(
        &mut self,
        request: proto::EventPairCreateEventPairRequest,
    ) -> Result<()> {
        let (a, b) = fidl::EventPair::create();
        self.alloc_client_handles(
            request.handles,
            [AnyHandle::EventPair(a), AnyHandle::EventPair(b)],
        )
    }

    pub fn create_event(&mut self, request: proto::EventCreateEventRequest) -> Result<()> {
        let a = fidl::Event::create();
        self.alloc_client_handles([request.handle], [AnyHandle::Event(a)])
    }

    #[cfg(target_os = "fuchsia")]
    pub fn create_vmo(&mut self, request: proto::VmoCreateVmoRequest) -> Result<()> {
        let opts = zx::VmoOptions::from_bits_truncate(request.options.bits());
        let a = zx::Vmo::create_with_opts(opts, request.size)
            .map_err(|e| proto::Error::TargetError(e.into_raw()))?;
        self.alloc_client_handles([request.handle], [AnyHandle::Vmo(a.into())])
    }

    #[cfg(not(target_os = "fuchsia"))]
    pub fn create_vmo(&mut self, _request: proto::VmoCreateVmoRequest) -> Result<()> {
        Err(proto::Error::TargetError(fidl::Status::NOT_SUPPORTED.into_raw()))
    }

    pub fn read_vmo(
        &mut self,
        request: proto::VmoReadVmoRequest,
    ) -> Result<proto::VmoReadVmoResponse> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        let data = handle.handle.read_vmo(request.offset, request.size)?;
        Ok(proto::VmoReadVmoResponse { data })
    }

    pub fn write_vmo(&mut self, request: proto::VmoWriteVmoRequest) -> Result<()> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        handle.handle.write_vmo(request.offset, &request.data)
    }

    pub fn get_vmo_size(
        &mut self,
        request: proto::VmoGetVmoSizeRequest,
    ) -> Result<proto::VmoGetVmoSizeResponse> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        let size = handle.handle.get_vmo_size()?;
        Ok(proto::VmoGetVmoSizeResponse { size })
    }

    pub fn set_vmo_size(&mut self, request: proto::VmoSetVmoSizeRequest) -> Result<()> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        handle.handle.set_vmo_size(request.size)
    }

    pub fn get_vmo_stream_size(
        &mut self,
        request: proto::VmoGetVmoStreamSizeRequest,
    ) -> Result<proto::VmoGetVmoStreamSizeResponse> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        let size = handle.handle.get_vmo_stream_size()?;
        Ok(proto::VmoGetVmoStreamSizeResponse { size })
    }

    pub fn set_vmo_stream_size(
        &mut self,
        request: proto::VmoSetVmoStreamSizeRequest,
    ) -> Result<()> {
        let handle = self
            .handles
            .get(&request.handle)
            .ok_or(proto::Error::BadHandleId(proto::BadHandleId { id: request.handle.id }))?;
        handle.handle.set_vmo_stream_size(request.size)
    }

    pub fn set_socket_disposition(
        &mut self,
        tid: NonZeroU32,
        request: proto::SocketSetSocketDispositionRequest,
    ) {
        if let Err(err) = self.using_handle(request.handle, |h, waker| {
            h.write_queue.push_back(
                WriteOp::SetDisposition(tid, request.disposition, request.disposition_peer),
                waker,
            );
            Ok(())
        }) {
            self.push_event(FDomainEvent::SocketDispositionSet(tid, Err(err)));
        }
    }

    pub fn read_socket(&mut self, tid: NonZeroU32, request: proto::SocketReadSocketRequest) {
        if let Err(e) = self.using_handle(request.handle, |h, waker| {
            h.read_queue.push_back(ReadOp::Socket(tid, request.max_bytes), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::SocketData(tid, Err(e)));
        }
    }

    pub fn read_channel(&mut self, tid: NonZeroU32, request: proto::ChannelReadChannelRequest) {
        if let Err(e) = self.using_handle(request.handle, |h, waker| {
            h.read_queue.push_back(ReadOp::Channel(tid), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::ChannelData(tid, Err(e)));
        }
    }

    pub fn write_socket(&mut self, tid: NonZeroU32, request: proto::SocketWriteSocketRequest) {
        if let Err(error) = self.using_handle(request.handle, |h, waker| {
            h.write_queue.push_back(
                WriteOp::Socket(SocketWrite { tid, wrote: 0, to_write: request.data }),
                waker,
            );
            Ok(())
        }) {
            self.push_event(FDomainEvent::WroteSocket(
                tid,
                Err(proto::WriteSocketError { error, wrote: 0 }),
            ));
        }
    }

    pub fn write_channel(&mut self, tid: NonZeroU32, request: proto::ChannelWriteChannelRequest) {
        // Go through the list of handles in the requests (which will either be
        // a simple list of handles or a list of HandleDispositions) and obtain
        // for each a `ShuttingDownHandle` which contains our handle state (the
        // "Shutting down" refers to the fact that we're pulling the handle out
        // of the FDomain in order to send it) and the rights the requester
        // would like the handle to have upon arrival at the other end of the
        // channel.
        let handles: Vec<Result<(ShuttingDownHandle, fidl::Rights)>> = match request.handles {
            proto::Handles::Handles(h) => h
                .into_iter()
                .map(|h| {
                    if h != request.handle {
                        self.take_handle(h).map(|handle_state| {
                            (ShuttingDownHandle::InUse(h, handle_state), fidl::Rights::SAME_RIGHTS)
                        })
                    } else {
                        Err(proto::Error::WroteToSelf(proto::WroteToSelf))
                    }
                })
                .collect(),
            proto::Handles::Dispositions(d) => d
                .into_iter()
                .map(|d| {
                    let res = match d.handle {
                        proto::HandleOp::Move_(h) => {
                            if h != request.handle {
                                self.take_handle(h).map(|x| ShuttingDownHandle::InUse(h, x))
                            } else {
                                Err(proto::Error::WroteToSelf(proto::WroteToSelf))
                            }
                        }
                        proto::HandleOp::Duplicate(h) => {
                            if h != request.handle {
                                // If the requester wants us to duplicate the
                                // handle, we do so now rather than letting
                                // `write_etc` do it. Otherwise we have to use a
                                // reference to the handle, and we get lifetime
                                // hell.
                                self.using_handle(h, |h, _| {
                                    h.handle.duplicate(fidl::Rights::SAME_RIGHTS)
                                })
                                .map(ShuttingDownHandle::Ready)
                            } else {
                                Err(proto::Error::WroteToSelf(proto::WroteToSelf))
                            }
                        }
                    };

                    res.and_then(|x| Ok((x, d.rights)))
                })
                .collect(),
        };

        if handles.iter().any(|x| x.is_err()) {
            let e = handles.into_iter().map(|x| x.err().map(Box::new)).collect();

            self.push_event(FDomainEvent::WroteChannel(
                tid,
                Err(proto::WriteChannelError::OpErrors(e)),
            ));
            return;
        }

        let handles = handles.into_iter().map(|x| x.unwrap()).collect::<Vec<_>>();

        if let Err(e) = self.using_handle(request.handle, |h, waker| {
            h.write_queue.push_back(
                WriteOp::Channel(tid, request.data, HandlesToWrite::SomeInUse(handles)),
                waker,
            );
            Ok(())
        }) {
            self.push_event(FDomainEvent::WroteChannel(
                tid,
                Err(proto::WriteChannelError::Error(e)),
            ));
        }
    }

    pub fn wait_for_signals(
        &mut self,
        tid: NonZeroU32,
        request: proto::FDomainWaitForSignalsRequest,
    ) {
        let result = self.using_handle(request.handle, |h, _| {
            let signals = fidl::Signals::from_bits_retain(request.signals);
            h.signal_waiters.push(SignalWaiter {
                tid,
                waiter: Box::pin(OnSignals::new(AnyHandleRef(Arc::clone(&h.handle)), signals)),
            });
            Ok(())
        });

        if let Err(e) = result {
            self.push_event(FDomainEvent::WaitForSignals(tid, Err(e)));
        } else {
            self.waker.wake_by_ref();
        }
    }

    pub fn close(&mut self, tid: NonZeroU32, request: proto::FDomainCloseRequest) {
        let mut states = Vec::with_capacity(request.handles.len());
        let mut result = Ok(());
        for hid in request.handles {
            match self.take_handle(hid) {
                Ok(state) => states.push((hid, state)),

                Err(e) => {
                    result = result.and(Err(e));
                }
            }
        }

        let action = Arc::new(CloseAction::Close {
            tid,
            count: AtomicU32::new(states.len().try_into().unwrap()),
            result: result.clone(),
        });

        if states.is_empty() {
            self.push_event(FDomainEvent::ClosedHandle(tid, result));
        } else {
            for (hid, state) in states {
                self.closing_handles.push(ClosingHandle {
                    action: Arc::clone(&action),
                    state: Some(ShuttingDownHandle::InUse(hid, state)),
                });
            }
            self.waker.wake_by_ref();
        }
    }

    pub fn duplicate(&mut self, request: proto::FDomainDuplicateRequest) -> Result<()> {
        let rights = request.rights;
        let handle = self.using_handle(request.handle, |h, _| h.handle.duplicate(rights));
        handle.and_then(|h| self.alloc_client_handles([request.new_handle], [h]))
    }

    pub fn replace(
        &mut self,
        tid: NonZeroU32,
        request: proto::FDomainReplaceRequest,
    ) -> Result<()> {
        let rights = request.rights;
        let new_hid = request.new_handle;
        match self.take_handle(request.handle) {
            Ok(state) => {
                self.closing_handles.push(ClosingHandle {
                    action: Arc::new(CloseAction::Replace { tid, new_hid, rights }),
                    state: Some(ShuttingDownHandle::InUse(request.handle, state)),
                });
                self.waker.wake_by_ref();
            }
            Err(e) => self.push_event(FDomainEvent::ReplacedHandle(tid, Err(e))),
        }

        Ok(())
    }

    pub fn signal(&mut self, request: proto::FDomainSignalRequest) -> Result<()> {
        let set = fidl::Signals::from_bits_retain(request.set);
        let clear = fidl::Signals::from_bits_retain(request.clear);

        self.using_handle(request.handle, |h, _| h.handle.signal(clear, set))
    }

    pub fn signal_peer(&mut self, request: proto::FDomainSignalPeerRequest) -> Result<()> {
        let set = fidl::Signals::from_bits_retain(request.set);
        let clear = fidl::Signals::from_bits_retain(request.clear);

        self.using_handle(request.handle, |h, _| h.handle.signal_peer(clear, set))
    }

    pub fn get_koid(
        &mut self,
        request: proto::FDomainGetKoidRequest,
    ) -> Result<proto::FDomainGetKoidResponse> {
        self.using_handle(request.handle, |h, _| {
            h.handle
                .as_handle_ref()
                .koid()
                .map(|k| proto::FDomainGetKoidResponse { koid: k.raw_koid() })
                .map_err(|e| proto::Error::TargetError(e.into_raw()))
        })
    }

    pub fn read_channel_streaming_start(
        &mut self,
        tid: NonZeroU32,
        request: proto::ChannelReadChannelStreamingStartRequest,
    ) {
        if let Err(err) = self.using_handle(request.handle, |h, waker| {
            h.handle.expected_type(fidl::ObjectType::CHANNEL)?;
            h.read_queue.push_back(ReadOp::StreamingChannel(tid, true), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::ChannelStreamingReadStart(tid, Err(err)));
        }
    }

    pub fn read_channel_streaming_stop(
        &mut self,
        tid: NonZeroU32,
        request: proto::ChannelReadChannelStreamingStopRequest,
    ) {
        if let Err(err) = self.using_handle(request.handle, |h, waker| {
            h.handle.expected_type(fidl::ObjectType::CHANNEL)?;
            h.read_queue.push_back(ReadOp::StreamingChannel(tid, false), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::ChannelStreamingReadStop(tid, Err(err)));
        }
    }

    pub fn read_socket_streaming_start(
        &mut self,
        tid: NonZeroU32,
        request: proto::SocketReadSocketStreamingStartRequest,
    ) {
        if let Err(err) = self.using_handle(request.handle, |h, waker| {
            h.handle.expected_type(fidl::ObjectType::SOCKET)?;
            h.read_queue.push_back(ReadOp::StreamingSocket(tid, true), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::SocketStreamingReadStart(tid, Err(err)));
        }
    }

    pub fn read_socket_streaming_stop(
        &mut self,
        tid: NonZeroU32,
        request: proto::SocketReadSocketStreamingStopRequest,
    ) {
        if let Err(err) = self.using_handle(request.handle, |h, waker| {
            h.handle.expected_type(fidl::ObjectType::SOCKET)?;
            h.read_queue.push_back(ReadOp::StreamingSocket(tid, false), waker);
            Ok(())
        }) {
            self.push_event(FDomainEvent::SocketStreamingReadStop(tid, Err(err)));
        }
    }
}

/// [`FDomain`] implements a stream of events, for protocol events and for
/// replies to long-running methods.
impl futures::Stream for FDomain {
    type Item = FDomainEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        ctx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let this = &mut *self;

        let mut closing_handles = std::mem::replace(&mut this.closing_handles, Vec::new());
        closing_handles.retain_mut(|x| x.poll_ready(this, ctx).is_pending());
        this.closing_handles = closing_handles;

        let handles = &mut this.handles;
        let event_queue = &mut this.event_queue;
        for state in handles.values_mut() {
            state.poll(event_queue, ctx);
        }

        if let Some(event) = self.event_queue.pop_front() {
            match event {
                UnprocessedFDomainEvent::Ready(event) => Poll::Ready(Some(event)),
                UnprocessedFDomainEvent::ChannelData(tid, message) => {
                    Poll::Ready(Some(FDomainEvent::ChannelData(tid, self.process_message(message))))
                }
                UnprocessedFDomainEvent::ChannelStreamingData(hid, message) => {
                    match self.process_message(message) {
                        Ok(message) => Poll::Ready(Some(FDomainEvent::ChannelStreamingData(
                            proto::ChannelOnChannelStreamingDataRequest {
                                handle: hid,
                                channel_sent: proto::ChannelSent::Message(message),
                            },
                        ))),
                        Err(e) => {
                            self.handles.get_mut(&hid).unwrap().async_read_in_progress = false;
                            Poll::Ready(Some(FDomainEvent::ChannelStreamingData(
                                proto::ChannelOnChannelStreamingDataRequest {
                                    handle: hid,
                                    channel_sent: proto::ChannelSent::Stopped(proto::AioStopped {
                                        error: Some(Box::new(e)),
                                    }),
                                },
                            )))
                        }
                    }
                }
            }
        } else {
            self.waker = ctx.waker().clone();
            Poll::Pending
        }
    }
}
