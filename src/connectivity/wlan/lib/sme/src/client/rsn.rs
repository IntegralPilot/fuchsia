// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::EstablishRsnaFailureReason;
use fidl_fuchsia_wlan_mlme::{EapolResultCode, SaeFrame};
use wlan_rsn::rsna::UpdateSink;
use wlan_rsn::{Error, NegotiatedProtection, auth};

#[derive(Debug)]
pub struct Rsna {
    pub negotiated_protection: NegotiatedProtection,
    pub supplicant: Box<dyn Supplicant>,
}

impl PartialEq for Rsna {
    fn eq(&self, other: &Self) -> bool {
        self.negotiated_protection == other.negotiated_protection
    }
}

pub trait Supplicant: std::fmt::Debug + std::marker::Send {
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    /// Starts the Supplicant. A Supplicant must be started after its creation and everytime it
    /// was reset.
    fn start(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error>;
    /// Resets all established Security Associations and invalidates all derived keys in this
    /// ESSSA. The Supplicant must be reset or destroyed when the underlying 802.11 association
    /// terminates. The replay counter is also reset.
    fn reset(&mut self);
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    /// Entry point for all incoming EAPOL frames. Incoming frames can be corrupted, invalid or
    /// of unsupported types; the Supplicant will filter and drop all unexpected frames.
    /// Outbound EAPOL frames, status and key updates will be pushed into the `update_sink`.
    /// The method will return an `Error` if the frame was invalid.
    fn on_eapol_frame(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: eapol::Frame<&[u8]>,
    ) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_eapol_conf(
        &mut self,
        update_sink: &mut UpdateSink,
        result: EapolResultCode,
    ) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_rsna_retransmission_timeout(&mut self, update_sink: &mut UpdateSink)
    -> Result<(), Error>;
    fn on_rsna_response_timeout(&self) -> EstablishRsnaFailureReason;
    fn on_rsna_completion_timeout(&self) -> EstablishRsnaFailureReason;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_pmk_available(
        &mut self,
        update_sink: &mut UpdateSink,
        pmk: &[u8],
        pmkid: &[u8],
    ) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_sae_handshake_ind(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_sae_frame_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: SaeFrame,
    ) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_sae_timeout(&mut self, update_sink: &mut UpdateSink, event_id: u64) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn initiate_owe(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error>;
    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401255153)
    fn on_owe_public_key_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        group: u16,
        public_key: Vec<u8>,
    ) -> Result<(), Error>;
    fn get_auth_cfg(&self) -> &auth::Config;
    fn get_auth_method(&self) -> auth::MethodName;
}

impl Supplicant for wlan_rsn::Supplicant {
    fn start(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        wlan_rsn::Supplicant::start(self, update_sink)
    }

    fn reset(&mut self) {
        wlan_rsn::Supplicant::reset(self)
    }

    fn on_eapol_frame(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: eapol::Frame<&[u8]>,
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_eapol_frame(self, update_sink, frame)
    }

    fn on_eapol_conf(
        &mut self,
        update_sink: &mut UpdateSink,
        result: EapolResultCode,
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_eapol_conf(self, update_sink, result)
    }

    fn on_rsna_retransmission_timeout(
        &mut self,
        update_sink: &mut UpdateSink,
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_rsna_retransmission_timeout(self, update_sink)
    }

    fn on_rsna_response_timeout(&self) -> EstablishRsnaFailureReason {
        EstablishRsnaFailureReason::RsnaResponseTimeout(wlan_rsn::Supplicant::incomplete_reason(
            self,
        ))
    }

    fn on_rsna_completion_timeout(&self) -> EstablishRsnaFailureReason {
        EstablishRsnaFailureReason::RsnaCompletionTimeout(wlan_rsn::Supplicant::incomplete_reason(
            self,
        ))
    }

    fn on_pmk_available(
        &mut self,
        update_sink: &mut UpdateSink,
        pmk: &[u8],
        pmkid: &[u8],
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_pmk_available(self, update_sink, pmk, pmkid)
    }

    fn on_sae_handshake_ind(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_sae_handshake_ind(self, update_sink)
    }

    fn on_sae_frame_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: SaeFrame,
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_sae_frame_rx(self, update_sink, frame)
    }

    fn on_sae_timeout(&mut self, update_sink: &mut UpdateSink, event_id: u64) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_sae_timeout(self, update_sink, event_id)
    }

    fn initiate_owe(&mut self, update_sink: &mut UpdateSink) -> Result<(), Error> {
        wlan_rsn::Supplicant::initiate_owe(self, update_sink)
    }

    fn on_owe_public_key_rx(
        &mut self,
        update_sink: &mut UpdateSink,
        group: u16,
        public_key: Vec<u8>,
    ) -> Result<(), Error> {
        wlan_rsn::Supplicant::on_owe_public_key_rx(self, update_sink, group, public_key)
    }

    fn get_auth_cfg(&self) -> &auth::Config {
        &self.auth_cfg
    }

    fn get_auth_method(&self) -> auth::MethodName {
        self.auth_cfg.method_name()
    }
}
