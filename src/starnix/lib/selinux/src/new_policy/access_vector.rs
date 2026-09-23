// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{Debug, Formatter, LowerHex};
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not, Sub, SubAssign};
use std::str::FromStr;

use selinux_policy_derive::{Parse, Serialize, Validate};

use super::PermissionId;
use super::traits::PolicyId;

/// Set of permissions that may be granted to sources accessing targets of a particular class.
#[derive(
    Copy, Clone, Default, Eq, Hash, PartialEq, PartialOrd, Ord, Parse, Serialize, Validate,
)]
pub struct AccessVector(u32);

impl AccessVector {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(u32::MAX);

    pub fn from_class_permission_id(id: PermissionId) -> Self {
        Self::from(id)
    }

    /// Returns the raw access vector value.
    pub fn value(&self) -> u32 {
        self.0
    }
}

impl From<u32> for AccessVector {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<AccessVector> for u32 {
    fn from(value: AccessVector) -> Self {
        value.0
    }
}

impl Debug for AccessVector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "AccessVector({:0>8x})", self.0)
    }
}

impl LowerHex for AccessVector {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        LowerHex::fmt(&self.0, f)
    }
}

impl FromStr for AccessVector {
    type Err = <u32 as FromStr>::Err;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Access Vector values are always serialized to/from hexadecimal.
        Ok(Self(u32::from_str_radix(value, 16)?))
    }
}

impl BitAnd for AccessVector {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitOr for AccessVector {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitAndAssign for AccessVector {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0
    }
}

impl BitOrAssign for AccessVector {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0
    }
}

impl SubAssign for AccessVector {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0 ^ (self.0 & rhs.0);
    }
}

impl Sub for AccessVector {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 ^ (self.0 & rhs.0))
    }
}

impl Not for AccessVector {
    type Output = Self;

    fn not(self) -> Self {
        Self(!self.0)
    }
}

impl From<PermissionId> for AccessVector {
    fn from(id: PermissionId) -> Self {
        Self((1 as u32) << (id.as_u32() - 1))
    }
}
