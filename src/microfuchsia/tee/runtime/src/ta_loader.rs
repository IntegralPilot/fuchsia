// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]

use anyhow::Error;
use tee_internal::binding::{TEE_Param, TEE_Result};
use tee_internal::{
    Error as TeeError, Param, ParamTypes, Result as TeeResult, SessionContext,
    param_list_to_binding_mut,
};

pub trait TAInterface {
    fn create(&self) -> TeeResult;
    fn destroy(&self);
    fn open_session(
        &self,
        param_types: ParamTypes,
        params: &mut [Param; 4],
    ) -> TeeResult<SessionContext>;
    fn close_session(&self, session_context: SessionContext);
    fn invoke_command(
        &self,
        session_context: SessionContext,
        command_id: u32,
        param_types: ParamTypes,
        params: &mut [Param; 4],
    ) -> TeeResult;
}

struct TAFunctions {
    create_fn: fn() -> TEE_Result,
    destroy_fn: fn(),
    open_session_fn: fn(
        param_types: u32,
        params: *mut TEE_Param,
        session_context: *mut SessionContext,
    ) -> TEE_Result,
    close_session_fn: fn(session_context: SessionContext),
    invoke_command_fn: fn(
        session_context: SessionContext,
        command_id: u32,
        param_types: u32,
        params: *mut TEE_Param,
    ) -> TEE_Result,
}

impl TAInterface for TAFunctions {
    fn create(&self) -> TeeResult {
        match TeeError::from_tee_result((self.create_fn)()) {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }

    fn destroy(&self) {
        (self.destroy_fn)();
    }

    fn open_session(
        &self,
        param_types: ParamTypes,
        params: &mut [Param; 4],
    ) -> TeeResult<SessionContext> {
        let mut session_context = SessionContext::from_value(0);
        let result = (self.open_session_fn)(
            param_types.as_u32(),
            param_list_to_binding_mut(params).as_mut_ptr(),
            &mut session_context,
        );
        match TeeError::from_tee_result(result) {
            None => Ok(session_context),
            Some(error) => Err(error),
        }
    }

    fn close_session(&self, session_context: SessionContext) {
        (self.close_session_fn)(session_context)
    }

    fn invoke_command(
        &self,
        session_context: SessionContext,
        command_id: u32,
        param_types: ParamTypes,
        params: &mut [Param; 4],
    ) -> TeeResult {
        let result = (self.invoke_command_fn)(
            session_context,
            command_id,
            param_types.as_u32(),
            param_list_to_binding_mut(params).as_mut_ptr(),
        );
        match TeeError::from_tee_result(result) {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }
}

fn load_sym(handle: *mut libc::c_void, name: &std::ffi::CStr) -> Result<*const (), Error> {
    let fun = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if fun.is_null() {
        anyhow::bail!("Could not find symbol {name:?}: {:?}", std::io::Error::last_os_error());
    }
    Ok(fun as *const ())
}

pub fn load_ta(name: &std::ffi::CStr) -> Result<impl TAInterface, Error> {
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        anyhow::bail!("Could not load {name:?}: {:?}", std::io::Error::last_os_error());
    }
    Ok(TAFunctions {
        create_fn: unsafe { std::mem::transmute(load_sym(handle, c"TA_CreateEntryPoint")?) },
        destroy_fn: unsafe { std::mem::transmute(load_sym(handle, c"TA_DestroyEntryPoint")?) },
        open_session_fn: unsafe {
            std::mem::transmute(load_sym(handle, c"TA_OpenSessionEntryPoint")?)
        },
        close_session_fn: unsafe {
            std::mem::transmute(load_sym(handle, c"TA_CloseSessionEntryPoint")?)
        },
        invoke_command_fn: unsafe {
            std::mem::transmute(load_sym(handle, c"TA_InvokeCommandEntryPoint")?)
        },
    })
}

#[cfg(test)]
mod test {
    use super::*;

    // The C string literal syntax c"foo" isn't supported by #[fuchsia::test] so
    // we use a helper function instead to construct &CStrs from literals.
    // TODO(https://fxbug.dev/332964901): Remove this and use C-string literals
    // once supported.
    fn c_str<'a>(s: &'a [u8]) -> &'a std::ffi::CStr {
        std::ffi::CStr::from_bytes_with_nul(s).unwrap()
    }

    #[fuchsia::test]
    fn load_missing_so() {
        let result = load_ta(c_str(b"libta_loader_test_missing.so\0"));
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn load_ta_missing_entry_points() {
        let result = load_ta(c_str(b"libta_loader_test_missing_entry_points.so\0"));
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn load_ta_complete() {
        let result = load_ta(c_str(b"libta_loader_test_complete.so\0"));
        assert!(!result.is_err());
    }
}
