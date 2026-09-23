// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Crate wlan-common hosts common libraries
//! to be used for WLAN SME, MLME, and binaries written in Rust.

#![cfg_attr(feature = "benchmark", feature(test))]
pub mod append;
pub mod bss;
pub mod buffer_reader;
pub mod buffer_writer;
pub mod capabilities;
pub mod channel;
pub mod data_writer;
pub mod energy;
pub mod error;
pub mod historical_list;
pub mod ie;
pub mod mac;
pub mod mgmt_writer;
pub mod organization;
pub mod scan;
pub mod security;
pub mod sequence;
pub mod sequestered;
pub mod sink;
pub mod stats;
#[cfg(target_os = "fuchsia")]
pub mod test_utils;
pub mod tim;
pub mod time;
#[cfg(target_os = "fuchsia")]
pub mod timer;
pub mod tx_vector;
pub mod wmm;

use channel::{Bandwidth, Channel};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use zerocopy::{Ref, Unalign};

pub use time::TimeUnit;

#[derive(Clone, Debug, PartialEq)]
pub struct RadioConfig {
    pub phy: fidl_ieee80211::WlanPhyType,
    pub channel: Channel,
}

impl From<RadioConfig> for fidl_sme::RadioConfig {
    fn from(radio_cfg: RadioConfig) -> fidl_sme::RadioConfig {
        let (cbw, _) = radio_cfg.channel.bandwidth.to_fidl();
        fidl_sme::RadioConfig {
            phy: radio_cfg.phy,
            primary: radio_cfg.channel.into(),
            bandwidth: cbw,
        }
    }
}

impl TryFrom<fidl_sme::RadioConfig> for RadioConfig {
    type Error = anyhow::Error;
    fn try_from(fidl_radio_cfg: fidl_sme::RadioConfig) -> Result<RadioConfig, Self::Error> {
        let cbw = Bandwidth::from_fidl(fidl_radio_cfg.bandwidth, 0)?;
        Ok(RadioConfig {
            phy: fidl_radio_cfg.phy,
            channel: Channel::new(fidl_radio_cfg.primary.number, cbw, fidl_radio_cfg.primary.band),
        })
    }
}

impl RadioConfig {
    pub fn new(
        phy: fidl_ieee80211::WlanPhyType,
        bandwidth: Bandwidth,
        primary_channel: u8,
        band: fidl_ieee80211::WlanBand,
    ) -> Self {
        RadioConfig { phy, channel: Channel::new(primary_channel, bandwidth, band) }
    }
}

pub type UnalignedView<B, T> = Ref<B, Unalign<T>>;
