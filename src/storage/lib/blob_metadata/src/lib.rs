// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod zerocopy_serialization;

use fprint::TypeFingerprint;
use fuchsia_merkle::{Hash, MerkleVerifier};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TypeFingerprint)]
pub struct BlobMetadataUnversioned {
    pub hashes: Vec<[u8; 32]>,
    pub chunk_size: u64,
    pub compressed_offsets: Vec<u64>,
    pub uncompressed_size: u64,
}

pub type BlobMetadata = BlobMetadataV53;
pub type BlobFormat = BlobFormatV53;
pub type MerkleLeaves = Vec<[u8; 32]>;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TypeFingerprint)]
pub struct BlobMetadataV53 {
    #[serde(with = "zerocopy_serialization")]
    pub merkle_leaves: MerkleLeaves,
    pub format: BlobFormatV53,
}

impl BlobMetadataV53 {
    pub fn empty() -> Self {
        Self { merkle_leaves: Vec::new(), format: BlobFormatV53::Uncompressed }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::empty()
    }

    pub fn into_merkle_verifier(self, root: Hash) -> Result<MerkleVerifier, anyhow::Error> {
        let hashes = if self.merkle_leaves.is_empty() {
            Box::new([root])
        } else {
            self.merkle_leaves.into_iter().map(Into::into).collect::<Box<[Hash]>>()
        };
        Ok(MerkleVerifier::new(root, hashes)?)
    }
}

impl From<BlobMetadataUnversioned> for BlobMetadataV53 {
    fn from(old: BlobMetadataUnversioned) -> Self {
        if old.compressed_offsets.is_empty() {
            Self { merkle_leaves: old.hashes, format: BlobFormat::Uncompressed }
        } else {
            Self {
                merkle_leaves: old.hashes,
                format: BlobFormat::ChunkedZstd {
                    uncompressed_size: old.uncompressed_size,
                    chunk_size: old.chunk_size,
                    compressed_offsets: old.compressed_offsets,
                },
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, TypeFingerprint)]
pub enum BlobFormatV53 {
    Uncompressed,
    ChunkedZstd { uncompressed_size: u64, chunk_size: u64, compressed_offsets: Vec<u64> },
    ChunkedLz4 { uncompressed_size: u64, chunk_size: u64, compressed_offsets: Vec<u64> },
}
