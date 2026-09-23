// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use zx::{self as zx, AsHandleRef, MessageBuf, MessageBufEtc};

use crate::{OnSignalsRef, RWHandle, ReadableHandle as _};

/// An I/O object representing a `Channel`.
pub struct Channel(RWHandle<zx::Channel>);

impl AsRef<zx::Channel> for Channel {
    fn as_ref(&self) -> &zx::Channel {
        self.0.get_ref()
    }
}

impl AsHandleRef for Channel {
    fn as_handle_ref(&self) -> zx::HandleRef<'_> {
        self.0.get_ref().as_handle_ref()
    }
}

impl From<Channel> for zx::Channel {
    fn from(channel: Channel) -> zx::Channel {
        channel.0.into_inner()
    }
}

impl Channel {
    /// Creates a new `Channel` from a previously-created `zx::Channel`.
    ///
    /// # Panics
    ///
    /// If called outside the context of an active async executor.
    pub fn from_channel(channel: zx::Channel) -> Self {
        Channel(RWHandle::new(channel))
    }

    /// Consumes `self` and returns the underlying `zx::Channel`.
    pub fn into_zx_channel(self) -> zx::Channel {
        self.0.into_inner()
    }

    /// Returns true if the channel received the `OBJECT_PEER_CLOSED` signal.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Returns a future that completes when `is_closed()` is true.
    pub fn on_closed(&self) -> OnSignalsRef<'_> {
        self.0.on_closed()
    }

    /// Receives a message on the channel and registers this `Channel` as
    /// needing a read on receiving a `zx::Status::SHOULD_WAIT`.
    ///
    /// Identical to `recv_from` except takes separate bytes and handles buffers
    /// rather than a single `MessageBuf`.
    pub fn read(
        &self,
        cx: &mut Context<'_>,
        bytes: &mut Vec<u8>,
        handles: &mut Vec<zx::NullableHandle>,
    ) -> Poll<Result<(), zx::Status>> {
        loop {
            let res = self.0.get_ref().read_split(bytes, handles);
            if res == Err(zx::Status::SHOULD_WAIT) {
                ready!(self.0.need_readable(cx)?);
            } else {
                return Poll::Ready(res);
            }
        }
    }

    /// Receives a message on the channel and registers this `Channel` as
    /// needing a read on receiving a `zx::Status::SHOULD_WAIT`.
    ///
    /// Identical to `recv_etc_from` except takes separate bytes and handles
    /// buffers rather than a single `MessageBufEtc`.
    pub fn read_etc(
        &self,
        cx: &mut Context<'_>,
        bytes: &mut Vec<u8>,
        handles: &mut Vec<zx::HandleInfo>,
    ) -> Poll<Result<(), zx::Status>> {
        loop {
            let res = self.0.get_ref().read_etc_split(bytes, handles);
            if res == Err(zx::Status::SHOULD_WAIT) {
                ready!(self.0.need_readable(cx)?);
            } else {
                return Poll::Ready(res);
            }
        }
    }

    /// Receives a message on the channel and registers this `Channel` as
    /// needing a read on receiving a `zx::Status::SHOULD_WAIT`.
    pub fn recv_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut MessageBuf,
    ) -> Poll<Result<(), zx::Status>> {
        let (bytes, handles) = buf.split_mut();
        self.read(cx, bytes, handles)
    }

    /// Receives a message on the channel and registers this `Channel` as
    /// needing a read on receiving a `zx::Status::SHOULD_WAIT`.
    pub fn recv_etc_from(
        &self,
        cx: &mut Context<'_>,
        buf: &mut MessageBufEtc,
    ) -> Poll<Result<(), zx::Status>> {
        let (bytes, handles) = buf.split_mut();
        self.read_etc(cx, bytes, handles)
    }

    /// Creates a future that receive a message to be written to the buffer
    /// provided.
    ///
    /// The returned future will return after a message has been received on
    /// this socket and been placed into the buffer.
    pub fn recv_msg<'a>(&'a self, buf: &'a mut MessageBuf) -> RecvMsg<'a> {
        RecvMsg { channel: self, buf }
    }

    /// Creates a future that receive a message to be written to the buffer
    /// provided.
    ///
    /// The returned future will return after a message has been received on
    /// this socket and been placed into the buffer.
    pub fn recv_etc_msg<'a>(&'a self, buf: &'a mut MessageBufEtc) -> RecvEtcMsg<'a> {
        RecvEtcMsg { channel: self, buf }
    }

    /// Writes a message into the channel.
    pub fn write(
        &self,
        bytes: &[u8],
        handles: &mut [zx::NullableHandle],
    ) -> Result<(), zx::Status> {
        self.0.get_ref().write(bytes, handles)
    }

    /// Writes a message into the channel.
    pub fn write_etc(
        &self,
        bytes: &[u8],
        handles: &mut [zx::HandleDisposition<'_>],
    ) -> Result<(), zx::Status> {
        self.0.get_ref().write_etc(bytes, handles)
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.get_ref().fmt(f)
    }
}

/// A future used to receive a message from a channel.
///
/// This is created by the `Channel::recv_msg` method.
#[must_use = "futures do nothing unless polled"]
pub struct RecvMsg<'a> {
    channel: &'a Channel,
    buf: &'a mut MessageBuf,
}

impl<'a> Future for RecvMsg<'a> {
    type Output = Result<(), zx::Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        this.channel.recv_from(cx, this.buf)
    }
}
/// A future used to receive a message from a channel.
///
/// This is created by the `Channel::recv_etc_msg` method.
#[must_use = "futures do nothing unless polled"]
pub struct RecvEtcMsg<'a> {
    channel: &'a Channel,
    buf: &'a mut MessageBufEtc,
}

impl<'a> Future for RecvEtcMsg<'a> {
    type Output = Result<(), zx::Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        this.channel.recv_etc_from(cx, this.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestExecutor;

    use std::future::poll_fn;
    use std::mem;
    use std::pin::pin;
    use std::task::Waker;

    #[test]
    fn can_receive() {
        let mut exec = TestExecutor::new();
        let bytes = &[0, 1, 2, 3];

        let (tx, rx) = zx::Channel::create();
        let f_rx = Channel::from_channel(rx);

        let mut receiver = pin!(async move {
            let mut buffer = MessageBuf::new();
            f_rx.recv_msg(&mut buffer).await.expect("failed to receive message");
            assert_eq!(bytes, buffer.bytes());
        });

        assert!(exec.run_until_stalled(&mut receiver).is_pending());

        let mut handles = Vec::new();
        tx.write(bytes, &mut handles).expect("failed to write message");

        assert!(exec.run_until_stalled(&mut receiver).is_ready());
    }

    #[test]
    fn can_receive_etc() {
        let mut exec = TestExecutor::new();
        let bytes = &[0, 1, 2, 3];

        let (tx, rx) = zx::Channel::create();
        let f_rx = Channel::from_channel(rx);

        let mut receiver = pin!(async move {
            let mut buffer = MessageBufEtc::new();
            f_rx.recv_etc_msg(&mut buffer).await.expect("failed to receive message");
            assert_eq!(bytes, buffer.bytes());
        });

        assert!(exec.run_until_stalled(&mut receiver).is_pending());

        let mut handles = Vec::new();
        tx.write_etc(bytes, &mut handles).expect("failed to write message");

        assert!(exec.run_until_stalled(&mut receiver).is_ready());
    }

    #[test]
    fn key_reuse() {
        let mut exec = TestExecutor::new();
        let (tx0, rx0) = zx::Channel::create();
        let (_tx1, rx1) = zx::Channel::create();
        let f_rx0 = Channel::from_channel(rx0);
        mem::drop(tx0);
        mem::drop(f_rx0);
        let f_rx1 = Channel::from_channel(rx1);
        // f_rx0 and f_rx1 use the same key.
        let mut receiver = pin!(async move {
            let mut buffer = MessageBuf::new();
            f_rx1.recv_msg(&mut buffer).await.expect("failed to receive message");
        });

        assert!(exec.run_until_stalled(&mut receiver).is_pending());
    }

    #[test]
    fn key_reuse_etc() {
        let mut exec = TestExecutor::new();
        let (tx0, rx0) = zx::Channel::create();
        let (_tx1, rx1) = zx::Channel::create();
        let f_rx0 = Channel::from_channel(rx0);
        mem::drop(tx0);
        mem::drop(f_rx0);
        let f_rx1 = Channel::from_channel(rx1);
        // f_rx0 and f_rx1 use the same key.
        let mut receiver = pin!(async move {
            let mut buffer = MessageBufEtc::new();
            f_rx1.recv_etc_msg(&mut buffer).await.expect("failed to receive message");
        });

        assert!(exec.run_until_stalled(&mut receiver).is_pending());
    }

    #[test]
    fn test_always_polls_channel() {
        let mut exec = TestExecutor::new();

        let (rx, tx) = zx::Channel::create();
        let rx_channel = Channel::from_channel(rx);

        let mut fut = pin!(poll_fn(|cx| {
            let mut bytes = Vec::with_capacity(64);
            let mut handles = Vec::new();
            rx_channel.read(cx, &mut bytes, &mut handles)
        }));

        assert_eq!(exec.run_until_stalled(&mut fut), Poll::Pending);

        tx.write(b"hello", &mut []).expect("write failed");

        // Poll the future directly which guarantees the port notification for the write hasn't
        // arrived.
        assert_eq!(fut.poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Ok(())));
    }
}
