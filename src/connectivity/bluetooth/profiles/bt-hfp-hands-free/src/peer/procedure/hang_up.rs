// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use at_commands as at;

use super::{CommandFromHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

/// HFP v1.8 §4.15.1
///
/// This procedure only handles sending the AT Commands to start hangup.  This will be followed by
/// an unsolicited +CIEV.
#[derive(Debug, PartialEq)]
pub enum HangUpProcedure {
    Started,
    WaitingForOk,
    Terminated,
}

impl HangUpProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for HangUpProcedure {
    fn name(&self) -> &str {
        "Hang Up Procedure"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        let output;
        match (&self, input) {
            (Self::Started, ProcedureInput::CommandFromHf(CommandFromHf::HangUpCall)) => {
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(Chup {})];
            }
            (Self::WaitingForOk, at_ok!()) => {
                *self = Self::Terminated;
                output = vec![];
            }
            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during a hang up procedure in state {:?}.",
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
