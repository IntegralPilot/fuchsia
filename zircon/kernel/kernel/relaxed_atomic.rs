// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::sync::atomic::{
    AtomicI8, AtomicI16, AtomicI32, AtomicI64, AtomicIsize, AtomicU8, AtomicU16, AtomicU32,
    AtomicU64, AtomicUsize,
};

use paste::paste;

/// Wrapper around atomic types that assumes `Ordering::Relaxed` for all
/// operations to simplify pure relaxed use cases.
#[repr(transparent)]
#[derive(Debug, Default)]
pub struct RelaxedAtomic<T> {
    wrapped: T,
}

// Use a macro to stamp out a bunch of implementations until generic_atomic is stabilized.
// https://github.com/rust-lang/rust/issues/130539.
macro_rules! impl_relaxed_atomic {
    ($atomic_type:ident, $prim_type:ty) => {
        impl RelaxedAtomic<$atomic_type> {
            /// Creates a new `RelaxedAtomic`.
            #[inline]
            pub const fn new(value: $prim_type) -> Self {
                Self { wrapped: $atomic_type::new(value) }
            }

            /// Loads the value with relaxed ordering.
            #[inline]
            pub fn load(&self) -> $prim_type {
                self.wrapped.load(::core::sync::atomic::Ordering::Relaxed)
            }

            /// Stores the value with relaxed ordering.
            #[inline]
            pub fn store(&self, desired: $prim_type) {
                self.wrapped.store(desired, ::core::sync::atomic::Ordering::Relaxed)
            }

            /// Adds to the current value, returning the previous value, with relaxed ordering.
            #[inline]
            pub fn fetch_add(&self, value: $prim_type) -> $prim_type {
                self.wrapped.fetch_add(value, ::core::sync::atomic::Ordering::Relaxed)
            }

            /// Subtracts from the current value, returning the previous value, with relaxed
            /// ordering.
            #[inline]
            pub fn fetch_sub(&self, value: $prim_type) -> $prim_type {
                self.wrapped.fetch_sub(value, ::core::sync::atomic::Ordering::Relaxed)
            }

            /// Bitwise "and" with the current value, returning the previous value, with relaxed
            /// ordering.
            #[inline]
            pub fn fetch_and(&self, value: $prim_type) -> $prim_type {
                self.wrapped.fetch_and(value, ::core::sync::atomic::Ordering::Relaxed)
            }

            /// Bitwise "or" with the current value, returning the previous value, with relaxed
            /// ordering.
            #[inline]
            pub fn fetch_or(&self, value: $prim_type) -> $prim_type {
                self.wrapped.fetch_or(value, ::core::sync::atomic::Ordering::Relaxed)
            }
        }
        paste! {
            pub type [<Relaxed $atomic_type>] = RelaxedAtomic<$atomic_type>;
        }
    };
}

impl_relaxed_atomic!(AtomicI8, i8);
impl_relaxed_atomic!(AtomicI16, i16);
impl_relaxed_atomic!(AtomicI32, i32);
impl_relaxed_atomic!(AtomicI64, i64);
impl_relaxed_atomic!(AtomicIsize, isize);
impl_relaxed_atomic!(AtomicU8, u8);
impl_relaxed_atomic!(AtomicU16, u16);
impl_relaxed_atomic!(AtomicU32, u32);
impl_relaxed_atomic!(AtomicU64, u64);
impl_relaxed_atomic!(AtomicUsize, usize);
