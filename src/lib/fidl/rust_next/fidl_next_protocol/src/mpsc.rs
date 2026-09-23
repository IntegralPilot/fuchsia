// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A basic [`Transport`] implementation based on MPSC channels.

use core::fmt;
use core::mem::{ManuallyDrop, take};
use core::pin::Pin;
use core::task::{Context, Poll};

use fidl_next_codec::Chunk;
use fuchsia_loom::future::AtomicWaker;
use fuchsia_loom::sync::atomic::{AtomicBool, Ordering};
use fuchsia_loom::sync::{Arc, mpsc};

use crate::{NonBlockingTransport, Transport};

/// A paired mpsc transport.
pub struct Mpsc {
    shared: Shared,
    exclusive: Exclusive,
}

impl Mpsc {
    /// Creates two mpscs which can communicate with each other.
    pub fn new() -> (Self, Self) {
        let state = Arc::new(State {
            send_wakers: [AtomicWaker::new(), AtomicWaker::new()],
            is_closed: AtomicBool::new(false),
        });
        let (a_send, a_recv) = mpsc::channel();
        let (b_send, b_recv) = mpsc::channel();
        (
            Mpsc {
                shared: Shared { state: state.clone(), end: 0, sender: ManuallyDrop::new(a_send) },
                exclusive: Exclusive { receiver: b_recv },
            },
            Mpsc {
                shared: Shared { state, end: 1, sender: ManuallyDrop::new(b_send) },
                exclusive: Exclusive { receiver: a_recv },
            },
        )
    }
}

/// The error type for paired mpsc transports.
#[derive(Clone, Debug)]
pub enum Error {}

impl fmt::Display for Error {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {}
    }
}

impl core::error::Error for Error {}

struct State {
    send_wakers: [AtomicWaker; 2],
    is_closed: AtomicBool,
}

/// The shared part of a paired mpsc transport.
pub struct Shared {
    state: Arc<State>,
    end: usize,
    sender: ManuallyDrop<mpsc::Sender<Vec<Chunk>>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Make sure that the mpsc is closed before waking the other end
        unsafe {
            ManuallyDrop::drop(&mut self.sender);
        }
        self.state.is_closed.store(true, Ordering::Relaxed);
        self.state.send_wakers[self.end].wake();
    }
}

/// The send future for a paired mpsc transport.
pub struct SendFutureState {
    buffer: Vec<Chunk>,
}

/// The exclusive part of a paired mpsc transport.
pub struct Exclusive {
    receiver: mpsc::Receiver<Vec<Chunk>>,
}

impl Transport for Mpsc {
    type Error = Error;

    fn split(self) -> (Self::Shared, Self::Exclusive) {
        (self.shared, self.exclusive)
    }

    type Shared = Shared;
    type SendBuffer = Vec<Chunk>;
    type SendFutureState = SendFutureState;

    fn acquire(_: &Self::Shared) -> Self::SendBuffer {
        Vec::new()
    }

    fn begin_send(_: &Self::Shared, buffer: Self::SendBuffer) -> Self::SendFutureState {
        SendFutureState { buffer }
    }

    fn poll_send(
        future_state: Pin<&mut SendFutureState>,
        _: &mut Context<'_>,
        shared: &Self::Shared,
    ) -> Poll<Result<(), Option<Error>>> {
        Poll::Ready(Self::send_immediately(future_state.get_mut(), shared))
    }

    type Exclusive = Exclusive;
    type RecvFutureState = ();
    type RecvBuffer = Vec<Chunk>;

    fn begin_recv(_: &Self::Shared, _: &mut Self::Exclusive) -> Self::RecvFutureState {}

    fn poll_recv(
        _: Pin<&mut Self::RecvFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
        exclusive: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>> {
        shared.state.send_wakers[1 - shared.end].register_by_ref(cx.waker());
        if shared.state.is_closed.load(Ordering::Relaxed) {
            return Poll::Ready(Err(None));
        }

        match exclusive.receiver.try_recv() {
            Ok(chunks) => Poll::Ready(Ok(chunks)),
            Err(mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Ready(Err(None)),
        }
    }
}

impl NonBlockingTransport for Mpsc {
    fn send_immediately(
        future_state: &mut Self::SendFutureState,
        shared: &Self::Shared,
    ) -> Result<(), Option<Self::Error>> {
        let chunks = take(&mut future_state.buffer);
        match shared.sender.send(chunks) {
            Ok(()) => {
                shared.state.send_wakers[shared.end].wake();
                Ok(())
            }
            Err(_) => Err(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use fuchsia_async as fasync;

    use super::Mpsc;
    use crate::testing::*;

    #[fasync::run_singlethreaded(test)]
    async fn close_on_drop() {
        test_close_on_drop(Mpsc::new).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn one_way() {
        test_one_way(Mpsc::new).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn one_way_nonblocking() {
        test_one_way_nonblocking(Mpsc::new).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn two_way() {
        test_two_way(Mpsc::new).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn multiple_two_way() {
        test_multiple_two_way(Mpsc::new).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn event() {
        test_event(Mpsc::new).await;
    }
}
