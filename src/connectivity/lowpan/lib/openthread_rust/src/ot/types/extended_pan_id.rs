// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Data type representing an extended PAN-ID.
/// Functional equivalent of [`otsys::otExtendedPanId`](crate::otsys::otExtendedPanId).
#[derive(Default, Copy, Clone)]
#[repr(transparent)]
pub struct ExtendedPanId(pub otExtendedPanId);

impl_ot_castable!(ExtendedPanId, otExtendedPanId);

impl std::fmt::Debug for ExtendedPanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ExtendedPanId({})", hex::encode(self.as_slice()))
    }
}

impl std::fmt::Display for ExtendedPanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.as_slice()))
    }
}

impl std::convert::From<[u8; 8]> for ExtendedPanId {
    fn from(value: [u8; 8]) -> Self {
        Self(otExtendedPanId { m8: value })
    }
}

impl ExtendedPanId {
    /// Returns the underlying address octets.
    pub fn into_array(self) -> [u8; 8] {
        self.0.m8
    }

    /// Returns this Extended PAN-ID as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.0.m8
    }

    /// Creates a `Vec<u8>` from this Extended PAN-ID.
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}
