// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

mod defer;
mod lossy_utf8;
mod opaque;
mod opaque_bytes;
mod pin_init;
mod ptr;
mod static_assert;
mod string;

pub use defer::{Deferred, defer};
pub use lossy_utf8::from_utf8_lossy;
pub use opaque::{Opaque, OpaqueFacade};
pub use opaque_bytes::OpaqueBytes;
pub use ptr::{AtomicConstPtr, ToMutPtr, slice_from_raw_parts, slice_from_raw_parts_mut};
pub use string::{parse_usize, to_array};
