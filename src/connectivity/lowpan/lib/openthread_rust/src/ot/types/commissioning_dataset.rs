// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Functional equivalent of [`otsys::otCommissioningDataset`](crate::otsys::otCommissioningDataset).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct CommissioningDataset(pub otCommissioningDataset);

impl_ot_castable!(CommissioningDataset, otCommissioningDataset);

impl CommissioningDataset {
    /// Whether the Dataset contains any extra unknown sub-TLV.
    pub fn has_extra_tlv(&self) -> bool {
        self.0.mHasExtraTlv()
    }

    /// Whether the Joiner UDP Port is set.
    pub fn is_joiner_udp_port_set(&self) -> bool {
        self.0.mIsJoinerUdpPortSet()
    }

    /// Whether the Border Router RLOC16 is set.
    pub fn is_locator_set(&self) -> bool {
        self.0.mIsLocatorSet()
    }

    /// Whether the Commissioner Session Id is set.
    pub fn is_session_id_set(&self) -> bool {
        self.0.mIsSessionIdSet()
    }

    /// Whether the Steering Data is set.
    pub fn is_steering_data_set(&self) -> bool {
        self.0.mIsSteeringDataSet()
    }

    /// Returns the Joiner UDP Port.
    pub fn joiner_udp_port(&self) -> u16 {
        self.0.mJoinerUdpPort
    }

    /// Returns the Border Router RLOC16.
    pub fn locator(&self) -> u16 {
        self.0.mLocator
    }

    /// Returns the Commissioner Session Id.
    pub fn session_id(&self) -> u16 {
        self.0.mSessionId
    }

    /// Returns the Steering Data.
    pub fn steering_data(&self) -> &SteeringData {
        (&self.0.mSteeringData).into()
    }
}
