// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Configurable and common handlers that act on PHYs in response to events.
//!
//! This module provides recipes for event handlers in hardware simulator tests. Handlers are
//! constructed using functions and always emit [`ActionResult`]s as output. These recipes can be
//! composed to form complex event handlers using terse and declarative syntax.
//!
//! # Examples
//!
//! Actions are exposed as functions that construct event handlers. The following example
//! demonstrates constructing a client event handler that scans and transmits packets to an AP.
//!
//! ```rust,ignore
//! let advertisements = [ProbeResponse { /* ... */ }];
//! let mut handler = branch::or((
//!     event::on_scan(action::send_advertisements_and_scan_completion(&client, advertisements)),
//!     event::on_transmit(action::send_packet(&ap, rx_info_with_default_ap())),
//! ));
//! ```
//!
//! A more complex action handles client association with an authentication tap for controlling the
//! authentication process. The following example constructs such a handler.
//!
//! ```rust,ignore
//! let beacons = [Beacon { /* ... */ }];
//! let control = AuthenticationControl {
//!     updates: UpdateSink::new(),
//!     authenticator: /* ... */,
//! };
//! let tap = AuthenticationTap {
//!     control: &mut control,
//!     // This event handler reacts to `AuthenticationEvent`s and is passed the
//!     // `AuthenticationControl` as state.
//!     handler: action::authenticate_with_control_state(),
//! };
//! let mut handler = branch::or((
//!     event::on_scan(action::send_advertisements_and_scan_completion(&client, beacons)),
//!     event::on_transmit(action::connect_with_authentication_tap(
//!         &client, &ssid, &bssid, &channel, &protection, tap,
//!     )),
//! ));
//! ```

use anyhow::{Context, bail};
use ieee80211::{Bssid, MacAddrBytes, Ssid};
use log::{debug, info};
use wlan_common::bss::Protection;
use wlan_common::channel::Channel;
use wlan_common::mac;
use wlan_rsn::rsna::UpdateSink;
use wlan_rsn::{Authenticator, auth};
use {
    fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211, fidl_fuchsia_wlan_mlme as fidl_mlme,
    fidl_fuchsia_wlan_tap as fidl_tap,
};

use crate::event::buffered::{AssocReqFrame, AuthFrame, Buffered, DataFrame, ProbeReqFrame};
use crate::event::{self, Handler, Stateful, branch};
use crate::{ApAdvertisement, CLIENT_MAC_ADDR, ProbeResponse};

/// The result of an action event handler.
///
/// All event handlers provided in this module emit this type as their output.
pub type ActionResult = Result<(), anyhow::Error>;

/// Authentication state and handler used to tap into the client authentication process in an event
/// handler.
///
/// See [`connect_with_authentication_tap`].
///
/// [`connect_with_authentication_tap`]: crate::event::action::connect_with_authentication_tap
#[derive(Debug)]
pub struct AuthenticationTap<'a, H> {
    pub control: &'a mut AuthenticationControl,
    /// An event handler that authenticates a client with an AP.
    ///
    /// The event handler must accept [`AuthenticationControl`] state and [`AuthenticationEvent`]
    /// events.
    ///
    /// [`AuthenticationControl`]: crate::event::action::AuthenticationControl
    /// [`AuthenticationEvent`]: crate::event::action::AuthenticationEvent
    pub handler: H,
}

impl<'a, H> AuthenticationTap<'a, H> {
    fn call<'e>(&mut self, event: &AuthenticationEvent<'e>) -> ActionResult
    where
        H: Handler<AuthenticationControl, AuthenticationEvent<'e>, Output = ActionResult>,
    {
        self.handler
            .by_ref()
            .context("failed to handle authentication event")
            .call(self.control, event)
            .matched()
            .unwrap_or(Ok(()))
    }
}

/// An authentication event that occurs as part of a [`TxArgs`].
///
/// See [`AuthenticationTap`].
///
/// [`AuthenticationTap`]: crate::event::action::AuthenticationTap
/// [`TxArgs`]: fidl_fuchsia_wlan_tap::TxArgs
#[derive(Debug)]
pub struct AuthenticationEvent<'a> {
    pub phy: &'a fidl_tap::WlantapPhyProxy,
    pub bssid: &'a Bssid,
    pub channel: &'a Channel,
    pub is_ready_for_sae: bool,
    pub is_ready_for_eapol: bool,
}

/// Authenticator (protocol, credentials, etc.) and message buffer used for client authentication.
#[derive(Debug)]
pub struct AuthenticationControl {
    pub authenticator: Authenticator,
    pub updates: UpdateSink,
}

pub fn send_advertisements<'h, S, E, I>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    advertisements: I,
) -> impl Handler<S, E, Output = ActionResult> + 'h
where
    S: 'h,
    E: 'h,
    I: IntoIterator,
    I::Item: ApAdvertisement + 'h,
{
    let advertisements: Vec<_> = advertisements.into_iter().collect();
    event::matched(move |_: &mut S, _: &E| {
        for advertisement in advertisements.iter() {
            advertisement.send(phy).context("failed to send AP advertisement")?;
        }
        Ok(())
    })
}

pub fn send_scan_completion<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    status: i32,
) -> impl Handler<S, fidl_tap::StartScanArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    use zx::MonotonicDuration;

    const SCAN_COMPLETION_DELAY: MonotonicDuration = MonotonicDuration::from_seconds(2i64);

    event::matched(move |_: &mut S, event: &fidl_tap::StartScanArgs| {
        log::info!(
            "TODO(https://fxbug.dev/42060050): Sleeping for {} second(s) before sending scan completion.",
            SCAN_COMPLETION_DELAY.into_seconds()
        );
        SCAN_COMPLETION_DELAY.sleep();
        phy.scan_complete(event.scan_id, status).context("failed to send scan completion")
    })
}

pub fn send_advertisements_and_scan_completion<'h, S, I>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    advertisements: I,
) -> impl Handler<S, fidl_tap::StartScanArgs, Output = ActionResult> + 'h
where
    S: 'h,
    I: IntoIterator,
    I::Item: ApAdvertisement + 'h,
{
    event::matched(|_, event: &fidl_tap::StartScanArgs| {
        debug!(
            "Sending AP advertisements and scan completion for scan event with ID: {:?}",
            event.scan_id,
        );
    })
    .and(send_advertisements(phy, advertisements))
    .try_and(send_scan_completion(phy, 0))
}

pub fn send_probe_response<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    ssid: &'h Ssid,
    bssid: &'h Bssid,
    channel: &'h Channel,
    protection: &'h Protection,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    event::extract(|_: Buffered<ProbeReqFrame>| {
        // NOTE: Normally the AP only sends probe responses on its channel, but hardware simulator
        //       does not support this at time of writing. This does not (yet) affect tests.
        ProbeResponse {
            channel: *channel,
            bssid: *bssid,
            ssid: ssid.clone(),
            protection: *protection,
            rssi_dbm: -10,
            wsc_ie: None,
        }
        .send(phy)
        .context("failed to send probe response frame")
    })
}

pub fn send_packet<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    rx_info: fidl_tap::WlanRxInfo,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    event::matched(move |_: &mut S, event: &fidl_tap::TxArgs| {
        phy.rx(&event.packet.data, &rx_info).context("failed to send packet")
    })
}

pub fn send_open_authentication<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    bssid: &'h Bssid,
    channel: &'h Channel,
    status: impl Into<mac::StatusCode>,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    let status = status.into();
    event::extract(move |_: Buffered<AuthFrame>| {
        crate::send_open_authentication(channel, bssid, status, phy)
            .context("failed to send authentication frame")
    })
}

// TODO(https://fxbug.dev/42080468): Preserve the updates in the sink for logging, inspection, etc.
pub fn authenticate_with_control_state<'h>()
-> impl Handler<AuthenticationControl, AuthenticationEvent<'h>, Output = ActionResult> + 'h {
    event::matched(|control: &mut AuthenticationControl, event: &AuthenticationEvent<'_>| {
        use wlan_rsn::rsna::SecAssocUpdate::{TxEapolKeyFrame, TxSaeFrame};

        let mut index = 0;
        while index < control.updates.len() {
            match &control.updates[index] {
                TxSaeFrame(frame) => {
                    if event.is_ready_for_sae {
                        crate::send_sae_authentication_frame(
                            &frame,
                            &event.channel,
                            &event.bssid,
                            event.phy,
                        )
                        .context("failed to send SAE authentication frame")?;
                        control.updates.remove(index);
                        continue; // The update has been removed: do NOT increment the index.
                    } else {
                        debug!("authentication: received unexpected SAE frame");
                    }
                }
                TxEapolKeyFrame { frame, .. } => {
                    if event.is_ready_for_eapol {
                        crate::rx_wlan_data_frame(
                            &event.channel,
                            &CLIENT_MAC_ADDR,
                            &event.bssid.clone().into(),
                            &event.bssid.clone().into(),
                            &frame[..],
                            mac::ETHER_TYPE_EAPOL,
                            event.phy,
                        )?;
                        control.updates.remove(index);
                        control
                            .authenticator
                            .on_eapol_conf(
                                &mut control.updates,
                                fidl_mlme::EapolResultCode::Success,
                            )
                            .context("failed to send EAPOL confirm")?;
                        continue; // The update has been removed: do NOT increment the index.
                    } else {
                        debug!("authentication: received unexpected EAPOL key frame");
                    }
                }
                _ => {}
            }
            index += 1;
        }
        Ok(())
    })
}

pub fn send_association_response<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    bssid: &'h Bssid,
    channel: &'h Channel,
    status: impl Into<mac::StatusCode>,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    let status = status.into();
    event::extract(move |_: Buffered<AssocReqFrame>| {
        crate::send_association_response(channel, bssid, status, phy)
            .context("failed to send association response frame")
    })
}

pub fn connect_with_open_authentication<'h, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    ssid: &'h Ssid,
    bssid: &'h Bssid,
    channel: &'h Channel,
    protection: &'h Protection,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    S: 'h,
{
    branch::or((
        send_open_authentication(phy, bssid, channel, fidl_ieee80211::StatusCode::Success),
        send_association_response(phy, bssid, channel, fidl_ieee80211::StatusCode::Success),
        send_probe_response(phy, ssid, bssid, channel, protection),
    ))
}

pub fn connect_with_authentication_tap<'h, H, S>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    ssid: &'h Ssid,
    bssid: &'h Bssid,
    channel: &'h Channel,
    protection: &'h Protection,
    tap: AuthenticationTap<'h, H>,
) -> impl Handler<S, fidl_tap::TxArgs, Output = ActionResult> + 'h
where
    H: Handler<AuthenticationControl, AuthenticationEvent<'h>, Output = ActionResult> + 'h,
    S: 'h,
{
    type Tap<'a, H> = AuthenticationTap<'a, H>;

    let authenticate =
        event::extract(Stateful(|tap: &mut Tap<'_, H>, frame: Buffered<AuthFrame>| {
            let frame = frame.get();
            match frame.auth_hdr.auth_alg_num {
                mac::AuthAlgorithmNumber::OPEN => {
                    if matches!(
                        tap.control.authenticator,
                        Authenticator { auth_cfg: auth::Config::ComputedPsk(_), .. },
                    ) {
                        crate::send_open_authentication(
                            channel,
                            bssid,
                            fidl_ieee80211::StatusCode::Success,
                            phy,
                        )
                        .context("failed to send open authentication frame")?;
                        tap.call(&AuthenticationEvent {
                            phy,
                            bssid,
                            channel,
                            is_ready_for_sae: false,
                            is_ready_for_eapol: false,
                        })
                    } else {
                        bail!("open authentication frame is incompatible with authenticator");
                    }
                }
                mac::AuthAlgorithmNumber::SAE => {
                    info!("auth_txn_seq_num: {}", { frame.auth_hdr.auth_txn_seq_num });
                    // Reset the authenticator and clear the update sink so that the PMK is not
                    // observed from the first connection attempt. The SAE handshake uses multiple
                    // frames so this reset only occurs on the first authentication frame.
                    // NOTE: A different approach is needed to test the retransmission of the first
                    //       authentication frame.
                    if frame.auth_hdr.auth_txn_seq_num == 1 {
                        tap.control.authenticator.reset();
                        tap.control.updates.clear();
                    }

                    tap.control
                        .authenticator
                        .on_sae_frame_rx(
                            &mut tap.control.updates,
                            fidl_mlme::SaeFrame {
                                peer_sta_address: bssid.to_array(),
                                status_code: frame
                                    .auth_hdr
                                    .status_code
                                    .into_fidl_or_refused_unspecified(),
                                seq_num: frame.auth_hdr.auth_txn_seq_num,
                                sae_fields: frame.elements.to_vec(),
                            },
                        )
                        .context("failed to process SAE frame with authenticator")?;
                    tap.call(&AuthenticationEvent {
                        phy,
                        bssid,
                        channel,
                        is_ready_for_sae: true,
                        is_ready_for_eapol: false,
                    })
                }
                auth_alg_num => {
                    bail!("unexpected authentication algorithm: {:?}", auth_alg_num);
                }
            }
        }));
    let associate = event::extract(Stateful(|tap: &mut Tap<'_, H>, _: Buffered<AssocReqFrame>| {
        crate::send_association_response(channel, bssid, fidl_ieee80211::StatusCode::Success, phy)
            .context("failed to send association response frame")?;
        match tap.control.authenticator.auth_cfg {
            auth::Config::ComputedPsk(_) => tap.control.authenticator.reset(),
            auth::Config::DriverSae { .. } => {
                bail!("hardware simulator does not support driver SAE");
            }
            auth::Config::Sae { .. } => {}
            auth::Config::Owe { .. } => {}
        }
        tap.control
            .authenticator
            .initiate(&mut tap.control.updates)
            .context("failed to initiate authenticator")?;
        tap.call(&AuthenticationEvent {
            phy,
            bssid,
            channel,
            is_ready_for_sae: true,
            is_ready_for_eapol: true,
        })
    }));
    let eapol = event::extract(Stateful(|tap: &mut Tap<'_, H>, frame: Buffered<DataFrame>| {
        // EAPOL frames are transmitted as LLC data frames.
        for mac::Msdu { llc_frame, .. } in frame.get() {
            assert_eq!(llc_frame.hdr.protocol_id.get(), mac::ETHER_TYPE_EAPOL);
            let mic_size = tap.control.authenticator.get_negotiated_protection().mic_size;
            let key_frame_rx = eapol::KeyFrameRx::parse(mic_size as usize, llc_frame.body)
                .context("failed to parse EAPOL frame")?;
            tap.control
                .authenticator
                .on_eapol_frame(&mut tap.control.updates, eapol::Frame::Key(key_frame_rx))
                .context("failed to process EAPOL frame with authenticator")?;
            tap.call(&AuthenticationEvent {
                phy,
                bssid,
                channel,
                is_ready_for_sae: true,
                is_ready_for_eapol: true,
            })?;
        }
        Ok(())
    }));

    event::with_state(
        tap,
        branch::or((
            authenticate,
            associate,
            send_probe_response(phy, ssid, bssid, channel, protection),
            eapol,
        )),
    )
}
