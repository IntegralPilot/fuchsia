// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library to allow storing enum discriminants out-of-line from payloads without maintaining
//! `unsafe` code. Makes structs smaller when they have enum fields whose discriminants are stored
//! with padding in the enum layout due to payload alignment requirements.
//!
//! An enum's internal padding can result in a lot of memory used even when the use case does not
//! require the ability to return a concrete reference to the enum type. This macro allows relaxing
//! the requirement at the expense of ergonomics, storing enum discriminants in bytes of a struct
//! that would otherwise be used by the compiler for padding. Callers then interact with the enum
//! "field" through generated accessors.

pub use split_enum_storage_macro::{SplitStorage, container};

/// A trait for enums which can be decomposed into a separate discriminant and data payload.
///
/// # Safety
///
/// Do not implement this manually. Use the derive macro provided by this crate.
pub unsafe trait SplitStorage {
    /// The data-less discriminant enum for this enum's split storage.
    type Discriminant;

    /// The payload union for this enum's split storage.
    type Payload;

    /// Split the enum's discriminant and payload for separate storage.
    fn decompose(self) -> (Self::Discriminant, std::mem::ManuallyDrop<Self::Payload>);
}
