// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An implementation of a server for a fidl interface.

use crate::encoding::{
    DefaultFuchsiaResourceDialect, DynamicFlags, EmptyStruct, Encode, Encoder, Flexible,
    FlexibleType, FrameworkErr, HandleFor, ProxyChannelBox, ProxyChannelFor, ResourceDialect,
    TransactionHeader, TransactionMessage, TransactionMessageType, TypeMarker,
};
use crate::{Error, epitaph};
use futures::task::{AtomicWaker, Context};
use std::sync::atomic::{self, AtomicBool};
use zx_status;

/// A type used from the innards of server implementations.
#[derive(Debug)]
pub struct ServeInner<D: ResourceDialect = DefaultFuchsiaResourceDialect> {
    waker: AtomicWaker,
    shutdown: AtomicBool,
    channel: <D::ProxyChannel as ProxyChannelFor<D>>::Boxed,
}

impl<D: ResourceDialect> ServeInner<D> {
    /// Creates a new set of server innards.
    pub fn new(channel: D::ProxyChannel) -> Self {
        let waker = AtomicWaker::new();
        let shutdown = AtomicBool::new(false);
        ServeInner { waker, shutdown, channel: channel.boxed() }
    }

    /// Gets a reference to the inner channel.
    pub fn channel(&self) -> &<D::ProxyChannel as ProxyChannelFor<D>>::Boxed {
        &self.channel
    }

    /// Converts the [`ServerInner`] back into a channel.
    ///
    /// **Warning**: This operation is dangerous, since the returned channel
    /// could have unread messages intended for this server. Use it carefully.
    pub fn into_channel(self) -> D::ProxyChannel {
        self.channel.unbox()
    }

    /// Sets the server to shutdown the next time the stream is polled.
    ///
    /// TODO(https://fxbug.dev/42153903): This should cause the channel to close on the
    /// next poll, but in fact the channel won't close until the user of the
    /// bindings drops their request stream, responders, and control handles.
    pub fn shutdown(&self) {
        self.shutdown.store(true, atomic::Ordering::Relaxed);
        self.waker.wake();
    }

    /// Sets the server to shutdown with an epitaph the next time the stream is polled.
    ///
    /// TODO(https://fxbug.dev/42153903): This should cause the channel to close on the
    /// next poll, but in fact the channel won't close until the user of the
    /// bindings drops their request stream, responders, and control handles.
    pub fn shutdown_with_epitaph(&self, status: impl Into<crate::Epitaph>) {
        let already_shutting_down = self.shutdown.swap(true, atomic::Ordering::Relaxed);
        if !already_shutting_down {
            // Ignore the error, best effort sending an epitaph.
            let status = match status.into() {
                crate::Epitaph::Explicit(res) => res,
                crate::Epitaph::PeerClosed => Err(zx_status::Status::PEER_CLOSED),
            };
            let _ = epitaph::write_epitaph_impl(self.channel.as_channel(), status);
            self.waker.wake();
        }
    }

    /// Checks if the server has been set to shutdown.
    pub fn check_shutdown(&self, cx: &Context<'_>) -> bool {
        if self.shutdown.load(atomic::Ordering::Relaxed) {
            return true;
        }
        self.waker.register(cx.waker());
        self.shutdown.load(atomic::Ordering::Relaxed)
    }

    /// Sends an encodable message to the client.
    #[inline]
    pub fn send<T: TypeMarker>(
        &self,
        body: impl Encode<T, D>,
        tx_id: u32,
        ordinal: u64,
        dynamic_flags: DynamicFlags,
    ) -> Result<(), Error> {
        let msg = TransactionMessage {
            header: TransactionHeader::new(tx_id, ordinal, dynamic_flags),
            body,
        };
        crate::encoding::with_tls_encoded::<TransactionMessageType<T>, D, ()>(
            msg,
            |bytes, handles| self.send_raw_msg(bytes, handles),
        )
    }

    /// Sends a framework error to the client.
    ///
    /// The caller must be inside a `with_tls_decode_buf` callback, and pass the
    /// buffers used to decode the request as the `tls_decode_buf` parameter.
    #[inline]
    pub fn send_framework_err(
        &self,
        framework_err: FrameworkErr,
        tx_id: u32,
        ordinal: u64,
        dynamic_flags: DynamicFlags,
        tls_decode_buf: (&mut Vec<u8>, &mut Vec<<D::Handle as HandleFor<D>>::HandleInfo>),
    ) -> Result<(), Error> {
        type Msg = TransactionMessageType<FlexibleType<EmptyStruct>>;
        let msg = TransactionMessage {
            header: TransactionHeader::new(tx_id, ordinal, dynamic_flags),
            body: Flexible::<()>::FrameworkErr(framework_err),
        };

        // RFC-0138 requires us to close handles in the incoming message before replying.
        let (bytes, handle_infos) = tls_decode_buf;
        handle_infos.clear();
        // Reuse the request decoding byte buffer for encoding (we can't call
        // `with_tls_encoded` as we're already inside `with_tls_decode_buf`).
        let mut handle_dispositions = Vec::new();
        Encoder::<D>::encode::<Msg>(bytes, &mut handle_dispositions, msg)?;
        debug_assert!(handle_dispositions.is_empty());
        self.send_raw_msg(&*bytes, &mut [])
    }

    /// Sends a raw message to the client.
    fn send_raw_msg(
        &self,
        bytes: &[u8],
        handles: &mut [<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition],
    ) -> Result<(), Error> {
        match self.channel.write_etc(bytes, handles) {
            Ok(()) | Err(None) => Ok(()),
            Err(Some(e)) => Err(Error::ServerResponseWrite(e.into())),
        }
    }
}
