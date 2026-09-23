// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common error types for the netstack.

use net_types::ip::{GenericOverIp, Ip};
use packet::Nested;
use thiserror::Error;

/// Error when something is not supported.
#[derive(Debug, PartialEq, Eq, Error, GenericOverIp)]
#[generic_over_ip()]
#[error("not supported")]
pub struct NotSupportedError;

/// Error when something exists unexpectedly.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("already exists")]
pub struct ExistsError;

impl From<ExistsError> for SocketError {
    fn from(_: ExistsError) -> SocketError {
        SocketError::Local(LocalAddressError::AddressInUse)
    }
}

/// Error when something unexpectedly doesn't exist, such as trying to
/// remove an element when the element is not present.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("not found")]
pub struct NotFoundError;

/// Error type for errors common to local addresses.
#[derive(Error, Debug, PartialEq, GenericOverIp)]
#[generic_over_ip()]
pub enum LocalAddressError {
    /// Cannot bind to address.
    #[error("can't bind to address")]
    CannotBindToAddress,

    /// Failed to allocate local port.
    #[error("failed to allocate local port")]
    FailedToAllocateLocalPort,

    /// Specified local address does not match any expected address.
    #[error("specified local address does not match any expected address")]
    AddressMismatch,

    /// The requested address/socket pair is in use.
    #[error("address in use")]
    AddressInUse,

    /// The address cannot be used because of its zone.
    ///
    /// TODO(https://fxbug.dev/42054471): Make this an IP socket error once UDP
    /// sockets contain IP sockets.
    #[error(transparent)]
    Zone(#[from] ZonedAddressError),

    /// The requested address is mapped (i.e. an IPv4-mapped-IPv6 address), but
    /// the socket is not dual-stack enabled.
    #[error("address is mapped")]
    AddressUnexpectedlyMapped,
}

/// Indicates a problem related to an address with a zone.
#[derive(Copy, Clone, Debug, Error, Eq, PartialEq, GenericOverIp)]
#[generic_over_ip()]
pub enum ZonedAddressError {
    /// The address scope requires a zone but didn't have one.
    #[error("the address requires a zone but didn't have one")]
    RequiredZoneNotProvided,
    /// The address has a zone that doesn't match an existing device constraint.
    #[error("the socket's device does not match the zone")]
    DeviceZoneMismatch,
}

/// An error encountered when attempting to create a UDP, TCP, or ICMP connection.
#[derive(Error, Debug, PartialEq)]
pub enum RemoteAddressError {
    /// No route to host.
    #[error("no route to host")]
    NoRoute,
}

/// Error type for connection errors.
#[derive(Error, Debug, PartialEq)]
pub enum SocketError {
    /// Errors related to the local address.
    #[error(transparent)]
    Local(#[from] LocalAddressError),

    /// Errors related to the remote address.
    #[error(transparent)]
    Remote(RemoteAddressError),
}

/// Error when link address resolution failed for a neighbor.
#[derive(Error, Debug, PartialEq)]
#[error("address resolution failed")]
pub struct AddressResolutionFailed;

/// An error and a serializer.
///
/// This error type encodes the common pattern of returning an error and the
/// original serializer that caused the error. For brevity, users are encouraged
/// to alias this to local types.
///
/// It provides a `From` implementation from `(E, S)` for convenience and
/// impedance matching with the [`packet`] crate.
#[derive(Debug, PartialEq, Eq)]
pub struct ErrorAndSerializer<E, S> {
    /// The error observed.
    pub error: E,
    /// The serializer accompanying the error.
    pub serializer: S,
}

impl<E, S> ErrorAndSerializer<E, S> {
    /// Changes the serializer type.
    pub fn map_serializer<N, F: FnOnce(S) -> N>(self, f: F) -> ErrorAndSerializer<E, N> {
        let Self { error, serializer } = self;
        ErrorAndSerializer { error, serializer: f(serializer) }
    }

    /// Changes the error type.
    pub fn map_err<N, F: FnOnce(E) -> N>(self, f: F) -> ErrorAndSerializer<N, S> {
        let Self { error, serializer } = self;
        ErrorAndSerializer { error: f(error), serializer }
    }

    /// Changes the error type using [`Into::into`].
    pub fn err_into<N: From<E>>(self) -> ErrorAndSerializer<N, S> {
        self.map_err(Into::into)
    }

    /// Consumes this pair and returns only the error, dropping the serializer.
    pub fn into_err(self) -> E {
        self.error
    }
}

impl<E, I, O> ErrorAndSerializer<E, Nested<I, O>> {
    /// A convenience function for dealing with [`Nested`] serializers.
    ///
    /// Equivalent to using [`ErrorAndSerializer::map_serializer`].
    pub fn into_inner(self) -> ErrorAndSerializer<E, I> {
        self.map_serializer(|s| s.into_inner())
    }
}
