// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 benchmark utilities.

/// Declare a benchmark function.
///
/// If `cfg(benchmark)` is enabled, a function named `name` is emitted and it
/// receives [`RealBencher`].
///
/// If `cfg(test)` is enabled, a module named `name` with a single test called
/// `test_bench` is emitted and it receives [`TestBencher`].
///
/// This setup allows for all benchmarking functions to run in unit test mode,
/// allowing problems to be noticed sooner.
///
/// Note that `$fn` doesn't have to be a named function - it can also be an
/// anonymous closure.
#[macro_export]
macro_rules! bench {
    ($name:ident, $fn:expr) => {
        #[cfg(benchmark)]
        pub(crate) fn $name(b: &mut $crate::testutil::RealBencher<'_>) {
            $fn(b);
        }

        #[cfg(test)]
        mod $name {
            use super::*;
            #[test]
            fn test_bench() {
                $fn(&mut $crate::testutil::TestBencher);
            }
        }
    };
}

/// A trait to allow faking of the type providing benchmarking.
pub trait Bencher {
    /// Benchmarks `inner` by running it multiple times.
    fn iter<T, F: FnMut() -> T>(&mut self, inner: F);

    /// Abstracts blackboxing.
    ///
    /// `black_box` prevents the compiler from optimizing a function with an
    /// unused return type.
    fn black_box<T>(placeholder: T) -> T;
}

/// An alias for the bencher used in real benchmarks.
pub use criterion::Bencher as RealBencher;

impl Bencher for RealBencher<'_> {
    fn iter<T, F: FnMut() -> T>(&mut self, inner: F) {
        criterion::Bencher::iter(self, inner)
    }

    #[inline(always)]
    fn black_box<T>(placeholder: T) -> T {
        core::hint::black_box(placeholder)
    }
}

/// A `Bencher` whose `iter` method runs the provided argument a small,
/// fixed number of times.
pub struct TestBencher;

impl Bencher for TestBencher {
    fn iter<T, F: FnMut() -> T>(&mut self, mut inner: F) {
        const NUM_TEST_ITERS: u32 = 3;
        for _ in 0..NUM_TEST_ITERS {
            let _: T = inner();
        }
    }

    #[inline(always)]
    fn black_box<T>(placeholder: T) -> T {
        placeholder
    }
}
