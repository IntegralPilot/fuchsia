// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A transport implementation which uses Zircon channels.

use core::marker::PhantomData;
use core::mem::replace;
use core::pin::Pin;
use core::slice;
use core::task::{Context, Poll};

use fidl_next_codec::decoder::InternalHandleDecoder;
use fidl_next_codec::encoder::InternalHandleEncoder;
use fidl_next_codec::fuchsia::{HandleDecoder, HandleEncoder};
use fidl_next_codec::{AsDecoder, CHUNK_SIZE, Chunk, DecodeError, Decoder, EncodeError, Encoder};
use fuchsia_async::{RWHandle, ReadableHandle as _};
use zx::sys::{
    ZX_ERR_BUFFER_TOO_SMALL, ZX_ERR_PEER_CLOSED, ZX_ERR_SHOULD_WAIT, ZX_OK, zx_channel_read,
    zx_channel_write, zx_handle_t,
};
use zx::{Channel, NullableHandle, Status};

use crate::{NonBlockingTransport, Transport};

/// The shared part of a channel.
pub struct Shared {
    channel: RWHandle<Channel>,
    // TODO: recycle send/recv buffers to reduce allocations
}

impl Shared {
    fn new(channel: Channel) -> Self {
        Self { channel: RWHandle::new(channel) }
    }
}

/// A channel buffer that contains handles and chunks.
#[derive(Default)]
pub struct Buffer {
    /// The chunks of the buffer.
    pub chunks: Vec<Chunk>,
    /// The handles of the buffer.
    pub handles: Vec<NullableHandle>,
}

impl Buffer {
    /// New buffer.
    pub fn new() -> Self {
        Self::default()
    }
}

impl InternalHandleEncoder for Buffer {
    #[inline]
    fn __internal_handle_count(&self) -> usize {
        self.handles.len()
    }
}

impl Encoder for Buffer {
    #[inline]
    fn bytes_written(&self) -> usize {
        Encoder::bytes_written(&self.chunks)
    }

    #[inline]
    fn write_zeroes(&mut self, len: usize) {
        Encoder::write_zeroes(&mut self.chunks, len)
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        Encoder::write(&mut self.chunks, bytes)
    }

    #[inline]
    fn rewrite(&mut self, pos: usize, bytes: &[u8]) {
        Encoder::rewrite(&mut self.chunks, pos, bytes)
    }
}

impl HandleEncoder for Buffer {
    fn push_handle(&mut self, handle: NullableHandle) -> Result<(), EncodeError> {
        self.handles.push(handle);
        Ok(())
    }

    fn handles_pushed(&self) -> usize {
        self.handles.len()
    }
}

// SAFETY: Moving a `Vec` does not invalidate any references to its elements.
// The chunks returned from `take_chunks` are located on the heap.
unsafe impl<'de> AsDecoder<'de> for Buffer {
    type Decoder = BufferDecoder<'de>;

    fn as_decoder(&'de mut self) -> Self::Decoder {
        BufferDecoder { buffer: self, chunks_taken: 0, handles_taken: 0 }
    }
}

/// The state for a channel send future.
pub struct SendFutureState {
    buffer: Buffer,
}

/// The exclusive part of a channel.
pub struct Exclusive {
    _phantom: PhantomData<()>,
}

/// The state for a channel receive future.
pub struct RecvFutureState {
    buffer: Option<Buffer>,
}

/// A decoder for a [`Buffer`].
pub struct BufferDecoder<'de> {
    buffer: &'de mut Buffer,
    chunks_taken: usize,
    handles_taken: usize,
}

impl InternalHandleDecoder for BufferDecoder<'_> {
    fn __internal_take_handles(&mut self, count: usize) -> Result<(), DecodeError> {
        if count > self.buffer.handles.len() - self.handles_taken {
            return Err(DecodeError::InsufficientHandles);
        }

        for i in self.handles_taken..self.handles_taken + count {
            let handle = replace(&mut self.buffer.handles[i], NullableHandle::invalid());
            drop(handle);
        }
        self.handles_taken += count;

        Ok(())
    }

    fn __internal_handles_remaining(&self) -> usize {
        self.buffer.handles.len() - self.handles_taken
    }
}

impl<'de> Decoder<'de> for BufferDecoder<'de> {
    fn take_chunks(&mut self, count: usize) -> Result<&'de mut [Chunk], DecodeError> {
        if count > self.buffer.chunks.len() - self.chunks_taken {
            return Err(DecodeError::InsufficientData);
        }

        let chunks = unsafe { self.buffer.chunks.as_mut_ptr().add(self.chunks_taken) };
        self.chunks_taken += count;

        unsafe { Ok(slice::from_raw_parts_mut(chunks, count)) }
    }

    fn commit(&mut self) {
        for handle in &mut self.buffer.handles[0..self.handles_taken] {
            // This handle was taken. To commit the current changes, we need to forget it.
            let _ = replace(handle, NullableHandle::invalid()).into_raw();
        }
    }

    fn finish(&self) -> Result<(), DecodeError> {
        if self.chunks_taken != self.buffer.chunks.len() {
            return Err(DecodeError::ExtraBytes {
                num_extra: (self.buffer.chunks.len() - self.chunks_taken) * CHUNK_SIZE,
            });
        }

        if self.handles_taken != self.buffer.handles.len() {
            return Err(DecodeError::ExtraHandles {
                num_extra: self.buffer.handles.len() - self.handles_taken,
            });
        }

        Ok(())
    }
}

impl HandleDecoder for BufferDecoder<'_> {
    fn take_raw_handle(&mut self) -> Result<zx_handle_t, DecodeError> {
        if self.handles_taken >= self.buffer.handles.len() {
            return Err(DecodeError::InsufficientHandles);
        }

        let handle = self.buffer.handles[self.handles_taken].raw_handle();
        self.handles_taken += 1;

        Ok(handle)
    }

    fn handles_remaining(&mut self) -> usize {
        self.buffer.handles.len() - self.handles_taken
    }
}

impl Transport for Channel {
    type Error = Status;

    fn split(self) -> (Self::Shared, Self::Exclusive) {
        (Shared::new(self), Exclusive { _phantom: PhantomData })
    }

    type Shared = Shared;
    type SendBuffer = Buffer;
    type SendFutureState = SendFutureState;

    fn acquire(_: &Self::Shared) -> Self::SendBuffer {
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

    type Exclusive = Exclusive;
    type RecvFutureState = RecvFutureState;
    type RecvBuffer = Buffer;

    fn begin_recv(_: &Self::Shared, _: &mut Self::Exclusive) -> Self::RecvFutureState {
        RecvFutureState { buffer: Some(Buffer::new()) }
    }

    fn poll_recv(
        mut future_state: Pin<&mut Self::RecvFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
        _: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>> {
        let buffer = future_state.buffer.as_mut().unwrap();

        let mut actual_bytes = 0;
        let mut actual_handles = 0;

        loop {
            let result = unsafe {
                zx_channel_read(
                    shared.channel.get_ref().raw_handle(),
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
                    if matches!(shared.channel.need_readable(cx)?, Poll::Pending) {
                        return Poll::Pending;
                    }
                }
                raw => return Poll::Ready(Err(Some(Status::err_from_raw(raw)))),
            }
        }
    }
}

impl NonBlockingTransport for Channel {
    fn send_immediately(
        future_state: &mut Self::SendFutureState,
        shared: &Self::Shared,
    ) -> Result<(), Option<Self::Error>> {
        let result = unsafe {
            zx_channel_write(
                shared.channel.get_ref().raw_handle(),
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

#[cfg(test)]
mod tests {
    use fidl_next_codec::{AsDecoder as _, DecoderExt as _, EncoderExt as _};
    use fuchsia_async as fasync;
    use zx::{Channel, Instant, NullableHandle, Signals, WaitResult};

    use crate::fuchsia::channel::Buffer;
    use crate::testing::*;

    #[fasync::run_singlethreaded(test)]
    async fn close_on_drop() {
        test_close_on_drop(Channel::create).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn one_way() {
        test_one_way(Channel::create).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn one_way_nonblocking() {
        test_one_way_nonblocking(Channel::create).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn two_way() {
        test_two_way(Channel::create).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn multiple_two_way() {
        test_multiple_two_way(Channel::create).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn event() {
        test_event(Channel::create).await;
    }

    struct HandleAndBoolean {
        handle: NullableHandle,
        boolean: bool,
    }

    mod wire {
        use core::mem::MaybeUninit;

        use fidl_next_codec::fuchsia::{HandleDecoder, HandleEncoder};
        use fidl_next_codec::{
            Constrained, Decode, DecodeError, Encode, EncodeError, FromWire, Slot, ValidationError,
            Wire, munge, wire,
        };

        #[derive(Debug)]
        #[repr(C)]
        pub struct HandleAndBoolean {
            handle: wire::fuchsia::Handle,
            boolean: bool,
        }

        impl Constrained for HandleAndBoolean {
            type Constraint = ();

            fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
                Ok(())
            }
        }

        unsafe impl Wire for HandleAndBoolean {
            type Narrowed<'de> = Self;

            fn zero_padding(out: &mut MaybeUninit<Self>) {
                unsafe {
                    out.as_mut_ptr().write_bytes(0, 1);
                }
            }
        }

        unsafe impl<E: HandleEncoder + ?Sized> Encode<HandleAndBoolean, E> for super::HandleAndBoolean {
            fn encode(
                self,
                encoder: &mut E,
                out: &mut MaybeUninit<HandleAndBoolean>,
                _: (),
            ) -> Result<(), EncodeError> {
                munge!(let HandleAndBoolean { handle, boolean } = out);
                self.handle.encode(encoder, handle, ())?;
                self.boolean.encode(encoder, boolean, ())?;
                Ok(())
            }
        }

        unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for HandleAndBoolean {
            fn decode(slot: Slot<'_, Self>, decoder: &mut D, _: ()) -> Result<(), DecodeError> {
                munge!(let Self { handle, boolean } = slot);
                Decode::decode(handle, decoder, ())?;
                Decode::decode(boolean, decoder, ())?;
                Ok(())
            }
        }

        impl FromWire<HandleAndBoolean> for super::HandleAndBoolean {
            fn from_wire(wire: HandleAndBoolean) -> Self {
                Self { handle: zx::NullableHandle::from_wire(wire.handle), boolean: wire.boolean }
            }
        }
    }

    #[test]
    fn partial_decode_drops_handles() {
        let (encode_end, check_end) = Channel::create();

        let mut buffer =
            Buffer::encode(HandleAndBoolean { handle: encode_end.into_handle(), boolean: false })
                .expect("encoding should succeed");
        // Modify the buffer so that the boolean value is invalid
        *buffer.chunks[0] |= 0x00000002_00000000;

        let mut decoder = buffer.as_decoder();
        decoder
            .decode::<wire::HandleAndBoolean>()
            .expect_err("decoding an invalid boolean should fail");

        // Decoding failed, so the handle should still be in the buffer.
        assert_eq!(
            check_end.wait_one(Signals::CHANNEL_PEER_CLOSED, Instant::INFINITE_PAST),
            WaitResult::TimedOut(Signals::CHANNEL_WRITABLE),
        );

        drop(buffer);

        // The handle should have been dropped with the buffer.
        assert_eq!(
            check_end.wait_one(Signals::CHANNEL_PEER_CLOSED, Instant::INFINITE_PAST),
            WaitResult::Ok(Signals::CHANNEL_PEER_CLOSED),
        );
    }

    #[test]
    fn complete_decode_moves_handles() {
        let (encode_end, check_end) = Channel::create();

        let mut buffer =
            Buffer::encode(HandleAndBoolean { handle: encode_end.into_handle(), boolean: false })
                .expect("encoding should succeed");

        let mut decoder = buffer.as_decoder();
        let decoded = decoder.decode::<wire::HandleAndBoolean>().expect("decoding should succeed");

        // The handle should remain un-signaled after successful decoding.
        assert_eq!(
            check_end.wait_one(Signals::CHANNEL_PEER_CLOSED, Instant::INFINITE_PAST),
            WaitResult::TimedOut(Signals::CHANNEL_WRITABLE),
        );

        drop(decoded);

        // Now the handle should be signaled.
        assert_eq!(
            check_end.wait_one(Signals::CHANNEL_PEER_CLOSED, Instant::INFINITE_PAST),
            WaitResult::Ok(Signals::CHANNEL_PEER_CLOSED),
        );
    }
}
