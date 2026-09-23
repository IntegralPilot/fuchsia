// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Represents the result (value) for a Link Metrics query.
#[derive(Default, Debug, Clone, Copy)]
#[repr(transparent)]
pub struct LinkMetricsValues(pub otLinkMetricsValues);

impl_ot_castable!(LinkMetricsValues, otLinkMetricsValues);

impl LinkMetricsValues {
    /// Get the link margin value
    pub fn link_margin(&self) -> u8 {
        self.0.mLinkMarginValue
    }

    /// Get the Rssi value
    pub fn rssi(&self) -> i8 {
        self.0.mRssiValue
    }
}
