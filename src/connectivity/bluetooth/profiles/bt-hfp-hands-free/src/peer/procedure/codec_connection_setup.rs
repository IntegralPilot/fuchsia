// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use at_commands as at;
use bt_hfp::codec_id::CodecId;
use log::warn;

use super::{CommandToHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok, at_resp};

use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;

#[derive(Debug, PartialEq)]
enum State {
    WaitingForBcs,
    WaitingForOk,
    Terminated,
}

/// HFP v1.8 §4.11.3
///
/// The second phase of audio connection setup, following Audio Connection Setup and followed by
/// SCO connection setup. This phase  may be skipped if the codec has already been selected.
#[derive(Debug, PartialEq)]
pub struct CodecConnectionSetupProcedure {
    // Whether the procedure has sent the phone status to the HF.
    state: State,
}

impl CodecConnectionSetupProcedure {
    pub fn new() -> Self {
        Self { state: State::WaitingForBcs }
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for CodecConnectionSetupProcedure {
    fn name(&self) -> &str {
        "Codec Connection Setup"
    }

    fn transition(
        &mut self,
        procedure_manipulated_state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>, Error> {
        let outputs = match (&self.state, input) {
            (State::WaitingForBcs, at_resp!(Bcs { codec })) => {
                let codec_id: CodecId = codec.try_into()?;
                if procedure_manipulated_state.supported_codecs.contains(&codec_id) {
                    procedure_manipulated_state.selected_codec = Some(codec_id);
                    self.state = State::WaitingForOk;
                    vec![
                        // This is the earliest point at which we have selected the codec for a SCO
                        // connection, so signal the peer task to await an incoming SCO connection
                        // with that codec.  The earlier we do this, the better, to prevent a race
                        // where we haven't yet started to listen for the SCO connection with the
                        // correct codec when it arrives.
                        ProcedureOutput::CommandToHf(CommandToHf::AwaitRemoteSco),
                        at_cmd!(Bcs { codec: codec }),
                    ]
                } else {
                    // According to HFP v1.8 Section 4.11.3, if the received codec ID is not
                    // available, the HF shall respond with AT+BAC with its available codecs.
                    warn!("Codec received is not supported. Sending supported codecs to AG.");
                    self.state = State::Terminated;
                    let supported_codecs = procedure_manipulated_state
                        .supported_codecs
                        .iter()
                        .map(|&x| x.into())
                        .collect();
                    vec![at_cmd!(Bac { codecs: supported_codecs })]
                }
            }
            (State::WaitingForOk, at_ok!()) => {
                self.state = State::Terminated;
                vec![]
            }
            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during a codec connection setup procedure with state: {:?}",
                    input,
                    self.state
                ));
            }
        };

        Ok(outputs)
    }

    fn is_terminated(&self) -> bool {
        self.state == State::Terminated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    use crate::config::HandsFreeFeatureSupport;

    #[fuchsia::test]
    fn properly_responds_to_supported_codec() {
        let mut procedure = CodecConnectionSetupProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);
        let agreed_codec = CodecId::MSBC;

        let response1 = at_resp!(Bcs { codec: agreed_codec.into() });

        assert!(!procedure.is_terminated());

        assert_matches!(procedure.transition(&mut state, response1), Ok(_));
        assert_eq!(state.selected_codec.expect("Codec agreed upon."), agreed_codec);

        let response2 = at_ok!();
        assert_matches!(procedure.transition(&mut state, response2), Ok(_));

        assert!(procedure.is_terminated())
    }

    #[fuchsia::test]
    fn properly_responds_to_unsupported_codec() {
        let mut procedure = CodecConnectionSetupProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);
        state.supported_codecs = Vec::new();

        let unsupported_codec = CodecId::MSBC;
        let response = at_resp!(Bcs { codec: unsupported_codec.into() });

        assert!(!procedure.is_terminated());

        assert_eq!(State::WaitingForBcs, procedure.state);
        assert_matches!(procedure.transition(&mut state, response), Ok(_));
        assert_eq!(State::Terminated, procedure.state);

        assert_matches!(state.selected_codec, None);

        assert!(procedure.is_terminated())
    }

    #[fuchsia::test]
    fn error_from_invalid_responses() {
        let mut procedure = CodecConnectionSetupProcedure::new();
        let config = HandsFreeFeatureSupport::default();
        let mut state = ProcedureManipulatedState::new(config);

        let response = at_resp!(TestResponse {});

        assert!(!procedure.is_terminated());

        assert_matches!(procedure.transition(&mut state, response), Err(_));

        assert!(!procedure.is_terminated())
    }
}
