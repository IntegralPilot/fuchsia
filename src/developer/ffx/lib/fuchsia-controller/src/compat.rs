// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fdomain_client::Error;

impl From<&Error> for FcTransportStatus {
    fn from(other: &Error) -> Self {
        match other {
            Error::SocketWrite(_) => Self::SOCKET_WRITE,
            Error::ChannelWrite(_) => Self::CHANNEL_WRITE,
            Error::FDomain(_) => Self::FDOMAIN,
            Error::Protocol(_) => Self::PROTOCOL,
            Error::ProtocolObjectTypeIncompatible => Self::PROTOCOL_OBJECT_TYPE_INCOMPATIBLE,
            Error::ProtocolRightsIncompatible => Self::PROTOCOL_RIGHTS_INCOMPATIBLE,
            Error::ProtocolSignalsIncompatible => Self::PROTOCOL_SIGNALS_INCOMPATIBLE,
            Error::ProtocolStreamEventIncompatible => Self::PROTOCOL_STREAM_EVENT_INCOMPATIBLE,
            Error::Transport(_) => Self::TRANSPORT,
            Error::ConnectionMismatch => Self::CONNECTION_MISMATCH,
            Error::StreamingAborted => Self::STREAMING_ABORTED,
        }
    }
}

impl From<Error> for FcTransportStatus {
    fn from(other: Error) -> Self {
        (&other).into()
    }
}

// Creates associated constants of TypeName of the form
// `pub const NAME: TypeName = TypeName(path::to::value);`
// and provides a private `assoc_const_name` method and a `Debug` implementation
// for the type based on `$name`.
// If multiple names match, the first will be used in `name` and `Debug`.
#[macro_export]
macro_rules! assoc_values {
    ($typename:ident, [$($name:ident = $value:literal;)*]) => {
        #[allow(non_upper_case_globals)]
        impl $typename {
            $(
                pub const $name: $typename = $typename($value);
            )*

            fn assoc_const_name(&self) -> Option<&'static str> {
                match self.0 {
                    $(
                        $value => Some(stringify!($name)),
                    )*
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Debug for $typename {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(concat!(stringify!($typename), "("))?;
                match self.assoc_const_name() {
                    Some(name) => f.write_str(&name)?,
                    None => ::std::fmt::Debug::fmt(&self.0, f)?,
                }
                f.write_str(")")
            }
        }
    }
}

/// The status returned to the caller beyond the FFI.
#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct FcTransportStatus(i32);

impl FcTransportStatus {
    pub fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    pub fn into_raw(&self) -> i32 {
        self.0
    }
}

// From the fuchsia-controller header.
// LINT.IfChange(fc_status)
assoc_values!(FcTransportStatus, [
    OK                                 =  0;
    SOCKET_WRITE                       = -1;
    CHANNEL_WRITE                      = -2;
    FDOMAIN                            = -3;
    PROTOCOL                           = -4;
    PROTOCOL_OBJECT_TYPE_INCOMPATIBLE  = -5;
    PROTOCOL_RIGHTS_INCOMPATIBLE       = -6;
    PROTOCOL_SIGNALS_INCOMPATIBLE      = -7;
    PROTOCOL_STREAM_EVENT_INCOMPATIBLE = -8;
    TRANSPORT                          = -9;
    CONNECTION_MISMATCH                = -10;
    STREAMING_ABORTED                  = -11;
    INVALID_ARGS                       = -44444;
    NOT_SUPPORTED                      = -55555;
    NOT_FOUND                          = -66666;
    BUFFER_TOO_SMALL                   = -77777;
    SHOULD_WAIT                        = -88888;
    INTERNAL                           = -99999;
    INTERRUPTED                        = -100000;
]);
// LINT.ThenChange(//src/developer/ffx/lib/fuchsia-controller/cpp/fuchsia_controller_internal/fuchsia_controller.h)

#[derive(Copy, Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
#[repr(transparent)]
pub struct FcErrorPayloadType(i32);

impl FcErrorPayloadType {
    pub fn into_raw(&self) -> i32 {
        self.0
    }
}

assoc_values!(FcErrorPayloadType, [
    NONE   = 0;
    STRING = 1;
    FIDL   = 2;
]);
