// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon cuonter objects.

use crate::{NullableHandle, Status, ok, sys};

/// An object representing a Zircon
/// [counter](https://fuchsia.dev/fuchsia-src/concepts/objects/counter.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Counter(NullableHandle);
impl_handle_based!(Counter);

impl Counter {
    /// Create a counter object.
    ///
    /// This object contains an integer which can be read, written, incremented and
    /// decremented. Readers can wait on [zx::Signals::COUNTER_POSITIVE] and
    /// [zx::Signals::COUNTER_NON_POSITIVE] to react on the respective value changes.
    ///
    /// See:
    /// [zx_counter_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/counter_create.md)
    /// syscall.
    ///
    /// # Panics
    ///
    /// If the kernel reports no memory available or the process' job policy denies counter creation.
    pub fn create() -> Counter {
        let options = 0;
        let mut handle = 0;
        let status = unsafe { sys::zx_counter_create(options, &mut handle) };
        ok(status).expect(
            "counter creation always succeeds except with OOM or when job policy denies it",
        );
        unsafe { Counter::from(NullableHandle::from_raw(handle)) }
    }

    /// Adds `value` to this counter.
    pub fn add(&self, value: i64) -> Result<(), Status> {
        let status = unsafe { sys::zx_counter_add(self.raw_handle(), value) };
        ok(status)
    }

    /// Reads the value of this counter.
    pub fn read(&self) -> Result<i64, Status> {
        let mut value = 0;
        let status = unsafe { sys::zx_counter_read(self.raw_handle(), &mut value) };
        ok(status).map(|()| value)
    }

    /// Sets the value of this counter to `value`.
    pub fn write(&self, value: i64) -> Result<(), Status> {
        let status = unsafe { sys::zx_counter_write(self.raw_handle(), value) };
        ok(status)
    }
}

// These tests are intended to verify the Rust bindings rather the counter
// Zircon object.  The tests for the Zircon object are part of the core-tests
// suite.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_create() {
        let counter = Counter::create();
        assert_eq!(counter.read().unwrap(), 0);
    }

    #[test]
    fn counter_add() {
        let counter = Counter::create();
        assert_eq!(counter.read().unwrap(), 0);
        assert!(counter.add(i64::MAX).is_ok());
        assert_eq!(counter.read().unwrap(), i64::MAX);
        assert_eq!(counter.add(1), Err(Status::OUT_OF_RANGE));
    }

    #[test]
    fn counter_read_write() {
        let counter = Counter::create();
        assert_eq!(counter.read().unwrap(), 0);
        assert!(counter.write(i64::MIN).is_ok());
        assert_eq!(counter.read().unwrap(), i64::MIN);
    }
}
