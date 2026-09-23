// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bitmap::{Bitmap, GetResult};
use crate::storage::Storage;
use zx_status::Status;

const BITS_PER_WORD: usize = usize::BITS as usize;

const fn last_idx(bitmax: usize) -> usize {
    (bitmax - 1) / BITS_PER_WORD
}

const fn first_idx(bitoff: usize) -> usize {
    bitoff / BITS_PER_WORD
}

fn get_mask(first: bool, last: bool, off: usize, max: usize) -> usize {
    let ones = !0usize;
    let mut mask = ones;
    if first {
        mask &= ones << (off % BITS_PER_WORD);
    }
    if last {
        mask &= ones >> ((BITS_PER_WORD - (max % BITS_PER_WORD)) % BITS_PER_WORD);
    }
    mask
}

fn mask_bits(data: usize, idx: usize, bitoff: usize, bitmax: usize, is_set: bool) -> usize {
    let mask = get_mask(idx == first_idx(bitoff), idx == last_idx(bitmax), bitoff, bitmax);
    if is_set { !(!mask | data) } else { mask & data }
}

/// A simple bitmap backed by generic storage.
#[derive(Debug, Default)]
pub struct RawBitmapGeneric<S: Storage> {
    bits: S,
    size: usize,
}

impl<S: Storage> RawBitmapGeneric<S> {
    /// Create a new raw bitmap with the given storage.
    pub const fn new(bits: S) -> Self {
        Self { bits, size: 0 }
    }

    /// Returns the size of this bitmap.
    pub fn size(&self) -> usize {
        self.size
    }

    fn data(&self) -> &[usize] {
        self.bits.get_data()
    }

    fn data_mut(&mut self) -> &mut [usize] {
        self.bits.get_data_mut()
    }

    /// Access the underlying storage read-only.
    pub fn storage(&self) -> &S {
        &self.bits
    }

    /// Access the underlying storage mutably.
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.bits
    }

    /// Shrinks the accessible portion of the bitmap, without re-allocating
    /// the underlying storage.
    ///
    /// This is useful for programs which require underlying bitmap storage
    /// to be aligned to a certain size (initialized via Reset), but want to
    /// restrict access to a smaller portion of the bitmap (via Shrink).
    pub fn shrink(&mut self, size: usize) -> Result<(), Status> {
        if size > self.size {
            return Err(Status::NO_MEMORY);
        }
        self.size = size;
        Ok(())
    }

    /// Returns true if all bits in the range `[bitoff, bitmax)` match `is_set`.
    ///
    /// If they do not match, returns false and the index of the first bit that doesn't match.
    /// An empty region (i.e. `bitoff >= bitmax`) will return true.
    pub fn scan(&self, bitoff: usize, mut bitmax: usize, is_set: bool) -> Option<usize> {
        bitmax = core::cmp::min(bitmax, self.size);
        if bitoff >= bitmax {
            return None;
        }
        let data = self.data();
        let mut i = first_idx(bitoff);
        let last = last_idx(bitmax);
        loop {
            let word = data[i];
            let masked = mask_bits(word, i, bitoff, bitmax, is_set);
            if masked != 0 {
                let first_match = i * BITS_PER_WORD + (masked.trailing_zeros() as usize);
                return Some(first_match);
            }
            if i == last {
                return None;
            }
            i += 1;
        }
    }

    /// Returns true if all bits in the range `[bitoff, bitmax)` match `is_set`, scanning in reverse.
    ///
    /// If they do not match, returns false and the index of the last bit that doesn't match.
    /// An empty region (i.e. `bitoff >= bitmax`) will return true.
    pub fn reverse_scan(&self, bitoff: usize, mut bitmax: usize, is_set: bool) -> Option<usize> {
        bitmax = core::cmp::min(bitmax, self.size);
        if bitoff >= bitmax {
            return None;
        }
        let data = self.data();
        let mut i = last_idx(bitmax);
        let first = first_idx(bitoff);
        loop {
            let word = data[i];
            let masked = mask_bits(word, i, bitoff, bitmax, is_set);
            if masked != 0 {
                let last_match = (i + 1) * BITS_PER_WORD - ((masked.leading_zeros() as usize) + 1);
                return Some(last_match);
            }
            if i == first {
                return None;
            }
            i -= 1;
        }
    }

    /// Finds the last run of `run_len` `is_set` bits, in `[bitoff, bitmax)`.
    ///
    /// Returns the start of the run, or `Status::NO_RESOURCES` if not found.
    pub fn reverse_find(
        &self,
        is_set: bool,
        bitoff: usize,
        bitmax: usize,
        run_len: usize,
    ) -> Result<usize, Status> {
        if bitmax <= bitoff {
            return Err(Status::INVALID_ARGS);
        }
        let mut scan_max = bitmax;
        loop {
            let first_mismatch = self.reverse_scan(bitoff, scan_max, !is_set);
            let start = match first_mismatch {
                Some(idx) => idx + 1,
                None => return Err(Status::NO_RESOURCES),
            };
            if start - bitoff < run_len {
                return Err(Status::NO_RESOURCES);
            }
            let first_diff = self.reverse_scan(start - run_len, start, is_set);
            match first_diff {
                None => return Ok(start - run_len),
                Some(idx) => {
                    scan_max = idx;
                }
            }
        }
    }

    /// Increases the bitmap size.
    pub fn grow(&mut self, size: usize) -> Result<(), Status> {
        if !S::SUPPORTS_GROW {
            return Err(Status::NO_RESOURCES);
        }
        if size < self.size {
            return Err(Status::INVALID_ARGS);
        } else if size == self.size {
            return Ok(());
        }
        let old_len = if self.size == 0 { 0 } else { last_idx(self.size) + 1 };
        let new_len = last_idx(size) + 1;
        let new_bitsize = core::mem::size_of::<usize>() * new_len;
        self.bits.grow(new_bitsize)?;
        let data = self.data_mut();
        for i in old_len..new_len {
            data[i] = 0;
        }
        let old_size = self.size;
        self.size = size;
        let clear_limit = core::cmp::min(old_len * BITS_PER_WORD, self.size);
        if old_size < clear_limit {
            self.clear(old_size, clear_limit)?;
        }
        Ok(())
    }

    /// Resets the bitmap; clearing and resizing it.
    ///
    /// Allocates memory, and can fail.
    pub fn reset(&mut self, size: usize) -> Result<(), Status> {
        self.size = size;
        if size == 0 {
            return Ok(());
        }
        let last = last_idx(size);
        self.bits.allocate(core::mem::size_of::<usize>() * (last + 1))?;
        self.clear_all();
        Ok(())
    }
}

impl<S: Storage> Bitmap<usize> for RawBitmapGeneric<S> {
    fn find(
        &self,
        is_set: bool,
        bitoff: usize,
        bitmax: usize,
        run_len: usize,
    ) -> Result<usize, Status> {
        if bitmax <= bitoff {
            return Err(Status::INVALID_ARGS);
        }
        let mut start;
        let mut scan_off = bitoff;
        loop {
            let first_mismatch = self.scan(scan_off, bitmax, !is_set);
            start = match first_mismatch {
                Some(idx) => idx,
                None => return Err(Status::NO_RESOURCES),
            };
            if bitmax - start < run_len {
                return Err(Status::NO_RESOURCES);
            }
            let first_diff = self.scan(start, start + run_len, is_set);
            match first_diff {
                None => return Ok(start),
                Some(idx) => {
                    scan_off = idx;
                }
            }
        }
    }

    fn get(&self, bitoff: usize, bitmax: usize) -> GetResult<usize> {
        let first_unset = self.scan(bitoff, bitmax, true);
        GetResult { all_set: first_unset.is_none(), first_unset: first_unset.unwrap_or(bitmax) }
    }

    fn set(&mut self, bitoff: usize, bitmax: usize) -> Result<(), Status> {
        if bitoff > bitmax || bitmax > self.size {
            return Err(Status::INVALID_ARGS);
        }
        if bitoff == bitmax {
            return Ok(());
        }
        let first = first_idx(bitoff);
        let last = last_idx(bitmax);
        let data = self.data_mut();
        for i in first..=last {
            let mask = get_mask(i == first, i == last, bitoff, bitmax);
            data[i] |= mask;
        }
        Ok(())
    }

    fn clear(&mut self, bitoff: usize, bitmax: usize) -> Result<(), Status> {
        if bitoff > bitmax || bitmax > self.size {
            return Err(Status::INVALID_ARGS);
        }
        if bitoff == bitmax {
            return Ok(());
        }
        let first = first_idx(bitoff);
        let last = last_idx(bitmax);
        let data = self.data_mut();
        for i in first..=last {
            let mask = get_mask(i == first, i == last, bitoff, bitmax);
            data[i] &= !mask;
        }
        Ok(())
    }

    fn clear_all(&mut self) {
        if self.size == 0 {
            return;
        }
        let last = last_idx(self.size);
        let data = self.data_mut();
        for i in 0..=last {
            data[i] = 0;
        }
    }
}
