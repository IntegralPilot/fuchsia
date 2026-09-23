// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Internal helpers for implementing futures against channel objects

use std::mem::ManuallyDrop;
use std::task::Waker;
use zx::Status;

use crate::channel::{Channel, try_read_raw};
use crate::message::Message;
use fdf_core::dispatcher::DriverDispatcherRef;
use fdf_core::handle::DriverHandle;
use fdf_sys::*;
use libasync_dispatcher::OnDispatcher;

use core::mem::MaybeUninit;
use core::task::{Context, Poll};
use fuchsia_sync::Mutex;
use std::sync::Arc;

pub use fdf_sys::fdf_handle_t;

// state for a read message that is controlled by a lock
#[derive(Default, Debug)]
struct ReadMessageStateOpLocked {
    /// the currently active waker for this read operation. Only set if there
    /// is currently a pending read operation awaiting a callback.
    waker: Option<Waker>,
    /// if the channel was dropped while a pending callback was active, so the
    /// callback should close the driverhandle when it fires.
    channel_dropped: bool,
    /// whether cancelation of this future will happen asynchronously through
    /// the callback or immediately when [`fdf_channel_cancel_wait`] is called.
    /// This is used to decide what's responsible for freeing the reference
    /// to this object when the future is canceled.
    cancelation_is_async: bool,
}

/// This struct is shared between the future and the driver runtime, with the first field
/// being managed by the driver runtime and the second by the future. It will be held by two
/// [`Arc`]s, one for each of the future and the runtime.
///
/// The future's [`Arc`] will be dropped when the future is either fulfilled or cancelled through
/// normal [`Drop`] of the future.
///
/// The runtime's [`Arc`]'s dropping varies depending on whether the dispatcher it was registered on
/// was synchronized or not, and whether it was cancelled or not. The callback will only ever be
/// called *up to* one time.
///
/// If the dispatcher is synchronized, then the callback will *only* be called on fulfillment of the
/// read wait.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct ReadMessageStateOp {
    /// This must be at the start of the struct so that `ReadMessageStateOp` can be cast to and from `fdf_channel_read`.
    read_op: fdf_channel_read,
    state: Mutex<ReadMessageStateOpLocked>,
}

impl ReadMessageStateOp {
    unsafe extern "C" fn handler(
        _dispatcher: *mut fdf_dispatcher,
        read_op: *mut fdf_channel_read,
        _status: i32,
    ) {
        // Note: we don't really do anything different based on whether the callback
        // says canceled. If the future was canceled by being dropped, it won't poll
        // again since it was dropped.
        // The only unusual case is when the dispatcher is shutting down, and in that
        // case we will wake the future and it will try to read and get a more useful
        // error.
        // Meanwhile, since we use the same state object across multiple
        // futures due to needing to handle async cancelation, trying to track the
        // underlying reason for the cancelation becomes more tricky than it's worth.

        // SAFETY: When setting up the read op, we incremented the refcount of the `Arc` to allow
        // for this handler to reconstitute it.
        let op: Arc<Self> = unsafe { Arc::from_raw(read_op.cast()) };

        let mut state = op.state.lock();
        if state.channel_dropped {
            // SAFETY: since the channel dropped we are the only outstanding owner of the
            // channel object.
            unsafe { fdf_handle_close(op.read_op.channel) };
        }
        let Some(waker) = state.waker.take() else {
            // the waker was already taken, presumably because the future was dropped.
            return;
        };
        // make sure to drop the lock before calling the waker.
        drop(state);
        waker.wake()
    }

    /// Called by the channel on drop to indicate that the channel has been dropped and
    /// find out whether it needs to defer dropping the handle until the callback is called.
    pub fn set_channel_dropped(&self) -> bool {
        let mut state = self.state.lock();
        if state.waker.is_some() {
            state.channel_dropped = true;
            false
        } else {
            true
        }
    }
}

/// An object for managing the state of an async channel read message operation that can be used to
/// implement futures.
pub struct ReadMessageState {
    op: Arc<ReadMessageStateOp>,
    channel: ManuallyDrop<DriverHandle>,
}

impl ReadMessageState {
    /// Creates a new raw read message state that can be used to implement a [`Future`] that reads
    /// data from a channel and then converts it to the appropriate type. It also allows for
    /// different ways of storing and managing the dispatcher we wait on by deferring the
    /// dispatcher used to poll time. This state is registered with the given [`Channel`]
    /// so that dropping the channel will correctly free resources.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the handle inside `channel` outlives this
    /// object.
    pub unsafe fn register_read_wait<T: ?Sized>(channel: &mut Channel<T>) -> Self {
        // SAFETY: The caller is responsible for ensuring that the handle is a correct channel handle
        // and that the handle will outlive the created [`ReadMessageState`].
        let channel_handle = unsafe { channel.handle.get_raw() };
        let op = channel
            .wait_state
            .get_or_insert_with(|| {
                Arc::new(ReadMessageStateOp {
                    read_op: fdf_channel_read {
                        channel: channel_handle.get(),
                        handler: Some(ReadMessageStateOp::handler),
                        ..Default::default()
                    },
                    state: Mutex::new(ReadMessageStateOpLocked::default()),
                })
            })
            .clone();
        Self {
            op,
            // SAFETY: We know this is a valid driver handle by construction and we are
            // storing this handle in a [`ManuallyDrop`] to prevent it from being double-dropped.
            // The caller is responsible for ensuring that the handle outlives this object.
            channel: ManuallyDrop::new(unsafe { DriverHandle::new_unchecked(channel_handle) }),
        }
    }

    /// Polls this channel read operation against the given dispatcher.
    #[expect(clippy::type_complexity)]
    pub fn poll_with_dispatcher<D: OnDispatcher>(
        &mut self,
        cx: &mut Context<'_>,
        dispatcher: D,
    ) -> Poll<Result<Option<Message<[MaybeUninit<u8>]>>, Status>> {
        let mut state = self.op.state.lock();

        match try_read_raw(&self.channel) {
            Ok(res) => Poll::Ready(Ok(res)),
            Err(Status::SHOULD_WAIT) => {
                // if we haven't yet set a waker, that means we haven't started the wait operation
                // yet.
                if state.waker.is_none() {
                    // increment the reference count of the read op to account for the copy that will be given to
                    // `fdf_channel_wait_async`.
                    let op = Arc::into_raw(self.op.clone());
                    let res = dispatcher.on_maybe_dispatcher(|dispatcher| {
                        let dispatcher = DriverDispatcherRef::from_async_dispatcher(dispatcher);
                        // if we're not running on the same dispatcher as we're waiting from, we
                        // want to force async cancellation
                        let options = if !dispatcher.is_current_dispatcher() {
                            FDF_CHANNEL_WAIT_OPTION_FORCE_ASYNC_CANCEL
                        } else {
                            0
                        };
                        // SAFETY: the `ReadMessageStateOp` starts with an `fdf_channel_read` struct and
                        // has `repr(C)` layout, so is safe to be cast to the latter.
                        let res = Status::ok(unsafe {
                            fdf_channel_wait_async(
                                fdf_core::dispatcher_ptr(&dispatcher).as_ptr(),
                                op.cast_mut().cast(),
                                options,
                            )
                        });
                        if res.is_ok() {
                            // only replace the waker if we succeeded, so we'll try again next time
                            // otherwise.
                            state.waker.replace(cx.waker().clone());
                        } else {
                            // reconstitute the arc we made for the callback so it can be dropped
                            // since the async wait didn't succeed.
                            drop(unsafe { Arc::from_raw(op) });
                        }
                        // if the dispatcher we're waiting on is unsynchronized, the callback
                        // will drop the Arc and we need to indicate to our own Drop impl
                        // that it should not.
                        res.map(|_| {
                            options == FDF_CHANNEL_WAIT_OPTION_FORCE_ASYNC_CANCEL
                                || dispatcher.is_unsynchronized()
                        })
                    });

                    // the default state should be that `drop` will free the arc.
                    state.cancelation_is_async = false;
                    match res {
                        Err(Status::BAD_STATE) => {
                            return Poll::Pending; // a pending await is being cancelled
                        }
                        Ok(cancelation_is_async) => {
                            state.cancelation_is_async = cancelation_is_async;
                        }
                        Err(e) => return Poll::Ready(Err(e)),
                    }
                }
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl Drop for ReadMessageState {
    fn drop(&mut self) {
        let mut state = self.op.state.lock();
        if state.waker.is_none() {
            // if there's no waker either the callback has already fired or we never waited on this
            // future in the first place, so just leave it be.
            return;
        }

        // SAFETY: since we hold a lifetimed-reference to the channel object here, the channel must
        // be valid.
        let res = Status::ok(unsafe { fdf_channel_cancel_wait(self.channel.get_raw().get()) });
        match res {
            Ok(_) => {}
            Err(Status::NOT_FOUND) => {
                // the callback is already being called or the wait was already cancelled, so just
                // return and leave it.
                return;
            }
            Err(e) => panic!("Unexpected error {e:?} cancelling driver channel read wait"),
        }
        // SAFETY: if the channel was waited on by a synchronized dispatcher, and the cancel was
        // successful, the callback will not be called and we will have to free the `Arc` that the
        // callback would have consumed.
        if !state.cancelation_is_async {
            // steal the waker so it doesn't get called, if there is one.
            state.waker.take();
            unsafe { Arc::decrement_strong_count(Arc::as_ptr(&self.op)) };
        }
    }
}

#[cfg(test)]
mod test {
    use std::pin::pin;
    use std::sync::Weak;

    use fdf_core::dispatcher::CurrentDispatcher;
    use fdf_env::test::{spawn_in_driver, spawn_in_driver_etc};
    use libasync_dispatcher::OnDispatcher;

    use crate::arena::Arena;
    use crate::channel::{Channel, read_raw};

    use super::*;

    /// assert that the strong count of an arc is correct
    #[track_caller]
    fn assert_strong_count<T>(arc: &Weak<T>, count: usize) {
        assert_eq!(Weak::strong_count(arc), count, "unexpected strong count on arc");
    }

    /// create, poll, and then immediately drop a read future for a channel and verify
    /// that the internal op arc has the right refcount at all steps. Returns a copy
    /// of the op arc at the end so it can be verified that the count goes down
    /// to zero correctly.
    async fn read_and_drop<T: ?Sized + 'static, D: OnDispatcher + Unpin>(
        channel: &mut Channel<T>,
        dispatcher: D,
    ) -> Weak<ReadMessageStateOp> {
        let fut = unsafe { read_raw(channel, dispatcher) };
        let op_arc = Arc::downgrade(&fut.raw_fut.op);
        assert_strong_count(&op_arc, 2);
        let mut fut = pin!(fut);
        let Poll::Pending = futures::poll!(fut.as_mut()) else {
            panic!("expected pending state after polling channel read once");
        };
        assert_strong_count(&op_arc, 3);
        op_arc
    }

    #[test]
    fn early_cancel_future() {
        spawn_in_driver("early cancellation", async {
            let (mut a, b) = Channel::create();

            // create, poll, and then immediately drop a read future for channel `a`
            // so that it properly sets up the wait.
            read_and_drop(&mut a, CurrentDispatcher).await;
            b.write_with_data(Arena::new(), |arena| arena.insert(1)).unwrap();
            assert_eq!(a.read(CurrentDispatcher).await.unwrap().unwrap().data(), Some(&1));
        })
    }

    #[test]
    fn very_early_cancel_state_drops_correctly() {
        spawn_in_driver("early cancellation drop correctness", async {
            let (mut a, _b) = Channel::<[u8]>::create();

            // drop before even polling it should drop the arc correctly
            let fut = unsafe { read_raw(&mut a, CurrentDispatcher) };
            let op_arc = Arc::downgrade(&fut.raw_fut.op);
            assert_strong_count(&op_arc, 2);
            drop(fut);
            assert_strong_count(&op_arc, 1);
        })
    }

    #[test]
    fn synchronized_early_cancel_state_drops_correctly() {
        spawn_in_driver("early cancellation drop correctness", async {
            let (mut a, _b) = Channel::<[u8]>::create();

            assert_strong_count(&read_and_drop(&mut a, CurrentDispatcher).await, 1);
        });
    }

    #[test]
    fn unsynchronized_early_cancel_state_drops_correctly() {
        // the channel needs to outlive the dispatcher for this test because the channel shouldn't
        // be closed before the read wait has been cancelled.
        let (mut a, _b) = Channel::<[u8]>::create();
        let unsync_op =
            spawn_in_driver_etc("early cancellation drop correctness", false, true, async move {
                // We send the arc out to be checked after the dispatcher has shut down so
                // that we can be sure that the callback has had a chance to be called.
                // We send the channel back out so that it lives long enough for the
                // cancellation to be called on it.
                read_and_drop(&mut a, CurrentDispatcher).await
            });

        // check that there are no more owners of the inner op for the unsynchronized dispatcher.
        assert_strong_count(&unsync_op, 0);
    }

    #[test]
    fn unsynchronized_early_cancel_state_drops_repeatedly_correctly() {
        // the channel needs to outlive the dispatcher for this test because the channel shouldn't
        // be closed before the read wait has been cancelled.
        let (mut a, _b) = Channel::<[u8]>::create();
        spawn_in_driver_etc("early cancellation drop correctness", false, true, async move {
            for _ in 0..10000 {
                let mut fut = unsafe { read_raw(&mut a, CurrentDispatcher) };
                let Poll::Pending = futures::poll!(&mut fut) else {
                    panic!("expected pending state after polling channel read once");
                };
                drop(fut);
            }
        });
    }
}
