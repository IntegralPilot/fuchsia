// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library for creating, serializing, and deserializing RFC 0207 delivery blobs. For example, to
//! create a Type 1 delivery blob:
//!
//! ```
//! use delivery_blob::{CompressionMode, Type1Blob};
//! let merkle = "68d131bc271f9c192d4f6dcd8fe61bef90004856da19d0f2f514a7f4098b0737";
//! let data: Vec<u8> = vec![0xFF; 8192];
//! let payload: Vec<u8> = Type1Blob::generate(&data, CompressionMode::Attempt);
//! ```

use crate::compression::{ChunkedArchive, ChunkedArchiveOptions, ChunkedDecompressor};
use crate::format::{SerializedType1Blob, SerializedType3Blob};
use serde::{Deserialize, Serialize};
use static_assertions::assert_eq_size;
use thiserror::Error;
use zerocopy::{IntoBytes, Ref};

pub mod compression;
pub use compression::DataBuffer;
mod format;

/// Pre-computed CRC-32 ISO HDLC lookup table.
pub(crate) const CRC_32: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// This library assumes usize is large enough to hold a u64.
assert_eq_size!(usize, u64);

/// Generate a delivery blob of the specified `delivery_type` for `data` using default parameters.
pub fn generate(delivery_type: DeliveryBlobType, data: &[u8]) -> Vec<u8> {
    match delivery_type {
        DeliveryBlobType::Type1 => Type1Blob::generate(data, CompressionMode::Attempt),
        DeliveryBlobType::Type2 => Type2Blob::generate(data, CompressionMode::Attempt),
        DeliveryBlobType::Type3 => Type3Blob::generate(data, CompressionMode::Attempt),
        _ => panic!("Unsupported delivery blob type: {:?}", delivery_type),
    }
}

/// Generate a delivery blob of the specified `delivery_type` for `data` using default parameters
/// and write the generated blob to `writer`.
pub fn generate_to(
    delivery_type: DeliveryBlobType,
    data: &[u8],
    writer: impl std::io::Write,
) -> Result<(), std::io::Error> {
    match delivery_type {
        DeliveryBlobType::Type1 => Type1Blob::generate_to(data, CompressionMode::Attempt, writer),
        DeliveryBlobType::Type2 => Type2Blob::generate_to(data, CompressionMode::Attempt, writer),
        DeliveryBlobType::Type3 => Type3Blob::generate_to(data, CompressionMode::Attempt, writer),
        _ => panic!("Unsupported delivery blob type: {:?}", delivery_type),
    }
}

/// Returns the type of delivery blob represented by `data`.
/// Returns an error if `data` is too short to contain a valid header.
///
/// **Note**: Only the header portion of the delivery blob needs to be present
/// in `data` (the full payload is not required).
pub fn delivery_blob_type(data: &[u8]) -> Result<DeliveryBlobType, DecompressError> {
    let header = DeliveryBlobHeader::parse(data)?.ok_or(DecompressError::NeedMoreData)?;
    Ok(header.delivery_type)
}

/// Returns the decompressed size of `delivery_blob`, delivery blob type is auto detected.
pub fn decompressed_size(delivery_blob: &[u8]) -> Result<u64, DecompressError> {
    DeliveryBlob::decompressed_size(delivery_blob)
}

/// Returns the decompressed size of the delivery blob from `reader`.
pub fn decompressed_size_from_reader(
    mut reader: impl std::io::Read,
) -> Result<u64, DecompressError> {
    let mut buf = vec![];
    loop {
        let already_read = buf.len();
        let new_size = already_read + 4096;
        buf.resize(new_size, 0);
        let new_size = already_read + reader.read(&mut buf[already_read..new_size])?;
        if new_size == already_read {
            return Err(DecompressError::NeedMoreData);
        }
        buf.truncate(new_size);
        match decompressed_size(&buf) {
            Ok(size) => {
                return Ok(size);
            }
            Err(DecompressError::NeedMoreData) => {}
            Err(e) => {
                return Err(e);
            }
        }
    }
}

/// Decompress a delivery blob in `delivery_blob`, delivery blob type is auto detected.
pub fn decompress(delivery_blob: &[u8]) -> Result<Vec<u8>, DecompressError> {
    DeliveryBlob::decompress(delivery_blob)
}

/// Decompress a delivery blob in `delivery_blob`, and write the decompressed blob to `writer`,
/// delivery blob type is auto detected.
pub fn decompress_to(
    delivery_blob: &[u8],
    writer: impl std::io::Write,
) -> Result<(), DecompressError> {
    DeliveryBlob::decompress_to(delivery_blob, writer)
}

/// Calculate the merkle root digest of the decompressed `delivery_blob`, delivery blob type is auto
/// detected.
pub fn calculate_digest(delivery_blob: &[u8]) -> Result<fuchsia_merkle::Hash, DecompressError> {
    let mut writer = fuchsia_merkle::BufferedMerkleRootBuilder::default();
    let () = DeliveryBlob::decompress_to(delivery_blob, &mut writer)?;
    Ok(writer.complete())
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DeliveryBlobError {
    #[error("Invalid or unsupported delivery blob type.")]
    InvalidType,

    #[error("Delivery blob header has incorrect magic.")]
    BadMagic,

    #[error("Integrity/checksum or other validity checks failed.")]
    IntegrityError,
}

#[derive(Debug, Error)]
pub enum DecompressError {
    #[error("DeliveryBlob error")]
    DeliveryBlob(#[from] DeliveryBlobError),

    #[error("ChunkedArchive error")]
    ChunkedArchive(#[from] compression::ChunkedArchiveError),

    #[error("Need more data")]
    NeedMoreData,

    #[error("io error")]
    IoError(#[from] std::io::Error),
}

#[cfg(target_os = "fuchsia")]
impl From<DeliveryBlobError> for zx::Status {
    fn from(value: DeliveryBlobError) -> Self {
        match value {
            // Unsupported delivery blob type.
            DeliveryBlobError::InvalidType => zx::Status::NOT_SUPPORTED,
            // Potentially corrupted delivery blob.
            DeliveryBlobError::BadMagic | DeliveryBlobError::IntegrityError => {
                zx::Status::IO_DATA_INTEGRITY
            }
        }
    }
}

/// Typed header of an RFC 0207 compliant delivery blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryBlobHeader {
    pub delivery_type: DeliveryBlobType,
    pub header_length: u32,
}

impl DeliveryBlobHeader {
    /// Attempt to parse `data` as a delivery blob. On success, returns validated blob header.
    /// **WARNING**: This function does not verify that the payload is complete. Only the full
    /// header of a delivery blob are required to be present in `data`.
    pub fn parse(data: &[u8]) -> Result<Option<DeliveryBlobHeader>, DeliveryBlobError> {
        let Ok((serialized_header, _metadata_and_payload)) =
            Ref::<_, format::SerializedHeader>::from_prefix(data)
        else {
            return Ok(None);
        };
        serialized_header.decode().map(Some)
    }
}

/// Type of delivery blob.
///
/// **WARNING**: These constants are used when generating delivery blobs and should not be changed.
/// Non backwards-compatible changes to delivery blob formats should be made by creating a new type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u32)]
pub enum DeliveryBlobType {
    /// Reserved for internal use.
    Reserved = 0,
    /// Type 1 delivery blobs use zstd-chunked compression with level 14 and 32KiB chunk size.
    Type1 = 1,
    /// Type 2 delivery blobs use zstd-chunked compression with level 21 and 128KiB chunk size.
    Type2 = 2,
    /// Type 3 delivery blobs support the lz4-chunked compression format.
    /// NOTE: Type 3 delivery blobs are currently UNSTABLE / EXPERIMENTAL and subject to change.
    Type3 = 3,
}

impl TryFrom<u32> for DeliveryBlobType {
    type Error = DeliveryBlobError;
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            value if value == DeliveryBlobType::Reserved as u32 => Ok(DeliveryBlobType::Reserved),
            value if value == DeliveryBlobType::Type1 as u32 => Ok(DeliveryBlobType::Type1),
            value if value == DeliveryBlobType::Type2 as u32 => Ok(DeliveryBlobType::Type2),
            value if value == DeliveryBlobType::Type3 as u32 => Ok(DeliveryBlobType::Type3),
            _ => Err(DeliveryBlobError::InvalidType),
        }
    }
}

impl From<DeliveryBlobType> for u32 {
    fn from(value: DeliveryBlobType) -> Self {
        value as u32
    }
}

/// Mode specifying when a delivery blob should be compressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompressionMode {
    /// Never compress input, output uncompressed.
    Never,
    /// Compress input, output compressed if saves space, otherwise uncompressed.
    Attempt,
    /// Compress input, output compressed unconditionally (even if space is wasted).
    Always,
}

/// Untyped header + metadata fields of an RFC 0207 delivery blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeliveryBlob {
    pub header: DeliveryBlobHeader,
    pub payload_length: usize,
    pub is_compressed: bool,
}

impl DeliveryBlob {
    /// Attempt to parse `data` as a delivery blob. On success, returns validated blob info,
    /// and the remainder of `data` representing the blob payload.
    ///
    /// If `allow_type3` is false, Type 3 delivery blobs will return
    /// `DeliveryBlobError::InvalidType`.
    pub fn parse(
        data: &[u8],
        allow_type3: bool,
    ) -> Result<Option<(DeliveryBlob, &[u8])>, DeliveryBlobError> {
        let Some(header) = DeliveryBlobHeader::parse(data)? else {
            return Ok(None);
        };
        match header.delivery_type {
            DeliveryBlobType::Type1 | DeliveryBlobType::Type2 => {
                let Ok((serialized_header, payload)) =
                    Ref::<_, format::SerializedType1Blob>::from_prefix(data)
                else {
                    return Ok(None);
                };
                serialized_header.decode().map(|metadata| Some((metadata, payload)))
            }
            DeliveryBlobType::Type3 => {
                if !allow_type3 {
                    return Err(DeliveryBlobError::InvalidType);
                }
                let Ok((serialized_header, payload)) =
                    Ref::<_, format::SerializedType3Blob>::from_prefix(data)
                else {
                    return Ok(None);
                };
                serialized_header.decode().map(|metadata| Some((metadata.into(), payload)))
            }
            _ => Err(DeliveryBlobError::InvalidType),
        }
    }

    /// Return the decompressed size of the blob without decompressing it.
    pub fn decompressed_size(delivery_blob: &[u8]) -> Result<u64, DecompressError> {
        let (header, payload) =
            Self::parse(delivery_blob, true)?.ok_or(DecompressError::NeedMoreData)?;
        if !header.is_compressed {
            return Ok(header.payload_length as u64);
        }

        let (decoded_archive, _chunk_data) =
            compression::decode_archive(payload, header.payload_length)?
                .ok_or(DecompressError::NeedMoreData)?;
        Ok(decoded_archive.decompressed_size() as u64)
    }

    /// Decompress a delivery blob in `delivery_blob`.
    pub fn decompress(delivery_blob: &[u8]) -> Result<Vec<u8>, DecompressError> {
        let mut decompressed = vec![];
        decompressed.reserve(Self::decompressed_size(delivery_blob)? as usize);
        Self::decompress_to(delivery_blob, &mut decompressed)?;
        Ok(decompressed)
    }

    /// Decompress a delivery blob in `delivery_blob` to `writer`.
    pub fn decompress_to(
        delivery_blob: &[u8],
        mut writer: impl std::io::Write,
    ) -> Result<(), DecompressError> {
        let (header, payload) =
            Self::parse(delivery_blob, true)?.ok_or(DecompressError::NeedMoreData)?;
        if !header.is_compressed {
            return Ok(writer.write_all(payload)?);
        }

        let (decoded_archive, chunk_data) =
            compression::decode_archive(payload, header.payload_length)?
                .ok_or(DecompressError::NeedMoreData)?;
        let mut decompressor = ChunkedDecompressor::new(decoded_archive)?;
        let mut result = Ok(());
        let mut chunk_callback = |chunk: &[u8]| {
            if let Err(e) = writer.write_all(chunk) {
                result = Err(e.into());
            }
        };
        decompressor.update(chunk_data, &mut chunk_callback)?;
        result
    }
}

/// Header + metadata fields of a Type 1 blob.
///
/// **WARNING**: Outside of storage-owned components, this should only be used for informational
/// or debugging purposes. The contents of this struct should be considered internal implementation
/// details and are subject to change at any time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Type1Blob {
    pub header: DeliveryBlobHeader,
    pub payload_length: usize,
    pub is_compressed: bool,
}

impl From<DeliveryBlob> for Type1Blob {
    fn from(blob: DeliveryBlob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl From<Type1Blob> for DeliveryBlob {
    fn from(blob: Type1Blob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl Type1Blob {
    pub const HEADER: DeliveryBlobHeader = DeliveryBlobHeader {
        delivery_type: DeliveryBlobType::Type1,
        header_length: std::mem::size_of::<SerializedType1Blob>() as u32,
    };

    pub const CHUNKED_ARCHIVE_OPTIONS: ChunkedArchiveOptions = ChunkedArchiveOptions::V2 {
        chunk_alignment: fuchsia_merkle::BLOCK_SIZE,
        minimum_chunk_size: 32 * 1024,
        compression_level: 14,
    };

    /// Generate a Type 1 delivery blob for `data` using the specified `mode`.
    pub fn generate(data: &[u8], mode: CompressionMode) -> Vec<u8> {
        let mut delivery_blob: Vec<u8> = vec![];
        Self::generate_to(data, mode, &mut delivery_blob).unwrap();
        delivery_blob
    }

    /// Generate a Type 1 delivery blob for `data` using the specified `mode`. Writes delivery blob
    /// directly into `writer`.
    pub fn generate_to(
        data: &[u8],
        mode: CompressionMode,
        writer: impl std::io::Write,
    ) -> Result<(), std::io::Error> {
        generate_blob_to(Self::HEADER, Self::CHUNKED_ARCHIVE_OPTIONS, data, mode, writer)
    }

    /// Attempt to parse `data` as a Type 1 delivery blob. On success, returns validated blob info,
    /// and the remainder of `data` representing the blob payload.
    pub fn parse(data: &[u8]) -> Result<Option<(Type1Blob, &[u8])>, DeliveryBlobError> {
        match DeliveryBlob::parse(data, true)? {
            Some((blob, payload)) if blob.header.delivery_type == DeliveryBlobType::Type1 => {
                Ok(Some((blob.into(), payload)))
            }
            Some(_) => Err(DeliveryBlobError::InvalidType),
            None => Ok(None),
        }
    }
}

/// Header + metadata fields of a Type 2 blob.
///
/// **WARNING**: Outside of storage-owned components, this should only be used for informational
/// or debugging purposes. The contents of this struct should be considered internal implementation
/// details and are subject to change at any time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Type2Blob {
    pub header: DeliveryBlobHeader,
    pub payload_length: usize,
    pub is_compressed: bool,
}

impl From<DeliveryBlob> for Type2Blob {
    fn from(blob: DeliveryBlob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl From<Type2Blob> for DeliveryBlob {
    fn from(blob: Type2Blob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl Type2Blob {
    pub const HEADER: DeliveryBlobHeader = DeliveryBlobHeader {
        delivery_type: DeliveryBlobType::Type2,
        header_length: std::mem::size_of::<SerializedType1Blob>() as u32,
    };

    pub const CHUNKED_ARCHIVE_OPTIONS: ChunkedArchiveOptions = ChunkedArchiveOptions::V2 {
        chunk_alignment: fuchsia_merkle::BLOCK_SIZE,
        minimum_chunk_size: 128 * 1024,
        compression_level: 21,
    };

    /// Generate a Type 2 delivery blob for `data` using the specified `mode`.
    pub fn generate(data: &[u8], mode: CompressionMode) -> Vec<u8> {
        let mut delivery_blob: Vec<u8> = vec![];
        Self::generate_to(data, mode, &mut delivery_blob).unwrap();
        delivery_blob
    }

    /// Generate a Type 2 delivery blob for `data` using the specified `mode`. Writes delivery blob
    /// directly into `writer`.
    pub fn generate_to(
        data: &[u8],
        mode: CompressionMode,
        writer: impl std::io::Write,
    ) -> Result<(), std::io::Error> {
        generate_blob_to(Self::HEADER, Self::CHUNKED_ARCHIVE_OPTIONS, data, mode, writer)
    }

    /// Attempt to parse `data` as a Type 2 delivery blob. On success, returns validated blob info,
    /// and the remainder of `data` representing the blob payload.
    pub fn parse(data: &[u8]) -> Result<Option<(Type2Blob, &[u8])>, DeliveryBlobError> {
        match DeliveryBlob::parse(data, true)? {
            Some((blob, payload)) if blob.header.delivery_type == DeliveryBlobType::Type2 => {
                Ok(Some((blob.into(), payload)))
            }
            Some(_) => Err(DeliveryBlobError::InvalidType),
            None => Ok(None),
        }
    }
}

/// Header + metadata fields of a Type 3 blob.
///
/// **NOTE**: Type 3 delivery blobs are currently UNSTABLE / EXPERIMENTAL and subject to change.
///
/// **WARNING**: Outside of storage-owned components, this should only be used for informational
/// or debugging purposes. The contents of this struct should be considered internal implementation
/// details and are subject to change at any time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Type3Blob {
    pub header: DeliveryBlobHeader,
    pub payload_length: usize,
    pub is_compressed: bool,
}

impl From<DeliveryBlob> for Type3Blob {
    fn from(blob: DeliveryBlob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl From<Type3Blob> for DeliveryBlob {
    fn from(blob: Type3Blob) -> Self {
        Self {
            header: blob.header,
            payload_length: blob.payload_length,
            is_compressed: blob.is_compressed,
        }
    }
}

impl Type3Blob {
    pub const HEADER: DeliveryBlobHeader = DeliveryBlobHeader {
        delivery_type: DeliveryBlobType::Type3,
        header_length: std::mem::size_of::<SerializedType3Blob>() as u32,
    };

    pub const CHUNKED_ARCHIVE_OPTIONS: ChunkedArchiveOptions =
        ChunkedArchiveOptions::V3 { compression_algorithm: compression::CompressionAlgorithm::Lz4 };

    /// Generate a Type 3 delivery blob for `data` using the specified `mode`.
    pub fn generate(data: &[u8], mode: CompressionMode) -> Vec<u8> {
        let mut delivery_blob: Vec<u8> = vec![];
        Self::generate_to(data, mode, &mut delivery_blob).unwrap();
        delivery_blob
    }

    /// Generate a Type 3 delivery blob for `data` using the specified `mode`. Writes delivery blob
    /// directly into `writer`.
    pub fn generate_to(
        data: &[u8],
        mode: CompressionMode,
        mut writer: impl std::io::Write,
    ) -> Result<(), std::io::Error> {
        let compressed = match mode {
            CompressionMode::Attempt | CompressionMode::Always => {
                let compressed = ChunkedArchive::new(data, Self::CHUNKED_ARCHIVE_OPTIONS)
                    .expect("failed to compress data");
                if mode == CompressionMode::Always || compressed.serialized_size() <= data.len() {
                    Some(compressed)
                } else {
                    None
                }
            }
            CompressionMode::Never => None,
        };

        let payload_length =
            compressed.as_ref().map(|archive| archive.serialized_size()).unwrap_or(data.len());
        let header =
            Self { header: Self::HEADER, payload_length, is_compressed: compressed.is_some() };
        let serialized_header: SerializedType3Blob = header.into();
        writer.write_all(serialized_header.as_bytes())?;

        if let Some(archive) = compressed {
            archive.write(writer)?;
        } else {
            writer.write_all(data)?;
        }
        Ok(())
    }

    /// Attempt to parse `data` as a Type 3 delivery blob. On success, returns validated blob info,
    /// and the remainder of `data` representing the blob payload.
    pub fn parse(data: &[u8]) -> Result<Option<(Type3Blob, &[u8])>, DeliveryBlobError> {
        let Ok((serialized_header, payload)) = Ref::<_, SerializedType3Blob>::from_prefix(data)
        else {
            return Ok(None);
        };
        serialized_header.decode().map(|metadata| Some((metadata, payload)))
    }
}

fn generate_blob_to(
    header_info: DeliveryBlobHeader,
    options: ChunkedArchiveOptions,
    data: &[u8],
    mode: CompressionMode,
    mut writer: impl std::io::Write,
) -> Result<(), std::io::Error> {
    let compressed = match mode {
        CompressionMode::Attempt | CompressionMode::Always => {
            let compressed = ChunkedArchive::new(data, options).expect("failed to compress data");
            if mode == CompressionMode::Always || compressed.serialized_size() <= data.len() {
                Some(compressed)
            } else {
                None
            }
        }
        CompressionMode::Never => None,
    };

    let payload_length =
        compressed.as_ref().map(|archive| archive.serialized_size()).unwrap_or(data.len());
    let blob =
        DeliveryBlob { header: header_info, payload_length, is_compressed: compressed.is_some() };
    let serialized_header: SerializedType1Blob = blob.into();
    writer.write_all(serialized_header.as_bytes())?;

    if let Some(archive) = compressed {
        archive.write(writer)?;
    } else {
        writer.write_all(data)?;
    }
    Ok(())
}

pub const MINIMUM_HEADER_SIZE: u32 = Type1Blob::HEADER.header_length;

#[cfg(test)]
mod tests {

    use super::*;
    use rand::RngExt as _;

    const DATA_LEN: usize = 500_000;

    #[test]
    fn compression_mode_never() {
        let data: Vec<u8> = vec![0; DATA_LEN];
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Never);
        // Payload should be uncompressed and have the same size as the original input data.
        let (header, _) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        assert!(!header.is_compressed);
        assert_eq!(header.payload_length, data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn compression_mode_always() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Always);
        let (header, _) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        // Payload is not very compressible, so we expect it to be larger than the original.
        assert!(header.is_compressed);
        assert!(header.payload_length > data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn compression_mode_attempt_uncompressible() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        // Data is random and therefore shouldn't be very compressible.
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Attempt);
        let (header, _) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        assert!(!header.is_compressed);
        assert_eq!(header.payload_length, data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn compression_mode_attempt_compressible() {
        let data: Vec<u8> = vec![0; DATA_LEN];
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Attempt);
        let (header, _) = Type1Blob::parse(&delivery_blob).unwrap().unwrap();
        // Payload should be compressed and smaller than the original input.
        assert!(header.is_compressed);
        assert!(header.payload_length < data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn get_decompressed_size() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Always);
        assert_eq!(decompressed_size(&delivery_blob).unwrap(), DATA_LEN as u64);
        assert_eq!(decompressed_size_from_reader(&delivery_blob[..]).unwrap(), DATA_LEN as u64);
    }

    #[test]
    fn get_delivery_blob_type() {
        let delivery_blob = Type1Blob::generate(&[], CompressionMode::Never);
        assert_eq!(delivery_blob_type(&delivery_blob).unwrap(), DeliveryBlobType::Type1);
    }

    #[test]
    fn get_delivery_blob_type_truncated() {
        let delivery_blob = Type1Blob::generate(&[], CompressionMode::Never);
        // Truncate the blob to 2 bytes, which is too short to contain a valid header.
        assert!(matches!(
            delivery_blob_type(&delivery_blob[..2]).unwrap_err(),
            DecompressError::NeedMoreData
        ));
    }

    #[test]
    fn get_delivery_blob_type_corrupted() {
        // Provide enough bytes to bypass the length check (64 bytes), but fill them with garbage.
        let bad_data = vec![0xFF; 64];

        // This should fail to parse the header (usually returning BadMagic or InvalidType).
        assert!(matches!(
            delivery_blob_type(&bad_data).unwrap_err(),
            DecompressError::DeliveryBlob(_)
        ));
    }

    #[test]
    fn test_calculate_digest() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        let delivery_blob = Type1Blob::generate(&data, CompressionMode::Always);
        assert_eq!(
            calculate_digest(&delivery_blob).unwrap(),
            fuchsia_merkle::root_from_slice(&data)
        );
    }

    #[test]
    fn type_2_round_trip() {
        let data: Vec<u8> = vec![0x42; DATA_LEN];
        let delivery_blob = Type2Blob::generate(&data, CompressionMode::Attempt);
        let (header, _) = Type2Blob::parse(&delivery_blob).unwrap().unwrap();
        assert_eq!(header.header.delivery_type, DeliveryBlobType::Type2);
        assert!(header.is_compressed);
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn type_2_vs_type_1_chunk_size() {
        let data: Vec<u8> = vec![0x42; 256 * 1024];
        let blob_v1 = Type1Blob::generate(&data, CompressionMode::Always);
        let blob_v2 = Type2Blob::generate(&data, CompressionMode::Always);

        let (_, payload_v1) = DeliveryBlob::parse(&blob_v1, true).unwrap().unwrap();
        let (decoded_v1, _) =
            compression::decode_archive(payload_v1, payload_v1.len()).unwrap().unwrap();
        assert_eq!(decoded_v1.seek_table().len(), 8);

        let (_, payload_v2) = DeliveryBlob::parse(&blob_v2, true).unwrap().unwrap();
        let (decoded_v2, _) =
            compression::decode_archive(payload_v2, payload_v2.len()).unwrap().unwrap();
        assert_eq!(decoded_v2.seek_table().len(), 2);
    }

    #[test]
    fn type_3_compression_mode_never() {
        let data: Vec<u8> = vec![0; DATA_LEN];
        let delivery_blob = Type3Blob::generate(&data, CompressionMode::Never);
        let (header, _) = Type3Blob::parse(&delivery_blob).unwrap().unwrap();
        assert!(!header.is_compressed);
        assert_eq!(header.payload_length, data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn type_3_compression_mode_always() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        let delivery_blob = Type3Blob::generate(&data, CompressionMode::Always);
        let (header, _) = Type3Blob::parse(&delivery_blob).unwrap().unwrap();
        assert!(header.is_compressed);
        assert!(header.payload_length > data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn type_3_compression_mode_attempt_compressible() {
        let data: Vec<u8> = vec![0; DATA_LEN];
        let delivery_blob = Type3Blob::generate(&data, CompressionMode::Attempt);
        let (header, _) = Type3Blob::parse(&delivery_blob).unwrap().unwrap();
        assert!(header.is_compressed);
        assert!(header.payload_length < data.len());
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn type_3_get_decompressed_size_and_digest() {
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(DATA_LEN).collect()
        };
        let delivery_blob = generate(DeliveryBlobType::Type3, &data);
        assert_eq!(decompressed_size(&delivery_blob).unwrap(), DATA_LEN as u64);
        assert_eq!(
            calculate_digest(&delivery_blob).unwrap(),
            fuchsia_merkle::root_from_slice(&data)
        );
        assert_eq!(decompress(&delivery_blob).unwrap(), data);
    }

    #[test]
    fn test_delivery_blob_parse() {
        let data: Vec<u8> = vec![1, 2, 3, 4];
        let type1_blob = generate(DeliveryBlobType::Type1, &data);
        let type3_blob = generate(DeliveryBlobType::Type3, &data);

        assert!(DeliveryBlob::parse(&type1_blob, false).unwrap().is_some());
        assert!(DeliveryBlob::parse(&type1_blob, true).unwrap().is_some());

        assert_eq!(
            DeliveryBlob::parse(&type3_blob, false).unwrap_err(),
            DeliveryBlobError::InvalidType
        );
        assert!(DeliveryBlob::parse(&type3_blob, true).unwrap().is_some());
    }
}
