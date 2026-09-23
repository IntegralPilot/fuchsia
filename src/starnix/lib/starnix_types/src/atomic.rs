// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Trait to map a numeric type to its atomic counterpart.
pub trait AsAtomic: Copy {
    type Atomic: AtomicOperations<Self>;
    const ONE: Self;
}

/// Trait for operations on atomics.
pub trait AtomicOperations<T> {
    fn new(value: T) -> Self;
    fn load(&self, order: Ordering) -> T;
    fn store(&self, value: T, order: Ordering);
    fn fetch_add(&self, value: T, order: Ordering) -> T;
}

macro_rules! impl_atomic {
    ($ty:ty, $atomic:ty) => {
        impl AsAtomic for $ty {
            type Atomic = $atomic;
            const ONE: $ty = 1;
        }

        impl AtomicOperations<$ty> for $atomic {
            fn new(value: $ty) -> Self {
                <$atomic>::new(value)
            }
            fn load(&self, order: Ordering) -> $ty {
                self.load(order)
            }
            fn store(&self, value: $ty, order: Ordering) {
                self.store(value, order)
            }
            fn fetch_add(&self, value: $ty, order: Ordering) -> $ty {
                self.fetch_add(value, order)
            }
        }
    };
}

impl_atomic!(i64, AtomicI64);
impl_atomic!(u64, AtomicU64);
impl_atomic!(u32, AtomicU32);
impl_atomic!(usize, AtomicUsize);
