// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;
use std::ops::AddAssign;

use crate::params::ParamAdapter;
use crate::ta_loader::TAInterface;
use anyhow::Error;
use api_impl::context;
use fidl_fuchsia_tee::{OpResult, Parameter, ReturnOrigin};
use tee_internal::{Result as TeeResult, SessionContext, binding};

// Reserve 0 as the canonically invalid session ID.
//
// TODO(https://fxbug.dev/371215993): Teach the client FIDL library about this value and add tests
// around its return and use.
const INVALID_SESSION_ID: u32 = 0;

// This structure stores the entry points to the TA and application-specific
// state like sessions.
pub struct TrustedApp<T: TAInterface> {
    interface: T,
    sessions: BTreeMap<u32, SessionContext>,
    next_session_id: u32,
}

// This helper handles setting up the context for calling in to a TA entry point and cleaning up
// after the call. This includes translating the FIDL type definitions into their in-memory
// representations and back as well as saving and restoring contextual information.
fn invoke_ta_entry_point<F, R>(
    entry_point: F,
    parameter_set: Vec<Parameter>,
) -> Result<(TeeResult<R>, OpResult), Error>
where
    F: FnOnce(tee_internal::ParamTypes, &mut [tee_internal::Param; 4]) -> TeeResult<R>,
{
    let (tee_result, return_params) = {
        let (mut adapter, param_types) = context::with_current_mut(|context| {
            ParamAdapter::from_fidl(parameter_set, &mut context.mapped_param_ranges)
        })?;
        let tee_result = entry_point(param_types, adapter.tee_params_mut());
        context::with_current_mut(|context| context.cleanup_after_call());
        (tee_result, adapter.export_to_fidl()?)
    };
    let return_code = match tee_result {
        Ok(_) => binding::TEE_SUCCESS,
        Err(error) => error as binding::TEE_Result,
    };
    let op_result = OpResult {
        return_code: Some(return_code as u64),
        return_origin: Some(ReturnOrigin::TrustedApplication),
        parameter_set: Some(return_params),
        ..Default::default()
    };
    Ok((tee_result, op_result))
}

impl<T: TAInterface> TrustedApp<T> {
    pub fn new(interface: T) -> Result<Self, Error> {
        interface.create()?;
        Ok(Self { interface, sessions: BTreeMap::new(), next_session_id: INVALID_SESSION_ID + 1 })
    }

    fn allocate_session_id(&mut self) -> u32 {
        let session_id = self.next_session_id;
        self.next_session_id.add_assign(1);
        session_id
    }

    // TODO(https://fxbug.dev/332956721): We're supposed to consult the property
    // `gpd.ta.multiSession` on the TA before asking it to open a second
    // session.  If this property isn't set, opening a second session is a
    // client error.
    pub fn open_session(
        &mut self,
        parameter_set: Vec<Parameter>,
    ) -> Result<(u32, OpResult), Error> {
        let (tee_result, op_result) = invoke_ta_entry_point(
            |param_types, params| self.interface.open_session(param_types, params),
            parameter_set,
        )?;
        let session_id = match tee_result {
            Ok(session_context) => {
                let session_id = self.allocate_session_id();
                let _ = self.sessions.insert(session_id, session_context);
                session_id
            }
            Err(_) => INVALID_SESSION_ID,
        };
        Ok((session_id, op_result))
    }

    pub fn close_session(&mut self, session_id: u32) -> Result<(), Error> {
        match self.sessions.remove(&session_id) {
            Some(session_context) => {
                self.interface.close_session(session_context);
                Ok(())
            }
            None => anyhow::bail!("Invalid session id"),
        }
    }

    pub fn invoke_command(
        &mut self,
        session_id: u32,
        command: u32,
        parameter_set: Vec<Parameter>,
    ) -> Result<OpResult, Error> {
        let session_context = match self.sessions.get_mut(&session_id) {
            Some(session_context) => session_context,
            None => anyhow::bail!("Invalid session id"),
        };
        let (_, op_result) = invoke_ta_entry_point(
            |param_types, params| {
                self.interface.invoke_command(*session_context, command, param_types, params)
            },
            parameter_set,
        )?;
        Ok(op_result)
    }

    pub fn destroy(&self) {
        self.interface.destroy()
    }
}
