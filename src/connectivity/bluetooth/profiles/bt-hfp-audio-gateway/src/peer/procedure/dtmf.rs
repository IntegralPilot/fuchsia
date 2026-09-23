// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Procedure, ProcedureError, ProcedureMarker, ProcedureRequest};

use crate::peer::service_level_connection::SlcState;
use crate::peer::slc_request::SlcRequest;
use crate::peer::update::AgUpdate;

use at_commands as at;

/// Represents the current state of the HF request to transmit a DTMF Code as defined in HFP v1.8,
/// Section 4.28.
#[derive(Debug, PartialEq, Clone, Copy)]
enum State {
    /// Initial state of the Procedure.
    Start,
    /// A request has been received from the HF to transmit a DTMF Code via the Audio Gateway.
    SendRequest,
    /// Terminal state of the procedure.
    Terminated,
}

impl State {
    /// Transition to the next state in the Dtmf procedure.
    fn transition(&mut self) {
        match *self {
            Self::Start => *self = Self::SendRequest,
            Self::SendRequest => *self = Self::Terminated,
            Self::Terminated => *self = Self::Terminated,
        }
    }
}

/// The HF may transmit DTMF Codes via this procedure. See HFP v1.8, Section 4.28.
///
/// This procedure is implemented from the perspective of the AG. Namely, outgoing `requests`
/// typically request information about the current state of the AG, to be sent to the remote
/// peer acting as the HF.
#[derive(Debug)]
pub struct DtmfProcedure {
    /// The current state of the procedure
    state: State,
}

impl Default for DtmfProcedure {
    fn default() -> Self {
        Self { state: State::Start }
    }
}

impl DtmfProcedure {
    /// Create a new Dtmf procedure in the Start state.
    pub fn new() -> Self {
        Self { state: State::Start }
    }
}

impl Procedure for DtmfProcedure {
    fn marker(&self) -> ProcedureMarker {
        ProcedureMarker::Dtmf
    }

    fn hf_update(&mut self, update: at::Command, _state: &mut SlcState) -> ProcedureRequest {
        match (self.state, &update) {
            (State::Start, at::Command::Vts { code }) => {
                self.state.transition();
                match code.as_str().try_into() {
                    Ok(code) => {
                        let response = Box::new(Into::into);
                        SlcRequest::SendDtmf { code, response }.into()
                    }
                    Err(()) => ProcedureError::InvalidHfArgument(update).into(),
                }
            }
            _ => ProcedureError::UnexpectedHf(update).into(),
        }
    }

    fn ag_update(&mut self, update: AgUpdate, _state: &mut SlcState) -> ProcedureRequest {
        match (self.state, update) {
            (State::SendRequest, update @ (AgUpdate::Ok | AgUpdate::Error)) => {
                self.state.transition();
                update.into()
            }
            (_, update) => ProcedureError::UnexpectedAg(update).into(),
        }
    }

    fn is_terminated(&self) -> bool {
        self.state == State::Terminated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use bt_hfp::dtmf::Code as DtmfCode;

    #[test]
    fn correct_marker() {
        let marker = DtmfProcedure::new().marker();
        assert_eq!(marker, ProcedureMarker::Dtmf);
    }

    #[test]
    fn procedure_handles_invalid_messages() {
        let mut proc = DtmfProcedure::new();
        let req = proc.hf_update(at::Command::CindRead {}, &mut SlcState::default());
        assert_matches!(req, ProcedureRequest::Error(err) if matches!(*err, ProcedureError::UnexpectedHf(_)));

        let req = proc.ag_update(AgUpdate::ThreeWaySupport, &mut SlcState::default());
        assert_matches!(req, ProcedureRequest::Error(err) if matches!(*err, ProcedureError::UnexpectedAg(_)));
    }

    #[test]
    fn procedure_with_invalid_dtmf_code() {
        let mut proc = DtmfProcedure::new();
        let req = proc.hf_update(at::Command::Vts { code: "foo".into() }, &mut SlcState::default());
        assert_matches!(req, ProcedureRequest::Error(err) if matches!(*err, ProcedureError::InvalidHfArgument(_)));
    }

    #[test]
    fn procedure_with_ok_response() {
        let mut proc = DtmfProcedure::new();
        let req = proc.hf_update(at::Command::Vts { code: "1".into() }, &mut SlcState::default());
        let update = match req {
            ProcedureRequest::Request(SlcRequest::SendDtmf { code: DtmfCode::One, response }) => {
                response(Ok(()))
            }
            x => panic!("Unexpected message: {:?}", x),
        };
        let req = proc.ag_update(update, &mut SlcState::default());
        assert_matches!(
            req,
            ProcedureRequest::SendMessages(msgs) if msgs == vec![at::Response::Ok]
        );
        assert!(proc.is_terminated());
    }

    #[test]
    fn procedure_with_err_response() {
        let mut proc = DtmfProcedure::new();
        let req = proc.hf_update(at::Command::Vts { code: "1".into() }, &mut SlcState::default());
        let update = match req {
            ProcedureRequest::Request(SlcRequest::SendDtmf { code: DtmfCode::One, response }) => {
                response(Err(()))
            }
            x => panic!("Unexpected message: {:?}", x),
        };
        let req = proc.ag_update(update, &mut SlcState::default());
        assert_matches!(
            req,
            ProcedureRequest::SendMessages(msgs) if msgs == vec![at::Response::Error]
        );
        assert!(proc.is_terminated());
    }
}
