// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides standalone FIDL encoding and decoding.

use crate::encoding::{
    AtRestFlags, Context, Decode, Decoder, Depth, Encode, Encoder, GenericMessage,
    GenericMessageType, HandleFor, MAGIC_NUMBER_INITIAL, NoHandleResourceDialect, ProxyChannelFor,
    ResourceDialect, ResourceTypeMarker, TypeMarker, ValueTypeMarker, WireFormatVersion,
};
use crate::{Error, Result};

/// Marker trait implemented for FIDL non-resource structs, tables, and unions.
/// These can be used with the persistence API and standalone encoding/decoding API.
pub trait Persistable:
    TypeMarker<Owned = Self>
    + Decode<Self, NoHandleResourceDialect>
    + for<'a> ValueTypeMarker<Borrowed<'a> = &'a Self>
    + for<'a> ValueTypeMarker<Borrowed<'a>: Encode<Self, NoHandleResourceDialect>>
{
}

/// Marker trait implemented for FIDL resource structs, tables, and unions.
/// These can be used with the standalone encoding/decoding API, but not the persistence API.
pub trait Standalone<D>:
    TypeMarker<Owned = Self> + Decode<Self, D> + for<'a> ResourceTypeMarker<Borrowed<'a> = &'a mut Self>
{
}

/// Header for RFC-0120 persistent FIDL messages.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct WireMetadata {
    /// Must be zero.
    disambiguator: u8,
    /// Magic number indicating the message's wire format. Two sides with
    /// different magic numbers are incompatible with each other.
    magic_number: u8,
    /// "At rest" flags set for this message. MUST NOT be validated by bindings.
    at_rest_flags: [u8; 2],
    /// Reserved bytes. Must be zero.
    reserved: [u8; 4],
}

impl WireMetadata {
    /// Creates a new `WireMetadata` with a specific context and magic number.
    #[inline]
    fn new_full(context: Context, magic_number: u8) -> Self {
        WireMetadata {
            disambiguator: 0,
            magic_number,
            at_rest_flags: context.at_rest_flags().into(),
            reserved: [0; 4],
        }
    }

    /// Returns the header's flags as an `AtRestFlags` value.
    #[inline]
    fn at_rest_flags(&self) -> AtRestFlags {
        AtRestFlags::from_bits_truncate(u16::from_le_bytes(self.at_rest_flags))
    }

    /// Returns the context to use for decoding the message body associated with
    /// this header. During migrations, this is dependent on `self.flags()` and
    /// controls dynamic behavior in the read path.
    #[inline]
    fn decoding_context(&self) -> Context {
        Context { wire_format_version: WireFormatVersion::V2 }
    }

    /// Returns an error if this header has an incompatible wire format.
    #[inline]
    pub fn validate_wire_format(&self) -> Result<()> {
        if self.magic_number != MAGIC_NUMBER_INITIAL {
            return Err(Error::IncompatibleMagicNumber(self.magic_number));
        }
        if !self.at_rest_flags().contains(AtRestFlags::USE_V2_WIRE_FORMAT) {
            return Err(Error::UnsupportedWireFormatVersion);
        }
        Ok(())
    }
}

/// The default context for persistent encoding.
#[inline]
fn default_persistent_encode_context() -> Context {
    Context { wire_format_version: WireFormatVersion::V2 }
}

/// Encodes a FIDL object to bytes following RFC-0120. This only works on
/// non-resource structs, tables, and unions. See `unpersist` for the reverse.
pub fn persist<T: Persistable>(body: &T) -> Result<Vec<u8>> {
    let context = default_persistent_encode_context();
    let header = WireMetadata::new_full(context, MAGIC_NUMBER_INITIAL);
    let msg = GenericMessage { header, body };
    let mut bytes = Vec::<u8>::new();
    let mut handles = Vec::new();
    Encoder::encode_with_context::<GenericMessageType<WireMetadata, T>>(
        context,
        &mut bytes,
        &mut handles,
        msg,
    )?;
    debug_assert!(handles.is_empty(), "value type contains handles");
    Ok(bytes)
}

/// Decodes a FIDL object from bytes following RFC-0120. Must be a non-resource
/// struct, table, or union. See `persist` for the reverse.
pub fn unpersist<T: Persistable>(bytes: &[u8]) -> Result<T> {
    let (header, body_bytes) = decode_wire_metadata(bytes)?;
    let mut output = T::new_empty();
    Decoder::decode_with_context::<T>(header.decoding_context(), body_bytes, &mut [], &mut output)?;
    Ok(output)
}

/// Encodes a FIDL object to bytes and wire metadata following RFC-0120. Must be
/// a non-resource struct, table, or union.
pub fn standalone_encode_value<T: Persistable>(body: &T) -> Result<(Vec<u8>, WireMetadata)>
where
    for<'a> T::Borrowed<'a>: Encode<T, NoHandleResourceDialect>,
{
    // This helper is needed to convince rustc that &T implements Encode<T>.
    fn helper<T: ValueTypeMarker>(body: T::Borrowed<'_>) -> Result<(Vec<u8>, WireMetadata)>
    where
        for<'a> T::Borrowed<'a>: Encode<T, NoHandleResourceDialect>,
    {
        let context = default_persistent_encode_context();
        let metadata = WireMetadata::new_full(context, MAGIC_NUMBER_INITIAL);
        let mut bytes = Vec::<u8>::new();
        let mut handles = Vec::new();
        Encoder::<NoHandleResourceDialect>::encode_with_context::<T>(
            context,
            &mut bytes,
            &mut handles,
            body,
        )?;
        debug_assert!(handles.is_empty(), "value type contains handles");
        Ok((bytes, metadata))
    }
    helper::<T>(body)
}

/// Encodes a FIDL object to bytes, handles, and wire metadata following
/// RFC-0120. Must be a resource struct, table, or union.
#[allow(clippy::type_complexity)]
pub fn standalone_encode_resource<T: Standalone<D>, D: ResourceDialect>(
    mut body: T,
) -> Result<(Vec<u8>, Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>, WireMetadata)>
where
    for<'a> T::Borrowed<'a>: Encode<T, D>,
{
    let context = default_persistent_encode_context();
    let metadata = WireMetadata::new_full(context, MAGIC_NUMBER_INITIAL);
    let mut bytes = Vec::<u8>::new();
    let mut handles = Vec::<_>::new();
    Encoder::encode_with_context::<T>(context, &mut bytes, &mut handles, &mut body)?;
    Ok((bytes, handles, metadata))
}

/// Decodes a FIDL object from bytes and wire metadata following RFC-0120. Must
/// be a non-resource struct, table, or union.
pub fn standalone_decode_value<T: Persistable>(bytes: &[u8], metadata: &WireMetadata) -> Result<T> {
    let mut output = T::Owned::new_empty();
    Decoder::decode_with_context::<T>(metadata.decoding_context(), bytes, &mut [], &mut output)?;
    Ok(output)
}

/// Decodes a FIDL object from bytes, handles, and wire metadata following
/// RFC-0120. Must be a resource struct, table, or union.
pub fn standalone_decode_resource<T: Standalone<D>, D: ResourceDialect>(
    bytes: &[u8],
    handles: &mut [<D::Handle as HandleFor<D>>::HandleInfo],
    metadata: &WireMetadata,
) -> Result<T> {
    let mut output = <T as TypeMarker>::Owned::new_empty();
    Decoder::decode_with_context::<T>(metadata.decoding_context(), bytes, handles, &mut output)?;
    Ok(output)
}

/// Decodes the persistently stored header from a message.
/// Returns the header and a reference to the tail of the message.
fn decode_wire_metadata(bytes: &[u8]) -> Result<(WireMetadata, &[u8])> {
    let mut header = new_empty!(WireMetadata, NoHandleResourceDialect);
    let context = Context { wire_format_version: WireFormatVersion::V2 };
    let header_len = <WireMetadata as TypeMarker>::inline_size(context);
    if bytes.len() < header_len {
        return Err(Error::OutOfRange { expected: header_len, actual: bytes.len() });
    }
    let (header_bytes, body_bytes) = bytes.split_at(header_len);
    Decoder::<NoHandleResourceDialect>::decode_with_context::<WireMetadata>(
        context,
        header_bytes,
        &mut [],
        &mut header,
    )
    .map_err(|_| Error::InvalidHeader)?;
    header.validate_wire_format()?;
    Ok((header, body_bytes))
}

unsafe impl TypeMarker for WireMetadata {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        1
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        8
    }
}

impl ValueTypeMarker for WireMetadata {
    type Borrowed<'a> = &'a Self;
    fn borrow(value: &<Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value
    }
}

unsafe impl<D: ResourceDialect> Encode<WireMetadata, D> for &WireMetadata {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<WireMetadata>(offset);
        unsafe {
            let buf_ptr = encoder.buf.as_mut_ptr().add(offset);
            (buf_ptr as *mut WireMetadata).write_unaligned(*self);
        }
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for WireMetadata {
    #[inline(always)]
    fn new_empty() -> Self {
        Self { disambiguator: 0, magic_number: 0, at_rest_flags: [0; 2], reserved: [0; 4] }
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        unsafe {
            let buf_ptr = decoder.buf.as_ptr().add(offset);
            let obj_ptr = self as *mut WireMetadata;
            std::ptr::copy_nonoverlapping(buf_ptr, obj_ptr as *mut u8, 8);
        }
        Ok(())
    }
}
