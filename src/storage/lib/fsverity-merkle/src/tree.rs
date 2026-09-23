// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builder::MerkleTreeBuilder;
use crate::util::FsVerityHasher;
use crate::{FsVerityHash, Sha256Hash, Sha512Hash};
use std::io;

/// A `MerkleTree` contains levels of hashes that can be used to verify the integrity of data.
///
/// While a single hash could be used to integrity check some data, if the data (or hash) is
/// corrupt, a single hash can not determine what part of the data is corrupt. A `MerkleTree`,
/// however, contains a hash for every block of data, allowing it to identify which blocks of
/// data are corrupt. A `MerkleTree` also allows individual blocks of data to be verified without
/// having to verify the rest of the data.
///
/// Furthermore, a `MerkleTree` contains multiple levels of hashes, where each level
/// contains hashes of blocks of hashes of the lower level. The top level always contains a
/// single hash, the merkle root. This tree of hashes allows a `MerkleTree` to determine which of
/// its own hashes are corrupt, if any.
///
/// # Structure Details
///
/// A merkle tree contains levels. A level is a row of the tree, starting at 0 and counting upward.
/// Level 0 represents the leaves of the tree which contain hashes of chunks of the input stream.
/// Each level consists of a hash for each block of hashes from the previous level (or, for
/// level 0, each block of data).
///
/// While building a `MerkleTree`, callers pass in an `FsverityHasher` which hashes based on a
/// particular algorithm and contains the necessary parameters to compute the merkle tree. The
/// `block size` is determined by the filesystem and the `salt` by the FsverityMetadata struct
/// stored in fxfs. When computing a hash, if `salt`.len() > 0, the block of data (or hashes) is
/// prepended by the `salt`.
///
/// For level 0, the length of the block is `block size`, except for the last block, which may be
/// less than `block size`. All other levels use a block length of `block size`.
#[derive(Clone, Eq, PartialEq, Debug)]
pub struct MerkleTree {
    levels: Vec<Box<[u8]>>,
    hasher: FsVerityHasher,
}

impl MerkleTree {
    /// Creates a `MerkleTree` from data.
    pub fn from_data(data: &[u8], hasher: FsVerityHasher) -> Self {
        match hasher {
            FsVerityHasher::Sha256(_) => Self::from_data_impl::<Sha256Hash>(data, hasher),
            FsVerityHasher::Sha512(_) => Self::from_data_impl::<Sha512Hash>(data, hasher),
        }
    }

    fn from_data_impl<D: FsVerityHash>(data: &[u8], hasher: FsVerityHasher) -> Self {
        let mut builder = MerkleTreeBuilder::<D>::new(hasher);
        builder.write(data);
        builder.finish()
    }

    // Creates a `MerkleTree` from a well-formed tree of hashes.
    //
    // This is used by `MerkleTreeBuilder` to construct the tree.
    //
    // A tree of hashes is well-formed iff:
    // - The length of the last level is 1.
    // - The length of every hash level is the length of the prior hash level divided by
    //   hashes_per_block (`block size` / `digest length`), rounded up to the nearest
    //   integer.
    pub(crate) fn from_levels(levels: Vec<Box<[u8]>>, hasher: FsVerityHasher) -> MerkleTree {
        MerkleTree { levels, hasher }
    }

    /// The root hash of the merkle tree.
    pub fn root(&self) -> &[u8] {
        &self.levels[self.levels.len() - 1][..]
    }

    /// Returns the levels of the tree as flat byte slices, from leaves (level 0) to root.
    pub fn levels(&self) -> &[Box<[u8]>] {
        &self.levels[..]
    }

    /// Returns the raw bytes of the leaf hashes (level 0 of the tree).
    pub fn leaf_hashes(&self) -> &[u8] {
        &self.levels[0][..]
    }

    /// Creates a `MerkleTree` from all of the bytes of a `Read`er.
    pub fn from_reader(
        reader: impl std::io::Read,
        hasher: FsVerityHasher,
    ) -> Result<MerkleTree, io::Error> {
        match hasher {
            FsVerityHasher::Sha256(_) => Self::from_reader_impl::<Sha256Hash>(reader, hasher),
            FsVerityHasher::Sha512(_) => Self::from_reader_impl::<Sha512Hash>(reader, hasher),
        }
    }

    fn from_reader_impl<D: FsVerityHash>(
        mut reader: impl std::io::Read,
        hasher: FsVerityHasher,
    ) -> Result<MerkleTree, io::Error> {
        let block_size = hasher.block_size() as usize;
        let mut builder = MerkleTreeBuilder::<D>::new(hasher);
        let mut buf = vec![0u8; block_size];
        loop {
            let size = reader.read(&mut buf)?;
            if size == 0 {
                break;
            }
            builder.write(&buf[0..size]);
        }
        Ok(builder.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FsVerityHasherOptions;
    use hex::FromHex;

    impl MerkleTree {
        /// Given the index of a block of data, lookup its hash.
        fn leaf_hash(&self, block: usize) -> &[u8] {
            let hash_size = self.hasher.hash_size();
            let start = block * hash_size;
            &self.levels[0][start..start + hash_size]
        }
    }

    #[test]
    fn test_single_full_hash_block_sha256() {
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let hashes_per_block = hasher.block_size() / hasher.hash_size();
        let mut leafs = Vec::new();
        let mut expected_leafs = Vec::new();
        {
            let block = vec![0xFF; hasher.block_size()];
            for _i in 0..hashes_per_block {
                leafs.push(hasher.hash_block(&block));
                expected_leafs.push(hasher.hash_block(&block));
            }
        }
        let root = hasher.hash_hashes(&leafs);

        // Convert Vec<Vec<u8>> to flat Box<[u8]> for the test
        let flat_leafs = leafs.concat().into_boxed_slice();
        let flat_root = root.clone().into_boxed_slice();

        let tree: MerkleTree = MerkleTree::from_levels(vec![flat_leafs, flat_root], hasher.clone());
        assert_eq!(tree.root(), root);
        for (i, leaf) in expected_leafs.iter().enumerate().take(hashes_per_block) {
            assert_eq!(tree.leaf_hash(i), leaf);
        }
    }

    #[test]
    fn test_single_full_hash_block_sha512() {
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let hashes_per_block = hasher.block_size() / hasher.hash_size();
        let mut leafs = Vec::new();
        let mut expected_leafs = Vec::new();
        {
            let block = vec![0xFF; hasher.block_size()];
            for _i in 0..hashes_per_block {
                leafs.push(hasher.hash_block(&block));
                expected_leafs.push(hasher.hash_block(&block));
            }
        }
        let root = hasher.hash_hashes(&leafs);

        // Convert Vec<Vec<u8>> to flat Box<[u8]> for the test
        let flat_leafs = leafs.concat().into_boxed_slice();
        let flat_root = root.clone().into_boxed_slice();

        let tree: MerkleTree = MerkleTree::from_levels(vec![flat_leafs, flat_root], hasher.clone());
        assert_eq!(tree.root(), root);
        for (i, leaf) in expected_leafs.iter().enumerate().take(hashes_per_block) {
            assert_eq!(tree.leaf_hash(i), leaf);
        }
    }

    #[test]
    fn test_from_reader_empty_sha256() {
        let data_to_hash = [0x00u8; 0];
        let tree = MerkleTree::from_reader(
            &data_to_hash[..],
            FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)),
        )
        .unwrap();
        let expected: [u8; 32] =
            FromHex::from_hex("0000000000000000000000000000000000000000000000000000000000000000")
                .unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_from_reader_empty_sha512() {
        let data_to_hash = [0x00u8; 0];
        let tree = MerkleTree::from_reader(
            &data_to_hash[..],
            FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)),
        )
        .unwrap();
        let expected: [u8; 64] = FromHex::from_hex("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_from_reader_oneblock_sha256() {
        let data_to_hash = [0xffu8; 8192];
        let tree = MerkleTree::from_reader(
            &data_to_hash[..],
            FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)),
        )
        .unwrap();
        let expected: [u8; 32] =
            FromHex::from_hex("e9c09b505561b9509f93b5c7990ed41427f708480c56306453d505e94076d600")
                .unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_from_reader_oneblock_sha512() {
        let data_to_hash = [0xffu8; 8192];
        let tree = MerkleTree::from_reader(
            &data_to_hash[..],
            FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)),
        )
        .unwrap();
        let expected: [u8; 64] = FromHex::from_hex("22750472f522bf68a1fe2a66ee1ac57759b322c634d931097b3751e3cd9fe9dd2d8f551631922bf8f675e4b5e3a38e6db11c7df0e5053e80ffbac2c2d7a0105b").unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_from_reader_unaligned_sha256() {
        let size = 2_109_440usize;
        let mut the_bytes = Vec::with_capacity(size);
        the_bytes.extend(std::iter::repeat(0xff).take(size));
        let tree = MerkleTree::from_reader(
            &the_bytes[..],
            FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 8192)),
        )
        .unwrap();
        let expected: [u8; 32] =
            FromHex::from_hex("fc21b1fbf53a4175470a7328085b5a03b2c87771cda6f1a4dbd1d1d5ce8babd5")
                .unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_from_reader_unaligned_sha512() {
        let size = 2_109_440usize;
        let mut the_bytes = Vec::with_capacity(size);
        the_bytes.extend(std::iter::repeat(0xff).take(size));
        let tree = MerkleTree::from_reader(
            &the_bytes[..],
            FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 8192)),
        )
        .unwrap();
        let expected: [u8; 64] = FromHex::from_hex("e0b048b63e814157443f42ccef9093482cee056f6afea5b9e26b772effa5077a8bac34ee8f7a877bf219e0f45a999154b0600a319c4bd7d0c9b59f8d17ce0f75").unwrap();
        assert_eq!(tree.root(), expected);
    }
}
