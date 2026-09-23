// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of chunked-compression library in Rust. Archives can be created by making a new
//! [`ChunkedArchive`] and serializing/writing it. An archive's header can be verified and seek
//! table decoded using [`decode_archive`].

use itertools::Itertools;
use rayon::prelude::*;
use std::ops::Range;
use thiserror::Error;
use zerocopy::byteorder::{LE, U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, Unaligned};

mod compression_algorithm;
pub use compression_algorithm::{
    CompressionAlgorithm, Compressor, Decompressor, ThreadLocalCompressor, ThreadLocalDecompressor,
};
mod compression_info;
pub use compression_info::{CompressionInfo, DataBuffer, StreamingDecompressor};

/// Validated chunk information from an archive. Compressed ranges are relative to the start of
/// compressed data (i.e. they start after the header and seek table).
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct ZstdError(pub usize);

impl std::fmt::Display for ZstdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = zstd::zstd_safe::get_error_name(self.0);
        let enum_code = unsafe { zstd::zstd_safe::zstd_sys::ZSTD_getErrorCode(self.0) };
        write!(f, "{:?} ({})", enum_code, msg)
    }
}

impl std::fmt::Debug for ZstdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

#[cfg(target_os = "fuchsia")]
impl From<ZstdError> for zx::Status {
    fn from(err: ZstdError) -> Self {
        use zstd::zstd_safe::zstd_sys::ZSTD_ErrorCode::*;
        let code = unsafe { zstd::zstd_safe::zstd_sys::ZSTD_getErrorCode(err.0) };
        match code {
            ZSTD_error_corruption_detected
            | ZSTD_error_checksum_wrong
            | ZSTD_error_literals_headerWrong
            | ZSTD_error_dictionary_corrupted
            | ZSTD_error_prefix_unknown => zx::Status::IO_DATA_INTEGRITY,

            ZSTD_error_version_unsupported
            | ZSTD_error_frameParameter_unsupported
            | ZSTD_error_parameter_unsupported => zx::Status::NOT_SUPPORTED,

            ZSTD_error_parameter_outOfBound
            | ZSTD_error_srcSize_wrong
            | ZSTD_error_dstSize_tooSmall => zx::Status::INVALID_ARGS,

            ZSTD_error_no_error
            | ZSTD_error_GENERIC
            | ZSTD_error_frameParameter_windowTooLarge
            | ZSTD_error_dictionary_wrong
            | ZSTD_error_dictionaryCreation_failed
            | ZSTD_error_parameter_combination_unsupported
            | ZSTD_error_tableLog_tooLarge
            | ZSTD_error_maxSymbolValue_tooLarge
            | ZSTD_error_maxSymbolValue_tooSmall
            | ZSTD_error_stabilityCondition_notRespected
            | ZSTD_error_stage_wrong
            | ZSTD_error_init_missing
            | ZSTD_error_memory_allocation
            | ZSTD_error_workSpace_tooSmall
            | ZSTD_error_dstBuffer_null
            | ZSTD_error_noForwardProgress_destFull
            | ZSTD_error_noForwardProgress_inputEmpty
            | ZSTD_error_frameIndex_tooLarge
            | ZSTD_error_seekableIO
            | ZSTD_error_dstBuffer_wrong
            | ZSTD_error_srcBuffer_wrong
            | ZSTD_error_sequenceProducer_failed
            | ZSTD_error_externalSequences_invalid
            | ZSTD_error_cannotProduce_uncompressedBlock
            | ZSTD_error_maxCode => zx::Status::INTERNAL,
        }
    }
}

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("Zstd error: {0}")]
    Zstd(ZstdError),
    #[error("LZ4 error: {0}")]
    Lz4(lz4::Error),
}

#[cfg(target_os = "fuchsia")]
impl From<&FormatError> for zx::Status {
    fn from(err: &FormatError) -> Self {
        match err {
            FormatError::Zstd(e) => zx::Status::from(*e),
            FormatError::Lz4(_) => zx::Status::IO_DATA_INTEGRITY,
        }
    }
}

// *NOTE*: Use caution when using the `#[source]` attribute or naming fields `source`. Some callers
// attempt to downcast library errors into the concrete type of the root cause.
// See https://docs.rs/thiserror/latest/thiserror/ for more information.
#[derive(Debug, Error)]
pub enum ChunkedArchiveError {
    #[error("Invalid or unsupported archive version.")]
    InvalidVersion,

    #[error("Archive header has incorrect magic.")]
    BadMagic,

    #[error("Integrity checks failed (e.g. incorrect CRC, inconsistent header fields).")]
    IntegrityError,

    #[error("Value is out of range or cannot be represented in specified type.")]
    OutOfRange,

    #[error("Error decompressing chunk {index}: {error}")]
    DecompressionError { index: usize, error: FormatError },

    #[error("Error compressing chunk {index}: {error}")]
    CompressionError { index: usize, error: FormatError },
}

/// Options for constructing a chunked archive.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ChunkedArchiveOptions {
    /// A chunked-compression V2 archive will be created.
    V2 {
        /// Chunked-compression V2 has a limit of 1023 chunks. If splitting the data up into
        /// `minimum_chunk_size`d chunks would exceed this limit then the chunk size increased by
        /// `chunk_alignment` until fewer than 1024 are required. `minimum_chunk_size` must be a
        /// multiple of `chunk_alignment`.
        minimum_chunk_size: usize,
        /// The chosen uncompressed chunk size must always be a multiple of this value.
        chunk_alignment: usize,
        /// The Zstd compression level to use when compressing chunks.
        compression_level: i32,
    },
    /// A chunked-compression V3 archive will be created.
    V3 {
        /// The compression algorithm to use to compress the chunks.
        compression_algorithm: CompressionAlgorithm,
    },
}

impl ChunkedArchiveOptions {
    const V2_VERSION: u16 = 2;
    const V2_MAX_CHUNKS: usize = 1023;

    const V3_VERSION: u16 = 3;
    const V3_MAX_CHUNKS: usize = u32::MAX as usize;
    const V3_CHUNK_SIZE: usize = 32 * 1024;
    const V3_ZSTD_COMPRESSION_LEVEL: i32 = 22;

    /// Which version of chunked-compression archive should be constructed.
    fn version(&self) -> u16 {
        match self {
            Self::V2 { .. } => Self::V2_VERSION,
            Self::V3 { .. } => Self::V3_VERSION,
        }
    }

    /// The compression algorithm to use to compress the chunks.
    fn compression_algorithm(&self) -> CompressionAlgorithm {
        match self {
            Self::V2 { .. } => CompressionAlgorithm::Zstd,
            Self::V3 { compression_algorithm } => *compression_algorithm,
        }
    }

    /// Calculate how large chunks must be for a given amount of data.
    fn chunk_size_for(&self, data_size: usize) -> usize {
        match self {
            Self::V2 { chunk_alignment, minimum_chunk_size: target_chunk_size, .. } => {
                if data_size <= (Self::V2_MAX_CHUNKS * target_chunk_size) {
                    *target_chunk_size
                } else {
                    let chunk_size = data_size.div_ceil(Self::V2_MAX_CHUNKS);
                    chunk_size.checked_next_multiple_of(*chunk_alignment).unwrap()
                }
            }
            Self::V3 { .. } => {
                assert!(
                    data_size.div_ceil(Self::V3_CHUNK_SIZE) <= Self::V3_MAX_CHUNKS,
                    "Chunked-compression V3 only supports data up to ~140TB"
                );
                Self::V3_CHUNK_SIZE
            }
        }
    }

    /// Constructs a compressor to compress chunks based on the specified options.
    pub fn compressor(&self) -> Compressor {
        match self {
            Self::V2 { compression_level, .. } => {
                let mut cctx = zstd::zstd_safe::CCtx::create();
                cctx.set_parameter(zstd::zstd_safe::CParameter::CompressionLevel(
                    *compression_level,
                ))
                .expect("setting the compression level should never fail");
                Compressor::Zstd(cctx)
            }
            Self::V3 { compression_algorithm: CompressionAlgorithm::Zstd } => {
                let mut cctx = zstd::zstd_safe::CCtx::create();
                cctx.set_parameter(zstd::zstd_safe::CParameter::CompressionLevel(
                    Self::V3_ZSTD_COMPRESSION_LEVEL,
                ))
                .expect("setting the compression level should never fail");
                Compressor::Zstd(cctx)
            }
            Self::V3 { compression_algorithm: CompressionAlgorithm::Lz4 } => {
                Compressor::Lz4 { compression_level: lz4::HcCompressionLevel::custom(12) }
            }
        }
    }

    /// Constructs a compressor object that uses a thread local compressor to compress chunks based
    /// on the specified options.
    pub fn thread_local_compressor(&self) -> ThreadLocalCompressor {
        match self {
            Self::V2 { compression_level, .. } => {
                ThreadLocalCompressor::Zstd { compression_level: *compression_level }
            }
            Self::V3 { compression_algorithm: CompressionAlgorithm::Zstd } => {
                ThreadLocalCompressor::Zstd { compression_level: Self::V3_ZSTD_COMPRESSION_LEVEL }
            }
            Self::V3 { compression_algorithm: CompressionAlgorithm::Lz4 } => {
                ThreadLocalCompressor::Lz4 {
                    compression_level: lz4::HcCompressionLevel::custom(12),
                }
            }
        }
    }

    /// Returns true if `version` is a valid chunked-compression version.
    fn is_valid_version(version: u16) -> bool {
        match version {
            Self::V2_VERSION => true,
            Self::V3_VERSION => true,
            _ => false,
        }
    }

    /// Returns the maximum number of chunks supported by the chunked-compression format at the
    /// specified version.
    fn max_chunks_for_version(version: u16) -> Result<usize, ChunkedArchiveError> {
        match version {
            Self::V2_VERSION => Ok(Self::V2_MAX_CHUNKS),
            Self::V3_VERSION => Ok(Self::V3_MAX_CHUNKS),
            _ => Err(ChunkedArchiveError::InvalidVersion),
        }
    }
}

/// Validated chunk information from an archive. Compressed ranges are relative to the start of
/// compressed data (i.e. they start after the header and seek table).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkInfo {
    pub decompressed_range: Range<usize>,
    pub compressed_range: Range<usize>,
}

impl ChunkInfo {
    fn from_entry(
        entry: &SeekTableEntry,
        header_length: usize,
    ) -> Result<Self, ChunkedArchiveError> {
        let decompressed_start = entry.decompressed_offset.get() as usize;
        let decompressed_size = entry.decompressed_size.get() as usize;
        let decompressed_range = decompressed_start
            ..decompressed_start
                .checked_add(decompressed_size)
                .ok_or(ChunkedArchiveError::OutOfRange)?;

        let compressed_offset = entry.compressed_offset.get() as usize;
        let compressed_start = compressed_offset
            .checked_sub(header_length)
            .ok_or(ChunkedArchiveError::IntegrityError)?;
        let compressed_size = entry.compressed_size.get() as usize;
        let compressed_range = compressed_start
            ..compressed_start
                .checked_add(compressed_size)
                .ok_or(ChunkedArchiveError::OutOfRange)?;

        Ok(Self { decompressed_range, compressed_range })
    }
}

/// Validated information from decoding an archive.
#[derive(Debug)]
pub struct DecodedArchive {
    compression_algorithm: CompressionAlgorithm,
    seek_table: Vec<ChunkInfo>,
}

impl DecodedArchive {
    /// The total size of decompressing all of the chunks in the archive.
    pub fn decompressed_size(&self) -> usize {
        self.seek_table.last().map_or(0, |entry| entry.decompressed_range.end)
    }

    pub fn seek_table(&self) -> &[ChunkInfo] {
        &self.seek_table
    }
}

/// Decodes a chunked archive header. Returns a `DecodedArchive` and any remaining bytes that are
/// part of the chunk data. Returns `Ok(None)` if `data` is not large enough to decode the archive
/// header & seek table.
pub fn decode_archive(
    data: &[u8],
    archive_length: usize,
) -> Result<Option<(DecodedArchive, /*archive_data*/ &[u8])>, ChunkedArchiveError> {
    match Ref::<_, ChunkedArchiveHeader>::from_prefix(data).map_err(Into::into) {
        Ok((header, data)) => header.decode_archive(data, archive_length as u64),
        Err(zerocopy::SizeError { .. }) => Ok(None), // Not enough data.
    }
}

/// Chunked archive header.
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C)]
struct ChunkedArchiveHeader {
    magic: [u8; 8],
    version: U16<LE>,
    // This field was added in V3 and should not be used if `version` is 2. Technically, this field
    // should be 0 in V2, Zstd has the value 0, and V2 always uses Zstd so accessing this field in
    // V2 should give the correct result.
    compression_algorithm: u8,
    reserved_0: u8,
    num_entries: U32<LE>,
    checksum: U32<LE>,
    reserved_1: U32<LE>,
    reserved_2: U64<LE>,
}

/// Chunked archive seek table entry.
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C)]
struct SeekTableEntry {
    decompressed_offset: U64<LE>,
    decompressed_size: U64<LE>,
    compressed_offset: U64<LE>,
    compressed_size: U64<LE>,
}

impl ChunkedArchiveHeader {
    const CHUNKED_ARCHIVE_MAGIC: [u8; 8] = [0x46, 0x9b, 0x78, 0xef, 0x0f, 0xd0, 0xb2, 0x03];
    const CHUNKED_ARCHIVE_CHECKSUM_OFFSET: usize = 16;

    fn new(
        seek_table: &[SeekTableEntry],
        options: ChunkedArchiveOptions,
    ) -> Result<Self, ChunkedArchiveError> {
        let header: ChunkedArchiveHeader = Self {
            magic: Self::CHUNKED_ARCHIVE_MAGIC,
            version: options.version().into(),
            compression_algorithm: options.compression_algorithm().into(),
            reserved_0: 0.into(),
            num_entries: TryInto::<u32>::try_into(seek_table.len())
                .or(Err(ChunkedArchiveError::OutOfRange))?
                .into(),
            checksum: 0.into(), // `checksum` is calculated below.
            reserved_1: 0.into(),
            reserved_2: 0.into(),
        };
        Ok(Self { checksum: header.checksum(seek_table).into(), ..header })
    }

    /// Calculate the checksum of the header + all seek table entries.
    fn checksum(&self, entries: &[SeekTableEntry]) -> u32 {
        let mut digest = crate::CRC_32.digest();
        digest.update(&self.as_bytes()[..Self::CHUNKED_ARCHIVE_CHECKSUM_OFFSET]);
        digest.update(
            &self.as_bytes()
                [Self::CHUNKED_ARCHIVE_CHECKSUM_OFFSET + self.checksum.as_bytes().len()..],
        );
        digest.update(entries.as_bytes());
        digest.finalize()
    }

    /// Calculate the total header length of an archive *including* all seek table entries.
    fn header_length(num_entries: usize) -> usize {
        std::mem::size_of::<ChunkedArchiveHeader>()
            + (std::mem::size_of::<SeekTableEntry>() * num_entries)
    }

    /// Validates the archive header and decodes the seek table.
    fn decode_archive(
        self,
        data: &[u8],
        archive_length: u64,
    ) -> Result<Option<(DecodedArchive, /*chunk_data*/ &[u8])>, ChunkedArchiveError> {
        // Deserialize seek table.
        let num_entries = self.num_entries.get() as usize;
        let Ok((entries, chunk_data)) =
            Ref::<_, [SeekTableEntry]>::from_prefix_with_elems(data, num_entries)
        else {
            return Ok(None);
        };
        let entries: &[SeekTableEntry] = Ref::into_ref(entries);

        // Validate archive header.
        if self.magic != Self::CHUNKED_ARCHIVE_MAGIC {
            return Err(ChunkedArchiveError::BadMagic);
        }
        let version = self.version.get();
        if !ChunkedArchiveOptions::is_valid_version(version) {
            return Err(ChunkedArchiveError::InvalidVersion);
        }
        if self.checksum.get() != self.checksum(entries) {
            return Err(ChunkedArchiveError::IntegrityError);
        }
        if entries.len() > ChunkedArchiveOptions::max_chunks_for_version(version)? {
            return Err(ChunkedArchiveError::IntegrityError);
        }
        let compression_algorithm = CompressionAlgorithm::try_from(self.compression_algorithm)?;

        // Validate seek table using invariants I0 through I5.

        // I0: The first seek table entry, if any, must have decompressed offset 0.
        if !entries.is_empty() && entries[0].decompressed_offset.get() != 0 {
            return Err(ChunkedArchiveError::IntegrityError);
        }

        // I1: The compressed offsets of all seek table entries must not overlap with the header.
        let header_length = Self::header_length(entries.len());
        if entries.iter().any(|entry| entry.compressed_offset.get() < header_length as u64) {
            return Err(ChunkedArchiveError::IntegrityError);
        }

        // I2: Each entry's decompressed offset must be equal to the end of the previous frame
        //     (i.e. to the previous frame's decompressed offset + length).
        for (prev, curr) in entries.iter().tuple_windows() {
            if (prev.decompressed_offset.get() + prev.decompressed_size.get())
                != curr.decompressed_offset.get()
            {
                return Err(ChunkedArchiveError::IntegrityError);
            }
        }

        // I3: Each entry's compressed offset must be greater than or equal to the end of the
        //     previous frame (i.e. to the previous frame's compressed offset + length).
        for (prev, curr) in entries.iter().tuple_windows() {
            if (prev.compressed_offset.get() + prev.compressed_size.get())
                > curr.compressed_offset.get()
            {
                return Err(ChunkedArchiveError::IntegrityError);
            }
        }

        // I4: Each entry must have a non-zero decompressed and compressed length.
        for entry in entries.iter() {
            if entry.decompressed_size.get() == 0 || entry.compressed_size.get() == 0 {
                return Err(ChunkedArchiveError::IntegrityError);
            }
        }

        // I5: Data referenced by each entry must fit within the specified file size.
        for entry in entries.iter() {
            let compressed_end = entry.compressed_offset.get() + entry.compressed_size.get();
            if compressed_end > archive_length {
                return Err(ChunkedArchiveError::IntegrityError);
            }
        }

        let seek_table = entries
            .iter()
            .map(|entry| ChunkInfo::from_entry(entry, header_length))
            .try_collect()?;
        Ok(Some((DecodedArchive { seek_table, compression_algorithm }, chunk_data)))
    }
}

/// In-memory representation of a compressed chunk.
pub struct CompressedChunk {
    /// Compressed data for this chunk.
    pub compressed_data: Vec<u8>,
    /// Size of this chunk when decompressed.
    pub decompressed_size: usize,
}

/// In-memory representation of a compressed chunked archive.
pub struct ChunkedArchive {
    /// Chunks this archive contains, in order. Right now we only allow creating archives with
    /// contiguous compressed and decompressed space.
    chunks: Vec<CompressedChunk>,
    /// Size used to chunk input when creating this archive. Last chunk may be smaller than this
    /// amount.
    chunk_size: usize,
    /// The options used to construct this archive.
    options: ChunkedArchiveOptions,
}

impl ChunkedArchive {
    /// Create a ChunkedArchive for `data` compressing each chunk in parallel. This function uses
    /// the `rayon` crate for parallelism. By default compression happens in the global thread pool,
    /// but this function can also be executed within a locally scoped pool.
    pub fn new(data: &[u8], options: ChunkedArchiveOptions) -> Result<Self, ChunkedArchiveError> {
        let chunk_size = options.chunk_size_for(data.len());
        let mut chunks: Vec<Result<CompressedChunk, ChunkedArchiveError>> = vec![];
        let compressor = options.thread_local_compressor();
        data.par_chunks(chunk_size)
            .enumerate()
            .map(|(index, chunk)| {
                let compressed_data = compressor.compress(chunk, index)?;
                Ok(CompressedChunk { compressed_data, decompressed_size: chunk.len() })
            })
            .collect_into_vec(&mut chunks);
        let chunks: Vec<_> = chunks.into_iter().try_collect()?;
        Ok(ChunkedArchive { chunks, chunk_size, options })
    }

    /// Accessor for compressed chunk data.
    pub fn chunks(&self) -> &Vec<CompressedChunk> {
        &self.chunks
    }

    /// The chunk size calculated for this archive during compression. Represents how input data
    /// was chunked for compression. Note that the final chunk may be smaller than this amount
    /// when decompressed.
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// Sum of sizes of all compressed chunks.
    pub fn compressed_data_size(&self) -> usize {
        self.chunks.iter().map(|chunk| chunk.compressed_data.len()).sum()
    }

    /// Total size of the archive in bytes.
    pub fn serialized_size(&self) -> usize {
        ChunkedArchiveHeader::header_length(self.chunks.len()) + self.compressed_data_size()
    }

    /// Write the archive to `writer`.
    pub fn write(self, mut writer: impl std::io::Write) -> Result<(), std::io::Error> {
        let seek_table = self.make_seek_table();
        let header = ChunkedArchiveHeader::new(&seek_table, self.options).unwrap();
        writer.write_all(header.as_bytes())?;
        writer.write_all(seek_table.as_slice().as_bytes())?;
        for chunk in self.chunks {
            writer.write_all(&chunk.compressed_data)?;
        }
        Ok(())
    }

    /// Create the seek table for this archive.
    fn make_seek_table(&self) -> Vec<SeekTableEntry> {
        let header_length = ChunkedArchiveHeader::header_length(self.chunks.len());
        let mut seek_table = vec![];
        seek_table.reserve(self.chunks.len());
        let mut compressed_size: usize = 0;
        let mut decompressed_offset: usize = 0;
        for chunk in &self.chunks {
            seek_table.push(SeekTableEntry {
                decompressed_offset: (decompressed_offset as u64).into(),
                decompressed_size: (chunk.decompressed_size as u64).into(),
                compressed_offset: ((header_length + compressed_size) as u64).into(),
                compressed_size: (chunk.compressed_data.len() as u64).into(),
            });
            compressed_size += chunk.compressed_data.len();
            decompressed_offset += chunk.decompressed_size;
        }
        seek_table
    }
}

/// Streaming decompressor for chunked archives. Example:
/// ```
/// // Create a chunked archive:
/// let data: Vec<u8> = vec![3; 1024];
/// let compressed = ChunkedArchive::new(&data, /*block_size*/ 8192).serialize().unwrap();
/// // Verify the header + decode the seek table:
/// let (seek_table, archive_data) = decode_archive(&compressed, compressed.len())?.unwrap();
/// let mut decompressed: Vec<u8> = vec![];
/// let mut on_chunk = |data: &[u8]| { decompressed.extend_from_slice(data); };
/// let mut decompressor = ChunkedDecompressor(seek_table);
/// // `on_chunk` is invoked as each slice is made available. Archive can be provided as chunks.
/// decompressor.update(archive_data, &mut on_chunk);
/// assert_eq!(data.as_slice(), decompressed.as_slice());
/// ```
pub struct ChunkedDecompressor {
    seek_table: Vec<ChunkInfo>,
    buffer: Vec<u8>,
    data_written: usize,
    curr_chunk: usize,
    total_compressed_size: usize,
    decompressor: Decompressor,
    decompressed_buffer: Vec<u8>,
    error_handler: Option<ErrorHandler>,
}

type ErrorHandler = Box<dyn Fn(usize, ChunkInfo, &[u8]) -> () + Send + 'static>;

impl ChunkedDecompressor {
    /// Create a new decompressor to decode an archive from a validated seek table.
    pub fn new(decoded_archive: DecodedArchive) -> Result<Self, ChunkedArchiveError> {
        let DecodedArchive { compression_algorithm, seek_table } = decoded_archive;
        let total_compressed_size =
            seek_table.last().map_or(0, |last_chunk| last_chunk.compressed_range.end);
        let decompressed_buffer =
            vec![0u8; seek_table.first().map_or(0, |c| c.decompressed_range.len())];
        Ok(Self {
            seek_table,
            buffer: vec![],
            data_written: 0,
            curr_chunk: 0,
            total_compressed_size,
            decompressor: compression_algorithm.decompressor(),
            decompressed_buffer,
            error_handler: None,
        })
    }

    /// Creates a new decompressor with an additional error handler invoked when a chunk fails to be
    /// decompressed.
    pub fn new_with_error_handler(
        decoded_archive: DecodedArchive,
        error_handler: ErrorHandler,
    ) -> Result<Self, ChunkedArchiveError> {
        Ok(Self { error_handler: Some(error_handler), ..Self::new(decoded_archive)? })
    }

    /// Returns the compression algorithm used by this decompressor.
    pub fn algorithm(&self) -> CompressionAlgorithm {
        match &self.decompressor {
            Decompressor::Zstd => CompressionAlgorithm::Zstd,
            Decompressor::Lz4 => CompressionAlgorithm::Lz4,
        }
    }

    pub fn seek_table(&self) -> &Vec<ChunkInfo> {
        &self.seek_table
    }

    fn finish_chunk(
        &mut self,
        data: &[u8],
        chunk_callback: &mut impl FnMut(&[u8]) -> (),
    ) -> Result<(), ChunkedArchiveError> {
        debug_assert_eq!(data.len(), self.seek_table[self.curr_chunk].compressed_range.len());
        let chunk = &self.seek_table[self.curr_chunk];
        let decompressed_size = self
            .decompressor
            .decompress_into(data, self.decompressed_buffer.as_mut_slice(), self.curr_chunk)
            .inspect_err(|_| {
                if let Some(error_handler) = &self.error_handler {
                    error_handler(self.curr_chunk, chunk.clone(), data.as_bytes());
                }
            })?;
        if decompressed_size != chunk.decompressed_range.len() {
            return Err(ChunkedArchiveError::IntegrityError);
        }
        chunk_callback(&self.decompressed_buffer[..decompressed_size]);
        self.curr_chunk += 1;
        Ok(())
    }

    /// Update the decompressor with more data.
    pub fn update(
        &mut self,
        mut data: &[u8],
        chunk_callback: &mut impl FnMut(&[u8]) -> (),
    ) -> Result<(), ChunkedArchiveError> {
        // Caller must not provide too much data.
        if self.data_written + data.len() > self.total_compressed_size {
            return Err(ChunkedArchiveError::OutOfRange);
        }
        self.data_written += data.len();

        // If we had leftover data from a previous read, append until we've filled a chunk.
        if !self.buffer.is_empty() {
            let to_read = std::cmp::min(
                data.len(),
                self.seek_table[self.curr_chunk]
                    .compressed_range
                    .len()
                    .checked_sub(self.buffer.len())
                    .unwrap(),
            );
            self.buffer.extend_from_slice(&data[..to_read]);
            if self.buffer.len() == self.seek_table[self.curr_chunk].compressed_range.len() {
                // Take self.buffer temporarily (so we don't have to split borrows).
                // That way we don't have to re-commit the pages we've already used in the buffer
                // for next time.
                let full_chunk = std::mem::take(&mut self.buffer);
                self.finish_chunk(&full_chunk[..], chunk_callback)?;
                self.buffer = full_chunk;
                // Draining the buffer will set the length to 0 but keep the capacity the same.
                self.buffer.clear();
            }
            data = &data[to_read..];
        }

        // Decode as many full chunks as we can.
        while !data.is_empty()
            && self.curr_chunk < self.seek_table.len()
            && self.seek_table[self.curr_chunk].compressed_range.len() <= data.len()
        {
            let len = self.seek_table[self.curr_chunk].compressed_range.len();
            self.finish_chunk(&data[..len], chunk_callback)?;
            data = &data[len..];
        }

        // Buffer the rest for the next call.
        if !data.is_empty() {
            debug_assert!(self.curr_chunk < self.seek_table.len());
            debug_assert!(self.data_written < self.total_compressed_size);
            self.buffer.extend_from_slice(data);
        }

        debug_assert!(
            self.data_written < self.total_compressed_size
                || self.curr_chunk == self.seek_table.len()
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Type1Blob;

    use super::*;
    use rand::RngExt as _;
    use std::matches;

    /// Create a compressed archive and ensure we can decode it as a valid archive that passes all
    /// required integrity checks.
    #[test]
    fn compress_simple() {
        let data: Vec<u8> = vec![0; 32 * 1024 * 16];
        let archive = ChunkedArchive::new(&data, Type1Blob::CHUNKED_ARCHIVE_OPTIONS).unwrap();
        // This data is highly compressible, so the result should be smaller than the original.
        let mut compressed: Vec<u8> = vec![];
        archive.write(&mut compressed).unwrap();
        assert!(compressed.len() <= data.len());
        // We should be able to decode and verify the archive's integrity in-place.
        assert!(decode_archive(&compressed, compressed.len()).unwrap().is_some());
    }

    /// Generate a header + seek table for verifying invariants/integrity checks.
    fn generate_archive(
        num_entries: usize,
        options: ChunkedArchiveOptions,
    ) -> (ChunkedArchiveHeader, Vec<SeekTableEntry>, /*archive_length*/ u64) {
        let mut seek_table = Vec::with_capacity(num_entries);
        let header_length = ChunkedArchiveHeader::header_length(num_entries) as u64;
        const COMPRESSED_CHUNK_SIZE: u64 = 1024;
        const DECOMPRESSED_CHUNK_SIZE: u64 = 2048;
        for n in 0..(num_entries as u64) {
            seek_table.push(SeekTableEntry {
                compressed_offset: (header_length + (n * COMPRESSED_CHUNK_SIZE)).into(),
                compressed_size: COMPRESSED_CHUNK_SIZE.into(),
                decompressed_offset: (n * DECOMPRESSED_CHUNK_SIZE).into(),
                decompressed_size: DECOMPRESSED_CHUNK_SIZE.into(),
            });
        }
        let header = ChunkedArchiveHeader::new(&seek_table, options).unwrap();
        let archive_length: u64 = header_length + (num_entries as u64 * COMPRESSED_CHUNK_SIZE);
        (header, seek_table, archive_length)
    }

    #[test]
    fn should_validate_self() {
        let (header, seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(header.decode_archive(serialized_table, archive_length).unwrap().is_some());
    }

    #[test]
    fn should_validate_empty() {
        let (header, _, archive_length) = generate_archive(0, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert!(header.decode_archive(&[], archive_length).unwrap().is_some());
    }

    #[test]
    fn should_detect_bad_magic() {
        let (header, seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let mut corrupt_magic = ChunkedArchiveHeader::CHUNKED_ARCHIVE_MAGIC;
        corrupt_magic[0] = !corrupt_magic[0];
        let bad_magic = ChunkedArchiveHeader { magic: corrupt_magic, ..header };
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            bad_magic.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::BadMagic
        ));
    }
    #[test]
    fn should_detect_wrong_version() {
        let (header, seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let invalid_version = ChunkedArchiveHeader { version: u16::MAX.into(), ..header };
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            invalid_version.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::InvalidVersion
        ));
    }

    #[test]
    fn should_detect_corrupt_checksum() {
        let (header, seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let corrupt_checksum =
            ChunkedArchiveHeader { checksum: (!header.checksum.get()).into(), ..header };
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            corrupt_checksum.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn should_reject_too_many_entries_v2() {
        let (too_many_entries, seek_table, archive_length) = generate_archive(
            ChunkedArchiveOptions::V2_MAX_CHUNKS + 1,
            Type1Blob::CHUNKED_ARCHIVE_OPTIONS,
        );

        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            too_many_entries.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i0_first_entry_zero() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert_eq!(seek_table[0].decompressed_offset.get(), 0);
        seek_table[0].decompressed_offset = 1.into();

        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i1_no_header_overlap() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let header_end = ChunkedArchiveHeader::header_length(seek_table.len()) as u64;
        assert!(seek_table[0].compressed_offset.get() >= header_end);
        seek_table[0].compressed_offset = (header_end - 1).into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i2_decompressed_monotonic() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert_eq!(
            seek_table[0].decompressed_offset.get() + seek_table[0].decompressed_size.get(),
            seek_table[1].decompressed_offset.get()
        );
        seek_table[1].decompressed_offset = (seek_table[1].decompressed_offset.get() - 1).into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i3_compressed_monotonic() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert!(
            (seek_table[0].compressed_offset.get() + seek_table[0].compressed_size.get())
                <= seek_table[1].compressed_offset.get()
        );
        seek_table[1].compressed_offset = (seek_table[1].compressed_offset.get() - 1).into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i4_nonzero_compressed_size() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert!(seek_table[0].compressed_size.get() > 0);
        seek_table[0].compressed_size = 0.into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i4_nonzero_decompressed_size() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        assert!(seek_table[0].decompressed_size.get() > 0);
        seek_table[0].decompressed_size = 0.into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn invariant_i5_within_archive() {
        let (header, mut seek_table, archive_length) =
            generate_archive(4, Type1Blob::CHUNKED_ARCHIVE_OPTIONS);
        let last_entry = seek_table.last_mut().unwrap();
        assert!(
            (last_entry.compressed_offset.get() + last_entry.compressed_size.get())
                <= archive_length
        );
        last_entry.compressed_offset = (archive_length + 1).into();
        let serialized_table = seek_table.as_slice().as_bytes();
        assert!(matches!(
            header.decode_archive(serialized_table, archive_length).unwrap_err(),
            ChunkedArchiveError::IntegrityError
        ));
    }

    #[test]
    fn max_chunks() {
        let ChunkedArchiveOptions::V2 { minimum_chunk_size, chunk_alignment, .. } =
            Type1Blob::CHUNKED_ARCHIVE_OPTIONS
        else {
            panic!()
        };
        assert_eq!(
            Type1Blob::CHUNKED_ARCHIVE_OPTIONS
                .chunk_size_for(minimum_chunk_size * ChunkedArchiveOptions::V2_MAX_CHUNKS),
            minimum_chunk_size
        );
        assert_eq!(
            Type1Blob::CHUNKED_ARCHIVE_OPTIONS
                .chunk_size_for(minimum_chunk_size * ChunkedArchiveOptions::V2_MAX_CHUNKS + 1),
            minimum_chunk_size + chunk_alignment
        );
    }

    #[test]
    fn test_decompressor_empty_archive() {
        let mut compressed: Vec<u8> = vec![];
        ChunkedArchive::new(&[], Type1Blob::CHUNKED_ARCHIVE_OPTIONS)
            .expect("compress")
            .write(&mut compressed)
            .expect("write archive");
        let (decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();
        assert!(decoded_archive.seek_table.is_empty());
        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();
        let mut chunk_callback = |_chunk: &[u8]| panic!("Archive doesn't have any chunks.");
        // Stream data into the decompressor in small chunks to exhaust more edge cases.
        chunk_data
            .chunks(4)
            .for_each(|data| decompressor.update(data, &mut chunk_callback).unwrap());
    }

    #[test]
    fn test_decompressor() {
        const UNCOMPRESSED_LENGTH: usize = 3_000_000;
        let data: Vec<u8> = {
            let range = rand::distr::Uniform::<u8>::new_inclusive(0, 255).unwrap();
            rand::rng().sample_iter(&range).take(UNCOMPRESSED_LENGTH).collect()
        };
        let mut compressed: Vec<u8> = vec![];
        ChunkedArchive::new(&data, Type1Blob::CHUNKED_ARCHIVE_OPTIONS)
            .expect("compress")
            .write(&mut compressed)
            .expect("write archive");
        let (decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        // Make sure we have multiple chunks for this test.
        let num_chunks = decoded_archive.seek_table.len();
        assert!(num_chunks > 1);

        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();

        let mut decoded_chunks: usize = 0;
        let mut decompressed_offset: usize = 0;
        let mut chunk_callback = |decompressed_chunk: &[u8]| {
            assert!(
                decompressed_chunk
                    == &data[decompressed_offset..decompressed_offset + decompressed_chunk.len()]
            );
            decompressed_offset += decompressed_chunk.len();
            decoded_chunks += 1;
        };

        // Stream data into the decompressor in small chunks to exhaust more edge cases.
        chunk_data
            .chunks(4)
            .for_each(|data| decompressor.update(data, &mut chunk_callback).unwrap());
        assert_eq!(decoded_chunks, num_chunks);
    }

    #[test]
    fn test_decompressor_corrupt_decompressed_size() {
        let data = vec![0; 3_000_000];
        let mut compressed: Vec<u8> = vec![];
        ChunkedArchive::new(&data, Type1Blob::CHUNKED_ARCHIVE_OPTIONS)
            .expect("compress")
            .write(&mut compressed)
            .expect("write archive");
        let (mut decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        // Corrupt the decompressed size of the chunk.
        decoded_archive.seek_table[0].decompressed_range =
            decoded_archive.seek_table[0].decompressed_range.start
                ..decoded_archive.seek_table[0].decompressed_range.end + 1;

        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();
        assert!(matches!(
            decompressor.update(&chunk_data, &mut |_chunk| {}),
            Err(ChunkedArchiveError::IntegrityError)
        ));
    }

    #[test]
    fn test_decompressor_corrupt_compressed_size() {
        let data = vec![0; 3_000_000];
        let mut compressed: Vec<u8> = vec![];
        ChunkedArchive::new(&data, Type1Blob::CHUNKED_ARCHIVE_OPTIONS)
            .expect("compress")
            .write(&mut compressed)
            .expect("write archive");
        let (mut decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        // Corrupt the compressed size of the chunk.
        decoded_archive.seek_table[0].compressed_range =
            decoded_archive.seek_table[0].compressed_range.start
                ..decoded_archive.seek_table[0].compressed_range.end - 1;
        let first_chunk_info = decoded_archive.seek_table[0].clone();
        let error_handler = move |chunk_index: usize, chunk_info: ChunkInfo, chunk_data: &[u8]| {
            assert_eq!(chunk_index, 0);
            assert_eq!(chunk_info, first_chunk_info);
            assert_eq!(chunk_data.len(), chunk_info.compressed_range.len());
        };

        let mut decompressor =
            ChunkedDecompressor::new_with_error_handler(decoded_archive, Box::new(error_handler))
                .unwrap();
        assert!(matches!(
            decompressor.update(&chunk_data, &mut |_chunk| {}),
            Err(ChunkedArchiveError::DecompressionError { .. })
        ));
    }

    #[test]
    fn test_decompressor_zstd_data_corruption() {
        let data = vec![0; 3_000_000];
        let mut compressed: Vec<u8> = vec![];
        let archive = match ChunkedArchive::new(&data, Type1Blob::CHUNKED_ARCHIVE_OPTIONS) {
            Ok(a) => a,
            Err(e) => {
                panic!("Failed to compress in test: {:?}", e);
            }
        };
        archive.write(&mut compressed).expect("write archive");
        let (decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        let mut corrupt_data = chunk_data.to_vec();
        if corrupt_data.len() > 100 {
            corrupt_data[100] = !corrupt_data[100];
        }

        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();
        let result = decompressor.update(&corrupt_data, &mut |_chunk| {});
        assert!(matches!(result, Err(ChunkedArchiveError::DecompressionError { .. })));
    }

    #[test]
    fn test_v3_zstd_roundtrip() {
        let data = vec![0; 3_000_000];
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Zstd };
        let mut compressed = vec![];
        ChunkedArchive::new(&data, options)
            .expect("compress")
            .write(&mut compressed)
            .expect("write");

        // Verify header.
        let (header, _) =
            Ref::<_, ChunkedArchiveHeader>::from_prefix(compressed.as_slice()).unwrap();
        assert_eq!(header.version.get(), 3);
        assert_eq!(header.compression_algorithm, CompressionAlgorithm::Zstd as u8);

        let (decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        // Decompress.
        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();
        let mut decompressed: Vec<u8> = vec![];
        let mut chunk_callback = |chunk: &[u8]| decompressed.extend_from_slice(chunk);
        decompressor.update(chunk_data, &mut chunk_callback).unwrap();

        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_v3_lz4_roundtrip() {
        let data = vec![0; 3_000_000];
        let options =
            ChunkedArchiveOptions::V3 { compression_algorithm: CompressionAlgorithm::Lz4 };
        let mut compressed = vec![];
        ChunkedArchive::new(&data, options)
            .expect("compress")
            .write(&mut compressed)
            .expect("write");

        // Verify header.
        let (header, _) =
            Ref::<_, ChunkedArchiveHeader>::from_prefix(compressed.as_slice()).unwrap();
        assert_eq!(header.version.get(), 3);
        assert_eq!(header.compression_algorithm, CompressionAlgorithm::Lz4 as u8);

        let (decoded_archive, chunk_data) =
            decode_archive(&compressed, compressed.len()).unwrap().unwrap();

        // Decompress.
        let mut decompressor = ChunkedDecompressor::new(decoded_archive).unwrap();
        let mut decompressed: Vec<u8> = vec![];
        let mut chunk_callback = |chunk: &[u8]| decompressed.extend_from_slice(chunk);
        decompressor.update(chunk_data, &mut chunk_callback).unwrap();

        assert_eq!(decompressed, data);
    }
}
