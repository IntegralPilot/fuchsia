// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents the Thread TREL counters.
///
/// Functional equivalent of [`otsys::otTrelCounters`](crate::otsys::otTrelCounters).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct TrelCounters(pub otTrelCounters);

impl_ot_castable!(TrelCounters, otTrelCounters);

impl TrelCounters {
    /// Get TREL tx packet counter
    pub fn get_tx_packets(&self) -> u64 {
        self.0.mTxPackets
    }

    /// Get TREL tx bytes counter
    pub fn get_tx_bytes(&self) -> u64 {
        self.0.mTxBytes
    }

    /// Get TREL tx failure counter
    pub fn get_tx_failure(&self) -> u64 {
        self.0.mTxFailure
    }

    /// Get TREL rx packet counter
    pub fn get_rx_packets(&self) -> u64 {
        self.0.mRxPackets
    }

    /// Get TREL rx bytes counter
    pub fn get_rx_bytes(&self) -> u64 {
        self.0.mRxBytes
    }

    /// Update TREL tx packet counter by `cnt`
    pub fn update_tx_packets(&mut self, cnt: u64) {
        self.0.mTxPackets += cnt;
    }

    /// Update TREL tx bytes counter by `cnt`
    pub fn update_tx_bytes(&mut self, cnt: u64) {
        self.0.mTxBytes += cnt;
    }

    /// Update TREL tx failure counter by `cnt`
    pub fn update_tx_failure(&mut self, cnt: u64) {
        self.0.mTxFailure += cnt;
    }

    /// Update TREL rx packet counter by `cnt`
    pub fn update_rx_packets(&mut self, cnt: u64) {
        self.0.mRxPackets += cnt;
    }

    /// Update TREL rx bytes counter by `cnt`
    pub fn update_rx_bytes(&mut self, cnt: u64) {
        self.0.mRxBytes += cnt;
    }

    /// Reset all counters
    pub fn reset_counters(&mut self) {
        self.0.mTxPackets = 0;
        self.0.mTxBytes = 0;
        self.0.mTxFailure = 0;
        self.0.mRxPackets = 0;
        self.0.mRxBytes = 0;
    }
}
