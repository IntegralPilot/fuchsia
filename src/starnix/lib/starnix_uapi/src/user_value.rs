// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Range;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// A value Starnix has received from userspace.
///
/// Typically, these values are received in syscall arguments and need to be validated before they
/// can be used directly. For example, integers need to be checked for overflow during arithmetical
/// operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, IntoBytes, KnownLayout, FromBytes, Immutable)]
#[repr(transparent)]
pub struct UserValue<T>(T);

impl<T: Copy + Eq + IntoBytes + FromBytes + Immutable> UserValue<T> {
    /// Create a UserValue from a raw value provided by userspace.
    pub fn from_raw(raw: T) -> Self {
        Self(raw)
    }

    /// The raw value that the user provided.
    pub fn raw(&self) -> T {
        self.0
    }

    /// Attempt to convert this value into another type.
    pub fn try_into<U: TryFrom<T>>(self) -> Result<U, <U as TryFrom<T>>::Error> {
        U::try_from(self.0)
    }
}

impl<T: Copy + PartialOrd> UserValue<T> {
    /// Returns the value that the user provided if the value is in the given range.
    pub fn validate(&self, range: Range<T>) -> Option<T> {
        if range.contains(&self.0) { Some(self.0) } else { None }
    }
}

/// Implements TryFrom for UserValue of specific types
macro_rules! impl_try_from_user_value {
    {
        $(
            $source:ty => $target:ty
        ),+
        $(,)?
    } => {
        $(
            impl TryFrom<$source> for UserValue<$target> {
                type Error = ();

                fn try_from(u: $source) -> Result<Self, Self::Error> {
                    let t = u.try_into().map_err(|_| ())?;
                    Ok(Self::from_raw(t))
                }
            }
        )*
    }
}

impl_try_from_user_value! {
    u32 => usize,
    u64 => usize,
    usize => u32,
    usize => u64,
}

impl<T: Copy + Eq + IntoBytes + FromBytes + Immutable> From<T> for UserValue<T> {
    fn from(value: T) -> Self {
        Self::from_raw(value)
    }
}

impl<T: PartialEq<T> + Copy + Eq + IntoBytes + FromBytes + Immutable> PartialEq<T>
    for UserValue<T>
{
    fn eq(&self, other: &T) -> bool {
        self.0 == *other
    }
}
