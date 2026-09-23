// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{SHA256_SALT_PADDING, SHA512_SALT_PADDING};
use anyhow::{Error, anyhow, ensure};
use fidl_fuchsia_io as fio;
use mundane::hash::{Digest, Hasher, Sha256, Sha512};
use std::fmt;
use storage_ptr_slice::PtrByteSlice;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// `FsVerityHasherOptions` contains relevant metadata for the FsVerityHasher. The `salt` is set
/// according to the FsverityMetadata struct stored in fxfs and `block_size` is that of the
/// filesystem.
#[derive(Clone, PartialEq, Eq)]
pub struct FsVerityHasherOptions {
    salt: Vec<u8>,
    block_size: usize,
    fsverity: bool,
}

impl FsVerityHasherOptions {
    pub fn new(salt: Vec<u8>, block_size: usize) -> Self {
        FsVerityHasherOptions { salt, block_size, fsverity: true }
    }

    pub fn new_dmverity(salt: Vec<u8>, block_size: usize) -> Self {
        FsVerityHasherOptions { salt, block_size, fsverity: false }
    }
}

/// The raw structure of an FsVerity descriptor. The values in this are not necessarily valid.
#[derive(Debug, Copy, Clone, KnownLayout, FromBytes, Immutable, IntoBytes)]
#[repr(C, packed)]
pub struct FsVerityDescriptorRaw {
    version: u8,
    algorithm: u8,
    block_size_log2: u8,
    salt_size: u8,
    _reserved_1: [u8; 4],
    file_size: [u8; 8],
    root_digest: [u8; 64],
    salt: [u8; 32],
    _reserved_2: [u8; 144],
}

impl FsVerityDescriptorRaw {
    pub fn new(
        algorithm: fio::HashAlgorithm,
        block_size: u64,
        file_size: u64,
        root: &[u8],
        salt: &[u8],
    ) -> Result<Self, Error> {
        ensure!(block_size.is_power_of_two() && block_size >= 1024, "Invalid merkle block size");
        ensure!(salt.len() <= 32, "Salt too long");
        let (hash_len, algorithm) = match algorithm {
            fio::HashAlgorithm::Sha256 => (<Sha256 as Hasher>::Digest::DIGEST_LEN, 1),
            fio::HashAlgorithm::Sha512 => (<Sha512 as Hasher>::Digest::DIGEST_LEN, 2),
            _ => return Err(anyhow!("Unknown hash type")),
        };
        ensure!(root.len() == hash_len, "Wrong length of root digest");

        let mut this = Self {
            version: 1,
            algorithm,
            block_size_log2: block_size.trailing_zeros() as u8,
            salt_size: salt.len() as u8,
            _reserved_1: [0u8; 4],
            file_size: file_size.to_le_bytes(),
            root_digest: [0u8; 64],
            salt: [0u8; 32],
            _reserved_2: [0u8; 144],
        };
        this.root_digest.as_mut_slice()[0..hash_len].copy_from_slice(root);
        this.salt.as_mut_slice()[0..salt.len()].copy_from_slice(salt);
        Ok(this)
    }

    pub fn write_to_slice(&self, dest: &mut [u8]) -> Result<(), Error> {
        self.write_to_prefix(dest).map_err(|_| anyhow!("Buffer too short"))
    }
}

/// A descriptor struct for fsverity backed by a pointer slice. It does not own the bytes
/// backing it.
#[derive(Debug, Clone)]
pub struct FsVerityDescriptor<'a> {
    inner: FsVerityDescriptorRaw,
    bytes: PtrByteSlice<'a>,
    descriptor_offset: usize,
}

impl<'a> FsVerityDescriptor<'a> {
    /// Create a descriptor from data that can be converted into a pointer slice.
    pub fn new(bytes: impl Into<PtrByteSlice<'a>>, block_size: usize) -> Result<Self, Error> {
        let bytes = bytes.into();
        ensure!(block_size.is_power_of_two() && block_size > 0, "Invalid block size.");
        // Descriptor is placed in the last block. Go to the start of the last block.
        let descriptor_offset = if bytes.len() == 0 {
            // This will fail properly below.
            0
        } else {
            ((bytes.len() - 1) / block_size) * block_size
        };
        ensure!(
            bytes.len() >= descriptor_offset + std::mem::size_of::<FsVerityDescriptorRaw>(),
            "Descriptor bytes too small"
        );
        let inner = bytes
            .subslice(
                descriptor_offset..descriptor_offset + std::mem::size_of::<FsVerityDescriptorRaw>(),
            )
            .read::<FsVerityDescriptorRaw>()
            .unwrap();

        ensure!(inner.version == 1, "Unsupported version {}", inner.version);

        ensure!(
            inner.algorithm == 1 || inner.algorithm == 2,
            "Unsupported algorithm {}",
            inner.algorithm
        );

        // Merkle block size here doesn't necessarily need to match fs block size, but it is the
        // most efficient choice, greatly simplifies handling, and is the only supported choice in
        // the destination fxfs. It it stored in the descriptor as the log_2 of the value. It must
        // be at least 1024 and also no more than system page size. We won't verify page size here
        // but also won't support more than 64KiB.
        ensure!(
            inner.block_size_log2 >= 10 && inner.block_size_log2 <= 16,
            "Only supports 1KiB-64KiB"
        );

        ensure!(inner.salt_size <= 32, "Salt too big for struct");
        let this = Self { inner, bytes, descriptor_offset };
        ensure!(this.block_size() == block_size, "Only support same block size as file system");
        Ok(this)
    }

    pub fn digest_len(&self) -> usize {
        match self.inner.algorithm {
            1 => <Sha256 as Hasher>::Digest::DIGEST_LEN,
            2 => <Sha512 as Hasher>::Digest::DIGEST_LEN,
            _ => unreachable!("This should be verified at creation time."),
        }
    }

    pub fn digest_algorithm(&self) -> fio::HashAlgorithm {
        match self.inner.algorithm {
            1 => fio::HashAlgorithm::Sha256,
            2 => fio::HashAlgorithm::Sha512,
            _ => unreachable!("This should be verified at creation time."),
        }
    }

    pub fn block_size(&self) -> usize {
        1usize << self.inner.block_size_log2
    }

    pub fn file_size(&self) -> usize {
        u64::from_le_bytes(self.inner.file_size) as usize
    }

    pub fn root_digest(&self) -> &[u8] {
        &self.inner.root_digest[..self.digest_len()]
    }

    pub fn salt(&self) -> &[u8] {
        &self.inner.salt[..self.inner.salt_size as usize]
    }

    /// Return a hasher configured based on this descriptor.
    pub fn hasher(&self) -> FsVerityHasher {
        match self.inner.algorithm {
            1 => FsVerityHasher::Sha256(FsVerityHasherOptions::new(
                self.salt().to_vec(),
                self.block_size(),
            )),
            2 => FsVerityHasher::Sha512(FsVerityHasherOptions::new(
                self.salt().to_vec(),
                self.block_size(),
            )),
            _ => unreachable!("This should be verified at creation time."),
        }
    }

    /// A vector containing a copy of all the leaf digests required for the file.
    pub fn leaf_digests(&self) -> Result<Vec<u8>, Error> {
        let block_size = self.block_size();
        Ok(match self.file_size().div_ceil(block_size) {
            0 => vec![],
            1 => self.root_digest().to_vec(),
            file_blocks => {
                let leaf_size = file_blocks * self.digest_len();
                let layer_size = leaf_size.next_multiple_of(block_size);
                ensure!(self.descriptor_offset >= layer_size, "No space for leaves in descriptor");
                let leaf_offset = self.descriptor_offset - layer_size;
                self.bytes.subslice(leaf_offset..(leaf_offset + leaf_size)).to_vec()
            }
        })
    }
}

/// `FsVerityHasher` is used by fsverity to construct merkle trees for verity-enabled files.
/// `FsVerityHasher` is parameterized by a salt and a block size.
#[derive(Clone, PartialEq, Eq)]
pub enum FsVerityHasher {
    Sha256(FsVerityHasherOptions),
    Sha512(FsVerityHasherOptions),
}

impl fmt::Debug for FsVerityHasher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsVerityHasher::Sha256(metadata) => f
                .debug_struct("FsVerityHasher::Sha256")
                .field("salt", &metadata.salt)
                .field("block_size", &metadata.block_size)
                .finish(),
            FsVerityHasher::Sha512(metadata) => f
                .debug_struct("FsVerityHasher::Sha512")
                .field("salt", &metadata.salt)
                .field("block_size", &metadata.block_size)
                .finish(),
        }
    }
}

impl FsVerityHasher {
    pub fn block_size(&self) -> usize {
        match self {
            FsVerityHasher::Sha256(metadata) => metadata.block_size,
            FsVerityHasher::Sha512(metadata) => metadata.block_size,
        }
    }

    pub fn hash_size(&self) -> usize {
        match self {
            FsVerityHasher::Sha256(_) => <Sha256 as Hasher>::Digest::DIGEST_LEN,
            FsVerityHasher::Sha512(_) => <Sha512 as Hasher>::Digest::DIGEST_LEN,
        }
    }

    pub fn fsverity(&self) -> bool {
        match &self {
            FsVerityHasher::Sha256(metadata) => metadata.fsverity,
            FsVerityHasher::Sha512(metadata) => metadata.fsverity,
        }
    }

    /// Computes the MerkleTree digest from a `block` of data.
    ///
    /// A MerkleTree digest is a hash of a block of data. The block will be zero filled if its
    /// len is less than the block_size, except for when the first data block is completely empty.
    /// If `salt.len() > 0`, we prepend the block with the salt which itself is zero filled up
    /// to the padding.
    ///
    /// # Panics
    ///
    /// Panics if `block.len()` exceeds `self.block_size()`.
    pub fn hash_block(&self, block: &[u8]) -> Vec<u8> {
        match self {
            FsVerityHasher::Sha256(metadata) => {
                if block.is_empty() {
                    // Empty files have a root hash of all zeroes.
                    return vec![0; <Sha256 as Hasher>::Digest::DIGEST_LEN];
                }
                assert!(block.len() <= metadata.block_size);
                let mut hasher = Sha256::default();
                let salt_size = metadata.salt.len() as u8;

                if salt_size > 0 {
                    hasher.update(&metadata.salt);
                    if metadata.fsverity && salt_size % SHA256_SALT_PADDING != 0 {
                        hasher.update(&vec![
                            0;
                            (SHA256_SALT_PADDING - salt_size % SHA256_SALT_PADDING)
                                as usize
                        ])
                    }
                }

                hasher.update(block);
                // Zero fill block up to self.block_size(). As a special case, if the first data
                // block is completely empty, it is not zero filled.
                if block.len() != metadata.block_size {
                    hasher.update(&vec![0; metadata.block_size - block.len()]);
                }
                hasher.finish().bytes().to_vec()
            }
            FsVerityHasher::Sha512(metadata) => {
                if block.is_empty() {
                    // Empty files have a root hash of all zeroes.
                    return vec![0; <Sha512 as Hasher>::Digest::DIGEST_LEN];
                }
                assert!(block.len() <= metadata.block_size);
                let mut hasher = Sha512::default();
                let salt_size = metadata.salt.len() as u8;

                if salt_size > 0 {
                    hasher.update(&metadata.salt);
                    if metadata.fsverity && salt_size % SHA512_SALT_PADDING != 0 {
                        hasher.update(&vec![
                            0;
                            (SHA512_SALT_PADDING - salt_size % SHA512_SALT_PADDING)
                                as usize
                        ])
                    }
                }

                hasher.update(block);
                // Zero fill block up to self.block_size(). As a special case, if the first data
                // block is completely empty, it is not zero filled.
                if block.len() != metadata.block_size {
                    hasher.update(&vec![0; metadata.block_size - block.len()]);
                }
                hasher.finish().bytes().to_vec()
            }
        }
    }

    /// Computes a MerkleTree digest from a block of `hashes`.
    ///
    /// Like `hash_block`, `hash_hashes` zero fills incomplete buffers and prepends the digests
    /// with a salt, which is zero filled up to the padding.
    ///
    /// # Panics
    ///
    /// Panics if any of the following conditions are met:
    /// - `hashes.len()` is 0
    /// - `hashes.len() > self.block_size() / digest length`
    pub fn hash_hashes(&self, hashes: &[Vec<u8>]) -> Vec<u8> {
        assert_ne!(hashes.len(), 0);
        match self {
            FsVerityHasher::Sha256(metadata) => {
                assert!(
                    hashes.len() <= (metadata.block_size / <Sha256 as Hasher>::Digest::DIGEST_LEN)
                );
                let mut hasher = Sha256::default();
                let salt_size = metadata.salt.len() as u8;
                if salt_size > 0 {
                    hasher.update(&metadata.salt);
                    if metadata.fsverity && salt_size % SHA256_SALT_PADDING != 0 {
                        hasher.update(&vec![
                            0;
                            (SHA256_SALT_PADDING - salt_size % SHA256_SALT_PADDING)
                                as usize
                        ])
                    }
                }

                for hash in hashes {
                    hasher.update(hash.as_slice());
                }
                for _ in 0..((metadata.block_size / <Sha256 as Hasher>::Digest::DIGEST_LEN)
                    - hashes.len())
                {
                    hasher.update(&[0; <Sha256 as Hasher>::Digest::DIGEST_LEN]);
                }

                hasher.finish().bytes().to_vec()
            }
            FsVerityHasher::Sha512(metadata) => {
                assert!(
                    hashes.len() <= (metadata.block_size / <Sha512 as Hasher>::Digest::DIGEST_LEN)
                );

                let mut hasher = Sha512::default();
                let salt_size = metadata.salt.len() as u8;
                if salt_size > 0 {
                    hasher.update(&metadata.salt);
                    if metadata.fsverity && salt_size % SHA512_SALT_PADDING != 0 {
                        hasher.update(&vec![
                            0;
                            (SHA512_SALT_PADDING - salt_size % SHA512_SALT_PADDING)
                                as usize
                        ])
                    }
                }

                for hash in hashes {
                    hasher.update(hash.as_slice());
                }
                for _ in 0..((metadata.block_size / <Sha512 as Hasher>::Digest::DIGEST_LEN)
                    - hashes.len())
                {
                    hasher.update(&[0; <Sha512 as Hasher>::Digest::DIGEST_LEN]);
                }

                hasher.finish().bytes().to_vec()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FsVerityHash, MerkleTreeBuilder, Sha256Hash, Sha512Hash};
    use fidl_fuchsia_io as fio;
    use hex::FromHex;
    use test_case::test_case;

    const BLOCK_SIZE: usize = 4096;

    #[test]
    fn test_hash_block_empty_sha256() {
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = [];
        let hash = hasher.hash_block(&block[..]);
        assert_eq!(hash, [0; 32]);
    }

    #[test]
    fn test_hash_block_empty_sha512() {
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = [];
        let hash = hasher.hash_block(&block[..]);
        assert_eq!(hash, [0; 64]);
    }

    #[test]
    fn test_hash_block_partial_block_sha256() {
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = vec![0xFF; hasher.block_size()];
        let mut block2: Vec<u8> = vec![0xFF; hasher.block_size() / 2];
        block2.append(&mut vec![0; hasher.block_size() / 2]);
        let hash = hasher.hash_block(&block[..]);
        let expected = hasher.hash_block(&block[..]);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_block_partial_block_sha512() {
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = vec![0xFF; hasher.block_size()];
        let mut block2: Vec<u8> = vec![0xFF; hasher.block_size() / 2];
        block2.append(&mut vec![0; hasher.block_size() / 2]);
        let hash = hasher.hash_block(&block[..]);
        let expected = hasher.hash_block(&block[..]);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_block_single_sha256() {
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = vec![0xFF; hasher.block_size()];
        let hash = hasher.hash_block(&block[..]);
        // Root hash of file size 4096 = block_size
        let expected: [u8; 32] =
            FromHex::from_hex("207f18729b037894447f948b81f63abe68007d0cd7c99a4ae0a3e323c52013a5")
                .unwrap();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_block_single_sha512() {
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let block = vec![0xFF; hasher.block_size()];
        let hash = hasher.hash_block(&block[..]);
        // Root hash of file size 4096 = block_size
        let expected: [u8; 64] = FromHex::from_hex("96d217a5f593384eb266b4bb2574b93c145ff1fd5ca89af52af6d4a14d2ce5200b2ddad30771c7cbcd139688e1a3847da7fd681490690adc945c3776154c42f6").unwrap();
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_hash_hashes_full_block_sha256() {
        let hasher = FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let mut leafs = Vec::new();
        {
            let block = vec![0xFF; hasher.block_size()];
            for _i in 0..hasher.block_size() / hasher.hash_size() {
                leafs.push(hasher.hash_block(&block));
            }
        }
        let root = hasher.hash_hashes(&leafs);
        // Root hash of file size 524288 = block_size * (block_size / hash_size) = 4096 * (4096 / 32)
        let expected: [u8; 32] =
            FromHex::from_hex("827c28168aba953cf74706d4f3e776bd8892f6edf7b25d89645409f24108fb0b")
                .unwrap();
        assert_eq!(root, expected);
    }

    #[test]
    fn test_hash_hashes_full_block_sha512() {
        let hasher = FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096));
        let mut leafs = Vec::new();
        {
            let block = vec![0xFF; hasher.block_size()];
            for _i in 0..hasher.block_size() / hasher.hash_size() {
                leafs.push(hasher.hash_block(&block));
            }
        }
        let root = hasher.hash_hashes(&leafs);
        // Root hash of file size 262144 = block_size * (block_size / hash_size) = 4096 * (4096 / 64)
        let expected: [u8; 64] = FromHex::from_hex("17d1728518330e0d48951ba43908ea7ad73ea018597643aabba9af2e43dea70468ba54fa09f9c7d02b1c240bd8009d1abd49c05559815a3b73ce31c5c26f93ba").unwrap();
        assert_eq!(root, expected);
    }

    #[test_case(FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)); "sha256")]
    #[test_case(FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xFF; 8], 4096)); "sha512")]
    fn test_hash_hashes_zero_pad_same_length(hasher: FsVerityHasher) {
        let data_hash = hasher.hash_block(&vec![0xFF; hasher.block_size()]);
        let mut zero_hash = Vec::with_capacity(hasher.hash_size());
        zero_hash.extend(std::iter::repeat(0).take(hasher.hash_size()));
        let hash_of_single_hash = hasher.hash_hashes(&[data_hash.clone()]);
        let hash_of_single_hash_and_zero_hash = hasher.hash_hashes(&[data_hash, zero_hash]);
        assert_eq!(hash_of_single_hash, hash_of_single_hash_and_zero_hash);
    }

    #[test_case(vec![0u8; BLOCK_SIZE + 256], BLOCK_SIZE ; "test_exact_size")]
    #[test_case(vec![0u8; 256], 0 ; "test_exact_size_from_zero")]
    #[test_case(vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE ; "test_block_aligned")]
    #[test_case(vec![0u8; BLOCK_SIZE], 0 ; "test_block_aligned_from_zero")]
    #[test_case(vec![0u8; BLOCK_SIZE + 300], BLOCK_SIZE ; "test_trailing_space")]
    #[test_case(vec![0u8; 300], 0 ; "test_trailing_space_from_zero")]
    fn descriptor_read_write_locations(mut buf: Vec<u8>, descriptor_offset: usize) {
        let salt = [4u8; 6];
        let root = [65u8; 32];
        let descriptor = FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha256,
            BLOCK_SIZE as u64,
            8192,
            root.as_slice(),
            salt.as_slice(),
        )
        .expect("Create raw descriptor");

        descriptor
            .write_to_slice(&mut buf.as_mut_slice()[descriptor_offset..])
            .expect("Writing to buf.");

        let descriptor2 =
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect("Parsing descriptor back");
        // Verify the raw values.
        assert_eq!(descriptor2.inner.version, descriptor.version);
        assert_eq!(descriptor2.inner.algorithm, descriptor.algorithm);
        assert_eq!(descriptor2.inner.block_size_log2, descriptor.block_size_log2);
        assert_eq!(descriptor2.inner.salt_size, descriptor.salt_size);
        assert_eq!(descriptor2.inner.file_size, descriptor.file_size);
        assert_eq!(descriptor2.inner.root_digest, descriptor.root_digest);
        assert_eq!(descriptor2.inner.salt, descriptor.salt);

        // Verify the processed values.
        assert_eq!(descriptor2.file_size(), 8192);
        assert_eq!(descriptor2.digest_len(), 32);
        assert_eq!(descriptor2.digest_algorithm(), fio::HashAlgorithm::Sha256);
        assert_eq!(descriptor2.root_digest(), root.as_slice());
        assert_eq!(descriptor2.salt(), salt.as_slice());
    }

    #[test_case(2, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256")]
    #[test_case(2, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512")]
    // Enough blocks to have a second layer of merkle tree.
    #[test_case(129, vec![0u8; BLOCK_SIZE * 3], BLOCK_SIZE * 2, 0, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256_big_file")]
    #[test_case(129, vec![0u8; BLOCK_SIZE * 4], BLOCK_SIZE * 3, 0, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512_big_file")]
    // Don't block align the end, just enough space for the descriptor.
    #[test_case(2, vec![0u8; BLOCK_SIZE + 256], BLOCK_SIZE, 0, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256_exact_fit")]
    #[test_case(2, vec![0u8; BLOCK_SIZE + 256], BLOCK_SIZE, 0, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512_exact_fit")]
    // A really big merkle buffer, everything should still be at the end of it.
    #[test_case(2, vec![0u8; BLOCK_SIZE * 100], BLOCK_SIZE * 99, BLOCK_SIZE * 98, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256_big_buf")]
    #[test_case(2, vec![0u8; BLOCK_SIZE * 100], BLOCK_SIZE * 99, BLOCK_SIZE * 98, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512_big_buf")]
    // File has only a single block. This is a special case for generating the leaf data.
    #[test_case(1, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256_one_block")]
    #[test_case(1, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512_one_block")]
    // File has no data blocks. This is a special case for generating the leaf data.
    #[test_case(0, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha256(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha256_empty_file")]
    #[test_case(0, vec![0u8; BLOCK_SIZE * 2], BLOCK_SIZE, 0, FsVerityHasher::Sha512(FsVerityHasherOptions::new(vec![0xAB; 8], BLOCK_SIZE)); "sha512_empty_file")]
    fn descriptor_merkle_leaves_locations(
        file_blocks: usize,
        buf: Vec<u8>,
        descriptor_offset: usize,
        leaf_offset: usize,
        hasher: FsVerityHasher,
    ) {
        match hasher {
            FsVerityHasher::Sha256(_) => descriptor_merkle_leaves_locations_impl::<Sha256Hash>(
                file_blocks,
                buf,
                descriptor_offset,
                leaf_offset,
                hasher,
            ),
            FsVerityHasher::Sha512(_) => descriptor_merkle_leaves_locations_impl::<Sha512Hash>(
                file_blocks,
                buf,
                descriptor_offset,
                leaf_offset,
                hasher,
            ),
        }
    }

    fn descriptor_merkle_leaves_locations_impl<D: FsVerityHash>(
        file_blocks: usize,
        mut buf: Vec<u8>,
        descriptor_offset: usize,
        leaf_offset: usize,
        hasher: FsVerityHasher,
    ) {
        let mut file = vec![0u8; BLOCK_SIZE * file_blocks];
        for i in 0..file_blocks {
            let offset = i * BLOCK_SIZE;
            file.as_mut_slice()[offset..(offset + BLOCK_SIZE)].fill(i as u8);
        }

        let (algorithm, salt) = match &hasher {
            FsVerityHasher::Sha256(options) => (fio::HashAlgorithm::Sha256, options.salt.clone()),
            FsVerityHasher::Sha512(options) => (fio::HashAlgorithm::Sha512, options.salt.clone()),
        };

        let hash_size = hasher.hash_size();
        let mut builder = MerkleTreeBuilder::<D>::new(hasher);
        builder.write(file.as_slice());
        let tree = builder.finish();

        let descriptor = FsVerityDescriptorRaw::new(
            algorithm,
            BLOCK_SIZE as u64,
            file.len() as u64,
            tree.root(),
            salt.as_slice(),
        )
        .expect("Creating raw descriptor");

        descriptor
            .write_to_slice(&mut buf.as_mut_slice()[descriptor_offset..])
            .expect("Writing descriptor");
        // FsVerity doesn't actually write out the leaves if there is one or fewer blocks.
        if file_blocks > 1 {
            let leaf_bytes = tree.leaf_hashes();
            buf.as_mut_slice()[leaf_offset..(leaf_offset + (file_blocks * hash_size))]
                .copy_from_slice(leaf_bytes);
        }

        let descriptor2 =
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect("Parsing decsriptor");
        assert_eq!(descriptor2.root_digest(), tree.root());

        let mut verifier_builder = MerkleTreeBuilder::<D>::new(descriptor2.hasher());
        let leaves = descriptor2.leaf_digests().expect("Finding leaf digests");
        for leaf in leaves.chunks_exact(hash_size) {
            let hash = D::read_from_bytes(leaf).unwrap();
            verifier_builder.push_data_hash(hash);
        }

        let verifier_tree = verifier_builder.finish();
        assert_eq!(verifier_tree.root(), tree.root());
    }

    #[test]
    fn test_raw_descriptor_failure_cases() {
        // The base case is valid.
        let descriptor = FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha256,
            BLOCK_SIZE as u64,
            12,
            &[0u8; 32],
            &[0u8; 32],
        )
        .expect("Creating valid descriptor");
        {
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
        }

        // Try with buf too small.
        {
            let mut buf = vec![0u8; 200];
            descriptor.write_to_slice(buf.as_mut_slice()).expect_err("Buffer too small");
        }

        // Block is too small or not power of two.
        FsVerityDescriptorRaw::new(fio::HashAlgorithm::Sha256, 256, 12, &[0u8; 32], &[0u8; 32])
            .expect_err("Bad block size");
        FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha256,
            4097 as u64,
            12,
            &[0u8; 32],
            &[0u8; 32],
        )
        .expect_err("Bad block size");

        // Salt is too long.
        FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha256,
            BLOCK_SIZE as u64,
            12,
            &[0u8; 32],
            &[0u8; 33],
        )
        .expect_err("Bad salt");

        // Hash length wrong at 33
        FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha256,
            BLOCK_SIZE as u64,
            12,
            &[0u8; 33],
            &[0u8; 32],
        )
        .expect_err("Bad hash length");
        FsVerityDescriptorRaw::new(
            fio::HashAlgorithm::Sha512,
            BLOCK_SIZE as u64,
            12,
            &[0u8; 33],
            &[0u8; 32],
        )
        .expect_err("Bad hash length");
    }

    #[test]
    fn test_descriptor_buf_too_small_for_leaves() {
        let raw_descriptor = FsVerityDescriptorRaw {
            version: 1,
            algorithm: 1,
            block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
            salt_size: 8,
            _reserved_1: [0u8; 4],
            file_size: 3000000u64.to_le_bytes(),
            root_digest: [0u8; 64],
            salt: [0u8; 32],
            _reserved_2: [0u8; 144],
        };
        let mut buf = vec![0u8; BLOCK_SIZE * 2];
        raw_descriptor.write_to_slice(&mut buf[BLOCK_SIZE..]).expect("Writing out descriptor");
        let descriptor = FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect("Parsing fine");
        descriptor.leaf_digests().expect_err("Not enough space for leaves");
    }

    #[test]
    fn test_descriptor_from_bytes_validation() {
        // Base case, success.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect("Parsing fine");
        }

        // Buffer too small to parse.
        {
            let buf = vec![0u8; 200];
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Buff too small");
        }

        // Bad block sizes provided to method
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), 4097).expect_err("Bad provided block size");
        }
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), 0).expect_err("Bad provided block size");
        }

        // Bad version
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 2,
                algorithm: 1,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Bad version");
        }

        // Bad algorithm type.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 3,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Bad algorithm");
        }

        // Bad block size. Too small.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: 9,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Bad block size");
        }

        // Bad block size. Too big.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: 128,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Bad block size");
        }

        // Salt size too big.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: BLOCK_SIZE.trailing_zeros() as u8,
                salt_size: 40,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Bad salt size");
        }

        // Block size doesn't match.
        {
            let descriptor = FsVerityDescriptorRaw {
                version: 1,
                algorithm: 1,
                block_size_log2: 2048usize.trailing_zeros() as u8,
                salt_size: 8,
                _reserved_1: [0u8; 4],
                file_size: 25u64.to_le_bytes(),
                root_digest: [0u8; 64],
                salt: [0u8; 32],
                _reserved_2: [0u8; 144],
            };
            let mut buf = vec![0u8; 256];
            descriptor.write_to_slice(buf.as_mut_slice()).expect("Writing out descriptor");
            FsVerityDescriptor::new(buf.as_slice(), BLOCK_SIZE).expect_err("Block size mismatch");
        }
    }
}
