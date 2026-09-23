// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use at_commands as at;

use super::{CommandFromHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

/// HFP v1.8 §§ 4.18, 4.19, 4.20
///
/// This procedure only handles sending the AT Commands to start call setup.  The rest of these
/// procedures come in as unsolicited +CIEVs and SCO setup, which are handled separately.
#[derive(Debug, PartialEq)]
pub enum InitiateCallProcedure {
    Started,
    WaitingForOk,
    Terminated,
}

impl InitiateCallProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for InitiateCallProcedure {
    fn name(&self) -> &str {
        "Initiate Call Procedure"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        let output;
        match (&self, input) {
            (
                Self::Started,
                ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromNumber { number }),
            ) => {
                // Validate that the number string is a valid AT string.
                let _ = bt_hfp::call::Number::from_non_at_string(&number)?;
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(AtdNumber { number })];
            }
            (
                Self::Started,
                ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromMemory { memory }),
            ) => {
                // Validate that the memory string is a valid AT string.
                let _ = bt_hfp::call::Number::from_non_at_string(&memory)?;
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(AtdMemory { location: memory })];
            }
            (Self::Started, ProcedureInput::CommandFromHf(CommandFromHf::CallActionRedialLast)) => {
                *self = Self::WaitingForOk;
                output = vec![at_cmd!(Bldn {})];
            }

            (Self::WaitingForOk, at_ok!()) => {
                *self = Self::Terminated;
                output = vec![]
            }

            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during an initiate call procedure in state {:?}.",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HandsFreeFeatureSupport;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn successful_dial_from_number_procedure() {
        let mut procedure = InitiateCallProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let number = String::from("+18005550199");
        let input = ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromNumber {
            number: number.clone(),
        });

        assert!(!procedure.is_terminated());
        assert_eq!(procedure, InitiateCallProcedure::Started);

        let outputs = procedure.transition(&mut state, input).expect("successful transition");
        assert_eq!(outputs[0], at_cmd!(AtdNumber { number }));
        assert_eq!(procedure, InitiateCallProcedure::WaitingForOk);

        let outputs = procedure.transition(&mut state, at_ok!()).expect("successful transition");
        assert!(outputs.is_empty());
        assert!(procedure.is_terminated());
    }

    #[fuchsia::test]
    fn successful_dial_from_memory_procedure() {
        let mut procedure = InitiateCallProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let memory = String::from("1");
        let input = ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromMemory {
            memory: memory.clone(),
        });

        assert!(!procedure.is_terminated());
        assert_eq!(procedure, InitiateCallProcedure::Started);

        let outputs = procedure.transition(&mut state, input).expect("successful transition");
        assert_eq!(outputs[0], at_cmd!(AtdMemory { location: memory }));
        assert_eq!(procedure, InitiateCallProcedure::WaitingForOk);

        let outputs = procedure.transition(&mut state, at_ok!()).expect("successful transition");
        assert!(outputs.is_empty());
        assert!(procedure.is_terminated());
    }

    #[fuchsia::test]
    fn error_on_control_characters_in_number() {
        let mut procedure = InitiateCallProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let input = ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromNumber {
            number: String::from("123456;\rAT+CMGD=1"),
        });

        let result = procedure.transition(&mut state, input);
        assert_matches!(result, Err(_));
        assert_eq!(procedure, InitiateCallProcedure::Started);
    }

    #[fuchsia::test]
    fn error_on_control_characters_in_memory() {
        let mut procedure = InitiateCallProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let input = ProcedureInput::CommandFromHf(CommandFromHf::CallActionDialFromMemory {
            memory: String::from("1\rAT+CHUP"),
        });

        let result = procedure.transition(&mut state, input);
        assert_matches!(result, Err(_));
        assert_eq!(procedure, InitiateCallProcedure::Started);
    }
}
