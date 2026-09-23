// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module contains types used to serialize/deserialize and verify delivery blobs into their
//! typed equivalents ([`DeliveryBlobHeader`], [`Type1Blob`], etc...).
//!
//! **WARNING**: Use caution when making changes to this file. All format changes for a given
//! delivery blob type **must** be backwards compatible, or a new type must be used.

use bitflags::bitflags;

use static_assertions::const_assert_eq;
use zerocopy::byteorder::{LE, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::{DeliveryBlobError, DeliveryBlobHeader, DeliveryBlobType, Type1Blob, Type3Blob};

/// Delivery blob magic number (0xfc1ab10b or "Fuchsia Blob" in big-endian).
const DELIVERY_BLOB_MAGIC: [u8; 4] = [0xfc, 0x1a, 0xb1, 0x0b];

// Binary format compatibility checks:
const_assert_eq!(std::mem::size_of::<SerializedHeader>(), 12);
const_assert_eq!(
    std::mem::size_of::<SerializedType1Blob>(),
    std::mem::size_of::<SerializedHeader>() + 16
);
const_assert_eq!(
    std::mem::size_of::<SerializedType3Blob>(),
    std::mem::size_of::<SerializedHeader>() + 16
);

bitflags! {
    /// Type 1 delivery blob flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct SerializedType1Flags : u32 {
        const IS_COMPRESSED = 0x00000001;
        const VALID_FLAGS_MASK = Self::IS_COMPRESSED.bits();
    }
}

impl From<&crate::DeliveryBlob> for SerializedType1Flags {
    fn from(value: &crate::DeliveryBlob) -> Self {
        if value.is_compressed {
            SerializedType1Flags::IS_COMPRESSED
        } else {
            SerializedType1Flags::empty()
        }
    }
}

impl From<&Type1Blob> for SerializedType1Flags {
    fn from(value: &Type1Blob) -> Self {
        SerializedType1Flags::from(&crate::DeliveryBlob::from(*value))
    }
}

impl From<&crate::Type2Blob> for SerializedType1Flags {
    fn from(value: &crate::Type2Blob) -> Self {
        SerializedType1Flags::from(&crate::DeliveryBlob::from(*value))
    }
}

/// Serialized header of an RFC 0207 compliant delivery blob.
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct SerializedHeader {
    magic: [u8; 4],
    delivery_type: U32<LE>,
    header_length: U32<LE>,
}

impl SerializedHeader {
    pub fn decode(&self) -> Result<DeliveryBlobHeader, DeliveryBlobError> {
        if self.magic != DELIVERY_BLOB_MAGIC {
            return Err(DeliveryBlobError::BadMagic);
        }
        Ok(DeliveryBlobHeader {
            delivery_type: self.delivery_type.get().try_into()?,
            header_length: self.header_length.get(),
        })
    }
}

impl From<&DeliveryBlobHeader> for SerializedHeader {
    fn from(value: &DeliveryBlobHeader) -> Self {
        Self {
            magic: DELIVERY_BLOB_MAGIC,
            delivery_type: Into::<u32>::into(value.delivery_type).into(),
            header_length: value.header_length.into(),
        }
    }
}

/// Serialized header + metadata of a Type 1 or Type 2 delivery blob. Use [`Type1Blob::parse`] or
/// [`Type2Blob::parse`] or [`DeliveryBlob::parse`] to deserialize and validate a delivery blob as
/// opposed to deserializing this struct directly.
///
/// **WARNING**: Changes to this format must be done in a backwards compatible manner, or a new
/// delivery blob type should be created. This format should be considered an implementation detail,
/// and not relied on outside of storage-owned components.
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct SerializedType1Blob {
    // Header:
    header: SerializedHeader,
    // Metadata:
    payload_length: U64<LE>,
    checksum: U32<LE>,
    flags: U32<LE>,
}

impl From<crate::DeliveryBlob> for SerializedType1Blob {
    fn from(value: crate::DeliveryBlob) -> Self {
        let serialized = Self {
            header: (&value.header).into(),
            payload_length: (value.payload_length as u64).into(),
            checksum: Default::default(), // Calculated below.
            flags: SerializedType1Flags::from(&value).bits().into(),
        };

        Self { checksum: serialized.checksum().into(), ..serialized }
    }
}

impl From<Type1Blob> for SerializedType1Blob {
    fn from(value: Type1Blob) -> Self {
        crate::DeliveryBlob::from(value).into()
    }
}

impl From<crate::Type2Blob> for SerializedType1Blob {
    fn from(value: crate::Type2Blob) -> Self {
        crate::DeliveryBlob::from(value).into()
    }
}

impl SerializedType1Blob {
    pub fn checksum(&self) -> u32 {
        // Create a copy of the serialized blob but with the checksum zeroed.
        let header = Self { checksum: 0.into(), ..*self };
        crate::CRC_32.checksum(header.as_bytes())
    }

    /// Decode and verify this serialized delivery blob.
    pub fn decode(&self) -> Result<crate::DeliveryBlob, DeliveryBlobError> {
        // Validate checksum before other integrity checks.
        if self.checksum.get() != self.checksum() {
            return Err(DeliveryBlobError::IntegrityError);
        }
        // Validate header.
        let header: DeliveryBlobHeader = self.header.decode()?;
        if header.delivery_type != DeliveryBlobType::Type1
            && header.delivery_type != DeliveryBlobType::Type2
        {
            return Err(DeliveryBlobError::InvalidType);
        }
        if header.header_length != Type1Blob::HEADER.header_length {
            return Err(DeliveryBlobError::IntegrityError);
        }
        // Validate and decode remaining metadata fields.
        let payload_length = self.payload_length.get() as usize;
        let flags = SerializedType1Flags::from_bits(self.flags.get())
            .ok_or(DeliveryBlobError::IntegrityError)?;

        Ok(crate::DeliveryBlob {
            header,
            payload_length,
            is_compressed: flags.contains(SerializedType1Flags::IS_COMPRESSED),
        })
    }
}

bitflags! {
    /// Type 3 delivery blob flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct SerializedType3Flags : u32 {
        const IS_COMPRESSED = 0x00000001;
        const VALID_FLAGS_MASK = Self::IS_COMPRESSED.bits();
    }
}

impl From<&Type3Blob> for SerializedType3Flags {
    fn from(value: &Type3Blob) -> Self {
        if value.is_compressed {
            SerializedType3Flags::IS_COMPRESSED
        } else {
            SerializedType3Flags::empty()
        }
    }
}

/// Serialized header + metadata of a Type 3 delivery blob. Use [`Type3Blob::parse`] to deserialize
/// and validate a Type 3 delivery blob as opposed to deserializing this struct directly.
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C)]
pub(crate) struct SerializedType3Blob {
    // Header:
    header: SerializedHeader,
    // Metadata:
    payload_length: U64<LE>,
    checksum: U32<LE>,
    flags: U32<LE>,
}

impl From<Type3Blob> for SerializedType3Blob {
    fn from(value: Type3Blob) -> Self {
        let serialized = Self {
            header: (&value.header).into(),
            payload_length: (value.payload_length as u64).into(),
            checksum: Default::default(), // Calculated below.
            flags: SerializedType3Flags::from(&value).bits().into(),
        };

        Self { checksum: serialized.checksum().into(), ..serialized }
    }
}

impl SerializedType3Blob {
    pub fn checksum(&self) -> u32 {
        // Create a copy of the serialized blob but with the checksum zeroed.
        let header = Self { checksum: 0.into(), ..*self };
        crate::CRC_32.checksum(header.as_bytes())
    }

    /// Decode and verify this serialized Type 3 delivery blob.
    pub fn decode(&self) -> Result<Type3Blob, DeliveryBlobError> {
        // Validate checksum before other integrity checks.
        if self.checksum.get() != self.checksum() {
            return Err(DeliveryBlobError::IntegrityError);
        }
        // Validate header.
        let header: DeliveryBlobHeader = self.header.decode()?;
        if header.delivery_type != DeliveryBlobType::Type3 {
            return Err(DeliveryBlobError::InvalidType);
        }
        if header.header_length != Type3Blob::HEADER.header_length {
            return Err(DeliveryBlobError::IntegrityError);
        }
        // Validate and decode remaining metadata fields.
        let payload_length = self.payload_length.get() as usize;
        let flags = SerializedType3Flags::from_bits(self.flags.get())
            .ok_or(DeliveryBlobError::IntegrityError)?;

        Ok(Type3Blob {
            header,
            payload_length,
            is_compressed: flags.contains(SerializedType3Flags::IS_COMPRESSED),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompressionMode;

    const TEST_DATA: &[u8] = &[1, 2, 3, 4];

    #[test]
    fn type_1_round_trip_uncompressed() {
        let delivery_blob = Type1Blob::generate(TEST_DATA, CompressionMode::Never);
        assert_eq!(
            delivery_blob.len(),
            std::mem::size_of::<SerializedType1Blob>() + TEST_DATA.len()
        );
        // We should be able to decode and verify the parsed data.
        let (metadata, payload) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        assert_eq!(metadata.header, Type1Blob::HEADER);
        assert_eq!(metadata.payload_length, TEST_DATA.len());
        assert_eq!(metadata.is_compressed, false);
        // Verify that the payload matches.
        assert_eq!(payload, TEST_DATA);
    }

    #[test]
    fn type_1_round_trip_empty() {
        let delivery_blob = Type1Blob::generate(&[], CompressionMode::Never);
        assert_eq!(delivery_blob.len(), std::mem::size_of::<SerializedType1Blob>());
        // We should be able to decode and verify the parsed data.
        let (metadata, payload) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        assert_eq!(metadata.header, Type1Blob::HEADER);
        assert_eq!(metadata.payload_length, 0);
        assert_eq!(metadata.is_compressed, false);
        assert!(payload.is_empty());
    }

    #[test]
    fn type_1_not_enough_data() {
        let delivery_blob = Type1Blob::generate(TEST_DATA, CompressionMode::Never);
        let not_enough_data = &delivery_blob[..std::mem::size_of::<SerializedType1Blob>() - 1];
        assert!(Type1Blob::parse(not_enough_data).unwrap().is_none());
    }

    #[test]
    fn type_1_bad_magic() {
        // Create a valid serialized Type 1 blob.
        let valid: SerializedType1Blob =
            Type1Blob { header: Type1Blob::HEADER, payload_length: 0, is_compressed: false }
                .try_into()
                .unwrap();
        assert!(Type1Blob::parse(valid.as_bytes()).is_ok());
        // Corrupt magic, recalculate checksum, and ensure we fail with the correct error.
        let mut has_corrupt_magic = SerializedType1Blob {
            header: SerializedHeader { magic: [0, 0, 0, 0], ..valid.header },
            ..valid
        };
        has_corrupt_magic.checksum = has_corrupt_magic.checksum().into();
        assert_eq!(
            Type1Blob::parse(has_corrupt_magic.as_bytes()).unwrap_err(),
            DeliveryBlobError::BadMagic
        );
    }

    #[test]
    fn type_1_invalid_type() {
        // We should fail to parse a Type 1 blob with the wrong type specified in the header.
        let has_invalid_type: SerializedType1Blob = Type1Blob {
            header: DeliveryBlobHeader {
                delivery_type: DeliveryBlobType::Reserved,
                header_length: 0,
            },
            payload_length: 0,
            is_compressed: false,
        }
        .try_into()
        .unwrap();
        assert_eq!(
            Type1Blob::parse(has_invalid_type.as_bytes()).unwrap_err(),
            DeliveryBlobError::InvalidType
        );
    }

    #[test]
    fn type_1_invalid_header_length() {
        // We should fail to parse a Type 1 blob with the wrong header length.
        let has_invalid_header_length: SerializedType1Blob = Type1Blob {
            header: DeliveryBlobHeader {
                delivery_type: DeliveryBlobType::Type1,
                header_length: Type1Blob::HEADER.header_length + 1,
            },
            payload_length: 0,
            is_compressed: false,
        }
        .try_into()
        .unwrap();
        assert_eq!(
            Type1Blob::parse(has_invalid_header_length.as_bytes()).unwrap_err(),
            DeliveryBlobError::IntegrityError
        );
    }

    #[test]
    fn type_1_verify_checksum() {
        // Verify that we calculate the correct checksum for a serialized Type 1 blob.
        let serialized: SerializedType1Blob = Type1Blob {
            header: Type1Blob::HEADER,
            payload_length: TEST_DATA.len(),
            is_compressed: false,
        }
        .try_into()
        .unwrap();
        assert!(serialized.decode().is_ok());
        assert_eq!(serialized.checksum.get(), serialized.checksum());
        // We should fail to parse a Type 1 blob with a corrupted checksum.
        let corrupted_checksum: u32 = !(serialized.checksum.get());
        let corrupted_blob =
            SerializedType1Blob { checksum: corrupted_checksum.into(), ..serialized };
        assert_eq!(
            Type1Blob::parse(corrupted_blob.as_bytes()).unwrap_err(),
            DeliveryBlobError::IntegrityError
        );
    }

    #[test]
    fn type_3_round_trip_uncompressed() {
        let delivery_blob = Type3Blob::generate(TEST_DATA, CompressionMode::Never);
        assert_eq!(
            delivery_blob.len(),
            std::mem::size_of::<SerializedType3Blob>() + TEST_DATA.len()
        );
        let (metadata, payload) = Type3Blob::parse(&delivery_blob).unwrap().unwrap();
        assert_eq!(metadata.header, Type3Blob::HEADER);
        assert_eq!(metadata.payload_length, TEST_DATA.len());
        assert_eq!(metadata.is_compressed, false);
        assert_eq!(payload, TEST_DATA);
    }

    #[test]
    fn type_3_verify_checksum() {
        let serialized: SerializedType3Blob = Type3Blob {
            header: Type3Blob::HEADER,
            payload_length: TEST_DATA.len(),
            is_compressed: false,
        }
        .try_into()
        .unwrap();
        assert!(serialized.decode().is_ok());
        assert_eq!(serialized.checksum.get(), serialized.checksum());
        let corrupted_checksum: u32 = !(serialized.checksum.get());
        let corrupted_blob =
            SerializedType3Blob { checksum: corrupted_checksum.into(), ..serialized };
        assert_eq!(
            Type3Blob::parse(corrupted_blob.as_bytes()).unwrap_err(),
            DeliveryBlobError::IntegrityError
        );
    }
}
