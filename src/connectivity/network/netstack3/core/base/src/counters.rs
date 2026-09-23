// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common counter abstractions.

use core::fmt::Debug;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::test_only::TestOnlyPartialEq;

/// An atomic counter for packet statistics, e.g. IPv4 packets received.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Increments the counter value by 1.
    pub fn increment(&self) {
        // Use relaxed ordering since we do not use packet counter values to
        // synchronize other accesses.  See:
        // https://doc.rust-lang.org/nomicon/atomics.html#relaxed
        let Self(v) = self;
        let _: u64 = v.fetch_add(1, Ordering::Relaxed);
    }

    /// Adds the provided value to the counter.
    pub fn add(&self, n: u64) {
        // Use relaxed ordering since we do not use packet counter values to
        // synchronize other accesses.  See:
        // https://doc.rust-lang.org/nomicon/atomics.html#relaxed
        let Self(v) = self;
        let _: u64 = v.fetch_add(n, Ordering::Relaxed);
    }

    /// Atomically retrieves the counter value as a `u64`.
    pub fn get(&self) -> u64 {
        // Use relaxed ordering since we do not use packet counter values to
        // synchronize other accesses.  See:
        // https://doc.rust-lang.org/nomicon/atomics.html#relaxed
        let Self(v) = self;
        v.load(Ordering::Relaxed)
    }
}

/// A context that stores counters.
///
/// `CounterContext` exposes access to counters for observation and debugging.
pub trait CounterContext<T> {
    /// Returns a reference to the counters.
    fn counters(&self) -> &T;
}

/// A context that provides access to per-resource counters for observation and
/// debugging.
pub trait ResourceCounterContext<R, T>: CounterContext<T> {
    /// Returns a reference to the set of counters on `resource`.
    fn per_resource_counters<'a>(&'a self, resource: &'a R) -> &'a T;

    /// Increments both the per-resource and stackwide versions of
    /// the counter returned by the callback.
    fn increment_both<F: Fn(&T) -> &Counter>(&self, resource: &R, cb: F) {
        cb(self.per_resource_counters(resource)).increment();
        cb(self.counters()).increment();
    }

    /// Adds `value` to both the per-resource and stackwide versions of
    /// the counter returned by the callback.
    fn add_both<F: Fn(&T) -> &Counter>(&self, resource: &R, value: u64, cb: F) {
        cb(self.per_resource_counters(resource)).add(value);
        cb(self.counters()).add(value);
    }

    /// Like `add_both` but takes a `usize` value, saturating to `u64::max` if
    /// conversion fails.
    fn add_both_usize<F: Fn(&T) -> &Counter>(&self, resource: &R, value: usize, cb: F) {
        self.add_both(resource, value.try_into().unwrap_or(u64::MAX), cb);
    }
}

mod sealed {
    use super::*;

    /// Used to implement the "sealed trait" pattern.
    pub trait Sealed {}

    impl Sealed for u64 {}
    impl Sealed for Counter {}
}

/// A marker trait to indicate types that may be used as a counter.
pub trait CounterRepr: sealed::Sealed + Default + Debug + TestOnlyPartialEq {
    /// Get the held counter value.
    fn get(&self) -> u64;
    /// Construct a new counter from the value.
    fn new(value: u64) -> Self;

    /// Convert one `CounterRepr` into another `CounterRepr`.
    fn cast<C: CounterRepr>(&self) -> C {
        C::new(self.get())
    }
}

impl CounterRepr for Counter {
    fn get(&self) -> u64 {
        self.get()
    }

    fn new(value: u64) -> Self {
        Counter(AtomicU64::new(value))
    }
}

// Only allow `u64` as a counter in tests.
#[cfg(any(test, feature = "testutils"))]
impl CounterRepr for u64 {
    fn get(&self) -> u64 {
        *self
    }

    fn new(value: u64) -> Self {
        value
    }
}

/// The specification of a [`CounterCollection`].
pub trait CounterCollectionSpec {
    /// The underlying [`CounterCollection`] for this specification.
    type CounterCollection<C: CounterRepr>: CounterCollection<Spec = Self>;
    /// A utility method to transform the underlying counter representation
    /// within [`Self::CounterCollection`].
    fn transform<C1: CounterRepr, C2: CounterRepr>(
        counters: &Self::CounterCollection<C1>,
    ) -> Self::CounterCollection<C2>;
}

/// A collection of counters.
pub trait CounterCollection: Debug + Default + TestOnlyPartialEq {
    /// The [`CounterCollectionSpec`] associated with this collection.
    type Spec: CounterCollectionSpec<CounterCollection<Self::Repr> = Self>;
    /// The counter representation held by this collection.
    type Repr: CounterRepr;
    /// A utility method to cast this collection of counters into a different
    /// underlying counter representation.
    fn cast<C: CounterRepr>(&self) -> <Self::Spec as CounterCollectionSpec>::CounterCollection<C> {
        <Self::Spec as CounterCollectionSpec>::transform::<Self::Repr, C>(self)
    }
}
