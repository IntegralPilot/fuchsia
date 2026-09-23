// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stream socket buffer support types.

use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll};

use assert_matches::assert_matches;
use async_ringbuf::traits::{
    AsyncConsumer as _, AsyncProducer, Consumer as _, Observer as _, Producer as _, Split as _,
};
use fuchsia_async::{self as fasync, ReadableHandle as _, WritableHandle as _};
use log::{debug, warn};
use pin_project::pin_project;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};
use netstack3_core::IpExt;
use netstack3_core::socket::ShutdownType;
use netstack3_core::tcp::{
    Buffer, BufferLimits, FragmentedPayload, NoConnection, Payload, ReceiveBuffer, SendBuffer,
};

use super::TcpSocketId;
use crate::bindings::util::DataNotifier;
use crate::bindings::{BindingsCtx, Ctx};

// Consider buffers idle after this much time with no data.
const TCP_IDLE_BUFFER_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);

/// Error emitted when new buffers are instantiated but there's no send/recv
/// task listening on the other side.
#[derive(Debug)]
struct TaskStoppedError;

#[derive(Debug)]
pub(crate) struct CoreReceiveBuffer {
    inner: CoreReceiveBufferInner,
    notifier: Option<DataNotifier>,
    new_buffer_sender: mpsc::UnboundedSender<ReceiveBufferReader>,
}

pub(crate) enum CoreReceiveBufferInner {
    Unallocated { capacity: usize },
    Ready { buffer: async_ringbuf::AsyncHeapProd<u8>, empty: bool, pending_capacity: Option<usize> },
    Defunct,
}

pub(super) type ReceiveBufferReader = async_ringbuf::AsyncHeapCons<u8>;

impl CoreReceiveBuffer {
    pub(super) fn new(
        new_buffer_sender: mpsc::UnboundedSender<ReceiveBufferReader>,
        capacity: usize,
        notifier: Option<DataNotifier>,
    ) -> Self {
        Self {
            inner: CoreReceiveBufferInner::Unallocated { capacity },
            notifier,
            new_buffer_sender,
        }
    }

    fn alloc_new_buffer(
        capacity: usize,
        new_buffer_sender: &mpsc::UnboundedSender<ReceiveBufferReader>,
    ) -> Result<async_ringbuf::AsyncHeapProd<u8>, TaskStoppedError> {
        let ring_buffer = async_ringbuf::AsyncHeapRb::new(capacity);
        let (prod, cons) = ring_buffer.split();
        new_buffer_sender
            .unbounded_send(cons)
            .map_err(|_: mpsc::TrySendError<_>| TaskStoppedError)?;
        Ok(prod)
    }

    pub(super) fn set_notifier(&mut self, notifier: DataNotifier) {
        assert_matches!(self.notifier.replace(notifier), None);
    }

    fn maybe_update_capacity(&mut self) {
        let Self { inner, notifier: _, new_buffer_sender } = self;
        match inner {
            CoreReceiveBufferInner::Unallocated { capacity: _ }
            | CoreReceiveBufferInner::Defunct => {}
            CoreReceiveBufferInner::Ready { buffer: _, empty, pending_capacity } => {
                // When we turn to empty and we have a pending capacity request,
                // update our buffers.
                let (true, Some(cap)) = (*empty, pending_capacity.as_ref()) else {
                    return;
                };
                match Self::alloc_new_buffer(*cap, new_buffer_sender) {
                    Ok(new_buffer) => {
                        *inner = CoreReceiveBufferInner::Ready {
                            buffer: new_buffer,
                            empty: true,
                            pending_capacity: None,
                        };
                    }
                    // If the task is stopped give up attempting to update
                    // the capacity.
                    Err(TaskStoppedError) => {
                        *pending_capacity = None;
                    }
                }
            }
        }
    }

    fn try_dealloc(&mut self) {
        let Self { inner, notifier: _, new_buffer_sender: _ } = self;
        let stash_capacity = match inner {
            CoreReceiveBufferInner::Unallocated { capacity: _ }
            | CoreReceiveBufferInner::Defunct => {
                // Nothing to do.
                return;
            }
            CoreReceiveBufferInner::Ready { buffer, empty, pending_capacity } => {
                if !*empty {
                    // There's possibly out of order data in the buffer, can't
                    // get rid of it.
                    return;
                }

                if !buffer.is_empty() {
                    // The reader is not caught up to the writer.
                    return;
                }

                pending_capacity.unwrap_or_else(|| buffer.capacity().get())
            }
        };
        *inner = CoreReceiveBufferInner::Unallocated { capacity: stash_capacity };
    }
}

impl Debug for CoreReceiveBufferInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreReceiveBufferInner::Unallocated { capacity } => f
                .debug_struct("CoreReceiveBufferInner::Unallocated")
                .field("capacity", capacity)
                .finish_non_exhaustive(),
            CoreReceiveBufferInner::Ready { buffer, empty, pending_capacity } => f
                .debug_struct("CoreReceiveBufferInner::Ready")
                .field("buffer_capacity", &buffer.capacity())
                .field("empty", &empty)
                .field("pending_capacity", &pending_capacity)
                .finish_non_exhaustive(),
            CoreReceiveBufferInner::Defunct => {
                f.debug_struct("CoreReceiveBufferInner::Defunct").finish()
            }
        }
    }
}

impl Buffer for CoreReceiveBuffer {
    fn limits(&self) -> BufferLimits {
        match &self.inner {
            CoreReceiveBufferInner::Unallocated { capacity } => {
                BufferLimits { capacity: *capacity, len: 0 }
            }
            CoreReceiveBufferInner::Ready { buffer, empty: _, pending_capacity: _ } => {
                let len = buffer.occupied_len();
                let capacity = buffer.capacity().into();
                BufferLimits { capacity, len }
            }
            CoreReceiveBufferInner::Defunct => BufferLimits { capacity: 0, len: 0 },
        }
    }

    fn target_capacity(&self) -> usize {
        match &self.inner {
            CoreReceiveBufferInner::Unallocated { capacity } => *capacity,
            CoreReceiveBufferInner::Ready { buffer, empty: _, pending_capacity } => {
                pending_capacity.as_ref().copied().unwrap_or_else(|| buffer.capacity().into())
            }
            CoreReceiveBufferInner::Defunct => 0,
        }
    }

    fn request_capacity(&mut self, size: usize) {
        match &mut self.inner {
            CoreReceiveBufferInner::Unallocated { capacity } => {
                *capacity = size;
            }
            CoreReceiveBufferInner::Ready { pending_capacity, .. } => {
                *pending_capacity = Some(size);
            }
            CoreReceiveBufferInner::Defunct => {}
        }
        self.maybe_update_capacity();
    }
}

/// Helper function to implement [`ReceiveBuffer`] for [`CoreReceiveBuffer`].
///
/// Returns the number of bytes from `payload` at `payload_offset` written into
/// `target` at `target_offset`.
fn write_payload<P: Payload>(
    target_offset: usize,
    payload_offset: usize,
    target: &mut [MaybeUninit<u8>],
    payload: &P,
) -> usize {
    if target_offset > target.len() {
        return 0;
    }
    let end = target.len().min(target_offset + payload.len() - payload_offset);
    let target = &mut target[target_offset..end];
    if target.is_empty() {
        // Avoid calling into payload if we have nothing to offer as a target.
        return 0;
    }
    payload.partial_copy_uninit(payload_offset, target);
    target.len()
}

impl ReceiveBuffer for CoreReceiveBuffer {
    fn write_at<P: Payload>(&mut self, offset: usize, data: &P) -> usize {
        let Self { inner, notifier: _, new_buffer_sender } = self;
        match inner {
            CoreReceiveBufferInner::Defunct => 0,
            CoreReceiveBufferInner::Unallocated { capacity } => {
                // We don't have any buffer space yet. Allocate the capacity
                // that we've promised via the API and write into the newly
                // created buffer.
                let new_buffer = match Self::alloc_new_buffer(*capacity, new_buffer_sender) {
                    Ok(buffer) => buffer,
                    Err(TaskStoppedError) => {
                        debug!("failed to allocate buffer, rx task is stopped");
                        *inner = CoreReceiveBufferInner::Defunct;
                        return 0;
                    }
                };
                *inner = CoreReceiveBufferInner::Ready {
                    buffer: new_buffer,
                    empty: false,
                    pending_capacity: None,
                };
                // Recurse, in our new state we can accept the data.
                self.write_at(offset, data)
            }
            CoreReceiveBufferInner::Ready { buffer, empty, pending_capacity: _ } => {
                let (a, b) = buffer.vacant_slices_mut();
                let mut written = write_payload(offset, 0, a, data);
                if let Some(offset) = (offset + written).checked_sub(a.len()) {
                    written += write_payload(offset, written, b, data);
                }
                // We must update `empty` here since we can't guarantee
                // `make_readable` will be called to update `empty`. So until it
                // does, we must assume that empty can only go from
                // `true->false` here whenever we write any bytes to the buffer.
                *empty &= written == 0;
                written
            }
        }
    }

    fn make_readable(&mut self, count: usize, has_outstanding: bool) {
        let Self { inner, notifier, new_buffer_sender: _ } = self;
        match inner {
            CoreReceiveBufferInner::Defunct => {}
            CoreReceiveBufferInner::Unallocated { capacity: _ } => {
                unreachable!("unallocated buffer can't be marked readable")
            }
            CoreReceiveBufferInner::Ready { buffer, empty, pending_capacity: _ } => {
                // TODO(https://fxbug.dev/440396857): fix the race condition where incoming data
                // may not have been written into the zircon socket before the client is
                // notified of the data being available.
                if let Some(notifier) = notifier {
                    notifier.notify();
                }

                // `empty` is tracking whether the *producer* side of the buffer
                // is empty, i.e., no outstanding bytes so we can always
                // overwrite with what the assembler tells us. The receive task
                // is responsible to drain every *consumer* side before moving
                // on to the next in case of capacity updates.
                *empty = !has_outstanding;
                // SAFETY:
                // - Not called concurrently (we hold a mutable reference here).
                // - Per ReceiveBuffer contract, all the bytes must've been
                //   initialized by write_at prior to marking them as readable,
                //   otherwise we'd be sending garbage to the application either
                //   way.
                unsafe { buffer.advance_write_index(count) };
                self.maybe_update_capacity();
            }
        }
    }
}

/// Abstracts [`Ctx`] and socket operations so [`receive_task`] can be tested
/// without core.
///
/// [`ReceiveTaskArgs`] is the proper production impl.
pub(super) trait ReceiveTaskOps {
    fn shutdown_recv(&mut self) -> Result<bool, NoConnection>;
    fn on_receive_buffer_read(&mut self);
    fn with_receive_buffer<F, R>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&mut CoreReceiveBuffer) -> R;
    fn has_send_buffer(&mut self) -> bool;
}

pub(super) struct ReceiveTaskArgs<I: IpExt> {
    pub(super) ctx: Ctx,
    pub(super) id: TcpSocketId<I>,
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
impl<I: IpExt> ReceiveTaskOps for ReceiveTaskArgs<I> {
    fn shutdown_recv(&mut self) -> Result<bool, NoConnection> {
        let Self { ctx, id } = self;
        ctx.api().tcp().shutdown(id, ShutdownType::Receive)
    }

    fn on_receive_buffer_read(&mut self) {
        let Self { ctx, id } = self;
        ctx.api().tcp().on_receive_buffer_read(id)
    }

    fn with_receive_buffer<F, R>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&mut CoreReceiveBuffer) -> R,
    {
        let Self { ctx, id } = self;
        ctx.api().tcp().with_receive_buffer(id, f)
    }

    fn has_send_buffer(&mut self) -> bool {
        let Self { ctx, id } = self;
        ctx.api().tcp().with_send_buffer(id, |_| ()).is_some()
    }
}

/// Shuttles bytes from the core buffers into a zircon socket.
pub(super) async fn receive_task<O: ReceiveTaskOps>(
    socket: Arc<zx::Socket>,
    mut ops: O,
    mut receiver: mpsc::UnboundedReceiver<ReceiveBufferReader>,
) {
    let handle = fasync::RWHandle::new(&*socket);
    let timer = fasync::Timer::new(zx::MonotonicInstant::INFINITE);
    let mut timer = pin!(timer);
    let mut can_wait_for_idle = false;
    while let Some(mut buffer) = receiver.next().await {
        loop {
            let (a, b) = buffer.as_mut_slices();
            let avail = a.len() + b.len();
            // If there is no data for us to write into the zx socket, wait for
            // data to be written into the buffer by core as it receives
            // segments.
            if avail == 0 {
                if buffer.is_closed() {
                    // Curb a possible ring buffer race here. The producer side
                    // can write some data and then close so we need to
                    // double-check that the buffer is empty before dropping the
                    // consumer side to avoid data loss.
                    if buffer.is_empty() {
                        break;
                    } else {
                        continue;
                    }
                }

                let timer_fut = if can_wait_for_idle {
                    timer.as_mut().reset(fasync::MonotonicInstant::after(TCP_IDLE_BUFFER_TIMEOUT));
                    timer.as_mut().left_future()
                } else {
                    futures::future::pending().right_future()
                };
                // Unblock either when we have some data in the buffer or we hit idle timeout.
                let fut = futures::future::select(buffer.wait_occupied(1), timer_fut);
                match fut.await {
                    futures::future::Either::Left(((), _timer)) => {
                        // We got more data in the buffer.
                    }
                    futures::future::Either::Right(((), _wait_occupied)) => {
                        // Timed out. Try to get rid of this buffer.
                        let _: Option<()> = ops.with_receive_buffer(|b| b.try_dealloc());
                        // Regardless of whether this actually deallocated the
                        // buffer or not, we should only attempt a dealloc by
                        // idle again after we see some more data coming in.
                        can_wait_for_idle = false;
                    }
                }
                // Loop again, these are the exit conditions.
                //
                // 1. We got more data in the buffer, we'll proceed as normal.
                // 2. The idle timer expired and we successfully deallocated the
                //    buffer. The buffer is observed as closed early in the loop
                //    and we move to waiting for a new buffer.
                // 3. The idle timer expired, but we failed to deallocate the
                //    buffer because more data came in. We can proceed as
                //    normal, looping again to consume the new data.
                continue;
            }

            // We've seen data coming in from core, we can wait for idle again.
            can_wait_for_idle = true;
            let a_written = if a.len() != 0 {
                futures::future::poll_fn(|ctx| {
                    loop {
                        futures::ready!(
                            handle.poll_writable(ctx).map(|r| r.expect("poll writable"))
                        );
                        let res = handle.get_ref().write(a).map_err(SocketErrorAction::from);
                        match res {
                            Err(SocketErrorAction::Wait) => {
                                futures::ready!(
                                    handle
                                        .need_writable(ctx)
                                        .map(|r| r.expect("waiting for writable"))
                                )
                            }
                            Err(SocketErrorAction::Shutdown) => return Poll::Ready(None),
                            Ok(written) => return Poll::Ready(Some(written)),
                        }
                    }
                })
                .await
            } else {
                Some(0)
            };
            let b_written = if a_written == Some(a.len()) && b.len() != 0 {
                // If we wrote everything into the socket then attempt a
                // non-waiting write from b.
                match handle.get_ref().write(b).map_err(SocketErrorAction::from) {
                    Ok(v) => Some(v),
                    Err(SocketErrorAction::Wait) => Some(0),
                    Err(SocketErrorAction::Shutdown) => None,
                }
            } else {
                Some(0)
            };

            let total_written = match (a_written, b_written) {
                (Some(a_written), Some(b_written)) => a_written + b_written,
                _ => {
                    // No more bytes can be written. Close the receive buffer, so our peer
                    // knows we can't read anymore. Then shutdown the task.
                    let _ = ops.shutdown_recv();
                    return;
                }
            };

            // Notify core that we've dequeued some bytes in case it wants to send an
            // updated window.
            if total_written > 0 {
                // NB: `skip` is a weird name for this method, it calls drop for all
                // the consumed bytes and then advances the read index.
                assert_eq!(
                    async_ringbuf::traits::Consumer::skip(&mut buffer, total_written),
                    total_written
                );

                ops.on_receive_buffer_read();
            }
        }
    }
    // If core has dropped its sender it means we're not receiving data anymore.
    // Notify applications so they're aware of it only if core is still holding
    // on to the send buffer. If it's not, then the send task should also exit
    // and the signal we want to send out is PEER_CLOSED instead.
    if ops.has_send_buffer() {
        socket
            .set_disposition(
                /* disposition */ Some(zx::SocketWriteDisposition::Disabled),
                /* peer_disposition */ None,
            )
            .expect("failed to set socket disposition");
    }
}

enum CoreSendBufferInner {
    Unallocated {
        capacity: usize,
    },
    Ready {
        buffer: async_ringbuf::AsyncHeapCons<u8>,
        pending_capacity: Option<usize>,
    },
    /// Socket is shutting down send.
    ///
    /// When shutting down, the buffer may carry an additional vec `extra` that
    /// is used to read _once_ from the zircon socket so that all the data is
    /// readily available to core as the shutdown is processed. `extra` is
    /// treated as data that is read _after_ `buffer`. Once in `ShuttingDown`
    /// the `CoreSendBuffer` must _not_ take in any more bytes, only reading is
    /// allowed.
    ShuttingDown {
        buffer: async_ringbuf::AsyncHeapCons<u8>,
        extra: Vec<u8>,
        extra_offset: usize,
        target_capacity: usize,
    },
}

impl CoreSendBufferInner {
    fn new_ready(capacity: usize) -> (Self, SendBufferWriter) {
        let ring_buffer = async_ringbuf::AsyncHeapRb::new(capacity);
        let (producer, cons) = ring_buffer.split();
        (Self::Ready { buffer: cons, pending_capacity: None }, SendBufferWriter { producer })
    }
}

struct SendBufferWriter {
    producer: async_ringbuf::AsyncHeapProd<u8>,
}

impl Debug for CoreSendBufferInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unallocated { capacity } => f
                .debug_struct("CoreSendBufferInner::Unallocated")
                .field("capacity", capacity)
                .finish(),
            Self::Ready { buffer, pending_capacity } => f
                .debug_struct("CoreSendBufferInner::Ready")
                .field("buffer_capacity", &buffer.capacity())
                .field("pending_capacity", pending_capacity)
                .finish_non_exhaustive(),
            Self::ShuttingDown { buffer, extra, extra_offset, target_capacity } => f
                .debug_struct("CoreSendBufferInner::ShuttingDown")
                .field("buffer_capacity", &buffer.capacity())
                .field("target_capacity", target_capacity)
                .field("extra", &extra.len())
                .field("extra_offset", extra_offset)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct CoreSendBuffer {
    inner: CoreSendBufferInner,
    signal_sender: TxTaskSender,
}

#[derive(Debug)]
pub(super) struct TxTaskSender(mpsc::Sender<()>);

#[derive(Debug)]
pub(super) struct TxTaskReceiver(mpsc::Receiver<()>);

impl TxTaskSender {
    pub(super) fn new() -> (Self, TxTaskReceiver) {
        // NOTE: The signal is only used for pending capacity request, so we can
        // live with a channel with a single buffer slot.
        let (sender, receiver) = mpsc::channel(1);
        (Self(sender), TxTaskReceiver(receiver))
    }

    fn signal(&mut self) {
        let Self(sender) = self;
        match sender.try_send(()) {
            Ok(()) => {}
            Err(e) if e.is_disconnected() => {
                // Send task can have stopped while we're signaling, just allow
                // this to happen.
            }
            Err(e) if e.is_full() => {
                // The task is already notified to do work but hasn't gotten
                // around to doing it.
                //
                // We're permissive here because requests for capacity updates
                // may race with the decision to deallocate or reallocate buffer
                // space.
            }
            // The API doesn't allow us to match exhaustively on all types of
            // errors, so this is a catch for errors that are not handled by the
            // boolean checks above.
            Err(e) => unreachable!("unexpected error {e:?}"),
        }
    }
}

struct NoPendingCapacityRequestError;

impl CoreSendBuffer {
    pub(super) fn new(capacity: usize, signal_sender: TxTaskSender) -> Self {
        Self { inner: CoreSendBufferInner::Unallocated { capacity }, signal_sender }
    }

    /// Applies a pending capacity request to the buffer.
    ///
    /// Returns a new buffer if there was a pending capacity update request.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is shutting down or if there is an allocated buffer
    /// that is not fully drained.
    fn apply_new_capacity(&mut self) -> Result<SendBufferWriter, NoPendingCapacityRequestError> {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::ShuttingDown { .. } => {
                panic!("apply_new_capacity: bad buffer state {inner:?}")
            }
            CoreSendBufferInner::Unallocated { .. } => Err(NoPendingCapacityRequestError),
            CoreSendBufferInner::Ready { buffer, pending_capacity } => {
                // Must not be called by the send task before waiting for the
                // buffer to be fully drained.
                assert!(buffer.is_empty());

                let new_capacity = pending_capacity.ok_or(NoPendingCapacityRequestError)?;
                let (new_buffer, writer) = CoreSendBufferInner::new_ready(new_capacity);
                *inner = new_buffer;
                Ok(writer)
            }
        }
    }

    /// Allocates a new buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is already allocated.
    fn allocate(&mut self) -> SendBufferWriter {
        let Self { inner, signal_sender: _ } = self;
        let capacity = match inner {
            CoreSendBufferInner::Unallocated { capacity } => *capacity,
            CoreSendBufferInner::ShuttingDown { .. } | CoreSendBufferInner::Ready { .. } => {
                panic!("allocate: bad buffer state {self:?}")
            }
        };
        let (new_buffer, writer) = CoreSendBufferInner::new_ready(capacity);
        *inner = new_buffer;
        writer
    }

    /// Deallocates the backing memory for this buffer.
    ///
    /// # Panics
    ///
    /// Panics if the buffer is already deallocated, shutting down, or not empty.
    fn dealloc(&mut self) {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::Unallocated { .. } | CoreSendBufferInner::ShuttingDown { .. } => {
                panic!("dealloc: bad buffer state {self:?}")
            }
            CoreSendBufferInner::Ready { buffer, pending_capacity } => {
                assert!(buffer.is_empty());
                let capacity = pending_capacity.unwrap_or_else(|| buffer.capacity().get());
                *inner = CoreSendBufferInner::Unallocated { capacity };
            }
        }
    }
}

impl Buffer for CoreSendBuffer {
    fn limits(&self) -> BufferLimits {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::Unallocated { capacity } => {
                BufferLimits { capacity: *capacity, len: 0 }
            }
            CoreSendBufferInner::Ready { buffer, pending_capacity: _ } => {
                let len = buffer.occupied_len();
                let capacity = buffer.capacity().get();
                BufferLimits { capacity, len }
            }
            CoreSendBufferInner::ShuttingDown {
                buffer,
                extra,
                extra_offset,
                target_capacity: _,
            } => {
                let len = buffer.occupied_len() + extra.len() - *extra_offset;
                let capacity = buffer.capacity().get() + extra.len();
                BufferLimits { capacity, len }
            }
        }
    }

    fn target_capacity(&self) -> usize {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::Unallocated { capacity } => *capacity,
            CoreSendBufferInner::Ready { buffer, pending_capacity } => match pending_capacity {
                None => buffer.capacity().into(),
                Some(r) => *r,
            },
            CoreSendBufferInner::ShuttingDown {
                buffer: _,
                extra: _,
                extra_offset: _,
                target_capacity,
            } => *target_capacity,
        }
    }

    fn request_capacity(&mut self, size: usize) {
        let Self { inner, signal_sender } = self;
        match inner {
            CoreSendBufferInner::Unallocated { capacity } => {
                *capacity = size;
            }
            CoreSendBufferInner::Ready { buffer: _, pending_capacity } => {
                match pending_capacity.replace(size) {
                    None => {
                        // Notify that we'd like to close the current ring
                        // buffer and replace it with a different capacity. The
                        // send task is responsible to listen for this signal
                        // and update the capacity when ready.
                        signal_sender.signal();
                    }
                    Some(_) => {}
                }
            }
            CoreSendBufferInner::ShuttingDown {
                buffer: _,
                extra: _,
                extra_offset: _,
                target_capacity,
            } => {
                // When shutting down just store the user request to prevent
                // weirdness over the API but there's no way to fulfill it.
                *target_capacity = size;
            }
        }
    }
}

impl SendBuffer for CoreSendBuffer {
    type Payload<'a> = FragmentedPayload<'a, 3>;
    fn mark_read(&mut self, count: usize) {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::Unallocated { capacity: _ } => {}
            CoreSendBufferInner::Ready { buffer, pending_capacity: _ } => {
                assert_eq!(async_ringbuf::traits::Consumer::skip(buffer, count), count);
            }
            CoreSendBufferInner::ShuttingDown {
                buffer,
                extra,
                extra_offset,
                target_capacity: _,
            } => {
                // The producer must've been closed by the send task before
                // anything happens on this buffer. This ensures when we're
                // reading the buffer length we're not racing with anything
                // else.
                assert!(buffer.is_closed());
                let count = count - async_ringbuf::traits::Consumer::skip(buffer, count);
                *extra_offset += count;
                assert!(
                    *extra_offset <= extra.len(),
                    "{extra_offset} exceed available length {}",
                    extra.len()
                );
            }
        }
    }

    fn peek_with<'a, F, R>(&'a mut self, offset: usize, f: F) -> R
    where
        F: FnOnce(Self::Payload<'a>) -> R,
    {
        let Self { inner, signal_sender: _ } = self;
        match inner {
            CoreSendBufferInner::Unallocated { capacity: _ } => {
                return f(FragmentedPayload::new_empty());
            }
            CoreSendBufferInner::Ready { buffer, pending_capacity: _ } => {
                let (a, b) = buffer.as_slices();
                if let Some(offset) = offset.checked_sub(a.len()) {
                    f(FragmentedPayload::new_contiguous(&b[offset..]))
                } else {
                    f(FragmentedPayload::from_iter([&a[offset..], b]))
                }
            }
            CoreSendBufferInner::ShuttingDown {
                buffer,
                extra,
                extra_offset,
                target_capacity: _,
            } => {
                let (a, b) = buffer.as_slices();
                let extra = &extra[*extra_offset..];
                match offset.checked_sub(a.len()) {
                    None => {
                        // Offset is in a.
                        f(FragmentedPayload::new([&a[offset..], b, extra]))
                    }
                    Some(b_offset) => match b_offset.checked_sub(b.len()) {
                        // Offset is in b.
                        None => f(FragmentedPayload::from_iter([&b[b_offset..], extra])),
                        // Offset is in extra.
                        Some(extra_offset) => {
                            f(FragmentedPayload::new_contiguous(&extra[extra_offset..]))
                        }
                    },
                }
            }
        }
    }
}

/// Abstracts [`Ctx`] and socket operations so [`send_task`] can be tested
/// without core.
///
/// [`SendTaskArgs`] is the proper production impl.
pub(super) trait SendTaskOps {
    fn do_send(&mut self);
    fn with_send_buffer<R, F: FnOnce(&mut CoreSendBuffer) -> R>(&mut self, f: F) -> Option<R>;
}

pub(super) struct SendTaskArgs<I: IpExt> {
    pub(super) ctx: Ctx,
    pub(super) id: TcpSocketId<I>,
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
impl<I: IpExt> SendTaskOps for SendTaskArgs<I> {
    fn do_send(&mut self) {
        let Self { ctx, id } = self;
        ctx.api().tcp().do_send(id);
    }

    fn with_send_buffer<R, F: FnOnce(&mut CoreSendBuffer) -> R>(&mut self, f: F) -> Option<R> {
        let Self { ctx, id } = self;
        ctx.api().tcp().with_send_buffer(id, f)
    }
}

/// Shuttles bytes from the zircon socket into core buffers.
pub(super) async fn send_task<O: SendTaskOps>(
    socket: Arc<zx::Socket>,
    mut ops: O,
    shutdown_receiver: oneshot::Receiver<oneshot::Sender<()>>,
    TxTaskReceiver(mut tx_task_receiver): TxTaskReceiver,
) {
    let handle = fasync::RWHandle::new(&*socket);
    let mut cur_buffer = None;
    let send_loop = async {
        loop {
            let buffer = match cur_buffer.as_mut() {
                Some(b) => b,
                None => {
                    let next_buffer = futures::select! {
                        b = wait_and_alloc_send_buffer(&handle, &mut ops).fuse() => b,
                        s = tx_task_receiver.next() => {
                            match s {
                                Some(()) => {
                                    // If we decided to deallocate a buffer
                                    // while a signal was pending on the
                                    // receiver we may have this pending signal
                                    // here. Ignore it and loop again to
                                    // allocate a new buffer.
                                    continue;
                                },
                                None => None,
                            }
                        }
                    };
                    match next_buffer {
                        Some(b) => cur_buffer.insert(b),
                        None => {
                            return;
                        }
                    }
                }
            };

            futures::select! {
                r = DriveSendBufferFut::new(&handle, &mut ops, buffer).fuse() => {
                    match r {
                        DriveSendBufferResult::Shutdown => {
                            return;
                        }
                        DriveSendBufferResult::NoBuffer => {
                            // Discard our current buffer and go back to the
                            // top.
                            cur_buffer = None;
                            continue;
                        }
                    }
                }
                s = tx_task_receiver.next() => {
                    match s {
                        // Attempt a capacity update,
                        Some(()) => (),
                        None => {
                            // Core has dropped the buffer, nothing else to do.
                            return;
                        },
                    }
                }
            }

            // When there's a pending buffer capacity update, wait for the
            // network flush without polling the zircon socket anymore and then
            // update the send buffer in the socket.

            // If all the capacity is vacant, that means everything has been
            // flushed to the network.
            let SendBufferWriter { producer } = buffer;
            producer.wait_vacant(producer.capacity().into()).await;
            match ops.with_send_buffer(|b| b.apply_new_capacity()) {
                Some(Ok(b)) => {
                    // We got a new buffer, loop through to start polling on it.
                    cur_buffer = Some(b);
                }
                Some(Err(NoPendingCapacityRequestError)) => {
                    // Don't need to change the buffer, continue driving the
                    // current one.
                }
                None => {
                    // Core got rid of its buffer, there's nothing else for us
                    // to do here.
                    return;
                }
            }
        }
    }
    .fuse();

    // We don't want to react to bindings dropping the shutdown sender, so
    // ensure we can't observe that future terminating in case that was dropped.
    let shutdown_receiver = shutdown_receiver.then(|x| async move {
        match x {
            Ok(sender) => sender,
            Err(oneshot::Canceled) => futures::future::pending().await,
        }
    });

    let signal_sender = {
        let mut send_loop = pin!(send_loop);
        let mut shutdown_receiver = pin!(shutdown_receiver);
        // Select biasing on the send loop, if that finishes we can just ignore
        // any outside shutdown requests.
        futures::select_biased! {
            () = send_loop => {
                // Send loop is over we can stop.
                return;
            }
            signal = shutdown_receiver => signal,
        }
    };

    // If we get here, it means application is requesting a send shutdown so we
    // must flush everything we can from the zircon socket and make it available
    // to core before responding and exiting from the send task.
    send_task_shutdown(socket, ops, cur_buffer);

    // Notify the bindings task that shutdown has finished. This may fail if the
    // the bindings task was cancelled, e.g. when Netstack is shutting down.
    match signal_sender.send(()) {
        Ok(()) => {}
        Err(()) => warn!("stream socket shutdown receiver closed unexpectedly"),
    }
}

enum DriveSendBufferResult {
    Shutdown,
    NoBuffer,
}

/// A hand-rolled future to drive the send buffer.
///
/// It is easier to reason about the necessary lifetimes via a hand-rolled
/// future.
///
/// Note that this future may be interrupted and dropped at _any point_ when new
/// capacity updates come in, so it must not yield between reading data from the
/// socket and advancing the buffer's write pointer.
#[pin_project(project = DriveSendBufferFutProj)]
struct DriveSendBufferFut<'a, O> {
    handle: &'a fasync::RWHandle<&'a zx::Socket>,
    ops: &'a mut O,
    buffer: &'a mut SendBufferWriter,
    last_write: fasync::MonotonicInstant,
    idle_timer_deadline: fasync::MonotonicInstant,
    #[pin]
    idle_timer: fasync::Timer,
}

impl<'a, O> DriveSendBufferFut<'a, O> {
    fn new(
        handle: &'a fasync::RWHandle<&'a zx::Socket>,
        ops: &'a mut O,
        buffer: &'a mut SendBufferWriter,
    ) -> Self {
        let last_write = fasync::MonotonicInstant::now();
        let idle_timer_deadline = zx::MonotonicInstant::INFINITE.into();
        let idle_timer = fasync::Timer::new(idle_timer_deadline);
        Self { handle, ops, buffer, last_write, idle_timer_deadline, idle_timer }
    }
}

impl<'a, O: SendTaskOps> Future for DriveSendBufferFut<'a, O> {
    type Output = DriveSendBufferResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let DriveSendBufferFutProj {
            handle,
            ops,
            buffer,
            last_write,
            idle_timer_deadline,
            mut idle_timer,
        } = self.project();
        let SendBufferWriter { producer: buffer } = buffer;

        loop {
            // If the buffer we're looking at has been closed by
            // core we should no longer be attempting to put more
            // bytes into it, even if there's space available.
            if buffer.is_closed() {
                return Poll::Ready(DriveSendBufferResult::NoBuffer);
            }

            let poll_socket = match handle.poll_readable(cx) {
                Poll::Ready(r) => {
                    let _: fasync::ReadableState = r.expect("poll readable");
                    let (a, b) = buffer.vacant_slices_mut();
                    let avail = a.len() + b.len();
                    // If we don't have any space to use, wait for free
                    // space as we send segments out.
                    if avail == 0 {
                        // NB: The wait vacant future holds no state, it is
                        // cheap enough to create a new future at every turn and
                        // poll once.
                        futures::ready!(buffer.wait_vacant(1).poll_unpin(cx));
                        continue;
                    }

                    assert_ne!(a.len(), 0);
                    let res = handle.get_ref().read_uninit(a).map_err(SocketErrorAction::from);
                    match res {
                        Err(SocketErrorAction::Wait) => {
                            match handle.need_readable(cx) {
                                Poll::Ready(r) => {
                                    // `need_readable` should not fail for
                                    // valid sockets with the correct
                                    // rights.
                                    r.expect("need readable");
                                    continue;
                                }
                                Poll::Pending => None,
                            }
                        }
                        Err(SocketErrorAction::Shutdown) => {
                            return Poll::Ready(DriveSendBufferResult::Shutdown);
                        }
                        Ok(a_read) => {
                            let a_read = a_read.len();
                            Some((a_read, a, b))
                        }
                    }
                }
                Poll::Pending => None,
            };

            if let Some((a_read, a, b)) = poll_socket {
                let b_read = if a_read == a.len() && b.len() != 0 {
                    // If we wrote everything into the first slice then attempt
                    // a non-waiting read into b.
                    match handle.get_ref().read_uninit(b).map_err(SocketErrorAction::from) {
                        Ok(read) => read.len(),
                        Err(SocketErrorAction::Wait) => 0,
                        Err(SocketErrorAction::Shutdown) => {
                            return Poll::Ready(DriveSendBufferResult::Shutdown);
                        }
                    }
                } else {
                    0
                };

                let total_read = a_read + b_read;
                // SAFETY: slices a and b have been initialized by zircon socket
                // reading up to the returned slice length. Buffer is
                // exclusively owned by this function.
                unsafe { buffer.advance_write_index(total_read) }

                assert!(total_read != 0);
                *last_write = fasync::MonotonicInstant::now();
                ops.do_send();

                continue;
            }

            // If we get here our socket is idle.
            let deadline = *last_write + TCP_IDLE_BUFFER_TIMEOUT;

            // The timer deadline is stashed to avoid resetting the timer more
            // often than needed.
            if deadline != *idle_timer_deadline {
                *idle_timer_deadline = deadline;
                idle_timer.as_mut().reset(deadline);
            }
            // NB: Timer allows us to poll it without crashing even after it's
            // complete. We're avoiding a second atomic read of the timer state
            // by not using its FusedFuture implementation.
            futures::ready!(idle_timer.as_mut().poll(cx));

            // If the timer is ready, then we're idling and post the idle timer
            // deadline.

            // NB: The wait vacant future holds no state, it is cheap enough
            // to create a new future at every turn and poll once.
            futures::ready!(buffer.wait_vacant(buffer.capacity().get()).poll_unpin(cx));
            // We have access to the full capacity on the writer side, we
            // can deallocate the buffer.
            let result = match ops.with_send_buffer(|b| b.dealloc()) {
                Some(()) => DriveSendBufferResult::NoBuffer,
                // Send buffer is gone, no reason to keep going.
                None => DriveSendBufferResult::Shutdown,
            };
            return Poll::Ready(result);
        }
    }
}

async fn wait_and_alloc_send_buffer<O: SendTaskOps>(
    handle: &fasync::RWHandle<&zx::Socket>,
    ops: &mut O,
) -> Option<SendBufferWriter> {
    enum ReadableOrClosed {
        Readable,
        Closed,
    }

    let readable_or_closed = futures::future::poll_fn(|ctx| {
        loop {
            // Ignore what fuchsia-async tells us in poll_readable. It may cache
            // state inside it and we want to make sure our object actually has
            // this state either way.
            let _: fasync::ReadableState = futures::ready!(
                handle.poll_readable(ctx).map(|r| r.expect("waiting for readable"))
            );
            // Check if we're actually readable. Pay a syscall here
            // to avoid the buffer allocation.
            match handle.get_ref().wait_one(
                zx::Signals::SOCKET_READABLE | zx::Signals::SOCKET_PEER_CLOSED,
                zx::MonotonicInstant::INFINITE_PAST,
            ) {
                zx::WaitResult::Ok(signals) => {
                    if signals.contains(zx::Signals::SOCKET_READABLE) {
                        return Poll::Ready(ReadableOrClosed::Readable);
                    }
                    if signals.contains(zx::Signals::SOCKET_PEER_CLOSED) {
                        return Poll::Ready(ReadableOrClosed::Closed);
                    }
                    // No signals are set, re-set up our wait.
                }
                zx::WaitResult::TimedOut(_signals) => {}
                e @ zx::WaitResult::Canceled(_) | e @ zx::WaitResult::Err(_) => {
                    panic!("unexpected error reading socket signals {e:?}")
                }
            }
            futures::ready!(handle.need_readable(ctx).map(|r| r.expect("waiting for readable")))
        }
    });

    match readable_or_closed.await {
        ReadableOrClosed::Readable => {
            // Reach into the socket and allocate a new buffer here.
            //
            // If core has dropped its send buffer allocation is skipped and we can
            // shutdown.
            ops.with_send_buffer(|b| b.allocate())
        }
        ReadableOrClosed::Closed => None,
    }
}

fn send_task_shutdown<O: SendTaskOps>(
    socket: Arc<zx::Socket>,
    mut ops: O,
    mut cur_buffer: Option<SendBufferWriter>,
) {
    // We have to look at the zircon socket to figure out how much data we need
    // to read and prealloc any necessary space.
    let zx::SocketInfo { mut rx_buf_available, .. } = socket.info().expect("zx socket get info");
    if rx_buf_available == 0 {
        // Early exit in case of no data available.
        return;
    }
    let (mut prod, new_cons) = match cur_buffer.take() {
        Some(SendBufferWriter { producer }) => (producer, None),
        None => {
            let ring_buffer = async_ringbuf::AsyncHeapRb::new(rx_buf_available);
            let (producer, cons) = ring_buffer.split();
            (producer, Some(cons))
        }
    };

    // Attempt to read as many bytes as we can from the zircon socket into our
    // available producer slices.
    let (a, b) = prod.vacant_slices_mut();
    let a_read = match socket.read_uninit(a).map_err(SocketErrorAction::from) {
        Ok(read) => read.len(),
        // We're already shutting down and we don't need to consider any waits
        // so consider any errors here as a no bytes available read.
        Err(SocketErrorAction::Wait | SocketErrorAction::Shutdown) => 0,
    };
    rx_buf_available -= a_read;
    // Only attempt to read into b if we still have expected available bytes.
    let b_read = if rx_buf_available != 0 {
        match socket.read_uninit(b).map_err(SocketErrorAction::from) {
            Ok(read) => read.len(),
            // Same as above.
            Err(SocketErrorAction::Wait | SocketErrorAction::Shutdown) => 0,
        }
    } else {
        0
    };
    rx_buf_available -= b_read;
    // SAFETY: slices a and b have been initialized by zircon socket
    // reading up to the returned slice length.
    let shutdown_bytes = a_read + b_read;
    unsafe { prod.advance_write_index(shutdown_bytes) };
    // Ensure we can't produce anymore bytes from here on.
    std::mem::drop(prod);

    let extra = if rx_buf_available != 0 {
        let mut extra_uninit = vec![MaybeUninit::uninit(); rx_buf_available];
        match socket.read_uninit(&mut extra_uninit[..]).map_err(SocketErrorAction::from) {
            Ok(extra_init) => {
                let read = extra_init.len();
                let (ptr, len, capacity) = extra_uninit.into_raw_parts();

                // Nothing else should be reading from the socket and we've
                // allocated exactly how much we expect to see so we can assert
                // here that the extra vector's length is exactly what we got.
                assert_eq!(read, len);

                // SAFETY:
                // - MaybeUninit<u8> has the same layout as u8.
                // - Assertion above guarantees we've initialized the vector to
                //   its entire length.
                unsafe { Vec::from_raw_parts(ptr as *mut u8, len, capacity) }
            }
            Err(SocketErrorAction::Wait | SocketErrorAction::Shutdown) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let shutdown_bytes = shutdown_bytes + extra.len();

    // We've accumulated all the data that the application has made available
    // now all that remains is pushing it into the core socket and let it drive
    // the connection to completion as needed.

    // We can ignore whether or not a send buffer was configured, given we could
    // be racing now with core state machine progression.
    let _: Option<()> = ops.with_send_buffer(move |b| {
        replace_with::replace_with(&mut b.inner, |b| {
            match b {
                CoreSendBufferInner::Unallocated { capacity } => {
                    let buffer = new_cons.unwrap_or_else(|| {
                        panic!("shutdown missing new consumer for {shutdown_bytes} bytes")
                    });
                    CoreSendBufferInner::ShuttingDown {
                        buffer,
                        extra,
                        extra_offset: 0,
                        target_capacity: capacity,
                    }
                }
                CoreSendBufferInner::ShuttingDown { .. } => {
                    // This should be the only place we're putting the buffer in
                    // shutdown so we shouldn't find the socket with an already
                    // shutdown send buffer.
                    unreachable!("send buffer already shutting down")
                }
                CoreSendBufferInner::Ready { buffer, pending_capacity } => {
                    let target_capacity = match pending_capacity {
                        None => buffer.capacity().get(),
                        Some(r) => r,
                    };
                    let buffer = match new_cons {
                        Some(new_cons) => {
                            // Update the consumer if we had to allocate a new
                            // one. Assert that the previous one did not have
                            // any data in it, otherwise this is dropping data.
                            assert!(buffer.is_empty());
                            new_cons
                        }
                        None => buffer,
                    };
                    CoreSendBufferInner::ShuttingDown {
                        buffer,
                        extra,
                        extra_offset: 0,
                        target_capacity,
                    }
                }
            }
        })
    });
}

enum SocketErrorAction {
    Wait,
    Shutdown,
}

impl From<zx::Status> for SocketErrorAction {
    fn from(value: zx::Status) -> Self {
        match value {
            zx::Status::SHOULD_WAIT => Self::Wait,
            // If the socket is peer closed it means we're racing with socket
            // closure, so we can exit the task loop.
            zx::Status::PEER_CLOSED => Self::Shutdown,
            // BAD_STATE is reported on disposition change, which is caused by a
            // shutdown call. This means we can stop the task from running, core
            // should've discarded the buffers either way.
            zx::Status::BAD_STATE => Self::Shutdown,
            e => panic!("unexpected zircon socket error: {e:?}"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::cell::RefCell;
    use std::ops::Range;
    use std::rc::Rc;

    use proptest::strategy::{Just, Strategy};
    use proptest::test_runner::{Config, TestCaseError};
    use proptest::{prop_assert, prop_assert_eq, proptest};
    use proptest_support::failed_seeds;
    use test_case::test_case;

    // Declare proptests. All functions are extracted to a `prop_`  variant to
    // keep rustfmt happier since it can't look inside this macro.
    proptest! {
        #![proptest_config(Config {
            failure_persistence: failed_seeds!(),
            ..Config::default()
        })]


        #[test]
        fn rcv_buffer_ready_in_order((warm, ops) in (
            0..(TEST_CAPACITY / 2),
            anybuffer::with_payload_ranges())
        ) {
            prop_rcv_buffer_ready_in_order(warm, ops)?
        }

        #[test]
        fn rcv_buffer_ready_out_of_order((warm, ops) in (
            0..TEST_CAPACITY,
            anybuffer::with_payload_ranges().prop_shuffle()
        )) {
            prop_rcv_buffer_ready_out_of_order(warm, ops)?
        }

        #[test]
        fn rcv_task_byte_shuttling(p in (1..=TEST_CAPACITY)){
            prop_rcv_task_byte_shuttling(p)?
        }

        #[test]
        fn send_buffer_ready((warm, seg, ack) in (
            0..TEST_CAPACITY,
            1..=TEST_CAPACITY,
            proptest::bool::ANY,
        )) {
            prop_send_buffer_ready_shutdown(warm, seg, None, ack)?
        }

        #[test]
        fn send_buffer_shutdown((warm, seg, extra, ack) in (
            0..TEST_CAPACITY,
            1..=TEST_CAPACITY,
            1..=TEST_CAPACITY,
            proptest::bool::ANY,
        )) {
            prop_send_buffer_ready_shutdown(warm, seg, Some(extra), ack)?
        }

        #[test]
        fn send_task_byte_shuttling((warm, seg) in (
            0..TEST_CAPACITY,
            1..=TEST_CAPACITY,
        )) {
            prop_send_task_byte_shuttling(warm, seg)?
        }

        #[test]
        fn send_task_shutdown((before, pending) in (
            proptest::option::of(((0..TEST_CAPACITY), (0..=TEST_CAPACITY))),
            0..=2*TEST_CAPACITY,
        )) {
            prop_send_task_shutdown(before, pending)?
        }
    }

    const TEST_CAPACITY: usize = 16;
    const TEST_PAYLOAD: [u8; TEST_CAPACITY] =
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

    fn prop_rcv_buffer_ready_in_order(
        warm: usize,
        ops: impl IntoIterator<Item = Range<usize>>,
    ) -> Result<(), TestCaseError> {
        // Use half capacity to test partial writes.
        const CAPACITY: usize = TEST_CAPACITY / 2;
        let (mut buffer, mut reader) = new_ready_receive_buffer(CAPACITY);
        warm_receive_buffer(&mut buffer, &mut reader, warm);
        prop_assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: CAPACITY });

        let len = ops.into_iter().try_fold(0, |offset, test_range| {
            let wr = buffer.write_at(0, &&TEST_PAYLOAD[test_range.clone()]);
            prop_assert_eq!(wr, test_range.end.min(CAPACITY) - offset);

            prop_assert_eq!(reader.occupied_len(), offset);
            prop_assert_eq!(buffer.limits(), BufferLimits { len: offset, capacity: CAPACITY });

            buffer.make_readable(wr, false);
            let end = wr + offset;
            prop_assert_eq!(reader.occupied_len(), end);
            prop_assert_eq!(clone_ringbuf(&reader), &TEST_PAYLOAD[0..end]);
            prop_assert_eq!(buffer.limits(), BufferLimits { len: end, capacity: CAPACITY });
            Ok(end)
        })?;
        prop_assert_eq!(async_ringbuf::traits::Consumer::skip(&mut reader, len), len);
        prop_assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: CAPACITY });

        Ok(())
    }

    fn prop_rcv_buffer_ready_out_of_order(
        warm: usize,
        ops: impl IntoIterator<Item = Range<usize>>,
    ) -> Result<(), TestCaseError> {
        let (mut buffer, mut reader) = new_ready_receive_buffer(TEST_CAPACITY);
        warm_receive_buffer(&mut buffer, &mut reader, warm);
        prop_assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: TEST_CAPACITY });

        let len = ops.into_iter().try_fold(0, |written, test_range| {
            let wr = buffer.write_at(test_range.start, &&TEST_PAYLOAD[test_range.clone()]);
            prop_assert_eq!(wr, test_range.end - test_range.start);
            Ok(written + wr)
        })?;

        // We have received the ranges possibly out of order, mark readable only
        // once and check that we end up in the right state.
        prop_assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: TEST_CAPACITY });
        buffer.make_readable(len, false);
        prop_assert_eq!(buffer.limits(), BufferLimits { len, capacity: TEST_CAPACITY });

        prop_assert_eq!(clone_ringbuf(&reader), &TEST_PAYLOAD[0..len]);
        prop_assert_eq!(async_ringbuf::traits::Consumer::skip(&mut reader, len), len);
        prop_assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: TEST_CAPACITY });

        Ok(())
    }

    fn prop_rcv_task_byte_shuttling(partial_read: usize) -> Result<(), TestCaseError> {
        // Set up a buffer and a zircon socket that is full enough to cause
        // partial writes by the read task. The happy byte-shuttling case is
        // covered by integration tests.
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (mut buffer, chan) = new_receive_buffer_and_channel(TEST_CAPACITY);
        let (socket, task_socket) = zx::Socket::create_stream();

        let zx::SocketInfo { rx_buf_max, .. } = socket.info().unwrap();
        let socket_preamble = rx_buf_max - partial_read;
        let total_msg = std::iter::repeat(0xAA)
            .take(socket_preamble)
            .chain(TEST_PAYLOAD)
            .chain(TEST_PAYLOAD)
            .collect::<Vec<_>>();
        let mut recvbuf = vec![0u8; total_msg.len()];
        prop_assert_eq!(task_socket.write(&total_msg[..socket_preamble]), Ok(socket_preamble));

        let rcv_task = receive_task(Arc::new(task_socket), (), chan);
        let mut rcv_task = pin!(rcv_task);

        prop_assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);

        let mut send_slice = &total_msg[socket_preamble..];
        let mut recv_slice = &mut recvbuf[..];

        while !send_slice.is_empty() {
            let wr = buffer.write_at(0, &send_slice);
            prop_assert!(wr != 0, "test can't make progress");

            buffer.make_readable(wr, false);
            prop_assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
            prop_assert_eq!(socket.read(&mut recv_slice[..partial_read]), Ok(partial_read));
            send_slice = &send_slice[wr..];
            recv_slice = &mut recv_slice[partial_read..];
        }
        // We managed to send TEST_PAYLOAD twice over the socket, read
        // everything else now and check that the data is correct.
        let in_buffer = buffer.limits().len;
        let in_socket = recv_slice.len() - in_buffer;
        prop_assert_eq!(socket.read(recv_slice), Ok(in_socket));
        prop_assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
        if in_buffer != 0 {
            prop_assert_eq!(socket.read(&mut recv_slice[in_socket..]), Ok(in_buffer));
        }

        // Most of the buffer is basically trash, just compare the tail of the
        // buffers to avoid comparing the entire buffer which is ~256K long to
        // match the zircon socket.
        let buffer_tail = total_msg.len() - TEST_PAYLOAD.len() * 2 - partial_read;
        prop_assert_eq!(&total_msg[buffer_tail..], &recvbuf[buffer_tail..]);

        // Dropping buffer causes receive task to end.
        drop(buffer);
        prop_assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Ready(()));

        Ok(())
    }

    fn prop_send_buffer_ready_shutdown(
        warm: usize,
        segment: usize,
        extra: Option<usize>,
        ack: bool,
    ) -> Result<(), TestCaseError> {
        let ring_buffer = async_ringbuf::AsyncHeapRb::new(TEST_CAPACITY);
        let (mut producer, mut cons) = ring_buffer.split();
        // Prime the buffer with `warm` bytes` so we exercise the ring buffer
        // split slices.
        prop_assert_eq!(producer.push_iter(std::iter::repeat(0xAA).take(warm)), warm);
        prop_assert_eq!(async_ringbuf::traits::Consumer::skip(&mut cons, warm), warm);

        prop_assert_eq!(producer.push_slice(&TEST_PAYLOAD), TEST_PAYLOAD.len());

        let (inner, producer) = match extra {
            Some(extra) => {
                drop(producer);
                (
                    CoreSendBufferInner::ShuttingDown {
                        buffer: cons,
                        extra: (&TEST_PAYLOAD[..extra]).to_vec(),
                        extra_offset: 0,
                        target_capacity: TEST_CAPACITY,
                    },
                    None,
                )
            }
            None => (
                CoreSendBufferInner::Ready {
                    buffer: cons,
                    // NB: arbitrary, just easier to construct than the idle
                    // variant.
                    pending_capacity: Some(TEST_CAPACITY),
                },
                Some(producer),
            ),
        };
        let (signal_sender, _receiver) = TxTaskSender::new();
        let mut buffer = CoreSendBuffer { inner, signal_sender };

        let expect = TEST_PAYLOAD
            .into_iter()
            .chain((&TEST_PAYLOAD[..extra.unwrap_or(0)]).iter().copied())
            .collect::<Vec<_>>();
        let mut read = vec![0xBB; expect.len()];

        prop_assert_eq!(buffer.limits().len, expect.len());

        let mut read_offset = 0;
        let mut buffer_offset = 0;
        while read_offset != read.len() {
            let read_end = (read_offset + segment).min(read.len());
            buffer.peek_with(buffer_offset, |pl| {
                pl.partial_copy(0, &mut read[read_offset..read_end]);
            });
            prop_assert_eq!(&read[read_offset..read_end], &expect[read_offset..read_end]);

            let did_read = read_end - read_offset;
            read_offset = read_end;

            if ack {
                buffer.mark_read(did_read);
                prop_assert_eq!(buffer.limits().len, read.len() - read_end);
                // NB: Producer must be dropped for the shutting down buffer
                // case, since we don't allow any more bytes to be produced
                // there.
                if let Some(producer) = producer.as_ref() {
                    prop_assert_eq!(producer.vacant_len(), read_offset);
                }
            } else {
                buffer_offset += did_read;
            }
        }
        prop_assert_eq!(read, expect);
        Ok(())
    }

    fn prop_send_task_byte_shuttling(warm: usize, seg: usize) -> Result<(), TestCaseError> {
        let mut executor = fasync::TestExecutor::new_with_fake_time();

        let (tx_task_sender, tx_task_receiver) = TxTaskSender::new();
        let buffer = CoreSendBuffer::new(TEST_CAPACITY, tx_task_sender);
        let buffer = Rc::new(RefCell::new(buffer));
        let (socket, task_socket) = zx::Socket::create_stream();
        let (_shutdown_sender, shutdown_receiver) = oneshot::channel();
        let send_task = send_task(
            Arc::new(task_socket),
            Rc::clone(&buffer),
            shutdown_receiver,
            tx_task_receiver,
        );
        let mut send_task = pin!(send_task);

        if warm != 0 {
            let warm_buff = std::iter::repeat(0xAA).take(warm).collect::<Vec<u8>>();
            prop_assert_eq!(socket.write(&warm_buff), Ok(warm));
            prop_assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
            {
                let mut buffer = buffer.borrow_mut();
                let inner = match &buffer.inner {
                    CoreSendBufferInner::Ready { buffer, pending_capacity: _ } => buffer,
                    s => return Err(TestCaseError::fail(format!("bad buffer state {s:?}"))),
                };
                prop_assert_eq!(inner.write_index(), warm);
                buffer.mark_read(warm);
            }
        }

        prop_assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        let expect = [&TEST_PAYLOAD[..], &TEST_PAYLOAD[..]].concat();
        prop_assert_eq!(socket.write(&expect), Ok(expect.len()));

        // Loop until we've accumulated `expect` in our receive
        // buffer.
        let mut received = vec![0u8; expect.len()];
        let mut received_offset = 0;
        while received_offset != received.len() {
            prop_assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
            let mut buffer = buffer.borrow_mut();
            match &buffer.inner {
                CoreSendBufferInner::Ready { .. } => {}
                s => return Err(TestCaseError::fail(format!("bad buffer state {s:?}"))),
            }
            let expect_len = TEST_CAPACITY.min(received.len() - received_offset);
            prop_assert_eq!(buffer.limits().len, expect_len);

            let received_end = received.len().min(received_offset + seg);
            buffer
                .peek_with(0, |f| f.partial_copy(0, &mut received[received_offset..received_end]));
            let mark_read = received_end - received_offset;
            buffer.mark_read(mark_read);
            prop_assert_eq!(buffer.limits().len, expect_len - mark_read);
            received_offset = received_end;
        }

        prop_assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        prop_assert_eq!(buffer.borrow().limits().len, 0);
        prop_assert_eq!(received, expect);

        // Close the signal, tx task should finish.
        buffer.borrow_mut().signal_sender.0.disconnect();
        prop_assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Ready(()));
        Ok(())
    }

    fn prop_send_task_shutdown(
        before: Option<(usize, usize)>,
        pending: usize,
    ) -> Result<(), TestCaseError> {
        let mut expect = vec![];

        let (signal_sender, _tx_task_receiver) = TxTaskSender::new();
        let mut buffer = CoreSendBuffer::new(TEST_CAPACITY, signal_sender);
        let (buffer, writer) = match before {
            Some((warm, in_buffer)) => {
                let mut writer = buffer.allocate();
                prop_assert_eq!(
                    writer.producer.push_iter(std::iter::repeat(0xAA).take(warm)),
                    warm
                );
                buffer.mark_read(warm);

                let in_buffer = &TEST_PAYLOAD[..in_buffer];
                expect.extend_from_slice(in_buffer);
                prop_assert_eq!(writer.producer.push_slice(in_buffer), in_buffer.len());
                (buffer, Some(writer))
            }
            None => (buffer, None),
        };

        // Use a different slice for what goes into extra so alignment to
        // capacity doesn't hide problems.
        let pending =
            TEST_PAYLOAD.into_iter().cycle().map(|x| x | 0x80).take(pending).collect::<Vec<_>>();
        expect.extend_from_slice(&pending[..]);

        let buffer = Rc::new(RefCell::new(buffer));
        let (socket, task_socket) = zx::Socket::create_stream();
        prop_assert_eq!(socket.write(&pending[..]), Ok(pending.len()));

        super::send_task_shutdown(Arc::new(task_socket), Rc::clone(&buffer), writer);
        let mut buffer = Rc::try_unwrap(buffer).unwrap().into_inner();
        if !pending.is_empty() {
            let (buffer, extra) = match &buffer.inner {
                CoreSendBufferInner::ShuttingDown {
                    buffer,
                    extra,
                    extra_offset: _,
                    target_capacity: _,
                } => (buffer, extra),
                b => return Err(TestCaseError::fail(format!("bad buffer state {b:?}"))),
            };
            let expect_buffer_len = expect.len().min(buffer.capacity().get());
            prop_assert_eq!(buffer.occupied_len(), expect_buffer_len);
            prop_assert_eq!(extra.len(), expect.len() - expect_buffer_len);
        }

        let mut got = vec![0xAAu8; expect.len()];
        buffer.peek_with(0, |p| p.partial_copy(0, &mut got[..]));
        prop_assert_eq!(got, expect);
        Ok(())
    }

    #[test]
    fn rcv_buffer_update_capacity() {
        let (mut buffer, mut chan) = new_receive_buffer_and_channel(TEST_CAPACITY / 2);
        assert_eq!(buffer.target_capacity(), TEST_CAPACITY / 2);
        // Requesting capacity in unallocated state takes effect immediately.
        assert_matches!(&buffer.inner, CoreReceiveBufferInner::Unallocated { .. });
        buffer.request_capacity(TEST_CAPACITY);
        assert_eq!(buffer.target_capacity(), TEST_CAPACITY);
        assert_eq!(buffer.limits(), BufferLimits { len: 0, capacity: TEST_CAPACITY });

        force_receive_buffer_ready(&mut buffer);
        let reader = chan.next().now_or_never().flatten().unwrap();
        assert_eq!(buffer.target_capacity(), TEST_CAPACITY);

        const CAP1: usize = 2;
        const CAP2: usize = 4;
        const CAP3: usize = 8;

        // Attempt to update capacity on an empty buffer, should succeed
        // immediately.
        buffer.request_capacity(CAP1);
        assert_eq!(buffer.target_capacity(), CAP1);
        assert!(reader.is_closed());
        let reader = chan.next().now_or_never().flatten().unwrap();
        assert_eq!(reader.capacity().get(), CAP1);

        const PAYLOAD: &'static [u8] = &[1, 2];
        assert_eq!(buffer.write_at(0, &PAYLOAD), PAYLOAD.len());
        buffer.request_capacity(CAP2);
        assert_eq!(buffer.target_capacity(), CAP2);
        assert!(!reader.is_closed());

        assert_eq!(buffer.write_at(0, &()), 0);
        buffer.request_capacity(CAP3);
        assert_eq!(buffer.target_capacity(), CAP3);
        assert!(!reader.is_closed());

        buffer.make_readable(1, /* has_outstanding */ true);
        assert!(!reader.is_closed());
        buffer.make_readable(1, /* has_outstanding */ false);

        assert!(reader.is_closed());
        assert_eq!(clone_ringbuf(&reader), PAYLOAD);
        let reader = chan.next().now_or_never().flatten().unwrap();
        assert_eq!(reader.capacity().get(), CAP3);
        assert!(!reader.is_closed());

        // After dropping the buffer the channel should be closed no more
        // surprise readers.
        drop(buffer);
        assert!(reader.is_closed());
        assert!(chan.next().now_or_never().unwrap().is_none());
    }

    #[test]
    fn receive_task_discard_on_idle() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (buffer, chan) = new_receive_buffer_and_channel(TEST_CAPACITY);
        let (socket, task_socket) = zx::Socket::create_stream();
        let buffer = Rc::new(RefCell::new(buffer));
        let rcv_task = receive_task(Arc::new(task_socket), Rc::clone(&buffer), chan);
        let mut rcv_task = pin!(rcv_task);
        assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
        assert_matches!(&buffer.borrow().inner, CoreReceiveBufferInner::Unallocated { .. });

        // Do some rounds of receiving data and hitting idle to show
        // deallocation and reallocation.
        const ROUNDS: usize = 3;
        for round in 0..ROUNDS {
            {
                let mut buffer = buffer.borrow_mut();
                let subslice = &TEST_PAYLOAD[round * 2..round * 2 + 2];
                assert_eq!(buffer.write_at(0, &subslice), subslice.len());
                let has_outstanding = true;
                // Only have of the buffer is made readable.
                buffer.make_readable(1, has_outstanding);
            }
            assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreReceiveBufferInner::Ready { .. });

            // Task should be waiting for an idle state.
            let next_wake =
                fasync::TestExecutor::next_timer().expect("should be waiting on a timer");
            assert_eq!(next_wake, executor.now() + TCP_IDLE_BUFFER_TIMEOUT);
            executor.set_fake_time(next_wake);

            // We have hit an idle timeout but we still have outstanding data,
            // we can't deallocate the buffer.
            assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreReceiveBufferInner::Ready { .. });
            // No new idle timer is installed because of this failed attempt.
            assert_eq!(fasync::TestExecutor::next_timer(), None);

            // Make more data available, but without any out of order bytes.
            let has_outstanding = false;
            buffer.borrow_mut().make_readable(1, has_outstanding);

            assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreReceiveBufferInner::Ready { .. });
            let next_wake =
                fasync::TestExecutor::next_timer().expect("should be waiting on a timer");
            assert_eq!(next_wake, executor.now() + TCP_IDLE_BUFFER_TIMEOUT);
            executor.set_fake_time(next_wake);

            assert_eq!(executor.run_until_stalled(&mut rcv_task), Poll::Pending);
            let capacity = assert_matches!(
                &buffer.borrow().inner,
                CoreReceiveBufferInner::Unallocated { capacity } => *capacity
            );
            assert_eq!(capacity, TEST_CAPACITY);
            assert_eq!(fasync::TestExecutor::next_timer(), None);
        }

        const EXPECT_BYTES: usize = ROUNDS * 2;
        let mut app_buffer = [0u8; EXPECT_BYTES + 1];
        assert_eq!(socket.read(&mut app_buffer), Ok(EXPECT_BYTES));
        assert_eq!(&app_buffer[..EXPECT_BYTES], &TEST_PAYLOAD[..EXPECT_BYTES]);
    }

    #[test]
    fn send_task_change_capacity() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();

        const CAP1: usize = TEST_CAPACITY;
        const CAP2: usize = CAP1 + 1;
        const CAP3: usize = CAP2 + 1;
        const CAP4: usize = CAP3 + 1;

        let (sender, tx_task_receiver) = TxTaskSender::new();
        let buffer = CoreSendBuffer::new(CAP1, sender);
        let buffer = Rc::new(RefCell::new(buffer));
        let (socket, task_socket) = zx::Socket::create_stream();
        let (_shutdown_sender, shutdown_receiver) = oneshot::channel();
        let send_task = send_task(
            Arc::new(task_socket),
            Rc::clone(&buffer),
            shutdown_receiver,
            tx_task_receiver,
        );
        let mut send_task = pin!(send_task);

        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);

        // Buffer is still not allocated.
        assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Unallocated { .. });
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP1 });
        assert_eq!(buffer.borrow().target_capacity(), CAP1);
        buffer.borrow_mut().request_capacity(CAP2);
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP2 });
        assert_eq!(buffer.borrow().target_capacity(), CAP2);

        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Unallocated { .. });
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP2 });
        assert_eq!(buffer.borrow().target_capacity(), CAP2);

        // Send something to allocate the buffer.
        assert_eq!(socket.write(&TEST_PAYLOAD), Ok(TEST_PAYLOAD.len()));
        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        buffer.borrow_mut().mark_read(TEST_PAYLOAD.len());
        assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Ready { .. });
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP2 });
        assert_eq!(buffer.borrow().target_capacity(), CAP2);

        buffer.borrow_mut().request_capacity(CAP3);
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP2 });
        assert_eq!(buffer.borrow().target_capacity(), CAP3);
        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP3 });
        assert_eq!(buffer.borrow().target_capacity(), CAP3);

        assert_eq!(socket.write(&TEST_PAYLOAD), Ok(TEST_PAYLOAD.len()));
        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        assert_eq!(
            buffer.borrow().limits(),
            BufferLimits { len: TEST_PAYLOAD.len(), capacity: CAP3 }
        );

        buffer.borrow_mut().request_capacity(CAP4);
        assert_eq!(
            buffer.borrow().limits(),
            BufferLimits { len: TEST_PAYLOAD.len(), capacity: CAP3 }
        );
        assert_eq!(buffer.borrow().target_capacity(), CAP4);

        // There's still pending data in the buffer, capacity must not have
        // changed yet.
        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        assert_eq!(
            buffer.borrow().limits(),
            BufferLimits { len: TEST_PAYLOAD.len(), capacity: CAP3 }
        );

        buffer.borrow_mut().mark_read(TEST_PAYLOAD.len());
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP3 });

        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Pending);
        assert_eq!(buffer.borrow().limits(), BufferLimits { len: 0, capacity: CAP4 });
        assert_eq!(buffer.borrow().target_capacity(), CAP4);

        // Close the signal, tx task should finish.
        buffer.borrow_mut().signal_sender.0.disconnect();
        assert_eq!(executor.run_until_stalled(&mut send_task), Poll::Ready(()));
    }

    #[test_case(true; "ack_before_idle")]
    #[test_case(false; "ack_after_idle")]
    fn send_task_discard_on_idle(ack_before_idle: bool) {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (task_sender, task_receiver) = TxTaskSender::new();
        let buffer = CoreSendBuffer::new(TEST_CAPACITY, task_sender);
        let (socket, task_socket) = zx::Socket::create_stream();
        let buffer = Rc::new(RefCell::new(buffer));
        let (_shutdown_sender, shutdown_receiver) = oneshot::channel();
        let snd_task =
            send_task(Arc::new(task_socket), Rc::clone(&buffer), shutdown_receiver, task_receiver);
        let mut snd_task = pin!(snd_task);
        assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
        assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Unallocated { .. });

        // Do some rounds of receiving data and hitting idle to show
        // deallocation and reallocation.
        const ROUNDS: usize = 3;
        for round in 0..ROUNDS {
            let payload = &TEST_PAYLOAD[round * 2..round * 2 + 2];
            assert_eq!(socket.write(&payload[..1]), Ok(1));
            assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Ready { .. });

            // Task should be waiting for an idle state.
            let next_wake =
                fasync::TestExecutor::next_timer().expect("should be waiting on a timer");
            assert_eq!(next_wake, executor.now() + TCP_IDLE_BUFFER_TIMEOUT);
            executor.set_fake_time(next_wake);

            // We have hit an idle timeout but we still have unacked data,
            // we can't deallocate the buffer.
            assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Ready { .. });
            // No new idle timer is installed because of this failed attempt.
            assert_eq!(fasync::TestExecutor::next_timer(), None);

            // We can write more data and that'll reset the idle timer.
            assert_eq!(socket.write(&payload[1..]), Ok(1));
            assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
            assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Ready { .. });
            let next_wake =
                fasync::TestExecutor::next_timer().expect("should be waiting on a timer");
            assert_eq!(next_wake, executor.now() + TCP_IDLE_BUFFER_TIMEOUT);
            executor.set_fake_time(next_wake);

            if !ack_before_idle {
                // Hit the idle state again before all the bytes are acked.
                assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
                assert_eq!(fasync::TestExecutor::next_timer(), None);
                assert_matches!(&buffer.borrow().inner, CoreSendBufferInner::Ready { .. });
            }

            // Check the buffer contents and ack outstanding bytes.
            {
                let mut buffer = buffer.borrow_mut();
                let buffer_payload = buffer.peek_with(0, |payload| payload.to_vec());
                assert_eq!(&buffer_payload[..], payload);
                buffer.mark_read(2);
            }

            assert_eq!(executor.run_until_stalled(&mut snd_task), Poll::Pending);
            let capacity = assert_matches!(
                &buffer.borrow().inner,
                CoreSendBufferInner::Unallocated { capacity } => *capacity
            );
            assert_eq!(capacity, TEST_CAPACITY);
            assert_eq!(fasync::TestExecutor::next_timer(), None);
        }
    }

    impl ReceiveTaskOps for () {
        fn shutdown_recv(&mut self) -> Result<bool, NoConnection> {
            Ok(true)
        }

        fn on_receive_buffer_read(&mut self) {}

        fn with_receive_buffer<F, R>(&mut self, _: F) -> Option<R>
        where
            F: FnOnce(&mut CoreReceiveBuffer) -> R,
        {
            None
        }

        fn has_send_buffer(&mut self) -> bool {
            false
        }
    }

    impl ReceiveTaskOps for Rc<RefCell<CoreReceiveBuffer>> {
        fn shutdown_recv(&mut self) -> Result<bool, NoConnection> {
            unimplemented!()
        }

        fn on_receive_buffer_read(&mut self) {}

        fn with_receive_buffer<F, R>(&mut self, f: F) -> Option<R>
        where
            F: FnOnce(&mut CoreReceiveBuffer) -> R,
        {
            Some(f(&mut self.borrow_mut()))
        }

        fn has_send_buffer(&mut self) -> bool {
            false
        }
    }

    impl SendTaskOps for Rc<RefCell<CoreSendBuffer>> {
        fn do_send(&mut self) {}
        fn with_send_buffer<R, F: FnOnce(&mut CoreSendBuffer) -> R>(&mut self, f: F) -> Option<R> {
            Some(f(&mut self.borrow_mut()))
        }
    }

    fn clone_ringbuf<B: async_ringbuf::traits::Consumer<Item = u8>>(b: &B) -> Vec<u8> {
        let (a, b) = b.as_slices();
        [a, b].concat()
    }

    fn new_receive_buffer_and_channel(
        capacity: usize,
    ) -> (CoreReceiveBuffer, mpsc::UnboundedReceiver<ReceiveBufferReader>) {
        let (snd, rcv) = mpsc::unbounded();
        let b = CoreReceiveBuffer::new(snd, capacity, /* notifier */ None);
        (b, rcv)
    }

    fn force_receive_buffer_ready(buffer: &mut CoreReceiveBuffer) {
        // Force entering the allocated state by writing some data to it.
        assert_eq!(buffer.write_at(0, &()), 0);
        let has_outstanding = false;
        buffer.make_readable(0, has_outstanding);
        assert_matches!(&mut buffer.inner, CoreReceiveBufferInner::Ready { .. });
    }

    fn new_ready_receive_buffer(capacity: usize) -> (CoreReceiveBuffer, ReceiveBufferReader) {
        let (mut b, mut rcv) = new_receive_buffer_and_channel(capacity);
        force_receive_buffer_ready(&mut b);
        let rd = rcv.next().now_or_never().flatten().unwrap();
        (b, rd)
    }

    fn warm_receive_buffer(
        buffer: &mut CoreReceiveBuffer,
        reader: &mut ReceiveBufferReader,
        warm: usize,
    ) {
        let CoreReceiveBuffer { inner, .. } = buffer;
        let buffer = assert_matches!(inner, CoreReceiveBufferInner::Ready { buffer, .. } => buffer);
        assert_eq!(buffer.push_iter(std::iter::repeat(0xAA).take(warm)), warm);
        assert_eq!(async_ringbuf::traits::Consumer::skip(reader, warm), warm);
    }

    mod anybuffer {
        use super::*;

        /// Produces 3 random ranges covering contiguous subslices of
        /// `TEST_PAYLOAD` (in order).
        pub(super) fn with_payload_ranges() -> impl Strategy<Value = [Range<usize>; 3]> {
            (0..TEST_CAPACITY)
                .prop_flat_map(|start| {
                    (Just(0..start), (start..TEST_CAPACITY).prop_map(move |end| start..end))
                })
                .prop_flat_map(|(first, second)| {
                    let start = second.end;
                    let third = (start..TEST_CAPACITY).prop_map(move |end| start..end);
                    (Just(first), Just(second), third)
                })
                .prop_map(|(a, b, c)| [a, b, c])
        }
    }
}
