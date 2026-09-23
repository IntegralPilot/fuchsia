// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use static_assertions::const_assert_eq;

/// `SparseHeader` represents the header section of a `SparseFile`
#[derive(Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct SparseHeader {
    /// Magic Number.
    pub magic: u32,
    /// Highest Major Version number supported.
    pub major_version: u16,
    /// Lowest Minor Version number supported.
    pub minor_version: u16,
    /// Size of the Header. (Defaults to 0)
    pub file_hdr_sz: u16,
    /// Size of the Header per-chunk. (Defaults to 0)
    pub chunk_hdr_sz: u16,
    /// Size of each block (Defaults to 4096)
    pub blk_sz: u32,
    /// Total number of blocks in the output image
    pub total_blks: u32,
    /// Total number of chunks.
    pub total_chunks: u32,
    /// Image Checksum... unused
    pub image_checksum: u32,
}

pub const SPARSE_HEADER_SIZE: u32 = std::mem::size_of::<SparseHeader>() as u32;
const_assert_eq!(SPARSE_HEADER_SIZE, 28);

impl SparseHeader {
    pub fn new(blk_sz: u32, total_blks: u32, total_chunks: u32) -> SparseHeader {
        SparseHeader {
            magic: SPARSE_HEADER_MAGIC,
            major_version: MAJOR_VERSION,
            minor_version: MINOR_VERSION,
            file_hdr_sz: std::mem::size_of::<SparseHeader>() as u16,
            chunk_hdr_sz: std::mem::size_of::<ChunkHeader>() as u16,
            blk_sz,
            total_blks,
            total_chunks,
            image_checksum: CHECKSUM, // Checksum verification unused
        }
    }

    pub fn valid(&self) -> bool {
        self.magic == SPARSE_HEADER_MAGIC
            && self.major_version == MAJOR_VERSION
            && self.minor_version == MINOR_VERSION
    }
}

/// `ChunkHeader` represents the header portion of a Chunk.
#[derive(Debug, Serialize, Deserialize)]
#[repr(C)]
pub struct ChunkHeader {
    pub chunk_type: u16,
    reserved1: u16,
    pub chunk_sz: u32,
    pub total_sz: u32,
}

pub const CHUNK_HEADER_SIZE: u32 = std::mem::size_of::<ChunkHeader>() as u32;
const_assert_eq!(CHUNK_HEADER_SIZE, 12);

pub const CHUNK_TYPE_RAW: u16 = 0xCAC1;
pub const CHUNK_TYPE_FILL: u16 = 0xCAC2;
pub const CHUNK_TYPE_DONT_CARE: u16 = 0xCAC3;
pub const CHUNK_TYPE_CRC32: u16 = 0xCAC4;

impl ChunkHeader {
    pub fn new(chunk_type: u16, reserved1: u16, chunk_sz: u32, total_sz: u32) -> ChunkHeader {
        ChunkHeader { chunk_type, reserved1, chunk_sz, total_sz }
    }

    pub fn valid(&self) -> bool {
        self.chunk_type == CHUNK_TYPE_RAW
            || self.chunk_type == CHUNK_TYPE_FILL
            || self.chunk_type == CHUNK_TYPE_DONT_CARE
            || self.chunk_type == CHUNK_TYPE_CRC32
    }
}

// Header constants.
pub const SPARSE_HEADER_MAGIC: u32 = 0xED26FF3A;
/// Maximum Major Version Supported.
const MAJOR_VERSION: u16 = 0x1;
// Minimum Minor Version Supported.
const MINOR_VERSION: u16 = 0x0;
/// The Checksum... hardcoded not used.
const CHECKSUM: u32 = 0xCAFED00D;
