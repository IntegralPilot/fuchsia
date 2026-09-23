// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use at_commands as at;
use bt_hfp::call::list::Idx as CallIndex;
use bt_hfp::call::{Direction, Number};
use bt_hfp::dtmf::Code as DtmfCode;
use fidl_fuchsia_bluetooth_hfp::CallState;
use std::fmt;
use std::fmt::Debug;

use crate::features::HfFeatures;
use crate::peer::ag_indicators::AgIndicatorIndex;
use crate::peer::at_connection;
use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

#[cfg(test)]
pub mod test;

// Individual procedures
pub mod answer_incoming;
use answer_incoming::AnswerIncomingProcedure;

pub mod audio_connection_setup;
use audio_connection_setup::AudioConnectionSetupProcedure;

pub mod codec_connection_setup;
use codec_connection_setup::CodecConnectionSetupProcedure;

pub mod send_dtmf_code;
use send_dtmf_code::SendDtmfCodeProcedure;

pub mod hang_up;
use hang_up::HangUpProcedure;

pub mod initiate_call;
use initiate_call::InitiateCallProcedure;

pub mod query_calls;
use query_calls::QueryCallsProcedure;

pub mod slc_initialization;
use slc_initialization::SlcInitProcedure;

macro_rules! at_ok {
    () => {
        crate::peer::procedure::ProcedureInput::AtResponseFromAg(
            crate::peer::at_connection::Response::Recognized(at_commands::Response::Ok),
        )
    };
}
pub(crate) use at_ok;

macro_rules! at_resp {
    ($variant: ident) => {
        crate::peer::procedure::ProcedureInput::AtResponseFromAg(
            crate::peer::at_connection::Response::Recognized(
                at::Response::Success(
                    at::Success::$variant { .. },
        )))
    };
    ($variant: ident $args: tt) => {
        crate::peer::procedure::ProcedureInput::AtResponseFromAg(
            crate::peer::at_connection::Response::Recognized(
                at::Response::Success(
                    at_commands::Success::$variant $args
        )))
    };
}
pub(crate) use at_resp;

macro_rules! at_cmd {
    ($variant: ident $args: tt) => {
        crate::peer::procedure::ProcedureOutput::AtCommandToAg(at::Command::$variant $args)
    };
}
pub(crate) use at_cmd;

#[macro_export]
macro_rules! impl_from_to_variant {
    ($source: path, $destination: ident, $destination_variant: ident) => {
        impl From<$source> for $destination {
            fn from(source: $source) -> $destination {
                $destination::$destination_variant(source)
            }
        }
    };
}

/// Commands that a procedure might take as input which drive its state machine;
/// these are inputs from this component or its FIDL clients.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandFromHf {
    StartSlci { hf_features: HfFeatures },
    CallActionDialFromNumber { number: String },
    CallActionDialFromMemory { memory: String },
    CallActionRedialLast,
    AnswerIncoming,
    HangUpCall,
    QueryCalls,
    StartAudioConnection,
    SendDtmfCode { code: DtmfCode },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcedureInput {
    AtResponseFromAg(at_connection::Response),
    CommandFromHf(CommandFromHf),
}

impl_from_to_variant!(at_connection::Response, ProcedureInput, AtResponseFromAg);
impl_from_to_variant!(CommandFromHf, ProcedureInput, CommandFromHf);

/// Commands that a procedure might emit to specify that the HF, i.e., this
/// component or its FIDL clients, should perform the specified action.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandToHf {
    SetInitialAgIndicatorValues {
        ordered_values: Vec<i64>,
    },
    SetAgIndicatorIndex {
        indicator: AgIndicatorIndex,
        index: i64,
    },
    AwaitRemoteSco,
    QueryCallsResponse {
        index: CallIndex,
        direction: Direction,
        state: CallState,
        multiparty: bool,
        number: Option<Number>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProcedureOutput {
    AtCommandToAg(at::Command),
    CommandToHf(CommandToHf),
}

impl_from_to_variant!(at::Command, ProcedureOutput, AtCommandToAg);
impl_from_to_variant!(CommandToHf, ProcedureOutput, CommandToHf);

pub trait ProcedureInputT<O: ProcedureOutputT>: Clone + Debug + PartialEq + Unpin {
    fn to_initialized_procedure(&self) -> Option<Box<dyn Procedure<Self, O>>>;

    fn can_start_procedure(&self) -> bool;
}

impl ProcedureInputT<ProcedureOutput> for ProcedureInput {
    /// Matches a specific input to procedure
    fn to_initialized_procedure(&self) -> Option<Box<dyn Procedure<Self, ProcedureOutput>>> {
        match self {
            ProcedureInput::CommandFromHf(CommandFromHf::StartSlci { .. }) => {
                Some(Box::new(SlcInitProcedure::new()))
            }

            ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromNumber { .. })
            | ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromMemory { .. })
            | ProcedureInput::CommandFromHf(CommandFromHf::CallActionRedialLast) => {
                Some(Box::new(InitiateCallProcedure::new()))
            }

            ProcedureInput::CommandFromHf(CommandFromHf::HangUpCall) => {
                Some(Box::new(HangUpProcedure::new()))
            }

            ProcedureInput::CommandFromHf(CommandFromHf::AnswerIncoming) => {
                Some(Box::new(AnswerIncomingProcedure::new()))
            }

            ProcedureInput::CommandFromHf(CommandFromHf::QueryCalls) => {
                Some(Box::new(QueryCallsProcedure::new()))
            }

            ProcedureInput::CommandFromHf(CommandFromHf::StartAudioConnection) => {
                Some(Box::new(AudioConnectionSetupProcedure::new()))
            }

            at_resp!(Bcs) => Some(Box::new(CodecConnectionSetupProcedure::new())),

            ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { .. }) => {
                Some(Box::new(SendDtmfCodeProcedure::new()))
            }

            _ => None,
        }
    }

    fn can_start_procedure(&self) -> bool {
        match self {
            ProcedureInput::CommandFromHf(CommandFromHf::StartSlci { .. })
            | ProcedureInput::CommandFromHf(CommandFromHf::StartAudioConnection)
            | at_resp!(Bcs)
            | ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromNumber { .. })
            | ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromMemory { .. })
            | ProcedureInput::CommandFromHf(CommandFromHf::CallActionRedialLast)
            | ProcedureInput::CommandFromHf(CommandFromHf::AnswerIncoming)
            | ProcedureInput::CommandFromHf(CommandFromHf::HangUpCall)
            | ProcedureInput::CommandFromHf(CommandFromHf::QueryCalls)
            | ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { .. }) => true,
            _ => false,
        }
    }
}

pub trait ProcedureOutputT: Clone + Debug + PartialEq + Unpin {}
impl ProcedureOutputT for ProcedureOutput {}

pub trait Procedure<I: ProcedureInputT<O>, O: ProcedureOutputT>: fmt::Debug {
    /// Returns the name of this procedure for logging.
    fn name(&self) -> &str;

    /// Receive a ProcedureInput to progress the procedure. Returns an error in updating
    /// the procedure or a ProcedureOutput.
    fn transition(
        &mut self,
        state: &mut ProcedureManipulatedState,
        input: I,
    ) -> Result<Vec<O>, Error>;

    /// Returns true if the Procedure is finished.
    fn is_terminated(&self) -> bool;
}
