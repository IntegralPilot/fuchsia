// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FsVerityHash;
use crate::tree::MerkleTree;
use crate::util::FsVerityHasher;
use std::cmp::min;
use zerocopy::IntoBytes;

/// A `MerkleTreeBuilder` generates a [`MerkleTree`] from one or more write calls.
#[derive(Clone, Debug)]
pub struct MerkleTreeBuilder<D> {
    /// Buffer to hold a partial block of data between [`MerkleTreeBuilder::write`] calls.
    /// `block.len()` will never exceed `hasher.block_size()`.
    block: Vec<u8>,
    levels: Vec<Vec<D>>,
    hasher: FsVerityHasher,
}

impl<D: FsVerityHash> MerkleTreeBuilder<D> {
    /// Creates a new, empty `MerkleTreeBuilder`.
    pub fn new(hasher: FsVerityHasher) -> Self {
        MerkleTreeBuilder {
            block: Vec::with_capacity(hasher.block_size().into()),
            levels: vec![Vec::new()],
            hasher,
        }
    }

    fn hash_block(&self, data: &[u8]) -> D {
        let hash_bytes = self.hasher.hash_block(data);
        D::read_from_bytes(&hash_bytes[..]).unwrap()
    }

    /// Append a buffer of bytes to the merkle tree.
    ///
    /// No internal buffering is required if all writes are `self.hasher.block_size()` aligned.
    pub fn write(&mut self, buf: &[u8]) {
        let block_size = self.hasher.block_size();
        // Fill the current partial block, if it exists.
        let buf = if self.block.is_empty() {
            buf
        } else {
            let left = block_size - self.block.len();
            let prefix = min(buf.len(), left.into());
            let (buf, rest) = buf.split_at(prefix);
            self.block.extend_from_slice(buf);
            if self.block.len() == block_size {
                self.push_data_hash(self.hash_block(&self.block[..]));
            }
            rest
        };

        // Write full blocks, saving any final partial block for later writes.
        for block in buf.chunks(block_size) {
            if block.len() == block_size {
                self.push_data_hash(self.hash_block(block));
            } else {
                self.block.extend_from_slice(block);
            }
        }
    }

    /// Save a data block hash, propagating full blocks of hashes to higher layers. Also clear a
    /// stored data block.
    pub fn push_data_hash(&mut self, hash: D) {
        let hashes_per_block = self.hasher.block_size() / self.hasher.hash_size();
        self.block.clear();
        self.levels[0].push(hash);
        if self.levels[0].len() % hashes_per_block == 0 {
            self.commit_tail_block(0);
        }
    }

    /// Hash a complete (or final partial) block of hashes, chaining to higher levels as needed.
    fn commit_tail_block(&mut self, level: usize) {
        let hashes_per_block = self.hasher.block_size() / self.hasher.hash_size();

        let len = self.levels[level].len();
        let next_level = level + 1;

        if next_level >= self.levels.len() {
            self.levels.push(Vec::new());
        }

        let first_hash = if len % hashes_per_block == 0 {
            len - hashes_per_block
        } else {
            if !self.hasher.fsverity() {
                let zero_hash = self.hash_block(&[]);
                for _ in 0..(hashes_per_block - (len % hashes_per_block)) {
                    self.levels[level].push(zero_hash);
                }
            }
            len - (len % hashes_per_block)
        };

        let block_bytes = self.levels[level][first_hash..].as_bytes();
        let hash = self.hash_block(block_bytes);

        self.levels[next_level].push(hash);
        if self.levels[next_level].len() % hashes_per_block == 0 {
            self.commit_tail_block(next_level);
        }
    }

    /// Finalize all levels of the merkle tree, converting this `MerkleTreeBuilder` instance to a
    /// [`MerkleTree`].
    pub fn finish(mut self) -> MerkleTree {
        let hashes_per_block = self.hasher.block_size() / self.hasher.hash_size();

        // The data protected by the tree may not be `hasher.block_size()` aligned. Commit a partial
        // data block before finalizing the hash levels.
        // Also, an empty tree consists of a single, empty block. Handle that case now as well.
        if !self.block.is_empty() || self.levels[0].is_empty() {
            self.push_data_hash(self.hash_block(&self.block[..]));
        }

        // Enumerate the hash levels, finalizing any that have a partial block of hashes.
        // `commit_tail_block` may add new levels to the tree, so don't assume a length up front.
        for level in 0.. {
            if level >= self.levels.len() {
                break;
            }

            let len = self.levels[level].len();
            if len > 1 && len % hashes_per_block != 0 {
                self.commit_tail_block(level);
            }
        }

        let flat_levels: Vec<Box<[u8]>> = self
            .levels
            .into_iter()
            .map(|level| level.as_bytes().to_vec().into_boxed_slice())
            .collect();

        MerkleTree::from_levels(flat_levels, self.hasher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::FsVerityHasherOptions;
    use crate::{Sha256Hash, Sha512Hash};
    use hex::FromHex;
    use test_case::test_case;

    /// Output produced via:
    /// fsverity digest foo.txt --out-descriptor=/tmp/descriptor
    /// hexdump /tmp/descriptor -e "16/1 \"%02x\" \"\n\"" -v
    #[allow(clippy::unused_unit)]
    #[test_case(vec![], "0000000000000000000000000000000000000000000000000000000000000000" ; "test_empty")]
    #[test_case(vec![0xFF; 8192], "e95eba0e6902ce10c80029e06051080479c696c21b63c3fffa4d7a01aa15e8cb" ; "test_oneblock")]
    #[test_case(vec![0xFF; 65536], "b4d0d21943e744d999df91cf5efe432744f41d1763a04a75ef6559e04a143857"; "test_small")]
    #[test_case(vec![0xFF; 2105344], "b4050a226383d94c09c004d59a81b08bed17726b79cf9bd0994931f13213652d"; "test_large")]
    #[test_case(vec![0xFF; 2109440], "1a07efa041afdf78b86df2c580ec6f8446eb6e802321252996563c14334a5342"; "test_unaligned")]
    fn sha256_tests_no_salt(input: Vec<u8>, output: &str) {
        let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(FsVerityHasher::Sha256(
            FsVerityHasherOptions::new(vec![], 4096),
        ));
        builder.write(input.as_slice());
        let tree = builder.finish();
        let expected: [u8; 32] = FromHex::from_hex(output).unwrap();
        assert_eq!(expected, tree.root());
    }

    /// Output produced via:
    /// fsverity digest foo.txt --out-descriptor=/tmp/descriptor --salt="ffffffffffffffff"
    /// hexdump /tmp/descriptor -e "16/1 \"%02x\" \"\n\"" -v
    #[test_case(vec![], "0000000000000000000000000000000000000000000000000000000000000000" ; "test_empty")]
    #[test_case(vec![0xFF; 8192], "e9c09b505561b9509f93b5c7990ed41427f708480c56306453d505e94076d600" ; "test_oneblock")]
    #[test_case(vec![0xFF; 65536], "ec6b4dc183833a5665b8d804c6e900f2543b54914f153e4139cb77b261f59615"; "test_small")]
    #[test_case(vec![0xFF; 2105344], "b433c8b632c79ca9fc2c04913541aa38970ae9da04a43269f67770221e79fe37"; "test_large")]
    #[test_case(vec![0xFF; 2109440], "fbd261c306f522aba5ac0c70229870594d236634f5afe68fe9656ea04eb4a4fe"; "test_unaligned")]
    fn sha256_tests_with_salt(input: Vec<u8>, output: &str) {
        let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(FsVerityHasher::Sha256(
            FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
        ));
        builder.write(input.as_slice());
        let tree = builder.finish();
        let expected: [u8; 32] = FromHex::from_hex(output).unwrap();
        assert_eq!(expected, tree.root());
    }

    /// Output produced via:
    /// fsverity digest foo.txt --out-descriptor=/tmp/descriptor --hash-alg=sha512
    /// hexdump /tmp/descriptor -e "16/1 \"%02x\" \"\n\"" -v
    #[test_case(vec![], "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000" ; "test_empty")]
    #[test_case(vec![0xFF; 8192], "9606a93b78555a72b49e583fca582bc5b0411decd879286378d86b5f42190f33b560071063622b3d8e0abc9118e3838dbe77301870743ffc80bd0c910ab3522e" ; "test_oneblock")]
    #[test_case(vec![0xFF; 65536], "524238523bf3c88f78fba612223d322b3c4290a9ecbd9c61aeb8f1c293b0740083a61a0b2e344b8dc6020ece806ea1d048885db1e77ee9fbaa63c3589f6e403b"; "test_small")]
    #[test_case(vec![0xFF; 2105344], "51977ac06edd17d32761e27d384f6c437ead6922f0a3fbabc3390d8f6e929bc1d9ff9e4ee34fb060484e8eff272f9cc36fa1cf26361c3258b5d8b87d8144b497"; "test_large")]
    #[test_case(vec![0xFF; 2109440], "f6e821f7cdd1306031080ff99c4c2d7270c6d6bbaa07f4e3040a5d20a1178af1e4f6377f898166d5835ec22b2fcca6d364711cf0c20862d40f3580b6b6276683"; "test_unaligned")]
    fn sha512_tests_no_salt(input: Vec<u8>, output: &str) {
        let mut builder = MerkleTreeBuilder::<Sha512Hash>::new(FsVerityHasher::Sha512(
            FsVerityHasherOptions::new(vec![], 4096),
        ));
        builder.write(input.as_slice());
        let tree = builder.finish();
        let expected: [u8; 64] = FromHex::from_hex(output).unwrap();
        assert_eq!(expected, tree.root());
    }

    /// Output produced via:
    /// fsverity digest foo.txt --out-descriptor=/tmp/descriptor --salt="ffffffffffffffff" --hash-alg=sha512
    /// hexdump /tmp/descriptor -e "16/1 \"%02x\" \"\n\"" -v
    #[test_case(vec![], "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000" ; "test_empty")]
    #[test_case(vec![0xFF; 8192], "22750472f522bf68a1fe2a66ee1ac57759b322c634d931097b3751e3cd9fe9dd2d8f551631922bf8f675e4b5e3a38e6db11c7df0e5053e80ffbac2c2d7a0105b" ; "test_oneblock")]
    #[test_case(vec![0xFF; 65536], "4f6a2e16dabf6347b9ae88d5c298befcff0cc71abe1905fa6aefcee14fa5acb89ecbf949daef002d11a9dbb51f211f0eb3e2f7f5e2911b0af2e9fb68c7799a94"; "test_small")]
    #[test_case(vec![0xFF; 2105344], "a92ddf722dfcf679a64b6364de7f823850f8f856e0ba2c53d66f75cf72d5572bf1d525b3c185e5c39818e2d29997d259f81363daab80a902f86291a71514f891"; "test_large")]
    #[test_case(vec![0xFF; 2109440], "b6913e8c1d3bb84b467e24667aedad0491ad86f548e849741969688b2526919a380946bebf481ec1ee1bdda86631e10c4a82e7329afdd84db2ac43994a524785"; "test_unaligned")]
    fn sha512_tests_with_salt(input: Vec<u8>, output: &str) {
        let mut builder = MerkleTreeBuilder::<Sha512Hash>::new(FsVerityHasher::Sha512(
            FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
        ));
        builder.write(input.as_slice());
        let tree = builder.finish();
        let expected: [u8; 64] = FromHex::from_hex(output).unwrap();
        assert_eq!(expected, tree.root());
    }

    #[test]
    fn test_unaligned_single_block_sha256() {
        let data = vec![0xFF; 8192];
        let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(FsVerityHasher::Sha256(
            FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
        ));
        let (first, second) = &data[..].split_at(1024);
        builder.write(first);
        builder.write(second);
        let tree = builder.finish();
        let expected: [u8; 32] =
            FromHex::from_hex("e9c09b505561b9509f93b5c7990ed41427f708480c56306453d505e94076d600")
                .unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_unaligned_single_block_sha512() {
        let data = vec![0xFF; 8192];
        let mut builder = MerkleTreeBuilder::<Sha512Hash>::new(FsVerityHasher::Sha512(
            FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
        ));
        let (first, second) = &data[..].split_at(1024);
        builder.write(first);
        builder.write(second);
        let tree = builder.finish();
        let expected: [u8; 64] = FromHex::from_hex("22750472f522bf68a1fe2a66ee1ac57759b322c634d931097b3751e3cd9fe9dd2d8f551631922bf8f675e4b5e3a38e6db11c7df0e5053e80ffbac2c2d7a0105b").unwrap();
        assert_eq!(tree.root(), expected);
    }

    #[test]
    fn test_unaligned_n_block_sha256() {
        let data = vec![0xFF; 65536];
        let expected: [u8; 32] =
            FromHex::from_hex("ec6b4dc183833a5665b8d804c6e900f2543b54914f153e4139cb77b261f59615")
                .unwrap();

        for chunk_size in &[1, 100, 1024, 8193] {
            let mut builder = MerkleTreeBuilder::<Sha256Hash>::new(FsVerityHasher::Sha256(
                FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
            ));
            for block in data.as_slice().chunks(*chunk_size) {
                builder.write(block);
            }
            let tree = builder.finish();

            assert_eq!(tree.root(), expected);
        }
    }

    #[test]
    fn test_unaligned_n_block_sha512() {
        let data = vec![0xFF; 65536];
        let expected: [u8; 64] = FromHex::from_hex("4f6a2e16dabf6347b9ae88d5c298befcff0cc71abe1905fa6aefcee14fa5acb89ecbf949daef002d11a9dbb51f211f0eb3e2f7f5e2911b0af2e9fb68c7799a94").unwrap();

        for chunk_size in &[1, 100, 1024, 8193] {
            let mut builder = MerkleTreeBuilder::<Sha512Hash>::new(FsVerityHasher::Sha512(
                FsVerityHasherOptions::new(vec![0xFF; 8], 4096),
            ));
            for block in data.as_slice().chunks(*chunk_size) {
                builder.write(block);
            }
            let tree = builder.finish();

            assert_eq!(tree.root(), expected);
        }
    }
}
