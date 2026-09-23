// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;

/// Helper struct that encapsulates generation of valid/invalid sets of flags based on
/// which rights are supported by a particular node type.
pub struct Rights {
    rights: fio::Rights,
}

impl Rights {
    /// Creates a new Rights struct based on the rights in specified in `fio::Rights`.
    pub fn new(rights: fio::Rights) -> Rights {
        Rights { rights }
    }

    /// Returns all supported rights.
    pub fn all_rights(&self) -> fio::Rights {
        self.rights
    }

    /// Returns all supported rights as `fio::Flags` right flags.
    pub fn all_flags(&self) -> fio::Flags {
        // We can do this conversion because Flags has a 1:1 to Rights.
        fio::Flags::from_bits_truncate(self.rights.bits())
    }

    /// Returns a vector of all valid rights combinations.
    pub fn rights_combinations(&self) -> Vec<fio::Rights> {
        build_flag_combinations(fio::Rights::empty(), self.rights)
    }

    /// Returns a vector of all valid flag combinations as `fio::Flags` flags.
    pub fn combinations(&self) -> Vec<fio::Flags> {
        build_flag_combinations(fio::Flags::empty(), self.all_flags())
    }

    // Returns all rights combinations that contains all the rights specified in `with_rights`.
    fn rights_combinations_containing(&self, with_rights: fio::Rights) -> Vec<fio::Rights> {
        let mut right_combinations = self.rights_combinations();
        right_combinations.retain(|&flags| flags.contains(with_rights));
        right_combinations
    }

    /// Returns all rights combinations that include *all* the specified rights in `with_rights` as
    /// `fio::Flags` flags. Will be empty if none of the requested rights are supported.
    pub fn combinations_containing(&self, with_rights: fio::Rights) -> Vec<fio::Flags> {
        self.rights_combinations_containing(with_rights)
            .into_iter()
            .map(|combination| fio::Flags::from_bits_truncate(combination.bits()))
            .collect()
    }

    /// Returns all rights combinations without the rights specified in `without_rights`,
    fn combinations_without_as_rights(&self, without_rights: fio::Rights) -> Vec<fio::Rights> {
        let mut right_combinations = self.rights_combinations();
        right_combinations.retain(|&flags| !flags.intersects(without_rights));
        right_combinations
    }

    /// Returns all rights combinations that does not include the specified rights in
    /// `without_rights` as `fio::Flags` flags. Will be empty if none are supported.
    pub fn combinations_without(&self, without_rights: fio::Rights) -> Vec<fio::Flags> {
        self.combinations_without_as_rights(without_rights)
            .into_iter()
            .map(|combination| fio::Flags::from_bits_truncate(combination.bits()))
            .collect()
    }
}

/// Returns a list of flag combinations to test. Returns a vector of the aggregate of
/// every constant flag and every combination of variable flags. For example, calling
/// build_flag_combinations(100, 011) would return [100, 110, 101, 111] (in binary),
/// whereas build_flag_combinations(0, 011) would return [000, 001, 010, 011].
pub fn build_flag_combinations<T: bitflags::Flags + Copy>(
    constant_flags: T,
    variable_flags: T,
) -> Vec<T> {
    let mut vec = vec![constant_flags];
    for flag in variable_flags.iter() {
        for i in 0..vec.len() {
            vec.push(vec[i].union(flag));
        }
    }
    vec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rights_combinations_flags() {
        const TEST_RIGHTS: fio::Rights = fio::Rights::empty()
            .union(fio::Rights::READ_BYTES)
            .union(fio::Rights::WRITE_BYTES)
            .union(fio::Rights::EXECUTE);

        // We should get 0, R, W, X, RW, RX, WX, RWX (8 in total).
        const EXPECTED_COMBINATIONS: [fio::Flags; 8] = [
            fio::Flags::empty(),
            fio::Flags::PERM_READ_BYTES,
            fio::Flags::PERM_WRITE_BYTES,
            fio::Flags::PERM_EXECUTE,
            fio::Flags::empty()
                .union(fio::Flags::PERM_READ_BYTES)
                .union(fio::Flags::PERM_WRITE_BYTES),
            fio::Flags::empty().union(fio::Flags::PERM_READ_BYTES).union(fio::Flags::PERM_EXECUTE),
            fio::Flags::empty().union(fio::Flags::PERM_WRITE_BYTES).union(fio::Flags::PERM_EXECUTE),
            fio::Flags::empty()
                .union(fio::Flags::PERM_READ_BYTES)
                .union(fio::Flags::PERM_WRITE_BYTES)
                .union(fio::Flags::PERM_EXECUTE),
        ];

        // Check that we get the expected Flags.
        let rights = Rights::new(TEST_RIGHTS);
        assert_eq!(
            rights.all_flags(),
            fio::Flags::PERM_READ_BYTES | fio::Flags::PERM_WRITE_BYTES | fio::Flags::PERM_EXECUTE
        );

        // Test that all possible combinations are generated correctly.
        let all_combinations = rights.combinations();
        assert_eq!(all_combinations.len(), EXPECTED_COMBINATIONS.len());
        for expected_rights in EXPECTED_COMBINATIONS {
            assert!(all_combinations.contains(&expected_rights));
        }

        // Test that combinations including READABLE are generated correctly.
        // We should get R, RW, RX, and RWX (4 in total).
        const EXPECTED_READABLE_COMBOS: [fio::Flags; 4] = [
            fio::Flags::PERM_READ_BYTES,
            fio::Flags::empty()
                .union(fio::Flags::PERM_READ_BYTES)
                .union(fio::Flags::PERM_WRITE_BYTES),
            fio::Flags::empty().union(fio::Flags::PERM_READ_BYTES).union(fio::Flags::PERM_EXECUTE),
            fio::Flags::empty()
                .union(fio::Flags::PERM_READ_BYTES)
                .union(fio::Flags::PERM_WRITE_BYTES)
                .union(fio::Flags::PERM_EXECUTE),
        ];
        let readable_combos = rights.combinations_containing(fio::Rights::READ_BYTES);
        assert_eq!(readable_combos.len(), EXPECTED_READABLE_COMBOS.len());
        for expected_rights in EXPECTED_READABLE_COMBOS {
            assert!(readable_combos.contains(&expected_rights));
        }

        // Test that combinations excluding READABLE are generated correctly.
        // We should get 0, W, X, and WX (4 in total).
        const EXPECTED_NONREADABLE_COMBOS: [fio::Flags; 4] = [
            fio::Flags::empty(),
            fio::Flags::PERM_WRITE_BYTES,
            fio::Flags::PERM_EXECUTE,
            fio::Flags::empty().union(fio::Flags::PERM_WRITE_BYTES).union(fio::Flags::PERM_EXECUTE),
        ];
        let nonreadable_combos = rights.combinations_without(fio::Rights::READ_BYTES);
        assert_eq!(nonreadable_combos.len(), EXPECTED_NONREADABLE_COMBOS.len());
        for expected_rights in EXPECTED_NONREADABLE_COMBOS {
            assert!(nonreadable_combos.contains(&expected_rights));
        }
    }
}
