// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zx_status::Status;

/// Result of a `get` operation on a bitmap.
#[derive(Default)]
pub struct GetResult<T> {
    /// True if all bits in the range were set.
    pub all_set: bool,
    /// The index of the first unset bit in the range, or `bitmax` if all were set.
    pub first_unset: T,
}

/// An abstract bitmap.
pub trait Bitmap<T>
where
    T: Copy + core::ops::Add<Output = T> + From<u8>,
{
    /// Finds a run of `run_len` `is_set` bits, between `bitoff` and `bitmax`.
    ///
    /// Returns the start of the run, or `Status::NO_RESOURCES` if not found.
    fn find(&self, is_set: bool, bitoff: T, bitmax: T, run_len: T) -> Result<T, Status>;

    /// Returns true if all the bits in `[bitoff, bitmax)` are set.
    ///
    /// Also returns the index of the first unset bit in that range.
    fn get(&self, bitoff: T, bitmax: T) -> GetResult<T>;

    /// Sets all bits in the range `[bitoff, bitmax)`.
    ///
    /// Only fails on allocation error or if `bitmax < bitoff`.
    fn set(&mut self, bitoff: T, bitmax: T) -> Result<(), Status>;

    /// Clears all bits in the range `[bitoff, bitmax)`.
    ///
    /// Only fails on allocation error or if `bitmax < bitoff`.
    fn clear(&mut self, bitoff: T, bitmax: T) -> Result<(), Status>;

    /// Clear all bits in the bitmap.
    fn clear_all(&mut self);

    /// Returns true if the bit at `bitoff` is set.
    fn get_one(&self, bitoff: T) -> bool {
        self.get(bitoff, bitoff + T::from(1)).all_set
    }

    /// Sets the bit at `bitoff`.
    fn set_one(&mut self, bitoff: T) -> Result<(), Status> {
        self.set(bitoff, bitoff + T::from(1))
    }

    /// Clears the bit at `bitoff`.
    fn clear_one(&mut self, bitoff: T) -> Result<(), Status> {
        self.clear(bitoff, bitoff + T::from(1))
    }
}
