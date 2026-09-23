// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for testing code written against the register access traits of
//! [`traits`](crate::traits).
//!
//! This module is only available with the `testing` crate feature.

pub mod x86;

use core::cell::Cell;
use core::fmt::Debug;
use std::vec::Vec;

use crate::traits::{ReadReg, SafeWriteReg, UnsafeWriteReg};

// A closure wrapper that implements [`ReadReg`], convenient for testing
// interfaces that deal in register reading.
pub struct FnReadReg<Layout, F: Fn() -> Layout>(pub F);

impl<Layout, F> ReadReg<Layout> for FnReadReg<Layout, F>
where
    F: Fn() -> Layout,
{
    fn read(&self) -> Layout {
        self.0()
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ExpectedAccess {
    Read,
    Write,
}

struct Expected<Layout> {
    access: ExpectedAccess,
    value: Layout,
}

/// A mock register that can be used in place of [`Register`][crate::Register]
/// for testing at sites that are generic over [`Reg`] and
/// related abstracted access traits.
///
/// It must be primed with the sequence of expected reads and writes, and will
/// panic if the actual accesses do not end up matching the expected sequence.
///
/// If fewer accesses are performed than expected, the [`ExpectationReg`] will
/// panic on drop.
pub struct ExpectationReg<Layout> {
    accesses: Vec<Expected<Layout>>,
    idx: Cell<usize>,
    mid_access: Cell<bool>,
}

impl<Layout> ExpectationReg<Layout> {
    #[must_use]
    pub fn new() -> Self {
        Self { accesses: Vec::new(), idx: Cell::new(0), mid_access: Cell::new(false) }
    }

    /// Records an expected read with the value to be returned.
    pub fn expect_read(&mut self, value: Layout) -> &mut Self {
        self.accesses.push(Expected { access: ExpectedAccess::Read, value });
        self
    }

    /// Records an expected write with the value to be written.
    pub fn expect_write(&mut self, value: Layout) -> &mut Self {
        self.accesses.push(Expected { access: ExpectedAccess::Write, value });
        self
    }

    // Returns the next expected access and its index, incrementing the index
    // for the next call.
    fn next_expectation(&self) -> (&Expected<Layout>, usize) {
        let idx = self.idx.get();
        let len = self.accesses.len();
        assert!(idx < len, "Number of expected accesses ({len}) exceeded");
        self.idx.set(idx + 1);
        (&self.accesses[idx], idx)
    }
}

impl<Layout> Drop for ExpectationReg<Layout> {
    fn drop(&mut self) {
        // If we are mid-access then we are panicking and should not make
        // assertions about the final number of accesses that were performed.
        if self.mid_access.get() {
            return;
        }
        let idx = self.idx.get();
        let len = self.accesses.len();
        assert_eq!(idx, len, "{len} accesses were expected; only {idx} were performed",);
    }
}

impl<Layout> ReadReg<Layout> for ExpectationReg<Layout>
where
    Layout: Copy,
{
    fn read(&self) -> Layout {
        self.mid_access.set(true);
        let (expected, idx) = self.next_expectation();
        assert_eq!(expected.access, ExpectedAccess::Read, "Expected a read for access #{idx}");
        self.mid_access.set(false);
        expected.value
    }
}

impl<Layout> SafeWriteReg<Layout> for ExpectationReg<Layout>
where
    Layout: Copy + Debug + PartialEq,
{
    fn write(&self, value: Layout) {
        self.mid_access.set(true);
        let (expected, idx) = self.next_expectation();
        assert_eq!(expected.access, ExpectedAccess::Write, "Expected a write for access #{idx}");
        assert_eq!(
            expected.value, value,
            "Expected a write of {:#?}; got {value:#?}",
            expected.value
        );
        self.mid_access.set(false);
    }
}

impl<Layout> UnsafeWriteReg<Layout> for ExpectationReg<Layout>
where
    Layout: Copy + Debug + PartialEq,
{
    unsafe fn write(&self, value: Layout) {
        <Self as SafeWriteReg<Layout>>::write(self, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::RwSafeReg;

    fn increment<R: RwSafeReg<u32>>(reg: R) {
        reg.modify(|value| *value += 1);
    }

    #[test]
    fn closures_as_read_regs() {
        let reg = FnReadReg(|| 10u32);
        assert_eq!(reg.read(), 10);
    }

    #[test]
    fn expectation_reg() {
        let mut reg = ExpectationReg::new();
        reg.expect_read(10u32).expect_write(11).expect_read(11).expect_write(12);
        increment(&reg);
        increment(&reg);
    }

    #[test]
    #[should_panic(expected = "Expected a write for access #1")]
    fn expectation_reg_access_mismatch() {
        let mut reg = ExpectationReg::new();
        reg.expect_read(10u32).expect_read(11).expect_read(12);

        // The mid-sequence panic below must not be compounded by the
        // exhaustion assertion when `reg` drops during the unwind, which
        // would abort the test process.
        increment(&reg);
    }

    #[test]
    #[should_panic(expected = "1 accesses were expected; only 0 were performed")]
    fn expectation_reg_unmet_expectations() {
        let mut reg = ExpectationReg::new();
        reg.expect_read(10u32);
    }
}
