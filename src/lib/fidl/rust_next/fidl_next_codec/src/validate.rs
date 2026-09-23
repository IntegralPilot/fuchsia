// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use crate::Slot;

/// Errors that can be produced when validating FIDL messages.
///
/// Validation errors may occur during both encoding and decoding.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum ValidationError {
    /// Vector too long.
    #[error("vector too long, has {count}, limit is {limit}")]
    VectorTooLong {
        /// The number of elements.
        count: u64,
        /// The maximum number of elements allowed.
        limit: u64,
    },
    /// String too long.
    #[error("string too long, has {count}, limit is {limit}")]
    StringTooLong {
        /// The number of bytes in the string.
        count: u64,
        /// The maximum number of bytes allowed.
        limit: u64,
    },
}

/// Implemented by types that have constraints that can be validated.
pub trait Constrained {
    /// Type of constraint information for this type.
    type Constraint: Copy;

    /// Validate a slot of this type against a constraint. Can be called when
    /// pointers/envelopes are just presence markers.
    fn validate(slot: Slot<'_, Self>, constraint: Self::Constraint) -> Result<(), ValidationError>;
}

// Arrays have the constraints of their member.
impl<T: Constrained, const N: usize> Constrained for [T; N] {
    type Constraint = T::Constraint;
    fn validate(
        mut slot: Slot<'_, Self>,
        constraint: Self::Constraint,
    ) -> Result<(), ValidationError> {
        // SAFETY: this slot must be initialized.
        let slice = unsafe { (slot.as_mut_ptr() as *mut [T]).as_mut() }.unwrap();
        for member in slice {
            // SAFETY: every member of the array must have been initialized already.
            let member_slot = unsafe { Slot::new_unchecked(member) };
            T::validate(member_slot, constraint)?;
        }
        Ok(())
    }
}
