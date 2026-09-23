// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides a set of stable, performant, non-cryptographic hash functions. The main purpose of this
//! module is to ensure consistency in calculating hashes for values persisted on disk. For
//! simplicity, this only provides high-level [`stable_hash`] and [`stable_hash_seeded`] functions.
//!
//! Currently uses the `rapidhash` algorithm.
//!
//! *WARNING*: These functions are **not** intended for use in security sensitive contexts.

use rapidhash::RapidInlineHasher;
use std::hash::{Hash, Hasher as _};

/// Randomly generated so that we have a different default seed than the underlying library.
const SEED: u64 = 0x0E91685CC9C613AB;

/// Calculate the stable hash of `value`, where `value` implements [`Hash`].
#[inline(always)]
pub fn stable_hash(value: impl Hash) -> u64 {
    stable_hash_seeded(value, SEED)
}

/// Calculate the stable hash of `value` using the given `seed`. `value` must implement [`Hash`].
#[inline(always)]
pub fn stable_hash_seeded(value: impl Hash, seed: u64) -> u64 {
    let mut hasher = RapidInlineHasher::new(seed);
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use crate::stable_hash::{stable_hash, stable_hash_seeded};

    const HASH_CHANGED_ERROR: &'static str = "ERROR: Hash calculation has changed!
This is a breaking change for the fxfs on-disk format and will result in filesystem corruption.
Please consult the storage team for further information, or see https://fxbug.dev/419133532.";

    /// Ensure hash stability for the [`hash`] function. If this test fails, the hash algorithm
    /// has changed, and existing images will appear to be corrupt.
    ///
    /// See https://fxbug.dev/419133532 for details.
    #[test]
    fn hash_stability_default_seed() {
        const TEST_CASES_DEFAULT_SEED: [(&str, u64); 4] = [
            ("Hey, Mr. Tambourine Man, play a song for me", 0x38CB62BF6CAA13C8),
            ("Take me on a trip upon your magic swirling ship", 0x3919C665B5837A0A),
            ("Far from the twisted reach of ..... sorrow", 0xF02083B6F5AF16EB),
            ("Yes, to dance beneath the diamond sky", 0x41B5DD19F640E07E),
        ];

        for (input, expected_hash) in TEST_CASES_DEFAULT_SEED {
            let calculated_hash = stable_hash(input);
            assert_eq!(calculated_hash, expected_hash, "{}", HASH_CHANGED_ERROR);
        }
    }

    /// Ensure hash stability for the [`hash_with_seed`] function. If this test fails, the hash
    /// algorithm has changed, and existing images will appear to be corrupt.
    ///
    /// See https://fxbug.dev/419133532 for details.
    #[test]
    fn hash_stability_custom_seed() {
        const CUSTOM_SEED: u64 = 0x6090973e8c9301cf;
        const TEST_CASES_CUSTOM_SEED: [(&str, u64); 4] = [
            ("Don't carry the world upon your shoulders", 0x98DE2AEFE2BDB0C2),
            ("For, well, you know that it's a fool who plays it cool", 0x59CF6912804B582F),
            ("By making his world a little colder", 0x3AAAEBDF6CB76519),
            ("Na-na-na-na-na, na-na-na-na", 0xCE95B7A14540CD0E),
        ];

        for (input, expected_hash) in TEST_CASES_CUSTOM_SEED {
            let calculated_hash = stable_hash_seeded(input, CUSTOM_SEED);
            assert_eq!(calculated_hash, expected_hash, "{}", HASH_CHANGED_ERROR);
        }
    }
}
