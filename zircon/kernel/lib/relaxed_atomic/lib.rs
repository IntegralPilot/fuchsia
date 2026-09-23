// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

use core::sync::atomic::{
    AtomicBool, AtomicI8, AtomicI16, AtomicI32, AtomicI64, AtomicIsize, AtomicU8, AtomicU16,
    AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use paste::paste;

/// A wrapper around an atomic primitive providing relaxed memory ordering operations.
#[repr(transparent)]
#[derive(Debug, Default)]
pub struct RelaxedAtomic<T>(T);

macro_rules! impl_relaxed_atomic {
    ($atomic_type:ident, $primitive_type:ident) => {
        impl $crate::RelaxedAtomic<$atomic_type> {
            /// Creates a new relaxed atomic initialized with `val`.
            pub const fn new(val: $primitive_type) -> Self {
                Self($atomic_type::new(val))
            }

            /// Loads the value with relaxed memory ordering.
            #[inline]
            pub fn load(&self) -> $primitive_type {
                self.0.load(Ordering::Relaxed)
            }

            /// Stores a value with relaxed memory ordering.
            #[inline]
            pub fn store(&self, val: $primitive_type) {
                self.0.store(val, Ordering::Relaxed);
            }

            /// Swaps a value with relaxed memory ordering.
            #[inline]
            pub fn swap(&self, val: $primitive_type) -> $primitive_type {
                self.0.swap(val, Ordering::Relaxed)
            }

            /// Performs compare-and-exchange with relaxed memory ordering.
            #[inline]
            pub fn compare_exchange(
                &self,
                current: $primitive_type,
                new: $primitive_type,
            ) -> Result<$primitive_type, $primitive_type> {
                self.0.compare_exchange(current, new, Ordering::Relaxed, Ordering::Relaxed)
            }

            /// Performs weak compare-and-exchange with relaxed memory ordering.
            #[inline]
            pub fn compare_exchange_weak(
                &self,
                current: $primitive_type,
                new: $primitive_type,
            ) -> Result<$primitive_type, $primitive_type> {
                self.0.compare_exchange_weak(current, new, Ordering::Relaxed, Ordering::Relaxed)
            }
        }

        paste! {
            #[doc = concat!("Alias for `RelaxedAtomic<", stringify!($atomic_type), ">`.")]
            pub type [<Relaxed $atomic_type>] = $crate::RelaxedAtomic<$atomic_type>;
        }
    };
}

macro_rules! impl_relaxed_atomic_numeric {
    ($atomic_type:ident, $primitive_type:ident) => {
        impl_relaxed_atomic!($atomic_type, $primitive_type);

        impl $crate::RelaxedAtomic<$atomic_type> {
            /// Adds to the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_add(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_add(val, Ordering::Relaxed)
            }

            /// Subtracts from the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_sub(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_sub(val, Ordering::Relaxed)
            }

            /// Bitwise ANDs the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_and(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_and(val, Ordering::Relaxed)
            }

            /// Bitwise NANDs the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_nand(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_nand(val, Ordering::Relaxed)
            }

            /// Bitwise ORs the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_or(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_or(val, Ordering::Relaxed)
            }

            /// Bitwise XORs the value with relaxed memory ordering.
            #[inline]
            pub fn fetch_xor(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_xor(val, Ordering::Relaxed)
            }

            /// Computes maximum with relaxed memory ordering.
            #[inline]
            pub fn fetch_max(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_max(val, Ordering::Relaxed)
            }

            /// Computes minimum with relaxed memory ordering.
            #[inline]
            pub fn fetch_min(&self, val: $primitive_type) -> $primitive_type {
                self.0.fetch_min(val, Ordering::Relaxed)
            }
        }
    };
}

impl_relaxed_atomic!(AtomicBool, bool);
impl_relaxed_atomic_numeric!(AtomicI8, i8);
impl_relaxed_atomic_numeric!(AtomicI16, i16);
impl_relaxed_atomic_numeric!(AtomicI32, i32);
impl_relaxed_atomic_numeric!(AtomicI64, i64);
impl_relaxed_atomic_numeric!(AtomicIsize, isize);
impl_relaxed_atomic_numeric!(AtomicU8, u8);
impl_relaxed_atomic_numeric!(AtomicU16, u16);
impl_relaxed_atomic_numeric!(AtomicU32, u32);
impl_relaxed_atomic_numeric!(AtomicU64, u64);
impl_relaxed_atomic_numeric!(AtomicUsize, usize);

#[cfg(ktest)]
/// Relaxed atomic unit tests.
#[unittest::suite(name = "relaxed_atomic_tests")]
mod tests {
    use super::{RelaxedAtomicBool, RelaxedAtomicI32, RelaxedAtomicU64};

    /// Tests boolean relaxed atomic operations.
    #[test]
    fn test_relaxed_atomic_bool() {
        let val = RelaxedAtomicBool::new(false);
        unittest::expect_false!(val.load());
        val.store(true);
        unittest::expect_true!(val.load());
        unittest::expect_true!(val.swap(false));
        unittest::expect_false!(val.load());
    }

    /// Tests numeric relaxed atomic operations.
    #[test]
    fn test_relaxed_atomic_numeric_ops() {
        let val = RelaxedAtomicI32::new(10);
        unittest::expect_eq!(val.load(), 10);
        unittest::expect_eq!(val.fetch_add(5), 10);
        unittest::expect_eq!(val.load(), 15);
        unittest::expect_eq!(val.fetch_sub(3), 15);
        unittest::expect_eq!(val.load(), 12);

        let uval = RelaxedAtomicU64::new(100);
        unittest::expect_true!(uval.compare_exchange(100, 200) == Ok(100));
        unittest::expect_eq!(uval.load(), 200);
    }
}
