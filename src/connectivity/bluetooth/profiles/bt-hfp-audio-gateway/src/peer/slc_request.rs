// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;

use crate::features::AgFeatures;
use crate::peer::calls::{Call, CallAction};
use crate::peer::gain_control::Gain;
use crate::peer::indicators::{AgIndicators, HfIndicator};
use crate::peer::procedure::ProcedureMarker;
use crate::peer::procedure::hold::CallHoldAction;
use crate::peer::update::AgUpdate;
use bt_hfp::dtmf::Code as DtmfCode;

/// A request made by the Service Level Connection for more information from the
/// HFP component, or report from the ServiceLevelConnection
pub enum SlcRequest {
    /// Sent when the SLC Initialization Procedure has completed
    Initialized,

    GetAgFeatures {
        response: Box<dyn FnOnce(AgFeatures) -> AgUpdate>,
    },

    GetAgIndicatorStatus {
        response: Box<dyn FnOnce(AgIndicators) -> AgUpdate>,
    },

    GetNetworkOperatorName {
        response: Box<dyn FnOnce(Option<String>) -> AgUpdate>,
    },

    GetSubscriberNumberInformation {
        response: Box<dyn FnOnce(Vec<String>) -> AgUpdate>,
    },

    SetNrec {
        enable: bool,
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    SendHfIndicator {
        indicator: HfIndicator,
        response: Box<dyn FnOnce() -> AgUpdate>,
    },

    SendDtmf {
        code: DtmfCode,
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    SpeakerVolumeSynchronization {
        level: Gain,
        response: Box<dyn FnOnce() -> AgUpdate>,
    },

    MicrophoneVolumeSynchronization {
        level: Gain,
        response: Box<dyn FnOnce() -> AgUpdate>,
    },

    QueryCurrentCalls {
        response: Box<dyn FnOnce(Vec<Call>) -> AgUpdate>,
    },

    Answer {
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    HangUp {
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    InitiateCall {
        call_action: CallAction,
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    Hold {
        command: CallHoldAction,
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    /// Setup the SCO connection, as requested by the CodecConnectionSetup
    SynchronousConnectionSetup {
        response: Box<dyn FnOnce(Result<(), ()>) -> AgUpdate>,
    },

    RestartCodecConnectionSetup {
        response: Box<dyn FnOnce() -> AgUpdate>,
    },
}

impl TryFrom<&SlcRequest> for ProcedureMarker {
    type Error = crate::error::Error;

    fn try_from(src: &SlcRequest) -> Result<ProcedureMarker, Self::Error> {
        use SlcRequest::*;
        Ok(match src {
            GetAgFeatures { .. } | GetAgIndicatorStatus { .. } => Self::SlcInitialization,
            GetNetworkOperatorName { .. } => Self::QueryOperatorSelection,
            GetSubscriberNumberInformation { .. } => Self::SubscriberNumberInformation,
            SetNrec { .. } => Self::Nrec,
            SendDtmf { .. } => Self::Dtmf,
            SendHfIndicator { .. } => Self::TransferHfIndicator,
            SpeakerVolumeSynchronization { .. } | MicrophoneVolumeSynchronization { .. } => {
                Self::VolumeSynchronization
            }
            QueryCurrentCalls { .. } => Self::QueryCurrentCalls,
            Answer { .. } => Self::Answer,
            HangUp { .. } => Self::HangUp,
            Hold { .. } => Self::Hold,
            InitiateCall { .. } => Self::InitiateCall,
            SynchronousConnectionSetup { .. } => Self::CodecConnectionSetup,
            RestartCodecConnectionSetup { .. } => Self::CodecSupport,
            Initialized => return Err(Self::Error::OutOfRange),
        })
    }
}

impl fmt::Debug for SlcRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s;
        let output = match &self {
            Self::Initialized => "Initialized",
            Self::GetAgFeatures { .. } => "GetAgFeatures",
            Self::GetAgIndicatorStatus { .. } => "GetAgIndicatorStatus",
            Self::GetSubscriberNumberInformation { .. } => "GetSubscriberNumberInformation",
            Self::SetNrec { enable: true, .. } => "SetNrec(enabled)",
            Self::SetNrec { enable: false, .. } => "SetNrec(disabled)",
            Self::GetNetworkOperatorName { .. } => "GetNetworkOperatorName",
            Self::QueryCurrentCalls { .. } => "QueryCurrentCalls ",
            // DTMF Code values are not displayed in Debug representation
            Self::SendDtmf { .. } => "SendDtmf",
            Self::SendHfIndicator { indicator, .. } => {
                s = format!("SendHfIndicator({:?})", indicator);
                &s
            }
            Self::SpeakerVolumeSynchronization { level, .. } => {
                s = format!("SpeakerVolumeSynchronization({:?})", level);
                &s
            }
            Self::MicrophoneVolumeSynchronization { level, .. } => {
                s = format!("MicrophoneVolumeSynchronization({:?})", level);
                &s
            }
            Self::Answer { .. } => "Answer",
            Self::HangUp { .. } => "HangUp",
            Self::Hold { .. } => "Hold",
            Self::InitiateCall { call_action, .. } => {
                s = format!("InitiateCall({:?})", call_action);
                &s
            }
            Self::SynchronousConnectionSetup { .. } => "SynchronousConnectionSetup",
            Self::RestartCodecConnectionSetup { .. } => "RestartCodecConnectionSetup",
        }
        .to_string();
        write!(f, "{}", output)
    }
}
