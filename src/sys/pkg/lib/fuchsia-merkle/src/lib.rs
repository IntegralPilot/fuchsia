// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `fuchsia_merkle` contains types and methods for building and working with merkle trees.
//!
//! See https://fuchsia.dev/fuchsia-src/concepts/security/merkleroot for information on constructing
//! merkle trees.

#![deny(missing_docs)]

use std::io::{self, Read};

pub use fuchsia_hash::{HASH_SIZE, Hash};

/// The size of a single block of data (or hashes), in bytes.
pub const BLOCK_SIZE: usize = 8192;

mod util;

mod merkle_root_builder;
pub use crate::merkle_root_builder::{
    BufferedMerkleRootBuilder, LeafHashCollector, MerkleRootBuilder, NoopLeafHashCollector,
};

mod merkle_verifier;
pub use crate::merkle_verifier::{MerkleVerifier, ReadSizedMerkleVerifier};

/// Computes the merkle root of in-memory data.
pub fn root_from_slice(slice: impl AsRef<[u8]>) -> Hash {
    MerkleRootBuilder::default().complete(slice.as_ref())
}

/// Computes the merkle root of the contents of a `std::io::Read`.
pub fn root_from_reader(mut reader: impl Read) -> Result<Hash, io::Error> {
    let mut builder = BufferedMerkleRootBuilder::default();
    std::io::copy(&mut reader, &mut builder)?;
    Ok(builder.complete())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_root_from_reader() {
        let file = b"hello world";
        let expected = MerkleRootBuilder::default().complete(file);

        let actual = root_from_reader(&file[..]).unwrap();
        assert_eq!(expected, actual);
    }
}
