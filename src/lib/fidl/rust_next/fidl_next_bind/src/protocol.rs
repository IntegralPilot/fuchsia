// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next_codec::{Decoded, wire as codec_wire};
use fidl_next_protocol::{Flexibility, Transport, wire as protocol_wire};

/// A FIDL protocol which has associated connection handles.
///
/// # Safety
///
/// The associated `Client` and `Server` types must be `#[repr(transparent)]`
/// wrappers around `Client<T>` and `Server<T>` respectively.
pub unsafe trait HasConnectionHandles<T> {
    /// The client for the protocol. It must be a `#[repr(transparent)]` wrapper
    /// around `Client<T>`.
    type Client;

    /// The server for the protocol. It must be a `#[repr(transparent)]` wrapper
    /// around `Server<T>`.
    type Server;
}

/// A discoverable FIDL protocol.
pub trait Discoverable {
    /// The service name to use to connect to this discoverable protocol.
    const PROTOCOL_NAME: &'static str;
}

/// A method of a protocol.
pub trait Method {
    /// The ordinal associated with the method;
    const ORDINAL: u64;

    /// The flexibility of the method.
    const FLEXIBILITY: Flexibility;

    /// The protocol the method is a member of.
    type Protocol;

    /// The request payload for the method.
    type Request;
}

/// A protocol method which has a response.
pub trait TwoWayMethod: Method {
    /// The response message for the method.
    type Response: Response;
}

/// A protocol response message.
///
/// Two-way FIDL responses can be strict or flexible, and flexible responses
/// wrap their responses in a FIDL union. This means that strict and flexible
/// FIDL responses have different wire formats even though we treat flexible
/// errors as protocol errors. So the protocol and bind layers get a little
/// mixed up here.
///
/// To solve this, FIDL response types implement this `Response` trait and
/// define how to "unwrap" themselves into their decoded `Payload` types. This
/// eliminates an unnecessary `.0` field access that we'd have to do otherwise.
pub trait Response {
    /// The payload of the response.
    type Payload;

    /// Converts a `Decoded` of this type to a `Decoded` of its payload type.
    fn into_payload<D>(decoded: Decoded<Self, D>) -> Decoded<Self::Payload, D>;
}

impl<T, E> Response for codec_wire::Result<'_, T, E> {
    type Payload = Self;

    fn into_payload<D>(decoded: Decoded<Self, D>) -> Decoded<Self::Payload, D> {
        decoded
    }
}

impl<T> Response for protocol_wire::Flexible<'_, T> {
    type Payload = T;

    fn into_payload<D>(decoded: Decoded<Self, D>) -> Decoded<Self::Payload, D> {
        let (ptr, decoder) = Decoded::into_raw_parts(decoded);
        let envelope = unsafe { codec_wire::Union::get_raw(ptr.cast()) };
        let inner = unsafe { codec_wire::Envelope::as_ptr(envelope) };
        unsafe { Decoded::new_unchecked(inner, decoder) }
    }
}

impl<T> Response for protocol_wire::Strict<T> {
    type Payload = T;

    fn into_payload<D>(decoded: Decoded<Self, D>) -> Decoded<Self::Payload, D> {
        let (ptr, decoder) = Decoded::into_raw_parts(decoded);
        unsafe { Decoded::new_unchecked(ptr.cast(), decoder) }
    }
}

/// A method which can be responded to with a single value.
///
/// For methods which return a result, this method implicitly returns `Ok` of
/// the given response.
pub trait Respond<R> {
    /// The returned response type.
    type Output;

    /// Makes a response from the given input.
    fn respond(response: R) -> Self::Output;
}

/// A method which can be responded `Err` to with a single value.
pub trait RespondErr<R> {
    /// The returned response type.
    type Output;

    /// Makes an `Err` response from the given input.
    fn respond_err(response: R) -> Self::Output;
}

/// A protocol which has a default transport type.
pub trait HasTransport {
    /// The default transport type for this protocol.
    type Transport: Transport;
}
