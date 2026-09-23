// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This is a simple Rust crate to hold some constants so that they can easily
//! be shared between various different FIDL bindings that are implemented in
//! Rust.
//!
//! It's in some ways the moral equivalent of //zircon/system/public/zircon/fidl.h

/// The maximum recursion depth of encoding and decoding. Each pointer to an
/// out-of-line object counts as one step in the recursion depth.
pub const MAX_RECURSION: usize = 32;

/// The maximum number of handles allowed in a FIDL message.
///
/// Note that this number is one less for large messages for the time being. See
/// (https://fxbug.dev/42068341) for progress, or to report problems caused by
/// this specific limitation.
pub const MAX_HANDLES: usize = 64;

/// Indicates that an optional value is present.
pub const ALLOC_PRESENT_U64: u64 = u64::MAX;
/// Indicates that an optional value is present.
pub const ALLOC_PRESENT_U32: u32 = u32::MAX;
/// Indicates that an optional value is absent.
pub const ALLOC_ABSENT_U64: u64 = 0;
/// Indicates that an optional value is absent.
pub const ALLOC_ABSENT_U32: u32 = 0;

/// Special ordinal signifying an epitaph message.
pub const EPITAPH_ORDINAL: u64 = 0xffffffffffffffffu64;

/// The current wire format magic number
pub const MAGIC_NUMBER_INITIAL: u8 = 1;
