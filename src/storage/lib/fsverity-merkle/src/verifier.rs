// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::builder::MerkleTreeBuilder;
use crate::util::FsVerityHasher;
use crate::{FsVerityHash, Sha256Hash, Sha512Hash};
use zx_status::Status;

use zerocopy::FromBytes;

/// Verifies data blocks against a pre-validated Merkle tree.
#[derive(Debug, Clone)]
pub struct MerkleVerifier {
    hasher: FsVerityHasher,
    leaf_hashes: Box<[u8]>,
}

impl MerkleVerifier {
    /// Constructs a `MerkleVerifier` from the expected root, leaf hashes, and hasher.
    ///
    /// Returns `INVALID_ARGS` if the lengths are incorrect or alignment checks fail.
    /// Returns `IO_DATA_INTEGRITY` if the leaf hashes do not match the expected root.
    pub fn new(
        expected_root: &[u8],
        leaf_hashes: Box<[u8]>,
        hasher: FsVerityHasher,
    ) -> Result<Self, Status> {
        match hasher {
            FsVerityHasher::Sha256(_) => {
                Self::validate_root::<Sha256Hash>(expected_root, &leaf_hashes, &hasher)?;
            }
            FsVerityHasher::Sha512(_) => {
                Self::validate_root::<Sha512Hash>(expected_root, &leaf_hashes, &hasher)?;
            }
        }

        Ok(Self { hasher, leaf_hashes })
    }

    fn validate_root<D: FsVerityHash>(
        expected_root_bytes: &[u8],
        leaf_hashes_bytes: &[u8],
        hasher: &FsVerityHasher,
    ) -> Result<(), Status> {
        let expected_root: &D =
            D::ref_from_bytes(expected_root_bytes).map_err(|_| Status::INVALID_ARGS)?;
        let count = leaf_hashes_bytes.len() / std::mem::size_of::<D>();
        let leaf_hashes: &[D] = <[D]>::ref_from_bytes_with_elems(leaf_hashes_bytes, count)
            .map_err(|_| Status::INVALID_ARGS)?;

        if rebuild_root(leaf_hashes, hasher) != expected_root.as_bytes() {
            return Err(Status::IO_DATA_INTEGRITY);
        }
        Ok(())
    }

    /// Verifies a chunk of data against the Merkle tree.
    ///
    /// Returns `IO_DATA_INTEGRITY` if the data does not match the expected hash.
    /// Returns `INVALID_ARGS` if the offset or data length is not aligned to the block size.
    pub fn verify(&self, offset: usize, data: &[u8]) -> Result<(), Status> {
        match &self.hasher {
            FsVerityHasher::Sha256(_) => self.verify_impl::<Sha256Hash>(offset, data),
            FsVerityHasher::Sha512(_) => self.verify_impl::<Sha512Hash>(offset, data),
        }
    }

    fn verify_impl<D: FsVerityHash>(&self, offset: usize, data: &[u8]) -> Result<(), Status> {
        let block_size = self.hasher.block_size();
        if offset % block_size != 0 || data.len() % block_size != 0 {
            return Err(Status::INVALID_ARGS);
        }

        let count = self.leaf_hashes.len() / std::mem::size_of::<D>();
        let leaf_hashes: &[D] = <[D]>::ref_from_bytes_with_elems(&self.leaf_hashes[..], count)
            .map_err(|_| Status::INVALID_ARGS)?;

        let mut leaf_nodes_offset = offset;

        for chunk in data.chunks(block_size) {
            let index = leaf_nodes_offset / block_size;
            if index >= leaf_hashes.len() {
                return Err(Status::IO_DATA_INTEGRITY);
            }

            if self.hasher.hash_block(chunk) != leaf_hashes[index].as_bytes() {
                return Err(Status::IO_DATA_INTEGRITY);
            }

            leaf_nodes_offset += block_size;
        }

        Ok(())
    }
}

fn rebuild_root<D: FsVerityHash>(leaf_hashes: &[D], hasher: &FsVerityHasher) -> Vec<u8> {
    let mut builder = MerkleTreeBuilder::<D>::new(hasher.clone());

    for hash in leaf_hashes {
        builder.push_data_hash(*hash);
    }

    builder.finish().root().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MerkleTree;
    use crate::util::{FsVerityHasher, FsVerityHasherOptions};
    use std::mem::size_of;
    use test_case::test_case;

    const TEST_BLOCK_SIZE: usize = 4096;

    #[derive(Debug, Clone, Copy)]
    enum HashType {
        Sha256,
        Sha512,
    }

    fn get_hasher(hash_type: HashType) -> FsVerityHasher {
        match hash_type {
            HashType::Sha256 => {
                FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![], TEST_BLOCK_SIZE))
            }
            HashType::Sha512 => {
                FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![], TEST_BLOCK_SIZE))
            }
        }
    }

    fn create_data(size: usize) -> Vec<u8> {
        let mut data = vec![0xFFu8; size];
        for i in 0..data.len() {
            data[i] = (i % 255) as u8;
        }
        data
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_successfully_validate_root(hash_type: HashType) {
        let data = create_data(2 * TEST_BLOCK_SIZE + TEST_BLOCK_SIZE / 2);
        let hasher = get_hasher(hash_type);
        let tree = MerkleTree::from_data(&data, hasher.clone());
        MerkleVerifier::new(tree.root(), tree.leaf_hashes().to_vec().into_boxed_slice(), hasher)
            .expect("build failed");
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_fail_to_validate_root(hash_type: HashType) {
        let data = create_data(2 * TEST_BLOCK_SIZE + TEST_BLOCK_SIZE / 2);
        let hasher = get_hasher(hash_type);
        let tree = MerkleTree::from_data(&data, hasher.clone());

        let mut leaf_hashes = tree.leaf_hashes().to_vec();
        leaf_hashes[0] ^= 0xFF;

        let err = MerkleVerifier::new(tree.root(), leaf_hashes.into_boxed_slice(), hasher)
            .expect_err("build succeeded");
        assert_eq!(err, Status::IO_DATA_INTEGRITY);
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_verify_empty_data(hash_type: HashType) {
        let hasher = get_hasher(hash_type);
        let tree = MerkleTree::from_data(&[], hasher.clone());
        let verifier = MerkleVerifier::new(
            tree.root(),
            tree.leaf_hashes().to_vec().into_boxed_slice(),
            hasher,
        )
        .expect("build failed");

        verifier.verify(0, &[]).expect("verify failed");
        assert_eq!(verifier.verify(1, &[]).expect_err("verify succeeded"), Status::INVALID_ARGS);
        assert_eq!(
            verifier.verify(0, &[0x00]).expect_err("verify succeeded"),
            Status::INVALID_ARGS
        );
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_verify_with_invalid_args(hash_type: HashType) {
        let data = create_data(2 * TEST_BLOCK_SIZE + TEST_BLOCK_SIZE / 2);
        let hasher = get_hasher(hash_type);
        let tree = MerkleTree::from_data(&data, hasher.clone());
        let verifier = MerkleVerifier::new(
            tree.root(),
            tree.leaf_hashes().to_vec().into_boxed_slice(),
            hasher,
        )
        .expect("build failed");

        assert_eq!(
            verifier.verify(1, &data[1..TEST_BLOCK_SIZE + 1]).expect_err("verify succeeded"),
            Status::INVALID_ARGS
        );
        assert_eq!(
            verifier.verify(0, &vec![0xAB; 4 * TEST_BLOCK_SIZE]).expect_err("verify succeeded"),
            Status::IO_DATA_INTEGRITY
        );
        assert_eq!(
            verifier.verify(0, &data[0..TEST_BLOCK_SIZE - 1]).expect_err("verify succeeded"),
            Status::INVALID_ARGS
        );
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_verification(hash_type: HashType) {
        let data = create_data(2 * TEST_BLOCK_SIZE + TEST_BLOCK_SIZE / 2);
        let hasher = get_hasher(hash_type);
        let tree = MerkleTree::from_data(&data, hasher.clone());
        let verifier = MerkleVerifier::new(
            tree.root(),
            tree.leaf_hashes().to_vec().into_boxed_slice(),
            hasher,
        )
        .expect("build failed");

        verifier.verify(0, &data[0..TEST_BLOCK_SIZE]).expect("verify failed");
        verifier
            .verify(TEST_BLOCK_SIZE, &data[TEST_BLOCK_SIZE..2 * TEST_BLOCK_SIZE])
            .expect("verify failed");
        verifier.verify(0, &data[0..2 * TEST_BLOCK_SIZE]).expect("verify failed");

        let mut corrupt_data = data.clone();
        corrupt_data[0] ^= 0xFF;
        assert_eq!(
            verifier.verify(0, &corrupt_data[0..TEST_BLOCK_SIZE]).expect_err("verify succeeded"),
            Status::IO_DATA_INTEGRITY
        );
    }

    #[test_case(HashType::Sha256; "sha256")]
    #[test_case(HashType::Sha512; "sha512")]
    fn test_hash_type_size_validation(hash_type: HashType) {
        let size = match hash_type {
            HashType::Sha256 => size_of::<Sha256Hash>(),
            HashType::Sha512 => size_of::<Sha512Hash>(),
        };
        let bad_slice_less = vec![0xABu8; size - 1];
        let bad_slice_more = vec![0xABu8; size + 1];

        match hash_type {
            HashType::Sha256 => {
                assert!(Sha256Hash::ref_from_bytes(bad_slice_less.as_slice()).is_err());
                assert!(Sha256Hash::ref_from_bytes(bad_slice_more.as_slice()).is_err());
            }
            HashType::Sha512 => {
                assert!(Sha512Hash::ref_from_bytes(bad_slice_less.as_slice()).is_err());
                assert!(Sha512Hash::ref_from_bytes(bad_slice_more.as_slice()).is_err());
            }
        }

        let good_slice = vec![0xABu8; size];
        match hash_type {
            HashType::Sha256 => {
                let _: &Sha256Hash =
                    Sha256Hash::ref_from_bytes(good_slice.as_slice()).expect("read_hash failed");
            }
            HashType::Sha512 => {
                let _: &Sha512Hash =
                    Sha512Hash::ref_from_bytes(good_slice.as_slice()).expect("read_hash failed");
            }
        }
    }

    #[test]
    fn test_mismatched_hasher_and_root_size() {
        let data = create_data(512);
        let sha256_hasher = get_hasher(HashType::Sha256);
        let sha512_hasher = get_hasher(HashType::Sha512);

        // Build a SHA-256 tree
        let tree = MerkleTree::from_data(&data, sha256_hasher);

        // Try to construct a verifier using a SHA-512 hasher but with a SHA-256 root (32 bytes).
        // This should fail with INVALID_ARGS because the root size (32) doesn't match the
        // expected SHA-512 digest size (64).
        assert_eq!(
            MerkleVerifier::new(
                tree.root(), // 32 bytes
                tree.leaf_hashes().to_vec().into_boxed_slice(),
                sha512_hasher, // Expects 64 bytes
            )
            .expect_err("build succeeded with mismatched hasher"),
            Status::INVALID_ARGS
        );
    }

    #[test]
    fn test_mismatched_hasher_rebuild_fails() {
        let data = create_data(512);
        let sha256_hasher = get_hasher(HashType::Sha256);
        let sha512_hasher = get_hasher(HashType::Sha512);

        // Build a SHA-256 tree
        let tree = MerkleTree::from_data(&data, sha256_hasher);

        // Pass a 64-byte expected root (to pass the length check of SHA-512)
        let fake_sha512_root = vec![0u8; 64];

        // Pass SHA-256 leaf hashes, but pad them to a multiple of 64 bytes so they pass the
        // initial length check for SHA-512. This allows us to test the actual root validation
        // failure.
        let mut leaf_hashes = tree.leaf_hashes().to_vec();
        while leaf_hashes.len() % 64 != 0 {
            leaf_hashes.push(0);
        }

        // This should fail with IO_DATA_INTEGRITY because the rebuilt root (computed using
        // SHA-512 on the padded SHA-256 hashes) will not match the fake SHA-512 root we passed.
        assert_eq!(
            MerkleVerifier::new(&fake_sha512_root, leaf_hashes.into_boxed_slice(), sha512_hasher)
                .expect_err("build succeeded"),
            Status::IO_DATA_INTEGRITY
        );
    }
}
