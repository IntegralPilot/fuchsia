// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `fsverity_merkle` contains types and methods for building and working with fsverity merkle trees.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

mod builder;
pub use crate::builder::MerkleTreeBuilder;

mod tree;
pub use crate::tree::MerkleTree;

mod util;
pub use crate::util::{
    FsVerityDescriptor, FsVerityDescriptorRaw, FsVerityHasher, FsVerityHasherOptions,
};

mod verifier;
pub use crate::verifier::MerkleVerifier;

pub const SHA256_SALT_PADDING: u8 = 64;
pub const SHA512_SALT_PADDING: u8 = 128;

/// A cryptographic hash digest used in a Merkle tree.
pub trait FsVerityHash:
    FromBytes
    + IntoBytes
    + KnownLayout
    + Immutable
    + Clone
    + Copy
    + std::fmt::Debug
    + Send
    + Sync
    + PartialEq
    + Eq
{
}

/// SHA-256 verity hash.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct Sha256Hash([u8; 32]);

impl FsVerityHash for Sha256Hash {}

impl From<[u8; 32]> for Sha256Hash {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<Sha256Hash> for [u8; 32] {
    fn from(hash: Sha256Hash) -> Self {
        hash.0
    }
}

/// SHA-512 verity hash.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, KnownLayout, Immutable)]
pub struct Sha512Hash([u8; 64]);

impl FsVerityHash for Sha512Hash {}

impl From<[u8; 64]> for Sha512Hash {
    fn from(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }
}

impl From<Sha512Hash> for [u8; 64] {
    fn from(hash: Sha512Hash) -> Self {
        hash.0
    }
}

/// Compute a merkle tree from a `&[u8]` for a particular hasher.
pub fn from_slice(slice: &[u8], hasher: FsVerityHasher) -> MerkleTree {
    match hasher {
        FsVerityHasher::Sha256(_) => from_slice_impl::<Sha256Hash>(slice, hasher),
        FsVerityHasher::Sha512(_) => from_slice_impl::<Sha512Hash>(slice, hasher),
    }
}

fn from_slice_impl<D: FsVerityHash>(slice: &[u8], hasher: FsVerityHasher) -> MerkleTree {
    let mut builder = MerkleTreeBuilder::<D>::new(hasher);
    builder.write(slice);
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_slice_sha256() {
        let file = vec![0xFF; 2105344];
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let expected = MerkleTree::from_data(&file[..], hasher.clone());
        let actual = from_slice(&file[..], hasher);
        assert_eq!(expected.root(), actual.root());
    }

    #[test]
    fn test_from_slice_sha512() {
        let file = vec![0xFF; 2105344];
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let expected = MerkleTree::from_data(&file[..], hasher.clone());
        let actual = from_slice(&file[..], hasher);
        assert_eq!(expected.root(), actual.root());
    }
}
