// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

#[cfg(test)]
extern crate std;

mod blob_id_allocator;

pub use blob_id_allocator::{
    AllocateError, AllocateErrorWith, BlobError, BlobIdAllocator, Header, Index, Iter, ZeroFill,
};
