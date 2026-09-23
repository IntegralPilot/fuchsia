// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::rwhandle::{RWHandle, ReadableHandle as _, WritableHandle as _};
use futures::ready;
use std::fmt;
use std::future::poll_fn;
use std::mem::MaybeUninit;
use std::num::NonZeroUsize;
use std::task::{Context, Poll};
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx::{self as zx, AsHandleRef};

/// Marker trait for types that can be read/written with a `Fifo`.
///
/// An implementation is provided for all types that implement
/// [`IntoBytes`], [`FromBytes`], and [`Immutable`].
pub trait FifoEntry: IntoBytes + FromBytes + Immutable {}

impl<O: IntoBytes + FromBytes + Immutable> FifoEntry for O {}

/// A buffer used to write `T` into [`Fifo`] objects.
pub trait FifoWriteBuffer<T> {
    fn as_slice(&self) -> &[T];
}

/// A buffer used to read `T` from [`Fifo`] objects.
///
/// # Safety
///
/// This trait is unsafe because the compiler cannot verify a correct
/// implementation of `as_bytes_ptr_mut`. See
/// [`FifoReadBuffer::as_bytes_ptr_mut`] for safety notes.
pub unsafe trait FifoReadBuffer<T> {
    /// Returns the number of slots available in the buffer to be rceived.
    fn count(&self) -> usize;
    /// Returns a mutable pointer to the buffer contents where FIFO entries must
    /// be written into.
    ///
    /// # Safety
    ///
    /// The returned memory *must* be at least `count() * sizeof<T>()` bytes
    /// long.
    fn as_mut_ptr(&mut self) -> *mut T;
}

impl<T: FifoEntry, const N: usize> FifoWriteBuffer<T> for [T; N] {
    fn as_slice(&self) -> &[T] {
        self
    }
}

impl<T: FifoEntry> FifoWriteBuffer<T> for [T] {
    fn as_slice(&self) -> &[T] {
        self
    }
}

unsafe impl<T: FifoEntry, const N: usize> FifoReadBuffer<T> for [T; N] {
    fn count(&self) -> usize {
        N
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self.as_mut_slice().as_mut_ptr()
    }
}

unsafe impl<T: FifoEntry> FifoReadBuffer<T> for [T] {
    fn count(&self) -> usize {
        self.len()
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self.as_mut_ptr()
    }
}

impl<T: FifoEntry> FifoWriteBuffer<T> for T {
    fn as_slice(&self) -> &[T] {
        std::slice::from_ref(self)
    }
}

unsafe impl<T: FifoEntry> FifoReadBuffer<T> for T {
    fn count(&self) -> usize {
        1
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut T
    }
}

unsafe impl<T: FifoEntry> FifoReadBuffer<T> for MaybeUninit<T> {
    fn count(&self) -> usize {
        1
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self.as_mut_ptr()
    }
}

unsafe impl<T: FifoEntry> FifoReadBuffer<T> for [MaybeUninit<T>] {
    fn count(&self) -> usize {
        self.len()
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        // TODO(https://github.com/rust-lang/rust/issues/63569): Use
        // `MaybeUninit::slice_as_mut_ptr` once stable.
        self.as_mut_ptr() as *mut T
    }
}

/// An I/O object representing a `Fifo`.
pub struct Fifo<R, W = R> {
    handle: RWHandle<zx::Fifo<R, W>>,
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> AsRef<zx::Fifo<R, W>> for Fifo<R, W> {
    fn as_ref(&self) -> &zx::Fifo<R, W> {
        self.handle.get_ref()
    }
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> AsHandleRef for Fifo<R, W> {
    fn as_handle_ref(&self) -> zx::HandleRef<'_> {
        self.handle.get_ref().as_handle_ref()
    }
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> From<Fifo<R, W>> for zx::Fifo<R, W> {
    fn from(fifo: Fifo<R, W>) -> zx::Fifo<R, W> {
        fifo.handle.into_inner()
    }
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> Fifo<R, W> {
    /// Creates a new `Fifo` from a previously-created `zx::Fifo`.
    ///
    /// # Panics
    ///
    /// If called on a thread that does not have a current async executor.
    pub fn from_fifo(fifo: impl Into<zx::Fifo<R, W>>) -> Self {
        Fifo { handle: RWHandle::new(fifo.into()) }
    }

    /// Writes entries to the fifo and registers this `Fifo` as needing a write on receiving a
    /// `zx::Status::SHOULD_WAIT`.
    ///
    /// Returns the number of elements processed.
    ///
    /// NOTE: Only one writer is supported; this will overwrite any waker registered with a previous
    /// invocation to `try_write`.
    pub fn try_write<B: ?Sized + FifoWriteBuffer<W>>(
        &self,
        cx: &mut Context<'_>,
        entries: &B,
    ) -> Poll<Result<NonZeroUsize, zx::Status>> {
        ready!(self.handle.poll_writable(cx)?);

        let entries = entries.as_slice();
        let fifo = self.as_ref();
        // SAFETY: Safety relies on us keeping the slice alive over the call to `write_raw`, which
        // we do.
        loop {
            let result = unsafe { fifo.write_raw(entries.as_ptr(), entries.len()) };
            match result {
                Err(zx::Status::SHOULD_WAIT) => ready!(self.handle.need_writable(cx)?),
                Err(e) => return Poll::Ready(Err(e)),
                Ok(count) => return Poll::Ready(Ok(count)),
            }
        }
    }

    /// Reads entries from the fifo into `entries` and registers this `Fifo` as needing a read on
    /// receiving a `zx::Status::SHOULD_WAIT`.
    ///
    /// NOTE: Only one reader is supported; this will overwrite any waker registered with a previous
    /// invocation to `try_read`.
    pub fn try_read<B: ?Sized + FifoReadBuffer<R>>(
        &self,
        cx: &mut Context<'_>,
        entries: &mut B,
    ) -> Poll<Result<NonZeroUsize, zx::Status>> {
        ready!(self.handle.poll_readable(cx)?);

        let buf = entries.as_mut_ptr();
        let count = entries.count();
        let fifo = self.as_ref();

        loop {
            // SAFETY: Safety relies on the pointer returned by `B` being valid,
            // which itself depends on a correct implementation of `FifoEntry` for
            // `R`.
            let result = unsafe { fifo.read_raw(buf, count) };

            match result {
                Err(zx::Status::SHOULD_WAIT) => ready!(self.handle.need_readable(cx)?),
                Err(e) => return Poll::Ready(Err(e)),
                Ok(count) => return Poll::Ready(Ok(count)),
            }
        }
    }

    /// Returns a reader and writer which have async functions that can be used to read and write
    /// requests.
    pub fn async_io(&mut self) -> (FifoReader<'_, R, W>, FifoWriter<'_, R, W>) {
        (FifoReader(self), FifoWriter(self))
    }
}

pub struct FifoWriter<'a, R, W>(&'a Fifo<R, W>);

impl<R: FifoEntry, W: FifoEntry> FifoWriter<'_, R, W> {
    /// NOTE: If this future is dropped or there is an error, there is no indication how many
    /// entries were successfully written.
    pub async fn write_entries(
        &mut self,
        entries: &(impl ?Sized + FifoWriteBuffer<W>),
    ) -> Result<(), zx::Status> {
        let mut entries = entries.as_slice();
        poll_fn(|cx| {
            while !entries.is_empty() {
                match ready!(self.0.try_write(cx, entries)) {
                    Ok(count) => entries = &entries[count.get()..],
                    Err(status) => return Poll::Ready(Err(status)),
                }
            }
            Poll::Ready(Ok(()))
        })
        .await
    }

    /// Same as Fifo::try_write.
    pub fn try_write<B: ?Sized + FifoWriteBuffer<W>>(
        &mut self,
        cx: &mut Context<'_>,
        entries: &B,
    ) -> Poll<Result<NonZeroUsize, zx::Status>> {
        self.0.try_write(cx, entries)
    }
}

pub struct FifoReader<'a, R, W>(&'a Fifo<R, W>);

impl<R: FifoEntry, W: FifoEntry> FifoReader<'_, R, W> {
    pub async fn read_entries(
        &mut self,
        entries: &mut (impl ?Sized + FifoReadBuffer<R>),
    ) -> Result<NonZeroUsize, zx::Status> {
        poll_fn(|cx| self.0.try_read(cx, entries)).await
    }

    /// Same as Fifo::try_read.
    pub fn try_read<B: ?Sized + FifoReadBuffer<R>>(
        &mut self,
        cx: &mut Context<'_>,
        entries: &mut B,
    ) -> Poll<Result<NonZeroUsize, zx::Status>> {
        self.0.try_read(cx, entries)
    }
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> fmt::Debug for Fifo<R, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.handle.get_ref().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DurationExt, TestExecutor, Timer};
    use futures::future::try_join;
    use futures::prelude::*;
    use zerocopy::{Immutable, KnownLayout};

    #[derive(
        Copy, Clone, Debug, PartialEq, Eq, Default, IntoBytes, KnownLayout, FromBytes, Immutable,
    )]
    #[repr(C)]
    struct Entry {
        a: u32,
        b: u32,
    }

    #[derive(
        Clone, Debug, PartialEq, Eq, Default, IntoBytes, KnownLayout, FromBytes, Immutable,
    )]
    #[repr(C)]
    struct WrongEntry {
        a: u16,
    }

    #[test]
    fn can_read_write() {
        let mut exec = TestExecutor::new();
        let element = Entry { a: 10, b: 20 };

        let (tx, rx) = zx::Fifo::<Entry>::create(2).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));
        let (_, mut tx) = tx.async_io();
        let (mut rx, _) = rx.async_io();

        let mut buffer = Entry::default();
        let receiver = rx.read_entries(&mut buffer).map_ok(|count| {
            assert_eq!(count.get(), 1);
        });

        // Sends an entry after the timeout has passed
        let sender = Timer::new(zx::MonotonicDuration::from_millis(10).after_now())
            .then(|()| tx.write_entries(&element));

        let done = try_join(receiver, sender);
        exec.run_singlethreaded(done).expect("failed to run receive future on executor");
        assert_eq!(buffer, element);
    }

    #[test]
    fn read_wrong_size() {
        let mut exec = TestExecutor::new();
        let elements = &[Entry { a: 10, b: 20 }][..];

        let (tx, rx) = zx::Fifo::<Entry>::create(2).expect("failed to create zx fifo");
        let wrong_rx = zx::Fifo::<WrongEntry>::from(rx.into_handle());
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(wrong_rx));
        let (_, mut tx) = tx.async_io();
        let (mut rx, _) = rx.async_io();

        let mut buffer = WrongEntry::default();
        let receiver = rx
            .read_entries(&mut buffer)
            .map_ok(|count| panic!("read should have failed, got {count}"));

        // Sends an entry after the timeout has passed
        let sender = Timer::new(zx::MonotonicDuration::from_millis(10).after_now())
            .then(|()| tx.write_entries(elements));

        let done = try_join(receiver, sender);
        let res = exec.run_singlethreaded(done);
        match res {
            Err(zx::Status::OUT_OF_RANGE) => (),
            _ => panic!("did not get out-of-range error"),
        }
    }

    #[test]
    fn write_wrong_size() {
        let mut exec = TestExecutor::new();
        let elements = &[WrongEntry { a: 10 }][..];

        let (tx, rx) = zx::Fifo::<Entry>::create(2).expect("failed to create zx fifo");
        let wrong_tx = zx::Fifo::<WrongEntry>::from(tx.into_handle());
        let wrong_rx = zx::Fifo::<WrongEntry>::from(rx.into_handle());
        let (mut tx, _rx) = (Fifo::from_fifo(wrong_tx), Fifo::from_fifo(wrong_rx));
        let (_, mut tx) = tx.async_io();

        let sender = Timer::new(zx::MonotonicDuration::from_millis(10).after_now())
            .then(|()| tx.write_entries(elements));

        let res = exec.run_singlethreaded(sender);
        match res {
            Err(zx::Status::OUT_OF_RANGE) => (),
            _ => panic!("did not get out-of-range error"),
        }
    }

    #[test]
    fn write_into_full() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut exec = TestExecutor::new();
        let elements =
            &[Entry { a: 10, b: 20 }, Entry { a: 30, b: 40 }, Entry { a: 50, b: 60 }][..];

        let (tx, rx) = zx::Fifo::<Entry>::create(2).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));

        // Use `writes_completed` to verify that not all writes
        // are transmitted at once, and the last write is actually blocked.
        let writes_completed = AtomicUsize::new(0);
        let sender = async {
            let (_, mut writer) = tx.async_io();
            writer.write_entries(&elements[..2]).await?;
            writes_completed.fetch_add(1, Ordering::SeqCst);
            writer.write_entries(&elements[2..]).await?;
            writes_completed.fetch_add(1, Ordering::SeqCst);
            Ok::<(), zx::Status>(())
        };

        // Wait 10 ms, then read the messages from the fifo.
        let receiver = async {
            Timer::new(zx::MonotonicDuration::from_millis(10).after_now()).await;
            let mut buffer = Entry::default();
            let (mut reader, _) = rx.async_io();
            let count = reader.read_entries(&mut buffer).await?;
            assert_eq!(writes_completed.load(Ordering::SeqCst), 1);
            assert_eq!(count.get(), 1);
            assert_eq!(buffer, elements[0]);
            let count = reader.read_entries(&mut buffer).await?;
            // At this point, the last write may or may not have
            // been written.
            assert_eq!(count.get(), 1);
            assert_eq!(buffer, elements[1]);
            let count = reader.read_entries(&mut buffer).await?;
            assert_eq!(writes_completed.load(Ordering::SeqCst), 2);
            assert_eq!(count.get(), 1);
            assert_eq!(buffer, elements[2]);
            Ok::<(), zx::Status>(())
        };

        let done = try_join(receiver, sender);

        exec.run_singlethreaded(done).expect("failed to run receive future on executor");
    }

    #[test]
    fn write_more_than_full() {
        let mut exec = TestExecutor::new();
        let elements =
            &[Entry { a: 10, b: 20 }, Entry { a: 30, b: 40 }, Entry { a: 50, b: 60 }][..];

        let (tx, rx) = zx::Fifo::<Entry>::create(2).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));
        let (_, mut tx) = tx.async_io();
        let (mut rx, _) = rx.async_io();

        let sender = tx.write_entries(elements);

        // Wait 10 ms, then read the messages from the fifo.
        let receiver = async {
            Timer::new(zx::MonotonicDuration::from_millis(10).after_now()).await;
            for e in elements {
                let mut buffer = [Entry::default(); 1];
                let count = rx.read_entries(&mut buffer[..]).await?;
                assert_eq!(count.get(), 1);
                assert_eq!(&buffer[0], e);
            }
            Ok::<(), zx::Status>(())
        };

        let done = try_join(receiver, sender);

        exec.run_singlethreaded(done).expect("failed to run receive future on executor");
    }

    #[test]
    fn read_multiple() {
        let mut exec = TestExecutor::new();
        let elements =
            &[Entry { a: 10, b: 20 }, Entry { a: 30, b: 40 }, Entry { a: 50, b: 60 }][..];
        let (tx, rx) = zx::Fifo::<Entry>::create(elements.len()).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));

        let write_fut = async {
            tx.async_io().1.write_entries(elements).await.expect("failed write entries");
        };
        let read_fut = async {
            // Use a larger buffer to show partial reads.
            let mut buffer = [Entry::default(); 5];
            let count = rx
                .async_io()
                .0
                .read_entries(&mut buffer[..])
                .await
                .expect("failed to read entries");
            assert_eq!(count.get(), elements.len());
            assert_eq!(&buffer[..count.get()], elements);
        };
        let ((), ()) = exec.run_singlethreaded(futures::future::join(write_fut, read_fut));
    }

    #[test]
    fn read_one() {
        let mut exec = TestExecutor::new();
        let elements =
            &[Entry { a: 10, b: 20 }, Entry { a: 30, b: 40 }, Entry { a: 50, b: 60 }][..];
        let (tx, rx) = zx::Fifo::<Entry>::create(elements.len()).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));

        let write_fut = async {
            tx.async_io().1.write_entries(elements).await.expect("failed write entries");
        };
        let read_fut = async {
            let (mut reader, _) = rx.async_io();
            for e in elements {
                let mut entry = Entry::default();
                assert_eq!(
                    reader.read_entries(&mut entry).await.expect("failed to read entry").get(),
                    1
                );
                assert_eq!(&entry, e);
            }
        };
        let ((), ()) = exec.run_singlethreaded(futures::future::join(write_fut, read_fut));
    }

    #[test]
    fn maybe_uninit_single() {
        let mut exec = TestExecutor::new();
        let element = Entry { a: 10, b: 20 };
        let (tx, rx) = zx::Fifo::<Entry>::create(1).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));

        let write_fut = async {
            tx.async_io().1.write_entries(&element).await.expect("failed write entries");
        };
        let read_fut = async {
            let mut buffer = MaybeUninit::<Entry>::uninit();
            let count =
                rx.async_io().0.read_entries(&mut buffer).await.expect("failed to read entries");
            assert_eq!(count.get(), 1);
            // SAFETY: We just read a new entry into the buffer.
            let read = unsafe { buffer.assume_init() };
            assert_eq!(read, element);
        };
        let ((), ()) = exec.run_singlethreaded(futures::future::join(write_fut, read_fut));
    }

    #[test]
    fn maybe_uninit_slice() {
        let mut exec = TestExecutor::new();
        let elements =
            &[Entry { a: 10, b: 20 }, Entry { a: 30, b: 40 }, Entry { a: 50, b: 60 }][..];
        let (tx, rx) = zx::Fifo::<Entry>::create(elements.len()).expect("failed to create zx fifo");
        let (mut tx, mut rx) = (Fifo::from_fifo(tx), Fifo::from_fifo(rx));

        let write_fut = async {
            tx.async_io().1.write_entries(elements).await.expect("failed write entries");
        };
        let read_fut = async {
            // Use a larger buffer to show partial reads.
            let mut buffer = [MaybeUninit::<Entry>::uninit(); 15];
            let count = rx
                .async_io()
                .0
                .read_entries(&mut buffer[..])
                .await
                .expect("failed to read entries");
            assert_eq!(count.get(), elements.len());
            let read = &mut buffer[..count.get()];
            for (i, v) in read.iter_mut().enumerate() {
                // SAFETY: This is the read region of the buffer, initialized by
                // reading from the FIFO.
                let read = unsafe { v.assume_init_ref() };
                assert_eq!(read, &elements[i]);
                // SAFETY: The buffer was partially initialized by reading from
                // the FIFO, the correct thing to do here is to manually drop
                // the elements that were initialized.
                unsafe {
                    v.assume_init_drop();
                }
            }
        };
        let ((), ()) = exec.run_singlethreaded(futures::future::join(write_fut, read_fut));
    }
}
