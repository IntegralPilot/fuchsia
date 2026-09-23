// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ie::SupportedRate;
use crate::mac::WlanGi;
use anyhow::{Error, bail};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_softmac as fidl_softmac;

pub const HT_NUM_MCS: u8 = 32; // Only support MCS 0-31
pub const HT_NUM_UNIQUE_MCS: u8 = 8;
pub const ERP_NUM_TX_VECTOR: u8 = 8;

const INVALID_TX_VECTOR_IDX: u16 = fidl_common::WLAN_TX_VECTOR_IDX_INVALID;

const HT_NUM_GI: u8 = 2;
const HT_NUM_CBW: u8 = 2;
const HT_NUM_TX_VECTOR: u8 = HT_NUM_GI * HT_NUM_CBW * HT_NUM_MCS;

const DSSS_CCK_NUM_TX_VECTOR: u8 = 4;

pub const START_IDX: u16 = 1 + INVALID_TX_VECTOR_IDX;
pub const HT_START_IDX: u16 = START_IDX;
pub const ERP_START_IDX: u16 = HT_START_IDX + HT_NUM_TX_VECTOR as u16;
pub const DSSS_CCK_START_IDX: u16 = ERP_START_IDX + ERP_NUM_TX_VECTOR as u16;
pub const MAX_VALID_IDX: u16 = DSSS_CCK_START_IDX + DSSS_CCK_NUM_TX_VECTOR as u16 - 1;

// Notes about HT:
// Changing CBW (channel bandwidth) from 20 MHz to 40 MHz advances index by 32
// Changing GI (gap interval) from 800 ns to 400 ns advances index by 64
//
//  Group   tx_vec_idx_t range    PHY   GI   CBW NSS MCS_IDX
//  0         1 -  32             HT    800  20  -   0-31
//  1        33 -  64             HT    800  40  -   0-31
//  2        65 -  96             HT    400  20  -   0-31
//  3        97 - 128             HT    400  40  -   0-31
//  4       129 - 136             ERP   -    -   -   0-7
//  5       137 - 138             DSSS  -    -   -   0-1
//  6       139 - 140             CCK   -    -   -   2-3
//
// TODO(https://fxbug.dev/42094755) VHT will be inserted between HT and ERP.

#[derive(PartialEq, Debug)]
/// Encapsulates parameters for transmitting a packet over a PHY.
///
/// MCS index is defined in
/// * HT: IEEE 802.11-2016 Table 19-27
/// * VHT: IEEE 802.11-2016 Table 21-30
///
/// We extend the definition of MCS index beyond IEEE 802.11-2016 as follows:
/// * For ERP/ERP-OFDM (WlanPhyType::Erp):
///     * 0: BPSK,   1/2 -> Data rate  6 Mbps
///     * 1: BPSK,   3/4 -> Data rate  9 Mbps
///     * 2: QPSK,   1/2 -> Data rate 12 Mbps
///     * 3: QPSK,   3/4 -> Data rate 18 Mbps
///     * 4: 16-QAM, 1/2 -> Data rate 24 Mbps
///     * 5: 16-QAM, 3/4 -> Data rate 36 Mbps
///     * 6: 64-QAM, 2/3 -> Data rate 48 Mbps
///     * 7: 64-QAM, 3/4 -> Data rate 54 Mbps
/// * For DSSS, HR/DSSS, and ERP-DSSS/CCK (WlanPhyType::Dsss and WlanPhyType::Cck):
///     * 0:  2 -> 1   Mbps DSSS
///     * 1:  4 -> 2   Mbps DSSS
///     * 2: 11 -> 5.5 Mbps CCK
///     * 3: 22 -> 11  Mbps CCK
pub struct TxVector {
    phy: fidl_ieee80211::WlanPhyType,
    gi: WlanGi,
    bandwidth: fidl_ieee80211::ChannelBandwidth,
    nss: u8, // Number of spatial streams for VHT and beyond.
    // For HT,  see IEEE 802.11-2016 Table 19-27
    // For VHT, see IEEE 802.11-2016 Table 21-30
    // For ERP, see comment above (this is a Fuchsia extension)
    mcs_idx: u8,
}

impl TxVector {
    pub fn new(
        phy: fidl_ieee80211::WlanPhyType,
        gi: WlanGi,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        mcs_idx: u8,
    ) -> Result<Self, Error> {
        let supported_mcs = match phy {
            fidl_ieee80211::WlanPhyType::Dsss => mcs_idx == 0 || mcs_idx == 1,
            fidl_ieee80211::WlanPhyType::Hr => mcs_idx == 2 || mcs_idx == 3,
            fidl_ieee80211::WlanPhyType::Ht => {
                match gi {
                    WlanGi::G_800NS | WlanGi::G_400NS => (),
                    other => bail!("Unsupported GI for HT PHY: {:?}", other),
                }
                match bandwidth {
                    fidl_ieee80211::ChannelBandwidth::Cbw20
                    | fidl_ieee80211::ChannelBandwidth::Cbw40
                    | fidl_ieee80211::ChannelBandwidth::Cbw40Below => (),
                    other => bail!("Unsupported CBW for HT PHY: {:?}", other),
                }
                mcs_idx < HT_NUM_MCS
            }
            fidl_ieee80211::WlanPhyType::Erp => mcs_idx < ERP_NUM_TX_VECTOR,
            other => bail!("Unsupported phy type: {:?}", other),
        };
        if supported_mcs {
            let nss = match phy {
                fidl_ieee80211::WlanPhyType::Ht => 1 + mcs_idx / HT_NUM_UNIQUE_MCS,
                // TODO(https://fxbug.dev/42094755): Support VHT NSS
                _ => 1,
            };
            Ok(Self { phy, gi, bandwidth, nss, mcs_idx })
        } else {
            bail!("Unsupported MCS {:?} for phy type {:?}", mcs_idx, phy);
        }
    }

    pub fn phy(&self) -> fidl_ieee80211::WlanPhyType {
        self.phy
    }

    pub fn from_supported_rate(erp_rate: &SupportedRate) -> Result<Self, Error> {
        let (phy, mcs_idx) = match erp_rate.rate() {
            2 => (fidl_ieee80211::WlanPhyType::Dsss, 0),
            4 => (fidl_ieee80211::WlanPhyType::Dsss, 1),
            11 => (fidl_ieee80211::WlanPhyType::Hr, 2),
            22 => (fidl_ieee80211::WlanPhyType::Hr, 3),
            12 => (fidl_ieee80211::WlanPhyType::Erp, 0),
            18 => (fidl_ieee80211::WlanPhyType::Erp, 1),
            24 => (fidl_ieee80211::WlanPhyType::Erp, 2),
            36 => (fidl_ieee80211::WlanPhyType::Erp, 3),
            48 => (fidl_ieee80211::WlanPhyType::Erp, 4),
            72 => (fidl_ieee80211::WlanPhyType::Erp, 5),
            96 => (fidl_ieee80211::WlanPhyType::Erp, 6),
            108 => (fidl_ieee80211::WlanPhyType::Erp, 7),
            other_rate => {
                bail!("Invalid rate {} * 0.5 Mbps for 802.11a/b/g.", other_rate);
            }
        };
        Self::new(phy, WlanGi::G_800NS, fidl_ieee80211::ChannelBandwidth::Cbw20, mcs_idx)
    }

    // We guarantee safety of the unwraps in the following two functions by testing all TxVecIdx
    // values exhaustively.

    pub fn from_idx(idx: TxVecIdx) -> Self {
        let phy = idx.to_phy();
        match phy {
            fidl_ieee80211::WlanPhyType::Ht => {
                let group_idx = (*idx - HT_START_IDX) / HT_NUM_MCS as u16;
                let gi = match (group_idx / HT_NUM_CBW as u16) % HT_NUM_GI as u16 {
                    1 => WlanGi::G_400NS,
                    _ => WlanGi::G_800NS,
                };
                let bandwidth = match group_idx % HT_NUM_CBW as u16 {
                    0 => fidl_ieee80211::ChannelBandwidth::Cbw20,
                    _ => fidl_ieee80211::ChannelBandwidth::Cbw40,
                };
                let mcs_idx = ((*idx - HT_START_IDX) % HT_NUM_MCS as u16) as u8;
                Self::new(phy, gi, bandwidth, mcs_idx).unwrap()
            }
            fidl_ieee80211::WlanPhyType::Erp => Self::new(
                phy,
                WlanGi::G_800NS,
                fidl_ieee80211::ChannelBandwidth::Cbw20,
                (*idx - ERP_START_IDX) as u8,
            )
            .unwrap(),
            fidl_ieee80211::WlanPhyType::Dsss | fidl_ieee80211::WlanPhyType::Hr => Self::new(
                phy,
                WlanGi::G_800NS,
                fidl_ieee80211::ChannelBandwidth::Cbw20,
                (*idx - DSSS_CCK_START_IDX) as u8,
            )
            .unwrap(),
            _ => unreachable!(),
        }
    }

    pub fn to_idx(&self) -> TxVecIdx {
        match self.phy {
            fidl_ieee80211::WlanPhyType::Ht => {
                let group_idx = match self.gi {
                    WlanGi::G_400NS => HT_NUM_CBW as u16,
                    _ => 0,
                } + match self.bandwidth {
                    fidl_ieee80211::ChannelBandwidth::Cbw40
                    | fidl_ieee80211::ChannelBandwidth::Cbw40Below => 1,
                    _ => 0,
                };
                TxVecIdx::new(HT_START_IDX + group_idx * HT_NUM_MCS as u16 + self.mcs_idx as u16)
                    .unwrap()
            }
            fidl_ieee80211::WlanPhyType::Erp => {
                TxVecIdx::new(ERP_START_IDX + self.mcs_idx as u16).unwrap()
            }
            fidl_ieee80211::WlanPhyType::Hr | fidl_ieee80211::WlanPhyType::Dsss => {
                TxVecIdx::new(DSSS_CCK_START_IDX + self.mcs_idx as u16).unwrap()
            }
            _ => unreachable!(),
        }
    }

    pub fn to_fidl_tx_info(
        &self,
        tx_flags: fidl_softmac::WlanTxInfoFlags,
        minstrel_enabled: bool,
    ) -> fidl_softmac::WlanTxInfo {
        fidl_softmac::WlanTxInfo {
            tx_flags: tx_flags.bits(),
            valid_fields: (fidl_softmac::WlanTxInfoValid::CHANNEL_BANDWIDTH
                | fidl_softmac::WlanTxInfoValid::PHY
                | fidl_softmac::WlanTxInfoValid::MCS
                | if minstrel_enabled {
                    fidl_softmac::WlanTxInfoValid::TX_VECTOR_IDX
                } else {
                    fidl_softmac::WlanTxInfoValid::empty()
                })
            .bits(),
            tx_vector_idx: self.to_idx().0,
            phy: self.phy,
            bandwidth: self.bandwidth,
            mcs: self.mcs_idx,
        }
    }
}

#[derive(Hash, PartialEq, Eq, Debug, Copy, Clone, Ord, PartialOrd)]
pub struct TxVecIdx(u16);
impl std::ops::Deref for TxVecIdx {
    type Target = u16;
    fn deref(&self) -> &u16 {
        &self.0
    }
}

impl TxVecIdx {
    pub fn new(value: u16) -> Option<Self> {
        if INVALID_TX_VECTOR_IDX < value && value <= MAX_VALID_IDX {
            Some(Self(value))
        } else {
            None
        }
    }

    // TODO(https://fxbug.dev/42163096): Add a const fn new when it's a stable feature.

    pub fn to_erp_rate(&self) -> Option<SupportedRate> {
        const ERP_RATE_LIST: [u8; ERP_NUM_TX_VECTOR as usize] = [12, 18, 24, 36, 48, 72, 96, 108];
        if self.is_erp() {
            Some(SupportedRate(ERP_RATE_LIST[(self.0 - ERP_START_IDX) as usize]))
        } else {
            None
        }
    }

    pub fn to_phy(&self) -> fidl_ieee80211::WlanPhyType {
        match self.0 {
            idx if idx < HT_START_IDX + HT_NUM_TX_VECTOR as u16 => fidl_ieee80211::WlanPhyType::Ht,
            idx if idx < ERP_START_IDX + ERP_NUM_TX_VECTOR as u16 => {
                fidl_ieee80211::WlanPhyType::Erp
            }
            idx if idx < DSSS_CCK_START_IDX + 2 => fidl_ieee80211::WlanPhyType::Dsss,
            idx if idx < DSSS_CCK_START_IDX + DSSS_CCK_NUM_TX_VECTOR as u16 => {
                fidl_ieee80211::WlanPhyType::Hr
            }
            // This panic is unreachable for any TxVecIdx constructed with TxVecIdx::new.
            // Verified by exhaustive test cases.
            _ => panic!("TxVecIdx has invalid value"),
        }
    }

    pub fn is_ht(&self) -> bool {
        HT_START_IDX <= self.0 && self.0 < HT_START_IDX + HT_NUM_TX_VECTOR as u16
    }

    pub fn is_erp(&self) -> bool {
        ERP_START_IDX <= self.0 && self.0 < ERP_START_IDX + ERP_NUM_TX_VECTOR as u16
    }
}

impl std::fmt::Display for TxVecIdx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tx_vector = TxVector::from_idx(*self);
        write!(f, "TxVecIdx {:3}: {:?}", self.0, tx_vector)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_tx_vector_idxs() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            idx.to_phy(); // Shouldn't panic for any value.
        }
        assert!(
            TxVecIdx::new(INVALID_TX_VECTOR_IDX).is_none(),
            "Should not be able to construct invalid tx vector idx"
        );
        assert!(
            TxVecIdx::new(MAX_VALID_IDX + 1).is_none(),
            "Should not be able to construct invalid tx vector idx"
        );
    }

    #[test]
    fn erp_rates() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            assert_eq!(idx.is_erp(), idx.to_erp_rate().is_some());
        }
    }

    #[test]
    fn phy_types() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            if idx.is_erp() {
                assert_eq!(idx.to_phy(), fidl_ieee80211::WlanPhyType::Erp);
            } else if idx.is_ht() {
                assert_eq!(idx.to_phy(), fidl_ieee80211::WlanPhyType::Ht);
            } else {
                assert!(
                    idx.to_phy() == fidl_ieee80211::WlanPhyType::Dsss
                        || idx.to_phy() == fidl_ieee80211::WlanPhyType::Hr
                );
            }
        }
    }

    #[test]
    fn to_and_from_idx() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            let tx_vector = TxVector::from_idx(idx);
            assert_eq!(idx, tx_vector.to_idx());
        }
    }

    #[test]
    fn ht_and_erp_phy_types() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            let tx_vector = TxVector::from_idx(idx);
            if idx.is_erp() {
                assert_eq!(tx_vector.phy(), fidl_ieee80211::WlanPhyType::Erp);
            } else if idx.is_ht() {
                assert_eq!(tx_vector.phy(), fidl_ieee80211::WlanPhyType::Ht);
            }
        }
    }

    #[test]
    fn from_erp_rates() {
        for idx in INVALID_TX_VECTOR_IDX + 1..=MAX_VALID_IDX {
            let idx = TxVecIdx::new(idx).expect("Could not make TxVecIdx from valid index");
            if idx.is_erp() {
                let erp_rate = idx.to_erp_rate().unwrap();
                let tx_vector = TxVector::from_supported_rate(&erp_rate)
                    .expect("Could not make TxVector from ERP rate.");
                assert_eq!(idx, tx_vector.to_idx());
            }
        }
    }
}
