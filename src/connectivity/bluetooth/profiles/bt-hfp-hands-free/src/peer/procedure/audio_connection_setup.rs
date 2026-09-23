// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]

use anyhow::{Result, format_err};
use at_commands as at;

use super::{CommandFromHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

/// HFP v1.8 §4.11.2
///
/// The first phase of audio connection setup, followed by Codec Connection Setup and SCO
/// connection setup. This phase is only run if the HF is initiating the connection.
#[derive(Debug, PartialEq)]
pub enum AudioConnectionSetupProcedure {
    Started,
    WaitingForOk,
    Terminated,
}

impl AudioConnectionSetupProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for AudioConnectionSetupProcedure {
    fn name(&self) -> &str {
        "Audio Connection Setup Procedure"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        let output;
        match (&self, input) {
            (Self::Started, ProcedureInput::CommandFromHf(CommandFromHf::StartAudioConnection)) => {
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(Bcc {})];
            }
            (Self::WaitingForOk, at_ok!()) => {
                *self = Self::Terminated;
                output = vec![]
            }

            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during an audio connection setup procedure in state {:?}.",
                    input,
                    self
                ));
            }
        }

        Ok(output)
    }

    fn is_terminated(&self) -> bool {
        *self == Self::Terminated
    }
}
