// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents border routing counters.
///
/// Functional equivalent of [`otsys::otBorderRoutingCounters`](crate::otsys::otBorderRoutingCounters).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct BorderRoutingCounters(pub otBorderRoutingCounters);

impl_ot_castable!(BorderRoutingCounters, otBorderRoutingCounters);

impl BorderRoutingCounters {
    /// Counters for inbound unicast packets.
    pub fn inbound_unicast(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mInboundUnicast)
    }

    /// Counters for inbound multicast packets.
    pub fn inbound_multicast(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mInboundMulticast)
    }

    /// Counters for outbound unicast packets.
    pub fn outbound_unicast(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mOutboundUnicast)
    }

    /// Counters for outbound multicast packets.
    pub fn outbound_multicast(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mOutboundMulticast)
    }

    /// The number of received RA packets.
    pub fn ra_rx(&self) -> u32 {
        self.0.mRaRx
    }

    /// The number of RA packets successfully transmitted.
    pub fn ra_tx_success(&self) -> u32 {
        self.0.mRaTxSuccess
    }

    /// The number of RA packets failed to transmit.
    pub fn ra_tx_failure(&self) -> u32 {
        self.0.mRaTxFailure
    }

    /// The number of received RS packets.
    pub fn rs_rx(&self) -> u32 {
        self.0.mRsRx
    }

    /// The number of RS packets successfully transmitted.
    pub fn rs_tx_success(&self) -> u32 {
        self.0.mRsTxSuccess
    }

    /// The number of RS packets failed to transmit.
    pub fn rs_tx_failure(&self) -> u32 {
        self.0.mRsTxFailure
    }

    /// Counters for inbound Internet when DHCPv6 PD enabled
    pub fn inbound_internet(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mInboundInternet)
    }

    /// Counters for outbound Internet when DHCPv6 PD enabled
    pub fn outbound_internet(&self) -> &PacketsAndBytes {
        PacketsAndBytes::ref_from_ot_ref(&self.0.mOutboundInternet)
    }
}

/// This structure represents border routing counters.
///
/// Functional equivalent of [`otsys::otPdProcessedRaInfo`](crate::otsys::otPdProcessedRaInfo).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct PdProcessedRaInfo(pub otPdProcessedRaInfo);

impl_ot_castable!(PdProcessedRaInfo, otPdProcessedRaInfo);

impl PdProcessedRaInfo {
    /// The number of platform generated RA handled by ApplyPlatformGeneratedRa.
    pub fn num_platform_ra_received(&self) -> u32 {
        self.0.mNumPlatformRaReceived
    }

    /// The number of PIO processed for adding OMR prefixes.
    pub fn num_platform_pio_processed(&self) -> u32 {
        self.0.mNumPlatformPioProcessed
    }

    /// The timestamp of last processed RA message.
    pub fn last_platform_ra_msec(&self) -> u32 {
        self.0.mLastPlatformRaMsec
    }
}
