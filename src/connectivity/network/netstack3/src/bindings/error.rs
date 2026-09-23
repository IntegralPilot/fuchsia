// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides an [`Error`] type for all errors observable by bindings.

use std::convert::Infallible as Never;

use netstack3_core::device_socket::SendFrameErrorReason;
use netstack3_core::error::{
    LocalAddressError, NotFoundError, NotSupportedError, RemoteAddressError, SocketError,
    ZonedAddressError,
};
use netstack3_core::ip::{
    IpSockCreationError, IpSockSendError, RawIpSocketIcmpFilterError, RawIpSocketSendToError,
    ResolveRouteError,
};
use netstack3_core::socket::{
    ConnectError, ExpectedConnError, ExpectedUnboundError, NotDualStackCapableError, SendError,
    SendToError, SetDualStackEnabledError, SetMulticastMembershipError,
};
use netstack3_core::{PendingDatagramSocketError, tcp, udp};
use thiserror::Error;

use crate::bindings::socket::datagram::{IcmpSendError, UdpSendError};
use crate::bindings::util::{
    DeviceNotFoundError, MulticastMembershipConversionError, SocketAddressError,
    WrongIpVersionError,
};

/// A single enumeration of all errors observable by bindings.
#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("{0}")]
    Untyped(&'static str),
    #[error(transparent)]
    ConnectError(#[from] ConnectError),
    #[error(transparent)]
    DeviceNotFoundError(#[from] DeviceNotFoundError),
    #[error(transparent)]
    ExpectedConnError(#[from] ExpectedConnError),
    #[error(transparent)]
    ExpectedUnboundError(#[from] ExpectedUnboundError),
    #[error(transparent)]
    IpSockCreationError(#[from] IpSockCreationError),
    #[error(transparent)]
    IpSockSendError(#[from] IpSockSendError),
    #[error(transparent)]
    LocalAddressError(#[from] LocalAddressError),
    #[error(transparent)]
    MulticastMembershipConversionError(#[from] MulticastMembershipConversionError),
    #[error(transparent)]
    NotDualStackCapableError(#[from] NotDualStackCapableError),
    #[error(transparent)]
    NotFoundError(#[from] NotFoundError),
    #[error(transparent)]
    NotSupportedError(#[from] NotSupportedError),
    #[error(transparent)]
    PendingDatagramSocketError(#[from] PendingDatagramSocketError),
    #[error(transparent)]
    RawIpSocketSendToError(#[from] RawIpSocketSendToError),
    #[error(transparent)]
    RawIpSocketIcmpFilterError(#[from] RawIpSocketIcmpFilterError),
    #[error(transparent)]
    RemoteAddressError(#[from] RemoteAddressError),
    #[error(transparent)]
    ResolveRouteError(#[from] ResolveRouteError),
    #[error(transparent)]
    SendError(#[from] SendError<packet_formats::error::ParseError>),
    #[error(transparent)]
    SendFrameErrorReason(#[from] SendFrameErrorReason),
    #[error(transparent)]
    SendToError(#[from] SendToError<packet_formats::error::ParseError>),
    #[error(transparent)]
    SetDualStackEnabledError(#[from] SetDualStackEnabledError),
    #[error(transparent)]
    SetMulticastMembershipError(#[from] SetMulticastMembershipError),
    #[error(transparent)]
    SocketAddressError(#[from] SocketAddressError),
    #[error(transparent)]
    SocketError(#[from] SocketError),
    #[error(transparent)]
    TcpAcceptError(#[from] tcp::AcceptError),
    #[error(transparent)]
    TcpBindError(#[from] tcp::BindError),
    #[error(transparent)]
    TcpConnectError(#[from] tcp::ConnectError),
    #[error(transparent)]
    TcpConnectionError(#[from] tcp::ConnectionError),
    #[error(transparent)]
    TcpListenError(#[from] tcp::ListenError),
    #[error(transparent)]
    TcpNoConnection(#[from] tcp::NoConnection),
    #[error(transparent)]
    TcpOriginalDestinationError(#[from] tcp::OriginalDestinationError),
    #[error(transparent)]
    TcpSetDeviceError(#[from] tcp::SetDeviceError),
    #[error(transparent)]
    TcpSetReuseAddrError(#[from] tcp::SetReuseAddrError),
    #[error(transparent)]
    CoreUdpSendError(#[from] udp::SendError),
    #[error(transparent)]
    CoreUdpSendToError(#[from] udp::SendToError),
    #[error(transparent)]
    UdpSendError(#[from] UdpSendError),
    #[error(transparent)]
    IcmpSendError(#[from] IcmpSendError),
    #[error(transparent)]
    WrongIpVersionError(#[from] WrongIpVersionError),
    #[error(transparent)]
    ZonedAddressError(#[from] ZonedAddressError),
}

impl<A: Into<Error>, B: Into<Error>> From<either::Either<A, B>> for Error {
    fn from(value: either::Either<A, B>) -> Self {
        match value {
            either::Either::Left(left) => left.into(),
            either::Either::Right(right) => right.into(),
        }
    }
}

impl From<Never> for Error {
    fn from(value: Never) -> Self {
        match value {}
    }
}

impl From<&'static str> for Error {
    fn from(value: &'static str) -> Self {
        Self::Untyped(value)
    }
}
