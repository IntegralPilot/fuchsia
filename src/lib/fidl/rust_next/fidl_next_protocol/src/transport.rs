// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::error::Error;
use core::pin::Pin;
use core::task::{Context, Poll};

use fidl_next_codec::{AsDecoder, Encoder};

/// A transport backend which can send and receive messages.
///
/// # Terminology
///
/// Note that this trait does not correspond directly with the FIDL notion of a
/// transport. For clarity, implementors of this trait should be called
/// "transport backends", as they are specific implementations of the more
/// general notion of a "transport" in FIDL.
///
/// In FIDL, protocols can be assigned a "transport" such as "Channel" or
/// "Driver". The choice of transport for a protocol controls the concrete types
/// used for client ends and server ends of that protocol. A protocol with
/// "Channel" transport will have client and server ends that are Zircon
/// channels; a protocol with "Driver" transport will have client and server
/// ends that are Driver channels.
///
/// All transport backends must be able to send and receive buffers of bytes. In
/// addition to those bytes, transport backends may also support sending and
/// receiving resource types like Zircon and Driver handles with those bytes.
/// The additional resource types a transport backend supports defines which
/// protocols can be run over that transport backend: a protocol can be run over
/// a transport backend if all of the resource types its messages may contain
/// can be sent and received using that transport backend. This may or may not
/// have a correlation with the FIDL transport of that protocol.
///
/// # Implementation
///
/// The futures provided by this trait should be cancel-safe, which constrains
/// their behavior:
///
/// - Operations should not partially complete.
/// - Operations should only complete during polling.
///
/// `SendFuture` should return `Poll::Ready` with an error when polled after the
/// transport backend is closed.
pub trait Transport {
    /// The error type for the transport backend.
    type Error: Clone + Error + Send + Sync + 'static;

    /// Splits the transport backend into shared and exclusive pieces.
    fn split(self) -> (Self::Shared, Self::Exclusive);

    /// The shared part of the transport backend. It is provided by shared
    /// reference while sending and receiving. For an MPSC, this would contain a
    /// sender.
    type Shared: Send + Sync;
    /// The exclusive part of the transport backend. It is provided by mutable
    /// reference only while receiving. For an MPSC, this would contain a
    /// receiver.
    type Exclusive: Send;

    /// The buffer type for sending.
    type SendBuffer: Encoder + Send;
    /// The future state for send operations.
    type SendFutureState: Send;

    /// Acquires an empty send buffer for the transport backend.
    fn acquire(shared: &Self::Shared) -> Self::SendBuffer;

    /// Begins sending a `SendBuffer` over this transport backend.
    ///
    /// Returns the state for a future which can be polled with `poll_send`.
    fn begin_send(shared: &Self::Shared, buffer: Self::SendBuffer) -> Self::SendFutureState;

    /// Polls a `SendFutureState` for completion with the shared part of the
    /// transport backend.
    ///
    /// When ready, polling returns one of three values:
    /// - `Ok(())` if the buffer was successfully sent.
    /// - `Err(None)` if the connection was terminated normally (e.g. with
    ///   `PEER_CLOSED`).
    /// - `Err(Some(error))` if the connection was terminated abnormally.
    fn poll_send(
        future: Pin<&mut Self::SendFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
    ) -> Poll<Result<(), Option<Self::Error>>>;

    /// The future state for receive operations.
    type RecvFutureState: Send;
    /// The buffer type for receivers.
    type RecvBuffer: for<'de> AsDecoder<'de> + Send;

    /// Begins receiving a `RecvBuffer` over this transport backend.
    ///
    /// Returns the state for a future which can be polled with `poll_recv`.
    fn begin_recv(shared: &Self::Shared, exclusive: &mut Self::Exclusive) -> Self::RecvFutureState;

    /// Polls a `RecvFutureState` for completion with a receiver.
    ///
    /// When ready, polling returns one of three values:
    /// - `Ok(buffer)` if `buffer` was successfully received.
    /// - `Err(None)` if the connection was terminated normally (e.g. with
    ///   `PEER_CLOSED`).
    /// - `Err(Some(error))` if the connection was terminated abnormally.
    fn poll_recv(
        future: Pin<&mut Self::RecvFutureState>,
        cx: &mut Context<'_>,
        shared: &Self::Shared,
        exclusive: &mut Self::Exclusive,
    ) -> Poll<Result<Self::RecvBuffer, Option<Self::Error>>>;
}

/// A transport backend which can send messages without blocking.
///
/// Because failed sends return immediately without waiting for an epitaph to be
/// read, `send_immediately` may observe transport backend closure prematurely.
///
/// Non-blocking send operations cannot apply backpressure, which can cause
/// memory exhaustion across the system. `NonBlockingTransport` is intended for
/// use only while porting existing code.
pub trait NonBlockingTransport: Transport {
    /// Completes a `SendFutureState` with the shared part of the transport
    /// backend without blocking.
    fn send_immediately(
        future_state: &mut Self::SendFutureState,
        shared: &Self::Shared,
    ) -> Result<(), Option<Self::Error>>;
}
