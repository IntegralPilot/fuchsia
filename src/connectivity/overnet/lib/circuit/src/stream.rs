// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{Error, Result};
use crate::protocol;

use fuchsia_sync::Mutex as SyncMutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::task::{Context, Poll, Waker, ready};

/// We shrink our internal buffers until they are no more than this much larger
/// than the actual data we are accumulating.
///
/// The value of 1MiB was discovered experimentally to yield acceptable
/// performance on tests.
const BUFFER_TRIM_GRANULARITY: usize = 1048576;

/// BUFFER_TRIM_GRANULARITY should be a power of 2.
const _: () = assert!(BUFFER_TRIM_GRANULARITY.is_power_of_two());

/// Indicates whether a stream is open or closed, and if closed, why it closed.
#[derive(Debug, Clone)]
enum Status {
    /// Stream is open.
    Open,
    /// Stream is closed. Argument may contain a reason for closure.
    Closed(Option<String>),
}

impl Status {
    fn is_closed(&self) -> bool {
        match self {
            Status::Open => false,
            Status::Closed(_) => true,
        }
    }

    fn reason(&self) -> Option<String> {
        match self {
            Status::Open => None,
            Status::Closed(x) => x.clone(),
        }
    }

    fn close(&mut self) {
        if let Status::Open = self {
            *self = Status::Closed(None);
        }
    }
}

/// Internal state of a stream. See `stream()`.
#[derive(Debug)]
struct State {
    /// The ring buffer itself.
    deque: VecDeque<u8>,
    /// How many bytes are readable. This is different from the length of the deque, as we may allow
    /// bytes to be in the deque that are "initialized but unavailable." Mostly that just
    /// accommodates a quirk of Rust's memory model; we have to initialize all bytes before we show
    /// them to the user, even if they're there just to be overwritten, and if we pop the bytes out
    /// of the deque Rust counts them as uninitialized again, so to avoid duplicating the
    /// initialization process we just leave the initialized-but-unwritten bytes in the deque.
    readable: usize,
    /// If the reader needs to sleep, it puts a waker here so it can be woken up
    /// again. It also lists how many bytes should be available before it should
    /// be woken up.
    notify_readable: Option<(Waker, usize)>,
    /// Whether this stream is closed. I.e. whether either the `Reader` or `Writer` has been dropped.
    closed: Status,
}

/// Read half of a stream. See `stream()`.
pub struct Reader(Arc<SyncMutex<State>>);

impl Reader {
    /// Debug
    pub fn inspect_shutdown(&self) -> String {
        let lock = self.0.lock();
        if lock.closed.is_closed() {
            lock.closed.reason().unwrap_or_else(|| "No epitaph".to_owned())
        } else {
            "Not closed".to_owned()
        }
    }

    /// Read bytes from the stream.
    ///
    /// The reader will wait until there are *at least* `size` bytes to read,
    /// Then it will call the given callback with a slice containing all
    /// available bytes to read.
    ///
    /// If the callback processes data successfully, it should return `Ok` with
    /// a tuple containing a value of the user's choice, and the number of bytes
    /// used. If the number of bytes returned from the callback is less than
    /// what was available in the buffer, the unused bytes will appear at the
    /// start of the buffer for subsequent read calls. It is allowable to `peek`
    /// at the bytes in the buffer by returning a number of bytes read that is
    /// smaller than the number of bytes actually used.
    ///
    /// If the callback returns `Error::BufferTooShort` and the expected buffer
    /// value contained in the error is larger than the data that was provided,
    /// we will wait again until there are enough bytes to satisfy the error and
    /// then call the callback again. If the callback returns
    /// `Error::BufferTooShort` but the buffer should have been long enough
    /// according to the error, `Error::CallbackRejectedBuffer` is returned.
    /// Other errors from the callback are returned as-is from `read` itself.
    ///
    /// If there are no bytes available to read and the `Writer` for this stream
    /// has already been dropped, `read` returns `Error::ConnectionClosed`. If
    /// there are *not enough* bytes available to be read and the `Writer` has
    /// been dropped, `read` returns `Error::BufferTooSmall`. This is the only
    /// time `read` should return `Error::BufferTooSmall`.
    ///
    /// Panics if the callback returns a number of bytes greater than the size
    /// of the buffer.
    pub async fn read<F, U>(&self, mut size: usize, mut f: F) -> Result<U>
    where
        F: FnMut(&[u8]) -> Result<(U, usize)>,
    {
        let mut f = move |_: &mut Context<'_>, b: &'_ [u8]| Poll::Ready(f(b));
        futures::future::poll_fn(move |ctx| self.poll_read(ctx, &mut size, &mut f)).await
    }

    /// Like `read` but a poll function rather than a future. Passes the
    /// `Context` to the callback so the callback can also poll.
    ///
    /// The `size` argument is a mutable reference so that if we determine
    /// during reading that more data is needed, we can update the size. It
    /// should always increase. The value of `size` is not meaningful after
    /// returning `Poll::Ready`.
    pub fn poll_read<F, U>(
        &self,
        ctx: &mut Context<'_>,
        size: &mut usize,
        mut f: F,
    ) -> Poll<Result<U>>
    where
        F: FnMut(&mut Context<'_>, &[u8]) -> Poll<Result<(U, usize)>>,
    {
        let mut state = self.0.lock();

        if let Status::Closed(reason) = &state.closed
            && *size == 0
        {
            return Poll::Ready(Err(Error::ConnectionClosed(reason.clone())));
        }

        if state.readable >= *size {
            let (first, _) = state.deque.as_slices();

            let first = if first.len() >= *size {
                first
            } else {
                state.deque.make_contiguous();
                state.deque.as_slices().0
            };

            debug_assert!(first.len() >= *size);

            let first = &first[..std::cmp::min(first.len(), state.readable)];
            let (ret, consumed) = match ready!(f(ctx, first)) {
                Err(Error::BufferTooShort(s)) => {
                    if s < first.len() {
                        return Poll::Ready(Err(Error::CallbackRejectedBuffer(s, first.len())));
                    }

                    *size = s;
                    ctx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                other => other?,
            };

            if consumed > first.len() {
                panic!("Read claimed to consume more bytes than it was given!");
            }

            state.readable -= consumed;
            state.deque.drain(..consumed);
            let target_capacity = std::cmp::max(
                state.deque.len().next_multiple_of(BUFFER_TRIM_GRANULARITY),
                BUFFER_TRIM_GRANULARITY,
            );

            if target_capacity <= state.deque.capacity() / 2 {
                state.deque.shrink_to(target_capacity);
            }
            return Poll::Ready(Ok(ret));
        }

        if let Status::Closed(reason) = &state.closed {
            if state.readable > 0 {
                return Poll::Ready(Err(Error::BufferTooShort(*size)));
            } else {
                return Poll::Ready(Err(Error::ConnectionClosed(reason.clone())));
            }
        }

        state.notify_readable = Some((ctx.waker().clone(), *size));
        Poll::Pending
    }

    /// Read a protocol message from the stream. This is just a quick way to wire
    /// `ProtocolObject::try_from_bytes` in to `read`.
    pub async fn read_protocol_message<P: protocol::ProtocolMessage>(&self) -> Result<P> {
        self.read(P::MIN_SIZE, P::try_from_bytes).await
    }

    /// This writes the given protocol message to the stream at the *beginning* of the stream,
    /// meaning that it will be the next thing read off the stream.
    pub(crate) fn push_back_protocol_message<P: protocol::ProtocolMessage>(
        &self,
        message: &P,
    ) -> Result<()> {
        let size = message.byte_size();
        let mut state = self.0.lock();
        let readable = state.readable;
        state.deque.resize(readable + size, 0);
        state.deque.rotate_right(size);
        let (first, _) = state.deque.as_mut_slices();

        let mut first = if first.len() >= size {
            first
        } else {
            state.deque.make_contiguous();
            state.deque.as_mut_slices().0
        };

        let got = message.write_bytes(&mut first)?;
        debug_assert!(got == size);
        state.readable += size;

        if let Some((waker, size)) = state.notify_readable.take() {
            if size <= state.readable {
                waker.wake();
            } else {
                state.notify_readable = Some((waker, size));
            }
        }

        Ok(())
    }

    /// Whether this stream is closed. Returns false so long as there is unread
    /// data in the buffer, even if the writer has hung up.
    pub fn is_closed(&self) -> bool {
        let state = self.0.lock();
        state.closed.is_closed() && state.readable == 0
    }

    /// Get the reason this reader is closed. If the reader is not closed, or if
    /// no reason was given, return `None`.
    pub fn closed_reason(&self) -> Option<String> {
        let state = self.0.lock();
        state.closed.reason()
    }

    /// Close this stream, giving a reason for the closure.
    pub fn close(self, reason: String) {
        let mut state = self.0.lock();
        match &state.closed {
            Status::Closed(Some(_)) => (),
            _ => state.closed = Status::Closed(Some(reason)),
        }
    }
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reader({:?})", Arc::as_ptr(&self.0))
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.closed.close();
    }
}

/// Write half of a stream. See `stream()`.
pub struct Writer(Arc<SyncMutex<State>>);

impl Writer {
    /// Write data to this stream.
    ///
    /// Space for `size` bytes is allocated in the stream immediately, and then the callback is
    /// invoked with a mutable slice to that region so that it may populate it. The slice given to
    /// the callback *may* be larger than requested but will never be smaller.
    ///
    /// The callback should return `Ok` with the number of bytes actually written, which may be less
    /// than `size`. If the callback returns an error, that error is returned from `write` as-is.
    /// Note that we do not specially process `Error::BufferTooSmall` as with `Reader::read`.
    ///
    /// Panics if the callback returns a number of bytes greater than the size of the buffer.
    pub fn write<F>(&self, size: usize, f: F) -> Result<()>
    where
        F: FnOnce(&mut [u8]) -> Result<usize>,
    {
        let mut state = self.0.lock();

        if let Status::Closed(reason) = &state.closed {
            return Err(Error::ConnectionClosed(reason.clone()));
        }

        let total_size = state.readable + size;

        if state.deque.len() < total_size {
            let total_size = std::cmp::max(total_size, state.deque.capacity());
            state.deque.resize(total_size, 0);
        }

        let readable = state.readable;
        let (first, second) = state.deque.as_mut_slices();

        let slice = if first.len() > readable {
            &mut first[readable..]
        } else {
            &mut second[(readable - first.len())..]
        };

        let slice = if slice.len() >= size {
            slice
        } else {
            state.deque.make_contiguous();
            &mut state.deque.as_mut_slices().0[readable..]
        };

        debug_assert!(slice.len() >= size);
        let size = f(slice)?;

        if size > slice.len() {
            panic!("Write claimed to produce more bytes than buffer had space for!");
        }

        state.readable += size;

        if let Some((waker, size)) = state.notify_readable.take() {
            if size <= state.readable {
                waker.wake();
            } else {
                state.notify_readable = Some((waker, size));
            }
        }

        Ok(())
    }

    /// Write a protocol message to the stream. This is just a quick way to wire
    /// `ProtocolObject::write_bytes` in to `write`.
    pub fn write_protocol_message<P: protocol::ProtocolMessage>(&self, message: &P) -> Result<()> {
        self.write(message.byte_size(), |mut buf| message.write_bytes(&mut buf))
    }

    /// Close this stream, giving a reason for the closure.
    pub fn close(self, reason: String) {
        self.0.lock().closed = Status::Closed(Some(reason))
    }

    /// Whether this stream is closed. Returns false so long as there is unread
    /// data in the buffer, even if the writer has hung up.
    pub fn is_closed(&self) -> bool {
        let state = self.0.lock();
        state.closed.is_closed() && state.readable == 0
    }

    /// Get the reason this writer is closed. If the writer is not closed, or if
    /// no reason was given, return `None`.
    pub fn closed_reason(&self) -> Option<String> {
        let state = self.0.lock();
        state.closed.reason()
    }
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Writer({:?})", Arc::as_ptr(&self.0))
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.closed.close();

        if let Some((waker, _)) = state.notify_readable.take() {
            waker.wake();
        }
    }
}

/// Creates a unidirectional stream of bytes.
///
/// The `Reader` and `Writer` share an expanding ring buffer. This allows
/// sending bytes between tasks with minimal extra allocations or copies.
pub fn stream() -> (Reader, Writer) {
    let reader = Arc::new(SyncMutex::new(State {
        deque: VecDeque::new(),
        readable: 0,
        notify_readable: None,
        closed: Status::Open,
    }));
    let writer = Arc::clone(&reader);

    (Reader(reader), Writer(writer))
}

#[cfg(test)]
mod test {
    use futures::FutureExt;
    use futures::channel::oneshot;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::*;

    impl protocol::ProtocolMessage for [u8; 4] {
        const MIN_SIZE: usize = 4;
        fn byte_size(&self) -> usize {
            4
        }

        fn write_bytes<W: std::io::Write>(&self, out: &mut W) -> Result<usize> {
            out.write_all(self)?;
            Ok(4)
        }

        fn try_from_bytes(bytes: &[u8]) -> Result<(Self, usize)> {
            if bytes.len() < 4 {
                return Err(Error::BufferTooShort(4));
            }

            Ok((bytes[..4].try_into().unwrap(), 4))
        }
    }

    #[fuchsia::test]
    async fn stream_test() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(4, |buf| Ok((buf[..4].to_vec(), 4))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4], got);

        writer
            .write(2, |buf| {
                buf[..2].copy_from_slice(&[9, 10]);
                Ok(2)
            })
            .unwrap();

        let got = reader.read(6, |buf| Ok((buf[..6].to_vec(), 6))).await.unwrap();

        assert_eq!(vec![5, 6, 7, 8, 9, 10], got);
    }

    #[fuchsia::test]
    async fn push_back_test() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(4, |buf| Ok((buf[..4].to_vec(), 4))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4], got);

        reader.push_back_protocol_message(&[4, 3, 2, 1]).unwrap();

        writer
            .write(2, |buf| {
                buf[..2].copy_from_slice(&[9, 10]);
                Ok(2)
            })
            .unwrap();

        let got = reader.read(10, |buf| Ok((buf[..10].to_vec(), 6))).await.unwrap();

        assert_eq!(vec![4, 3, 2, 1, 5, 6, 7, 8, 9, 10], got);
    }

    #[fuchsia::test]
    async fn writer_sees_close() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(4, |buf| Ok((buf[..4].to_vec(), 4))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4], got);

        std::mem::drop(reader);

        assert!(matches!(
            writer.write(2, |buf| {
                buf[..2].copy_from_slice(&[9, 10]);
                Ok(2)
            }),
            Err(Error::ConnectionClosed(None))
        ));
    }

    #[fuchsia::test]
    async fn reader_sees_closed() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(4, |buf| Ok((buf[..4].to_vec(), 4))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4], got);

        writer
            .write(2, |buf| {
                buf[..2].copy_from_slice(&[9, 10]);
                Ok(2)
            })
            .unwrap();

        std::mem::drop(writer);

        assert!(matches!(reader.read(7, |_| Ok(((), 1))).await, Err(Error::BufferTooShort(7))));

        let got = reader.read(6, |buf| Ok((buf[..6].to_vec(), 6))).await.unwrap();

        assert_eq!(vec![5, 6, 7, 8, 9, 10], got);
        assert!(matches!(
            reader.read(1, |_| Ok(((), 1))).await,
            Err(Error::ConnectionClosed(None))
        ));
    }

    #[fuchsia::test]
    async fn reader_sees_closed_when_polling() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(8, |buf| Ok((buf[..8].to_vec(), 8))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4, 5, 6, 7, 8], got);

        let fut = reader
            .read(1, |_| -> Result<((), usize)> { panic!("This read should never succeed!") });
        let mut fut = std::pin::pin!(fut);

        assert!(fut.poll_unpin(&mut Context::from_waker(Waker::noop())).is_pending());

        std::mem::drop(writer);

        assert!(matches!(
            fut.poll_unpin(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(Error::ConnectionClosed(None)))
        ));
    }

    #[fuchsia::test]
    async fn reader_sees_closed_separate_task() {
        let (reader, writer) = stream();
        writer
            .write(8, |buf| {
                buf[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
                Ok(8)
            })
            .unwrap();

        let got = reader.read(8, |buf| Ok((buf[..8].to_vec(), 8))).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4, 5, 6, 7, 8], got);

        let (sender, receiver) = oneshot::channel();
        let task = fuchsia_async::Task::spawn(async move {
            let fut = reader.read(1, |_| Ok(((), 1)));
            let mut fut = std::pin::pin!(fut);
            let mut writer = Some(writer);
            let fut = futures::future::poll_fn(move |cx| {
                let ret = fut.as_mut().poll(cx);

                if writer.take().is_some() {
                    assert!(matches!(ret, Poll::Pending));
                }

                ret
            });
            assert!(matches!(fut.await, Err(Error::ConnectionClosed(None))));
            sender.send(()).unwrap();
        });

        receiver.await.unwrap();
        task.await;
    }

    #[fuchsia::test]
    async fn reader_buffer_too_short() {
        let (reader, writer) = stream();
        let (sender, receiver) = oneshot::channel();
        let mut sender = Some(sender);

        let reader_task = async move {
            let got = reader
                .read(1, |buf| {
                    if buf.len() != 4 {
                        sender.take().unwrap().send(buf.len()).unwrap();
                        Err(Error::BufferTooShort(4))
                    } else {
                        Ok((buf[..4].to_vec(), 4))
                    }
                })
                .await
                .unwrap();
            assert_eq!(vec![1, 2, 3, 4], got);
        };

        let writer_task = async move {
            writer
                .write(2, |buf| {
                    buf[..2].copy_from_slice(&[1, 2]);
                    Ok(2)
                })
                .unwrap();

            assert_eq!(2, receiver.await.unwrap());

            writer
                .write(2, |buf| {
                    buf[..2].copy_from_slice(&[3, 4]);
                    Ok(2)
                })
                .unwrap();
        };

        futures::future::join(pin!(reader_task), pin!(writer_task)).await;
    }
}
