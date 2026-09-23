// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Represents the steering data.
/// Functional equivalent of [`otsys::otSteeringData`](crate::otsys::otSteeringData).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct SteeringData(pub otSteeringData);

impl_ot_castable!(SteeringData, otSteeringData);

impl SteeringData {
    /// Returns the length of steering data (bytes).
    pub fn length(&self) -> u8 {
        self.0.mLength
    }

    /// Returns the byte values of the steering data.
    pub fn into_array(&self) -> [u8; 16] {
        self.0.m8
    }

    /// Returns the Steering Data as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        &self.0.m8
    }

    /// Creates a `Vec<u8>` from this Steering Data.
    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }
}
