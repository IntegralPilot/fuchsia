// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use at_commands as at;

use super::{CommandFromHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

/// HFP v1.8 §4.13
///
/// This procedure only handles sending the AT Commands to answer a call, ATA.  The rest of the
/// process of answering a call is handled by unsolicited responses such as RING, +CIEV and +CLIP.
#[derive(Debug, PartialEq)]
pub enum AnswerIncomingProcedure {
    Started,
    WaitingForOk,
    Terminated,
}

impl AnswerIncomingProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for AnswerIncomingProcedure {
    fn name(&self) -> &str {
        "Answer Incoming Procedure"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        let output;
        match (&self, input) {
            (Self::Started, ProcedureInput::CommandFromHf(CommandFromHf::AnswerIncoming)) => {
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(Answer {})]; // ATA command
            }
            (Self::WaitingForOk, at_ok!()) => {
                *self = Self::Terminated;
                output = vec![];
            }
            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during an answer incoming procedure in state {:?}.",
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
