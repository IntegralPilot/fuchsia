// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Epitaph support for Channel and AsyncChannel.

use crate::encoding::{
    self, DynamicFlags, EpitaphBody, NoHandleResourceDialect, TransactionHeader,
    TransactionMessage, TransactionMessageType,
};
use crate::error::Error;
use crate::{AsyncChannel, Channel, TransportError};
use zx_status;

/// Extension trait that provides Channel-like objects with the ability to send a FIDL epitaph.
pub trait ChannelEpitaphExt {
    /// Consumes the channel and writes an epitaph.
    fn close_with_epitaph(
        self,
        status: impl Into<Result<(), zx_status::Status>>,
    ) -> Result<(), Error>;
}

impl<T: ChannelLike> ChannelEpitaphExt for T {
    fn close_with_epitaph(
        self,
        status: impl Into<Result<(), zx_status::Status>>,
    ) -> Result<(), Error> {
        write_epitaph_impl(&self, status)
    }
}

/// Indicates an object is "channel-like" in that it can receive epitaphs.
pub trait ChannelLike {
    /// Write an epitaph to a channel. Same as write_etc but is never fed handles.
    fn write_epitaph(&self, bytes: &[u8]) -> Result<(), TransportError>;
}

impl ChannelLike for Channel {
    fn write_epitaph(&self, bytes: &[u8]) -> Result<(), TransportError> {
        self.write_etc(bytes, &mut []).map_err(Into::into)
    }
}

impl ChannelLike for AsyncChannel {
    fn write_epitaph(&self, bytes: &[u8]) -> Result<(), TransportError> {
        self.write_etc(bytes, &mut []).map_err(Into::into)
    }
}

pub(crate) fn write_epitaph_impl<T: ChannelLike>(
    channel: &T,
    status: impl Into<Result<(), zx_status::Status>>,
) -> Result<(), Error> {
    let msg = TransactionMessage {
        header: TransactionHeader::new(0, encoding::EPITAPH_ORDINAL, DynamicFlags::empty()),
        body: &EpitaphBody { error: status.into() },
    };
    encoding::with_tls_encoded::<TransactionMessageType<EpitaphBody>, NoHandleResourceDialect, ()>(
        msg,
        |bytes, handles| {
            assert!(handles.is_empty(), "Epitaph should not have handles!");
            match channel.write_epitaph(bytes) {
                Ok(()) | Err(TransportError::Status(zx_status::Status::PEER_CLOSED)) => Ok(()),
                Err(e) => Err(Error::ServerEpitaphWrite(e)),
            }
        },
    )
}
