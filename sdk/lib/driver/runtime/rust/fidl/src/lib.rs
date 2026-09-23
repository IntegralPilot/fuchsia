// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for using FIDL with the fuchsia driver framework C API
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

pub mod wire;

use fuchsia_sync::{Mutex, MutexGuard};
use std::marker::PhantomData;
use std::num::NonZero;
use std::pin::Pin;
use std::ptr::NonNull;
use std::slice;
use std::task::{Context, Poll};

use fidl_next::protocol::NonBlockingTransport;
use fidl_next::{AsDecoder, Chunk, HasExecutor};
use zx::Status;

use fdf_channel::arena::{Arena, ArenaBox};
use fdf_channel::channel::Channel;
use fdf_channel::futures::ReadMessageState;
use fdf_channel::message::Message;
use fdf_core::dispatcher::CurrentDispatcher;
use fdf_core::handle::{DriverHandle, MixedHandle, MixedHandleType};
use libasync_dispatcher::OnDispatcher;

/// A wrapper around a dispatcher reference object that can be used with the [`fidl_next`] bindings
/// to spawn client and server dispatchers on a driver runtime provided async dispatcher.
pub type FidlExecutor<D = CurrentDispatcher> = libasync_fidl::FidlExecutor<D>;

/// A fidl-compatible driver channel that also holds a reference to the
/// dispatcher. Defaults to using [`CurrentDispatcher`].
#[derive(Debug, PartialEq)]
pub struct DriverChannel<D = CurrentDispatcher> {
    dispatcher: D,
    channel: Channel<[Chunk]>,
}

impl<D> DriverChannel<D> {
    /// Create a new driver fidl channel that will perform its operations on the given
    /// dispatcher handle.
    pub fn new_with_dispatcher(dispatcher: D, channel: Channel<[Chunk]>) -> Self {
        Self { dispatcher, channel }
    }

    /// Create a new driver fidl channel pair that will perform its operations on the given
    /// dispatcher handles.
    pub fn create_with_dispatchers(dispatcher1: D, dispatcher2: D) -> (Self, Self) {
        let (channel1, channel2) = Channel::create();
        (
            Self { dispatcher: dispatcher1, channel: channel1 },
            Self { dispatcher: dispatcher2, channel: channel2 },
        )
    }

    /// Create a new driver fidl channel pair that will perform its operations on the given
    /// dispatcher handle, if the dispatcher implements [`Clone`]
    pub fn create_with_dispatcher(dispatcher: D) -> (Self, Self)
    where
        D: Clone,
    {
        Self::create_with_dispatchers(dispatcher.clone(), dispatcher)
    }

    /// Does a server side token exchange from a [`zx::Channel`]'s handle to obtain
    /// a driver runtime [`DriverChannel`] synchronously.
    pub fn receive_from_token_with_dispatcher(
        dispatcher: D,
        token: zx::Channel,
    ) -> Result<DriverChannel<D>, Status> {
        let mut handle = 0;
        Status::ok(unsafe { fdf_sys::fdf_token_receive(token.into_raw(), &mut handle) })?;
        let handle = NonZero::new(handle).ok_or(Status::BAD_HANDLE)?;
        let channel = unsafe { Channel::from_driver_handle(DriverHandle::new_unchecked(handle)) };
        Ok(DriverChannel::new_with_dispatcher(dispatcher, channel))
    }

    /// Returns the underlying data channel
    pub fn into_channel(self) -> Channel<[Chunk]> {
        self.channel
    }

    /// Returns the underlying `fdf_handle_t` for this channel
    pub fn into_driver_handle(self) -> DriverHandle {
        self.channel.into_driver_handle()
    }
}

impl DriverChannel<CurrentDispatcher> {
    /// Create a new driver fidl channel that will perform its operations on the
    /// [`CurrentDispatcher`].
    pub fn new(channel: Channel<[Chunk]>) -> Self {
        Self::new_with_dispatcher(CurrentDispatcher, channel)
    }

    /// Create a new driver fidl channel pair that will perform its operations on the
    /// [`CurrentDispatcher`].
    pub fn create() -> (Self, Self) {
        Self::create_with_dispatcher(CurrentDispatcher)
    }

    /// Does a server side token exchange from a [`zx::Channel`]'s handle to obtain
    /// a driver runtime [`DriverChannel`] synchronously.
    pub fn receive_from_token(token: zx::Channel) -> Result<DriverChannel, Status> {
        Self::receive_from_token_with_dispatcher(CurrentDispatcher, token)
    }
}

impl fidl_next::InstanceFromServiceTransport<zx::Channel> for DriverChannel<CurrentDispatcher> {
    fn from_service_transport(handle: zx::Channel) -> Self {
        DriverChannel::receive_from_token(handle).unwrap()
    }
}

/// Creates a pair of [`fidl_next::ClientEnd`] and [`fidl_next::ServerEnd`] backed by a new
/// pair of [`DriverChannel`]s using dispatchers of type `D`.
pub fn create_channel_with_dispatchers<P, D>(
    client_dispatcher: D,
    server_dispatcher: D,
) -> (fidl_next::ClientEnd<P, DriverChannel<D>>, fidl_next::ServerEnd<P, DriverChannel<D>>) {
    let (client_channel, server_channel) =
        DriverChannel::create_with_dispatchers(client_dispatcher, server_dispatcher);
    (
        fidl_next::ClientEnd::from_untyped(client_channel),
        fidl_next::ServerEnd::from_untyped(server_channel),
    )
}

/// Creates a pair of [`fidl_next::ClientEnd`] and [`fidl_next::ServerEnd`] backed by a new
/// pair of [`DriverChannel`]s using dispatchers of type `D`, where `D` implements [`Clone`]
pub fn create_channel_with_dispatcher<P, D: Clone>(
    dispatcher: D,
) -> (fidl_next::ClientEnd<P, DriverChannel<D>>, fidl_next::ServerEnd<P, DriverChannel<D>>) {
    create_channel_with_dispatchers(dispatcher.clone(), dispatcher)
}

/// Creates a pair of [`fidl_next::ClientEnd`] and [`fidl_next::ServerEnd`] backed by a new
/// pair of [`DriverChannel`]s using the default [`CurrentDispatcher`]
pub fn create_channel<P>()
-> (fidl_next::ClientEnd<P, DriverChannel>, fidl_next::ServerEnd<P, DriverChannel>) {
    create_channel_with_dispatcher(CurrentDispatcher)
}

/// A channel buffer.
#[derive(Default)]
pub struct SendBuffer {
    handles: Vec<Option<MixedHandle>>,
    data: Vec<Chunk>,
}

impl SendBuffer {
    fn new() -> Self {
        Self { handles: Vec::new(), data: Vec::new() }
    }
}

impl fidl_next::Encoder for SendBuffer {
    #[inline]
    fn bytes_written(&self) -> usize {
        fidl_next::Encoder::bytes_written(&self.data)
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        fidl_next::Encoder::write(&mut self.data, bytes)
    }

    #[inline]
    fn rewrite(&mut self, pos: usize, bytes: &[u8]) {
        fidl_next::Encoder::rewrite(&mut self.data, pos, bytes)
    }

    fn write_zeroes(&mut self, len: usize) {
        fidl_next::Encoder::write_zeroes(&mut self.data, len);
    }
}

impl fidl_next::encoder::InternalHandleEncoder for SendBuffer {
    #[inline]
    fn __internal_handle_count(&self) -> usize {
        self.handles.len()
    }
}

impl fidl_next::fuchsia::HandleEncoder for SendBuffer {
    fn push_handle(&mut self, handle: zx::NullableHandle) -> Result<(), fidl_next::EncodeError> {
        if let Some(handle) = MixedHandle::from_zircon_handle(handle) {
            if handle.is_driver() {
                return Err(fidl_next::EncodeError::ExpectedZirconHandle);
            }
            self.handles.push(Some(handle));
        } else {
            self.handles.push(None);
        }
        Ok(())
    }

    unsafe fn push_raw_driver_handle(&mut self, handle: u32) -> Result<(), fidl_next::EncodeError> {
        if let Some(handle) = NonZero::new(handle) {
            // SAFETY: the fidl framework is responsible for providing us with a valid, otherwise
            // unowned handle.
            let handle = unsafe { MixedHandle::from_raw(handle) };
            if !handle.is_driver() {
                return Err(fidl_next::EncodeError::ExpectedDriverHandle);
            }
            self.handles.push(Some(handle));
        } else {
            self.handles.push(None);
        }
        Ok(())
    }

    fn handles_pushed(&self) -> usize {
        self.handles.len()
    }
}

#[doc(hidden)] // Internal implementation detail of the fidl bindings.
pub struct RecvBuffer {
    message: Option<Message<[Chunk]>>,
}

unsafe impl<'de> AsDecoder<'de> for RecvBuffer {
    type Decoder = RecvBufferDecoder<'de>;

    fn as_decoder(&'de mut self) -> Self::Decoder {
        RecvBufferDecoder { buffer: self, data_offset: 0, handle_offset: 0 }
    }
}

#[doc(hidden)] // Internal implementation detail of the fidl bindings.
pub struct RecvBufferDecoder<'de> {
    buffer: &'de mut RecvBuffer,
    data_offset: usize,
    handle_offset: usize,
}

impl RecvBufferDecoder<'_> {
    fn next_handle(&self) -> Result<&MixedHandle, fidl_next::DecodeError> {
        let Some(message) = &self.buffer.message else {
            return Err(fidl_next::DecodeError::InsufficientHandles);
        };

        let Some(handles) = message.handles() else {
            return Err(fidl_next::DecodeError::InsufficientHandles);
        };
        if handles.len() < self.handle_offset + 1 {
            return Err(fidl_next::DecodeError::InsufficientHandles);
        }
        handles[self.handle_offset].as_ref().ok_or(fidl_next::DecodeError::RequiredHandleAbsent)
    }
}

impl<'de> fidl_next::Decoder<'de> for RecvBufferDecoder<'de> {
    fn take_chunks(&mut self, count: usize) -> Result<&'de mut [Chunk], fidl_next::DecodeError> {
        let Some(message) = &mut self.buffer.message else {
            return Err(fidl_next::DecodeError::InsufficientData);
        };

        let Some(data) = message.data_mut() else {
            return Err(fidl_next::DecodeError::InsufficientData);
        };
        if data.len() < self.data_offset + count {
            return Err(fidl_next::DecodeError::InsufficientData);
        }
        let pos = self.data_offset;
        self.data_offset += count;

        let ptr = data.as_mut_ptr();
        Ok(unsafe { slice::from_raw_parts_mut(ptr.add(pos), count) })
    }

    fn commit(&mut self) {
        if let Some(handles) = self.buffer.message.as_mut().and_then(Message::handles_mut) {
            for handle in handles.iter_mut().take(self.handle_offset) {
                core::mem::forget(handle.take());
            }
        }
    }

    fn finish(&self) -> Result<(), fidl_next::DecodeError> {
        if let Some(message) = &self.buffer.message {
            let data_len = message.data().unwrap_or(&[]).len();
            if self.data_offset != data_len {
                return Err(fidl_next::DecodeError::ExtraBytes {
                    num_extra: data_len - self.data_offset,
                });
            }
            let handle_len = message.handles().unwrap_or(&[]).len();
            if self.handle_offset != handle_len {
                return Err(fidl_next::DecodeError::ExtraHandles {
                    num_extra: handle_len - self.handle_offset,
                });
            }
        }

        Ok(())
    }
}

impl fidl_next::decoder::InternalHandleDecoder for RecvBufferDecoder<'_> {
    fn __internal_take_handles(&mut self, count: usize) -> Result<(), fidl_next::DecodeError> {
        let Some(handles) = self.buffer.message.as_mut().and_then(Message::handles_mut) else {
            return Err(fidl_next::DecodeError::InsufficientHandles);
        };
        if handles.len() < self.handle_offset + count {
            return Err(fidl_next::DecodeError::InsufficientHandles);
        }
        let pos = self.handle_offset;
        self.handle_offset = pos + count;
        Ok(())
    }

    fn __internal_handles_remaining(&self) -> usize {
        self.buffer
            .message
            .as_ref()
            .map(|buffer| buffer.handles().unwrap_or(&[]).len() - self.handle_offset)
            .unwrap_or(0)
    }
}

impl fidl_next::fuchsia::HandleDecoder for RecvBufferDecoder<'_> {
    fn take_raw_handle(&mut self) -> Result<zx::sys::zx_handle_t, fidl_next::DecodeError> {
        let result = {
            let handle = self.next_handle()?.resolve_ref();
            let MixedHandleType::Zircon(handle) = handle else {
                return Err(fidl_next::DecodeError::ExpectedZirconHandle);
            };
            handle.raw_handle()
        };
        let pos = self.handle_offset;
        self.handle_offset = pos + 1;
        Ok(result)
    }

    fn take_raw_driver_handle(&mut self) -> Result<u32, fidl_next::DecodeError> {
        let result = {
            let handle = self.next_handle()?.resolve_ref();
            let MixedHandleType::Driver(handle) = handle else {
                return Err(fidl_next::DecodeError::ExpectedDriverHandle);
            };
            unsafe { handle.get_raw().get() }
        };
        let pos = self.handle_offset;
        self.handle_offset = pos + 1;
        Ok(result)
    }

    fn handles_remaining(&mut self) -> usize {
        fidl_next::decoder::InternalHandleDecoder::__internal_handles_remaining(self)
    }
}

/// The inner state of a receive future used by [`fidl_next::protocol::Transport`].
pub struct DriverRecvState(ReadMessageState);

/// The shared part of a driver channel.
pub struct Shared<D> {
    channel: Mutex<DriverChannel<D>>,
}

impl<D> Shared<D> {
    fn new(channel: Mutex<DriverChannel<D>>) -> Self {
        Self { channel }
    }

    fn get_locked(&self) -> MutexGuard<'_, DriverChannel<D>> {
        self.channel.lock()
    }
}

/// The exclusive part of a driver channel.
pub struct Exclusive {
    _phantom: PhantomData<()>,
}

impl<D: OnDispatcher> fidl_next::Transport for DriverChannel<D> {
    type Error = Status;

    fn split(self) -> (Self::Shared, Self::Exclusive) {
        (Shared::new(Mutex::new(self)), Exclusive { _phantom: PhantomData })
    }

    type Shared = Shared<D>;

    type SendBuffer = SendBuffer;

    type SendFutureState = SendBuffer;

    fn acquire(_shared: &Self::Shared) -> Self::SendBuffer {
        SendBuffer::new()
    }

    type Exclusive = Exclusive;

    type RecvFutureState = DriverRecvState;

    type RecvBuffer = RecvBuffer;

    fn begin_send(_shared: &Self::Shared, buffer: Self::SendBuffer) -> Self::SendFutureState {
        buffer
    }

    fn poll_send(
        mut buffer: Pin<&mut Self::SendFutureState>,
        _cx: &mut Context<'_>,
        shared: &Self::Shared,
    ) -> Poll<Result<(), Option<Self::Error>>> {
        Poll::Ready(Self::send_immediately(&mut *buffer, shared))
    }

    fn begin_recv(
        shared: &Self::Shared,
        _exclusive: &mut Self::Exclusive,
    ) -> Self::RecvFutureState {
        // SAFETY: The `receiver` owns the channel we're using here and will be the same
        // receiver given to `poll_recv`, so must outlive the state object we're constructing.
        let state =
            unsafe { ReadMessageState::register_read_wait(&mut shared.get_locked().channel) };
        DriverRecvState(state)
    }

    fn poll_recv(
        mut future: Pin<&mut Self::RecvFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
        _exclusive: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>> {
        use std::task::Poll::*;
        match future.as_mut().0.poll_with_dispatcher(cx, shared.get_locked().dispatcher.clone()) {
            Ready(Ok(maybe_buffer)) => {
                let buffer = maybe_buffer.map(|buffer| {
                    buffer.map_data(|_, data| {
                        let bytes = data.len();
                        assert_eq!(
                            0,
                            bytes % size_of::<Chunk>(),
                            "Received driver channel buffer was not a multiple of {} bytes",
                            size_of::<Chunk>()
                        );
                        // SAFETY: we verified that the size of the message we received was the correct
                        // multiple of chunks and we know that the data pointer is otherwise valid and
                        // from the correct arena by construction.
                        unsafe {
                            let ptr = ArenaBox::into_ptr(data).cast();
                            ArenaBox::new(NonNull::slice_from_raw_parts(
                                ptr,
                                bytes / size_of::<Chunk>(),
                            ))
                        }
                    })
                });

                Ready(Ok(RecvBuffer { message: buffer }))
            }
            Ready(Err(err)) => {
                if err == Status::PEER_CLOSED {
                    Ready(Err(None))
                } else {
                    Ready(Err(Some(err)))
                }
            }
            Pending => Pending,
        }
    }
}

impl<D: OnDispatcher> fidl_next::protocol::NonBlockingTransport for DriverChannel<D> {
    fn send_immediately(
        future_state: &mut Self::SendFutureState,
        shared: &Self::Shared,
    ) -> Result<(), Option<Self::Error>> {
        let arena = Arena::new();
        let message = Message::new_with(arena, |arena| {
            let data = arena.insert_slice(&future_state.data);
            let handles = future_state.handles.split_off(0);
            let handles = arena.insert_from_iter(handles);
            (Some(data), Some(handles))
        });
        match shared.get_locked().channel.write(message) {
            Ok(()) => Ok(()),
            Err(Status::PEER_CLOSED) => Err(None),
            Err(e) => Err(Some(e)),
        }
    }
}

impl<D> fidl_next::RunsTransport<DriverChannel<D>> for fidl_next::fuchsia_async::FuchsiaAsync {}
impl<D: OnDispatcher> fidl_next::RunsTransport<DriverChannel<D>> for FidlExecutor<D> {}

impl<D: OnDispatcher + 'static> HasExecutor for DriverChannel<D> {
    type Executor = FidlExecutor<D>;

    fn executor(&self) -> Self::Executor {
        FidlExecutor::from(self.dispatcher.clone())
    }
}
