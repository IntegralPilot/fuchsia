// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{BlockSize, BlockSizeSpec, GenericBlockSize, MaskShiftSpec};
use std::sync::atomic::{AtomicU64, Ordering};

/// Cached system page size in the MaskShiftSpec representation, initialized on first access.
static PAGE_MASK_SHIFT: AtomicU64 = AtomicU64::new(0);

/// Initializes `PAGE_MASK_SHIFT`.
#[cold]
#[inline(never)]
fn initialize_page_mask_shift() -> u64 {
    let page_size = zx::system_get_page_size();
    let repr = MaskShiftSpec::new(page_size as u64).0;
    PAGE_MASK_SHIFT.store(repr, Ordering::Relaxed);
    repr
}

/// Returns the system page size in the MaskShiftSpec representation, initializing it if not already
/// cached.
#[inline(always)]
fn page_mask_shift() -> u64 {
    let val = PAGE_MASK_SHIFT.load(Ordering::Relaxed);
    if val != 0 { val } else { initialize_page_mask_shift() }
}

/// [`BlockSizeSpec`] implementation representing the Fuchsia system memory page size.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PageSizeSpec;

impl BlockSizeSpec for PageSizeSpec {
    #[inline(always)]
    fn size(self) -> u64 {
        self.mask() + 1
    }

    #[inline(always)]
    fn mask(self) -> u64 {
        page_mask_shift() >> 32
    }

    #[inline(always)]
    fn shift(self) -> u32 {
        page_mask_shift() as u32
    }
}

/// The system memory page size.
pub const PAGE_SIZE: GenericBlockSize<PageSizeSpec> = GenericBlockSize(PageSizeSpec);

impl From<GenericBlockSize<PageSizeSpec>> for BlockSize {
    #[inline(always)]
    fn from(_value: GenericBlockSize<PageSizeSpec>) -> Self {
        GenericBlockSize(MaskShiftSpec(page_mask_shift()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_page_size() {
        let expected = zx::system_get_page_size() as u64;
        assert_eq!(PAGE_SIZE.get(), expected);
        assert_eq!(PAGE_SIZE.mask(), expected - 1);
        assert_eq!(PAGE_SIZE.shift(), (expected - 1).trailing_ones());
        assert_eq!(BlockSize::from(PAGE_SIZE).get(), expected);
        assert_eq!(BlockSize::from(PAGE_SIZE).mask(), expected - 1);
        assert_eq!(BlockSize::from(PAGE_SIZE).shift(), (expected - 1).trailing_ones());
        assert!(PAGE_SIZE.is_aligned(expected));
        assert!(!PAGE_SIZE.is_aligned(expected - 1));
    }
}
