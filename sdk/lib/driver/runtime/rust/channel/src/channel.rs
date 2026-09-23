// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for the driver runtime channel stable ABI

use core::future::Future;
use std::mem::ManuallyDrop;
use std::sync::Arc;
use zx::Status;

use crate::arena::{Arena, ArenaBox};
use crate::futures::{ReadMessageState, ReadMessageStateOp};
use crate::message::Message;
use fdf_core::handle::{DriverHandle, MixedHandle};
use fdf_sys::*;
use libasync_dispatcher::OnDispatcher;

use core::marker::PhantomData;
use core::mem::{MaybeUninit, size_of_val};
use core::num::NonZero;
use core::pin::Pin;
use core::ptr::{NonNull, null_mut};
use core::task::{Context, Poll};

pub use fdf_sys::fdf_handle_t;

/// Implements a message channel through the Fuchsia Driver Runtime
#[derive(Debug)]
pub struct Channel<T: ?Sized + 'static> {
    // Note: if we're waiting on a callback we can't drop the handle until
    // that callback has fired.
    pub(crate) handle: ManuallyDrop<DriverHandle>,
    pub(crate) wait_state: Option<Arc<ReadMessageStateOp>>,
    _p: PhantomData<Message<T>>,
}

impl<T: ?Sized> Drop for Channel<T> {
    fn drop(&mut self) {
        let mut can_drop = true;

        if let Some(current_wait) = &self.wait_state {
            // channel_dropped() will return true if we can drop the handle ourselves.
            // otherwise the channel should not be dropped until the callback is called.
            can_drop = current_wait.set_channel_dropped();
        }

        if can_drop {
            // SAFETY: If there's no current wait active, we are the only
            // owner of the handle.
            unsafe {
                ManuallyDrop::drop(&mut self.handle);
            }
        };
    }
}

impl<T: ?Sized + 'static> Channel<T> {
    /// Creates a new channel pair that can be used to send messages of type `T`
    /// between threads managed by the driver runtime.
    pub fn create() -> (Self, Self) {
        let mut channel1 = 0;
        let mut channel2 = 0;
        // This call cannot fail as the only reason it would fail is due to invalid
        // option flags, and 0 is a valid option.
        Status::ok(unsafe { fdf_channel_create(0, &mut channel1, &mut channel2) })
            .expect("failed to create channel pair");
        // SAFETY: if fdf_channel_create returned ZX_OK, it will have placed
        // valid channel handles that must be non-zero.
        unsafe {
            (
                Self::from_handle_unchecked(NonZero::new_unchecked(channel1)),
                Self::from_handle_unchecked(NonZero::new_unchecked(channel2)),
            )
        }
    }

    /// Returns a reference to the inner handle of the channel.
    pub fn driver_handle(&self) -> &DriverHandle {
        &self.handle
    }

    /// Takes the inner handle to the channel. The caller is responsible for ensuring
    /// that the handle is freed.
    ///
    /// # Panics
    ///
    /// This function will panic if the channel has previously had a read wait
    /// registered on it.
    pub fn into_driver_handle(self) -> DriverHandle {
        assert!(
            self.wait_state.is_none(),
            "A read wait has been registered on this channel so it can't be destructured"
        );

        // SAFETY: We will be forgetting `self` after this, so we can safely
        // take ownership of the raw handle for reconstituting into a `DriverHandle`
        // object after.
        let handle = unsafe { self.handle.get_raw() };

        // we don't want to call drop here because we've taken the handle out of the
        // object.
        std::mem::forget(self);

        // SAFETY: We just took this handle from the object we just forgot, so we
        // are the only owner of it.
        unsafe { DriverHandle::new_unchecked(handle) }
    }

    /// Initializes a [`Channel`] object from the given non-zero handle.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle is not invalid and that it is
    /// part of a driver runtime channel pair of type `T`.
    unsafe fn from_handle_unchecked(handle: NonZero<fdf_handle_t>) -> Self {
        // SAFETY: caller is responsible for ensuring that it is a valid channel
        Self {
            handle: ManuallyDrop::new(unsafe { DriverHandle::new_unchecked(handle) }),
            wait_state: None,
            _p: PhantomData,
        }
    }

    /// Initializes a [`Channel`] object from the given [`DriverHandle`],
    /// assuming that it is a channel of type `T`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle is a [`Channel`]-based handle that is
    /// using type `T` as its wire format.
    pub unsafe fn from_driver_handle(handle: DriverHandle) -> Self {
        Self { handle: ManuallyDrop::new(handle), wait_state: None, _p: PhantomData }
    }

    /// Writes the [`Message`] given to the channel. This will complete asynchronously and can't
    /// be cancelled.
    ///
    /// The channel will take ownership of the data and handles passed in,
    pub fn write(&self, message: Message<T>) -> Result<(), Status> {
        // get the sizes while the we still have refs to the data and handles
        let data_len = message.data().map_or(0, |data| size_of_val(data) as u32);
        let handles_count = message.handles().map_or(0, |handles| handles.len() as u32);

        let (arena, data, handles) = message.into_raw();

        // transform the `Option<NonNull<T>>` into just `*mut T`
        let data_ptr = data.map_or(null_mut(), |data| data.cast().as_ptr());
        let handles_ptr = handles.map_or(null_mut(), |handles| handles.cast().as_ptr());

        // SAFETY:
        // - Normally, we could be reading uninit bytes here. However, as long as fdf_channel_write
        //   doesn't allow cross-LTO then it won't care whether the bytes are initialized.
        // - The `Message` will generally only construct correctly if the data and handles pointers
        //   inside it are from the arena it holds, but just in case `fdf_channel_write` will check
        //   that we are using the correct arena so we do not need to re-verify that they are from
        //   the same arena.
        let res = Status::ok(unsafe {
            fdf_channel_write(
                self.handle.get_raw().get(),
                0,
                arena.as_ptr(),
                data_ptr,
                data_len,
                handles_ptr,
                handles_count,
            )
        });

        // SAFETY: this is the valid-by-contruction arena we were passed in through the [`Message`]
        // object, and now that we have completed `fdf_channel_write` it is safe to drop our copy
        // of it.
        unsafe { fdf_arena_drop_ref(arena.as_ptr()) };

        res
    }

    /// Shorthand for calling [`Self::write`] with the result of [`Message::new_with`]
    pub fn write_with<F>(&self, arena: Arena, f: F) -> Result<(), Status>
    where
        F: for<'a> FnOnce(
            &'a Arena,
        )
            -> (Option<ArenaBox<'a, T>>, Option<ArenaBox<'a, [Option<MixedHandle>]>>),
    {
        self.write(Message::new_with(arena, f))
    }

    /// Shorthand for calling [`Self::write`] with the result of [`Message::new_with`]
    pub fn write_with_data<F>(&self, arena: Arena, f: F) -> Result<(), Status>
    where
        F: for<'a> FnOnce(&'a Arena) -> ArenaBox<'a, T>,
    {
        self.write(Message::new_with_data(arena, f))
    }
}

/// Attempts to read from the channel, returning a [`Message`] object that can be used to
/// access or take the data received if there was any. This is the basic building block
/// on which the other `try_read_*` methods are built.
pub(crate) fn try_read_raw(
    channel: &DriverHandle,
) -> Result<Option<Message<[MaybeUninit<u8>]>>, Status> {
    let mut out_arena = null_mut();
    let mut out_data = null_mut();
    let mut out_num_bytes = 0;
    let mut out_handles = null_mut();
    let mut out_num_handles = 0;
    Status::ok(unsafe {
        fdf_channel_read(
            channel.get_raw().get(),
            0,
            &mut out_arena,
            &mut out_data,
            &mut out_num_bytes,
            &mut out_handles,
            &mut out_num_handles,
        )
    })?;
    // if no arena was returned, that means no data was returned.
    if out_arena.is_null() {
        return Ok(None);
    }
    // SAFETY: we just checked that the `out_arena` is non-null
    let arena = Arena(unsafe { NonNull::new_unchecked(out_arena) });
    let data_ptr = if !out_data.is_null() {
        let ptr = core::ptr::slice_from_raw_parts_mut(out_data.cast(), out_num_bytes as usize);
        // SAFETY: we just checked that the pointer was non-null, the slice version of it should
        // be too.
        Some(unsafe { ArenaBox::new(NonNull::new_unchecked(ptr)) })
    } else {
        None
    };
    let handles_ptr = if !out_handles.is_null() {
        let ptr = core::ptr::slice_from_raw_parts_mut(out_handles.cast(), out_num_handles as usize);
        // SAFETY: we just checked that the pointer was non-null, the slice version of it should
        // be too.
        Some(unsafe { ArenaBox::new(NonNull::new_unchecked(ptr)) })
    } else {
        None
    };
    Ok(Some(unsafe { Message::new_unchecked(arena, data_ptr, handles_ptr) }))
}

/// Reads a message from the channel asynchronously
///
/// # Panic
///
/// Panics if this is not run from a driver framework dispatcher.
///
/// # Safety
///
/// The caller is responsible for ensuring that the channel object's
/// handle lifetime is longer than the returned future.
pub(crate) unsafe fn read_raw<T: ?Sized, D>(
    channel: &mut Channel<T>,
    dispatcher: D,
) -> ReadMessageRawFut<D> {
    // SAFETY: The caller promises that the message state object can't outlive the handle.
    let raw_fut = unsafe { ReadMessageState::register_read_wait(channel) };
    ReadMessageRawFut { raw_fut, dispatcher }
}

impl<T> Channel<T> {
    /// Attempts to read an object of type `T` and a handle set from the channel
    pub fn try_read(&self) -> Result<Option<Message<T>>, Status> {
        // read a message from the channel
        let Some(message) = try_read_raw(&self.handle)? else {
            return Ok(None);
        };
        // SAFETY: It is an invariant of Channel<T> that messages sent or received are always of
        // type T.
        Ok(Some(unsafe { message.cast_unchecked() }))
    }

    /// Reads an object of type `T` and a handle set from the channel asynchronously
    pub async fn read<D: OnDispatcher + Unpin>(
        &mut self,
        dispatcher: D,
    ) -> Result<Option<Message<T>>, Status> {
        // SAFETY: By calling `read_raw` in an async context that holds this channel's lifetime open
        // beyond the resolution of the future, we ensure that the channel handle outlives the
        // future state object.
        let Some(message) = unsafe { read_raw(self, dispatcher) }.await? else {
            return Ok(None);
        };
        // SAFETY: It is an invariant of Channel<T> that messages sent or received are always of
        // type T.
        Ok(Some(unsafe { message.cast_unchecked() }))
    }
}

impl Channel<[u8]> {
    /// Attempts to read an object of type `T` and a handle set from the channel
    pub fn try_read_bytes(&self) -> Result<Option<Message<[u8]>>, Status> {
        // read a message from the channel
        let Some(message) = try_read_raw(&self.handle)? else {
            return Ok(None);
        };
        // SAFETY: It is an invariant of Channel<[u8]> that messages sent or received are always of
        // type [u8].
        Ok(Some(unsafe { message.assume_init() }))
    }

    /// Reads a slice of type `T` and a handle set from the channel asynchronously
    pub async fn read_bytes<D: OnDispatcher + Unpin>(
        &mut self,
        dispatcher: D,
    ) -> Result<Option<Message<[u8]>>, Status> {
        // read a message from the channel
        // SAFETY: By calling `read_raw` in an async context that holds this channel's lifetime open
        // beyond the resolution of the future, we ensure that the channel handle outlives the
        // future state object.
        let Some(message) = unsafe { read_raw(self, dispatcher) }.await? else {
            return Ok(None);
        };
        // SAFETY: It is an invariant of Channel<[u8]> that messages sent or received are always of
        // type [u8].
        Ok(Some(unsafe { message.assume_init() }))
    }
}

impl<T> From<Channel<T>> for MixedHandle {
    fn from(value: Channel<T>) -> Self {
        MixedHandle::from(value.into_driver_handle())
    }
}

impl<T: ?Sized> std::cmp::Ord for Channel<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.handle.cmp(&other.handle)
    }
}

impl<T: ?Sized> std::cmp::PartialOrd for Channel<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ?Sized> std::cmp::PartialEq for Channel<T> {
    fn eq(&self, other: &Self) -> bool {
        self.handle.eq(&other.handle)
    }
}

impl<T: ?Sized> std::cmp::Eq for Channel<T> {}

impl<T: ?Sized> std::hash::Hash for Channel<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.handle.hash(state);
    }
}

pub(crate) struct ReadMessageRawFut<D> {
    pub(crate) raw_fut: ReadMessageState,
    dispatcher: D,
}

impl<D: OnDispatcher + Unpin> Future for ReadMessageRawFut<D> {
    type Output = Result<Option<Message<[MaybeUninit<u8>]>>, Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let dispatcher = self.dispatcher.clone();
        self.as_mut().raw_fut.poll_with_dispatcher(cx, dispatcher)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Write, stdout};
    use std::pin::pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, mpsc};

    use fdf_core::dispatcher::{CurrentDispatcher, DispatcherBuilder};
    use fdf_core::handle::MixedHandleType;
    use fdf_env::test::spawn_in_driver;
    use futures::channel::oneshot;
    use futures::poll;
    use libasync_dispatcher::{AsAsyncDispatcherRef, AsyncDispatcher, OnDispatcher};

    use super::*;
    use crate::test_utils::*;

    #[test]
    fn send_and_receive_bytes_synchronously() {
        let (first, second) = Channel::create();
        let arena = Arena::new();
        assert_eq!(first.try_read_bytes().unwrap_err(), Status::SHOULD_WAIT);
        first.write_with_data(arena.clone(), |arena| arena.insert_slice(&[1, 2, 3, 4])).unwrap();
        assert_eq!(second.try_read_bytes().unwrap().unwrap().data().unwrap(), &[1, 2, 3, 4]);
        assert_eq!(second.try_read_bytes().unwrap_err(), Status::SHOULD_WAIT);
        second.write_with_data(arena.clone(), |arena| arena.insert_slice(&[5, 6, 7, 8])).unwrap();
        assert_eq!(first.try_read_bytes().unwrap().unwrap().data().unwrap(), &[5, 6, 7, 8]);
        assert_eq!(first.try_read_bytes().unwrap_err(), Status::SHOULD_WAIT);
        assert_eq!(second.try_read_bytes().unwrap_err(), Status::SHOULD_WAIT);
        drop(second);
        assert_eq!(
            first.write_with_data(arena.clone(), |arena| arena.insert_slice(&[9, 10, 11, 12])),
            Err(Status::PEER_CLOSED)
        );
    }

    #[test]
    fn send_and_receive_bytes_asynchronously() {
        spawn_in_driver("channel async", async {
            let arena = Arena::new();
            let (mut first, second) = Channel::create();

            assert!(poll!(pin!(first.read_bytes(CurrentDispatcher))).is_pending());
            second.write_with_data(arena, |arena| arena.insert_slice(&[1, 2, 3, 4])).unwrap();
            assert_eq!(
                first.read_bytes(CurrentDispatcher).await.unwrap().unwrap().data().unwrap(),
                &[1, 2, 3, 4]
            );
        });
    }

    #[test]
    fn send_and_receive_objects_synchronously() {
        let arena = Arena::new();
        let (first, second) = Channel::create();
        let (tx, rx) = mpsc::channel();
        first
            .write_with_data(arena.clone(), |arena| arena.insert(DropSender::new(1, tx.clone())))
            .unwrap();
        rx.try_recv().expect_err("should not drop the object when sent");
        let message = second.try_read().unwrap().unwrap();
        assert_eq!(message.data().unwrap().0, 1);
        rx.try_recv().expect_err("should not drop the object when received");
        drop(message);
        rx.try_recv().expect("dropped when received");
    }

    #[test]
    fn send_and_receive_handles_synchronously() {
        println!("Create channels and write one end of one of the channel pairs to the other");
        let (first, second) = Channel::<()>::create();
        let (inner_first, inner_second) = Channel::<String>::create();
        let message = Message::new_with(Arena::new(), |arena| {
            (None, Some(arena.insert_boxed_slice(Box::new([Some(inner_first.into())]))))
        });
        first.write(message).unwrap();

        println!("Receive the channel back on the other end of the first channel pair.");
        let mut arena = None;
        let message =
            second.try_read().unwrap().expect("Expected a message with contents to be received");
        let (_, received_handles) = message.into_arena_boxes(&mut arena);
        let mut first_handle_received =
            ArenaBox::take_boxed_slice(received_handles.expect("expected handles in the message"));
        let first_handle_received = first_handle_received
            .first_mut()
            .expect("expected one handle in the handle set")
            .take()
            .expect("expected the first handle to be non-null");
        let first_handle_received = first_handle_received.resolve();
        let MixedHandleType::Driver(driver_handle) = first_handle_received else {
            panic!("Got a non-driver handle when we sent a driver handle");
        };
        let inner_first_received = unsafe { Channel::from_driver_handle(driver_handle) };

        println!("Send and receive a string across the now-transmitted channel pair.");
        inner_first_received
            .write_with_data(Arena::new(), |arena| arena.insert("boom".to_string()))
            .unwrap();
        assert_eq!(inner_second.try_read().unwrap().unwrap().data().unwrap(), &"boom".to_string());
    }

    async fn ping(mut chan: Channel<u8>) {
        println!("starting ping!");
        chan.write_with_data(Arena::new(), |arena| arena.insert(0)).unwrap();
        while let Ok(Some(msg)) = chan.read(CurrentDispatcher).await {
            let next = *msg.data().unwrap();
            println!("ping! {next}");
            chan.write_with_data(msg.take_arena(), |arena| arena.insert(next + 1)).unwrap();
        }
    }

    async fn pong(mut chan: Channel<u8>) {
        println!("starting pong!");
        while let Some(msg) = chan.read(CurrentDispatcher).await.unwrap() {
            let next = *msg.data().unwrap();
            println!("pong! {next}");
            if next > 10 {
                println!("bye!");
                break;
            }
            chan.write_with_data(msg.take_arena(), |arena| arena.insert(next + 1)).unwrap();
        }
    }

    #[test]
    fn async_ping_pong() {
        spawn_in_driver("async ping pong", async {
            let (ping_chan, pong_chan) = Channel::create();
            CurrentDispatcher.spawn(ping(ping_chan));
            pong(pong_chan).await;
        });
    }

    #[test]
    fn async_ping_pong_on_fuchsia_async() {
        spawn_in_driver("async ping pong", async {
            let (ping_chan, pong_chan) = Channel::create();

            let fdf_dispatcher = DispatcherBuilder::new()
                .name("fdf-async")
                .create()
                .expect("failure creating non-blocking dispatcher for fdf operations on rust-async dispatcher")
                .release();

            let rust_async_dispatcher = DispatcherBuilder::new()
                .name("fuchsia-async")
                .allow_thread_blocking()
                .create()
                .expect("failure creating blocking dispatcher for rust async")
                .release();

            rust_async_dispatcher
                .post_task_sync(move |_| {
                    fdf_core::override_current_dispatcher(
                        fdf_dispatcher.as_dispatcher_ref(),
                        || {
                            let mut executor = fuchsia_async::LocalExecutor::default();
                            executor.run_singlethreaded(ping(ping_chan));
                        },
                    );
                })
                .unwrap();

            pong(pong_chan).await
        });
    }

    async fn recv_lots_of_bytes_with_cancellations(
        mut rx: Channel<[u8]>,
        fin_tx: oneshot::Sender<()>,
        pending_count: Arc<AtomicU64>,
    ) {
        let mut immediate_count = 0;
        let mut count = 0;
        loop {
            // try to read as fast as we can, but any time we get a pending drop the future
            // and then re-try with a proper await so we re-read and get it. This tests
            // the reliability of the channel read's drop cancellation.
            let mut next_fut = Box::pin(rx.read_bytes(CurrentDispatcher));
            let next = match futures::poll!(&mut next_fut) {
                Poll::Pending => {
                    pending_count.fetch_add(1, Ordering::Relaxed);
                    drop(next_fut);
                    rx.read_bytes(CurrentDispatcher).await
                }
                Poll::Ready(r) => {
                    immediate_count += 1;
                    r
                }
            };
            match next {
                Err(Status::PEER_CLOSED) | Ok(None) => break,
                Err(_) => {
                    next.unwrap();
                }
                Ok(Some(msg)) => {
                    assert_eq!(msg.data().unwrap(), &[count as u8; 100]);
                    count += 1;
                }
            }
        }
        println!("read total: {count}, immediate: {immediate_count}, pending: {pending_count:?}");
        // send the channel out as well so that the cancellation can finish
        fin_tx.send(()).unwrap();
    }

    async fn send_lots_of_bytes(
        tx: Channel<[u8]>,
        fin_rx: oneshot::Receiver<()>,
        pending_count: Arc<AtomicU64>,
    ) {
        // The potential failure modes here are not entirely deterministic, so we want to
        // make sure that we get enough runs through the danger path (a pending read that is
        // dropped) so that we exercise it thoroughly. To that end, we will do up to 10,000
        // writes but stop early if we have 500 pending events.
        let arena = Arena::new();
        print!("writing: ");
        for i in 0..10000 {
            tx.write_with_data(arena.clone(), |arena| arena.insert_slice(&[i as u8; 100])).unwrap();
            // the following print and flush is not just aesthetic. It helps slow down the
            // writes a bit so that the reader dispatcher is more likely to have to wait for
            // further data.
            print!(".");
            stdout().flush().unwrap();
            if pending_count.load(Ordering::Relaxed) > 500 {
                break;
            }
        }
        drop(tx);
        fin_rx.await.unwrap();
    }

    async fn send_and_recv_lots_of_bytes_with_cancellations(dispatcher: AsyncDispatcher) {
        let (tx, rx) = Channel::create();
        let (fin_tx, fin_rx) = oneshot::channel();
        let pending_count = Arc::new(AtomicU64::new(0));
        dispatcher.spawn(recv_lots_of_bytes_with_cancellations(rx, fin_tx, pending_count.clone()));

        send_lots_of_bytes(tx, fin_rx, pending_count).await;
    }

    #[test]
    fn send_and_recv_lots_of_bytes_with_cancellations_on_synchronized_dispatcher() {
        spawn_in_driver(
            "lots of bytes and with some cancellations on a synchronized dispatcher",
            async {
                let dispatcher =
                    DispatcherBuilder::new().name("fdf-synchronized").create().unwrap().release();

                send_and_recv_lots_of_bytes_with_cancellations(dispatcher.as_async_dispatcher())
                    .await;
            },
        );
    }

    #[test]
    fn send_and_recv_lots_of_bytes_with_cancellations_on_unsynchronized_dispatcher() {
        spawn_in_driver(
            "lots of bytes and with some cancellations on an unsynchronized dispatcher",
            async {
                let dispatcher = DispatcherBuilder::new()
                    .name("fdf-unsynchronized")
                    .unsynchronized()
                    .create()
                    .unwrap()
                    .release();

                send_and_recv_lots_of_bytes_with_cancellations(dispatcher.as_async_dispatcher())
                    .await;
            },
        );
    }

    #[test]
    fn send_and_recv_lots_of_bytes_with_cancellations_on_fuchsia_async_dispatcher() {
        spawn_in_driver(
            "lots of bytes and with some cancellations on a fuchsia-async overridden dispatcher",
            async {
                let fdf_dispatcher = DispatcherBuilder::new()
                    .name("fdf-async")
                    .create()
                    .expect("failure creating non-blocking dispatcher for fdf operations on rust-async dispatcher")
                    .release();

                let dispatcher = DispatcherBuilder::new()
                    .name("fdf-fuchsia-async")
                    .allow_thread_blocking()
                    .create()
                    .expect("failure creating blocking dispatcher for rust async")
                    .release();

                let (tx, rx) = Channel::create();
                let (fin_tx, fin_rx) = oneshot::channel();
                let pending_count = Arc::new(AtomicU64::new(0));

                let pending_count_clone = pending_count.clone();
                dispatcher
                    .post_task_sync(move |_| {
                        fdf_core::override_current_dispatcher(
                            fdf_dispatcher.as_dispatcher_ref(),
                            || {
                                let mut executor = fuchsia_async::LocalExecutor::default();
                                executor.run_singlethreaded(recv_lots_of_bytes_with_cancellations(
                                    rx,
                                    fin_tx,
                                    pending_count_clone,
                                ));
                            },
                        );
                    })
                    .unwrap();

                send_lots_of_bytes(tx, fin_rx, pending_count).await;
            },
        );
    }
}
