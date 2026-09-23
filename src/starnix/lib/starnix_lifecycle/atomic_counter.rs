// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helper class to implement a counter that can be shared across threads.

use starnix_types::atomic::{AsAtomic, AtomicOperations};
use std::sync::atomic::Ordering;

/// A generic atomic counter.
#[derive(Debug)]
pub struct AtomicCounter<T: AsAtomic>(T::Atomic);

impl<T: AsAtomic> AtomicCounter<T> {
    pub fn new(value: T) -> Self {
        Self(T::Atomic::new(value))
    }

    pub fn next(&self) -> T {
        self.add(T::ONE)
    }

    pub fn add(&self, amount: T) -> T {
        self.0.fetch_add(amount, Ordering::Relaxed)
    }

    pub fn get(&self) -> T {
        self.0.load(Ordering::Relaxed)
    }

    pub fn reset(&mut self, value: T) {
        self.0.store(value, Ordering::Relaxed);
    }
}

impl AtomicCounter<u32> {
    pub const fn new_const(value: u32) -> Self {
        Self(std::sync::atomic::AtomicU32::new(value))
    }
}

impl AtomicCounter<usize> {
    pub const fn new_const(value: usize) -> Self {
        Self(std::sync::atomic::AtomicUsize::new(value))
    }
}

impl<T: AsAtomic> Default for AtomicCounter<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: AsAtomic> From<T> for AtomicCounter<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[::fuchsia::test]
    fn test_new() {
        let counter: AtomicCounter<u64> = AtomicCounter::<u64>::new(0);
        assert_eq!(counter.get(), 0);
    }

    #[::fuchsia::test]
    fn test_one_thread() {
        let mut counter = AtomicCounter::<u64>::default();
        assert_eq!(counter.get(), 0);
        assert_eq!(counter.add(5), 0);
        assert_eq!(counter.get(), 5);
        assert_eq!(counter.next(), 5);
        assert_eq!(counter.get(), 6);
        counter.reset(2);
        assert_eq!(counter.get(), 2);
        assert_eq!(counter.next(), 2);
        assert_eq!(counter.get(), 3);
    }

    #[::fuchsia::test]
    fn test_multiple_thread() {
        const THREADS_COUNT: u64 = 10;
        const INC_ITERATIONS: u64 = 1000;
        let mut thread_handles = Vec::new();
        let counter = Arc::new(AtomicCounter::<u64>::default());

        for _ in 0..THREADS_COUNT {
            thread_handles.push(std::thread::spawn({
                let counter = Arc::clone(&counter);
                move || {
                    for _ in 0..INC_ITERATIONS {
                        counter.next();
                    }
                }
            }));
        }
        for handle in thread_handles {
            handle.join().expect("join");
        }
        assert_eq!(THREADS_COUNT * INC_ITERATIONS, counter.get());
    }
}
