// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use core::mem::{ManuallyDrop, MaybeUninit, replace, take};
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use fidl_constants::EPITAPH_ORDINAL;

use fidl_next_codec::{EncodeError, EncoderExt as _};
use fuchsia_loom::cell::UnsafeCell;
use fuchsia_loom::future::AtomicWaker;
use fuchsia_loom::hint::unreachable_unchecked;
use fuchsia_loom::sync::Mutex;
use fuchsia_loom::sync::atomic::{AtomicUsize, Ordering};
use pin_project::pin_project;

use crate::wire::{Epitaph, MessageHeader};
use crate::{Flexibility, NonBlockingTransport, ProtocolError, Transport};

// Indicates that the connection has been requested to stop. Connections are
// always stopped as they are terminated.
const STOPPING_BIT: usize = 1 << 0;
// Indicates that the connection has been provided a termination reason.
const TERMINATED_BIT: usize = 1 << 1;
const BITS_COUNT: usize = 2;

// Each refcount represents a thread which is attempting to access the shared
// part of the transport.
const REFCOUNT: usize = 1 << BITS_COUNT;

#[derive(Clone, Copy)]
struct State(usize);

impl State {
    fn is_stopping(self) -> bool {
        self.0 & STOPPING_BIT != 0
    }

    fn is_terminated(self) -> bool {
        self.0 & TERMINATED_BIT != 0
    }

    fn refcount(self) -> usize {
        self.0 >> BITS_COUNT
    }
}

/// A wrapper around a transport which connectivity semantics.
///
/// The [`Transport`] trait only provides the bare minimum API surface required
/// to send and receive data. On top of that, FIDL requires that clients and
/// servers respect additional messaging semantics. Those semantics are provided
/// by [`Connection`]:
///
/// - `Transport`s are difficult to close because they may be accessed from
///   several threads simultaneously. `Connection`s provide a mechanism for
///   gracefully closing transports by causing all sends to pend until the
///   connection is terminated, and all receives to fail instead of pend.
/// - FIDL connections may send and receive an epitaph as the final message
///   before the underlying transport is closed. This epitaph should be provided
///   to all sends when they fail, which requires additional coordination.
pub struct Connection<T: Transport> {
    // The lowest `BITS_COUNT` of this field contain flags indicating the
    // current state of the transport. The remainder of the upper bits contain
    // the number of threads attempting to access the `shared` field.
    state: AtomicUsize,
    // A thread will drop `shared` if:
    //
    // - the connection is dropped before being terminated, or
    // - it set `TERMINATED_BIT` while the refcount was 0, or
    // - it decremented the refcount to 0 while `TERMINATED_BIT` was set.
    //
    // These cases are handled by `drop`, `terminate`, and `with_shared`
    // respectively.
    shared: UnsafeCell<ManuallyDrop<T::Shared>>,
    stop_waker: AtomicWaker,
    // TODO: switch this to intrusive linked list in send futures
    termination_wakers: Mutex<Vec<Waker>>,
    // Initialized if `TERMINATED_BIT` is set.
    termination_reason: UnsafeCell<MaybeUninit<ProtocolError<T::Error>>>,
}

unsafe impl<T: Transport> Send for Connection<T> {}
unsafe impl<T: Transport> Sync for Connection<T> {}

impl<T: Transport> Drop for Connection<T> {
    fn drop(&mut self) {
        self.state.with_mut(|state| {
            let state = State(*state);

            if !state.is_terminated() {
                self.shared.with_mut(|shared| {
                    // SAFETY: The connection was not terminated before being
                    // dropped, so `shared` has not yet been dropped.
                    unsafe {
                        ManuallyDrop::drop(&mut *shared);
                    }
                });
            } else {
                self.termination_reason.with_mut(|termination_reason| {
                    // SAFETY: The connection was terminated before being
                    // dropped, so `termination_reason` is initialized.
                    unsafe {
                        MaybeUninit::assume_init_drop(&mut *termination_reason);
                    }
                });
            }
        });
    }
}

impl<T: Transport> Connection<T> {
    /// Creates a new connection from the shared part of a transport.
    pub fn new(shared: T::Shared) -> Self {
        Self {
            state: AtomicUsize::new(0),
            shared: UnsafeCell::new(ManuallyDrop::new(shared)),
            stop_waker: AtomicWaker::new(),
            termination_wakers: Mutex::new(Vec::new()),
            termination_reason: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// # Safety
    ///
    /// This thread must have loaded `state` with at least `Ordering::Acquire`
    /// and observed that `TERMINATED_BIT` was set.
    unsafe fn get_termination_reason_unchecked(&self) -> ProtocolError<T::Error> {
        self.termination_reason.with(|termination_reason| {
            // SAFETY: The caller guaranteed that `state` was loaded with at
            // least `Ordering::Acquire` ordering and observed that
            // `TERMINATED_BIT` was set.
            unsafe { MaybeUninit::assume_init_ref(&*termination_reason).clone() }
        })
    }

    /// Returns the termination reason for the connection, if any.
    pub fn get_termination_reason(&self) -> Option<ProtocolError<T::Error>> {
        if State(self.state.load(Ordering::Acquire)).is_terminated() {
            // SAFETY: We loaded the state with `Ordering::Acquire` and observed
            // that `TERMINATED_BIT` was set.
            unsafe { Some(self.get_termination_reason_unchecked()) }
        } else {
            None
        }
    }

    /// # Safety
    ///
    /// `shared` must not have been dropped. See the documentation on `shared`
    /// for acceptable criteria.
    unsafe fn get_shared_unchecked(&self) -> &T::Shared {
        self.shared.with(|shared| {
            // SAFETY: The caller guaranteed that `shared` has not been dropped.
            unsafe { &*shared }
        })
    }

    fn with_shared<U>(
        &self,
        success: impl FnOnce(&T::Shared) -> U,
        failure: impl FnOnce(Option<ProtocolError<T::Error>>) -> U,
    ) -> U {
        let pre_increment = State(self.state.fetch_add(REFCOUNT, Ordering::Acquire));

        // After the refcount drops to zero (and `shared` is dropped), threads
        // may still increment and decrement the refcount to attempt to read it.
        // To avoid dropping `shared` more than once, we prevent the refcount
        // from being decremented to 0 more than once after `TERMINATED_BIT` is
        // set.
        //
        // We do this by having each thread check whether its increment changed
        // the refcount from 0 to 1 while `TERMINATED_BIT` was set. If it did,
        // the thread will not decrement that refcount, leaving it "dangling"
        // instead. This ensures that the refcount never falls below 1 again.
        if pre_increment.is_terminated() && pre_increment.refcount() == 0 {
            // SAFETY: We loaded `state` with `Ordering::Acquire` and observed
            // that `TERMINATED_BIT` was set.
            let termination_reason = unsafe { self.get_termination_reason_unchecked() };
            return failure(Some(termination_reason));
        }

        let mut success_result = None;
        if !pre_increment.is_stopping() {
            // SAFETY: Termination always sets `STOPPING_BIT`. We incremented
            // the refcount while `STOPPING_BIT` was not set, so `shared` won't
            // be dropped until we decrement our refcount.
            let shared = unsafe { self.get_shared_unchecked() };
            success_result = Some(success(shared));
        }

        let pre_decrement = State(self.state.fetch_sub(REFCOUNT, Ordering::AcqRel));

        if !pre_decrement.is_stopping() {
            success_result.unwrap()
        } else if !pre_decrement.is_terminated() {
            failure(None)
        } else {
            // The connection is terminated. If we decremented the refcount to
            // 0, then we need to drop `shared`.
            if pre_decrement.refcount() == 1 {
                self.shared.with_mut(|shared| {
                    // SAFETY: We decremented the refcount to 0 while
                    // `TERMINATED_BIT` was set.
                    unsafe {
                        ManuallyDrop::drop(&mut *shared);
                    }
                });
            }

            // SAFETY: We loaded `state` with `Ordering::Acquire` and observed
            // that `TERMINATED_BIT` was set.
            let termination_reason = unsafe { self.get_termination_reason_unchecked() };
            failure(Some(termination_reason))
        }
    }

    /// Sends a message to the underlying transport.
    ///
    /// Returns a `SendFutureState` which can be polled to completion.
    pub fn send_message_raw(
        &self,
        f: impl FnOnce(&mut T::SendBuffer) -> Result<(), EncodeError>,
    ) -> Result<SendFutureState<T>, EncodeError> {
        self.with_shared(
            |shared| {
                let mut buffer = T::acquire(shared);
                f(&mut buffer)?;
                Ok(SendFutureState::Running { future_state: T::begin_send(shared, buffer) })
            },
            |error| {
                Ok(error
                    // Some(Error) => Terminated
                    .map(|error| SendFutureState::Terminated { error })
                    // None => Stopping
                    .unwrap_or(SendFutureState::Stopping))
            },
        )
    }

    /// Sends an epitaph to the underlying transport.
    ///
    /// This send ignores the current state of the connection, and does not
    /// report back any errors encountered while sending.
    ///
    /// # Safety
    ///
    /// The connection must not be terminated, and the returned future must be
    /// completed or canceled before the connection is terminated.
    pub unsafe fn send_epitaph(&self, error: i32) -> SendEpitaphFuture<'_, T> {
        // SAFETY: The caller has guaranteed that the connection is not
        // terminated, and will not be terminated until the returned future is
        // completed or canceled. As long as the connection is not terminated,
        // `shared` will not be dropped.
        let shared = unsafe { self.get_shared_unchecked() };

        let mut buffer = T::acquire(shared);
        buffer.encode_next(MessageHeader::new(0, EPITAPH_ORDINAL, Flexibility::Strict)).unwrap();
        buffer.encode_next(Epitaph::new(error)).unwrap();
        let future_state = T::begin_send(shared, buffer);

        SendEpitaphFuture { shared, future_state }
    }

    /// Returns a new [`RecvFuture`] which receives the next message.
    ///
    /// # Safety
    ///
    /// The connection must not be terminated, and the returned future must be
    /// completed or canceled before the connection is terminated.
    pub unsafe fn recv<'a>(&'a self, exclusive: &'a mut T::Exclusive) -> RecvFuture<'a, T> {
        // SAFETY: The caller has guaranteed that the connection is not
        // terminated, and will not be terminated until the returned future is
        // completed or canceled. As long as the connection is not terminated,
        // `shared` will not be dropped.
        let shared = unsafe { self.get_shared_unchecked() };
        let future_state = T::begin_recv(shared, exclusive);

        RecvFuture { connection: self, exclusive, future_state }
    }

    /// Stops the connection to wait for termination.
    ///
    /// This modifies the behavior of this connection's futures:
    ///
    /// - Polled [`SendFutureState`]s will return `Poll::Pending` without
    ///   calling [`poll_send`].
    /// - Polled [`RecvFuture`]s will call [`poll_recv`], but will return
    ///   `Poll::Ready` with an error when they would normally return
    ///   `Poll::Pending`.
    ///
    /// [`poll_send`]: Transport::poll_send
    /// [`poll_recv`]: Transport::poll_recv
    pub fn stop(&self) {
        let prev_state = State(self.state.fetch_or(STOPPING_BIT, Ordering::Relaxed));
        if !prev_state.is_stopping() {
            self.stop_waker.wake();
        }
    }

    /// Terminates the connection.
    ///
    /// This causes this connection's futures to return `Poll::Ready` with an
    /// error of the given termination reason.
    ///
    /// # Safety
    ///
    /// `terminate` may only be called once per connection.
    pub unsafe fn terminate(&self, reason: ProtocolError<T::Error>) {
        self.termination_reason.with_mut(|termination_reason| {
            // SAFETY: The caller guaranteed that this is the only time
            // `terminate` is called on this connection.
            unsafe {
                termination_reason.write(MaybeUninit::new(reason));
            }
        });
        let pre_terminate =
            State(self.state.fetch_or(STOPPING_BIT | TERMINATED_BIT, Ordering::AcqRel));

        // If we set `TERMINATED_BIT` and the refcount was 0, then we need to
        // drop `shared`.
        if !pre_terminate.is_terminated() && pre_terminate.refcount() == 0 {
            self.shared.with_mut(|shared| {
                // SAFETY: We set `TERMINATED_BIT` while the refcount was 0.
                unsafe {
                    ManuallyDrop::drop(&mut *shared);
                }
            });
        }

        // Wake all of the futures waiting for a termination reason
        let wakers = take(&mut *self.termination_wakers.lock().unwrap());
        for waker in wakers {
            waker.wake();
        }
    }
}

pub type SendFutureOutput<T> = Result<(), ProtocolError<<T as Transport>::Error>>;

#[pin_project(project = SendFutureStateProj, project_replace = SendFutureStateProjOwn)]
pub enum SendFutureState<T: Transport> {
    Running {
        #[pin]
        future_state: T::SendFutureState,
    },
    Stopping,
    Terminated {
        error: ProtocolError<T::Error>,
    },
    Waiting {
        waker_index: usize,
    },
    Finished,
}

impl<T: Transport> SendFutureState<T> {
    fn register_termination_waker(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        connection: &Connection<T>,
        waker_index: Option<usize>,
    ) -> Poll<SendFutureOutput<T>> {
        let mut wakers = connection.termination_wakers.lock().unwrap();

        // Re-check the state now that we're holding the lock again. This
        // prevents us from adding wakers after termination (which would "leak"
        // them).
        if let Some(termination_reason) = connection.get_termination_reason() {
            Poll::Ready(Err(termination_reason))
        } else {
            let waker = cx.waker().clone();
            if let Some(waker_index) = waker_index {
                // Overwrite an existing waker
                let old_waker = replace(&mut wakers[waker_index], waker);

                // Drop the old waker outside of the mutex lock
                drop(wakers);
                drop(old_waker);
            } else {
                // Insert a new waker
                let waker_index = wakers.len();
                wakers.push(waker);

                // Update the state outside of the mutex lock. If we were
                // running then a `T::SendFutureState` may be dropped.
                drop(wakers);
                self.set(SendFutureState::Waiting { waker_index });
            }
            Poll::Pending
        }
    }

    pub fn poll_send(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        connection: &Connection<T>,
    ) -> Poll<SendFutureOutput<T>> {
        match self.as_mut().project() {
            SendFutureStateProj::Running { future_state } => {
                let result = connection.with_shared(
                    |shared| {
                        T::poll_send(future_state, cx, shared)
                            // `Err(Some(error))` =>
                            //   `Err(Some(TransportError(error)))`
                            .map_err(|error| error.map(ProtocolError::TransportError))
                    },
                    |error| Poll::Ready(Err(error)),
                );

                let result = match result {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                    Poll::Ready(Err(None)) => {
                        self.as_mut().register_termination_waker(cx, connection, None)
                    }
                    Poll::Ready(Err(Some(error))) => Poll::Ready(Err(error)),
                };

                if result.is_ready() {
                    self.set(Self::Finished);
                }

                result
            }
            SendFutureStateProj::Stopping => self.register_termination_waker(cx, connection, None),
            SendFutureStateProj::Terminated { .. } => {
                let state = self.project_replace(Self::Finished);
                let SendFutureStateProjOwn::Terminated { error } = state else {
                    // SAFETY: We just checked that our state is Terminated.
                    unsafe { unreachable_unchecked() }
                };
                Poll::Ready(Err(error))
            }
            SendFutureStateProj::Waiting { waker_index } => {
                let waker_index = *waker_index;
                self.register_termination_waker(cx, connection, Some(waker_index))
            }
            SendFutureStateProj::Finished => {
                panic!("SendFuture polled after returning `Poll::Ready`")
            }
        }
    }

    pub fn send_immediately(self, connection: &Connection<T>) -> SendFutureOutput<T>
    where
        T: NonBlockingTransport,
    {
        match self {
            SendFutureState::Running { mut future_state } => {
                connection.with_shared(
                    |shared| {
                        // Connection is running, try to send immediately.
                        T::send_immediately(&mut future_state, shared).map_err(|e| {
                            // Immediate send failed:
                            // - `None` => `PeerClosed`
                            // - `Some(T::Error)` => `TransportError(T::Error)`
                            e.map_or(ProtocolError::PeerClosed, ProtocolError::TransportError)
                        })
                    },
                    // Getting shared failed, but we may have a termination
                    // reason. If we don't have one, return `Stopped`.
                    |error| Err(error.unwrap_or(ProtocolError::Stopped)),
                )
            }
            SendFutureState::Stopping | SendFutureState::Waiting { waker_index: _ } => {
                // Try to get the termination reason. If we don't have one yet,
                // return `Stopped`.
                Err(connection.get_termination_reason().unwrap_or(ProtocolError::Stopped))
            }
            SendFutureState::Terminated { error } => Err(error),
            SendFutureState::Finished => panic!("SendFuture polled after returning `Poll::Ready`"),
        }
    }
}

/// A future which sends an encoded message to a connection.
#[must_use = "futures do nothing unless polled"]
#[pin_project]
pub struct SendFuture<'a, T: Transport> {
    connection: &'a Connection<T>,
    #[pin]
    state: SendFutureState<T>,
}

impl<'a, T: Transport> SendFuture<'a, T> {
    /// Creates a `SendFuture` from its raw connection reference and state.
    pub fn from_raw_parts(connection: &'a Connection<T>, state: SendFutureState<T>) -> Self {
        Self { connection, state }
    }
}

impl<T: NonBlockingTransport> SendFuture<'_, T> {
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
    pub fn send_immediately(self) -> SendFutureOutput<T> {
        self.state.send_immediately(self.connection)
    }
}

impl<T: Transport> Future for SendFuture<'_, T> {
    type Output = SendFutureOutput<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.state.poll_send(cx, this.connection)
    }
}

#[pin_project]
pub struct SendEpitaphFuture<'a, T: Transport> {
    shared: &'a T::Shared,
    #[pin]
    future_state: T::SendFutureState,
}

impl<T: Transport> Future for SendEpitaphFuture<'_, T> {
    type Output = Result<(), Option<T::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        T::poll_send(this.future_state, cx, this.shared)
    }
}

/// A future which receives an encoded message over the transport.
#[must_use = "futures do nothing unless polled"]
#[pin_project]
pub struct RecvFuture<'a, T: Transport> {
    connection: &'a Connection<T>,
    exclusive: &'a mut T::Exclusive,
    #[pin]
    future_state: T::RecvFutureState,
}

impl<T: Transport> Future for RecvFuture<'_, T> {
    type Output = Result<T::RecvBuffer, ProtocolError<T::Error>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // SAFETY: This future is created by `Connection::recv`. The connection
        // will not be terminated until this is completed or canceled, and so
        // `shared` will not be dropped.
        let shared = unsafe { this.connection.get_shared_unchecked() };

        let termination_reason = match T::poll_recv(this.future_state, cx, shared, this.exclusive) {
            Poll::Pending => {
                // Receive didn't complete, register waker before
                // re-checking state.
                this.connection.stop_waker.register_by_ref(cx.waker());
                let state = State(this.connection.state.load(Ordering::Relaxed));
                if state.is_stopping() {
                    // The connection is stopping. Return an error that the
                    // connection has been stopped.
                    ProtocolError::Stopped
                } else {
                    // Still running, we'll get polled again later.
                    return Poll::Pending;
                }
            }

            // Receive succeeded.
            Poll::Ready(Ok(buffer)) => return Poll::Ready(Ok(buffer)),

            // Normal failure: return peer closed error.
            Poll::Ready(Err(None)) => ProtocolError::PeerClosed,

            // Abnormal failure: return transport error.
            Poll::Ready(Err(Some(error))) => ProtocolError::TransportError(error),
        };

        Poll::Ready(Err(termination_reason))
    }
}
