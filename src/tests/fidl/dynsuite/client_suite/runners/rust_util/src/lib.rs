// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_clientsuite::FidlErrorKind;

/// Classify a [`fidl::Error`] as a [`FidlErrorKind`] that can be returned in a
/// dynsuite client test.
pub fn classify_error(error: fidl::Error) -> FidlErrorKind {
    match error {
        fidl::Error::InvalidBoolean
        | fidl::Error::InvalidHeader
        | fidl::Error::IncompatibleMagicNumber(_)
        | fidl::Error::UnsupportedWireFormatVersion
        | fidl::Error::InvalidResponseOrdinal { .. }
        | fidl::Error::Invalid
        | fidl::Error::OutOfRange { .. }
        | fidl::Error::OutOfHandles
        | fidl::Error::ExtraBytes
        | fidl::Error::ExtraHandles
        | fidl::Error::NonZeroPadding { .. }
        | fidl::Error::MaxRecursionDepth
        | fidl::Error::NotNullable
        | fidl::Error::UnexpectedNullRef
        | fidl::Error::Utf8Error
        | fidl::Error::InvalidBitsValue
        | fidl::Error::InvalidEnumValue
        | fidl::Error::UnknownUnionTag
        | fidl::Error::InvalidPresenceIndicator
        | fidl::Error::InvalidInlineBitInEnvelope
        | fidl::Error::InvalidInlineMarkerInEnvelope
        | fidl::Error::InvalidNumBytesInEnvelope
        | fidl::Error::InvalidHostHandle
        | fidl::Error::IncorrectHandleSubtype { .. }
        | fidl::Error::MissingExpectedHandleRights { .. } => FidlErrorKind::DecodingError,

        fidl::Error::UnknownOrdinal { .. }
        | fidl::Error::InvalidResponseTxid { .. }
        | fidl::Error::UnexpectedSyncResponse => FidlErrorKind::UnexpectedMessage,

        fidl::Error::UnsupportedMethod { .. } => FidlErrorKind::UnknownMethod,

        fidl::Error::ClientChannelClosed { .. } => FidlErrorKind::ChannelPeerClosed,

        _ => FidlErrorKind::OtherError,
    }
}

/// Returns the FIDL method name for a request/event enum value.
// TODO(https://fxbug.dev/42078541): Provide this in FIDL bindings.
pub fn method_name(request_or_event: &impl std::fmt::Debug) -> String {
    let mut string = format!("{:?}", request_or_event);
    let len = string.find('{').unwrap_or(string.len());
    let len = string[..len].trim_end().len();
    string.truncate(len);
    assert!(
        string.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "failed to parse the method name from {:?}",
        request_or_event
    );
    string
}
