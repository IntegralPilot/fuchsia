// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::scanner::Scanner;
use crate::client::{Context, TimedEvent};
use crate::device::DeviceOps;
use anyhow::bail;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fuchsia_async as fasync;
use futures::Future;
use wlan_common::mac::BeaconHdr;
use wlan_common::timer::EventHandle;
use wlan_common::{TimeUnit, ie};
use zerocopy::SplitByteSlice;

pub trait ChannelActions {
    fn switch_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> impl Future<Output = Result<(), zx::Status>>;
    fn schedule_channel_switch_timeout(&mut self, time: zx::MonotonicInstant) -> EventHandle;
    fn disable_scanning(&mut self) -> impl Future<Output = Result<(), zx::Status>>;
    fn enable_scanning(&mut self);
    fn disable_tx(&mut self) -> Result<(), zx::Status>;
    fn enable_tx(&mut self);
}

pub struct ChannelActionHandle<'a, D> {
    ctx: &'a mut Context<D>,
    scanner: &'a mut Scanner,
}

impl<'a, D: DeviceOps> ChannelActions for ChannelActionHandle<'a, D> {
    async fn switch_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> Result<(), zx::Status> {
        self.ctx.device.set_channel(primary, bandwidth, vht_secondary_80_channel).await
    }
    fn schedule_channel_switch_timeout(&mut self, time: zx::MonotonicInstant) -> EventHandle {
        self.ctx.timer.schedule_at(time, TimedEvent::ChannelSwitch)
    }
    async fn disable_scanning(&mut self) -> Result<(), zx::Status> {
        let mut bound_scanner = self.scanner.bind(self.ctx);
        bound_scanner.disable_scanning().await
    }
    fn enable_scanning(&mut self) {
        let mut bound_scanner = self.scanner.bind(self.ctx);
        bound_scanner.enable_scanning()
    }
    fn disable_tx(&mut self) -> Result<(), zx::Status> {
        // TODO(https://fxbug.dev/42060974): Support transmission pause.
        Err(zx::Status::NOT_SUPPORTED)
    }
    fn enable_tx(&mut self) {}
}

#[derive(Default)]
pub struct ChannelState {
    // The current main channel configured in the driver. If None, the driver may
    // be set to any channel.
    primary: Option<fidl_ieee80211::ChannelNumber>,
    bandwidth: Option<fidl_ieee80211::ChannelBandwidth>,
    vht_secondary_80_channel: Option<fidl_ieee80211::ChannelNumber>,
    pending_channel_switch: Option<(ChannelSwitch, EventHandle)>,
    beacon_interval: Option<TimeUnit>,
    last_beacon_timestamp: Option<fasync::MonotonicInstant>,
}

pub struct BoundChannelState<'a, T> {
    channel_state: &'a mut ChannelState,
    actions: T,
}

impl ChannelState {
    #[cfg(test)]
    pub fn new_with_primary_channel(primary: fidl_ieee80211::ChannelNumber) -> Self {
        Self { primary: Some(primary), ..Default::default() }
    }

    pub fn get_primary(&self) -> Option<fidl_ieee80211::ChannelNumber> {
        self.primary
    }

    pub fn get_bandwidth(&self) -> Option<fidl_ieee80211::ChannelBandwidth> {
        self.bandwidth
    }

    pub fn get_vht_secondary_80_channel(&self) -> Option<fidl_ieee80211::ChannelNumber> {
        self.vht_secondary_80_channel
    }

    pub fn bind<'a, D>(
        &'a mut self,
        ctx: &'a mut Context<D>,
        scanner: &'a mut Scanner,
    ) -> BoundChannelState<'a, ChannelActionHandle<'a, D>> {
        BoundChannelState { channel_state: self, actions: ChannelActionHandle { ctx, scanner } }
    }

    #[cfg(test)]
    pub fn test_bind<'a, T: ChannelActions>(&'a mut self, actions: T) -> BoundChannelState<'a, T> {
        BoundChannelState { channel_state: self, actions }
    }

    fn channel_switch_time_from_count(&self, channel_switch_count: u8) -> fasync::MonotonicInstant {
        let beacon_interval =
            self.beacon_interval.clone().unwrap_or(TimeUnit::DEFAULT_BEACON_INTERVAL);
        let beacon_duration = fasync::MonotonicDuration::from(beacon_interval);
        let duration = beacon_duration * channel_switch_count;
        let now = fasync::MonotonicInstant::now();
        let mut last_beacon =
            self.last_beacon_timestamp.unwrap_or_else(|| fasync::MonotonicInstant::now());
        // Calculate the theoretical latest beacon timestamp before now.
        // Note this may be larger than last_beacon_timestamp if a beacon frame was missed.
        while now - last_beacon > beacon_duration {
            last_beacon += beacon_duration;
        }
        last_beacon + duration
    }
}

impl<'a, T: ChannelActions> BoundChannelState<'a, T> {
    /// Immediately set a new main channel in the device.
    pub async fn set_main_channel(
        &mut self,
        primary: fidl_ieee80211::ChannelNumber,
        bandwidth: fidl_ieee80211::ChannelBandwidth,
        vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    ) -> Result<(), zx::Status> {
        self.channel_state.pending_channel_switch.take();
        let result =
            self.actions.switch_channel(primary, bandwidth, vht_secondary_80_channel).await;
        match result {
            Ok(()) => {
                log::info!("Switched to new main channel {:?}", primary);
                self.channel_state.primary.replace(primary);
                self.channel_state.bandwidth.replace(bandwidth);
                self.channel_state.vht_secondary_80_channel.replace(vht_secondary_80_channel);
            }
            Err(e) => {
                log::error!("Failed to switch to new main channel {:?}: {}", primary, e);
            }
        }
        self.actions.enable_scanning();
        self.actions.enable_tx();
        result
    }

    /// Clear the main channel, disable any channel switches, and return to a
    /// normal idle state. The device will remain on whichever channel was
    /// most recently configured.
    pub fn clear_main_channel(&mut self) {
        self.channel_state.primary.take();
        self.channel_state.pending_channel_switch.take();
        self.channel_state.last_beacon_timestamp.take();
        self.channel_state.beacon_interval.take();
        self.actions.enable_scanning();
        self.actions.enable_tx();
    }

    pub async fn handle_beacon(
        &mut self,
        header: &BeaconHdr,
        elements: &[u8],
    ) -> Result<(), anyhow::Error> {
        self.channel_state.last_beacon_timestamp.replace(fasync::MonotonicInstant::now());
        self.channel_state.beacon_interval.replace(header.beacon_interval);
        self.handle_channel_switch_elements_if_present(elements, false).await
    }

    pub async fn handle_announcement_frame(
        &mut self,
        elements: &[u8],
    ) -> Result<(), anyhow::Error> {
        self.handle_channel_switch_elements_if_present(elements, true).await
    }

    async fn handle_channel_switch_elements_if_present(
        &mut self,
        elements: &[u8],
        action_frame: bool,
    ) -> Result<(), anyhow::Error> {
        let current_band = self.channel_state.primary.map(|c| c.band).ok_or_else(|| {
            anyhow::anyhow!("Received channel switch announcement before main channel is set")
        })?;
        let mut csa_builder = ChannelSwitchBuilder::<&[u8]>::default();
        for (ie_type, range) in ie::IeSummaryIter::new(elements) {
            match ie_type {
                ie::IeType::CHANNEL_SWITCH_ANNOUNCEMENT => {
                    let csa = ie::parse_channel_switch_announcement(&elements[range])?;
                    csa_builder.add_channel_switch_announcement(*csa);
                }
                ie::IeType::SECONDARY_CHANNEL_OFFSET => {
                    let sco = ie::parse_sec_chan_offset(&elements[range])?;
                    csa_builder.add_secondary_channel_offset((*sco).clone());
                }
                ie::IeType::EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT => {
                    let ecsa = ie::parse_extended_channel_switch_announcement(&elements[range])?;
                    csa_builder.add_extended_channel_switch_announcement(*ecsa);
                }
                ie::IeType::CHANNEL_SWITCH_WRAPPER => {
                    let csw = ie::parse_channel_switch_wrapper(&elements[range])?;
                    csa_builder.add_channel_switch_wrapper(csw);
                }
                ie::IeType::WIDE_BANDWIDTH_CHANNEL_SWITCH if action_frame => {
                    let wbcs = ie::parse_wide_bandwidth_channel_switch(&elements[range])?;
                    csa_builder.add_wide_bandwidth_channel_switch((*wbcs).clone());
                }
                ie::IeType::TRANSMIT_POWER_ENVELOPE if action_frame => {
                    let tpe = ie::parse_transmit_power_envelope(&elements[range])?;
                    csa_builder.add_transmit_power_envelope(tpe);
                }
                _ => (),
            }
        }
        match csa_builder.build(current_band) {
            ChannelSwitchResult::ChannelSwitch(cs) => self.handle_channel_switch(cs).await,
            ChannelSwitchResult::NoChannelSwitch => Ok(()),
            ChannelSwitchResult::Error(err) => Err(err.into()),
        }
    }

    async fn handle_channel_switch(
        &mut self,
        channel_switch: ChannelSwitch,
    ) -> Result<(), anyhow::Error> {
        if !channel_switch.compatible() {
            bail!("Incompatible channel switch announcement received.");
        }

        self.actions.disable_scanning().await?;
        if channel_switch.channel_switch_count == 0 {
            self.set_main_channel(
                channel_switch.primary,
                channel_switch.bandwidth,
                channel_switch.vht_secondary_80_channel,
            )
            .await
            .map_err(|e| e.into())
        } else {
            if channel_switch.pause_transmission {
                // TODO(b/254334420): Determine if this should be fatal to the switch.
                self.actions.disable_tx()?;
            }
            let time = self
                .channel_state
                .channel_switch_time_from_count(channel_switch.channel_switch_count);
            let event_id = self.actions.schedule_channel_switch_timeout(time.into());
            self.channel_state.pending_channel_switch.replace((channel_switch, event_id));
            Ok(())
        }
    }

    pub async fn handle_channel_switch_timeout(&mut self) -> Result<(), anyhow::Error> {
        if let Some((channel_switch, _handle)) = self.channel_state.pending_channel_switch.take() {
            self.set_main_channel(
                channel_switch.primary,
                channel_switch.bandwidth,
                channel_switch.vht_secondary_80_channel,
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub struct ChannelSwitch {
    pub channel_switch_count: u8,
    pub primary: fidl_ieee80211::ChannelNumber,
    pub bandwidth: fidl_ieee80211::ChannelBandwidth,
    pub vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
    pub pause_transmission: bool,
    pub new_operating_class: Option<u8>,
    // TODO(https://fxbug.dev/42180124): Support transmit power envelope.
    pub new_transmit_power_envelope_specified: bool,
}

impl ChannelSwitch {
    // TODO(https://fxbug.dev/42180124): Support channel switch related feature queries.
    /// Determines whether this ChannelSwitch can be performed by the driver.
    fn compatible(&self) -> bool {
        self.new_operating_class.is_none()
            && !self.new_transmit_power_envelope_specified
            && !self.pause_transmission
    }
}

#[derive(Default)]
pub struct ChannelSwitchBuilder<B> {
    channel_switch: Option<ie::ChannelSwitchAnnouncement>,
    secondary_channel_offset: Option<ie::SecChanOffset>,
    extended_channel_switch: Option<ie::ExtendedChannelSwitchAnnouncement>,
    new_country: Option<ie::CountryView<B>>,
    wide_bandwidth_channel_switch: Option<ie::WideBandwidthChannelSwitch>,
    transmit_power_envelope: Option<ie::TransmitPowerEnvelopeView<B>>,
}

#[derive(Debug)]
pub enum ChannelSwitchResult {
    ChannelSwitch(ChannelSwitch),
    NoChannelSwitch,
    Error(ChannelSwitchError),
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelSwitchError {
    #[error("Frame contains multiple channel switch elements with conflicting information.")]
    ConflictingElements,
    #[error("Invalid channel switch mode {}", _0)]
    InvalidChannelSwitchMode(u8),
}

impl<B: SplitByteSlice> ChannelSwitchBuilder<B> {
    // Convert a set of received channel-switch-related IEs into the parameters
    // for a channel switch. Returns an error if the IEs received do not describe
    // a deterministic, valid channel switch.
    pub fn build(self, current_band: fidl_ieee80211::WlanBand) -> ChannelSwitchResult {
        // Extract shared information from the channel switch or extended channel switch elements
        // present. If both are present we check that they agree on the destination channel and then
        // use the CSA instead of the ECSA. This decision is to avoid specifying a
        // new_operating_class wherever possible, since operating class switches are unsupported.
        let (mode, new_channel_number, channel_switch_count, new_operating_class, band) =
            if let Some(csa) = self.channel_switch {
                let band = if let Some(ecsa) = self.extended_channel_switch {
                    // If both CSA and ECSA elements are present, make sure they match.
                    if csa.new_channel_number != ecsa.new_channel_number {
                        return ChannelSwitchResult::Error(ChannelSwitchError::ConflictingElements);
                    }
                    let band = ecsa.get_band();
                    if band.is_unknown() { current_band } else { band }
                } else {
                    current_band
                };
                // IEEE Std 802.11-2016 11.9.8 describes the operation of a CSA.
                (csa.mode, csa.new_channel_number, csa.channel_switch_count, None, band)
            } else if let Some(ecsa) = self.extended_channel_switch {
                let band = ecsa.get_band();
                let band = if band.is_unknown() { current_band } else { band };
                // IEEE Std 802.11-2016 11.10 describes the operation of an extended CSA.
                (
                    ecsa.mode,
                    ecsa.new_channel_number,
                    ecsa.channel_switch_count,
                    Some(ecsa.new_operating_class),
                    band,
                )
            } else {
                return ChannelSwitchResult::NoChannelSwitch;
            };

        let pause_transmission = match mode {
            1 => true,
            0 => false,
            other => {
                return ChannelSwitchResult::Error(ChannelSwitchError::InvalidChannelSwitchMode(
                    other,
                ));
            }
        };

        // IEEE Std 802.11-2016 9.4.2.159 Table 9-252 specifies that wide bandwidth channel switch
        // elements are treated identically to those in a VHT element.
        let vht_cbw_and_segs = self
            .wide_bandwidth_channel_switch
            .map(|wbcs| (wbcs.new_width, wbcs.new_center_freq_seg0, wbcs.new_center_freq_seg1));
        let sec_chan_offset =
            self.secondary_channel_offset.unwrap_or(ie::SecChanOffset::SECONDARY_NONE);
        let (bandwidth, secondary80_num) =
            wlan_common::channel::derive_wide_channel_bandwidth(vht_cbw_and_segs, sec_chan_offset)
                .to_fidl();
        let vht_secondary_80_channel =
            fidl_ieee80211::ChannelNumber { band, number: secondary80_num };

        ChannelSwitchResult::ChannelSwitch(ChannelSwitch {
            channel_switch_count: channel_switch_count,
            primary: fidl_ieee80211::ChannelNumber { band, number: new_channel_number },
            bandwidth,
            vht_secondary_80_channel,
            pause_transmission,
            new_operating_class,
            new_transmit_power_envelope_specified: self.transmit_power_envelope.is_some(),
        })
    }

    pub fn add_channel_switch_announcement(&mut self, csa: ie::ChannelSwitchAnnouncement) {
        self.channel_switch.replace(csa);
    }

    pub fn add_secondary_channel_offset(&mut self, sco: ie::SecChanOffset) {
        self.secondary_channel_offset.replace(sco);
    }

    pub fn add_extended_channel_switch_announcement(
        &mut self,
        ecsa: ie::ExtendedChannelSwitchAnnouncement,
    ) {
        self.extended_channel_switch.replace(ecsa);
    }

    pub fn add_wide_bandwidth_channel_switch(&mut self, wbcs: ie::WideBandwidthChannelSwitch) {
        self.wide_bandwidth_channel_switch.replace(wbcs);
    }

    pub fn add_transmit_power_envelope(&mut self, tpe: ie::TransmitPowerEnvelopeView<B>) {
        self.transmit_power_envelope.replace(tpe);
    }

    pub fn add_channel_switch_wrapper(&mut self, csw: ie::ChannelSwitchWrapperView<B>) {
        csw.new_country.map(|new_country| self.new_country.replace(new_country));
        csw.new_transmit_power_envelope.map(|tpe| self.add_transmit_power_envelope(tpe));
        csw.wide_bandwidth_channel_switch.map(|wbcs| self.add_wide_bandwidth_channel_switch(*wbcs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use wlan_common::mac::CapabilityInfo;
    use wlan_common::timer::EventId;

    const NEW_CHANNEL: u8 = 10;
    const NEW_OPERATING_CLASS: u8 = 81;
    const COUNT: u8 = 30;

    const CHANNEL_SWITCH_ANNOUNCEMENT_HEADER: &[u8] = &[37, 3];

    fn csa(
        mode: u8,
        new_channel_number: u8,
        channel_switch_count: u8,
    ) -> ie::ChannelSwitchAnnouncement {
        ie::ChannelSwitchAnnouncement { mode, new_channel_number, channel_switch_count }
    }

    fn csa_bytes(mode: u8, new_channel_number: u8, channel_switch_count: u8) -> Vec<u8> {
        let mut elements = vec![];
        elements.extend(CHANNEL_SWITCH_ANNOUNCEMENT_HEADER);
        elements.extend(&[mode, new_channel_number, channel_switch_count]);
        elements
    }

    fn ecsa(
        mode: u8,
        new_operating_class: u8,
        new_channel_number: u8,
        channel_switch_count: u8,
    ) -> ie::ExtendedChannelSwitchAnnouncement {
        ie::ExtendedChannelSwitchAnnouncement {
            mode,
            new_operating_class,
            new_channel_number,
            channel_switch_count,
        }
    }

    fn wbcs(seg0: u8, seg1: u8) -> ie::WideBandwidthChannelSwitch {
        ie::WideBandwidthChannelSwitch {
            new_width: ie::VhtChannelBandwidth::CBW_80_160_80P80,
            new_center_freq_seg0: seg0,
            new_center_freq_seg1: seg1,
        }
    }

    #[test_case(Some(NEW_OPERATING_CLASS), false, false ; "when operating class present")]
    #[test_case(None, true, false ; "when new TPE present")]
    #[test_case(Some(NEW_OPERATING_CLASS), true, false ; "when operating class and new TPE present")]
    #[test_case(None, false, true ; "when operating class and new TPE absent")]
    #[fuchsia::test]
    fn channel_switch_compatible(
        new_operating_class: Option<u8>,
        new_transmit_power_envelope_specified: bool,
        expected_compatible: bool,
    ) {
        let channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission: false,
            new_operating_class,
            new_transmit_power_envelope_specified,
        };
        assert_eq!(channel_switch.compatible(), expected_compatible);
    }

    #[test]
    fn empty_builder_returns_no_csa() {
        let builder = ChannelSwitchBuilder::<&[u8]>::default();
        assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::NoChannelSwitch
        );
    }

    #[test_case(0, false ; "when transmission is not paused")]
    #[test_case(1, true ; "when transmission is paused")]
    #[fuchsia::test]
    fn basic_csa_20mhz(mode: u8, pause_transmission: bool) {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(mode, NEW_CHANNEL, COUNT));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::FiveGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                number: 0,
            },
            pause_transmission,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test_case(0, false ; "when transmission is not paused")]
    #[test_case(1, true ; "when transmission is paused")]
    #[fuchsia::test]
    fn basic_ecsa_20mhz(mode: u8, pause_transmission: bool) {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_extended_channel_switch_announcement(ecsa(
            mode,
            NEW_OPERATING_CLASS,
            NEW_CHANNEL,
            COUNT,
        ));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::FiveGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission,
            new_operating_class: Some(NEW_OPERATING_CLASS),
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test]
    fn basic_csa_40mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(0, NEW_CHANNEL, COUNT));
        builder.add_secondary_channel_offset(ie::SecChanOffset::SECONDARY_ABOVE);
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw40,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission: false,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test]
    fn basic_csa_80mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(0, NEW_CHANNEL, COUNT));
        builder.add_secondary_channel_offset(ie::SecChanOffset::SECONDARY_ABOVE);
        builder.add_wide_bandwidth_channel_switch(wbcs(NEW_CHANNEL + 8, 0));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw80,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission: false,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test]
    fn basic_csa_160mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(0, NEW_CHANNEL, COUNT));
        builder.add_secondary_channel_offset(ie::SecChanOffset::SECONDARY_ABOVE);
        builder.add_wide_bandwidth_channel_switch(wbcs(NEW_CHANNEL + 8, NEW_CHANNEL + 16));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw160,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission: false,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test]
    fn basic_csa_80p80mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(0, NEW_CHANNEL, COUNT));
        builder.add_secondary_channel_offset(ie::SecChanOffset::SECONDARY_ABOVE);
        builder.add_wide_bandwidth_channel_switch(wbcs(NEW_CHANNEL + 8, NEW_CHANNEL + 100));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw80P80,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL + 100,
            },
            pause_transmission: false,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test_case(0, false ; "when transmission is not paused")]
    #[test_case(1, true ; "when transmission is paused")]
    #[fuchsia::test]
    fn mixed_csa_ecsa_20mhz(mode: u8, pause_transmission: bool) {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(mode, NEW_CHANNEL, COUNT));
        builder.add_extended_channel_switch_announcement(ecsa(
            mode,
            NEW_OPERATING_CLASS,
            NEW_CHANNEL,
            COUNT,
        ));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::FiveGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        let expected_channel_switch = ChannelSwitch {
            channel_switch_count: COUNT,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: NEW_CHANNEL,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            pause_transmission,
            new_operating_class: None,
            new_transmit_power_envelope_specified: false,
        };
        assert_eq!(channel_switch, expected_channel_switch);
    }

    #[test]
    fn mixed_csa_ecsa_mismatch_20mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(0, NEW_CHANNEL, COUNT));
        let mut ecsa = ecsa(0, NEW_OPERATING_CLASS, NEW_CHANNEL, COUNT);
        ecsa.new_channel_number += 1;
        builder.add_extended_channel_switch_announcement(ecsa);
        let err = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::Error(err) => err
        );
        assert_matches!(err, ChannelSwitchError::ConflictingElements);
    }

    #[test]
    fn basic_csa_invalid_mode_20mhz() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_channel_switch_announcement(csa(123, NEW_CHANNEL, COUNT));
        let err = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::TwoGhz),
            ChannelSwitchResult::Error(err) => err
        );
        assert_matches!(err, ChannelSwitchError::InvalidChannelSwitchMode(123));
    }

    #[test]
    fn basic_ecsa_invalid_op_class_fallback_to_current_band() {
        let mut builder = ChannelSwitchBuilder::<&[u8]>::default();
        builder.add_extended_channel_switch_announcement(ecsa(0, 20, NEW_CHANNEL, COUNT));
        let channel_switch = assert_matches!(
            builder.build(fidl_ieee80211::WlanBand::FiveGhz),
            ChannelSwitchResult::ChannelSwitch(cs) => cs
        );
        assert_eq!(channel_switch.primary.band, fidl_ieee80211::WlanBand::FiveGhz);
    }

    #[derive(Default)]
    struct MockChannelActions {
        actions: Vec<ChannelAction>,
        event_id_ctr: EventId,
    }

    #[derive(Debug, Copy, Clone)]
    enum ChannelAction {
        SwitchChannel(
            fidl_ieee80211::ChannelNumber,
            fidl_ieee80211::ChannelBandwidth,
            fidl_ieee80211::ChannelNumber,
        ),
        Timeout(EventId, fasync::MonotonicInstant),
        DisableScanning,
        EnableScanning,
        DisableTx,
        EnableTx,
    }

    impl ChannelActions for &mut MockChannelActions {
        async fn switch_channel(
            &mut self,
            primary: fidl_ieee80211::ChannelNumber,
            bandwidth: fidl_ieee80211::ChannelBandwidth,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber,
        ) -> Result<(), zx::Status> {
            self.actions.push(ChannelAction::SwitchChannel(
                primary,
                bandwidth,
                vht_secondary_80_channel,
            ));
            Ok(())
        }
        fn schedule_channel_switch_timeout(&mut self, time: zx::MonotonicInstant) -> EventHandle {
            self.event_id_ctr += 1;
            self.actions.push(ChannelAction::Timeout(self.event_id_ctr, time.into()));
            EventHandle::new_test(self.event_id_ctr)
        }
        async fn disable_scanning(&mut self) -> Result<(), zx::Status> {
            self.actions.push(ChannelAction::DisableScanning);
            Ok(())
        }
        fn enable_scanning(&mut self) {
            self.actions.push(ChannelAction::EnableScanning);
        }
        fn disable_tx(&mut self) -> Result<(), zx::Status> {
            self.actions.push(ChannelAction::DisableTx);
            Ok(())
        }
        fn enable_tx(&mut self) {
            self.actions.push(ChannelAction::EnableTx);
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn channel_state_ignores_empty_beacon_frame() {
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let mut actions = MockChannelActions::default();
        let header = BeaconHdr::new(TimeUnit(10), CapabilityInfo(0));
        let elements = [];
        channel_state
            .test_bind(&mut actions)
            .handle_beacon(&header, &elements[..])
            .await
            .expect("Failed to handle beacon");

        assert!(actions.actions.is_empty());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn channel_state_handles_immediate_csa_in_beacon_frame() {
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });

        let mut actions = MockChannelActions::default();
        let header = BeaconHdr::new(TimeUnit(10), CapabilityInfo(0));
        let mut elements = vec![];
        elements.extend(csa_bytes(0, NEW_CHANNEL, 0));
        channel_state
            .test_bind(&mut actions)
            .handle_beacon(&header, &elements[..])
            .await
            .expect("Failed to handle beacon");

        assert_eq!(actions.actions.len(), 4);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (primary, bandwidth, vht_secondary_80_channel) = assert_matches!(actions.actions[1], ChannelAction::SwitchChannel(chan, bw, s80) => (chan, bw, s80));
        assert_eq!(primary.number, NEW_CHANNEL);
        assert_eq!(bandwidth, fidl_ieee80211::ChannelBandwidth::Cbw20);
        assert_eq!(
            vht_secondary_80_channel,
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 0 }
        );
        assert_matches!(actions.actions[2], ChannelAction::EnableScanning);
        assert_matches!(actions.actions[3], ChannelAction::EnableTx);
    }

    #[test]
    fn channel_state_handles_delayed_csa_in_beacon_frame() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let bcn_header = BeaconHdr::new(TimeUnit(10), CapabilityInfo(0));
        let mut time = fasync::MonotonicInstant::from_nanos(0);
        exec.set_fake_time(time);
        let mut actions = MockChannelActions::default();

        // First channel switch announcement (count = 2)
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = csa_bytes(0, NEW_CHANNEL, 2);
            let fut = bound_channel_state.handle_beacon(&bcn_header, &elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle beacon"
            );
        }
        assert_eq!(actions.actions.len(), 2);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (_first_event_id, event_time) =
            assert_matches!(actions.actions[1], ChannelAction::Timeout(eid, time) => (eid, time));
        assert_eq!(event_time, (time + (bcn_header.beacon_interval * 2u16).into()).into());
        actions.actions.clear();

        time += bcn_header.beacon_interval.into();
        exec.set_fake_time(time);

        // Second channel switch announcement (count = 1)
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = csa_bytes(0, NEW_CHANNEL, 1);
            let fut = bound_channel_state.handle_beacon(&bcn_header, &elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle beacon"
            );
        }
        assert_eq!(actions.actions.len(), 2);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (_second_event_id, event_time) =
            assert_matches!(actions.actions[1], ChannelAction::Timeout(eid, time) => (eid, time));
        assert_eq!(event_time, (time + bcn_header.beacon_interval.into()).into());
        actions.actions.clear();

        time += bcn_header.beacon_interval.into();
        exec.set_fake_time(time);

        // Timeout results in completion.
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let fut = bound_channel_state.handle_channel_switch_timeout();
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle channel switch timeout"
            );
        }

        assert_eq!(actions.actions.len(), 3);
        let (primary, bandwidth, vht_secondary_80_channel) = assert_matches!(actions.actions[0], ChannelAction::SwitchChannel(chan, bw, s80) => (chan, bw, s80));
        assert_eq!(primary.number, NEW_CHANNEL);
        assert_eq!(bandwidth, fidl_ieee80211::ChannelBandwidth::Cbw20);
        assert_eq!(
            vht_secondary_80_channel,
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 0 }
        );
        assert_matches!(actions.actions[1], ChannelAction::EnableScanning);
        assert_matches!(actions.actions[2], ChannelAction::EnableTx);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn channel_state_cannot_pause_tx() {
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let bcn_header = BeaconHdr::new(TimeUnit(10), CapabilityInfo(0));
        let mut actions = MockChannelActions::default();

        channel_state
            .test_bind(&mut actions)
            .handle_beacon(&bcn_header, &csa_bytes(1, NEW_CHANNEL, 2)[..])
            .await
            .expect_err("Shouldn't handle channel switch with tx pause");
        assert_eq!(actions.actions.len(), 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn channel_state_cannot_parse_malformed_csa() {
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let bcn_header = BeaconHdr::new(TimeUnit(10), CapabilityInfo(0));
        let mut actions = MockChannelActions::default();

        let mut element = vec![];
        element.extend(CHANNEL_SWITCH_ANNOUNCEMENT_HEADER);
        element.extend(&[10, 0, 0][..]); // Garbage info.
        channel_state
            .test_bind(&mut actions)
            .handle_beacon(&bcn_header, &element[..])
            .await
            .expect_err("Should not handle malformed beacon");
        assert_eq!(actions.actions.len(), 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn channel_state_handles_immediate_csa_in_action_frame() {
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });

        let mut actions = MockChannelActions::default();
        channel_state
            .test_bind(&mut actions)
            .handle_announcement_frame(&csa_bytes(0, NEW_CHANNEL, 0)[..])
            .await
            .expect("Failed to handle beacon");

        assert_eq!(actions.actions.len(), 4);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (primary, bandwidth, vht_secondary_80_channel) = assert_matches!(actions.actions[1], ChannelAction::SwitchChannel(chan, bw, s80) => (chan, bw, s80));
        assert_eq!(primary.number, NEW_CHANNEL);
        assert_eq!(bandwidth, fidl_ieee80211::ChannelBandwidth::Cbw20);
        assert_eq!(
            vht_secondary_80_channel,
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 0 }
        );
        assert_matches!(actions.actions[2], ChannelAction::EnableScanning);
        assert_matches!(actions.actions[3], ChannelAction::EnableTx);
    }

    #[test]
    fn channel_state_handles_delayed_csa_in_announcement_frame() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let bcn_header = BeaconHdr::new(TimeUnit(100), CapabilityInfo(0));
        let bcn_time: fasync::MonotonicInstant =
            fasync::MonotonicInstant::from_nanos(0) + bcn_header.beacon_interval.into();
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let mut actions = MockChannelActions::default();

        // Empty beacon frame to configure beacon parameters.
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = [];
            let fut = bound_channel_state.handle_beacon(&bcn_header, &elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle beacon"
            );
        }
        assert!(actions.actions.is_empty());

        // CSA action frame arrives some time between beacons.
        exec.set_fake_time(bcn_time - fasync::MonotonicDuration::from_micros(500));
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = csa_bytes(0, NEW_CHANNEL, 1);
            let fut = bound_channel_state.handle_announcement_frame(&elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle announcement"
            );
        }
        assert_eq!(actions.actions.len(), 2);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (_event_id, event_time) =
            assert_matches!(actions.actions[1], ChannelAction::Timeout(eid, time) => (eid, time));
        assert_eq!(event_time, bcn_time);
        actions.actions.clear();

        // Timeout arrives.
        exec.set_fake_time(bcn_time);
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let fut = bound_channel_state.handle_channel_switch_timeout();
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle channel switch timeout"
            );
        }
        assert_eq!(actions.actions.len(), 3);
        let (primary, bandwidth, vht_secondary_80_channel) = assert_matches!(actions.actions[0], ChannelAction::SwitchChannel(chan, bw, s80) => (chan, bw, s80));
        assert_eq!(primary.number, NEW_CHANNEL);
        assert_eq!(bandwidth, fidl_ieee80211::ChannelBandwidth::Cbw20);
        assert_eq!(
            vht_secondary_80_channel,
            fidl_ieee80211::ChannelNumber { band: fidl_ieee80211::WlanBand::TwoGhz, number: 0 }
        );
        assert_matches!(actions.actions[1], ChannelAction::EnableScanning);
        assert_matches!(actions.actions[2], ChannelAction::EnableTx);
    }

    #[test]
    fn channel_state_handles_delayed_csa_in_announcement_frame_with_missed_beacon() {
        let mut exec = fasync::TestExecutor::new_with_fake_time();
        let mut channel_state =
            ChannelState::new_with_primary_channel(fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            });
        let bcn_header = BeaconHdr::new(TimeUnit(100), CapabilityInfo(0));
        exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));
        let mut actions = MockChannelActions::default();

        // Empty beacon frame to configure beacon parameters.
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = [];
            let fut = bound_channel_state.handle_beacon(&bcn_header, &elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle beacon"
            );
        }
        assert!(actions.actions.is_empty());

        // Advance time by a bit more than one beacon, simulating a missed frame.
        exec.set_fake_time(
            fasync::MonotonicInstant::from_nanos(0)
                + bcn_header.beacon_interval.into()
                + fasync::MonotonicDuration::from_micros(500),
        );

        // CSA action frame arrives after the missed beacon.
        {
            let mut bound_channel_state = channel_state.test_bind(&mut actions);
            let elements = csa_bytes(0, NEW_CHANNEL, 1);
            let fut = bound_channel_state.handle_announcement_frame(&elements[..]);
            let mut fut = pin!(fut);
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Ok(_)),
                "Failed to handle announcement"
            );
        }
        assert_eq!(actions.actions.len(), 2);
        assert_matches!(actions.actions[0], ChannelAction::DisableScanning);
        let (_event_id, event_time) =
            assert_matches!(actions.actions[1], ChannelAction::Timeout(eid, time) => (eid, time));
        // The CSA should be timed based on our best estimate of the missed beacon.
        assert_eq!(
            event_time,
            fasync::MonotonicInstant::from_nanos(0) + (bcn_header.beacon_interval * 2u16).into()
        );
    }
}
