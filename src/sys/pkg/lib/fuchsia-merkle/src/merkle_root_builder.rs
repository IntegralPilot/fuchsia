// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::util::{HASHES_PER_BLOCK, hash_block, make_hash_hasher, update_with_zeros};
use crate::{BLOCK_SIZE, Hash};
use bssl_crypto::digest::Sha256;
use fuchsia_hash::HASH_SIZE;

const EMPTY_MERKLE_TREE_ROOT: Hash = Hash::from_array([
    0x15, 0xec, 0x7b, 0xf0, 0xb5, 0x07, 0x32, 0xb4, 0x9f, 0x82, 0x28, 0xe0, 0x7d, 0x24, 0x36, 0x53,
    0x38, 0xf9, 0xe3, 0xab, 0x99, 0x4b, 0x00, 0xaf, 0x08, 0xe5, 0xa3, 0xbf, 0xfe, 0x55, 0xfd, 0x8b,
]);

/// Hashes blocks of hashes.
struct LevelHasher {
    hasher: Sha256,
    offset: usize,
}

/// Calculates the merkle root for a given set of data. The leaf hashes of the merkle tree can
/// optionally be collected for use with [`crate::MerkleVerifier`].
///
/// [`MerkleRootBuilder`] only accepts complete blocks of data except for the last block. This
/// avoids buffering data internally. If complete blocks of data can't be guaranteed then
/// [`BufferedMerkleRootBuilder`] should be used instead.
pub struct MerkleRootBuilder<T> {
    levels: Vec<LevelHasher>,
    offset: usize,
    leaf_hash_collector: T,

    /// Holds the root of the tree when the offset of every level of the tree is a multiple of
    /// [`BLOCK_SIZE`]. This only happens when [`offset`] is equal to `8192 * 256 ^ N` for every
    /// `N > 0`.
    ///
    /// If [`Self::push_hash()`] immediately turned the root into a new level then
    /// [`Self::complete`] wouldn't be able to recover the root if no more data was added.
    root: Option<Hash>,
}

impl<T: LeafHashCollector> MerkleRootBuilder<T> {
    /// Creates a new `MerkleRootBuilder` with a `LeafHashCollector`.
    ///
    /// Use `MerkleRootBuilder::default()` if a `LeafHashCollector` isn't needed.
    pub fn new(leaf_hash_collector: T) -> Self {
        Self { levels: Vec::new(), offset: 0, root: None, leaf_hash_collector }
    }

    /// Appends a full block of data to the merkle tree.
    ///
    /// `MerkleRootBuilder` doesn't buffer any data so only full blocks can be added until
    /// [`Self::complete`] is called. Use [`BufferedMerkleRootBuilder`] if buffering is required.
    pub fn add_block(&mut self, data: &[u8; BLOCK_SIZE]) {
        self.push_hash(hash_block(&data[..], self.offset));
    }

    /// Appends a final amount of data to the merkle tree and returns the merkle root.
    pub fn complete(mut self, data: &[u8]) -> T::Output {
        // Add all of the complete blocks from `data`.
        let remaining = self.add_blocks(data);

        // Handle the last partial block when `data` isn't a block multiple.
        if !remaining.is_empty() {
            self.push_hash(hash_block(remaining, self.offset));
        }

        // Special case for the merkle tree of no data.
        if self.offset == 0 {
            self.leaf_hash_collector.add_leaf_hash(EMPTY_MERKLE_TREE_ROOT);
            return self.leaf_hash_collector.complete(EMPTY_MERKLE_TREE_ROOT);
        }

        // Special case for when every level of the tree is already complete.
        if let Some(root) = self.root {
            return self.leaf_hash_collector.complete(root);
        }

        // Each row of the tree is padded with zeros to fill the last block and the resulting hash
        // is bubbled up to the next level. Since `root` wasn't set, there's at least 1 level that
        // needs padding so `bubbling_digest` is guaranteed to be set by the end of the loop.
        let mut bubbling_digest: Option<[u8; HASH_SIZE]> = None;
        for mut level_hasher in self.levels {
            if level_hasher.offset.is_multiple_of(BLOCK_SIZE) && bubbling_digest.is_none() {
                continue;
            }
            if let Some(digest) = bubbling_digest.take() {
                level_hasher.hasher.update(&digest);
                level_hasher.offset += HASH_SIZE;
            }
            let zeros = BLOCK_SIZE - (level_hasher.offset % BLOCK_SIZE);
            update_with_zeros(&mut level_hasher.hasher, zeros);
            bubbling_digest = Some(level_hasher.hasher.digest());
        }
        self.leaf_hash_collector.complete(Hash::from(bubbling_digest.unwrap()))
    }

    /// Adds a hash of a single block of data to the merkle tree.
    pub(crate) fn push_hash(&mut self, mut digest: Hash) {
        self.offset += BLOCK_SIZE;
        self.leaf_hash_collector.add_leaf_hash(digest);

        if let Some(root) = self.root.take() {
            // The root was set but more data has now been added. Create a new level containing the
            // root.
            let mut hasher = make_hash_hasher(HASHES_PER_BLOCK, self.levels.len() + 1, 0);
            hasher.update(root.as_bytes());
            self.levels.push(LevelHasher { hasher, offset: HASH_SIZE });
        }

        for (level, level_hasher) in self.levels.iter_mut().enumerate() {
            level_hasher.hasher.update(digest.as_bytes());
            level_hasher.offset += HASH_SIZE;

            // Only bubble the hash up to the next level when this level completes a block.
            if !level_hasher.offset.is_multiple_of(BLOCK_SIZE) {
                return;
            }
            // Initialize a new hasher for this level and bubble the old hasher up to the next
            // level.
            let new_hasher = make_hash_hasher(HASHES_PER_BLOCK, level + 1, level_hasher.offset);
            let old_hasher = std::mem::replace(&mut level_hasher.hasher, new_hasher);
            digest = Hash::from_array(old_hasher.digest());
        }

        self.root = Some(digest);
    }

    /// Adds all of the whole blocks from `data` and returns the remaining bytes.
    fn add_blocks<'a>(&mut self, mut data: &'a [u8]) -> &'a [u8] {
        while let Some((block, remainder)) = data.split_first_chunk::<BLOCK_SIZE>() {
            data = remainder;
            self.add_block(block);
        }
        data
    }
}

impl Default for MerkleRootBuilder<NoopLeafHashCollector> {
    fn default() -> Self {
        Self::new(NoopLeafHashCollector)
    }
}

/// Calculates the merkle root for a given set of data. The leaf hashes of the merkle tree can
/// optionally be collected for use with a `MerkleVerifier`.
///
/// [`BufferedMerkleRootBuilder`] is able to accept data that isn't a multiple of [`BLOCK_SIZE`] by
/// internally buffering the data.
///
/// If all of the data is already in memory then `MerkleRootBuilder::default().complete(data)`
/// should be used instead as it avoids buffering the tail end of the data if it's not a complete
/// block.
pub struct BufferedMerkleRootBuilder<T> {
    buffer: Vec<u8>,
    builder: MerkleRootBuilder<T>,
}

impl<T: LeafHashCollector> BufferedMerkleRootBuilder<T> {
    /// Creates a new `BufferedMerkleRootBuilder` with a `LeafHashCollector`.
    ///
    /// Use `BufferedMerkleRootBuilder::default()` if a `LeafHashCollector` isn't needed.
    pub fn new(leaf_hash_collector: T) -> Self {
        Self { buffer: Vec::new(), builder: MerkleRootBuilder::new(leaf_hash_collector) }
    }

    /// Appends data to the merkle tree.
    pub fn write(&mut self, mut data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if self.buffer.len().saturating_add(data.len()) < BLOCK_SIZE {
            // The current buffer and the new data won't complete a block. Buffer the data and
            // return.
            self.buffer.extend_from_slice(data);
            return;
        }

        if !self.buffer.is_empty() {
            // If the buffered data isn't empty then take bytes from the start of the new data to
            // complete a block. The above case handles when a block wouldn't be completed so this
            // is guaranteed to complete a block.
            let (head, tail) = data.split_at(BLOCK_SIZE - self.buffer.len());
            self.buffer.extend_from_slice(head);
            self.builder.add_block(self.buffer.as_slice().try_into().unwrap());
            self.buffer.clear();
            data = tail;
        }

        // Add all remaining whole blocks.
        data = self.builder.add_blocks(data);
        if !data.is_empty() {
            // Buffer any remaining data.
            self.buffer.extend_from_slice(data);
        }
    }

    /// Finishes building the merkle tree and returns the merkle root.
    pub fn complete(self) -> T::Output {
        self.builder.complete(&self.buffer)
    }
}

impl Default for BufferedMerkleRootBuilder<NoopLeafHashCollector> {
    fn default() -> Self {
        Self::new(NoopLeafHashCollector)
    }
}

impl<T: LeafHashCollector> std::io::Write for BufferedMerkleRootBuilder<T> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A trait for collecting the leaf hashes of a merkle tree while constructing a merkle root.
pub trait LeafHashCollector {
    /// The output type of [`MerkleRootBuilder::complete`]. This allows for `complete` to return
    /// just the merkle root with `NoopLeafHashCollector` and also return the leaf hashes when a
    /// real `LeafHashCollector` is used.
    type Output;

    /// This method is called as each leaf hash in the merkle tree is created.
    ///
    /// If the merkle tree consists of just a root node then this method will be called for the root
    /// node.
    fn add_leaf_hash(&mut self, hash: Hash);

    /// Transforms the merkle root into Self::Output.
    fn complete(self, root: Hash) -> Self::Output;
}

/// A `LeafHashCollector` that doesn't collect leaf hashes.
pub struct NoopLeafHashCollector;
impl LeafHashCollector for NoopLeafHashCollector {
    type Output = Hash;

    fn add_leaf_hash(&mut self, _hash: Hash) {}

    fn complete(self, root: Hash) -> Self::Output {
        root
    }
}

impl LeafHashCollector for Vec<Hash> {
    type Output = (Hash, Vec<Hash>);

    fn add_leaf_hash(&mut self, hash: Hash) {
        self.push(hash);
    }

    fn complete(self, root: Hash) -> Self::Output {
        (root, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const AB_BLOCK: [u8; BLOCK_SIZE] = [0xAB; BLOCK_SIZE];

    fn add_blocks<T: LeafHashCollector>(block_count: usize, builder: &mut MerkleRootBuilder<T>) {
        for _ in 0..block_count {
            builder.add_block(&AB_BLOCK);
        }
    }

    // Hashes and the length of repeated 0xAB bytes. These were generated from the C++
    // implementation in //src/lib/digest.
    #[test_case("15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b", 0; "0")]
    #[test_case("2fabda7adeac120f587ae7b159c39d1716758b2c65e25b856470bbbb8b94e25e", 1; "1")]
    #[test_case("fd2acae20c551eea1df052f39478b33536ebf3b0c6ceef649a010de06b71822b", 4096; "4096")]
    #[test_case("85a0918a035140e720f4a5f10b7768d5b04959ed526da401cf4893aac73f436a", 8191; "8191")]
    #[test_case("1dc023a7a88f56fab52e0b078a92cd566546d11aed479c66a14d6592e0f011eb", 8192; "8192")]
    #[test_case("b6eae68aa1ad1b4fc74618575135f0d1daedc6bb07dc10f8fa41916d0578c318", 8193; "8193")]
    #[test_case(
        "35689edb3c4c46e44bb4ff00a7f8ac628d97ba16e1233f78759a41da597ed8dd",
        8192 * 2 - 1; "16383"
    )]
    #[test_case(
        "a6a903830d911d7555f9243b748d27a5aec1e385f91d845c854f46e02339070a",
        8192 * 2; "16384"
    )]
    #[test_case(
        "54c9eb182fc5bfc2984ca45049cf8cf1d071f2165e5190605b7970f824e03d8f",
        8192 * 2 + 1; "16385"
    )]
    #[test_case(
        "c96631f69f0cfaf2626dc4230aeb7fa50a05cd7e897039d2f56cdec99dc0b3b1",
        8192 * 256 - 1; "2097151"
    )]
    #[test_case(
        "cb717b240ae7ddf1ad197a3272db3cb4175bb7b7242d5050c6317df2bb8ed4b4",
        8192 * 256; "2097152"
    )]
    #[test_case(
        "9942b1f926e3295053e97b2b5a1471b6dff7df3d130f589c2881f9b98eee2fd0",
        8192 * 256 + 1; "2097153"
    )]
    #[test_case(
        "59f9f6275ec8b672446bc8fc2da0a7ce625a4c55fcbb3891144ace7a145f23e2",
        8192 * 256 * 2 - 1; "4194303"
    )]
    #[test_case(
        "fa21580bb27be533d2846e75e7fb3acca3fa2605f3fd3a4d49db2d023b10324b",
        8192 * 256 * 2; "4194304"
    )]
    #[test_case(
        "aec444c86e0553ed4a2a5e49faa2db9e6c9f0bdb6b97a8bd2b84aebec30b4558",
        8192 * 256 * 2 + 1; "4194305"
    )]
    #[test_case(
        "799bb12a5f633f9949f4619e4e0af447c980f895e7261521abf69a2dba67ce9e",
        8192 * 256 * 256 - 1; "536870911"
    )]
    #[test_case(
        "b3b1a0ceb8e5949a5ba3f6db646e037438fe15053113c1b50b290678596467e3",
        8192 * 256 * 256; "536870912"
    )]
    #[test_case(
        "53b85ef0285254b7bb32bc6cd5092fdf1a610d96446848f4dd2841e4c93a55d2",
        8192 * 256 * 256 + 1; "536870913"
    )]
    fn test_merkle_root_builder_matches_lib_digest(hash: &str, size: usize) {
        let expected: Hash = hash.parse().unwrap();
        let mut builder = MerkleRootBuilder::default();
        add_blocks(size / BLOCK_SIZE, &mut builder);
        let actual = builder.complete(&AB_BLOCK[0..(size % BLOCK_SIZE)]);
        assert_eq!(actual, expected, "size={size}");
    }

    #[test_case(0, &[], false; "0")]
    #[test_case(8192, &[], true; "8192")]
    #[test_case(8192 * 2, &[64], false; "16384")]
    #[test_case(8192 * 255, &[8192 - 32], false; "2088960")]
    #[test_case(8192 * 256, &[8192], true; "2097152")]
    #[test_case(8192 * 257, &[8192 + 32, 32], false; "2105344")]
    #[test_case(8192 * 256 * 2, &[8192 * 2, 64], false; "4194304")]
    #[test_case(8192 * 256 * 255, &[8192 * 255, 8192 - 32], false; "534773760")]
    #[test_case(8192 * 256 * 256, &[8192 * 256, 8192], true; "536870912")]
    fn test_merkle_tree_structure(size: usize, expected_offsets: &[usize], is_root_set: bool) {
        let mut builder = MerkleRootBuilder::default();
        add_blocks(size / BLOCK_SIZE, &mut builder);
        let actual_offsets = builder.levels.iter().map(|level| level.offset).collect::<Vec<_>>();
        assert_eq!(&actual_offsets, expected_offsets);
        assert_eq!(builder.root.is_some(), is_root_set);
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
    #[test_case(8192 * 256 * 256 - 1; "536870911")]
    #[test_case(8192 * 256 * 256; "536870912")]
    #[test_case(8192 * 256 * 256 + 1; "536870913")]
    fn test_rebuild_from_leaf_hashes(size: usize) {
        let mut builder = MerkleRootBuilder::new(Vec::new());
        add_blocks(size / BLOCK_SIZE, &mut builder);
        let (expected_root, leaf_hashes) = builder.complete(&AB_BLOCK[0..(size % BLOCK_SIZE)]);

        // There should always be leaf hashes, even for the empty input.
        assert!(!leaf_hashes.is_empty());

        let mut builder = MerkleRootBuilder::default();
        for hash in leaf_hashes {
            builder.push_hash(hash);
        }

        assert_eq!(builder.complete(&[]), expected_root);
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
    fn test_buffered_merkle_root_builder(size: usize) {
        let data = vec![0; size];
        let expected_root = MerkleRootBuilder::default().complete(&data);

        let mut builder = BufferedMerkleRootBuilder::default();
        for chunk in data.chunks(6079) {
            builder.write(chunk);
        }
        assert_eq!(builder.complete(), expected_root);

        let mut builder = BufferedMerkleRootBuilder::default();
        builder.write(&data);
        assert_eq!(builder.complete(), expected_root);
    }

    #[test]
    fn test_buffered_merkle_root_builder_as_writer() {
        let data = vec![0; 8192 * 3 + 60];
        let expected_root = MerkleRootBuilder::default().complete(&data);

        let mut reader = std::io::Cursor::new(data);
        let mut builder = BufferedMerkleRootBuilder::default();
        std::io::copy(&mut reader, &mut builder).unwrap();

        assert_eq!(builder.complete(), expected_root);
    }
}
