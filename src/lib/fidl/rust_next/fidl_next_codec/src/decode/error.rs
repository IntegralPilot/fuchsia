// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use crate::ValidationError;

/// Errors that can be produced when decoding FIDL messages.
#[derive(Clone, Error, Debug)]
pub enum DecodeError {
    /// A required handle was absent
    #[error("required handle is absent")]
    RequiredHandleAbsent,

    /// A required value was absent
    #[error("required value is absent")]
    RequiredValueAbsent,

    /// A unit was set to a value other than 0
    #[error("`unit` field has an invalid value; expected 0, found {0}")]
    InvalidUnit(u8),

    /// A boolean was set to a value other than 0 or 1
    #[error("`bool` field has an invalid value; expected 0 or 1, found {0}")]
    InvalidBool(u8),

    /// A handle was set to a value other than 0 or u32::MAX
    #[error("handle has an invalid presence marker; expected 0 or u32::MAX, found {0}")]
    InvalidHandlePresence(u32),

    /// A pointer was set to a value other than 0 or u64::MAX
    #[error("pointer has an invalid presence marker; expected 0 or u64::MAX, found {0}.")]
    InvalidPointerPresence(u64),

    /// An envelope had an invalid size
    #[error("invalid envelope size; expected a multiple of 8, found {0}")]
    InvalidEnvelopeSize(u32),

    /// An enum had an invalid ordinal
    #[error("invalid enum ordinal; expected a valid ordinal, found {0}")]
    InvalidEnumOrdinal(i128),

    /// A union had an invalid ordinal
    #[error("invalid union ordinal; expected a valid ordinal, found {0}")]
    InvalidUnionOrdinal(usize),

    /// A bit set had an invalid set of bits
    #[error("invalid bits; expected a subset of {expected:b}, found {actual:b}")]
    InvalidBits {
        /// The expected set of bits
        expected: usize,
        /// The actual set of bits
        actual: usize,
    },

    /// An envelope was out-of-line, but the out-of-line data was too small
    #[error(
        "envelope has out-of-line data which is too small; expected more than 4 bytes out-of-line, \
        found {0} bytes"
    )]
    OutOfLineValueTooSmall(u32),

    /// An envelope had inline data that was too big
    #[error(
        "envelope has inline data which is too big; expected 4 bytes or fewer, found {0} bytes"
    )]
    InlineValueTooBig(usize),

    /// An envelope should always be inline, but it contained out-of-line data
    #[error("envelope should always be inline, but it contained {0} out-of-line bytes")]
    ExpectedInline(usize),

    /// An envelope consumed a different number of handles than it indicated in its metadata
    #[error(
        "envelope consumed a different number of handles than it claimed that it would; expected \
        {expected} to be consumed, found {actual} were consumed"
    )]
    IncorrectNumberOfHandlesConsumed {
        /// The number of handles the envelope was expected to consume
        expected: usize,
        /// The number of handles actually consumed by the envelope
        actual: usize,
    },

    /// Envelope flags other than the inline bit were set.
    #[error("invalid envelope flags: {0:#b}")]
    InvalidEnvelopeFlags(u16),

    /// An optional value was marked absent but its size was non-zero
    #[error("optional value is absent but has a non-zero size; expected 0, found {0}")]
    InvalidOptionalSize(u64),

    /// A vector had a length greater than its allowed limit
    #[error(
        "vector has a length greater than the allowed limit; expected no more than {limit} \
        elements, found {size} elements"
    )]
    VectorTooLong {
        /// The actual size of the vector
        size: u64,
        /// The maximum allowed size of the vector
        limit: u64,
    },

    /// A string contained non-UTF8 data
    #[error("string has non-UTF8 content; {0}")]
    InvalidUtf8(#[from] core::str::Utf8Error),

    /// A union was marked absent, but its envelope was not set to zero
    #[error("union is absent but has a non-zero envelope")]
    InvalidUnionEnvelope,

    /// A framework error contained an unrecognized error code.
    #[error("framework error has an unrecognized error code")]
    InvalidFrameworkError(i32),

    /// The decoder ran out of data before decoding finished
    #[error("reached the end of the buffer before decoding finished")]
    InsufficientData,

    /// The decoder ran out of handles before decoding finished
    #[error("consumed all handles before decoding finished")]
    InsufficientHandles,

    /// Attempted to decode a driver handle with an decoder that does not support them.
    #[error("cannot decode driver handles with this decoder")]
    DriverHandlesUnsupported,

    /// The next available handle was a zircon handle but expected a driver handle
    #[error("expected next handle to be a driver handle")]
    ExpectedDriverHandle,

    /// The next available handle was a driver handle but expected a zircon handle
    #[error("expected next handle to be a zircon handle")]
    ExpectedZirconHandle,

    /// Decoding finished without consuming all of the bytes
    #[error(
        "finished decoding before all bytes were consumed; completed with {num_extra} bytes left \
        over"
    )]
    ExtraBytes {
        /// The number of bytes left over after decoding finished
        num_extra: usize,
    },

    /// Decoding finished without consuming all of the handles
    #[error(
        "finished decoding before all handles were consumed; completed with {num_extra} handles \
        left over"
    )]
    ExtraHandles {
        /// The number of handles left over after decoding finished
        num_extra: usize,
    },

    /// Invalid empty struct
    #[error("invalid empty struct")]
    InvalidEmptyStruct,

    /// Invalid padding
    #[error("invalid padding")]
    InvalidPadding,

    /// Validation Failed
    #[error("validation failed")]
    Validation(#[from] ValidationError),
}
