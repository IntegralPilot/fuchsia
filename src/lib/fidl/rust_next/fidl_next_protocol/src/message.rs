// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next_codec::{AsDecoder, DecodeError, DecoderExt};

use crate::Transport;
use crate::wire::MessageHeader;

/// A FIDL message with a decoded message header.
///
/// This is a simple wrapper around `T::RecvBuffer` that skips the transaction
/// header when `as_decoder` is called. The message header can be retrieved by
/// calling `header()`.
pub struct Message<T: Transport> {
    buffer: T::RecvBuffer,
}

impl<T: Transport> Message<T> {
    /// Decodes the given buffer, returning the message header and
    pub fn decode(mut buffer: T::RecvBuffer) -> Result<Self, DecodeError> {
        let _ = buffer.as_decoder().decode_prefix::<MessageHeader>()?;
        Ok(Self { buffer })
    }

    /// Returns the message header.
    pub fn header(&mut self) -> MessageHeader {
        let mut decoder = self.buffer.as_decoder();
        unsafe { *decoder.take_slot::<MessageHeader>().unwrap().deref_unchecked() }
    }
}

unsafe impl<'de, T: Transport> AsDecoder<'de> for Message<T> {
    type Decoder = <T::RecvBuffer as AsDecoder<'de>>::Decoder;

    fn as_decoder(&'de mut self) -> Self::Decoder {
        let mut decoder = self.buffer.as_decoder();
        let _ = decoder.take_slot::<MessageHeader>();
        decoder
    }
}
