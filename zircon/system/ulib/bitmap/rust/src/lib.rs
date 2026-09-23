// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

use zr as _;

pub mod bitmap;
pub mod raw_bitmap;
pub mod rle_bitmap;
pub mod storage;

pub use bitmap::{Bitmap, GetResult};
pub use raw_bitmap::RawBitmapGeneric;
pub use rle_bitmap::{Element, FreeList, RleBitmap, RleBitmapBase, RleBitmapElement};
pub use storage::{DefaultStorage, FixedStorage, Storage};

#[cfg(all(not(is_kernel), target_os = "fuchsia"))]
pub use storage::VmoStorage;

#[cfg(test)]
mod tests;
