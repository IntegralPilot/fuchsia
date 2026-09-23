// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::{hash_block, hash_block_aligned, make_hash_hasher, update_with_zeros};
use crate::{BLOCK_SIZE, HASH_SIZE, Hash, MerkleRootBuilder};
use storage_ptr_slice::PtrByteSlice;
use zx_status::Status;

/// Verifies data against the leaf hashes of a merkle tree.
#[derive(Clone)]
pub struct MerkleVerifier {
    hashes: Box<[Hash]>,
}

impl MerkleVerifier {
    /// Constructs a [`MerkleVerifier`] from the root and leaf hashes of a merkle tree.
    ///
    /// Returns `IO_DATA_INTEGRITY` if the leaf hashes are inconsistent with the root.
    pub fn new(root: Hash, hashes: Box<[Hash]>) -> Result<Self, Status> {
        if rebuild_root(&hashes) != root {
            Err(Status::IO_DATA_INTEGRITY)
        } else {
            Ok(Self { hashes })
        }
    }

    /// Verifies a `data` slice against the Merkle tree, assuming it corresponds to original data
    /// starting at `offset`.
    ///
    /// # Requirements:
    /// - The `offset` must be aligned to `BLOCK_SIZE`.
    /// - The length of `data` must be a multiple of `BLOCK_SIZE`, *except* if `data` contains the
    ///   final chunk of the original data source.
    pub fn verify(&self, offset: usize, data: &[u8]) -> Result<(), Status> {
        if !offset.is_multiple_of(BLOCK_SIZE) {
            return Err(Status::INVALID_ARGS);
        }
        let ending = data.len().checked_add(offset).ok_or(Status::INVALID_ARGS)?;
        if ending.div_ceil(BLOCK_SIZE) > self.hashes.len() {
            return Err(Status::INVALID_ARGS);
        }

        for (i, chunk) in data.chunks(BLOCK_SIZE).enumerate() {
            let hash = hash_block(chunk, offset + i * BLOCK_SIZE);
            if self.hashes[offset / BLOCK_SIZE + i] != hash {
                return Err(Status::IO_DATA_INTEGRITY);
            }
        }

        Ok(())
    }

    /// Verifies aligned, zero-padded data against the Merkle tree.
    ///
    /// The buffer length (`data.len()`) must be a multiple of 4096 bytes.
    /// `unaligned_len` specifies the actual valid data length within the buffer
    /// (`0 < unaligned_len <= data.len()`).
    /// Any buffer bytes beyond `unaligned_len` up to `data.len()` MUST be zeroed for verification
    /// to pass.
    ///
    /// Note: Does not support the null blob (empty 0-length blob, `unaligned_len == 0`).
    pub fn verify_aligned(
        &self,
        offset: usize,
        data: PtrByteSlice<'_>,
        unaligned_len: usize,
    ) -> Result<(), Status> {
        let len = data.len();
        let ending = len.checked_add(offset).ok_or(Status::INVALID_ARGS)?;
        if !offset.is_multiple_of(BLOCK_SIZE)
            || !len.is_multiple_of(4096)
            || unaligned_len == 0
            || unaligned_len > len
        {
            return Err(Status::INVALID_ARGS);
        }
        if ending.div_ceil(BLOCK_SIZE) > self.hashes.len() {
            return Err(Status::INVALID_ARGS);
        }

        for (i, block) in data.chunks(BLOCK_SIZE).enumerate() {
            let block_buffer_len = block.len();
            let block_unaligned_len =
                std::cmp::min(block_buffer_len, unaligned_len.saturating_sub(i * BLOCK_SIZE));
            let hash = hash_block_aligned(
                offset + i * BLOCK_SIZE,
                block,
                block_buffer_len,
                block_unaligned_len,
            );
            if self.hashes[offset / BLOCK_SIZE + i] != hash {
                return Err(Status::IO_DATA_INTEGRITY);
            }
        }

        Ok(())
    }
}

impl std::fmt::Debug for MerkleVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The leaf hashes are unlikely to be useful for debugging and could be megabytes of data.
        // The root of the merkle tree uniquely identifies the blob and the hash count can quickly
        // give a rough idea of the size of the blob.
        let root = rebuild_root(&self.hashes);
        f.debug_struct("MerkleVerifier")
            .field("root", &root)
            .field("hash_count", &self.hashes.len())
            .finish()
    }
}

fn rebuild_root(leaf_hashes: &[Hash]) -> Hash {
    let mut builder = MerkleRootBuilder::default();
    for hash in leaf_hashes {
        builder.push_hash(*hash);
    }
    builder.complete(&[])
}

fn hash_hashes(
    hashes_per_hash: usize,
    index: usize,
    hashes: impl ExactSizeIterator<Item = Hash>,
) -> Hash {
    let mut hasher = make_hash_hasher(hashes_per_hash, 1, index * hashes_per_hash * HASH_SIZE);
    let hash_count = hashes.len();
    for hash in hashes {
        hasher.update(hash.as_bytes());
    }
    if hash_count != hashes_per_hash {
        update_with_zeros(&mut hasher, (hashes_per_hash - hash_count) * HASH_SIZE);
    }
    Hash::from_array(hasher.digest())
}

/// Verifies reads against a merkle tree.
///
/// [`MerkleVerifier`] verifies data at a granularity of [`BLOCK_SIZE`]. Consequently, it stores one
/// hash (of size [`HASH_SIZE`]) for each data block. This hash storage consumes memory equal to
/// 1/256th of the original data size.
///
/// [`ReadSizedMerkleVerifier`] optimizes memory usage when reads are always aligned and are
/// guaranteed to be a multiple of the [`BLOCK_SIZE`]. Instead of storing a hash for every
/// [`BLOCK_SIZE`] blocks like [`MerkleVerifier`], it stores only one hash for each read sized
/// chunk. For example, 128KiB aligned reads would require storing 1/16th the number of hashes.
#[derive(Clone)]
pub struct ReadSizedMerkleVerifier {
    read_size: usize,
    hashes: Box<[Hash]>,
}

impl ReadSizedMerkleVerifier {
    /// Constructs a [`ReadSizedMerkleVerifier`] from an existing [`MerkleVerifier`] and a
    /// `read_size`.
    ///
    /// Returns an error if `read_size` is not a multiple of [`BLOCK_SIZE`].
    pub fn new(verifier: MerkleVerifier, read_size: usize) -> Result<Self, Status> {
        if read_size == 0 || !read_size.is_multiple_of(BLOCK_SIZE) {
            return Err(Status::INVALID_ARGS);
        }
        let hashes_per_hash = read_size / BLOCK_SIZE;
        let mut level_1_hashes =
            Vec::with_capacity(verifier.hashes.len().div_ceil(hashes_per_hash));
        for (i, hashes) in verifier.hashes.chunks(hashes_per_hash).enumerate() {
            level_1_hashes.push(hash_hashes(hashes_per_hash, i, hashes.iter().copied()));
        }
        Ok(ReadSizedMerkleVerifier { read_size, hashes: level_1_hashes.into_boxed_slice() })
    }

    /// Verifies a `data` slice against the Merkle tree, assuming it corresponds to original data
    /// starting at `offset`.
    ///
    /// # Requirements:
    /// - The `offset` must be aligned to the configured read size granularity.
    /// - The length of `data` must be a multiple of the read size, *except* if `data` represents
    ///   the final chunk of the original data source (in which case it can be shorter).
    pub fn verify(&self, offset: usize, data: &[u8]) -> Result<(), Status> {
        let end = offset.checked_add(data.len()).ok_or(Status::INVALID_ARGS)?;
        if !offset.is_multiple_of(self.read_size) {
            // The offset must read aligned.
            return Err(Status::INVALID_ARGS);
        }

        let hash_start_index = offset / self.read_size;
        let hash_end_index = end.div_ceil(self.read_size);

        if !end.is_multiple_of(self.read_size) && hash_end_index != self.hashes.len() {
            // The end is not aligned and it's not the end of the data.
            return Err(Status::INVALID_ARGS);
        }
        if hash_end_index > self.hashes.len() {
            return Err(Status::INVALID_ARGS);
        }

        let hashes_per_hash = self.read_size / BLOCK_SIZE;
        for (i, chunk) in data.chunks(self.read_size).enumerate() {
            let hash = hash_hashes(
                hashes_per_hash,
                hash_start_index + i,
                chunk.chunks(BLOCK_SIZE).enumerate().map(|(j, chunk)| {
                    hash_block(chunk, offset + self.read_size * i + j * BLOCK_SIZE)
                }),
            );
            if hash != self.hashes[hash_start_index + i] {
                return Err(Status::IO_DATA_INTEGRITY);
            }
        }

        Ok(())
    }

    /// Verifies aligned, zero-padded data against the Merkle tree.
    ///
    /// The buffer length (`data.len()`) must be a multiple of 4096 bytes.
    /// `unaligned_len` specifies the actual valid data length within the buffer
    /// (`0 < unaligned_len <= data.len()`).
    /// Any buffer bytes beyond `unaligned_len` up to `data.len()` MUST be zeroed for verification
    /// to pass.
    ///
    /// Note: Does not support the null blob (empty 0-length blob, `unaligned_len == 0`).
    pub fn verify_aligned(
        &self,
        offset: usize,
        data: PtrByteSlice<'_>,
        unaligned_len: usize,
    ) -> Result<(), Status> {
        let len = data.len();
        let end = offset.checked_add(len).ok_or(Status::INVALID_ARGS)?;
        if !offset.is_multiple_of(self.read_size)
            || !len.is_multiple_of(4096)
            || unaligned_len == 0
            || unaligned_len > len
        {
            return Err(Status::INVALID_ARGS);
        }

        let hash_start_index = offset / self.read_size;
        let hash_end_index = end.div_ceil(self.read_size);

        if !end.is_multiple_of(self.read_size) && hash_end_index != self.hashes.len() {
            // The end is not aligned and it's not the end of the data.
            return Err(Status::INVALID_ARGS);
        }
        if hash_end_index > self.hashes.len() {
            return Err(Status::INVALID_ARGS);
        }

        let hashes_per_hash = self.read_size / BLOCK_SIZE;
        for (i, chunk) in data.chunks(self.read_size).enumerate() {
            let block_base_offset = offset + i * self.read_size;
            let hash = hash_hashes(
                hashes_per_hash,
                hash_start_index + i,
                chunk.chunks(BLOCK_SIZE).enumerate().map(|(j, block)| {
                    let block_buffer_len = block.len();
                    let block_unaligned_len = std::cmp::min(
                        block_buffer_len,
                        unaligned_len.saturating_sub(i * self.read_size + j * BLOCK_SIZE),
                    );
                    hash_block_aligned(
                        block_base_offset + j * BLOCK_SIZE,
                        block,
                        block_buffer_len,
                        block_unaligned_len,
                    )
                }),
            );
            if hash != self.hashes[hash_start_index + i] {
                return Err(Status::IO_DATA_INTEGRITY);
            }
        }

        Ok(())
    }
}

impl std::fmt::Debug for ReadSizedMerkleVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The leaf hashes are unlikely to be useful for debugging and could be megabytes of data.
        // The root of the merkle tree can't be recovered either. The read size and hash count can
        // give a rough idea of the size of the blob.
        f.debug_struct("ReadAlignedMerkleVerifier")
            .field("read_size", &self.read_size)
            .field("hash_count", &self.hashes.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Error, anyhow};
    use assert_matches::assert_matches;
    use test_case::test_case;

    fn create_data(size: usize) -> Vec<u8> {
        const USIZE_SIZE: usize = size_of::<usize>();
        let mut data = vec![0xABu8; size];
        for (i, block) in data.chunks_mut(BLOCK_SIZE).enumerate() {
            // Place the index of the block at the start of the block to make every block unique.
            if block.len() < USIZE_SIZE {
                block.copy_from_slice(&i.to_le_bytes()[0..block.len()]);
            } else {
                block[0..USIZE_SIZE].copy_from_slice(&i.to_le_bytes());
            }
        }
        data
    }

    #[test_case(0; "0")]
    #[test_case(1; "1")]
    #[test_case(4096; "4096")]
    #[test_case(8191; "8191")]
    #[test_case(8192; "8192")]
    #[test_case(8193; "8193")]
    #[test_case(8192 * 2 - 1; "16383")]
    #[test_case(8192 * 2; "16384")]
    #[test_case(8192 * 2 + 1; "16385")]
    #[test_case(8192 * 256 - 1; "2097151")]
    #[test_case(8192 * 256; "2097152")]
    #[test_case(8192 * 256 + 1; "2097153")]
    #[test_case(8192 * 256 * 2 - 1; "4194303")]
    #[test_case(8192 * 256 * 2; "4194304")]
    #[test_case(8192 * 256 * 2 + 1; "4194305")]
    fn test_successfully_validate_root(size: usize) {
        let data = create_data(size);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);

        MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
    }

    #[test_case(8193; "8193")]
    #[test_case(8192 * 2 - 1; "16383")]
    #[test_case(8192 * 2; "16384")]
    #[test_case(8192 * 2 + 1; "16385")]
    #[test_case(8192 * 256 - 1; "2097151")]
    #[test_case(8192 * 256; "2097152")]
    #[test_case(8192 * 256 + 1; "2097153")]
    #[test_case(8192 * 256 * 2 - 1; "4194303")]
    #[test_case(8192 * 256 * 2; "4194304")]
    #[test_case(8192 * 256 * 2 + 1; "4194305")]
    fn test_fail_to_validate_root(size: usize) {
        let data = create_data(size);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);

        {
            let mut leaf_hashes = leaf_hashes.clone().into_boxed_slice();
            let mut first_hash: [u8; HASH_SIZE] = leaf_hashes[0].into();
            first_hash[0] ^= 0xFF;
            leaf_hashes[0] = Hash::from_array(first_hash);
            MerkleVerifier::new(root, leaf_hashes).expect_err("The merkle root shouldn't match");
        }

        {
            let mut leaf_hashes = leaf_hashes.into_boxed_slice();
            let mut last_hash: [u8; HASH_SIZE] = (*leaf_hashes.last().unwrap()).into();
            last_hash[31] ^= 0xFF;
            *leaf_hashes.last_mut().unwrap() = Hash::from_array(last_hash);
            MerkleVerifier::new(root, leaf_hashes).expect_err("The merkle root shouldn't match");
        }
    }

    #[test]
    fn test_verify_empty_data() {
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&[]);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        verifier.verify(0, &[]).unwrap();
        assert_matches!(verifier.verify(1, &[]), Err(Status::INVALID_ARGS));
        assert_matches!(verifier.verify(0, &[0x00]), Err(Status::IO_DATA_INTEGRITY));
        assert_matches!(verifier.verify(0, &[0x01]), Err(Status::IO_DATA_INTEGRITY));
    }

    #[test]
    fn test_verify_with_invalid_args() {
        let data = create_data(16 * 1024 + 20);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        // Offset isn't aligned.
        assert_matches!(verifier.verify(1, &data[1..]), Err(Status::INVALID_ARGS));
        // Too much data.
        assert_matches!(verifier.verify(8192, &data), Err(Status::INVALID_ARGS));
        // Still too much data but it's within the same block as the original data it's detected
        // with the hashes.
        assert_matches!(verifier.verify(8192, &data[8191..]), Err(Status::IO_DATA_INTEGRITY));
        assert_matches!(verifier.verify(8192, &data[8192..]), Ok(()));
    }

    #[test]
    fn test_invalid_read_sizes() {
        let data = create_data(16 * 1024 + 20);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        assert_matches!(
            ReadSizedMerkleVerifier::new(verifier.clone(), 0),
            Err(Status::INVALID_ARGS)
        );
        assert_matches!(
            ReadSizedMerkleVerifier::new(verifier.clone(), 1),
            Err(Status::INVALID_ARGS)
        );
        assert_matches!(
            ReadSizedMerkleVerifier::new(verifier.clone(), 8191),
            Err(Status::INVALID_ARGS)
        );
        assert_matches!(
            ReadSizedMerkleVerifier::new(verifier, 100 * 1024),
            Err(Status::INVALID_ARGS)
        );
    }

    #[test]
    fn test_verify_reads_with_multiple_reads() {
        const READ_SIZE: usize = 16 * 1024;
        let data = create_data(READ_SIZE * 3 + 20);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        let verifier = ReadSizedMerkleVerifier::new(verifier, READ_SIZE).unwrap();
        verifier.verify(0, &data).unwrap();
        verifier.verify(0, &data[0..READ_SIZE * 2]).unwrap();
        verifier.verify(READ_SIZE, &data[READ_SIZE..READ_SIZE * 3]).unwrap();
        verifier.verify(READ_SIZE, &data[READ_SIZE..READ_SIZE * 3 + 20]).unwrap();
        verifier.verify(READ_SIZE * 2, &data[READ_SIZE * 2..READ_SIZE * 3 + 20]).unwrap();
    }

    #[test]
    fn test_verify_reads_with_invalid_args() {
        const READ_SIZE: usize = 16 * 1024;
        let data = create_data(READ_SIZE * 3 + 20);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        let verifier = ReadSizedMerkleVerifier::new(verifier, READ_SIZE).unwrap();
        // Offset isn't aligned.
        assert_matches!(verifier.verify(1, &data[1..READ_SIZE + 1]), Err(Status::INVALID_ARGS));
        // Read past the end.
        assert_matches!(
            verifier.verify(READ_SIZE * 4, &data[0..READ_SIZE]),
            Err(Status::INVALID_ARGS)
        );
        // Not the end of the data and the data is not a multiple of the read size.
        assert_matches!(verifier.verify(0, &data[0..READ_SIZE - 10]), Err(Status::INVALID_ARGS));
        // At end of the data and it's the wrong amount of data but it's within the last block so
        // it's detected by the hashes.
        assert_matches!(
            verifier.verify(READ_SIZE * 3, &data[READ_SIZE * 3..READ_SIZE * 3 + 19]),
            Err(Status::IO_DATA_INTEGRITY)
        );
        let mut last_block_with_an_extra_byte = data[READ_SIZE * 3..READ_SIZE * 3 + 20].to_vec();
        last_block_with_an_extra_byte.push(0xAB);
        assert_matches!(
            verifier.verify(READ_SIZE * 3, &last_block_with_an_extra_byte),
            Err(Status::IO_DATA_INTEGRITY)
        );
        assert_matches!(
            verifier.verify(READ_SIZE * 3, &data[READ_SIZE * 3..READ_SIZE * 3 + 20]),
            Ok(())
        );
    }

    fn verify_with_first_bit_flipped(
        verifier: &MerkleVerifier,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Error> {
        data[0] ^= 0x01;
        match verifier.verify(offset, data) {
            Ok(()) => Err(anyhow!("verify_with_first_bit_flipped should have failed")),
            Err(Status::IO_DATA_INTEGRITY) => {
                data[0] ^= 0x01;
                Ok(())
            }
            Err(e) => Err(anyhow!("unexpected error in verify_with_first_bit_flipped: {e:?}")),
        }
    }

    fn verify_with_last_bit_flipped(
        verifier: &MerkleVerifier,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Error> {
        *data.last_mut().unwrap() ^= 0x80;
        match verifier.verify(offset, data) {
            Ok(()) => Err(anyhow!("verify_with_last_bit_flipped should have failed")),
            Err(Status::IO_DATA_INTEGRITY) => {
                *data.last_mut().unwrap() ^= 0x80;
                Ok(())
            }
            Err(e) => Err(anyhow!("unexpected error in verify_with_last_bit_flipped: {e:?}")),
        }
    }

    fn run_verify_tests(data: &mut [u8], verifier: MerkleVerifier) -> Result<(), Error> {
        // Verify all of the data at once.
        verifier.verify(0, data).context("verify all data")?;
        verify_with_first_bit_flipped(&verifier, 0, data).context("verify all data")?;
        verify_with_last_bit_flipped(&verifier, 0, data).context("verify all data")?;

        // Verify 1 block at a time.
        for (i, block) in data.chunks_mut(BLOCK_SIZE).enumerate() {
            let offset = i * BLOCK_SIZE;
            let context = || format!("verify 1 block at a time: offset={offset}");
            verifier.verify(offset, block).with_context(context)?;
            verify_with_first_bit_flipped(&verifier, offset, block).with_context(context)?;
            verify_with_last_bit_flipped(&verifier, offset, block).with_context(context)?;
        }

        // Verify 4 blocks at a time.
        const BLOCK_COUNT: usize = 4;
        for (i, block) in data.chunks_mut(BLOCK_SIZE * BLOCK_COUNT).enumerate() {
            let offset = i * BLOCK_SIZE * BLOCK_COUNT;
            let context = || format!("verify {BLOCK_COUNT} blocks at a time: offset={offset}");
            verifier.verify(offset, block).with_context(context)?;
            verify_with_first_bit_flipped(&verifier, offset, block).with_context(context)?;
            verify_with_last_bit_flipped(&verifier, offset, block).with_context(context)?;
        }
        Ok(())
    }

    fn verify_reads_with_first_bit_flipped(
        verifier: &ReadSizedMerkleVerifier,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Error> {
        data[0] ^= 0x01;
        match verifier.verify(offset, data) {
            Ok(()) => Err(anyhow!("verify_reads_with_first_bit_flipped should have failed")),
            Err(Status::IO_DATA_INTEGRITY) => {
                data[0] ^= 0x01;
                Ok(())
            }
            Err(e) => {
                Err(anyhow!("unexpected error in verify_reads_with_first_bit_flipped: {e:?}"))
            }
        }
    }

    fn verify_reads_with_last_bit_flipped(
        verifier: &ReadSizedMerkleVerifier,
        offset: usize,
        data: &mut [u8],
    ) -> Result<(), Error> {
        *data.last_mut().unwrap() ^= 0x80;
        match verifier.verify(offset, data) {
            Ok(()) => Err(anyhow!("verify_reads_with_last_bit_flipped should have failed")),
            Err(Status::IO_DATA_INTEGRITY) => {
                *data.last_mut().unwrap() ^= 0x80;
                Ok(())
            }
            Err(e) => Err(anyhow!("unexpected error in verify_reads_with_last_bit_flipped: {e:?}")),
        }
    }

    fn run_read_sized_verify_tests(
        data: &mut [u8],
        verifier: MerkleVerifier,
        read_size: usize,
    ) -> Result<(), Error> {
        let verifier = ReadSizedMerkleVerifier::new(verifier, read_size)
            .context("Failed to create read aligned verifier")?;

        for (i, chunk) in data.chunks_mut(read_size).enumerate() {
            let offset = i * read_size;
            let context = || format!("verify read: offset={offset}");
            verifier.verify(offset, chunk).with_context(context)?;
            verify_reads_with_first_bit_flipped(&verifier, offset, chunk).with_context(context)?;
            verify_reads_with_last_bit_flipped(&verifier, offset, chunk).with_context(context)?;
        }
        Ok(())
    }

    fn verify_test(data: &mut [u8]) -> Result<(), Error> {
        let hashes = Vec::with_capacity(data.len().div_ceil(BLOCK_SIZE));
        let (root, hashes) = MerkleRootBuilder::new(hashes).complete(data);
        let verifier = MerkleVerifier::new(root, hashes.into_boxed_slice())
            .with_context(|| format!("create verifier: data-size={}", data.len()))?;
        run_verify_tests(data, verifier.clone())
            .with_context(|| format!("verify data-size={}", data.len()))?;
        for read_size in [8 * 1024, 32 * 1024, 96 * 1024, 128 * 1024, 216 * 1024] {
            run_read_sized_verify_tests(data, verifier.clone(), read_size).with_context(|| {
                format!("verify read read-size={} data-size={}", read_size, data.len())
            })?;
        }
        Ok(())
    }

    #[test_case(1; "1")]
    #[test_case(4096; "4096")]
    #[test_case(8192 - 1; "8191")]
    #[test_case(8192; "8192")]
    #[test_case(8192 + 1; "8193")]
    #[test_case(8192 * 2 - 1; "16383")]
    #[test_case(8192 * 2; "16384")]
    #[test_case(8192 * 2 + 1; "16385")]
    #[test_case(8192 * 256 - 1; "2097151")]
    #[test_case(8192 * 256; "2097152")]
    #[test_case(8192 * 256 + 1; "2097153")]
    fn test_verification(size: usize) {
        verify_test(&mut create_data(size)).unwrap();
    }

    #[test]
    fn test_verify_aligned() {
        for size in [1, 100, 4096, 4097, 8192, 16384, 65536, 131072] {
            let data = create_data(size);
            let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
            let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
            let read_sized_verifier =
                ReadSizedMerkleVerifier::new(verifier.clone(), 128 * 1024).unwrap();

            let buffer_len = size.next_multiple_of(4096);
            let mut buf = vec![0u8; buffer_len];
            buf[..size].copy_from_slice(&data);

            // Test MerkleVerifier::verify_aligned
            verifier
                .verify_aligned(0, PtrByteSlice::from(&buf[..]), size)
                .expect("verify_aligned failed");

            // Test ReadSizedMerkleVerifier::verify_aligned
            read_sized_verifier
                .verify_aligned(0, PtrByteSlice::from(&buf[..]), size)
                .expect("read_sized_verifier.verify_aligned failed");

            // Test sub-chunk verification with non-zero offsets.
            for chunk_offset in (0..size).step_by(8192) {
                let chunk_unaligned_len = std::cmp::min(8192, size - chunk_offset);
                let chunk_buffer_len = chunk_unaligned_len.next_multiple_of(4096);
                let chunk_slice = &buf[chunk_offset..chunk_offset + chunk_buffer_len];
                verifier
                    .verify_aligned(
                        chunk_offset,
                        PtrByteSlice::from(chunk_slice),
                        chunk_unaligned_len,
                    )
                    .expect("verify_aligned failed for non-zero offset sub-chunk");
            }

            // Test corruption detection
            let mut corrupt_buf = buf.clone();
            corrupt_buf[0] ^= 0x01;
            assert_matches!(
                verifier.verify_aligned(0, PtrByteSlice::from(&corrupt_buf[..]), size),
                Err(Status::IO_DATA_INTEGRITY)
            );
            assert_matches!(
                read_sized_verifier.verify_aligned(0, PtrByteSlice::from(&corrupt_buf[..]), size),
                Err(Status::IO_DATA_INTEGRITY)
            );

            // Test corruption detection at non-zero offset
            if size > 8192 {
                let mut corrupt_nonzero_buf = buf.clone();
                corrupt_nonzero_buf[8192] ^= 0x01;
                let chunk_unaligned_len = std::cmp::min(8192, size - 8192);
                let chunk_buffer_len = chunk_unaligned_len.next_multiple_of(4096);
                assert_matches!(
                    verifier.verify_aligned(
                        8192,
                        PtrByteSlice::from(&corrupt_nonzero_buf[8192..8192 + chunk_buffer_len]),
                        chunk_unaligned_len
                    ),
                    Err(Status::IO_DATA_INTEGRITY)
                );
            }

            // Test unzeroed tail padding detection (if size < buffer_len)
            if size < buffer_len {
                let mut unzeroed_tail_buf = buf.clone();
                unzeroed_tail_buf[size] = 0xAA;
                assert_matches!(
                    verifier.verify_aligned(0, PtrByteSlice::from(&unzeroed_tail_buf[..]), size),
                    Err(Status::IO_DATA_INTEGRITY)
                );
                assert_matches!(
                    read_sized_verifier.verify_aligned(
                        0,
                        PtrByteSlice::from(&unzeroed_tail_buf[..]),
                        size
                    ),
                    Err(Status::IO_DATA_INTEGRITY)
                );
            }
        }

        // Test non-4096-aligned buffer length returns INVALID_ARGS
        let data = create_data(100);
        let (root, leaf_hashes) = MerkleRootBuilder::new(Vec::new()).complete(&data);
        let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();
        let unaligned_buf = [0u8; 100];
        assert_matches!(
            verifier.verify_aligned(0, PtrByteSlice::from(&unaligned_buf[..]), 100),
            Err(Status::INVALID_ARGS)
        );
    }

    #[test]
    #[ignore]
    fn test_very_large_verification() {
        const MAX_BUF: usize = 256 * 1024 * 1024 + 8192;
        let parallelism = std::thread::available_parallelism().unwrap().get();
        std::thread::scope(|scope| {
            for thread in 0..parallelism {
                scope.spawn(move || {
                    let mut data = create_data(MAX_BUF + 1);
                    for size in ((8192 * (thread + 1))..MAX_BUF).step_by(8192 * parallelism) {
                        verify_test(&mut data[0..size - 1]).unwrap();
                        verify_test(&mut data[0..size]).unwrap();
                        verify_test(&mut data[0..size + 1]).unwrap();
                    }
                });
            }
        });
    }
}
