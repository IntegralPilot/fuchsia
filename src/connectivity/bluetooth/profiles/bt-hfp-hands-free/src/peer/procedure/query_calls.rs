// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use at_commands as at;
use bt_hfp::call::list::Idx as CallIndex;
use bt_hfp::call::{Direction, Number};
use fidl_fuchsia_bluetooth_hfp::CallState;

use super::{
    CommandFromHf, CommandToHf, Procedure, ProcedureInput, ProcedureOutput, at_cmd, at_ok,
};

use crate::peer::procedure::at_resp;
use crate::peer::procedure_manipulated_state::ProcedureManipulatedState;
#[derive(Debug, PartialEq)]
pub enum QueryCallsProcedure {
    Started,
    WaitingForClccOrOk,
    Terminated,
}

/// HFP v1.8 §4.32.1
///
/// This procedure handles sending the AT Commands to query calls.
impl QueryCallsProcedure {
    pub fn new() -> Self {
        Self::Started
    }
}

impl Procedure<ProcedureInput, ProcedureOutput> for QueryCallsProcedure {
    fn name(&self) -> &str {
        "Query List of Current Calls"
    }

    fn transition(
        &mut self,
        _state: &mut ProcedureManipulatedState,
        input: ProcedureInput,
    ) -> Result<Vec<ProcedureOutput>> {
        let output;
        match (&self, input) {
            (Self::Started, ProcedureInput::CommandFromHf(CommandFromHf::QueryCalls)) => {
                *self = Self::WaitingForClccOrOk;
                output = vec![at_cmd!(Clcc {})];
            }
            (
                Self::WaitingForClccOrOk,
                at_resp!(Clcc {
                    index,
                    direction,
                    status,
                    mode: _,
                    multiparty,
                    number,
                    ty: _,
                    alpha: _,
                    priority: _
                }),
            ) => {
                let procedure_output =
                    at_response_to_output(index, direction, status, multiparty, number)?;
                output = vec![procedure_output];
            }
            (Self::WaitingForClccOrOk, at_ok!()) => {
                *self = Self::Terminated;
                output = vec![];
            }
            (_, input) => {
                return Err(format_err!(
                    "Received invalid response {:?} during a query call list procedure in state {:?}.",
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

fn at_response_to_output(
    index_int: i64,
    direction_int: i64,
    state_int: i64,
    multiparty_int: i64,
    number_string: Option<String>,
) -> Result<ProcedureOutput> {
    let index = i64_to_index(index_int)?;
    let direction = i64_to_direction(direction_int)?;
    let state = i64_to_call_state(state_int)?;
    let multiparty = i64_to_bool(multiparty_int)?;
    let number = number_string
        .map(|s| Number::from_at_string(&s))
        .transpose()
        .map_err(|e| format_err!("Invalid phone number in CLCC: {}", e))?;

    Ok(ProcedureOutput::CommandToHf(CommandToHf::QueryCallsResponse {
        index,
        direction,
        state,
        multiparty,
        number,
    }))
}

fn i64_to_index(n: i64) -> Result<CallIndex> {
    match n {
        n if n > 0 => Ok(n as usize),
        n => Err(format_err!("Index value expected; got {n}")),
    }
}

fn i64_to_direction(n: i64) -> Result<Direction> {
    match n {
        0 => Ok(Direction::MobileOriginated),
        1 => Ok(Direction::MobileTerminated),
        n => Err(format_err!("Direction value expected; got {n}")),
    }
}

fn i64_to_bool(n: i64) -> Result<bool> {
    match n {
        0 => Ok(false),
        1 => Ok(true),
        n => Err(format_err!("Bool value expected; got {n}")),
    }
}

fn i64_to_call_state(n: i64) -> Result<CallState> {
    match n {
        0 => Ok(CallState::OngoingActive),
        1 => Ok(CallState::OngoingHeld),
        2 => Ok(CallState::OutgoingDialing),
        3 => Ok(CallState::OutgoingAlerting),
        4 => Ok(CallState::IncomingRinging),
        5 => Ok(CallState::IncomingWaiting),
        6 => Ok(CallState::OngoingHeld),
        n => Err(format_err!("Call state value expected; got {n}")),
    }
}
