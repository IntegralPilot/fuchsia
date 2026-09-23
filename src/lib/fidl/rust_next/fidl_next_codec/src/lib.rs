// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Encoding and decoding support for FIDL.
//!
//! This crate provides a number of types and traits related to encoding and decoding FIDL types:
//!
//! ## Encoding
//!
//! Here, "encoding" refers to the process of converting a Rust value to an encoded FIDL message.
//! This process is captured in the [`Encode`] trait, which is parameterized over an _encoder_.
//!
//! ### Encodable types
//!
//! Encoding a type relies on two primary traits:
//!
//! - [`Encodable`] is the most fundamental trait for encodable types. It specifies the encoded form
//!   of the type with the associated [`Encoded`](Encodable::Encoded) type. That encoded form is its
//!   wire type which must implement [`Wire`].
//! - Whereas `Encodable` specifies the wire type something encodes into, [`Encode`] does the actual
//!   encoding. [`Encode`] is parameterized over the encoder used to encode the type.
//!
//! These traits are all you need to encode basic FIDL values. For more specialized encoding, some
//! types may implement these specialized encoding traits as well:
//!
//! #### Encoding optional types
//!
//! [`EncodableOption`] is a variant of [`Encodable`] which specifies how optional values of a type
//! should be encoded. Most FIDL types are [boxed](WireBox) when optional, which places the encoded
//! value out-of-line. However, optional strings, vectors, unions, and handles are not boxed when
//! optional. Instead, they have special optional wire types like [`WireOptionalString`] and
//! [`WireOptionalVector`] which are optimized to save space on the wire. Implementing
//! [`EncodableOption`] allows `Option`s of a type to be encoded.
//!
//! Implementing [`EncodableOption`] is optional, and only required if you encode an `Option` of
//! your type. The generated bindings will always generate an implementation of [`EncodableOption`]
//! for its types.
//!
//! [`EncodeOption`] is the variant of [`Encode`] for [`EncodableOption`].
//!
//! #### Encoding by reference
//!
//! [`EncodeRef`] is a variant of [`Encode`] for types which can be encoded by reference. [`Encode`]
//! encodes by value - it consumes the value being encoded. This allows [`Encode`] to encode
//! resource types, which cannot be duplicated because they contain resources like handles. By
//! contrast, [`EncodeRef`] does not consume the value being encoded, and can only be implemented
//! for non-resource types. [`EncodeRef`] allows for more flexible and efficient encoding for many
//! types.
//!
//! Optional types can also support encoding by reference with the [`EncodeOptionRef`] trait.
//!
//! ### Encoders
//!
//! Encodable types may only support encoding with specific kinds of encoders. They express these
//! constraints by bounding the `E` type when implementing [`Encode<E>`] to require that the encoder
//! implements some important traits. This crate provides the most fundamental encoder traits:
//!
//! - Most FIDL types require encoders to implement [`Encoder`] so that they can write out-of-line
//!   data. Strings, vectors, and tables are all examples of types which write out-of-line data.
//!   The [`EncoderExt`] extension trait provides useful methods for encoders when it is brought
//!   into scope (`use fidl_next::EncoderExt as _`).
//! - Types containing Fuchsia handles can only be encoded by encoders which implement
//!   [`HandleEncoder`](fuchsia::HandleEncoder).
//! - The [`InternalHandleEncoder`](encoder::InternalHandleEncoder) trait is an implementation
//!   detail. FIDL envelopes, which are used by tables and unions, may contain encoded types that
//!   the decoder doesn't recognize. If a decoder encounters an envelope containing a type it
//!   doesn't recognize, it needs to ignore the data and skip any handles it contained. To skip the
//!   correct number of handles, envelopes need to track the number of handles their value encoded.
//!   This is the case even if the envelope doesn't contain any types which contain handles.
//!   [`InternalHandleEncoder`](encoder::InternalHandleEncoder) provides this functionality to
//!   envelopes without requiring the encoder to actually support encoding handles.
//!
//! An implementation of [`Encoder`] is provided for `Vec<Chunk>`.
//!
//! ## Decoding
//!
//! Here, "decoding" has a very specific meaning. It refers to the process of validating and
//! rewriting a buffer of [`Chunk`]s to ensure it contains a valid [`Wire`] type. This definition is
//! narrow, and does not include converting a wire type to a natural type.
//!
//! The process of decoding is captured in the [`Decode`] trait. Like encoding, [`Decode`] is
//! parameterized over a _decoder_.
//!
//! ### Decodable types
//!
//! Types which implement [`Decode`] must first implement [`Wire`] to guarantee that their
//! representation conforms to the requirements of the FIDL wire format specification. Then, they
//! can specify how to validate and decode their wire form with the [`decode`](Decode::decode)
//! method.
//!
//! [`decode`](Decode::decode) needs to do three things to be correct:
//!
//! 1. Verify that the encoded bytes are valid for the type. For `bool`s, this means verifying that
//!    the encoded byte is either exactly 0 or exactly 1. If the encoded bytes are not valid for the
//!    type, `decode` **must** return an error.
//! 2. Reify pointers by decoding any out-of-line data and replacing presence indicators with the
//!    value of a pointer to that decoded data.
//! 3. Move resources from the decoder into the buffer. On the wire, handles are replaced with a
//!    presence indicator. They are transported separately by the transport because they require
//!    special handling.
//!
//! Note that [`decode`](Decode::decode) only manipulates data in-place, and only returns whether it
//! succeeded or failed.
//!
//! ### Decoders
//!
//! Like encoding, some types may only support encoding with specific types of decoders. We express
//! these constraints by bounding the `D` type when implementing [`Decode<D>`] to require that the
//! decoder implements some important traits:
//!
//! - Most FIDL types require decoders to implement [`Decoder`] so that they can decode out-of-line
//!   data. The [`DecoderExt`] extension trait provides useful methods for decoders when it is
//!   brought into scope (`use fidl_next::DecoderExt as _`).
//! - Types containing Fuchsia handles can only be decoded by decoders which implement
//!   [`HandleDecoder`](fuchsia::HandleDecoder).
//! - Like encoding, the [`InternalHandleDecoder`](decoder::InternalHandleDecoder) trait is an
//!   implementation detail for FIDL envelopes.
//!
//! An implementation of [`Decoder`] is provided for `&mut [Chunk]`.
//!
//! #### Committing
//!
//! Decoding a wire type can fail at any point, even after resources have been taken out of the
//! decoder. This presents a problem: partially-decoded values cannot be dropped, but may contain
//! resources that must be dropped.
//!
//! To solve this problem, taking a resource from a decoder happens in two phases:
//!
//! 1. While decoding the resource is copied from the decoder into the buffer. The resource is left
//!    in the decoder.
//! 2. After decoding finishes successfully, the decoder is [committed](Decoder::commit). Calling
//!    `commit` semantically completes moving the resources from the decoder into the buffer.
//!
//! If decoding fails before `commit` is called, the decoder remains responsible for dropping the
//! taken resources. After `commit` is called, the wire value is responsible for dropping the taken
//! resources.
//!
//! ## Wire types
//!
//! FIDL types which are used in-place without copying them out of the buffer implement [`Wire`] and
//! are called "wire types". [`Wire`] is an unsafe trait which bundles together the necessary
//! guarantees and functional machinery for wire types. The most important thing it does is promise
//! that the implementing type follows any layout requirements for FIDL's wire types.
//!
//! ### Primitives
//!
//! The FIDL wire specification requires "natural alignment" for wire primitives, which means that
//! wire primitives must have alignment equal to their size. A four-byte `int32` must be
//! four-aligned, and so may differ from Rust's native `i32` type. To accommodate these differences,
//! multibyte primitive types have special wire forms. Single-byte primitive types have the same
//! natural and wire types.
//!
//! | FIDL type     | Natural type  | Wire type     |
//! | ------------- | ------------- | ------------- |
//! | `bool`        | `bool`        | `bool`        |
//! | `int8`        | `i8`          | `i8`          |
//! | `int16`       | `i16`         | [`WireI16`]   |
//! | `int32`       | `i32`         | [`WireI32`]   |
//! | `int64`       | `i64`         | [`WireI64`]   |
//! | `uint8`       | `u8`          | `u8`          |
//! | `uint16`      | `u16`         | [`WireU16`]   |
//! | `uint32`      | `u32`         | [`WireU32`]   |
//! | `uint64`      | `u64`         | [`WireU64`]   |
//! | `float32`     | `f32`         | [`WireF32`]   |
//! | `float64`     | `f64`         | [`WireF64`]   |
//!
//! All wire primitives implement `Deref` and dereference to their native primitive types.
//!
//! ### Containers
//!
//! This crate provides wire types for containers supported by FIDL:
//!
//! | FIDL type     | Natural type  | Wire type         |
//! | ------------- | ------------- | ----------------- |
//! | `box<T>`      | `Option<T>`   | [`WireBox<T>`]    |
//! | `array<T, N>` | `[T; N]`      | `[T; N]`          |
//! | `vector<T>`   | `Vec<T>`      | [`WireVector<T>`] |
//! | `string`      | `String`      | [`WireString`]    |
//!
//! ### Lifetimes
//!
//! Wire types with out-of-line data may contain pointers to other parts of a decoded buffer, and so
//! cannot be allowed to outlive that decoded buffer. As a result, wire types are parameterized over
//! the lifetime of the decoder they are contained in (typically named `'de`). This lifetime isn't
//! important when reading data with wire types, but can impose important constraints when _moving_
//! wire types and converting them to natural types.
//!
//! After decoding, shared references to wire types can be obtained and used without any
//! restrictions. These shared references allow reading the data from wire types, and provide all of
//! the functionality required for handle-less FIDL.
//!
//! However, in order to move resources like handles out of wire types, wire values must be taken
//! from the decoder. Taking a wire type out of a decoder is an all-or-nothing operation - you must
//! take the entire decoded wire value from a decoder, and it must be moved or dropped before the
//! decoder can be moved or dropped. You can think of taking the wire value as sticking the decoder
//! in place until the wire value is converted to a natural type or dropped. This means that for
//! FIDL protocols, you must take the entire received FIDL message.
//!
//! When wire values are taken out of a decoder, they are parameterized over the lifetime of the
//! decoder (usually `'de`) to prevent the decoder from being dropped. These values can be treated
//! like ordinary Rust values.
//!
//! ### Conversion to natural types
//!
//! Natural types can support conversion from a wire type by implementing [`FromWire`]. This trait
//! has a [`from_wire`](FromWire::from_wire) method which parallels `From::from` and implements
//! conversion from some wire type. Like `Encode` and `Decode`, the [`FromWireOption`] variant
//! allows types to be converted from wire optional types. Optionally and for convenience only, wire
//! types can name a good default natural type to convert into with [`IntoNatural`].
//!
//! Natural types that can be converted from a reference to a wire type (i.e. without moving the
//! wire type) may implement [`FromWireRef`]. Similarly for options, the [`FromWireOptionRef`] trait
//! allows options to be converted from a reference to a wire type.

#![deny(
    future_incompatible,
    missing_docs,
    nonstandard_style,
    unused,
    warnings,
    clippy::all,
    clippy::alloc_instead_of_core,
    clippy::missing_safety_doc,
    clippy::std_instead_of_core,
    clippy::undocumented_unsafe_blocks,
    rustdoc::broken_intra_doc_links,
    rustdoc::missing_crate_level_docs
)]
#![forbid(unsafe_op_in_unsafe_fn)]

mod chunk;
mod convert;
mod copy_optimization;
mod decode;
mod decoded;
pub mod decoder;
#[cfg(feature = "driver")]
pub mod driver;
mod encode;
pub mod encoder;
#[cfg(feature = "fuchsia")]
pub mod fuchsia;
pub mod never;
mod slot;
mod validate;
pub mod wire;
mod wire_type;

pub use bitflags;
pub use munge::munge;

pub use self::chunk::*;
#[cfg(feature = "compat")]
pub use self::compat::*;
pub use self::convert::*;
pub use self::copy_optimization::*;
pub use self::decode::*;
pub use self::decoded::*;
pub use self::decoder::{Decoder, DecoderExt};
pub use self::encode::*;
pub use self::encoder::{Encoder, EncoderExt};
pub use self::slot::*;
pub use self::validate::*;
pub use self::wire_type::*;
