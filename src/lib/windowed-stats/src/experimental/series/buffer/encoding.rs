// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Encodings and format descriptors.

use byteorder::WriteBytesExt;
use std::io;

use crate::experimental::series::buffer::RingBuffer;
use crate::experimental::series::interpolation::InterpolationKind;
use crate::experimental::series::statistic::{Aggregation, SerialStatistic};

/// Describes the encoding of a [`RingBuffer`] type.
///
/// `Encoding` types specify a compression and payload of a buffer and provide serialization for
/// the corresponding type and subtype descriptors, which are part of the larger buffer
/// serialization format.
///
/// [`RingBuffer`]: crate::experimental::series::buffer::RingBuffer
pub trait Encoding<A> {
    type Compression: Compression;

    const PAYLOAD: <Self::Compression as Compression>::Payload;

    fn serialize(mut write: impl io::Write) -> io::Result<()> {
        write.write_u8(<Self::Compression as Compression>::BUFFER_TYPE_DESCRIPTOR)?;
        write.write_u8(Self::PAYLOAD.buffer_subtype_descriptor())
    }
}

/// A buffer compression strategy.
pub trait Compression {
    type Payload: Payload;

    const BUFFER_TYPE_DESCRIPTOR: u8;
}

/// A buffer payload data type.
pub trait Payload: Copy {
    fn buffer_subtype_descriptor(self) -> u8;
}

pub mod compression {
    use crate::experimental::series::buffer::encoding::{Compression, payload};

    #[derive(Debug)]
    pub enum Uncompressed {}

    impl Compression for Uncompressed {
        type Payload = payload::Uncompressed;

        const BUFFER_TYPE_DESCRIPTOR: u8 = 0;
    }

    /// Compression based _primarily_ on Simple8b and RLE.
    #[derive(Debug)]
    pub enum Simple8bRle {}

    impl Compression for Simple8bRle {
        type Payload = payload::Simple8bRle;

        const BUFFER_TYPE_DESCRIPTOR: u8 = 1;
    }

    /// Compression based _primarily_ on Delta, Simple8b, and RLE.
    #[derive(Debug)]
    pub enum DeltaSimple8bRle {}

    impl Compression for DeltaSimple8bRle {
        type Payload = payload::DeltaSimple8bRle;

        const BUFFER_TYPE_DESCRIPTOR: u8 = 2;
    }
}

pub mod payload {
    use crate::experimental::series::buffer::encoding::Payload;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    #[repr(u8)]
    pub enum Uncompressed {
        Float32 = 0,
    }

    impl Uncompressed {
        pub const fn buffer_subtype_descriptor(self) -> u8 {
            self as u8
        }
    }

    impl Payload for Uncompressed {
        fn buffer_subtype_descriptor(self) -> u8 {
            <Uncompressed>::buffer_subtype_descriptor(self)
        }
    }

    /// Payload for compression based primarily on Simple8b and RLE.
    ///
    /// This payload also describes compression using Zigzag encodings.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    #[repr(u8)]
    pub enum Simple8bRle {
        /// Signed integers using Zigzag encoding.
        Signed = 1,
        /// Unsigned integers.
        Unsigned = 0,
    }

    impl Simple8bRle {
        pub const fn buffer_subtype_descriptor(self) -> u8 {
            self as u8
        }
    }

    impl Payload for Simple8bRle {
        fn buffer_subtype_descriptor(self) -> u8 {
            <Simple8bRle>::buffer_subtype_descriptor(self)
        }
    }

    /// Payload for compression based on the delta-variant of Simple8b RLE.
    ///
    /// This payload also describes compression using Zigzag encodings.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    #[repr(u8)]
    pub enum DeltaSimple8bRle {
        /// Unsigned integers.
        Unsigned = 0,
        /// Signed integers using Zigzag encoding.
        Signed = 1,
        /// Unsigned integers but the diffs are signed and Zigzag encoded.
        UnsignedWithSignedDiff = 2,
    }

    impl DeltaSimple8bRle {
        pub const fn buffer_subtype_descriptor(self) -> u8 {
            self as u8
        }
    }

    impl Payload for DeltaSimple8bRle {
        fn buffer_subtype_descriptor(self) -> u8 {
            <DeltaSimple8bRle>::buffer_subtype_descriptor(self)
        }
    }
}

/// Serializes the type descriptors a `Statistic`'s associated buffer.
///
/// Given a `Statistic` type `F` and `Interpolation` type `P` with a configured buffer, this
/// function writes the associated buffer's type descriptors to the `Write` target `write`.
pub fn serialize_buffer_type_descriptors<F, P>(write: impl io::Write) -> io::Result<()>
where
    F: SerialStatistic<P>,
    P: InterpolationKind,
{
    type BufferEncoding<F, P> =
        <<F as SerialStatistic<P>>::Buffer as RingBuffer<Aggregation<F>>>::Encoding;

    <BufferEncoding<F, P> as Encoding<Aggregation<F>>>::serialize(write)
}
