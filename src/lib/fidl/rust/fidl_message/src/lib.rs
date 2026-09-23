// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module provides an API for encoding and decoding FIDL transaction
//! messages directly, without using protocol bindings or Zircon channels.
//! The messages must be value types (no handles).
//!
//! # Usage
//!
//! ## Encoding
//!
//! 1. Create a header with `TransactionHeader::new`.
//! 2. Use one of the `encode_*` methods to encode the message.
//!
//! ## Decoding
//!
//! 1. Decode the header with `decode_transaction_header`.
//! 2. Use one of the `decode_*` methods to decode the message body.
//!

pub use fidl::encoding::{decode_transaction_header, Decode, DynamicFlags, TransactionHeader};

use fidl::encoding::{
    Decoder, EmptyStruct, Encode, Encoder, Flexible, FlexibleResult, FlexibleResultType,
    FlexibleType, FrameworkErr, NoHandleResourceDialect, ResultType, TransactionMessage,
    TransactionMessageType, TypeMarker, ValueTypeMarker,
};
use fidl::{new_empty, Error};

/// A trait for types that can be a FIDL request/response body.
/// This is implemented for `()` and FIDL structs, tables, and unions.
pub use fidl::for_fidl_message_crate::Body;

/// A trait for types that can be the domain error variant of a FIDL response.
/// This only applies to two-way methods that use error syntax.
/// This is implemented for primitives and user-defined FIDL types.
pub use fidl::for_fidl_message_crate::ErrorType;

/// Encodes a FIDL transaction message (request or response).
/// Use this for one-way methods, events, and two-way method requests.
/// For two-way method responses:
/// - use `encode_response_result` if the method has error syntax (strict or flexible).
/// - use `encode_response_flexible` if the method is flexible (no error syntax).
/// - use this method otherwise.
pub fn encode_message<T: Body>(header: TransactionHeader, body: T) -> Result<Vec<u8>, Error>
where
    for<'a> <<T as Body>::MarkerAtTopLevel as ValueTypeMarker>::Borrowed<'a>:
        Encode<T::MarkerAtTopLevel, NoHandleResourceDialect>,
{
    encode::<T::MarkerAtTopLevel>(header, T::MarkerAtTopLevel::borrow(&body))
}

/// Encodes a FIDL transaction response for a two-way method with error syntax.
/// Wraps the body in a result union set to ordinal 1 (Ok) or 2 (Err).
pub fn encode_response_result<T: Body, E: ErrorType>(
    header: TransactionHeader,
    result: Result<T, E>,
) -> Result<Vec<u8>, Error>
where
    for<'a> <<T as Body>::MarkerInResultUnion as ValueTypeMarker>::Borrowed<'a>:
        Encode<T::MarkerInResultUnion, NoHandleResourceDialect>,
    for<'a> <<E as ErrorType>::Marker as ValueTypeMarker>::Borrowed<'a>:
        Encode<E::Marker, NoHandleResourceDialect>,
{
    encode::<FlexibleResultType<T::MarkerInResultUnion, E::Marker>>(
        header,
        FlexibleResult::new(match result {
            Ok(ref body) => Ok(T::MarkerInResultUnion::borrow(body)),
            Err(ref e) => Err(E::Marker::borrow(e)),
        }),
    )
}

/// Encodes a FIDL transaction response for a flexible two-way method without
/// error syntax. Wraps the body in a result union set to ordinal 1 (success).
pub fn encode_response_flexible<T: Body>(
    header: TransactionHeader,
    body: T,
) -> Result<Vec<u8>, Error>
where
    for<'a> <<T as Body>::MarkerInResultUnion as ValueTypeMarker>::Borrowed<'a>:
        Encode<T::MarkerInResultUnion, NoHandleResourceDialect>,
{
    encode::<FlexibleType<T::MarkerInResultUnion>>(
        header,
        Flexible::Ok(T::MarkerInResultUnion::borrow(&body)),
    )
}

/// Encodes a FIDL transaction response for a flexible two-way method,
/// for use in an open protocol when the method is unknown to the server.
pub fn encode_response_flexible_unknown(header: TransactionHeader) -> Result<Vec<u8>, Error> {
    encode::<FlexibleType<EmptyStruct>>(
        header,
        Flexible::<()>::FrameworkErr(FrameworkErr::UnknownMethod),
    )
}

/// Decodes a FIDL transaction message body (request or response).
/// Assumes `header` and `body` come from `decode_transaction_header`.
/// Use this for one-way methods, events, and two-way method requests.
/// For two-way method responses:
/// - use `decode_response_strict_result` if the method is strict and has error syntax.
/// - use `decode_response_flexible_result` if the method is flexible and has error syntax.
/// - use `decode_response_flexible` if the method is flexible (no error syntax).
/// - use this method otherwise.
pub fn decode_message<T: Body>(header: TransactionHeader, body: &[u8]) -> Result<T, Error>
where
    <T::MarkerAtTopLevel as TypeMarker>::Owned:
        Decode<T::MarkerAtTopLevel, NoHandleResourceDialect>,
{
    decode::<T::MarkerAtTopLevel>(header, body)
}

/// Decodes a FIDL response body for a flexible two-way method with error syntax.
/// Assumes `header` and `body` come from `decode_transaction_header`.
pub fn decode_response_strict_result<T: Body, E: ErrorType>(
    header: TransactionHeader,
    body: &[u8],
) -> Result<Result<T, E>, Error>
where
    <T::MarkerInResultUnion as TypeMarker>::Owned:
        Decode<T::MarkerInResultUnion, NoHandleResourceDialect>,
{
    decode::<ResultType<T::MarkerInResultUnion, E::Marker>>(header, body)
}

/// Return type for functions that decode flexible responses.
pub enum MaybeUnknown<T> {
    /// The server replied normally.
    Known(T),
    /// The server did not recognize the method ordinal.
    Unknown,
}

/// Decodes a FIDL response body for a flexible two-way method without error syntax.
/// Assumes `header` and `body` come from `decode_transaction_header`.
pub fn decode_response_flexible<T: Body>(
    header: TransactionHeader,
    body: &[u8],
) -> Result<MaybeUnknown<T>, Error>
where
    <T::MarkerInResultUnion as TypeMarker>::Owned:
        Decode<T::MarkerInResultUnion, NoHandleResourceDialect>,
{
    match decode::<FlexibleType<T::MarkerInResultUnion>>(header, body)? {
        Flexible::Ok(value) => Ok(MaybeUnknown::Known(value)),
        Flexible::FrameworkErr(err) => match err {
            FrameworkErr::UnknownMethod => Ok(MaybeUnknown::Unknown),
        },
    }
}

/// Decodes a FIDL response body for a flexible two-way method with error syntax.
/// Assumes `header` and `body` come from `decode_transaction_header`.
pub fn decode_response_flexible_result<T: Body, E: ErrorType>(
    header: TransactionHeader,
    body: &[u8],
) -> Result<MaybeUnknown<Result<T, E>>, Error>
where
    <T::MarkerInResultUnion as TypeMarker>::Owned:
        Decode<T::MarkerInResultUnion, NoHandleResourceDialect>,
{
    match decode::<FlexibleResultType<T::MarkerInResultUnion, E::Marker>>(header, body)? {
        FlexibleResult::Ok(value) => Ok(MaybeUnknown::Known(Ok(value))),
        FlexibleResult::DomainErr(err) => Ok(MaybeUnknown::Known(Err(err))),
        FlexibleResult::FrameworkErr(err) => match err {
            FrameworkErr::UnknownMethod => Ok(MaybeUnknown::Unknown),
        },
    }
}

fn encode<T: TypeMarker>(
    header: TransactionHeader,
    body: impl Encode<T, NoHandleResourceDialect>,
) -> Result<Vec<u8>, Error> {
    let msg = TransactionMessage { header, body };
    let mut combined_bytes = Vec::<u8>::new();
    let mut handles = Vec::new();
    Encoder::encode::<TransactionMessageType<T>>(&mut combined_bytes, &mut handles, msg)?;
    debug_assert!(handles.is_empty(), "value type contains handles");
    Ok(combined_bytes)
}

fn decode<T: TypeMarker>(header: TransactionHeader, body: &[u8]) -> Result<T::Owned, Error>
where
    T::Owned: Decode<T, NoHandleResourceDialect>,
{
    let mut output = new_empty!(T);
    Decoder::decode_into::<T>(&header, body, &mut [], &mut output)?;
    Ok(output)
}
