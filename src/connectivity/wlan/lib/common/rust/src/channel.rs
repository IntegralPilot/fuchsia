// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ie;
use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use std::fmt;

// IEEE Std 802.11-2016, Annex E
// Note the distinction of index for primary20 and index for center frequency.
// Fuchsia OS minimizes the use of the notion of center frequency,
// with following exceptions:
// - Cbw80P80's secondary frequency segment
// - Frequency conversion at device drivers
pub type MHz = u16;
pub const BASE_FREQ_2GHZ: MHz = 2407;
pub const BASE_FREQ_5GHZ: MHz = 5000;

pub const INVALID_CHAN_IDX: u8 = 0;

/// Channel bandwidth. Cbw80P80 requires the specification of
/// channel index corresponding to the center frequency
/// of the secondary consecutive frequency segment.
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Bandwidth {
    Cbw20,
    Cbw40, // Same as Cbw40Above
    Cbw40Below,
    Cbw80,
    Cbw160,
    Cbw80P80 { vht_secondary_80_channel: u8 },
}

impl Bandwidth {
    // TODO(https://fxbug.dev/42164482): Implement `From `instead.
    pub fn to_fidl(&self) -> (fidl_ieee80211::ChannelBandwidth, u8) {
        match self {
            Bandwidth::Cbw20 => (fidl_ieee80211::ChannelBandwidth::Cbw20, 0),
            Bandwidth::Cbw40 => (fidl_ieee80211::ChannelBandwidth::Cbw40, 0),
            Bandwidth::Cbw40Below => (fidl_ieee80211::ChannelBandwidth::Cbw40Below, 0),
            Bandwidth::Cbw80 => (fidl_ieee80211::ChannelBandwidth::Cbw80, 0),
            Bandwidth::Cbw160 => (fidl_ieee80211::ChannelBandwidth::Cbw160, 0),
            Bandwidth::Cbw80P80 { vht_secondary_80_channel } => {
                (fidl_ieee80211::ChannelBandwidth::Cbw80P80, *vht_secondary_80_channel)
            }
        }
    }

    pub fn from_fidl(
        fidl_cbw: fidl_ieee80211::ChannelBandwidth,
        fidl_vht_secondary_80_channel: u8,
    ) -> Result<Self, anyhow::Error> {
        match fidl_cbw {
            fidl_ieee80211::ChannelBandwidth::Cbw20 => Ok(Bandwidth::Cbw20),
            fidl_ieee80211::ChannelBandwidth::Cbw40 => Ok(Bandwidth::Cbw40),
            fidl_ieee80211::ChannelBandwidth::Cbw40Below => Ok(Bandwidth::Cbw40Below),
            fidl_ieee80211::ChannelBandwidth::Cbw80 => Ok(Bandwidth::Cbw80),
            fidl_ieee80211::ChannelBandwidth::Cbw160 => Ok(Bandwidth::Cbw160),
            fidl_ieee80211::ChannelBandwidth::Cbw80P80 => {
                Ok(Bandwidth::Cbw80P80 { vht_secondary_80_channel: fidl_vht_secondary_80_channel })
            }
            fidl_ieee80211::ChannelBandwidthUnknown!() => {
                Err(format_err!("Unknown channel bandwidth from fidl: {:?}", fidl_cbw))
            }
        }
    }
}

/// A Channel defines the frequency spectrum to be used for radio synchronization.
/// See for sister definitions in FIDL and C/C++
///  - //sdk/fidl/fuchsia.wlan.common/wlan_common.fidl |struct wlan_channel_t|
///  - //sdk/fidl/fuchsia.wlan.mlme/wlan_mlme.fidl |struct WlanChan|
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub struct Channel {
    pub primary: u8,
    pub bandwidth: Bandwidth,
    pub band: fidl_ieee80211::WlanBand,
}

// Fuchsia's short CBW notation. Not IEEE standard.
impl fmt::Display for Bandwidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Bandwidth::Cbw20 => write!(f, ""), // Vanilla plain 20 MHz bandwidth
            Bandwidth::Cbw40 => write!(f, "+"), // SCA, often denoted by "+1"
            Bandwidth::Cbw40Below => write!(f, "-"), // SCB, often denoted by "-1",
            Bandwidth::Cbw80 => write!(f, "V"), // VHT 80 MHz (V from VHT)
            Bandwidth::Cbw160 => write!(f, "W"), // VHT 160 MHz (as Wide as V + V ;) )
            Bandwidth::Cbw80P80 { vht_secondary_80_channel } => {
                write!(f, "+{}P", vht_secondary_80_channel)
            } // VHT 80Plus80 (not often obvious, but P is the first alphabet)
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{} ({:?})", self.primary, self.bandwidth, self.band)
    }
}

impl Channel {
    pub const fn new(primary: u8, bandwidth: Bandwidth, band: fidl_ieee80211::WlanBand) -> Self {
        Channel { primary, bandwidth, band }
    }

    fn get_band_start_freq(&self) -> Result<MHz, anyhow::Error> {
        match self.band {
            fidl_ieee80211::WlanBand::TwoGhz => Ok(BASE_FREQ_2GHZ),
            fidl_ieee80211::WlanBand::FiveGhz => Ok(BASE_FREQ_5GHZ),
            _ => Err(format_err!("cannot get band start freq for channel {}", self)),
        }
    }

    fn get_center_chan_idx(&self) -> Result<u8, anyhow::Error> {
        let is_valid = match self.band {
            fidl_ieee80211::WlanBand::TwoGhz => (1..=14).contains(&self.primary),
            fidl_ieee80211::WlanBand::FiveGhz => (36..=177).contains(&self.primary),
            _ => false,
        };
        if !is_valid {
            return Err(format_err!(
                "cannot get center channel index for an invalid primary channel {}",
                self
            ));
        }

        let p = self.primary;
        match self.bandwidth {
            Bandwidth::Cbw20 => Ok(p),
            Bandwidth::Cbw40 => {
                p.checked_add(2).ok_or_else(|| format_err!("invalid channel for Cbw40: {}", p))
            }
            Bandwidth::Cbw40Below => p
                .checked_sub(2)
                .filter(|&res| res != INVALID_CHAN_IDX)
                .ok_or_else(|| format_err!("invalid channel for Cbw40Below: {}", p)),
            Bandwidth::Cbw80 | Bandwidth::Cbw80P80 { .. } => match p {
                36..=48 => Ok(42),
                52..=64 => Ok(58),
                100..=112 => Ok(106),
                116..=128 => Ok(122),
                132..=144 => Ok(138),
                149..=161 => Ok(155),
                165..=177 => Ok(171),
                _ => {
                    return Err(format_err!(
                        "cannot get center channel index for invalid channel {}",
                        self
                    ));
                }
            },
            Bandwidth::Cbw160 => match p {
                36..=64 => Ok(50),
                100..=128 => Ok(114),
                149..=177 => Ok(163),
                _ => {
                    return Err(format_err!(
                        "cannot get center channel index for invalid channel {}",
                        self
                    ));
                }
            },
        }
    }

    /// Returns the center frequency of the first consecutive frequency segment of the channel
    /// in MHz if the channel is valid, Err(String) otherwise.
    pub fn get_center_freq(&self) -> Result<MHz, anyhow::Error> {
        // IEEE Std 802.11-2016, 21.3.14
        let start_freq = self.get_band_start_freq()?;
        let center_chan_idx = self.get_center_chan_idx()?;
        let spacing: MHz = 5;
        let offset = spacing
            .checked_mul(center_chan_idx as u16)
            .ok_or_else(|| format_err!("overflow computing channel spacing offset for {}", self))?;
        start_freq
            .checked_add(offset)
            .ok_or_else(|| format_err!("overflow computing center frequency for {}", self))
    }
}

impl Into<fidl_ieee80211::ChannelNumber> for Channel {
    fn into(self) -> fidl_ieee80211::ChannelNumber {
        fidl_ieee80211::ChannelNumber { band: self.band, number: self.primary }
    }
}
impl Channel {
    pub fn from_fidl(
        fidl_channel: fidl_ieee80211::ChannelNumber,
        fidl_cbw: fidl_ieee80211::ChannelBandwidth,
        fidl_vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> Result<Self, anyhow::Error> {
        if fidl_cbw == fidl_ieee80211::ChannelBandwidth::Cbw80P80 {
            if fidl_vht_secondary_80_channel.band != fidl_channel.band {
                return Err(format_err!(
                    "vht_secondary_80_channel band ({:?}) does not match primary band ({:?})",
                    fidl_vht_secondary_80_channel.band,
                    fidl_channel.band
                ));
            }
        }
        let cbw = Bandwidth::from_fidl(fidl_cbw, fidl_vht_secondary_80_channel.number)?;
        Ok(Channel::new(fidl_channel.number, cbw, fidl_channel.band))
    }
}

/// Derive channel given DSSS param set, HT operation, and VHT operation IEs from
/// beacon or probe response, and the primary channel from which such frame is
/// received on.
///
/// Primary channel is extracted from HT op, DSSS param set, or `rx_primary_channel`,
/// in descending priority.
pub fn derive_channel(
    rx_primary_channel: fidl_ieee80211::ChannelNumber,
    dsss_channel: Option<u8>,
    ht_op: Option<ie::HtOperation>,
    vht_op: Option<ie::VhtOperation>,
) -> Channel {
    let primary = ht_op
        .as_ref()
        .map(|ht_op| ht_op.primary_channel)
        .or(dsss_channel)
        .unwrap_or(rx_primary_channel.number);

    let ht_op_cbw = ht_op.map(|ht_op| ht_op.ht_op_info.sta_chan_width());
    let vht_cbw_and_segs =
        vht_op.map(|vht_op| (vht_op.vht_cbw, vht_op.center_freq_seg0, vht_op.center_freq_seg1));

    let cbw = match ht_op_cbw {
        // Inspect vht/ht op parameters to determine the channel width.
        Some(ie::StaChanWidth::ANY) => {
            // Safe to unwrap `ht_op` because `ht_op_cbw` is only Some(_) if `ht_op` has a value.
            let sec_chan_offset = ht_op.unwrap().ht_op_info.secondary_chan_offset();
            derive_wide_channel_bandwidth(vht_cbw_and_segs, sec_chan_offset)
        }
        // Default to Cbw20 if HT CBW field is set to 0 or not present.
        _ => Bandwidth::Cbw20,
    };

    Channel::new(primary, cbw, rx_primary_channel.band)
}

/// Derive a CBW for a primary channel or channel switch.
/// VHT parameter derivation is defined identically by:
///     IEEE Std 802.11-2016 9.4.2.159 Table 9-252 for channel switching
///     IEEE Std 802.11-2016 11.40.1 Table 11-24 for VHT operation
/// SecChanOffset is defined identially by:
///     IEEE Std 802.11-2016 9.4.2.20 for channel switching
///     IEEE Std 802.11-2016 9.4.2.57 Table 9-168 for HT operation
pub fn derive_wide_channel_bandwidth(
    vht_cbw_and_segs: Option<(ie::VhtChannelBandwidth, u8, u8)>,
    sec_chan_offset: ie::SecChanOffset,
) -> Bandwidth {
    use ie::VhtChannelBandwidth as Vcb;
    match vht_cbw_and_segs {
        Some((Vcb::CBW_80_160_80P80, _, 0)) => Bandwidth::Cbw80,
        Some((Vcb::CBW_80_160_80P80, seg0, seg1)) if seg0.abs_diff(seg1) == 8 => Bandwidth::Cbw160,
        Some((Vcb::CBW_80_160_80P80, seg0, seg1)) if seg0.abs_diff(seg1) > 16 => {
            // See IEEE 802.11-2016, Table 9-252, about channel center frequency segment 1
            Bandwidth::Cbw80P80 { vht_secondary_80_channel: seg1 }
        }
        // Use HT CBW if
        // - VHT op is not present, or
        // - VHT CBW field is set to 0
        _ => match sec_chan_offset {
            ie::SecChanOffset::SECONDARY_ABOVE => Bandwidth::Cbw40,
            ie::SecChanOffset::SECONDARY_BELOW => Bandwidth::Cbw40Below,
            ie::SecChanOffset::SECONDARY_NONE | _ => Bandwidth::Cbw20,
        },
    }
}

/// Converts a 20MHz primary channel center frequency in MHz to a channel number. Returns an error
/// if the frequency does not correspond to the center frequency of a valid
/// standard channel in the 2.4GHz or 5GHz bands.
pub fn primary_channel_from_freq(freq: u32) -> Option<u8> {
    // 2.4 GHz: Channels 1-13
    if (2412..=2472).contains(&freq) && (freq - 2412) % 5 == 0 {
        Some(((freq - 2407) / 5) as u8)
    // 2.4 GHz: Channel 14
    } else if freq == 2484 {
        Some(14)
    // 5 GHz: Channels 36-144
    } else if (5180..=5720).contains(&freq) && (freq - 5180) % 20 == 0 {
        Some(((freq - 5000) / 5) as u8)
    // 5 GHz: Channels 149-173
    } else if (5745..=5865).contains(&freq) && (freq - 5745) % 20 == 0 {
        Some(((freq - 5000) / 5) as u8)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_ieee80211::WlanBand::{FiveGhz, TwoGhz};

    fn rx_channel(number: u8, band: fidl_ieee80211::WlanBand) -> fidl_ieee80211::ChannelNumber {
        fidl_ieee80211::ChannelNumber { band, number }
    }

    #[test]
    fn fmt_display() {
        let mut c = Channel::new(100, Bandwidth::Cbw40, FiveGhz);
        assert_eq!(format!("{}", c), "100+ (FiveGhz)");
        c.bandwidth = Bandwidth::Cbw160;
        assert_eq!(format!("{}", c), "100W (FiveGhz)");
        c.bandwidth = Bandwidth::Cbw80P80 { vht_secondary_80_channel: 200 };
        assert_eq!(format!("{}", c), "100+200P (FiveGhz)");
    }

    #[test]
    fn test_band_start_freq() {
        assert_eq!(
            BASE_FREQ_2GHZ,
            Channel::new(1, Bandwidth::Cbw20, TwoGhz).get_band_start_freq().unwrap()
        );
        assert_eq!(
            BASE_FREQ_5GHZ,
            Channel::new(100, Bandwidth::Cbw20, FiveGhz).get_band_start_freq().unwrap()
        );
    }

    #[test]
    fn test_get_center_chan_idx() {
        assert!(Channel::new(1, Bandwidth::Cbw80, TwoGhz).get_center_chan_idx().is_err());
        assert_eq!(
            9,
            Channel::new(11, Bandwidth::Cbw40Below, TwoGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(8, Channel::new(6, Bandwidth::Cbw40, TwoGhz).get_center_chan_idx().unwrap());
        assert_eq!(36, Channel::new(36, Bandwidth::Cbw20, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(38, Channel::new(36, Bandwidth::Cbw40, FiveGhz).get_center_chan_idx().unwrap());

        // 80 MHz ranges: beginning and end of each range
        assert_eq!(42, Channel::new(36, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(42, Channel::new(48, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(58, Channel::new(52, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(58, Channel::new(64, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(
            106,
            Channel::new(100, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            106,
            Channel::new(112, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            122,
            Channel::new(116, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            122,
            Channel::new(128, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            138,
            Channel::new(132, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            138,
            Channel::new(144, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            155,
            Channel::new(149, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            155,
            Channel::new(161, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            171,
            Channel::new(165, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            171,
            Channel::new(177, Bandwidth::Cbw80, FiveGhz).get_center_chan_idx().unwrap()
        );

        // 160 MHz ranges: beginning and end of each range
        assert_eq!(50, Channel::new(36, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(50, Channel::new(64, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap());
        assert_eq!(
            114,
            Channel::new(100, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            114,
            Channel::new(128, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            163,
            Channel::new(149, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap()
        );
        assert_eq!(
            163,
            Channel::new(177, Bandwidth::Cbw160, FiveGhz).get_center_chan_idx().unwrap()
        );

        // 80+80 MHz
        assert_eq!(
            42,
            Channel::new(36, Bandwidth::Cbw80P80 { vht_secondary_80_channel: 155 }, FiveGhz)
                .get_center_chan_idx()
                .unwrap()
        );

        // These tests are edge-case channel validity checks.
        // Channel 1 Cbw40Below has the potential to underflow.
        // Channel 255 Cbw40 has the potential to overflow, but is caught by earlier channel index
        // validation.
        assert!(Channel::new(1, Bandwidth::Cbw40Below, TwoGhz).get_center_chan_idx().is_err());
        assert!(Channel::new(255, Bandwidth::Cbw40, TwoGhz).get_center_chan_idx().is_err());
    }

    #[test]
    fn test_get_center_freq() {
        assert_eq!(
            2412 as MHz,
            Channel::new(1, Bandwidth::Cbw20, TwoGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            2437 as MHz,
            Channel::new(6, Bandwidth::Cbw20, TwoGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            2447 as MHz,
            Channel::new(6, Bandwidth::Cbw40, TwoGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            2427 as MHz,
            Channel::new(6, Bandwidth::Cbw40Below, TwoGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5180 as MHz,
            Channel::new(36, Bandwidth::Cbw20, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5190 as MHz,
            Channel::new(36, Bandwidth::Cbw40, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5210 as MHz,
            Channel::new(36, Bandwidth::Cbw80, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5775 as MHz,
            Channel::new(149, Bandwidth::Cbw80, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5855 as MHz,
            Channel::new(165, Bandwidth::Cbw80, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5250 as MHz,
            Channel::new(36, Bandwidth::Cbw160, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5570 as MHz,
            Channel::new(100, Bandwidth::Cbw160, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5815 as MHz,
            Channel::new(149, Bandwidth::Cbw160, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5815 as MHz,
            Channel::new(165, Bandwidth::Cbw160, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5815 as MHz,
            Channel::new(173, Bandwidth::Cbw160, FiveGhz).get_center_freq().unwrap()
        );
        assert_eq!(
            5210 as MHz,
            Channel::new(36, Bandwidth::Cbw80P80 { vht_secondary_80_channel: 155 }, FiveGhz)
                .get_center_freq()
                .unwrap()
        );
    }

    #[test]
    fn test_primarychannel_from_freq() {
        assert_eq!(Some(1), primary_channel_from_freq(2412));
        // This is between the center frequencies of channel 1 and 2
        assert_eq!(None, primary_channel_from_freq(2413));
        assert_eq!(Some(6), primary_channel_from_freq(2437));
        assert_eq!(Some(13), primary_channel_from_freq(2472));
        assert_eq!(Some(14), primary_channel_from_freq(2484));
        // This is below the range of recognized 5GHz channels
        assert_eq!(None, primary_channel_from_freq(5160));
        assert_eq!(Some(36), primary_channel_from_freq(5180));
        // This is between the center frequencies of channel 36 and 40
        assert_eq!(None, primary_channel_from_freq(5190));
        assert_eq!(Some(165), primary_channel_from_freq(5825));
        assert_eq!(Some(173), primary_channel_from_freq(5865));
        // This is below the range of recognized 2.4GHz channels
        assert_eq!(None, primary_channel_from_freq(2400));
        // This is below the range of recognized 5GHz channels
        assert_eq!(None, primary_channel_from_freq(5000));
        // This is above the range of recognized channels
        assert_eq!(None, primary_channel_from_freq(5885));
    }

    const RX_PRIMARY_CHAN: u8 = 11;
    const RX_PRIMARY_CHAN_5GHZ: u8 = 36;
    const HT_PRIMARY_CHAN: u8 = 48;

    #[test]
    fn test_derive_channel_basic() {
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN, band: TwoGhz },
            None,
            None,
            None,
        );
        assert_eq!(channel, Channel::new(RX_PRIMARY_CHAN, Bandwidth::Cbw20, TwoGhz));
    }

    #[test]
    fn test_derive_channel_with_dsss_param() {
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN, band: TwoGhz },
            Some(6),
            None,
            None,
        );
        assert_eq!(channel, Channel::new(6, Bandwidth::Cbw20, TwoGhz));
    }

    #[test]
    fn test_derive_channel_with_ht_20mhz() {
        let expected_channel = Channel::new(HT_PRIMARY_CHAN, Bandwidth::Cbw20, FiveGhz);

        let test_params = [
            (ie::StaChanWidth::TWENTY_MHZ, ie::SecChanOffset::SECONDARY_NONE),
            (ie::StaChanWidth::TWENTY_MHZ, ie::SecChanOffset::SECONDARY_ABOVE),
            (ie::StaChanWidth::TWENTY_MHZ, ie::SecChanOffset::SECONDARY_BELOW),
            (ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_NONE),
        ];

        for (ht_width, sec_chan_offset) in test_params.iter() {
            let ht_op = ht_op(HT_PRIMARY_CHAN, *ht_width, *sec_chan_offset);
            let channel = derive_channel(
                fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
                Some(RX_PRIMARY_CHAN_5GHZ),
                Some(ht_op),
                None,
            );
            assert_eq!(channel, expected_channel);
        }
    }

    #[test]
    fn test_derive_channel_with_ht_40mhz() {
        let ht_op =
            ht_op(HT_PRIMARY_CHAN, ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_ABOVE);
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
            Some(RX_PRIMARY_CHAN_5GHZ),
            Some(ht_op),
            None,
        );
        assert_eq!(channel, Channel::new(HT_PRIMARY_CHAN, Bandwidth::Cbw40, FiveGhz));
    }

    #[test]
    fn test_derive_channel_with_ht_40mhz_below() {
        let ht_op =
            ht_op(HT_PRIMARY_CHAN, ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_BELOW);
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
            Some(RX_PRIMARY_CHAN_5GHZ),
            Some(ht_op),
            None,
        );
        assert_eq!(channel, Channel::new(HT_PRIMARY_CHAN, Bandwidth::Cbw40Below, FiveGhz));
    }

    #[test]
    fn test_derive_channel_with_vht_80mhz() {
        let ht_op =
            ht_op(HT_PRIMARY_CHAN, ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_ABOVE);
        let vht_op = vht_op(ie::VhtChannelBandwidth::CBW_80_160_80P80, 8, 0);
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
            Some(RX_PRIMARY_CHAN_5GHZ),
            Some(ht_op),
            Some(vht_op),
        );
        assert_eq!(channel, Channel::new(HT_PRIMARY_CHAN, Bandwidth::Cbw80, FiveGhz));
    }

    #[test]
    fn test_derive_channel_with_vht_160mhz() {
        let ht_op =
            ht_op(HT_PRIMARY_CHAN, ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_ABOVE);
        let vht_op = vht_op(ie::VhtChannelBandwidth::CBW_80_160_80P80, 0, 8);
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
            Some(RX_PRIMARY_CHAN_5GHZ),
            Some(ht_op),
            Some(vht_op),
        );
        assert_eq!(channel, Channel::new(HT_PRIMARY_CHAN, Bandwidth::Cbw160, FiveGhz));
    }

    #[test]
    fn test_derive_channel_with_vht_80plus80mhz() {
        let ht_op =
            ht_op(HT_PRIMARY_CHAN, ie::StaChanWidth::ANY, ie::SecChanOffset::SECONDARY_ABOVE);
        let vht_op = vht_op(ie::VhtChannelBandwidth::CBW_80_160_80P80, 18, 1);
        let channel = derive_channel(
            fidl_ieee80211::ChannelNumber { number: RX_PRIMARY_CHAN_5GHZ, band: FiveGhz },
            Some(RX_PRIMARY_CHAN_5GHZ),
            Some(ht_op),
            Some(vht_op),
        );
        assert_eq!(
            channel,
            Channel::new(
                HT_PRIMARY_CHAN,
                Bandwidth::Cbw80P80 { vht_secondary_80_channel: 1 },
                FiveGhz
            )
        );
    }

    #[test]
    fn test_derive_channel_none() {
        let channel = derive_channel(rx_channel(8, TwoGhz), None, None, None);
        assert_eq!(channel, Channel::new(8, Bandwidth::Cbw20, TwoGhz));
    }

    #[test]
    fn test_derive_channel_no_rx_primary() {
        let channel = derive_channel(rx_channel(8, TwoGhz), Some(6), None, None);
        assert_eq!(channel, Channel::new(6, Bandwidth::Cbw20, TwoGhz))
    }

    fn ht_op(
        primary_channel: u8,
        chan_width: ie::StaChanWidth,
        offset: ie::SecChanOffset,
    ) -> ie::HtOperation {
        let ht_op_info =
            ie::HtOpInfo::new().with_sta_chan_width(chan_width).with_secondary_chan_offset(offset);
        ie::HtOperation { primary_channel, ht_op_info, basic_ht_mcs_set: ie::SupportedMcsSet(0) }
    }

    fn vht_op(vht_cbw: ie::VhtChannelBandwidth, seg0: u8, seg1: u8) -> ie::VhtOperation {
        ie::VhtOperation {
            vht_cbw,
            center_freq_seg0: seg0,
            center_freq_seg1: seg1,
            basic_mcs_nss: ie::VhtMcsNssMap(0),
        }
    }
}
