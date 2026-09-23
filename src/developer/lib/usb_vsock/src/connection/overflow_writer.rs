// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::io::WriteHalf;
use futures::lock::Mutex;
use futures::{AsyncWrite, FutureExt};
use std::collections::VecDeque;
use std::future::Future;
use std::io::{Error, ErrorKind, IoSlice};
use std::sync::Weak;
use std::task::{ready, Context, Poll, Waker};

use crate::connection::{ConnectionStateWriter, ConnectionStateWriterFut};

/// A wrapper around the write half of a socket that never fails due to the
/// socket being full. Instead it puts an infinitely-growing buffer in front of
/// the socket so it can always accept data.
///
/// When the buffer is non-empty, some async process must poll the
/// [`OverflowWriter::poll_handle_overflow`] function to keep bytes moving from
/// the buffer into the actual socket.
pub struct OverflowWriter<S> {
    inner: WriteHalf<S>,
    overflow: VecDeque<u8>,
}

/// Status returned from [`OverflowWriter::write_all`].
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum OverflowWriterStatus {
    NoOverflow,
    Overflow,
}

impl OverflowWriterStatus {
    /// Whether the status indicates that the socket overflowed during the write.
    pub fn overflowed(&self) -> bool {
        matches!(self, OverflowWriterStatus::Overflow)
    }
}

impl<S: AsyncWrite + Send + 'static> OverflowWriter<S> {
    /// Make a new [`OverflowWriter`], wrapping the given socket write half.
    pub fn new(inner: WriteHalf<S>) -> Self {
        OverflowWriter { inner, overflow: VecDeque::new() }
    }

    /// Write all data into the writer. As much data as possible will drain into
    /// the socket, but leftover data will be buffered and written later.
    ///
    /// If this write caused the socket to overflow, that is, if there was no
    /// buffered data but this write made us *start* buffering data, we will
    /// return [`OverflowWriterStatus::Overflow`].
    pub fn write_all(&mut self, mut data: &[u8]) -> Result<OverflowWriterStatus, Error> {
        if !self.overflow.is_empty() {
            self.overflow.extend(data.iter());
            // We only report when the socket *starts* to overflow. Piling on
            // more overflow data isn't signaled in the return value.
            return Ok(OverflowWriterStatus::NoOverflow);
        }

        let mut cx = Context::from_waker(Waker::noop());
        loop {
            match std::pin::Pin::new(&mut self.inner).poll_write(&mut cx, data) {
                Poll::Ready(res) => {
                    let res = res?;

                    if res == 0 {
                        return Err(ErrorKind::WriteZero.into());
                    }

                    if res == data.len() {
                        return Ok(OverflowWriterStatus::NoOverflow);
                    }

                    data = &data[res..];
                }
                Poll::Pending => {
                    self.overflow.extend(data.iter());
                    return Ok(OverflowWriterStatus::Overflow);
                }
            };
        }
    }
}

/// Future that drains any backlogged data from an OverflowWriter.
pub struct OverflowHandleFut<S> {
    writer: Weak<Mutex<ConnectionStateWriter<S>>>,
    guard_storage: Option<ConnectionStateWriterFut<S>>,
}

impl<S> OverflowHandleFut<S> {
    /// Create a new future to drain all backlogged data from the given writer.
    pub fn new(writer: Weak<Mutex<ConnectionStateWriter<S>>>) -> Self {
        OverflowHandleFut { writer, guard_storage: None }
    }
}

impl<S: AsyncWrite + Send + 'static> Future for OverflowHandleFut<S> {
    type Output = Result<(), Error>;

    fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Self::Output> {
        let lock_fut = if let Some(lock_fut) = &mut self.guard_storage {
            lock_fut
        } else {
            let Some(payload_socket) = self.writer.upgrade() else {
                return std::task::Poll::Ready(Ok(()));
            };
            self.guard_storage.insert(ConnectionStateWriter::wait_available(payload_socket))
        };

        let mut lock = ready!(lock_fut.poll_unpin(ctx));
        let lock = &mut *lock;
        self.guard_storage = None;
        let ConnectionStateWriter::Available(lock) = &mut *lock else {
            unreachable!("ConnectionStateWriterFut didn't wait until writer was available!");
        };

        while !lock.overflow.is_empty() {
            let (data_a, data_b) = lock.overflow.as_slices();
            let res = ready!(std::pin::Pin::new(&mut lock.inner)
                .poll_write_vectored(ctx, &[IoSlice::new(data_a), IoSlice::new(data_b)]))?;

            if res == 0 {
                return Poll::Ready(Err(ErrorKind::WriteZero.into()));
            }

            lock.overflow.drain(..res);
        }

        Poll::Ready(Ok(()))
    }
}
