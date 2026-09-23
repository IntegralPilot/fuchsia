// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for using FIDL with the libasync C API
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};

use fidl_next::protocol::NonBlockingTransport;
use fidl_next::protocol::fuchsia::channel::Buffer;
use fidl_next::{CHUNK_SIZE, ClientEnd, Executor, HasExecutor, ServerEnd, Transport};
use futures::task::AtomicWaker;
use libasync::callback_state::CallbackSharedState;
use libasync::{JoinHandle, OnDispatcher};
use libasync_sys::{async_begin_wait, async_dispatcher, async_wait};
use zx::sys::{
    ZX_CHANNEL_PEER_CLOSED, ZX_CHANNEL_READABLE, ZX_ERR_BUFFER_TOO_SMALL, ZX_ERR_CANCELED,
    ZX_ERR_PEER_CLOSED, ZX_ERR_SHOULD_WAIT, ZX_OK, zx_channel_read, zx_channel_write,
    zx_packet_signal_t, zx_status_t,
};
use zx::{Channel, Status};

/// A fidl-compatible channel that uses a [`libasync`] dispatcher.
#[derive(Debug, PartialEq)]
pub struct AsyncChannel<D> {
    dispatcher: D,
    channel: Arc<Channel>,
}

impl<D> AsyncChannel<D> {
    /// Creates an async channel bound to the dispatcher `d` that can be used with fidl bindings.
    pub fn new_on_dispatcher(dispatcher: D, channel: Channel) -> Self {
        Self { dispatcher, channel: Arc::new(channel) }
    }

    /// A shortcut for creating a [`fidl_next`] compatible [`ClientEnd`] out of a
    /// [`Channel`] and dispatcher.
    pub fn client_from_zx_channel_on_dispatcher<P>(
        from: ClientEnd<P, Channel>,
        dispatcher: D,
    ) -> ClientEnd<P, Self> {
        let channel = from.into_untyped();
        ClientEnd::from_untyped(Self { dispatcher, channel: Arc::new(channel) })
    }

    /// A shortcut for creating a [`fidl_next`] compatible [`ServerEnd`] out of a
    /// [`Channel`] and dispatcher.
    pub fn server_from_zx_channel_on_dispatcher<P>(
        from: ServerEnd<P, Channel>,
        dispatcher: D,
    ) -> ServerEnd<P, Self> {
        let channel = from.into_untyped();
        ServerEnd::from_untyped(Self { dispatcher, channel: Arc::new(channel) })
    }
}

impl<D: Default> AsyncChannel<D> {
    /// Creates an async channel bound to the [`Default`] instance of dispatcher `D` that can
    /// be used with fidl bindings.
    pub fn new(channel: Channel) -> Self {
        Self::new_on_dispatcher(D::default(), channel)
    }

    /// A shortcut for creating a [`fidl_next`] compatible [`ClientEnd`] out of a
    /// [`Channel`].
    pub fn client_from_zx_channel<P>(from: ClientEnd<P, Channel>) -> ClientEnd<P, Self> {
        Self::client_from_zx_channel_on_dispatcher(from, D::default())
    }

    /// A shortcut for creating a [`fidl_next`] compatible [`ServerEnd`] out of a
    /// [`Channel`].
    pub fn server_from_zx_channel<P>(from: ServerEnd<P, Channel>) -> ServerEnd<P, Self> {
        Self::server_from_zx_channel_on_dispatcher(from, D::default())
    }
}

impl<D: OnDispatcher> Transport for AsyncChannel<D> {
    type Error = Status;
    type Shared = Arc<Channel>;
    type Exclusive = Exclusive<D>;
    type SendBuffer = Buffer;
    type SendFutureState = SendFutureState;
    type RecvFutureState = RecvFutureState;
    type RecvBuffer = Buffer;

    fn split(self) -> (Self::Shared, Self::Exclusive) {
        let channel = self.channel;
        let object = channel.raw_handle();
        (
            channel.clone(),
            Exclusive {
                dispatcher: self.dispatcher,
                callback_state: CallbackState::new(
                    async_wait {
                        handler: Some(RecvCallbackState::handler),
                        object,
                        trigger: ZX_CHANNEL_PEER_CLOSED | ZX_CHANNEL_READABLE,
                        ..Default::default()
                    },
                    RecvCallbackState {
                        _channel: channel,
                        canceled: AtomicBool::new(false),
                        waker: AtomicWaker::new(),
                    },
                ),
            },
        )
    }

    fn acquire(_shared: &Self::Shared) -> Self::SendBuffer {
        Buffer::new()
    }

    fn begin_send(_: &Self::Shared, buffer: Self::SendBuffer) -> Self::SendFutureState {
        SendFutureState { buffer }
    }

    fn poll_send(
        future_state: Pin<&mut Self::SendFutureState>,
        _: &mut Context<'_>,
        shared: &Self::Shared,
    ) -> Poll<Result<(), Option<Self::Error>>> {
        Poll::Ready(Self::send_immediately(future_state.get_mut(), shared))
    }

    fn begin_recv(
        _shared: &Self::Shared,
        exclusive: &mut Self::Exclusive,
    ) -> Self::RecvFutureState {
        RecvFutureState {
            buffer: Some(Buffer::new()),
            callback_state: Arc::downgrade(&exclusive.callback_state),
        }
    }

    fn poll_recv(
        mut future_state: Pin<&mut Self::RecvFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
        exclusive: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>> {
        let buffer = future_state.buffer.as_mut().unwrap();

        let mut actual_bytes = 0;
        let mut actual_handles = 0;

        loop {
            let result = unsafe {
                zx_channel_read(
                    shared.raw_handle(),
                    0,
                    buffer.chunks.as_mut_ptr().cast(),
                    buffer.handles.as_mut_ptr().cast(),
                    (buffer.chunks.capacity() * CHUNK_SIZE) as u32,
                    buffer.handles.capacity() as u32,
                    &mut actual_bytes,
                    &mut actual_handles,
                )
            };

            match result {
                ZX_OK => {
                    unsafe {
                        buffer.chunks.set_len(actual_bytes as usize / CHUNK_SIZE);
                        buffer.handles.set_len(actual_handles as usize);
                    }
                    return Poll::Ready(Ok(future_state.buffer.take().unwrap()));
                }
                ZX_ERR_PEER_CLOSED => return Poll::Ready(Err(None)),
                ZX_ERR_BUFFER_TOO_SMALL => {
                    let min_chunks = (actual_bytes as usize).div_ceil(CHUNK_SIZE);
                    buffer.chunks.reserve(min_chunks - buffer.chunks.capacity());
                    buffer.handles.reserve(actual_handles as usize - buffer.handles.capacity());
                }
                ZX_ERR_SHOULD_WAIT => {
                    exclusive.wait_readable(cx)?;
                    return Poll::Pending;
                }
                raw => return Poll::Ready(Err(Some(Status::err_from_raw(raw)))),
            }
        }
    }
}

impl<D: OnDispatcher> NonBlockingTransport for AsyncChannel<D> {
    fn send_immediately(
        future_state: &mut Self::SendFutureState,
        shared: &Self::Shared,
    ) -> Result<(), Option<Self::Error>> {
        let result = unsafe {
            zx_channel_write(
                shared.raw_handle(),
                0,
                future_state.buffer.chunks.as_ptr().cast::<u8>(),
                (future_state.buffer.chunks.len() * CHUNK_SIZE) as u32,
                future_state.buffer.handles.as_ptr().cast(),
                future_state.buffer.handles.len() as u32,
            )
        };

        match result {
            ZX_OK => {
                // Handles were written to the channel, so we must not drop them.
                unsafe {
                    future_state.buffer.handles.set_len(0);
                }
                Ok(())
            }
            ZX_ERR_PEER_CLOSED => Err(None),
            _ => Err(Some(Status::err_from_raw(result))),
        }
    }
}

/// A wrapper around a dispatcher reference object that can be used with the [`fidl_next`] bindings
/// to spawn client and server dispatchers on a driver runtime provided async dispatcher.
pub struct FidlExecutor<D>(D);

impl<D> std::ops::Deref for FidlExecutor<D> {
    type Target = D;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<D> From<D> for FidlExecutor<D> {
    fn from(value: D) -> Self {
        FidlExecutor(value)
    }
}

impl<D: OnDispatcher + 'static> Executor for FidlExecutor<D> {
    type JoinHandle<T: 'static> = JoinHandle<T>;

    fn spawn<F>(&self, future: F) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.0.compute(future).detach_on_drop()
    }
}

impl<D: OnDispatcher> fidl_next::RunsTransport<AsyncChannel<D>> for FidlExecutor<D> {}

impl<D: OnDispatcher + 'static> HasExecutor for AsyncChannel<D> {
    type Executor = FidlExecutor<D>;

    fn executor(&self) -> Self::Executor {
        FidlExecutor(self.dispatcher.clone())
    }
}

type CallbackState = CallbackSharedState<async_wait, RecvCallbackState>;

#[doc(hidden)] // Internal implementation detail of fidl_next api
pub struct Exclusive<D> {
    callback_state: Arc<CallbackState>,
    dispatcher: D,
}

impl<D: OnDispatcher> Exclusive<D> {
    fn wait_readable(&mut self, cx: &Context<'_>) -> Result<(), Status> {
        self.callback_state.waker.register(cx.waker());
        if self.callback_state.canceled.load(Ordering::Relaxed) {
            // the dispatcher has shut down so we can't wait again
            return Err(Status::CANCELED);
        }

        if Arc::strong_count(&self.callback_state) > 1 {
            // the callback is holding a strong reference to this so we're already waiting
            // (or maybe in the process of cancelling) for a callback, so just return.
            return Ok(());
        }
        self.dispatcher.on_maybe_dispatcher(|dispatcher| {
            let callback_state_ptr = CallbackState::make_raw_ptr(self.callback_state.clone());
            // SAFETY: fill this in
            Status::ok(unsafe { async_begin_wait(dispatcher.inner().as_ptr(), callback_state_ptr) })
                .inspect_err(|_| {
                    // SAFETY: The wait failed so we have an outstanding reference to the callback
                    // state that needs to be freed since the callback will not be called.
                    unsafe { CallbackState::release_raw_ptr(callback_state_ptr) };
                })
        })
    }
}

/// State shared between the callback and the future.
struct RecvCallbackState {
    _channel: Arc<Channel>,
    canceled: AtomicBool,
    waker: AtomicWaker,
}

impl RecvCallbackState {
    unsafe extern "C" fn handler(
        _dispatcher: *mut async_dispatcher,
        callback_state_ptr: *mut async_wait,
        status: zx_status_t,
        _packet: *const zx_packet_signal_t,
    ) {
        debug_assert!(
            status == ZX_OK || status == ZX_ERR_CANCELED,
            "task callback called with status other than ok or canceled"
        );
        // SAFETY: This callback's copy of the `async_task` object was refcounted for when we
        // started the wait.
        let state = unsafe { CallbackState::from_raw_ptr(callback_state_ptr) };
        if status == ZX_ERR_CANCELED {
            state.canceled.store(true, Ordering::Relaxed);
        }
        state.waker.wake();
    }
}

/// The state for a channel recv future.
pub struct RecvFutureState {
    buffer: Option<Buffer>,
    callback_state: Weak<CallbackState>,
}

impl Drop for RecvFutureState {
    fn drop(&mut self) {
        let Some(state) = self.callback_state.upgrade() else { return };
        // todo: properly implement cancelation
        state.waker.wake();
    }
}

/// The state for a channel send future.
pub struct SendFutureState {
    buffer: Buffer,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf::CurrentDispatcher;
    use fdf_env::test::spawn_in_driver;
    use fidl_next::{ClientDispatcher, ClientEnd, IgnoreEvents};
    use fidl_next_fuchsia_examples_gizmo::Device;

    #[fuchsia::test]
    async fn wait_pending_at_dispatcher_shutdown() {
        spawn_in_driver("driver fidl server", async {
            let (_server_chan, client_chan) = Channel::create();
            let client_end: ClientEnd<Device, _> = ClientEnd::<Device, _>::from_untyped(
                AsyncChannel::new_on_dispatcher(CurrentDispatcher, client_chan),
            );
            let client_dispatcher = ClientDispatcher::new(client_end);
            let _client = client_dispatcher.client();
            CurrentDispatcher.spawn(async {
                println!(
                    "client task finished: {:?}",
                    client_dispatcher.run(IgnoreEvents).await.map(|_| ())
                );
            });
            (_server_chan, _client)
        });
    }
}
