// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::wire;
use crate::responder::Responder;
use crate::{AsHandleRef, Channel, ChannelMessageStream, ChannelWriter, Error, Handle};
use fidl_fuchsia_fdomain as proto;
use fidl_next::AsDecoder;
use fidl_next_codec::decoder::InternalHandleDecoder;
use fidl_next_codec::encoder::InternalHandleEncoder;
use fidl_next_codec::{CHUNK_SIZE, Chunk, DecodeError, Decoder, EncodeError, Encoder};
use fidl_next_protocol::Transport;
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt};

use core::slice;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

/// A decoder which supports FDomain handles.
pub trait HandleDecoder {
    /// Takes the next raw handle from the decoder.
    ///
    /// The returned raw handle must not be considered owned until the decoder is committed.
    fn take_raw_handle(&mut self) -> Result<u32, DecodeError>;

    /// Returns the number of handles remaining in the decoder.
    fn handles_remaining(&mut self) -> usize;
}

/// An encoder which supports FDomain handles.
pub trait HandleEncoder {
    /// Pushes a handle into the encoder.
    fn push_handle(&mut self, handle: Handle) -> Result<(), EncodeError>;

    /// Returns the number of handles added to the encoder.
    fn handles_pushed(&self) -> usize;
}

/// Send buffer for an FDomain channel.
#[derive(Default)]
pub struct SendBuffer {
    handles: Vec<Handle>,
    chunks: Vec<Chunk>,
}

impl SendBuffer {
    /// New buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Retrieve the handles.
    pub fn handles(&self) -> &[Handle] {
        &self.handles
    }
}

impl InternalHandleEncoder for SendBuffer {
    #[inline]
    fn __internal_handle_count(&self) -> usize {
        self.handles.len()
    }
}

impl Encoder for SendBuffer {
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

impl HandleEncoder for SendBuffer {
    fn push_handle(&mut self, handle: Handle) -> Result<(), EncodeError> {
        self.handles.push(handle.into());
        Ok(())
    }

    fn handles_pushed(&self) -> usize {
        self.handles.len()
    }
}

/// A receive buffer for an FDomain channel.
pub struct RecvBuffer {
    handles: Vec<wire::Handle>,
    chunks: Vec<Chunk>,
}

pub struct BufferDecoder<'de> {
    buffer: &'de mut RecvBuffer,
    chunks_taken: usize,
    handles_taken: usize,
}

unsafe impl<'de> AsDecoder<'de> for RecvBuffer {
    type Decoder = BufferDecoder<'de>;

    fn as_decoder(&'de mut self) -> Self::Decoder {
        BufferDecoder { buffer: self, chunks_taken: 0, handles_taken: 0 }
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
            handle.invalidate();
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

impl InternalHandleDecoder for BufferDecoder<'_> {
    fn __internal_take_handles(&mut self, count: usize) -> Result<(), DecodeError> {
        if count > self.buffer.handles.len() - self.handles_taken {
            return Err(DecodeError::InsufficientHandles);
        }

        for i in self.handles_taken..self.handles_taken + count {
            drop(self.buffer.handles[i].take_handle());
        }
        self.handles_taken += count;

        Ok(())
    }

    fn __internal_handles_remaining(&self) -> usize {
        self.buffer.handles.len() - self.handles_taken
    }
}

impl HandleDecoder for BufferDecoder<'_> {
    fn take_raw_handle(&mut self) -> Result<u32, DecodeError> {
        if self.handles_taken >= self.buffer.handles.len() {
            return Err(DecodeError::InsufficientHandles);
        }

        let handle = self.buffer.handles[self.handles_taken].as_raw_handle();
        self.handles_taken += 1;

        Ok(handle)
    }

    fn handles_remaining(&mut self) -> usize {
        self.buffer.handles.len() - self.handles_taken
    }
}

/// Sender for an FDomain channel.
#[derive(Clone)]
pub struct Shared {
    writer: ChannelWriter,
}

/// The state for a channel send future.
pub enum SendFutureState {
    /// The message was too big to fit in a channel.
    BadGeometry,
    Wait(oneshot::Receiver<Result<(), Error>>),
}

/// A channel receiver.
pub struct Exclusive {
    stream: ChannelMessageStream,
}

impl Transport for Channel {
    type Error = Error;

    fn split(self) -> (Self::Shared, Self::Exclusive) {
        let (stream, writer) = self.stream().expect("could not split channel");
        (Shared { writer }, Exclusive { stream })
    }

    type Shared = Shared;
    type SendBuffer = SendBuffer;
    type SendFutureState = SendFutureState;

    fn acquire(_: &Self::Shared) -> Self::SendBuffer {
        SendBuffer::new()
    }

    fn begin_send(sender: &Self::Shared, buffer: Self::SendBuffer) -> Self::SendFutureState {
        let client = sender.writer.as_channel().as_handle_ref().client();
        let handle = sender.writer.as_channel().as_handle_ref().proto();
        let data = buffer.chunks;
        let handles = buffer.handles;

        // SAFETY: It should be safe to byte-cast from chunks always.
        let data = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * CHUNK_SIZE).to_vec()
        };

        if data.len() > zx_types::ZX_CHANNEL_MAX_MSG_BYTES as usize
            || handles.len() > zx_types::ZX_CHANNEL_MAX_MSG_HANDLES as usize
        {
            SendFutureState::BadGeometry
        } else {
            let handles =
                proto::Handles::Handles(handles.into_iter().map(|x| x.take_proto()).collect());
            let (sender, receiver) = oneshot::channel();
            let mut client = client.0.lock();
            client.request(
                crate::ordinals::WRITE_CHANNEL,
                proto::ChannelWriteChannelRequest { handle, data, handles },
                Responder::WriteChannel(sender),
            );

            SendFutureState::Wait(receiver)
        }
    }

    fn poll_send(
        mut future_state: Pin<&mut Self::SendFutureState>,
        ctx: &mut Context<'_>,
        _: &Self::Shared,
    ) -> Poll<Result<(), Option<Self::Error>>> {
        match &mut *future_state {
            SendFutureState::BadGeometry => Poll::Ready(Err(Some(Error::FDomain(
                proto::Error::TargetError(fidl::Status::OUT_OF_RANGE.into_raw()),
            )))),
            SendFutureState::Wait(receiver) => receiver.poll_unpin(ctx).map(|x| {
                match x.expect("Receiver disappeared with no reply") {
                    Ok(x) => Ok(x),
                    Err(Error::FDomain(proto::Error::TargetError(e)))
                        if e == fidl::Status::PEER_CLOSED.into_raw() =>
                    {
                        Err(None)
                    }
                    Err(e) => Err(Some(e)),
                }
            }),
        }
    }

    type Exclusive = Exclusive;
    type RecvFutureState = ();
    type RecvBuffer = RecvBuffer;

    fn begin_recv(_: &Self::Shared, _: &mut Self::Exclusive) -> Self::RecvFutureState {}

    fn poll_recv(
        _: Pin<&mut Self::RecvFutureState>,
        ctx: &mut Context<'_>,
        _: &Self::Shared,
        exclusive: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>> {
        let poll_stream = exclusive.stream.poll_next_unpin(ctx);

        let Some(msg) = ready!(poll_stream).transpose().map_err(Some)? else {
            return Poll::Ready(Err(None));
        };

        // SAFETY: It should be safe to byte-cast to a chunk always.
        let chunks = unsafe {
            std::slice::from_raw_parts(
                msg.bytes.as_ptr() as *const Chunk,
                msg.bytes.len() / CHUNK_SIZE,
            )
            .to_vec()
        };
        let handles = msg.handles.into_iter().map(|x| Handle::from(x.handle).into()).collect();

        Poll::Ready(Ok(RecvBuffer { handles, chunks }))
    }
}
