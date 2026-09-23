// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use fsverity_merkle::MerkleVerifier;
use fsverity_merkle::{FsVerityHasher, FsVerityHasherOptions};
use zx_status::Status;

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum DmVerityError {
    #[error("Unsupported hash algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("Root hash mismatch")]
    RootHashMismatch,
    #[error("Invalid verifier arguments")]
    InvalidVerifierArgs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Sha256,
    Sha512,
}

impl std::str::FromStr for HashAlgorithm {
    type Err = DmVerityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sha256" => Ok(HashAlgorithm::Sha256),
            "sha512" => Ok(HashAlgorithm::Sha512),
            _ => Err(DmVerityError::UnsupportedAlgorithm(s.to_string())),
        }
    }
}

impl HashAlgorithm {
    pub fn as_str(&self) -> &'static str {
        match self {
            HashAlgorithm::Sha256 => "sha256",
            HashAlgorithm::Sha512 => "sha512",
        }
    }
}

/// Optional construction parameters for a dm-verity target.
///
/// These correspond to the optional target parameters in Linux dm-verity. They default to false if
/// all features are disabled.
///
/// See https://docs.kernel.org/admin-guide/device-mapper/verity.html
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DmVerityTargetOptionalParams {
    /// If set, blocks containing only zeroes will bypass verification and always return zeroes
    /// instead.
    // TODO(https://fxbug.dev/338125341): Add support for ignore_zero_blocks.
    pub ignore_zero_blocks: bool,
    /// If set, the system will restart if data corruption is detected.
    // TODO(https://fxbug.dev/338243823): Add support for restart on corruption.
    pub restart_on_corruption: bool,
}

/// Construction parameters for a dm-verity target.
///
/// Mirrors the mandatory parameters passed to the dm-verity target in Linux, along with optional
/// `DmVerityTargetOptionalParams`.
///
/// See https://docs.kernel.org/admin-guide/device-mapper/verity.html
#[derive(Debug, Clone)]
pub struct DmVerityTargetParams {
    /// The version of the dm-verity target.
    pub version: String,
    /// The path to the data block device to be verified.
    pub block_device_path: String,
    /// The path to the block device containing the Merkle tree hashes.
    pub hash_device_path: String,
    /// The size of a data block in bytes.
    pub data_block_size: u64,
    /// The size of a hash block in bytes.
    pub hash_block_size: u64,
    /// The total number of data blocks on the data device.
    pub num_data_blocks: u64,
    /// The root block index on the hash device where the Merkle tree begins.
    pub hash_start_block: u64,
    /// The cryptographic hash algorithm used (e.g., SHA256 or SHA512).
    pub hash_algorithm: HashAlgorithm,
    /// The hexadecimal encoded root digest of the Merkle tree. This hash should be trusted.
    pub root_digest: String,
    /// The hexadecimal encoded salt used when hashing blocks.
    pub salt: String,
    /// Optional target parameters.
    pub optional_params: DmVerityTargetOptionalParams,
}

/// Creates a new dm-verity Merkle tree verifier and verifies the leaf hashes against the trusted
/// root digest found in `params`.
pub fn create_verifier(
    params: &DmVerityTargetParams,
    leaf_hashes: Box<[u8]>,
) -> Result<MerkleVerifier, DmVerityError> {
    let salt_bytes = hex::decode(&params.salt).map_err(|_| DmVerityError::InvalidVerifierArgs)?;

    let options = FsVerityHasherOptions::new_dmverity(salt_bytes, params.hash_block_size as usize);

    let hasher = match params.hash_algorithm {
        HashAlgorithm::Sha256 => FsVerityHasher::Sha256(options),
        HashAlgorithm::Sha512 => FsVerityHasher::Sha512(options),
    };

    let expected_root_bytes =
        hex::decode(&params.root_digest).map_err(|_| DmVerityError::InvalidVerifierArgs)?;

    let verifier =
        MerkleVerifier::new(&expected_root_bytes, leaf_hashes, hasher).map_err(|status| {
            match status {
                Status::IO_DATA_INTEGRITY => DmVerityError::RootHashMismatch,
                _ => DmVerityError::InvalidVerifierArgs,
            }
        })?;

    Ok(verifier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fsverity_merkle::MerkleTree;
    use std::str::FromStr;
    use test_case::test_case;

    const TEST_BLOCK_SIZE: u64 = 128;

    fn create_verifier_test(
        params: &DmVerityTargetParams,
        leaf_hashes: &[u8],
    ) -> Result<MerkleVerifier, DmVerityError> {
        create_verifier(params, leaf_hashes.to_vec().into_boxed_slice())
    }

    fn create_data(size: usize) -> Vec<u8> {
        let mut data = vec![0xABu8; size];
        for (i, block) in data.chunks_mut(TEST_BLOCK_SIZE as usize).enumerate() {
            let index_bytes = (i as u64).to_le_bytes();
            let len = std::cmp::min(block.len(), index_bytes.len());
            block[0..len].copy_from_slice(&index_bytes[0..len]);
        }
        data
    }

    fn build_test_verifier(
        data: &[u8],
        hash_algorithm: HashAlgorithm,
        salt: &str,
    ) -> (MerkleVerifier, DmVerityTargetParams) {
        let salt_bytes = hex::decode(salt).expect("Failed to decode salt");
        let hasher = match hash_algorithm {
            HashAlgorithm::Sha256 => FsVerityHasher::Sha256(FsVerityHasherOptions::new_dmverity(
                salt_bytes.clone(),
                TEST_BLOCK_SIZE as usize,
            )),
            HashAlgorithm::Sha512 => FsVerityHasher::Sha512(FsVerityHasherOptions::new_dmverity(
                salt_bytes.clone(),
                TEST_BLOCK_SIZE as usize,
            )),
        };

        let tree = MerkleTree::from_data(data, hasher);
        let root_digest = hex::encode(tree.root());
        let leaf_hashes = tree.leaf_hashes();

        let params = DmVerityTargetParams {
            version: "1".to_string(),
            block_device_path: "mock".to_string(),
            hash_device_path: "mock".to_string(),
            data_block_size: TEST_BLOCK_SIZE,
            hash_block_size: TEST_BLOCK_SIZE,
            num_data_blocks: (data.len() as u64).div_ceil(TEST_BLOCK_SIZE),
            hash_start_block: 0,
            hash_algorithm,
            root_digest,
            salt: salt.to_string(),
            optional_params: DmVerityTargetOptionalParams::default(),
        };

        let verifier =
            create_verifier_test(&params, &leaf_hashes).expect("Failed to create verifier");
        (verifier, params)
    }

    #[test_case(HashAlgorithm::Sha256; "sha256")]
    #[test_case(HashAlgorithm::Sha512; "sha512")]
    fn test_verify_read_success(hash_algorithm: HashAlgorithm) {
        let data = create_data(512);
        let (verifier, _) = build_test_verifier(&data, hash_algorithm, "aabbcc");

        verifier.verify(0, &data).expect("Failed to verify read");
    }

    #[test_case(HashAlgorithm::Sha256; "sha256")]
    #[test_case(HashAlgorithm::Sha512; "sha512")]
    fn test_verify_read_corrupted_data(hash_algorithm: HashAlgorithm) {
        let mut data = create_data(512);
        let (verifier, _) = build_test_verifier(&data, hash_algorithm, "aabbcc");

        // Corrupt the first byte of data
        data[0] ^= 0xFF;

        verifier
            .verify(0, &data)
            .expect_err("Read verification should have failed for corrupted data");
    }

    #[test_case(HashAlgorithm::Sha256; "sha256")]
    #[test_case(HashAlgorithm::Sha512; "sha512")]
    fn test_verify_root_mismatch(hash_algorithm: HashAlgorithm) {
        let data = create_data(512);
        let salt = "aabbcc";
        let salt_bytes = hex::decode(salt).expect("Failed to decode salt");
        let hasher = match hash_algorithm {
            HashAlgorithm::Sha256 => FsVerityHasher::Sha256(FsVerityHasherOptions::new_dmverity(
                salt_bytes.clone(),
                TEST_BLOCK_SIZE as usize,
            )),
            HashAlgorithm::Sha512 => FsVerityHasher::Sha512(FsVerityHasherOptions::new_dmverity(
                salt_bytes.clone(),
                TEST_BLOCK_SIZE as usize,
            )),
        };

        let tree = MerkleTree::from_data(&data, hasher);
        let leaf_hashes = tree.leaf_hashes();

        let digest_size = match hash_algorithm {
            HashAlgorithm::Sha256 => 32,
            HashAlgorithm::Sha512 => 64,
        };
        let wrong_root_digest = "00".repeat(digest_size);

        let params = DmVerityTargetParams {
            version: "1".to_string(),
            block_device_path: "mock".to_string(),
            hash_device_path: "mock".to_string(),
            data_block_size: TEST_BLOCK_SIZE,
            hash_block_size: TEST_BLOCK_SIZE,
            num_data_blocks: (data.len() as u64).div_ceil(TEST_BLOCK_SIZE),
            hash_start_block: 0,
            hash_algorithm,
            root_digest: wrong_root_digest,
            salt: salt.to_string(),
            optional_params: DmVerityTargetOptionalParams::default(),
        };

        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with root mismatch"),
            DmVerityError::RootHashMismatch
        );
    }

    #[test_case(HashAlgorithm::Sha256; "sha256")]
    #[test_case(HashAlgorithm::Sha512; "sha512")]
    fn test_verify_read_non_zero_offset(hash_algorithm: HashAlgorithm) {
        let data = create_data(1024);
        let (verifier, _) = build_test_verifier(&data, hash_algorithm, "aabbcc");

        let block_index = 2;
        let offset = block_index * TEST_BLOCK_SIZE as usize;
        let block_data = &data[offset..(offset + TEST_BLOCK_SIZE as usize)];

        verifier.verify(offset, block_data).expect("Failed to verify read at non-zero offset");
    }

    #[test_case(HashAlgorithm::Sha256; "sha256")]
    #[test_case(HashAlgorithm::Sha512; "sha512")]
    fn test_verify_read_non_zero_offset_corrupted(hash_algorithm: HashAlgorithm) {
        let data = create_data(1024);
        let (verifier, _) = build_test_verifier(&data, hash_algorithm, "aabbcc");

        let block_index = 2;
        let offset = block_index * TEST_BLOCK_SIZE as usize;

        let mut block_data = data[offset..(offset + TEST_BLOCK_SIZE as usize)].to_vec();
        block_data[0] ^= 0xFF;

        verifier.verify(offset, &block_data).expect_err(
            "Read verification should have failed for corrupted data at non-zero offset",
        );
    }

    #[test]
    fn test_invalid_root_digest() {
        let data = create_data(512);
        let (_, mut params) = build_test_verifier(&data, HashAlgorithm::Sha256, "aabbcc");
        let leaf_hashes = vec![0u8; 32];

        // Non-hex characters
        params.root_digest = "invalid_hex".to_string();
        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with invalid root digest"),
            DmVerityError::InvalidVerifierArgs
        );

        // Odd length
        params.root_digest = "a".to_string();
        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with odd-length root digest"),
            DmVerityError::InvalidVerifierArgs
        );

        // Wrong size (expect 32 bytes for SHA-256)
        params.root_digest = "00".repeat(31);
        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with wrong-sized root digest"),
            DmVerityError::InvalidVerifierArgs
        );
    }

    #[test]
    fn test_invalid_salt() {
        let data = create_data(512);
        let (_, mut params) = build_test_verifier(&data, HashAlgorithm::Sha256, "aabbcc");
        let leaf_hashes = vec![0u8; 32];

        // Non-hex characters
        params.salt = "invalid_hex".to_string();
        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with invalid salt"),
            DmVerityError::InvalidVerifierArgs
        );

        // Odd length
        params.salt = "a".to_string();
        assert_eq!(
            create_verifier_test(&params, &leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with odd-length salt"),
            DmVerityError::InvalidVerifierArgs
        );
    }

    #[test]
    fn test_invalid_leaf_hashes_length() {
        let data = create_data(512);
        let (_, params) = build_test_verifier(&data, HashAlgorithm::Sha256, "aabbcc");

        // SHA-256 expects multiples of 32 bytes.
        let bad_leaf_hashes = vec![0u8; 31];
        assert_eq!(
            create_verifier_test(&params, &bad_leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with short leaf hashes"),
            DmVerityError::InvalidVerifierArgs
        );

        let bad_leaf_hashes = vec![0u8; 33];
        assert_eq!(
            create_verifier_test(&params, &bad_leaf_hashes)
                .expect_err("create_verifier passed unexpectedly with long leaf hashes"),
            DmVerityError::InvalidVerifierArgs
        );
    }

    #[test]
    fn test_unsupported_algorithm() {
        assert_eq!(
            HashAlgorithm::from_str("md5")
                .expect_err("HashAlgorithm::from_str passed unexpectedly for md5"),
            DmVerityError::UnsupportedAlgorithm("md5".to_string())
        );
        HashAlgorithm::from_str("sha256")
            .expect("HashAlgorithm::from_str failed unexpectedly for sha256");
        HashAlgorithm::from_str("sha512")
            .expect("HashAlgorithm::from_str failed unexpectedly for sha512");
    }
}
