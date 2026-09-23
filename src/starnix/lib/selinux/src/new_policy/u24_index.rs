// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::{Index, IndexMut};

use thiserror::Error;

use super::error::ParseError;

/// Compact 24-bit (3-byte) index into a policy array.
///
/// Reduces memory footprint compared to `u32`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct U24Index([u8; 3]);

impl U24Index {
    /// Maximum integer value supported by [`U24Index`].
    const MAX: usize = 0x00FF_FFFF;
}

impl From<U24Index> for usize {
    fn from(index: U24Index) -> Self {
        u32::from_le_bytes([index.0[0], index.0[1], index.0[2], 0]) as usize
    }
}

impl TryFrom<usize> for U24Index {
    type Error = U24IndexOverflow;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value <= Self::MAX {
            let bytes = (value as u32).to_le_bytes();
            Ok(Self([bytes[0], bytes[1], bytes[2]]))
        } else {
            Err(U24IndexOverflow(value))
        }
    }
}

impl<T> Index<U24Index> for [T] {
    type Output = T;

    fn index(&self, index: U24Index) -> &Self::Output {
        &self[usize::from(index)]
    }
}

impl<T> IndexMut<U24Index> for [T] {
    fn index_mut(&mut self, index: U24Index) -> &mut Self::Output {
        &mut self[usize::from(index)]
    }
}

/// Error returned when an integer index exceeds [`U24Index::MAX`].
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("index value {0} exceeds 24-bit maximum (16,777,215)")]
pub struct U24IndexOverflow(usize);

impl From<U24IndexOverflow> for ParseError {
    fn from(err: U24IndexOverflow) -> Self {
        ParseError::IndexOutOfRange { index: err.0, max: U24Index::MAX }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u24_index_layout_and_conversion() {
        assert_eq!(std::mem::size_of::<U24Index>(), 3);
        assert_eq!(std::mem::align_of::<U24Index>(), 1);

        let idx: U24Index = 1234567_usize.try_into().unwrap();
        assert_eq!(usize::from(idx), 1234567);

        let try_idx: Result<U24Index, _> = 16_777_215_usize.try_into();
        assert!(try_idx.is_ok());
        assert_eq!(usize::from(try_idx.unwrap()), 16_777_215);

        let overflow: Result<U24Index, _> = 16_777_216_usize.try_into();
        assert!(overflow.is_err());
    }
}
