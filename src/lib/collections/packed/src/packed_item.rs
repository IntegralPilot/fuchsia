// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines the `PackedItem` trait for types that can be tightly packed in memory.

/// A trait for types that can be packed into a single contiguous buffer of bytes.
pub trait PackedItem: zerocopy::IntoBytes + zerocopy::Immutable + zerocopy::Unaligned {
    /// Reconstructs the item from a byte slice.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the slice contains data that is byte-identical
    /// to a slice returned by `IntoBytes::as_bytes` for a valid instance of this type.
    unsafe fn from_bytes(bytes: &[u8]) -> &Self;
}

impl PackedItem for [u8] {
    unsafe fn from_bytes(bytes: &[u8]) -> &Self {
        bytes
    }
}

impl PackedItem for str {
    unsafe fn from_bytes(bytes: &[u8]) -> &Self {
        // SAFETY: The documented safety constraint for `from_bytes` requires
        // the caller to guarantee these bytes are identical to the output from
        // `<str as zerocopy::IntoBytes>::as_bytes`, which contains valid UTF-8.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}
