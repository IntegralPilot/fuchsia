// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities used by tests in both file and directory modules.

#![cfg(test)]

#[cfg(test)]
pub(crate) mod assertions;

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
    use super::build_flag_combinations;

    bitflags::bitflags! {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct TestFlags: u32 {
            const A = 1<<0;
            const B = 1<<1;
            const C = 1<<2;
        }
    }

    #[test]
    fn test_build_flag_combinations() {
        let constant_flags = TestFlags::C;
        let variable_flags = TestFlags::A | TestFlags::B;
        let generated_combinations = build_flag_combinations(constant_flags, variable_flags);
        let expected_result = [
            TestFlags::C,
            TestFlags::A | TestFlags::C,
            TestFlags::B | TestFlags::C,
            TestFlags::A | TestFlags::B | TestFlags::C,
        ];
        assert_eq!(generated_combinations, expected_result);
    }

    #[test]
    fn test_build_flag_combinations_with_empty_constant_flags() {
        let constant_flags = TestFlags::empty();
        let variable_flags = TestFlags::A | TestFlags::B;
        let generated_combinations = build_flag_combinations(constant_flags, variable_flags);
        let expected_result =
            [TestFlags::empty(), TestFlags::A, TestFlags::B, TestFlags::A | TestFlags::B];
        assert_eq!(generated_combinations, expected_result);
    }

    #[test]
    fn test_build_flag_combinations_with_empty_variable_flags() {
        let constant_flags = TestFlags::A | TestFlags::B;
        let variable_flags = TestFlags::empty();
        let generated_combinations = build_flag_combinations(constant_flags, variable_flags);
        let expected_result = [constant_flags];
        assert_eq!(generated_combinations, expected_result);
    }

    #[test]
    fn test_build_flag_combinations_with_empty_flags() {
        let constant_flags = TestFlags::empty();
        let variable_flags = TestFlags::empty();
        let generated_combinations = build_flag_combinations(constant_flags, variable_flags);
        let expected_result = [constant_flags];
        assert_eq!(generated_combinations, expected_result);
    }
}
