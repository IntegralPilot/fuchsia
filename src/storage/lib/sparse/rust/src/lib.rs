// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_endian = "big")]
assert!(false, "This library assumes little-endian!");

pub mod builder;
mod format;
pub mod reader;

use crate::format::{CHUNK_HEADER_SIZE, ChunkHeader, SparseHeader};
use crate::reader::SparseReader;

use core::fmt;
use serde::de::DeserializeOwned;
use thiserror::Error;

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tempfile::{NamedTempFile, TempPath};
#[cfg(target_os = "fuchsia")]
use zx;

// Size of blocks to write.  Note that the format supports varied block sizes; this is the preferred
// size by this library.
const BLK_SIZE: u32 = 0x1000;

#[derive(Debug, Clone, Copy)]
pub enum SparseDataType {
    Header,
    ChunkHeader,
    FillValue,
    Checksum,
}

impl std::fmt::Display for SparseDataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header => write!(f, "header"),
            Self::ChunkHeader => write!(f, "chunk header"),
            Self::FillValue => write!(f, "fill value"),
            Self::Checksum => write!(f, "checksum"),
        }
    }
}

#[derive(Debug, Error)]
pub enum DeserializeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bincode error: {0}")]
    Bincode(#[from] Box<bincode::ErrorKind>),
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum UnalignedSource {
    #[error("buffer length {0}")]
    Buffer(usize),

    #[error("Reader length {0}")]
    Reader(u64),

    #[error("Skip length {0}")]
    Skip(u64),

    #[error("Fill length {0}")]
    Fill(u64),

    #[error("Vmo size {0}")]
    Vmo(u64),
}

#[derive(Debug, Error)]
pub enum SparseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to deserialize {ty}: {source}")]
    Deserialize {
        ty: SparseDataType,
        #[source]
        source: DeserializeError,
    },

    #[error("Failed to serialize {ty}: {source}")]
    Serialize {
        ty: SparseDataType,
        #[source]
        source: Box<bincode::ErrorKind>,
    },

    #[error("Invalid sparse image header")]
    InvalidHeader,

    #[error("Invalid chunk header")]
    InvalidChunkHeader,

    #[error("Invalid chunk type {0}")]
    InvalidChunkType(u16),

    #[error("Given maximum download size ({0}) is less than the block size ({1})")]
    MaxDownloadSizeTooSmall(u64, u32),

    #[error("No source for Raw chunk")]
    NoSourceForRawChunk,

    #[error("Chunk is not block aligned")]
    UnalignedChunk,

    #[error("Failed to copy contents: {0}")]
    CopyContents(#[source] std::io::Error),

    #[error("Failed to fill contents: {0}")]
    FillContents(#[source] std::io::Error),

    #[error("Failed to skip contents: {0}")]
    SkipContents(#[source] std::io::Error),

    #[cfg(target_os = "fuchsia")]
    #[error("Zircon error: {0}")]
    Zircon(#[from] zx::Status),

    #[error("Invalid {0}")]
    UnalignedDataSource(UnalignedSource),

    #[error("Sparse image would contain too many blocks")]
    TooManyBlocks,
}

fn deserialize_from<'a, T: DeserializeOwned, R: Read + ?Sized>(
    source: &mut R,
) -> Result<T, DeserializeError> {
    let mut buf = vec![0u8; std::mem::size_of::<T>()];
    source.read_exact(&mut buf[..])?;
    bincode::deserialize(&buf[..]).map_err(Into::into)
}

/// A union trait for `Write` and `Seek` that also allows truncation.
pub trait Writer: Write + Seek {
    /// Sets the length of the output stream.
    fn set_len(&mut self, size: u64) -> Result<(), SparseError>;
}

impl Writer for File {
    fn set_len(&mut self, size: u64) -> Result<(), SparseError> {
        File::set_len(self, size).map_err(SparseError::from)
    }
}

impl Writer for Cursor<Vec<u8>> {
    fn set_len(&mut self, size: u64) -> Result<(), SparseError> {
        Vec::resize(self.get_mut(), size as usize, 0u8);
        Ok(())
    }
}

// A wrapper around a Reader, which makes it seem like the underlying stream is only self.1 bytes
// long.  The underlying reader is still advanced upon reading.
// This is distinct from `std::io::Take` in that it does not modify the seek offset of the
// underlying reader.  In other words, `LimitedReader` can be used to read a window within the
// reader (by setting seek offset to the start, and the size limit to the end).
struct LimitedReader<'a, R>(pub &'a mut R, pub usize);

impl<'a, R: Read + Seek> Read for LimitedReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let offset = self.0.stream_position()?;
        let avail = self.1.saturating_sub(offset as usize);
        let to_read = std::cmp::min(avail, buf.len());
        self.0.read(&mut buf[..to_read])
    }
}

/// Returns whether the image in `reader` appears to be in the sparse format.
pub fn is_sparse_image<R: Read + Seek>(reader: &mut R) -> bool {
    || -> Option<bool> {
        let header: SparseHeader = deserialize_from(reader).ok()?;
        let is_sparse = header.magic == format::SPARSE_HEADER_MAGIC;
        reader.seek(SeekFrom::Start(0)).ok()?;
        Some(is_sparse)
    }()
    .unwrap_or(false)
}

#[derive(Clone, PartialEq, Debug)]
pub enum Chunk {
    /// `Raw` represents a set of blocks to be written to disk as-is.
    /// `start` is the offset in the expanded image at which the Raw section starts.
    /// `start` and `size` are in bytes, but must be block-aligned.
    Raw { start: u64, size: u64 },
    /// `Fill` represents a Chunk that has the `value` repeated enough to fill `size` bytes.
    /// `start` is the offset in the expanded image at which the Fill section starts.
    /// `start` and `size` are in bytes, but must be block-aligned.
    Fill { start: u64, size: u64, value: u32 },
    /// `DontCare` represents a set of blocks that need to be "offset" by the
    /// image recipient.  If an image needs to be broken up into two sparse images, and we flash n
    /// bytes for Sparse Image 1, Sparse Image 2 needs to start with a DontCareChunk with
    /// (n/blocksize) blocks as its "size" property.
    /// `start` is the offset in the expanded image at which the DontCare section starts.
    /// `start` and `size` are in bytes, but must be block-aligned.
    DontCare { start: u64, size: u64 },
    /// `Crc32Chunk` is used as a checksum of a given set of Chunks for a SparseImage.  This is not
    /// required and unused in most implementations of the Sparse Image format. The type is included
    /// for completeness. It has 4 bytes of CRC32 checksum as describable in a u32.
    #[allow(dead_code)]
    Crc32 { checksum: u32 },
}

impl Chunk {
    /// Attempts to read a `Chunk` from `reader`.  The reader will be positioned at the first byte
    /// following the chunk header and any extra data; for a Raw chunk this means it will point at
    /// the data payload, and for other chunks it will point at the next chunk header (or EOF).
    /// `offset` is the current offset in the logical volume.
    pub fn read_metadata<R: Read>(
        reader: &mut R,
        offset: u64,
        block_size: u32,
    ) -> Result<Self, SparseError> {
        let header: ChunkHeader = deserialize_from(reader)
            .map_err(|e| SparseError::Deserialize { ty: SparseDataType::ChunkHeader, source: e })?;
        if !header.valid() {
            return Err(SparseError::InvalidChunkHeader);
        }

        let size = header.chunk_sz as u64 * block_size as u64;
        match header.chunk_type {
            format::CHUNK_TYPE_RAW => Ok(Self::Raw { start: offset, size }),
            format::CHUNK_TYPE_FILL => {
                let value: u32 = deserialize_from(reader).map_err(|e| {
                    SparseError::Deserialize { ty: SparseDataType::FillValue, source: e }
                })?;
                Ok(Self::Fill { start: offset, size, value })
            }
            format::CHUNK_TYPE_DONT_CARE => Ok(Self::DontCare { start: offset, size }),
            format::CHUNK_TYPE_CRC32 => {
                let checksum: u32 = deserialize_from(reader).map_err(|e| {
                    SparseError::Deserialize { ty: SparseDataType::Checksum, source: e }
                })?;
                Ok(Self::Crc32 { checksum })
            }
            // We already validated the chunk_type in `ChunkHeader::is_valid`.
            _ => unreachable!(),
        }
    }

    fn valid(&self, block_size: u32) -> bool {
        self.output_size() % (block_size as u64) == 0
    }

    /// Returns the offset into the logical image the chunk refers to, or None if the chunk has no
    /// output data.
    fn output_offset(&self) -> Option<u64> {
        match self {
            Self::Raw { start, .. } => Some(*start),
            Self::Fill { start, .. } => Some(*start),
            Self::DontCare { start, .. } => Some(*start),
            Self::Crc32 { .. } => None,
        }
    }

    /// Return number of bytes the chunk expands to when written to the partition.
    fn output_size(&self) -> u64 {
        match self {
            Self::Raw { size, .. } => *size,
            Self::Fill { size, .. } => *size,
            Self::DontCare { size, .. } => *size,
            Self::Crc32 { .. } => 0,
        }
    }

    /// Return number of blocks the chunk expands to when written to the partition.
    fn output_blocks(&self, block_size: u32) -> u32 {
        self.output_size().div_ceil(block_size as u64) as u32
    }

    /// `chunk_type` returns the integer flag to represent the type of chunk
    /// to use in the ChunkHeader
    fn chunk_type(&self) -> u16 {
        match self {
            Self::Raw { .. } => format::CHUNK_TYPE_RAW,
            Self::Fill { .. } => format::CHUNK_TYPE_FILL,
            Self::DontCare { .. } => format::CHUNK_TYPE_DONT_CARE,
            Self::Crc32 { .. } => format::CHUNK_TYPE_CRC32,
        }
    }

    /// `chunk_data_len` returns the length of the chunk's header plus the
    /// length of the data when serialized.
    ///
    /// This gets included in the sparse header and is encoded as a u32.
    /// But while we are tracking the offsets and total sizes we need it to be
    /// a u64 to help keep track of files that are greater than 4 GiB
    fn chunk_data_len(&self) -> u32 {
        let header_size = format::CHUNK_HEADER_SIZE;
        let data_size = match self {
            Self::Raw { size, .. } => *size as u32,
            Self::Fill { .. } => std::mem::size_of::<u32>() as u32,
            Self::DontCare { .. } => 0,
            Self::Crc32 { .. } => std::mem::size_of::<u32>() as u32,
        };
        header_size.checked_add(data_size).unwrap()
    }

    /// Writes the chunk to the given Writer.  `source` is a Reader containing the data payload for
    /// a Raw type chunk, with the seek offset pointing to the first byte of the data payload, and
    /// with exactly enough bytes available for the rest of the data payload.
    fn write<W: Write, R: Read>(
        &self,
        source: Option<&mut R>,
        dest: &mut W,
        block_size: u32,
    ) -> Result<(), SparseError> {
        if !self.valid(block_size) {
            return Err(SparseError::UnalignedChunk);
        }
        let header = ChunkHeader::new(
            self.chunk_type(),
            0x0,
            self.output_blocks(block_size),
            self.chunk_data_len(),
        );

        bincode::serialize_into(&mut *dest, &header)
            .map_err(|e| SparseError::Serialize { ty: SparseDataType::ChunkHeader, source: e })?;

        match self {
            Self::Raw { size, .. } => {
                if source.is_none() {
                    return Err(SparseError::NoSourceForRawChunk);
                }
                let n = std::io::copy(source.unwrap(), dest)?;
                let size = *size as u64;
                if n < size {
                    let zeroes = vec![0u8; (size - n) as usize];
                    dest.write_all(&zeroes)?;
                }
            }
            Self::Fill { value, .. } => {
                // Serialize the value,
                bincode::serialize_into(dest, value).map_err(|e| SparseError::Serialize {
                    ty: SparseDataType::FillValue,
                    source: e,
                })?;
            }
            Self::DontCare { .. } => {
                // DontCare has no data to write
            }
            Self::Crc32 { checksum } => {
                bincode::serialize_into(dest, checksum).map_err(|e| SparseError::Serialize {
                    ty: SparseDataType::Checksum,
                    source: e,
                })?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Raw { start, size } => {
                format!("RawChunk: start: {}, total bytes: {}", start, size)
            }
            Self::Fill { start, size, value } => {
                format!("FillChunk: start: {}, value: {}, n_blocks: {}", start, value, size)
            }
            Self::DontCare { start, size } => {
                format!("DontCareChunk: start: {}, bytes: {}", start, size)
            }
            Self::Crc32 { checksum } => format!("Crc32Chunk: checksum: {:?}", checksum),
        };
        write!(f, "{}", message)
    }
}

/// Chunk::write takes an Option of something that implements Read. The compiler still requires a
/// concrete type for the generic argument even when the Option is None. This constant can be used
/// in place of None to avoid having to specify a type for the source.
pub const NO_SOURCE: Option<&mut Cursor<&[u8]>> = None;

/// An in-memory description of an Android sparse image file.
///
/// Holds a sequence of [`Chunk`] definitions that describe how unsparsed image data
/// should be formatted or partitioned. Can be serialized to a destination writer or
/// lazily read via a [`SparseSliceReader`].
#[derive(Clone, Debug, PartialEq)]
pub struct SparseFileWriter {
    /// The sequence of chunks that make up the sparse image.
    pub chunks: Vec<Chunk>,
}

impl SparseFileWriter {
    /// Creates a new `SparseFileWriter` from a sequence of [`Chunk`]s.
    pub fn new(chunks: Vec<Chunk>) -> SparseFileWriter {
        SparseFileWriter { chunks }
    }

    /// Returns the total number of blocks represented by all chunks in this sparse image.
    pub fn total_blocks(&self) -> u32 {
        self.chunks.iter().map(|c| c.output_blocks(BLK_SIZE)).sum()
    }

    /// Returns the total unsparsed size in bytes represented by this sparse image.
    pub fn total_bytes(&self) -> u64 {
        self.chunks.iter().map(|c| c.output_size() as u64).sum()
    }

    /// Returns the total serialized size (in bytes) of the sparse image file,
    /// including the file header, all chunk headers, and chunk payloads.
    pub fn file_size(&self) -> u64 {
        let mut size = format::SPARSE_HEADER_SIZE as u64;
        for chunk in &self.chunks {
            size += chunk.chunk_data_len() as u64;
        }
        size
    }

    /// Creates an `io::Read` stream that lazily reads the serialized sparse image bytes
    /// directly from `source` without creating an intermediate file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`SparseError::UnalignedChunk`] if any chunk is not aligned to the block size,
    /// or [`SparseError::Serialize`] if header serialization fails.
    pub fn slice_reader<'a, R: Read + Seek>(
        &'a self,
        source: &'a mut R,
    ) -> Result<SparseSliceReader<'a, R>, SparseError> {
        SparseSliceReader::new(self, source)
    }

    /// Writes the serialized sparse image to `writer`, reading raw payload data from `reader`.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to `writer` or seeking/reading from `reader` fails,
    /// or if any chunk is invalid or cannot be serialized.
    pub fn write<W: Write + Seek, R: Read + Seek>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Result<(), SparseError> {
        let header = SparseHeader::new(
            BLK_SIZE.try_into().unwrap(),          // Size of the blocks
            self.total_blocks(),                   // Total blocks in this image
            self.chunks.len().try_into().unwrap(), // Total chunks in this image
        );

        bincode::serialize_into(&mut *writer, &header)
            .map_err(|e| SparseError::Serialize { ty: SparseDataType::Header, source: e })?;

        for chunk in &self.chunks {
            let mut reader = if let &Chunk::Raw { start, size } = chunk {
                if reader.stream_position()? != start {
                    reader.seek(SeekFrom::Start(start))?;
                }
                Some(LimitedReader(reader, start as usize + size as usize))
            } else {
                None
            };
            chunk.write(reader.as_mut(), writer, BLK_SIZE)?;
        }

        Ok(())
    }
}

/// `SparseSliceReader` is an `io::Read` stream that lazily emits the binary
/// Android Sparse Image serialization (headers and payload) for a single
/// `SparseFileWriter` slice directly from the underlying source reader without
/// intermediate disk files.
pub struct SparseSliceReader<'a, R> {
    source: &'a mut R,
    header_bytes: Vec<u8>,
    header_pos: usize,
    chunks: &'a [Chunk],
    chunk_idx: usize,
    chunk_header_bytes: Vec<u8>,
    chunk_header_pos: usize,
    payload_pos: u64,
}

impl<'a, R: Read + Seek> SparseSliceReader<'a, R> {
    /// Creates a new `SparseSliceReader` that lazily streams the serialized representation
    /// of `writer` using payload bytes from `source`.
    ///
    /// # Errors
    ///
    /// Returns [`SparseError::UnalignedChunk`] if any chunk is not block-aligned,
    /// or [`SparseError::Serialize`] if header serialization fails.
    pub fn new(writer: &'a SparseFileWriter, source: &'a mut R) -> Result<Self, SparseError> {
        let header = SparseHeader::new(
            BLK_SIZE.try_into().unwrap(),
            writer.total_blocks(),
            writer.chunks.len().try_into().unwrap(),
        );
        let header_bytes = bincode::serialize(&header)
            .map_err(|e| SparseError::Serialize { ty: SparseDataType::Header, source: e })?;
        let mut reader = Self {
            source,
            header_bytes,
            header_pos: 0,
            chunks: &writer.chunks,
            chunk_idx: 0,
            chunk_header_bytes: Vec::new(),
            chunk_header_pos: 0,
            payload_pos: 0,
        };
        reader.prepare_next_chunk_header()?;
        Ok(reader)
    }

    fn prepare_next_chunk_header(&mut self) -> Result<(), SparseError> {
        if self.chunk_idx < self.chunks.len() {
            let chunk = &self.chunks[self.chunk_idx];
            if !chunk.valid(BLK_SIZE) {
                return Err(SparseError::UnalignedChunk);
            }
            let header = ChunkHeader::new(
                chunk.chunk_type(),
                0x0,
                chunk.output_blocks(BLK_SIZE),
                chunk.chunk_data_len(),
            );
            self.chunk_header_bytes = bincode::serialize(&header).map_err(|e| {
                SparseError::Serialize { ty: SparseDataType::ChunkHeader, source: e }
            })?;
            self.chunk_header_pos = 0;
            self.payload_pos = 0;
        }
        Ok(())
    }
}

impl<'a, R: Read + Seek> Read for SparseSliceReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        let mut total_written = 0;

        while total_written < buf.len() {
            // 1. Emit SparseHeader bytes if remaining
            if self.header_pos < self.header_bytes.len() {
                let to_copy = std::cmp::min(
                    buf.len() - total_written,
                    self.header_bytes.len() - self.header_pos,
                );
                buf[total_written..total_written + to_copy].copy_from_slice(
                    &self.header_bytes[self.header_pos..self.header_pos + to_copy],
                );
                self.header_pos += to_copy;
                total_written += to_copy;
                continue;
            }

            if self.chunk_idx >= self.chunks.len() {
                break;
            }

            // 2a. Emit ChunkHeader bytes if remaining
            if self.chunk_header_pos < self.chunk_header_bytes.len() {
                let to_copy = std::cmp::min(
                    buf.len() - total_written,
                    self.chunk_header_bytes.len() - self.chunk_header_pos,
                );
                buf[total_written..total_written + to_copy].copy_from_slice(
                    &self.chunk_header_bytes
                        [self.chunk_header_pos..self.chunk_header_pos + to_copy],
                );
                self.chunk_header_pos += to_copy;
                total_written += to_copy;
                continue;
            }

            // 2b. Emit Chunk payload
            let chunk = &self.chunks[self.chunk_idx];
            match chunk {
                Chunk::Raw { start, size } => {
                    let remaining_payload = *size - self.payload_pos;
                    if remaining_payload > 0 {
                        if self.payload_pos == 0 {
                            if self.source.stream_position()? != *start {
                                self.source.seek(SeekFrom::Start(*start))?;
                            }
                        }
                        let to_read =
                            std::cmp::min(buf.len() - total_written, remaining_payload as usize);
                        let n =
                            self.source.read(&mut buf[total_written..total_written + to_read])?;
                        if n == 0 {
                            // If EOF reached on source earlier than expected, fill remainder with zeroes
                            let zeroes = std::cmp::min(
                                buf.len() - total_written,
                                remaining_payload as usize,
                            );
                            buf[total_written..total_written + zeroes].fill(0);
                            self.payload_pos += zeroes as u64;
                            total_written += zeroes;
                            if self.payload_pos >= *size {
                                self.chunk_idx += 1;
                                self.prepare_next_chunk_header().map_err(|e| {
                                    std::io::Error::new(std::io::ErrorKind::Other, e)
                                })?;
                            }
                            continue;
                        }
                        self.payload_pos += n as u64;
                        total_written += n;
                        if self.payload_pos >= *size {
                            self.chunk_idx += 1;
                            self.prepare_next_chunk_header()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        }
                        return Ok(total_written);
                    }
                }
                Chunk::Fill { value, .. } => {
                    if self.payload_pos < 4 {
                        let val_bytes = value.to_le_bytes();
                        let remaining = 4 - self.payload_pos as usize;
                        let to_copy = std::cmp::min(buf.len() - total_written, remaining);
                        let start = self.payload_pos as usize;
                        buf[total_written..total_written + to_copy]
                            .copy_from_slice(&val_bytes[start..start + to_copy]);
                        self.payload_pos += to_copy as u64;
                        total_written += to_copy;
                        if self.payload_pos >= 4 {
                            self.chunk_idx += 1;
                            self.prepare_next_chunk_header()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        }
                        continue;
                    }
                }
                Chunk::Crc32 { checksum } => {
                    if self.payload_pos < 4 {
                        let val_bytes = checksum.to_le_bytes();
                        let remaining = 4 - self.payload_pos as usize;
                        let to_copy = std::cmp::min(buf.len() - total_written, remaining);
                        let start = self.payload_pos as usize;
                        buf[total_written..total_written + to_copy]
                            .copy_from_slice(&val_bytes[start..start + to_copy]);
                        self.payload_pos += to_copy as u64;
                        total_written += to_copy;
                        if self.payload_pos >= 4 {
                            self.chunk_idx += 1;
                            self.prepare_next_chunk_header()
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                        }
                        continue;
                    }
                }
                Chunk::DontCare { .. } => {
                    self.chunk_idx += 1;
                    self.prepare_next_chunk_header()
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                    continue;
                }
            }

            self.chunk_idx += 1;
            self.prepare_next_chunk_header()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        }

        Ok(total_written)
    }
}

impl fmt::Display for SparseFileWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, r"SparseFileWriter: {} Chunks:", self.chunks.len())
    }
}

/// `add_sparse_chunk` takes the input vec, v and given `Chunk`, chunk, and
/// attempts to add the chunk to the end of the vec. If the current last chunk
/// is the same kind of Chunk as the `chunk`, then it will merge the two chunks
/// into one chunk.
///
/// Example: A `FillChunk` with value 0 and size 1 is the last chunk
/// in `v`, and `chunk` is a FillChunk with value 0 and size 1, after this,
/// `v`'s last element will be a FillChunk with value 0 and size 2.
fn add_sparse_chunk(r: &mut Vec<Chunk>, chunk: Chunk) {
    match r.last_mut() {
        // We've got something in the Vec... if they are both the same type,
        // merge them, otherwise, just push the new one
        Some(last) => match (&last, &chunk) {
            (Chunk::Raw { start, size }, Chunk::Raw { size: new_length, .. })
                if size.checked_add(*new_length).is_some() =>
            {
                *last = Chunk::Raw { start: *start, size: size + new_length };
                return;
            }
            (
                Chunk::Fill { start, size, value },
                Chunk::Fill { size: new_size, value: new_value, .. },
            ) if value == new_value && size.checked_add(*new_size).is_some() => {
                *last = Chunk::Fill { start: *start, size: size + new_size, value: *value };
                return;
            }
            (Chunk::DontCare { start, size }, Chunk::DontCare { size: new_size, .. })
                if size.checked_add(*new_size).is_some() =>
            {
                *last = Chunk::DontCare { start: *start, size: size + new_size };
                return;
            }
            _ => {}
        },
        None => {}
    }

    // If the chunk types differ they cannot be merged.
    // If they are both Fill but have different values, they cannot be merged.
    // Crc32 cannot be merged.
    // If we don't have any chunks then we add it
    r.push(chunk);
}

/// Reads a sparse image from `source` and expands it to its unsparsed representation in `dest`.
pub fn unsparse<W: Writer, R: Read + Seek>(
    source: &mut R,
    dest: &mut W,
) -> Result<(), SparseError> {
    let header: SparseHeader = deserialize_from(source)
        .map_err(|e| SparseError::Deserialize { ty: SparseDataType::Header, source: e })?;
    if !header.valid() {
        return Err(SparseError::InvalidHeader);
    }

    for _ in 0..header.total_chunks {
        expand_chunk(source, dest, header.blk_sz)?;
    }
    // Truncate output to its current seek offset, in case the last chunk we wrote was DontNeed.
    let offset = dest.stream_position()?;
    dest.set_len(offset)?;
    dest.flush()?;
    Ok(())
}

/// Reads a chunk from `source`, and expands it, writing the result to `dest`.
fn expand_chunk<R: Read + Seek, W: Write + Seek>(
    source: &mut R,
    dest: &mut W,
    block_size: u32,
) -> Result<(), SparseError> {
    let header: ChunkHeader = deserialize_from(source)
        .map_err(|e| SparseError::Deserialize { ty: SparseDataType::ChunkHeader, source: e })?;
    if !header.valid() {
        return Err(SparseError::InvalidChunkHeader);
    }
    let size = (header.chunk_sz * block_size) as usize;
    match header.chunk_type {
        format::CHUNK_TYPE_RAW => {
            let limit = source.stream_position()? as usize + size;
            std::io::copy(&mut LimitedReader(source, limit), dest)
                .map_err(SparseError::CopyContents)?;
        }
        format::CHUNK_TYPE_FILL => {
            let value: [u8; 4] = deserialize_from(source).map_err(|e| {
                SparseError::Deserialize { ty: SparseDataType::FillValue, source: e }
            })?;
            assert!(size % 4 == 0);
            let repeated = value.repeat(size / 4);
            dest.write_all(&repeated).map_err(SparseError::FillContents)?;
        }
        format::CHUNK_TYPE_DONT_CARE => {
            dest.seek(SeekFrom::Current(size as i64)).map_err(SparseError::SkipContents)?;
        }
        format::CHUNK_TYPE_CRC32 => {
            let _: u32 = deserialize_from(source).map_err(|e| SparseError::Deserialize {
                ty: SparseDataType::Checksum,
                source: e,
            })?;
        }
        _ => return Err(SparseError::InvalidChunkType(header.chunk_type)),
    };
    Ok(())
}

/// Takes a `sparse_file` and breaks it into multiple `SparseFileWriter` slices whose
/// serialized file size will not exceed `max_download_size`.
///
/// # Arguments
///
/// * `sparse_file` - The sparse file writer containing chunk definitions to resparse.
/// * `max_download_size` - Maximum size in bytes for each resparsed slice.
///
/// # Errors
///
/// Returns [`SparseError::MaxDownloadSizeTooSmall`] if `max_download_size` is less than or
/// equal to the block size ([`BLK_SIZE`]).
pub fn resparse(
    sparse_file: SparseFileWriter,
    max_download_size: u64,
) -> Result<Vec<SparseFileWriter>, SparseError> {
    if max_download_size <= BLK_SIZE as u64 {
        return Err(SparseError::MaxDownloadSizeTooSmall(max_download_size, BLK_SIZE));
    }
    let mut ret = Vec::<SparseFileWriter>::new();

    // File length already starts with a header for the SparseFile as
    // well as the size of a potential DontCare and Crc32 Chunk
    let sunk_file_length = format::SPARSE_HEADER_SIZE as u64
        + Chunk::DontCare { start: 0, size: BLK_SIZE.into() }.chunk_data_len() as u64
        + Chunk::Crc32 { checksum: 2345 }.chunk_data_len() as u64;

    let total_image_bytes = sparse_file.total_bytes();
    let mut chunk_pos = 0;
    let mut offset_in_raw = 0u64;
    let mut output_offset = 0u64;

    while chunk_pos < sparse_file.chunks.len() {
        log::trace!(
            "Starting a new file at chunk position: {}, offset_in_raw: {}",
            chunk_pos,
            offset_in_raw
        );

        let mut file_len = sunk_file_length;
        let mut chunks = Vec::<Chunk>::new();

        if output_offset > 0 {
            // If we already have written bytes... add a DontCare block to
            // move the pointer
            log::trace!("Adding a DontCare chunk offset: {}", output_offset);
            let dont_care = Chunk::DontCare { start: 0, size: output_offset };
            chunks.push(dont_care);
        }

        loop {
            if chunk_pos >= sparse_file.chunks.len() {
                log::trace!("Finished iterating chunks");
                break;
            }

            let chunk = &sparse_file.chunks[chunk_pos];
            match chunk {
                Chunk::Raw { start, size } => {
                    let remaining_raw = *size - offset_in_raw;
                    let chunk_header_len = format::CHUNK_HEADER_SIZE as u64;
                    let available_in_file = max_download_size.saturating_sub(file_len);

                    if available_in_file < chunk_header_len + BLK_SIZE as u64 {
                        // Cannot fit even one block in current file.
                        let remainder_size = total_image_bytes.saturating_sub(output_offset);
                        if remainder_size > 0 {
                            let dont_care =
                                Chunk::DontCare { start: output_offset, size: remainder_size };
                            chunks.push(dont_care);
                        }
                        break;
                    }

                    let max_raw_payload = ((available_in_file - chunk_header_len)
                        / BLK_SIZE as u64)
                        * BLK_SIZE as u64;
                    let to_take = std::cmp::min(remaining_raw, max_raw_payload);

                    if to_take == 0 {
                        let remainder_size = total_image_bytes.saturating_sub(output_offset);
                        if remainder_size > 0 {
                            let dont_care =
                                Chunk::DontCare { start: output_offset, size: remainder_size };
                            chunks.push(dont_care);
                        }
                        break;
                    }

                    let sub_chunk = Chunk::Raw { start: *start + offset_in_raw, size: to_take };
                    add_sparse_chunk(&mut chunks, sub_chunk);
                    file_len += chunk_header_len + to_take;
                    output_offset += to_take;
                    offset_in_raw += to_take;

                    if offset_in_raw == *size {
                        chunk_pos += 1;
                        offset_in_raw = 0;
                    } else {
                        // Current file is full.
                        let remainder_size = total_image_bytes.saturating_sub(output_offset);
                        if remainder_size > 0 {
                            let dont_care =
                                Chunk::DontCare { start: output_offset, size: remainder_size };
                            chunks.push(dont_care);
                        }
                        break;
                    }
                }
                other => {
                    let curr_chunk_data_len = other.chunk_data_len() as u64;
                    if (file_len + curr_chunk_data_len) > max_download_size {
                        let remainder_size = total_image_bytes.saturating_sub(output_offset);
                        if remainder_size > 0 {
                            let dont_care =
                                Chunk::DontCare { start: output_offset, size: remainder_size };
                            chunks.push(dont_care);
                        }
                        break;
                    }

                    add_sparse_chunk(&mut chunks, other.clone());
                    file_len += curr_chunk_data_len;
                    output_offset += other.output_size() as u64;
                    chunk_pos += 1;
                }
            }
        }

        let resparsed = SparseFileWriter::new(chunks);
        log::trace!("resparse: Adding new SparseFile: {}", resparsed);
        ret.push(resparsed);
    }

    Ok(ret)
}

/// Takes a provided `reader` and generates a set of `SparseFileWriter`s representing
/// the resparsed slices with the provided `max_download_size` constraining slice size.
///
/// # Arguments
///
/// * `reader` - The sparse reader of an existing sparse file.
/// * `max_download_size` - Maximum size in bytes that can be downloaded by the device for each slice.
///
/// # Errors
///
/// Returns [`SparseError::MaxDownloadSizeTooSmall`] if `max_download_size` is less than or
/// equal to the block size ([`BLK_SIZE`]).
pub fn resparse_sparse_img_writers<R: Read + std::io::Seek>(
    reader: &mut SparseReader<R>,
    max_download_size: u64,
) -> Result<Vec<SparseFileWriter>, SparseError> {
    log::debug!("Building writers from Reader");
    let mut chunks = vec![];
    for (chunk, _offset) in reader.chunks() {
        chunks.push(chunk.clone());
    }
    let sparse_file = SparseFileWriter::new(chunks);
    resparse(sparse_file, max_download_size)
}

/// Takes a provided `reader` and generates a set of temporary files in `dir`
/// in the Sparse image format. With the provided `max_download_size`
/// constraining file size.
///
/// # Arguments
///
/// * `reader` - The Sparse Reader of a Sparse File
/// * `dir` - Path to the directory to write the Sparse file(s).
/// * `max_download_size` - Maximum size that can be downloaded by the device.
pub fn resparse_sparse_img<R: Read + std::io::Seek>(
    reader: &mut SparseReader<R>,
    dir: &Path,
    max_download_size: u64,
) -> Result<Vec<TempPath>, SparseError> {
    let mut ret = Vec::<TempPath>::new();
    log::debug!("Resparsing sparse file");
    for re_sparsed_file in resparse_sparse_img_writers(reader, max_download_size)? {
        let (file, temp_path) = NamedTempFile::new_in(dir)?.into_parts();
        let mut file_create = File::from(file);

        log::debug!("Writing resparsed {} to disk", re_sparsed_file);
        re_sparsed_file.write(reader, &mut file_create)?;

        ret.push(temp_path);
    }

    log::debug!("Finished building sparse files");

    Ok(ret)
}

/// Returns the fill value if the entire slice consists of a single repeated 32-bit integer.
/// Returns None if the slice is empty, not a multiple of 4 bytes, or contains varying values.
#[inline]
pub(crate) fn find_fill_value(buf: &[u8]) -> Option<u32> {
    if buf.len() < 4 || buf.len() % 4 != 0 {
        return None;
    }
    let first = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    for chunk in buf[4..].chunks_exact(4) {
        if u32::from_le_bytes(chunk.try_into().unwrap()) != first {
            return None;
        }
    }
    Some(first)
}

/// Takes the given `file_to_upload` for the `named` partition and generates `SparseFileWriter`
/// slices on the fly, emitting each slice as soon as it reaches `max_download_size`.
///
/// # Arguments
///
/// * `name` - Name of the partition for the image. Used for logs only.
/// * `file_to_upload` - Path to the file to translate to sparse image format.
/// * `max_download_size` - Maximum size that can be downloaded by the device for each slice.
/// * `on_slice` - Callback invoked with each `SparseFileWriter` slice as it is completed.
///
/// # Errors
///
/// Returns [`SparseError::MaxDownloadSizeTooSmall`] if `max_download_size` is less than or
/// equal to the block size ([`BLK_SIZE`]), or an I/O error if `file_to_upload` fails to open
/// or read.
pub fn build_sparse_writers_streaming<F>(
    name: &str,
    file_to_upload: &str,
    max_download_size: u64,
    mut on_slice: F,
) -> Result<(), SparseError>
where
    F: FnMut(SparseFileWriter) -> Result<(), SparseError>,
{
    if max_download_size <= BLK_SIZE.into() {
        return Err(SparseError::MaxDownloadSizeTooSmall(max_download_size, BLK_SIZE));
    }
    if BLK_SIZE as usize % std::mem::size_of::<u32>() != 0 {
        return Err(SparseError::UnalignedDataSource(UnalignedSource::Buffer(BLK_SIZE as usize)));
    }
    log::debug!("Building sparse writers (streaming) for: {}. File: {}", name, file_to_upload);
    let mut in_file = File::open(file_to_upload)?;
    let total_image_bytes = in_file.metadata()?.len().next_multiple_of(BLK_SIZE.into());

    let sunk_file_length = u64::from(
        format::SPARSE_HEADER_SIZE
            + CHUNK_HEADER_SIZE
            + Chunk::Crc32 { checksum: 2345 }.chunk_data_len(),
    );

    let mut current_slice_chunks = Vec::<Chunk>::new();
    let mut current_slice_file_len = sunk_file_length;
    let mut output_offset = 0u64;
    let mut total_read = 0usize;

    let mut buf = [0u8; BLK_SIZE as usize];
    loop {
        let read = in_file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        // Zero-fill remainder
        buf[read..].fill(0);

        let start = total_read as u64;
        let size = buf.len().try_into().unwrap();
        let candidate_chunk = if let Some(value) = find_fill_value(&buf) {
            Chunk::Fill { start, size, value }
        } else {
            Chunk::Raw { start, size }
        };

        let candidate_data_len = u64::from(candidate_chunk.chunk_data_len());

        if current_slice_file_len + candidate_data_len > max_download_size {
            let remainder_size = total_image_bytes.saturating_sub(output_offset);
            if remainder_size > 0 {
                let dont_care = Chunk::DontCare { start: output_offset, size: remainder_size };
                current_slice_chunks.push(dont_care);
            }

            let slice_writer = SparseFileWriter::new(current_slice_chunks);
            log::trace!("Emitting completed sparse slice: {}", slice_writer);
            on_slice(slice_writer)?;

            current_slice_chunks = if output_offset > 0 {
                vec![Chunk::DontCare { start: 0, size: output_offset }]
            } else {
                Vec::new()
            };
            current_slice_file_len = sunk_file_length + u64::from(CHUNK_HEADER_SIZE);
        }

        add_sparse_chunk(&mut current_slice_chunks, candidate_chunk);
        current_slice_file_len += candidate_data_len;
        output_offset += buf.len() as u64;
        total_read += read;
    }

    if !current_slice_chunks.is_empty() {
        let slice_writer = SparseFileWriter::new(current_slice_chunks);
        log::trace!("Emitting final sparse slice: {}", slice_writer);
        on_slice(slice_writer)?;
    }

    Ok(())
}

/// Takes the given `file_to_upload` for the `named` partition and creates a
/// set of `SparseFileWriter` slices in memory with the provided `max_download_size`
/// constraining slice size.
///
/// # Arguments
///
/// * `name` - Name of the partition for the image. Used for logs only.
/// * `file_to_upload` - Path to the file to translate to sparse image format.
/// * `max_download_size` - Maximum size that can be downloaded by the device for each slice.
///
/// # Errors
///
/// Returns [`SparseError::MaxDownloadSizeTooSmall`] if `max_download_size` is less than or
/// equal to the block size ([`BLK_SIZE`]), or an I/O error if `file_to_upload` fails to open
/// or read.
pub fn build_sparse_writers(
    name: &str,
    file_to_upload: &str,
    max_download_size: u64,
) -> Result<Vec<SparseFileWriter>, SparseError> {
    let mut writers = Vec::new();
    build_sparse_writers_streaming(name, file_to_upload, max_download_size, |writer| {
        writers.push(writer);
        Ok(())
    })?;
    Ok(writers)
}

/// Takes the given `file_to_upload` for the `named` partition and creates a
/// set of temporary files in the given `dir` in Sparse Image Format. With the
/// provided `max_download_size` constraining file size.
///
/// # Arguments
///
/// * `name` - Name of the partition the image. Used for logs only.
/// * `file_to_upload` - Path to the file to translate to sparse image format.
/// * `dir` - Path to write the Sparse file(s).
/// * `max_download_size` - Maximum size that can be downloaded by the device.
pub fn build_sparse_files(
    name: &str,
    file_to_upload: &str,
    dir: &Path,
    max_download_size: u64,
) -> Result<Vec<TempPath>, SparseError> {
    let mut in_file = File::open(file_to_upload)?;
    let mut ret = Vec::<TempPath>::new();
    log::trace!("Resparsing sparse file");
    for re_sparsed_file in build_sparse_writers(name, file_to_upload, max_download_size)? {
        let (file, temp_path) = NamedTempFile::new_in(dir)?.into_parts();
        let mut file_create = File::from(file);

        log::trace!("Writing resparsed {} to disk", re_sparsed_file);
        re_sparsed_file.write(&mut in_file, &mut file_create)?;

        ret.push(temp_path);
    }

    log::debug!("Finished building sparse files");

    Ok(ret)
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    #[cfg(target_os = "linux")]
    use crate::build_sparse_files;

    use super::builder::{DataSource, SparseImageBuilder};
    use super::{
        BLK_SIZE, Chunk, NO_SOURCE, SparseFileWriter, add_sparse_chunk, resparse, unsparse,
    };
    use rand::Rng as _;
    use rand::rngs::SmallRng;
    use std::io::{Cursor, Read as _, Seek as _, SeekFrom, Write as _};
    #[cfg(target_os = "linux")]
    use std::path::Path;
    #[cfg(target_os = "linux")]
    use std::process::{Command, Stdio};
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_fill_into_bytes() {
        let mut dest = Cursor::new(Vec::<u8>::new());

        let fill_chunk = Chunk::Fill { start: 0, size: (5 * BLK_SIZE).into(), value: 365 };
        fill_chunk.write(NO_SOURCE, &mut dest, BLK_SIZE).unwrap();
        assert_eq!(dest.into_inner(), [194, 202, 0, 0, 5, 0, 0, 0, 16, 0, 0, 0, 109, 1, 0, 0]);
    }

    #[test]
    fn test_raw_into_bytes() {
        const EXPECTED_RAW_BYTES: [u8; 22] =
            [193, 202, 0, 0, 1, 0, 0, 0, 12, 16, 0, 0, 49, 50, 51, 52, 53, 0, 0, 0, 0, 0];

        let mut source = Cursor::new(Vec::<u8>::from(&b"12345"[..]));
        let mut sparse = Cursor::new(Vec::<u8>::new());
        let chunk = Chunk::Raw { start: 0, size: BLK_SIZE.into() };

        chunk.write(Some(&mut source), &mut sparse, BLK_SIZE).unwrap();
        let buf = sparse.into_inner();
        assert_eq!(buf.len(), 4108);
        assert_eq!(&buf[..EXPECTED_RAW_BYTES.len()], EXPECTED_RAW_BYTES);
        assert_eq!(&buf[EXPECTED_RAW_BYTES.len()..], &[0u8; 4108 - EXPECTED_RAW_BYTES.len()]);
    }

    #[test]
    fn test_dont_care_into_bytes() {
        let mut dest = Cursor::new(Vec::<u8>::new());
        let chunk = Chunk::DontCare { start: 0, size: (5 * BLK_SIZE).into() };

        chunk.write(NO_SOURCE, &mut dest, BLK_SIZE).unwrap();
        assert_eq!(dest.into_inner(), [195, 202, 0, 0, 5, 0, 0, 0, 12, 0, 0, 0]);
    }

    #[test]
    fn test_sparse_file_into_bytes() {
        let mut source = Cursor::new(Vec::<u8>::from(&b"123"[..]));
        let mut sparse = Cursor::new(Vec::<u8>::new());
        let mut chunks = Vec::<Chunk>::new();
        // Add a fill chunk
        let fill = Chunk::Fill { start: 0, size: 4096, value: 5 };
        chunks.push(fill);
        // Add a raw chunk
        let raw = Chunk::Raw { start: 0, size: 12288 };
        chunks.push(raw);
        // Add a dontcare chunk
        let dontcare = Chunk::DontCare { start: 0, size: 4096 };
        chunks.push(dontcare);

        let sparsefile = SparseFileWriter::new(chunks);
        sparsefile.write(&mut source, &mut sparse).unwrap();

        sparse.seek(SeekFrom::Start(0)).unwrap();
        let mut unsparsed = Cursor::new(Vec::<u8>::new());
        unsparse(&mut sparse, &mut unsparsed).unwrap();
        let buf = unsparsed.into_inner();
        assert_eq!(buf.len(), 4096 + 12288 + 4096);
        {
            let chunks = buf[..4096].chunks(4);
            for chunk in chunks {
                assert_eq!(chunk, &[5u8, 0, 0, 0]);
            }
        }
        assert_eq!(&buf[4096..4099], b"123");
        assert_eq!(&buf[4099..16384], &[0u8; 12285]);
        assert_eq!(&buf[16384..], &[0u8; 4096]);
    }

    ////////////////////////////////////////////////////////////////////////////
    // Tests for resparse

    #[test]
    fn test_resparse_bails_on_too_small_size() {
        let sparse = SparseFileWriter::new(Vec::<Chunk>::new());
        assert!(resparse(sparse, 4095).is_err());
    }

    #[test]
    fn test_resparse_splits() {
        let max_download_size = 4096 * 2;

        let mut chunks = Vec::<Chunk>::new();
        chunks.push(Chunk::Raw { start: 0, size: 4096 });
        chunks.push(Chunk::Fill { start: 4096, size: 4096, value: 2 });
        // We want 2 sparse files with the second sparse file having a
        // DontCare chunk and then this chunk
        chunks.push(Chunk::Raw { start: 8192, size: 4096 });

        let input_sparse_file = SparseFileWriter::new(chunks);
        let resparsed_files = resparse(input_sparse_file, max_download_size).unwrap();
        assert_eq!(2, resparsed_files.len());

        assert_eq!(3, resparsed_files[0].chunks.len());
        assert_eq!(Chunk::Raw { start: 0, size: 4096 }, resparsed_files[0].chunks[0]);
        assert_eq!(Chunk::Fill { start: 4096, size: 4096, value: 2 }, resparsed_files[0].chunks[1]);
        assert_eq!(Chunk::DontCare { start: 8192, size: 4096 }, resparsed_files[0].chunks[2]);

        assert_eq!(2, resparsed_files[1].chunks.len());
        assert_eq!(Chunk::DontCare { start: 0, size: 8192 }, resparsed_files[1].chunks[0]);
        assert_eq!(Chunk::Raw { start: 8192, size: 4096 }, resparsed_files[1].chunks[1]);
    }

    #[test]
    fn test_resparse_splits_large_raw_chunk() {
        // A single 16KB raw chunk resparsed with max download size 8KB
        let max_download_size = 4096 * 2;
        let mut chunks = Vec::<Chunk>::new();
        chunks.push(Chunk::Raw { start: 0, size: 16384 });

        let input_sparse_file = SparseFileWriter::new(chunks);
        let resparsed_files = resparse(input_sparse_file, max_download_size).unwrap();

        // Should split into 3 files:
        // File 0: Raw [0..4096], DontCare [4096..16384] (or Raw 8192 if budget allows)
        // Check total blocks and logical expansion
        let mut total_output = 0;
        for file in &resparsed_files {
            assert!(file.total_blocks() * 4096 <= 16384 + 4096);
            for chunk in &file.chunks {
                if let Chunk::Raw { size, .. } = chunk {
                    total_output += size;
                }
            }
        }
        assert_eq!(total_output, 16384);
    }

    ////////////////////////////////////////////////////////////////////////////
    // Tests for add_sparse_chunk

    #[test]
    fn test_add_sparse_chunk_adds_empty() {
        let init_vec = Vec::<Chunk>::new();
        let mut res = init_vec.clone();
        add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: 4096, value: 1 });
        assert_eq!(0, init_vec.len());
        assert_ne!(init_vec, res);
        assert_eq!(Chunk::Fill { start: 0, size: 4096, value: 1 }, res[0]);
    }

    #[test]
    fn test_add_sparse_chunk_fill() {
        // Test they merge.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Fill { start: 0, size: 8192, value: 1 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: 8192, value: 1 });
            assert_eq!(1, res.len());
            assert_eq!(Chunk::Fill { start: 0, size: 16384, value: 1 }, res[0]);
        }

        // Test don't merge on different value.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Fill { start: 0, size: 4096, value: 1 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: 4096, value: 2 });
            assert_ne!(res, init_vec);
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::Fill { start: 0, size: 4096, value: 1 },
                    Chunk::Fill { start: 0, size: 4096, value: 2 }
                ]
            );
        }

        // Test don't merge on different type.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Fill { start: 0, size: 4096, value: 2 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::DontCare { start: 0, size: 4096 });
            assert_ne!(res, init_vec);
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::Fill { start: 0, size: 4096, value: 2 },
                    Chunk::DontCare { start: 0, size: 4096 }
                ]
            );
        }

        // Test don't merge when too large.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Fill { start: 0, size: 4096, value: 1 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: u64::MAX - 4095, value: 1 });
            assert_ne!(res, init_vec);
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::Fill { start: 0, size: 4096, value: 1 },
                    Chunk::Fill { start: 0, size: u64::MAX - 4095, value: 1 }
                ]
            );
        }
    }

    #[test]
    fn test_add_sparse_chunk_dont_care() {
        // Test they merge.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::DontCare { start: 0, size: 4096 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::DontCare { start: 0, size: 4096 });
            assert_eq!(1, res.len());
            assert_eq!(Chunk::DontCare { start: 0, size: 8192 }, res[0]);
        }

        // Test they don't merge on different type.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::DontCare { start: 0, size: 4096 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: 4096, value: 1 });
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::DontCare { start: 0, size: 4096 },
                    Chunk::Fill { start: 0, size: 4096, value: 1 }
                ]
            );
        }

        // Test they don't merge when too large.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::DontCare { start: 0, size: 4096 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::DontCare { start: 0, size: u64::MAX - 4095 });
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::DontCare { start: 0, size: 4096 },
                    Chunk::DontCare { start: 0, size: u64::MAX - 4095 }
                ]
            );
        }
    }

    #[test]
    fn test_add_sparse_chunk_raw() {
        // Test they merge.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Raw { start: 0, size: 12288 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Raw { start: 0, size: 16384 });
            assert_eq!(1, res.len());
            assert_eq!(Chunk::Raw { start: 0, size: 28672 }, res[0]);
        }

        // Test they don't merge on different type.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Raw { start: 0, size: 12288 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 3, size: 8192, value: 1 });
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::Raw { start: 0, size: 12288 },
                    Chunk::Fill { start: 3, size: 8192, value: 1 }
                ]
            );
        }

        // Test they don't merge when too large.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Raw { start: 0, size: 4096 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Raw { start: 0, size: u64::MAX - 4095 });
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [
                    Chunk::Raw { start: 0, size: 4096 },
                    Chunk::Raw { start: 0, size: u64::MAX - 4095 }
                ]
            );
        }
    }

    #[test]
    fn test_add_sparse_chunk_crc32() {
        // Test they don't merge on same type (Crc32 is special).
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Crc32 { checksum: 1234 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Crc32 { checksum: 2345 });
            assert_eq!(2, res.len());
            assert_eq!(res, [Chunk::Crc32 { checksum: 1234 }, Chunk::Crc32 { checksum: 2345 }]);
        }

        // Test they don't merge on different type.
        {
            let mut init_vec = Vec::<Chunk>::new();
            init_vec.push(Chunk::Crc32 { checksum: 1234 });
            let mut res = init_vec.clone();
            add_sparse_chunk(&mut res, Chunk::Fill { start: 0, size: 4096, value: 1 });
            assert_eq!(2, res.len());
            assert_eq!(
                res,
                [Chunk::Crc32 { checksum: 1234 }, Chunk::Fill { start: 0, size: 4096, value: 1 }]
            );
        }
    }

    ////////////////////////////////////////////////////////////////////////////
    // Integration
    //

    #[test]
    fn test_roundtrip() {
        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, _temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(1 * 4096, 0);
        rng.fill_bytes(&mut buf);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let content_size = buf.len();

        // build a sparse file
        let mut sparse_file = NamedTempFile::new_in(&tmpdir).unwrap().into_file();
        SparseImageBuilder::new()
            .add_source(DataSource::Buffer(Box::new([0xffu8; 8192])))
            .add_source(DataSource::Reader { reader: Box::new(file), size: content_size as u64 })
            .add_source(DataSource::Fill(0xaaaa_aaaau32, 1024))
            .add_source(DataSource::Skip(16384))
            .build(&mut sparse_file)
            .expect("Build sparse image failed");
        sparse_file.seek(SeekFrom::Start(0)).unwrap();

        let mut orig_file = NamedTempFile::new_in(&tmpdir).unwrap().into_file();
        unsparse(&mut sparse_file, &mut orig_file).expect("unsparse failed");
        orig_file.seek(SeekFrom::Start(0)).unwrap();

        let mut unsparsed_bytes = vec![];
        orig_file.read_to_end(&mut unsparsed_bytes).expect("Failed to read unsparsed image");
        assert_eq!(unsparsed_bytes.len(), 8192 + 20480 + content_size);
        assert_eq!(&unsparsed_bytes[..8192], &[0xffu8; 8192]);
        assert_eq!(&unsparsed_bytes[8192..8192 + content_size], &buf[..]);
        assert_eq!(&unsparsed_bytes[8192 + content_size..12288 + content_size], &[0xaau8; 4096]);
        assert_eq!(&unsparsed_bytes[12288 + content_size..], &[0u8; 16384]);
    }

    #[test]
    /// test_with_simg2img is a "round trip" test that does the following
    ///
    /// 1. Generates a pseudorandom temporary file
    /// 2. Builds sparse files out of it
    /// 3. Uses the android tool simg2img to take the sparse files and generate
    ///    the "original" image file out of them.
    /// 4. Asserts the originally created file and the one created by simg2img
    ///    have binary equivalent contents.
    ///
    /// This gives us a reasonable expectation of correctness given that the
    /// Android-provided sparse tools are able to interpret our sparse images.
    #[cfg(target_os = "linux")]
    fn test_with_simg2img() {
        let simg2img_path = Path::new("./host_x64/test_data/storage/sparse/simg2img");
        assert!(
            Path::exists(simg2img_path),
            "simg2img binary must exist at {}",
            simg2img_path.display()
        );

        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        // Dont want it to neatly fit a block size
        buf.resize(50 * 4096 + 1244, 0);
        rng.fill_bytes(&mut buf);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        // build a sparse file
        let files = build_sparse_files(
            "test",
            temp_path.to_path_buf().to_str().expect("Should succeed"),
            tmpdir.path(),
            4096 * 2,
        )
        .unwrap();

        let mut simg2img_output = tmpdir.path().to_path_buf();
        simg2img_output.push("output");

        let mut simg2img = Command::new(simg2img_path)
            .args(&files[..])
            .arg(&simg2img_output)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn simg2img");
        let res = simg2img.wait().expect("simg2img did was not running");
        assert!(res.success(), "simg2img did not succeed");
        let mut simg2img_stdout = simg2img.stdout.take().expect("Get stdout from simg2img");
        let mut simg2img_stderr = simg2img.stderr.take().expect("Get stderr from simg2img");

        let mut stdout = String::new();
        simg2img_stdout.read_to_string(&mut stdout).expect("Reading simg2img stdout");
        assert_eq!(stdout, "");

        let mut stderr = String::new();
        simg2img_stderr.read_to_string(&mut stderr).expect("Reading simg2img stderr");
        assert_eq!(stderr, "");

        let simg2img_output_bytes =
            std::fs::read(simg2img_output).expect("Failed to read simg2img output");

        assert_eq!(
            buf,
            simg2img_output_bytes[0..buf.len()],
            "Output from simg2img should match our generated file"
        );

        assert_eq!(
            simg2img_output_bytes[buf.len()..],
            vec![0u8; simg2img_output_bytes.len() - buf.len()],
            "The remainder of our simg2img_output_bytes should be 0"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_resparse_from_sparse() {
        use crate::reader::SparseReader;
        use crate::resparse_sparse_img;

        let simg2img_path = Path::new("./host_x64/test_data/storage/sparse/simg2img");
        assert!(
            Path::exists(simg2img_path),
            "simg2img binary must exist at {}",
            simg2img_path.display()
        );

        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, _temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(10 * 4096, 0);
        rng.fill_bytes(&mut buf);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let content_size = buf.len();

        // build a sparse file
        let sparse_file_tmp = NamedTempFile::new_in(&tmpdir).unwrap();
        let mut sparse_file = sparse_file_tmp.into_file();
        SparseImageBuilder::new()
            .add_source(DataSource::Buffer(Box::new([0xffu8; 4096 * 2])))
            .add_source(DataSource::Reader { reader: Box::new(file), size: content_size as u64 })
            .add_source(DataSource::Fill(0xaaaa_aaaau32, 1024))
            .add_source(DataSource::Skip(16384))
            .build(&mut sparse_file)
            .expect("Build sparse image failed");
        sparse_file.seek(SeekFrom::Start(0)).unwrap();

        let mut reader = SparseReader::new(sparse_file).expect("create reader");

        let files = resparse_sparse_img(&mut reader, tmpdir.path(), 4096 * 3).unwrap();

        // Re build the image from the sparse files
        let mut simg2img_output_sparsed = tmpdir.path().to_path_buf();
        simg2img_output_sparsed.push("output_sparsed");

        let mut simg2img_sparsed = Command::new(simg2img_path)
            .args(&files[..])
            .arg(&simg2img_output_sparsed)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn simg2img");
        let res = simg2img_sparsed.wait().expect("simg2img did was not running");
        assert!(res.success(), "simg2img did not succeed");
        let mut simg2img_stdout = simg2img_sparsed.stdout.take().expect("Get stdout from simg2img");
        let mut simg2img_stderr = simg2img_sparsed.stderr.take().expect("Get stderr from simg2img");

        let mut stdout = String::new();
        simg2img_stdout.read_to_string(&mut stdout).expect("Reading simg2img stdout");
        assert_eq!(stdout, "");

        let mut stderr = String::new();
        simg2img_stderr.read_to_string(&mut stderr).expect("Reading simg2img stderr");
        assert_eq!(stderr, "");
    }

    #[test]
    fn test_find_fill_value() {
        assert_eq!(super::find_fill_value(&[]), None);
        assert_eq!(super::find_fill_value(&[1, 2, 3]), None);
        assert_eq!(super::find_fill_value(&[1, 2, 3, 4, 5]), None);

        let mut buf = [0u8; 4096];
        assert_eq!(super::find_fill_value(&buf), Some(0));

        buf.fill(0xaa);
        assert_eq!(super::find_fill_value(&buf), Some(0xaaaa_aaaa));

        for chunk in buf.chunks_exact_mut(4) {
            chunk.copy_from_slice(&0x1234_5678u32.to_le_bytes());
        }
        assert_eq!(super::find_fill_value(&buf), Some(0x1234_5678));

        // Mismatch at start
        buf[0] = 0x00;
        assert_eq!(super::find_fill_value(&buf), None);

        // Restore and mismatch in middle
        buf[0] = 0x78;
        buf[2048] = 0x00;
        assert_eq!(super::find_fill_value(&buf), None);

        // Restore and mismatch at end
        buf[2048] = 0x78;
        buf[4095] = 0x00;
        assert_eq!(super::find_fill_value(&buf), None);
    }

    #[test]
    fn test_sparse_slice_reader_matches_write() {
        let mut source_data = Vec::<u8>::new();
        source_data.resize(4096 * 4, 0);
        let mut rng: SmallRng = rand::make_rng();
        rng.fill_bytes(&mut source_data);

        let mut chunks = Vec::<Chunk>::new();
        chunks.push(Chunk::Raw { start: 0, size: 4096 * 2 });
        chunks.push(Chunk::Fill { start: 4096 * 2, size: 4096, value: 0x1234_5678 });
        chunks.push(Chunk::DontCare { start: 4096 * 3, size: 4096 });
        chunks.push(Chunk::Raw { start: 4096 * 3, size: 4096 });

        let writer = SparseFileWriter::new(chunks);

        // 1. Write via SparseFileWriter::write
        let mut written_bytes = Cursor::new(Vec::<u8>::new());
        let mut source_cursor = Cursor::new(source_data.clone());
        writer.write(&mut source_cursor, &mut written_bytes).unwrap();
        let expected = written_bytes.into_inner();

        // 2. Read via SparseSliceReader
        let mut slice_source = Cursor::new(source_data.clone());
        let mut slice_reader = writer.slice_reader(&mut slice_source).unwrap();
        let mut read_bytes = Vec::<u8>::new();
        slice_reader.read_to_end(&mut read_bytes).unwrap();

        assert_eq!(expected, read_bytes);
        assert_eq!(read_bytes.len() as u64, writer.file_size());
    }
}
