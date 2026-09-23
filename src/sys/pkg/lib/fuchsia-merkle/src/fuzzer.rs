// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use arbitrary::{Arbitrary, Result, Unstructured};
use fuchsia_merkle::{
    BLOCK_SIZE, HASH_SIZE, Hash, MerkleRootBuilder, MerkleVerifier, ReadSizedMerkleVerifier,
};
use fuzz::fuzz;
use storage_ptr_slice::PtrByteSlice;

#[derive(Debug)]
struct RandomMerkleVerifierInput {
    data_size: usize,
    data_byte_to_corrupt: usize,
    hash_byte_to_corrupt: usize,
    root_byte_to_corrupt: usize,
    read_size: usize,
}

impl<'a> Arbitrary<'a> for RandomMerkleVerifierInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let data_size: usize = u.int_in_range(1..=512 * 1024 * 1024 + 20)?;
        let leaf_hash_bytes = data_size.div_ceil(BLOCK_SIZE) * HASH_SIZE;
        Ok(Self {
            data_size,
            data_byte_to_corrupt: u.int_in_range(0..=data_size - 1)?,
            hash_byte_to_corrupt: u.int_in_range(0..=leaf_hash_bytes - 1)?,
            root_byte_to_corrupt: u.int_in_range(0..=HASH_SIZE - 1)?,
            // Read sizes from 8KiB to 256KiB.
            read_size: 8192 * u.int_in_range(1..=32)?,
        })
    }

    fn size_hint(_depth: usize) -> (usize, Option<usize>) {
        arbitrary::size_hint::and_all(&[
            // `data_size` is always less than 2^30.
            (1, Some(4)),
            // `data_byte_to_corrupt` matches `data_size`.
            (1, Some(4)),
            // `hash_byte_to_corrupt` is 1/256th of `data_size` so it's always less than 2^22.
            (1, Some(3)),
            // `root_byte_to_corrupt` will always consume 1 byte.
            (1, Some(1)),
            // `read_size` will always consume 1 byte.
            (1, Some(1)),
        ])
    }
}

fn create_data(size: usize) -> Vec<u8> {
    let mut data = vec![0xAB; size];
    for (i, block) in data.chunks_mut(BLOCK_SIZE).enumerate() {
        // Place the index of the block at the start of the block to make every block unique.
        if block.len() < size_of::<usize>() {
            block.copy_from_slice(&i.to_le_bytes()[0..block.len()]);
        } else {
            block[0..size_of::<usize>()].copy_from_slice(&i.to_le_bytes());
        }
    }
    data
}

fn fuchsia_merkle_fuzzer_impl(input: RandomMerkleVerifierInput) {
    // Create the data 1 byte larger than required. The extra byte is used to try to verify beyond
    // the size of the data.
    let mut data = create_data(input.data_size + 1);
    let (root, leaf_hashes) =
        MerkleRootBuilder::new(Vec::new()).complete(&data[0..input.data_size]);

    {
        let mut leaf_hashes = leaf_hashes.clone().into_boxed_slice();
        let hash_index = input.hash_byte_to_corrupt / HASH_SIZE;
        let mut hash: [u8; HASH_SIZE] = leaf_hashes[hash_index].into();
        hash[input.hash_byte_to_corrupt % HASH_SIZE] ^= 0xFF;
        leaf_hashes[hash_index] = Hash::from_array(hash);
        // Constructing the verifier should fail with a flipped bit in the hashes.
        MerkleVerifier::new(root, leaf_hashes).unwrap_err();
    }
    {
        let leaf_hashes = leaf_hashes.clone().into_boxed_slice();
        let mut root: [u8; HASH_SIZE] = root.into();
        root[input.root_byte_to_corrupt] ^= 0xFF;
        // Constructing the verifier should fail with a flipped bit in the root.
        MerkleVerifier::new(Hash::from_array(root), leaf_hashes).unwrap_err();
    }

    let verifier = MerkleVerifier::new(root, leaf_hashes.into_boxed_slice()).unwrap();

    // Verify all of the data at once.
    verifier.verify(0, &data[0..input.data_size]).unwrap();

    // Verifying too many and too few bytes should fail.
    if (input.data_size - 1) % BLOCK_SIZE != 0 {
        // If the input size - 1 is a multiple of the block size then verifying 1 too few bytes is
        // valid.
        verifier.verify(0, &data[0..input.data_size - 1]).unwrap_err();
    }
    verifier.verify(0, &data[0..input.data_size + 1]).unwrap_err();

    let read_aligned_verifier =
        ReadSizedMerkleVerifier::new(verifier.clone(), input.read_size).unwrap();
    // Verify every read aligned chunk.
    for (i, chunk) in data[0..input.data_size].chunks(input.read_size).enumerate() {
        read_aligned_verifier.verify(i * input.read_size, chunk).unwrap();
    }
    // Verifying too many and too few bytes should fail.
    let ending_start_offset = (input.data_size / input.read_size) * input.read_size;
    if ending_start_offset < input.data_size - 1 {
        // If the input size - 1 is a multiple of the read size then 0 bytes would be verified which
        // isn't supported.
        read_aligned_verifier
            .verify(ending_start_offset, &data[ending_start_offset..input.data_size - 1])
            .unwrap_err();
    }
    read_aligned_verifier
        .verify(ending_start_offset, &data[ending_start_offset..input.data_size + 1])
        .unwrap_err();

    // Also verify using verify_aligned on page-aligned buffers.
    let aligned_data_len = input.data_size.next_multiple_of(4096);
    let mut aligned_data = vec![0u8; aligned_data_len];
    aligned_data[0..input.data_size].copy_from_slice(&data[0..input.data_size]);

    verifier.verify_aligned(0, PtrByteSlice::from(&aligned_data[..]), input.data_size).unwrap();

    for (i, chunk) in data[0..input.data_size].chunks(input.read_size).enumerate() {
        let chunk_offset = i * input.read_size;
        let chunk_unaligned_len = chunk.len();
        let chunk_buf_len = chunk_unaligned_len.next_multiple_of(4096);
        let chunk_slice = &aligned_data[chunk_offset..chunk_offset + chunk_buf_len];
        read_aligned_verifier
            .verify_aligned(chunk_offset, PtrByteSlice::from(chunk_slice), chunk_unaligned_len)
            .unwrap();
    }

    data[input.data_byte_to_corrupt] ^= 0xFF;
    // Verification should fail with a flipped bit.
    verifier.verify(0, &data[0..input.data_size]).unwrap_err();

    let offset = input.data_byte_to_corrupt / input.read_size * input.read_size;
    // Verification should fail with a flipped bit.
    read_aligned_verifier
        .verify(offset, &data[offset..std::cmp::min(offset + input.read_size, data.len() - 1)])
        .unwrap_err();

    aligned_data[input.data_byte_to_corrupt] ^= 0xFF;
    verifier.verify_aligned(0, PtrByteSlice::from(&aligned_data[..]), input.data_size).unwrap_err();
    let chunk_unaligned_len = std::cmp::min(input.read_size, input.data_size - offset);
    let chunk_buf_len = chunk_unaligned_len.next_multiple_of(4096);
    read_aligned_verifier
        .verify_aligned(
            offset,
            PtrByteSlice::from(&aligned_data[offset..offset + chunk_buf_len]),
            chunk_unaligned_len,
        )
        .unwrap_err();
}

#[fuzz]
fn fuchsia_merkle_fuzzer(input: RandomMerkleVerifierInput) {
    fuchsia_merkle_fuzzer_impl(input);
}
