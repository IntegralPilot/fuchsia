// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents multi radio link information associated with a neighbor.
///
/// Functional equivalent of [`otsys::otMultiRadioNeighborInfo`](crate::otMultiRadioNeighborInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct MultiRadioNeighborInfo(pub otMultiRadioNeighborInfo);

impl_ot_castable!(MultiRadioNeighborInfo, otMultiRadioNeighborInfo);

impl MultiRadioNeighborInfo {
    /// Whether the neighbor supports IEEE 802.15.4 radio link.
    pub fn is_ieee_802_15_4_supported(&self) -> bool {
        self.0.mSupportsIeee802154()
    }

    /// Whether the neighbor supports Thread Radio Encapsulation Link (TREL) radio link.
    pub fn is_trel_supported(&self) -> bool {
        self.0.mSupportsTrelUdp6()
    }

    /// Preference level for the IEEE 802.15.4 radio link.
    pub fn ieee_802_15_4_preference(&self) -> u8 {
        self.0.mIeee802154Info.mPreference
    }

    /// Preference level for the Thread Radio Encapsulation Link (TREL) radio link.
    pub fn trel_preference(&self) -> u8 {
        self.0.mTrelUdp6Info.mPreference
    }
}
