// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Procedure, ProcedureError, ProcedureMarker, ProcedureRequest};

use crate::peer::service_level_connection::SlcState;
use crate::peer::slc_request::SlcRequest;
use crate::peer::update::AgUpdate;

use at_commands as at;
use log::info;

/// The maximum number of characters of a long alphanumeric name.
/// Defined in HFP v1.9, Section 4.8.
pub const MAX_LONG_ALPHANUMERIC_NAME_SIZE: usize = 16;

/// Strips enclosing double quotes from `s`, if present.
fn strip_quotes(s: &str) -> &str {
    s.trim_matches('"')
}

/// Formats the provided `name` to conform to the current network operator
/// format. Returns `None` if the provided operator name is invalid.
pub fn format_operator_name(
    format: at::NetworkOperatorNameFormat,
    name: impl AsRef<str>,
) -> Option<String> {
    let name_str = name.as_ref();
    if !is_valid_operator_name(name_str) {
        return None;
    }
    let mut raw_name = strip_quotes(name_str).to_string();

    match format {
        at::NetworkOperatorNameFormat::LongAlphanumeric => {
            let max_raw_len = MAX_LONG_ALPHANUMERIC_NAME_SIZE.saturating_sub(2);
            if raw_name.chars().count() > max_raw_len {
                let full_name = raw_name.clone();
                raw_name = raw_name.chars().take(max_raw_len).collect();
                info!("Truncating network operator name \"{}\" to \"{}\"", full_name, raw_name);
            }
        }
    }
    Some(format!("\"{}\"", raw_name))
}

/// Returns true if the network operator name is valid.
/// An operator name is valid if it does not contain control characters or
/// internal quotes. Note that double quotes enclosing the string (at the start
/// and end) are allowed.
pub fn is_valid_operator_name(name: &str) -> bool {
    if name.chars().any(|c| c.is_ascii_control()) {
        return false;
    }
    !strip_quotes(name).contains('"')
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// Initial state of the procedure.
    Start,
    /// A request has been received from the HF to set the network operator format.
    SetFormatRequest,
    /// A request has been received from the HF to send the current Network Name.
    GetName,
    /// The AG has responded to the HF's request with the current Network Name and the procedure
    /// is complete.
    Terminated,
}

impl State {
    fn is_start(&self) -> bool {
        if let Self::Start = self { true } else { false }
    }
    /// Transition to the next state in the QOS procedure.
    /// If `skip_set_format` is set, the transition will skip the `SetFormat` state.
    fn transition(&mut self, skip_set_format: bool) {
        match *self {
            Self::Start if skip_set_format => *self = Self::GetName,
            Self::Start => *self = Self::SetFormatRequest,
            Self::SetFormatRequest => *self = Self::GetName,
            Self::GetName => *self = Self::Terminated,
            Self::Terminated => *self = Self::Terminated,
        }
    }
}

/// Represents the Query Operator Selection procedure as defined in HFP v1.8 Section 4.8.
///
/// The HF may request the name of the currently selected Network Operator in the AG
/// via this procedure.
///
/// This procedure is implemented from the perspective of the AG. Namely, outgoing `requests`
/// typically request information about the current state of the AG, to be sent to the remote
/// peer acting as the HF.
pub struct QueryOperatorProcedure {
    state: State,
}

impl QueryOperatorProcedure {
    pub fn new() -> Self {
        Self { state: State::Start }
    }
}

impl Procedure for QueryOperatorProcedure {
    fn marker(&self) -> ProcedureMarker {
        ProcedureMarker::QueryOperatorSelection
    }

    fn hf_update(&mut self, update: at::Command, state: &mut SlcState) -> ProcedureRequest {
        // This format is required to be used by the spec, so it's the default if none is set.
        let format = state
            .ag_network_operator_name_format
            .unwrap_or(at::NetworkOperatorNameFormat::LongAlphanumeric);
        match (self.state, update) {
            (State::Start, at::Command::Cops { three: _, format }) => {
                // The remote peer has requested to set the network name format.
                state.ag_network_operator_name_format = Some(format);
                self.state.transition(/* skip_set_format= */ false);
                AgUpdate::Ok.into()
            }
            (State::Start, at::Command::CopsRead {})
            | (State::SetFormatRequest, at::Command::CopsRead {}) => {
                self.state.transition(/* skip_set_format= */ self.state.is_start());
                let response = Box::new(move |network_name: Option<String>| {
                    let name = network_name
                        .and_then(|n| format_operator_name(format, n))
                        .unwrap_or_else(String::new);
                    AgUpdate::NetworkOperatorName(format, name)
                });
                SlcRequest::GetNetworkOperatorName { response }.into()
            }
            (State::Terminated, at::Command::Cops { .. })
            | (State::Terminated, at::Command::CopsRead {}) => {
                ProcedureError::AlreadyTerminated.into()
            }
            (_, update) => ProcedureError::UnexpectedHf(update).into(),
        }
    }

    fn ag_update(&mut self, update: AgUpdate, _state: &mut SlcState) -> ProcedureRequest {
        match (self.state, update) {
            (State::GetName, update @ AgUpdate::NetworkOperatorName(..)) => {
                self.state.transition(/* skip_set_format= */ false);
                update.into()
            }
            (State::Terminated, AgUpdate::NetworkOperatorName(..)) => {
                ProcedureError::AlreadyTerminated.into()
            }
            (_, update) => ProcedureError::UnexpectedAg(update).into(),
        }
    }

    /// Returns true if the Procedure is finished.
    fn is_terminated(&self) -> bool {
        self.state == State::Terminated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn state_transitions() {
        let mut state = State::Start;
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::SetFormatRequest);
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::GetName);
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::Terminated);
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::Terminated);
    }

    #[test]
    fn state_transition_when_skipping_set_format() {
        let mut state = State::Start;
        state.transition(/* skip_set_format= */ true);
        assert_eq!(state, State::GetName);
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::Terminated);
        state.transition(/* skip_set_format= */ false);
        assert_eq!(state, State::Terminated);
    }

    #[test]
    fn correct_marker() {
        let marker = QueryOperatorProcedure::new().marker();
        assert_eq!(marker, ProcedureMarker::QueryOperatorSelection);
    }

    #[test]
    fn is_terminated_in_terminated_state() {
        let mut proc = QueryOperatorProcedure::new();
        assert!(!proc.is_terminated());
        proc.state = State::SetFormatRequest;
        assert!(!proc.is_terminated());
        proc.state = State::GetName;
        assert!(!proc.is_terminated());
        proc.state = State::Terminated;
        assert!(proc.is_terminated());
    }

    #[test]
    fn unexpected_hf_update_returns_error() {
        let mut procedure = QueryOperatorProcedure::new();
        let mut state = SlcState::default();
        // SLCI AT command.
        let random_hf = at::Command::CindRead {};
        assert_matches!(
            procedure.hf_update(random_hf, &mut state),
            ProcedureRequest::Error(err) if matches!(*err, ProcedureError::UnexpectedHf(_))
        );
    }

    #[test]
    fn unexpected_ag_update_returns_error() {
        let mut procedure = QueryOperatorProcedure::new();
        let mut state = SlcState::default();
        // SLCI AT command.
        let random_ag = AgUpdate::ThreeWaySupport;
        assert_matches!(
            procedure.ag_update(random_ag, &mut state),
            ProcedureRequest::Error(err) if matches!(*err, ProcedureError::UnexpectedAg(_))
        );
    }

    #[test]
    fn updates_produce_expected_requests() {
        let mut p = QueryOperatorProcedure::new();
        let test_operator_name = Some("Foobar".to_string());
        let mut state = SlcState::default();

        // The HF request to set the format should update the shared state.
        let expected_format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format: expected_format };
        assert_matches!(p.hf_update(update1, &mut state), ProcedureRequest::SendMessages(_));
        assert_eq!(state.ag_network_operator_name_format, Some(expected_format));

        let update2 = at::Command::CopsRead {};
        let update3 = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => {
                response(test_operator_name)
            }
            x => {
                panic!("Expected get network operator request but got: {:?}", x);
            }
        };

        assert_matches!(p.ag_update(update3, &mut state), ProcedureRequest::SendMessages(_));

        // Check that the procedure is terminated and any new messages produce an error.
        assert!(p.is_terminated());
        assert_matches!(
            p.hf_update(
                at::Command::Cops {
                    three: 3,
                    format: at::NetworkOperatorNameFormat::LongAlphanumeric
                },
                &mut state
            ),
            ProcedureRequest::Error(err) if matches!(*err, ProcedureError::AlreadyTerminated)
        );
        assert_matches!(
            p.ag_update(
                AgUpdate::NetworkOperatorName(
                    at::NetworkOperatorNameFormat::LongAlphanumeric,
                    "foo".into()
                ),
                &mut state
            ),
            ProcedureRequest::Error(err) if matches!(*err, ProcedureError::AlreadyTerminated)
        );
    }

    #[test]
    fn updates_when_skipping_set_format_produce_expected_requests() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();
        let test_operator_name = Some("Bar".to_string());

        let update1 = at::Command::CopsRead {};
        let update2 = match p.hf_update(update1, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => {
                response(test_operator_name)
            }
            x => panic!("Expected get network operator request but got: {:?}", x),
        };
        assert_matches!(p.ag_update(update2, &mut state), ProcedureRequest::SendMessages(_));
        assert!(p.is_terminated());
    }

    #[test]
    fn update_with_empty_name_produces_expected_requests() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();
        let test_operator_name = None;

        let update1 = at::Command::CopsRead {};
        let update2 = match p.hf_update(update1, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => {
                response(test_operator_name)
            }
            x => panic!("Expected get network operator request but got: {:?}", x),
        };
        assert_matches!(p.ag_update(update2, &mut state), ProcedureRequest::SendMessages(_));
        assert!(p.is_terminated());
    }

    #[test]
    fn valid_operator_name() {
        assert!(is_valid_operator_name("T-Mobile"));
        assert!(is_valid_operator_name(""));
        assert!(!is_valid_operator_name("T-Mobile\r\n"));
        assert!(!is_valid_operator_name("My\"Operator"));
        assert!(is_valid_operator_name("T-Mobile, Inc."));
    }

    #[test]
    fn operator_name_formatting_quoted() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        let result = response_closure(Some("T-Mobile".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"T-Mobile\""
        );
    }

    #[test]
    fn operator_name_formatting_long_truncated() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        let result = response_closure(Some("Fuchsia Telecom Network".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"Fuchsia Teleco\""
        );
    }

    #[test]
    fn operator_name_formatting_empty_quoted() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        let result = response_closure(Some("".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"\""
        );
    }

    #[test]
    fn operator_name_formatting_none_empty() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        let result = response_closure(None);
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == ""
        );
    }

    #[test]
    fn operator_name_formatting_control_characters_filtered() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        // Network operator names containing control characters should be
        // filtered to an empty string.
        let result = response_closure(Some("Foo-Mobile\r\n+CIEV: 1,1\r\n".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == ""
        );
    }

    #[test]
    fn operator_name_formatting_quotes_filtered() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        // Network operator names containing internal double quotes should be
        // filtered to an empty string.
        let result = response_closure(Some("My\"Operator".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == ""
        );
    }

    #[test]
    fn operator_name_formatting_enclosing_quotes_allowed() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        // Network operator names enclosed in double quotes are allowed and
        // formatted cleanly.
        let result = response_closure(Some("\"Foo-Mobile\"".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"Foo-Mobile\""
        );
    }

    #[test]
    fn operator_name_formatting_multiple_enclosing_quotes_allowed() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        // Network operator names with multiple sets of enclosing double quotes
        // are allowed and formatted cleanly.
        let result = response_closure(Some("\"\"Foo-Mobile\"\"".to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"Foo-Mobile\""
        );
    }

    #[test]
    fn operator_name_formatting_multibyte_truncate_no_panic() {
        let mut p = QueryOperatorProcedure::new();
        let mut state = SlcState::default();

        let format = at::NetworkOperatorNameFormat::LongAlphanumeric;
        let update1 = at::Command::Cops { three: 3, format };
        let _ = p.hf_update(update1, &mut state);

        let update2 = at::Command::CopsRead {};
        let response_closure = match p.hf_update(update2, &mut state) {
            ProcedureRequest::Request(SlcRequest::GetNetworkOperatorName { response }) => response,
            x => panic!("Expected get network operator request but got: {:?}", x),
        };

        // Multi-byte UTF-8 operator names should be truncated by
        // character count without panicking on byte boundaries, taking
        // into account the two surrounding quotes.
        let long_multibyte_name = "Foo-Mobile🛜🛜🛜🛜🛜🛜🛜🛜🛜🛜";
        assert!(long_multibyte_name.len() > MAX_LONG_ALPHANUMERIC_NAME_SIZE);
        let result = response_closure(Some(long_multibyte_name.to_string()));
        assert_matches!(
            result,
            AgUpdate::NetworkOperatorName(f, name) if f == format && name == "\"Foo-Mobile🛜🛜🛜🛜\""
        );
    }
}
