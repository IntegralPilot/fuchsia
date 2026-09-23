// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use at_commands as at;

use super::{CommandFromHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

/// HFP v1.8 §4.28
#[derive(Debug, PartialEq)]
pub enum SendDtmfCodeProcedure {
    Started,
    WaitingForOk,
    Terminated,
}

impl SendDtmfCodeProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for SendDtmfCodeProcedure {
    fn name(&self) -> &str {
        "Send DTMF Code Procedure"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        match (&self, input) {
            (
                Self::Started,
                ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { code }),
            ) => {
                *self = Self::WaitingForOk;
                Ok(vec![at_cmd!(Vts { code: code.into() })])
            }
            (Self::WaitingForOk, at_ok!()) => {
                *self = Self::Terminated;
                Ok(vec![])
            }
            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during a send DTMF code procedure in state {:?}.",
                    input,
                    self
                ));
            }
        }
    }

    fn is_terminated(&self) -> bool {
        *self == Self::Terminated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use bt_hfp::dtmf::Code as DtmfCode;

    use crate::config::HandsFreeFeatureSupport;

    #[fuchsia::test]
    fn successful_dtmf_code_procedure() {
        let mut procedure = SendDtmfCodeProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let code = DtmfCode::One;
        let input = ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { code });

        assert!(!procedure.is_terminated());
        assert_eq!(procedure, SendDtmfCodeProcedure::Started);

        let outputs = procedure.transition(&mut state, input).expect("successful transition");
        assert_eq!(outputs[0], at_cmd!(Vts { code: String::from("1") }));
        assert_eq!(procedure, SendDtmfCodeProcedure::WaitingForOk);

        let outputs = procedure.transition(&mut state, at_ok!()).expect("successful transition");
        assert!(outputs.is_empty());
        assert!(procedure.is_terminated());
    }

    #[fuchsia::test]
    fn error_on_invalid_input_in_started_state() {
        let mut procedure = SendDtmfCodeProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        // Unexpected
        let result = procedure.transition(&mut state, at_ok!());
        assert_matches!(result, Err(_));
    }

    #[fuchsia::test]
    fn error_on_invalid_input_in_waiting_for_ok_state() {
        let mut procedure = SendDtmfCodeProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let code = DtmfCode::One;
        let input = ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { code });
        let _ = procedure.transition(&mut state, input).unwrap();

        // Unexpected
        let input = ProcedureInput::CommandFromHf(CommandFromHf::SendDtmfCode { code });
        let result = procedure.transition(&mut state, input);
        assert_matches!(result, Err(_));
    }
}
