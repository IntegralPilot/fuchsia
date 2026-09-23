// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides decoding for FIDL types.

mod error;

pub use self::error::*;

use crate::{Constrained, Slot};

/// Decodes a value from the given slot.
///
/// # Safety
///
/// If `decode` returns `Ok`, then the provided `slot` will contain a valid decoded value after the
/// decoder is committed.
pub unsafe trait Decode<D: ?Sized>: Constrained {
    /// Decodes a value into a slot using a decoder.
    ///
    /// If decoding succeeds, `slot` will contain a valid decoded value after the decoder is
    /// committed. If decoding fails, an error will be returned.
    fn decode(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: Self::Constraint,
    ) -> Result<(), DecodeError>;
}

// SAFETY: If all `N` elements are successfully decoded, the entire array is decoded.
unsafe impl<D: ?Sized, T: Decode<D>, const N: usize> Decode<D> for [T; N] {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        for i in 0..N {
            T::decode(slot.index(i), decoder, constraint)?;
        }
        Ok(())
    }
}
