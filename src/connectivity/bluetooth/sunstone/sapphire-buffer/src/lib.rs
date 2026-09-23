// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

#[cfg(feature = "alloc")]
pub mod allocated;
pub mod buffer;
pub mod multibuf;
pub mod storage;

#[cfg(feature = "alloc")]
pub use crate::allocated::StdVecBufferProvider;
pub use crate::buffer::{Buffer, BufferAccessor, Handle, OutOfBounds, UnboundHandle};
pub use crate::multibuf::MultiBuf;
pub use crate::storage::StorageBufferProvider;
